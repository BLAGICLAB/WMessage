"""skills package — Skill 注册表 + 内置 Skill。

Phase 2 实现：
- builtin: 3 个内置文档生成 Skill（minimax-docx/xlsx/pptx），透传 Rust 薄壳的 doc_make_* 命令
- registry: 合并 builtin + 自定义 SKILL.md loader（Phase 3 接入）

Phase 3 扩展：
- 用户数据目录 ~/.openclaw/skills/*/SKILL.md 加载
- LLM L2 语义路由（关键词 L1 兜底）
"""
from agent_core.skills.builtin import (
    BuiltinSkill,
    BUILTIN_SKILLS,
    find_builtin_skill_by_intent,
    get_builtin_skill,
)

__all__ = [
    "BuiltinSkill",
    "BUILTIN_SKILLS",
    "find_builtin_skill_by_intent",
    "get_builtin_skill",
]
