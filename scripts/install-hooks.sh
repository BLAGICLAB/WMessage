#!/usr/bin/env bash
# 一次性安装：把 git 指向版本化的 .githooks/ 目录（不是 .git/hooks/）。
# 幂等——再跑一次没副作用。安装到本仓库 only，不动 global config。
set -euo pipefail
# readlink -f 解析软链：经 symlink 调用时 $0 是链路路径，dirname 会落错目录
cd "$(dirname "$(readlink -f "$0")")/.."

HOOKS_DIR="$(pwd)/.githooks"

# 目录缺失（partial checkout / 稀疏克隆）时拒绝配置，避免 hooksPath 指向空路径
if [ ! -d "$HOOKS_DIR" ]; then
    echo "✗ $HOOKS_DIR 不存在，拒绝安装" >&2
    exit 1
fi

# 确保 hook 和脚本可执行（clone 后权限可能丢）
# nullglob + 空守卫：目录缺失/为空（partial checkout）时 glob 不字面展开，chmod 不中止安装
shopt -s nullglob
_hooks=(.githooks/*)
_scripts=(scripts/*.sh)
if [ ${#_hooks[@]} -gt 0 ]; then chmod +x "${_hooks[@]}"; fi
if [ ${#_scripts[@]} -gt 0 ]; then chmod +x "${_scripts[@]}"; fi

# 本仓库 only：core.hooksPath 是 repo-local 配置，不会影响其它项目
# 先确认在 git 工作树内：否则裸 git config 会写进 ~/.gitconfig（global），违背「不动 global」承诺
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "✗ 不在 git 工作树内，拒绝写入配置" >&2
    exit 1
fi
git config --local core.hooksPath "$HOOKS_DIR"

echo "✓ git hooks installed"
echo "  core.hooksPath = $HOOKS_DIR"
echo "  pre-commit     → scripts/test-fast.sh   (~10-30s)"
echo "  pre-push       → scripts/test-all.sh    (~1-3 min)"
echo ""
echo "想临时跳过：git commit --no-verify / git push --no-verify"