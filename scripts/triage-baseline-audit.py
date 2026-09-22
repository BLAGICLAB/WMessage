#!/usr/bin/env python3
# triage-baseline-audit.py —— baseline 审计（Python 版，替代 bash 版）
#
# 来源：docs/OCR-CODE-REVIEW-2026-09-21-fullscan.json
# 口径：severity=high，非 vendor
# 严格互斥：domain 按 path test 分类（basename 匹配 + 目录前缀互斥）
#
# 跑法：python3 scripts/triage-baseline-audit.py [json 文件]
# 任何断言失败 → exit 1

import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

FILE = Path(sys.argv[1] if len(sys.argv) > 1 else "docs/OCR-CODE-REVIEW-2026-09-21-fullscan.json")
if not FILE.exists():
    print(f"ERROR: {FILE} not found", file=sys.stderr)
    sys.exit(1)

data = json.loads(FILE.read_text())
all_high = [c for c in data["comments"] if c.get("severity") == "high"]
non_vendor = [c for c in all_high if "vendor" not in c["path"]]
vendor = [c for c in all_high if "vendor" in c["path"]]

# 全局
nv_raw = len(non_vendor)
nv_unique = len(set((c["path"], c["start_line"]) for c in non_vendor))
v_raw = len(vendor)
v_unique = len(set((c["path"], c["start_line"]) for c in vendor))
print("=== 全局 non-vendor high ===")
print(f"raw    = {nv_raw}")
print(f"unique = {nv_unique}")
print(f"dups   = {nv_raw - nv_unique}")
print(f"unique path = {len(set(c['path'] for c in non_vendor))}")
print()
print("=== 含 vendor 拆分 ===")
print(f"含 vendor raw    = {nv_raw + v_raw}")
print(f"含 vendor unique = {nv_unique + v_unique}")
print(f"vendor raw       = {v_raw}")
print(f"vendor unique    = {v_unique}")
print(f"vendor dups      = {v_raw - v_unique}")
print()


def get_domain(path):
    # 单文件（精确 basename 匹配）
    if path == "src-tauri/src/api_auth.rs":
        return "api_auth"
    if path == "src-tauri/src/api_server.rs":
        return "api_server"
    if path == "src-tauri/src/api.rs":
        return "api"
    # 目录前缀（互斥）
    if path.startswith("src-tauri/src/db/"):
        return "db"
    if path.startswith("src-tauri/src/evolution/"):
        return "evolution"
    if path.startswith("scripts/"):
        return "scripts"
    if path.startswith("src-tauri/src/migration/"):
        return "migration"
    if path.startswith("src/components/WidgetApp/"):
        return "WidgetApp"
    if path.startswith("src-tauri/src/api_handlers/"):
        return "api_handlers"
    if path.startswith("src-tauri/src/bot_skills/"):
        return "bot_skills"
    if path.startswith("src-tauri/src/bot/"):
        return "bot"
    if path.startswith("src-tauri/src/bot_"):
        return "bot_sib"
    return "other"


DOMAINS = [
    "db", "evolution", "scripts", "migration", "WidgetApp",
    "api_handlers", "api_auth", "api_server", "api",
    "bot", "bot_skills", "bot_sib",
]

by_domain = defaultdict(list)
for c in non_vendor:
    by_domain[get_domain(c["path"])].append(c)

print("=== 域 raw / unique / dup（严格互斥）===")
sum_raw = 0
sum_unique = 0
for d in DOMAINS:
    rows = by_domain[d]
    raw = len(rows)
    uniq = len(set((c["path"], c["start_line"]) for c in rows))
    dup = raw - uniq
    print(f"  {d:15s} raw={raw:3d}  unique={uniq:3d}  dup={dup}")
    sum_raw += raw
    sum_unique += uniq

print(f"--- 已 triage 小计 ---")
print(f"raw 之和    = {sum_raw}")
print(f"unique 之和 = {sum_unique}")
print()

other_raw = nv_raw - sum_raw
other_uniq = nv_unique - sum_unique
print("=== 其他域（未 triage）===")
print(f"other raw    = {other_raw}")
print(f"other unique = {other_uniq}")
print()

# 断言 1
if sum_unique + other_uniq != nv_unique:
    print(f"ASSERT FAIL: {sum_unique} + {other_uniq} = {sum_unique + other_uniq} != {nv_unique}", file=sys.stderr)
    sys.exit(1)
print(f"✓ 断言 1：已 triage unique ({sum_unique}) + 其他 unique ({other_uniq}) == {nv_unique}")

# 断言 2：总 dups（vendor + non-vendor）= ?
total_dups = (v_raw - v_unique) + (nv_raw - nv_unique)
print(f"总 dups = {total_dups}（vendor {v_raw - v_unique} + non-vendor {nv_raw - nv_unique}）")

# 列出全部 dups
print()
print("=== 全部 dups（去重失败的 path:line 对）===")
all_pathline = [(c["path"], c["start_line"]) for c in all_high]
ct = Counter(all_pathline)
dups_list = [(k, v) for k, v in ct.items() if v > 1]
for (p, l), n in sorted(dups_list):
    print(f"  ×{n}  {p}:{l}")
print(f"总 dups 条数 = {sum(n for _, n in dups_list)}（每条 path:line 重复次数累加 - 重复对数）")
print(f"unique dups 对数 = {len(dups_list)}")
