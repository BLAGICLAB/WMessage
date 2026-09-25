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
    """解析文档模块树里的 `── xxx.rs` 条目。返回裸名（不带目录前缀）。

    Phase 0.6 保守选择：保留原 regex 行为，不推栈、不拼层。
    拆出子目录里的同名文件由 `resolve_entry()` 走「同名就近」匹配。
    """
    return TREE_ENTRY_RE.findall(doc_text())


def resolve_entry(name):
    """把 tree entry 名字解析为 SRC 下真实文件路径。

    1. 直接 `SRC/name` 存在 → 原路径（不重映射）
    2. 否则在 SRC 里找路径末段匹配的 .rs（候选路径以 entry 名结尾，
       多段名如 `config/types.rs` 要求完整尾段一致）：
       - 唯一尾段匹配 → 返回（stderr 打印重映射行）
       - 零个或多个匹配 → None（响亮失败，让调用方报“指不到”/“歧义”）
    3. 都不存在 → None（让调用方报“指不到”）

    Phase 6 收紧：废弃「同名就近取第一个」——多同名歧义必须显式修文档树，
    不允许静默吞掉错误映射。
    """
    candidates = sorted(SRC.rglob(name))
    direct = SRC / name
    if direct.exists():
        return direct.relative_to(SRC)
    rels = [c.relative_to(SRC).as_posix() for c in candidates]
    tail_matches = [r for r in rels if r.endswith(name)]
    if len(tail_matches) == 1:
        chosen = tail_matches[0]
        print(
            f"[audit_module_map] tree entry {name!r}: 原路径不在，"
            f"尾段唯一匹配 → {chosen}",
            flush=True,
        )
        return chosen
    if len(tail_matches) > 1:
        print(
            f"[audit_module_map] tree entry {name!r}: 尾段匹配歧义 "
            f"{len(tail_matches)} 个 → {tail_matches}（拒绝静默就近，请修文档树）",
            flush=True,
        )
    return None


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
    """每个 tree entry 必须能解析到真实文件。

    Phase 0.6 降级：原路径不在时走「同名就近」匹配（见 `resolve_entry` 重映射日志）。
    Phase 6 必须收紧为「路径末段 + 模块前缀匹配」（见 OCR-FIX-PLAN-2026-09-21.md Phase 6）。
    """
    missing = []
    for e in tree_entries():
        rel = resolve_entry(e)
        if rel is None:
            missing.append(e)
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
