#!/usr/bin/env bash
# eval 一键脚本：真模型 x 生产提示词 x 记录型 stub（花真钱，跑前见确认）。
# 被测面与判据见 src-tauri/src/bot/eval.rs 头注释；「改了必跑」口径见 docs/testing.md。
#
# 用法：
#   bash scripts/run-eval.sh                                # 提示词 eval 全量（13 用例）
#   WMESSAGE_EVAL_ONLY=E05,E11 bash scripts/run-eval.sh     # 只跑个别用例
#   bash scripts/run-eval.sh --memory                       # 末尾追加记忆黄金集（recall@1/@5，本地 ONNX，不花模型钱）
#
# 产出：PASS/FAIL 明细 + 汇总 + tokens；数字与偏差记入 DEVLOG（纪律，不进门禁）。
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MANIFEST="$ROOT/src-tauri/Cargo.toml"

echo "eval 会调用真实模型消耗账户额度；配置快照 src-tauri/eval-bot-config.json 只含连接参数，"
printf 'key 走系统 keyring。按回车继续，Ctrl-C 取消：'
# 非交互（EOF）= 未确认，零执行退出
read -r || exit 1

if [ ! -f "$ROOT/src-tauri/eval-bot-config.json" ]; then
  echo "缺评估配置快照 src-tauri/eval-bot-config.json：按 src-tauri/src/bot/eval.rs 头注释先拷贝" >&2
  exit 1
fi

status=0

echo "== 提示词 eval（bot::eval）=="
cargo test --manifest-path "$MANIFEST" --lib bot::eval -- --ignored --nocapture || status=1

if [ "${1:-}" = "--memory" ]; then
  echo "== 记忆黄金集（tests/memory_eval.rs eval_recall_report）=="
  cargo test --manifest-path "$MANIFEST" --test memory_eval eval_recall_report -- --ignored --nocapture || status=1
fi

echo "== 收尾 =="
echo "把 PASS/FAIL 数字与偏差记入 DEVLOG；口径见 docs/testing.md。"
exit "$status"
