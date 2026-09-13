#!/usr/bin/env python3
"""
模块地图对拍（防架构文档漂移，风格同 audit_tauri_bridge.py）：

`docs/rust-bot-architecture.md` 顶部那棵模块树是新人理解代码的第一入口，
而模块拆分/合并是常态（本项目近期做过 bot.rs 四拆、prompts/ 抽取、paths 抽取）——
靠自觉同步必漂，所以用脚本对拍：

- 树里每条 `── xxx.rs` 必须**指向真实文件**（模块删了/改名了，文档没同步 → FAIL）
- 每个源码 `.rs` 必须**在文档里被提到**（相对路径或文件名；新增模块漏写文档 → FAIL）
- `lib.rs` 声明的每个顶层模块必须在文档里有对应条目

执行：python3 -m pytest tests-audit/audit_module_map.py -v
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/src"
ARCH_DOC = ROOT / "docs/rust-bot-architecture.md"

# 模块树条目：`├── bot/registry.rs (716)` / `│   prompts/system.rs ...`
TREE_ENTRY_RE = re.compile(r"^[│├└─ ]*──\s+([A-Za-z_0-9/]+\.rs)", re.M)
# lib.rs / 子模块 mod.rs 的模块声明
MOD_DECL_RE = re.compile(r"^\s*(?:pub(?:\(crate\))? )?mod ([a-z_0-9]+);", re.M)


def doc_text():
    assert ARCH_DOC.exists(), "缺 docs/rust-bot-architecture.md"
    return ARCH_DOC.read_text()


def tree_entries():
    return TREE_ENTRY_RE.findall(doc_text())


def source_files():
    return sorted(p.relative_to(SRC) for p in SRC.rglob("*.rs"))


def declared_modules():
    """lib.rs 顶层模块 + 子目录 mod.rs 里的子模块（`bot/xxx` 形式）"""
    out = set()
    for rel in ["lib.rs", "bot/mod.rs", "bot_skills/mod.rs", "memory/mod.rs", "prompts/mod.rs"]:
        p = SRC / rel
        if not p.exists():
            continue
        for m in MOD_DECL_RE.findall(p.read_text()):
            out.add(m if rel == "lib.rs" else f"{rel.split('/')[0]}/{m}")
    return out


def test_module_tree_is_parseable():
    assert tree_entries(), "架构文档里没解析到模块树条目（`── xxx.rs`）——树被挪走了还是改了写法？"


def test_tree_entries_point_to_existing_files():
    missing = [e for e in tree_entries() if not (SRC / e).exists()]
    assert not missing, (
        "模块树指向不存在的文件（模块已删/改名，文档没同步）：\n  " + "\n  ".join(missing)
    )


def test_every_source_file_is_mentioned():
    doc = doc_text()
    unlisted = [
        str(f) for f in source_files() if str(f) not in doc and f.name not in doc
    ]
    assert not unlisted, (
        "以下源码文件在 docs/rust-bot-architecture.md 里找不到"
        "（新增模块请登记进模块树）：\n  " + "\n  ".join(unlisted)
    )


def test_declared_modules_have_doc_entry():
    doc = doc_text()
    missing = sorted(
        m for m in declared_modules() if f"{m}.rs" not in doc and f"{m}/" not in doc
    )
    assert not missing, (
        "lib.rs / mod.rs 声明的模块在文档里没有对应条目：\n  " + "\n  ".join(missing)
    )
