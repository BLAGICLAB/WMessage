#!/usr/bin/env bash
# Pre-commit fast gate：智能跳过 + 增量缓存 + 每步计时。
# 目标：< 30s（warm）/ < 10s（只跑相关测试）/ "nothing to test"（无相关改动）。
# 设计意图：每次 commit 不该卡 1-3 分钟；fmt + 编译 + 收集能挡掉 90% 误操作。
#
# 优化（2026-08-18）：
#   1. cargo test → cargo nextest run（并行 + 智能重试，比 cargo test 快 30-50%）
#   2. git diff 智能跳过（只跑改动的代码相关测试）
#   3. sccache 编译缓存（.cargo/config.toml 已配）
#   4. tsc incremental（tsconfig.json 已开）
#   5. vitest --changed 只跑 changed files
#   6. 每步 time + 总耗时统计

set -euo pipefail
cd "$(dirname "$0")/.."

# cargo 在 ~/.cargo/bin 但 macOS 终端默认 PATH 不带，hook 跑在 git 子进程里
# 同样拿不到，所以显式 export。
export PATH="$HOME/.cargo/bin:$PATH"

# ─── 总计时 ───────────────────────────────────────────────
TOTAL_START=$(date +%s)

# ─── 智能跳过：根据改动文件决定跑哪些 gate ────────────────
# pre-commit 阶段看 staged files（--cached）；脚本被手动调用时看 unstaged + staged。
if git diff --cached --name-only 2>/dev/null | grep -q .; then
    CHANGED=$(git diff --cached --name-only)
else
    CHANGED=$(git diff --name-only)
fi

# 也算上 untracked files（新增但未 add）
UNTRACKED=$(git ls-files --others --exclude-standard 2>/dev/null || true)
if [[ -n "$UNTRACKED" ]]; then
    CHANGED="$CHANGED
$UNTRACKED"
fi

# ─── 步骤 0: 审计批次号防线（防注释考古回潮） ─────────────────────────
# staged（无 staged 时看 unstaged）的 .rs/.ts/.tsx 新增行不得带审计批次号
#（批次N审计 / P0-12 / T1-3 / NEW-C-6 等——批次号是审计过程产物，入注释即化石）；
# 确需引用历史口径时行内加 audit-ok 放行。
DIFF_SRC=$(git diff --cached -U0 -- '*.rs' '*.ts' '*.tsx' 2>/dev/null || true)
if [[ -z "$DIFF_SRC" ]]; then
    DIFF_SRC=$(git diff -U0 -- '*.rs' '*.ts' '*.tsx' 2>/dev/null || true)
fi
BAD_LINES=$(echo "$DIFF_SRC" | grep '^+' | grep -v '^+++' \
    | grep -v 'audit-ok' \
    | grep -E '批次[0-9]+审计|\bP[012]-[0-9]+\b|\bT[0-9]-[0-9]+\b|\bNEW-[A-Z]-[0-9]+\b' || true)
if [[ -n "$BAD_LINES" ]]; then
    echo "✗ [0/N] 审计批次号防线：新增行含审计批次号（阶段5起禁止入注释；确需引用加 audit-ok）："
    echo "$BAD_LINES" | head -10 | sed 's/^/    /'
    exit 1
fi
echo "  ✓ [0/N] 审计批次号防线通过"

# ── 是否需要跑 cargo 链路（fmt + check + test） ──
NEED_CARGO=false
if echo "$CHANGED" | grep -qE '^(src-tauri/|Cargo\.toml|Cargo\.lock)'; then
    NEED_CARGO=true
fi

# ── 是否需要跑 tsc + vitest ──
NEED_TS=false
if echo "$CHANGED" | grep -qE '^(src/|tsconfig.*\.json|vite\.config\.ts|vitest\.config\.ts|package\.json)'; then
    NEED_TS=true
fi

# ── 是否需要跑 pytest ──
NEED_PYTEST=false
if echo "$CHANGED" | grep -qE '^tests-audit/'; then
    NEED_PYTEST=true
fi

# 全跳过：返回 0
if [[ "$NEED_CARGO" == false && "$NEED_TS" == false && "$NEED_PYTEST" == false ]]; then
    TOTAL_END=$(date +%s)
    echo "⏭ nothing to test（本次改动不在 src-tauri/ / src/ / tests-audit/）"
    echo "=== pre-commit 总耗时: $((TOTAL_END - TOTAL_START))s ==="
    exit 0
fi

echo "━━━━━━ 改动文件 ━━━━━━"
echo "$CHANGED" | sort -u | head -40 | sed 's/^/  /'
echo ""
echo "  cargo:  $NEED_CARGO"
echo "  ts:     $NEED_TS"
echo "  pytest: $NEED_PYTEST"
echo ""

# ── 步骤计时 helper ──
step() {
    local label="$1"
    local start end
    start=$(date +%s)
    echo "─── $label ─────────────────────────────"
    shift
    if "$@"; then
        end=$(date +%s)
        echo "  ⏱  $label: $((end - start))s"
        return 0
    else
        end=$(date +%s)
        echo "  ✗  $label: FAILED（$((end - start))s）"
        return 1
    fi
}

# ─── 步骤 1: cargo fmt ────────────────────────────────────
if [[ "$NEED_CARGO" == true ]]; then
    step "[1/N] cargo fmt --check" \
        cargo fmt --manifest-path src-tauri/Cargo.toml --check
fi

# ─── 步骤 2: cargo check（编译检查） ───────────────────────
if [[ "$NEED_CARGO" == true ]]; then
    step "[2/N] cargo check (compile-only)" \
        cargo check --manifest-path src-tauri/Cargo.toml --quiet
fi

# ─── 步骤 3: pytest collect-only ──────────────────────────
if [[ "$NEED_PYTEST" == true ]]; then
    step "[3/N] pytest collect-only" \
        python3 -m pytest tests-audit/audit_pre_step_pre_execute.py --collect-only -q
fi

# ─── 步骤 2.5: cargo machete（未使用 Rust 依赖门禁，未安装则 skip） ──
if [[ "$NEED_CARGO" == true ]]; then
    if command -v cargo-machete >/dev/null 2>&1; then
        step "[2.5/N] cargo machete（未使用依赖）" \
            cargo machete src-tauri
    else
        echo "─── [2.5/N] cargo machete：未安装，skip（装：cargo install cargo-machete --locked）"
    fi
fi

# ─── 步骤 3.5: Tauri 桥一致性（命令注册 ↔ 前端 invoke / emit ↔ listen） ──
# 纯文本扫描，<1s；src/ 或 src-tauri/ 有改动就跑（桥两头都在这两个目录）
if [[ "$NEED_CARGO" == true || "$NEED_TS" == true ]]; then
    step "[3.5/N] tauri bridge 一致性（invoke/emit ↔ listen）" \
        python3 -m pytest tests-audit/audit_tauri_bridge.py -q
fi

# ─── 步骤 4: tsc --noEmit（类型检查） ─────────────────────
if [[ "$NEED_TS" == true ]]; then
    step "[4/N] tsc --noEmit (类型检查，incremental 缓存)" \
        npx --no-install tsc --noEmit -p tsconfig.json
fi

# ─── 步骤 4.5: knip（前端死代码/未使用依赖门禁） ──
# knip 已收进 devDependencies，npx 走本地安装（无网络依赖）
if [[ "$NEED_TS" == true ]]; then
    step "[4.5/N] knip（unused files/exports/deps）" \
        npx --no-install knip --no-progress
fi

# ─── 步骤 5: vitest run（前端 unit） ──────────────────────
# vitest --changed 只跑与改动文件相关的测试（基于 git diff）
# 全部未改 → 跳过整个 vitest
if [[ "$NEED_TS" == true ]]; then
    # vitest --changed 在没改 src/ 时仍会跑（基线扫描）。我们再加一层门：
    if echo "$CHANGED" | grep -qE '^(src/|src/components/.*\.test\.tsx?)'; then
        step "[5/N] vitest --changed (只跑改动的测试)" \
            npx --no-install vitest --run --changed
    else
        step "[5/N] vitest run (default)" \
            npx --no-install vitest --run --reporter=default
    fi
fi

TOTAL_END=$(date +%s)
echo ""
echo "✓ fast gate 通过"
echo "=== pre-commit 总耗时: $((TOTAL_END - TOTAL_START))s ==="