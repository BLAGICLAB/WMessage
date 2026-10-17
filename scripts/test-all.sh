#!/usr/bin/env bash
# 唯一全量门禁（pre-push 自动跑，也可手动）：cargo nextest 全量 + vitest + 一致性对拍。
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:$PATH"

TOTAL_START=$(date +%s)

echo "[1/3] cargo nextest run (lib + integration tests，并行，比 cargo test 快 30-50%)"
cargo nextest run --manifest-path src-tauri/Cargo.toml --no-fail-fast -j num-cpus

echo "[2/3] npm test (vitest run, 前端 unit 测试)"
npm test

# 对拍脚本两种形态必须分开跑：独立守卫脚本交给 pytest 时只会贡献「一 tests 没收集到」，
# 自己的 main() 压根不执行，同批用例全过就整批退出 0——守卫形同虚设且看不出来。
# 另外文件名不匹配 pytest 默认的 test_*.py glob，整目录 `pytest tests-audit/` 收不到
# 任何用例（no tests ran），所以 pytest 用例必须逐个点名。
# set -e 让任何一步失败直接中断整门禁。
echo "[3/3] tests-audit 一致性对拍（pytest 用例 3 个 + 独立守卫脚本 2 个）"
python3 -m pytest \
    tests-audit/audit_error_codes.py \
    tests-audit/audit_module_map.py \
    tests-audit/audit_tauri_bridge.py \
    -q
python3 tests-audit/audit_evolution_layering.py
python3 tests-audit/audit_no_eval.py

TOTAL_END=$(date +%s)
echo ""
echo "✓ 全量测试通过"
echo "=== 总耗时: $((TOTAL_END - TOTAL_START))s ==="
