#!/usr/bin/env bash
# 生成更新清单 latest.json（GitHub Releases 发布用）。
# 前置：带签名密钥环境变量的 Windows 出包已产出安装器与 .sig。
# 用法：scripts/make-update-manifest.sh <版本> [更新说明]  产物写仓库根 latest.json
set -euo pipefail
cd "$(dirname "$0")/.."

VER="${1:?用法: make-update-manifest.sh <版本> [更新说明]}"
NOTES="${2:-}"

NSIS_DIR="src-tauri/target/x86_64-pc-windows-gnu/release/bundle/nsis"
SETUP="$NSIS_DIR/wmessage_${VER}_x64-setup.exe"
SIG="$SETUP.sig"
OUT="latest.json"
TAG="v${VER}"
ASSET="wmessage_x64-setup.exe"
URL="https://github.com/BLAGICLAB/WMessage/releases/download/${TAG}/${ASSET}"

[[ -f "$SETUP" ]] || { echo "✗ 缺安装器 ${SETUP}，先出包" >&2; exit 1; }
[[ -f "$SIG" ]] || { echo "✗ 缺签名 ${SIG}，构建时须设 TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)" >&2; exit 1; }

SIG_BODY="$(cat "$SIG")"

# 版本对拍：latest.json 的 version 必须 > 当前安装版（Cargo.toml）才会被判「有更新」；
# 忘 bump 就发 = 用户点检查恒「已是最新」。相等只警告不阻断（可能故意重发）。
CARGO_VER="$(grep -m1 '^version' src-tauri/Cargo.toml | sed 's/version = "\(.*\)"/\1/')"
if [[ "$VER" == "$CARGO_VER" ]]; then
  echo "⚠ 版本 ${VER} 与 Cargo.toml 当前版本相同——检查更新将判「已是最新」，确认是否忘了 bump" >&2
fi

python3 - "$VER" "$NOTES" "$URL" "$SIG_BODY" "$OUT" <<'PY'
import json
import sys
from datetime import datetime, timezone

ver, notes, url, sig, out = sys.argv[1:6]
data = {
    "version": ver,
    "notes": notes,
    "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": {
        "windows-x86_64": {
            "signature": sig,
            "url": url,
        }
    },
}
with open(out, "w", encoding="utf-8") as f:
    f.write(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
PY

echo "✓ 已生成 ${OUT}（v${VER} → ${URL}）"
echo "  发布：GitHub Releases ${TAG} 上传 ${ASSET}（由 ${SETUP} 改名）与 ${OUT}"
