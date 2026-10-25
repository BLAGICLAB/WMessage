# 实机复验手册：2026-10-22 修完 D-2 / D-6 / D-7 之后

> **面向 Windows 实机执行者**。你不需要仓库，只需要：① 绿色包目录；② 探针脚本
> `scripts/regression-probe.ps1`；③ 这份手册 + 场景卡文档 `docs/realmachine-regression-test.md`。
>
> **本手册只讲「这轮改了什么、要复验什么、怎么判」**；每个 S 场景卡的提示词与完整判据
> 在主文档 `docs/realmachine-regression-test.md`（§四 T0 档 / §五 T1 档 / §六 T2 档）。

## 〇、本轮改了什么（源码侧已提交，实机还没验）

| 缺陷 | 现象 | 修法 | commit |
|---|---|---|---|
| **D-2** | `open_url` 的 DNS 失败记 **WARN**，同一故障在 `fetch_url` 记 **ERROR** | 公网闸的错误拆成两类：策略拒绝仍 `Denied`/WARN，解析失败改 `Error`/ERROR | `270e822c` |
| **D-6** | ready 超时被报成「启动后立即退出（退出码 1）」——那个 1 是 `taskkill /F` 的签名 | 就绪判定四态 + **先判后停**（停止前取快照）+ 停止结果如实进文案 | `aa80d2f9` `7f80865f` `a8b54f10` |
| **D-7** | 本地服务面板显示「合计内存 0 MB」（Windows 采不到资源） | 快照 JSON 补 `snapshotSupported`；采不到时 `totalRssMb` 给 `null`；阈值告警在「没判」时**不判成「没超」** | `2eeb707d` |

**这三项的源码侧都做了变异复验**（把修复改回 → 对应测试必须红 → 改回后绿）。
**实机侧一次都还没跑**——所以本文 §1.5 里那三行仍然是 ❌ FAIL，跑完请按 §四 回填。

### 〇之二、2026-10-23 晚间这一轮：技能「刚切就断」

| 缺陷 | 现象 | 修法 | commit |
|---|---|---|---|
| **D-16** | `use_skill` 换技能后**整轮直接收尾**：新技能 0 步被判「已完成」，模型再没被叫醒，本轮零产出（PPT 根本没生成） | 活动快照改「活的优先」（Running > Paused > 终态）+ 同层取最近启动的；切技能时旧 run 出表；DSL 调度器遇接管就收手 | `a55bb74c` |
| **D-17** | 用户点名的技能被 intent 正则截胡：「用 PPT-craft-master 技能做PPT」先命中内置 `pptx-design` 的 `PPT` 正则 → 启错技能 → 模型只能中途切换 → 正踩 D-16 | 路由加「点名规则」并**整体排在 intent 规则之前**（大小写不敏感；只对带 `-`/`_` 的名字生效） | `708507bc` |
| **D-18** | 老库缺 `bot_sessions.onion_config` 两列 → 每会话读 onion 配置必炸 `no such column`，per-onion 配置永远不生效 | `open_db` 补调 `ensure_onion_config_columns` | `fad6ffa6` |

> **另有一条观察级 D-19**（同一次报障里发现的，不在修复范围）：上一版日志在**启动瞬间**有两条
> `no such column`（`sched_last` / `file_path`）——都是 `open_db` 本身失败，但**此后再没复发**，
> 库随后照常工作。已排除「退役列 DROP 失败」与「当前代码还在查这两列」两种解释，
> 剩下的指向用户老库里残留的库对象，**没有实机 DB 可查就没下结论**（详见附录 D-19）。
> 复验时顺带 grep 一下确认还在不在（见 §2.7 末尾）。

**D-16 的实机现场**（2026-10-23 13:33 那次的 bot.log 原文）：

```
[2026-10-23 13:33:10] skill_completed | name: pptx-design | 切换技能结束 | steps: 4
[2026-10-23 13:33:10.335] INFO | skill.start | name: ppt-craft-master | risk=medium | mode=interactive | max_steps=20
[2026-10-23 13:33:10] skill_completed | name: ppt-craft-master | steps: 0        ← 新技能 0 步被判「完成」
[2026-10-23 13:33:10.417] INFO | trace.complete | trace_id=7 | status=done | turns=5 | tools=5 | files=0
```

新技能加载后**再没有下一次 `llm.request`**——模型没被叫醒，整轮零产出（`files=0`）。
根因三处同源：切换时旧 run 被标终态却留在表里 → 活动快照取到那条终态 run → 主循环判「已完成」直接收尾。

**D-17 是这条链的导火索**：用户明明点了名，路由却按 intent 正则启了别的技能，
模型只能中途 `use_skill` 切换——正踩 D-16。修完点名直接命中，根本不需要切换。

这三项的源码侧也做了变异复验（改回 → 对应测试红 → 改回后绿）。**实机侧同样一次都还没跑**——
见本文 §2.7 的那张卡。

## 一、换 exe

### 1.1 mac 侧产物（2026-10-26 出整包时重建，SHA256 随文核对）

上一轮那份 `wmessage.exe`（SHA256 `adebd0d…`，基线 `a20f0ec3`）**跟着 `src-tauri/target/` 整个清掉了**——
本机一度没有可拷的产物，照旧 SHA256 去核对会直接扑空。2026-10-23 出整包时重建过一次
（SHA256 `a5332331…`，基线 `27ccf452`）；**晚间又重建了一次**（`71f5017c…`，基线 `c6fe3560`）；
**2026-10-26 出整包时又重建了一次——当前这份是最新产物**：

```
/Users/renshi/Projects/wmessage/src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe
```

| 项 | 值 |
|---|---|
| 大小 | 76,051,927 B（约 72.5 MB） |
| 格式 | `PE32+ executable (GUI) x86-64 (stripped to external PDB), for MS Windows` |
| SHA256 | `94eb0ae23cba5ba45239b45e65716b5f6dd2ffa793069209b740b1308cc11572` |
| 构建基线 | `0f51232b`（含 10-26 那三个技能中断根因的修复 + release 构建修复） |
| 整包 | **本轮出了新整包**——`wmessage-portable-2026-10-26.zip`（110,485,886 B） |
| 整包 SHA256 | `895de1d88813574794f831de6e7085ac87cc9f732b2cde72d4f1bd8246743169` |

> 已被替换、别再用的旧产物：`71f5017c…`（75,200,216 B，基线 `c6fe3560`，10-23 晚间重建）、
> `a5332331…`（75,081,877 B，基线 `27ccf452`，10-23 出整包那轮）、
> `4fc1cdad…`（74,550,625 B，基线 `76dc0335`，10-22 那轮重建）、
> `e24a91b1…`（74,635,241 B，基线 `1b44d38e`，不含 D-11）、
> `adebd0d…`（基线 `a20f0ec3`，产物已被 target 清理）。
>
> 旧整包 `wmessage-portable-2026-10-23.zip`（SHA256 `e0d98a9237fc4dc2af6d0b15b31f40dbe5d367964bf1e86c01af2d7641bdb2dc`，
> 对应 exe `a5332331…`）同样作废。
>
> **大小不能当校验依据**——同一份源码的两次构建差过 85 KB（74,635,241 → 74,550,625），
> 认 SHA256。

> ⚠️ **本轮（10-26）出包时撞上的坑，动手前先看一眼**：出整包跑的是 release 构建，
> 而**仓库门禁（fmt / check / clippy / nextest）全走 debug profile**——
> `debug_assertions` 恒为真，被 `#[cfg(debug_assertions)]` 门控的项在门禁视角里永远存在，
> release 下才消失。`93658dba`（bot_chat.rs 拆分）留了一条无条件 `pub use` 转出这类项，
> 于是 release 构建 E0432 编译不过，**断了 207 个提交没人发现**。
> 已修（`0f51232b`），打包手册第 7 步的回归检查也从 debug 改成 `cargo check --release`。
> **以后出包前先跑那条 release 检查**，别等第 1 步挂。

重建命令（换机器或产物再丢时用）：

```bash
cd /Users/renshi/Projects/wmessage
npm run build                                          # 前端产物（tauri build 需要 dist/）
npx tauri build --target x86_64-pc-windows-gnu --no-bundle
shasum -a 256 src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe
```

> 大小**不能**当校验依据——同一份源码的两次构建差过 85 KB（74,635,241 → 74,550,625），
> 认 SHA256。

把 `wmessage.exe` 拷到 Windows 机器上，**替换绿色包里的 `wmessage.exe`**。

> **只换 exe**：绿色包里其余内容（`dotnet/`、`onnxruntime.dll`、模型目录、WebView2 运行时）
> 本轮没动，不用重新出包。

> **绿色包放在哪**：2026-10-22 22:45 那次跑的是 `C:\wmessage-portable-2026-10-22`
> （`app_data_dir | path=C:\wmessage-portable-2026-10-22`）——从 `D:\Program Files` 挪到了
> `C:\`，因为 `Program Files` 普通账号写不进去。**下面所有命令里的 `$PKG` 都指你实际放的目录**，
> 换机器时改这一处即可。
>
> **2026-10-26 出了新整包**（`wmessage-portable-2026-10-26.zip`）：解压到
> `C:\wmessage-portable-2026-10-26`，把 `$PKG` 改成它。旧包（`C:\wmessage-portable-2026-10-22`
> 或 `-2026-10-23`）里的 `wmessage.db` 不会自动跟过来——**要旧数据就把旧的 `wmessage.db`
> 与 `AI_Gen_Files` 一起拷过去**（或者新包里直接换 exe，那数据自然还在原地）。
>
> **⚠️ 10-23 那轮实际跑的位置又变了**：13:41 那次的日志写着
> `app_data_dir | path=D:\Program Files\wmessage-portable-2026-10-23`——**10-23 整包被放回了
> `D:\Program Files`**，而且是**以管理员身份跑的**（日志里 `ui_automation_toggle | user=Administrator`），
> 所以 `Program Files` 写得进去、便携语义完整（数据跟着包走）。
> **换成 10-26 整包时同样注意**：放到 `D:\Program Files` 就得以管理员身份跑，
> 否则退回 `%APPDATA%`（见主文档 §1.0 ①）——那是账号问题，不是包的问题。

### 1.2 Windows 侧替换步骤

```powershell
$PKG = "D:\Program Files\wmessage-portable-2026-10-26"   # 按你实际放的位置改

# 1) 确认应用没在跑（否则 exe 被占着，换不进去）
Get-Process -Name wmessage -ErrorAction SilentlyContinue | Select-Object Id, ProcessName

# 2) 备份旧 exe（出问题能退回去）
Copy-Item "$PKG\wmessage.exe" "$PKG\wmessage.exe.bak-2026-10-26" -Force

# 3) 替换（把拷过去的 wmessage.exe 放到 $PKG 下再执行）
Copy-Item "D:\transfer\wmessage.exe" "$PKG\wmessage.exe" -Force

# 4) 校验：认 SHA256（大小不能当校验依据）
Get-FileHash "$PKG\wmessage.exe" -Algorithm SHA256 |
  Select-Object Hash   # 期望 94eb0ae23cba5ba45239b45e65716b5f6dd2ffa793069209b740b1308cc11572
```

### 1.3 重启应用

双击 `wmessage.exe`（**先确认没有第二个实例**——2026-10-22 那次就撞过
`HotKey already registered`：两个实例同时跑，快捷键、端口、数据目录全互相干扰）。

启动后先确认数据目录落点没变：

```powershell
$PS  = "D:\wmessage-probe\regression-probe.ps1"
$PKG = "D:\Program Files\wmessage-portable-2026-10-26"
powershell -ExecutionPolicy Bypass -File $PS locate -ExeDir $PKG
```

> `locate` 会打出活的 `data_dir`（便携目录 vs `%APPDATA%` 回退）。
> **后面所有读日志的命令都以它打出的目录为准**，不要默认就在 exe 同目录。
>
> **顺带确认模型可用**：换包不换配置——绿包里的 `bot-config.json` 可能还记着已下线的
> `MiniMax-M2`，一开口就 404（S01 第一发就栽在这，见主文档 §1.5 的那条备注）。
> **测任何要模型回话的场景前，先在设置页确认激活的模型。**

## 二、先跑这 7 项（修过的那几个）

> 每项跑完顺手打一次水位线：`mark` → 操作 → `report`。
> 通用命令：
> ```powershell
> powershell -ExecutionPolicy Bypass -File $PS mark   -ExeDir $PKG
> powershell -ExecutionPolicy Bypass -File $PS report -ExeDir $PKG
> ```

### 2.1 D-6 · 就绪超时不再被误报成「启动后立即退出」

**提示词**（原样粘贴，一次一件事）：

```
用 start_service 起一个服务：command=python，args=["-u","-m","http.server","8080","--bind","127.0.0.1"]，name 叫 win-ready-timeout，readyPattern="__never_match__"，readyTimeoutSecs=5。
```

**判据**：

- PASS：工具返回说「在 5s 内未就绪，已停止」；**不得**出现「启动后立即退出」，**不得**出现「退出码 1」。
- 地面真相：
  ```powershell
  Get-Content "$PKG\bot.log" -Encoding UTF8 |
    Select-String "service\.(started|stop_requested|exited|stopped|start_failed)" |
    Select-Object -Last 8
  ```
  期望：`service.exited` 那行的 **uptime_ms ≈ 5000**（= readyTimeoutSecs），
  且**带 `stop_reason=ready_timeout`**（本轮新增，用来区分「我们杀的」与「它自己退的」）。
- **对照组**（必须做）：同一个服务换能匹配上的 pattern 再起一次——
  ```
  用 start_service 起一个服务：command=python，args=["-u","-m","http.server","8080","--bind","127.0.0.1"]，name 叫 win-ready-ok，readyPattern="Serving HTTP"，readyTimeoutSecs=30。
  ```
  应当 `service.ready` 且 `elapsed_ms` 只有几百毫秒。

> **两个坑**：Windows 上 `command=python`（`python3` 命中 Store 别名桩，退出码 9009）；
> `args` 必须带 `-u`（否则 stdout 块缓冲，`Serving HTTP` 那行不 flush，readyPattern 永远不匹配）。

### 2.2 D-2 · open_url 的 DNS 失败记 ERROR

**提示词**：

```
用 open_url 打开 http://www.ecsfa.org.cn
```

**判据**：

- PASS：`tool.return` 这条是 **ERROR** 级（与 `fetch_url` 同因同级）。
- 反向对照：让模型 `用 open_url 打开 http://127.0.0.1:8080/` → 必须仍是 **WARN**（门禁拒绝，不是故障）。

**地面真相**：

```powershell
powershell -ExecutionPolicy Bypass -File $PS fixes -ExeDir $PKG
```

该子命令会直接打出 `tool.return | tool=open_url|fetch_url` 的原始行（含级别），
以及 D-1 的路径泄漏检查（应为空）与 D-6 的生命周期行。

### 2.3 D-7 · 本地服务面板不说「合计内存 0 MB」

**步骤**：起一个受管服务（用 2.1 的对照命令那个 `win-ready-ok` 正好），
打开设置页 →「本地服务」。

**判据**：

- PASS：面板**出现**一行灰字「本平台暂不支持进程资源采集——内存 / CPU 显示为空不代表没占用」，
  且**不出现**「合计内存 0 MB」（采不到时这一项整个不显示）。
- FAIL：仍写「合计内存 0 MB」，或既没有能力提示也没有合计内存（空得没解释）。

> 这一项**必须人眼看**：机器人的界面是 WebView2，UIAutomation 读不到 DOM
> （2026-10-22 实测：`ui_inspect` 只给 16 个壳控件，导航项一个都没有）。

### 2.4 D-8 · 截屏不再弹控制台窗口

**步骤**：让模型做一次 UI 自动化（`screenshot` 或 `ui_inspect` 任一）。

**判据**：PASS = **桌面上不再闪出 PowerShell 控制台窗口**（2026-10-22 那次每次截屏都闪一个）。

### 2.5 D-1 · 面板不再显示构建机路径（**2026-10-22 22:45 已复验转 PASS**）

**已验到的证据**：`mcp.builtin_materialized | id: builtin-playwright | name: Playwright | 包: C:\wmessage-portable-2026-10-22\playwright-mcp（就位，10-22 包上的证据）`——路径指向**本包目录**、状态是「就位」，构建机 macOS 绝对路径不再泄漏。

**回归确认**（换新包后顺手看一眼）：设置页 →「MCP 服务器」，看内置 Playwright 那条的包路径。

**判据**：PASS = 路径指向**当前绿色包目录**下的 `playwright-mcp`（即 `$PKG\playwright-mcp`）
或明确说「未随包分发」，**不得**出现 `/Users/...` 这样的 macOS 路径。
探针的 `fixes` 子命令会扫 `bot.log` 里的 `/Users/`、`/home/<user>/` 泄漏（应为空）。

### 2.6 W03 · 端口闸在 Windows 上只诊断且明说（回归确认）

**步骤**：起一个绑 `0.0.0.0` 的服务：

```
用 start_service 起一个服务：command=python，args=["-u","-m","http.server","8080","--bind","0.0.0.0"]，name 叫 win-port-gate，readyPattern="Serving HTTP"，readyTimeoutSecs=30。
```

**判据**：

- PASS：审计有 `service.port_violation_precheck | ... | action=diagnostic | enforced=false`；
  工具返回带 `precheckNote`；服务**没被杀**（端口闸在 Windows 上本就不生效）；
  面板**只有一行**「本平台暂不支持监听端口采集……」提示，**没有**第二行「采集失败」；
  审计里**没有** `service.listening_sample_failed`。

### 2.7 D-16 / D-17 · 技能不再「刚切就断」

**提示词**（原样粘贴，附件带上那个 .docx——一次只做这一件事）：

```
根据文件内容帮我做个PPT，用于职代会上宣贯，用PPT-craft-master技能做
```

**判据**（地面真相 = bot.log 原文 + 落盘文件，**不是**模型自报）——三条同时成立才算 PASS：

1. **点名路由生效**：日志里出现 `pre_step.route_skill | skill=ppt-craft-master`，
   **不是** `skill=pptx-design`（D-17 的修法就是让点名压过 intent 正则：
   内置 `pptx-design` 的 `PPT` 正则不再截胡）。
2. **切了也不断**：若日志里出现 `use_skill`，它后面**必须还有下一次 `llm.request`**（同一 trace_id）；
   **不得**出现 `skill_completed | name: ppt-craft-master | steps: 0` 紧跟着 `trace.complete`
   ——那是 D-16 的现场：新技能 0 步被判「已完成」，模型没被叫醒。
3. **真有产出**：`trace.complete | trace_id=… | status=done | turns=… | tools=… | files=1`
   （`files` 大于 0），且 `AI_Gen_Files\` 下**真的多出 .pptx**（文件系统核对，不是模型说做了）。

**地面真相命令**：

```powershell
# ① 路由与切换：把这几类行按时间捞出来看（改前会看到 use_skill 后直接 skill_completed steps: 0）
Get-Content "$PKG\bot.log" -Encoding UTF8 |
  Select-String "pre_step\.route_skill|skill\.start|skill_completed|use_skill|llm\.request|trace\.complete"

# ② 产出：看文件有没有真落盘（大小/时间对得上，不是模型口头声称）
Get-ChildItem "$PKG\AI_Gen_Files" -Filter *.pptx | Sort-Object LastWriteTime -Descending |
  Select-Object -First 3 Name, Length, LastWriteTime
```

> **顺带确认 D-18（不用做动作，换完 exe 跑一轮聊天后 grep 一下）**：
> 老库补列后，每会话读 onion 配置不再报错——
> `bot.log` 里**不该再有** `bot_onions read_meta_fail … no such column: onion_config`：
> ```powershell
> Get-Content "$PKG\bot.log" -Encoding UTF8 | Select-String "read_meta_fail"
> ```
>
> **再顺带看一眼 D-19（观察级 · 未定位，不修只观察）**：
> 上一版日志在启动瞬间有两条 `no such column`（`sched_last` / `file_path`，见附录 D-19）。
> 换完新 exe 跑一轮后，看还犯不犯：
> ```powershell
> Get-Content "$PKG\bot.log" -Encoding UTF8 | Select-String "no such column"
> ```
> **不出现** → 这条就此划掉；**还出现** → 把下面这段的回填给执行者，一并定位：
> ```sql
> PRAGMA table_info('tasks');
> SELECT type, name, sql FROM sqlite_master WHERE tbl_name IN ('tasks','scheduled_jobs');
> ```

## 三、再跑 S 序列剩余项

### 3.1 跑法（三条，别省）

1. **一项一段**：每项前 `mark` 打水位线 → 只做这一项 → `report` 取这段区间的事件。
   同一轮塞多项时，后一项的失败会把前一项的现象盖掉（主文档 §1.3 铁律三）。
2. **一轮一事实**：应用自报不算数。停了服务要看进程真没了、开了端口要看真被占、
   写了机密要看条目真还在——地面真相在主文档 §1.4 的第三种形态里。
3. **贴原文**：结论列只写 PASS / FAIL / 部分 PASS，证据列贴日志原文。
   「大概好了」不算结论。

macOS / Linux 上的等价命令（Windows 用同名子命令的 `.ps1`）：

```bash
bash scripts/regression-probe.sh mark      # 打水位线
bash scripts/regression-probe.sh report    # 取水位线之后的事件
bash scripts/regression-probe.sh fixes     # 本轮修过的三个缺陷的证据（D-1 / D-2 / D-6）
bash scripts/regression-probe.sh services  # 受管服务的真实进程与监听端口
bash scripts/regression-probe.sh orphans   # 宿主已死、服务还活着的孤儿
```

> `fixes` 是 unix 侧本轮新补的（此前只有 Windows 脚本有）：一条命令拿到
> 「本轮修的东西在日志里的落点」，不用在几千行 bot.log 里手翻。

### 3.2 顺序（前置条件是递进的，别跳着做）

| 段 | 项 | 为什么这个顺序 |
|---|---|---|
| **第一段** | S03 → S04 | 安全边界：MCP 的 SSRF 闸与 keyring 机密。都在「MCP 服务」面板里做完，顺手建好的服务器给后面用 |
| **第二段** | S08 → S10 → S11 → S13 | 静默失效（这批的失败形态都是「看起来正常」）。S08 要反复起停服务，放前面把服务生命周期踩熟；S10 与 S13 都要起 stdio MCP 桩服务器（各存一份桩脚本：S10 那份带服务器标记，S13 那份会先吐一条超长行） |
| **第三段** | S15 ~ S25 | 可感知语义，逐项独立，随时可停 |

> **S07 在 Windows 上跳过**（端口闸真杀是 macOS/Linux 专属）。**S08 的 pid 0 变异注入单独安排**——
> 那条有真实杀伤力（拿 launchd 子树当后代逐个 SIGKILL），别混在别的场景里。

### 3.3 结果回填

> **S01 已经跑过一轮（2026-10-22 22:52，🟡 部分 PASS）**：宽度闸拦住了 `intents: .*`、
> 没弹确认窗、bot.log 里无 `skill_create` 行。**还差两条**：① 技能目录
> `skills/bad-broad-intents/` 不存在（文件系统核对，命令在主文档 S01 卡片里）；
> ② 对照组（合规 intents → 弹高危确认窗，点拒绝 → `用户拒绝了创建，技能未创建`）。
> 另外那一轮顺带暴露了 D-11（准入核拒绝记 ERROR 而非 WARN），修完后重跑 S01 复验级别。

| 项 | 场景卡 | 档位 | 本轮结论 | 证据（原文） |
|---|---|---|---|---|
| S03 | 主文档 §四 | T0 | ✅ **PASS**（执行者核对无误） | 证据原文在执行者一侧 |
| S04 | 主文档 §四 | T0 | 🟡 语义面无异常，**UI 布局缺陷已修待复验**（D-12） | 见主文档 §1.5 的 S04 行 |
| S08 | 主文档 §五 | T1 | 🟡 正常路径 PASS（`stop_reason=tool` 落地）；「停不掉」那半**在同权限机器上不可达**，卡片已重写（D-13） | 见主文档 §1.5 的 S08 行 |
| S10 | 主文档 §五 | T1 | ⬜ 待跑（**卡片前提 2026-10-22 已重写**：原卡要求两台**同名** MCP 服务器，而产品从初版就有名称唯一闸，建不出来——靶子改成两台**同前缀不同名**的服务器，见主文档 S10 卡与附录 D-14） | |
| S11 | 主文档 §五 | T1 | ⬜ 待跑 | |
| S13 | 主文档 §五 | T1 | ⬜ 待跑 | |
| S15 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S16 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S17 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S18 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S19 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S20 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S21 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S22 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S23 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S24 | 主文档 §六 | T2 | ⬜ 待跑 | |
| S25 | 主文档 §六 | T2 | ⬜ 待跑 | |
| **S07** | 主文档 §五 | T1 | **跳过** | macOS/Linux 专属（端口闸真杀），Windows 测不了——见主文档附录 C |

**三条提示词铁律**（主文档 §1.3，写在这里免得翻）：

1. 必须带完整指令外壳（只把裸代码/裸路径粘进对话框，模型会当成普通问题回答，`tools=0`）；
2. 必须要求「原样执行，不许改动任何字符」；
3. 一次只做一件事（同一轮塞多个任务，后一件的失败会把前一件的现象盖掉）。

## 四、结果回填到哪

| 跑完什么 | 回填到哪 |
|---|---|
| §二 的 7 项 | 主文档 `docs/realmachine-regression-test.md` §1.5 表里对应行（把 ❌ 改成 ✅/🟡 并贴原文） |
| §三 的 S 项 | 同上 §1.5（新增行）+ 上面的表 |
| 发现新缺陷 | 主文档 **附录 D**：接着现有编号往下写（真缺陷 / 观察级 / 已排除 三档），格式照 D-1~D-9 |
| 踩到的坑（不是缺陷） | 主文档 §1.5 表下方的「日志里几条不是缺陷、但值得记的」 |

**判据口径**（主文档 §1.4）：

- 判据必须用**地面真相**核对（进程 / 端口 / 凭据管理器 / 通知 / 日志原文）；
- 没有权威出处的说法**不许**写进文档；
- 别只读采样函数的返回值就把调用链读全——D-3 就是这么被实机否证的。

## 五、这轮已知测不了的（不用白费时间）

| 项 | 为什么测不了 |
|---|---|
| **S07** 端口闸真杀 | macOS/Linux 专属（Windows 没有监听 socket 采集路径，闸本就不生效）。**macOS 侧本机已跑通**（2026-10-23，沙箱掐掉 `lsof` 的 exec，脚本 `scripts/sandbox-lsof-test.sh`；顺带查出并修掉真缺陷 D-15）——Windows 侧不用做 |
| **W06 的 UIPI 拒绝分支** | 应用以管理员运行，UIPI 只拦「低→高」，高→任何都不拦 |
| **W07 的降级触发法** | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-3d-apis` 是**推断的、本机未验**；没触发就退回 RDP 人工看 |
| **W08 的「值有没有被改对」** | Windows 凭据管理器只能看条目名，看不出密码（只能验「条目还在不在、没被删」） |

---

## 六、只剩这些要人做（2026-10-23 分诊后的清单）

主文档 §1.6 做了分诊：本机能定的**行为**那半已经跑完（S10 的后端判据本机真子进程跑通，
并做过变异复验）。下面这些是**本机定不了**的——要么要真模型回合，要么要 Windows 平台/权限。
照抄即可，**一次一件事**（同一轮塞多个任务，后一件的失败会把前一件的现象盖掉）。

### 6.1 要真模型回话的 4 条

**S10 · MCP 工具函数名跨断连稳定**（主文档 §五）——代码那半本机已过，这里只剩模型那半：

1. 面板加两台 stdio：命令 `python`，参数**两行**（桩的绝对路径 + 标记 `A` / `B`），
   **名字分别 `p x` 与 `p.x`**。桩脚本用主文档 S10 卡里那份（零依赖，标记会进工具描述与调用结果）。
2. 问：`列出你现在能用的所有工具，把函数名原样列出来。`
   → 期望看到两个：`mcp_p_x_echo` 与 `mcp_p_x_echo_2`（**谁带 `_2` 不固定**，别假设）。
3. 问：`调用函数名 mcp_p_x_echo 的工具，参数 text=probe`（另一个名字再来一次）。
   → bot.log 里 `tool.return | tool=mcp_p_x_echo | ms=… | preview=echo[A]: probe`
   ——**preview 里的字母就是答的那台**，这是地面真相，不是模型自报。
4. 关掉其中一台（等面板状态点变了）→ 再问第 2 步那句 → **剩下的名字必须还是它原来的那个**。
5. 打开它、关掉另一台 → 剩下的名字必须是**另一个**名字。
   → **两次剩下的名字必须不同**：相同就是序号在漂移（= FAIL）。
6. 用刚消失的那个名字调一次 → 期望「已被禁用」而不是「未知工具」。

**S13 · MCP stdio 读侧行长上限**——后端本机已过，只剩宿主 stderr 那行诊断：

```
[mcp] 服务器输出超长行（>1048576 字节）已截断——该行被跳过，后续消息照常处理
```

**GUI 启动看不到这行**，要从 cmd 里直接跑 exe（主文档 S13 卡有完整跑法与桩脚本）。

**S11 · 子 agent 工具预算触达落 `budget_exceeded`**（主文档 S11 卡）——
真派一个烧满工具预算的子 agent。判据：子 agent 状态是 `budget_exceeded`（不是 succeeded/failed），
部分结果照旧进 `result_json.summary`，父 agent 下一轮**读到了「预算触顶」这个原因**。

**S12 · `rollback: none` 时不执行回滚段**（主文档 S12 卡）——A/B 两个技能真跑一遍。
判据：`skill_dsl_rollback_start` 在 A（none）里**没有**、在 B（auto）里有。

### 6.2 要 Windows 平台差异的（各一条）

| 项 | 一句话动作 | 判据落点 |
|---|---|---|
| S08 / S32 | 起一个长跑服务 → 停它 → `Get-Process -Id <pid>` 与 `tasklist` **两处都空** | 进程真没了才算过 |
| S08 权限不匹配版 | 宿主普通权限 + 目标管理员权限，才造得出「停不掉」 | `service.stop_failed` + 状态不写 Exited |
| S18 | 改 MCP 服务器的 token → 看它是否重连 | 旧凭据不再被用 |
| S19 | 聊天 busy 时再发一条 | 收到的是拒答，不是模型答复 |
| S17 | 看 MCP 面板顶部那两行能力提示 | 两行都在，且没有「采集失败」红字 |
| S05 / S06 / D-11 | 重跑 S01 与「技能步骤被拒」那条 | 审计级别是 **WARN** 不是 ERROR |
| S04 / D-12 | 打开任意 MCP 服务器的编辑表单 | 机密提示独占一行；「超时（秒）」标签与输入框同行 |

### 6.3 要 GUI / 系统通知的（原样跑，本机代不了）

S15（另有专项方案 `docs/realmachine-overflow-test.md`）、S16、S20、S21、S23、S24、S25、S26~S29。

> **别做的事**：不要拿「模型说它做成了」当通过——判据一律按主文档 §1.4 的地面真相口径
> （进程 / 端口 / 凭据管理器 / 通知 / 日志原文）。应用自报不算数。
