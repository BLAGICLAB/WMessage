"""变量替换 — Skill body / DSL 调用参数模板里的 ${...} 占位符替换。

Phase 1 简化版（Phase 2 增强为完整嵌套路径支持）：
- ${stepN.id}       →  Step N 的 task_id（从结果文本提取的 UUID）
- ${stepN.result}   →  Step N 的完整返回文本
- ${prev.id}        →  上一个 step 的 task_id
- ${prev.result}    →  上一个 step 的完整返回

未知 / 越界占位符保留原样（不报错）。
"""
from __future__ import annotations

import re
from typing import Any, Dict, List, Optional

_VAR_RE = re.compile(r"\$\{([^}]+)\}")
_UUID_RE = re.compile(
    r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    re.IGNORECASE,
)
_STEP_FIELD_RE = re.compile(r"^step(\d+)\.(id|result)$")
_PREV_FIELD_RE = re.compile(r"^prev\.(id|result)$")


# ───────────────────────── 变量替换 ─────────────────────────


def substitute_vars(text: Any, ctx: List[Dict[str, Any]]) -> Any:
    """替换 ${...} 占位符。

    Args:
        text: 模板文本（非 str 直接返回原值）
        ctx: 已完成步骤列表，每项 {"index", "id", "result", "title"}

    Returns:
        替换后的文本（未知占位符保留 ${...} 原样）
    """
    if not isinstance(text, str):
        return text
    if "${" not in text:
        return text

    def replace(m: re.Match) -> str:
        key = m.group(1).strip()

        m_step = _STEP_FIELD_RE.match(key)
        if m_step:
            idx = int(m_step.group(1))
            field = m_step.group(2)
            for c in ctx:
                if c.get("index") == idx:
                    val = c.get(field)
                    if val is not None:
                        return str(val)
            return m.group(0)  # 越界 / 字段 None 保留

        m_prev = _PREV_FIELD_RE.match(key)
        if m_prev and ctx:
            field = m_prev.group(1)
            val = ctx[-1].get(field)
            if val is not None:
                return str(val)
            return m.group(0)

        return m.group(0)  # 未知占位符保留

    return _VAR_RE.sub(replace, text)


# ───────────────────────── UUID 提取 ─────────────────────────


def extract_task_id(result_text: Optional[str]) -> Optional[str]:
    """从 tool 返回文本提取首个 UUID v4。

    Args:
        result_text: tool 调用返回的文本

    Returns:
        UUID 字符串，无则返回 None
    """
    if not result_text:
        return None
    m = _UUID_RE.search(result_text)
    return m.group(0).lower() if m else None
