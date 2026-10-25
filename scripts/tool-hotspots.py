#!/usr/bin/env python3
"""工具调用形态分析（上下文成本与工具调用优化方案 P4/P5/P7 的收益依据）。

只读打开执行痕迹库，算四件事：
1. 每轮的并行度（一次模型请求里带几个 tool_calls）——P4 并行引导的直接抓手；
2. 「同类工具连续独占轮数」分布——连续几轮都是同一个工具在独占；
3. 一次提问内工具结果的累计体量——P7 滑动窗触发阈值该定在哪；
4. 工具调用频次——P5 往哪些 schema 加真实示例。

跑法：python3 scripts/tool-hotspots.py [wmessage.db 路径]
默认路径取标准安装位置（macOS）。只读打开，不写库。
"""
import sqlite3
import sys
from collections import Counter, defaultdict

DEFAULT_DB = (
    "/Users/renshi/Library/Application Support/com.renshi.wmessage/wmessage.db"
)


def main() -> None:
    db_path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_DB
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    cur = conn.cursor()
    rows = cur.execute(
        """
        SELECT s.trace_id, s.turn, s.name, COALESCE(LENGTH(s.result), 0)
        FROM exec_spans s
        JOIN exec_traces t ON s.trace_id = t.id
        ORDER BY t.id, s.turn, s.id
        """
    ).fetchall()
    if not rows:
        print("痕迹表里没有 span（应用还没跑过工具，或痕迹已被 30 天保留期清理）")
        return

    print(f"span 总数 {len(rows)}，涉及 trace {len({r[0] for r in rows})} 个")

    # ── 1. 每轮并行度 ──
    per_turn: dict = defaultdict(list)
    for trace_id, turn, name, _ in rows:
        per_turn[(trace_id, turn)].append(name)
    parallelism = Counter(len(v) for v in per_turn.values())
    total_rounds = sum(parallelism.values())
    multi = sum(n for k, n in parallelism.items() if k > 1)
    print("\n[1] 每轮 tool_calls 条数分布（轮 = 模型的一次请求）：")
    for k in sorted(parallelism):
        print(f"  {k} 条/轮: {parallelism[k]:6d} 轮 ({parallelism[k] / total_rounds:6.1%})")
    print(f"  多调用轮（≥2 条）占 {multi / total_rounds:.1%}")

    # ── 2. 同类工具连续独占轮数 ──
    runs: list = []
    for trace_id in {r[0] for r in rows}:
        seq = sorted(
            (turn, names) for (t, turn), names in per_turn.items() if t == trace_id
        )
        cur_tool, cur_len = None, 0
        for _turn, names in seq:
            uniq = set(names)
            tool = uniq.pop() if len(uniq) == 1 else None
            if tool is not None and tool == cur_tool:
                cur_len += 1
            else:
                if cur_tool is not None:
                    runs.append((cur_tool, cur_len))
                cur_tool, cur_len = tool, 1
        if cur_tool is not None:
            runs.append((cur_tool, cur_len))
    run_len = Counter(length for _tool, length in runs)
    print("\n[2] 同类工具连续独占轮数分布（连续独占段的长度）：")
    for k in sorted(run_len):
        print(f"  {k:2d} 轮: {run_len[k]:6d} 段")
    long_rounds = sum(n for k, n in run_len.items() if k >= 3 for _ in range(n))
    print(f"  连续独占 ≥3 轮的轮次占比: {long_rounds / total_rounds:.1%}")
    by_tool: dict = defaultdict(list)
    for tool, length in runs:
        by_tool[tool].append(length)
    print("  按工具看连续独占段（段数 / 最长 / 平均）：")
    for tool, lens in sorted(by_tool.items(), key=lambda kv: -max(kv[1])):
        print(
            f"    {tool:24s} {len(lens):5d} 段  最长 {max(lens):3d}  平均 {sum(lens) / len(lens):.1f}"
        )

    # ── 3. 一次提问内工具结果累计体量 ──
    per_trace: dict = defaultdict(int)
    for trace_id, _turn, _name, blen in rows:
        per_trace[trace_id] += blen
    sizes = sorted(per_trace.values())

    def pct(seq, p):
        return seq[min(len(seq) - 1, int(len(seq) * p))]

    print("\n[3] 一次提问内工具结果累计体量（字符；= P7 滑动窗的触发口径）：")
    print(
        f"  trace 数 {len(sizes)}  p50 {pct(sizes, .5)}  p75 {pct(sizes, .75)}  "
        f"p90 {pct(sizes, .9)}  p95 {pct(sizes, .95)}  max {sizes[-1]}"
    )
    for thr in (8000, 12000, 24000, 48000, 72000):
        over = sum(1 for s in sizes if s > thr)
        print(f"  超过 {thr:6d} 字符: {over:3d}/{len(sizes)} ({over / len(sizes):5.1%})")

    # ── 4. 工具调用频次 ──
    freq = Counter(name for _t, _turn, name, _b in rows)
    print("\n[4] 工具调用频次 Top 25：")
    for name, n in freq.most_common(25):
        print(f"  {name:24s} {n:5d}")
    print(f"\n不同工具数 {len(freq)}，调用总数 {sum(freq.values())}")

    conn.close()


if __name__ == "__main__":
    main()
