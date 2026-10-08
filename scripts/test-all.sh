#!/usr/bin/env bash
# 唯一全量门禁（pre-push 自动跑，也可手动）：cargo nextest 全量 + vitest。
# tests-audit/ 一致性对拍脚本已降级为按需手跑（python3 -m pytest tests-audit/<脚本>.py）。
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"

TOTAL_START=$(date +%s)

echo "[1/2] cargo nextest run (lib + integration tests，并行，比 cargo test 快 30-50%)"
cargo nextest run --manifest-path src-tauri/Cargo.toml --no-fail-fast -j num-cpus

echo "[2/2] npm test (vitest run, 前端 unit 测试)"
npm test

TOTAL_END=$(date +%s)
echo ""
echo "✓ 全量测试通过"
echo "=== 总耗时: $((TOTAL_END - TOTAL_START))s ==="
