# WMessage Windows 绿色版打包手册（供 openclaw / 打包代理执行）

> 目标产物：`wmessage-portable-<日期>.zip`，解压即用、数据随目录走的 Windows x64 便携包。
> 全程在 macOS 构建机上完成（mingw 交叉编译），不需要 Windows 机器参与构建。
> 本文是**执行手册**：按顺序照做即可。历史决策见 DEVLOG.md 2026-09-10 条目。

## 0. 前置条件（构建机一次性准备）

| 依赖 | 安装 | 验证 |
|---|---|---|
| mingw-w64 | `brew install mingw-w64` | `which x86_64-w64-mingw32-gcc` |
| Rust 目标 | `rustup target add x86_64-pc-windows-gnu` | `rustup target list --installed \| grep windows-gnu` |
| .NET SDK 8+ | https://dot.net | `dotnet --version` |
| Node 依赖 | 仓库根 `npm install`（如已装跳过） | `ls node_modules/.bin/tauri` |

以下命令默认在**仓库根目录**（`wmessage/`，含 `package.json` 的那层）执行。

## 1. 编译 wmessage.exe

```bash
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
npx tauri build --target x86_64-pc-windows-gnu --no-bundle
```

- 产物：`src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe`（约 54 MB）
- **必须走完整 `tauri build`**（它会先跑 `npm run build` 构建前端并嵌入资源）。
  直接 `cargo build` 出的 exe 缺内置页面资源，运行报「无法访问此页面」。
- ort（记忆 v2 语义引擎）在 Windows 目标是 `load-dynamic` 模式（见 `src-tauri/Cargo.toml`
  尾部 `[target.'cfg(target_os = "windows")'.dependencies]`），构建期不需要 onnxruntime 文件；
  但 **onnxruntime.dll 必须随包分发**（第 3 步），缺失时运行会 panic 而不是降级。

## 2. 构建 .NET 侧车（Word 修订工具）

```bash
bash scripts/publish-docx-dotnet.sh wmessage-portable-$(date +%F)
```

- 产出 `wmessage-portable-<日期>/dotnet/`（self-contained .NET 8，用户机免装运行时）。
- 若 `src-tauri/dotnet/` 自上次出包以来无改动，可直接复用上一版绿色包的 `dotnet/` 目录。

## 3. 获取 onnxruntime.dll（记忆 v2 语义引擎）

版本与 `src-tauri/Cargo.toml` 的 ort 匹配（ort 2.0.0-rc.13 ↔ ONNX Runtime **1.28.0**；
ort 升级时需同步，核对 `ort-sys-*/build/download/dist.tsv` 里的 `ms@<版本>`）。

```bash
cd /tmp
curl -sL -o ort.nupkg "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.28.0/microsoft.ml.onnxruntime.1.28.0.nupkg"
unzip -o -j ort.nupkg \
  "runtimes/win-x64/native/onnxruntime.dll" \
  "runtimes/win-x64/native/onnxruntime_providers_shared.dll" \
  -d ort-dll
cd - # 回仓库根
```

GitHub 直连不通时用 NuGet 源（api.nuget.org 可用）；pyke CDN 的 tar.lzma2 是 341MB
静态库不是 DLL，不要用。

## 4. 组装绿色包目录

```bash
OUT=wmessage-portable-$(date +%F)   # 第 2 步已创建（内含 dotnet/）
PREV=wmessage-portable-2026-09-05   # 上一版绿色包，作为固定文件来源

cp src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe "$OUT/"
cp /tmp/ort-dll/onnxruntime.dll /tmp/ort-dll/onnxruntime_providers_shared.dll "$OUT/"
cp -R bge-small-zh-v1.5 "$OUT/"          # 仓库根的模型目录
cp -R pp-ocr-v6 "$OUT/"                  # OCR 模型（ocr_image 工具，PP-OCRv6 small，仓库根随仓库提交）
cp "$PREV"/{WebView2Loader.dll,MicrosoftEdgeWebview2Setup.exe,README.txt} "$OUT/"
```

最终结构（zip 根目录即此内容，**不要再包一层文件夹**…… zip 内以这些条目为顶层）：

```
wmessage.exe                        主程序
WebView2Loader.dll                  必需（缺它报「找不到 webview2loader.dll」）
onnxruntime.dll                     语义引擎，必需随包
onnxruntime_providers_shared.dll    ONNX Runtime 配套
bge-small-zh-v1.5/                  语义模型（含 tokenizer.json、onnx/model_quantized.onnx）
pp-ocr-v6/                          OCR 模型（det.onnx / rec.onnx / keys.txt / cls.onnx，PP-OCRv6 small）
dotnet/                             Word 修订工具（self-contained）
MicrosoftEdgeWebview2Setup.exe      Win10 首次备用（Win11 自带 WebView2）
README.txt                          使用说明（更新「文件说明」和「更新记录」两节）
```

固定文件来源说明：
- `MicrosoftEdgeWebview2Setup.exe`：仓库根也有一份；或微软官方 Evergreen 引导安装器。
- `WebView2Loader.dll`：从上一版绿色包复制；拿不到就从 NuGet 包
  `Microsoft.Web.WebView2` 的 `build/native/x64/WebView2Loader.dll` 提取。

## 5. 更新 README.txt

改两处：
1. 「文件说明」按上表对齐（含 exe 实际大小）。
2. 「更新记录」顶部加一条：`基于 main @ <git rev-parse --short HEAD> 出包` +
   自上一版以来的 `git log --oneline` 要点；若 exe 含未提交工作区改动需注明。

## 6. 打 zip（必须用 Python zipfile）

macOS 自带 `zip` 的 Unix 扩展字段会让 Windows 资源管理器解压报「位置不可用」，**禁用**。

```bash
python3 - <<'EOF'
import os, zipfile
src = f"wmessage-portable-{__import__('datetime').date.today()}"
out = src + ".zip"
if os.path.exists(out): os.remove(out)
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
    for root, dirs, files in os.walk(src):
        dirs.sort()
        for f in sorted(files):
            if f == ".DS_Store": continue
            p = os.path.join(root, f)
            z.write(p, os.path.relpath(p, src))
print("done", out)
EOF
```

预期 zip 约 100 MB（2026-09-10 基准 76 MB + pp-ocr-v6 约 24 MB 压缩后）。

## 7. 校验清单（全部通过才算完成）

```bash
python3 - <<'EOF'
import zipfile
z = zipfile.ZipFile(f"wmessage-portable-{__import__('datetime').date.today()}.zip")
assert z.testzip() is None, "zip 损坏"
names = z.namelist(); top = {n.split("/")[0] for n in names}
need = {"wmessage.exe","WebView2Loader.dll","onnxruntime.dll",
        "onnxruntime_providers_shared.dll","MicrosoftEdgeWebview2Setup.exe",
        "README.txt","dotnet","bge-small-zh-v1.5","pp-ocr-v6"}
assert need <= top, need - top
assert "dotnet/wm-docx-revisions.exe" in names
assert "bge-small-zh-v1.5/onnx/model_quantized.onnx" in names
assert "bge-small-zh-v1.5/tokenizer.json" in names
for f in ("det.onnx","rec.onnx","keys.txt","cls.onnx"):
    assert f"pp-ocr-v6/{f}" in names, f"pp-ocr-v6/{f} 缺失"
print("ZIP_OK", len(names), "files")
EOF
```

- [ ] exe 无 onnxruntime 静态导入（抽查）：
  `x86_64-w64-mingw32-objdump -p .../wmessage.exe | grep -i onnxruntime` 应无输出
- [ ] macOS 侧回归：`cd src-tauri && cargo check` 通过（Cargo.toml 有 target 特异配置时必跑）
- [ ] DEVLOG.md 记一条出包记录
- [ ] Windows 实机验收（人工）：解压双击 wmessage.exe；重点验证机器人记忆语义检索
  （模型/引擎异常会自动降级关键词模式，不报错但功能缩水，需肉眼确认），并用一张带文字的
  本地图片让机器人跑 `ocr_image`（缺 pp-ocr-v6/ 时工具会报「请运行 scripts/fetch_ocr_models.sh…」）

## 坑位速查

| 坑 | 应对 |
|---|---|
| `ort-sys: no prebuilt binaries available for target x86_64-pc-windows-gnu` | 确认 Cargo.toml Windows 目标已加 `load-dynamic`（2026-09-10 起已在仓库） |
| 直出 exe 报「无法访问此页面」 | 用了 `cargo build` 而非完整 `tauri build`，重跑第 1 步 |
| Windows 解压报「位置不可用」 | zip 是 macOS `zip` 打的，改用第 6 步 Python zipfile |
| 用户机报「找不到 webview2loader.dll」 | 漏拷 WebView2Loader.dll，与 WebView2 Runtime 无关 |
| Win10 白屏/起不来 | 让用户先跑包内 MicrosoftEdgeWebview2Setup.exe 装 WebView2 |
| dotnet/ 失效静默走 Python | dotnet/ 必须整目录随包；缺失时 Word 修订回退 Python（需用户机有 Python 3.10+） |
