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


# 泛型参数支持两层嵌套：invoke<T> / invoke<Promise<T>> / invoke<Record<string, number>>
INVOKE_RE = re.compile(r'\binvoke(?:<(?:[^<>]|<[^<>]*>)*>)?\(\s*"([^"]+)"')


def invoke_names(text):
    """单段文本里 invoke("...") / invoke<T>("...") 的字面量命令名"""
    return [m.group(1) for m in INVOKE_RE.finditer(text)]


# 本地包装函数：签名形如 `const NAME = useCallback((FIRST: string, ...) => { invoke(FIRST, ...) })`
# 把命令名以运行期参数传入 invoke 时，需要在调用点提取字面量（前端目前全是直接
# invoke，此机制是为将来出现包装形态留的网，眼下无调用方）。
# 形态检测：(1) 首参类型为 string；(2) body 内确实调用 invoke(FIRST_PARAM, ...)
# 第二个条件用括号配平扫描函数体，try/catch/嵌套闭包/await chain 都能跨过。
_WRAPPER_SIG_RE = re.compile(
    r'(?:const|let|var)\s+(\w+)\s*=\s*(?:useCallback\s*\()?\s*'
    r'(?:async\s+)?\(?\s*'
    r'(\w+)\s*:\s*string\b',
    re.DOTALL,
)


def _find_arrow_body(text, start):
    """从签名尾部起找箭头函数的 body 范围（平衡花括号）。返回 (body_start, body_end) 或 None。"""
    i = start
    paren_depth = 0
    while i < len(text):
        c = text[i]
        if c == '(':
            paren_depth += 1
        elif c == ')':
            paren_depth -= 1
            if paren_depth < 0:
                # 越过参数结束：跳过类型注解/空白，找 =>
                j = i + 1
                while j < len(text) and text[j].isspace():
                    j += 1
                # 跳过返回类型 : Promise<X>
                if j < len(text) and text[j] == ':':
                    # 平衡 < > 跳到下一个顶层 token
                    depth = 0
                    j += 1
                    while j < len(text):
                        if text[j] == '<':
                            depth += 1
                        elif text[j] == '>':
                            depth -= 1
                        elif text[j] in '( {' and depth == 0:
                            break
                        j += 1
                    while j < len(text) and text[j].isspace():
                        j += 1
                # 现在应看到 =>
                arrow = text.find('=>', j)
                if arrow == -1 or arrow > j + 60:
                    return None
                k = arrow + 2
                while k < len(text) and text[k].isspace():
                    k += 1
                if k >= len(text) or text[k] != '{':
                    # 表达式体不展开（风险大），跳过
                    return None
                # 平衡花括号
                depth = 1
                end = k + 1
                while end < len(text) and depth > 0:
                    cc = text[end]
                    if cc == '{':
                        depth += 1
                    elif cc == '}':
                        depth -= 1
                    end += 1
                return (k, end) if depth == 0 else None
        i += 1
    return None


def discover_invoke_wrappers(text):
    """找出文本中所有把命令名作为首参传给 invoke 的本地包装函数名。
    例：`const runCmd = useCallback((cmd: string, args) => { invoke(cmd, args) })`
    返回 wrapper 名字集合（按文件级扫描，每个 wrapper 自己负责检查 body）。"""
    wrappers = set()
    for m in _WRAPPER_SIG_RE.finditer(text):
        name, first_param = m.group(1), m.group(2)
        body_range = _find_arrow_body(text, m.end())
        if not body_range:
            continue
        body = text[body_range[0]:body_range[1]]
        # body 内必须真的 invoke(first_param, ...)——避免误把同名局部函数当包装器
        if re.search(rf'\binvoke\(\s*{re.escape(first_param)}\s*,', body):
            wrappers.add(name)
    return wrappers


def wrapper_call_names(text, wrappers):
    """扫所有 `WRAPPER_NAME("literal", ...)` 调用，提取首参字面量（命令名）。"""
    if not wrappers:
        return []
    alt = '|'.join(re.escape(w) for w in wrappers)
    pattern = re.compile(rf'\b(?:{alt})\(\s*"([^"]+)"')
    return [m.group(1) for m in pattern.finditer(text)]


def frontend_invokes():
    """src/ 里 invoke("...") / invoke<T>("...") 的字面量命令名
    + 通过包装函数（如 runCmd）间接调 invoke 的命令名（首参是字面量）"""
    out = {}  # name -> file
    for f, text in TS_SRC.items():
        for name in invoke_names(text):
            out.setdefault(name, f)
        wrappers = discover_invoke_wrappers(text)
        for name in wrapper_call_names(text, wrappers):
            out.setdefault(name, f)
    return out


def plugin_calls():
    """src/ 里全部 "plugin:xxx|yyy" 字面量（含三元表达式里的动态分支）"""
    out = {}
    for f, t in TS_SRC.items():
        for m in re.finditer(r'"(plugin:[a-z0-9-]+)\|[a-z_]+"', t):
            out.setdefault(m.group(1), f)
    return out


def registered_plugins():
    """lib.rs .plugin(tauri_plugin_xxx::init(...)/::Builder::new()) 注册集合
    （plugin: 名用连字符；Builder 形态如 global_shortcut::Builder::new()）"""
    return {
        m.group(1).replace("_", "-")
        for m in re.finditer(r"tauri_plugin_(\w+)::(?:init|Builder)\b", LIB_RS)
    }


def strip_rs_comments(text: str) -> str:
    """剥 // 与 /* */ 注释（字符串字面量原样保留，emit 字面量不受影响；
    字符串内的 // 不启注释）。注释掉的 emit 不再算后端事件。"""
    out = []
    i, n = 0, len(text)
    state = None  # None | "line" | "block" | 引号字符
    while i < n:
        c = text[i]
        two = text[i:i + 2]
        if state == "line":
            out.append("\n" if c == "\n" else " ")
            if c == "\n":
                state = None
            i += 1
        elif state == "block":
            out.append("\n" if c == "\n" else " ")
            if two == "*/":
                out.append("  ")
                i += 2
                state = None
            else:
                i += 1
        elif state == '"':
            if c == "\\":
                out.append("  ")
                i += 2
            else:
                out.append(c)
                if c == '"':
                    state = None
                i += 1
        else:
            if two == "//":
                state = "line"
                out.append("  ")
                i += 2
            elif two == "/*":
                state = "block"
                out.append("  ")
                i += 2
            elif c == '"':
                state = '"'
                out.append(c)
                i += 1
            else:
                out.append(c)
                i += 1
    return "".join(out)


def emitted_events():
    """后端 .emit(\"e\")/.emit_to(\"w\",\"e\") + 闭包 emit(\"e\") 与前端 emit/emitTo 的事件名
    （Rust 侧先剥注释，注释掉的 emit 不算）"""
    backend = {}
    for f, raw in RUST_SRC.items():
        t = strip_rs_comments(raw)
        for m in re.finditer(r'\.emit\(\s*"([^"]+)"', t):
            backend.setdefault(m.group(1), f)
        for m in re.finditer(r'\.emit_to\(\s*"[^"]*"\s*,\s*"([^"]+)"', t):
            backend.setdefault(m.group(1), f)
        # bot_model_loop 等处的注入闭包：let emit = |event, payload| ... emit("e", ...)
        for m in re.finditer(r'(?<![\w.])emit\(\s*"([^"]+)"', t):
            backend.setdefault(m.group(1), f)
        # emit(CONST_NAME, ...) / app.emit(CONST_NAME, ...) 字面量常量名：去
        # src-tauri/src/**/*.rs 找 `pub const NAME: &str = "value"`（剥注释后扫）
        for m in re.finditer(r'\bemit\(\s*([A-Z][A-Z0-9_]+)\b', t):
            const_name = m.group(1)
            resolved = resolve_rust_str_const(const_name)
            if resolved:
                backend.setdefault(resolved, f)
    frontend = {}
    for f, t in TS_SRC.items():
        for m in re.finditer(r'(?<![\w.])emit(?:To)?\(\s*"([^"]+)"', t):
            frontend.setdefault(m.group(1), f)
    return backend, frontend


def listened_events():
    out = {}
    for f, t in TS_SRC.items():
        # 直接 listen<T>("evt", ...) 形式
        for m in re.finditer(r'\blisten(?:<[^>]*>)?\(\s*"([^"]+)"', t):
            out.setdefault(m.group(1), f)
        # useTauriListen<T>("evt", ...) 包装钩子：事件名是首参字面量
        for m in re.finditer(r'\buseTauriListen(?:<[^>]*>)?\(\s*"([^"]+)"', t):
            out.setdefault(m.group(1), f)
        # useTauriListen<T>(CONST_NAME, ...) 字面量常量名：去对应 lib 文件查 const 值
        for m in re.finditer(r'\buseTauriListen(?:<[^>]*>)?\(\s*([A-Z][A-Z0-9_]+)\b', t):
            const_name = m.group(1)
            resolved = resolve_string_constant(const_name)
            if resolved:
                out.setdefault(resolved, f)
    return out


_STRING_CONST_CACHE = {}
_RUST_CONST_CACHE = {}


def resolve_string_constant(name):
    """在 src/lib/*.ts 里找 `export const NAME = "value";` 的字面量值；找不到返回 None。"""
    if name in _STRING_CONST_CACHE:
        return _STRING_CONST_CACHE[name]
    pattern = re.compile(rf'^export\s+const\s+{re.escape(name)}\s*=\s*"([^"]+)"', re.MULTILINE)
    for lib in (ROOT / "src" / "lib").glob("*.ts"):
        m = pattern.search(lib.read_text())
        if m:
            _STRING_CONST_CACHE[name] = m.group(1)
            return m.group(1)
    _STRING_CONST_CACHE[name] = None
    return None


def resolve_rust_str_const(name):
    """在 src-tauri/src/**/*.rs 里找 `pub const NAME: &str = "value";` 字面量值；找不到返回 None。"""
    if name in _RUST_CONST_CACHE:
        return _RUST_CONST_CACHE[name]
    pattern = re.compile(rf'^pub\s+const\s+{re.escape(name)}\s*:\s*&str\s*=\s*"([^"]+)"', re.MULTILINE)
    for f in (ROOT / "src-tauri" / "src").rglob("*.rs"):
        m = pattern.search(f.read_text())
        if m:
            _RUST_CONST_CACHE[name] = m.group(1)
            return m.group(1)
    _RUST_CONST_CACHE[name] = None
    return None


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


def test_invoke_names_nested_generics():
    """invoke 泛型提取：无泛型 / 单层 / 双层嵌套均能拿到命令名"""
    assert invoke_names('await invoke("cmd_a")') == ["cmd_a"]
    assert invoke_names('invoke<T>("cmd_b")') == ["cmd_b"]
    assert invoke_names('invoke<Promise<T>>("cmd_c")') == ["cmd_c"]
    assert invoke_names('invoke<Record<string, number>>("cmd_d")') == ["cmd_d"]
    # 非字面量参数不误报
    assert invoke_names('invoke(name, { a: 1 })') == []


def test_invoke_names_real_sample_memory_tuning_get():
    """真实样本：MemoryPanel.tsx 的双层泛型调用曾被旧正则漏抓（实际已注册）"""
    assert "memory_tuning_get" in frontend_invokes()


def test_registered_plugins_builder_form():
    """Builder 形态注册须识别（真实样本：lib.rs tauri_plugin_global_shortcut::Builder::new()）"""
    assert "global-shortcut" in registered_plugins()


def test_emitted_events_ignores_commented_emit():
    """注释掉的 emit 不算后端事件；字符串里的 // 不误吞注释"""
    cleaned = strip_rs_comments('// .emit("ghost_a");\n/* .emit_to("w", "ghost_b") */\nx.emit("real_c");')
    names = [m.group(1) for m in re.finditer(r'\.emit\(\s*"([^"]+)"', cleaned)]
    assert names == ["real_c"]
    # URL 里的 // 不启行注释（字符串内容原样保留）
    assert 'emit("e", "https://a.dev")' == strip_rs_comments('emit("e", "https://a.dev")')


def test_extractors_nonempty():
    assert len(registered_commands()) >= 50, "命令提取异常（当前 55 个注册）"
    assert len(frontend_invokes()) >= 50, "前端 invoke 提取异常"
    assert plugin_calls(), "plugin 调用提取异常"
    backend, frontend = emitted_events()
    assert backend and frontend, "事件 emit 提取异常"
    assert listened_events(), "事件 listen 提取异常"


# ───────────────────────── 包装函数发现 ─────────────────────────


def test_discover_invoke_wrappers_basic():
    """典型形态：const NAME = useCallback((cmd: string, args) => { invoke(cmd, args) })"""
    src = '''
const runCmd = useCallback(
  async (cmd: string, args: Record<string, unknown>): Promise<boolean> => {
    try {
      await invoke(cmd, args);
      return true;
    } catch (e) {
      return false;
    }
  },
  []
);
'''
    assert discover_invoke_wrappers(src) == {"runCmd"}


def test_discover_invoke_wrappers_no_invoke_in_body():
    """首参 typed string 但 body 内不 invoke(FIRST, ...) → 不是包装器"""
    src = '''
const format = (s: string, prefix: string) => {
  return prefix + s.toUpperCase();
};
'''
    assert discover_invoke_wrappers(src) == set()


def test_discover_invoke_wrappers_wrong_param_name():
    """invoke 调用的不是首参 → 不是包装器"""
    src = '''
const buggy = useCallback(async (a: string, b: string) => {
  await invoke(b, {});
});
'''
    assert discover_invoke_wrappers(src) == set()


def test_wrapper_call_names_extracts_literals():
    """`runCmd("literal", ...)` 字面量首参全部提取（合成输入，命令名取中性名）"""
    src = '''
await runCmd("alpha_cmd", { id: 1 });
await runCmd("beta_cmd", { id: 2 });
// 非字面量跳过：
const name = "dynamic";
runCmd(name, {});
'''
    assert sorted(wrapper_call_names(src, {"runCmd"})) == [
        "alpha_cmd",
        "beta_cmd",
    ]


def test_wrapper_call_names_no_match():
    """空 wrappers 集合 → 立即返回空列表（不要白做工）"""
    assert wrapper_call_names("runCmd('bar')", set()) == []


def test_frontend_invokes_picks_up_real_commands():
    """端到端——frontend_invokes 必须抓全 src/ 里的真实命令并映射到文件。

    当前前端没有 `invoke(cmd, ...)` 包装函数，直接调 invoke 是唯一形态；
    包装函数探测是为将来出现的形态留的通用网，眼下无调用方也保留。
    """
    invokes = frontend_invokes()
    assert len(invokes) > 100, "前端命令面被意外清空"
    assert invokes["apps_list_active"] == "src/lib/apps.ts"
    assert invokes["mem_delete"] == "src/components/SettingsPage/MemoryPanel.tsx"
    for name, path in invokes.items():
        assert path.endswith((".ts", ".tsx")), f"{name} 映射到非前端文件 {path}"
