#!/usr/bin/env bash
# S07 的采集器真坏那一半：把 lsof 的 exec 掐掉，跑「监听采样失败」的端到端判定。
#
# 为什么用沙箱而不是 `sudo mv /usr/sbin/lsof`：
# 改系统文件要 sudo，中途失败可能把 lsof 留在改名状态（别的工具依赖它）；
# 沙箱只对**这个进程**生效，跑完就没了，不留痕迹。
#
# 注意：采样器试的是两个候选（PATH 里的 `lsof` 与绝对路径 `/usr/sbin/lsof`），
# 同一个文件——沙箱按路径禁 exec，两个候选一起失效，才是真正的「采集失败」。
# 只把 PATH 摘掉是**不够**的：绝对路径那个候选照样能跑，采到东西就还是「采成功」。
#
# 用法：bash scripts/sandbox-lsof-test.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE='(version 1)(allow default)(deny process-exec (subpath "/usr/sbin/lsof"))'

if ! command -v sandbox-exec >/dev/null 2>&1; then
  echo "✗ 本机没有 sandbox-exec（macOS 专有）。Linux 侧没有 lsof，采集走 /proc/net/*，"
  echo "  要造「采集失败」得让 /proc/net 不可读——见主文档 S07 卡的说明。"
  exit 1
fi

cd "$ROOT/src-tauri" || exit 1
sandbox-exec -p "$PROFILE" \
  cargo test --offline --lib listening_sample_reports_failure_when_lsof_cannot_run -- --nocapture
