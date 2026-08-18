#!/usr/bin/env bash
# Pre-commit fast gate: format check + minimal compile check + pytest sanity.
# Runs in ~10-30s. Full suite lives in test-all.sh (pre-push).
# 设计意图：每次 commit 不该卡 1-3 分钟；fmt + 编译 + 收集能挡掉 90% 误操作。
set -euo pipefail
cd "$(dirname "$0")/.."

# cargo 在 ~/.cargo/bin 但 macOS 终端默认 PATH 不带，hook 跑在 git 子进程里
# 同样拿不到，所以显式 export。
export PATH="$HOME/.cargo/bin:$PATH"

echo "[1/3] cargo fmt --check"
cargo fmt --manifest-path src-tauri/Cargo.toml --check

echo "[2/3] cargo check (compile-only, 不跑测试)"
cargo check --manifest-path src-tauri/Cargo.toml --quiet

echo "[3/3] pytest collect-only (sanity, 不跑断言)"
python3 -m pytest tests-audit/audit_pre_step_pre_execute.py --collect-only -q

echo "✓ fast gate 通过"