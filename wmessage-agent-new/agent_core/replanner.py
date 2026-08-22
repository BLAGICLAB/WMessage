"""replanner — 触发条件判断 + 重规划。

来自 wmessage-agent-spec.md §X.2.2 阶段 3：

触发条件（任一满足即触发，阈值在 harness_policy.toml 可调）：
1. 当前 task 失败 / 报错（extract error pattern）
2. 单 task 完成 N 步（默认 N=3）
3. 执行发现环境与原计划不符（git status 异常、关键文件缺失）
4. 全局步数超过阈值（防失控）
5. 用户主动 /replan 中断

行为：
- 输入 = (原 TaskGraph, 已完成节点, 最新环境状态, 最近报错)
- 输出 = 新 TaskGraph（节点可新增 / 裁剪 / 标记重试）

Phase 1 实现：
- 触发条件函数（纯函数，可单测）
- replan() 简单版：保留已完成节点、失败节点标记重试、未开始节点保留
- LLM 模式预留（Phase 2 接入）
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional

from agent_core.types import TaskGraph, TaskNode


class ReplanReason(str, Enum):
    """Replan 触发原因。"""

    TASK_FAILED = "task_failed"
    STEP_THRESHOLD = "step_threshold"
    ENV_DRIFT = "env_drift"
    GLOBAL_LIMIT = "global_limit"
    USER_INTERRUPT = "user_interrupt"


@dataclass
class ReplanTrigger:
    """触发条件命中信息。"""

    should_replan: bool
    reason: Optional[ReplanReason] = None
    detail: str = ""


@dataclass
class ReplanPolicy:
    """重规划策略（来自 harness_policy.toml）。"""

    trigger_step_threshold: int = 3  # 单 task 每 3 步触发
    max_replans_per_graph: int = 5  # 单 graph 最多 5 次重规划
    enable_env_drift_check: bool = True  # 是否启用环境漂移检测

    @classmethod
    def from_dict(cls, d: Optional[Dict[str, Any]]) -> "ReplanPolicy":
        if not d:
            return cls()
        return cls(
            trigger_step_threshold=d.get("trigger_step_threshold", 3),
            max_replans_per_graph=d.get("max_replans_per_graph", 5),
            enable_env_drift_check=d.get("enable_env_drift_check", True),
        )


# ───────────────────────── 触发条件 ─────────────────────────


def should_replan_after_step(
    policy: ReplanPolicy,
    task: TaskNode,
    react_steps_completed: int,
    last_error: Optional[str] = None,
) -> ReplanTrigger:
    """单 task 完成一步后判断是否触发 Replan。

    Args:
        policy: 重规划策略
        task: 当前 task
        react_steps_completed: 当前 task 已完成的 ReAct 步数
        last_error: 最近一次错误（None = 无错误）

    Returns:
        ReplanTrigger(should_replan, reason, detail)
    """
    if last_error:
        return ReplanTrigger(
            should_replan=True,
            reason=ReplanReason.TASK_FAILED,
            detail=f"task {task.id} error: {last_error[:200]}",
        )

    if react_steps_completed > 0 and react_steps_completed % policy.trigger_step_threshold == 0:
        return ReplanTrigger(
            should_replan=True,
            reason=ReplanReason.STEP_THRESHOLD,
            detail=f"task {task.id} completed {react_steps_completed} steps (threshold={policy.trigger_step_threshold})",
        )

    return ReplanTrigger(should_replan=False)


def should_replan_global(
    policy: ReplanPolicy,
    replan_count: int,
    global_step_limit: Optional[int] = None,
    current_global_step: int = 0,
) -> ReplanTrigger:
    """全局重规划上限检测。

    Args:
        policy: 重规划策略
        replan_count: 当前已重规划次数
        global_step_limit: 全局步数上限（None = 不限）
        current_global_step: 当前全局步数

    Returns:
        ReplanTrigger
    """
    if replan_count >= policy.max_replans_per_graph:
        return ReplanTrigger(
            should_replan=False,  # 已达上限，不再触发
            reason=ReplanReason.GLOBAL_LIMIT,
            detail=f"replan count {replan_count} reached max {policy.max_replans_per_graph}",
        )

    if global_step_limit and current_global_step >= global_step_limit:
        return ReplanTrigger(
            should_replan=False,
            reason=ReplanReason.GLOBAL_LIMIT,
            detail=f"global step {current_global_step} reached limit {global_step_limit}",
        )

    return ReplanTrigger(should_replan=False)


def detect_env_drift(
    env_state: Dict[str, Any],
    original_env_state: Dict[str, Any],
) -> ReplanTrigger:
    """检测环境漂移：env_state 与原计划对比，关键字段变化即触发。"""
    if not env_state or not original_env_state:
        return ReplanTrigger(should_replan=False)

    # 关键字段：git_status / files_hint / error
    drift_keys = ["git_status", "files_hint", "error"]
    for k in drift_keys:
        if k in env_state and k in original_env_state:
            if env_state[k] != original_env_state[k]:
                return ReplanTrigger(
                    should_replan=True,
                    reason=ReplanReason.ENV_DRIFT,
                    detail=f"{k} changed: {original_env_state[k]} → {env_state[k]}",
                )

    return ReplanTrigger(should_replan=False)


# ───────────────────────── 重规划 ─────────────────────────


@dataclass
class CompletedStep:
    """已完成步骤的简化记录（用于 replan 输入）。"""

    task_id: str
    title: str
    result_summary: str
    task_id_uuid: Optional[str] = None  # 从结果提取的 task UUID


def replan(
    original_graph: TaskGraph,
    completed: List[CompletedStep],
    failed_task: Optional[TaskNode] = None,
    error: Optional[str] = None,
    policy: Optional[ReplanPolicy] = None,
    llm: Optional[Callable[[str], str]] = None,
) -> TaskGraph:
    """生成新 TaskGraph。

    规则（Phase 1 简化版）：
    1. 已完成节点保留 status=done
    2. 失败的 failed_task → status=pending, attempts 增 1（下次执行会重试）
    3. 未开始的节点保留
    4. 已完成节点的依赖若失败 → 标记 skipped（不可达）
    5. plan_version 递增

    Args:
        original_graph: 原 TaskGraph
        completed: 已完成步骤列表
        failed_task: 失败的 task（None = 没有失败，只是常规重规划）
        error: 失败原因
        policy: 重规划策略（暂未使用，预留）
        llm: LLM 客户端（Phase 1 未使用，预留 LLM 智能重规划）

    Returns:
        新 TaskGraph
    """
    policy = policy or ReplanPolicy()
    completed_ids = {c.task_id for c in completed}
    failed_id = failed_task.id if failed_task else None

    new_tasks: List[TaskNode] = []
    for t in original_graph.tasks:
        if t.id in completed_ids:
            # 保留为 done
            new_t = TaskNode(
                id=t.id,
                title=t.title,
                deps=list(t.deps),
                verify_hint=t.verify_hint,
            )
            new_tasks.append(new_t)
        elif t.id == failed_id:
            # 失败 → 重试（attempts 已在 agent_loop 里 +1）
            new_t = TaskNode(
                id=t.id,
                title=t.title,
                deps=list(t.deps),
                verify_hint=t.verify_hint,
            )
            new_tasks.append(new_t)
        else:
            # 未开始 → 检查依赖是否被跳过
            skip = any(
                d in {c.task_id for c in completed if False} or d == failed_id
                for d in t.deps
            )
            if skip and failed_id:
                # 依赖失败 → 跳过（保留节点但标记）— Phase 1 简化：直接保留
                pass
            new_t = TaskNode(
                id=t.id,
                title=t.title,
                deps=list(t.deps),
                verify_hint=t.verify_hint,
            )
            new_tasks.append(new_t)

    new_graph = TaskGraph(
        tasks=new_tasks,
        root_query=original_graph.root_query,
        mode=original_graph.mode,
        plan_version=original_graph.plan_version + 1,
    )

    # Phase 2: llm is not None 时调 LLM 智能重规划
    if llm is not None:
        # TODO(Phase 2): LLM-based replan
        pass

    return new_graph


# ───────────────────────── 错误匹配 ─────────────────────────


_ERROR_PATTERNS = [
    (r"ModuleNotFoundError", "missing_module"),
    (r"PermissionError", "permission_denied"),
    (r"FileNotFoundError", "file_missing"),
    (r"AssertionError", "assert_failed"),
    (r"超时|TimeoutError|timeout", "timeout"),
    (r"CommandError|exit_code.*[1-9]", "command_failed"),
]


def classify_error(error: str) -> str:
    """从错误文本提取错误类别（用于决定 replan 策略）。

    Returns:
        错误类别标识（missing_module / permission_denied / ... / unknown）
    """
    if not error:
        return "unknown"
    for pattern, label in _ERROR_PATTERNS:
        if re.search(pattern, error, re.IGNORECASE):
            return label
    return "unknown"
