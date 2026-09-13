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

echo "[2/3] pytest tests-audit/（一致性检查）"
python3 -m pytest tests-audit/audit_pre_step_pre_execute.py tests-audit/audit_tauri_bridge.py tests-audit/audit_error_codes.py tests-audit/audit_module_map.py -v

echo "[3/3] npm test (vitest run, 前端 unit 测试)"
npm test

TOTAL_END=$(date +%s)
echo ""
echo "✓ 全量测试通过"
echo "=== pre-push 总耗时: $((TOTAL_END - TOTAL_START))s ==="