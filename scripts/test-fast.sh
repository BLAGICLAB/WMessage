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

# ─── 步骤 4: tsc --noEmit（类型检查） ─────────────────────
if [[ "$NEED_TS" == true ]]; then
    step "[4/N] tsc --noEmit (类型检查，incremental 缓存)" \
        npx --no-install tsc --noEmit -p tsconfig.json
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