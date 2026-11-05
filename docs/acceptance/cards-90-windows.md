# ACC-W · Windows 专章（L3 发布层）

> 平台：Windows 10 / 11。macOS 是主线，Windows 只跑平台差异项——不重复 `cards-01` ~ `cards-08` 的主线卡。
> 现状：最近一轮 W10–W12 未跑，W04–W06 未跑，`docs/realmachine-regression-test.md` 的 W01–W09 只在 2026-10-22 跑过一次。
> **Windows 面是当前最大的结构性缺口**——本专章就是补这个缺口。

## 跑法

```powershell
# 一、换构建：记录 SHA256（出包后 SHA256 与路径两处都要记账）
Get-FileHash "D:\wmessage-portable-2026-10-30\wmessage.exe" -Algorithm SHA256

# 二、先定位数据目录——不知道 data_dir 在哪，后面所有观测都读错地方
powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "D:\Program Files\wmessage-portable-2026-10-30"

# 三、标水位线（bot.log 轮转后水位线失效并告警）
powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 mark

# 四、跑卡后取证据
powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 report
```

**五处平台差异**（每张卡都要看，改写的是测法不是卡面）：

| 项 | Windows 差异 | 对测法的影响 |
|---|---|---|
| ① 数据目录 | exe 同目录可写 → 锚定 exe 目录；不可写（如 `Program Files`）→ 回退 `%APPDATA%\com.renshi.wmessage`，记 WARN `data_dir_fallback` | **第一件事**：确认数据落在哪 |
| ② 端口闸 | `listening_supported()` = `cfg!(linux\|macos)`，Windows 上没有进程快照与监听 socket 采集路径 | 端口闸类卡整卡跳过；面板必须明说「本平台暂不支持」 |
| ③ 停止阶梯 | 无 unix 信号语义，走 `taskkill /PID <pid> /T /F` | 子进程树是否真被收掉要看 `/T` 是否生效 |
| ④ 技能物化目录 | `builtin_skills/` 是 `skills/` 的**兄弟**目录；数据目录回退后物化副本在 `%APPDATA%` 下 | 按回退后的真实目录找，别照 macOS 路径猜 |
| ⑤ 其余 | 注入走 UIAutomation + SendInput（UIPI 会拦）；窗口 id 是 `pid:hwnd`（不复用）；行尾 `\r\n`；路径分隔 `\` | 读 `bot.log` 找行要带 `\r` 容忍；桩子与回执里的路径是反斜杠 |

---

### ACC-W-01 · 安装版安装目录与数据目录落点

- **SPEC**：6-108（数据目录落点）
- **层级**：L3 发布层
- **历史卡**：W01,W10,S31（全）
- **前置**：Windows 实机 + 安装包（`wmessage_<版本>_x64-setup.exe`，出包流程见 `docs/WINDOWS-INSTALLER-DESIGN-2026-11-05.md`）
- **步骤**：
  1. 运行安装包，确认装到 `%LOCALAPPDATA%\wmessage`，目录结构与方案 §三 一致（逐项 `Test-Path`：`wmessage.exe`、`bge-small-zh-v1.5\`、`pp-ocr-v6\`、`dotnet\`、`onnxruntime.dll`）。
  2. 启动应用跑一轮对话，确认数据文件位置：`%APPDATA%\com.renshi.wmessage` 下出现 `wmessage.db` / `bot.log` / `AI_Gen_Files`；exe 目录旁无 `wmessage.db`。
  3. exe 目录旁预放一个 `wmessage.db` 再启动——数据仍落 `%APPDATA%`，痕迹不被锚定。
- **判据**：
  1. 【可观测】安装目录结构与预期一致，且 **exe 旁无 `_up_` 目录**。
  2. 【可观测】数据文件在 `%APPDATA%\com.renshi.wmessage`，exe 目录旁无。
  3. 【可审计】`bot.log` 有数据目录定版留痕（数据目录 = app_data）。
- **证据**：两处目录树截图 + `bot.log` 片段。
- **失败处置**：先查安装目录实际结构（是否缺资源 / 有 `_up_`），再查 `src-tauri/src/paths.rs` 的判定。
- **缺口补验**：升级语义（用户自放文件保留）与卸载勾选删数据的实机复跑。

---

### ACC-W-02 · 停止阶梯：子进程树是否被收干净

- **SPEC**：3-35（停止有回显）
- **层级**：L3 发布层
- **历史卡**：W12（冒烟人工段）
- **前置**：先让机器人起一个长跑子进程（`run_python` 跑 `time.sleep(300)` 或启动一个受管服务）
- **步骤**：
  1. 点停止。
  2. 用 `Get-Process` / `tasklist` 查子进程是否还在。
  3. 查应用自身是否存活（停止只停执行，不退应用）。
- **判据**：
  1. 【可观测】停止后子进程不在 `tasklist` 里（`/T` 树杀生效）。
  2. 【可观测】应用主进程仍在运行，聊天窗可继续用。
  3. 【可核对】`taskkill` 的目标 PID 树内无残留进程。
- **证据**：`tasklist` 截图（前/后各一张）。
- **失败处置**：Windows 上没有 unix 信号语义，若残留先看 `taskkill` 是否带了 `/T`。
- **缺口补验**：本条在近几轮提交（`/stop` 可打断、check 薄片等待）改动后未复跑。

---

### ACC-W-03 · 交付给用户的产物不带「来自互联网」标记

- **SPEC**：6-109（产物不带「来自互联网」标记）
- **层级**：L3 发布层
- **历史卡**：W03（全）
- **前置**：五个文档硬依赖（openpyxl / python-docx / python-pptx / pypdf / reportlab）在位
- **步骤**：
  1. 让机器人依次产出 docx / word_revisions / xlsx / pdf / pptx 五种产物。
  2. 对每个产物跑 `Get-Item -Stream *`，看有没有 `Zone.Identifier` 这条备用数据流。
  3. 另测：Word 模板上传、run_python 产物搬运两个出口。
  4. **复制 / 搬运路径**（2026-11-02 新增覆盖）：下载一个带标记的 .docx，让机器人复制到产物目录、或用桌面清理归档到另一目录，对副本跑 `Get-Item -Stream *`。
  5. 另下载一个带标记的 .exe，走同一条复制路径，对副本跑 `Get-Item -Stream *`（可执行文件不收口）。
- **判据**：
  1. 【可核对】五个产物 + 两个出口共七处，`Get-Item -Stream *` 均只列出主数据流，无 `Zone.Identifier`。
  2. 【可观测】用 Word 打开产物不进受保护视图。
  3. 【可核对】用户自己放的原始文件（未走出口）仍带原样标记——**app 不擅自改用户的文件**。
  4. 【可核对】步骤 4：复制 / 归档后的文档副本 `Get-Item -Stream *` 无 `Zone.Identifier`；步骤 5：可执行文件副本仍带 `Zone.Identifier`。
  5. 【可观测】在 360 安全卫士的「主动防御 / 拦截记录」里，重现「打开文档 → 关闭」不再出现「修改 文档或图片文件」条目（Word 删除 Office 文档缓存副本那一次不再被拦）。
- **证据**：每个产物的 `Get-Item -Stream *` 输出。
- **失败处置**：剥不掉时按 SPEC 只记审计不阻断（标记只影响 Office 的打开路径，文件本身是好的）。
- **缺口补验**：本条是 Windows 上「杀软连坐拦写文档」的根因，改动后必须复跑。

---

### ACC-W-04 · 本机运行环境面板（Windows 装法）

- **SPEC**：6-110（本机运行环境监测）
- **层级**：L3 发布层
- **历史卡**：—（本机运行环境面板是 10-27 新增，Windows 专面从未跑过）
- **前置**：设置页 → 机器人 → 本机运行环境
- **步骤**：
  1. 展开 Python 包 / Node 全局包 / 运行时与启动器三组。
  2. 记录每组缺项与给出的安装命令。
  3. 核对命令是否可直接复制执行。
- **判据**：
  1. 【可观测】Python 组命令是 `python -m pip install …`（**不是 `python3`**——Windows 官方安装器只装 `python.exe`）。
  2. 【可观测】Node 组是 `npm i -g …`，只列三个（pptxgenjs / @mermaid-js/mermaid-cli / @agent-canvas/cli）。
  3. 【可观测】运行时组逐项给命令（如 `soffice` 单列一条），五条文档硬依赖单独标出。
  4. 【可核对】`C:\Program Files\LibreOffice\program` 与 `C:\Program Files\dotnet` 两个固定探测目录被纳入搜索——不补就是「装完了还报缺」。
  5. 【可观测】面板用途栏明说「Windows 上只有 python」。
- **证据**：三组展开态截图 + 复制出的命令文本。
- **失败处置**：装法来自 Rust 侧一处（`src-tauri/src/env_probe.rs`），核对 winget 包 ID 时以 `microsoft/winget-pkgs` 清单为准。
- **缺口补验**：本条是 Windows 专面新增条目，必须在本轮跑出结论。

---

### ACC-W-05 · 端口闸与进程资源采集的平台能力声明

- **SPEC**：6-110 / 3-82（MCP 面板资源采集）
- **层级**：L3 发布层
- **历史卡**：W04
- **前置**：设置页 → MCP 面板，已配置一个 stdio 型外部服务器并启用
- **步骤**：
  1. 打开 MCP 面板，观察资源采集区。
  2. 观察「端口与地址」区。
- **判据**：
  1. 【可观测】面板明说「本平台暂不支持进程资源采集」与「本平台暂不支持监听端口采集」——**采空是没判，不是没占用**。
  2. 【可核对】`mcp_status` 回带的 `listeningSupported` 与 `resourceSupported` 在 Windows 上为 false。
  3. 【可观测】面板**不得**显示「无监听端口 / 无资源占用」这类把没判说成判了没事的文案。
- **证据**：MCP 面板截图 + `mcp_status` 返回体片段。
- **失败处置**：判据是平台能力标志位，不是文案。

---

### ACC-W-06 · MCP stdio 读侧行长上限

- **SPEC**：3-82（MCP 外部服务器）
- **层级**：L3 发布层
- **历史卡**：W04,W05
- **前置**：一个会输出超长单行（无换行）的第三方 MCP 服务器，或构造一个 2MB 单行输出
- **步骤**：
  1. 启用该服务器并调用其工具。
  2. 观察宿主是否无界分配、是否 OOM。
  3. 观察该行之后的消息是否仍能正常应答。
- **判据**：
  1. 【可观测】宿主不崩溃、不卡死。
  2. 【可观测】该行按「不是消息」跳过，**会话不断**，服务器后续应答正常。
  3. 【可核对】读侧每行最多取 1MB；stderr 侧只留每行尾部 4KB。
- **证据**：调用前后内存快照 + `bot.log` 片段。
- **失败处置**：若宿主无界分配，查 `rmcp` 读侧的行长上限是否在本平台生效。

---

### ACC-W-07 · 凭据管理器条目核对（结构性不可测项）

- **SPEC**：5-105（密钥纪律）
- **层级**：L3 发布层
- **历史卡**：W08
- **前置**：已配置 MCP 机密（env / headers 各一项）
- **步骤**：
  1. `cmdkey /list` 列出凭据条目名。
  2. 用应用侧改一次机密（脱敏视图里改写），确认条目数与条目名不变。
  3. 删除一行机密，确认对应条目消失。
- **判据**：
  1. 【可核对】`cmdkey /list` 能看到对应条目名。
  2. 【可观测】**值验证在 Windows 侧结构性不可测**——`cmdkey` 只给条目名不给值；这一格判据要写「不可测」，不能靠猜。
  3. 【可观测】删除某行机密后对应条目消失，其余条目不受影响。
- **证据**：`cmdkey /list` 输出截图 + MCP 面板脱敏视图截图。
- **失败处置**：条目名对不上时先确认数据目录（回退路径会改条目作用域）。
- **缺口补验**：实测遗留 4 条 `wmessage.bot.v1` 条目 ACL 为 `-A`，**待拍板**，本轮确认其是否仍存在。

---

### ACC-W-08 · 行尾与路径形态（Windows 特有）

- **SPEC**：3-77（read_text_file）/ 3-76（回灌上限）
- **层级**：L3 发布层
- **历史卡**：—（形态差异从未专卡）
- **前置**：一个含 CRLF 行尾的文本文件、一个含非 ASCII 字符名的文件
- **步骤**：
  1. 让机器人读该 CRLF 文件。
  2. 让机器人读该中文名文件。
  3. 看回灌内容里的行尾与路径形态。
- **判据**：
  1. 【可观测】回灌内容里的行尾是 `\r\n` 形态（不是被抹成 `\n`）。
  2. 【可观测】路径分隔符是 `\`。
  3. 【可观测】非 ASCII 文件名读取不乱码、不截半字（切点落在 code point 边界）。
- **证据**：截图 + `bot.log` 中工具返回片段。
- **失败处置**：行尾被规范化时查 `read_text_file` 的按字节预算收页逻辑是否误吞 `\r`。

---

### ACC-W-09 · 键盘鼠标注入（UIAutomation + SendInput）

- **SPEC**：3-78（工具清单，含桌面自动化六件）
- **层级**：L3 发布层
- **历史卡**：W07
- **前置**：桌面自动化总开关已开；另开一个记事本窗口
- **步骤**：
  1. 让机器人 `list_windows` 列窗口并 `activate_window` 激活记事本。
  2. 让机器人 `ui_inspect` 读控件树。
  3. 让机器人 `ui_click` 点一个按钮、`ui_type` 输入文本。
  4. 让机器人 `wait_for_element` 等一个元素出现。
- **判据**：
  1. 【可观测】四个动作在目标窗口上生效（记事本里真的出现了输入的文本）。
  2. 【可观测】注入前目标窗口确实被激活（Windows 上前台判定是 `GetForegroundWindow() == hwnd` 精确匹配）。
  3. 【可观测】被 UIPI 拦时，工具回**失败**状态而不是假成功。
- **证据**：记事本窗口截图 + `bot.log` 片段。
- **失败处置**：UIPI 会拦高权限窗口的注入——被拦是平台行为，不是应用缺陷，但必须如实报失败。

---

### ACC-W-10 · 安装版出包与目录结构

- **SPEC**：6-108（数据目录落点）
- **层级**：L3 发布层
- **历史卡**：W01,W10
- **前置**：完成一次打包（见 `docs/WINDOWS-INSTALLER-DESIGN-2026-11-05.md`：暂存脚本 → 交叉构建）
- **步骤**：
  1. 暂存 + 交叉构建，产出 `bundle/nsis/*.exe` 安装器。
  2. 本地核对：`7z l` 列安装器内容，应含 `pp-ocr-v6/det.onnx`、`bge-small-zh-v1.5/onnx/model_quantized.onnx`、`dotnet/wm-docx-revisions.exe`、`onnxruntime.dll` 四项，且**无 `_up_` 目录、无 `.DS_Store`**。
  3. 实机安装后核对目录结构（配合 ACC-W-01）。
- **判据**：
  1. 【可观测】构建成功产出安装器，体积 ~85–100MB。
  2. 【可核对】`7z l` 断言四项资源在包内、无 `_up_`、无 `.DS_Store`（**不要用 `strings`**：NSIS LZMA solid 压缩使路径不裸露，恒查不到——首包验证曾误判资源缺失）。
  3. 【可核对】安装器的 SHA256 与发版记录一致。
- **证据**：构建日志 + `7z l` 输出 + SHA256。
- **失败处置**：资源落位不对先查 `tauri.windows.conf.json` 的 resources 映射与暂存目录；**换包必须重记 SHA256 与路径两处**。

---

### ACC-W-11 · 探针脚本的 Windows 形态

- **SPEC**：6-112（日志体系）
- **层级**：L3 发布层
- **历史卡**：W01–W09 的取证底座
- **前置**：`D:\wmessage-probe\regression-probe.ps1` 就位
- **步骤**：
  1. 跑 `locate` / `mark` / `report` / `services` / `orphans` / `keyring` / `fixes` 七个子命令。
  2. 核对 `report` 输出的事件分组与工具调用流水。
- **判据**：
  1. 【可观测】七个命令全部退出码 0。
  2. 【可核对】`services` 报出的 pid 与端口与任务管理器一致（pid 取自 `service.started` 事件而非应用自报）。
  3. 【可核对】`orphans` 在宿主已死时能报出还活着的服务。
  4. 【可核对】读 `bot.log` 的行匹配容忍 `\r`（CRLF 不导致漏行）。
- **证据**：各子命令输出留存。
- **失败处置**：水位线在 `bot.log` 轮转后失效并告警——先重打 mark。

---

### ACC-W-12 · 冒烟 14 项人工段（Windows 对照）

- **SPEC**：6-106（SQLite）/ 6-112（日志体系）/ 2-24（聊天常挂载）
- **层级**：L3 发布层
- **历史卡**：W12
- **前置**：全新解压的包，数据目录为空
- **步骤**：逐项对照 `docs/realmachine-smoke-test-2026-10-25.md` §一 的 14 项基线冒烟（启动无 panic / error.log 零增量 / 关键表数 / 四页渲染 / 两轮真实往返 / 优雅退出与 `app_exit_cleanup` / 文件面静态）。
- **判据**：
  1. 【可观测】14 项逐项有结论，不许「大致正常」。
  2. 【可核对】`PRAGMA integrity_check` 返回 ok。
  3. 【可审计】退出后 `bot.log` 中出现 `app_exit_cleanup`。
  4. 【可观测】四页渲染截图齐全。
- **证据**：14 项结论表 + 截图集。
- **失败处置**：macOS 有 `scripts/realmachine/smoke.sh` 自动段，Windows 没有对应脚本——这一段只能人工过。

---

## 与主线卡的对应

| W 卡 | 对应功能 | 主线卡 |
|---|---|---|
| ACC-W-01 | 6-108 | ACC-P-03 |
| ACC-W-02 | 3-35 | ACC-I-03 |
| ACC-W-03 | 6-109 | ACC-P-04 |
| ACC-W-04 | 6-110 | ACC-P-05 |
| ACC-W-05 | 6-110 / 3-82 | ACC-P-05 / ACC-T-04 |
| ACC-W-06 | 3-82 | ACC-T-04 |
| ACC-W-07 | 5-105 | ACC-G-03 |
| ACC-W-08 | 3-77 / 3-76 | ACC-E-21 / ACC-E-20 |
| ACC-W-09 | 3-78 | ACC-E-22 |
| ACC-W-10 | 6-108 | ACC-P-03 |
| ACC-W-11 | 6-112 | ACC-P-07 |
| ACC-W-12 | 6-106 / 6-112 / 2-24 | ACC-P-01 / ACC-P-07 / ACC-C-03 |
