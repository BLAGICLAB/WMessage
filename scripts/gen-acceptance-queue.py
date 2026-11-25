#!/usr/bin/env python3
"""从验收矩阵生成实机队列文档。

`docs/acceptance/realmachine-queue.md` 是矩阵的**投影**：未验行的实机欠账
按族分组排队。矩阵一改就要重新生成，否则两处会互相矛盾（矩阵写已验、
队列还挂着待跑——正是 D-7 记的那个缺陷换了个马甲）。

用法：python3 scripts/gen-acceptance-queue.py
"""

import csv
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
MATRIX = ROOT / "docs" / "acceptance" / "matrix.csv"
OUT = ROOT / "docs" / "acceptance" / "realmachine-queue.md"

FAM_TITLES = {
    "ACC-K": "K 族 · 任务页 / 看板",
    "ACC-C": "C 族 · 聊天入口与聊天窗",
    "ACC-T": "T 族 · 工具、技能与 MCP",
    "ACC-E": "E 族 · 工具执行面与错误文案",
    "ACC-O": "O 族 · Agent 运行时与预算",
    "ACC-G": "G 族 · 治理与权限",
    "ACC-P": "P 族 · 编排与平台",
    "ACC-I": "I 族 · 交互细节",
}


def main() -> int:
    with MATRIX.open(encoding="utf-8") as f:
        rows = list(csv.DictReader(f))
    pool = [r for r in rows if r["状态"] == "未验" and "L1" in r["验收层"]]

    items = []
    for r in pool:
        gap = r["缺口说明"]
        if "缺实机：" not in gap:
            continue
        items.append(
            {
                "id": r["功能ID"],
                "card": r["新卡号"] if r["新卡号"] not in ("", "—") else r["既有卡号"],
                "real": gap.split("缺实机：", 1)[1].strip().rstrip("。"),
                "l1_gap": "L1 也缺" in gap,
            }
        )
    items.sort(key=lambda x: (int(x["id"].split("-")[0]), x["id"]))

    groups: dict[str, list] = {}
    for it in items:
        key = it["card"].rsplit("-", 1)[0] if it["card"].startswith("ACC-") else "其他"
        groups.setdefault(key, []).append(it)
    order = sorted(groups, key=lambda k: min(int(i["id"].split("-")[0]) for i in groups[k]))

    l1_all = sum(1 for i in items if not i["l1_gap"])

    lines = [
        "# 实机队列（未验行的实机欠账）",
        "",
        "> **这是什么**：验收矩阵里「未验」且验收层含 L1 的行，逐条核过 L1 之后剩下的**实机欠账**",
        "> 排队表。每行给：矩阵功能 ID（回填用）、验收卡号（**卡里的步骤/判据/证据才是可跑的**）、",
        "> 实机要点（从矩阵「缺实机」段摘出）、以及 L1 是否也有缺。",
        ">",
        "> **怎么用**：按卡号逐张跑——卡里已写好前置 / 步骤 / 判据 / 证据 / 失败处置。跑完把结论回填到",
        "> `docs/acceptance/matrix.csv` 对应行的「缺口说明」，并**同批回写该卡的「缺口补验」段**",
        "> （D-7 纪律：矩阵写已验、卡面写未验，两处互相矛盾正是那个缺陷）。",
        ">",
        "> **本文件是矩阵的投影**：由 `python3 scripts/gen-acceptance-queue.py` 生成——矩阵一改就重新生成，",
        "> 否则两处会互相矛盾。",
        ">",
        f"> 口径：矩阵未验行 {len(pool)} 条，实机要点可摘出的 {len(items)} 条；其中 **L1 全过（只缺实机）{l1_all} 条**、",
        f"> **L1 也有缺 {len(items) - l1_all} 条**——后者实机之前先补自动化，否则跑出来的结论没有长期约束力。",
        "",
    ]

    for fam_key in order:
        grp = groups[fam_key]
        title = FAM_TITLES.get(fam_key, fam_key)
        lines.append(f"## {title}（{len(grp)} 行）")
        lines.append("")
        lines.append("| 功能 ID | 验收卡 | 实机要点 | L1 |")
        lines.append("|---|---|---|---|")
        for it in grp:
            l1mark = "也缺（先补自动化）" if it["l1_gap"] else "全过"
            lines.append(f"| {it['id']} | {it['card']} | {it['real']} | {l1mark} |")
        lines.append("")

    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"已生成 {OUT.relative_to(ROOT)}：{len(items)} 行 / {len(order)} 组")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
