#!/usr/bin/env python3
"""验收卡判据体检：把卡片【可核对】判据里的 grep 命令逐条实跑，报命中与否。

用途：代码改过之后，卡里的 grep 目标可能改名、移位、被删——照原文跑会
命中 0，读卡的人会误判成「卡片过期」或「功能没了」。本脚本把这类漂移
一次列出来。

读法：
  命中      = 判据指向的东西还在
  ★ 0 命中   = 该判据要求的命令在现码里跑不出东西——多半是命令写错或
             目标改名（2026-11-25 一轮就抓出 10 处：emitted_any 改名、
             pub fn 收窄成 pub(crate)、方括号未转义、栏宽收进常量、
             命令指向已删除的实现等）
  无命中（判据要求）= 判据原文就写着「无命中」（退役两列那类），0 命中
             才是它要的

**不启子进程**：卡面文本是文档内容，交给 shell 解释就是命令注入。这里用
Python 自己实现 grep 的常用语义（BRE 基本正则 + -r/-F/-n/-i/-c），只读
文件不执行任何外部命令。

用法：python3 scripts/audit-card-greps.py            # 全跑，只出报告
     python3 scripts/audit-card-greps.py ACC-K-03    # 只看某张卡
"""

import pathlib
import re
import shlex
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CARDS_DIR = ROOT / "docs" / "acceptance"
GREP_RE = re.compile(r"`(grep[^`]*)`")
# 判据原文写着「无命中」/「不存在」时，0 命中是它要的
EXPECT_EMPTY_RE = re.compile(r"无命中|已退役|不存在|0 命中")


def card_blocks() -> list[tuple[str, str, list[str]]]:
    out = []
    for path in sorted(CARDS_DIR.glob("cards-*.md")):
        lines = path.read_text(encoding="utf-8").split("\n")
        cur = None
        buf: list[str] = []
        for line in lines:
            if line.startswith("### "):
                if cur:
                    out.append((cur, str(path.relative_to(ROOT)), buf))
                cur = line.split(" ")[1] if len(line.split(" ")) > 1 else ""
                buf = []
                continue
            if cur:
                buf.append(line)
        if cur:
            out.append((cur, str(path.relative_to(ROOT)), buf))
    return out


def bre_to_py(pat: str) -> str:
    """BRE 基本正则 → Python 正则：\\| 是「或」；+ ? ( ) | 在 BRE 里是字面量。"""
    out: list[str] = []
    i = 0
    while i < len(pat):
        c = pat[i]
        if c == "\\" and i + 1 < len(pat):
            nxt = pat[i + 1]
            out.append("|" if nxt == "|" else re.escape(nxt))
            i += 2
            continue
        if c in "+?()|":
            out.append("\\" + c)
            i += 1
            continue
        out.append(c)
        i += 1
    return "".join(out)


def grep(argv: list[str]) -> tuple[int, list[str]]:
    """跑等价 grep：返回 (命中行数, 命中样例)。只读文件。"""
    flags: set[str] = set()
    rest: list[str] = []
    for a in argv[1:]:
        if a.startswith("-") and len(a) > 1 and not a.startswith("--"):
            flags.update(a[1:])
        else:
            rest.append(a)
    if not rest:
        return 0, []
    pattern = rest[0]
    targets = rest[1:] or ["."]
    fixed = "F" in flags
    icase = re.IGNORECASE if "i" in flags else 0
    rx = (
        re.compile(re.escape(pattern), icase)
        if fixed
        else re.compile(bre_to_py(pattern), icase)
    )
    files: list[pathlib.Path] = []
    for t in targets:
        if any(ch in t for ch in "*?["):
            # 路径带通配：按 shell 的做法在仓库根展开
            base = ROOT
            for part in t.split("/")[:-1]:
                base = base / part
            pat = t.split("/")[-1]
            if base.is_dir():
                files.extend(sorted(x for x in base.glob(pat) if x.is_file()))
            continue
        p = ROOT / t
        if p.is_dir():
            files.extend(sorted(x for x in p.rglob("*") if x.is_file()))
        elif p.exists():
            files.append(p)
    hits = 0
    samples: list[str] = []
    for f in files:
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        for idx, line in enumerate(text.split("\n"), 1):
            if rx.search(line):
                hits += 1
                if len(samples) < 2:
                    samples.append(f"{f.relative_to(ROOT)}:{idx}: {line.strip()[:88]}")
    return hits, samples


def main() -> int:
    only = set(sys.argv[1:])
    total = 0
    machine = 0
    suspect: list[str] = []
    for card_id, path, block in card_blocks():
        if only and card_id not in only:
            continue
        for line in block:
            if "缺口补验" in line or "失败处置" in line:
                continue
            if not line.strip().startswith(("1.", "2.", "3.", "4.", "5.", "6.", "7.", "8.")):
                continue
            for raw in GREP_RE.findall(line):
                cmd = raw.strip()
                if not cmd.startswith("grep"):
                    continue
                try:
                    argv = shlex.split(cmd)
                except ValueError:
                    print(f"  {card_id}: 无法拆参 — {cmd[:90]}")
                    continue
                if not argv or argv[0] != "grep":
                    continue
                if len(argv) < 3:
                    # 散文里提到的命令片段（如「`grep -c` 返回 0」），不是完整命令
                    continue
                total += 1
                hits, samples = grep(argv)
                if hits:
                    print(f"  {card_id}: 命中 ({hits} 行) — {cmd[:88]}")
                    for s in samples:
                        print(f"        └ {s}")
                elif (
                    "<data_dir>" in cmd
                    or "<卡 id>" in cmd
                    or "bot.log" in cmd
                    or "bot-config.json" in cmd
                ):
                    # 运行时路径（占位或本机真路径）：要真机上的真库/真日志才跑得了
                    machine += 1
                    print(f"  {card_id}: 实机待跑（运行时路径） — {cmd[:88]}")
                elif EXPECT_EMPTY_RE.search(line):
                    print(f"  {card_id}: 无命中（判据要求） — {cmd[:88]}")
                else:
                    print(f"  {card_id}: ★ 0 命中 — {cmd[:88]}")
                    suspect.append(f"{card_id}: {cmd}")
    print()
    print(
        f"共 {total} 条 grep 判据；实机待跑 {machine} 条；待查 {len(suspect)} 条"
        "（待查 = 命令在现码里跑不出东西，多半是命令写错或目标改名）"
    )
    for s in suspect:
        print(f"  ★ {s}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
