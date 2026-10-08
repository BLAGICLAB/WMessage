#!/usr/bin/env bash
# 已下线（2026-10-08 基线收敛）：本脚本曾是 .githooks/pre-commit 的生成器，
# 与仓库里版本化的 hook 逐字重复。hooks 安装一律走 scripts/install-hooks.sh。
echo "install-git-hook.sh 已废弃，请用 scripts/install-hooks.sh" >&2
exit 1
