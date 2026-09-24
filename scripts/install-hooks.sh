#!/usr/bin/env bash
# 一次性安装：把 git 指向版本化的 .githooks/ 目录（不是 .git/hooks/）。
# 幂等——再跑一次没副作用。安装到本仓库 only，不动 global config。
set -euo pipefail
cd "$(dirname "$0")/.."

HOOKS_DIR="$(pwd)/.githooks"

# 确保 hook 和脚本可执行（clone 后权限可能丢）
# nullglob + 空守卫：目录缺失/为空（partial checkout）时 glob 不字面展开，chmod 不中止安装
shopt -s nullglob
_hooks=(.githooks/*)
_scripts=(scripts/*.sh)
if [ ${#_hooks[@]} -gt 0 ]; then chmod +x "${_hooks[@]}"; fi
if [ ${#_scripts[@]} -gt 0 ]; then chmod +x "${_scripts[@]}"; fi

# 本仓库 only：core.hooksPath 是 repo-local 配置，不会影响其它项目
git config core.hooksPath "$HOOKS_DIR"

echo "✓ git hooks installed"
echo "  core.hooksPath = $HOOKS_DIR"
echo "  pre-commit     → scripts/test-fast.sh   (~10-30s)"
echo "  pre-push       → scripts/test-all.sh    (~1-3 min)"
echo ""
echo "想临时跳过：git commit --no-verify / git push --no-verify"