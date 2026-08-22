"""Skill body loader — frontmatter + body 解析（Phase 1 简化版）。

支持的 frontmatter 格式：
- TOML（`key = "value"`）— 默认，标准 wmessage-agent-spec.md §W 风格
- YAML（`key: value`）— Phase 1 兼容老 Skill 格式

Phase 2 切换为 PyYAML + 完整 frontmatter 校验。

DSL 步骤解析：
- 识别 `## Step N: 标题` + 下一行 `tool_name({...})`
- 这部分 Phase 2 会迁移到新 frontmatter + body 格式，本文件保留为兼容老 Skill
"""
from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


_FRONTMATTER_RE = re.compile(r"^---\n(.*?)\n---\n(.*)$", re.DOTALL)
_STEP_RE = re.compile(r"^##\s*Step\s+(\d+)[:.]\s*(.+)$", re.IGNORECASE)
_TOOL_CALL_RE = re.compile(r"^(\w+)\s*\((.*)\)\s*$", re.DOTALL)


class SkillLoadError(Exception):
    """Skill 加载 / 解析失败。"""


# ───────────────────────── 顶层入口 ─────────────────────────


def load_skill(path) -> Tuple[Dict[str, Any], str]:
    """加载 SKILL.md 文件 → (frontmatter dict, body str)。

    Args:
        path: 文件路径

    Returns:
        (frontmatter, body)
        - 无 frontmatter → ({}, 全文)
        - frontmatter 解析失败 → ({}, 全文)（fallback 不抛错）
    """
    text = Path(path).read_text(encoding="utf-8")
    m = _FRONTMATTER_RE.match(text)
    if not m:
        return {}, text

    fm_text, body = m.group(1), m.group(2)
    try:
        fm = _parse_frontmatter(fm_text)
    except Exception:
        fm = _parse_simple_frontmatter(fm_text)
    return fm, body


def parse_dsl_steps(body: str) -> List[Dict[str, Any]]:
    """从 Skill body 解析 DSL 步骤。

    识别 `## Step N: 标题` + 下一行 `tool_name({...})`。

    Returns:
        [{"index", "title", "tool_call"}, ...]
    """
    steps: List[Dict[str, Any]] = []
    lines = body.split("\n")
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        m = _STEP_RE.match(line)
        if m:
            idx = int(m.group(1))
            title = m.group(2).strip()
            # 下一行非空 = tool_call
            j = i + 1
            while j < len(lines) and not lines[j].strip():
                j += 1
            tool_call = lines[j].strip() if j < len(lines) else ""
            steps.append({
                "index": idx,
                "title": title,
                "tool_call": tool_call,
            })
            i = j + 1
        else:
            i += 1
    return steps


def parse_tool_call_args(tool_call_text: str) -> Tuple[Optional[str], Optional[Any]]:
    """`list_tasks({})` → ('list_tasks', {})。

    Returns:
        (name, args) — 解析失败返回 (None, None)
    """
    m = _TOOL_CALL_RE.match(tool_call_text.strip())
    if not m:
        return None, None
    name = m.group(1)
    args_text = m.group(2).strip()
    if not args_text or args_text == "{}":
        return name, {}
    try:
        args = json.loads(args_text)
    except json.JSONDecodeError:
        return name, None
    return name, args


# ───────────────────────── frontmatter 解析 ─────────────────────────


def _parse_frontmatter(text: str) -> Dict[str, Any]:
    """用 tomli 解析 TOML 风格 frontmatter。"""
    try:
        import tomli
        return tomli.loads(text)
    except ImportError:
        return _parse_simple_frontmatter(text)


def _parse_simple_frontmatter(text: str) -> Dict[str, Any]:
    """YAML 风格的简易 frontmatter 解析（fallback）。

    支持：
    - key: value（字符串 / inline list [a, b]）
    - key: （下一行接 - item 列表）
    - # 注释
    """
    result: Dict[str, Any] = {}
    current_list_key: Optional[str] = None

    for raw in text.split("\n"):
        line = raw.rstrip()
        if not line or line.lstrip().startswith("#"):
            continue

        # list item: "  - value"
        m_list = re.match(r"^\s+-\s+(.+)$", line)
        if m_list and current_list_key:
            v = m_list.group(1).strip().strip('"').strip("'")
            if isinstance(result.get(current_list_key), list):
                result[current_list_key].append(v)
            continue

        # key: value
        if ":" in line:
            k, _, v = line.partition(":")
            k = k.strip()
            v = v.strip()
            if not v:
                current_list_key = k
                result[k] = []
            elif v.startswith("[") and v.endswith("]"):
                items = [
                    x.strip().strip('"').strip("'")
                    for x in v[1:-1].split(",")
                    if x.strip()
                ]
                result[k] = items
                current_list_key = None
            else:
                result[k] = v.strip('"').strip("'")
                current_list_key = None

    return result
