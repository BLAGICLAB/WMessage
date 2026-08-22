"""intent_router — Phase 2 关键词 L1 路由。

设计：
- 简单子串匹配（Phase 1/2 启发式）
- Phase 3 接入 LLM L2（语义判断，初期关）

Phase 2 实现：
- 仅匹配 builtin skills（minimax-docx/xlsx/pptx）
- 自定义 SKILL.md 加载留到 Phase 3（与 WMessage 数据目录 skills/* 集成）

命中后：
- 返回 BuiltinSkill 元数据
- main.py chat handler 用其 tools_whitelist 构造 TaskGraph
- AgentLoop 跑 PREV，LLM 在白名单内自决调 doc_make_*
"""
from __future__ import annotations

from typing import Optional

from agent_core.skills.builtin import BuiltinSkill, BUILTIN_SKILLS, find_builtin_skill_by_intent


def route_intent(query: str) -> Optional[BuiltinSkill]:
    """路由用户查询到 builtin skill。

    Args:
        query: 用户自然语言

    Returns:
        BuiltinSkill 命中，None 未命中
    """
    return find_builtin_skill_by_intent(query)


def list_routable_skills() -> list:
    """列出所有可路由的 builtin skill（调试 / 配置 UI 用）。"""
    return list(BUILTIN_SKILLS.values())
