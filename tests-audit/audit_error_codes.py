#!/usr/bin/env python3
"""
CommandError 错误码跨语言一致性检查（风格同 audit_tauri_bridge.py）：

- Rust 真相源：`src-tauri/src/error.rs` 的 `pub enum CommandErrorCode`，
  每个变体上方的 `#[serde(rename = "…")]` 即线协议字符串
- 前端镜像：`src/lib/errorHandler.ts` 的 `export type CommandErrorCode = "…" | …`

任一侧增删 code 而漏同步另一侧 → FAIL。这样「前端 TS 从枚举生成/对齐」不是
口头约定，而是提交门禁上的硬约束（Rust 单测只能锁 Rust 内部，
前端单测只能锁前端自身，跨语言这层必须有脚本）。

执行：python3 -m pytest tests-audit/audit_error_codes.py -v
"""

from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parent.parent
ERROR_RS = (ROOT / "src-tauri/src/error.rs").read_text()
ERROR_HANDLER_TS = (ROOT / "src/lib/errorHandler.ts").read_text()

ENUM_HEADER = "pub enum CommandErrorCode {"
TS_TYPE_HEADER = "export type CommandErrorCode ="


def rust_codes():
    """按声明顺序取 CommandErrorCode 各变体的 serde rename 字符串"""
    start = ERROR_RS.index(ENUM_HEADER) + len(ENUM_HEADER)
    end = ERROR_RS.index("\n}", start)
    body = ERROR_RS[start:end]
    # #[serde(rename = "X")] 后跟变体名；顺序即声明顺序
    return re.findall(r'#\[serde\(rename = "([A-Z_]+)"\)\]', body)


def ts_codes():
    """取前端 CommandErrorCode 联合类型的字面量（按书写顺序）"""
    start = ERROR_HANDLER_TS.index(TS_TYPE_HEADER) + len(TS_TYPE_HEADER)
    end = ERROR_HANDLER_TS.index(";", start)
    body = ERROR_HANDLER_TS[start:end]
    return re.findall(r'"([A-Z_]+)"', body)


def ts_all_codes_const():
    """取 ALL_COMMAND_ERROR_CODES 数组里的字面量（顺序与 Rust ALL 同源）"""
    start = ERROR_HANDLER_TS.index("export const ALL_COMMAND_ERROR_CODES")
    # 注意：类型注解 `readonly CommandErrorCode[]` 里也有 `[]`，
    # 必须锚定赋值号后的 `= [` 才是数组起点
    start = ERROR_HANDLER_TS.index("= [", start) + 2
    end = ERROR_HANDLER_TS.index("]", start)
    return re.findall(r'"([A-Z_]+)"', ERROR_HANDLER_TS[start:end])


def test_rust_enum_has_codes_and_no_duplicates():
    codes = rust_codes()
    assert codes, "error.rs 未解析到任何 CommandErrorCode（rename 写法变了吗？）"
    assert len(codes) == len(set(codes)), f"Rust 枚举存在重复 code: {codes}"


def test_frontend_union_matches_rust_enum():
    rust, ts = rust_codes(), ts_codes()
    assert sorted(rust) == sorted(ts), (
        "CommandErrorCode 前后端不一致：\n"
        f"  Rust 独有: {sorted(set(rust) - set(ts))}\n"
        f"  前端独有: {sorted(set(ts) - set(rust))}\n"
        "（Rust 侧改 src-tauri/src/error.rs，前端同步 src/lib/errorHandler.ts）"
    )


def test_frontend_all_codes_const_matches_union():
    ts_union, ts_all = ts_codes(), ts_all_codes_const()
    assert sorted(ts_all) == sorted(ts_union), (
        "ALL_COMMAND_ERROR_CODES 与联合类型不一致：\n"
        f"  联合独有: {sorted(set(ts_union) - set(ts_all))}\n"
        f"  数组独有: {sorted(set(ts_all) - set(ts_union))}"
    )
    assert len(ts_all) == len(set(ts_all)), "ALL_COMMAND_ERROR_CODES 存在重复项"


def test_declaration_order_matches():
    """顺序漂移不致命，但两边对齐更易比对；真出现不一致时给出明确提示"""
    rust, ts = rust_codes(), ts_codes()
    if rust != ts:
        pytest.fail(
            "CommandErrorCode 声明顺序前后端不一致（集合一致但顺序不同）：\n"
            f"  Rust: {rust}\n"
            f"  前端: {ts}\n"
            "顺带把两边顺序对齐，方便人工核对与 grep。"
        )
