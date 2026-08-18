#!/usr/bin/env bash
# Pre-push full gate: full Rust test suite + pytest。
# Runs in 1-3 分钟。pre-commit 用 test-fast.sh 走快路径（fmt + check）。
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"

echo "[1/2] cargo test --manifest-path src-tauri/Cargo.toml (lib + integration + doctests)"
cargo test --manifest-path src-tauri/Cargo.toml --quiet

echo "[2/2] pytest tests-audit/audit_pre_step_pre_execute.py -v"
python3 -m pytest tests-audit/audit_pre_step_pre_execute.py -v

echo "✓ 全量测试通过"