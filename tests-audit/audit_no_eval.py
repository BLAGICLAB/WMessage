#!/usr/bin/env python3
"""
audit_no_eval.py — 前端源码禁动态求值静态守卫（CSP 收紧配套）。

背景：生产 csp 已移除 eval 放行项（docs/CSP-TIGHTEN-VERIFY-2026-10-07.md）。
本守卫防「未来代码/依赖引入动态求值调用」导致运行时被 CSP 拦截或诱导放松 CSP。
（本脚本内的敏感 token 一律拼接构造——Mimosa/人工审阅都不必读字面量。）

扫描范围：src/ 下 .ts/.tsx（排除 *.test.* 与 *.d.ts）。
模式（六类）：
  1. 动态求值直接调用
  2. Function 构造器（new 形式 / 字符串首参形式）
  3. setTimeout/setInterval 字符串参数
  4. 动态 import(非字面量参数)
  5. 间接调用形式（逗号运算符 / 全局对象属性访问）
  6. Function 别名赋值启发式

误报处理：剥离注释与字符串字面量后匹配；排除测试文件。
正则字面量内的行注释起始符可能误启剥离——该局限方向为漏报（假阴性），
由 WebView 运行时 CSP 强制兜底，不做完整 tokenizer。

自测：--selftest——正例必须 pass、各反例必须 fail、注释/字符串误报必须 pass。

退出码：0 = 通过；1 = 有违例/自测失败。
"""
from __future__ import annotations
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SRC = REPO_ROOT / "src"

# 敏感 token 拼接构造（避免本文件出现可直接被误读的字面量）
EVAL = "e" + "v" + "a" + "l"
EVAL_CALL = EVAL + "("
EVAL_RE = r"\b" + EVAL + r"\s*\("
INDIRECT_RE = r"\(0,\s*" + EVAL + r"\)|window\." + EVAL + r"|globalThis\." + EVAL + r"|self\." + EVAL
ALIAS_RE = r"[:=]\s*Function\b\s*(?!\()"

PATTERNS = [
    (EVAL_RE, "动态求值直接调用"),
    (r"\bnew\s+Function\s*\(", "new Function 构造器"),
    (r"\bFunction\s*\(\s*[\"'`]", "Function 字符串首参构造器"),
    (r"\b(?:setTimeout|setInterval)\s*\(\s*[\"'`]", "定时器字符串参数"),
    (r"\bimport\s*\(\s*[^\"'`\s)]", "动态 import 非字面量参数"),
    (INDIRECT_RE, "间接动态求值形式"),
    (ALIAS_RE, "Function 别名赋值（启发式）"),
]


def strip_comments_and_strings(text: str) -> str:
    """注释内容与字符串**内部**替换为空格（引号定界符保留，行号稳定）——
    定界符必须保留：`Function("…")`/`setTimeout("…")` 等字符串首参形态
    依赖引号识别；内部清空防内容误触其他模式。"""
    out: list[str] = []
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
        elif state in ('"', "'", "`"):
            if c == "\\":
                out.append("  ")
                i += 2
            elif c == state:
                out.append(c)  # 闭定界符保留
                state = None
                i += 1
            else:
                out.append(" ")  # 字符串内部清空（内容不参与模式匹配）
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
            elif c in ('"', "'", "`"):
                state = c
                out.append(c)
                i += 1
            else:
                out.append(c)
                i += 1
    return "".join(out)


def scan_text(text: str) -> list[str]:
    cleaned = strip_comments_and_strings(text)
    hits = []
    for pat, label in PATTERNS:
        for m in re.finditer(pat, cleaned):
            line_no = cleaned.count("\n", 0, m.start()) + 1
            hits.append(f"line {line_no}: {label}（{cleaned[m.start():m.end()][:40]}）")
    return hits


def source_files() -> list[Path]:
    if not SRC.exists():
        return []
    return [
        p for p in sorted(SRC.rglob("*"))
        if p.suffix in (".ts", ".tsx")
        and ".test." not in p.name
        and not p.name.endswith(".d.ts")
    ]


def run_checks() -> list[str]:
    violations = []
    for f in source_files():
        for hit in scan_text(f.read_text(encoding="utf-8", errors="replace")):
            violations.append(f"{f.relative_to(REPO_ROOT)}: {hit}")
    return violations


def selftest() -> int:
    import tempfile
    ok = True
    # 正例：注释/字符串里的敏感词、合法定时器回调、字面量动态 import 均放行。
    # 已知局限（记录用）：正则字面量内含敏感词（如 /eval(/）会被行注释剥离
    # 之前误匹配——本项目 src 无此形态，不进正例夹具
    pass_fixture = "\n".join([
        "// 注释里的 " + EVAL_CALL + " 与 new Function( 不算",
        'const s = "字符串里的 " + "' + EVAL_CALL + ' 也不算";',
        "const t = `模板里的 " + EVAL_CALL + " 也不算`;",
        "setTimeout(() => tick(), 10);",
        "await import(\"./mod\");",
    ])
    # 反例：六类真实形态必须全部抓到
    fail_fixtures = [
        EVAL_CALL + '"1 + 1";',
        'const f = new Function("return 1");',
        'Function("return 2")();',
        'setTimeout("tick()", 10);',
        "setInterval('beep()', 100);",
        "await import(dynamicName);",
        "(0, " + EVAL + ')("x");',
        "window." + EVAL + '("y");',
        "const g = globalThis." + EVAL + ";",
    ]
    with tempfile.TemporaryDirectory() as td:
        probe = Path(td) / "probe.ts"
        probe.write_text(pass_fixture, encoding="utf-8")
        if scan_text(probe.read_text(encoding="utf-8")):
            print("✗ selftest：正例被误判（注释/字符串剥离或合法形态误伤）", file=sys.stderr)
            ok = False
        for i, sample in enumerate(fail_fixtures):
            probe.write_text(sample, encoding="utf-8")
            if not scan_text(probe.read_text(encoding="utf-8")):
                print(f"✗ selftest：反例 {i} 未被抓到：{sample}", file=sys.stderr)
                ok = False
    if ok:
        print("✓ audit_no_eval selftest 通过（正例放行 + 反例全抓）")
        return 0
    return 1


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    files = source_files()
    if not files:
        print("✗ src/ 下未找到 .ts/.tsx 源文件（路径异常？）", file=sys.stderr)
        return 1
    violations = run_checks()
    if violations:
        print(f"✗ 前端禁动态求值守卫：{len(violations)} 处违例", file=sys.stderr)
        for v in violations:
            print(f"  {v}", file=sys.stderr)
        print("  生产 csp 已收紧动态求值放行项，此类调用运行时必被拦截；"
              "确需动态求值须先重新评估 CSP（docs/CSP-TIGHTEN-VERIFY-2026-10-07.md §8）",
              file=sys.stderr)
        return 1
    print(f"✓ 前端禁动态求值守卫通过（{len(files)} 文件，六类模式零命中）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
