#!/usr/bin/env bash
# Pre-push full gate: full Rust test suite + pytest + vitest。
# 优化（2026-08-18）：cargo test → cargo nextest run（并行 + 智能重试）。
# pre-commit 用 test-fast.sh 走快路径（fmt + check + vitest）。
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"

TOTAL_START=$(date +%s)

echo "[1/3] cargo nextest run (lib + integration tests，并行，比 cargo test 快 30-50%)"
cargo nextest run --manifest-path src-tauri/Cargo.toml --no-fail-fast -j num-cpus

echo "[2/3] pytest tests-audit/（一致性检查，strict：XFAIL/XPASS 也算失败）"
# X 必须在模板末尾（BSD mktemp）：带 .log 后缀会 mkstemp 失败/残留字面量文件，
# /tmp 残留同名文件后每次 push 必撞 "File exists"（实锤：2026-10-06 推送三连挂）
PYTEST_LOG=$(mktemp /tmp/pytest-audit.XXXXXX)
# EXIT trap 兜底清理：pytest 失败时 errexit 会在下一行管道处直接退出，
# 没有 trap 就会漏删（/tmp 残留同名文件正是「推送三连挂」的根因）
trap 'rm -f "$PYTEST_LOG"' EXIT
# pytest 退出码收进管道组末尾的赋值：pytest 失败时管道组整体仍以 0 退出，
# errexit 不会在 xfail 扫描与 PYTEST_EXIT 透传前中断（否则那两段是死代码）
PYTEST_EXIT=0
{ python3 -m pytest tests-audit/audit_pre_step_pre_execute.py tests-audit/audit_tauri_bridge.py tests-audit/audit_error_codes.py tests-audit/audit_module_map.py -v 2>&1; PYTEST_EXIT=$?; } | tee "$PYTEST_LOG"
# 锚定 pytest 总结行（行首 = 填充）：-v 转录正文出现「N xfailed」字样（docstring/参数化 id）不再误触发
if grep -qE '^=+ .*[1-9][0-9]* xfailed' "$PYTEST_LOG" || grep -qE '^=+ .*[1-9][0-9]* xpassed' "$PYTEST_LOG"; then
    echo "✗ tests-audit 存在 xfail/xpass（Phase 6 政策：0 xfail 门禁）" >&2
    rm -f "$PYTEST_LOG"
    exit 1
fi
rm -f "$PYTEST_LOG"
if [[ $PYTEST_EXIT -ne 0 ]]; then
    exit "$PYTEST_EXIT"
fi

echo "[2.5/3] evolution 分层依赖方向守卫（策略纯度/上下文边界/数据层方向）"
if ! python3 tests-audit/audit_evolution_layering.py; then
    echo "✗ evolution 分层守卫失败（分层契约见 docs/EVOLUTION-LAYERING-BATCH-A.md §8）" >&2
    exit 1
fi

echo "[2.6/3] 前端禁动态求值静态守卫（CSP 收紧配套）"
if ! python3 tests-audit/audit_no_eval.py; then
    echo "✗ 前端禁 eval 守卫失败（评估见 docs/CSP-TIGHTEN-VERIFY-2026-10-07.md §8）" >&2
    exit 1
fi

echo "[3/3] npm test (vitest run, 前端 unit 测试)"
npm test

TOTAL_END=$(date +%s)
echo ""
echo "✓ 全量测试通过"
echo "=== pre-push 总耗时: $((TOTAL_END - TOTAL_START))s ==="