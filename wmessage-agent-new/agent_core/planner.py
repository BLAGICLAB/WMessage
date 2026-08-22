"""planner — 粗粒度 TaskGraph 生成（DAG）。

Phase 1 实现：
- plan() 主入口：分发到启发式 / LLM 模式
- _heuristic_plan()：纯函数，正则/关键词拆分，无外部依赖，可测
- _validate_plan()：校验 TaskGraph 良构（无环、deps 存在、ID 唯一）
- _llm_plan()：调用注入的 LLM 客户端（Phase 1 stub 实现，Phase 2 接入 MiniMax-M3）

设计约束（来自 wmessage-agent-spec.md §X.2.2 阶段 1）：
- 粒度到「单一可验证单元」，不写实现细节
- 默认 max_tasks=20（harness_policy.toml 可调）
- 默认每个 task 带 command `echo done` 占位 verify_hint（用户/Skill 后续可覆盖）
- Skill body 命中时整段作为 1 个 task（planner_hint 引导拆分）
"""
from __future__ import annotations

import re
from typing import Any, Callable, Dict, List, Optional

from agent_core.types import PlanResult, TaskGraph, TaskNode, VerifyHint

# ───────────────────────── Exceptions ─────────────────────────


class PlanError(Exception):
    """planner 错误基类。"""


class PlanValidationError(PlanError):
    """TaskGraph 不良构（环、缺失 dep、ID 重复）。"""


class PlanLLMError(PlanError):
    """LLM 调用 / 解析失败。"""


# ───────────────────────── 主入口 ─────────────────────────


def plan(
    query: str,
    context: Optional[Dict[str, Any]] = None,
    skill_body: Optional[str] = None,
    max_tasks: int = 20,
    llm: Optional[Callable[[str], str]] = None,
) -> PlanResult:
    """生成 TaskGraph。

    Args:
        query: 用户自然语言需求
        context: 当前 session 上下文（文件树 / git / 最近报错），可选
        skill_body: 命中 Skill 后注入的 body（None = 无 Skill）
        max_tasks: 最大任务数（默认 20，harness_policy.toml 可调）
        llm: LLM 客户端（None = 启发式；Phase 2 接入 MiniMax-M3）

    Returns:
        PlanResult(graph=TaskGraph, reasoning=规划思路)

    Raises:
        PlanValidationError: 校验失败（环/缺失/重复）
        PlanLLMError: LLM 模式调用失败
        ValueError: 参数非法
    """
    if not isinstance(query, str):
        raise ValueError(f"query 必须是 str，实际 {type(query).__name__}")
    if max_tasks < 1:
        raise ValueError(f"max_tasks 必须 ≥ 1，实际 {max_tasks}")

    if llm is None:
        graph, reasoning = _heuristic_plan(query, context or {}, skill_body, max_tasks)
    else:
        graph, reasoning = _llm_plan(query, context or {}, skill_body, max_tasks, llm)

    _validate_plan(graph)
    return PlanResult(graph=graph, reasoning=reasoning)


# ───────────────────────── 启发式实现 ─────────────────────────


# 句子切分：中文标点 + 英文 ". " (period + space，避免误伤 "config.yaml" / "Mr.")
# Phase 2 LLM 模式不依赖此启发式
_SENTENCE_SPLIT = re.compile(r"[。！？!?;；\n]+|\.\s+")
# 任务动词（粗匹配，启发式专用）
_TASK_VERBS = re.compile(
    r"(实现|写|创建|建|修复|改|添加|删除|移除|重构|优化|跑|运行|测试|"
    r"verify|check|implement|write|create|fix|add|remove|refactor|"
    r"optimize|run|test|debug|deploy)",
    re.IGNORECASE,
)


def _heuristic_plan(
    query: str,
    context: Dict[str, Any],
    skill_body: Optional[str],
    max_tasks: int,
) -> tuple:
    """启发式规划 — 纯函数，无外部依赖。

    规则：
    1. query 为空 → 返回空 TaskGraph
    2. 有 skill_body → 整段作为 1 个 task（planner_hint 引导拆分后续覆盖）
    3. 多句子 → 按句子切分，每句生成 1 个 task，线性依赖
    4. 单句子 → 1 个 task
    5. 默认 verify_hint: command `echo done` + expect exit_0（占位）
    """
    reasoning_parts = []
    tasks: List[TaskNode] = []

    query_stripped = query.strip()
    if not query_stripped:
        reasoning_parts.append("query 为空，返回空 TaskGraph")
        graph = TaskGraph(tasks=[], root_query=query)
        return graph, "\n".join(reasoning_parts)

    # 命中 Skill：整段作为 1 个 task
    if skill_body and skill_body.strip():
        first_task = TaskNode(
            id="t1",
            title=query_stripped[:80] + ("…" if len(query_stripped) > 80 else ""),
            deps=[],
            verify_hint=_default_verify_hint(),
        )
        tasks.append(first_task)
        reasoning_parts.append("命中 Skill，整段作为单一 task")
        reasoning_parts.append(f"Skill body 已注入（{len(skill_body)} chars）")
    else:
        # 按句子切分
        sentences = [s.strip() for s in _SENTENCE_SPLIT.split(query_stripped) if s.strip()]
        if not sentences:
            sentences = [query_stripped]

        sentences = sentences[:max_tasks]
        for i, sent in enumerate(sentences, 1):
            deps = [f"t{i-1}"] if i > 1 else []
            tasks.append(
                TaskNode(
                    id=f"t{i}",
                    title=sent[:80] + ("…" if len(sent) > 80 else ""),
                    deps=deps,
                    verify_hint=_default_verify_hint(),
                )
            )
        reasoning_parts.append(f"按 {len(sentences)} 个句子拆分，线性依赖")

    if len(query_stripped) > max_tasks * 80:
        reasoning_parts.append(
            f"⚠️ query 较长（{len(query_stripped)} 字符），任务数已达上限 {max_tasks}"
        )

    graph = TaskGraph(tasks=tasks, root_query=query_stripped)
    return graph, "\n".join(reasoning_parts)


def _default_verify_hint() -> VerifyHint:
    """Phase 1 默认 verify_hint 占位 — 用户/Skill 可覆盖。"""
    return VerifyHint(
        kind="command",
        command='echo "task done"',
        expect="exit_0",
    )


# ───────────────────────── LLM 模式 ─────────────────────────


def _llm_plan(
    query: str,
    context: Dict[str, Any],
    skill_body: Optional[str],
    max_tasks: int,
    llm: Callable[[str], str],
) -> tuple:
    """LLM 模式 — 调用 llm(prompt) 拿 JSON，返回 TaskGraph。

    LLM 必须返回 JSON：
    {
      "tasks": [{"id": "t1", "title": "...", "deps": [], "verifyHint": {...} | null}],
      "reasoning": "..."
    }
    """
    prompt = _build_llm_prompt(query, context, skill_body, max_tasks)
    try:
        raw = llm(prompt)
    except Exception as e:
        raise PlanLLMError(f"LLM 调用失败：{e}") from e

    try:
        import json
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        raise PlanLLMError(f"LLM 返回非 JSON：{e}\n原始：{raw[:200]}") from e

    tasks_raw = data.get("tasks", [])
    if not isinstance(tasks_raw, list):
        raise PlanLLMError("LLM 返回的 tasks 不是 list")

    tasks: List[TaskNode] = []
    for t in tasks_raw[:max_tasks]:
        if not isinstance(t, dict):
            raise PlanLLMError(f"task 不是 dict：{t}")
        vh_raw = t.get("verifyHint")
        vh = VerifyHint.from_dict(vh_raw) if vh_raw else _default_verify_hint()
        tasks.append(
            TaskNode(
                id=str(t["id"]),
                title=str(t["title"]),
                deps=list(t.get("deps", [])),
                verify_hint=vh,
            )
        )

    graph = TaskGraph(tasks=tasks, root_query=query.strip())
    reasoning = data.get("reasoning", "")
    return graph, reasoning


def _build_llm_prompt(query: str, context: dict, skill_body: Optional[str], max_tasks: int) -> str:
    """构造 LLM prompt — Phase 2 接入 MiniMax-M3 后优化。"""
    parts = [
        f"你是一个任务规划器。给定用户需求，输出 JSON 格式的 TaskGraph。",
        f"",
        f"约束：",
        f"- 最多 {max_tasks} 个 task",
        f"- 每个 task 是「单一可验证单元」，不要写实现细节",
        f"- task.title 一句话即可",
        f"- task.deps 列出依赖的 task.id（线性或 DAG）",
        f"- task.verifyHint：每个 task 必须有可机器校验的 verify 提示",
        f"",
        f"用户需求：{query}",
    ]
    if context:
        parts.append(f"\n当前上下文：{context}")
    if skill_body:
        parts.append(f"\nSkill 指引（planner_hint）：\n{skill_body[:500]}")
    parts.append(
        "\n输出 JSON：\n"
        '{"tasks": [{"id": "t1", "title": "...", "deps": [], '
        '"verifyHint": {"kind": "command", "command": "...", "expect": "exit_0"}}], '
        '"reasoning": "..."}'
    )
    return "\n".join(parts)


# ───────────────────────── 校验 ─────────────────────────


def _validate_plan(graph: TaskGraph) -> None:
    """校验 TaskGraph 良构。

    检查项：
    1. task.id 唯一
    2. task.deps 中所有 id 都存在
    3. 无环（DAG）
    4. task.title 非空
    """
    if not graph.tasks:
        return

    # ID 唯一
    seen = set()
    for t in graph.tasks:
        if t.id in seen:
            raise PlanValidationError(f"重复 task id：{t.id}")
        seen.add(t.id)
        if not t.title.strip():
            raise PlanValidationError(f"task {t.id} title 为空")

    # deps 存在
    for t in graph.tasks:
        for d in t.deps:
            if d not in seen:
                raise PlanValidationError(f"task {t.id} 依赖不存在的 task {d}")

    # 无环（拓扑排序，DFS 染色）
    WHITE, GRAY, BLACK = 0, 1, 2
    color = {t.id: WHITE for t in graph.tasks}

    def dfs(node: str) -> None:
        color[node] = GRAY
        for t in graph.tasks:
            if node in t.deps:
                if color[t.id] == GRAY:
                    raise PlanValidationError(f"检测到环：{node} → {t.id}")
                if color[t.id] == WHITE:
                    dfs(t.id)
        color[node] = BLACK

    for t in graph.tasks:
        if color[t.id] == WHITE:
            dfs(t.id)
