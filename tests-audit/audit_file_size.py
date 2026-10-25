#!/usr/bin/env python3
"""
源码文件行数门禁：新文件 ≤ 1500 行，存量超限文件只许变小不许变大。

背景：仓库曾有单文件 6699 行（`bot_model_loop.rs`），改一处要在几千行里
翻，评审看不懂、冲突必炸、审查者只能抽查。约定「每个文件 ≤ 1200 行」写了
在 `docs/rust-bot-architecture.md` 的拆分口径里，但**没有任何脚本检查过**——
于是约定形同虚设：拆完 `bot_skills.rs` 当天就全族超标，至今没人发现。

这条门禁不追求「全仓立刻没有大文件」，而是把**方向**钉死：

- **新文件**：一律 ≤ 1500 行。新增超限文件直接拒。
- **存量超限文件**：登记在 `WHITELIST` 里，各自带当前行数上限。
  只许变小——变小了就把新行数更新回清单（棘轮收紧），拆到 ≤1500 就整条销掉；
  变大了就 fail（想加内容就拆文件）。

**2026-10-25：白名单已清空（18 条全部销掉）。** 存量超限的清账过程见
`docs/FILE-SPLIT-PLAN-2026-10-25.md`。清单空着不等于债务清零——它只是
把「18 个文件超标」换成「10 个文件贴着线」：

    p50=394  p75=681  p90=1058  p95=1187  p99=1452  max=1487

阈值 1500 卡在 p99 与最大值之上，只放行百分之一的最长尾巴，且**余量已经很
薄**——离上限最近的 `bot/services/tests.rs`（1487 行）只剩 13 行。往这些文件
加几行就会被拦，这是门禁的设计意图（把「该拆了」的信号持续化），不是意外。

上限取 1500 而不是 1200：1200 落全仓分布的 p75~p90 之间，作为「该拆了」
的信号合理，但作为硬门禁会让拆分压力落到每个中等文件上。1500 留出的余量
足够容纳正常的功能块，又比存量峰值低得多——新文件从第一天就被约束住，
存量靠棘轮慢慢收敛。

**2026-10-25：前端 `.ts/.tsx` 纳管进来。** 此前本脚本只扫 Rust，前端那
4 个 2000+ 行文件（SettingsPage 4068 / ChatPanel.test 3105 /
SettingsPage.test 2985 / ChatPanel 2270）长期无人拦。拆分方案与账本见
`docs/FILE-SPLIT-FRONTEND-2026-10-25.md`：

- 扫描根增加前端 `src/`，后缀 `.ts` / `.tsx`（含测试文件）；
- 阈值沿用 1500（前端口径与 Rust 口径统一）；
- 纳管时前端已无超限文件（最大 `src/App.tsx` 1182 行），白名单无需新增条目；
- 口径从「Rust 行数」改成「前后端统一行数」。

口径：
- 扫描 `src-tauri/src/`、`src-tauri/tests/`、`src-tauri/examples/` 下的
  `*.rs`（`src-tauri/vendor/` 是 vendored 依赖，非业务代码，不在内）。
- 扫描仓库根 `src/` 下的 `*.ts` / `*.tsx`。
- 行数 = 换行符个数（与 `wc -l` 同值）。
- 白名单键：Rust 侧相对 `src-tauri/`（如 `src/bot/x.rs`），前端侧相对仓库根
  （如 `src/components/X.tsx`）。两侧后缀不同，不会撞键。

执行：python3 -m pytest tests-audit/audit_file_size.py -v
"""

from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
CRATE = ROOT / "src-tauri"

# Rust 侧扫描根（相对 src-tauri/）。vendor/ 不在其中——vendored 依赖不是业务代码。
RUST_SCAN_ROOTS = ("src", "tests", "examples")
RUST_SUFFIXES = (".rs",)

# 前端扫描根（相对仓库根）。与 Rust 侧同口径、同阈值。
FRONTEND_SCAN_ROOTS = ("src",)
FRONTEND_SUFFIXES = (".ts", ".tsx")

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
WHITELIST = {}


def _scan(base: Path, roots: tuple[str, ...], suffixes: tuple[str, ...]) -> list[tuple[str, Path]]:
    """(白名单键, 绝对路径) 列表。键 = 相对 base 的 posix 路径。"""
    out: list[tuple[str, Path]] = []
    for r in roots:
        root = base / r
        if not root.exists():
            continue
        for p in root.rglob("*"):
            if not p.is_file() or p.suffix not in suffixes:
                continue
            if "vendor" in p.parts:
                continue
            out.append((p.relative_to(base).as_posix(), p))
    return sorted(out, key=lambda kv: kv[0])


def rust_files() -> list[tuple[str, Path]]:
    """Rust 侧：(相对 src-tauri/ 的路径, 绝对路径)。"""
    return _scan(CRATE, RUST_SCAN_ROOTS, RUST_SUFFIXES)


def frontend_files() -> list[tuple[str, Path]]:
    """前端侧：(相对仓库根的路径, 绝对路径)。"""
    return _scan(ROOT, FRONTEND_SCAN_ROOTS, FRONTEND_SUFFIXES)


def scanned_files() -> list[tuple[str, Path]]:
    """两侧合并：(白名单键, 绝对路径)。白名单与上限检查都按这个口径走。"""
    return rust_files() + frontend_files()


def line_count(path: Path) -> int:
    """行数 = 换行符个数（与 `wc -l` 同值）。"""
    return path.read_bytes().count(b"\n")


def test_scan_scope_is_not_empty():
    """扫描口径不能空转——空目录会让全部门禁「零命中即通过」。

    口径写错（比如把 SCAN_ROOTS 写成不存在的目录）时，脚本会一路绿灯，
    白名单和新文件上限一起失效。Rust 侧与前端侧分别断言——前端刚纳管，
    口径写错同样会整条失效。
    """
    rs = rust_files()
    assert len(rs) > 100, f"Rust 侧只捞到 {len(rs)} 个文件，扫描根可能写错了"
    assert any(p.name == "bot_model_loop.rs" for _, p in rs), "扫描口径漏掉了 src/ 下的业务文件"

    fe = frontend_files()
    assert len(fe) > 100, f"前端侧只捞到 {len(fe)} 个文件，前端扫描根可能写错了"
    assert any(p.name == "App.tsx" for _, p in fe), "前端扫描口径漏掉了 src/ 下的业务组件"
    assert any(p.name.endswith(".test.tsx") for _, p in fe), "前端扫描口径漏掉了测试文件"


def test_new_files_respect_size_limit():
    """新文件（非白名单内）一律 ≤ 1500 行。"""
    over = []
    for rel, p in scanned_files():
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
        p = whitelist_path(rel)
        if p is None:
            continue  # 文件已删除，由 test_whitelist_has_no_stale_entries 报
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
        p = whitelist_path(rel)
        if p is None:
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
    for rel, p in scanned_files():
        if rel in WHITELIST:
            continue
        n = line_count(p)
        if n > NEW_FILE_LIMIT:
            missing.append(f"{rel}: {n} 行")
    assert not missing, (
        "有文件超限但没登记进白名单（登记即豁免，但要先拆小）：\n  " + "\n  ".join(missing)
    )


def whitelist_path(rel: str) -> Path | None:
    """按白名单键定位文件：Rust 侧相对 src-tauri/，前端侧相对仓库根。

    键的后缀决定归属（.rs → Rust 侧，.ts/.tsx → 前端侧），这样两侧的
    相对路径不会互相撞键。
    """
    if rel.endswith(".rs"):
        base = CRATE
    else:
        base = ROOT
    p = base / rel
    return p if p.exists() else None


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-v"]))
