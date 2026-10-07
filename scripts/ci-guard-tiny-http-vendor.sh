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

# 1. Cargo.toml patch 段（行首锚定：注释掉的 # tiny_http = ... 行首是 #，不匹配）
if ! grep -q '^[[:space:]]*tiny_http = { path = "vendor/tiny_http" }' "$CARGO_TOML"; then
    fail "Cargo.toml 已无 [patch.crates-io] tiny_http = vendor 指向，请查 PATCHES.md §3 重新 vendor"
fi

# 2. cargo metadata 检查 tiny_http 实际解析位置
if ! command -v jq >/dev/null 2>&1; then
    fail "需要 jq 解析 cargo metadata，请安装 jq 后重跑"
fi
if ! command -v cargo >/dev/null 2>&1; then
    fail "需要 cargo 运行 cargo metadata，请安装 Rust toolchain 后重跑"
fi

# manifest_path 判定（主流程与 --selftest 共用）：
#   通过 = tiny_http 的 manifest 恰好落在 src-tauri/vendor/tiny_http/（patch 真生效）
#   拒绝 = registry 来源 / 同名后缀目录（tiny_http_evil）/ 无关路径
check_manifest_path() {
    case "$1" in
        */src-tauri/vendor/tiny_http/Cargo.toml) return 0 ;;
        *) return 1 ;;
    esac
}

# 自测模式：正例必过、registry 反例与同名后缀反例必拒——守卫必须能正确失败、
# 也能正确通过（负路径真机验证方式见文件头「验证记录」）
if [ "${1:-}" = "--selftest" ]; then
    check_manifest_path "/repo/src-tauri/vendor/tiny_http/Cargo.toml" \
        || { echo "✗ selftest 正例失败（vendor 路径应通过）" >&2; exit 1; }
    if check_manifest_path "$HOME/.cargo/registry/src/index.crates.io/tiny_http-0.12.0/Cargo.toml"; then
        echo "✗ selftest 反例失败（registry 路径应拒绝）" >&2
        exit 1
    fi
    if check_manifest_path "/repo/src-tauri/vendor/tiny_http_evil/Cargo.toml"; then
        echo "✗ selftest 反例失败（同名后缀目录应拒绝）" >&2
        exit 1
    fi
    echo "✓ ci-guard-tiny-http-vendor selftest 通过"
    exit 0
fi

# 不吞 cargo 的 stderr：工具链/manifest 报错直接可见，避免被误读成「patch 已误删」。
# 注意不能加 --no-deps：tiny_http 是被 [patch.crates-io] 替换的依赖、不是 workspace
# member，--no-deps 的 packages 里根本没有它（实锤：守卫长期误报「patch 已误删」）
MANIFEST_PATH=$(cd src-tauri && cargo metadata --format-version 1 \
    | jq -r '.packages[] | select(.name == "tiny_http") | .manifest_path' | head -1)

if [ -z "$MANIFEST_PATH" ] || [ "$MANIFEST_PATH" = "null" ]; then
    fail "cargo metadata 未找到 tiny_http 包（patch 可能已误删）"
fi

if check_manifest_path "$MANIFEST_PATH"; then
    : # OK
else
    fail "tiny_http manifest 不在 vendor 目录：$MANIFEST_PATH"
fi

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
echo "  manifest:      $MANIFEST_PATH"
echo "  patch markers: lib.rs=$MARKER_LIB, connection.rs=$MARKER_CONN"
exit 0

# 验证记录（2026-10-07）：
#   正路径真机：./scripts/ci-guard-tiny-http-vendor.sh → exit 0（patch 生效，manifest 落 vendor）
#   负路径真机：临时注释 Cargo.toml 的 patch 行 → exit 1（第 1 步报「已无 patch 指向」）→ 已还原
#   判定函数负路径：--selftest（registry 路径 / tiny_http_evil 同名后缀均拒绝）
# 历史缺陷：旧实现用 --no-deps 查 .source，tiny_http 非 workspace member 根本
# 不出现在 packages 里 → 守卫恒失败（长期误报「patch 已误删」）