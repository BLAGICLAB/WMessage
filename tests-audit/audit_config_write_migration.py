#!/usr/bin/env python3
"""
配置写路径 × MCP 机密迁移不变量对拍：

任何调用 write_bot_config_file_locked（配置落盘唯一内核）的生产函数，其函数体
必须先调用 migrate_mcp_server_secrets_locked——skip_serializing 使任何写回都会
把文件里未迁移的明文 env/headers 剥掉，keyring 无副本即永久丢失（2026-11-16
审计 P1-1：bot_set_config / bot_set_active_model / bot_set_perm_mode /
builtin::materialize_at_startup 四条路径漏迁移）。

机械口径：扫描每个含 `write_bot_config_file_locked(` 调用的函数体（花括号配平），
断言体内出现 `migrate_mcp_server_secrets_locked(`。`#[cfg(test)]` 模块跳过
（测试直调内核注入凭据存储桩，无 app 可传，迁移由生产调用方保证）。

执行：python3 -m pytest tests-audit/audit_config_write_migration.py -v
"""
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src-tauri/src"
WRITE_FN = "write_bot_config_file_locked"
MIGRATE_FN = "migrate_mcp_server_secrets_locked"


def strip_line_comments(text: str) -> str:
    """剥 // 行注释（避免把注释里的符号误判为调用）。字符串里的 // 少见且
    本扫描只找函数名，误剥不影响结论。"""
    out = []
    for line in text.splitlines():
        i = line.find("//")
        out.append(line if i < 0 else line[:i])
    return "\n".join(out)


def function_bodies(text: str):
    """产出 (fn_name, body_text)。按 `fn NAME(` 起点做花括号配平截取函数体。"""
    text = strip_line_comments(text)
    for m in __import__("re").finditer(r"fn\s+(\w+)\s*[<(]", text):
        name = m.group(1)
        brace = text.find("{", m.end())
        if brace < 0:
            continue
        depth = 0
        end = None
        for i in range(brace, len(text)):
            c = text[i]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    end = i
                    break
        if end is not None:
            yield name, text[m.start():end]


def split_test_modules(text: str):
    """按 #[cfg(test)] 切开：返回 (生产代码, 测试代码)。"""
    idx = text.find("#[cfg(test)]")
    if idx < 0:
        return text, ""
    return text[:idx], text[idx:]


@pytest.fixture(scope="module")
def violations():
    bad = []
    for path in SRC.glob("**/*.rs"):
        if ".mimosa" in str(path):
            continue
        prod, _ = split_test_modules(path.read_text())
        for fn_name, body in function_bodies(prod):
            if WRITE_FN not in body:
                continue
            # 内核自身排除：write_bot_config_file_locked（薄壳转发）与
            # write_bot_config_file_locked_with（签名行含 WRITE_FN 子串）
            if fn_name.startswith(WRITE_FN):
                continue
            if MIGRATE_FN not in body:
                bad.append(f"{path.relative_to(ROOT)}: fn {fn_name} 调用 {WRITE_FN} 但函数体无 {MIGRATE_FN}")
    return bad


def test_every_config_write_path_migrates_mcp_secrets_first(violations):
    assert not violations, (
        "配置写路径缺 MCP 机密迁移前置（skip_serializing 写回即剥明文）：\n"
        + "\n".join(violations)
    )


def test_migration_is_invoked_at_startup_and_mcp_commands():
    """启动迁移（lib.rs）与 MCP 写路径迁移（mcp/commands.rs）两处显式调用点必须存在。"""
    lib = (SRC / "lib.rs").read_text()
    assert f"{MIGRATE_FN}(" in lib, "lib.rs 启动迁移缺失"
    mcp_cmds = (SRC / "bot/mcp/commands.rs").read_text()
    assert f"{MIGRATE_FN}(" in mcp_cmds, "mcp/commands.rs 写路径迁移缺失"
