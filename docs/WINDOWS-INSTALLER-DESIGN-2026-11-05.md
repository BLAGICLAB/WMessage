# Windows 安装版落地方案（2026-11-05）

> **状态**：代码侧已落地（2026-11-05，B1–B4 + B6），B5 实机验证待做。本文给「配置改法 + 打包流程 + 校验清单」三件，
> 落地按 §七 的六批切分推进；每批一个逻辑变更一个 commit。
>
> **一句话**：Windows 分发形态从「绿色 zip（解压即用）」改成「NSIS 安装版
> （装到 `%LOCALAPPDATA%\wmessage`）」，三类随包资源（OCR 模型 / 向量模型 /
> .NET 侧车）全部进安装包，数据目录统一到 `app_data`——便携模式随之退役。

## 一、拍板登记

| 编号 | 决策 | 出处 | 直接后果 |
|---|---|---|---|
| D-1 | .NET 侧车继续 **self-contained**（用户机免装 .NET 8 运行时） | 2026-11-05 用户拍板 | 安装包 +79MB；Word 修订能力零依赖，不回退 Python 兜底 |
| D-2 | OCR 模型 + 向量模型 **随安装包一起装**（不做按需下载） | 同上 | 安装包未压缩 ~190MB；语义记忆 / OCR 装完即用，离线可用 |
| D-3 | 数据目录 **统一到 app_data**，便携模式（数据随 exe 走）退役 | 同上 | SPEC 6-108 改写；验收卡 ACC-P-03 / ACC-W-01 改写 |
| D-4 | **不做老数据迁移**（现存数据均为测试数据，空数据启动） | 2026-11-05 用户拍板 | §4.2 只剩便携分支退役；B2 无迁移逻辑；SPEC/验收卡不出现迁移口径 |
| D-5 | playwright-mcp **维持手动**（不随安装包分发） | 2026-11-05 用户拍板 | 包仍由用户拷到安装目录 `%LOCALAPPDATA%\wmessage\playwright-mcp\`；缺失时内置卡片显示「连不上」引导；升级不在卸载清单、不丢 |

**待拍板**（本文给默认值，不阻塞落地；真要改只影响 §七 的某一批）：

| 项 | 默认 | 说明 |
|---|---|---|
| 绿色包是否停发 | **停发**，安装版取代 | 便携模式退役后绿色包只剩「解压即用」一个卖点，且要维护两套打包流程；停发后 `docs/PACKAGING-WINDOWS-PORTABLE.md` 归档 |
| 是否顺带做自动更新 | **不做**，另立方案 | 自动更新要 CI + updater 密钥 + 发布源，与本方案正交；本方案落地后自动更新的最大卡点（安装布局）已消失 |
| macOS Developer ID 签名 | **暂不做** | 只影响「别人机器上装 mac 版」是否要过 Gatekeeper，与 Windows 安装版无关 |

## 二、事实基线

每条都有出处（版本：tauri 2.12.3 / tauri-utils 2.10.1 / tauri-bundler 2.10.4）。

| 事实 | 出处 |
|---|---|
| NSIS `currentUser` → `$INSTDIR = $LOCALAPPDATA\${PRODUCTNAME}` = `%LOCALAPPDATA%\wmessage`；`perMachine` → `$PROGRAMFILES64\wmessage` | tauri-bundler `src/bundle/windows/nsis/installer.nsi:546-547` |
| 资源装在 `$INSTDIR` **根**（= exe 同目录），落点 = `resource.target()` | `installer.nsi:684`（`SetOutPath $INSTDIR`）、`:696-700`（`File /a "/oname=..."`）、`nsis/mod.rs:858-868` |
| 卸载只删「自己装过的文件」，最后 `RMDir "$INSTDIR"`（非空即保留） | `installer.nsi:831-865` |
| 卸载时勾选「删除应用数据」→ `RmDir /r "$APPDATA\${BUNDLEID}"` + `$LOCALAPPDATA\${BUNDLEID}` | `installer.nsi:914-929` |
| 升级（update mode）先跑旧卸载器再装新版 → 资源被整体替换；用户自放文件不在删除清单里 → 保留 | `installer.nsi:824-864` + `:868`（`$UpdateMode <> 1` 分支） |
| `app_data_dir()` = `dirs::data_dir()` + identifier → Windows 上 = `%APPDATA%\com.renshi.wmessage` | tauri 2.12.3 `src/path/desktop.rs:251-256` |
| `resource_relpath()` 把路径里的 `..` 映射成 **`_up_`** | tauri-utils 2.10.1 `src/resources.rs:17-29` |
| map 形式（`{源: 目标}`）的目标 = `目标/源内相对路径`（目录会展开、单文件直接落名） | 同上 `:205-251` |
| macOS bundler 用**同一套** `resource.target()` 落资源 | tauri-bundler `src/bundle/macos/app.rs:97` → `settings.rs:1223-1230` |
| macOS 上交叉构建 NSIS 可行，需系统装 `makensis` | tauri 官方文档「Build Windows apps on Linux and macOS」；`nsis/mod.rs:222-243`（macOS 分支 `which::which("makensis")`） |
| 本机已装 makensis | `/opt/homebrew/bin/makensis`（`which makensis` 命中） |
| `WebView2Loader.dll` 由 bundler 自动收进包（gnu target） | `nsis/mod.rs:808-822` |
| Windows 目标 ort 是 **load-dynamic**：构建期不需要 onnxruntime 文件，运行期缺 dll 会 panic 不降级 | `src-tauri/Cargo.toml` 尾部 `[target.'cfg(target_os = "windows")'.dependencies]` |
| onnx dll 来源 = NuGet `microsoft.ml.onnxruntime`（1.28.0 ↔ ort 2.0.0-rc.13 配对；取 nupkg 内 `runtimes/win-x64/native/` 两个 dll） | 绿色包手册 §3 同款口径（`docs/PACKAGING-WINDOWS-PORTABLE.md`） |

### 三个关键发现

**发现 1（要修的真 bug）：当前 `bundle.resources` 的 `..` 会变成 `_up_` 目录。**
现配置是 list 形式 `["../bge-small-zh-v1.5"]`，目标路径 = `resource_relpath("../bge-small-zh-v1.5/…")`
= `_up_/bge-small-zh-v1.5/…`。而 macOS bundler 用同一套 `resource.target()`（`macos/app.rs:97`），
于是打包产物的模型落在 `Contents/Resources/_up_/bge-small-zh-v1.5/…`；
`memory/embed.rs:40-61` 的解析口径是「env → exe 同目录 → 编译期仓库根」，
**打包版里三个位置全不命中** → 语义记忆在打包版里静默降级为关键词模式
（只有构建机自己因为编译期绝对路径兜住）。map 形式修掉的是 **Windows** 侧：
资源落 `$INSTDIR` 根 = exe 同目录，解析直接命中。macOS 侧资源位置虽变对
（`Resources/bge-small-zh-v1.5`），解析仍找 exe 同目录（`Contents/MacOS`），
`.app` 打包版依旧不命中——mac 打包版降级是现状，不在本方案修掉范围（§九 登记）。

**发现 2：资源落点天然对齐运行时口径 → 三处「exe 同目录」解析零改动。**
`embed.rs`（bge）、`ocr.rs`（pp-ocr-v6）、`py/env.rs`（dotnet 侧车）找的都是
「exe 同目录 / 子目录」，而 NSIS 把资源装在 `$INSTDIR` 根（= exe 同目录）→
只要配置写对目标路径，**这三处代码一行不用改**。

**发现 3：卸载的「删除应用数据」正好命中我们的数据目录。**
Tauri's uninstaller 勾选删除应用数据时清 `$APPDATA\${BUNDLEID}`，
`BUNDLEID` = `com.renshi.wmessage` = 我们的 identifier = `app_data_dir()` 的末段 →
勾选即彻底清干净，不勾选即保留。**这正是 D-3 想要的卸载语义**，不用自己写卸载逻辑。

## 三、目标安装布局

```
%LOCALAPPDATA%\wmessage\            ← $INSTDIR（currentUser，免管理员）
├── wmessage.exe                    主程序
├── uninstall.exe                   卸载器
├── WebView2Loader.dll              bundler 自动收（gnu target）
├── onnxruntime.dll                 ONNX Runtime（语义引擎 + OCR 共用）
├── onnxruntime_providers_shared.dll
├── bge-small-zh-v1.5\              ← 向量模型（23MB）
│   ├── onnx\model_quantized.onnx
│   └── tokenizer.json
├── pp-ocr-v6\                      ← OCR 模型（31MB）
│   ├── det.onnx  rec.onnx  cls.onnx  keys.txt
└── dotnet\                         ← .NET 8 self-contained 侧车（79MB）
    ├── wm-docx-revisions.exe
    ├── coreclr.dll  hostfxr.dll  …

%APPDATA%\com.renshi.wmessage\       ← 数据目录（D-3 后唯一落点）
├── wmessage.db  bot-config.json  bot.log
├── AI_Gen_Files\  runtime\
```

WebView2 运行时**不随包**：`webviewInstallMode` 用 `embedBootstrapper`
（把 ~1.8MB 的引导程序打进安装器，运行时缺失再联网拉）——与今天绿色包
随包放 `MicrosoftEdgeWebview2Setup.exe` 的行为一致。

## 四、配置改法

### 4.1 `src-tauri/tauri.conf.json`

```jsonc
{
  "bundle": {
    "active": true,
    "targets": ["app", "dmg", "nsis"],       // 加 nsis（macOS 侧 app/dmg 不动）

    // list 形式 → map 形式：把 `..` 造成的 `_up_` 变成显式目标目录（发现 1）
    "resources": {
      "../bge-small-zh-v1.5": "bge-small-zh-v1.5",
      "../pp-ocr-v6": "pp-ocr-v6",
      "bundle-staging/dotnet": "dotnet",
      "bundle-staging/onnxruntime.dll": "onnxruntime.dll",
      "bundle-staging/onnxruntime_providers_shared.dll": "onnxruntime_providers_shared.dll"
    },

    "windows": {
      "webviewInstallMode": { "type": "embedBootstrapper" },   // 原 downloadBootstrapper
      "nsis": {
        "installMode": "currentUser",                          // 保持不变
        "languages": ["SimpChinese", "English"],               // 默认只有 English
        "displayLanguageSelector": true
      }
    }
    // macOS 段、publisher、icon 等不动
  }
}
```

要点：

- **源路径相对 `src-tauri/` 解析**（Tauri CLI 构建时把 cwd 设为 tauri 目录；首轮构建以
  日志里解析出的实际路径为准）；
  `bundle-staging/` 是新加的**暂存目录**（`src-tauri/bundle-staging/`，gitignore），
  由打包脚本在构建前铺好——**内容缺失必须构建失败**，不许静默少装一项。
- map 形式的目标是**扁平**目录名（`dotnet` / `pp-ocr-v6` / `bge-small-zh-v1.5`），
  装出来正好是 §三 的布局，命中三处「exe 同目录」解析。
- `bundle-staging/onnxruntime*.dll` 用「单文件 → 同名落点」的映射，
  装出来在 `$INSTDIR` 根（与绿色包一致）。
- `bundle.windows.bundleVcRuntime` **不用改**：默认已经是 `false`
  （tauri-utils 2.10.1 `config.rs:1112`），MinGW 产物不会多带 MSVC 运行库 DLL，
  与今天绿色包口径一致——写这行属于多余配置。
- **签名**：仍不签名（与绿色包同）。`certificateThumbprint` / `timestampUrl` 留空；
  后果是 SmartScreen 提示，绿色包同样如此。

### 4.2 `src-tauri/src/paths.rs`（便携模式退役，无数据迁移）

**改动 1：`probe_dir` 去掉便携分支。**

现在三条分支（`paths.rs:91-118`）：① cargo target 隔离（测试/开发，保留）
② exe 目录可写 → 便携 ③ 退 app_data。D-3 之后 ② 整条删掉：

```rust
// 删掉整段便携判定：
//   if dir.join("wmessage.db").exists() || dir.join("AI_Gen_Files").exists() { return dir }
//   if !is_under_system_temp(dir) { 写探针 … return dir }
// 结果：数据目录恒为 app_data_dir()（cargo target 的测试隔离分支不动）
```

连带：`is_macos_app_bundle_dir` / `is_under_system_temp` 两个判定在
`probe_dir` 里不再有调用点 → **随本次改动删除**（不留死代码）；
`is_cargo_target_dir` 保留（测试隔离仍需要）。

**改动 2：无迁移逻辑（D-4）。**

现存数据均为测试数据，绿色包停发后直接从空数据启动，不做任何老数据
迁移/导入；数据目录恒为 `app_data_dir()`，`open_db` 前没有别的步骤。

### 4.3 打包脚本（暂存目录铺装）

新增 `scripts/stage-windows-bundle.sh`（与 `scripts/publish-docx-dotnet.sh` 同风格）：

1. `.NET`：`bash scripts/publish-docx-dotnet.sh` 出 self-contained publish 产物 →
   归位到 `src-tauri/bundle-staging/dotnet/`（**扁平**，剔除 `.pdb`，与现脚本口径一致）。
2. `ONNX Runtime`：从 NuGet 拉 `microsoft.ml.onnxruntime`（ORT_VER=1.28.0，与 ort
   2.0.0-rc.13 配对），解出 `runtimes/win-x64/native/` 两个 dll → 归位到
   `src-tauri/bundle-staging/`。Windows 目标 ort 是 load-dynamic，构建期不需要 dll；
   `tauri-build` 在编译期就校验资源存在性——staging 缺件时交叉构建直接失败（fail-closed）。
3. **缺一即 fail-closed**：任一项缺失 → 退出码非 0 + 明确报错（不许产出缺功能的包）。

### 4.4 不改的地方（明确划界）

- `embed.rs` / `ocr.rs` / `py/env.rs` 的解析口径**一行不改**（发现 2）。
- `py/env.rs` 的 dotnet 侧车找的是 `exe 同目录/dotnet/wm-docx-revisions.exe`
  → 安装后落在 `$INSTDIR\dotnet\` ✓ 命中。
- keyring 条目（`wmessage.bot.v1`）、技能物化副本、DB schema 迁移逻辑都不动。
- playwright-mcp 不进安装包（D-5）：解析口径本就是 exe 同目录（`bot/mcp/builtin.rs`
  pick_package_dir），安装后放 `$INSTDIR\playwright-mcp\` 即命中；随包分发被否
  （Chromium 双内核 +300MB，仅少数人用）。

## 五、打包流程（macOS 交叉构建 → Windows 安装包）

```bash
# 0 前置（一次性）
brew install nsis                                  # 本机已有 makensis ✓
# 1 暂存（模型与 dotnet 在仓库里，onnx dll 从 NuGet 拉）
bash scripts/stage-windows-bundle.sh
# 2 构建（前端 + Rust + 打包）
npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis
# 3 产物
src-tauri/target/x86_64-pc-windows-gnu/release/bundle/nsis/wmessage_<版本>_x64-setup.exe
```

发布更新（设置页「检查更新」用，`docs/UPDATE-CHECK-DESIGN-2026-11-05.md`）：

```bash
# 4 带 Signing 密钥出包（签名由 createUpdaterArtifacts 触发，私钥在 ~/.tauri/，不进仓）
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/wmessage-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
# （重新跑 §五 1-2 步的暂存与构建，产物旁多出 .sig）
# 5 生成清单 + 发布
bash scripts/make-update-manifest.sh <版本> "<更新说明>"
# 6 GitHub Releases 建 v<版本>：上传 wmessage_x64-setup.exe（setup.exe 改名，固定资产名）
#   与 latest.json 两个文件；检查更新端点 = releases/latest/download/latest.json
```

流程约束：

- **必须走完整 `tauri build`**（它先跑 `npm run build` 把前端嵌进 exe）；
  直接 `cargo build` 出的 exe 缺内置页面资源，运行报「无法访问此页面」。
- 构建前工作区须是干净 commit（出包记账沿用现有规矩：记录 `git rev-parse --short HEAD`）。
- **不主动出包**：按 AGENTS.md 的约定，改完先在 macOS 现有环境过一遍，
  出包只在明确要求时做；出包后连带 SHA256 与路径两处记账
  （SHA256 记在 DEVLOG 的出包记录里）。
- onnx dll 由暂存脚本从 NuGet 下载（需联网）；首次交叉构建各依赖全量编译，耗时较长。

## 六、校验清单

### A. 本地（macOS）可查

- [ ] `npm run tauri build -- --target x86_64-pc-windows-gnu --bundles nsis` 成功，产出 `bundle/nsis/*.exe`
- [ ] 体积核对：`ls -lh` 安装器应 ~100–120MB（含 79MB dotnet + 31MB OCR + 23MB bge + 54MB exe 的压缩结果）
- [ ] 资源落位核对（不装也能查）：`strings` 安装器应能查到 `pp-ocr-v6/det.onnx`、
      `bge-small-zh-v1.5/onnx/model_quantized.onnx`、`dotnet/wm-docx-revisions.exe`
      三个**目标路径**；**查不到 `_up_` 字样**（那是发现 1 的复发信号）
- [ ] `src-tauri/bundle-staging/` 三个来源齐全（dotnet 侧车 / 两个 onnx dll）

### B. 实机（Windows）安装后

- [ ] 安装到 `%LOCALAPPDATA%\wmessage`，目录结构与 §三 一致（逐项 `Test-Path`）
- [ ] **exe 旁无 `_up_` 目录**（发现 1 的实机确认）
- [ ] 运行时三能力自检：
      - 语义记忆：新增一条记忆 → 检索能命中（`bge` 加载成功，不报「模型目录缺失」）
      - OCR：截图或图片 → 能出字（`pp-ocr-v6` 加载成功）
      - Word 修订：改一份 docx → 走 .NET 引擎（`wm-docx-revisions.exe` 可执行）
- [ ] 数据目录断言：`%APPDATA%\com.renshi.wmessage\wmessage.db` 存在，
      **exe 目录旁没有** `wmessage.db` / `AI_Gen_Files`（D-3 生效）
- [ ] 设置页「打开产物文件夹」指向 `%APPDATA%\com.renshi.wmessage\AI_Gen_Files`
      （后端回传路径，前端不拼）
- [ ] 卸载 → 目录清空、卸载器消失；**勾选「删除应用数据」** → `%APPDATA%\com.renshi.wmessage` 消失；
      **不勾选** → 数据仍在

### C. 升级语义

- [ ] 装旧版 → 在 exe 目录放一个自造文件 `my-note.txt` → 装新版
      → `my-note.txt` **仍在**（不在卸载清单里）
- [ ] 升级后模型/侧车是**新版**（对比 `det.onnx` 的 SHA256 与旧包）
- [ ] 升级后数据仍在 `%APPDATA%\com.renshi.wmessage`（不在安装目录里）
- [ ] 安装器检测到程序在运行会提示关闭（`installer.nsi:690` 的 `CheckIfAppIsRunning`）

### D. 门禁与回归

- [ ] `bash scripts/test-fast.sh` 绿（改 `paths.rs` 与配置后必跑）
- [ ] `bash scripts/test-all.sh` 绿
- [ ] `python3 -m pytest tests-audit/audit_file_size.py -q` 绿（paths.rs 改动不越 1500 行）
- [ ] `python3 -m pytest tests-audit/audit_acceptance_matrix.py -q` 绿（SPEC ↔ 矩阵 ↔ 卡三方对齐）
- [ ] `cargo check --release --manifest-path src-tauri/Cargo.toml` 通过
      （**必须带 `--release`**：debug profile 下 `debug_assertions` 恒真，release 才暴露的问题）

## 七、切分批次

| 批 | 内容 | 验收 | 风险 |
|---|---|---|---|
| B1 | `tauri.conf.json` 的 resources 改 map 形式（含 staging 三个来源） | 门禁 A 的 `strings` 断言（Windows 侧 `_up_` 修复）+ macOS `.app` 里模型落到 `Resources/bge-small-zh-v1.5` 且仍能起 | 低；mac 打包版解析不命中是已知现状（§九），不在 B1 范围 |
| B2 | `paths.rs`：便携分支退役 + 单测（无迁移逻辑，D-4） | `cargo test --lib` 绿；现有便携分支用例随分支删除，改写为「恒 app_data / cargo target 隔离」口径 | **中**：动的是所有平台的数据目录判定 |
| B3 | `scripts/stage-windows-bundle.sh` | 脚本跑通、缺件 fail-closed | 低 |
| B4 | NSIS 段配置（languages / embedBootstrapper / bundleVcRuntime / targets） | 门禁 A 全绿 | 低 |
| B5 | Windows 实机验证（§六 B/C） | 实机记录回填 | 中：需 Windows 环境 |
| B6 | 文档同步（见 §八） | `audit_acceptance_matrix.py` 绿 | 低 |

B2 与 B1 有依赖顺序（先 B1 定资源布局，再 B2 定数据目录，两者互不阻塞但要分开提交）。
**B5 需要一台 Windows 机器**；B1–B4、B6 全部在 macOS 完成。

## 八、文档同步清单（B6 必做）

| 文档 | 动作 |
|---|---|
| `SPEC.md:175` | 「便携模式：数据库随 exe 走（…）；Windows 绿色 zip + macOS dmg」→ 改写为「数据目录固定 `app_data`；Windows 安装版（NSIS currentUser）+ macOS dmg」 |
| `SPEC.md:342` | 便携模式机制详录 → 改写为「数据目录恒 app_data（不做老数据迁移，D-4）」 |
| `SPEC.md:103` | 「便携模式下数据目录跟着 exe 走」一句 → 删/改（后端回传路径的机制不变） |
| `docs/acceptance/matrix.csv:109` | 6-108 便携模式 → 改为「数据目录落点」条目，验收卡指向 ACC-P-03（改写后） |
| `docs/acceptance/cards-08-platform.md` | ACC-P-03「便携模式：数据库随 exe 走」→ 改写为「数据目录恒 app_data」 |
| `docs/acceptance/cards-90-windows.md` | ACC-W-01「便携模式与数据目录落点」→ 改写为「安装版安装目录/资源落位/升级卸载语义」 |
| `docs/PACKAGING-WINDOWS-PORTABLE.md` | 停发则归档（移 `docs/archive/`），或改为「旧版流程，仅供最后一版绿色包维护」 |
| `AGENTS.md` 地图 | 加一行本方案；门禁清单若新增校验脚本也要加 |
| `DEVLOG.md` | 记「为什么从绿色包改安装版」+ 四条拍板（含 D-4 不做迁移） |

> `tests-audit/audit_acceptance_matrix.py` 会把 SPEC 条目 ↔ 矩阵 ↔ 验收卡三方对拍，
> **改 SPEC 功能条目必须同批同步矩阵与卡**，否则门禁红。

## 九、风险与回退

| 风险 | 处置 |
|---|---|
| NSIS 交叉构建在 macOS 上失败（makensis 版本/插件不全） | 本机已有 makensis；首轮实机验证。若失败，退回「绿色包继续出 + 安装版另找 Windows 构建机」 |
| 安装包体积过大（~100–120MB）寄送不便 | D-2 已定「模型一起装」；若邮件发不出，改用网盘/分卷（沿用现有分卷寄送口径） |
| macOS `.app` 打包版语义记忆/OCR 解析不命中（现状，非本方案引入） | 本方案不动解析口径；若要修另立小改动（mac 下解析加查 `Contents/Resources`） |
| 卸载误删数据 | Tauri's「删除应用数据」默认**不勾选**（`$DeleteAppDataCheckboxState = 1` 才清），我们不改这个默认 |
| `_up_` 目录复发 | 门禁 A 的 `strings` 断言 + 实机「无 `_up_` 目录」两条兜底 |

## 十、偏离登记

落地偏离三条，均已回填本文相应段落：

1. **Windows 资源拆进 `tauri.windows.conf.json` 平台配置**（方案原稿把 staging 资源放 base conf）。
   原因：base conf 的资源表对所有目标生效，放 staging 三来源会让 mac 构建也强依赖暂存目录
   （`tauri-build` 编译期就校验资源存在性，缺件直接构建失败）；平台配置按 `--target` triple 生效
   （tauri-utils `config/parse.rs` 的 `read_platform(target, …)`，已对源码定案），mac 构建不合并
   windows conf。处置：base conf 只留 bge map，windows conf 自足列出全部五项资源。
2. **onnx dll 来源改 NuGet**（方案原稿误记「ort-sys copy-dylibs 落 dll」）。事实：Windows 目标
   ort 是 load-dynamic（`Cargo.toml` 尾部按 target 配置），构建期不需要 dll；dll 来源与绿色包手册
   §3 同款（NuGet microsoft.ml.onnxruntime，1.28.0 ↔ ort 2.0.0-rc.13）。处置：暂存脚本从 NuGet
   拉取并解压两个 dll；§二/§4.3/§五 已同步改写。另：`tauri-build` 编译期就校验资源存在性
   （比方案预期的 bundler 期更早），staging 缺件时交叉构建 `cargo build` 直接失败——fail-closed
   提前生效（本次交叉构建实证）。
3. **fallback WARN 收口改条件**（方案未涉及，B2 连带修复）。便携分支退役后「exe 目录 ≠ 数据目录」
   是安装版常态，原「exe 目录 ≠ 数据目录即 WARN」会在每次启动误报；改为只在 app_data_dir 解析失败
   退 temp 这条真正的异常路径记。`is_under_system_temp` / `is_macos_app_bundle_dir` 两个孤儿判定
   随本次改动删除。

落地记录：B1 `facdcf11` / B2 `cd0d84e3` / B3 `bfea56de` / B4 `accbcd70` / B6 `1c2f05b3`；
B5（Windows 实机 §六 B/C）待做。
