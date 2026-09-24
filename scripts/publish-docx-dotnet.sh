#!/usr/bin/env bash
# 绿色包 .NET 侧车打包：dotnet publish win-x64 self-contained（Release）+ 归位到便携包 dotnet/。
# 固化 2026-09-02 手工流程：dotnet/ = publish 产物整目录拷贝，仅剔除 wm-docx-revisions.pdb。
# 用法：scripts/publish-docx-dotnet.sh [输出根目录]（默认 wmessage-portable-<当天日期>，已 gitignore）
# 跨平台：macOS/Linux 直接跑（cross publish win-x64）；Windows 需 Git Bash / WSL。
set -euo pipefail
cd "$(dirname "$0")/.."

PROJECT="src-tauri/dotnet/WmDocxRevisions/WmDocxRevisions.csproj"
RID="win-x64"
PUBLISH_DIR="src-tauri/dotnet/WmDocxRevisions/bin/Release/net8.0/${RID}/publish"
OUT_ROOT="${1:-wmessage-portable-$(date +%F)}"
# OUT_ROOT 校验（fail-closed）：拒空 / 绝对路径 / 含 .. / 以 - 开头——rm -rf "$OUT_DIR" 不可被路径注入
case "$OUT_ROOT" in
  ""|/*|*..*|-*)
    echo "✗ OUT_ROOT 非法：'$OUT_ROOT'（须为相对路径，不含 ..，不以 - 开头）" >&2
    echo "  用法：scripts/publish-docx-dotnet.sh [输出根目录]" >&2
    exit 1
    ;;
esac
OUT_DIR="${OUT_ROOT}/dotnet"

command -v dotnet >/dev/null 2>&1 || { echo "✗ 未找到 dotnet SDK，请先安装 https://dot.net"; exit 1; }

echo "[1/3] dotnet publish -c Release -r ${RID} --self-contained"
dotnet publish "$PROJECT" -c Release -r "$RID" --self-contained

echo "[2/3] 归位 publish 产物 → ${OUT_DIR}/（剔除 .pdb，幂等重建）"
rm -rf -- "$OUT_DIR"
mkdir -p -- "$OUT_DIR"
# 手工包核对结论：dotnet/ 与 publish 目录文件清单一致，唯一差别是不带 wm-docx-revisions.pdb
find "$PUBLISH_DIR" -maxdepth 1 -type f ! -name '*.pdb' -exec cp {} "$OUT_DIR/" \;

echo "[3/3] 校验产物完整性"
COUNT=$(find "$OUT_DIR" -maxdepth 1 -type f | wc -l | tr -d ' ')
for f in wm-docx-revisions.exe coreclr.dll hostfxr.dll; do
  [[ -f "$OUT_DIR/$f" ]] || { echo "✗ 缺少关键文件: $f"; exit 1; }
done

echo ""
echo "✓ .NET 侧车就绪: ${OUT_DIR}/（${COUNT} 个文件）"
echo "  下一步（手工）：把 wmessage.exe / WebView2Loader.dll / MicrosoftEdgeWebview2Setup.exe / README.txt 放进 ${OUT_ROOT}/ 即完整绿色包"
