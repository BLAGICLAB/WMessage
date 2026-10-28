#!/usr/bin/env python3
"""
风控常量单源守卫：`bot/constants.rs` 是 Agent 风控工程常量的唯一真相源。

两条规则：
1. constants.rs 里的每个 `pub const` 必须在生产代码里被引用至少一次。
   零引用的常量是「死副本」——真值在别处的字面量里，改 constants.rs 不生效，
   审阅者却以为改了风控策略。
2. 其他文件里同名的 `const` / `static` 只能是指向 constants 的别名
   （右侧含 `constants::`），不能再写一份字面量。两处字面量改一处漏一处
   不会编译失败也不会测试失败，只会在数值分叉后才暴露。

测试目录（`/tests/`、`tests.rs`、`_tests.rs`）不算生产引用：常量只被测试
引用等于生产代码没在用它。文件内联的 `#[cfg(test)] mod` 无法按路径排除，
是本守卫的已知盲区。

执行：python3 -m pytest tests-audit/audit_constants_single_source.py -v
"""

from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/src"
CONSTANTS = SRC / "bot/constants.rs"

PUB_CONST_RE = re.compile(r"^\s*pub\s+const\s+(\w+)\s*:", re.M)
DEF_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+(\w+)\s*:\s*[^=]+?=\s*(.+?);",
    re.M | re.S,
)


def is_test_path(rel: str) -> bool:
    return "/tests/" in rel or rel.endswith("tests.rs") or rel.endswith("_tests.rs")


def rust_files():
    for p in sorted(SRC.rglob("*.rs")):
        if ".mimosa" in p.parts:
            continue
        yield p, p.relative_to(ROOT).as_posix()


def constant_names() -> list[str]:
    return PUB_CONST_RE.findall(CONSTANTS.read_text(encoding="utf-8"))


def reference_re(name: str) -> re.Pattern:
    # `constants::NAME` 或 `constants::{…, NAME, …}`
    return re.compile(rf"constants::(?:\{{[^}}]*\b{name}\b[^}}]*\}}|{name}\b)")


def test_every_constant_is_referenced_by_production_code():
    names = constant_names()
    assert names, "constants.rs 里没有解析到任何 pub const，正则失效"
    prod_sources = [
        (rel, p.read_text(encoding="utf-8"))
        for p, rel in rust_files()
        if p != CONSTANTS and not is_test_path(rel)
    ]
    dead = []
    for name in names:
        pat = reference_re(name)
        if not any(pat.search(text) for _, text in prod_sources):
            dead.append(name)
    assert not dead, (
        "constants.rs 里以下常量没有任何生产引用（死副本，真值在别处的字面量里）：\n  "
        + "\n  ".join(dead)
    )


def test_no_duplicate_literal_definitions_outside_constants():
    names = set(constant_names())
    dupes = []
    for p, rel in rust_files():
        if p == CONSTANTS:
            continue
        text = p.read_text(encoding="utf-8")
        for m in DEF_RE.finditer(text):
            name, rhs = m.group(1), m.group(2)
            if name in names and "constants::" not in rhs:
                line = text.count("\n", 0, m.start()) + 1
                dupes.append(f"{rel}:{line}  const {name} = {rhs.strip()}")
    assert not dupes, (
        "以下文件把 constants.rs 已有的常量又写了一份字面量（应改为引用 constants::）：\n  "
        + "\n  ".join(dupes)
    )
