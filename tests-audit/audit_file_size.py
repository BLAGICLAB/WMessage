#!/usr/bin/env python3
"""
源码文件行数门禁：新文件 ≤ 1500 行，存量超限文件只许变小不许变大。

背景：仓库曾有单文件 6699 行（`bot_model_loop.rs`），改一处要在几千行里
翻，评审看不懂、冲突必炸、审查者只能抽查。约定「每个文件 ≤ 1200 行」写了
在 `docs/rust-bot-architecture.md` 的拆分口径里，但**没有任何脚本检查过**——
于是约定形同虚设：拆完 `bot_skills.rs` 当天就全族超标，至今没人发现。

这条门禁不追求「全仓立刻没有大文件」（那不现实：`bot/registry.rs` 3827 行
是工具注册表，`db/trace.rs` 3040 行是写路径，拆它们是独立批次的事），
而是把**方向**钉死：

- **新文件**：一律 ≤ 1500 行。新增超限文件直接拒。
- **存量超限文件**：登记在 `WHITELIST` 里，各自带当前行数上限。
  只许变小——变小了就把新行数更新回清单（棘轮收紧），拆到 ≤1500 就整条销掉；
  变大了就 fail（想加内容就拆文件）。

上限取 1500 而不是 1200：1200 落全仓分布的 p75~p90 之间，作为「该拆了」
的信号合理，但作为硬门禁会让拆分压力落到每个中等文件上。1500 是拆分后
最大子模块（925 行）留出的余量，同时比存量峰值低得多——新文件从第一天
就被约束住，存量靠棘轮慢慢收敛。

口径：
- 扫描 `src-tauri/src/`、`src-tauri/tests/`、`src-tauri/examples/` 下的
  `*.rs`（`src-tauri/vendor/` 是 vendored 依赖，非业务代码，不在内）。
- 行数 = 换行符个数（与 `wc -l` 同值）。
- **前端 `.ts/.tsx` 不在本门禁口径内**——那条 ≤1200 约定出自 Rust bot
  架构文档，前端另有目录与构建约束。前端最大文件（SettingsPage.tsx 4068
  行）要治是独立批次的事，不在本脚本混进来。

执行：python3 -m pytest tests-audit/audit_file_size.py -v
"""

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "src-tauri"

# 扫描根（相对 src-tauri/）。vendor/ 不在其中——vendored 依赖不是业务代码。
SCAN_ROOTS = ("src", "tests", "examples")

# 新文件行数上限
NEW_FILE_LIMIT = 1500

# 存量超限文件白名单：路径 → 登记时的行数上限（只许变小不许变大）。
#
# 形状同 audit.rs 的 freeform 冻结白名单：白名单内的文件豁免「新文件上限」，
# 但按登记时的行数卡死——比登记时小才放行。
#
# 维护方式：
# - 文件变小 → 把上限更新成新行数（棘轮收紧）；拆到 ≤1500 就整条销掉
# - 文件变大 → 门禁 fail；要么拆文件，要么先拆再加
# - 文件删除 → 整条销掉
WHITELIST = {
    "src/bot/registry.rs": 3827,
    "src/py/document.rs": 2456,
    "src/bot/services.rs": 2391,
}


def source_files():
    """扫描口径内的全部 .rs 文件（相对 src-tauri/ 的路径）。"""
    out = []
    for r in SCAN_ROOTS:
        base = CRATE / r
        if not base.exists():
            continue
        out.extend(p for p in base.rglob("*.rs") if "vendor" not in p.parts)
    return sorted(out)


def line_count(path: Path) -> int:
    """行数 = 换行符个数（与 `wc -l` 同值）。"""
    return path.read_bytes().count(b"\n")


def test_scan_scope_is_not_empty():
    """扫描口径不能空转——空目录会让全部门禁「零命中即通过」。

    口径写错（比如把 SCAN_ROOTS 写成不存在的目录）时，脚本会一路绿灯，
    白名单和新文件上限一起失效。
    """
    files = source_files()
    assert len(files) > 100, f"扫描口径只捞到 {len(files)} 个文件，SCAN_ROOTS 可能写错了"
    assert any(p.parts[-1] == "bot_model_loop.rs" for p in files), "扫描口径漏掉了 src/ 下的业务文件"


def test_new_files_respect_size_limit():
    """新文件（非白名单内）一律 ≤ 1500 行。"""
    over = []
    for p in source_files():
        rel = p.relative_to(CRATE).as_posix()
        if rel in WHITELIST:
            continue
        n = line_count(p)
        if n > NEW_FILE_LIMIT:
            over.append(f"{rel}: {n} 行（上限 {NEW_FILE_LIMIT}）")
    assert not over, (
        "新文件超出行数上限（拆成子模块再提交，或把它登记进白名单）：\n  "
        + "\n  ".join(over)
    )


def test_whitelisted_files_only_shrink():
    """白名单内的存量超限文件只许变小，不许变大。

    这是棘轮：文件被拆小后把上限更新成新行数，债务只减不增；
    反过来往白名单文件里加内容会 fail，逼着拆文件而不是继续堆。
    """
    grown = []
    for rel, limit in sorted(WHITELIST.items()):
        p = CRATE / rel
        if not p.exists():
            continue  # 文件已删除，由 test_frozen_list_has_no_stale_entries 报
        n = line_count(p)
        if n > limit:
            grown.append(f"{rel}: 现 {n} 行 > 登记上限 {limit} 行（拆文件，或先拆再加）")
    assert not grown, "白名单文件变大（只许变小）：\n  " + "\n  ".join(grown)


def test_whitelist_has_no_stale_entries():
    """白名单必须与现状一一对应：文件已删或已降到上限内就得销掉条目。

    条目烂在清单里会有两个坏处：一是「超限文件」这个说法失真（清单成了
    历史名录而不是现状）；二是真正的新超限文件混在里面时看不出来——
    清单是唯一决定「谁被豁免」的地方，脏了就失去筛选力。
    """
    stale = []
    for rel, limit in sorted(WHITELIST.items()):
        p = CRATE / rel
        if not p.exists():
            stale.append(f"{rel}: 文件已不存在 → 从白名单销掉")
            continue
        n = line_count(p)
        if n <= NEW_FILE_LIMIT:
            stale.append(
                f"{rel}: 现 {n} 行 ≤ {NEW_FILE_LIMIT} → 已不超限，从白名单整条销掉"
            )
    assert not stale, "白名单有过期条目（白名单只登记「当前仍超限」的文件）：\n  " + "\n  ".join(stale)


def test_whitelist_covers_every_over_limit_file():
    """所有仍超 1500 行的文件都必须在白名单里。

    这条是上一条的镜像：清单漏登记 = 新超限文件绕过了上限检查
    （`test_new_files_respect_size_limit` 会把它当新文件拦，但那时报的是
    「请登记进白名单」而不是「这文件超限」，容易被人随手登记了事）。
    """
    missing = []
    for p in source_files():
        rel = p.relative_to(CRATE).as_posix()
        if rel in WHITELIST:
            continue
        n = line_count(p)
        if n > NEW_FILE_LIMIT:
            missing.append(f"{rel}: {n} 行")
    assert not missing, (
        "有文件超限但没登记进白名单（登记即豁免，但要先拆小）：\n  " + "\n  ".join(missing)
    )


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
