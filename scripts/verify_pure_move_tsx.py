#!/usr/bin/env python3
"""纯搬移核对器（前端版）：证明「拆分只搬不改」。

用法：
    python3 scripts/verify_pure_move_tsx.py <原文件> --manifest <清单> --base-dir <目录>

清单格式（每行一条，`<相对路径>:<起>-<止>`，行号是**原文件**里 1-based 闭区间）：
    ChatPanel.test.tsx:164-623
    ChatPanel.send.test.tsx:625-1099

可选：
    --allow-chunk 'rel:a-b'   声明该段是「搬移 + 接线」的获准改动（可重复）

为什么比 Rust 版简单：
前端没有格式化器（仓库无 prettier），搬移是**字节级**的——不存在 rustfmt
重排行边界的问题，所以比对口径可以更硬：逐行 lstrip 后必须完全相同。
Rust 版那套「代码段 / 注释段 / 字面量段」的词法分类在这里不需要。

比对口径：
1. **被搬块**：原文件 [起-止] 的每一行 lstrip() 后，必须在目标文件里
   **连续**出现（顺序与内容都不变）。只放行缩进差异——整块搬到别的层级时
   必然少一层缩进，缩进本身不是内容。
2. **获准段**（--allow-chunk）：该段允许出现接线改动，但仍要在目标文件里
   找得到，且**改动量 ≤ 该段的 1/3**。用来声明「搬走之后必然要改的那几行」
   （比如每个测试文件各自裁掉的未用 import）。
3. **覆盖检查**：原文件里既不在清单、也不在获准段里的行，必须全部是
   import / export / 注释 / 空行 / 闭合括号——出现实体代码行就是「未登记的
   改动」，说明有内容被悄悄漏掉了。

退出码：0 = 全部被搬块逐字找到且覆盖检查干净；1 = 有被搬块找不到，或覆盖
检查里有未登记的实体代码行。
"""

from __future__ import annotations

import argparse
import difflib
import sys
from pathlib import Path

def parse_manifest(path: Path) -> list[tuple[str, int, int]]:
    out: list[tuple[str, int, int]] = []
    for lineno, raw in enumerate(path.read_text().split("\n"), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if ":" not in line or "-" not in line:
            raise SystemExit(
                f"清单第 {lineno} 行格式不对：{raw!r}（应为 `<路径>:<起>-<止>`）"
            )
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


def parse_spans(items: list[str], flag: str) -> set[tuple[str, int, int]]:
    out: set[tuple[str, int, int]] = set()
    for c in items:
        rel, _, span = c.rpartition(":")
        a, _, b = span.partition("-")
        try:
            out.add((rel, int(a), int(b)))
        except ValueError:
            raise SystemExit(
                f"{flag} 格式不对：{c!r}（应为 <路径>:<起>-<止>）"
            )
    return out


def strip_lines(lines: list[str]) -> list[str]:
    return [l.lstrip() for l in lines]


def find_contiguous(haystack: list[str], needle: list[str]) -> int:
    """needle 在 haystack 里首次出现的起始下标；找不到返回 -1。"""
    if not needle:
        return 0
    n, m = len(haystack), len(needle)
    if m > n:
        return -1
    first = needle[0]
    for start in range(n - m + 1):
        if haystack[start] != first:
            continue
        if haystack[start : start + m] == needle:
            return start
    return -1


def is_structural(line: str) -> bool:
    """是否是「允许出现在清单之外」的行：import / export / 注释 / 空行 / 闭合括号。"""
    s = line.strip()
    if not s:
        return True
    if s.startswith(("//", "/*", "*")):
        return True
    if s.startswith(("import ", "export ", "import{", "export{")):
        return True
    if s.startswith("import type ") or s.startswith("export type "):
        return True
    if s.startswith(("}", ")", "]")):
        return True
    return False


def main() -> int:
    ap = argparse.ArgumentParser(
        description="纯搬移核对器（前端版）：按清单逐段验证原文在新文件里逐字找得到",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    ap.add_argument("original", type=Path, help="拆分前的原文件")
    ap.add_argument(
        "--manifest", required=True, type=Path, help="chunk 清单（<路径>:<起>-<止> 每行一条）"
    )
    ap.add_argument(
        "--base-dir", required=True, type=Path, help="清单里相对路径的基准目录"
    )
    ap.add_argument(
        "--allow-chunk",
        action="append",
        default=[],
        metavar="REL:A-B",
        help="声明「这段的差异是已知且获准的」（可重复）；该段仍会打印改动量",
    )
    ap.add_argument(
        "--list-uncovered",
        action="store_true",
        help="打印未被清单覆盖的原文件行（默认只报非结构性行）",
    )
    ap.add_argument("-q", "--quiet", action="store_true", help="只在失败时输出")
    args = ap.parse_args()

    orig_lines = args.original.read_text().split("\n")
    chunks = parse_manifest(args.manifest)
    allowed = parse_spans(args.allow_chunk, "--allow-chunk")

    target_cache: dict[str, list[str]] = {}

    def target_stripped(rel: str) -> list[str]:
        if rel not in target_cache:
            p = args.base_dir / rel
            if not p.exists():
                raise SystemExit(f"清单指向的文件不存在：{p}")
            target_cache[rel] = strip_lines(p.read_text().split("\n"))
        return target_cache[rel]

    failures: list[tuple[str, int, int, str]] = []
    ok_count = 0
    allowed_report: list[tuple[str, int, int, int, int]] = []

    for rel, a, b in chunks:
        if b > len(orig_lines):
            raise SystemExit(
                f"清单区间 {rel}:{a}-{b} 越界（原文件只有 {len(orig_lines)} 行）"
            )
        chunk = strip_lines(orig_lines[a - 1 : b])
        target = target_stripped(rel)
        if find_contiguous(target, chunk) >= 0:
            ok_count += 1
        else:
            # 找最长公共前缀，给出可定位的分歧点
            head = 0
            for x, y in zip(chunk, target):
                if x != y:
                    break
                head += 1
            failures.append((rel, a, b, f"…{' | '.join(chunk[head:head + 3])}"))

    for rel, a, b in sorted(allowed):
        if b > len(orig_lines):
            raise SystemExit(
                f"--allow-chunk 区间 {rel}:{a}-{b} 越界（原文件只有 {len(orig_lines)} 行）"
            )
        chunk = strip_lines(orig_lines[a - 1 : b])
        target = target_stripped(rel)
        matcher = difflib.SequenceMatcher(a=chunk, b=target, autojunk=False)
        matched = sum(block.size for block in matcher.get_matching_blocks())
        total = len(chunk)
        changed = total - matched
        limit = total // 3
        allowed_report.append((rel, a, b, matched, changed))
        if changed > limit:
            failures.append(
                (
                    rel,
                    a,
                    b,
                    f"获准段的改动量 {changed} 行超过 1/3 上限（{limit}）：匹配 {matched}/{total}",
                )
            )

    if not args.quiet or failures:
        print(f"原文件 {args.original}（{len(orig_lines)} 行）")
        print(
            f"清单 {args.manifest}：{len(chunks)} 段，{ok_count} 段逐字找到，"
            f"{len(failures)} 段失败"
        )
        if allowed_report:
            print(f"获准段 {len(allowed_report)} 段（允许 ≤1/3 改动）：")
            for rel, a, b, matched, changed in allowed_report:
                print(f"  {rel}:{a}-{b}  匹配 {matched} 行，改动 {changed} 行")

    if failures:
        print("\n✗ 以下 chunk 在目标文件里找不到逐字对应：")
        for rel, a, b, frag in failures:
            print(f"  {rel}:{a}-{b}")
            print(f"    {frag}")

    # 覆盖检查：清单与获准段之外的原文件行
    covered: set[int] = set()
    for _, a, b in chunks:
        covered.update(range(a, b + 1))
    for _, a, b in allowed:
        covered.update(range(a, b + 1))

    uncovered = [(i, l) for i, l in enumerate(orig_lines, 1) if i not in covered and l.strip()]
    structural = [(i, l) for i, l in uncovered if is_structural(l)]
    other = [(i, l) for i, l in uncovered if not is_structural(l)]

    if not args.quiet:
        print(
            f"\n清单未覆盖的原文件行：{len(uncovered)} 行"
            f"（结构性 {len(structural)} / 其余 {len(other)}）"
        )
        if other:
            print("  以下非结构性行没被任何 chunk 覆盖——要么补进清单，要么确认它是获准的改动：")
            for i, l in other[:40]:
                print(f"    {i}: {l.strip()[:100]}")
            if len(other) > 40:
                print(f"    … 还有 {len(other) - 40} 行")
        elif structural:
            print("  （全是 import / export / 注释 / 闭合括号，符合预期）")
        if args.list_uncovered:
            for i, l in structural:
                print(f"    {i}: {l.strip()[:100]}")

    return 1 if failures or other else 0


if __name__ == "__main__":
    sys.exit(main())
