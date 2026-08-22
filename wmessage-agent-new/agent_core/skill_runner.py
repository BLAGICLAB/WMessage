"""Phase 1 mock Skill end-to-end runner。

按 SPEC V Phase 1 验收最后一项：「1 个 mock Skill 端到端跑通」。

本文件证明 IPC + planner + executor + var_substitute + verify 可以被串起来。
Phase 2 会被 `agent_core/agent_loop.py`（PREV 主循环）整体替代。

端到端流程：
1. load_skill   — 解析 SKILL.md frontmatter + body
2. plan         — 调 planner，skill_body 注入，整段作为 1 task
3. parse_dsl    — 解析 body 中的 ## Step N + tool_call({...})
4. for each step:
   ├─ substitute_vars  — 替换 ${step1.id} / ${prev.id} 等
   ├─ parse_tool_call  — 提取 tool_name + args
   ├─ executor.execute — mock tool 调用（返回含 UUID 文本）
   ├─ extract_task_id  — 从结果提取 UUID
   └─ push ctx          — 进入上下文
5. 返回 {frontmatter, plan, reasoning, ctx, steps_executed}
"""
from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Optional

from agent_core.planner import plan
from agent_core.skill_loader import (
    load_skill,
    parse_dsl_steps,
    parse_tool_call_args,
    SkillLoadError,
)
from agent_core.var_substitute import substitute_vars, extract_task_id


class MockSkillError(Exception):
    """Mock Skill 执行失败。"""


def run_mock_skill(
    skill_path,
    query: str,
    executor,
    max_tasks: int = 20,
) -> Dict[str, Any]:
    """端到端跑 mock Skill。

    Args:
        skill_path: SKILL.md 路径
        query: 用户自然语言
        executor: 调用方提供的 executor（RecordingExecutor 或后续 IPC executor）
        max_tasks: planner 最大任务数

    Returns:
        {
            "frontmatter": dict,
            "plan": TaskGraph.to_dict(),
            "reasoning": str,
            "ctx": [{"index", "title", "id", "result"}, ...],
            "steps_executed": int,
        }

    Raises:
        MockSkillError: DSL 解析失败 / 变量替换异常
    """
    fm, body = load_skill(skill_path)

    # 1. planner
    plan_result = plan(query=query, skill_body=body, max_tasks=max_tasks)

    # 2. DSL 解析
    steps = parse_dsl_steps(body)

    # 3. 顺序执行
    ctx: List[Dict[str, Any]] = []
    for step in steps:
        substituted = substitute_vars(step["tool_call"], ctx)
        name, args = parse_tool_call_args(substituted)
        if name is None:
            raise MockSkillError(
                f"step {step['index']} 工具调用解析失败：{substituted!r}"
            )
        if args is None:
            raise MockSkillError(
                f"step {step['index']} args 不是合法 JSON：{substituted!r}"
            )

        # execute_tool（mock / 真 IPC）
        result_text = executor.execute(name, args)

        ctx.append({
            "index": step["index"],
            "title": step["title"],
            "id": extract_task_id(result_text),
            "result": result_text,
        })

    return {
        "frontmatter": fm,
        "plan": plan_result.graph.to_dict(),
        "reasoning": plan_result.reasoning,
        "ctx": ctx,
        "steps_executed": len(steps),
    }


def ctx_summary(ctx: List[Dict[str, Any]]) -> str:
    """格式化 ctx 为人类可读 summary（注入 system prompt / 给 LLM 看）。"""
    if not ctx:
        return "(无已完成步骤)"
    lines = []
    for c in ctx:
        lines.append(f"step {c['index']} [{c['title']}]: {c['result'][:300]}")
    return "\n".join(lines)
