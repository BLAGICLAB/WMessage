# 真机变异测试方案：2026-10-23 变更回归

针对 **2026-10-23 00:00 → 20:02** 这批改动的实机验收方案。**macOS 与 Windows 双平台**，两平台的测法有实质差异，见 §1.0。

与 `docs/realmachine-regression-test.md`（覆盖 10-19 ~ 10-22）互补：**那份验「10 月 19 日那批改动」，这份验「今天这批」**。且这份的骨架是**变异注入复验**——每张卡都写清「把 bug 注回去，重跑应当 FAIL」的锚点，证明这套测试有牙齿。

| 项 | 值 |
|---|---|
| 覆盖窗口 | `git log --since=2026-10-23 00:00` → 68 个提交 / 99 个文件 / +11 285 −973 行；**外加实机跑卡时踩出来的 1 条崩溃修复**（M39） |
| 基线 | `7cd490c1`（2026-10-22 23:56，批 2 网络 API 面收尾） |
| 行为变更 | 42 条（`fix` 32 / `feat` 7 / `test` 3），其余 26 条是文档 |
| 卡片 | 42 张场景卡（M01–M39 双平台为主 + W01–W03 Windows 专章），覆盖 37 条行为变更 + 1 条实机崩溃修复 |
| 变异注入 | 25 条（V01–V25），总表见 §七 |
| 配套探针 | macOS/Linux → `scripts/regression-probe.sh`；**Windows → `scripts/regression-probe.ps1`** |
| 兄弟文档 | `docs/realmachine-regression-test.md`（10-19~10-22）· `docs/realmachine-rerun-2026-10-22.md`（复验手册）· `docs/realmachine-overflow-test.md`（溢出漏斗）· `docs/SUBAGENT-HANDOFF-BUDGET-REDESIGN-2026-10-23.md`（子 agent 预算方案） |

---

## 〇、这份文档是什么

**是**：一份「怎么在真机上证明今天这批改动是对的」的逐项清单。每一项给一条可直接粘贴的提示词、一个能判 PASS/FAIL 的观测点、以及一次变异注入复验（把 bug 注回去，应当 FAIL）。

**不是**：不是单测的替代，也不是「再跑一遍功能」。单测证明纯函数在被喂进特定入参时表现正确；这里撬的是**跨模块接缝**——单测结构性够不到的地方：

| 接缝 | 坏了会怎样 | 单测能不能看见 |
|---|---|---|
| 前端叠层 ↔ 后端流式广播 | 切一次设置页，在途回复的流式过程永久丢 | 不能（订阅注销发生在挂载层） |
| 路由表 ↔ 真实技能目录 | 用户点名的技能被 intent 正则截胡，逼出「刚切就断」 | 不能（路由表是进程级全局状态） |
| keyring ↔ 配置写盘 | 一次无关写盘把唯一一份密钥永久抹掉 | 不能（keyring 是系统服务） |
| 窗口 id ↔ 真实窗口树 | 旧 id 打到另一扇窗 | 不能（id 复用只在真机上发生） |
| 子 agent 墙钟 ↔ 真实 LLM 回合 | 熔断时正文悬空，父 agent 从零重烧 | 不能（时间预算只有真跑才耗得完） |
| 标签合并 ↔ 真实归档数据 | 子串命中把从没拥有该簇成员标签的卡打上主标签 | 不能（要真造出聚簇数据） |

**选样原则**：只收「错了会静默发生、且当场看不出来」的项。纯文档对拍、纯门禁脚本、纯 UI 美化不在此列（它们归 `bash scripts/test-fast.sh` 管）。今天 68 个提交里，26 条是文档、5 条改的不是运行时行为（门禁工具本身 / db 锁断言 / 3 条纯测试提交）→ 这 31 条去附录 C。剩下的 37 条行为变更落进 41 张卡——**一个提交可以拆成多张卡**（`1c9ea780` 一条拆成 8 张：M02 / M06 / M07 / M08 / M09 / M10 / M38 / W02），**两张卡也可以合一条提交**（M19 覆盖 `323d4daf` + `bafb57e9`，M33 覆盖 `d660eb0b` + `09526909`）。

> **M39 是这张表的第 42 张卡，也是唯一一张不在上面那个窗口里的卡**：它是跑 M01 时实机崩出来的（见附录 D-1）。**卡片的第一个作用就是发现文档之外的问题**——这一张就是。

**与「单测变异脚本」的关系**：仓库里已有 `scripts/mutation-trace-funnel.sh`——那是**单元测试层**的变异（注入 → 跑 `cargo test` → 看 KILLED / SURVIVED）。本文是**真机层**的同构物：注入 → 跑真实场景 → 看判据是否 FAIL。两者互补：单测变异能证明「有约束在守」，真机变异能证明「约束在真实环境里真的生效」。

---

## 一、前提条件与通用判据

### 1.0 Windows 实机适配（先读这一节）

Windows 上有四处平台差异会**直接改写测法**，不是「注意一下」的程度。

#### ① 数据目录可能不在 `D:\Program Files\...`

便携包的 `data_dir` 判定顺序（`src-tauri/src/paths.rs`）：exe 同目录可写 → 锚定 exe 目录；不可写（如 `Program Files`）→ 回退 `%APPDATA%\com.renshi.wmessage`，并记一条 WARN `data_dir_fallback`。

> **第一件事：确认数据到底落在哪。**
> ```powershell
> powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "D:\Program Files\wmessage-portable-2026-10-23"
> ```
>
> **本批卡片的判据几乎全落在数据目录里**（`bot.log` / `wmessage.db` / `runtime/flags/` / `checkpoints/`）——**不知道 `data_dir` 在哪，后面所有观测都读错地方**。

#### ② 端口闸在 Windows 上**不生效**——这不是 bug

`listening_supported()` = `cfg!(linux|macos)`。Windows 上没有进程快照与监听 socket 的采集路径，所以面板会明说「本平台暂不支持监听端口采集」。

**这改变了 M31 的测法**：Windows 上能验的是「平台能力标志说不说真话」，验不了「闸真杀」。

#### ③ 停止阶梯走 `taskkill`

Windows 上没有 unix 那套信号语义，停止走 `taskkill /PID <pid> /T /F`。本批**没有改停止阶梯**，但 M29 / M30 的地面真相要用 `Get-Process` / `tasklist` 对照。

#### ④ 其余平台差异速查

| 项 | macOS / Linux | Windows |
|---|---|---|
| 键盘 / 鼠标注入 | AX（辅助功能）+ CGEvent | UIAutomation + SendInput（**UIPI 会拦**） |
| 窗口 id | `pid:AXWindows下标`（**下标会复用**） | `pid:hwnd`（hwnd 不复用） |
| 前台判定 | 两级（应用 pid + 窗口标题），标题读不到就退化为「同应用即可」 | `GetForegroundWindow() == hwnd`（精确匹配） |
| 控件当前值 | AXValue | UIA ValuePattern（**修复前恒 None**，见 W02） |
| 密码框识别 | 角色 `AXSecureTextField` | UIA `CurrentIsPassword` |
| MCP 机密存储 | macOS Keychain | Windows 凭据管理器（`cmdkey /list` 只能看条目名） |
| 文件权限位 | unix 权限位生效（M10 在这里验） | **没有 unix 权限位**，M10 在 Windows 上不生效 |
| 产物「来自互联网」标记 | 不存在这个概念（W03 在 Windows 上才验） | `Zone.Identifier` 备用数据流 |

### 1.1 环境

| # | 条件 | 怎么确认 |
|---|---|---|
| 1 | 应用跑起来了 | macOS：`npm run tauri dev`（**必须用 dev**：有几条诊断走 stderr，Finder 启动的 .app 会把 stderr 丢进系统日志）。Windows：绿色包双击（**stderr 在 GUI 启动下同样看不到**） |
| 2 | bot 已启用 | 任务卡绑定目录 + `bot-enabled.flag` 存在 |
| 3 | 探针能找到日志 | macOS：`bash scripts/regression-probe.sh locate`；**Windows：`powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "..."`** |
| 4 | 每个场景前打水位线 | `mark` → 场景后 `report`（两平台同名子命令） |
| 5 | **本批额外**：本地嵌入模型可用 | M28 / M29 / M36 的标签聚类走 `embed_text`（本地 ONNX）；引擎不可用 → 扫描**静默返回 0 个簇**，弹窗只显示「没有发现需要合并的标签簇」 |
| 6 | **本批额外**：已配厂商（LLM 可用） | M21 / M22 / M24 / M25 / M26 都要真发请求 |
| 7 | **macOS 两项系统权限**（跑 M01–M10 / M38 之前先给） | 「系统设置 → 隐私与安全性 → **辅助功能**」（AX 读窗口/控件）与「**屏幕录制**」（`screencapture` 留证）。**缺辅助功能** → `list_windows` 报「本进程尚未获得 macOS「辅助功能」权限」；**缺屏幕录制** → `screenshot` 报「截图失败：screencapture 退出码 exit status: 1」。两项都缺时模型会一路重试工具、把轮次烧光（2026-10-23 实机就是这样从 turn=0 烧到 turn=21），**先把权限给足再开跑** |

> **Windows 上执行不需要仓库源码**：只要绿色包 + 探针脚本（`D:\wmessage-probe\regression-probe.ps1`，一个文件）。本文的源码坐标只是**靶点溯源**，执行时不必打开。

> **日志路径**：`<data_dir>/bot.log`，`<data_dir>/error.log` 是 WARN/ERROR 镜像。
> 轮转 5 MiB，保留两代——**跨轮转的水位线失效**，脚本会提示重新 `mark`。

### 1.2 授权模式决定能跑哪些场景

设置页 →「机器人」分区里的四个开关决定绝大多数场景能不能跑：

| 开关 | 管住什么 | 不开的话 |
|---|---|---|
| 允许机器人执行 Python | `run_python` / `ast_inspect` | 被拒 |
| 允许机器人启动本地服务 | `start_service` | 被拒 |
| 允许服务暴露到非 loopback | 端口闸 L1 | macOS/Linux：绑 `0.0.0.0` 的服务被杀掉。**Windows：这条闸本就不生效** |
| 允许机器人操作桌面界面 | M01–M10 全部 | 被拒：`UI 自动化未开启：请到 设置页 打开「允许机器人操作桌面界面」后再用这些工具。` |

**授权模式切 `yolo` 时，以上开关全部豁免**——所以验「被拒」语义时**必须切回 strict**，否则测的是「根本没闸」。

### 1.3 提示词的三条铁律

1. **必须带完整指令外壳。** 只把裸代码/裸路径粘进对话框，模型会当成普通问题直接回答，`tools=0`，一个都没跑。
2. **必须要求「原样执行，不许改动任何字符」。** 模型很爱"贴心地"补个换行、加个 `len()`——字节数一偏，边界测试作废。
3. **一次一件事。** 同一轮里塞多个任务，后一件的失败会把前一件的现象盖掉。

### 1.4 判据的三种形态

| 形态 | 怎么看 | 适合什么 |
|---|---|---|
| **工具返回原文** | 聊天框里模型贴出的工具返回值 | 参数闸、拒绝语义、schema 契约 |
| **审计行** | `probe report`（bot.log） | 状态机、闸的判定、生命周期 |
| **地面真相** | macOS：`ps` / `lsof` / `security find-generic-password` / `ls -l`；Windows：`Get-Process` / `cmdkey /list` / 通知中心 | 进程、端口、机密、文件权限、系统通知 |

**铁律：应用自报不算数。** `stop_service` 说「已停」不算，要看进程真的没了；面板说「无资源占用」不算，要看端口真的没被占。这批改动里有一半的 bug 形态就是「自报与事实分家」。

### 1.5 变异注入的通用流程

**单测层**的注入脚本见 `scripts/mutation-trace-funnel.sh`（判定词：KILLED / SURVIVED / BUILD_ERR / NO-OP）。**真机层**的流程如下：

```bash
# 0. 工作区必须干净（还原会覆盖真改动）
git status --short            # 应当无输出

# 1. 定位锚点（行号会漂，先 grep 别硬写行号）
grep -n "fn detail_is_dangerous" -A 12 src-tauri/src/bot_ui.rs

# 2. 改坏 → 重启应用 → 跑对应场景 → 期望 FAIL
# 3. 还原
git checkout -- src-tauri/src/bot_ui.rs
```

**不要用 `sed -i`**（BSD/GNU 语义不同，还会留 `.bak`）。正确写法是写临时文件再 `mv -f` 覆盖。

> **Windows 上的变异复验要多一步出包。** 绿色包是**从 macOS 交叉编译**出来的（`npx tauri build --target x86_64-pc-windows-gnu --no-bundle`，见 `docs/PACKAGING-WINDOWS-PORTABLE.md` §1），所以注入流程是：
> ```bash
> # mac 侧：改坏 → 交叉编译 → 组包 → 传到 Windows
> CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
> CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
> CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
> npx tauri build --target x86_64-pc-windows-gnu --no-bundle
> # 把新的 wmessage.exe 覆盖进绿色包目录 → 传到 Windows → 覆盖旧 exe → 重启应用
> ```
> 只换 `wmessage.exe` 就够（前端资源与 DLL 没动）。**覆盖 exe 前先关掉正在跑的 wmessage.exe**，否则文件被占用写不进去。
>
> **前端变异不用出包**：改 `src/` 下的 TS/TSX，Vite 热更新即可。

### 1.6 判据里最容易踩的两个坑

1. **`spawn_subagent` 的回执是纯文本，不是 JSON。** 必须逐字核对：`生效预算：墙钟 {}s / 轮数 {} / 工具调用 {}。`
2. **`check_subagent` 的返回是 camelCase JSON**（`subagentId` / `roundsUsed` / `roundsBudget`），但 **`subagents` 表的列名是 snake_case**（`budget_json` / `result_json` / `context_summary`），且 `budget_json` / `result_json` **内部**是 camelCase。

---

## 二、场景总表

按「错了会怎样」分档，不按模块分。

### T0 档：安全边界（11 项）— 错了会真实造成损害

| # | 场景 | 提交 | 关键观测 | 平台 |
|---|---|---|---|---|
| M01 | 危险动作闸按小写匹配 | `80487196` | 英文按钮弹确认窗 | 双 |
| M02 | 无标题按钮 fail-closed | `1c9ea780` | 纸飞机按钮弹确认窗 | 双 |
| M03 | 敏感控件闸认角色与 IsPassword | `4c698b08` | 拒绝文案 + 密码框空 | 双 |
| M04 | `ui_type` 拒绝带换行的文本 | `4559a905` | 拒绝文案 + 目标框空 | 双 |
| M05 | 注入前要求目标窗口在前台 | `d61707df` | 拒绝文案 + 前台窗口没多字 | 双 |
| M06 | 窗口 id 复用（TOCTOU） | `1c9ea780` | 「已经不是刚才那扇窗了」 | **仅 macOS** |
| M07 | 闸门审的控件 ≠ 动的控件 | `1c9ea780` | 「界面已变：控件 …」 | 双 |
| M08 | 注入无全局互斥 | `1c9ea780` | 两框各只有自己那串 | 双（难造） |
| M09 | UI 总开关变更零留痕 | `1c9ea780` | `ui_automation_toggle` 审计行 | 双 |
| M10 | flag 文件权限过宽 | `1c9ea780` | `-rw-------` | **仅 unix** |
| M39 | 不许操作自己的窗口 | **本轮实机跑出来的** | 清单里没有自己 + 碰自己被拒 + **应用不崩** | 双 |

### T1 档：静默失效与状态谎报（15 项）— 最阴的一类

| # | 场景 | 提交 | 关键观测 | 平台 |
|---|---|---|---|---|
| M11 | OCR 解码炸弹面 | `e07b7a0d` | 字节/像素闸的拒绝文案 | 双 |
| M12 | 必应大图先查长度再流式读 | `992d7c5c` | 超限直接拒 | 双 |
| M13 | 图谱铭牌不拼 `innerHTML` | `c23bc5d7` | 标题里的尖括号不被解析 | 双 |
| M14 | 厂商条目不借全局主 key | `48e96dad` | 探活报错 + 无 Authorization 头 | 双 |
| M15 | 写盘前安置旧明文密钥 | `5c5b6e93` | 保存中止 + 盘上明文原样还在 | 双（破坏性） |
| M16 | 终稿自检侧调过 vendor 未验证门 | `4e8f5581` | `final_selfcheck.vendor_not_verified` | 双 |
| M17 | 空 token 文件改走原子覆写 | `afe7f47b` | `api_start` 成功 + 磁盘非空 | 双（破坏性） |
| M18 | 设置页改叠层 · 在途流式不断 | `7520669b` | 切设置期间流式不中断 | 双 |
| M19 | 在途流式可回灌 | `323d4daf` `bafb57e9` | 重载后气泡接着长 | 双（**只能验整页重载**） |
| M20 | 输入草稿落盘 | `01ba711b` | 切设置后输入框还在 | 双 |
| M21 | 子 agent 预算治理 | `968f26be` | 回执含「生效预算」 | 双 |
| M22 | 子 agent 收尾契约 + digest | `ef3f3baa` | 「残留产出」「机检未过」 | 双 |
| M23 | 切技能取活着的 run | `a55bb74c` | 新技能真跑起来 | 双 |
| M24 | 用户点名技能优先于 intent | `708507bc` | `skill.start` 的 name 是点名的那个 | 双 |
| M25 | 等确认技能不占同名技能 | `b1ac8348` | `skill_expired` + 另一会话能启 | 双 |

### T2 档：可感知语义（13 项）

| # | 场景 | 提交 | 关键观测 | 平台 |
|---|---|---|---|---|
| M26 | 出口闸：自检判定不当终稿 | `73331d76` | 终稿不是裸 Y/N | 双 |
| M27 | 老库补 onion 两列 | `fad6ffa6` | 启动日志无缺列报错 | 双 |
| M28 | 标签合并搬进事务 + 别名精确匹配 | `bac7f803` | 子串卡 tags 不动 | 双 |
| M29 | 标签合并/撤销后广播 + 占位值拒收 | `8572b5d3` | 两页不刷新就变 | 双 |
| M30 | 程序运行的僵尸行 | `ce3faead` | 失败行落终态 | 双 |
| M31 | 采集失败的审计记在转折上 | `d020b6c7` | 只记一条 | macOS 专有 |
| M32 | 机器人总开关关闭时不空转 | `633e7111` | 无 `sched_job_skip` 刷屏 | 双 |
| M33 | 上下文栏默认看改动 | `d660eb0b` `09526909` | 累计 ± 与「看全文」开关 | 双 |
| M34 | MCP 面板资源行按 5s 轮询 | `01eaaa44` | 「已运行」秒数在涨 | 双 |
| M35 | `taskEq` 键序无关比较 | `b2ad135a` | 乱序键不再判成改动 | 双 |
| M36 | 标签弹窗「新建主标签」能打出名字 | `48b53c9b` | 输入框能连着打字 | 双 |
| M37 | 机器人气泡宽度只增不减 | `cd060bcd` | `min-width` 单调不减 | 双 |
| M38 | macOS `list_windows` 前台标注 | `1c9ea780` | 只有前台那行带「（前台）」 | **仅 macOS** |

### Windows 专章（3 项）

| # | 场景 | 提交 | 关键观测 |
|---|---|---|---|
| W01 | 坐标兜底点击把像素换算成归一化值 | `25fdcd9b` | 落点在目标控件上，不在左上角 |
| W02 | Windows `UiNode.value` 不恒 None | `1c9ea780` | `ui_inspect` 有「值:」段；密码框没有 |
| W03 | 产物交付前剥「来自互联网」标记 | `d37d3fb0` | 产物不带 `Zone.Identifier` |

---

## 三、T0 档：安全边界

### M01 · 危险动作闸按小写匹配

**靶点**：`src-tauri/src/bot_ui.rs::detail_is_dangerous`（提交 `80487196`）

**bug 形态**：`DANGEROUS_KEYWORDS` 全是小写字面量，而修复前的判定是大小写敏感的子串匹配。英文 UI 的标准写法是首字母大写——`Send` / `Submit` / `Delete` / `OK` / `Remove` / `Pay` / `Buy` / `Order` 全部**不命中** → 确认弹窗**一个都不弹**，点击直接执行。

> 反向形态（修好后必须防住）：`ok` 若按子串找，窗口名 `Microsoft Outlook`、`Token窗口` 都含 `ok` → 每次点击都要人确认 → 确认疲劳。

**步骤**：

1. 设置页 → 打开「允许机器人操作桌面界面」；**macOS 先把「辅助功能」和「屏幕录制」两项系统权限给足**（见 §1.1 第 7 行——2026-10-23 实机第一次跑这张卡就是权限没给全，模型一路重试把轮次烧到 21 轮）。
2. 打开一个**英文界面**应用（macOS：Safari / 邮件；Windows：记事本英文界面或浏览器）。
3. 在聊天框发：

```
在当前打开的英文界面应用里，找到标题里含 Send 的按钮并点击它。如果找不到含 Send 的按钮，就找含 Delete 的按钮点击。点击前先告诉我你找到的按钮标题。
```

**判据**：

- PASS：弹出确认窗，detail 是 `在「Mail」（Mail）里点击「Send」`；bot.log 出现 `confirm | id: … | kind: danger | ui_click | …`。
- FAIL：**不弹窗**，点击直接执行。
- **负向（必做）**：点 `Microsoft Outlook` 窗口里的普通按钮 → **不应**弹窗。
- **中文侧回归**：点中文「发送」按钮 → 应弹窗（防变异时只顾英文）。

**变异注入复验**：把 `detail_is_dangerous` 里的 `let lower = detail.to_lowercase();` 改成 `let lower = detail.to_string();` → 重跑应 **FAIL**（英文按钮不再弹窗）。
单测侧同款变异已覆盖：`bot_ui::tests::english_button_titles_must_hit_the_dangerous_gate` → 变红；`short_keyword_ok_must_not_trip_on_window_names` → 变红。

**平台**：跨平台（判定在 `bot_ui.rs` 公共层，两平台共用）。

---

### M02 · 无标题按钮完全绕过危险闸（fail-closed）

**靶点**：`src-tauri/src/bot_ui.rs::click_needs_confirm`（提交 `1c9ea780`）

**bug 形态**：detail 模板是 `在「{窗口}」（{应用}）里点击「{title 或 role}」`；目标控件**没有标题**时退化成 **role**（如 `AXButton`）→ role 名不含任何危险关键词 → **一个确认都不弹**。微信/飞书/Telegram 的**纸飞机发送按钮**、**垃圾桶删除按钮**都是纯图标 → 点它们直接发出 / 直接删除。

**步骤**：

1. 打开微信 / Telegram / 飞书（macOS、Windows 都行）。
2. `ui_inspect` 找**标题为空**的按钮（输出里标题显示为 `-`，格式 `- id: {id} | 角色: {role} | 标题: -`）。
3. 在聊天框发：

```
先对当前这个聊天应用做 ui_inspect，找出一个标题为空（输出里标题显示为 -）的按钮，然后把它的控件 id 报给我。

拿到 id 之后，用 ui_click 点它。
```

> 注意：macOS 上 `ui_inspect` 只读 **AXTitle**；很多图标按钮在 AX 里其实**有** Accessibility Title，所以要挑**确实**空的。Windows 上 UIA 的 `CurrentName()` 对纯图标常常是空，更容易造。

**判据**：

- PASS：弹确认窗，detail 里带 `（该控件没有标题，无法判断是否属于危险操作，按保守策略需要确认）`；bot.log 出现 `confirm | … | kind: danger | ui_click | …`。
- FAIL：直接执行（消息发出 / 记录被删）。
- **负向回归**：点**有标题且不危险**的控件（例：「表情」）→ **不应**弹窗。

**变异注入复验**：把 `click_needs_confirm` 里的 `node.title.trim().is_empty() ||` 删掉（只留 `detail_is_dangerous(detail)`）→ 重跑应 **FAIL**（点纸飞机不再弹确认）。
单测侧同款变异已覆盖：`bot_ui::tests::untitled_click_target_must_be_confirmed` → 变红。

> **关联变异**：`bot_ui.rs` 里 `let untitled = node.title.trim().is_empty();` 改成 `let untitled = false;` → 单测**不会红**（`click_needs_confirm` 自己又判了一次），但**弹窗文案少了后缀**。真机判据：detail 里缺 `（该控件没有标题…）`。

**平台**：跨平台。

---

### M03 · 敏感控件闸认角色与 IsPassword

**靶点**：`src-tauri/src/bot_ui.rs::gate_sensitive`（提交 `4c698b08`）

**bug 形态**：修复前只看「角色 / 标识 / 标题里有没有『密码』这类词」。

- **macOS**：密码框角色是 `AXSecureTextField`，**标题常为空** → hay 里既没有「密码」也没有「passwd」→ **直接放行**。而 `AXSecureTextField` 里也没有「密码」二字 → 即使把角色算进 hay 也不命中。
- **Windows**：UIA 密码框在**本地化环境**里 `CurrentLocalizedControlType()` 只是普通「编辑」，`CurrentName()` 也可能为空 → **直接放行**。

后果：键盘注入对安全框与真实键盘等效——**密码被代填**。

**步骤**：

1. 目标应用准备一个**标题为空的密码框**：macOS 用「系统设置 → 通用 → 锁定屏幕 → 更改密码」或 Safari 登录框；Windows 用 Chrome 密码管理器 / 账户登录页。
2. 先 `activate_window` 把目标窗口置前（否则先被 M05 的前台闸拦掉，看不到敏感闸的文案）。
3. `ui_inspect` 拿到该控件 id。
4. 在聊天框发：

```
先对目标窗口做 ui_inspect，把那个角色是密码框或编辑框、标题为空的控件 id 找出来报给我；然后用 ui_type 往这个控件里输入字符串 变异测试 2026-10-23。
```

**判据**：

- PASS：返回 `拒绝操作：目标控件是敏感控件（AXSecureTextField），不代填密码 / 验证码类内容。`（macOS）或 `（编辑）`（Windows 本地化）；**密码框为空**、没有光标落进去、没有被聚焦（拒绝发生在 `focus_node` 之前）。
- FAIL：密码框里出现输入的字符。

**变异注入复验**：删掉 `gate_sensitive` 里的 `if node.is_password || SENSITIVE_ROLES.iter().any(|r| role.eq_ignore_ascii_case(r)) { return Some(sensitive_denied(node)); }` → 重跑应 **FAIL**（密码框被代填）。
单测侧同款变异已覆盖：`sensitive_gate_blocks_password_role_without_a_title` 与 `sensitive_gate_blocks_uia_password_flag` → 同时变红。

> **平台侧锚点**：`ui/windows.rs::collect` 里 `let is_password = unsafe { child.CurrentIsPassword() }…` 改成 `false` → Windows 上密码框放行。**macOS 上改不动 windows.rs**（`cfg(windows)` 不参与编译），这条只能去 Windows 跑。

**平台**：跨平台，但**判定材料不同**——macOS 靠**角色字符串**（`ui/macos.rs` 恒填 `is_password: false`），Windows 靠 **UIA `CurrentIsPassword`** 透传到 `UiNode.is_password`。

---

### M04 · `ui_type` 拒绝带换行的文本

**靶点**：`src-tauri/src/bot_ui.rs::gate_no_line_break`（提交 `4559a905`）

**bug 形态**：工具 schema 承诺「输入完不会自动发出去，发送是另一次 `ui_click`」，但代码从头到尾**没有中和换行**：文本原样注入。一个 `\n` 就是一次**回车键**——微信 / 飞书 / Electron 收到回车**当场发送或提交**。

危险动作确认闸拦不住：它只看 detail 里有无「发送 / 提交」等词，而 detail 是「在「XX」（XX）的「输入框」里输入：你好\n再见」——不含关键词。CRLF（`\r\n`）与单独 CR（`\r`）同样等价回车。

**步骤**：

1. 目标应用：**必须是会「发送」的应用**（微信聊天窗口 / 飞书 / 任何 Electron 应用）。记事本不会发送，**测不出当场发出**，只能测「拒绝」分支。
2. 在聊天框发：

```
在当前这个聊天应用的输入框里输入两行文本：第一行是 变异测试A，第二行是 变异测试B。直接用 ui_type 一次输入带换行的这两行。
```

**判据**：

- PASS：工具返回
  `ui_type 拒绝带换行符的文本：注入换行等于按回车键，目标应用会当场发送 / 提交，而发送必须是另一次 ui_click。请去掉换行后重试（多行内容按行拆成多次 ui_type）。`
  且**目标输入框是空的**（一个字都没进去）。
- FAIL：聊天窗口里**多出一条已发出的消息**（内容含第一行）。

**变异注入复验**：把 `gate_no_line_break` 里的 `if text.contains('\n') || text.contains('\r')` 改成 `if false` → 重跑应 **FAIL**（文字被当场发出去）。
单测侧同款变异已覆盖：`bot_ui::tests::ui_type_rejects_text_carrying_line_breaks`（覆盖 LF / CRLF / 单独 CR / 普通单行）→ 变红。

> 只保留 `'\n'`（删掉 `|| text.contains('\r')`）→ 单测对 CRLF / CR 两条断言变红，但 LF 那条仍过。

**平台**：跨平台（闸在平台分发之前）。两平台注入器都把 `\n` 当回车：macOS `CGEvent::keyboard_set_unicode_string`，Windows `SendInput` + `KEYEVENTF_UNICODE`。

---

### M05 · 注入前要求目标窗口在前台

**靶点**：`src-tauri/src/bot_ui.rs::foreground_verdict`（提交 `d61707df`）

**bug 形态**：修复前 `ui_type` 只做「聚焦控件 + 注入」两步：AXPress 聚焦的是**后台窗口**，但**键盘事件路由给前台应用**。用户正开着浏览器 / IDE / 终端 / 另一扇聊天窗，机器人对后台的微信窗口执行 `ui_type` → **这段文字被敲进用户当前正在用的应用**（地址栏、终端命令、另一扇聊天窗的输入框），可能**当场发出去**。

修复前**没有任何前台校验**，闸门结构上不存在。

**步骤**：

1. 准备**两个窗口**：目标窗口（例：记事本）+ 前台干扰窗口（例：终端 / 浏览器地址栏）。
2. **关键**：让目标窗口**处于后台**、干扰窗口在前台，且**不调 `activate_window`**。
3. 在聊天框发：

```
用 ui_type 往「记事本」窗口的输入框里输入字符串 变异测试 2026-10-23。不要先调 activate_window。
```

**判据**：

- PASS：返回
  `目标窗口不在前台：键盘注入会落进当前正在使用的应用（前台不是目标窗口）。先调 activate_window 把目标窗口置前，再输入。`
  且**前台干扰窗口里一个字符都没多**、目标输入框是空的。
- FAIL：前台窗口里出现「变异测试 2026-10-23」（终端里会被当命令，浏览器地址栏会变成 URL）。
- **正向补充**：先 `activate_window`（或手工点一下目标窗口）→ `ui_type` 应成功，返回 `已输入 15 个字符到「{title}」（控件 {id}）` + 前后截图路径。

**变异注入复验**：把 `foreground_verdict` 里的 `Ok(false) => Err(...)` 改成 `Ok(false) => Ok(())` → 重跑应 **FAIL**（文字被打进前台应用）。
单测侧同款变异已覆盖：`bot_ui::tests::injection_must_wait_for_the_target_window_to_come_to_front`（三态全测：Ok(true) / Ok(false) / Err）→ 变红。

> **fail-closed 破口**：把 `Err(e) => Err(format!("没法确认目标窗口是否在前台，已放弃注入：{e}"))` 改成 `Err(_) => Ok(())` → 判不出来也放行 → 同一条单测变红。

**平台**：跨平台，但**判定口径完全不同**：

- **macOS**（`ui/macos.rs`）：macOS **没有公开的「哪个窗口在前台」标志**，只能两级——应用 pid（`NSWorkspace.frontmostApplication().processIdentifier()`）+ 同应用 `AXFocusedWindow` 标题。**标题读不到或目标窗口标题为空 → 退化为「同应用即可」**。所以**同应用多窗口**（微信开两扇窗）时只有标题能区分。
- **Windows**（`ui/windows.rs::is_foreground`）：`GetForegroundWindow() == target hwnd`，**同一个 hwnd 精确匹配**，比 macOS 严格。
- **只有 `ui_type` 有 `bot_ui.rs` 层的前台闸**。`ui_click` 没有——但 **Windows 的 `ui_click` 坐标兜底路径有前台闸**（`ui/windows.rs`）：`目标窗口不在前台：坐标兜底点击会打在盖住它的那个窗口上，先激活目标窗口`。

> **Windows 上验前台闸时，必须让应用与目标窗口同权限级别**（都用普通权限），否则先撞 UIPI 而不是前台闸。

---

### M06 · 窗口 id 复用（TOCTOU）

**靶点**：`src-tauri/src/bot_ui.rs::window_identity_ok`（提交 `1c9ea780`）

**bug 形态**：macOS 窗口 id 是 `pid:AXWindows下标`（`ui/macos.rs` 的 `format!("{pid}:{idx}")`），**下标可复用**。窗口 A 关闭后，同应用新窗口 B 补位到同一下标 → **旧 id 指向另一扇窗**。修复前闸门只查「这个 id 还存在」，不比对标题。

具体形态：模型 `list_windows` 拿到 `501:0`（微信「张三的聊天」），用户手动关掉这扇窗、又开了一扇「李四的聊天」→ 旧 id `501:0` 现在指向李四的窗口 → 机器人的点击/输入落在**李四的窗口**里。

**步骤**：

1. 目标应用开两扇同应用窗口（例：两个备忘录，或微信两个会话）。
2. `list_windows` 记下第一扇的 id（如 `501:0`）和标题。
3. **关掉第一扇**，在同一个应用里**新开一扇**（同应用 → 下标补位）。
4. 在聊天框发：

```
用之前那个窗口 id 往窗口里点击一下标题含 文字 的控件。窗口 id 我给你：501:0。不用重新 list_windows。
```

**判据**：

- PASS：返回
  `窗口 501:0 已经不是刚才那扇窗了（标题从「A」变成「B」）：同一下标补位到了别的窗口，界面可能已经变了。请重新 list_windows 取窗口 id 与控件 id 后再操作。`
  且**动作没有发生**（B 窗口里什么都没被点）。
- FAIL：动作落到了新窗口 B 上。

**变异注入复验**：把 `window_identity_ok` 里的 `Some(c) => gated.title.trim().is_empty() || c.title == gated.title` 改成 `Some(_c) => true` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`bot_ui::tests::reused_window_id_must_not_pass_the_gate` → 变红。

> 变异 `None => false` 改成 `None => true` → 窗口已关闭也放行 → 同一条单测变红。
> **调用点锚点**：`gate_window_unchanged`；调用处 `tool_ui_click` 与 `tool_ui_type`（**都在确认弹窗之后**——弹窗期间窗口可能已经换了）。

**平台**：判定在 `bot_ui.rs` 公共层 → 跨平台成立，但**只有 macOS 的窗口 id 会补位复用**（Windows 的 id 是 `pid:hwnd`，hwnd 不复用）。**真机验必须 macOS。**

---

### M07 · 闸门审的控件 ≠ 动的控件

**靶点**：`src-tauri/src/ui/mod.rs::NodeIdentity::same_as`（提交 `1c9ea780`）

**bug 形态**：`resolve_target` 拿的是**控件树快照节点**（给闸门看），而 `press_node` / `focus_node` 是**按 id 重新走一遍树解析**——界面一重排（列表刷新、按钮挪位、虚拟滚动），同一个 id 已经指向别的控件 → **闸门审的是旧控件，动的是新控件**。

具体形态：`ui_inspect` 看到按钮 `1:0.2` 是「发送」，用户在这期间把界面刷新了 → `ui_click 1:0.2` 实际点到了「删除」。

**步骤**：

1. 目标应用里找一个**位置会变的控件**（例：会重排的列表、输入联想下拉）。
2. `ui_inspect` 拿到控件 id。
3. **在 inspect 与 click 之间让界面变**（例：让列表滚动一下 / 触发搜索联想）。
4. 在聊天框发：

```
用 ui_click 点击控件 id 1:0.2。

（在你点击之前，请先让当前的列表向下滚动一段，再点这个 id。）
```

**判据**：

- PASS：返回
  `点击失败：界面已变：控件 1:0.2 现在是「X」（Y），与刚才审过的那个（A，B）不是同一个控件——放弃这次操作`
  且目标控件未被点。
- FAIL：点到了重排后的另一个控件。

**变异注入复验**：把 `NodeIdentity::same_as` 整函数改成 `true` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`ui::tests::node_identity_detects_a_replaced_control` → 变红。

> **容差**：`POSITION_EPSILON` 当前是 `1.0` 像素。改成 `1000.0` → 同一条单测的「位置变了」断言变红（差 60px 就判为同一控件）。
> **源码守卫**：`ui::tests::platform_backends_must_verify_node_identity_before_acting` 用 `include_str!` 断言 `macos.rs` / `windows.rs` 里有 `verify_identity(node_id, &el, expect)?;`。删掉那行 → 变红。**注意**：macOS 上 `windows.rs` 不参与编译，但 `include_str!` 仍能读到源码 → **这个守卫在 macOS 上照样能跑红**。

**平台**：跨平台（判定逻辑在 `ui/mod.rs::NodeIdentity` 共享，两平台后端各自调 `verify_identity`）。

---

### M08 · 注入无全局互斥

**靶点**：`src-tauri/src/bot_ui.rs::with_inject_lock` + 静态 `INJECT_LOCK`（提交 `1c9ea780`）

**bug 形态**：修复前 `tool_ui_click` 与 `tool_ui_type` 各自 `spawn_blocking`，**两段注入没有共同的进程级锁**。键盘/鼠标注入是**全局资源**：

- A 会话：focus 完控件（还没 type）
- B 会话：插进来 type → **文本进了另一个控件**

**步骤**：

1. 同时开**两个聊天会话窗口**，各让机器人往**不同的输入框**输入**不同的可辨识文本**：

```
（会话 A）在「备忘录」窗口的输入框里输入字符串 甲甲甲甲甲甲甲甲甲甲甲。
（会话 B）在「TextEdit」窗口的输入框里输入字符串 乙乙乙乙乙乙乙乙乙乙。
```

2. **两条动作尽量同时发起**（模型通常串行发工具调用，要真并发得同时开两个会话窗口）。

**判据**：

- PASS：每个输入框里只有自己那串字符（不交错、不串框）。bot.log 里两条 `ui.type | …` 记录**不应时间交叠**。
- FAIL：出现交错字符（`甲乙甲乙…`）或串框。

**变异注入复验**：把 `with_inject_lock` 的函数体改成直接 `f()`（去掉锁）→ 重跑应 **FAIL**。
单测侧同款变异已覆盖：`bot_ui::tests::injection_sections_must_not_interleave`（4 线程 × 20 次，断言 `max_seen == 1`）→ 变红。

> **注意**：两个调用点都要套——`tool_ui_click` 与 `tool_ui_type` 各一处。只删 type 那处的锁，单测仍红（单测直接调 `with_inject_lock`），但真机行为上 click 路径就失去互斥了。
> **真机串行时永远复现不了**（锁有没有都一个样）——**这条的主证据是单测**，真机只做体验确认。

**平台**：跨平台（锁在 `bot_ui.rs` 公共层）。**进程级**锁——只在本进程内互斥。

---

### M09 · UI 总开关变更零留痕

**靶点**：`src-tauri/src/ui/mod.rs::ui_set_enabled`（提交 `1c9ea780`）

**bug 形态**：修复前 `ui_set_enabled` 只写/删 flag 文件，**不写任何审计**。事后无从查「UI 自动化是什么时候被谁打开的」「是不是我开的」——这是本仓权限面最大的一次开口。

**步骤**：

1. 找到 `bot.log`（数据目录，见 §1.0 ①）。
2. 记下行数 → 设置页 → 「允许机器人操作桌面界面」**关一次再开一次**（两次都做）。

**判据**：

- PASS：`bot.log` **新增两行**，各含 `ui_automation_toggle`，`enabled=false` / `enabled=true` 各一条，都带 ` | user=` 段。行样：
  ```
  [2026-10-23T07:12:34.567Z] INFO | ui_automation_toggle | enabled=true | user=renshi
  ```
- FAIL：没有任何新行，或只有 `enabled=true` 没 `enabled=false`（说明 `before != after` 判断被改坏）。

**变异注入复验**：删掉 `ui_set_enabled` 里整个 `if before != after { … }` 块 → 重跑应 **FAIL**（开关来回切一次，bot.log 里没有新行）。
单测侧同款变异已覆盖：`ui::tests::ui_toggle_change_must_leave_an_audit_event` → 变红。

> **关联变异**：把 kv 里的 `("user", os_user())` 删掉 → 单测**仍会绿**（它只查 `contains("ui_automation_toggle")` 与长度变长）。真机判据：那行**缺 ` | user=` 段**。

**平台**：跨平台。`os_user()` 读 `USER`（macOS）/ `USERNAME`（Windows）环境变量。

---

### M10 · flag 文件权限过宽

**靶点**：`src-tauri/src/ui/mod.rs::write_flag_file`（提交 `1c9ea780`）

**bug 形态**：flag 文件 `{data_dir}/runtime/flags/ui-enabled.flag` 存在 = UI 自动化开启。修复前用 `std::fs::write` 落成 **0644**（umask 默认）→ **同机任意进程都能创建该文件，把闸门顶开**。另一进程 `touch` 该文件 → 下一次 UI 工具调用 `ui_enabled()` 返回 true → **机器人自动获得桌面控制权**，用户完全不知情。

**步骤**：

1. 找到 `{data_dir}/runtime/flags/ui-enabled.flag`（先开关一次让它存在）。
2. `ls -l` → 判据。
3. **旧 flag 收紧路径**：手工 `chmod 644 {flag}` → 设置页把开关**关一次再开一次** → 再 `ls -l`。

**判据**：

- PASS：`ls -l` 显示 **`-rw-------`（600）**；`chmod 644` 之后再切一次开关，**仍是 600**。
- FAIL：看到 `-rw-r--r--`；或切完开关仍是 644（补刀那行没跑）。

**变异注入复验**：把 `.mode(0o600)` 改成 `.mode(0o644)` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`ui::tests::ui_flag_file_must_not_be_world_accessible` → 变红。

> **第二处变异**：删掉 `std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;` → `ui::tests::pre_existing_wide_permission_flag_gets_tightened` → 变红（它先手工造一个 0644 的旧 flag，再调 `write_flag_file`）。

**平台**：**仅 unix（macOS / Linux）**。Windows 走 `#[cfg(not(unix))]` 的 `std::fs::write` → **权限收紧不生效**（源码注释已如实声明）。两条相关单测都带 `#[cfg(unix)]`，**Windows 上不编译**。

---

### M39 · 不许操作 wmessage 自己的窗口

**靶点**：`src-tauri/src/ui/mod.rs::reject_own_window` / `without_own_windows`（**2026-10-23 实机跑 M01 时踩出来的崩溃**，见附录 D-1）

**bug 形态**：`list_windows` 把**所有** GUI 应用列出来——**包括 wmessage 自己**。模型拿到自己的窗口 id 后 `ui_inspect` 它 → AX 树遍历走进 wmessage 窗口里的 **WKWebView** → 读它的无障碍属性时，WebKit 断言「这条 API 只能在主线程调」，而控件树是在**后台 tokio 线程**读的 → **SIGTRAP，整个应用当场退出**。

这条不是「保守起见」，是**已经崩过一次**（崩溃报告 `~/Library/Logs/DiagnosticReports/wmessage-2026-10-23-211232.ips`）。

**修复**：两道闸，都在共享层 `ui/mod.rs`（两平台一次生效）——

1. `without_own_windows`：`list_windows` 清单里**剔掉**本进程自己的窗口 → 模型看不见就不会去碰；
2. `reject_own_window`：`inspect_tree` / `press_node` / `focus_node` / `activate_window` / `window_is_foreground` 全部**拒收**本进程 pid 的 id → 模型自己编 id、或拿上一轮的旧 id 来，也一样被拒。

**步骤**：

1. 设置页 → 打开「允许机器人操作桌面界面」；macOS 给足「辅助功能」权限。
2. 在聊天框发：

```
先 list_windows，把可接管窗口的清单原样贴给我。然后挑一个标题里含 Send 的按钮的窗口，inspect 它并把控件树贴出来。
```

3. 再单独试一次「碰自己的窗口」：

```
用 ui_inspect 检查窗口 id <把你机器上 wmessage 自己的窗口 id 填这里>，把控件树贴出来。
```

**判据**：

- PASS：第 2 步的清单里**没有** wmessage 自己的窗口（应用名不是 wmessage）；第 3 步返回 `不能操作 wmessage 自己的窗口——UI 自动化只用于接管外部应用`，**应用不崩**。
- FAIL：清单里出现 wmessage 自己的窗口；或第 3 步把应用带崩（现象：应用窗口消失，进程没了，`DiagnosticReports` 里多一份 `wmessage-*.ips`）。

**变异注入复验**：

- 把 `ui/mod.rs::inspect_tree` 里的 `reject_own_window(window_id)?;` 删掉 → 重跑第 3 步应 **FAIL**（应用崩）。单测侧同款变异已覆盖：`ui::tests::entry_points_must_reject_our_own_window` → 变红。
- 把 `ui/mod.rs::list_windows` 里的 `Ok(without_own_windows(wins))` 改成 `Ok(wins)` → 第 2 步应 **FAIL**（清单里冒出自己）。单测侧：`entry_points_must_reject_our_own_window` → 变红。
- 把 `reject_own_window` 里的 `pid_of(id) == Some(own_pid())` 改成恒假 → `ui::tests::own_window_must_not_be_automated` → 变红。

**平台**：跨平台（规则在共享层；macOS 上必崩，Windows 上是同类风险——UIA 读自己的 WebView2 树同样没意义）。

> **既有缺陷，不是本批回归**：UI 六件套是 10-21 的 `d2674e33` 引入的，`list_windows` 从那时起就没排除过自己。**今天这批卡片把它踩出来了**——这正是这套卡片存在的意义。

---

## 四、T1 档：静默失效与状态谎报

### M11 · OCR 解码炸弹面

**靶点**：`src-tauri/src/ocr.rs::read_image_file` / `check_pixel_budget` / `image_dimensions`（提交 `e07b7a0d`）

**bug 形态**：原先 `std::fs::read` 无上限读图，Windows 侧解码后 `w×h×3` 整幅驻留内存；macOS 路径同样无闸。几 MB 的压缩 PNG 能撑爆内存。

改后：读文件前加字节上限（**32 MB**），解码前只读文件头拿宽高、对总像素设上限（**20 MP**），超限在解码前就拒——PDF 转图那条路径也覆盖。

**步骤**：

1. 造一张**超过 20 MP** 的 PNG（例：8000×3000 = 24 MP）。放桌面。
2. 在聊天框发：

```
用 ocr_image 工具识别桌面上的大图.png，把识别结果原样贴给我。
```

3. 再造一个**超过 32 MB** 的文件（如把 PNG 复制多份改名），同样调用。

**判据**：

- PASS：工具返回 `图片像素数超过上限（8000×3000 = 24000000 > 20000000）`（字节超限时是 `图片文件超过大小上限（… > 33554432 字节）`），**进程内存不暴涨**。
- FAIL：开始解码 → 内存飙升 / 应用卡死 / 被系统杀掉。

**变异注入复验**：把 `check_pixel_budget` 里的 `if pixels > max_pixels` 改成 `if false` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`ocr::tests::check_pixel_budget_rejects_oversized` 与 `recognize_rejects_oversized_pixels_before_decode` → 变红。

> **另一处变异**：把 `read_image_file` 里的字节闸去掉 → `ocr::tests::read_image_file_rejects_over_limit` → 变红。

**平台**：跨平台（字节闸与像素闸都在 `ocr.rs` 公共层，位于平台分发之前）。

---

### M12 · 必应大图先查长度再流式读

**靶点**：`src-tauri/src/bing.rs`（提交 `992d7c5c`）

**bug 形态**：下载大图时先把整个响应体 `bytes().await` 进内存，才检查是否超过 8 MB。一个超大响应能把这块内存吃满，**闸下得太晚**。改后：先查 `content_length` 超限直接拒；再用 `bytes_stream()` 流式读，边读边计数，超限即中止。

**步骤**：这条**真机上很难自然造出超大响应**（要自建一个 > 8 MB 的图片服务）。

1. 自建一个返回 > 8 MB 体的 HTTP 服务（或指向一个已知大图 URL）。
2. 让机器人触发必应每日壁纸拉取。

**判据**：

- PASS：拉取失败并报 `壁纸图片超过大小上限（… > 8388608）`，**内存不暴涨**。
- FAIL：内存先涨到响应体大小才报错。

**变异注入复验**：把流式计数的那段改成「先 `bytes().await` 再判断」→ 重跑应 **FAIL**（回到旧行为）。
单测侧同款变异已覆盖：`bing::tests::over_limit_is_strictly_greater` / `push_capped_rejects_chunk_over_limit` / `push_capped_accepts_exact_limit` → 变红。

**平台**：跨平台。

---

### M13 · 图谱铭牌不拼 `innerHTML`

**靶点**：`src/components/GraphPage/GraphCanvas.tsx`（提交 `c23bc5d7`）

**bug 形态**：铭牌用 `off.innerHTML = …` 拼任务标题 → 标题里的尖括号被当标记解析，标题变成真的元素。任务标题是用户输入的 → 存储型 XSS 的一条跳板。

**步骤**：

1. 建一个标题含尖括号的任务（例：`测试 <img src=x onerror=alert(1)>`）。
2. 打开图谱页 → 找到该任务的铭牌。

**判据**：

- PASS：铭牌里**显示字面量** `<img src=x onerror=alert(1)>`（原样文本，不是元素）；没有弹窗；DOM 里没有多出来的 `img` 节点。
- FAIL：尖括号被解析成元素 / 弹窗 / 铭牌文字丢失。

**变异注入复验**：把 `GraphCanvas.tsx` 里铭牌那段改回 `innerHTML` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`src/ui/domSinkGuard.test.ts` 里的 `innerHTML` 禁用守卫 → 变红。

**平台**：跨平台（纯前端）。

---

### M14 · 厂商条目没配 key 时不借全局主 key

**靶点**：`src-tauri/src/bot/config/keyring.rs::llm_key_source` / `probe_auth`；`config/types.rs::resolve_model_override`；`config/commands.rs::bot_test_connection`（提交 `48e96dad`）

**bug 形态**：厂商条目（带 `vendor` 名）没配自己的 key 时，三处回落全局主 key：

- **请求侧** `read_llm_key`：落到 `read_api_key()`，而**主 key 会带着该厂商的 base_url 出门**（base_url 是用户填的，可能指向任何第三方）→ 凭据泄漏。
- **每卡覆盖** `resolve_model_override`：同样带主 key 打该条目自己的 base_url。
- **探活** `bot_test_connection`：对着用户填的 base_url 拿**全局主 key**探活；对方回个 2xx 就把厂商写进 `verified_vendors` → 此后该路径稳定带主 key 打过去。

触发输入：添加一个带 `vendor` 名的模型条目，base_url 指向第三方，**但不给该厂商配 key**；全局主 key 已存在。

**步骤**：

1. **前置**：自建一个能打请求日志的 mock 端点（例 `http://127.0.0.1:8099/v1`）；设置页 → 厂商中心 → 新增厂商，填这个 base_url，**不填**该厂商的 API Key；全局主 key 已配置。
2. 点该模型行右侧插头（`aria-label="测试连接"`）。
3. 查 mock 端点日志。
4. 在聊天区 🧠 下拉选该厂商模型发一句。

**判据**：

- PASS：探活返回
  `{"ok":false,"status":200,"error":"厂商「X」还没配 API Key：本次只探了端点连通性（未带鉴权），不算连接测试通过。请先在厂商页填该厂商的 Key。"}`
  且**mock 端点日志里看不到 `Authorization` 头**；厂商页左栏**绿点不亮**、模型行插头灰、🧠 下拉里该厂商模型不出现；发消息时报 `机器人 API 未配置：请到设置页「机器人设置」填写 API Key`。
- FAIL：探活 `ok:true` / 厂商转正 / **mock 端点收到了带 `Authorization: Bearer <主key>` 的请求**。

**变异注入复验**：把 `llm_key_source` 里的 `Some(_) => LlmKeySource::VendorKeyMissing` 改成 `Some(_) => LlmKeySource::GlobalKey` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`keyring::vendor_key_tests::vendor_entries_never_borrow_the_global_key` → 变红。

> **探活侧注入**：把 `probe_auth` 里的 `None => ProbeAuth::Anonymous` 改成 `None => ProbeAuth::WithKey(global_key.to_string())` → `probe_never_sends_the_global_key_to_a_vendor_endpoint` → 变红。
> **每卡侧注入**：删掉 `resolve_model_override` 里的 `if k.trim().is_empty() { return Err(CommandError::ApiKeyMissing); }` → 同上。

**平台**：跨平台一致（判定是纯函数 + keyring 抽象）。

> **文档漂移（顺手登记，非本批 bug）**：`bot_test_connection` 的文档注释仍写着「没配过才回落全局主 key」——与修完的行为相反。
> **注意**：改厂商 key 或改 base_url 会把该厂商移出 `verifiedVendors`，测完要重新测连接。

---

### M15 · 写盘前安置盘上残留的旧明文密钥

**靶点**：`src-tauri/src/bot/config/io.rs::ensure_legacy_plaintext_backed`（提交 `5c5b6e93`）· **破坏性卡片**

**bug 形态**：`bot-config.json` 里的 `apiKey` / `tavilyKey` / `braveKey` 是 `#[serde(skip_serializing)]` → **每一次写盘都会把盘上残留的旧明文抹掉**。启动迁移在 keyring 故障时按「保留明文下次再试」→ 此后**任何一次无关写盘**都会把这份唯一密钥抹掉，而 keyring 里从来没有过它 → **密钥永久丢失**。

触发输入：盘上 `bot-config.json` 含 `"apiKey": "sk-xxx"`（旧版本写出来的文件），且凭据存储此刻读不到/写不进。

**步骤**：

1. **备份**：`cp <data_dir>/bot-config.json /tmp/bot-config.json.bak`。
2. 手工在 `bot-config.json` 里加 `"apiKey": "sk-test-plaintext"`（模拟旧版本文件）；确认钥匙串里没有这一条（macOS：`security find-generic-password -s wmessage.bot.v1 -a api-key` 应查不到）。
3. **制造 keyring 故障**：macOS `security lock-keychain`（Windows：制造凭据管理器访问失败）。
4. 触发写盘：设置页改任意一项并保存（走 `bot_set_config`）。

**判据**：

- PASS：保存报错，文案是
  `系统凭据存储访问失败：系统凭据存储暂时不可用，bot-config.json 里还没迁进去的旧密钥（api-key）安置不下；本次保存已中止（否则这份密钥会被永久抹掉）。请先让系统凭据存储可用（解锁钥匙串 / 确认应用有钥匙串访问权限）后重试。`
  且 `bot-config.json` 里 `"sk-test-plaintext"` **原样还在**。
- FAIL：保存成功，且 `bot-config.json` 里的 `"sk-test-plaintext"` **消失了**（密钥被抹掉）。

**变异注入复验**：把 `write_bot_config_file_locked_with` 里的 `ensure_legacy_plaintext_backed(dir, read_slot, write_slot)?;` 删掉 → 重跑应 **FAIL**（盘上明文被抹）。
单测侧同款变异已覆盖：`config::io::tests::write_is_blocked_when_the_credential_store_cannot_take_the_disk_plaintext` → 变红（它断言盘上文件原样不动）。

**恢复**：`security unlock-keychain`；`cp /tmp/bot-config.json.bak <data_dir>/bot-config.json`。

> **别在有真实 key 的机器上做这张卡。** 「修复后明文从盘上消失并进钥匙串」是**正确行为**。
> **审计事件 `config.plaintext_key_stripped` 只在「授权弹窗『始终允许该目录』」路径（`add_allowed_dir`）写**，`bot_set_config` 路径不写——**别把它当这张卡的通用判据**。

**平台**：macOS = Keychain（注入手段 `security lock-keychain`）；Windows = 凭据管理器（**只能验「条目在不在 / 保存是否被中止」**，看不到值）；Linux = 降级明文文件（`bot-api-key.txt` 等，0600）。

---

### M16 · 终稿自检的侧调请求也过 vendor 未验证门

**靶点**：`src-tauri/src/bot_model_loop.rs::decide_selfcheck`（提交 `4e8f5581`）

**bug 形态**：主请求在装配前有 vendor 门（`ensure_vendor_verified` → 响亮拦截），但**终稿自检的侧调请求绕过了它** → 主请求被拦、侧调请求照发 → **一次真实带 key 的请求打往未验证的 base_url**。

触发输入：`bot-config.json` 的 `selfcheckModelId` 指向一个带 `vendor` 且不在 `verifiedVendors` 里的模型库条目。

**步骤**：

1. **前置（较硬）**：`selfcheckModelId` **没有前端设置入口**——只能手改 `<data_dir>/bot-config.json`（字段名 camelCase `selfcheckModelId`，值 = 模型库条目 id）或走 `bot_set_config` 的同名参数。
2. 备份 `bot-config.json` → 改 `selfcheckModelId` → 重启应用。
3. 发一条会触发多轮 + 工具调用 + 长答复的消息（终稿自检的触发条件是硬编码的：`final_text` ≥ 80 字 或 `function_calls` ≥ 3 次，且 `finalSelfcheckRoundGap`（默认 8 轮）达标）。
4. 查 `bot.log` 有无 `final_selfcheck.vendor_not_verified`；查 mock 端点有无侧调请求。

**判据**：

- PASS：bot.log 出现
  ```
  [<ts>] WARN | final_selfcheck.vendor_not_verified | entry=<条目 id> | vendor=<厂商名> | err=厂商「X」未通过连接测试，已拦截请求：请到 设置→模型厂商 完成「测试连接」后再试；或在聊天区 🧠 下拉切换到已验证的模型
  ```
  且**第三方 base_url 侧没有收到那次侧调请求**；**主流程不阻断**（用户看不出异常）。
- FAIL：bot.log 无该行，且 mock 端点收到一条带 key 的侧调请求。

**变异注入复验**：把 `decide_selfcheck` 里的 vendor 门整块删掉（`Ok(r) => SelfcheckDecision::Use(r)`）→ 重跑应 **FAIL**。
单测侧同款变异已覆盖：`bot_model_loop::selfcheck_vendor_gate_tests::selfcheck_must_not_use_a_vendor_that_failed_the_connection_test` → 变红。

**平台**：跨平台一致（纯判定 + 同一个 `ensure_vendor_verified`）。

---

### M17 · 空 token 文件改走原子覆写

**靶点**：`src-tauri/src/api_auth.rs::load_or_create_token_at`（提交 `afe7f47b`）· **破坏性卡片**

**bug 形态**：`runtime/flags/api-token.txt` 存在但**内容为空**（写 token 的进程崩在 create 与 write 之间，或人工清空）→ `load_or_create_token` 走「生成路径」→ `create_token_file_atomic` 用 `create_new` → 空文件已占着路径 → **必然撞 `AlreadyExists`** → 返回 `false` → 50 次重试读到的还是空 → 报错。`api_start` 因此**恒失败**，且注释里「下次调用自愈」不成立。

**步骤**：

1. **备份**：`cp <data_dir>/runtime/flags/api-token.txt /tmp/api-token.bak`，并记下 token 文本。
2. 造坏态：`: > <data_dir>/runtime/flags/api-token.txt`（truncate 到 0 字节）。
3. 设置页 → 「开启外部机器人 API 接口」→ 点 `已关闭` 按钮。

**判据**：

- PASS：按钮文案由 `已关闭` 变 `已开启`，其下显示 `运行中 · http://127.0.0.1:{port}`，再下方「访问令牌（Bearer Token）」输入框出现 32 位 token；磁盘上文件非空（32 位小写 hex，uuid v4 simple 无连字符）；**同目录不得留 `*.tmp` 残渣**（tmp 里是明文 token）；`curl -H "Authorization: Bearer <token>" http://127.0.0.1:<port>/…` 返 200。
- FAIL：设置页红字 `内部错误：token 文件被并发创建但内容一直为空（读超时，下次调用自愈）`；文件仍是 0 字节。

**变异注入复验**：把 `load_or_create_token_at` 里的 `return repair_empty_token_file(path);` 删掉（让它继续落到首建路径）→ 重跑应 **FAIL**。
单测侧同款变异已覆盖：`api_auth::tests::load_or_create_token_at_repairs_empty_file` → 变红。

> **辅助变异**：把 `repair_empty_token_file` 里的 `write_token_file(path, &token)?` 改成 `std::fs::write(path, &token)?` → `load_or_create_token_at_repair_leaves_no_tmp` → 变红。

**恢复**：若要保留原 token：`cp /tmp/api-token.bak <data_dir>/runtime/flags/api-token.txt`，然后设置页关一次再开一次（内存里的 token 来自磁盘，不重启会与活服务不一致）。

**平台**：跨平台（tmp + `rename` 覆写，Windows 上 `std::fs::rename` 走 `MoveFileEx`+`REPLACE_EXISTING`，同样是替换目标）。

---

### M18 · 设置页改叠层 · 在途流式不断

**靶点**：`src/App.tsx` 的设置壳分支（提交 `7520669b`）

> **与 `docs/realmachine-regression-test.md` 的 S33 交叉**：S33 验「行为对不对」，本卡只补**变异注入锚点**与更细的判据。

**bug 形态**：`function App()` 里原先是**提前 return**（`if (view === "settings") { return (…) }`）→ 整棵主树被替换 → `ChatPanel` 随之卸载 → 它的 11 路 `bot-*` 订阅一起注销 → 回复进行中切设置，那段时间后端广播的增量**没有任何接收者**（广播即焚、无游标无回放）→ 在途回复的**流式过程**永久丢失。最终正文后端会落库、回来还看得到，丢的正是过程。

**步骤**：

1. 在聊天里发一条会跑工具的长回复（提示词见下），让它流十几秒。
2. **回复进行中**点左下角「设置」。
3. 在设置页停留 ≥ 10s（期间可改一项设置）。
4. 点「返回」。

```
请在当前项目里读一下 src/lib/diff.ts 这个文件，然后在它的文件末尾追加一段注释，说明这个模块负责 unified diff 的着色与分段；追加完成后，再逐行解释一遍你刚才写入的内容。
```

**判据**：

- PASS：回到聊天页后**流式气泡仍在增长**（不是停在离开那一刻的半截）；设置页盖住期间主树元素**点不到**（`inert` 生效）；⌘K 只弹**一个**命令面板；设置页里触发的错误弹的是**应用内**错误弹窗，不是原生 `alert`。
- FAIL：流式停在离开设置页那一刻的半截；主树元素能被点到；命令面板弹两个；错误弹成原生 alert。

**变异注入复验**：把 `src/App.tsx` 里的设置壳整块改回提前 return → 重跑应 **FAIL**（流式判据挂）。
单测侧同款变异已覆盖：`src/App.test.tsx` 的 `在途回复切设置：流式订阅仍在（设置页叠层，主树不卸载）` → 变红。

> **第二处变异**：把 `src/components/ConfirmMap/ConfirmMap.tsx` 的 `<Modal … portal>` 删掉 `portal` → 全局确认弹窗留在 inert 子树里「看得见却按不动」→ `设置页盖住主树时：全局弹窗逃出 inert 子树（还能点）` → 变红。

**平台**：跨平台（纯前端，`inert` 属性在 macOS WebKit 与 Windows WebView2 都受支持）。

---

### M19 · 在途流式可回灌

**靶点**：后端 `src-tauri/src/bot/stream_log.rs`（`record` / `emit_recorded`）；前端 `src/components/ChatPanel/ChatPanel.tsx::resumeInflight`、`src/components/ChatPanel/streamState.ts::noteSeq`（提交 `323d4daf` `bafb57e9`）

> **与 S34 交叉**：S34 验行为，本卡补变异锚点。
> **⚠️ 一条必须先说清的前提**：`S33` 那张卡的「重启应用」口径**不成立**。回放缓冲是**进程内存**（`static SESSION_LOGS: OnceLock<Mutex<HashMap<String, SessionLog>>>`），完整退出应用（⌘Q / 托盘退出）会把缓冲和在途登记一起带走，**重启后必然接不回**。这是设计边界，不是回归。**要验回灌，必须是「客户端不在场、宿主进程仍在场」＝前端整页重载**。

**bug 形态**：后端广播是「即焚」的——事件只往窗口广播一次，客户端不在场时那段时间**没有任何接收者**，也没有游标与回放 → 在途回复的流式过程**永久**没了。

**修复后的接口（判据用）**：

| 通道 | 形状 |
|---|---|
| Tauri command | `bot_stream_replay`（参数 `sessionId` / `afterSeq`）、`bot_inflight_list`（无参） |
| `bot_stream_replay` 返回 | `{ events: [{seq, event, payload}], lastSeq, truncated, inflight }` |
| `bot_inflight_list` 返回 | `[{ sessionId, startedAt, lastSeq, eventCount }]` |
| 载荷新增字段 | `seq`（会话级单调递增，**进程内跨轮次连续、不重置**） |

**步骤**（**开发模式** `npm run tauri dev`）：

1. 聊天里发一条会跑工具的长回复（提示词见下），让它流十几秒。
2. **回复进行中**，devtools 控制台执行 `location.reload()`。
3. 页面回来后看聊天区。

```
请先读一下 src/components/ChatPanel/streamState.ts，看懂它在做什么；然后在 src/components/ChatPanel/constants.ts 的末尾追加一行注释，写明这个文件存放前端共享常量；改完后再把 streamState.ts 的三个核心导出函数（metaOf / noteSeq / fetchReplay）依次解释一遍。
```

**判据**：

- PASS：气泡里能看到重载前**已流出的半截正文**（不是从零开始），并且**继续增长**；回合秒表从本轮真实起点起算；补齐与实时重叠处正文**不重复叠加**。
- FAIL：气泡空白 / 只剩最终正文（补齐没生效）；或正文叠成两半。

**后端侧的独立验法**（dev 模式）：devtools → Tauri Events 面板看 `bot-chat-delta` 的 payload → 应有 `seq` 字段且单调递增；devtools console 执行 `await window.__TAURI__.core.invoke("bot_inflight_list")` → 回复期间非空、结束后清空。

**变异注入复验**：

- 前端：把 `resumeInflight` 函数体第一行后插入 `return;`（掐断回灌）→ 重跑应 **FAIL**。单测侧同款变异已覆盖：`src/components/ChatPanel.test.tsx` 的「在途回合」4 例 → 全红。
- 前端第二处：把 `streamState.ts::noteSeq` 改成永远放行 → 正文叠成两半。
- 后端：把 `stream_log.rs::record` 里的 `let seq = entry.next_seq;` 之前插入 `return None;` → 序号不再分配、事件不再进缓冲 → 重跑应 **FAIL**。单测侧同款变异已覆盖：`bot::stream_log` 的 7 例 → 红。

**平台**：跨平台。**数据不落盘**（纯进程内存）→ **重启应用必然丢**（见上面的前提）。

> **只能靠代码审计/单测的项**：缓冲条数/字节上限（`MAX_EVENTS_PER_SESSION = 20_000`、`MAX_BYTES_PER_SESSION = 8 MiB`）、收尾保留期（`RETAIN_AFTER_FINISH_MS = 120_000`）、跨轮次序号连续、`truncated` 报截断。
> **设计说明**：`bot-usage-delta`（累计型）与 `bot-file-changed`（走 `app.emit` 而非 `emit_recorded`）**不进缓冲**——后者没有 `seq` 字段。

---

### M20 · 输入区草稿落到组件外

**靶点**：`src/components/ChatPanel/chatDrafts.ts::writeChatDrafts`（提交 `01ba711b`）

**bug 形态**：草稿只存在 `ChatPanel` 的 `useState` 里 → 设置页在 `App.tsx` 里是提前 return（整棵主树被替换）→ 聊天面板随之卸载 → 草稿归零 → **切一次设置再回来，输入框全空**。同一条链上还有两个漏：附件 `filesBySession` 与任务引用 `refsBySession` 是另外两份 per-session Map，`refsBySession` 在删会话时原先漏清。

**修复后的判据通道**：localStorage 键名 **`wmessage-chat-drafts`**，结构 `{ "<sessionId>": { text, files, refs } }`。空草稿不落盘；发出去即清空；删会话连同草稿一并清掉。

**步骤**：

1. 在聊天输入框打一段**不发送**的中文。
2. 点左下角「设置」→ 停留几秒 → 点「返回」。
3. devtools console 执行 `localStorage.getItem("wmessage-chat-drafts")`。

```
测试草稿：这段话不要发出去，我只想确认切设置之后输入框里的字还在不在。
```

**判据**：

- PASS：返回后输入框里**那段中文还在**；`localStorage` 里能查到 `wmessage-chat-drafts` 且该 sessionId 的 `text` 非空；把草稿发出去后该 sessionId 的条目**消失**。
- FAIL：返回后输入框空了；或 `localStorage` 里没有该键。

**变异注入复验**：把 `writeChatDrafts` 函数体第一行后插入 `return;` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`src/App.test.tsx` 的 `聊天草稿跨「切设置 → 返回」存活` 与 `src/components/ChatPanel.test.tsx` 的 `草稿跨挂载存活` → 变红。

> **第二处变异**：把 `readChatDrafts` 改成直接 `return new Map();` → 同上。
> **只测不验的项**：脏数据归一（坏 JSON / 形状不对）、删会话清草稿。

**平台**：跨平台。数据落点是 **webview 的 origin 级 localStorage**，不是 `wmessage.db`——**卡片里别写死路径**，用 `localStorage.getItem(...)` 当判据通道。

---

### M21 · 子 agent 预算治理

**靶点**：`src-tauri/src/db/subagents.rs::SubagentBudget::clamped`；`src-tauri/src/bot/dispatch.rs`（软墙钟）；`src-tauri/src/bot_model_loop.rs`（墙钟熔断）；`src-tauri/src/bot_orchestrator.rs::classify_outcome` / `spawn_receipt_text`（提交 `968f26be`）

**bug 形态**：四个独立缺陷：

- **(a) 墙钟两条路两个闸。** 配置键 `subagent.max_wall_secs` 走 `clamp_subagent_max_wall_secs`（30..=3600），但 LLM 显式给 `budget.maxWallSeconds` 时只钳「≥1 秒」→ 模型给 `999999` 原样生效 → 墙钟硬顶由模型说了算。
- **(b) spawn 回执不回显生效预算。** 模型给了 999999，回执只有「已派发子 agent（非阻塞）：subagentId=sa_xxx status=queued。」→ 模型不知道自己给了一小时，会按错的口径安排后续动作。
- **(c) 无软墙钟收尾窗。** 墙钟快耗尽时 dispatch 层不拦工具调用 → 模型继续无限探索 → 外层 `tokio::time::timeout` 掐掉整条 future → **正在跑的工具、已经说出口的正文、已落盘的产物全悬空** → `result_json` 写 NULL，父 agent 只能从零重烧。
- **(d) 墙钟熔断标记判反。** `WALL_FUSE_MARKER = "⏹ 已熔断（墙钟上限）"` **包含** `FUSE_MARKER = "⏹ 已熔断"` → 先判通用标记 → 墙钟触顶被认成工具调用熔断（`error = "max_tool_calls"`），真实原因 `max_wall_seconds` 被丢掉。

**步骤**：

1. 前置：已配厂商；bot 已启用；设置页「Agent 运行参数」→ `subagent.max_wall_secs` 可调（范围 30..=3600，默认 600）。
2. 派一个子 agent，给它超界预算：

```
派一个子 agent 去做这件事：调研本机文件清单并汇总。给它的 budget 里 maxWallSeconds 设成 999999。
```

3. 再派一个短墙钟的：

```
派一个子 agent 去做这件事：把桌面上的「变异测试-大文件.txt」完整读出来并逐行总结。给它的墙钟预算设成 30 秒。
```

**判据**：

- PASS：第 2 步的回执是
  `已派发子 agent（非阻塞）：subagentId={id} status={status}。生效预算：墙钟 3600s / 轮数 30 / 工具调用 100。`
  （**被钳到上界 3600**，不是 999999）；`subagents` 表的 `budget_json` = `{"maxWallSeconds":3600,…}`。
- FAIL：回执出现 `生效预算：墙钟 999999s` → 钳制没生效。
- 第 3 步 PASS：bot.log 出现 `subagent_wall_soft_stop`（WARN）；子 agent 行徽标 `预算耗尽`；`subagents` 表 `status='budget_exceeded'`、`error='max_wall_seconds'`、`result_json` 含非空 `summary`。
- FAIL：`error='max_tool_calls'`（判反了）；或 `result_json` 为 NULL / `summary` 为空；或 bot.log 只有 `⏹ 墙钟预算触达，执行被强制停止（部分产物已保留）` 而**无** `fuse_wall`。

**变异注入复验**：

- 把 `SubagentBudget::clamped` 里的 `self.max_wall_seconds = crate::bot::params::clamp_subagent_max_wall_secs(self.max_wall_seconds);` 改成 `self.max_wall_seconds = self.max_wall_seconds.max(1);` → 重跑应 **FAIL**。单测侧同款变异已覆盖：`bot_orchestrator::orchestrator_tests::budget_wall_clock_is_capped_at_the_same_bound_as_config` → 变红。
- 把 `classify_outcome` 里 `WALL_FUSE_MARKER` 判块挪到 `FUSE_MARKER` 判块之后 → 重跑应 **FAIL**（`error` 变成 `max_tool_calls`）。单测侧：`classify_outcome_treats_wall_fuse_as_budget_exceeded_and_keeps_output` → 变红。
- 把 `spawn_receipt_text` 里的 `budget_note` 改成 `String::new()` → 回执缺「生效预算」→ **FAIL**。单测侧：断言 `fresh.contains("生效预算")` → 变红。

> **注意**：`subagent_wall_soft_stop` 与 `fuse_wall` 是**下划线**命名，不符合「点号式」事件名惯例（`subagent.machine_check_passed` 那种）——按真实字符串核对，别按惯例猜。
> **无单测锚点的两处**：墙钟熔断「带正文退出」在模型循环里的位置（`bot_model_loop.rs` 里没有任何 `wall_budget` 的测试）、`fused_on_wall` 排除机检——**只能靠真机或代码审计**。

**平台**：跨平台（纯逻辑，无平台 API）。

---

### M22 · 子 agent 收尾契约三段修复 + digest 补渲染

**靶点**：`src-tauri/src/bot_orchestrator.rs`（`repair_json_candidate` / `parse_json_candidate` / `repair_final_json_once` / `build_subagent_digest` / `machine_criterion_shape_error`）；`src-tauri/src/prompts/subagent.rs`（提交 `ef3f3baa`）

**bug 形态**：

- **(a) 收尾 JSON 格式死判失败。** 收尾 JSON **带尾逗号**或**被 max_tokens 截断** → 解析不出 → 落 `status='failed'`、`error='result_json_unparseable'`。真机上确实发生过：13780 字节的政策调研稿确实落盘，产物齐、机检全过，行状态却是 failed。
- **(b) 衔接把真相藏着（digest 缺渲染）。** 失败 / 超预算行父 agent 只看到「失败」二字，判断不了该不该带残留重试 → **从零重烧**。
- **(c) 机器可判定条目派发期不验。** 模型写 `file_exists:调研.md 已完成` → 服务端把整句当成路径 → 文件当然不存在 → 假阴性，要等十几分钟才暴露。
- **(d) 收尾 schema 太重。** 必填键含 `acceptance_check` / `artifacts` / `blockers` / `next_actions` / `confidence` → 模型漏一个整份报废。

**判据用的原文**：

| 通道 | 内容 |
|---|---|
| 审计事件 | `subagent.machine_check_passed`（Info，kv `subagent_id`/`total`）、`subagent.machine_check_failed`（Warn，kv `subagent_id`/`failed`/`total`）、`subagent.parse_failure_recovered`（Warn） |
| bot.log 自由文本 | `subagent_final_json_repaired \| 收尾 JSON 补答成功（{} 字符）`、`subagent_final_json_repair_failed \| 补答后仍不可解析`、`subagent_digest \| session={sid} absorbed={}` |
| digest 渲染（父 agent 上下文可见） | `   残留产出：{}`（失败 / 超预算行，`DIGEST_MAX_RESIDUAL_CHARS = 300`）、`   ⚠️ 机检未过 {} 项：{}`（成功行；只认 `machine: true` **且** `met: false` 的条目） |
| 派发期拒绝文案 | `file_exists: 后面必须给相对路径。正确写法：file_exists:report.md` / `file_exists: 后面只能跟相对路径，不能带空格或说明文字（收到「{rest}」）。正确写法：file_exists:report.md；说明另起一条自然语言验收标准` / `目标不能为空。正确写法：objective="调研 X，产出 Y，含来源"` |

**步骤**：

```
派一个子 agent 去做这件事：调研 5 家供应商的报价，产出 report.md，验收标准里写 file_exists:report.md。给它的墙钟预算设成 30 秒。
```

```
派一个子 agent 去做这件事：随便写一段总结。验收标准里必须有 file_exists:report.md 和 file_nonempty:output/数据.xlsx。
```

```
派一个子 agent 去做这件事：调研三家供应商。验收标准第一条写 file_exists:调研.md 已完成。
```

```
派一个子 agent 去做这件事：把桌面上的「长文.txt」读出来，写一份不少于 3000 字的详细总结报告，并在最后按任务包装末尾的 schema 输出收尾 JSON。
```

**判据**：

- 第 1 条 PASS：父 agent 下一轮的消息里出现 `残留产出：` 一行，内容非空；子 agent 行状态 `failed`（带残留，不是干净失败）。
- 第 2 条 PASS：父 agent 消息里出现 `⚠️ 机检未过 2 项：` 后跟条目名；bot.log 有 `subagent.machine_check_failed`（`failed=2`）。
- 第 3 条 PASS：**派发当场被拒**，`spawn_subagent` 工具返回错误文案含 `file_exists:report.md`；**没有**新建行。
- 第 4 条 PASS：行状态 `succeeded`（不是 failed），`result_json.summary` 非空；bot.log 出现 `subagent_final_json_repaired | 收尾 JSON 补答成功（N 字符）`。
- FAIL 对照：父 agent 只看到「成功」/「失败」而无「残留产出」「机检未过」；或派发当场没被拒（十几分钟后才报「产物不存在」）；或状态是 `failed` + `error='result_json_unparseable'`。

**变异注入复验**：

- 把 `repair_json_candidate` 里的 `let no_commas = strip_trailing_commas(trimmed);` 改成 `let no_commas = trimmed.to_string();` → 重跑应 **FAIL**。单测侧：`orchestrator_tests::parse_final_json_repairs_trailing_comma_and_truncation` → 变红。
- 把 `parse_json_candidate` 里的 `if require_semantic && v.get("status").is_none() && v.get("checks").is_none() { return None; }` 删掉 → `parse_final_json_rejects_truncated_fragment_without_status` → 变红。
- 把 `build_subagent_digest` 里的「残留产出」整块删掉 → `digest_shows_residual_output_for_failed_rows` → 变红；把「机检未过」整块删掉 → `digest_shows_machine_unmet_criteria` → 变红。
- 把 `machine_criterion_shape_error` 里「带空格」的整块删掉 → `spawn_rejects_machine_criterion_with_trailing_prose` → 变红。
- 把 `prompts/subagent.rs` 的 `RESULT_SCHEMA_HINT` 加回 `"next_actions": []` 和 `"confidence": 0.0` → `prompts::subagent::subagent_prompt_tests::result_schema_hint_is_lightweight` → 变红。

> **无单测锚点**：`repair_final_json_once`（第 3 段补答）需要真实 LLM，**只能真机**。

**平台**：跨平台（`serde_json` 解析 + 纯字符串渲染 + 文件 `std::fs::metadata` 核验）。

---

### M23 · 切技能后活动快照取活着的 run

**靶点**：`src-tauri/src/bot_skills/state.rs::pick_active_run`；`src-tauri/src/bot_skills/runtime.rs::take_over_switched_runs`；`src-tauri/src/bot_skills/scheduler.rs`（提交 `a55bb74c`）

**bug 形态**：同一个会话里，先 `use_skill` 启动技能 A，再 `use_skill` 切到技能 B。修复前 `start_skill` 把旧技能 A 标成 `Completed` **但留在 `SKILL_RUNS` 表里**。于是同会话同时有：A（终态，留在表里）、B（`Running`）。

`active_skill_run_for` 用 `guard.values().find(…)` 取 **HashMap 迭代序第一条** → 很可能取到 A → 主循环 `advance_skill` 判 `AdvanceAction::Finish` → `skill_finish` 把**新启的技能 B 也标成完成并直接收尾** → **模型再没被叫醒，表现为「技能刚加载就断，本轮零产出」**。取哪条取决于 HashMap 迭代序 → **时好时坏**。

**步骤**：

1. 前置：已安装 ≥2 个技能。
2. 在聊天框发：`用 <技能A> 帮我做一下 <任务A>`
3. **在技能 A 还没跑完时**（或紧接着）发：`用 <技能B> 帮我做一下 <任务B>`

```
用 minimax-docx 帮我修订一下桌面上的 汇报.docx。
```

```
用 pptx-design 帮我做一个关于项目进展的 PPT。
```

（换成实际技能名。**关键是要在同一会话里连发两条，分别点名不同技能**。）

**判据**：

- PASS：技能 B 的内容**真的跑起来了**（有工具调用、有正文产出），不是「加载完就断」；bot.log 出现 `skill_completed | name: <技能A> | 切换技能结束 | steps: N`；若 B 是 DSL 技能，还会出现 `skill_dsl_run_replaced | name: <技能A> | active: <技能B>`，且**没有** `skill_dsl_finish_signal | name: <技能B>`。
- FAIL：本轮零产出；或 bot.log 出现 `skill_dsl_finish_signal | name: <技能B>`（接管方被标完成）；或技能 B 在设置页显示 `完成` 但实际一步没跑。

**变异注入复验**：

- 把 `pick_active_run` 里的 `.cmp(&run_liveness_rank(b))` 整段删掉（只按时间/名字排）→ 重跑应 **FAIL**。单测侧同款变异已覆盖：`bot_skills::state::tests::pick_active_run_prefers_live_over_terminal_regardless_of_input_order` → **确定性红**。
- 把 `take_over_switched_runs` 里的 `runs.remove(n);` 删掉 → `take_over_switched_runs_drops_previous_skill_of_same_session` → 变红。
- 把 `scheduler.rs` 里的 `if run.name != name { … }` 块删掉 → `src-tauri/tests/skill_e2e.rs::scheduler_e2e_bails_out_when_another_skill_takes_over` → 变红。

**平台**：跨平台（纯内存 HashMap + 纯函数排序）。**不依赖 LLM**。

> **顺序提醒**：M23 与 M24 是同源事故链（路由截胡 → 模型中途 `use_skill` 切换 → 切换路径「刚切就断」）。**先跑 M24（路由命中正确的技能），再跑 M23**，否则 M23 的失败可能只是被路由 bug 触发。

---

### M24 · 用户点名的技能优先于 intent 正则

**靶点**：`src-tauri/src/intent_router.rs::explicit_name_pattern`；`src-tauri/src/bot_skills/manage.rs::intent_rules_from_dirs`（提交 `708507bc`）

**bug 形态**：用户发 `用 PPT-craft-master 技能做PPT`。修复前路由表只认 frontmatter 的 `intents` 正则，先命中先赢。内置 `pptx-design` 的 intents 里有一条 `"PPT"` 正则 → **先命中** → **启错技能**。后果链：模型只能中途 `use_skill` 切换 → 而切换路径**正是「技能刚加载就断」的事故现场**。

次生 bug：没有声明 `intents` 的技能**根本无法被点名**；第三个边界 bug：裸名字匹配会让 `my-ppt-craft-master` 命中 `ppt-craft-master`（子串误命中）。

**修复后的规则**：点名规则 = `(?i)(?:^|[^A-Za-z0-9_.\-]){name}(?:$|[^A-Za-z0-9_.\-])`，**所有点名规则整体在前、所有 intent 规则整体在后**。

**步骤**：

1. 前置：已安装一个**名字带 `-` 或 `_`** 的技能（例 `ppt-craft-master`）；另有一个 intents 含 `"PPT"` 的技能（例内置 `pptx-design`）。
2. 发：`用 ppt-craft-master 技能做PPT`
3. 大小写变体：`用 PPT-Craft-Master 技能做个 PPT`
4. **对照组（必做）**：`做个PPT`
5. **负例**：`用 my-ppt-craft-master-副本 技能做个PPT`

**判据**：

- 第 2/3 步 PASS：bot.log 的 `skill.start` 审计事件 `name=ppt-craft-master`（**不是** `pptx-design`）；路由审计行 `action=skill` / `detail=ppt-craft-master`。
- 第 2 步 FAIL：`skill.start` 的 `name=pptx-design` → 点名被 intent 截胡。
- 第 4 步 PASS：这次命中 `pptx-design`（intent 正则仍工作）。**若对照组也命中 `ppt-craft-master` → 点名规则写太松（误命中）**。
- 第 5 步 PASS：**不**命中 `ppt-craft-master`（名字前后是名字字符时不算点名）。

**变异注入复验**：

- 把 `explicit_name_pattern` 里的 `if !name.contains('-') && !name.contains('_') { return None; }` 删掉 → 重跑应 **FAIL**。单测侧：`intent_router::tests::explicit_name_pattern_matches_standalone_name_only` → 变红。
- 去掉 `(?i)` → 同一条单测变红。
- 把 `[^A-Za-z0-9_.\-]` 改成 `[^A-Za-z0-9]` → 同一条单测变红。
- 把 `intent_rules_from_dirs` 里的 `rules.extend(visit_skill_dirs(...))` 改成 `rules = visit_skill_dirs(...)`（intent 覆盖点名）→ `bot_skills::manage::tests::intent_rules_cover_enabled_skills_with_name_rule_before_intent_rule` → 变红。

**平台**：跨平台（纯正则 + Vec 操作）。**不依赖 LLM**。

> **坑**：`intent_router::ROUTES` 是**进程级单例**——真机上重建路由表的时机是 app 启动 + `skills_import` / `skills_delete` 成功后。

---

### M25 · 等确认的技能不再无限期占着同名技能

**靶点**：`src-tauri/src/bot_skills/runtime.rs`（`advance_skill` / `sweep_expired_pauses` / `skill_conflict_with_existing` / `confirm_state` / `skill_timed_out`）（提交 `b1ac8348`）

**bug 形态**：会话 A 启动技能 S → 走到某一步触发 `__await_user__` → 状态变 `Paused` → **用户再也不回这个会话**。修复前 `Paused` 不查超时 → 这个 run 无限期留在 `SKILL_RUNS` 表里 → `skill_conflict_with_existing` 把别的会话的 Paused 当成活动 → **会话 B 想启动同名技能 S，被拒**：

```
技能「S」正在另一个会话/任务中运行，请等它结束后再启动
```

→ **同名技能被永久占位**。另两处：用户超时后才点确认 → 恢复 `Running` → **过期计划复活**继续动手。

**步骤**：

1. 前置：一个带 `__await_user__`（或任何会暂停等确认的步骤）的技能；**两个会话**（同开两个聊天窗口）。
2. 会话 A 发 `用 <技能S> 帮我 <任务>`，走到等确认那一步，**不做任何确认**。
3. 等过技能声明的 `timeout_secs`。
4. 在**会话 B** 发 `用 <技能S> 帮我 <任务>`。
5. **对照（必做）**：会话 A 刚暂停（< timeout_secs）时立刻在会话 B 发同名技能 → 期望**被拒**。
6. **确认路径复验**：会话 A 暂停 → 等过期 → 点确认（或发 `/confirm`）。

**判据**：

- 第 4 步 PASS：bot.log 出现 `skill_expired | name: <技能S> | 等待确认超时，已过期并释放占位`；会话 B 的 `use_skill` **成功**（不再被拒）；会话 A 的技能状态变「可恢复失败」（`failed_recoverable`），不是永远「等待确认」。
- 第 4 步 FAIL：会话 B 被拒，文案是 `技能「S」正在另一个会话/任务中运行，请等它结束后再启动`。
- 第 5 步 PASS：**仍然被拒**（同名技能确实被占着）。**这条没有 = 放行条件写太松**。
- 第 6 步 PASS：bot.log 出现 `skill_confirm | name: <技能S> | approved: true | -> Failed 等待确认超时，已过期`，且技能**没有**恢复运行（不再有后续工具调用）。
- 第 6 步 FAIL：`-> Running` → 过期计划复活继续动手。

**变异注入复验**：

- 把 `advance_skill` 里的 `SkillState::Paused` 分支改回 `AdvanceAction::AwaitConfirm` → 重跑应 **FAIL**。单测侧同款变异已覆盖：`bot_skills::runtime::tests::advance_skill_paused_fails_after_timeout` → 变红。
- 把 `skill_conflict_with_existing` 里的 `SkillState::Paused => !skill_timed_out(r, now_ms),` 改成 `SkillState::Paused => true,` → `skill_conflict_expired_pause_no_longer_blocks` → 变红。
- 把 `confirm_state` 里的 `if skill_timed_out(run, now_ms) { … return; }` 块删掉 → `confirm_state_expired_pause_fails_instead_of_resuming` → 变红。
- 把 `skill_timed_out` 里的 `>` 改成 `>=` → `advance_skill_paused_within_timeout_awaits_confirm` → 变红（边界 179s/180s）。

**平台**：跨平台（纯内存 HashMap + 整数毫秒运算）。**不依赖 LLM**。

> **口径提示**：DSL 的 `__await_user__` 路径**不在本层判超时**——超时裁决统一归状态机层（`advance_skill`）与清扫（`sweep_expired_pauses`）。做卡片时不要把这两处混为一谈。

---

## 五、T2 档：可感知语义

### M26 · 出口闸：自检判定字样不能当终稿

**靶点**：`src-tauri/src/bot_model_loop.rs`（出口闸）（提交 `73331d76`）

**bug 形态**：工具语义自检与终稿自检都问「请仅用一个字回答：Y/N」，判定字样**永远不是给用户的答复**。这条原先只在「刚问完、正在等这一轮回答」的窗口内生效——模型若用行动代替回答（继续调工具核验），自检态随「以行动代回答」释放，那句问话却还留在上下文里，模型在后面某轮补回一个 Y；那个 Y 落到循环出口就成了终稿。

**真机实测**：用户只看到一个 Y，模型本要写的报告压根没产出（前端收尾用 `bot_chat` 返回值覆盖流式气泡，用户看到的就是那个 Y）。

改成在**出口处**执行这条不变式：终稿是纯 Y/N 单字 + 本轮问过自检 → 续跑一轮，让模型说出真实结果；追问额度有硬顶（2 次），仍只给判定字样就换成一句可读说明。

**作用域不能省**：没问过自检的回合里，一个 Y 就是合法答复（用户明说「只回答 Y 或 N」），闸不能劫走。

**步骤**：

1. 发一条会触发工具自检、且模型用行动代替回答的消息（要真触发「用行动代回答」这条路径较难，最稳是让模型先调一次工具再回答）。
2. 观察最终气泡。

**判据**：

- PASS：最终气泡**不是**裸 `Y` / `N`；若是追问耗尽的情形，文案是 `（模型只回了自检判定「{}」，没有给出可读的结果）`；续跑后有真实正文。
- FAIL：最终气泡只有一个 `Y`（或 `N`）。
- **对照组（必做）**：发 `只回答 Y 或 N` 这类明确要单字的请求 → 闸**不能**劫走，气泡里就该是 `Y`。

**变异注入复验**：把出口闸的「纯 Y/N 单字 + 本轮问过自检」判定退回旧口径 → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`src-tauri/tests/llm_integration.rs` 的 4 条（工具自检迟到的判定不能当终稿 / 终稿自检迟到的判定同样不能当终稿 / 没问过自检的裸 Y 仍作答复 / 追问额度封顶后）→ 变红。

**平台**：跨平台。

---

### M27 · 老库补 onion 两列

**靶点**：`src-tauri/src/db/mod.rs::open_db` → `migrations::ensure_onion_config_columns`（提交 `fad6ffa6`）

**bug 形态**：`ensure_onion_config_columns` 定义了却**全仓没有调用点**。`bot_sessions` 的 DDL 是 `CREATE TABLE IF NOT EXISTS`，老库走不到那句 DDL，两列永远补不上 → **每会话读 onion 配置必炸 `no such column: onion_config`**（每会话的 per-onion 配置永远不生效，日志里每次读都记一条 `read_meta_fail`）。

**步骤**：

1. 造一个**老库**：拿一个没有 `onion_config` / `onion_max_turn` 两列的 `wmessage.db` 备份（或手工 `ALTER TABLE bot_sessions DROP COLUMN`）。
2. 用它替换当前库 → 启动应用 → 开一个会话 → 看启动日志。

**判据**：

- PASS：启动日志**没有** `no such column: onion_config` 报错；`PRAGMA table_info(bot_sessions)` 里有 `onion_config` 与 `onion_max_turn` 两列。
- FAIL：每次读 onion 配置报缺列（每次开新会话一条 `read_meta_fail`）。

**变异注入复验**：把 `open_db` 里的 `migrations::ensure_onion_config_columns(&conn)?;` 删掉 → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`db::tests::open_db_calls_ensure_onion_config_columns`（接线守卫，用 `concat!` 拆串防自匹配）→ 变红；`ensure_onion_config_columns_adds_columns_to_legacy_bot_sessions` → 变红。

**平台**：跨平台（SQLite 层）。

---

### M28 · 标签合并搬进事务 + 别名按精确相等匹配

**靶点**：`src-tauri/src/bot/tags_consolidate.rs::merge_tag_cluster`；`src-tauri/src/lib.rs::tags_consolidate_merge`（提交 `bac7f803`）

**bug 形态**：

- **① 无事务**：合并逐条 `UPDATE` 自动提交，回滚凭据（`tag_merge_history` 行）在全部 UPDATE 之后才写 → 中途任一条失败时前面的改写已落盘而凭据没写 → **半合并态且不可回滚**，「合并可完整撤销」在失败路径上根本不成立。命令层还走裸 `open_db`，绕过 DB 写锁。
- **② 子串越界**：SQL 用 `instr(tags, ?)` 做子串粗筛（别名「测试」命中「集成测试」），Rust 侧只按精确相等滤别名、却**无条件**给命中的卡 push 主标签 → 从未拥有该簇成员标签的卡被打上主标签。

**判据用的原文**：

```
[<ts>] INFO | tag_merge | run_id=tgc-<ms>-<6位hex> | merged_from=<别名逗号串> | merged_to=<主标签> | affected_task_count=<N> | before_tags_per_task={"<task_id>":["旧标签",…]}
[<ts>] INFO | tag_merge_rollback | run_id=… | restored_task_count=<N> | timestamp_ms=<ms>
```

**步骤**：

1. **前置门槛（硬）**：归档页 →「整理标签」弹窗的扫描走 `embed_text`（本地 ONNX 嵌入模型）。**嵌入引擎不可用时扫描静默返回 0 个簇**，弹窗只显示「没有发现需要合并的标签簇」→ 卡片无从下手。**做之前先确认机器上有嵌入模型**。
2. 造数据：归档卡两张——A 的 tags 含 `测试`、B 的 tags 只含 `集成测试`；两者还要能被聚成同簇（余弦 ≥ 0.92）。
3. 归档页 →「整理标签」→ 选簇 + 主标签 → 合并。

**判据**：

- PASS：B 卡 tags **原样不动**（`集成测试` 仍在、没有被加主标签）；A 卡 tags 被改写；bot.log 的 `tag_merge` 行 `affected_task_count` **只含 A**；`tag_merge_history` 有对应快照行。
- FAIL：B 卡被打上主标签（子串越界）；或 `affected_task_count` 含 B。

**变异注入复验**：

- 把 `merge_tag_cluster` 里「精确相等判定」那 4 行（`if !old_tags.iter().any(|t| aliases.contains(t)) { continue; }`）删掉 → 重跑应 **FAIL**。单测侧同款变异已覆盖：`tags_consolidate::tests::merge_skips_cards_that_only_contain_the_alias_as_a_substring` → 变红。
- 把 `let tx = conn.transaction()…` 改成 `let tx = &mut *conn;` 并删掉 `tx.commit()…` → `merge_rolls_back_every_rewrite_when_the_history_row_cannot_be_written` → 变红。
- 把 `lib.rs::tags_consolidate_merge` 里的 `crate::db::lock::with_write_conn(&app, …)` 改回裸 `open_db` → 并发一致性回退。

**平台**：跨平台一致（SQLite + Rust 事务）。Windows 上 SQLite 文件锁更严格 → ① 的失败路径在 Windows 上更容易自然触发。

> **备份**：合并前 `cp <data_dir>/wmessage.db /tmp/wmessage.db.bak`；或事后用「设置页 → 操作历史 → 撤销」回滚。

---

### M29 · 标签合并/撤销后广播 + 新建主标签占位值拒收

**靶点**：`src-tauri/src/lib.rs::broadcast_rewritten_tasks`；`src-tauri/src/bot/tags_consolidate.rs::merge_tag_cluster`；`src/components/TagConsolidateModal.tsx`（提交 `8572b5d3`）

**bug 形态**：

- **① 不广播**：合并改写了归档卡的 tags，但命令层不广播任何事件 → 前端主窗口不知道变了 → 归档页与图谱一直挂着合并前的标签。模块头一直承诺却没写的 `tag_merge` 审计事件（此前只有回滚有审计）。
- **② 占位值下传**：「+ 新建主标签…」是个 select 选项，占位值 `"__new__"` 原样下传 → 不填名字直接提交 → 整簇被并进一个叫 `__new__` 的标签。
- **③ 主标签不 trim、不限长**：`"  买菜 "` 与 `"买菜"` 变成两个标签；超长名后端也收（与任务标签写路径 `MAX_TAG_LEN=30` 口径不一致）。

**判据用的原文**：

- 后端错误文案：`主标签不能为空` / `主标签名还没填——请输入新主标签名再合并` / `主标签过长（上限 30 字）` / `成员别名至少 1 个`
- 前端就地拦截：`请填写新主标签名` / `新主标签名不能是占位值` / `新主标签名过长（上限 30 字）`（红字显示在底部条带，提交按钮文案 `合并 {N} 个簇` **禁用**）
- 广播事件：`tasks-changed`（全体窗口）、`tasks-updated`（payload `{"source":"main","upserts":[…],"deletes":[]}`）

**步骤**：

1. **同时开着归档页和图谱页**。
2. 归档页 →「整理标签」→ 某簇的「主标签：」下拉选「+ 新建主标签…」→ **不填名字** → 看提交按钮。
3. 从 DevTools 直接 `invoke("tags_consolidate_merge", { main: "__new__", members: ["买菜"] })` → 看后端错误。
4. 填个合法名 → 合并 → **不手动刷新**，看归档页与图谱页。

**判据**：

- 第 2 步 PASS：红字 `请填写新主标签名`；提交按钮**禁用**；点击**不发** `tags_consolidate_merge` 请求。
- 第 3 步 PASS：错误 `主标签名还没填——请输入新主标签名再合并`（**证明双层校验**：前端拦一道，后端也拦一道）。
- 第 4 步 PASS：归档页 / 图谱页**不刷新就显示新标签**；bot.log 出现 `tag_merge` 行。
- FAIL：合并后两页仍挂着旧标签（不广播）；或 `"__new__"` 被当成真标签建出来。

**变异注入复验**：

- 把 `merge_tag_cluster` 里的 `if main == NEW_MAIN_PLACEHOLDER { return Err("主标签名还没填——请输入新主标签名再合并".to_string()); }` 删掉 → 重跑应 **FAIL**。单测侧同款变异已覆盖：`tags_consolidate::tests::merge_rejects_the_new_tag_placeholder_as_main` → 变红。
- 把 `let main = main.trim();` 改成 `let main = main;` → `merge_trims_the_main_tag` → 变红。
- 把 `lib.rs::broadcast_rewritten_tasks` 里两条 `emit` 都删掉 → 重跑应 **FAIL**（归档页/图谱不刷新）。**注意：这条没有后端单测，只有 UI 观察。**
- 把 `lib.rs` 里 `crate::audit::write_event(… "tag_merge" …)` 整段删掉 → `bot.log` 里查不到合并事件 → **FAIL**。

**平台**：跨平台一致（前端校验 + 后端校验双层；广播是 Tauri 事件）。

---

### M30 · 程序运行的僵尸行

**靶点**：`src-tauri/src/apps/mod.rs`（`mark_run_failed` / `sweep_stale_runs_conn` / `sweep_stale_runs` / `finish_run_failed`）（提交 `ce3faead`）

**bug 形态**：失败路径不回写 → `app_runs` 的行**永远停在 `running`** → 程序页上显示「运行中」，用户以为程序还在跑，实际早就没影了。

**步骤**：

1. 程序页 → 跑一个**会失败**的程序（例：脚本里写一句会抛异常的话，或传一个不存在的脚本路径）。
2. 看程序页的历史列表。

**判据**：

- PASS：该行落终态（`status='failed'` + 摘要），页面显示「失败」而不是「运行中」。
- FAIL：该行永远显示「运行中」。

**清扫路径（独立一条）**：进程崩在写回之前时，行会永远停在 running。`sweep_stale_runs` 在**程序页历史读取前**与**新运行开始前**触发，阈值 `STALE_RUN_AGE_MS = 2 小时`（取 `py::runtime::MAX_TIMEOUT_SECS` 的四倍）。

**变异注入复验**：把 `mark_run_failed` 的调用点删掉（或让它 `return` 不写回）→ 重跑应 **FAIL**。
单测侧同款变异已覆盖：`apps::tests` 里「运行行必须落终态」那组用例 → 变红。

**平台**：跨平台。

---

### M31 · 采集失败的审计记在转折上

**靶点**：`src-tauri/src/bot/services.rs::note_sample_status`（提交 `d020b6c7`）· **macOS 专有**

**bug 形态**：S07 卡的判据是「只在『成功 → 失败』转折记一条 WARN，不刷屏」。本机把 `lsof` 的 exec 掐掉（沙箱，不动系统文件）复现时，这条断言是红的：**转折那一刻什么都没记，之后每轮失败各记一条**。

根因在 `note_sample_status`：`swap` 返回的是**换之前**的值，条件写成 `prev || complete` 时记的其实是「上一轮失败、本轮还失败」——与函数自述正好相反。改成 `if !prev || complete { return; }`。

**步骤**：

1. macOS 上用 `bash scripts/sandbox-lsof-test.sh`（沙箱只对这个进程掐掉 lsof 的 exec，不用 sudo、不改系统文件）。
2. 跑一个受管服务（例：`start_service` 起一个 python 服务），等 30s 采样轮跑过。
3. 看 `bot.log` 里 `service.listening_sample_failed` 的条数。

**判据**：

- PASS：**只有一条** `service.listening_sample_failed`（转折那一条）；之后的连续失败不再记。面板快照 JSON 的 `listeningSampleOk` 翻 `false`。
- FAIL：一条都没有（转折没记），或每 30s 刷一条（连着记）。

**变异注入复验**：把 `note_sample_status` 里的 `if !prev || complete {` 改回 `if prev || complete {` → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`bot::services::tests::failed_listening_sample_is_audited_once_and_reported_as_not_judged` → 变红；`bot::procs::tests::listening_sample_reports_failure_when_lsof_cannot_run` → 变红。

**平台**：**macOS 专有**（Linux 侧采样走 `/proc`，掐掉 lsof 造不出同样的失败）。Linux 侧采样失败另找造法（把 `/proc` 采集路径掐掉）。

---

### M32 · 机器人总开关关闭时到点作业不再每 30s 空转一轮

**靶点**：`src-tauri/src/bot_scheduler.rs::classify_job` / `clear_pending_retry_after_panic`（提交 `633e7111`）

**bug 形态**：机器人总开关关闭时，到点作业仍被每 30s 一轮地「派发 → 在 `run_scheduled_job` 里被拒」→ **一天几千条噪声审计**。而且 `classify_due` 的水位推进语义被破坏——开关关闭期间水位也推进了，恢复后就不会补跑。

**修复后的语义**：`classify_job` 判出 `JobAction::Held`——**不派发、水位不推进**（恢复后 2h 窗口内补跑、超窗判 missed，与开关关闭前语义一致）。

**步骤**：

1. 设一个到点作业（例：每分钟一次）。
2. **关掉机器人总开关**（设置页 → 机器人分区的主开关）。
3. 等 5 分钟以上（跨过好几个 30s 扫描轮）。
4. 看 `bot.log` 里的调度审计行。

**判据**：

- PASS：开关关闭期间**没有** `sched_job_skip | id: … | 机器人开关未开启` 刷屏（一条都不该有）；作业的 `sched_last` 水位**不推进**。
- FAIL：每 30s 一条 `sched_job_skip`（一天几千条）；或水位推进了（恢复后不补跑）。

**panic 路径（独立一条）**：作业 panic 后待触发的重试还挂在行上 → 不清的话下一轮扫描它又到点、又跑一次、又 panic（**每 30s 一轮的无限环**）。修复后清掉 `retry_at` / `retry_count`，并记 `sched_job_panic`；清失败记 `sched_job_retry_clear_failed`。

**变异注入复验**：

- 把 `classify_job` 里的 `JobAction::Held` 分支改回 `JobAction::Dispatch`（或去掉 `bot_on` 判断）→ 重跑应 **FAIL**（`sched_job_skip` 刷屏）。单测侧同款变异已覆盖：`bot_scheduler::tests::classify_job_due_held_when_bot_off` → 变红。
- 把 `clear_pending_retry_after_panic` 里的 `retry_at` / `retry_count` 归零去掉 → `classify_job_retry_paths` → 变红。

**平台**：跨平台。

---

### M33 · 上下文栏默认看改动

**靶点**：后端 `src-tauri/src/db/trace.rs`（`summarize_path` / `read_before_snapshot` / `file_changes_for_session`）；前端 `src/components/ChatPanel/ContextRail.tsx`、`src/lib/diff.ts`、`src/lib/sessionFileChanges.ts`（提交 `d660eb0b` `09526909`）

**bug 形态**：侧栏标题承诺的是「**本会话文件改动**」，但前端那份 ± 是按 path 去重「**后者覆盖前者**」的 → 一个文件被改多次就只剩**最后一次**的数字，与标题对不上；而且点开文件甩一屏**全文**，用户得自己找变化点。

**判据用的接口与文案**：

| 通道 | 形状 |
|---|---|
| Tauri command | `bot_session_file_changes`（`sessionId`；`path` 不给 = 清单，给 = 单文件详情） |
| 返回结构 | `{ items: [{ path, kind, added, deleted, changes, mode, drifted, note }], detail: { …, diff, perChange, truncated, drifted, note } \| null }` |
| 数据库表 | `file_changes`（`trace_id` / `span_id` / `path` / `kind` / `added` / `deleted` / `diff` / `truncated` / `before_ref` / `before_sha` / `after_sha` / `created_at`） |
| 前端文案 | `本会话文件改动` · `读取改动中…` · `本会话还没有文件改动` · `无行级 diff` · `逐次` · `返回文件列表` · `看全文` · `读取改动…` · `· 本会话改了 {N} 次` · `这个文件相对本会话开始时没有净变化。` · `这次改动没有行级 diff` · `看改动` · `⋯ 省略 {N} 行未改动` · `diff 超长已截断（完整内容看文件）` |
| 后端降级文案 | `文件在 AI 最后一次修改后又被改过，此处包含非 AI 的改动` · `二进制文件没有行级 diff；可切「查看全文」用系统默认程序打开` |

**步骤**：

1. 在聊天里让机器人**改同一个文件两次以上**：

```
请在当前项目里完成两步：第一步，在 src/format.ts 的文件末尾追加一行注释 // demo step 1；第二步，把刚才那行注释改成 // demo step 2（不要新增文件，就改这一处）。改完请告诉我这个文件现在最后一行是什么。
```

2. 点右侧栏「文件」页签 → 看「**本会话文件改动**」清单。
3. 点某个文件 → 看详情页默认是不是 diff；两段改动之间有没有 `⋯ 省略 N 行未改动`。
4. 点右上角「**看全文**」→ 再点「**看改动**」→ 回得来吗。
5. AI 写完后再**手动改**该文件 → 回详情页看漂移提示。

**判据**：

- PASS：清单的 `+N −M` = **各次之和**（不是最后一次）；详情头部有 `· 本会话改了 2 次`；默认视图是 diff 不是全文；有 `看全文` 按钮且能来回切；两段改动之间是 `⋯ 省略 N 行未改动`；手动改过后详情出现 `文件在 AI 最后一次修改后又被改过，此处包含非 AI 的改动`。
- FAIL：`+N −M` 只等于最后一次的数；点开直接是全文；省略行数少/多一行。

**变异注入复验**：

- 后端：把 `trace.rs::summarize_path` 里 Session 档的 `added` / `deleted` 改成取最后一条 → 重跑应 **FAIL**。单测侧同款变异已覆盖：`db::trace::tests::session_mode_diffs_against_session_start` → 变红。
- 后端：把 `read_before_snapshot` 的 `return FileState::Missing(…)` 改成 `return FileState::Text(String::new());` → `missing_snapshot_degrades_to_per_change_with_reason` → 变红。
- 前端：把 `src/lib/diff.ts::parseDiffSegments` 里的 `oldStart - prevEnd` 改成 `oldStart - prevEnd - 1` → `src/lib/diff.test.ts` 的「两段之间的省略行数 = 下一段起始旧行 − 本段起始旧行 − 本段计数」→ 变红。

**平台**：跨平台。数据落点：`file_changes` 行在 `wmessage.db`；起始快照在 `data_dir/checkpoints/<before_ref>`。

> **别写死绝对路径**：启动时写审计事件 `app_data_dir`（kv `path`），先 grep 这一行拿到本次落点。

---

### M34 · MCP 面板资源行按 5s 轮询

**靶点**：`src/components/SettingsPage/McpPanel.tsx`（提交 `01eaaa44`）

**bug 形态**：资源行（pid / 内存 / CPU / **已运行**）是活的量，但只有挂载与事件两个刷新点 → 它们会永远停在打开那一刻的值（**「已运行」停在 0 秒**就是这么来的）。

**步骤**：

1. 设置页 → MCP 分类 → 启动一个 MCP 服务器（或观察已有的）。
2. 盯着资源行的「已运行」列，放 15 秒。

**判据**：

- PASS：「已运行」秒数**每 5s 递增**；切走本分类再切回来，数值**立刻**刷新（不卡在离开那一刻）。
- FAIL：秒数一直停在 0（或一个固定值）。

**变异注入复验**：把 `McpPanel.tsx` 里那个 `setInterval(tick, 5000)` 改成 `setInterval(tick, 60000)` → 重跑应 **FAIL**（15 秒内看不到递增）。
单测侧同款变异已覆盖：`src/components/SettingsPage/McpPanel.test.tsx` → 变红。

> **间隔口径**：5s 与后端采样层的 5s TTL 缓存对齐——轮询更快反而拿同一份快照。
> **副作用**：`active` 属性控制「不可见时是否后台轮询」——设置页分类是「惰性挂载 + hidden 隐藏」，面板挂上后不会卸载，所以可见性要由外面告知。变异时别把这个开关删了。

**平台**：跨平台（纯前端）。

---

### M35 · `taskEq` 改用键序无关的比较

**靶点**：`src/storage.ts`（提交 `b2ad135a`）

**bug 形态**：`taskEq` 用 `JSON.stringify` 全串比 → **键序不同就算两个不同的任务** → 每次从后端拿回来的任务都被判成「变了」→ 触发无谓的重新渲染 / 重复保存。

**步骤**：

1. 打开任务时间线页，观察任务列表。
2. （真机不易直接看见「键序不同的同一任务」——这条的主证据是单测。）

**判据**：

- PASS：同一任务在键序不同的两次载入之间**不被判成改动**（前端无无谓重渲染、无重复保存）。
- FAIL：每次载入都判成改动。

**变异注入复验**：把 `taskEq` 改回 `JSON.stringify` 全串比 → 重跑应 **FAIL**。
单测侧同款变异已覆盖：`src/storage.test.ts` → 变红。

**平台**：跨平台（纯前端）。

---

### M36 · 标签弹窗的「新建主标签」能打出名字

**靶点**：`src/components/TagConsolidateModal.tsx::mainTagError` / `ClusterSelection`；`src/lib/botConsolidate.ts`（提交 `48b53c9b`）

**bug 形态**：① 「新建」模式与主标签名**挤在同一个字段** `main` 里：输入框的渲染条件是 `main === "__new__"`，用户敲第一个字符 → `main` 变成那个字符 → **输入框当场消失** → 名字一个都打不全，选「新建」等于把簇并进一个乱码标签。② 提交前不校验：没填名字、名字超长都能提交。

**步骤**：

1. 前置（与 M28/M29 同）：确认嵌入模型可用，归档页有簇。
2. 归档页 →「整理标签」→ 某簇「主标签：」选「+ 新建主标签…」→ 输入框出现 → **连续打完整个名字**（例「买菜」）。
3. 清空 → 看红字与按钮。
4. 填 31 个字 → 看红字。

**判据**：

- PASS：输入框出现（`placeholder="新主标签名"`、自动聚焦）且**能连着打完整个名字**；未填名 → 红字 `请填写新主标签名` + 按钮禁用 + 点击**不发**请求；填占位值 → 红字 `新主标签名不能是占位值`；超长 → 红字 `新主标签名过长（上限 30 字）`；合法名 → 按钮可用，提交后弹窗关闭，主标签 = trim 后的新名。
- FAIL：输入第一个字符后**输入框消失**；或没填名也能提交。

**变异注入复验**：

- 把 `mainTagError` 里的 `if (!v) return "请填写新主标签名";` 改成 `if (!v) return null;` → 重跑应 **FAIL**。单测侧同款变异已覆盖：`src/components/TagConsolidateModal.test.tsx` → 变红。
- 把 `src/lib/botConsolidate.ts` 的 `MAX_MAIN_TAG_LEN = 30` 改成 `31` → `src/lib/botConsolidate.test.ts`「长度上限与后端任务标签口径一致」→ 变红。

**平台**：跨平台（纯前端）。唯一跨端约束：前后端常量必须一致——`NEW_MAIN_PLACEHOLDER = "__new__"`、`MAX_MAIN_TAG_LEN = 30` ↔ Rust `NEW_MAIN_PLACEHOLDER` / `MAX_TAG_LEN = 30`。

> **「输入框在第一个字符后消失」是瞬时行为，真机截图取证困难** → **这条的主证据应是前端单测**，真机只做体验确认。

---

### M37 · 机器人气泡宽度只增不减

**靶点**：`src/components/ChatPanel/stickyBubbleWidth.ts::stickyFloor`（提交 `cd060bcd`）

**bug 形态**：气泡是**内容自适应宽度**。工具折叠行里「正在跑 xxx」出现就把气泡撑宽，那行在工具跑完后变短又缩回去 → **同一条回复在流式过程中宽度反复跳**。

**步骤**：

1. 发一条会连着跑多个工具、且工具名一长一短的长回复：

```
请在当前项目里做一次端到端的自查：先跑一条很长的命令（例如 ls -la 加上项目目录列表），再用 edit_file 把 src/format.ts 里 fmtElapsed 函数加一段注释说明它的用途；改完之后请把整个自查过程按时间顺序写一段说明给我。
```

2. 盯着**同一条**助手回复的气泡左/右边缘。
3. devtools 选中该气泡的**外层列**（不是气泡本身），看 `min-width`。

**判据**：

- PASS：气泡左/右边缘在整个流式过程中**只往外扩、不往回收**；`min-width` 值单调不减，且**不超过行内可用宽度**（行右缘 − 列左缘）。
- FAIL：工具跑完后气泡**缩回**；或 `min-width` 超过行内可用宽度（横向溢出）。

**变异注入复验**：把 `stickyFloor` 改成 `return measured;`（退化成自适应）→ 重跑应 **FAIL**。
单测侧同款变异已覆盖：`src/components/ChatPanel/stickyBubbleWidth.test.tsx` 的「下限计算：只增不减」与「变窄时把宽度写回下限」→ 变红。

> **注意**：下限写在外层**列**的 `style.minWidth` 上（`MessageList.tsx` 的 `colRef`），不是气泡本身。用户气泡（`max-w-[85%] min-w-0`）**不参与**下限。

**平台**：跨平台（依赖 `ResizeObserver`，macOS WebKit 与 Windows WebView2 都支持）。

---

### M38 · macOS `list_windows` 的「（前台）」标注不再恒假

**靶点**：`src-tauri/src/ui/macos.rs::list_windows` / `foreground_matches` / `same_window_by_title`（提交 `1c9ea780`）

**bug 形态**：`UiWindow.focused` 在 macOS 侧**恒为 `false`**（`macos.rs` 里写死 `focused: false`），Windows 侧是真测的（`GetForegroundWindow`）。同契约两平台一真一假 → `tool_list_windows` 据此在文案末尾标 **「（前台）」** → **macOS 上这个标注是假的**，模型照着标注操作会被 M05 的前台闸拦下。

**步骤**：

1. 打开两个应用（例：备忘录 + Safari），**把 Safari 置前**。
2. `list_windows` → 看输出。

**判据**：

- PASS：Safari 那行**带**「（前台）」，备忘录那行**不带**。行样：
  ```
  可接管窗口 N 个：
  - id: 501:0 | 应用: 备忘录 | 标题: 未命名3（前台）
  ```
- FAIL：两行都不带（`focused` 恒 false 的老形态）或**两行都带**。
- **交叉验证**：把带「（前台）」的那行 id 直接给 `ui_type` → **不应**被 M05 的前台闸拒。

**变异注入复验**：

- 把 `macos.rs::foreground_matches` 里的 `frontmost_pid == target_pid` 改成 `true` → 重跑应 **FAIL**。单测侧同款变异已覆盖：`macos::tests::foreground_window_check_needs_the_same_app_and_title` → 变红。
- 把 `macos.rs::list_windows` 里的 `let focused = is_foreground_window(pid, &title);` 改回 `let focused = false;` → **单测不会红**，真机才看得出（所有行都不带「（前台）」）。

**平台**：**仅 macOS**（Windows 本来就是真的）。

> **平台固有限制**：macOS 没有公开的「哪个窗口在前台」标志，只能两级（应用 pid + 窗口标题）。同应用多窗口且标题都相同时（两扇都叫「未命名」），**两扇都带「（前台）」**——这是平台限制，不是 bug。

---

## 六、Windows 实机专章

Windows 上有四处平台差异会**直接改写测法**，见 §1.0。这三张卡只跑 Windows。

### W01 · 坐标兜底点击把像素换算成归一化值

**靶点**：`src-tauri/src/ui/mod.rs::pixels_to_normalized_absolute`；`src-tauri/src/ui/windows.rs::click_at`（提交 `25fdcd9b`）

**bug 形态**：`SendInput` 的绝对鼠标坐标**不是像素**，是「虚拟桌面宽高的 65536 分之一」。修复前把像素原样塞进 `MOUSEINPUT.dx/dy` → 落点 ≈ `(x/65535)×屏宽`。1920 宽屏上 x=500 → 落点 ≈ 14.6px 处；点屏幕中部（x≈960）→ 实际落到**屏幕左上角约 28px 处**——可能点中左上角的窗口按钮、菜单、标签页。**一次不受控的真实点击**可能点中左上角任意控件——比不点更糟。

**触发条件**：只有**控件不支持 Invoke 模式**（UIA `Invoke` pattern 取不到）时才走坐标兜底。多数按钮有 Invoke → 走不到这条路；**输入框、文本控件**没有 Invoke → 走坐标。

**步骤**：

1. 启动**普通权限**的记事本（或浏览器），用 `ui_inspect` 找一个**不支持 Invoke** 的控件（输入框 / 文本区）。
2. 让模型点该控件。
3. 看动作后截图。

**判据**：

- PASS：动作后截图里落点在**目标控件**上；点输入框后光标真的落在输入框里。
- FAIL（变异后）：光标 / 点击效果出现在**屏幕左上角**（1920 宽屏上约 x<30, y<20 的区域）。
- **多显示器变体**：把第二个屏接在**左侧**（虚拟桌面原点 x = -1920），点左屏上的控件 → 落点必须正确。

**变异注入复验**：

- 把 `pixels_to_normalized_absolute` 里的 `let nx = ((x - origin_x) / width * 65535.0).round();` 改成 `let nx = (x - origin_x).round();` → 重跑应 **FAIL**。单测侧同款变异已覆盖：`ui::tests::pixel_coordinates_must_not_be_used_as_normalized_values`（期望 `(32768, 32768)`，实得 `(960, 540)`）与 `normalized_coordinates_follow_the_virtual_desktop_origin` → 变红。
  > **这条在 macOS 上就能复现**：该函数标了 `#[cfg(any(windows, test))]`，macOS 上 `cfg(test)` 时仍编译 → `cd src-tauri && cargo test --lib ui::tests::pixel_coordinates` 即可跑红。
- Windows 侧：把 `ui/windows.rs::click_at` 里的 `let (sx, sy) = super::pixels_to_normalized_absolute(…)` 改回 `let (sx, sy) = (x as i32, y as i32);` → 落点缩到左上角。**Windows 上必做这一步**（macOS 的单测只覆盖纯函数，不覆盖调用点）。
- 把 `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_LEFTDOWN` 里的 `MOUSEEVENTF_VIRTUALDESK` 去掉 → 多显示器下原点算错。**没有任何单测覆盖**，只能真机验。

**平台**：**Windows 独有**——调用点 `click_at` 只存在于 `ui/windows.rs`，macOS 的点击走 AXPress，不做坐标换算，**macOS 真机永远验不到这条**。

> **UIPI 干扰**：Windows 上验这条时，应用必须与目标窗口**同权限级别**（推荐都用普通权限的记事本），否则撞 UIPI 而不是坐标换算。UIPI 的文案是 `注入点击失败（SendInput 只发出 {sent}/2 条——多半是被 UIPI 拦了：目标程序权限更高）`。

---

### W02 · Windows `UiNode.value` 不恒 None

**靶点**：`src-tauri/src/ui/windows.rs::collect`（提交 `1c9ea780`）

**bug 形态**：Windows 后端 `collect` 里写死 `value: None` → **所有控件的「当前值」恒空**。后果一：`tool_ui_inspect` 的过滤条件之一是 `n.value.to_lowercase().contains(&query)` → **在 Windows 上「按控件内容找控件」整体失效**；后果二：`ui_inspect` 输出里**永远没有** `| 值: xxx` 段。

**步骤**：

1. Windows 上用 `ui_type` 往某个输入框打入「变异测试 2026-10-23」。
2. 再 `ui_inspect`（带 `query=变异测试`）→ 看能不能搜到。

**判据**：

- PASS：`ui_inspect` 输出里非空值的控件多一段 ` | 值: {v}`，格式如
  `- id: 1:0.2 | 角色: 编辑 | 标题: 搜索 | 值: 变异测试 2026-10-23`；带 `query` 能搜到。
- **负向（重要）**：**密码框不显示** `| 值:`（`windows.rs::collect` 里 `value: if is_password { None } else { value_of(&child) }`）——避免明文泄漏进工具输出。
- FAIL：所有控件都没有「值:」段；或带 query 搜不到。

**变异注入复验**：把 `windows.rs::collect` 里的 `value: if is_password { None } else { value_of(&child) }` 改回 `value: None` → 重跑应 **FAIL**。
单测侧：`ui::tests::windows_backend_must_read_the_value_pattern`（源码字符串守卫）→ 变红。

**平台**：**Windows 独有**。macOS 的 AXValue 本来就读得到。

---

### W03 · 产物交付前剥「来自互联网」标记

**靶点**：`src-tauri/src/platform/zone_id.rs::strip_zone_identifier` / `strip_zone_identifier_rec`；`src-tauri/src/py/document.rs::strip_download_mark`；`src-tauri/src/py/harvest.rs`（提交 `d37d3fb0`）

**bug 形态**：Windows 上复制文件会连带复制 `Zone.Identifier` 备用数据流（`CopyFileEx` 复制全部数据流）→ app 交出去的副本被 Office 当成「外来的文件」：打开走受保护视图，把文件复制进 Office 文档缓存（`INetCache\Content.Word\~WRDxxxx.docx`），杀软再把这次「写文档」按风险程序连坐拦掉。

**2026-10-23 实机挨的就是这个。**

**步骤**：

1. Windows 上让机器人产出一个文档（docx / xlsx / pdf / pptx 都行）：

```
用 doc_make_word 把桌面上的 变异测试.txt 整理成一份正式的 Word 报告，输出到桌面。
```

2. 查产物有没有 `Zone.Identifier` 流。

**判据**：

- PASS：产物**没有** `Zone.Identifier` 备用数据流 → Office 打开不走进护保护视图、杀软不连坐。
- FAIL：产物带 `Zone.Identifier` → 打开走受保护视图。

**验法（PowerShell）**：

```powershell
Get-Item -Path "C:\Users\<你>\Desktop\报告.docx" -Stream * | Select-Object Stream
# 输出里不应有 Zone.Identifier
```

**变异注入复验**：把 `py/document.rs` 里 `strip_download_mark(&app, Path::new(&out), "doc_make_word");` 这一行删掉 → 重跑应 **FAIL**（产物带标记）。
单测侧同款变异已覆盖：`py::document::download_mark_guard_tests`（产物收口回归锁——每个交付给用户的文档产物都要剥标记，**新增文档产物时必须一并接上收口**）→ 变红。

**平台**：**Windows 独有**。macOS / Linux 没有这个概念，`strip_zone_identifier` 在非 Windows 上一律返回 `Ok(false)`。

> **剥不掉只记日志不中断**——标记只影响打开路径，文件本身是好的。剥不掉的项通过 `audit` 留痕，不中断遍历。
> **目录自身的标记不动**：Office 看的是**文件**的标记。

---

## 七、变异注入清单

本批 25 条注入，按「注入位置」分三组。**注入 → 重跑对应卡片 → 期望 FAIL → 还原**。

### 7.1 前端注入（改 `src/` 下的 TS/TSX，Vite 热更新，不用出包）

| # | 注入 | 对应卡片 | 注入后应当 |
|---|---|---|---|
| V01 | `chatDrafts.ts::writeChatDrafts` 开头插 `return;` | M20 | 切设置后输入框空了 → **FAIL** |
| V02 | `ChatPanel.tsx::resumeInflight` 开头插 `return;` | M19 | 重载后只剩最终正文 → **FAIL** |
| V03 | `streamState.ts::noteSeq` 改成永远放行 | M19 | 正文叠成两半 → **FAIL** |
| V04 | `App.tsx` 设置壳改回提前 return | M18 | 在途流式中断 → **FAIL** |
| V05 | `stickyBubbleWidth.ts::stickyFloor` 改成 `return measured;` | M37 | 气泡宽度又跳 → **FAIL** |
| V06 | `ContextRail.tsx` 砍掉「看全文」开关 | M33 | 点开直接甩全文 → **FAIL** |
| V07 | `diff.ts::parseDiffSegments` 的省略行数算式少减 1 | M33 | `⋯ 省略 N 行` 少一行 → **FAIL** |
| V08 | `McpPanel.tsx` 的 `setInterval(tick, 5000)` 改 60000 | M34 | 「已运行」不再涨 → **FAIL** |
| V09 | `storage.ts::taskEq` 改回 `JSON.stringify` 全串比 | M35 | 每次载入都判成改动 → **FAIL** |
| V10 | `TagConsolidateModal.tsx::mainTagError` 的空名检查改 `return null` | M36 | 没填名也能提交 → **FAIL** |
| V11 | `botConsolidate.ts::MAX_MAIN_TAG_LEN` 改 31 | M36 | 前后端常量对拍失败 → **FAIL** |
| V12 | `GraphCanvas.tsx` 铭牌改回 `innerHTML` | M13 | 尖括号被解析成元素 → **FAIL** |

### 7.2 后端注入（改 `src-tauri/src/` 下的 Rust，需重编译）

| # | 注入 | 对应卡片 | 注入后应当 |
|---|---|---|---|
| V13 | `bot_ui.rs::detail_is_dangerous` 去掉 `to_lowercase()` | M01 | 英文按钮不弹确认 → **FAIL** |
| V14 | `bot_ui.rs::click_needs_confirm` 删掉 `node.title.trim().is_empty() \|\|` | M02 | 无标题按钮不弹确认 → **FAIL** |
| V15 | `bot_ui.rs::gate_no_line_break` 的条件改 `if false` | M04 | 带换行的文本被发出去 → **FAIL** |
| V16 | `bot_ui.rs::gate_sensitive` 删掉角色/IsPassword 判定 | M03 | 密码框被代填 → **FAIL** |
| V17 | `bot_ui.rs::foreground_verdict` 的 `Ok(false)` 改 `Ok(())` | M05 | 文字打进前台应用 → **FAIL** |
| V18 | `bot_ui.rs::with_inject_lock` 去掉锁 | M08 | 两框串字 → **FAIL** |
| V19 | `ui/mod.rs::pixels_to_normalized_absolute` 去掉 `* 65535.0` | W01 | 落点缩到左上角 → **FAIL** |
| V20 | `bot_orchestrator.rs::classify_outcome` 的 `WALL_FUSE_MARKER` 判块挪后 | M21 | 墙钟熔断被认成工具调用熔断 → **FAIL** |
| V21 | `bot_orchestrator.rs::build_subagent_digest` 删掉「残留产出」块 | M22 | 父 agent 看不到残留 → **FAIL** |
| V22 | `tags_consolidate.rs::merge_tag_cluster` 删掉精确相等判定 | M28 | 子串卡被打上主标签 → **FAIL** |
| V23 | `lib.rs::broadcast_rewritten_tasks` 删掉两条 emit | M29 | 归档页/图谱不刷新 → **FAIL** |
| V24 | `bot/config/io.rs::ensure_legacy_plaintext_backed` 调用点删掉 | M15 | 盘上明文被抹 → **FAIL** |
| V25 | `ui/mod.rs::inspect_tree` 的 `reject_own_window` 调用删掉 | M39 | **应用被 WebKit 的 SIGTRAP 带走** → **FAIL** |

### 7.3 「最值得注入」的三条

如果时间只够跑三条：

1. **V19（Windows 坐标兜底）**——单测在 macOS 上就能跑红，但**真机只有 Windows 会出这个 bug**（macOS 走 AXPress）。这是「单测能跑红、真机才见伤害」的典型。
2. **V17（前台闸）**——真机的伤害最直观：文字直接敲进用户正在用的终端 / 地址栏，可能当场执行。
3. **V24（明文密钥安置）**——失败形态是**永久丢失**用户的 API Key，且发生在「一次无关的保存」上，最容易在用户毫无察觉时发生。

---

## 附录 A：逐项执行清单

按表序做。每项记 PASS/FAIL + 观测到的原文（贴日志行，不要只写「过了」）。

| # | 卡片 | 关键观测 | 预计耗时 | 破坏性 |
|---|---|---|---|---|
| M01 | 危险动作闸小写匹配 | 英文按钮弹确认窗 + 负向不弹 | 10 min | |
| M02 | 无标题按钮 fail-closed | 纸飞机按钮弹确认窗 + 负向不弹 | 10 min | |
| M03 | 敏感控件闸 | 拒绝文案 + 密码框空 | 10 min | |
| M04 | `ui_type` 拒绝换行 | 拒绝文案 + 目标框空 | 10 min | |
| M05 | 注入前要求前台 | 拒绝文案 + 前台窗口没多字 | 10 min | |
| M06 | 窗口 id 复用 | 「已经不是刚才那扇窗了」 | 15 min | |
| M07 | 闸门审的控件 ≠ 动的控件 | 「界面已变：控件 …」 | 15 min | |
| M08 | 注入全局互斥 | 两框各只有自己那串 | 15 min | |
| M09 | UI 开关变更留痕 | `ui_automation_toggle` 两行 | 5 min | |
| M10 | flag 文件权限 | `-rw-------` | 5 min | |
| M39 | 不许操作自己的窗口 | 清单无自己 + 碰自己被拒 + **应用不崩** | 10 min | |
| M11 | OCR 解码炸弹面 | 字节/像素闸拒绝 | 10 min | |
| M12 | 必应大图限长 | 超限直接拒 | 10 min | |
| M13 | 图谱铭牌不拼 innerHTML | 尖括号按字面显示 | 5 min | |
| M14 | 厂商不借全局主 key | 探活报错 + 无 Authorization 头 | 15 min | |
| M15 | 安置旧明文密钥 | 保存中止 + 盘上明文还在 | 15 min | ⚠️ |
| M16 | 终稿自检 vendor 门 | `final_selfcheck.vendor_not_verified` | 20 min | ⚠️ |
| M17 | 空 token 原子覆写 | `api_start` 成功 + 磁盘非空 | 10 min | ⚠️ |
| M18 | 设置页叠层 | 切设置期间流式不中断 | 10 min | |
| M19 | 在途流式回灌 | 重载后气泡接着长 | 15 min | |
| M20 | 输入草稿落盘 | 切设置后输入框还在 | 5 min | |
| M21 | 子 agent 预算治理 | 回执含「生效预算」 | 20 min | |
| M22 | 子 agent 收尾契约 | 「残留产出」「机检未过」 | 30 min | |
| M23 | 切技能取活着的 run | 新技能真跑起来 | 15 min | |
| M24 | 点名技能优先 | `skill.start` 的 name | 10 min | |
| M25 | 等确认技能释放占位 | `skill_expired` + 另一会话能启 | 20 min | |
| M26 | 出口闸拦自检判定 | 终稿不是裸 Y/N | 20 min | |
| M27 | 老库补 onion 两列 | 启动无缺列报错 | 10 min | ⚠️ |
| M28 | 标签合并事务 + 别名精确 | 子串卡 tags 不动 | 20 min | ⚠️ |
| M29 | 标签广播 + 占位值拒收 | 两页不刷新就变 | 15 min | ⚠️ |
| M30 | 程序运行僵尸行 | 失败行落终态 | 10 min | |
| M31 | 采集失败审计记转折 | 只记一条 | 15 min | macOS |
| M32 | 总开关关闭不空转 | 无 `sched_job_skip` 刷屏 | 10 min | |
| M33 | 上下文栏默认看改动 | 累计 ± 与「看全文」 | 15 min | |
| M34 | MCP 面板 5s 轮询 | 「已运行」在涨 | 5 min | |
| M35 | `taskEq` 键序无关 | 乱序键不再判改动 | 10 min | |
| M36 | 标签弹窗打出名字 | 输入框能连着打字 | 10 min | |
| M37 | 气泡宽度只增不减 | `min-width` 单调不减 | 10 min | |
| M38 | macOS 前台标注 | 只有前台那行带「（前台）」 | 5 min | macOS |
| W01 | 坐标兜底点击归一化 | 落点在目标控件上 | 20 min | Windows |
| W02 | Windows 控件值 | 有「值:」段；密码框没有 | 10 min | Windows |
| W03 | 剥「来自互联网」标记 | 产物无 `Zone.Identifier` | 15 min | Windows |

**总计约 11 小时**（不含变异复验）。建议分三段：

- **第一段（T0，M01–M10）**：安全边界。这段是**优先级最高**的——M03 / M04 / M05 的失败形态都是「真实操作落到不该落的地方」。
- **第二段（T1，M11–M25）**：静默失效。**M14 / M15 / M17 / M21 / M22 破坏性最高**，做之前先备份。
- **第三段（T2 + Windows 专章）**：可感知语义 + 平台专章。

> **Windows 用户的排期建议**：W 序列约 45 分钟，插在第一段之后跑完。**W01 排最久**（它是本次修复在 Windows 上的主战场，且要反复找「不支持 Invoke 的控件」）。**M31 只跑 macOS**（Linux 侧采样走 `/proc`，造不出同样的失败）。

---

## 附录 B：审计事件速查（本批涉及）

日志格式：`[YYYY-MM-DD HH:MM:SS.mmm] LEVEL | event | k=v | k=v`。WARN/ERROR 同时镜像到 `error.log`。

**UI 自动化**：`ui_automation_toggle`（**下划线命名**，kv `enabled` / `user`）· `ui.activate` · `ui.click` · `ui.type` · `confirm`（kv `id` / `kind=danger` / `tool` / `detail`）· `confirm_timeout` · `confirm_skipped`

**技能**：`skill.start`（kv `name` / `risk` / `mode` / `max_steps` / `timeout_secs` / `rollback`）· `skill_completed` · `skill_expired` · `skill_paused` · `skill_confirm` · `skill_dsl_run_replaced` · `skill_dsl_finish_signal` · `skill_dsl_await_user` · `pre_step.route_skill`

**子 agent**：`subagent_spawned` · `subagent_done` · `subagent_failed` · `subagent_cancelled` · `subagent_budget_exceeded` · `subagent_wall_soft_stop` · `fuse_wall` · `subagent.machine_check_passed` · `subagent.machine_check_failed` · `subagent.parse_failure_recovered` · `subagent_final_json_repaired` · `subagent_final_json_repair_failed` · `subagent_digest`

**凭据 / 配置**：`config.plaintext_key_stripped`（**只在 `add_allowed_dir` 路径写**）· `final_selfcheck.vendor_not_verified` · `final_selfcheck.model_resolve_failed` · `bot_config.verified_vendors_persist_failed`

**标签**：`tag_merge`（kv `run_id` / `merged_from` / `merged_to` / `affected_task_count` / `before_tags_per_task`）· `tag_merge_rollback`（kv `run_id` / `restored_task_count` / `timestamp_ms`）

**调度**：`sched_job_run` · `sched_job_skip` · `sched_job_done` · `sched_job_err` · `sched_job_retry` · `sched_job_autopaused` · `sched_job_panic` · `sched_job_retry_clear_failed` · `sched_job_stale` · `sched_job_missed` · `sched_job_update_skip` · `sched_job_delete_skip` · `sched_job_history_skip` · `sched_job_state_skip` · `sched_jobs_scan_failed`

**服务**：`service.listening_sample_failed` · `service.started` · `service.ready` · `service.start_failed` · `service.exited` · `service.stop_requested` · `service.stopped` · `service.stop_failed`

**其它**：`app_data_dir`（启动时写一次，kv `path`——**定位数据目录用这一行**）· `tool.return` · `tool.call` · `tool.output.over_budget` · `budget_tool_calls_exceeded`

> **走 stderr（不在 bot.log）**：MCP 长行截断诊断、机密读取失败、工具数超限（>128）、stdio 子进程树停止后仍在运行。

---

## 附录 C：只测不验（不进卡片的 5 条）

今天 42 条行为变更里有 5 条改的不是运行时行为，卡片对它们没有观测面：

| 提交 | 改了什么 | 为什么不做真机卡 |
|---|---|---|
| `1595c5ac` | 审计扫描器认得统一出口 `emit_recorded` 与 reg 登记表 | 改的是**门禁工具本身**（`tests-audit/audit_tauri_bridge.py`），不是运行时行为。它的验收口径是「门禁跑通」——`python3 -m pytest tests-audit/audit_tauri_bridge.py -q` |
| `2a4d5fd1` | `with_write_conn` 改用 `lock_db_write` | 闭包内调契约函数时断言会误判——**单测层的事**，真机上没有可观测差异（真机上要么能写要么不能写，没有第三条路） |
| `f1abcacc` | S21 的并发绑定不丢（测试提交） | 只加测试，不改运行时行为 |
| `a301a88b` `664fa39f` | S10 的真子进程 e2e / 同前缀不同名可共存（测试提交） | 同上 |

> **但这 4 条要过门禁**：跑 `bash scripts/test-all.sh`（含 `tests-audit/` 对拍）。`audit_doc_claims.py` 会核对**本文与兄弟文档里引用的每个审计事件名**在源码里真的存在——本文已纳入该门禁的检查范围。

---

## 附录 D：实机发现清单（待回填）

跑完把发现按这个格式回填。**格式与 `docs/realmachine-regression-test.md` 的附录 D 一致**，方便日后合并。

| # | 卡片 | 现象 | 级别 | 状态 |
|---|---|---|---|---|
| D-1 | M01 | `ui_inspect` 自己的窗口 → 应用被 WebKit 的 SIGTRAP 断言带走 | **真缺陷** | 🔧 **已修**（本轮）· 待实机复验 |

**级别口径**（沿用兄弟文档）：

- **真缺陷**：真机跑出来的、与卡片判据矛盾的行为
- **观察级**：行为符合预期但设计上别扭、或无法判定
- **已排除**：实机否证了卡片的前提或判据 → **要改卡片**，不是改代码

---

### D-1（真缺陷 · 已修）`ui_inspect` 自己的窗口 → 应用当场崩掉

**发现时间**：2026-10-23 21:12（跑 M01 时）
**发现人**：实机跑卡
**级别**：真缺陷（整进程退出，不是报错不是卡顿）
**是否本批回归**：**否**——UI 六件套是 10-21 的 `d2674e33` 引入的，`list_windows` 从那时起就没排除过自己；今天这批卡片把它踩出来了

#### 现象

跑 M01（危险动作闸按小写匹配）时，应用崩了。崩溃报告：
`~/Library/Logs/DiagnosticReports/wmessage-2026-10-23-211232.ips`

```
exception: EXC_BREAKPOINT / SIGTRAP
termination: Trace/BPT trap: 5
legacyInfo.threadTriggered: tokio-rt-worker        ← 崩溃发生在后台线程
```

崩溃栈（自下而上）：

```
bot::registry::call_ui_inspect
  └ bot_ui::tool_ui_inspect
      └ ui::inspect_tree
          └ ui::macos::collect
              └ ui::macos::collect
                  └ ui::macos::children
                      └ ui::macos::copy_attr          ← 读 AXChildren
                          └ AXUIElementCopyAttributeValue
                              └ AppKit 无障碍
                                  └ WebKit::WebViewImpl::accessibilityAttributeValue
                                      └ WebKit::crashDueToApplicationCallingMainThreadOnlyWebKitAPIFromBackgroundThread()
                                          └ WTFCrashWithInfoImpl → SIGTRAP
```

#### 根因

三件事撞在一起：

1. **`list_windows` 把 wmessage 自己列进去了**——`running_apps()` 枚举所有 `Regular` 策略的 GUI 应用，wmessage 自己就在里面（macos.rs 的注释还写着「无标题的窗口（面板、托盘窗）也列出来，但标注出来让模型自己挑」）。模型因此拿到了自己窗口的 id。
2. **AX 树遍历走进了本进程的 webview**——`collect` 递归读 `AXChildren`，走到 wmessage 窗口里的 **WKWebView** 时，那个元素的辅助功能属性是**本进程内**的 WebKit 实现的。
3. **这条 API 只能在主线程调**——控件树是在 `spawn_blocking` 的后台 tokio 线程上读的，WebKit 直接断言失败 → SIGTRAP。

**为什么 Rust 的 `catch_unwind` 拦不住**：`bot::registry` 给工具调用包了 `catch_unwind`（崩溃栈里能看到 `catch_unwind` / `unwind_safe` 帧），但 SIGTRAP 是**信号**，不是 Rust panic——`catch_unwind` 只管 panic，信号会直接带走整个进程。

**为什么这不是「顶多报个错」**：整进程退出 = 用户正在的会话、正在跑的服务、内存里的在途状态全部没了。

#### 修复

规则：**UI 自动化不能操作本进程自己的窗口**。两道闸，都在共享层 `src-tauri/src/ui/mod.rs`（两平台一次生效，不改平台后端）：

| 闸 | 位置 | 作用 |
|---|---|---|
| `without_own_windows` | `list_windows` | 清单里**剔掉**自己的窗口 → 模型看不见就不会碰 |
| `reject_own_window` | `inspect_tree` / `press_node` / `focus_node` / `activate_window` / `window_is_foreground` | 拒收自己 pid 的 id → 模型**自己编 id**、或拿上一轮的旧 id 来，也一样被拒 |

拒绝文案：`不能操作 wmessage 自己的窗口——UI 自动化只用于接管外部应用`

拦下来不只是为了不崩：机器人操作自己的界面没有意义——这条能力是替用户操作**外部**应用。

#### 回归锁

| 测试 | 钉住什么 |
|---|---|
| `ui::tests::own_window_must_not_be_automated` | 拦截本身拦得住（自己的窗口 id 拒、外部窗口放行、畸形 id 不误伤） |
| `ui::tests::list_windows_must_not_include_our_own_windows` | 清单里剔掉自己的窗口 |
| `ui::tests::entry_points_must_reject_our_own_window` | **接线守卫**：五个入口都真的调了拦截——「定义了没人调等于没有」（`ensure_onion_config_columns` 那个坑） |

**变异复验**（都跑过，期望红 → 还原后绿）：删掉 `inspect_tree` 的 `reject_own_window` 调用 → `entry_points_must_reject_our_own_window` 红（报「inspect_tree 必须调 reject_own_window——定义了没人调等于没有」）；把 `Ok(without_own_windows(wins))` 改成 `Ok(wins)` → 同一条红（报「list_windows 必须把本进程自己的窗口剔掉」）。

#### 待办

- [ ] **实机复验 M39**：清单里应看不到 wmessage 自己；碰自己的窗口应被拒且应用不崩
- [ ] **Windows 侧确认**：Windows 的 `list_windows` 同样会列自己（UIA 桌面根元素的顶层窗口），规则已覆盖，但 Windows 上没验过读自己的 WebView2 树会怎样

#### 顺带发现（未修）

- **`list_windows` 的文案与实现分家**：工具 schema（`bot/registry.rs`）说「只列有标题的窗口」，空清单时的提示也这么写；但 macOS 后端**把无标题窗口也列出来了**（注释明说「无标题的窗口也列出来，但标注出来让模型自己挑」），只有 Windows 后端跳过无标题窗口。这条会误导模型去选面板/托盘窗（M02 / M06 两张卡都要靠无标题控件）。
- **权限不全时模型会烧轮次**：辅助功能与屏幕录制两项权限缺一时，工具一路报错，模型从 turn=0 烧到 turn=21 才停。§1.1 已加前置条件，但产品侧也许该在权限缺失时**一次性说清缺哪两项**。

---

## 附录 E：与兄弟文档的分工

| 文档 | 覆盖窗口 | 侧重 |
|---|---|---|
| `docs/realmachine-regression-test.md` | 2026-10-19 ~ 10-22 | 32 张双平台场景卡 + 9 张 Windows 专章；本文的**写法来源** |
| `docs/realmachine-rerun-2026-10-22.md` | 同上（换 exe 后复验） | 复验手册：跑法、分段顺序、结果回填口径 |
| `docs/realmachine-overflow-test.md` | 溢出回灌 | 工具大结果溢出回灌的边界三连 |
| `docs/realmachine-mutation-test-2026-10-23.md`（本文） | 2026-10-23 | 39 张双平台卡 + 3 张 Windows 专章；**每张卡带变异注入锚点**；跑卡时踩出的崩溃进附录 D |

**重叠的地方**：M18 / M19 与兄弟文档的 S33 / S34 交叉（那两张验行为，本文这两张验变异）。**不重复的部分以本文为准**——兄弟文档里那两张卡没写变异锚点。

**单位测试层的对应物**：`scripts/mutation-trace-funnel.sh`（注入 → 跑 `cargo test` → KILLED / SURVIVED）。本文是**真机层**的同构物：注入 → 跑真实场景 → 看判据是否 FAIL。
