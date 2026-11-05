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
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

python3 - "$VER" "$NOTES" "$PUB_DATE" "$URL" "$SIG_BODY" "$OUT" <<'PY'
import json
import sys
from datetime import datetime, timezone

ver, notes, pub_date, url, sig, out = sys.argv[1:7]
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
echo "  发布：GitHub Releases v${TAG} 上传 ${ASSET}（由 ${SETUP} 改名）与 ${OUT}"
