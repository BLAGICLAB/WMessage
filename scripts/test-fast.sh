#!/usr/bin/env bash
# Pre-commit fast gate：智能跳过 + 增量缓存 + 每步计时。
# 目标：< 30s（warm）/ < 10s（只跑相关测试）/ "nothing to test"（无相关改动）。
# 设计意图：每次 commit 不该卡 1-3 分钟；fmt + 编译 + 收集能挡掉 90% 误操作。
#
# 步骤总览（详见 docs/testing.md；编号即运行时输出里的 [N/N]）：
#   [0/N]   工单号防线：staged/unstaged 新增行 + untracked 全文的 .rs/.ts/.tsx
#           含工单号/拍板记录模式拒提交（这些只进 DEVLOG，不进源码；
#           audit-ok 行内豁免）——纯文本检查，<1s
#   [1/N]   cargo fmt --check：格式漂移
#   [2/N]   cargo check：编译错误（增量缓存）
#   [2.5/N] cargo machete src-tauri：未使用 Rust 依赖（未安装则 skip 并提示）
#   [3/N]   tsc --noEmit：前端类型检查（incremental）
#   [3.5/N] knip --no-progress：前端死文件/死 export/未使用 npm 依赖
#           （knip 在 devDependencies，npx --no-install 走本地安装）
#   [3.6/N] oxlint（前端 linter，error 级门禁）
#   [4/N]   vitest --changed：前端单测（只跑改动相关）
#
# 性能设计：git diff 智能跳过（只跑改动相关的链路）；tsc/vitest 各有增量机制；
# 全量测试（cargo nextest + vitest）在 pre-push 的 test-all.sh。
# tests-audit/ 一致性对拍脚本（tauri 桥/错误码/模块地图/分层）已降级为按需手跑。

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

# ─── 步骤 0: 工单号防线（工单号/拍板记录只进 DEVLOG，不进源码） ───────────────
# 扫描口径 = diff 新增行（staged，无 staged 时看 unstaged）+ untracked 新文件全文：
# untracked 不进任何 diff，不扫则新写文件不 add 即可绕过（下游 NEED_* 判定本就含 untracked）。
# 禁入模式：批次N审计 / P0-12 / T1-3 / NEW-C-6 / W8-ATTACH / OCR C5 / B0-1 / AUDIT-* / 「拍板」；
# 确需引用历史口径时行内加 audit-ok 放行。
DIFF_SRC=$(git diff --cached -U0 -- '*.rs' '*.ts' '*.tsx' 2>/dev/null || true)
if [[ -z "$DIFF_SRC" ]]; then
    DIFF_SRC=$(git diff -U0 -- '*.rs' '*.ts' '*.tsx' 2>/dev/null || true)
fi
NEW_LINES=$(echo "$DIFF_SRC" | grep '^+' | grep -v '^+++' || true)
# untracked 的 .rs/.ts/.tsx 全文并入同一扫描流（-z 读法防空格/特殊字符路径被引号包裹漏读）
while IFS= read -r -d '' f; do
    if [[ -f "$f" ]]; then
        NEW_LINES="$NEW_LINES
$(cat "$f")"
    fi
done < <(git ls-files -z --others --exclude-standard -- '*.rs' '*.ts' '*.tsx' 2>/dev/null)
BAD_LINES=$(echo "$NEW_LINES" \
    | grep -v 'audit-ok' \
    | grep -E '批次[0-9]+审计|\bP[012]-[0-9]+\b|\bT[0-9]-[0-9]+\b|\bNEW-[A-Z]-[0-9]+\b|\bW[0-9]+-[A-Z0-9]+|\bB[0-9]+-[0-9A-Z]|\bOCR[ -][CcRr][0-9]|\bAUDIT-[A-Z0-9]+|\bSUBA-[0-9]+|拍板' || true)
if [[ -n "$BAD_LINES" ]]; then
    echo "✗ [0/N] 工单号防线：diff 新增行 / untracked 文件含工单号或拍板记录（只进 DEVLOG，不进源码；确需引用加 audit-ok）："
    echo "$BAD_LINES" | head -10 | sed 's/^/    /'
    exit 1
fi
echo "  ✓ [0/N] 工单号防线通过"

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

# 全跳过：返回 0
if [[ "$NEED_CARGO" == false && "$NEED_TS" == false ]]; then
    TOTAL_END=$(date +%s)
    echo "⏭ nothing to test（本次改动不在 src-tauri/ / src/）"
    echo "=== pre-commit 总耗时: $((TOTAL_END - TOTAL_START))s ==="
    exit 0
fi

echo "━━━━━━ 改动文件 ━━━━━━"
echo "$CHANGED" | sort -u | head -40 | sed 's/^/  /'
echo ""
echo "  cargo:  $NEED_CARGO"
echo "  ts:     $NEED_TS"
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

# ─── 步骤 2.5: cargo machete（未使用 Rust 依赖门禁，未安装则 skip） ──
if [[ "$NEED_CARGO" == true ]]; then
    if command -v cargo-machete >/dev/null 2>&1; then
        step "[2.5/N] cargo machete（未使用依赖）" \
            cargo machete src-tauri
    else
        echo "─── [2.5/N] cargo machete：未安装，skip（装：cargo install cargo-machete --locked）"
    fi
fi

# ─── 步骤 3: tsc --noEmit（类型检查） ─────────────────────
if [[ "$NEED_TS" == true ]]; then
    step "[3/N] tsc --noEmit (类型检查，incremental 缓存)" \
        npx --no-install tsc --noEmit -p tsconfig.json
fi

# ─── 步骤 3.5: knip（前端死代码/未使用依赖门禁） ──
# knip 已收进 devDependencies，npx 走本地安装（无网络依赖）
if [[ "$NEED_TS" == true ]]; then
    step "[3.5/N] knip（unused files/exports/deps）" \
        npx --no-install knip --no-progress
fi

# ─── 步骤 3.6: oxlint（前端 linter） ──
# 规则配置见 .oxlintrc.json。
# 门禁口径：默认退出码——error 拦（rules-of-hooks 等），warning 放行。
if [[ "$NEED_TS" == true ]]; then
    step "[3.6/N] oxlint（error 级门禁）" \
        npx --no-install oxlint src
fi

# ─── 步骤 4: vitest run（前端 unit） ──────────────────────
# vitest --changed 只跑与改动文件相关的测试（基于 git diff）
# 全部未改 → 跳过整个 vitest
if [[ "$NEED_TS" == true ]]; then
    # vitest --changed 在没改 src/ 时仍会跑（基线扫描）。我们再加一层门：
    if echo "$CHANGED" | grep -qE '^(src/|src/components/.*\.test\.tsx?)'; then
        step "[4/N] vitest --changed (只跑改动的测试)" \
            npx --no-install vitest --run --changed
    else
        step "[4/N] vitest run (default)" \
            npx --no-install vitest --run --reporter=default
    fi
fi

TOTAL_END=$(date +%s)
echo ""
echo "✓ fast gate 通过"
echo "=== pre-commit 总耗时: $((TOTAL_END - TOTAL_START))s ==="