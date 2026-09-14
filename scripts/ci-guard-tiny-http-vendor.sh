#!/usr/bin/env bash
# CI 守卫：确保 src-tauri/Cargo.toml [patch.crates-io] 的 tiny_http vendor 路径不被改坏。
#
# 检查项：
#   1. Cargo.toml [patch.crates-io] 段仍含 `tiny_http = { path = "vendor/tiny_http" }`
#   2. cargo metadata 报 tiny_http source 仍是 directory，路径正确
#   3. vendor 目录存在且 src/lib.rs + src/connection.rs 各含至少一处 [wmessage patch] 标记
#
# 失败：退出码 1，提示指向 src-tauri/vendor/tiny_http/PATCHES.md
# 成功：退出码 0

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

CARGO_TOML="src-tauri/Cargo.toml"
VENDOR_DIR="src-tauri/vendor/tiny_http"
PATCHES_MD="src-tauri/vendor/tiny_http/PATCHES.md"

fail() {
    echo "✗ ci-guard-tiny-http-vendor 失败：$1" >&2
    echo "  详见：$PATCHES_MD" >&2
    exit 1
}

# 1. Cargo.toml patch 段
if ! grep -q 'tiny_http = { path = "vendor/tiny_http" }' "$CARGO_TOML"; then
    fail "Cargo.toml 已无 [patch.crates-io] tiny_http = vendor 指向，请查 PATCHES.md §3 重新 vendor"
fi

# 2. cargo metadata 检查 source
#    jq 不一定在；先尝试 jq，否则用 python3 兜底
if ! command -v jq >/dev/null 2>&1; then
    fail "需要 jq 解析 cargo metadata，请安装 jq 后重跑"
fi

SOURCE=$(cd src-tauri && cargo metadata --format-version 1 --no-deps 2>/dev/null \
    | jq -r '.packages[] | select(.name == "tiny_http") | .source' | head -1)

if [ -z "$SOURCE" ] || [ "$SOURCE" = "null" ]; then
    fail "cargo metadata 未找到 tiny_http 包（patch 可能已误删）"
fi

# source 应形如 "directory+file:///.../src-tauri/vendor/tiny_http"
case "$SOURCE" in
    *vendor/tiny_http*) : ;;  # OK
    *) fail "tiny_http source 不是 vendor 目录：$SOURCE" ;;
esac

# 3. vendor 目录 + patch 标记
if [ ! -d "$VENDOR_DIR/src" ]; then
    fail "vendor 目录缺失：$VENDOR_DIR/src"
fi

MARKER_LIB=$(grep -c "wmessage patch" "$VENDOR_DIR/src/lib.rs" || true)
MARKER_CONN=$(grep -c "wmessage patch" "$VENDOR_DIR/src/connection.rs" || true)

if [ "$MARKER_LIB" -lt 1 ]; then
    fail "vendor/tiny_http/src/lib.rs 缺 [wmessage patch] 标记（patch 可能被回退）"
fi
if [ "$MARKER_CONN" -lt 1 ]; then
    fail "vendor/tiny_http/src/connection.rs 缺 [wmessage patch] 标记（patch 可能被回退）"
fi

echo "✓ ci-guard-tiny-http-vendor OK"
echo "  source:        $SOURCE"
echo "  patch markers: lib.rs=$MARKER_LIB, connection.rs=$MARKER_CONN"
exit 0