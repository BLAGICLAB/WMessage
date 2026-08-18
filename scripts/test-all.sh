#!/usr/bin/env bash
# Pre-push full gate: full Rust test suite + pytest + vitest。
# Runs in 1-3 分钟。pre-commit 用 test-fast.sh 走快路径（fmt + check + vitest）。
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"

echo "[1/3] cargo test --manifest-path src-tauri/Cargo.toml (lib + integration + doctests)"
cargo test --manifest-path src-tauri/Cargo.toml --quiet

echo "[2/3] pytest tests-audit/audit_pre_step_pre_execute.py -v"
python3 -m pytest tests-audit/audit_pre_step_pre_execute.py -v

echo "[3/3] npm test (vitest run, 前端 unit 测试)"
npm test

echo "✓ 全量测试通过"