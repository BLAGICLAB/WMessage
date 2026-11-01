#!/usr/bin/env python3
"""审计 SOP 基线对拍（防 SOP 文档里的真值表悄悄过期）。

背景：审计 SOP（`docs/agent-audit-sop-zcode.md`）里有一张「风控常量真值表」，
审计者靠它判断代码里的限额对不对。这张表一旦过期，审计就会对着**不存在的
规格**找问题——把「三档 turn 预算」当缺失项报出来，而代码里那套预算
（档位 + 墙钟）早已撤销。这不是文档小瑕疵，是**审计产出全是假发现**。

所以这张表的真值不是「人抄的定稿值」，而是 `bot/constants.rs` 里的常量。
本守卫逐条对拍：

1. **常量表对拍**（双向）
   - constants.rs 里的每个 `pub const` 都必须在 SOP 表里出现（漏 = 审计者
     不知道这个限额存在，审计维度就少一块）
   - SOP 表里的每行都必须在 constants.rs 里存在，且值表达式一致
     （多 = SOP 写了一个代码里没有的常量 → 审计者会去找一个不存在的约束）
2. **结构计数对拍**
   - 工具总数 = `TOOLS_TABLE` 条目数（= `SCHEMA_*` 常量数）
   - 调度早退点数 = `dispatch.rs` 生产段 `ToolResult::denied(` 的出现次数
     （与 `pre_tool_hook_tests.rs` 的计数断言同口径——断言值变了这里也会红）
   - 子 agent profile 档数 / 内置 onion 配置档数
3. **事件名对拍**
   - SOP 正文里引用成 `code` 的审计事件名必须在源码里真存在
   （与 `audit_doc_claims.py` 同思路，但那份管的是实机卡片）

**为什么值比对用「表达式」而不是「数值」**：constants.rs 里写 `128 * 1024`
而 SOP 表里写 `128 * 1024` 是同一份约定；写成 `131072` 也是同一个数。
本守卫按「表达式求值后相等」判定——`128 * 1024` 与 `131072` 判等，
`128 * 1024` 与 `128` 判不等。

执行：python3 -m pytest tests-audit/audit_sop_baseline.py -v
"""

from pathlib import Path
import re
import sys

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from audit_doc_claims import (  # noqa: E402  —— 事件名规则单源，不另立一套
    EVENT_RE,
    FILE_EXTS,
    NOT_EVENTS,
    in_source,
    source_blob,
)

ROOT = Path(__file__).resolve().parent.parent
SOP = ROOT / "docs/agent-audit-sop-zcode.md"
CONSTANTS = ROOT / "src-tauri/src/bot/constants.rs"
SCHEMAS = ROOT / "src-tauri/src/bot/registry/schemas.rs"
DISPATCH = ROOT / "src-tauri/src/bot/dispatch.rs"
ONIONS = ROOT / "src-tauri/resources/onions"

# SOP 里的基线区块标记（成对；区块外的内容不参与对拍）
BASELINE_BEGIN = "<!-- sop-baseline:start -->"
BASELINE_END = "<!-- /sop-baseline -->"
COUNTS_BEGIN = "<!-- sop-counts:start -->"
COUNTS_END = "<!-- sop-counts:end -->"

# SOP 常量表行：`| `NAME` | `VALUE` | 含义 |`
SOP_CONST_ROW = re.compile(r"^\|\s*`([A-Z_][A-Z0-9_]*)`\s*\|\s*`([^`]+)`\s*\|", re.M)
# SOP 计数块：`键 = 数字`
SOP_COUNT_ROW = re.compile(r"^([^\n=]+?)\s*=\s*(\d+)\s*$", re.M)

# constants.rs：pub const NAME: TYPE = EXPR;
CONST_DECL = re.compile(
    r"^pub const (?P<name>\w+):\s*(?P<ty>[^=]+?)\s*=\s*(?P<val>.+?);", re.M | re.S
)


def sop_text() -> str:
    if not SOP.exists():
        pytest.fail(f"审计 SOP 不存在：{SOP}")
    return SOP.read_text()


def baseline_region() -> str:
    """SOP 的基线区块（常量表 + 计数块都在里面）。"""
    text = sop_text()
    if BASELINE_BEGIN not in text or BASELINE_END not in text:
        pytest.fail(
            f"SOP 里找不到基线区块标记（{BASELINE_BEGIN} / {BASELINE_END}）——"
            "基线区块被删了或改了标记名，审计者就没真值可对了"
        )
    return text.split(BASELINE_BEGIN, 1)[1].split(BASELINE_END, 1)[0]


def counts_region() -> str:
    text = sop_text()
    if COUNTS_BEGIN not in text or COUNTS_END not in text:
        pytest.fail(
            f"SOP 里找不到计数块标记（{COUNTS_BEGIN} / {COUNTS_END}）"
        )
    return text.split(COUNTS_BEGIN, 1)[1].split(COUNTS_END, 1)[0]


def constants_source() -> dict[str, str]:
    """constants.rs 的 {常量名: 值表达式}（只取生产段，排除测试）。"""
    src = CONSTANTS.read_text()
    prod = src.split("#[cfg(test)]", 1)[0]
    out: dict[str, str] = {}
    for m in CONST_DECL.finditer(prod):
        out[m.group("name")] = " ".join(m.group("val").split())
    return out


def sop_constants() -> dict[str, str]:
    """SOP 常量表的 {常量名: 值表达式}。"""
    region = baseline_region()
    out: dict[str, str] = {}
    for m in SOP_CONST_ROW.finditer(region):
        out[m.group(1)] = " ".join(m.group(2).split())
    return out


def eval_expr(expr: str):
    """把 Rust 常量表达式求值成数字；求不了返回 None。

    只支持「整数 × 整数」的乘法与单个整数——constants.rs 里的值就这两种形态
    （`200` / `128 * 1024` / `2 * 1024 * 1024`）。求不了就不是可比值，
    调用方按字符串比对。
    """
    e = expr.strip()
    if not e:
        return None
    if "[" in e or "(" in e:
        return None
    if "*" not in e:
        try:
            return int(e.replace("_", ""))
        except ValueError:
            return None
    parts = [p.strip() for p in e.split("*")]
    total = 1
    for p in parts:
        try:
            total *= int(p.replace("_", ""))
        except ValueError:
            return None
    return total


def string_items(expr: str) -> list[str]:
    """表达式里的字符串字面量列表（数组常量按内容比，不按写法比）。"""
    return re.findall(r'"([^"]*)"', expr)


def same_value(a: str, b: str) -> bool:
    va, vb = eval_expr(a), eval_expr(b)
    if va is not None and vb is not None:
        return va == vb
    # 数组 / 元组常量：按字符串字面量集合比（元素顺序也重要）
    ia, ib = string_items(a), string_items(b)
    if ia or ib:
        return ia == ib
    return a == b


def test_sop_constants_all_present_in_source():
    """constants.rs 的每个常量都必须在 SOP 表里（漏 = 审计维度缺一块）。"""
    src = constants_source()
    sop = sop_constants()
    missing = sorted(set(src) - set(sop))
    if missing:
        pytest.fail(
            "SOP 的常量表漏了代码里的常量：\n"
            + "\n".join(f"  {m} = {src[m]}" for m in missing)
            + "\n（新增风控常量必须同步进 SOP 表——审计者要靠它核对代码）"
        )


def test_sop_constants_match_source_values():
    """SOP 表里的每一行都必须与 constants.rs 的值一致（多 = 写了不存在的常量）。"""
    src = constants_source()
    sop = sop_constants()
    bad = []
    for name, val in sorted(sop.items()):
        if name not in src:
            bad.append(f"  {name} = {val}  ← 代码里没有这个常量")
        elif not same_value(val, src[name]):
            bad.append(f"  {name}: SOP 写 {val}，代码是 {src[name]}")
    if bad:
        pytest.fail(
            "SOP 常量表与 bot/constants.rs 不一致：\n"
            + "\n".join(bad)
            + "\n（要么改代码，要么改 SOP 的表——两边必须同源）"
        )


def test_sop_table_not_empty():
    """空表 = 对拍形同虚设，必须拦。"""
    sop = sop_constants()
    assert len(sop) >= 40, (
        f"SOP 常量表只解析出 {len(sop)} 行——表格格式变了（行首 | `NAME` | `VALUE` |）"
    )


def _expected_counts() -> dict[str, int]:
    schemas = SCHEMAS.read_text()
    dispatch = DISPATCH.read_text()
    prod_dispatch = dispatch.split("#[cfg(test)]", 1)[0]
    onions = sorted(ONIONS.glob("*.yaml"))
    return {
        "工具总数": schemas.count("ToolDef {"),
        "调度早退点数": prod_dispatch.count("ToolResult::denied("),
        "子 agent profile 档数": len(
            re.findall(r"^pub const (?:RESEARCH|CODER|GENERAL)_TOOLS", schemas, re.M)
        ),
        "内置 onion 配置档数": len(onions),
    }


def test_sop_counts_match_code():
    """SOP 里的结构计数必须与代码一致。"""
    expected = _expected_counts()
    region = counts_region()
    got = {}
    for m in SOP_COUNT_ROW.finditer(region):
        key = m.group(1).strip()
        if key:
            got[key] = int(m.group(2))
    for key, want in expected.items():
        assert key in got, f"SOP 计数块缺「{key}」这一行（代码里是 {want}）"
        assert got[key] == want, (
            f"SOP 计数「{key}」写 {got[key]}，代码里是 {want}——"
            "SOP 的地图与代码分家了"
        )


def test_early_return_count_matches_repo_assertion():
    """早退点数必须与仓库自己的计数断言一致（两边都从同一处读，别各说各话）。"""
    test_file = ROOT / "src-tauri/src/bot/dispatch/pre_tool_hook_tests.rs"
    src = test_file.read_text()
    # 锚点：断言里的 `production.matches("ToolResult::denied(").count(), <N>`
    m = re.search(
        r'matches\(\s*"ToolResult::denied\("\)\s*\.count\(\)\s*,\s*(\d+)', src
    )
    assert m, "找不到 dispatch 早退点的计数断言——锚点变了，同步本守卫"
    asserted = int(m.group(1))
    actual = DISPATCH.read_text().split("#[cfg(test)]", 1)[0].count(
        "ToolResult::denied("
    )
    assert asserted == actual, (
        f"仓库断言说 {asserted} 处，dispatch.rs 生产段实际 {actual} 处——"
        "断言与代码已分家"
    )


def test_sop_event_names_exist_in_source():
    """SOP 正文里引用成 `code` 的审计事件名必须在源码里真存在。

    事件名规则复用 `audit_doc_claims.py`（单源）：同一套正则、同一个
    `NOT_EVENTS` 豁免表、同一个 `in_source` 判定——两处规则各写一份必然漂。
    """
    blob = source_blob()
    missing = []
    for lineno, line in enumerate(sop_text().splitlines(), 1):
        for span in re.findall(r"`([^`\n]+)`", line):
            if not EVENT_RE.match(span):
                continue
            if span.endswith(FILE_EXTS) or "www." in span:
                continue
            if span in NOT_EVENTS:
                continue
            if not in_source(span, blob):
                missing.append(f"  第 {lineno} 行: `{span}`")
    if missing:
        pytest.fail(
            "SOP 引用的审计事件名在源码里不存在：\n"
            + "\n".join(missing)
            + "\n（要么代码改了名——改 SOP；要么 SOP 写了从未存在的事件——核对来源）"
        )
