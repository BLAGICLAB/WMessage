#!/usr/bin/env python3
"""
bot 族命名空间守卫：族外不得直接摸 `crate::bot_*::` 深层路径。

背景：同一个条目曾有两条合法路径（`crate::bot::X` 与 `crate::bot_chat::X`），
新代码不知道该走哪条，评审也无从判断哪条算「正规路径」。批 9 把 `crate::bot::`
定成唯一规范路径——族外一律走门面，族内互引不受限。

族的范围 = bot 命名空间：
- `bot.rs`（门面本体）
- `bot/` 目录（含 `bot/mcp/`、`bot/services/` 等子目录）
- `bot_*.rs` 扁平文件（bot_chat / bot_model_loop / bot_orchestrator / …）
- `bot_*/` 目录族（bot_skills/ / bot_orchestrator/ 等）

族内互引豁免的理由：bot_chat.rs 引用 crate::bot_model_loop:: 是族内近邻调用，
绕门面反而多绕一层；`crate::bot_skills::parse_meta` 这类族内自指更是天经地义。
豁免的边界是「文件在族内」，不是「路径在族内」——族内文件引用族外路径照样放行
（那是另一族的事，由各自的门面管）。

执行：python3 -m pytest tests-audit/audit_bot_namespace.py -v
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/src"

# 深层路径：`crate::bot_xxx::`（`crate::bot::` 是门面本身，不算）
DEEP_PATH_RE = re.compile(r"\bcrate::bot_[a-z0-9_]+::")


def in_bot_family(rel: str) -> bool:
    """文件是否属于 bot 命名空间。

    口径与门面注释一致：`bot.rs` / `bot/` 目录 / `bot_*.rs` / `bot_*/` 目录族。
    `bot_skills/remote.rs` 算族内（它就是 bot 族的一块），`memory/extract.rs`
    不算（那是记忆族，跨族引用必须走门面）。
    """
    return rel == "bot.rs" or rel.startswith("bot/") or rel.startswith("bot_")


def code_lines(text: str):
    """逐行返回「可能是代码」的行号与内容：跳过注释行与块注释内部。

    注释里的路径不是引用（`//! crate::bot_py::X 仍可用` 是散文，不是代码）。
    但只跳注释行、不做字符串字面量解析是有意的保守取舍：宁可漏报也不误报，
    误报会让守卫变成噪声而被忽略。
    """
    out = []
    in_block = False
    for i, line in enumerate(text.split("\n"), 1):
        if in_block:
            if "*/" in line:
                in_block = False
            continue
        stripped = line.strip()
        if stripped.startswith("//"):
            continue
        if "/*" in line and "*/" not in line:
            in_block = True
            continue
        if "/*" in line and "*/" in line:
            # 同行闭合的块注释：只扫 /* 之后的部分
            head, _, tail = line.partition("/*")
            tail = tail.partition("*/")[2]
            out.append((i, head + tail))
            continue
        out.append((i, line))
    return out


def source_files():
    return sorted(p for p in SRC.rglob("*.rs"))


def violations():
    """返回族外的深层路径引用清单：(相对路径, 行号, 命中的路径)。"""
    hits = []
    for p in source_files():
        rel = p.relative_to(SRC).as_posix()
        if in_bot_family(rel):
            continue
        for lineno, line in code_lines(p.read_text()):
            for m in DEEP_PATH_RE.finditer(line):
                hits.append((rel, lineno, m.group(0)))
    return hits


def test_bot_family_scope_is_not_empty():
    """族定义不能空转——族内豁免建立在「确实有个族」之上。

    族定义写错（比如把前缀写成 `bot_/`）会让豁免范围悄悄变窄或变宽，
    这里钉住族内成员数量，防止口径漂移无人察觉。
    """
    family = [p for p in source_files() if in_bot_family(p.relative_to(SRC).as_posix())]
    assert len(family) > 30, f"bot 族只剩 {len(family)} 个文件，族定义可能写错了"


def test_no_deep_bot_paths_outside_the_family():
    """族外不得出现 `crate::bot_*::` 深层路径。"""
    hits = violations()
    assert not hits, (
        "bot 族之外出现 `crate::bot_*::` 深层路径"
        "（跨族引用必须走 `crate::bot::` 门面；门面缺 re-export 就先补）：\n  "
        + "\n  ".join(f"{rel}:{ln}  {path}" for rel, ln, path in hits)
    )


def test_facade_rejects_direct_deep_use():
    """门面自身不得重新开口子——门面是路径收敛的终点，不是又一条深层路径的源头。

    门面内部当然要从源模块取条目（`pub use crate::bot_chat::X`），那是转出
    而不是绕过；这里查的是门面有没有把深层路径原样透给族外（`pub use
    crate::bot_chat::bot_chat` 这种「把整个模块搬过来」的口子）。
    """
    facade = (SRC / "bot.rs").read_text()
    whole_module = re.findall(r"pub(?:\(crate\))?\s+use\s+crate::bot_[a-z0-9_]+::\*\s*;", facade)
    assert not whole_module, (
        "门面不许整模块透传（`pub use crate::bot_xxx::*`）——"
        "逐项转出，族外才拿得到该拿的那几项：\n  " + "\n  ".join(whole_module)
    )
