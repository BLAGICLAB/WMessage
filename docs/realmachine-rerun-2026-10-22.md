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

## 一、换 exe

### 1.1 mac 侧产物（**要重新构建**：上一份已随 `target/` 整个清掉）

上一轮那份 `wmessage.exe`（SHA256 `adebd0d…`，基线 `a20f0ec3`）**跟着 `src-tauri/target/` 一起没了**——
本机现在没有可拷的产物，照旧 SHA256 去核对会直接扑空。重建（mac 交叉编译 Windows 目标，
`mingw-w64` 已在 PATH）：

```bash
cd /Users/renshi/Projects/wmessage
npm run build                                          # 前端产物（tauri build 需要 dist/）
npx tauri build --target x86_64-pc-windows-gnu --no-bundle
shasum -a 256 src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe
```

产物落在 `src-tauri/target/x86_64-pc-windows-gnu/release/wmessage.exe`。
**重建后把上面命令打出的 SHA256 与大小回填到本表**（旧的那两行已作废）。

| 项 | 值 |
|---|---|
| 大小 | _（重建后回填）_ |
| 格式 | `PE32+ executable (GUI) x86-64 (stripped to external PDB), for MS Windows` |
| SHA256 | _（重建后回填）_ |
| 构建基线 | _（重建后回填：填 `git rev-parse --short HEAD`）_ |

把 `wmessage.exe` 拷到 Windows 机器上，
**替换 `D:\Program Files\wmessage-portable-2026-10-22\wmessage.exe`**。

> **只换 exe**：绿色包里其余内容（`dotnet/`、`onnxruntime.dll`、模型目录、WebView2 运行时）
> 本轮没动，不用重新出包。

### 1.2 Windows 侧替换步骤

```powershell
$PKG = "D:\Program Files\wmessage-portable-2026-10-22"

# 1) 确认应用没在跑（否则 exe 被占着，换不进去）
Get-Process -Name wmessage -ErrorAction SilentlyContinue | Select-Object Id, ProcessName

# 2) 备份旧 exe（出问题能退回去）
Copy-Item "$PKG\wmessage.exe" "$PKG\wmessage.exe.bak-2026-10-22" -Force

# 3) 替换（把拷过去的 wmessage.exe 放到 $PKG 下再执行）
Copy-Item "D:\transfer\wmessage.exe" "$PKG\wmessage.exe" -Force

# 4) 校验：大小 + 修改时间（对照 §1.1 回填后的大小）
Get-Item "$PKG\wmessage.exe" | Select-Object Length, LastWriteTime
```

### 1.3 重启应用

双击 `wmessage.exe`（**先确认没有第二个实例**——2026-10-22 那次就撞过
`HotKey already registered`：两个实例同时跑，快捷键、端口、数据目录全互相干扰）。

启动后先确认数据目录落点没变：

```powershell
$PS  = "D:\wmessage-probe\regression-probe.ps1"
$PKG = "D:\Program Files\wmessage-portable-2026-10-22"
powershell -ExecutionPolicy Bypass -File $PS locate -ExeDir $PKG
```

> `locate` 会打出活的 `data_dir`（便携目录 vs `%APPDATA%` 回退）。
> **后面所有读日志的命令都以它打出的目录为准**，不要默认就在 exe 同目录。

## 二、先跑这 6 项（修过的那几个）

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

### 2.5 D-1 · 面板不再显示构建机路径

**步骤**：设置页 →「MCP 服务器」，看内置 Playwright 那条的包路径。

**判据**：PASS = 路径指向 `D:\Program Files\wmessage-portable-2026-10-22\playwright-mcp`
或明确说「未随包分发」，**不得**出现 `/Users/...` 这样的 macOS 路径。
探针的 `fixes` 子命令也会扫 `bot.log` 里的 `/Users/`、`/home/<user>/` 泄漏（应为空）。

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
| **第二段** | S08 → S10 → S11 → S13 | 静默失效（这批的失败形态都是「看起来正常」）。S08 要反复起停服务，放前面把服务生命周期踩熟；S13 要真的起一台 stdio MCP 桩服务器 |
| **第三段** | S15 ~ S25 | 可感知语义，逐项独立，随时可停 |

> **S07 在 Windows 上跳过**（端口闸真杀是 macOS/Linux 专属）。**S08 的 pid 0 变异注入单独安排**——
> 那条有真实杀伤力（拿 launchd 子树当后代逐个 SIGKILL），别混在别的场景里。

### 3.3 结果回填

| 项 | 场景卡 | 档位 | 本轮结论 | 证据（原文） |
|---|---|---|---|---|
| S03 | 主文档 §四 | T0 | ⬜ 待跑 | |
| S04 | 主文档 §四 | T0 | ⬜ 待跑 | |
| S08 | 主文档 §五 | T1 | ⬜ 待跑 | |
| S10 | 主文档 §五 | T1 | ⬜ 待跑 | |
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
| §二 的 6 项 | 主文档 `docs/realmachine-regression-test.md` §1.5 表里对应行（把 ❌ 改成 ✅/🟡 并贴原文） |
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
| **S07** 端口闸真杀 | macOS/Linux 专属（Windows 没有监听 socket 采集路径，闸本就不生效） |
| **W06 的 UIPI 拒绝分支** | 应用以管理员运行，UIPI 只拦「低→高」，高→任何都不拦 |
| **W07 的降级触发法** | `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-3d-apis` 是**推断的、本机未验**；没触发就退回 RDP 人工看 |
| **W08 的「值有没有被改对」** | Windows 凭据管理器只能看条目名，看不出密码（只能验「条目还在不在、没被删」） |
