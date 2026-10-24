# 真机变异测试方案：2026-10-24 变更回归

针对 **2026-10-23 20:02 → 2026-10-24 10:51** 这批改动的实机验收方案。**macOS 与 Windows 双平台**，两平台的测法有实质差异，见 §1.0。

与 `docs/realmachine-mutation-test-2026-10-23.md`（覆盖 10-23 00:00 → 20:02）**首尾相接、不重叠**：上一份验「10-23 白天那批」，这份验「10-23 夜里到 10-24 上午这批」。两份的骨架同构——**每张卡都写清「把 bug 注回去，重跑应当 FAIL」的锚点**，证明这套测试有牙齿。

| 项 | 值 |
|---|---|
| 覆盖窗口 | `cd060bcd`（10-23 18:24）→ `ec8eee30`（10-24 10:51），**51 个提交 / 84 个文件 / +7 008 −832 行** |
| 基线 | `cd060bcd`（上一份文档窗口的最后一笔提交） |
| 行为变更 | 41 条（`fix` 29 / `feat` 8 / `chore` 3 / `style` 1）；其中 **4 条 UI 修复已在上一份的附录 D 覆盖并实机复验**（D-1~D-4）→ 本份不重复出卡，4 条非行为变更进附录 A |
| 卡片 | **38 张场景卡**：M40–M74（35 张双平台为主）+ W04–W06（3 张 Windows 专章），覆盖 33 条行为变更 |
| 变异注入 | **50 条（V26–V75）**，总表见 §七（编号接上一份的 V01–V25） |
| 配套探针 | macOS/Linux → `scripts/regression-probe.sh`；**Windows → `scripts/regression-probe.ps1`**（与上一份同一套） |
| 兄弟文档 | `docs/realmachine-mutation-test-2026-10-23.md`（10-23 白天）· `docs/realmachine-regression-test.md`（10-19~10-22）· `docs/realmachine-rerun-2026-10-22.md`（复验手册）· `docs/realmachine-overflow-test.md`（溢出漏斗）· `docs/SUBAGENT-EFFICIENCY-TRANSPARENCY-DESIGN-2026-10-24.md`（本批子 agent 三条设计）· `docs/MCP-ON-DEMAND-LIFECYCLE-DESIGN-2026-10-24.md` · `docs/SKILL-ENABLE-TOGGLE-DESIGN-2026-10-24.md` |

---

## 〇、这份文档是什么

**是**：一份「怎么在真机上证明 10-23 夜里到 10-24 上午这批改动是对的」的逐项清单。每一项给一条可直接粘贴的提示词（或一个可复制的 UI 操作）、一个能判 PASS/FAIL 的观测点、以及一次变异注入复验（把 bug 注回去，应当 FAIL）。

**不是**：不是单测的替代，也不是「再跑一遍功能」。单测证明纯函数在被喂进特定入参时表现正确；这里撬的是**跨模块接缝**——单测结构性够不到的地方：

| 接缝 | 坏了会怎样 | 单测能不能看见 |
|---|---|---|
| 技能 frontmatter ↔ 意图路由表 ↔ 启动预审 | 关掉的技能仍被广告给模型，用户消息仍被劫持进它 | 不能（路由表是进程级全局状态，关掉后要立刻重建才看得出） |
| 进程状态 ↔ 真实子进程 | 面板说「已启用」，实际没进程；模型看不见工具 | 不能（子进程生命周期只在真机上发生） |
| DB 写锁 ↔ 长事务窗口 | 主窗导入持锁时，用户的应答被静默丢掉 | 不能（要真造出长事务窗口） |
| tokio worker ↔ 阻塞磁盘 IO | 一个大文件读入把整个 worker 占住，所有异步任务一起卡 | 不能（要真跑出这个并发窗口） |
| 前台判定 ↔ 确认弹窗 | 弹窗自己抢走前台，注入打进了弹窗 | 不能（只有真机上弹窗真弹出来才发生） |
| 定时作业 ↔ 墙钟 | 暂停数日的作业恢复瞬间补跑一次 | 不能（要真等一个保鲜窗口） |
| 导出文件名 ↔ 本地时区 | UTC+8 晚 8 点后，文件名日期比实际大一天 | 不能（时区只在本机成立） |

**选样原则**：只收「错了会静默发生、且当场看不出来」的项。纯文档对拍、纯门禁脚本、纯格式化不在此列（它们归 `bash scripts/test-fast.sh` 管）。今天 51 个提交里，10 条是文档、4 条不改运行时行为 → 这 14 条进附录 A。剩下的 33 条行为变更落进 38 张卡——**一个提交可以拆成多张卡**（`efd5d481` MCP 按需启停拆成 M50 / M51 / W04 三张；`2283fb3d` 动作流拆成 M59 / M60 两张），**两张卡也可以合一条提交**（M73 覆盖 `286720db`，M74 覆盖 `d6766145`，两者同属「口径单源」这条线）。

> **上一份已经验过的四条不在这里重复出卡**：`895c31b1`（不许操作自己的窗口，上一份的 M39 / D-1）、`14f99201`（原生密码框按 AXSubrole 认，D-3）、`81b62c27`（聚焦改置 AXFocused，D-2）、`e45c8736`（控件树深度硬顶 16，D-4）——四条都已实机复验过，见上一份的附录 D。**复跑时不必重跑这四条**；但如果换过 exe，按上一份手册的顺序先复验它们。

**与「单测变异脚本」的关系**：仓库里已有 `scripts/mutation-trace-funnel.sh`——那是**单元测试层**的变异（注入 → 跑 `cargo test` → 看 KILLED / SURVIVED）。本文是**真机层**的同构物：注入 → 跑真实场景 → 看判据是否 FAIL。两者互补：单测变异能证明「有约束在守」，真机变异能证明「约束在真实环境里真的生效」。

---

## 一、前提条件与通用判据

### 1.0 Windows 实机适配（先读这一节）

Windows 上有五处平台差异会**直接改写测法**，不是「注意一下」的程度。

#### ① 数据目录可能不在 `D:\Program Files\...`

便携包的 `data_dir` 判定顺序（`src-tauri/src/paths.rs`）：exe 同目录可写 → 锚定 exe 目录；不可写（如 `Program Files`）→ 回退 `%APPDATA%\com.renshi.wmessage`，并记一条 WARN `data_dir_fallback`。

> **第一件事：确认数据到底落在哪。**
> ```powershell
> powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "D:\Program Files\wmessage-portable-2026-10-24"
> ```
>
> **本批卡片的判据几乎全落在数据目录里**（`bot.log` / `wmessage.db` / `runtime/flags/` / `checkpoints/` / `mcp_tool_cache` / `builtin_skills/`）——**不知道 `data_dir` 在哪，后面所有观测都读错地方**。

#### ② 端口闸在 Windows 上**不生效**——这不是 bug

`listening_supported()` = `cfg!(linux|macos)`。Windows 上没有进程快照与监听 socket 的采集路径，所以面板会明说「本平台暂不支持监听端口采集」。

**本批没有卡依赖端口闸**，但 M70 / M71（阻塞线程）要用「应用还活着吗」当反证——Windows 上改用任务管理器看 CPU 与「应用无响应」状态。

#### ③ 停止阶梯走 `taskkill`

Windows 上没有 unix 那套信号语义，停止走 `taskkill /PID <pid> /T /F`。**本批没改停止阶梯**，但 W04（MCP 子进程回收）的地面真相要用 `Get-Process` / `tasklist` 对照——**子进程树是否真被收掉**，在 Windows 上要看 `/T`（树杀）有没有生效。

#### ④ 技能物化目录在 Windows 上是 `%APPDATA%` 下的兄弟目录

`builtin_skills/` 是用户技能目录 `skills/` 的**兄弟**（不是子目录）。Windows 上如果数据目录回退到 `%APPDATA%\com.renshi.wmessage`，物化副本就在那里——M48 的清理验证要在**回退后的真实目录**里看，不能照 macOS 的路径猜。

#### ⑤ 其余平台差异速查

| 项 | macOS / Linux | Windows |
|---|---|---|
| 键盘 / 鼠标注入 | AX（辅助功能）+ CGEvent | UIAutomation + SendInput（**UIPI 会拦**） |
| 窗口 id | `pid:AXWindows下标`（**下标会复用**） | `pid:hwnd`（hwnd 不复用） |
| 前台判定 | 两级（应用 pid + 窗口标题），标题读不到就退化为「同应用即可」 | `GetForegroundWindow() == hwnd`（精确匹配） |
| 控件当前值 | AXValue | UIA ValuePattern |
| 密码框识别 | 角色 `AXSecureTextField` + **AXSubrole**（上一份 D-3 修的） | UIA `CurrentIsPassword` |
| MCP 子进程回收 | 进程组信号 | `taskkill /T`（W04 专验） |
| 文件权限位 | unix 权限位生效 | **没有 unix 权限位** |
| 产物「来自互联网」标记 | 不存在这个概念 | `Zone.Identifier` 备用数据流 |

### 1.1 环境

| # | 条件 | 怎么确认 |
|---|---|---|
| 1 | 应用跑起来了 | macOS：`npm run tauri dev`（**必须用 dev**：有几条诊断走 stderr，Finder 启动的 .app 会把 stderr 丢进系统日志）。Windows：绿色包双击（**stderr 在 GUI 启动下同样看不到**） |
| 2 | bot 已启用 | 任务卡绑定目录 + `bot-enabled.flag` 存在 |
| 3 | 探针能找到日志 | macOS：`bash scripts/regression-probe.sh locate`；**Windows：`powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "..."`** |
| 4 | 每个场景前打水位线 | `mark` → 场景后 `report`（两平台同名子命令） |
| 5 | **本批额外**：已配厂商（LLM 可用） | M45 / M46 / M52 / M58 / M62 / M68 都要真发请求 |
| 6 | **本批额外**：有一个已装的外部 MCP 服务器 | M50 / M51 / W04（三张卡都要真起一个子进程） |
| 7 | **本批额外**：至少一个自建技能 | M47 / M48 / M49（`pptx-design` 已退役，见 M48） |
| 8 | **macOS 两项系统权限**（跑 M40 / M41 / W04 之前先给） | 「系统设置 → 隐私与安全性 → **辅助功能**」与「**屏幕录制**」。**缺辅助功能** → `list_windows` 报「本进程尚未获得 macOS「辅助功能」权限」 |

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
| 允许机器人操作桌面界面 | M40 / M41 / W04 | 被拒：`UI 自动化未开启：请到 设置页 打开「允许机器人操作桌面界面」后再用这些工具。` |

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
| **地面真相** | macOS：`ps` / `lsof` / `ls -l` / `sqlite3`；Windows：`Get-Process` / `tasklist` / 资源管理器 | 进程、端口、文件、库里的行 |

**铁律：应用自报不算数。** 面板说「已启用」不算，要看进程真的没起来；面板说「没有运行记录」不算，要看库里真的没行；模型说「已输入」不算，要看目标框真的进了字。这批改动里有一半的 bug 形态就是「自报与事实分家」。

### 1.5 变异注入的通用流程

**单测层**的注入脚本见 `scripts/mutation-trace-funnel.sh`（判定词：KILLED / SURVIVED / BUILD_ERR / NO-OP）。**真机层**的流程如下：

```bash
# 0. 工作区必须干净（还原会覆盖真改动）
git status --short            # 应当无输出

# 1. 定位锚点（行号会漂，先 grep 别硬写行号）
grep -n "fn clamp_max_depth" -A 12 src-tauri/src/bot_ui.rs

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

### 1.6 判据里最容易踩的三个坑

1. **`spawn_subagent` 的回执是纯文本，不是 JSON。** 必须逐字核对：`生效预算：墙钟 {}s / 轮数 {} / 工具调用 {}。`
2. **`read_own_card` 的返回是 camelCase JSON**：`budgetUsage` 一块里含 `toolCallsUsed` / `toolCallsMax` / `roundsUsed` / `roundsMax` / `wallRemainingSecs`；**静态 `budget` 键已退役**（上一份的 M21 / M22 记的是旧结构，别照旧预期判）。
3. **MCP 状态有五态不是三态**：`connected`（调用中）/ `idle`（静默待命）/ `pending`（待发现工具）/ `down`（失败）/ `absent`（进程内无槽位）。上一份的卡里若写了「启用后应该是 connected」，那是旧口径——本批启用后**应该是 idle**。

---

## 二、场景总表

按「错了会怎样」分档，不按模块分。

### T0 档：安全边界与数据破坏（5 项）— 错了会真实造成损害

| # | 场景 | 提交 | 关键观测 | 平台 |
|---|---|---|---|---|
| M40 | 敏感词 `pin` 改整词匹配 | `d2efb15f` | Spinner / typing 不被误拦；真 PIN 照拦 | 双 |
| M41 | 前台校验锁内复检 + 置前与注入并锁 | `3efa1e25` | 确认弹窗抢前台后注入不落错窗口 | 双 |
| M42 | 程序 id 收白名单 | `009e811f` | `.` 被拒；`apps/` 根目录完好 | 双 |
| M43 | 工作区导入补路径闸与字节上限 | `7d66f8a6` | 非 `.json` / 超限文件被拒 | 双 |
| M44 | 任务卡补标签双上限（三写口同源） | `d86656c1` | 11 个标签 / 31 字标签被拒 | 双 |

### T1 档：静默失效与状态谎报（13 项）— 最阴的一类

| # | 场景 | 提交 | 关键观测 | 平台 |
|---|---|---|---|---|
| M45 | 截止时间三写口归一 | `ec0e25c7` | 空格分隔格式被拒 | 双 |
| M46 | 非法列值响亮拒绝 + 完成三件套 | `20187e41` | `column: "x"` 被拒；完成卡清机器人接管标记 | 双 |
| M47 | 技能启用开关（frontmatter 单源） | `ec96469e` | 关掉后不进清单 / 不产路由 / 启动即拒 | 双 |
| M48 | 内置 pptx-design 退役 + 残留副本清理 | `afe580ac` | 技能列表里没有僵尸 `pptx-design` | 双 |
| M49 | 超时清扫覆盖死会话的 Running | `921f3477` | 死会话不再占位同名技能 | 双 |
| M50 | MCP 按需启停：启用不拉起子进程 | `efd5d481` | 启用后无进程；状态是 `idle` | 双 |
| M51 | MCP 工具清单落盘快照 + 删除清快照 | `efd5d481` | 无连接也能挂上工具；删服务器后快照清空 | 双 |
| M52 | 子 agent 工具预算触顶改收尾窗 | `ffc0487c` | 触顶后拒读放行写；产物真落盘 | 双 |
| M53 | 删除与导入落库必广播 + 补审计 | `20165f0e` | 导入后界面立刻出新卡；审计有 `tasks_import` | 双 |
| M54 | 重读任务并入 mutate 串行链 | `af1d959c` | 导入完成后新卡不被在途写盖掉 | 双 |
| M55 | 浮窗 Esc 层级裁决 | `a808778c` | 命令面板开着时 Esc 不关任务浮窗 | 双 |
| M56 | 运行记录 / 版本读取失败不伪装空库 | `607e0fe6` | 读失败显示「读取失败」而不是「还没有记录」 | 双 |
| M57 | 问答应答与提问落库持写锁 | `e3604e71` | 长事务窗口内应答不丢 | 双 |

### T2 档：可感知语义（17 项）

| # | 场景 | 提交 | 关键观测 | 平台 |
|---|---|---|---|---|
| M58 | 契约卡回实时余量 | `b270735d` | `budgetUsage` 有实时数；`budget` 键退役 | 双 |
| M59 | 右侧栏动作流实时上屏 | `2283fb3d` | 工具调用逐条上屏；return 收口 call 行 | 双 |
| M60 | 子 agent 行补工具调用次 / 预算 | `2283fb3d` | 行上出现 `47/100 次` | 双 |
| M61 | token 用量记账三列 | `95b3ea90` | 终态行三列有值；老行为 NULL | 双 |
| M62 | 预算纪律三条红线进通用底座 | `329ee3f6` | 读文件一次读完；产物早落盘 | 双 |
| M63 | 重试加保鲜窗口 | `6f06f6ed` | 过期重试丢弃，不补跑 | 双 |
| M64 | 防重入拒绝补审计 | `edf3c763` | 被防重入拒绝时有 `sched_job_skip` | 双 |
| M65 | 错过的作业补发 sched-status | `b88238a5` | 徽标即时翻「已错过」 | 双 |
| M66 | 完成判据归一到 isTaskCompleted | `0bdbebfb` | 遗留 done 卡显示「已完成」 | 双 |
| M67 | 导出文件名日期换单源 dayKey | `42262c9f` | 晚 8 点后文件名日期是本地日期 | 双 |
| M68 | 探活文案分无厂商 / 厂商两条 | `331f7ad2` | 不再出现「厂商「」」 | 双 |
| M69 | 运行记录四个写口持写锁 | `25ed736b` | 跑成功的程序不因回写失败报错 | 双 |
| M70 | 六个读命令进阻塞线程 | `f2ee3f7b` | 大库下界面不卡 | 双 |
| M71 | 保留期清扫与批量落库进阻塞线程 | `07280b91` | 长事务期间 worker 不被拖住 | 双 |
| M72 | 查询 schema 删掉工作流 / 定时残留承诺 | `6b441c16` | schema 不再承诺不产出的标记 | 双 |
| M73 | 聊天任务池过滤换 isActiveTask 单源 | `286720db` | 回收站软删卡不进待办列表 | 双 |
| M74 | 工作区三个写口补置线程标记 | `d6766145` | 闭包内契约断言不误判 | 双 |

### Windows 专章（3 项）

| # | 场景 | 提交 | 关键观测 |
|---|---|---|---|
| W04 | MCP 按需启停在 Windows 上：调用完即回收 | `efd5d481` | 子进程树真被 `taskkill /T` 收掉 |
| W05 | 导入路径闸在 Windows 上（Program Files + 路径形态） | `7d66f8a6` `20165f0e` | 导出侧拒的路径导入侧同样拒 |
| W06 | 导出文件名日期跨时区 | `42262c9f` | 晚 8 点后文件名日期 = 本地日期 |

---

## 三、T0 档：安全边界与数据破坏

### M40 · 敏感词 `pin` 改整词匹配

**靶点**：`src-tauri/src/bot_ui.rs::gate_sensitive`（提交 `d2efb15f`）

**bug 形态**：敏感词表里的 `pin`（支付密码）用**子串**匹配。子串会把 `Spinner` / `shipping` / `typing` / `Spinning wheel` 这类**普通控件**判成密码框 → 机器人对这些框一律拒填。

> 这是**反向形态**的 bug：闸太宽 → 功能瘫掉，而不是闸太松 → 危险动作漏放。但同一条子串规则在危险词侧已经踩过坑（`ok` 命中 `Outlook` / `Token窗口`），所以这次的修法是**给 `pin` 加整词豁免**，与 `DANGEROUS_WHOLE_WORDS` 同款。

本次提交同时把**确认行为写进工具 schema**（`ui_click` 的「没有标题的图标按钮（纸飞机/垃圾桶）前会弹窗请用户确认」与 `ui_type` 的「往「发送 / 提交」类输入框打字同样要先经用户确认」）——模型看不见的闸等于没有闸。

**步骤**：

1. 打开一个**带 Spinner / typing / shipping 之类字样的界面**（macOS：Safari 打开一个含这些词的测试页；Windows：记事本里把这三个词打出来当控件标题不现实，改用浏览器控制台 DOM 里造三个同名按钮）。
2. 摸出目标控件 id（`ui_inspect`）。
3. 在聊天框发：

```
先对当前窗口做 ui_inspect，把标题里含 Spinner 的控件 id 找出来报给我；然后用 ui_type 往这个控件里输入字符串 变异测试 2026-10-24。
```

4. 再对 `typing` 与 `shipping` 各做一次（同一条消息流程，只换关键词）。

**判据**：

- PASS：三个控件**都不被拒**（工具返回「已输入 N 个字符到「…」」），目标框里出现输入的字符串。
- FAIL：返回 `拒绝操作：目标控件是敏感控件（…），不代填密码 / 验证码类内容。`——**普通控件被当密码框**。
- **反向必做**：造一个标题**恰好是** `PIN`（整词）的输入框 → 必须**被拒**（`gate_sensitive` 拦的是密码框，整词豁免不能把真 PIN 放过去）。
- **schema 副判据**：让模型报出 `ui_click` 与 `ui_type` 的工具说明 → 说明里必须含「没有标题的图标按钮」与「往「发送 / 提交」类输入框打字同样要先经用户确认」两句。

**变异注入复验**：把 `SENSITIVE_WHOLE_WORDS` 里的 `"pin"` 删掉（回到子串匹配）→ 重跑应 **FAIL**（Spinner 被误拒）。
单测侧同款变异已覆盖：`bot_ui::tests::sensitive_pin_matches_whole_word_only` → 变红（该测试同时钉住「Spinner / shipping / typing 不拦」与「整词 PIN 照拦」两半）。

> **反向变异**：把 `contains_word(&hay, k)` 改成 `true` → 所有敏感词都会命中 → 同一条单测的「整词 PIN 照拦」那半变红，真机上表现为**所有输入框都拒填**。

**平台**：跨平台（判定在 `bot_ui.rs` 公共层，两平台共用同一份 `SENSITIVE_KEYWORDS`）。

---

### M41 · 前台校验锁内复检 + 置前与注入并锁

**靶点**：`src-tauri/src/bot_ui.rs::tool_ui_type` / `tool_activate_window`（提交 `3efa1e25`）

**bug 形态**：两个洞叠在一起。

**洞一：前台校验发生在锁外。** 流程是「入口处前台校验通过 → 等确认弹窗（最长 60s）→ 截图 → 窗口复核 → 等注入锁 → 注入」。**每个 await 点之间用户都可能切走前台**。入口那道校验只负责快速失败，真正作数的那一道必须落在**注入锁内**。

**洞二：置前与注入不共锁。** `activate_window` 与注入走两把不同的锁——并发会话 A 调 `activate_window` 把窗口 X 置前，正好落在会话 B 的「前台校验通过 → 注入」之间 → **B 的键盘事件注进了刚被 A 置前的窗口 X**（B 以为自己在往窗口 Y 里打字）。

**步骤**：

1. 准备**两个窗口**：目标窗口（例：文本编辑器）+ 干扰窗口（例：浏览器）。
2. 触发一次**危险动作确认弹窗**（点一个标题含「删除」的按钮 → 弹窗出现）。
3. **在弹窗出现后**，把前台切到干扰窗口（点一下干扰窗口）。
4. 在弹窗上点「确认」（或按回车确认）。
5. 观察注入落点。

**判据**：

- PASS：注入被**锁内复检**挡住 —— 返回 `目标窗口不在前台：键盘注入会落进当前正在使用的应用（前台不是目标窗口）。先调 activate_window 把目标窗口置前，再输入。`，**干扰窗口里一个字符都没多**，目标框也是空的。
- FAIL：文字出现在**干扰窗口**（不是目标窗口）里——注入跑到了被置前的另一扇窗。
- **正向补充**：先 `activate_window` 目标窗口（并保持它在前台）→ `ui_type` 成功，返回 `已输入 N 个字符到「{title}」（控件 {id}）`。

**变异注入复验**：
- 删掉 `tool_ui_type` 注入锁内的 `foreground_verdict(crate::ui::window_is_foreground(&wid))?;` 这一行 → 重跑应 **FAIL**（文字打进前台应用）。
- 把 `tool_activate_window` 里的 `with_inject_lock(|| crate::ui::activate_window(&wid))` 改成不取锁（直接调）→ 重跑应 **FAIL**（并发置前与注入互相踩）。

单测侧同款变异已覆盖：`bot_ui::tests::injection_must_wait_for_the_target_window_to_come_to_front`（三态全测：Ok(true) / Ok(false) / Err）→ 变红。

**平台**：跨平台，但**判定口径完全不同**：

- **macOS**（`ui/macos.rs`）：macOS **没有公开的「哪个窗口在前台」标志**，只能两级——应用 pid + 同应用 `AXFocusedWindow` 标题。**标题读不到或目标窗口标题为空 → 退化为「同应用即可」**。所以**同应用多窗口**（微信开两扇窗）时只有标题能区分。
- **Windows**（`ui/windows.rs::is_foreground`）：`GetForegroundWindow() == target hwnd`，**同一个 hwnd 精确匹配**，比 macOS 严格。

> **Windows 上验这张卡必须让应用与目标窗口同权限级别**（都用普通权限），否则先撞 UIPI 而不是前台闸。

---

### M42 · 程序 id 收白名单

**靶点**：`src-tauri/src/apps/manage.rs::validate_app_id`（提交 `009e811f`）

**bug 形态**：旧校验是**黑名单**（空 / 含 `/` / 含 `\` / 含 `..` 就拒）。但 `join` 只对「恰好一个普通分量」的 id 才不会走出 `apps/`——**单个点号 `.` 是合法的路径分量**，`apps_root.join(".")` 直接折叠回 `apps/` 根。

后果链：`apps_root.join(id)` 折叠到根 → 归档后的**彻底删除**（`remove_dir_all`）就变成**对整个 `apps/` 根的删除** → **用户所有程序一次性全没**。

> 修复方向是**白名单**而不是加一条黑名单：`[A-Za-z0-9_-]+`——空格、非 ASCII、`:` 全部拒掉（`:` 在 Windows 上还是盘符分隔的原料）。

**步骤**：

1. 设置页 →「程序」页 → 上架一个新程序，**id 填一个点号** `.`（或用能调 Tauri 命令的 HTTP 侧直接打 `apps_create`）。
2. 再上架一个正常 id 的程序（如 `wage`），确认它真的建出来了。

**判据**：

- PASS：`.` 被拒，错误文案是 `id 只能是字母/数字/`-`/`_`（单个目录分量）`；**`apps/` 目录完好**——`wage` 那个程序还在，目录结构没被折叠。
- FAIL：`.` 被接受（库行落在 id=`.` 上），且后续归档 + 彻底删除会把 `apps/` 根一起删掉——**所有程序消失**。
- **负向（必做）**：正常 id（`wage` / `pdf-merge-2` / `hello_world`）必须放行。
- **地面真相**：`ls <data_dir>/apps/`（Windows：`dir <data_dir>\apps\`）——**目录里是程序目录，不该出现 `.` 或空目录**。

**变异注入复验**：把 `validate_app_id` 里的白名单判定改成 `let ok = !id.is_empty() && !id.contains('/') && !id.contains('\\') && !id.contains("..");`（回到黑名单）→ 重跑应 **FAIL**（`.` 被接受）。
单测侧同款变异已覆盖：`apps::manage::tests::app_id_rejects_non_component_values` 与 `create_rejects_dot_id_without_touching_root` → 两条同时变红。

> **正向变异**：把白名单收成 `[A-Za-z0-9]+`（连 `-` / `_` 一起拒）→ `app_id_rejects_non_component_values` 里 `pdf-merge-2` / `hello_world` 那两行变红。

**平台**：跨平台（判定在纯函数里，与文件系统无关）。**但后果在 Windows 上更狠**——`remove_dir_all` 在 Windows 上删不掉占用中的文件，会留半残状态。

---

### M43 · 工作区导入补路径闸与字节上限

**靶点**：`src-tauri/src/db/workspace.rs::workspace_import` + `src-tauri/src/db/tasks.rs::read_import_bounded`（提交 `7d66f8a6`）

**bug 形态**：`tasks_import`（任务数据导入）一直有**路径闸**（只许 `.json`、不许 `..` 组件）与**字节上限**（64 MiB）；但 `workspace_import`（工作区链接导入）是**裸读**——`std::fs::read_to_string(&path)`，路径随便选、大小不限。

后果：**导出侧拒的路径，导入侧照读**。这条闸设计上的不对称就是漏洞本身——用户能选到 `~/.ssh/id_rsa`、浏览器 profile 里的 cookie 库、任何大文件，而界面上装成「导入工作区链接」。

**步骤**：

1. 设置页 →「任务数据管理」或工作区相关入口 → 导入。
2. 选一个**不是 `.json`** 的文件（比如 `~/.bash_history` 或 `~/notes.txt`）。
3. 再造一个**超过 64 MiB 的 `.json`** 文件（`python3 -c "open('big.json','w').write('a'*70*1024*1024)"`）试着导入。
4. 最后选一个**合法的 `.json`** 工作区文件导入，确认正常路径仍然通。

**判据**：

- PASS：非 `.json` 被拒，文案 `必须是 .json 文件`；超限文件被拒，文案 `导入文件超过大小上限（67108864 字节）`；**合法文件照常导入成功**。
- FAIL：非 `.json` 被读进内存（界面报的是「不是有效的工作区链接 JSON」这类**解析错**而不是路径闸），或超限文件被整读进内存（应用卡死 / 内存暴涨）。
- **地面真相**：应用内存占用在导超限时**不应出现尖峰**。

**路径闸不止扩展名这一条**（`check_export_path` 的六个分支都要能拒）：路径为空 / 路径不允许包含 `..` / 必须是 `.json` 文件 / 路径必须以文件名结尾 / 父目录不存在或无法解析 / 目标路径是符号链接已拒绝。**符号链接那条在 macOS 与 Linux 上尤其重要**——导入路径若是指向别处的软链，闸要 fail-closed 拒掉。

> **判据的措辞**：超限时报的是「**大小上限**」而不是「编码错误」——`Take` 截断可能切断 UTF-8 码点边界，直接 `read_to_string` 会把超限文件误报成编码错误。这条改动的价值之一就是把两种失败分开了。

**变异注入复验**：
- 删掉 `workspace_import` 里的 `crate::db::tasks::check_export_path_gate(path.clone()).await?;` → 重跑应 **FAIL**（非 `.json` 被读进去）。
- 把 `read_import_bounded` 里的 `if buf.len() as u64 > max_bytes` 改成 `if false` → 重跑应 **FAIL**（超限文件整读进内存）。

单测侧同款变异已覆盖：`db::tasks::import_bounded_tests::bounded_read_rejects_over_limit_instead_of_truncating` → 变红。

**平台**：跨平台（闸在 Rust 侧，与平台无关）。**路径形态的差异见 W05。**

---

### M44 · 任务卡补标签双上限（三写口同源）

**靶点**：`src-tauri/src/db/tasks.rs::apply_task_patch` 的 `tags` 分支（提交 `d86656c1`）

**bug 形态**：任务卡标签有**两个上限**（`MAX_TAGS = 10` 个、`MAX_TAG_LEN = 30` 字）。bot 工具写口与 HTTP 写口都收着这两个上限，但 **UI 主写口（`task_patch`）零校验**。

后果：**同一行数据在三个写口有两套标准**。用户从 UI 给一张卡贴了 15 个标签 → 库里存下来了 → 随后机器人用 `edit_task` 改这张卡的任何字段时，**因为携带了超限标签而被同一常量拒绝**——「刚才还好好的卡突然改不动了」。反过来，机器人写不进去的标签，UI 能写进去。

空数组语义也不同源：bot / HTTP 两写口的约定是「**空数组 = 清除**」，UI 写口此前把 `[]` 当成「设成空数组」存下去。

**步骤**：

1. 在任务页打开一张卡 → 详情浮窗 → 标签编辑 → **贴 11 个标签**（每个 1 字）。
2. 存。
3. 再给某张卡贴一个 **31 字的单个标签**，存。

**判据**：

- PASS：11 个标签被拒，文案 `标签最多 10 个`；31 字标签被拒，文案 `单个标签最长 30 字`；**库里没有落进超限数据**（用 `sqlite3 <data_dir>/wmessage.db "select tags from tasks where id='…'"` 查）。
- FAIL：超限标签被静默存进库 → 随后用机器人 `edit_task` 改这张卡**被拒**。
- **必做**：标签清空（UI 提交空数组）→ 库里 `tags` 应为 **NULL**（不是 `[]`）。

**变异注入复验**：把 `apply_task_patch` 的 `tags` 分支里两处上限检查删掉 → 重跑应 **FAIL**（11 个标签存进库）。
单测侧同款变异已覆盖：`db::tasks::task_patch_tests::task_patch_tags_enforces_shared_limits` → 变红（该测试同时钉住上限与空数组清除两半）。

> **反向变异**：把 `if task.tags.as_ref().is_some_and(|t| t.is_empty()) { task.tags = None; }` 删掉 → 同一条单测的「空数组清除」那半变红；真机上表现为标签清不掉。

**平台**：跨平台（写入口在后端，与 UI 前端无关）。

---

## 四、T1 档：静默失效与状态谎报

### M45 · 截止时间三写口归一

**靶点**：`src-tauri/src/db/tasks.rs::due_format_error`（提交 `ec0e25c7`）

**bug 形态**：截止时间有**两个合法形态**：`YYYY-MM-DD`（当天 23:59 截止）与 `YYYY-MM-DDTHH:mm`（带时刻）。`task_patch` 写口一直按这个口径校验，但：

- **bot 工具写口**（`create_task` / `edit_task`）只查**长度**（≤30 字），不查**格式**；
- **HTTP 写口**根本不查；
- **工具 schema 还在教空格分隔**（`2026-10-24 09:00`）。

后果链：空格格式落进库 → `due_notify` 解析失败 → **按「无截止」处理 → 通知静默不发** → 任务就这么无声地过期了。用户设了截止，机器人没提醒，**没有任何一处报错**。

**步骤**：

1. 在聊天框发：

```
用 create_task 建一张任务卡，标题写「截止格式验证」，截止时间填 2026-10-24 09:00（日期和时间之间有一个空格）。原样填写，不要改成别的格式。
```

2. 再发一条，把截止时间填成半截的 `2026-10`。
3. 再发一条，填 `2026-10-24T09:30`（合法形态）。

**判据**：

- PASS：前两条被拒，文案里含 `截止时间格式必须是 YYYY-MM-DD 或 YYYY-MM-DDTHH:mm`；第三条**建卡成功**。
- FAIL：带空格的 / 半截的截止时间被存进库 → 到点**没有通知**。
- **HTTP 侧同验**（可选）：直接打 HTTP 接口提交带空格的截止 → 也必须被拒（`400` + 同一句拒因）。
- **落地必查**：`sqlite3 <data_dir>/wmessage.db "select id,title,due from tasks where title='截止格式验证'"` → 不该有带空格的 due 行。

**变异注入复验**：
- 把 `due_param_denied` 里的 `crate::db::tasks::due_format_error(d).map(...)` 删掉（只留长度闸）→ 重跑应 **FAIL**（空格格式被接受）。
- 把 `check_due` 里的 `crate::db::tasks::due_format_error(trimmed)` 改成 `None` → HTTP 侧失守。

单测侧同款变异已覆盖：`bot::tools::tool_due_format_tests::due_param_denied_rejects_space_format_and_half_typed` 与 `api_handlers::validate::tests::check_due_rejects_invalid_shapes` → 变红。

> **工具 schema 也算判据**：`create_task` / `edit_task` 的 `due` 参数说明里**不该再出现空格分隔的示例**。

**平台**：跨平台（校验全在后端）。

---

### M46 · 非法列值响亮拒绝 + 完成三件套

**靶点**：`src-tauri/src/bot/tools.rs::tool_create_task` / `tool_edit_task` / `tool_complete_task`（提交 `20187e41`）

**bug 形态**：两处。

**其一：非法列值静默降级。** schema 的 `column` 枚举只有 `todo` / `doing`，但代码是 `match v["column"].as_str() { Some("doing") => Doing, _ => Todo }`——传 `column: "archived"` 会**静默建成 todo 列**。模型收到「建卡成功」，以为建到了它要的列。`edit_task` 那侧更隐蔽：`parse::<TaskStatus>().ok()` 失败就**跳过这个字段**，模型收到「没有可修改的字段」——**错误归因被带偏**。

**其二：完成三件套漏两件。** 完成一张卡要做三件事：列改 `done` + 打完成时间 + **复位归档位 + 清掉机器人接管标记**。`tool_complete_task` 此前只做前两件 → 完成卡上**仍显示「机器人执行中…」**。

**步骤**：

1. 在聊天框发：

```
用 create_task 建一张任务卡，标题写「列值验证」，column 填 archived。字段名和值都原样填写，不要自己挑一个合法的。
```

2. 再发一条，用 `edit_task` 把一张卡 `column` 改成 `archived`。
3. 再发一条，用 `complete_task` 完成一张**正被机器人执行**的卡（或先让它处于 `bot_assigned` 状态）。

**判据**：

- PASS：前两条被拒，文案分别是 `column 只能是 todo 或 doing（收到 "archived"）` 与 `column 不合法：…`；第三条完成后，卡片状态行**不再显示「机器人执行中…」**，且归档位被复位（卡回到未归档）。
- FAIL：非法列值被静默降级成 todo（模型以为建成了 archived）；完成卡仍显示「机器人执行中…」。
- **落地必查**：`sqlite3 <data_dir>/wmessage.db "select id,column,bot_assigned,archived from tasks where title='列值验证'"` → `column` 应是 `todo`（建卡成功）或根本没有这张卡（被拒）；**绝不该是 `archived`**。

**变异注入复验**：
- 把 `tool_create_task` 的 `Some(other) => { return ToolResult::denied(...) }` 改回 `_ => TaskStatus::Todo` → 重跑应 **FAIL**（非法列值静默降级）。
- 把 `tool_complete_task` 里的 `next.bot_assigned = None;` 删掉 → 重跑应 **FAIL**（完成卡仍显示机器人执行中）。

单测侧同款变异已覆盖：`bot::tools` 的列值 / 完成语义守卫测试 → 变红。

> **口径提示**：主窗口的 `task_set_column` 与 HTTP 写口早就收着这两个上限与完成三件套——本次是让 bot 写口**对齐**，不是新增规则。三写口同一套标准才是这张卡要验的。

**平台**：跨平台。

---

### M47 · 技能启用开关（frontmatter 单源）

**靶点**：`src-tauri/src/bot_skills/manage.rs::skills_set_enabled` / `effective_skill_md` + `src-tauri/src/bot_skills/parse.rs::set_frontmatter_enabled`（提交 `ec96469e`）

**bug 形态**：`enabled` 字段一直存在却没有入口——想关掉一个技能只能自己去技能目录手改 `SKILL.md`。本次给设置页每条技能加了拨杆，把状态写进**技能自身 frontmatter 的 `enabled` 键**（单一事实源，不另开状态表）。

三道**既有**闸本来就以 `meta.enabled` 为准，本次只补了写路径：

| 闸 | 位置 | 关掉后的效果 |
|---|---|---|
| 清单过滤 | `build_skill_block` | 技能不再被广告给模型 |
| 意图路由 | `intent_rules_from_dirs` | 不产路由条目，用户消息不被劫持 |
| 启动预审 | `runtime::preflight` | `use_skill` 启动即拒 |

**最容易出错的地方是「写哪一份」**：落盘目标是**生效副本**（按 `skill_search_paths` 优先级取第一份存在的 `SKILL.md`，与读取侧同口径）——写错副本就是「**关的是 A、生效的是 B**」（用户点了开关毫无反应，或关不掉真正生效的那份）。

**步骤**：

1. 设置页 →「技能」分区 → 找一个自建技能 → 把最右的拨杆**关掉**。
2. 观察：**整行置灰**；**解释文字走鼠标悬停 tooltip**（按钮后不挂文字）。
3. 在聊天框发一条消息，**点名那个技能**（例：`用 xxx 技能做点事`），看模型有没有把它当候选。
4. 再发一条**含该技能描述里关键词**的消息，看会不会被路由进去。
5. 把拨杆**再打开**，重复第 3 步。

**判据**：

- PASS：关掉后 → 设置页该行置灰；**模型那边完全看不到这个技能**（清单里没有）；**点名的消息不被路由进它**；直接 `use_skill` 启动被拒（返回「技能已禁用」类的拒绝文案）。
- FAIL：清单里仍在（模型照样选它）/ 用户消息仍被路由进它 / `use_skill` 仍能启动。
- **落盘必查**：生效副本的 `SKILL.md` frontmatter 里 `enabled: false` 真的写进去了（**且其余键没丢**——`version` / `description` 等必须还在）。
- **地面真相**：检查**没有** `.tmp` 残渣留在技能目录里（原子写：同目录 tmp + rename）。
- **打开后复验**：重新打开 → 清单恢复、路由恢复、`use_skill` 能启动。

**变异注入复验**：
- 把 `effective_skill_md` 改成「取优先级**最后**一份存在的」（`dirs.iter().rev()`）→ 重跑应 **FAIL**（写到了不生效的那份副本，开关点了没反应）。
- 把 `write_enabled_to` 里的 `let Some(rewritten) = set_frontmatter_enabled(&text, enabled) else { ... }` 改成「解析不出来就用原文写回」→ 落盘读不回目标值 → 复验第 3 步应 **FAIL**。

单测侧同款变异已覆盖：`bot_skills::manage::tests::write_enabled_to_flips_frontmatter_flag`、`write_enabled_to_leaves_no_tmp_residue`、`effective_skill_md_prefers_earlier_dir` → 变红；前端 `SkillsPanel.test.tsx` 的「启用开关」四条（拨杆调 `skills_set_enabled` / tooltip 形态 / 缺省不误关 / 关闭态置灰）→ 变红。

> **单源设计的反面**（为什么不多开一张状态表）：两份事实源必然漂移，且用户手改 `SKILL.md` 时不知道该改哪一份——漂移后的表现正是「**我明明关掉了它还在**」。

**平台**：跨平台（写盘与闸都在 Rust 侧）。

---

### M48 · 内置 pptx-design 退役 + 残留物化副本清理

**靶点**：`src-tauri/src/bot_skills/builtin.rs::purge_retired_skills_in` / `materialize_at_startup`（提交 `afe580ac`）

**bug 形态**：随包分发内置技能这条路关掉（`BUILTIN_SKILLS` 清空、`assets/skills/pptx-design/` 删除）。**退役必须带清理**——物化目录 `builtin_skills/` **在技能搜索路径里**，资产撤了、清单里那条也撤了之后，旧版本物化出来的副本**仍会被扫到、出现在设置页技能列表里**：

- 用户在设置页**删不掉**（删了，下次启动若资产还在会被物化回来；资产真没了就变成**永久僵尸技能**）；
- **模型还能调用它**（它就在搜索路径里）。

**步骤**：

1. 用**上一版 exe**（物化了 `pptx-design` 的版本）启动一次 → 让它物化出 `builtin_skills/pptx-design/`。
2. 换成**本版 exe** 启动。
3. 打开设置页 →「技能」分区 → **找 `pptx-design`**。

**判据**：

- PASS：设置页技能列表里**没有 `pptx-design`**；`builtin_skills/` 目录下的 `pptx-design` 子目录**已被删掉**；bot.log 里有 `builtin_skill_retired`（清理成功）或 `builtin_skill_purge_failed`（清理失败，带原因）——**两者必有其一，不该静默**。
- FAIL：`pptx-design` 还在技能列表里、**删不掉**、模型还能调它。
- **不得误删**：用户自建技能（住在同父的 `skills/` 目录）**必须还在**——清理只碰 `builtin_skills/` 的子目录。
- **地面真相**：`ls <data_dir>/builtin_skills/`（Windows：回退数据目录下的同名目录，见 §1.0 ④）→ 目录应为空或只剩在册技能。

**变异注入复验**：
- 把 `materialize_at_startup` 里的 `purge_retired_builtin_skills(&dir)` 调用删掉 → 重跑应 **FAIL**（僵尸技能留在列表里）。
- 把 `purge_retired_skills_in` 的 `if skills.iter().any(|s| s.dir_name == name) { continue; }` 改成无条件删 → 重跑应 **FAIL**（在册技能被误删）。

单测侧同款变异已覆盖：`bot_skills::builtin::tests` 里的退役清理内核测试 → 变红；资产目录 ↔ 登记清单的双向一致由 `asset_dir_matches_registration_list` 守住 → 变红。

> **M24 实机卡的对照技能**：上一份文档的 M24 原本用内置 `pptx-design` 作对照技能，现已改为自建最小技能即可，卡仍可照跑。

**平台**：跨平台（物化与清理都在 Rust 侧）。**但数据目录位置在 Windows 上会回退**——见 §1.0 ④。

---

### M49 · 超时清扫覆盖死会话的 Running

**靶点**：`src-tauri/src/bot_skills/runtime.rs::sweep_expired_pauses` / `skill_conflict_with_existing`（提交 `921f3477`）

**bug 形态**：技能运行槽位有三种状态（Running / Paused / 终态）。**清扫只扫 Paused**（等确认等到超时的），**Running 不扫**。

而 `Running` 有一条真实的死法：**模型循环带 `Err` 退出**（LLM 报错、网络断、panic）时，`advance_skill` 不会被调用（它只由本会话循环调）→ 那一行**永远停在 Running**。

后果：这一行**永久占着同名技能**——同名技能再也起不来，直到**重启进程**。用户视角：「昨天那个技能卡住了，别的会话也用不了同名技能，重启应用才好」。

修复两处：清扫覆盖**超时的 Running**（标 `Failed`，`end_reason` = `运行超时（会话未收尾），已过期`）；冲突闸里 `Running` 也要看是否超时（超时的 Running 不算活动）。

**步骤**：

1. 在会话 A 里启动一个**同名技能**，让它进入等确认（Paused）或让它跑起来（Running）。
2. **制造一条死会话**：让会话 A 的模型循环带错误退出——最省事的做法是**在模型请求前断网 / 改一个无效的厂商配置**，让这轮直接报错退出。
3. 等到该技能的 `timeout_secs` 超时（默认几十分钟，测试可把 timeout 设短）。
4. 在**另一个会话 B** 里启动**同名**技能。

**判据**：

- PASS：会话 B 启动同名技能**成功**（返回里没有「正在另一个会话/任务中运行」）；bot.log 出现 `skill_expired | name: <技能名> | 等待确认超时，已过期并释放占位`（清扫痕迹）；此后该技能可正常启动。
- FAIL：会话 B 被拒（`技能「X」正在另一个会话/任务中运行，请等它结束后再启动`），且**重启应用前一直如此**。
- **占位在内存里**：技能运行态是**进程级内存注册表**（`bot_skills::state::skill_runs`），**不落库**——判据看**行为 + 审计**，不查库表。正因如此「重启应用就好」是这个 bug 的既有表现；修复后**不必重启**。
- **清扫的触发时机**：清扫发生在**新技能启动的那一刻**（不是后台定时器）——所以 `skill_expired` 那条审计会**与这次启动同时出现**，不是提前出现的。
- **未超时的仍要拦**：启一个正常在跑（未超时）的同名技能 → 会话 B **仍应被拒**（这条别漏，否则就变成「同名技能可以并发跑了」）。

**变异注入复验**：
- 把 `sweep_expired_pauses` 里的 `if run.state == SkillState::Running && skill_timed_out(run, now_ms)` 这一段删掉 → 重跑应 **FAIL**（死会话继续占位）。
- 把 `skill_conflict_with_existing` 里的 `SkillState::Running => !skill_timed_out(r, now_ms)` 改回 `SkillState::Running => true` → 重跑应 **FAIL**（超时的 Running 仍拦同名）。

单测侧同款变异已覆盖：`bot_skills::runtime::tests::sweep_expired_pauses_marks_failed_and_frees_slot` 与 `conflict_gate_lets_timed_out_running_pass` → 变红。

**平台**：跨平台。

---

### M50 · MCP 按需启停：启用不再拉起子进程

**靶点**：`src-tauri/src/bot/mcp/manager.rs`（`Slot` 三态：`Idle` / `Connected` / `Down`）+ `apply_config`（提交 `efd5d481`）

**bug 形态**：stdio 服务器是**真进程**（npx / node 常驻 80–150 MB）。原先「启用 = 连上并常驻到退出」让装十台服务器的机器**白吃几 GB 待机内存**，而大部分服务器压根没人调。

改成三态槽位：**开关打开只表示「允许机器人用它」（静默待命，没有进程在跑）**，模型真正调用时才拉起，调用结束立刻回收。

状态串变了（五态）：

| state | 含义 |
|---|---|
| `idle` | 已启用、工具清单已知、**没有进程在跑**（按需启停的常态） |
| `connected` | 调用中 |
| `pending` | 已启用但工具清单还没发现 |
| `down` | 连接失败 |
| `absent` | 进程内无槽位（未启用 / 还没轮到） |

**步骤**：

1. 设置页 →「MCP」分区 → 确认有一台已配置的外部服务器（**没有就现配一台**：`npx -y @modelcontextprotocol/server-everything` 之类）。
2. 打开它的**启用开关**。
3. **不要问模型任何事**——只做第 2 步。
4. 观察：服务器状态行显示什么；用系统工具看有没有子进程。

**判据**：

- PASS：状态是 `idle`（不是 `connected`）；**系统里没有该服务器对应的子进程**（macOS：`ps -ef | grep -i npx`；Windows：`Get-Process | Where-Object {$_.ProcessName -like "*node*"}`）。
- FAIL：开关一打开就起了常驻进程（待机内存凭空多出上百 MB）。
- **提示**：上一份文档的卡若写了「启用后应该是 connected」——**那是旧口径**，按本卡的 `idle` 判。

**变异注入复验**：把 `apply_config` 里启用分支的「转 `Idle`（不连）」改回「转 `Connected`（连上）」→ 重跑应 **FAIL**（开关一开就起进程）。
单测侧同款变异已覆盖：`bot::mcp::manager::tests::enabling_a_server_does_not_spawn_a_process` → 变红。

> **状态串判据的坑**：`state` 字段的取值集合变了（新增 `idle` / `pending`），**任何按「connected / down / absent」三态写的断言现在都会漏**。真机上先确认面板显示的是新串。
>
> **跨文档提醒**：上一份文档的 **M34**（MCP 面板资源行按 5s 轮询）第一步是「启动一个 MCP 服务器」——按需启停下**光启用不启进程**，那一步要么改成「先让模型真调一次」，要么按面板显示的 `idle` 判。复跑 M34 时别按旧预期判「资源行该有 pid / 内存」。

**平台**：跨平台（进程生命周期管理在 Rust 侧）；**Windows 上的子进程回收另见 W04**。

---

### M51 · MCP 工具清单落盘快照 + 删除服务器清快照

**靶点**：`src-tauri/src/bot/mcp/tool_cache.rs`（`ToolCache::load` / `save`）+ `manager.rs::write_tool_cache` + `commands.rs::purge_tool_cache`（提交 `efd5d481`）

**bug 形态**：按需启停的关键解耦是**工具元数据与活连接拆开**：工具清单**发现一次即落盘**（按服务器 id 存「配置指纹 + 工具清单」，指纹认 env / headers 这些机密字段）。

不这么做的后果很直接：**没有连接就没法挂工具 → 模型看不见它 → 永远不会去调**。按需启停与「模型看得见工具」这两件事，只有靠盘上快照才能同时成立。

配套两条：服务器被删除时，它的快照条目也要走（留着是**孤儿条目**）；快照写盘失败只记审计（指纹对不上就用不上，不阻断）。

**步骤**：

1. 设置页 → MCP → 启用一台服务器 → 点开「查看工具」展开（**这一步会按需发现一次**：用户显式要看清单 = 连一次、列完就断）。
2. 观察工具清单有内容。
3. **完全退出应用并重启**（不再有任何连接）。
4. 在聊天框里让模型列一下可用工具 / 直接点名那个服务器的工具让它调一次。
5. 回设置页 → **删掉这台服务器** → 再看工具快照文件。

**判据**：

- PASS：第 4 步模型**看得见**那台服务器的工具（清单来自盘上快照，没有连接也能挂上）；第 5 步后快照里**该服务器的条目被清掉**（不再是孤儿）。
- FAIL：重启后模型**看不见**那台服务器的工具（模型永远不会去调它）；或删除后快照条目仍在。
- **审计判据**：快照写盘失败时 bot.log 有 `mcp.tool_cache_write_failed`；删除清快照失败时有 `mcp.tool_cache_purge_failed`。
- **地面真相**：工具快照文件在数据目录下（`<data_dir>/` 下的 MCP 工具缓存文件），删服务器后对应条目消失。

**变异注入复验**：
- 把 `manager.rs` 里挂载工具时的 `super::tool_cache::ToolCache::load(&dir)` 改成「只用活连接的槽位」→ 重跑应 **FAIL**（重启后模型看不见工具）。
- 把 `commands.rs::mcp_server_delete` 里的 `purge_tool_cache(&app, &id)` 调用删掉 → 重跑应 **FAIL**（快照留孤儿条目）。

单测侧同款变异已覆盖：`bot::mcp::manager::tests::cached_tools_are_mounted_without_any_live_connection` 与 `discovered_tools_are_written_to_the_disk_snapshot` → 变红。

> **发现冷却**：快照缺失时会按需发现一次；发现失败进 **10 分钟冷却**（免得每轮对话都去连一遍坏服务器——超时 10s 起步，整轮提问被拖死）。真机上若某台服务器刚配好就问模型，**第一次可能看不到工具**——这是冷却在起作用，**不是坏了**；隔 10 分钟再试。

**平台**：跨平台。

---

### M52 · 子 agent 工具预算触顶改收尾窗

**靶点**：`src-tauri/src/bot/dispatch.rs::execute_tool_impl` 的收尾窗分支（`WRAP_UP_TOOLS`）（提交 `ffc0487c`）

**bug 形态**：工具预算触顶后的旧行为是**硬拒所有工具**。但触顶时模型手里往往**已经有料**（查了 40 次文件），最该做的下一件事是「**把已有发现写成产物文件**」——而写产物恰好也在被拒的名单里。

结果就是**零产出**：跑满预算、烧掉全部轮次、什么都没留下。真实案例在设计文档里记着：**100 次调用零产出**。

改成**收尾窗**：触顶后只放行四个工具——`write_artifact_file` / `write_file` / `edit_file` / `read_own_card`（后者让模型触顶后还能查自己的状态与剩余量）。**读拒、写放**；被拒不耗熔断裕量（硬熔断在模型循环层，那里才是兜底）。

**步骤**：

1. 在聊天框发一条会派子 agent 的指令，**给一个很小的工具预算**（比如工具调用 3 次），但要求它产出一个文件：

```
派一个子 agent 去读 D:\tmp\mutest\a.txt 和 b.txt，把两边的关键行整理成一份 markdown 写到产物目录里。工具调用预算给 3 次。
```

> macOS 上路径换成 `/tmp/mutest/a.txt`。**文件先自己造好**：`a.txt` 写 3 行、`b.txt` 写 3 行。

2. 观察子 agent 的工具调用序列与最终产物。

**判据**：

- PASS：触顶后**读类工具被拒**（返回 `⚠️ 工具调用预算已触顶，收尾窗内只放行写产物工具（write_artifact_file / write_file / edit_file）与 read_own_card；请把已有发现写成产物文件，然后输出收尾 JSON。`）；**写产物工具放行**且**产物文件真的落盘**（内容是那两行关键行）；子 agent 最终输出收尾 JSON。
- FAIL：触顶后写产物也被拒 → **零产出**（产物目录空的）；或触顶后读类工具仍然放行（预算形同虚设）。
- **审计判据**：bot.log 有 `subagent_budget_tool_calls_exceeded`（首次触顶）与 `budget_wrap_up_deny`（收尾窗内被拒的调用）。

**变异注入复验**：
- 把 `WRAP_UP_TOOLS` 里的 `"write_artifact_file"` 删掉 → 重跑应 **FAIL**（触顶后写产物被拒 → 零产出）。
- 把收尾窗分支的 `if !WRAP_UP_TOOLS.contains(&name)` 改成 `if false` → 重跑应 **FAIL**（收尾窗内读类工具仍放行，预算不再约束）。

单测侧同款变异已覆盖：`bot::dispatch::pre_tool_hook_tests::wrap_up_tools_exist_in_every_profile_whitelist`（每个 profile 都得有可用写产物工具 + `read_own_card`）与 `wrap_up_branch_allows_writes_without_acquiring`（接线守卫：放行路径不得再走预算 acquire）→ 变红。

> **收尾窗的「零产出复活」风险**：放行清单里的工具名如果不在该 profile 的白名单里，收尾窗对这个 profile **整体关闭**（写了也过不了白名单闸）→ 触顶等于零产出路径复活。守卫测试 `wrap_up_tools_exist_in_every_profile_whitelist` 就是钉这一条。

**平台**：跨平台（纯后端逻辑）。

---

### M53 · 删除与导入落库必广播 + 导入导出补审计

**靶点**：`src-tauri/src/db/tasks.rs::db_delete` / `tasks_import` / `tasks_export`（提交 `20165f0e`）

**bug 形态**：三处**协议缺口**。

**其一：删除不广播。** `db_delete` 落库后**不发** `tasks-changed`。当前前端调用方**自己补发**闭环成立，但任何后端复用直调本命令都会**静默漏发**——界面上的卡删不掉。

**其二：导入不广播。** 同上，`tasks_import` 落库后界面不知道有新卡。

**其三：导入导出无审计。** 导出可以把**全部任务 + people 表**写到任意过闸路径，导入可以把任意 `.json` 读进库——两个都是数据面的大动作，**一条审计都没有**。

**步骤**：

1. 任务页 → 选中一张卡 → 删除 → **不刷新页面**，看列表。
2. 设置页 → 任务数据管理 → **导入**一个含 2 张卡的 `.json` → **不刷新页面**，看列表。
3. 同一步骤做一次**导出**。

**判据**：

- PASS：第 1 步列表**立刻**少一张卡（不刷新也更新）；第 2 步列表**立刻**多两张卡；bot.log 里有 `tasks_import`（带 `count` 与 `path`）与 `tasks_export`（带 `count` 与 `path`）两条审计。
- FAIL：删除 / 导入后界面不动（必须刷新才更新）；或审计行缺失。
- **导入 0 条不写审计**：空文件导入（`imported == 0`）**不该**有 `tasks_import` 审计，也**不该**广播（这是刻意的口径，别当 bug）。
- **地面真相**：`sqlite3 <data_dir>/wmessage.db "select count(*) from tasks"` 与界面上的卡数一致。

**变异注入复验**：
- 把 `db_delete` 里的 `if ids_len > 0 { app_emit.emit("tasks-changed", ()) }` 删掉 → 重跑应 **FAIL**（删除后界面不更新）。
- 把 `tasks_import` 里的 `if imported > 0 { … emit("tasks-changed", ()) }` 删掉 → 重跑应 **FAIL**（导入后界面不更新）。

> **审计字段口径**：路径进审计前过 `escape_for_log(path, 120)`——**审计里不该出现完整长路径**，看的时候别以为路径被截了是 bug。

**平台**：跨平台。

---

### M54 · 重读任务并入 mutate 串行链

**靶点**：`src/App.tsx::reloadTasks`（提交 `af1d959c`）

**bug 形态**：前端有一条全局 `mutating.chain` 串行链（所有改库操作排队执行）。但 `reloadTasks`（重读全量任务）**不在链上**。

于是：reload 完成时若链上恰有**在途 mutate 停在 await 窗口**（等一个还没回来的 IPC），那条 mutate 恢复后会拿**导入前的旧基线**无条件回写 `tasksRef`——**弹出「导入完成 N 条」，界面却没有新卡**。

这是典型的**读改写竞态**：两个写者各拿各的基线，后写的赢。

**步骤**：

1. 准备一个含 3 张卡的 `.json` 导入文件。
2. 任务页上**同时**做两件事：点「导入」，**紧接着**（不等它完成）改一张卡的标题（制造一个在途 mutate）。
3. 看最终界面。

**判据**：

- PASS：导入完成后**三张新卡都在**，且那张被改标题的卡的标题也**保住了**（没有互相盖掉）。
- FAIL：导入提示说「完成 3 条」，界面**一张新卡都没有**（被在途写的旧基线盖掉）；或反过来，改标题的修改被吞掉。
- **重复跑**：这个竞态**不一定每次都撞上**——**同一个动作连做 5 次**，至少要撞上 1 次才算 FAIL。

**变异注入复验**：把 `reloadTasks` 里的 `const p = mutating.chain.then(run);` 改回 `const p = run();`（不进链）→ 重跑应 **FAIL**（竞态复现概率显著上升）。

> **同类竞态的通用验法**：任何「导入 / 重读 / 批量改」的动作，都配一个**同时在途的单点修改**跑一遍。这类 bug 单测极难覆盖——单测里没有真正的 await 窗口。

**平台**：跨平台（纯前端逻辑，Vite 热更新即可验，不用出包）。

---

### M55 · 浮窗 Esc 层级裁决

**靶点**：`src/components/TaskTimeline/TaskDetailPanel.tsx` 的 Esc 层级裁决 + `src/App.tsx` 传 `escBlocked`（提交 `a808778c`）

**bug 形态**：任务详情浮窗自己挂了 `window` 级 `keydown` 监听，Esc → `handleClose`（保存并关闭）。它与**命令面板**（更上层的浮层）**没有层级协商**：命令面板开着时按一次 Esc，**两个浮窗一起关**。

后果有两个方向，而且都很糟：

- 浮窗的 `saveOnClose` **连带提交草稿**（用户只想关命令面板，却把没写完的备注提交了）；
- 浮窗的 `onDiscard` **回滚已落库的改动**。

修复：引入 `escBlocked`（更上层浮层开着时 Esc 让位给它，浮窗不动）+ 标题编辑态的 `stopPropagation`（只还原标题草稿，不把事件冒泡到 window 关掉整个浮窗）。

裁决顺序（自上而下）：**执行痕迹弹层 > 更上层浮层（命令面板）> 标题编辑态 > 关浮窗**。

**步骤**：

1. 打开一张任务卡 → 详情浮窗（**改一下备注但先不提交**，停在编辑态）。
2. 按 `⌘K`（或点搜索入口）打开**命令面板**。
3. 按一次 **Esc**。
4. 再重复一次，但浮窗里**标题处于编辑态**（光标在标题里）。

**判据**：

- PASS：第 3 步**命令面板关了，任务浮窗还开着**，且**浮窗里的草稿没被提交**（备注仍是编辑态的内容，未落库）。
- FAIL：两个浮窗一起关；或浮窗关了并**把草稿提交了**。
- 第 4 步：标题编辑态按 Esc → **只还原标题草稿**，**浮窗不关**。

**变异注入复验**：
- 把 `TaskDetailPanel` 里的 `if (escBlocked) return;` 删掉 → 重跑应 **FAIL**（命令面板开着时 Esc 连浮窗一起关）。
- 把标题 textarea 的 `e.stopPropagation()` 删掉 → 重跑第 4 步应 **FAIL**（标题编辑态按 Esc 把浮窗关了）。

单测侧同款变异已覆盖：`TaskDetailPanel.test.tsx` 的「Esc 关面板（window 级）」与「标题 blur 提交 patch；Esc 还原不提交」→ 变红。

> **前端变异不用出包**：改 `src/` 下的 TS/TSX，Vite 热更新即可（§1.5）。

**平台**：跨平台。

---

### M56 · 运行记录 / 版本读取失败不再伪装成空库

**靶点**：`src/components/AppsPage/AppsPage.tsx` 的 `RunsList` / `VersionsList`（提交 `607e0fe6`）

**bug 形态**：读失败被 `.catch(() => setRuns([]))` 吞成**空数组**。于是「读不到」和「真的没有」在界面上**长得一模一样**——都显示「还没有运行记录」。

这是本批最典型的一类「状态谎报」：**失败伪装成空**。用户会以为程序从没跑过，而真相可能是数据库读失败、路径不对、权限被拒。

**步骤**：

1. 程序页 → 找一个**跑过**的程序 → 展开「运行历史」（先确认现在显示的是真实历史，**不是**「还没有运行记录」）。
2. **制造读失败**（可逆，不破坏数据）：把运行记录表临时改名——`sqlite3 <data_dir>/wmessage.db "alter table app_runs rename to app_runs_tmp;"`（版本页签同理改 `app_versions`）。
   > **别用「把库改名」**——`open_db` 会在原位**重建一个空库**，读取会「成功」并返回空列表，压根测不到这条。**别用「把库改坏」**——那会让整个应用到处报错，判据会被别的噪声淹掉。改表名最小、最可逆。
   > **⚠️ 2026-10-24 实机否证：改表名这条路也走不通。** `open_db` 是**每连接跑全套 DDL**（CREATE TABLE IF NOT EXISTS），改名后下一次读命令会把 `app_runs` **原地重建为空表**——读取照样「成功」返回 0 行，界面显示「还没有运行记录」是对真空表的**正确**呈现，不是伪装空库。本卡的实机注入手段（库级/表级改名）在本构建上双双不可用，错误态判据回 `AppsPage.test.tsx` 的单测（`.catch(() => setLoadError(true))` 分支）钉。复跑者不要在改名上再花时间。
3. 刷新程序页 → 看「运行历史」与「版本」两个页签。
4. 恢复：`sqlite3 <data_dir>/wmessage.db "alter table app_runs_tmp rename to app_runs;"`。

**判据**：

- PASS：读失败时显示 `运行记录读取失败，稍后重试` / `版本快照读取失败，稍后重试`——**不是**「还没有运行记录」；恢复后正常显示历史。
- FAIL：读失败显示「还没有运行记录」（用户被误导成「程序从没跑过」）。
- **已知形态（非缺陷）**：**已经加载过一次**之后再次失败，界面**保留上一次的列表**、不翻错误态（判据分支要求 `runs === null`）——这不是「伪装成空库」，判据按**首次加载失败**这一路来判。

**变异注入复验**：把 `RunsList` 的 `.catch(() => setLoadError(true))` 改回 `.catch(() => setRuns([]))` → 重跑应 **FAIL**（失败又变回空库）。
单测侧同款变异已覆盖：`AppsPage.test.tsx` 的运行历史 / 版本页签用例 → 变红。

> **同类模式值得整仓库扫一遍**：`.catch(() => set([]))` 这个写法在本仓库出现过多处，每一处都是「失败伪装成空」的候选。

**平台**：跨平台。

---

### M57 · 问答应答与提问落库持写锁

**靶点**：`src-tauri/src/ask_engine.rs::ask_respond` / `engine_ask_user`（提交 `e3604e71`）

**bug 形态**：通知表（ask 提问 / 应答）的写入**不持 DB 写锁**，而全仓其它写口都持。

后果：**主窗有一个长事务**（例如导入大批任务、或一次大标签整理）持锁超过 `busy_timeout` 时，ask 的应答落库会**静默失败**——用户点了选项（选了「方案 A」），**应答没了**，机器人那边永远等不到，于是这次提问**卡死在等待态**。

这是「静默丢应答」：界面上的按钮点了没反应，用户只能重来。

**步骤**：

1. **造一个长事务窗口**：在任务页做一次**大批量导入**（几百上千条卡的 `.json`）——这会持写锁一段时间。
2. **在这期间**让机器人发起一次提问（`ask_user`），并**在导入还没结束时**点一个选项应答。
3. 看机器人是否收到了应答、提问是否闭合。

**判据**：

- PASS：应答**被记下来**了（不丢），提问随后正常闭合，机器人继续往下走。
- FAIL：应答**静默消失**——用户点了，机器人还在等，提问悬着。
- **落地必查**：`sqlite3 <data_dir>/wmessage.db "select id,question,answer from notifications where answer is not null"` → 应答行在。

**变异注入复验**：把 `ask_respond` 里的 `let _g = crate::db::lock_db_write();` 删掉 → 重跑应 **FAIL**（长事务窗口内应答丢失的概率上升）。
单测侧同款变异已覆盖：`ask_engine` 的写锁守卫测试 → 变红。

> **这一族的共同形态**：`25ed736b`（运行记录四个写口）、`d6766145`（工作区三个写口）修的是**同一个病**：某几个写口漏了锁。**判据通用**——「长事务窗口内，这个写口的操作还在不在」。

**平台**：跨平台。

---

## 五、T2 档：可感知语义

### M58 · 契约卡回实时余量

**靶点**：`src-tauri/src/bot_orchestrator.rs::own_card_payload`（提交 `b270735d`）

**bug 形态**：子 agent 的 `read_own_card` 此前返回**静态的 `budget` 键**（上限值），模型看不到「我已经用了多少」。模型因此无法判断「还该不该再探一轮」——**要么早停（没料就交卷），要么烧光预算**。

现在返回 `budgetUsage` **单块**，含实时余量：`toolCallsUsed` / `toolCallsMax` / `roundsUsed` / `roundsMax` / `wallRemainingSecs`。**静态 `budget` 键退役**（两份会让模型不知道看哪个）。

**步骤**：

1. 派一个子 agent，让它**跑几轮**再调 `read_own_card`。
2. 观察 `read_own_card` 的返回 JSON。

**判据**：

- PASS：返回里有 `budgetUsage` 一块，五个字段齐全且 `toolCallsUsed` / `roundsUsed` 是**已经用了多少**（不是上限）；**顶层没有 `budget` 键**。
- FAIL：只有静态上限、没有已用量；或 `budget` 与 `budgetUsage` **两份都在**（模型不知道看哪个）。
- **边界**：预算缺失（老行）时 `toolCallsMax` / `roundsMax` 为 `null`，但 `toolCallsUsed` **仍要报**——余量读数不依赖预算存在。

**变异注入复验**：
- 把 `own_card_payload` 里的 `"budgetUsage"` 键名改回 `"budget"` → 重跑应 **FAIL**（静态键复活）。
- 把 `"toolCallsUsed": tool_calls_used` 改成 `"toolCallsUsed": budget.as_ref().map(|b| b.max_tool_calls)` → 重跑应 **FAIL**（报的是上限不是已用）。

单测侧同款变异已覆盖：`bot_orchestrator::orchestrator_tests::own_card_payload_reports_live_budget_usage` → 变红。

> **§1.6 第 2 条**：上一份文档记的 `read_own_card` 返回结构（`budget` / camelCase）**已过时**，别照旧预期判。

**平台**：跨平台。

---

### M59 · 右侧栏动作流实时上屏

**靶点**：`src/lib/subagentActivity.ts::applyActivityEvent` + `src-tauri/src/bot/dispatch.rs`（两相位旁路 UI 事件）（提交 `2283fb3d`）

**bug 形态**：子 agent 在干什么，此前**只能在 bot.log 里事后查**——用户看到的是一个「转圈的行」，不知道它在读什么文件、调什么工具。

现在 dispatch 在工具调用的**两相位**（`call` / `return`）旁路 UI 事件 `subagent-activity`，前端合并成「一行一条动作」的视图模型。

**步骤**：

1. 派一个子 agent 去读几个文件（至少 3 次工具调用）。
2. **不关窗口**，盯右侧栏「子 Agent」工作面。

**判据**：

- PASS：右侧栏**实时**出现动作行（一条工具调用一行），能看到工具名；工具返回后该行**收口**（同一个 `toolCallId` 的 call 行补上结局，不新增第二行）；列表恒持**最近 6 条**（`ACTIVITY_MAX_ITEMS`），新在头。
- FAIL：动作流不出现 / 一直停在「转圈」/ 同一个调用出现**两行**（call 与 return 没对上）。
- **乱序边界**：return 先于 call 到达（乱序 / 重启）→ 该动作直接当一条**已完成**动作入列头，**不是**被丢掉。

**变异注入复验**：
- 把 `applyActivityEvent` 里「return 按 `toolCallId` 收口同一条 call 行」那一段删掉（return 一律入列头）→ 重跑应 **FAIL**（同一个调用出现两行）。
- 把 `ACTIVITY_MAX_ITEMS` 改成 60 → 真机上表现为**列表不再滚动截断**——单测侧同款变异已覆盖：`src/lib/subagentActivity.test.ts` 的「列表恒持最近 N 条，新在头」→ 变红。

单测侧同款变异已覆盖：`src/lib/subagentActivity.test.ts` 四条 → 变红。

> **事件即焚**：动作流**不落库**（历史回看走详情面板的结果与 digest）。所以**重启应用后动作流会清空**——这是设计，不是 bug。

**平台**：跨平台。

---

### M60 · 子 agent 行补工具调用次 / 预算

**靶点**：`src/components/ChatPanel/SubagentRowLine.tsx::toolCallsText`（提交 `2283fb3d`）

**bug 形态**：子 agent 行此前只报**轮数**与运行时长。**收尾窗从 N/N 起开**，只看轮数看不出触顶——一个跑了 12 轮但已经用满 100 次工具调用的子 agent，行上看不出来。

现在行上补一列工具调用进度：`{已用}/{预算} 次`（只有已用没有预算时**只报已用数，不编分母**）。

**步骤**：

1. 派一个子 agent，跑到工具调用预算接近触顶。
2. 看子 agent 行上的进度文案。

**判据**：

- PASS：行上出现 `47/100 次` 这样的**已用/预算**；只有已用数时是 `47 次`（**不编分母**）；读不到仪表（终态）时整段消失。
- FAIL：工具调用数不显示；或**没有预算却编了个分母**（如 `47/0 次`）。
- **与轮数并排**：轮数与工具调用数**并排同报**——`10/20` 与 `10/200` 的紧迫度完全不同，只报已用数看不出还剩多少。

**变异注入复验**：把 `toolCallsText` 里的 `s.toolCallsUsed == null` 判据去掉（无仪表也显示）→ 重跑应 **FAIL**（终态行的工具调用段不消失）。
单测侧同款变异已覆盖：`SubagentRowLine.test.tsx` 的「工具调用与轮数并排同报：触顶（100/100 次）一眼可见」与「只有已用工具数没有预算时只报已用数，不编分母」→ 变红。

**平台**：跨平台。

---

### M61 · token 用量记账三列

**靶点**：`src-tauri/src/db/subagents.rs::update_subagent_usage` + `ensure_usage_columns`（提交 `95b3ea90`）

**bug 形态**：子 agent 的 token 用量此前**无处可查**——模型循环里累计了 prompt / completion / cache_read 三档，但只活在一轮里，**跑完就没了**。想回答「这个子 agent 花了多少 token」只能看日志。

现在补三列（`prompt_tokens` / `completion_tokens` / `cache_read_tokens`），**随终态同事务落库**；循环累计**不新增埋点**（值来自 `LoopTrace` 的原子累计）。

**口径要点**：**NULL = 未记**（老行 / 墙钟掐断 / panic 路径拿不到 trace）——**对账时按「未记」读，不编 0**。

**步骤**：

1. 派一个子 agent 跑完（正常终态）。
2. 查库。

**判据**：

- PASS：`select prompt_tokens, completion_tokens, cache_read_tokens from subagents where id='…'` → 三个数**有值**（prompt 明显大于 0）。
- FAIL：三列全 NULL（正常跑完的子 agent 不该是 NULL）。
- **老库口径**：本次启动前的旧子 agent 行 → 三列 **NULL**（不是 0）——迁移补列，老数据不倒填。
- **异常路径**：被墙钟掐断 / panic 的子 agent → 三列可能是 NULL（拿不到 trace），这是**如实记录**，不是 bug。

**变异注入复验**：
- 把 `run_subagent` 里的 `if let Some((p, c, cr)) = usage_for_db { … }` 删掉 → 重跑应 **FAIL**（终态三列为 NULL）。
- 把 `ensure_usage_columns` 从 `open_db` 里摘掉 → **老库启动后报缺列**（老库不带这三列 → 启动日志报错）→ 这条是「迁移接线守卫」。

单测侧同款变异已覆盖：`db::subagents` 的列迁移与 usage 写入测试 → 变红。

> **迁移位置有讲究**：三列的补齐排在 `task_id` 重建迁移**之后**——重建搬运的是显式列清单，本列由后面这一步补。

**平台**：跨平台。

---

### M62 · 预算纪律三条红线进通用底座

**靶点**：`docs/prompts/subagent/subagent_base.md` 第 6 条 + `src-tauri/src/bot/registry.rs::SCHEMA_READ_TEXT_FILE`（提交 `329ee3f6`）

**bug 形态**：三条**提示词层**的红线进通用底座（不只进子 agent 底座）：

1. **小文件一次读完**——读文件 ≤500 行一次读完（不传 `limit`），分页只用于更大的文件；**把一个文件拆成碎片逐段读 = 白烧预算**。
2. **打转自检**——连续多轮重复同类调用（重读已读内容、原地打转）→ 停，换策略或直接收尾。
3. **产物早落盘**——带文件产物的任务尽早落盘：先写骨架再增量补齐，**触顶时留下半成品而不是零**。

`read_text_file` 的**工具描述**同步教整读（模型看的是 schema，不是底座提示词）。

**步骤**：

1. 造一个 120 行的文件（`seq 1 120 > /tmp/mutest/small.txt`）。
2. 派一个子 agent 去读它并总结。

**判据**：

- PASS：子 agent **一次读完**（`read_text_file` 调用**不带** `limit` / `offset`），而不是分页若干次；工具描述里能看到「≤500 行的文件不传 limit 一次读完」。
- FAIL：同一个 120 行文件被读成 3–4 次（每次带 `limit`）——**轮次白烧**；或工具描述仍是「大文件用 offset/limit 分页读」这种会诱发分页的措辞。
- **副判据**：子 agent 的收尾 JSON 里若触顶，应当**已经落了盘**（产物目录有半成品）而不是零。

**变异注入复验**：
- 把 `SCHEMA_READ_TEXT_FILE` 描述里的「≤500 行的文件不传 limit 一次读完，offset/limit 分页只用于更大的文件」改回「大文件用 offset/limit 分页读」→ 重跑应 **FAIL**（模型回到分页读法）。
- 把底座第 6 条里的三条红线删掉 → 底座不再教整读 → 工具描述还在，但行为仍可能退化（**这条的判据要靠现象，不能只看 prompt**）。

单测侧同款变异已覆盖：`src-tauri/tests/fixtures/tools_baseline.json` 的 schema 基线对拍 → 变红（描述变了、基线没变）。

> **底座提示词变更是**「模型看得见的契约」——真机上很难直接断言模型读没读到红线，所以这张卡的主判据是**行为**（分页次数），schema 描述只是佐证。

**平台**：跨平台。

---

### M63 · 重试加保鲜窗口

**靶点**：`src-tauri/src/bot_scheduler.rs::classify_job`（`JobAction::RetryStale`）（提交 `6f06f6ed`）

**bug 形态**：作业失败后进入**重试**（`retry_at`）。旧的判定是「`retry_at <= now` → 派发」——**没有时效**。

于是：**作业被暂停数日后恢复**（或进程长期关闭后重启）→ 那个早就过期的重试**在恢复的瞬间立刻跑一次**。用户视角：「我暂停了三天的定时，今天打开它自己跑了一遍」——而且跑的是**过期的那个重试**（原始失败原因可能早就无关了）。

修复：重试加**保鲜窗口**，与正常到点的补跑窗口同宽（`CATCHUP_WINDOW = 2 小时`）。拖过窗口的重试**消费掉**（清 `retry_at`、清 `retry_count`）回到正常排期——**不跑**。

**步骤**：

1. 造一个会失败的重试：建一个定时作业，让它执行一次并**失败**（比如内容引用一个不存在的路径）。
2. 确认它进入了重试待命（库里 `retry_at` 有值）。
3. **把系统时间往前拨 3 小时**（或等 3 小时）。
4. 让作业重新可跑（打开机器人总开关 / 恢复作业）。

**判据**：

- PASS：恢复瞬间**没有**立刻跑一次；bot.log 里有 `sched_job_retry_expired | id: … | 重试到点已超 2h 保鲜窗口，丢弃重试回到正常排期`；库里 `retry_at` 与 `retry_count` 被清零，作业回到正常排期。
- FAIL：恢复瞬间**立刻补跑了一次**那个过期重试。
- **边界必做**：把 `retry_at` 调到**窗口内**（< 2 小时）→ 仍**照常派发**（保鲜窗口不能把正常重试也毙掉）。
- **不留噪声**：过期重试被消费掉后，**下一轮不该再判 RetryStale**（否则天天一条审计噪声）。
- **「回到正常排期」对 never-run 作业的语义（2026-10-24 实测）**：`sched_last=NULL` 的作业（从没跑过）在重试被丢弃后会**立即跑一次**——`classify_due` 对无基线作业是「首跑即补」语义（occurrence 从 epoch 起算必然 ≤ now 且 `sched_last.is_some()` 为 false 不判 Missed）。这是代码一致行为，**不是**「回到正常排期——不跑」被违反；用 DB 拨针复验本卡时，靶作业要么曾有 sched_last，要么预期里把首跑算进去。

**变异注入复验**：
- 把 `classify_job` 里的 `if now.timestamp_millis() - retry_at > CATCHUP_WINDOW.num_milliseconds() { return JobAction::RetryStale; }` 删掉 → 重跑应 **FAIL**（过期重试被派发）。
- 把 `JobAction::RetryStale` 分支里的 `update_job_sched_fields`/`update_job_run_state` 调用删掉（只记审计不清 `retry_at`）→ 重跑应 **FAIL**（天天一条 `sched_job_retry_expired` 噪声）。

单测侧同款变异已覆盖：`bot_scheduler::sched_tests::classify_job_stale_retry_is_dropped_not_dispatched`（含窗口边界内 / 边界外 / 远在过去三态）→ 变红。

> **拨系统时间有副作用**：验完记得改回去；改时间前先关掉其它定时作业，避免误触发。

**平台**：跨平台。

---

### M64 · 防重入拒绝补审计

**靶点**：`src-tauri/src/bot_scheduler.rs::run_scheduled_job`（提交 `edf3c763`）

**bug 形态**：到点执行作业前有**防重入守卫**（`SchedGuard::acquire`）：上一次还在跑，本次到点**不触发**。

但拒绝**不留痕**——直接 `return`。而 `ask_user` 的等待**可达 24 小时**：守卫 24 小时不放，**次日到点的同一个作业会被这里静默吞掉**。用户只知道「定时好像没跑」，**没有任何一处说为什么**。

**步骤**：

1. 建一个定时作业，内容会让机器人**发起提问并等应答**（`ask_user`）。
2. 让它到点执行 → 进入等待（守卫被占住）。
3. **第二天同一时刻**（或把某个作业的 `schedule` 设成几分钟后重跑同一个 id），让同一作业再次到点。

**判据**：

- PASS：第二次到点时 bot.log 有 `sched_job_skip | id: … | 上一次执行仍在进行，本次到点不触发`。
- FAIL：**静默跳过**——没有任何日志行，用户无从知道作业被吞了。
- **区分两种 skip**：另有 `sched_job_skip | id: … | 机器人开关未开启`（开关关着）——**两者文案不同**，别混。

**变异注入复验**：把 `run_scheduled_job` 里 `SchedGuard::acquire` 失败分支的 `crate::bot::audit_log(...)` 删掉 → 重跑应 **FAIL**（拒绝无声无息）。

> **这一条的教训**：**拒绝也是一种行为**，必须留痕。「没发生」和「发生了但被拒」在用户眼里一样，在日志里必须不一样。

**平台**：跨平台。

---

### M65 · 错过的作业补发 sched-status

**靶点**：`src-tauri/src/bot_scheduler.rs::jobs_tick`（`JobAction::Missed` 分支）（提交 `b88238a5`）

**bug 形态**：错过补跑窗口的作业会发审计（`sched_job_missed`）并通知用户，但**不发 UI 事件**。定时页的徽标靠 `sched-status` 事件即时翻转——不发事件 = **徽标要等下次 reload 才翻成「已错过」**。

这属于「用户看到的是旧状态」：页面开着不动，徽标一直显示「待执行」，直到用户手动刷新。

**步骤**：

1. 建一个**每分钟**触发的定时作业。
2. **停掉应用 / 或把系统时间拨过补跑窗口**，让某个 occurrence 被判「已错过」。
3. **保持定时页开着**（不要刷新），看徽标。

**判据**：

- PASS：定时页的徽标**当场**翻成「已错过」（不必 reload）。
- FAIL：徽标一直停在旧状态，**刷新页面后**才变。
- **审计同源**：bot.log 里有 `sched_job_missed | id: … | schedule: … | 到点已超 2h 补跑窗口，跳过不补跑`。

**变异注入复验**：把 `JobAction::Missed` 分支里的 `app.emit("sched-status", …)` 删掉 → 重跑应 **FAIL**（徽标不即时翻）。
单测侧同款变异已覆盖：调度事件面的一致性守卫测试 → 变红。

**平台**：跨平台。

---

### M66 · 完成判据三处归一到 isTaskCompleted

**靶点**：`src/lib/taskScope.ts::isTaskCompleted`（使用点：`homeData.ts::planPhases`、`TaskDetailPanel.tsx`、`TodoCard.tsx`）（提交 `0bdbebfb`）

**bug 形态**：**同屏自相矛盾**。三处判「这张卡完成了吗」的口径不一致：

- 「今日排期」三态按 `completedAt` 判；
- 同屏的池存量 / KPI 按**完成列**判；
- 卡片状态行按 `column === "done" && completedAt` 判。

于是**一张遗留的 done 列卡（没有 `completedAt`）**：在「今日排期」里算**未完成**、在池存量里算**完成**、卡片上绿勾亮着状态行却写「**未完成**」。

统一到 `isTaskCompleted`（按完成列判），`completedAt` 只管**时间文案**（没有完成时间就显示「已完成」）。

**步骤**：

1. 造一张**完成列**的卡（把一张卡拖到已完成列 / 标记完成），但**确保它没有完成时间戳**（老数据形态）。
2. 看首页今日排期的三态、池存量 / KPI、卡片状态行三处。

**判据**：

- PASS：三处口径一致——今日排期里它算**已完成**；卡片状态行显示 `已完成`（不是「未完成」）；池存量与 KPI 与之一致。
- FAIL：同屏两处归属互斥（排期说没完成、KPI 说完成）；或卡片绿勾旁写着「未完成」。

**变异注入复验**：
- 把 `planPhases` 里的 `if (isTaskCompleted(t)) phases.done++;` 改回 `if (t.completedAt) phases.done++;` → 重跑应 **FAIL**（排期口径分家）。
- 把 `TodoCard` 里的 `isTaskCompleted(task)` 改回 `task.column === "done" && task.completedAt` → 重跑应 **FAIL**（状态行显示「未完成」）。

单测侧同款变异已覆盖：`src/components/HomePage/homeData.test.ts` 的「按完成列/doing 列分三态（isTaskCompleted 单源）」→ 变红（该用例专门塞了一张「遗留 done 无 completedAt」的卡）。

> **口径单源的价值**：这类矛盾单测极难覆盖——三处各判各的，每处单测都「通过」，只有同屏对比才暴露。

**平台**：跨平台。

---

### M67 · 导出文件名日期换单源 dayKey + 图谱详情卡走 format

**靶点**：`src/components/HomePage/homeData.ts::dayKey`（使用点：`SettingsPage/MemoryPanel.tsx`）+ `src/components/GraphPage/GraphPage.tsx`（提交 `42262c9f`）

**bug 形态**：两处**本地时区与呈现规则的双实现**。

**其一：导出文件名用 UTC。** 记忆导出文件名是 `new Date().toISOString().slice(0,10)`——**UTC 日期**。在 **UTC+8 的晚 8 点之后**，UTC 日期比本地日期**大一天** → 导出文件名写着「明天」。

**其二：图谱详情卡裸显原值。** 截止时间直接 `{selectedNode.task.due}`（`2026-10-24T09:00` 这样的机器格式），完成时间用 `new Date(...).toLocaleDateString()`（**另一套**呈现规则）。同一个仓库里**两种呈现规则并存**。

**步骤**：

1. 把系统时区设成 **UTC+8**（中国标准时间），把系统时间设到 **20:00 之后**。
2. 设置页 → 记忆面板 → 点导出 → 看**默认文件名**。
3. 图谱页 → 点一个**有截止时间**的节点 → 看详情卡的「截止」与「完成于」两行。

**判据**：

- PASS：导出文件名里的日期 = **本地日期**（不是 UTC 的明天）；图谱详情卡的截止与完成于**用与仓库其它页一致的呈现格式**（不是裸 `2026-10-24T09:00`）。
- FAIL：文件名日期比本地日期大一天（UTC+8 晚 8 点后）；或图谱详情卡显示裸机器格式。

**变异注入复验**：
- 把 `MemoryPanel` 里的 `dayKey(new Date())` 改回 `new Date().toISOString().slice(0, 10)` → 重跑应 **FAIL**（文件名回到 UTC 日期）。
- 把 `GraphPage` 里的 `formatDue(selectedNode.task.due)` 改回 `{selectedNode.task.due}` → 重跑应 **FAIL**（详情卡裸显原值）。

单测侧同款变异已覆盖：`src/lib/taskScope.test.ts` / `format` 相关的呈现规则测试 → 变红。

> **时区陷阱**：**在 UTC 白天验这张卡看不出问题**——只有「本地时区日期 ≠ UTC 日期」的时段才暴露。**验之前先把时间拨到 20:00 之后**。

**平台**：跨平台，但**只在非 UTC 时区才验得到**（本机是 UTC+8，正好）。

---

### M68 · 探活文案分无厂商 / 厂商两条

**靶点**：`src-tauri/src/bot/config/commands.rs::bot_test_connection`（提交 `331f7ad2`）

**bug 形态**：未配 key 时要如实说「这次只探了连通性」，但旧代码用 `vendor.clone().unwrap_or_default()` 拼厂商名——**没有厂商的老条目**会拼出 `厂商「」还没配 API Key`。

两处错：**空名厂商**（读着像坏数据）+ **指路方向错**（没有厂商的老条目该配**全局主 Key**，文案却让用户去「厂商页填该厂商的 Key」——那里根本没有厂商可填）。

**步骤**：

1. 设置页 → API 厂商 → 造一个**没有厂商归属的老条目**（无厂商名），**不配 key** → 点「测试连接」。
2. 再造一个**有厂商名**的条目，同样不配 key → 点「测试连接」。

**判据**：

- PASS：第 1 步文案是 `全局还没配 API Key：本次只探了端点连通性（未带鉴权），不算连接测试通过。请先在设置里填全局主 Key。`；第 2 步文案是 `厂商「{名}」还没配 API Key：…请先在厂商页填该厂商的 Key。`。
- FAIL：出现 `厂商「」`（空名）；或无厂商条目也让用户去「厂商页」填 key。

**变异注入复验**：把 `bot_test_connection` 里的 `match vendor.as_deref()` 改回 `format!("厂商「{}」…", vendor.clone().unwrap_or_default())` → 重跑应 **FAIL**（空名厂商 + 错方向）。

**平台**：跨平台。

---

### M69 · 运行记录四个写口持写锁

**靶点**：`src-tauri/src/apps/mod.rs::run` / `mark_run_failed` / `sweep_stale_runs`（提交 `25ed736b`）

**bug 形态**：程序运行的**四个写口**（记录开始、回写成功、回写失败、清扫陈旧行）**不持 DB 写锁**。

后果：主窗有长事务持锁时，这四个写口的写会**失败**——而 `run()` 的成功回写路径把写失败**当成整个程序运行失败**上报：**程序明明跑成功了，用户看到的却是「运行失败」**。

（另一半是僵尸行：失败路径不回写 → 状态永远停在 running，上一份文档的 M30 修过一半；本条补的是锁纪律。）

**步骤**：

1. 程序页 → 上架一个程序 → 点「运行」。
2. **同时**在另一个窗口做一次**大批量导入**（制造长事务窗口）。
3. 看程序运行的结果横幅。

**判据**：

- PASS：程序**跑成功**就显示成功（哪怕回写慢）；运行记录里那一行的状态是 `ok` / 成功。
- FAIL：程序跑成功了但横幅是**失败**（或红点）；或运行记录那一行状态不落终态。

**变异注入复验**：把 `run()` 里成功回写块中的 `let _g = db::lock_db_write();` 删掉 → 重跑应 **FAIL**（长事务窗口内回写失败 → 跑成功的程序报错）。
单测侧同款变异已覆盖：`apps::mod` 的写锁守卫测试 → 变红。

> **地面真相**：`sqlite3 <data_dir>/wmessage.db "select status from app_runs order by started_at desc limit 3"` → 状态是终态，不是 `running`。

**平台**：跨平台。

---

### M70 · 六个读命令进阻塞线程

**靶点**：`src-tauri/src/apps/commands.rs`（`apps_list_active` / `apps_run` / `apps_open_output` / `apps_list_archived` / `apps_runs_list` / `apps_versions_list`）（提交 `f2ee3f7b`）

**bug 形态**：这六个命令在 **async 命令函数里直接做同步磁盘 IO**（开库 + 全表查询 + 读目录）。tokio worker 被占住 → **所有异步任务一起卡**（不只是这一个命令）。

在库里数据量大、或磁盘慢（机械盘 / 网络盘 / 容器挂载）时，表现为「点一下程序页，整个应用卡住几秒」。

**步骤**：

1. 造一批数据（几百条程序运行记录 + 几十个程序）。
2. 点程序页的各个页签（列表 / 运行历史 / 版本 / 归档）。
3. **同时**观察另一个窗口的输入响应（打字是否即时）。

**判据**：

- PASS：切页签时**界面不卡**（打字即时、动画不掉帧）；读取慢时表现为「加载中…」而不是「整个应用冻住」。
- FAIL：点一次程序页，整个应用**僵住**几秒（另一个窗口的输入也延迟）。
- **反向必做**：人为把数据目录放到**慢盘**（或用 `taskset` / 限速）→ 卡顿应**明显减轻**（IO 不再占 worker）。

**变异注入复验**：把 `apps_list_active` 的 `spawn_blocking` 改回直接在 async 里同步做 → 重跑应 **FAIL**（界面卡顿回来）。
单测侧同款变异已覆盖：`apps::commands` 的阻塞线程守卫测试 → 变红。

**平台**：跨平台。**Windows 上验卡顿要用「应用无响应」观察**（见 §1.0 ②）。

---

### M71 · 保留期清扫与批量落库进阻塞线程

**靶点**：`src-tauri/src/db/trace.rs::spawn_retention_sweep` + `src-tauri/src/trace_sink.rs::writer`（提交 `07280b91`）

**bug 形态**：两处同步磁盘 IO 在 tokio worker 上跑：

1. **保留期清扫**：`retire_traces_before_days` = 写锁 + 开库 + **全表 DELETE**。
2. **批量落库**：`trace_sink` 的 writer 循环里 `open_db`（全套 DDL + PRAGMA 的同步 IO）+ 批量 insert 事务。

两者都在 worker 上 → 持锁等主窗长事务时，**整个 worker 被拖住**。

**步骤**：

1. 开一个长事务窗口（大批量导入）。
2. 让 trace 观察记录持续产生（跑一轮带工具调用的对话）。
3. 观察应用响应。

**判据**：

- PASS：长事务期间应用**不卡**；trace 记录在事务结束后**照常落库**（不丢）。
- FAIL：应用卡住；或 trace 记录**静默丢失**（writer 被拖垮 / 连接建不出来时丢弃）。

**变异注入复验**：
- 把 `spawn_retention_sweep` 里的 `spawn_blocking` 改回直接同步调用 → 重跑应 **FAIL**（清扫期间 worker 被拖住）。
- 把 `trace_sink` writer 里的 `spawn_blocking` 改回同步 → 同上。

> **丢弃也要有说法**：`trace_sink` 打不开库时会丢弃那一批并打 stderr 诊断——**stderr 在 GUI 启动下看不到**（§1.1 第 1 行），所以真机上「trace 少了」的第一个动作是**用 dev 模式复现看 stderr**。

**平台**：跨平台。

---

### M72 · 查询 schema 删掉工作流 / 定时残留承诺

**靶点**：`src-tauri/src/bot/registry.rs::SCHEMA_QUERY_TASKS` / `SCHEMA_QUERY_SINGLE_TASK`（提交 `6b441c16`）

**bug 形态**：**画布（工作流）退役后**，任务查询工具的 schema 还在承诺「输出行带（工作流：名称）标记，工作流相关问题可按标记汇总回答」与「所属工作流」。

而这些标记**早就不产出了**。schema 是给模型看的契约——**承诺一个不产出的东西，模型就会去找它、找不到就编**。这是本批最典型的「schema 撒谎」。

**步骤**：

1. 在聊天框发：

```
列出我所有带「工作流」标记的任务。
```

2. 再让模型报出 `query_tasks` 与 `query_single_task` 的工具说明。

**判据**：

- PASS：模型**不再**去找工作流标记；工具说明里**没有**「（工作流：名称）标记」与「所属工作流」字样。
- FAIL：工具说明仍承诺工作流标记；或模型**编造**工作流名来回答。
- **落地必查**：`query_tasks` 的输出行里**不该有**工作流标记（早就没了）。

**变异注入复验**：把 `SCHEMA_QUERY_TASKS` 的描述改回带「输出行带（工作流：名称）标记」那句 → 重跑应 **FAIL**（schema 又开始撒谎）。
单测侧同款变异已覆盖：`src-tauri/tests/fixtures/tools_baseline.json` 的 schema 基线对拍 → 变红（描述变了、基线没变）。

> **这类改动的通用验法**：**改 schema 必须同步改基线夹具**——`tools_baseline.json` 就是钉它的。**基线没改 = 门禁会红**；基线改了但实现没改 = 门禁也会红。

**平台**：跨平台。

---

### M73 · 聊天任务池过滤换 isActiveTask 单源

**靶点**：`src/App.tsx::openTasks`（提交 `286720db`）

**bug 形态**：聊天右侧栏的「任务池」用 `t.column !== "done" && !t.archived` 过滤，而任务页的任务池用 `isActiveTask`。两套过滤口径 → **回收站里软删的卡（`deletedAt` 有值）混进了聊天的待办列表**。

用户视角：「这张卡我明明删了，怎么还在待办里」。

**步骤**：

1. 造一张卡 → 删掉（进回收站，**软删**）。
2. 打开聊天面板 → 看右侧栏「任务池」。

**判据**：

- PASS：回收站里的卡**不在**聊天任务池里；任务页与聊天任务池的**同一批卡顺序一致**（两边共用 `sortPoolTasks`）。
- FAIL：软删卡出现在聊天任务池里。

**变异注入复验**：把 `openTasks` 的 `visibleTasks.filter(isActiveTask)` 改回 `visibleTasks.filter((t) => t.column !== "done" && !t.archived)` → 重跑应 **FAIL**（软删卡混进来）。
单测侧同款变异已覆盖：`src/lib/taskScope.test.ts` 的 `isActiveTask` 用例 → 变红。

**平台**：跨平台。

---

### M74 · 工作区三个写口补置线程标记

**靶点**：`src-tauri/src/db/workspace.rs::workspace_upsert` / `workspace_delete` / `workspace_import`（提交 `d6766145`）

**bug 形态**：这三个写口直接取**裸锁**（`lock_or_recover(&DB_WRITE_LOCK, …)`），而全仓其它写口走 `lock_db_write()`——后者除了加锁，还**置了线程标记**。

**线程标记干什么**：闭包内调**契约函数**时，契约断言靠这个标记判断「当前是不是在写事务里」。裸锁 → 标记没置 → 闭包内的契约断言**误判**（以为不在事务里，实际在），断言报错或走错分支。

这与「上一份文档里 `with_write_conn` 改用 `lock_db_write`」是同一类修复——**锁不只是锁，还是一个上下文标记**。

**步骤**：

1. 触发工作区的三个写口：工作区里**新增一项**（upsert）、**删一项**（delete）、**导入**一份工作区 JSON。
2. 看这三个动作有没有报错。

**判据**：

- PASS：三个动作**都成功**，无契约断言报错。
- FAIL：某个动作报出契约断言类的错误（消息形如「不在事务里」之类）。

**变异注入复验**：把这三处的 `super::lock_db_write()` 改回 `crate::db::lock::lock_or_recover(&super::DB_WRITE_LOCK, "db::workspace DB_WRITE_LOCK")` → 重跑应 **FAIL**（契约断言误判）。

**平台**：跨平台。

---

## 六、Windows 专章

### W04 · MCP 按需启停在 Windows 上：调用完即回收

**靶点**：`src-tauri/src/bot/mcp/manager.rs`（`Slot::Idle` / `retire`）+ `src-tauri/src/bot/mcp/mod.rs`（提交 `efd5d481`）

**为什么 Windows 专章**：按需启停的整个收益来自「**用完即关**」——进程留在内存里就等于没做。这件事在 Windows 上**有一个独立于实现的坑**：回收子进程要走 `taskkill /T`（**树杀**），只杀父进程的话，npx / node 拉起的**孙进程会留下来继续吃内存**。

**步骤**：

1. 配置一台外部 MCP 服务器（stdio 型，命令是 `npx -y @modelcontextprotocol/server-everything` 或类似会**再拉子进程**的）。
2. 启用它 → 让模型**真调一次**它的工具。
3. 调用结束后**立刻**查进程树。

**判据**：

- PASS：调用结束后 **server 与它的子进程都没有了**（`Get-Process | Where-Object {$_.ProcessName -like "*node*"}` 里不再有它）；MCP 面板状态回到 `idle`。
- FAIL：**孙进程残留**（父进程没了、`node` 还在）——内存白吃，正是这次改造要消灭的东西。
- **面板 ≠ 事实**：面板说 `idle` 不算，要看进程真的没了。

**变异注入复验**：把回收路径里的**树杀**改成只杀父进程 → 重跑应 **FAIL**（孙进程残留）。

> **不主动出 Windows 包**：修完先在现有环境（macOS）过一遍，有问题直接反馈；出包只在明确要求时做（换 exe 还要连带 SHA256 与路径两处记账）。

**平台**：Windows 专有。

---

### W05 · 导入路径闸在 Windows 上

**靶点**：`src-tauri/src/db/tasks.rs::check_export_path` + `src-tauri/src/db/workspace.rs::workspace_import`（提交 `7d66f8a6` `20165f0e`）

**为什么 Windows 专章**：路径闸的判定材料（分隔符、`..` 组件、扩展名、结尾形态）与**平台的文件系统语法**耦合。Windows 上多两类东西：**盘符冒号**（`C:`）与 **UNC 路径**（`\\server\share`），还有**路径以 `\` 结尾**（目录形态）。

**步骤**：

1. 在 Windows 上依次选这几个路径做「导入」：
   - 一个**目录**（扩展名为空）；
   - 一个 `..` 组件的路径（例：`D:\tmp\..\wmessage-secrets.json`）；
   - 一个**非 `.json`** 的文件；
   - 一个 `.JSON`（**大写扩展名**）文件——**这条应当放行**（闸是 `eq_ignore_ascii_case`）。
2. 确认导出侧与导入侧**拒的路径一致**。

**判据**：

- PASS：目录 / `..` 组件 / 非 `.json` **三者都被拒**（文案分别是「路径必须以文件名结尾」/「路径不允许包含 `..`」/「必须是 .json 文件」）；**`.JSON` 放行**。
- FAIL：某个该拒的路径被读进去了；或 `.JSON` 被误拒。
- **两侧同源**：导出侧拒的路径，导入侧**同样拒**——「导出侧拒的路径导入侧不能照读」。

**变异注入复验**：把 `workspace_import` 里的 `check_export_path_gate(path.clone()).await?;` 删掉 → 重跑应 **FAIL**（该拒的路径被读）。
单测侧同款变异已覆盖：`db::tasks::import_bounded_tests` 与路径闸的守卫测试 → 变红。

**平台**：Windows 专有（路径语法差异）。

---

### W06 · 导出文件名日期跨时区

**靶点**：`src/components/SettingsPage/MemoryPanel.tsx`（使用 `src/components/HomePage/homeData.ts::dayKey`）（提交 `42262c9f`）

**为什么 Windows 专章**：时区问题的**触发条件是「本地日期 ≠ UTC 日期」**，与平台无关，但**只在非 UTC 时区成立**。本机是 UTC+8，所以 macOS 侧也能验——**这张卡在 Windows 上的意义是：换机器复验时最容易忘了改时区**。

**步骤**：

1. Windows 上把时区设成 **UTC+8**、时间设到 **20:00 之后**（同一天内 UTC 还是昨天）。
2. 设置页 → 记忆面板 → 点导出 → 看默认文件名。

**判据**：

- PASS：文件名里的日期 = **本地日期**（例：本地 10-24 晚上 → 文件名是 `wmessage-memories-20261024.json`，**不是** `20261025`）。
- FAIL：文件名日期比本地大一天。

**变异注入复验**：把 `dayKey(new Date())` 改回 `new Date().toISOString().slice(0, 10)` → 重跑应 **FAIL**。

**平台**：Windows 专有（macOS 侧见 M67）。

---

## 七、变异注入总表

编号接上一份的 V01–V25。**每条变异 = 把修复改坏，重跑对应卡，期望 FAIL**。

| # | 卡 | 注入锚点 | 改坏方式 | 期望现象 |
|---|---|---|---|---|
| V26 | M40 | `bot_ui.rs::SENSITIVE_WHOLE_WORDS` | 删掉 `"pin"` | Spinner 被误拒 |
| V27 | M40 | `bot_ui.rs::gate_sensitive` | `contains_word(&hay, k)` → `true` | 所有输入框都拒填 |
| V28 | M41 | `bot_ui.rs::tool_ui_type` | 删掉锁内 `foreground_verdict(...)?` | 文字打进前台应用 |
| V29 | M41 | `bot_ui.rs::tool_activate_window` | 去掉 `with_inject_lock` | 并发置前与注入互相踩 |
| V30 | M42 | `apps/manage.rs::validate_app_id` | 白名单改回黑名单 | `.` 被接受 → 删整个 `apps/` 根 |
| V31 | M43 | `db/workspace.rs::workspace_import` | 删 `check_export_path_gate` | 非 `.json` 被读 |
| V32 | M43 | `db/tasks.rs::read_import_bounded` | `if buf.len() > max_bytes` → `if false` | 超限文件整读进内存 |
| V33 | M44 | `db/tasks.rs::apply_task_patch` | 删 tags 两处上限检查 | 超限标签进库 |
| V34 | M44 | `db/tasks.rs::apply_task_patch` | 删「空数组清除」分支 | 标签清不掉 |
| V35 | M45 | `bot/tools.rs::due_param_denied` | 删 `due_format_error` 调用 | 空格格式被接受 → 通知静默不发 |
| V36 | M45 | `api_handlers/validate.rs::check_due` | 返回 `None` | HTTP 侧失守 |
| V37 | M46 | `bot/tools.rs::tool_create_task` | 非法列值改回 `_ => Todo` | 非法列值静默降级 |
| V38 | M46 | `bot/tools.rs::tool_complete_task` | 删 `next.bot_assigned = None` | 完成卡仍显示机器人执行中 |
| V39 | M47 | `bot_skills/manage.rs::effective_skill_md` | 取最后一份副本 | 开关点了没反应 |
| V40 | M47 | `bot_skills/manage.rs::write_enabled_to` | 解析不出就用原文写回 | 写回读不回目标值 |
| V41 | M48 | `bot_skills/builtin.rs::materialize_at_startup` | 删 `purge_retired_builtin_skills` | 僵尸技能留在列表 |
| V42 | M48 | `bot_skills/builtin.rs::purge_retired_skills_in` | 去掉「在册则跳过」 | 在册技能被误删 |
| V43 | M49 | `bot_skills/runtime.rs::sweep_expired_pauses` | 删 Running 超时分支 | 死会话继续占位同名技能 |
| V44 | M49 | `bot_skills/runtime.rs::skill_conflict_with_existing` | `Running => true` | 超时的 Running 仍拦同名 |
| V45 | M50 | `bot/mcp/manager.rs::apply_config` | 启用分支转 `Connected` | 开关一开就起常驻进程 |
| V46 | M51 | `bot/mcp/manager.rs` 工具挂载 | 不读 `ToolCache::load` | 重启后模型看不见工具 |
| V47 | M51 | `bot/mcp/commands.rs::mcp_server_delete` | 删 `purge_tool_cache` | 快照留孤儿条目 |
| V48 | M52 | `bot/dispatch.rs::WRAP_UP_TOOLS` | 删 `"write_artifact_file"` | 触顶后零产出 |
| V49 | M52 | `bot/dispatch.rs` 收尾窗分支 | `if !WRAP_UP_TOOLS.contains(&name)` → `if false` | 收尾窗内读类工具仍放行 |
| V50 | M53 | `db/tasks.rs::db_delete` | 删 `emit("tasks-changed")` | 删除后界面不更新 |
| V51 | M53 | `db/tasks.rs::tasks_import` | 删 `emit("tasks-changed")` | 导入后界面不更新 |
| V52 | M54 | `src/App.tsx::reloadTasks` | `mutating.chain.then(run)` → `run()` | 导入结果被在途写盖掉 |
| V53 | M55 | `TaskDetailPanel.tsx` Esc 裁决 | 删 `if (escBlocked) return;` | 命令面板开着时 Esc 连浮窗一起关 |
| V54 | M55 | `TaskDetailPanel.tsx` 标题 textarea | 删 `e.stopPropagation()` | 标题编辑态 Esc 把浮窗关了 |
| V55 | M56 | `AppsPage.tsx::RunsList` | `.catch(() => setLoadError(true))` → `setRuns([])` | 失败又变回空库 |
| V56 | M57 | `ask_engine.rs::ask_respond` | 删 `lock_db_write()` | 长事务窗口内应答丢失 |
| V57 | M58 | `bot_orchestrator.rs::own_card_payload` | 键名改回 `budget` | 静态键复活 |
| V58 | M58 | `bot_orchestrator.rs::own_card_payload` | `toolCallsUsed` 报上限 | 报的是上限不是已用 |
| V59 | M59 | `src/lib/subagentActivity.ts::applyActivityEvent` | 删「按 toolCallId 收口」 | 同一个调用出现两行 |
| V60 | M60 | `SubagentRowLine.tsx::toolCallsText` | 删 `toolCallsUsed == null` 判据 | 终态行工具调用段不消失 |
| V61 | M61 | `bot_orchestrator.rs::run_subagent` | 删 usage 落库 | 终态三列 NULL |
| V62 | M62 | `bot/registry.rs::SCHEMA_READ_TEXT_FILE` | 描述改回「分页读」 | 模型回到分页读法 |
| V63 | M63 | `bot_scheduler.rs::classify_job` | 删保鲜窗口判定 | 过期重试被派发 |
| V64 | M64 | `bot_scheduler.rs::run_scheduled_job` | 删防重入审计 | 拒绝无声无息 |
| V65 | M65 | `bot_scheduler.rs::jobs_tick` Missed 分支 | 删 `emit("sched-status")` | 徽标不即时翻 |
| V66 | M66 | `homeData.ts::planPhases` | 改回 `if (t.completedAt)` | 排期口径分家 |
| V67 | M66 | `TodoCard.tsx` 状态行 | 改回 `column === "done" && completedAt` | 绿勾旁写「未完成」 |
| V68 | M67 | `MemoryPanel.tsx` | `dayKey(new Date())` → `toISOString().slice(0,10)` | 文件名回到 UTC 日期 |
| V69 | M68 | `bot/config/commands.rs::bot_test_connection` | 改回 `unwrap_or_default()` | 空名厂商 + 错方向 |
| V70 | M69 | `apps/mod.rs::run` 成功回写块 | 删 `lock_db_write()` | 跑成功的程序报错 |
| V71 | M70 | `apps/commands.rs::apps_list_active` | `spawn_blocking` 改回同步 | 界面卡顿回来 |
| V72 | M71 | `db/trace.rs::spawn_retention_sweep` | `spawn_blocking` 改回同步 | 清扫拖住 worker |
| V73 | M72 | `bot/registry.rs::SCHEMA_QUERY_TASKS` | 描述改回带工作流标记 | schema 又开始撒谎 |
| V74 | M73 | `src/App.tsx::openTasks` | `filter(isActiveTask)` 改回旧条件 | 软删卡混进待办 |
| V75 | M74 | `db/workspace.rs` 三个写口 | `lock_db_write()` 改回裸锁 | 契约断言误判 |

> **表比卡片多**：M40 / M41 / M43–M49 / M51–M53 / M55 / M58 / M66 这 15 张卡各给了**两条**变异（正反两向：一条打掉主修复、一条打掉反向保护），其余 20 张卡各一条。**一条卡配一条变异是下限，不是上限**——正反两向都验，才知道闸的边界在哪儿。
>
> **变异编号不跟卡号对齐**（V26 之后是连续排的，不是 V26→M40 这样一一对应）——查表时按**卡列**找。

---

## 八、本批涉及的审计事件名总表

判据落在「审计里出现哪条事件」上的卡片，对照这张表 grep `bot.log`。

| 域 | 事件名 | 出处 |
|---|---|---|
| **技能** | `skill.enabled_changed` | M47（拨杆改开关） |
| | `builtin_skill_retired` | M48（退役副本清理成功） |
| | `builtin_skill_purge_failed` | M48（副本清不掉，带原因） |
| | `builtin_skill_seeded` / `builtin_skill_updated` | M48（物化） |
| | `skill.start` | M47 / M49（技能启动） |
| **MCP** | `mcp.tool_cache_write_failed` | M51（快照写盘失败） |
| | `mcp.tool_cache_purge_failed` | M51（删服务器清快照失败） |
| | `mcp.discover_failed` | M51（发现失败进冷却） |
| **子 agent** | `subagent_budget_tool_calls_exceeded` | M52（首次触顶） |
| | `budget_wrap_up_deny` | M52（收尾窗内被拒的调用） |
| | `budget_tool_calls_exceeded` | M52（预算闸） |
| **调度** | `sched_job_skip` | M64（防重入拒绝 / 机器人开关关闭） |
| | `sched_job_retry_expired` | M63（过期重试被丢弃） |
| | `sched_job_missed` | M65（错过补跑窗口） |
| | `sched_job_run` / `sched_job_done` / `sched_job_err` | M63 / M64 / M65（作业生命周期） |
| **数据面** | `tasks_import` | M53（导入审计，带 count 与 path） |
| | `tasks_export` | M53（导出审计，带 count 与 path） |
| **工具** | `tool.call` / `tool.return` | M52（工具两相位） |
| | `tool.output.over_budget` | M52（输出溢出） |

> **走 stderr（不在 bot.log）**：MCP 长行截断诊断、机密读取失败、工具数超限（>128）、stdio 子进程树停止后仍在运行、trace_sink 打不开库时丢弃批次。

---

## 附录 A：只测不验（不进卡片的 4 条 + 已覆盖的 4 条）

### A.1 不改运行时行为的 4 条

今天 51 个提交里有 4 条改的不是运行时行为，卡片对它们没有观测面：

| 提交 | 改了什么 | 为什么不做真机卡 |
|---|---|---|
| `6fa02464` | 去掉迁移 CAS 落选分支的 stderr 喊话 | **噪声清理**——那是每次连接打开的正常路径，不是异常。它的验收口径是「日志不再刷屏」，且 stderr 在 GUI 启动下根本看不到 → 无观测面 |
| `3cf2c02d` | panic 清重试的取舍写明 + storage 测试格式复原 | 注释与测试格式，**不改运行时行为** |
| `cb72cd50` | 池项对勾恒 false 的口径写明 | 注释，**不改运行时行为** |
| `c2298582` | 补齐上一批漏掉的 rustfmt | 纯格式化。**它的验收口径是门禁**：`cargo clippy` / `cargo fmt --check` 通过 |

> **这 4 条要过门禁**：跑 `bash scripts/test-all.sh`（含 `tests-audit/` 对拍）。`audit_doc_claims.py` 会核对**本文与兄弟文档里引用的每个审计事件名**在源码里真的存在——**本文已纳入该门禁的检查范围**。

### A.2 上一份文档已覆盖并实机复验的 4 条（不在本份重复出卡）

| 提交 | 内容 | 上一份的位置 |
|---|---|---|
| `895c31b1` | 不许操作自己的窗口（读本进程 webview 无障碍树会把应用带崩） | M39 / 附录 D-1 |
| `14f99201` | 敏感闸认得 macOS 原生密码框（密码身份在 AXSubrole） | 附录 D-3 |
| `81b62c27` | 聚焦改置 AXFocused 属性（文本框不支持按压） | 附录 D-2 |
| `e45c8736` | 控件树深度显式请求生效到硬顶 16 | 附录 D-4 |

**四条都已实机复验过**（上一份附录 D 的「✅ 实机复验」记录）。**换 exe 后的复跑顺序**：先复验这四条 → 再跑本份 M40–M74 → 最后 W04–W06。

---

## 附录 B：实机发现清单（待回填）

跑完把发现按这个格式回填。**格式与 `docs/realmachine-regression-test.md` 与上一份变异测试文档的附录 D 一致**，方便日后合并。

| # | 卡片 | 现象 | 级别 | 状态 |
|---|---|---|---|---|
| D-9 | | | | |
| D-10 | | | | |

**级别口径**（沿用兄弟文档）：

- **真缺陷**：真机跑出来的、与卡片判据矛盾的行为
- **观察级**：行为符合预期但设计上别扭、或无法判定
- **已排除**：实机否证了卡片的前提或判据 → **要改卡片**，不是改代码

> **本批的特有观察点**：
> - **M40 / M41 依赖 macOS 两项系统权限**（辅助功能 + 屏幕录制）——缺任一项，现象会退化成「工具一路报错」，**看不出卡片要验的东西**。跑之前先给权限（§1.1 第 8 行）。
> - **M50 / M51 / W04 依赖一台外部 MCP 服务器**——没有就现配一台 `npx` 型 stdio 服务器。
> - **M54 是竞态**——**同一个动作连做 5 次**，至少撞上 1 次才算 FAIL。
> - **M57 / M69 / M70 / M71 依赖长事务窗口**——用**大批量导入**制造。

---

## 附录 C：与兄弟文档的分工

| 文档 | 覆盖窗口 | 侧重 |
|---|---|---|
| `docs/realmachine-regression-test.md` | 2026-10-19 ~ 10-22 | 32 张双平台场景卡 + 9 张 Windows 专章；**本文的写法来源** |
| `docs/realmachine-rerun-2026-10-22.md` | 同上（换 exe 后复验） | 复验手册：跑法、分段顺序、结果回填口径 |
| `docs/realmachine-overflow-test.md` | 溢出回灌 | 工具大结果溢出回灌的边界三连 |
| `docs/realmachine-mutation-test-2026-10-23.md` | 2026-10-23 00:00 → 20:02 | 42 张卡（39 双平台 + 3 Windows 专章）；**每张卡带变异注入锚点**；D-1~D-4 是实机跑出来的四条真缺陷 |
| `docs/realmachine-mutation-test-2026-10-24.md`（本文） | 2026-10-23 20:02 → 2026-10-24 10:51 | 38 张卡（35 双平台 + 3 Windows 专章）；**每张卡带变异注入锚点**；延续 V01–V25 的变异编号 |

**重叠的地方**：M53（导入导出审计）与上一份的 S 序列里「数据管理」那张卡交叉（那一张验行为，本文这一张验变异 + 审计事件）。**不重复的部分以本文为准**——兄弟文档里那一张没有变异锚点。

**单位测试层的对应物**：`scripts/mutation-trace-funnel.sh`（注入 → 跑 `cargo test` → KILLED / SURVIVED）。本文是**真机层**的同构物：注入 → 跑真实场景 → 看判据是否 FAIL。
