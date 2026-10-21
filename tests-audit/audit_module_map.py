#!/usr/bin/env python3
"""
模块地图对拍（防架构文档漂移，风格同 audit_tauri_bridge.py）：

`docs/rust-bot-architecture.md` 顶部那棵模块树是新人理解代码的第一入口，
而模块拆分/合并是常态（本项目近期做过 bot.rs 四拆、prompts/ 抽取、paths 抽取）——
靠自觉同步必漂，所以用脚本对拍：

- 树里每条 `── xxx.rs` 必须**指向真实文件**（模块删了/改名了，文档没同步 → FAIL）
- 每个源码 `.rs` 必须**在文档里被提到**（相对路径或文件名；新增模块漏写文档 → FAIL）
- `lib.rs` 声明的每个顶层模块必须在文档里有对应条目
- 同一条目在同一层不得出现两次（拼接/复制粘贴残留 → FAIL）
- 带 `(NNN)` 行数标注的条目，行数必须与文件实际一致（标注过期 → FAIL）

执行：python3 -m pytest tests-audit/audit_module_map.py -v
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/src"
ARCH_DOC = ROOT / "docs/rust-bot-architecture.md"

# 模块树条目：`├── bot/registry.rs (716)` / `│   prompts/system.rs ...`
# 组 1 = 缩进前缀（分层用）、组 2 = 文件名或相对路径、组 3 = 行数标注（可为 None）
TREE_ENTRY_RE = re.compile(
    r"^([│├└─ ]*)──\s+([A-Za-z_0-9/]+\.rs)(?:\s*\((\d+)(?:行)?\))?", re.M
)
# §6.4 源文件清单条目：`- `bot/registry.rs``
SECTION_6_4_RE = re.compile(r"^- `([A-Za-z_0-9/]+\.rs)`", re.M)
# 文档开头的散文数字：「共 155 个文件、约 11.2 万行」
PROSE_FILES_RE = re.compile(r"共\s*(\d+)\s*个文件")
PROSE_LINES_RE = re.compile(r"约\s*([\d.]+)\s*万行")
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
    return [m.group(2) for m in TREE_ENTRY_RE.finditer(doc_text())]


def tree_entry_rows():
    """树条目三元组：(缩进前缀, 裸名, 行数标注或 None)。

    行数标注只有部分条目有（`── bot.rs (116)`），没有就是没写——不是过期，
    对拍时跳过，否则这条断言会退化成「逼全树补行数」的格式门禁。
    """
    return [(m.group(1), m.group(2), m.group(3)) for m in TREE_ENTRY_RE.finditer(doc_text())]


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


def section_6_4_entries():
    """§6.4「源文件清单」里的条目（`- `path`` 格式）。

    与模块树的 `── xxx.rs` 是**两套格式**：脚本原先只解析模块树，
    §6.4 那份自称「完整列表、由本脚本对拍」的清单其实零校验——
    里面躺着 3 条指向不存在文件的幽灵条目、漏登 26 个真实文件，
    脚本照样全绿。
    """
    doc = doc_text()
    assert "## 6.4" in doc, "架构文档里没有 §6.4 源文件清单（挪走了还是改了标题？）"
    sec = doc.split("## 6.4", 1)[1]
    # §6.4 是最后一节；若后面还有 `## ` 标题就截到那里
    sec = sec.split("\n## ", 1)[0]
    return SECTION_6_4_RE.findall(sec)


def total_source_lines():
    """源码总行数（换行符个数，与 `wc -l` 同值）。"""
    return sum(p.read_bytes().count(b"\n") for p in SRC.rglob("*.rs"))


def declared_modules():
    """lib.rs 顶层模块 + 全部 mod.rs（动态发现）里的子模块声明"""
    out = set()
    for p in [SRC / "lib.rs", *sorted(SRC.rglob("mod.rs"))]:
        if not p.exists():
            continue
        rel = p.relative_to(SRC).as_posix()
        # lib.rs 的声明就是顶层名；子目录 mod.rs 的声明带目录前缀（a/b/mod.rs
        # 里的 `mod x;` → a/b/x）。硬编码清单会随目录增长漂移，改 rglob 动态发现
        prefix = "" if rel == "lib.rs" else rel.rsplit("/", 1)[0] + "/"
        for m in MOD_DECL_RE.findall(p.read_text()):
            out.add(f"{prefix}{m}" if prefix else m)
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
    """每个源文件必须在 §6.4 的源文件清单里**列出**。

    判据从「全文任意位置提到」收紧到「§6.4 列出」：旧判据下，26 个漏登记
    的文件在正文散文里被提到过，照样过审——「清单自称完整、实际缺 26 条」
    与「文档说的事实被代码否掉」是同一个形状。
    """
    listed = set(section_6_4_entries())
    unlisted = [f.as_posix() for f in source_files() if f.as_posix() not in listed]
    assert not unlisted, (
        "以下源码文件没在 §6.4 源文件清单里列出"
        "（新增模块请登记进 §6.4）：\n  " + "\n  ".join(unlisted)
    )


def test_section_6_4_entries_point_to_existing_files():
    """§6.4 的每条清单必须指向真实文件。

    幽灵条目（模块已删、文档没同步）会让读者按清单导航时点到不存在的文件。
    """
    ghosts = [e for e in section_6_4_entries() if not (SRC / e).exists()]
    assert not ghosts, (
        "§6.4 源文件清单里有指向不存在文件的条目"
        "（模块已删/改名，清单没同步）：\n  " + "\n  ".join(ghosts)
    )


def test_prose_file_and_line_counts_match_reality():
    """文档开头的散文数字必须等于实测值。

    这句话是架构文档的入口第一屏，文档别处用「≤1200 行」当拆分阈值、
    用总行数估算评审范围——数字错了会系统性偏低。旧实现写下即错
    （166 个文件 / 8.9 万行，实际 155 / 11.2 万），而脚本对散文零校验。
    """
    head = doc_text().split("\n## ", 1)[0]
    m_files = PROSE_FILES_RE.search(head)
    m_lines = PROSE_LINES_RE.search(head)
    assert m_files, "文档开头没写「共 N 个文件」——数字必须可被校验"
    assert m_lines, "文档开头没写「约 N 万行」——数字必须可被校验"
    actual_files = len(source_files())
    actual_lines = total_source_lines()
    assert int(m_files.group(1)) == actual_files, (
        f"文档写 {m_files.group(1)} 个文件，实际 {actual_files}"
    )
    written_wan = float(m_lines.group(1))
    actual_wan = round(actual_lines / 10000, 1)
    assert written_wan == actual_wan, (
        f"文档写约 {written_wan} 万行，实际 {actual_lines} 行（{actual_wan} 万）"
    )


def test_declared_modules_have_doc_entry():
    doc = doc_text()
    missing = sorted(
        m for m in declared_modules() if f"{m}.rs" not in doc and f"{m}/" not in doc
    )
    assert not missing, (
        "lib.rs / mod.rs 声明的模块在文档里没有对应条目：\n  " + "\n  ".join(missing)
    )


def test_tree_entries_are_not_duplicated():
    """同一缩进层里的条目不得出现两次。

    key 带缩进前缀：树里子目录条目按父目录分组写裸文件名（`db/paths.rs` 写成
    `│   ├── paths.rs`），同名不同父目录是真两条文件，只按裸名去重会误报。
    """
    seen = set()
    dups = []
    for prefix, name, _ in tree_entry_rows():
        if (prefix, name) in seen:
            dups.append(f"{name}（缩进前缀 {prefix!r}）")
        seen.add((prefix, name))
    assert not dups, (
        "模块树里同一条目重复出现（拼接/复制粘贴残留）：\n  " + "\n  ".join(dups)
    )


def test_annotated_line_counts_match_files():
    """带 `(NNN)` 行数标注的条目，行数必须与文件实际一致。

    行数口径 = 换行符个数，与 `wc -l` 同值；纯 Python 计数避免 subprocess
    的跨平台差异，也省掉每个文件一次进程启动（这份脚本要进门禁按秒计）。
    """
    drift = []
    for _, name, annotated in tree_entry_rows():
        if annotated is None:
            continue
        rel = resolve_entry(name)
        if rel is None:
            # 指不到文件由 test_tree_entries_point_to_existing_files 报，不重复报
            continue
        actual = (SRC / rel).read_bytes().count(b"\n")
        if actual != int(annotated):
            drift.append(f"{name}：文档写 {annotated}，实际 {actual}（wc -l 口径）")
    assert not drift, "模块树行数标注与实际文件漂移：\n  " + "\n  ".join(drift)
