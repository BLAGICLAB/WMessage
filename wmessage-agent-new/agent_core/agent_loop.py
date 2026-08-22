"""agent_loop — PREV 主循环编排（Plan → Execute → Replan → Verify）。

来自 wmessage-agent-spec.md §X.2 主循环。

主流程：
1. plan: TaskGraph 已由外部 planner 生成
2. for each task in graph (topological order):
   a. skill.start audit
   b. ReAct sub-loop: react_step → executor.execute → ctx push
   c. verify: verify(task, executor)
   d. pass → ctx.push → next task
      fail → attempts++, check replan trigger, replan if needed
3. skill.finish audit
4. return AgentLoopResult

Phase 1 实现：
- 顺序执行 task（无并行）
- ReAct 单步循环（react_max_steps 限制）
- Verify 失败触发 Replan（attempt 1 次后）
- 状态机：NoActive → Continue → Finish / Fail

Phase 2 扩展：
- 并行 task 调度（如果 deps 满足）
- Reflexion 反思循环（默认关）
- Orchestrator-Worker 多子 Agent
"""
from __future__ import annotations

import time
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Callable, Dict, List, Optional

from agent_core.audit_log import AuditLogger
from agent_core.react_step import (
    ActionKind,
    ReactAction,
    ReactState,
    react_step,
)
from agent_core.replanner import (
    CompletedStep,
    ReplanPolicy,
    ReplanTrigger,
    replan,
    should_replan_after_step,
    should_replan_global,
)
from agent_core.types import TaskGraph, TaskNode, VerifyHint, VerifyResult
from agent_core.var_substitute import extract_task_id
from agent_core.verify import (
    LocalVerifyExecutor,
    VerifyExecutor,
    verify as verify_task,
)


class LoopStatus(str, Enum):
    """主循环状态。"""

    OK = "ok"
    FAILED = "failed"
    AWAIT_USER = "await_user"  # 等用户确认
    REPLAN_EXHAUSTED = "replan_exhausted"  # 重规划次数用尽


@dataclass
class AgentLoopResult:
    """主循环返回。"""

    status: str  # LoopStatus
    ctx: List[Dict[str, Any]] = field(default_factory=list)
    replan_count: int = 0
    completed_tasks: List[str] = field(default_factory=list)
    failed_tasks: List[str] = field(default_factory=list)
    error: Optional[str] = None
    duration_ms: int = 0
    audit_lines: List[str] = field(default_factory=list)


@dataclass
class AgentLoopConfig:
    """主循环配置。"""

    react_max_steps: int = 30
    max_task_attempts: int = 2  # verify 失败最多重试 2 次
    replan_policy: ReplanPolicy = field(default_factory=ReplanPolicy)
    global_step_limit: Optional[int] = None  # 全局步数上限（None = 不限）
    enable_replan: bool = True
    enable_audit: bool = True


class AgentLoop:
    """PREV 主循环。"""

    def __init__(
        self,
        llm,
        executor: VerifyExecutor,
        audit: Optional[AuditLogger] = None,
        config: Optional[AgentLoopConfig] = None,
    ):
        """初始化。

        Args:
            llm: LLM 客户端（react_step 需要）
            executor: VerifyExecutor（verify + tool.execute 都用）
            audit: AuditLogger（None = 不写审计）
            config: AgentLoopConfig
        """
        self.llm = llm
        self.executor = executor
        self.audit = audit or AuditLogger()  # 默认内存模式（不落盘）
        self.config = config or AgentLoopConfig()

    def run(
        self,
        graph: TaskGraph,
        env_state: Optional[Dict[str, Any]] = None,
        tools_whitelist: Optional[List[str]] = None,
        skill_run_id: Optional[str] = None,
        skill_name: str = "agent_loop",
    ) -> AgentLoopResult:
        """执行主循环。

        Args:
            graph: planner 生成的 TaskGraph
            env_state: 环境状态（文件树 / git / 最近报错）
            tools_whitelist: 允许的 tool_name 列表
            skill_run_id: Skill 运行 ID（审计用）
            skill_name: Skill 名（审计用）

        Returns:
            AgentLoopResult
        """
        start = time.time()
        ctx: List[Dict[str, Any]] = []
        completed: List[CompletedStep] = []
        failed_tasks: List[str] = []
        audit_lines: List[str] = []
        replan_count = 0
        current_graph = graph
        env_state = env_state or {}

        skill_run_id = skill_run_id or f"sr-{int(time.time() * 1000)}"
        audit_lines.append(
            self.audit.skill_start(skill_run_id=skill_run_id, skill=skill_name)
        )

        max_outer_iter = self.config.replan_policy.max_replans_per_graph + 1
        for outer_iter in range(max_outer_iter):
            # 重规划后：清空 ctx/completed/failed，用新 graph 重跑
            if outer_iter > 0:
                ctx = []
                completed = []
                failed_tasks = []
                audit_lines.append(
                    self.audit.info(
                        "loop.rerun",
                        iter=outer_iter,
                        graph_version=current_graph.plan_version,
                    )
                )

            rerun_needed = False

            for task in current_graph.tasks:
                attempts = 0
                task_done = False
                task_error: Optional[str] = None

                while attempts < self.config.max_task_attempts and not task_done:
                    attempts += 1

                    # ReAct sub-loop
                    react_state = ReactState(
                        task=task,
                        env_state=env_state,
                        step=1,
                    )
                    last_tool_result: Optional[str] = None
                    last_tool_call_result: Optional[str] = None
                    react_steps = 0

                    while react_state.step <= self.config.react_max_steps:
                        action = react_step(
                            react_state,
                            self.llm,
                            tools_whitelist=tools_whitelist,
                        )
                        react_steps += 1

                        if action.kind == ActionKind.TOOL_CALL.value:
                            tool_result = self.executor.execute(
                                action.tool_name, action.tool_args
                            )
                            last_tool_result = tool_result
                            last_tool_call_result = tool_result
                            react_state.history.append({
                                "step": react_state.step,
                                "action_kind": "tool_call",
                                "tool": action.tool_name,
                                "result": tool_result[:300],
                            })
                            react_state.env_state = {
                                **react_state.env_state,
                                "last_tool": tool_result[:200],
                            }
                            react_state.step += 1

                            # 全局步数超限检测
                            trig = should_replan_global(
                                self.config.replan_policy,
                                replan_count=replan_count,
                                global_step_limit=self.config.global_step_limit,
                                current_global_step=len(ctx) + react_steps,
                            )
                            if trig.reason:
                                task_error = f"global_limit: {trig.detail}"
                                break
                            continue

                        if action.kind == ActionKind.TASK_DONE.value:
                            last_tool_result = action.result or ""
                            react_state.step = self.config.react_max_steps + 1
                            continue

                        if action.kind == ActionKind.ASK_USER.value:
                            # 等用户确认
                            audit_lines.append(self.audit.warn(
                                "await_user",
                                task_id=task.id,
                                question=action.question,
                            ))
                            return AgentLoopResult(
                                status=LoopStatus.AWAIT_USER.value,
                                ctx=ctx,
                                replan_count=replan_count,
                                completed_tasks=[c.task_id for c in completed],
                                failed_tasks=failed_tasks,
                                duration_ms=int((time.time() - start) * 1000),
                                audit_lines=audit_lines,
                            )
                        # 未知 kind（validate_action 已过滤）
                        break

                    # Verify
                    if task.verify_hint is None:
                        # 没 verify_hint 视为通过（Phase 1 兜底）
                        verify_result = VerifyResult(
                            passed=True, output="no verify_hint", kind="none"
                        )
                    else:
                        verify_result = verify_task(task, self.executor)

                    if verify_result.passed:
                        task_done = True
                        audit_lines.append(
                            self.audit.task_done(task.id, attempts=attempts)
                        )
                        # UUID 提取：优先最后 tool_call 结果（task_done 会覆盖 last_tool_result，
                        # 但 tool 返回的 UUID 更具语义价值）
                        uuid_source = (
                            last_tool_call_result
                            or last_tool_result
                            or verify_result.output
                        )
                        ctx_entry = {
                            "index": task.id,
                            "title": task.title,
                            "id": extract_task_id(uuid_source),
                            "result": last_tool_result or verify_result.output,
                        }
                        ctx.append(ctx_entry)
                        completed.append(CompletedStep(
                            task_id=task.id,
                            title=task.title,
                            result_summary=(last_tool_result or verify_result.output)[:300],
                            task_id_uuid=ctx_entry["id"],
                        ))
                        audit_lines.append(self.audit.verify_pass(
                            task.id, evidence=verify_result.evidence
                        ))
                    else:
                        audit_lines.append(self.audit.verify_fail(
                            task.id, reason=verify_result.error or "unknown"
                        ))
                        task_error = verify_result.error or "verify failed"

                    # Replan 触发判断（仅 verify 失败时）
                    if (
                        not task_done
                        and self.config.enable_replan
                        and replan_count < self.config.replan_policy.max_replans_per_graph
                    ):
                        trig = should_replan_after_step(
                            self.config.replan_policy,
                            task=task,
                            react_steps_completed=react_steps,
                            last_error=task_error,
                        )
                        if trig.should_replan:
                            audit_lines.append(self.audit.replan_triggered(
                                reason=trig.reason.value,
                                graph_id=current_graph.plan_version,
                                replan_count=replan_count + 1,
                            ))
                            current_graph = replan(
                                current_graph,
                                completed=completed,
                                failed_task=task,
                                error=task_error,
                                policy=self.config.replan_policy,
                            )
                            replan_count += 1
                            rerun_needed = True
                            break  # 跳出 while + for，外层 outer_iter 重跑

                if not task_done:
                    failed_tasks.append(task.id)
                    audit_lines.append(self.audit.task_fail(
                        task.id,
                        reason=task_error or "max attempts",
                        attempts=attempts,
                    ))
                    # 失败但不立即中断（后续 task 可能独立完成）
                    continue

            if not rerun_needed:
                break

        # 全部 task 跑完
        status = (
            LoopStatus.OK.value
            if not failed_tasks
            else LoopStatus.FAILED.value
        )
        if (
            failed_tasks
            and replan_count >= self.config.replan_policy.max_replans_per_graph
        ):
            status = LoopStatus.REPLAN_EXHAUSTED.value

        audit_lines.append(self.audit.skill_finish(
            skill_run_id=skill_run_id,
            status=status,
            end_reason="ok" if status == LoopStatus.OK.value else "failed",
            completed_steps=len(completed),
            total_steps=len(current_graph.tasks),
        ))

        return AgentLoopResult(
            status=status,
            ctx=ctx,
            replan_count=replan_count,
            completed_tasks=[c.task_id for c in completed],
            failed_tasks=failed_tasks,
            error=None if status == LoopStatus.OK.value else f"{len(failed_tasks)} task(s) failed",
            duration_ms=int((time.time() - start) * 1000),
            audit_lines=audit_lines,
        )
