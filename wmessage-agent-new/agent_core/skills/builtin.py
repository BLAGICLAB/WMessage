"""Phase 2 内置 Skill — 透传 Rust 薄壳 doc_* 命令。

设计原则：
- 这些 Skill 不在 Python 端写生成逻辑（不重复造轮子）
- Rust bot_py.rs 已经有 doc_make_word / doc_make_excel / doc_make_ppt 完整实现
- Python Skill 只做元数据 + 路由 + 透传

执行链路：
  Python chat → intent_router 命中 builtin
       ↓
  AgentLoop 跑 PREV，工具白名单 = [doc_make_word]
       ↓
  LLM 输出 tool_call({"name": "doc_make_word", "args": {...}})
       ↓
  executor.execute("doc_make_word", args)
       ↓
  Phase 2: MockExecutor 返回 fake path
  Phase 3: IPCExecutor 透传 Rust → Rust bot_py.rs::doc_make_word → 生成文件

迁移来源：
- 原 OpenClaw minimax-docx/xlsx/ppt-slides（已在 WMessage 数据目录 skills/）
- 这些 Skill 的"机制"在 Rust 侧已成熟，Python 端只需路由 + 元数据
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional


@dataclass
class BuiltinSkill:
    """内置 Skill 元数据。

    对应 wmessage-agent-spec.md §W Skill frontmatter 字段（简化版）。
    """

    name: str
    description: str
    intents: List[str]
    mode: str  # "auto" | "interactive"
    risk: str  # "low" | "medium" | "high"
    tools_whitelist: List[str]
    react_max_steps: int = 15
    timeout_s: int = 60
    # 元数据：用于 audit / intent_router 调试
    tags: List[str] = field(default_factory=list)


# ───────────────────────── 3 个文档生成 Skill ─────────────────────────

DOCX_SKILL = BuiltinSkill(
    name="minimax-docx",
    description=(
        "生成 Word 文档（标题 + 段落 + 表格，公文 / 提案 / 报告 等格式）。"
        "底层透传 Rust doc_make_word（python-docx 实现）。"
    ),
    intents=[
        "生成 word",
        "做个 word",
        "做个文档",
        "写文档",
        "docx",
        "word 文件",
        "word 文档",
        "生成文档",
        "写个文档",
        "word 报告",
    ],
    mode="auto",
    risk="low",
    tools_whitelist=["doc_make_word"],
    react_max_steps=10,
    timeout_s=60,
    tags=["document", "word", "docx"],
)

XLSX_SKILL = BuiltinSkill(
    name="minimax-xlsx",
    description=(
        "生成 Excel 表格（多 sheet + 公式支持）。"
        "底层透传 Rust doc_make_excel（openpyxl 实现）。"
    ),
    intents=[
        "生成 excel",
        "做个 excel",
        "做个表格",
        "xlsx",
        "excel 文件",
        "excel 表格",
        "表格",
        "导出表格",
        "做个 xlsx",
    ],
    mode="auto",
    risk="low",
    tools_whitelist=["doc_make_excel"],
    react_max_steps=10,
    timeout_s=60,
    tags=["document", "excel", "xlsx", "table"],
)

PPTX_SKILL = BuiltinSkill(
    name="minimax-pptx",
    description=(
        "生成 PowerPoint 演示文稿（多 slide + theme 配色）。"
        "底层透传 Rust doc_make_ppt（python-pptx 实现 + 10 套配色 theme）。"
    ),
    intents=[
        "生成 ppt",
        "做个 ppt",
        "做幻灯片",
        "pptx",
        "ppt 文件",
        "幻灯片",
        "演示文稿",
        "做个 pptx",
        "做个演示",
    ],
    mode="auto",
    risk="low",
    tools_whitelist=["doc_make_ppt"],
    react_max_steps=15,
    timeout_s=90,
    tags=["document", "ppt", "pptx", "slide", "presentation"],
)


# ───────────────────────── 注册表 ─────────────────────────

BUILTIN_SKILLS: Dict[str, BuiltinSkill] = {
    "minimax-docx": DOCX_SKILL,
    "minimax-xlsx": XLSX_SKILL,
    "minimax-pptx": PPTX_SKILL,
}


def get_builtin_skill(name: str) -> Optional[BuiltinSkill]:
    """按 name 查 builtin skill。"""
    return BUILTIN_SKILLS.get(name)


def find_builtin_skill_by_intent(query: str) -> Optional[BuiltinSkill]:
    """Phase 2 关键词 L1 路由 — 子串匹配。

    Args:
        query: 用户自然语言查询

    Returns:
        第一个命中的 BuiltinSkill，无命中返回 None
    """
    q = (query or "").lower()
    for skill in BUILTIN_SKILLS.values():
        for intent in skill.intents:
            if intent.lower() in q:
                return skill
    return None
