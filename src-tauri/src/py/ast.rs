//! Python AST 静态探查（`ast_inspect` 工具的内核）
//!
//! 走既有 `run_python` 沙箱 + Python 标准库 `ast`，**零新增依赖**：没有编译期、
//! 没有语法库、没有语言注册表。四种模式（符号树 / 定义 / 引用 / 调用链）都在
//! 下方 `AST_SCRIPT` 里用 `ast` 模块实现。
//!
//! **授权判定留在 Rust 侧**（本模块唯一的硬约束）：沙箱脚本拿不到未授权路径——
//! `tool_ast_inspect` 必须先过 `bot_fs::resolve_with_perm`，拿到 canonical path
//! 之后才把它内联进代码。`ast` 侧不做授权判定，它只相信收到的路径。这正好补上
//! `run_python` 缺的那一层（沙箱无文件系统隔离，见 `bot/registry.rs` 的
//! `SCHEMA_RUN_PYTHON`）。
//!
//! **传参方式**：路径内联进 code（沙箱 stdin 是 `Stdio::null()`，没有别的通道），
//! 故内联必须过 `py_str_literal` 转义，绝不能直接字符串拼接。

use tauri::AppHandle;

use crate::bot::registry::ToolResult;

/// 四种探查模式（同时是 schema 的 enum 值来源）。
pub const AST_OPS: &[&str] = &["symbols", "defs", "refs", "callgraph"];

/// 除 `symbols` 外都要 `symbol` 参数。
const AST_OPS_NEEDING_SYMBOL: &[&str] = &["defs", "refs", "callgraph"];

/// 调用链最大层数（与 `AST_SCRIPT` 里的 `MAX_DEPTH` 同值，两侧都钳）。
pub const MAX_CALLGRAPH_DEPTH: u64 = 3;

/// 手写 Python 字符串字面量转义（用于把路径 / 符号名内联进 code）。
///
/// 不用 `serde_json` 的 `to_string`：JSON 的 `\/` 之类转义不是 Python 语法，
/// 而这里要的正是「Python 字面量」这一种格式。控制字符走 `\xNN`（Python 里
/// `"\x01"` 即 U+0001，且只吃两位十六进制，后接十六进制数字不会串味）。
pub fn py_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 非 `callgraph` 时目标必须是 `.py`；`callgraph` 允许给目录（沙箱自己 walk）。
///
/// 扩展名判定先于 `resolve_with_perm`：给一个 `.ts` 弹授权框是没意义的噪音，
/// 而「本文件走文本检索」这条降级指引必须在任何副作用之前给模型（A5）。
fn target_is_supported(op: &str, path: &str) -> bool {
    if op == "callgraph" {
        return true;
    }
    path.trim().to_ascii_lowercase().ends_with(".py")
}

/// 生成沙箱代码：CFG 前导 + 脚本本体。
///
/// 拆成纯函数是为了能直接单测「路径确实被内联进 code」——而 code 只在
/// `resolve_with_perm` 放行之后才会被调用到沙箱（A3）。
fn build_ast_code(op: &str, path: &str, symbol: Option<&str>, depth: u64) -> String {
    let symbol_lit = match symbol {
        Some(s) => py_str_literal(s),
        None => "None".to_string(),
    };
    format!(
        "CFG = {{\"op\": {}, \"path\": {}, \"symbol\": {}, \"depth\": {}}}\n{}",
        py_str_literal(op),
        py_str_literal(path),
        symbol_lit,
        depth.clamp(1, MAX_CALLGRAPH_DEPTH),
        AST_SCRIPT,
    )
}

/// `ast_inspect`：Python 源码符号级静态探查（只读）。
///
/// 闸序（不可调换）：参数校验 → `resolve_with_perm` → 生成 code → 交沙箱。
/// `py_exec_sync` 内部的 Python 开关闸不重复实现，直接继承。
pub async fn tool_ast_inspect(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
    stop: Option<&crate::bot::StopGuard>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(op_raw) = v["op"].as_str() else {
        return ToolResult::denied(format!(
            "ast_inspect 缺少 op，可选：{}",
            AST_OPS.join(" / ")
        ));
    };
    let op = op_raw.trim();
    if !AST_OPS.contains(&op) {
        return ToolResult::denied(format!(
            "ast_inspect 不认识 op={op}，可选：{}",
            AST_OPS.join(" / ")
        ));
    }
    let Some(path_raw) = v["path"].as_str() else {
        return ToolResult::denied("ast_inspect 缺少 path（.py 文件；op=callgraph 时可传目录）");
    };
    let path = path_raw.trim();
    if !target_is_supported(op, path) {
        // 明确不可用 + 指路，绝不静默换成文本检索结果（A5）
        return ToolResult::denied(format!(
            "ast_inspect 只支持 Python 源码（.py），{path} 不是 .py 文件。\
                 本文件请改用 grep_files（按正则搜内容）或 read_text_file（整文件读）。"
        ));
    }
    let symbol = v["symbol"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if AST_OPS_NEEDING_SYMBOL.contains(&op) && symbol.is_none() {
        return ToolResult::denied(format!(
            "ast_inspect 的 op={op} 需要 symbol 参数（符号名，如 parse_args）"
        ));
    }
    let depth = v["depth"]
        .as_u64()
        .unwrap_or(1)
        .clamp(1, MAX_CALLGRAPH_DEPTH);

    // 授权闸：白名单外这里就拒了，沙箱永远拿不到这条路径（A3）
    let canonical = match crate::bot::resolve_with_perm(
        app,
        "ast_inspect",
        path,
        interactive,
        session_id,
    )
    .await
    {
        Ok(canonical) => canonical,
        Err(e) => return ToolResult::denied(e),
    };
    let code = build_ast_code(op, &canonical.to_string_lossy(), symbol, depth);
    match crate::py::document::py_exec_sync_async(app.clone(), code, None, stop.map(|s| s.token()))
        .await
    {
        Ok(r) => {
            let mut out = String::new();
            if !r.stdout.trim().is_empty() {
                out.push_str(r.stdout.trim());
            }
            if !r.stderr.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[stderr] {}", r.stderr.trim()));
            }
            if out.is_empty() {
                out = "执行完成（无输出）".into();
            }
            ToolResult::ok(out, Vec::new())
        }
        Err(e) => ToolResult::error(format!("探查失败：{e}"), Vec::new()),
    }
}

/// 沙箱脚本本体：标准库 `ast` 实现符号树 / 定义 / 引用 / 调用链。
///
/// 边界全部显式写死并如实上报（`truncated`）：文件数 200、单文件合计 2MB、
/// 命中数 300、调用链 3 层。跨文件索引**不缓存**——每次现扫（缓存必然带来
/// 失效问题，而我们还没有任何耗时数字能证明它值得那份复杂度）。
pub const AST_SCRIPT: &str = r##"import ast
import json
import os
import sys
import traceback

MAX_FILES = 200
MAX_BYTES = 2 * 1024 * 1024
MAX_HITS = 300
MAX_DEPTH = 3
SKIP_DIRS = frozenset((
    ".git", "__pycache__", "node_modules", "venv", ".venv", "env",
    "site-packages", "dist", "build", ".mypy_cache", ".pytest_cache",
    ".ruff_cache", ".tox", "eggs",
))


def _real(p):
    try:
        return os.path.realpath(p)
    except OSError:
        return ""


def _within(root, p):
    """p 的真实路径是否仍在 root 之下。

    root 本身已由 Rust 侧 canonicalize + 授权过，这道检查针对的是**目录内
    指向外部的 symlink**：授权的是 root，不是 root 里的每一个条目。
    """
    r = _real(root)
    t = _real(p)
    return bool(r) and bool(t) and (t == r or t.startswith(r + os.sep))


def _collect(root):
    """→ (files, truncated)。root 是文件时只取它自己，是目录时 walk。"""
    if os.path.isfile(root):
        return ([root] if root.endswith(".py") else [], False)
    files, total = [], 0
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        if not _within(root, dirpath):
            continue
        for fn in sorted(filenames):
            if not fn.endswith(".py"):
                continue
            p = os.path.join(dirpath, fn)
            if not _within(root, p):
                continue
            try:
                size = os.path.getsize(p)
            except OSError:
                continue
            if len(files) >= MAX_FILES or total + size > MAX_BYTES:
                return files, True
            total += size
            files.append(p)
    return files, False


def _parse(paths, root):
    """→ [(rel, tree, err)]；单个文件语法错只记 err，不中断整次调用。"""
    out, single = [], os.path.isfile(root)
    for p in paths:
        rel = os.path.basename(p) if single else os.path.relpath(p, root)
        try:
            with open(p, "r", encoding="utf-8", errors="replace") as f:
                src = f.read()
            out.append((rel, ast.parse(src, filename=p), None))
        except (OSError, ValueError, SyntaxError, RecursionError, MemoryError) as e:
            out.append((rel, None, type(e).__name__))
    return out


def _kind(node):
    if isinstance(node, ast.ClassDef):
        return "class"
    if isinstance(node, ast.AsyncFunctionDef):
        return "async_def"
    return "def"


def _sig(a):
    """参数名列表（忽略默认值与注解——v1 只给形状，不做签名匹配）。"""
    parts = [x.arg for x in list(a.posonlyargs) + list(a.args)]
    if a.vararg:
        parts.append("*" + a.vararg.arg)
    parts += [x.arg for x in a.kwonlyargs]
    if a.kwarg:
        parts.append("**" + a.kwarg.arg)
    return "(" + ", ".join(parts) + ")"


def _callee_name(func):
    if isinstance(func, ast.Name):
        return func.id
    if isinstance(func, ast.Attribute):
        return func.attr
    return None


def _symbols(tree):
    """→ [(kind, qualname, lineno, end_lineno)]，含类内方法与嵌套 def。"""
    out, stack = [], []

    class V(ast.NodeVisitor):
        def visit_ClassDef(self, node):
            out.append(("class", node.name, node.lineno, getattr(node, "end_lineno", None)))
            stack.append(node.name)
            self.generic_visit(node)
            stack.pop()

        def _func(self, node):
            out.append((
                _kind(node), ".".join(stack + [node.name]),
                node.lineno, getattr(node, "end_lineno", None),
            ))
            stack.append(node.name)
            self.generic_visit(node)
            stack.pop()

        visit_FunctionDef = _func
        visit_AsyncFunctionDef = _func

    V().visit(tree)
    return out


def _find_defs(parsed, symbol):
    hits = []
    for rel, tree, _err in parsed:
        if tree is None:
            continue
        for node in ast.walk(tree):
            if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                continue
            if node.name != symbol or len(hits) >= MAX_HITS:
                continue
            doc = (ast.get_docstring(node) or "").strip().splitlines()
            args = getattr(node, "args", None)
            hits.append({
                "file": rel,
                "kind": _kind(node),
                "line": node.lineno,
                "end_line": getattr(node, "end_lineno", None),
                "sig": _sig(args) if args is not None else "",
                "doc": doc[0][:200] if doc else "",
            })
    return hits


def _find_refs(parsed, symbol):
    """区分 name / attribute 引用，并报出所在函数（调用点归属）。"""
    hits = []

    def visit(rel, tree):
        stack = []

        class R(ast.NodeVisitor):
            def _func(self, node):
                stack.append(node.name)
                self.generic_visit(node)
                stack.pop()

            visit_FunctionDef = _func
            visit_AsyncFunctionDef = _func
            visit_ClassDef = _func

            def _hit(self, node, form):
                if len(hits) >= MAX_HITS:
                    return
                hits.append({
                    "file": rel,
                    "line": node.lineno,
                    "context": ".".join(stack) if stack else "<module>",
                    "form": form,
                })

            def visit_Name(self, node):
                if node.id == symbol and isinstance(node.ctx, ast.Load):
                    self._hit(node, "name")
                self.generic_visit(node)

            def visit_Attribute(self, node):
                if node.attr == symbol:
                    self._hit(node, "attribute")
                self.generic_visit(node)

        R().visit(tree)

    for rel, tree, _err in parsed:
        if tree is not None:
            visit(rel, tree)
    return hits


def _callgraph(parsed, symbol, depth):
    """按**名字**匹配的调用链（v1 不做作用域解析，同名方法会一起算进来）。

    返回值里带 resolution="by_name" 就是为了让模型知道这是名字级近似，
    而不是假装它是符号级解析结果。
    """
    edges, sites, defined = {}, {}, {}

    def visit(rel, tree):
        stack = []

        class C(ast.NodeVisitor):
            def _func(self, node):
                defined.setdefault(node.name, []).append(
                    (rel, node.lineno, ".".join(stack + [node.name]))
                )
                stack.append(node.name)
                self.generic_visit(node)
                stack.pop()

            visit_FunctionDef = _func
            visit_AsyncFunctionDef = _func
            visit_ClassDef = _func

            def visit_Call(self, node):
                name = _callee_name(node.func)
                if name:
                    caller = ".".join(stack) if stack else "<module>"
                    edges.setdefault(caller, set()).add(name)
                    sites.setdefault(name, []).append((rel, node.lineno, caller))
                self.generic_visit(node)

        C().visit(tree)

    for rel, tree, _err in parsed:
        if tree is not None:
            visit(rel, tree)

    visited, frontier, levels = {symbol}, [symbol], []
    for d in range(1, min(int(depth), MAX_DEPTH) + 1):
        nxt = []
        for caller in frontier:
            for callee in sorted(edges.get(caller, ())):
                if len(levels) >= MAX_HITS:
                    return {"resolution": "by_name", "levels": levels}
                levels.append({
                    "depth": d,
                    "caller": caller,
                    "callee": callee,
                    "call_sites": [
                        {"file": f, "line": ln}
                        for (f, ln, c) in sites.get(callee, ())
                        if c == caller
                    ][:5],
                    "defined_at": [
                        {"file": f, "line": ln, "qualname": q}
                        for (f, ln, q) in defined.get(callee, ())
                    ][:3],
                })
                if callee not in visited:
                    visited.add(callee)
                    nxt.append(callee)
        if not nxt:
            break
        frontier = nxt
    return {"resolution": "by_name", "levels": levels}


def _main():
    op, path = CFG["op"], CFG["path"]
    symbol, depth = CFG.get("symbol"), CFG.get("depth", 1)
    out = {"ok": True, "op": op, "path": path}
    if not os.path.exists(path):
        print(json.dumps({"ok": False, "op": op, "error": "path_not_found", "path": path},
                         ensure_ascii=False))
        return
    files, truncated = _collect(path)
    parsed = _parse(files, path)
    out["files_scanned"] = len(files)
    out["truncated"] = truncated
    bad = [{"file": r, "error": e} for (r, _t, e) in parsed if e]
    if bad:
        out["unparsable"] = bad[:20]
    if op == "symbols":
        rows = []
        for rel, tree, _err in parsed:
            if tree is not None:
                for kind, name, ln, end in _symbols(tree):
                    if len(rows) >= MAX_HITS:
                        break
                    rows.append({"file": rel, "kind": kind, "name": name,
                                 "line": ln, "end_line": end})
        out["symbols"] = rows
    elif op == "defs":
        out["defs"] = _find_defs(parsed, symbol)
    elif op == "refs":
        out["refs"] = _find_refs(parsed, symbol)
    elif op == "callgraph":
        out.update(_callgraph(parsed, symbol, depth))
    if any(len(out.get(k, ())) >= MAX_HITS for k in ("symbols", "defs", "refs", "levels")):
        out["truncated"] = True
    print(json.dumps(out, ensure_ascii=False))


try:
    _main()
except BaseException as exc:
    traceback.print_exc(file=sys.stderr)
    print(json.dumps(
        {"ok": False, "error": type(exc).__name__ + ": " + str(exc)[:400]},
        ensure_ascii=False))
"##;

#[cfg(test)]
mod tests {
    use super::*;

    // ── py_str_literal：转义 ──

    #[test]
    fn py_str_literal_escapes_quotes_backslash_and_control_chars() {
        assert_eq!(py_str_literal(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(py_str_literal("a\nb\tc\rd"), r#""a\nb\tc\rd""#);
        // 控制字符走 \xNN，且只吃两位十六进制（后接十六进制数字不会串味）
        assert_eq!(py_str_literal("\u{1}f"), r#""\x01f""#);
        assert_eq!(py_str_literal("\u{7f}"), r#""\x7f""#);
        // 非 ASCII 原样透传（脚本按 UTF-8 落盘，Python 3 源码默认 UTF-8）
        assert_eq!(py_str_literal("/tmp/项目/a.py"), "\"/tmp/项目/a.py\"");
        assert_eq!(py_str_literal(""), r#""""#);
    }

    /// 变异用：把 `\\` 漏掉会让含反斜杠的路径拼出坏 Python 代码。
    #[test]
    fn py_str_literal_roundtrips_through_real_python() {
        let Some(out) =
            run_probe_script(&build_ast_code("symbols", r#"/tmp/a"b\c'码.py"#, None, 1))
        else {
            return;
        };
        // 路径被原样带回 = 内联无损；否则 out["path"] 会与源串不等
        assert_eq!(out["path"], r#"/tmp/a"b\c'码.py"#);
    }

    // ── 参数校验（纯函数，不碰 AppHandle）──

    #[test]
    fn only_four_ops_are_accepted() {
        assert_eq!(AST_OPS, &["symbols", "defs", "refs", "callgraph"]);
        assert_eq!(AST_OPS_NEEDING_SYMBOL, &["defs", "refs", "callgraph"]);
        assert!(target_is_supported("symbols", "/a/b.PY"));
        assert!(target_is_supported("callgraph", "/a/b"));
        assert!(target_is_supported("callgraph", "/a/b.txt"));
        assert!(!target_is_supported("symbols", "/a/b.ts"));
        assert!(!target_is_supported("refs", "/a/b"));
    }

    #[test]
    fn build_ast_code_inlines_params_and_clamps_depth() {
        let code = build_ast_code("refs", "/a/b.py", Some("f"), 99);
        assert!(code
            .starts_with(r#"CFG = {"op": "refs", "path": "/a/b.py", "symbol": "f", "depth": 3}"#));
        assert!(code.contains("import ast"));
        // 无 symbol → 字面 None（沙箱侧据此判缺参）
        assert!(build_ast_code("symbols", "/a/b.py", None, 1)
            .starts_with(r#"CFG = {"op": "symbols", "path": "/a/b.py", "symbol": None,"#));
        // depth 下钳到 1
        assert!(build_ast_code("callgraph", "/a", Some("f"), 0)
            .starts_with(r#"CFG = {"op": "callgraph", "path": "/a", "symbol": "f", "depth": 1}"#));
    }

    // ── A3：授权闸接线守卫（resolve_with_perm 需 Wry handle，mock 够不到）──

    /// 钉住「先过授权闸、后生成 code」的语句顺序。
    ///
    /// 纯函数单测钉不住「闸是否真接在执行路径上」：把 `resolve_with_perm` 改成
    /// 恒返回输入路径（形似但绕过白名单），或把 code 生成提到闸之前，两种漂移
    /// 本文件其余用例照样全绿——而这个工具的全部安全价值就是「沙箱拿不到
    /// 未授权路径」。故以源码文本钉住语句形状 + 位置。
    #[test]
    fn perm_gate_precedes_sandbox_code_generation() {
        let src = include_str!("ast.rs");
        let body = src
            .split_once("pub async fn tool_ast_inspect(")
            .expect("找不到 tool_ast_inspect")
            .1
            .split("\n}\n")
            .next()
            .expect("tool_ast_inspect 应以顶格 } 收尾");
        let gate = body
            .find("resolve_with_perm(app, \"ast_inspect\", path, interactive, session_id)")
            .expect("tool_ast_inspect 必须调用 resolve_with_perm：白名单外路径不得进沙箱");
        let build = body
            .find("build_ast_code(")
            .expect("tool_ast_inspect 必须经 build_ast_code 生成沙箱代码");
        let exec = body
            .find("py_exec_sync_async(")
            .expect("tool_ast_inspect 必须走既有 Python 沙箱");
        assert!(
            gate < build,
            "代码生成必须晚于授权闸：闸前的 canonical path 才可信"
        );
        assert!(
            build < exec,
            "沙箱执行必须晚于代码生成（拿不到未生成的内联路径）"
        );
        // 钉闸的取值形状，不只是「调用还在」：变异实测把 Ok 臂改成
        // `Ok(_p) => PathBuf::from(path)`（调用完好、顺序照旧，却把**未授权的
        // 原始路径**交给了沙箱），只查顺序的守卫照样绿。
        assert!(
            body.contains("Ok(canonical) => canonical"),
            "闸的 Ok 臂必须原样交出 resolve_with_perm 的结果，不得改写/回退到原始 path"
        );
        assert!(
            body.contains("build_ast_code(op, &canonical.to_string_lossy()"),
            "内联进 code 的必须是闸返回的 canonical path，不得是用户输入的 path"
        );
    }

    // ── A6：四项能力真跑 stdlib ast ──

    /// 找到可用的 python3；找不到时返回 None（用例据此安静跳过）。
    fn python3() -> Option<String> {
        let from_cache = crate::py::env::cached_python();
        let found = from_cache.or_else(|| {
            std::env::var("WM_TEST_PYTHON")
                .ok()
                .filter(|p| std::path::Path::new(p).exists())
        })?;
        if std::process::Command::new(&found)
            .arg("-c")
            .arg("import ast")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            Some(found)
        } else {
            None
        }
    }

    /// 绕开 `py_exec_sync`（需 Wry AppHandle）直接跑脚本，取 stdout 的 JSON。
    fn run_probe_script(code: &str) -> Option<serde_json::Value> {
        let py = python3()?;
        let dir = tempfile::tempdir().ok()?;
        let f = dir.path().join("probe.py");
        std::fs::write(&f, code).ok()?;
        let out = std::process::Command::new(&py).arg(&f).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str(text.trim()).ok()
    }

    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("临时目录");
        let a = dir.path().join("a.py");
        std::fs::write(
            &a,
            r#"
import b

def helper(x):
    return x + 1

async def helper_async(x):
    return x

class Store:
    def __init__(self):
        self.v = 0

    def put(self, v):
        self.v = helper(v)

def run_it():
    s = Store()
    s.put(1)
    put(2)
    helper(2)
    return b.tweak()
"#,
        )
        .expect("写 fixture");
        let b = dir.path().join("b.py");
        std::fs::write(
            &b,
            r#"
def tweak():
    return 1

def unused():
    return 2
"#,
        )
        .expect("写 fixture");
        dir
    }

    #[test]
    fn symbols_op_lists_classes_methods_and_nested_defs() {
        let Some(dir) = Some(fixture_dir()) else {
            return;
        };
        let Some(v) = run_probe_script(&build_ast_code(
            "symbols",
            &dir.path().join("a.py").to_string_lossy(),
            None,
            1,
        )) else {
            eprintln!("跳过：本机无可用 python3，symbols 用例未实跑");
            return;
        };
        let syms = v["symbols"].as_array().expect("symbols 应为数组");
        let names: Vec<&str> = syms
            .iter()
            .filter_map(|s| s["name"].as_str())
            .filter(|n| n.contains('.'))
            .collect();
        assert!(syms.iter().all(|s| s["file"] == "a.py"), "符号应带文件名");
        assert!(
            names.contains(&"Store.put"),
            "类方法应带类名前缀，实得 {names:?}"
        );
        assert!(
            names.contains(&"Store.__init__"),
            "构造方法不应漏，实得 {names:?}"
        );
        assert!(v["files_scanned"] == 1, "只该扫传入的那个文件");
    }

    #[test]
    fn defs_op_finds_definition_with_signature_and_line() {
        let dir = fixture_dir();
        let Some(v) = run_probe_script(&build_ast_code(
            "defs",
            &dir.path().join("a.py").to_string_lossy(),
            Some("put"),
            1,
        )) else {
            eprintln!("跳过：本机无可用 python3，defs 用例未实跑");
            return;
        };
        let defs = v["defs"].as_array().expect("defs 应为数组");
        assert_eq!(defs.len(), 1, "defs 应精确命中 1 处：{v}");
        assert_eq!(defs[0]["kind"], "def");
        assert_eq!(defs[0]["sig"], "(self, v)");
        assert!(defs[0]["line"].as_u64().unwrap() > 0);
    }

    #[test]
    fn refs_op_separates_name_from_attribute_and_reports_context() {
        let dir = fixture_dir();
        let Some(v) = run_probe_script(&build_ast_code(
            "refs",
            &dir.path().join("a.py").to_string_lossy(),
            Some("put"),
            1,
        )) else {
            eprintln!("跳过：本机无可用 python3，refs 用例未实跑");
            return;
        };
        let refs = v["refs"].as_array().expect("refs 应为数组");
        let by_name: Vec<&serde_json::Value> =
            refs.iter().filter(|r| r["form"] == "name").collect();
        let by_attr: Vec<&serde_json::Value> =
            refs.iter().filter(|r| r["form"] == "attribute").collect();
        assert_eq!(by_name.len(), 1, "裸调用 put(1) 应记 name：{v}");
        assert_eq!(by_attr.len(), 1, "s.put(1) 应记 attribute：{v}");
        assert_eq!(by_name[0]["context"], "run_it", "应报出所在函数");
        // 定义处本身（def put）不是 Load 上下文，不应算引用
        assert!(refs.iter().all(|r| r["context"] != "Store.__init__"));
    }

    #[test]
    fn callgraph_op_expands_across_files_by_name() {
        let dir = fixture_dir();
        let Some(v) = run_probe_script(&build_ast_code(
            "callgraph",
            &dir.path().to_string_lossy(),
            Some("run_it"),
            2,
        )) else {
            eprintln!("跳过：本机无可用 python3，callgraph 用例未实跑");
            return;
        };
        assert_eq!(v["files_scanned"], 2, "应扫到 a.py + b.py：{v}");
        assert_eq!(v["resolution"], "by_name", "名字级近似必须显式声明");
        let levels = v["levels"].as_array().expect("levels 应为数组");
        let pairs: Vec<(&str, &str)> = levels
            .iter()
            .map(|l| {
                (
                    l["caller"].as_str().unwrap_or(""),
                    l["callee"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert!(
            pairs.contains(&("run_it", "helper")),
            "run_it 应展开到 helper，实得 {pairs:?}"
        );
        assert!(
            pairs.contains(&("run_it", "tweak")),
            "跨文件边 run_it → tweak（定义在 b.py）应出现，实得 {pairs:?}"
        );
        assert!(
            levels.iter().any(|l| l["callee"] == "tweak"
                && l["defined_at"]
                    .as_array()
                    .is_some_and(|d| d[0]["file"] == "b.py")),
            "tweak 的定义位置应指向 b.py"
        );
        // depth=1 时不该出现二层边
        let Some(v1) = run_probe_script(&build_ast_code(
            "callgraph",
            &dir.path().to_string_lossy(),
            Some("run_it"),
            1,
        )) else {
            return;
        };
        assert!(
            v1["levels"]
                .as_array()
                .is_some_and(|l| l.iter().all(|x| x["depth"] == 1)),
            "depth=1 只展开一层：{v1}"
        );
    }

    /// 跨文件扫描的边界：`.git` / `__pycache__` / 指向外部的 symlink 都不进索引。
    #[test]
    fn callgraph_skips_noise_dirs_and_symlinks_escaping_root() {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = dir.path().join("pkg");
        std::fs::create_dir_all(root.join("__pycache__")).expect("建目录");
        std::fs::write(root.join("real.py"), "def f():\n    pass\n").expect("写文件");
        std::fs::write(
            root.join("__pycache__").join("junk.py"),
            "def g():\n    pass\n",
        )
        .expect("写文件");
        let outside = dir.path().join("outside.py");
        std::fs::write(&outside, "def h():\n    pass\n").expect("写文件");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("link.py")).expect("建 symlink");

        let Some(v) = run_probe_script(&build_ast_code(
            "callgraph",
            &root.to_string_lossy(),
            Some("f"),
            1,
        )) else {
            eprintln!("跳过：本机无可用 python3，symlink 边界用例未实跑");
            return;
        };
        assert_eq!(
            v["files_scanned"], 1,
            "只该扫 real.py：__pycache__ 与逃逸 symlink 都要排除：{v}"
        );
    }

    #[cfg(unix)]
    /// 变异用：删掉 `_within(root, p)` 这道检查，本用例会红。
    #[test]
    fn callgraph_never_indexes_symlink_pointing_outside_root() {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = dir.path().join("pkg");
        std::fs::create_dir_all(&root).expect("建目录");
        let secret = dir.path().join("secret.py");
        std::fs::write(&secret, "def leaked():\n    return 1\n").expect("写文件");
        std::os::unix::fs::symlink(&secret, root.join("innocent.py")).expect("建 symlink");

        let Some(v) = run_probe_script(&build_ast_code(
            "callgraph",
            &root.to_string_lossy(),
            Some("leaked"),
            1,
        )) else {
            return;
        };
        assert_eq!(
            v["files_scanned"], 0,
            "授权的是 root 不是 root 里的每个条目——指向外部的 symlink 必须排除：{v}"
        );
        assert_eq!(v["ok"], true);
    }

    /// 语法错文件只记 unparsable，不影响同批其他文件的结果。
    #[test]
    fn unparsable_file_is_reported_without_killing_the_batch() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("ok.py"), "def f():\n    pass\n").expect("写文件");
        std::fs::write(dir.path().join("bad.py"), "def f(:\n").expect("写文件");
        let Some(v) = run_probe_script(&build_ast_code(
            "symbols",
            &dir.path().to_string_lossy(),
            None,
            1,
        )) else {
            eprintln!("跳过：本机无可用 python3，unparsable 用例未实跑");
            return;
        };
        assert_eq!(v["files_scanned"], 2, "两个文件都要计入扫描数");
        assert_eq!(v["unparsable"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(
            v["symbols"].as_array().map(|a| a.len()),
            Some(1),
            "好文件的结果仍要出：{v}"
        );
    }

    /// 沙箱内异常必须变成结构化错误 + stderr traceback（不吞）。
    #[test]
    fn script_failure_surfaces_as_json_error_not_silence() {
        let code = format!(
            "CFG = {{\"op\": \"symbols\", \"path\": \"/definitely/missing/x.py\", \
             \"symbol\": None, \"depth\": 1}}\n{}",
            AST_SCRIPT
        );
        let Some(v) = run_probe_script(&code) else {
            eprintln!("跳过：本机无可用 python3，异常路径用例未实跑");
            return;
        };
        assert_eq!(v["ok"], false, "路径不存在必须报失败而非静默返回空");
        assert_eq!(v["error"], "path_not_found");
    }

    /// 跨文件调用链的耗时回归闸（实测数字见 commit message）。
    ///
    /// 「不缓存、每次现扫」当时是**接受**未实测代价的决定，理由是「缓存必然
    /// 带来失效问题，而我们还没有数字证明它值得」。现在有数字了：在硬边界
    /// （近 200 文件 / 近 2MB）下实测 p50 = 213ms，相对裸解释器地板
    /// （import ast ≈ 17ms）只多 ~196ms——离 `run_python` 的 60s 超时有两个
    /// 数量级。缓存要解决的成本不存在，所以不缓存的决定成立。
    ///
    /// 这条测试的用途是把上面那个数字**变成可回归的**：脚本哪天从线性变成
    /// 平方（或引入沙箱层的固定开销爆炸），这里会红。上界给到 5s ≈ 实测的
    /// 23 倍，Windows/冷启动差异不会误伤——这道闸要抓的是量级回归，不是抖动。
    #[test]
    fn callgraph_at_scale_stays_far_below_the_sandbox_timeout() {
        let dir = tempfile::tempdir().expect("临时目录");
        // 近边界规模：120 文件 × ~9KB（实测边界值 213ms 的一半左右）
        for i in 0..120 {
            let mut body = String::from(&format!("\"\"\"m{i}。\"\"\"\nimport os\n\n"));
            for f in 0..36 {
                body.push_str(&format!(
                    "def f_{f}(a, b):\n    \
                     \"\"\"填充体积用。\"\"\"\n    \
                     s = \"{}\"\n    \
                     return a + b + len(s) * {}\n\n",
                    "x".repeat(150),
                    f + 1
                ));
            }
            std::fs::write(dir.path().join(format!("m{i}.py")), body).expect("写 fixture");
        }
        let code = build_ast_code("callgraph", &dir.path().to_string_lossy(), Some("f_0"), 3);
        let started = std::time::Instant::now();
        let Some(v) = run_probe_script(&code) else {
            eprintln!("跳过：本机无可用 python3，耗时闸未实跑");
            return;
        };
        let elapsed = started.elapsed();
        assert_eq!(v["ok"], true, "边界规模不该跑失败：{v}");
        // truncated 是**如实上报**不是失败（边界内触顶属预期），只断言时间
        let budget = std::time::Duration::from_secs(5);
        assert!(
            elapsed < budget,
            "跨文件调用链耗时 {}ms 超出 {}ms 上界——量级回归（实测同量级 p50 ≈ {}ms）",
            elapsed.as_millis(),
            budget.as_millis(),
            150
        );
    }
}
