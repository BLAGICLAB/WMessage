#!/usr/bin/env bash
# 安装版打包前置：铺装 bundle-staging（tauri.windows.conf.json 的 resources 源）。
# 1) .NET 侧车：复用 publish-docx-dotnet.sh
# 2) ONNX Runtime dll：Windows 目标 ort 为 load-dynamic，从 NuGet 拉
#    microsoft.ml.onnxruntime（ORT_VER 与 Cargo.toml 的 ort 配对，ort 升级须同步改此处）
# 3) Windows 目标类型检查（cfg(windows) 代码在 mac 上永不编译，出包前先拦一道）
# 缺件即退出非 0（fail-closed）。用法：bash scripts/stage-windows-bundle.sh
set -euo pipefail
cd "$(dirname "$0")/.."

STAGE_DIR="src-tauri/bundle-staging"
ORT_VER="1.28.0"

echo "[1/4] .NET 侧车 publish → ${STAGE_DIR}/dotnet/"
bash scripts/publish-docx-dotnet.sh "$STAGE_DIR"

echo "[2/4] ONNX Runtime dll（NuGet microsoft.ml.onnxruntime ${ORT_VER}）"
mkdir -p "$STAGE_DIR"
if [[ ! -f "$STAGE_DIR/onnxruntime.dll" || ! -f "$STAGE_DIR/onnxruntime_providers_shared.dll" ]]; then
  NUPKG="$(mktemp -d)/ort.nupkg"
  curl -sSL --fail -o "$NUPKG" \
    "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/${ORT_VER}/microsoft.ml.onnxruntime.${ORT_VER}.nupkg" \
    || { echo "✗ ONNX Runtime ${ORT_VER} 下载失败（NuGet）" >&2; exit 1; }
  unzip -o -j "$NUPKG" \
    "runtimes/win-x64/native/onnxruntime.dll" \
    "runtimes/win-x64/native/onnxruntime_providers_shared.dll" \
    -d "$STAGE_DIR/"
fi

echo "[3/4] 校验 staging 完整性（fail-closed）"
for f in dotnet/wm-docx-revisions.exe dotnet/coreclr.dll dotnet/hostfxr.dll \
         onnxruntime.dll onnxruntime_providers_shared.dll; do
  [[ -f "$STAGE_DIR/$f" ]] || { echo "✗ staging 缺件: $f" >&2; exit 1; }
done

COUNT=$(find "$STAGE_DIR" -type f | wc -l | tr -d ' ')
echo "[4/4] Windows 目标类型检查（cargo check --bins --target x86_64-pc-windows-gnu）"
# cfg(windows) 的代码在 mac 上永不编译——门禁编的是宿主目标，这类编译期错只有
# 交叉构建或 Windows 实机看得见。把这一关前置到出包流程，省掉整轮构建白跑。
cargo check --manifest-path src-tauri/Cargo.toml --bins --target x86_64-pc-windows-gnu \
  || { echo "✗ Windows 目标编译不过——先修 Windows 专属代码再出包" >&2; exit 1; }

echo ""
echo "✓ bundle-staging 就绪：${COUNT} 个文件（dotnet 侧车 + 2 个 onnx dll）；Windows 目标类型检查通过"
echo "  下一步：npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis"
