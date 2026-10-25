#!/usr/bin/env python3
"""纯搬移核对器：证明「拆分只搬不改」。

用法：
    python3 scripts/verify_pure_move.py <原文件> --manifest <清单> --base-dir <目录>

清单格式（每行一条，`<相对路径>:<起>-<止>`，行号是**原文件**里 1-based 闭区间）：
    schemas.rs:181-486
    schemas.rs:1831-2686
    handlers.rs:487-890
    claims.rs:892-1540

背景：拆分大文件时的红线是**纯搬移**——除了 `use` 增删、`mod` 声明、
可见性修饰、必要的 `include_str!` 路径修正，不应出现任何逻辑行改动。
靠人眼过 diff 会漏：一次拆分涉及几千行，而漏掉的那一行往往就是
「顺手优化」——行为漂移的来源。

本工具把这条红线变成**可机械验证**的命题：清单里声明的每一段原文，
都必须在目标文件里**逐字找得到**（比对口径见下），且原文件里没有
任何一段被悄悄漏掉。

比对口径（为什么不是逐行比对）：
- **不是逐行**：rustfmt 会因为加长（如 `pub(super) ` 前缀）或缩进变化而
  **重排行边界**——`fn f(a: A) -> B {` 会被拆成三行。逐行比对会把
  「只是重新排版」误判成「内容被改」。
- **空白无关**：代码段的空白（缩进、换行、行内空格）全部剔除后比对，
  所以行边界怎么变都不影响判定。
- **字面量保真**：字符串 / 字符 / 原始字符串的**内容逐字保留**（不剔除空白）。
  唯一的例外是普通字符串的**反斜杠续行**：Rust 规范会把续行之后的换行与
  空白整段吃掉，那段缩进本就不是内容，去缩排时它会跟着动，所以归一化掉。
  schema 常量里的 JSON 描述文本里空格是有意义的（`"a": 1` vs `"a":1`），
  抹掉会让真实改动蒙混过关。
- **注释只放行前导缩进**：注释的**文字**逐字保留，但每行的前导空白不比——
  内联 `mod tests { ... }` 整块搬成独立文件时必然要去掉一层缩进，
  注释随之左移；按字面量逐字比会把纯搬移误报成「内容被改」。
  原始字符串里的缩进**仍然逐字保留**（它本来就是内容）。
- **可见性前缀可剥离**：`--strip-prefix` 指定的修饰（如 `pub(super) `）
  在两侧都剔除，于是「加可见性」这种获准的改动不算内容变化。

覆盖检查：清单之外的原文件行会被列出来。它们**应当只含** `use` /
`mod` / 注释 / 空白（这些是被允许的改动）；若混进了逻辑行，说明清单
漏了一段，那段内容就没人验证了。

退出码：0 = 全部 chunk 逐字找到；1 = 有 chunk 找不到（并打印分歧位置）。
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# rustfmt 拆行时补的尾随逗号（不是内容变化）
TRAILING_COMMA_RE = re.compile(r",(\s*[)\]}])")
# 普通字符串里的反斜杠续行（续行之后的空白不是内容）
CONTINUATION_RE = re.compile(r"\\\n[ \t]*")
# `use` 语句起点（含 pub / pub(crate) / pub(super) 变体）
USE_START_RE = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?use\b")


# ── 迷你词法：把文本切成「代码段」「注释段」「字面量段」 ────────────────────
def segment(text: str) -> list[tuple[str, str]]:
    """返回 [(类别, 片段), ...]，类别 ∈ {code, comment, literal}。

    三类的比对口径不同（见 `key()`）：
    - `code`    代码：空白无关，另可剥可见性前缀
    - `comment` 注释：**逐行去前导空白**，其余逐字保留
    - `literal` 字符串 / 字符：逐字保留，但**反斜杠续行的缩进归一化**
      （Rust 规范会把续行后的空白整段吃掉，它不是内容）
    - `raw`     原始字符串：逐字保留（含缩进——那里的空白就是内容）
    """
    out: list[tuple[str, str]] = []
    i = 0
    n = len(text)
    buf: list[str] = []
    code_start = 0

    def flush() -> None:
        if buf:
            out.append((False, "".join(buf)))
            buf.clear()

    while i < n:
        c = text[i]
        # 原始字符串 r#"..."# / r"..."
        if c == "r" and i + 1 < n and (text[i + 1] == '"' or text[i + 1] == "#"):
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                close = '"' + "#" * hashes
                end = text.find(close, j + 1)
                if end == -1:
                    raise SystemExit("未闭合的原始字符串")
                flush()
                out.append(("raw", text[i : end + len(close)]))
                i = end + len(close)
                code_start = i
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            flush()
            out.append(("literal", text[i : j + 1]))
            i = j + 1
            code_start = i
            continue
        if c == "'":
            # 字符字面量 'x' / '\n'；生命周期 'a 不是
            j = i + 1
            if j < n and text[j] == "\\":
                j += 2
                while j < n and text[j] != "'":
                    j += 1
                flush()
                out.append(("literal", text[i : j + 1]))
                i = j + 1
                code_start = i
                continue
            if j + 1 < n and text[j + 1] == "'":
                flush()
                out.append(("literal", text[i : j + 2]))
                i = j + 2
                code_start = i
                continue
            # 生命周期（'a）不是字符字面量——当普通代码处理，别把 ' 吃掉
            buf.append(c)
            i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            end = text.find("\n", i)
            if end == -1:
                end = n
            flush()
            out.append(("comment", text[i:end]))
            i = end
            code_start = i
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 0
            j = i
            while j < n:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                    if depth == 0:
                        break
                else:
                    j += 1
            flush()
            out.append(("comment", text[i:j]))
            i = j
            code_start = i
            continue
        buf.append(c)
        i += 1
    flush()
    _ = code_start
    return out


def key(text: str, strip_prefixes: list[str]) -> str:
    """比对键：代码段剔空白与可见性前缀；注释段逐行去前导空白；字面量段逐字保留。

    为什么注释与字符串要分开：
    - **字符串字面量**里的空格是有意义的（`"a": 1` vs `"a":1`），逐字保留。
    - **注释的前导缩进**不是内容——内联 `mod tests { ... }` 整块搬成独立文件时
      必然要去掉一层缩进，注释也就跟着左移。若把注释按字面量逐字比，
      一次纯搬移会被报成「内容被改」。
    注释的**文字**仍然逐字保留：改一个字照样被抓到。
    """
    parts: list[str] = []
    for kind, frag in segment(text):
        if kind == "literal":
            # 普通字符串的反斜杠续行：Rust 规范会把续行之后的换行与空白整段
            # 吃掉，所以那段缩进**不是字符串内容**——去缩排时它会跟着动。
            # 归一化掉，否则一次纯搬移会被报成「字符串被改了」。
            parts.append(CONTINUATION_RE.sub(lambda _m: "\\\n", frag))
        elif kind == "raw":
            parts.append(frag)
        elif kind == "comment":
            parts.append("\n".join(line.lstrip() for line in frag.split("\n")))
        else:
            for p in strip_prefixes:
                frag = frag.replace(p, "")
            # rustfmt 把长签名拆行时会补尾随逗号——那不是内容变化，抹掉。
            # 代价：`(1,)` 与 `(1)` 在本工具眼里相同（rustfmt 不会制造这种改动）。
            frag = TRAILING_COMMA_RE.sub(r"\1", frag)
            parts.append("".join(frag.split()))
    return "".join(parts)


def use_statement_lines(lines: list[str]) -> set[int]:
    """属于 `use` 语句的行号集合（1-based，含跨行 use 组的续行与收尾 `};`）。"""
    out: set[int] = set()
    i = 0
    n = len(lines)
    while i < n:
        s = lines[i].strip()
        if USE_START_RE.match(s):
            j = i
            while j < n and ";" not in lines[j]:
                j += 1
            out.update(range(i + 1, min(j + 1, n) + 1))
            i = j + 1
            continue
        i += 1
    return out


def parse_manifest(path: Path) -> list[tuple[str, int, int]]:
    out: list[tuple[str, int, int]] = []
    for lineno, raw in enumerate(path.read_text().split("\n"), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if ":" not in line or "-" not in line:
            raise SystemExit(f"清单第 {lineno} 行格式不对：{raw!r}（应为 `<路径>:<起>-<止>`）")
        rel, _, span = line.rpartition(":")
        a, _, b = span.partition("-")
        try:
            a_i, b_i = int(a), int(b)
        except ValueError:
            raise SystemExit(f"清单第 {lineno} 行行号不是整数：{raw!r}")
        if a_i < 1 or b_i < a_i:
            raise SystemExit(f"清单第 {lineno} 行区间不合法：{raw!r}")
        out.append((rel, a_i, b_i))
    return out


def longest_found_prefix(chunk_key: str, target_key: str) -> int:
    """chunk_key 的最长前缀（在 target_key 中能找到）的长度。"""
    lo, hi = 0, len(chunk_key)
    best = 0
    while lo <= hi:
        mid = (lo + hi) // 2
        if chunk_key[:mid] in target_key:
            best = mid
            lo = mid + 1
        else:
            hi = mid - 1
    return best


def main() -> int:
    ap = argparse.ArgumentParser(
        description="纯搬移核对器：按清单逐段验证原文在新文件里逐字找得到",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("original", type=Path, help="拆分前的原文件")
    ap.add_argument("--manifest", required=True, type=Path, help="chunk 清单（<路径>:<起>-<止> 每行一条）")
    ap.add_argument("--base-dir", required=True, type=Path, help="清单里相对路径的基准目录")
    ap.add_argument("--strip-prefix", action="append", default=[], metavar="PREFIX",
                    help="比对时在两侧都剔除的前缀（可重复，如 'pub(super) '）")
    ap.add_argument("--allow-chunk", action="append", default=[], metavar="REL:A-B",
                    help="声明「这段的差异是已知且获准的」（可重复，如 'registry.rs:2791-3826'）；"
                         "该段仍会打印分歧位置，但不计入失败")
    ap.add_argument("--list-uncovered", action="store_true", help="打印未被清单覆盖的原文件行")
    ap.add_argument("-q", "--quiet", action="store_true", help="只在失败时输出")
    args = ap.parse_args()

    orig_lines = args.original.read_text().split("\n")
    chunks = parse_manifest(args.manifest)

    # 目标文件的比对键（缓存）
    target_keys: dict[str, str] = {}

    def target_key(rel: str) -> str:
        if rel not in target_keys:
            p = args.base_dir / rel
            if not p.exists():
                raise SystemExit(f"清单指向的文件不存在：{p}")
            target_keys[rel] = key(p.read_text(), args.strip_prefix)
        return target_keys[rel]

    failures: list[tuple[str, int, int, str, int]] = []
    allowed: set[tuple[str, int, int]] = set()
    for c in args.allow_chunk:
        rel, _, span = c.rpartition(":")
        a, _, b = span.partition("-")
        try:
            allowed.add((rel, int(a), int(b)))
        except ValueError:
            raise SystemExit(f"--allow-chunk 格式不对：{c!r}（应为 <路径>:<起>-<止>）")
    covered: set[int] = set()
    ok_count = 0
    allowed_count = 0

    for rel, a, b in chunks:
        if b > len(orig_lines):
            raise SystemExit(f"清单区间 {rel}:{a}-{b} 越界（原文件只有 {len(orig_lines)} 行）")
        chunk_text = "\n".join(orig_lines[a - 1 : b])
        ck = key(chunk_text, args.strip_prefix)
        tk = target_key(rel)
        covered.update(range(a, b + 1))
        if ck and ck in tk:
            ok_count += 1
        else:
            div = longest_found_prefix(ck, tk)
            failures.append((rel, a, b, ck[div : div + 140], div))
            if (rel, a, b) in allowed:
                allowed_count += 1

    if not args.quiet or failures:
        print(f"原文件 {args.original}（{len(orig_lines)} 行）")
        print(f"清单 {args.manifest}：{len(chunks)} 段，{ok_count} 段逐字找到，"
              f"{len(failures) - allowed_count} 段失败，{allowed_count} 段为已声明的获准改动")

    if failures:
        print("\n✗ 以下 chunk 在目标文件里找不到逐字对应：")
        for rel, a, b, frag, div in failures:
            tag = "（已声明为获准改动）" if (rel, a, b) in allowed else ""
            print(f"  {rel}:{a}-{b}  匹配到第 {div} 字符后中断{tag}：")
            print(f"    …{frag}")
        print("  （可见性前缀用 --strip-prefix 声明；`include_str!` 路径等获准改动"
              "用 --allow-chunk 声明该段）")

    # 覆盖检查：清单之外的原文件行
    uncovered = [(i, l) for i, l in enumerate(orig_lines, 1) if i not in covered and l.strip()]
    use_lines = use_statement_lines(orig_lines)
    structural = []
    other = []
    for i, l in uncovered:
        s = l.strip()
        if (
            i in use_lines
            or s in ("}", "};")
            or s.startswith("//")
            or s.startswith("#[")
            or s.startswith("mod ")
        ):
            structural.append((i, l))
        else:
            other.append((i, l))

    if not args.quiet:
        print(f"\n清单未覆盖的原文件行：{len(uncovered)} 行"
              f"（结构性 {len(structural)} / 其余 {len(other)}）")
        if other and (args.list_uncovered or True):
            print("  以下非结构性行没被任何 chunk 覆盖——要么补进清单，要么确认它是获准的改动：")
            for i, l in other[:40]:
                print(f"    {i}: {l.strip()[:100]}")
            if len(other) > 40:
                print(f"    … 还有 {len(other) - 40} 行")
        elif structural:
            print("  （全是 use / mod / 注释，符合「获准改动」的预期）")

    return 1 if (len(failures) - allowed_count) else 0


if __name__ == "__main__":
    sys.exit(main())
