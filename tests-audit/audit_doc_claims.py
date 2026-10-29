#!/usr/bin/env python3
"""回归卡片判据 ↔ 源码同源对拍（实机验收文档的「审计事件名」那一层）：

- 真相源：`src-tauri/src/**/*.rs` + `src/**/*.{ts,tsx}`
- 被查文档：`docs/realmachine-regression-test.md`、`docs/realmachine-rerun-2026-10-22.md`、
  `docs/realmachine-mutation-test-2026-10-23.md`、`docs/realmachine-mutation-test-2026-10-24.md`、
  `docs/realmachine-smoke-test-2026-10-25.md`

**为什么要有这道闸**：实机卡片的判据大半落在「审计里出现哪条事件」上。
卡片写的事件名一旦和代码分家（改名、删除、压根没写过），执行者在机器上
grep 不到那一行，只能靠猜来判断是「应用坏了」还是「文档过期」——
附录 D-3 就是这么被实机否证的（读了一半调用链，把「没走到那行代码」
误判成「那行代码有 bug」）。所以文档里引用的每个审计事件名都必须
在源码里真的存在。

**只查事件名这一层**：中文期望文案大量是「运行时拼出来的」（`format!` 组合、
占位符替换），整句比对会得到一堆假失败——那类文本按「判据原文在文档里、
实现改了由代码自身的单测钉住」处理，不在本闸硬对。

执行：python3 -m pytest tests-audit/audit_doc_claims.py -v
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parent.parent
DOCS = [
    ROOT / "docs/realmachine-regression-test.md",
    ROOT / "docs/realmachine-rerun-2026-10-22.md",
    ROOT / "docs/realmachine-mutation-test-2026-10-23.md",
    ROOT / "docs/realmachine-mutation-test-2026-10-24.md",
    ROOT / "docs/realmachine-smoke-test-2026-10-25.md",
]
SRC_DIRS = [ROOT / "src-tauri/src", ROOT / "src"]

# 文件名 / 主机名 / 领域名词——长得像 `a.b` 但不是审计事件
FILE_EXTS = (
    ".rs",
    ".dll",
    ".exe",
    ".py",
    ".json",
    ".ps1",
    ".sh",
    ".md",
    ".toml",
    ".ts",
    ".tsx",
    ".js",
    ".html",
    ".png",
    ".db",
    ".flag",
)
# 逐条写明理由：这些 token 形似事件名，实则不是事件
NOT_EVENTS = {
    "http.server": "Python 标准库模块名（python -m http.server）",
    "meta.rollback": "技能 frontmatter 的字段名，不是审计事件",
    "subagent.max_wall_secs": "已退役的参数表键（墙钟整套取消），只在历史实机方案里出现",
}

EVENT_RE = re.compile(r"^[a-z][a-z0-9_]*(\.[a-z0-9_]+)+$")


def source_blob() -> str:
    parts = []
    for d in SRC_DIRS:
        for p in d.rglob("*"):
            if p.suffix in (".rs", ".ts", ".tsx") and p.is_file():
                parts.append(p.read_text(errors="ignore"))
    return "\n".join(parts)


def event_candidates() -> list[tuple[str, int, str]]:
    """文档里形如 `a.b` 的审计事件名候选：(文档名, 行号, token)"""
    out = []
    for doc in DOCS:
        for lineno, line in enumerate(doc.read_text().splitlines(), 1):
            for span in re.findall(r"`([^`\n]+)`", line):
                if not EVENT_RE.match(span):
                    continue
                if span.endswith(FILE_EXTS) or "www." in span:
                    continue
                if span in NOT_EVENTS:
                    continue
                out.append((doc.name, lineno, span))
    return out


def in_source(token: str, blob: str) -> bool:
    if token in blob:
        return True
    # 运行时拼出来的事件名（如 `manage_file.{action}`）：前缀一致即算命中
    parts = token.split(".")
    for i in range(len(parts) - 1, 0, -1):
        if f"{'.'.join(parts[:i])}.{{" in blob:
            return True
    return False


def test_event_names_exist_in_source():
    blob = source_blob()
    cands = event_candidates()
    assert cands, "没解析到任何审计事件名——文档的引用写法变了吗？"
    missing = [(d, n, t) for d, n, t in cands if not in_source(t, blob)]
    if missing:
        detail = "\n".join(f"  {d}:{n}  {t}" for d, n, t in missing)
        pytest.fail(
            "文档引用的审计事件名在源码里不存在：\n"
            f"{detail}\n"
            "（要么代码改了名——改文档；要么文档写了从未存在的事件——核对来源）"
        )
