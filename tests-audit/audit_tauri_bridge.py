#!/usr/bin/env python3
"""
Tauri 桥双向一致性检查（阶段 5 规则固化，风格同 audit_pre_step_pre_execute.py）：

- 命令：lib.rs invoke_handler! 注册集合 ↔ src/ 前端 invoke("...") 调用集合
  - 前端调了未注册 → FAIL（真 bug：运行时才炸）
  - plugin:xxx|yyy 调用 → 核对 lib.rs 有 tauri_plugin_xxx 的 .plugin(...) 注册
  - 注册了但前端不调 → 仅 WARN（可能有正当内部/测试用途）
- 事件：后端 emit/emit_to + 前端 emit/emitTo 的事件名 ↔ 前端 listen(...) 的事件名
  - 听而无发 → FAIL；发而无听 → 仅 WARN

执行：python3 -m pytest tests-audit/audit_tauri_bridge.py -v
"""
from pathlib import Path
import re

import pytest

ROOT = Path(__file__).resolve().parent.parent
LIB_RS = (ROOT / "src-tauri/src/lib.rs").read_text()
RUST_SRC = {
    str(p.relative_to(ROOT)): p.read_text()
    for p in (ROOT / "src-tauri/src").glob("**/*.rs")
}
TS_SRC = {
    str(p.relative_to(ROOT)): p.read_text()
    for p in (ROOT / "src").glob("**/*")
    if p.suffix in (".ts", ".tsx")
}


def registered_commands():
    """invoke_handler(tauri::generate_handler![ ... ]) 里的命令名（取路径末段）"""
    m = re.search(r"generate_handler!\[(.*?)\]\s*\)", LIB_RS, re.S)
    assert m, "lib.rs 未找到 invoke_handler generate_handler! 块"
    cmds = set()
    for entry in m.group(1).split(","):
        entry = entry.strip()
        if entry:
            cmds.add(entry.rsplit("::", 1)[-1])
    return cmds


def frontend_invokes():
    """src/ 里 invoke("...") / invoke<T>("...") 的字面量命令名"""
    out = {}  # name -> file
    for f, t in TS_SRC.items():
        for m in re.finditer(r'\binvoke(?:<[^>]*>)?\(\s*"([^"]+)"', t):
            out.setdefault(m.group(1), f)
    return out


def plugin_calls():
    """src/ 里全部 "plugin:xxx|yyy" 字面量（含三元表达式里的动态分支）"""
    out = {}
    for f, t in TS_SRC.items():
        for m in re.finditer(r'"(plugin:[a-z0-9-]+)\|[a-z_]+"', t):
            out.setdefault(m.group(1), f)
    return out


def registered_plugins():
    """lib.rs .plugin(tauri_plugin_xxx::init(...)) 注册集合（plugin: 名用连字符）"""
    return {
        m.group(1).replace("_", "-")
        for m in re.finditer(r"tauri_plugin_(\w+)::init", LIB_RS)
    }


def emitted_events():
    """后端 .emit(\"e\")/.emit_to(\"w\",\"e\") + 闭包 emit(\"e\") 与前端 emit/emitTo 的事件名"""
    backend = {}
    for f, t in RUST_SRC.items():
        for m in re.finditer(r'\.emit\(\s*"([^"]+)"', t):
            backend.setdefault(m.group(1), f)
        for m in re.finditer(r'\.emit_to\(\s*"[^"]*"\s*,\s*"([^"]+)"', t):
            backend.setdefault(m.group(1), f)
        # bot_model_loop 等处的注入闭包：let emit = |event, payload| ... emit("e", ...)
        for m in re.finditer(r'(?<![\w.])emit\(\s*"([^"]+)"', t):
            backend.setdefault(m.group(1), f)
    frontend = {}
    for f, t in TS_SRC.items():
        for m in re.finditer(r'(?<![\w.])emit(?:To)?\(\s*"([^"]+)"', t):
            frontend.setdefault(m.group(1), f)
    return backend, frontend


def listened_events():
    out = {}
    for f, t in TS_SRC.items():
        for m in re.finditer(r'\blisten(?:<[^>]*>)?\(\s*"([^"]+)"', t):
            out.setdefault(m.group(1), f)
    return out


# ───────────────────────── 命令一致性 ─────────────────────────


def test_frontend_invokes_all_registered():
    registered = registered_commands()
    bad = {
        name: f
        for name, f in frontend_invokes().items()
        if not name.startswith("plugin:") and name not in registered
    }
    assert not bad, "前端调用了未注册的 Tauri 命令：" + "; ".join(
        f"{name}（{f}）" for name, f in sorted(bad.items())
    )


def test_plugin_calls_have_registered_plugin():
    plugins = registered_plugins()
    bad = {
        name: f
        for name, f in plugin_calls().items()
        if name.removeprefix("plugin:") not in plugins
    }
    assert not bad, "前端调用了未在 lib.rs 注册的插件：" + "; ".join(
        f"{name}（{f}）" for name, f in sorted(bad.items())
    )


def test_registered_but_uncalled_warn_only():
    registered = registered_commands()
    called = set(frontend_invokes())
    unused = sorted(registered - called)
    if unused:
        import warnings

        warnings.warn(
            "已注册但前端未调用的命令（仅提示，可能有内部/测试用途）："
            + ", ".join(unused),
            stacklevel=1,
        )


# ───────────────────────── 事件一致性 ─────────────────────────


def test_listened_events_have_emitter():
    backend, frontend = emitted_events()
    emitters = set(backend) | set(frontend)
    bad = {e: f for e, f in listened_events().items() if e not in emitters}
    assert not bad, "前端 listen 了无人 emit 的事件：" + "; ".join(
        f"{e}（{f}）" for e, f in sorted(bad.items())
    )


def test_backend_emits_have_listener_warn_only():
    backend, _ = emitted_events()
    listened = set(listened_events())
    unused = sorted(set(backend) - listened)
    if unused:
        import warnings

        warnings.warn(
            "后端 emit 但前端无人 listen 的事件（仅提示）：" + ", ".join(unused),
            stacklevel=1,
        )


# ───────────────────────── 自检（提取器本身不空转） ─────────────────────────


def test_extractors_nonempty():
    assert len(registered_commands()) >= 50, "命令提取异常（当前 55 个注册）"
    assert len(frontend_invokes()) >= 50, "前端 invoke 提取异常"
    assert plugin_calls(), "plugin 调用提取异常"
    backend, frontend = emitted_events()
    assert backend and frontend, "事件 emit 提取异常"
    assert listened_events(), "事件 listen 提取异常"
