# 真机冒烟测试方案：2026-10-29 → 10-31 变更回归

针对 **2026-10-28 20:54 → 2026-10-31 16:27** 这批改动的实机验收方案。**macOS 与 Windows 双平台**，两平台的测法有实质差异，见 §1.0——本批有一处**平台级安全能力差异**（Windows 上根本没有 run_python 出网硬闸），必须先读。

与 `docs/realmachine-smoke-test-2026-10-25.md`（覆盖到 `be529600`）**首尾相接、不重叠**：上一份验「10-24 上午之后到 10-25 下午」，这份验「10-28 晚上之后到 10-31 傍晚」。骨架同构——**每张卡都写清「把 bug 注回去，重跑应当 FAIL」的锚点**，证明这套测试有牙齿。

| 项 | 值 |
|---|---|
| 覆盖窗口 | `e6546f8a`（10-28 20:54）→ `42fe53e8`（10-31 16:27，HEAD），**71 个提交 / 183 个文件 / +10 073 −5 344 行** |
| 基线 | `e6546f8a`（上一份冒烟验的最后一版之前；上一份覆盖到 `be529600`，其间另有 20 余个提交由本份首次覆盖） |
| 提交构成 | fix 26 / docs 19 / refactor 11 / feat 10 / test 2 / chore 2 / perf 1 |
| 每日分布 | 10-29 共 22 个 · 10-30 共 22 个 · 10-31 共 27 个 |
| 落点分布 | 后端 `src-tauri/` 129 个文件 · 前端 `src/` 35 个文件 · 文档与门禁 19 个 |
| 行为变更 | **23 条产品面**（feat 10 + fix 16 中 14 条可观测 + 2 条改了审计写侧的 refactor）；另有 19 条 docs、11 条 refactor、2 条 test、2 条 chore、1 条 perf → 不出卡，归附录 A |
| 卡片 | **M93–M115（23 张主卡）+ W13–W15（3 张 Windows 专章）**，覆盖 23 条未验过的产品行为变更 |
| 变异注入 | **39 条（V97–V135）**，总表见 §四（编号接上一份的 V76–V96） |
| 自动化基线 | `bash scripts/realmachine/smoke.sh`（macOS 14 项）——**先跑一遍**：它是「这版能不能起来、链路通不通」的地基，卡片跑在它之后 |
| 配套探针 | macOS/Linux → `scripts/regression-probe.sh`；**Windows → `scripts/regression-probe.ps1`**（与前几份同一套） |
| 兄弟文档 | `docs/realmachine-smoke-test-2026-10-25.md`（上一轮方案）· `docs/realmachine-smoke-report-2026-10-25.md`（上一轮报告）· `docs/AGENT-BUDGET-SIMPLIFICATION-DESIGN-2026-10-28.md`（预算简化八条拍板）· `docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md`（断网 + HTTP 工具整合方案）· `docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md`（工具事实表）· `docs/acceptance/event-names.md`（事件名白名单） |

---

## 〇、这份文档是什么

**是**：一份「怎么在真机上证明 10-28 晚上之后这批改动是对的」的逐项清单。每一项给一条可直接粘贴的提示词（或一个可复制的 UI 操作）、一个能判 PASS/FAIL 的观测点、以及一次变异注入复验（把 bug 注回去，应当 FAIL）。

**不是**：不是单测的替代，也不是「再跑一遍功能」。单测证明纯函数在被喂进特定入参时表现正确；这里撬的是**跨模块接缝**——单测结构性够不到的地方：

| 接缝 | 坏了会怎样 | 单测能不能看见 |
|---|---|---|
| seatbelt 沙箱 ↔ Python 进程 ↔ 内核 EPERM | 闸"生效"只写在提示词里，脚本照样联网出去 | 不能（EPERM 只在真机真跑时出现） |
| 目的地闸 ↔ 重定向链 ↔ reqwest 策略 | 首跳过了闸、302 跳到白名单外域名，绕过去了 | 不能（每跳判定只在真请求里发生） |
| 凭据引用 ↔ keyring ↔ 审计 sink 面 | key 进了 trace span 落盘、或进了 `tool.call` 的 args_preview | 不能（sink 面是运行时产物） |
| 预算简化 ↔ 提示词/schema 文案 ↔ 模型行为 | 模型按 schema 里"50 轮"自我节流，安全网 200 轮形同虚设 | 不能（模型读的是文案，常量是另一份） |
| 回灌/收尾 ↔ SSE ↔ 前端状态机 | 正文半截丢失、判死后新轮被旧轮清场 | 不能（SSE 事件边界与前端代际都在运行时） |
| 子 agent 停滞看门狗 ↔ 工具执行期 | 600s 零余量，长工具跑到一半被判假死强停 | 不能（要真跑一次长工具才撞上） |
| 提问等待环 ↔ 超时竞态 ↔ 通知表 | 用户的回答被扔掉、模型按假设继续、两层各说各话 | 不能（竞态要真并发才复现） |

**选样原则**：只收「错了会静默发生、且当场看不出来」的项。纯文档对拍、纯门禁脚本、纯格式化不在此列（它们归 `bash scripts/test-fast.sh` 管）。**这批 71 个提交里有 11 个 refactor**——其中绝大多数是常量搬家与死码清除，由两个纯搬移核对器 + 一个常量单源门禁兜底（Rust 用 `scripts/verify_pure_move.py`、前端用 `scripts/verify_pure_move_tsx.py`），核对器不认的那部分落到 **M104（模型面文案对齐）** 与 **M114（参数卡退役）** 两张「第二真相源」卡上。

> **上一份已经验过的不在这里重复出卡**：上一份的 18 张主卡（M75–M92）验的是 10-24→10-25 那批改动，与本窗口无交集。**唯一的交集是一处口径**：上一份 M78 验的「思考段回灌开关开与关是两副面孔」，本批 `82a180e7` 把那个开关**整个退役**（改成无条件回灌）——所以 M78 的判据作废，由本份的 M99 接手。

**与「单测变异脚本」的关系**：仓库里已有 `scripts/mutation-trace-funnel.sh`——那是**单元测试层**的变异（注入 → 跑 `cargo test` → 看 KILLED / SURVIVED）。本文是**真机层**的同构物：注入 → 跑真实场景 → 看判据是否 FAIL。两者互补：单测变异能证明「有约束在守」，真机变异能证明「约束在真实环境里真的生效」。

---

## 一、前提条件与通用判据

### 1.0 Windows 实机适配（先读这一节）

Windows 上有几处平台差异会**直接改写测法**，不是「注意一下」的程度。本批**每张卡都要看这一节**，尤其 §1.0 ①——那是一条**平台级安全能力缺失**，不是「注意一下」。

#### ① macOS 的出网硬闸在 Windows 上不存在——这是本批最大的平台缺口

`run_python` 的出网硬闸靠 macOS 的 `sandbox-exec`（`src-tauri/src/py/seatbelt.rs`）：

- profile 是 `(version 1)\n(allow default)\n(deny network*)`（`seatbelt.rs:20`）——`deny network*` 是**前缀通配**，socket 全家（outbound / inbound / bind）一并拒，含 loopback；
- 模块本身是 `#[cfg(target_os = "macos")]` 条件编译（`src-tauri/src/py/mod.rs:27-28`）；
- **Windows 上 `src-tauri/src/py/runtime.rs:921-924` 走的是 `silent_cmd(py)` 分支——无沙箱、无出网硬闸**，「run_python 不联网」只剩提示词层软约束。

这是**拍板过的平台差异**（`docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md` 偏离登记），不是缺陷。但它意味着：

> **M93（seatbelt 硬闸）整卡在 Windows 上跳过**——判据是「脚本的 socket 连接被内核以 EPERM 拒」，Windows 上根本没有这条路径。
>
> Windows 上要验的**不是**「断了网」，而是「**明说断了但实际没断**」——这条缺口登记在 SPEC 的已知缺口里。跑 Windows 专章 W13 时，要亲眼确认「提示词说断网、实际没断」这件事被如实登记，而不是被当成 PASS。

#### ② 数据目录可能不在 `D:\Program Files\...`

便携包的 `data_dir` 判定顺序（`src-tauri/src/paths.rs`）：exe 同目录可写 → 锚定 exe 目录；不可写（如 `Program Files`）→ 回退 `%APPDATA%\com.renshi.wmessage`，并记一条 WARN `data_dir_fallback`。

> **第一件事：确认数据到底落在哪。**
> ```powershell
> powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "D:\Program Files\wmessage-portable-2026-10-31"
> ```
>
> **本批卡片的判据几乎全落在数据目录里**（`bot.log` / `wmessage.db` / 技能目录 / 摘要与记忆文件）——**不知道 `data_dir` 在哪，后面所有观测都读错地方**。

#### ③ 停止阶梯走 `taskkill`

Windows 上没有 unix 那套信号语义，停止走 `taskkill /PID <pid> /T /F`。本批有**四张卡与「/stop 打断」有关**（M107 等待环、M108 赛跑防线、M109 确认窗、M113 许可排队）——Windows 上要对照看 `/T`（树杀）有没有生效。

#### ④ 其余平台差异速查

| 项 | macOS / Linux | Windows |
|---|---|---|
| 键盘 / 鼠标注入 | AX（辅助功能）+ CGEvent | UIAutomation + SendInput（**UIPI 会拦**） |
| 窗口 id | `pid:AXWindows下标`（**下标会复用**） | `pid:hwnd`（hwnd 不复用） |
| 前台判定 | 两级（应用 pid + 窗口标题），标题读不到就退化为「同应用即可」 | `GetForegroundWindow() == hwnd`（精确匹配） |
| 行尾 | `\n` | `\r\n`（**读 `bot.log` 找行要带 `\r` 容忍**） |
| 路径分隔 | `/` | `\`（**桩子与回执里的路径**在 Windows 上是反斜杠） |
| 出网硬闸 | seatbelt `deny network*` | **无**（见 ①） |
| 文件产物落点 | `AI_Gen_Files` | 同（跟着 `data_dir` 走，便携锚定时在 exe 目录旁） |

### 1.1 这批的通用判据（每张卡都按这三条取证）

1. **审计行**：探针 `report` 读水位线之后的事件（`scripts/regression-probe.sh mark` / `report`）。事件名与本卡的「判据」一栏逐字比对——**行格式换了要照 §1.0 ④ 容忍 `\r`**。事件名白名单见 `docs/acceptance/event-names.md`；**本批新增的 5 个名字还没进白名单**，见 §附录 C-1。
2. **库里的地面真相**：只读 sqlite（`file:<db>?mode=ro`，不碰应用状态）。本批常用 `exec_spans`（工具调用逐条：`trace_id` / `turn` / `tool_call_id` / `name` / `args` / `result`）与 `exec_traces`（一次执行：`turn_count` / `tool_calls` / `prompt_tokens` / `cache_read_tokens`）。
3. **界面/气泡的可见结果**：截图。用户能看见的东西才叫「可感知」。

### 1.2 前置：跑之前先备好三样

1. **水位线**：`bash scripts/regression-probe.sh mark`（Windows：`regression-probe.ps1 mark`）——之后所有 `report` 只看这之后的行。
2. **基线冒烟**：`bash scripts/realmachine/smoke.sh`（macOS；**Windows 无对应脚本**，见 W15）。14 项全绿再往下跑；有任何一项红了，**先停下查那项**，不要带着地基问题跑卡片。
3. **一份「本轮要验的配置」清单**：本批把**设置页的「Agent 运行参数」整卡删了**（M114），参数表从 24 行缩到 3 行。**唯一还剩的用户可调项**是：
   - `tools.python_timeout_secs`（Python 执行超时，机器人设置卡，可调，钳 1..600）
   - `trace.retention_days`（执行痕迹保留期，只读展示）
   - `memory.extract_min_interval_secs`（记忆抽取最小间隔，只读展示）

   **卡与卡之间会改开关，改完不复位，后面的卡就全废了**——本批尤其要注意「权限模式」（`perm_mode`）和「目的地白名单」（`allowedDomains`）的改动会**清空全部会话授权**（`invalidate_grants()`，M94 依赖这一点）。

### 1.3 驱动方式（本批沿用上一轮的三件套）

| 手段 | 用途 |
|---|---|
| `scripts/regression-probe.sh`（`locate`/`mark`/`report`/`services`/`orphans`/`keyring`/`fixes`） | 日志水位、事件分组、进程与端口地面真相 |
| 只读 sqlite（`file:<db>?mode=ro`） | 工具调用逐条参数与返回、trace 汇总的 token 记账 |
| 截图 + 剪贴板回车 | 界面与气泡的可见结果；**驱动中文输入用剪贴板 + 回车，别用合成 `kp:return`**（上一轮已见输入法层把它吃掉，伪影不是应用问题） |

**弹窗 60s 窗口**：授权弹窗 / 工具确认弹窗 60 秒无响应即默认拒绝（fail-closed，符合预期）。**自动化驱动时，人手节奏必须 < 60s**，否则会吃到一次「拒绝」并误判成应用缺陷。

> **本批新增一类 24h 弹窗**：提问卡（`bot-ask`）的等待上限是 `ASK_TIMEOUT_SECS = 24 * 3600`（`src-tauri/src/ask_engine.rs:152`），不是 60s。**提问卡不答不算失败**，但会在 M107/M108 里被豁免看门狗——跑卡时注意别把「提问卡挂着」误判成「应用卡死」。

### 1.4 观测点速查

| 看什么 | 去哪看 |
|---|---|
| 审计事件 | `bot.log`（探针 `report`；结构化行是 `[时间戳] LEVEL \| 事件 \| kv`，无级别行是 `[时间戳] 事件 \| kv`，**两种都要认**） |
| 工具调用明细 | `exec_spans`（`args` / `result` / `turn` / `tool_call_id`） |
| 一次执行的汇总 | `exec_traces`（`turn_count` / `tool_calls` / `prompt_tokens` / `completion_tokens` / `cache_read_tokens`） |
| 缓存命中 | `llm.usage` 的 `cache_hit_pct` / `cache_read_tokens` / `input_tokens` |
| 每轮的请求体规模 | `llm.request` 的 `msgs_count` / `model` / `provider` / `reasoning` |
| 熔断三段 | `fuse_wrap_up_enter`（`axis=rounds` \| 工具轴）/ `fuse_wrap_up_deny` / `fuse_wrap_up_rounds` / `fuse` / `soft_warn` |
| 出网目的地判定 | `bot.http_request` 的 `dest` 字段（`product`/`whitelist`/`session`/`asked`/`yolo`/`strict`/`subagent`/`unattended`/`confirm_denied`/`network_error`） |
| 出网硬闸 | `bot.log` 里的 `run_python \| seatbelt_probe \| 出网闸生效…` 与 `run_python err \| kind=seatbelt_unavailable \| …`（**自由文本行，不是结构化事件名**） |
| 提问 / 确认 | `bot-ask` / `bot-ask-resolved` / `bot-confirm`（前端事件，同名双向） |
| 子 agent | `subagent_progress` / `subagent_whitelist_deny` / `onion.stage_attached` / `onion.stage_attach_failed` |
| 规则闸 | `tool_rule.hit`（`action` 取值 `deny` / `ask` / `ask_denied` / `ask_denied_subagent`） |
| LLM 重试 | `llm.retry`（`attempt`）/ `llm.request_failed` / `llm.response` |
| 文件读取 | `bot_fs.read`（`path`/`offset`/`end`/`total`） |

### 1.5 本批的风控常量真值表（判据要对着它数）

全部在 `src-tauri/src/bot/constants.rs`（本批新设的**单一真相源**）。**改了这里而没改这张表 = 判据全错**。

| 常量 | 值 | 行 | 含义 |
|---|---|---|---|
| `MAX_ROUNDS` | 200 | 23 | 轮数安全网（主会话与子 agent 同数） |
| `MAX_FUNCTION_CALLS` | 400 | 25 | 工具调用安全网（同上） |
| `HISTORY_BUDGET_CHARS` | 128 000 | 34 | 会话历史字符预算 |
| `HISTORY_TRUNCATE_TARGET_PCT` | 70 | 37 | 历史截断滞回目标比例（%） |
| `MAX_TOKENS_DEFAULT` / `_CAP` | 16 384 / 32 768 | 40 / 43 | 单轮输出默认 / 硬顶 |
| `TOOL_OUTPUT_BYTES` | 128 KB | 49 | 单条工具回填 payload 上限 |
| `CONTEXT_CHARS_HARD_CAP` | 200 000 | 59 | 单次请求 msgs 总字符硬顶 |
| `TOOL_WINDOW_TRIGGER_CHARS` / `_KEEP_RECENT` | 24 000 / 6 | 64 / 69 | 轮内工具结果滑动窗 |
| `LLM_HEADER_TIMEOUT_SECS` | 60 | 81 | LLM 首包超时 |
| `LLM_MAX_ATTEMPTS` | 3 | 86 | LLM 总尝试数（首次 + 重试） |
| `LLM_RETRY_DELAY_MS` | 1 500 | 91 | 重试间隔（**固定退避，无指数**） |
| `HTTP_REQUEST_TIMEOUT_SECS` / `HTTP_SAVE_TIMEOUT_SECS` | 120 / 300 | 95 / 98 | http_request 超时 / 下载超时 |
| `PYTHON_TIMEOUT_SECS` | 120 | 101 | Python 单次执行默认超时 |
| `PY_MAX_TIMEOUT_SECS` | 600 | 107 | run_python 工具路径天花板 |
| `APPS_MAX_TIMEOUT_SECS` | 1 800 | 111 | apps（程序）天花板 |
| `MCP_TIMEOUT_MAX_SECS` | 600 | 115 | MCP stdio 调用天花板 |
| `READ_MAX_BYTES` | 100 KB | 118 | 文件读取单次上限 |
| `READ_MAX_LINE_CHARS` | 2 000 | 122 | read_text_file 单行渲染上限（**字符**） |
| `READ_PAGE_TARGET_BYTES` | 56 KB | 127 | read_text_file 默认页字节预算 |
| `READ_MAX_LINES` | 2 000 | 129 | 文件读取最大行数 |
| `FUSE_SOFT_WARN_PCT` / `FUSE_WRAP_UP_PCT` | 90 / 95 | 168 / 171 | 熔断阶梯软告警 / 收尾窗阈值（%） |
| `WRAP_UP_ROUNDS` | 3 | 178 | 收尾窗轮数 |
| `WRAP_UP_TOOLS` | 4 项 | 184 | `write_artifact_file` / `write_file` / `edit_file` / `read_own_card` |
| `FINAL_SELFCHECK_ROUND_GAP_DEFAULT` / `_CAP` | 8 / 100 | 206 / 209 | 终稿自检轮次间隔 |
| `VERIFIER_RETRY_MAX_CAP` | 2 | 214 | Verifier 修复重试预算 |
| `SEARCH_MAX_RESULTS_DEFAULT` / `_CAP` / `_MIN` | 6 / 10 / 1 | 221 / 218 / 219 | web_search 条数 |
| `ASK_BUDGET_DEFAULT` | 3 | 224 | ask_user 单次执行可问次数 |
| `SKILL_DEFAULT_MAX_STEPS` / `SKILL_MAX_STEPS_LIMIT` | 100 / 200 | 231 / 234 | 技能步数闸（**仅 auto 模式**） |
| `SKILL_MAX_LIFETIME_SECS` | 3 600 | 240 | 技能绝对寿命上限（**不是墙钟**） |
| `SUBAGENT_MAX_GLOBAL_RUNNING` / `_PER_SESSION_RUNNING` | 3 / 2 | 250 / 252 | 子 agent 并发闸 |
| `SUBAGENT_CHECK_WAIT_MS_CAP` | 600 000 | 255 | `check_subagent` 单次等待上限（ms） |
| `SUBAGENT_STALL_TIMEOUT_SECS` | 600 | 264 | 子 agent 停滞超时 |

> **零余量耦合**：`PY_MAX_TIMEOUT_SECS`(600) = `MCP_TIMEOUT_MAX_SECS`(600) = `SUBAGENT_STALL_TIMEOUT_SECS`(600)。长工具执行期靠 `tool_in_flight` 标记挂起停滞判定兜住——**这三个数必须一起动**（M101 验的就是这个兜底）。

---

## 二、场景卡

### T0 档：安全边界与数据破坏

### M93 · run_python 断网：自证探测 + fail-closed，闸不生效就不跑

**靶点**：`src-tauri/src/py/seatbelt.rs`（`SEATBELT_PROFILE` / `seatbelt_probe_cached`）+ `src-tauri/src/py/runtime.rs:878-925`（接线与平台分支）+ `src-tauri/src/py/mod.rs:27-28`（条件编译）（提交 `728d267d`）

**bug 形态**：`run_python` 此前只有提示词层的「不联网」软约束。**提示词不是安全边界**——模型可以无视、脚本可以自己起子进程绕过、用户装的 skill 里可以塞任意代码。断网口径统一了三处覆盖（主聊天红线 `prompts/system.rs:74`、子 agent 三个 profile `prompts/subagent.rs:134/143/170`、工具描述 `registry/schemas.rs:139`），但三处全是软约束。

改成：macOS 上 `run_python` 的 Python 进程经 `sandbox-exec` 启动，profile 为 `(version 1)\n(allow default)\n(deny network*)`——`deny network*` 是**前缀通配**，socket 全家（outbound / inbound / bind）一并拒，含 loopback，且**随 fork/exec 继承**（脚本起子进程 `os.system("curl …")` 同断网）。

**自证式探测**是这个设计的核心：不能只信「我包了 sandbox-exec」，必须**证明闸真的拦住了**。探测连 `127.0.0.1:1`（loopback 上肯定没人监听的端口）：

| 探测 stdout | 判定 | 理由 |
|---|---|---|
| `SB_ERR 1` | **Available（闸生效）** | errno 1 = EPERM，内核直接拒 socket 连接——**唯一证明形态** |
| `SB_CONNECT_OK` | Unavailable「沙箱内 socket 连接竟然成功——网络闸未生效」 | 竟然连上了 |
| `SB_ERR 61` | Unavailable「ECONNREFUSED 而非 EPERM——deny network* 未拦截」 | 到了内核说明没拦 |
| 其他 / `sandbox-exec` 启动失败 | Unavailable「闸状态无法证明」 | 无法证明即不可用 |

**fail-closed**：探测不出 `Available` 就**拒跑**，报 `Python 出网闸（macOS 沙箱）未生效，已拒绝执行：{why}；这是安全失败（fail-closed），请反馈开发者`。**没有放行开关**——无配置键、无环境变量旁路、无「我确认过了」选项。

**apps 同享**：apps（程序页）走 `runtime::run_python` → `run_python_ungated` → `run_python_at` 同一组装路径（`apps/mod.rs:152`），闸自动同享——「脚本自包含」从**交付约定**变成**技术事实**（`apps` 的 schema 契约同步从「禁联网」改写为「脚本要自包含、不依赖外网」，见 `registry/schemas.rs:278/279`）。

**步骤**：

1. 确认 Python 编程开关已开（设置页 → 机器人设置），权限模式不是 strict。
2. 在聊天里让模型用 `run_python` 跑一段**带联网意图**的脚本（例如 `import urllib.request; print(urllib.request.urlopen("http://example.com", timeout=5).status)`）。
3. 观察两件事：**脚本的实际报错**（应是连接被拒，不是 200），以及 `bot.log` 里的 seatbelt 探针行。
4. 再让模型跑一段**不联网**的脚本（例如算个矩阵），确认**没有被误杀**。

**判据**：

- PASS：联网脚本**失败且报错是权限类**（`Permission denied` / `Operation not permitted` / errno 1），**不是** HTTP 200、**也不是** `Connection refused`（后者说明 profile 没拦）；`bot.log` 出现 `run_python | seatbelt_probe | 出网闸生效（sandbox 内 socket 被拒，探测证明）`；**不联网的脚本正常跑完**（闸不能把一切杀光——那是 fail-open 过头）。
- FAIL：联网脚本**成功拿到 200**（闸根本没生效）；或报错是 `Connection refused`（errno 61，说明 profile 没拦）；或**所有** run_python 都失败（包括纯计算脚本）（闸过严，把正常能力一起杀了）；或日志出现 `run_python err | kind=seatbelt_unavailable | …` **而实际联网是通的**（探测误判 + 已 fail-closed，此时应用行为是对的，但**闸在真机上根本不可用**——按发现登记，不是 PASS）。
- **apps 侧**：在程序页跑一个自带脚本的 app，确认它**同样断网**（apps 同享）。若 apps 能联网而 `run_python` 不能 → 同享链路断了，登记为 P1。
- **子进程半**：让脚本起子进程联网（`os.system("curl -m 3 http://example.com")`），确认**子进程也断网**（`deny network*` 随 fork/exec 继承）。

**变异注入复验**：把 `SEATBELT_PROFILE` 里的 `(deny network*)` 改成 `(allow network*)` → 重跑应 **FAIL**（探测会打出 `SB_CONNECT_OK`，闸被判为未生效 → fail-closed 拒跑；或者若同时把 fail-closed 去掉，则联网脚本直接 200）。
更狠的第二步：把 profile 改回 `(deny network*)`、但把探测脚本的判定从「只认 `SB_ERR 1`」放宽成「任何 `SB_ERR` 都算生效」 → 重跑应 **FAIL**（errno 61 会被误判成闸生效——`seatbelt.rs:157` 的锚点测试 `sandboxed_socket_error_is_eperm_not_refused` 正是防这个）。
同款变异的单测侧由 `seatbelt.rs:127/141/157` 兜着。

**平台**：**macOS 专有**。Windows 上整卡跳过（无 seatbelt，见 §1.0 ①）；Windows 的平台差异登记见 W13。

---

### M94 · 目的地闸与 http_request：白名单、按次授权、每跳都判

**靶点**：`src-tauri/src/bot_web/destination.rs`（`classify` / `check_destination` / `Grants`）+ `src-tauri/src/bot_web/http.rs`（`execute` 的逐跳循环）+ `src-tauri/src/bot/tools/http.rs`（工具层）+ `src-tauri/src/bot/config/types.rs:204`（`allowed_domains`）（提交 `cf57b52b` / `73bdfd62`）

**bug 形态**：新增 `http_request` 工具（GET/POST/PUT/PATCH/DELETE/HEAD + 请求体 + 自定义头 + 下载落盘），模型可以往任意地址发任意请求。**公网闸**（`fetch.rs` 的 `check_public_url`）只挡环回/私网/链路本地，**不挡「用户知不知道这个公网站」**。结果是：模型被诱导把本机文件内容或密钥拼进查询串发出去，事后无迹可寻。

改成**两道独立的闸**：公网闸管「是不是公网」，目的地闸管「这个公网是不是用户知情过的」。

**目的地闸的判定顺序即信任顺序**（`destination.rs:148-193`，纯函数 `classify` 可离线钉住）：

| 序 | 判据 | 结果 |
|---|---|---|
| 1 | `PRODUCT_EGRESS_HOSTS`（**8 个**固定域名：tavily / brave / r.jina.ai / github 系 3 个 / skillhub） | `Allowed("product")` |
| 2 | 用户配置过：`allowedDomains` 白名单 ∪ provider `baseUrl` 的 host ∪ MCP http 端点的 host | `Allowed("whitelist")` |
| 3 | 会话内授权（host × 会话） | `Allowed("session")` |
| 4 | **子 agent 会话** | `Denied("subagent")`——**即使 yolo 也不放行** |
| 5 | **无人值守**（`interactive=false`） | `Denied("unattended")` |
| 6 | 权限档：`yolo` 放行 / `strict` 拒 / `ask`·`auto` 弹窗 | 各自对应 |

**按次弹窗的授权粒度**是 host × 会话：批准后**同会话同 host 不再问**，但**没有「始终允许」**——持久信任只有设置页白名单一条路，授权面不积累第二套账（`destination.rs:12-13`）。**任何配置写入都清空全部会话授权**（`invalidate_grants()`，`bot/config/commands.rs:395/484/524` 三处调用）——用户删掉白名单里的域名后，靠弹窗拿到的会话授权也不该继续有效。

**每跳都判**是这张卡最容易漏的一半：`http_request` 手工跟随重定向（reqwest 的自动重定向被 `Policy::none()` 关掉，`bot_web/http.rs:68`），**每一个跳的目标 host 都重新过一遍目的地闸**（`http.rs:117`）——首跳过了闸、302 到白名单外域名不能成为绕过路径。每跳还重跑公网闸并重新钉 IP。

**步骤**：

1. 设置页 → 机器人设置，在「联网工具目的地白名单」里加一个域名（一行一个，支持 `*.example.com` 通配）。
2. 让模型 `http_request` 请求一个**白名单内**的域名 → 应当**直接放行，不弹窗**。
3. 让模型 `http_request` 请求一个**白名单外**的域名 → 应当**弹窗**（`bot-confirm`，文案「访问域名 {host}（批准仅本会话内有效；长期信任请在设置页加入白名单）」）。**批准**，再请求**同一个 host** → 应当**不再弹窗**（会话授权命中）。
4. 换一个**白名单外**的 host 再请求 → 应当**又弹窗**（授权粒度是 host × 会话，不是会话）。
5. **配置写入失效**：在设置页改任意一项配置并保存 → 再请求第 3 步获批过的 host → 应当**又弹窗**（授权被整体作废）。
6. **每跳判定**：找一个会 302 到白名单外域名的地址（本地起一个临时 302 服务即可，或用一个已知跳转到别处的公开短链），让模型 `http_request` 它 → 应当**被拒**（不是「首跳合法就放行」）。
7. **saveAs**：让模型下载一个 CSV 类文件并 `saveAs`，确认文件**原样落盘**在 `AI_Gen_Files`、返回绝对路径与字节数、**不返回正文**。

**判据**：

- PASS：第 2 步 `bot.http_request` 的 `dest="whitelist"` 且**无弹窗**；第 3 步 `dest="asked"` 且**有弹窗**、批准后请求成功；第 4 步 `dest="asked"` 又弹一次；第 5 步**又弹一次**；第 6 步 `dest` ∈ {`strict`/`confirm_denied`/`subagent`/`unattended`} 且**被拒**（`bot.http_request` 落 **Warn** 级）；第 7 步文件落盘、路径存在、`bytes` 与实际大小一致、正文**未**出现在返回里。
- FAIL：白名单外**不弹窗**直接放行（闸不存在）；或白名单内也弹窗（白名单不生效）；或弹窗批准后**下次仍弹**（会话授权没记上）；或**换个 host 不弹**（授权粒度错成「按会话一次」）；或改配置后授权**仍然有效**（失效机制没接上）；或 302 到白名单外**放行**（每跳判定漏了——**这是本卡最关键的一条**）。
- **子 agent 半**：让子 agent（`spawn_subagent`）去请求白名单外域名 → 应当**直接被拒**、`dest="subagent"`，**不弹窗**（`destination.rs:166-173`；注意这条**优先于 yolo**）。
- **无人值守半**：走后台执行路径（`interactive=false`）请求白名单外域名 → `dest="unattended"`，**不弹窗**直接拒。
- **通配符半**：白名单加 `*.example.com`，请求 `a.example.com` 应**放行**；请求 `example.com`（裸 apex）应**不放行**（`host_matches` 明确不让裸 suffix 命中）；请求 `notexample.com` 应**不放行**。
- **产品出口半**：请求 `api.tavily.com` 之类 → `dest="product"`，不弹窗。

**变异注入复验**：把 `classify` 里 `if user_hosts.iter().any(...)` 那行删掉（白名单不生效）→ 重跑应 **FAIL**（白名单内外一个样）。
更精准的变体：把 `http.rs:117` 的 `check_destination` 调用挪到循环**外面**（只判首跳）→ 重跑第 6 步应 **FAIL**（302 到白名单外被放行）。
同款变异的单测侧由 `destination.rs:299-381` 的 `classify` 用例与 `bot_web/tests.rs` 兜着。

**平台**：跨平台（目的地逻辑与平台无关）。**注意 `fetch_url` 不走目的地闸**——D1 拍板不做，SPEC 记缺口，见 §附录 C-2。

---

### M95 · 凭据引用与四处审计旁路面：key 一次都不许落盘

**靶点**：`src-tauri/src/bot/tools/http.rs`（`resolve_secret` / `audit_event_kv` / `preview_args` / `redact_result_text`）+ `src-tauri/src/bot/dispatch.rs:21-40`（`sink_args_preview` / `sink_result_text`）+ `src-tauri/src/bot_web/http.rs:76-91`（响应头脱敏）（提交 `fa794e24`，基座 `cf57b52b`）

**bug 形态**：模型要给外部 API 发鉴权头。**明文 key 写进 `headers` 参数 = key 进对话上下文、进 trace span 落盘、进审计日志**。这是一条典型的「凭据经模型面泄露」链路。

改成**引用形式**：`"keyring:<厂商名>"`（设置页给该厂商配过的 key）或 `"env:<变量名>"`（启动前设好的环境变量）。引用形式下凭据**不进模型上下文、不进审计、不进产物**。读不到就**明确报错**（「读厂商 {vendor} 的 key 失败」/「环境变量 {name} 不存在」），不静默填空。

**但引用形式只堵了模型面**——运行时还有**四个 sink 面**会碰明文：

| sink 面 | 原形态 | 改后 |
|---|---|---|
| `tool.call` 的 args_preview | 前 80 字符**必含完整 URL（含查询串）** | `sink_args_preview` 裁成 `"{METHOD} {host}{path}"` |
| **trace span 的 args** | 记**完整原始入参**（Authorization 明文 + 查询串落盘） | 改走同一份 `sink_args_preview` |
| `tool.return` 的 preview | `result.text` 前 80 字（首行「来源：」带完整 final_url 查询串） | 改走 `sink_result_text`（「来源：」行去查询串与 fragment） |
| **子 agent 动作流 return 相位** | 同上 | 同上 |

**模型可见的 tool result 不脱敏**——模型需要知道重定向后的最终落点，这是有意的分工（审计面脱敏、模型面不脱敏）。

**步骤**：

1. 设置页给某个厂商配好 key（或在启动应用前 `export MY_TEST_KEY=...`）。
2. 让模型 `http_request` 带上 `headers: {"Authorization": "Bearer keyring:<厂商名>"}`。
3. 观察四处：`bot.log` 的 `tool.call` 行、trace span 的 args 列、`tool.return` 行、子 agent 动作流（若在子 agent 里做）。
4. 再试**明文** key（`"Bearer sk-abc123..."`），对比落盘面。

**判据**：

- PASS：引用形式下，四处 sink 面**均不出现** key 的任何片段（明文值、base64 片段、前后缀都不许）；`bot.http_request` 的 kv **只有 5 个键**：`host`/`method`/`status`/`bytes`/`dest`（**没有** headers、没有 body、没有完整 URL）；响应头里的凭据类只回**名 + `⟨值不回显⟩`**（`set-cookie` / `www-authenticate` / `proxy-authenticate`）。
- FAIL：key 的任意片段出现在 `bot.log`、trace span args、或 `tool.return` preview 里；或 `bot.http_request` 的 kv 多了 `url`/`headers`/`body` 之类。
- **已知残留面（不算 FAIL，但要如实登记）**：**明文** key 会进 `tool.call` 的 args_preview（前 80 字符内）——`preview_args` 只裁 URL 不裁 headers。schema 描述因此劝阻「鉴权值建议用引用形式……明文值会留在对话上下文里，能不用就不用」。**明文路径本身没有被堵**。
- **反向半**：用 `"keyring:不存在厂商"` → 应当**明确报错**说读不到，**不能**静默发一个空 header 出去。

**变异注入复验**：把 `dispatch.rs` 的 `sink_args_preview` 改回裸的 `args.chars().take(80)` → 重跑应 **FAIL**（完整 URL 的查询串回到审计面）。
更精准的变体：把 `trace span` 那处（`dispatch.rs:829`）从 `sink_args_preview` 改回 `clamp_span_text(app, name, "args", args.to_string())` → 重跑应 **FAIL**（**只有 trace 这一面漏**，`tool.call` 看着还是对的——这正是本卡要防的「同一个明文，四条路只堵了三条」）。

**平台**：跨平台。Windows 上 `env:` 引用要求**启动应用前**设好环境变量——中途设无效。

---

### M96 · read_text_file 双闸：单行 2000 字符 + 每页 56KB

**靶点**：`src-tauri/src/bot_fs/read.rs:225`（`render_read_slice`）+ `src-tauri/src/bot/constants.rs:122/127`（`READ_MAX_LINE_CHARS` / `READ_PAGE_TARGET_BYTES`）（提交 `effedc6f` / `0f10ebe6`）

**bug 形态**：两个洞。

**洞一（单行）**：minified 产物、单行 JSON 这类文件，**整行进上下文**且**没有「这行被截断了」的标注**——模型不知道自己拿到的是残缺的，还可能把截断处的拼接当成真实内容。原先只有 100KB 磁盘闸与 2000 行行数闸，两道都拦不住「1 行 3MB」。

**洞二（页大小）**：默认页是**固定 500 行**。中文一行 60 汉字 ≈ 180 字节，500 行 ≈ **90KB**——**必撞 64KB 回灌漏斗转存**，模型拿到的是头尾节选而不是整页，页大小与回灌漏斗**单位不一致**（一个按行、一个按字节）。

改后：**单行超 2000 字符**渲染时截断并标注 `{前 2000 字符}…[本行超 2000 字符已截断]`；**默认页改字节预算 56KB**——与回灌漏斗同单位（64KB 的 7/8，余量给行号与提示语），中文页自动窄、英文页自动宽，**一页永不撞 64KB 转存**。

**步骤**：

1. 造一个**单行超长**的文件（`python3 -c "open('/tmp/long.txt','w').write('A'*5000)"`），让模型 `read_text_file` 读它。
2. 造一个**多行**文件（中文，每行 60 汉字 × 300 行 ≈ 54KB；再做一个 3000 行的），让模型 `read_text_file` **不传 limit** 读它。
3. 对照：读一个**纯英文**的多行文件（同样行数、字节数接近），看页大小差异。
4. 观察 `bot_fs.read` 的 `offset`/`end`/`total` 三个 kv。

**判据**：

- PASS：单行超长文件返回的行**带 `[本行超 2000 字符已截断]` 标注**，且模型**知道自己拿到的是截断的**（问它「这行完整吗」应答「被截断了」而不是复述残段当全文）；不传 limit 时中文页**明显窄于**英文页（字节预算在起作用）；`bot_fs.read` 的 `end` 落在 56KB 预算内（`total` 远大于 `end` 时页是被字节预算收的，**不是**被行数收的）；**模型能接着往下读**（用返回的 `offset={end+1}` 续读拿到后续内容）。
- FAIL：超长单行**原样返回且无标注**；或不传 limit 时中文页与英文页**页大小相同**（说明还在按行数收）；或中文页**超过 64KB**（说明字节预算没生效，模型拿到的是转存节选）；或模型把被截断的行**当完整内容复述**。
- **口径半**：显式传小 `limit`（如 `limit=10`）时**不受字节预算影响**（`byte_capped=false`），这是设计如此——不是缺陷。

**变异注入复验**：把 `render_read_slice` 里的 `line.chars().take(READ_MAX_LINE_CHARS)` 改成不截断（`line.chars()`）→ 重跑第 1 步应 **FAIL**（标注也没了）。
第二个变体：把 `READ_PAGE_TARGET_BYTES` 改回按行数（删掉字节预算分支）→ 重跑第 2 步应 **FAIL**（中文页不再变窄 / 撞回 64KB 转存）。
注意：截断按**字符**不按字节——2500 汉字截断后保留 2000 字符 = 6000 字节，无残字（`bot_fs/tests.rs` 的 `read_slice_line_cap_counts_chars_not_bytes` 兜着）。

**平台**：跨平台。路径分隔符不同（见 §1.0 ④）。

---

### T1 档：静默失效与状态谎报

### M97 · 预算简化整套：墙钟没了、档位没了、旋钮没了，兜底还在不在

**靶点**：`src-tauri/src/bot/constants.rs`（新安全网常量）+ `src-tauri/src/bot/params.rs`（参数表 24 → 3 行）+ `src-tauri/src/bot/config/types.rs`（BotConfig 十键删除）+ `src/components/SettingsPage/BotCoreSection.tsx`（整卡删除）+ `src-tauri/src/bot_model_loop/fuse.rs:59`（`gate_call` 单点判定）+ `src-tauri/src/bot_skills/parse.rs`（`timeout_secs` 废弃）+ `src-tauri/src/bot_skills/runtime.rs`（`skill_timed_out` 改寿命口径）（提交 `705a4285` / `8f33a09a` / `6655ab52` / `e4087f9e` / `36ac4ce3` / `82a180e7` / `387d6dbb` / `57aa6154` / `b7e4f1fd`）

**bug 形态**：上一代是「三档 × 六预算 + 60/20/15 比例派生 + 墙钟」——**配置面极宽，且每加一个拨杆就多一处「页面说 A 实际跑 B」的机会**（历史上已发生三次）。本批把整套装退役，改成**开发者常量**。

**退役清单**（这批删掉的东西比留下的多一个量级）：

| 退役项 | 原值/原语义 | 现状 |
|---|---|---|
| 墙钟（主会话 + 子 agent + 技能链路） | 子 agent 墙钟预算、60% 份额、软墙钟 10% 预留窗 | **全链路无会话级时间预算**；时间兜底改由工具级超时 + 停滞超时承担 |
| 档位（轻量/均衡/深度） | 每档定下六个数 | **无档位无预设**；`task_mode` 配置键与设置页三档选择器一并删除 |
| 子 agent 独立预算 | 轮数/工具数/墙钟三轴 + LLM 可自报额度 | **与主会话共用安全网**；`spawn_subagent` 的 `maxTurns`/`maxToolCalls`/`maxWallSeconds` 三个参数**从 schema 删掉**，派发方加减不了额度 |
| 工具回填 payload 第三态 | 档位 64/128/200KB + 可配 + `0 = 不截断` | **固定 128KB 不可配**，`0 = 不截断`整条路径退役 |
| 思考段回灌开关 | 默认关（要求回灌的模型首次必 400） | **无条件回灌**（仅 OpenAI 兼容路径） |
| 设置页「Agent 运行参数」整卡 | 24 行可编辑/只读参数 | **整卡删除**；参数表只剩 3 行（见 §1.2） |
| 技能墙钟 | `timeout_secs` frontmatter（默认 1800s / 上限 3600s） | **frontmatter 废弃**（解析忽略并告警一次）；改为绝对寿命上限 3600s（**含 Paused 等确认的时间**，是 Paused 槽位泄漏保护，不是性能预算） |

**留下的兜底**（必须验它们真的在）：

- `MAX_ROUNDS = 200` / `MAX_FUNCTION_CALLS = 400`——主会话与子 agent **同一个数**；
- `SUBAGENT_STALL_TIMEOUT_SECS = 600`——子 agent 没有墙钟后的**唯一时间维度**；
- 熔断三段阶梯：`FUSE_SOFT_WARN_PCT = 90` → `FUSE_WRAP_UP_PCT = 95` → 硬熔断（**带正文**）。

**步骤**：

1. 打开设置页 → 确认**「Agent 运行参数」整卡已不在**（截图存证）；确认任务模式三档选择器**已不在**。
2. 翻 `bot-config.json` → 确认这 12 个键**已不在**：`maxRounds` / `maxFunctionCalls` / `historyBudgetChars` / `historyTruncateTargetPct` / `reasoningEchoEnabled` / `inTurnContextEdit` / `searchMaxResults` / `askBudget` / `maxTokens` / `reasoningEffort` / `taskMode` / `maxToolOutputBytes`（另有 `subagentMaxWallSecs` / `subagentMaxTurns` / `subagentMaxToolCalls` 也在本批删除）。
3. **老配置迁移**：拿一份**含旧键**的 `bot-config.json`（从备份里取）覆盖进去，启动应用 → 确认**不报错**、旧键被忽略或清掉、其余键完好。
4. 跑一轮正常对话，确认**没有任何「预算」相关报错**。
5. 造一个**跑飞**的场景（让模型陷入死循环式探索，比如反复读同一批文件）→ 观察轮数/工具调用两条安全网是否在 200/400 处**带正文停下**（不是静默截断）。
6. 技能侧：造一个带 `timeout_secs: 1800` 的旧技能 → 启动时应**告警一次**「frontmatter timeout_secs 已废弃」，且技能**照常跑**（不是被当成超时杀掉）。

**判据**：

- PASS：设置页无「Agent 运行参数」卡、无任务模式选择器；`bot-config.json` 里旧键**全部消失**；带旧键的配置文件启动**不报错**（能起来、其余配置完好）；正常对话**零预算报错**；跑飞场景在 200 轮或 400 次工具调用处**停下且带正文**（正文不是空的）；旧技能的 `timeout_secs` 只告警一次、不影响执行。
- FAIL：设置页仍能看到 Agent 参数卡或三档选择器（整卡没删干净）；旧键导致**启动失败**或读不出其它配置（迁移没做）；正常对话报「预算不足」之类（配置键删了但消费侧还在读）；跑飞时**正文丢失**（硬熔断把产出清零——这正是设计原则 3 要防的）；旧技能因 `timeout_secs` **被杀掉**。
- **口径半**：参数表剩下 3 行，其中 `tools.python_timeout_secs` **可编辑**、`trace.retention_days` 与 `memory.extract_min_interval_secs` **只读**。三行是「页面显示 = 实际生效」契约的维护面——**改这三行之外的东西没有 UI**。

**变异注入复验**：把 `fuse.rs` 的 `gate_call` 里「收尾窗放行的写类调用**不计**计数」改回「照计」→ 重跑第 5 步应 **FAIL**（写类调用永远撞不到熔断线，模型在收尾窗里继续干活直到轮数硬停）。
第二个变体：把 `constants.rs` 的 `MAX_ROUNDS` 改回 50 → 重跑应 **FAIL**（跑飞场景的停点变了）。

**平台**：跨平台。**Windows 注意**：设置页整卡删除后要确认没有残留的空容器或错位布局（截图比对）。

---

### M98 · 轮数轴收尾窗：最后 3 轮只放写产物类，出口是轮数不是丢弃

**靶点**：`src-tauri/src/bot_model_loop/request.rs:402`（`rounds_wrap_up_at`）+ `src-tauri/src/bot_model_loop/fuse.rs:59/75/80`（`gate_call` / `wrap_up_rounds_exhausted`）+ `src-tauri/src/bot/constants.rs:178/184`（`WRAP_UP_ROUNDS` / `WRAP_UP_TOOLS`）（提交 `7e79b684` / `6b589f0e`）

**bug 形态**：预算简化批落地后全量审计发现，设计稿写「两轴各三段」，**实现里轮数轴只有两段**（90% 软警告 → 100% 硬停）——**硬停丢正文**恰是设计原则 3 要防的「产出归零」。工具轴有收尾窗，轮数轴没有。

改后：轮数轴也进收尾窗——`max_rounds - 3` 轮起（200 轮时**第 198 轮**进窗），窗内**只放行写产物类**（`WRAP_UP_TOOLS` 四项：`write_artifact_file` / `write_file` / `edit_file` / `read_own_card`），**出口是轮数自然硬停**（`max_rounds`），**带正文**。

**两轴独立**：工具轴（95% 计数）与轮数轴（`max_rounds - 3`）任一进窗即生效，**先进窗者说了算**。**「写产物类」是显式白名单数组**，不是工具分类表。

**统一后的单点判定**（`6b589f0e` 的收敛）：改前主会话在 95% 处**拒读却把被拒的也计数**，还把放行的写**回退计数**（写永远计不到熔断线），且 dispatch 另有一道按 100% 预算拒读的闸，两道闸各算各的。改后——预算内计数执行；进收尾窗后写类放行**不计**、探索类拒绝**不计**，**计数停在阈值**。

**步骤**（**这卡要临时改常量**，改完记得复位）：

1. 临时把 `MAX_ROUNDS` 调小（建议 **10**，`WRAP_UP_ROUNDS = 3` 不变 → 第 8 轮进窗），让收尾窗在几轮内可见。
2. 让模型做一个**需要多步探索**的任务（例如「读这个目录里所有 `.log` 并汇总」），让它持续探索到撞轮数轴。
3. 观察：`fuse_wrap_up_enter`（`axis=rounds`）→ 探索类工具被拒（`fuse_wrap_up_deny`，`axis=rounds`）→ 模型转向写产物 → `fuse_wrap_up_rounds` → 带正文硬停。
4. **对照半**：把 `MAX_ROUNDS` 调到 **6**，让**工具轴**先撞（`axis` 无 `rounds`），确认两轴事件可区分。

**判据**：

- PASS：出现 `fuse_wrap_up_enter` 且 **`axis=rounds`**；进窗后**探索类工具被拒**（`fuse_wrap_up_deny` 带 `axis=rounds`）；**写产物类仍放行**（模型能真的把发现写成文件）；用尽 3 轮后出现 `fuse_wrap_up_rounds`，最终**带正文**停下（正文非空、包含模型的实际产出）；工具轴对照半出现 **`axis` 不带 `rounds`** 的 `fuse_wrap_up_enter`（两轴可区分）。
- FAIL：轮数轴**没有收尾窗**（撞到 200 轮直接硬停、正文丢失）；或进窗后**探索类仍放行**（白名单失效）；或写产物类**也被拒**（模型没法收尾，产出归零——同一后果的另一条路径）；或收尾窗**无限延续**（`fuse_wrap_up_rounds` 不出现，窗口没有出口——轮数必须能把它关掉）；或 `axis` 键缺失（两轴分不清，本卡判据作废）。
- **改完记得复位** `MAX_ROUNDS` 回 200。

**变异注入复验**：把 `fuse.rs:75` 的 `wrap_up_rounds_exhausted` 的比较从 `round > r0 + WRAP_UP_ROUNDS` 改成 `round > r0 + 99`（窗口几乎无限）→ 重跑应 **FAIL**（`fuse_wrap_up_rounds` 不出现，模型在收尾窗里继续干活）。
第二个变体：把 `WRAP_UP_TOOLS` 里的 `write_artifact_file` 删掉 → 重跑应 **FAIL**（模型写不出产物，收尾窗变成「只许读不许写」的牢笼）。

**平台**：跨平台。

---

### M99 · 思考段无条件回灌：开关没了，协议要求还在

**靶点**：`src-tauri/src/bot_model_loop/core.rs`（回灌盖章点）+ `src-tauri/src/bot_model_loop/types.rs`（`ModelLoopDeps` 撤两旗标）+ `src-tauri/src/bot/registry/schemas.rs`（`SCHEMA_RUN_PYTHON` 描述同步）（提交 `82a180e7`，接上一份的 M78）

**bug 形态**：带思考模式的模型把思维链放在 `reasoning_content` 字段里，**带 tools 的请求必须把它随 assistant 消息完整回传，缺了上游直接 400**。DeepSeek 官方原话：「携带了 tools 参数的请求，在后续所有请求中，必须完整回传 reasoning_content 给 API……若未正确回传，API 会返回 400 报错」，且明确「即使该轮模型未实际进行工具调用」也要回传。

上一轮（10-25）落地时这是个**默认关的开关**——**用一次线上失败换一次「记得去开开关」，不划算**（要求回灌的模型第一次必 400）。本批改成**无条件回灌**：上一轮流里有 `reasoning_content` 就原样带回，不再查表、不再查开关。

**覆盖范围比「工具轮那一条」宽**——自检返工、幻觉守卫返工、长度续跑这三条路径都会再发一次请求，它们推回的 assistant 消息同样要带。盖章点统一落在「消息栈最后一条 assistant 消息」上，更早的内容一个字节不动（隐式前缀缓存不受影响）。**Anthropic 协议走 thinking 块原样回传，不受此开关影响**（本开关本来就只管 OpenAI 兼容路径）。

**同批另一件事**：轮内工具结果滑动窗（`loop.in_turn_context_edit` 旗标）**也一并退役成恒开**——`request.rs:434` 的 `if deps.in_turn_context_edit && !*tool_window_applied` 改成 `if !*tool_window_applied`。

**步骤**：

1. 找一个**开思考模式**且走 **OpenAI 兼容端点**的模型（模型库里已 enabled 的 DeepSeek V4 Flash 即可）。
2. 在聊天里让它**调一次工具**（例如「列出我的待办任务」）。
3. **再追问一轮**，让第二轮带上第一轮的 assistant 消息（触发多轮工具调用）。
4. 观察上游是否 400，以及 `bot.log` 的 `llm.request`（`provider` / `reasoning` / `msgs_count`）。

**判据**：

- PASS：全程 **零 400**；`llm.request` 的 `provider` = openai 兼容、`reasoning` 字段在位；模型**引用上一轮自己的结论**时内容正确（说明 `reasoning_content` 被带回去了、没把消息栈搞坏）。
- FAIL：上游 400 且错误指向 `reasoning_content` 缺失（回灌没生效或盖章点错了位置）；或回灌**盖到了每一条 assistant 消息**上（`llm.request` 的 `msgs_count` 不变但**流量大增**——缓存前缀被改写）。
- **Anthropic 路径对照**：用 Anthropic 端点的模型跑同样的多轮工具调用 → 应**不受影响**（走 thinking 块原样回传），零 400。
- **上一份 M78 的判据已作废**：不再有「开关开/关两副面孔」，本卡只验「恒回灌不炸」。

**变异注入复验**：把盖章点从「消息栈最后一条 assistant 消息」改成「给每一条 assistant 消息都盖」→ 重跑应 **FAIL**（请求体暴涨、缓存前缀被改写）。
第二个变体：把 `if http.provider == ApiProvider::Openai && !reasoning_text.trim().is_empty()` 的 provider 判断删掉 → 在 Anthropic 路径上重跑应 **FAIL**（或 400、或 thinking 块重复）。

**平台**：跨平台，但**必须用 OpenAI 兼容端点的模型**才验得到（Anthropic 路径不受此改动影响）。

---

### M100 · LLM 重试 3 次 + 超时三档天花板：120 / 600 / 1800 各管各的

**靶点**：`src-tauri/src/bot/constants.rs:86/91/101/107/111`（`LLM_MAX_ATTEMPTS` / `LLM_RETRY_DELAY_MS` / `PYTHON_TIMEOUT_SECS` / `PY_MAX_TIMEOUT_SECS` / `APPS_MAX_TIMEOUT_SECS`）+ `src-tauri/src/bot_model_loop/request.rs:641-715`（主重试循环）+ `src-tauri/src/py/runtime.rs:40-61`（`resolve_timeout_with_cap`）（提交 `fb1bd0ac` / `19d23d2b` / `256fea25`）

**bug 形态**：两个方向相反的问题。

**LLM 重试**：`LLM_MAX_ATTEMPTS` 从 2 提到 **3**（总尝试数 = 首次 + 2 次重试），与 OpenAI / Anthropic 官方 SDK 默认口径一致（`max_retries=2` 即 3 次尝试）。退避是**固定的 1500ms**，**无指数退避**。

**超时**：`MAX_TIMEOUT_SECS` 此前**同时**是 run_python 工具路径的天花板**和** apps 单次运行的天花板。apps 是程序页用户直跑、无子 agent 停滞判定（600s 停滞上界只约束工具路径），而 SPEC 拍过 30 分钟（转写长录音场景）——**不该随工具路径收紧**。改成两条独立天花板：

| 路径 | 默认 | 上限 | 常量 |
|---|---|---|---|
| run_python 工具 | **120s** | **600s** | `PYTHON_TIMEOUT_SECS` / `PY_MAX_TIMEOUT_SECS` |
| apps（程序） | 120s | **1800s** | `APPS_MAX_TIMEOUT_SECS` |
| MCP stdio | — | 600s | `MCP_TIMEOUT_MAX_SECS` |

**五档天花板在本批末收进单一真相源**（`256fea25`）：`PY_MAX_TIMEOUT_SECS` / `APPS_MAX_TIMEOUT_SECS` / `MCP_TIMEOUT_MAX_SECS` / `HTTP_REQUEST_TIMEOUT_SECS` / `HTTP_SAVE_TIMEOUT_SECS` 全部搬进 `bot/constants.rs`，各自原居所的常量已删。

**重试的两条路径与最终失败**：

| 失败类型 | 重试条件 | 耗尽后 |
|---|---|---|
| 网络/发送失败 | `attempt < 3` → 审计 `llm.retry`（`err`/`attempt`）→ 检查 `/stop` → sleep 1500ms | `CommandError::LlmRequestFailed` → 前端「请求大模型失败：{detail}」+ **「重试」入口**（`recoverable=true`）；审计 `llm.request_failed`（Error，含 `attempt`） |
| 429 / 5xx | `attempt < 3` → 审计 `llm.retry`（`status`/`attempt`）→ 检查 `/stop` → sleep 1500ms | `CommandError::LlmApiError{status}`；429→「上游限流：请稍后重试」，5xx→「上游服务异常：请稍后重试」；审计 `llm.response`（Warn，含 `status`） |

**重试只在流式产出开始前**——已开始流式产出后不重试（已执行的工具在 msgs 里不重放）。

**步骤**：

1. **超时口径**：让模型跑一个 `timeoutSecs=900` 的 run_python → 应当被**钳到 600s**（日志 `run_python | timeout clamped | requested=900 cap=600`）。
2. **apps 口径**：在程序页把某个 app 的 `timeoutSecs` 设成 **1500** → 应当**不被钳**（`cap=1800`），而同样的值在 run_python 工具路径会被钳到 600。
3. **LLM 重试（正向难构造）**：改 `baseUrl` 指一个**返回 503 的本地假服务** → 让模型发一轮 → 观察 `llm.retry` 出现 **2 次**、`attempt` 到 3、上游收到 **3 次**请求、最终 `llm.response`（Warn / `status=503`）。
4. **LLM 重试（`/stop` 打断）**：同上（503 服务），但在第 1 次重试等待期间按 `/stop` → 应当**不再发下一次请求**（`RequestOutcome::Stopped`）。

**判据**：

- PASS：第 1 步日志出现 `timeout clamped | requested=900 cap=600`；第 2 步 apps 的 `cap=1800` 且**未被钳**（同一数值在工具路径被钳——**两条路径确实分开了**）；第 3 步 `llm.retry` **恰好 2 条**、`attempt` 递增到 3、假服务收到 **3 次**请求、最终 `llm.response` 带 `status=503`；第 4 步 `/stop` 后假服务**只收到 1 次**请求。
- FAIL：第 1 步**无 clamped 日志**（钳制没生效，900s 真的跑了 900s）；第 2 步 apps **也被钳到 600**（两条天花板没分开）；第 3 步 `llm.retry` **0 条或 1 条**（重试没做）；或出现 **4 次以上**请求（重试次数算错，「3 次尝试」被当成「3 次重试」= 4 次尝试）；或第 4 步 `/stop` 后**仍发了第 2 次**。
- **口径半**：重试间隔固定 1500ms，**无指数退避**——观察两次重试的时间差应当**相同**（若看到 1.5s → 3s 的指数，那是另一套实现，与常量表不符）。

**变异注入复验**：把 `request.rs:692` 的 `if attempt < MAX_LLM_ATTEMPTS` 改成 `if attempt < 5` → 重跑第 3 步应 **FAIL**（请求数变 5 次）。
第二个变体：把 `resolve_timeout_with_cap` 的 `None => (DEFAULT_TIMEOUT_SECS, false)` 改成 `None => (cap, false)` → 重跑第 1 步应 **FAIL**（不传 timeoutSecs 时默认变成 600s，而不是 120s）。

**平台**：跨平台。

---

### M101 · 子 agent 停滞判定的三处修法：长工具不该被判假死

**靶点**：`src-tauri/src/tool_guard.rs:81/173`（`tool_in_flight` / `set_subagent_tool_in_flight`）+ `src-tauri/src/bot/subagent_runner.rs:511/528-561`（watcher 判定与取消检测）+ `src-tauri/src/bot_model_loop/shell.rs:277-284`（标记翻转）+ `src-tauri/src/bot_archive.rs`（pack/unpack 心跳）（提交 `0a4eae66` / `20b3e3f5` / `1c91a49d`）

**bug 形态**：子 agent 没有墙钟了（本批退役），**600s 停滞超时是它唯一的时间维度**。但判定本身有三处漏：

1. **零余量误杀窗**：`PY_MAX_TIMEOUT_SECS` = 600 = `SUBAGENT_STALL_TIMEOUT_SECS` = 600。工具执行期**不发流式增量**，看门狗只认流式事件 → 长工具跑到 600s 时**刚好撞上阈值**，被判「假死」强停，产物半截。
2. **DB 读失败误判取消**：watcher 查子 agent 状态用 `load_subagent(&conn, &sid).ok().flatten()...unwrap_or(true)`——**读失败（忙/锁竞争）按终态处理** → 健康的 run 被误停。
3. **pack/unpack 无心跳**：打大包/解压是**纯 CPU/IO**，既没有流式事件也没有工具开始/返回 → 超过 600s 必被判停滞。

**三处修法**：

| 漏 | 修法 |
|---|---|
| ① 零余量 | `tool_guard.rs` 加 `tool_in_flight` 标记；`shell.rs` 在 `bot-tool-running` / `bot-tool-done` 时翻转；watcher 判定前置 `!tool_running && idle_ms >= 600_000` 才判停滞 |
| ② 误判取消 | `unwrap_or(true)` → `unwrap_or(false)`（**读失败按「未取消」继续跑**，真取消由下一 tick 复查兜住）——与排队路径 `runner.rs:179` 的容错方向对齐 |
| ③ 无心跳 | pack/unpack 的**逐条目循环**接 `touch_subagent_progress`（`bot_archive.rs` 内 5 处：unpack_zip / unpack_tar / collect_files / pack_zip / pack_tar） |

**步骤**（**这卡要临时改常量**）：

1. 临时把 `SUBAGENT_STALL_TIMEOUT_SECS` 调小（例如 **20**），让停滞判定在几秒内可见。
2. 让子 agent 跑一个**长工具**：一个 `run_python` 睡眠脚本（`time.sleep(40)`，超过 20s 阈值） → 应当**不被判停滞**，正常跑完。
3. 让子 agent 跑一个**真正卡死**的场景（如 `run_python` 死循环 + 不产出） → 观察 20s 后是否被判停滞并停止（`bot.log` 出现 `subagent_stalled | id: … | idle_ms: … | 停滞超时，已强制停止`）。
4. **pack/unpack 半**：让子 agent 打一个**大包**（构造一个几百 MB 的目录），观察它在打包过程中**不被判停滞**（逐条目心跳在续命）。
5. **DB 读失败半**：这一步工程上难以自然触发——用 `BEGIN EXCLUSIVE` 持锁的方式制造读失败（或直接注入变异），观察**子 agent 不被误停**。
6. **改完记得复位** `SUBAGENT_STALL_TIMEOUT_SECS` 回 600。

**判据**：

- PASS：第 2 步长工具**跑完**、子 agent 终态是「完成」而非「停滞」；第 3 步真卡死场景在阈值后**被判停滞并停止**、日志出现 `subagent_stalled`；第 4 步大包**打完**；第 5 步读失败时子 agent **继续跑**（不被误停）。
- FAIL：第 2 步长工具**被强停**（误杀窗没堵住——零余量耦合漏了）；或第 3 步**永远卡着不停**（看门狗彻底失效）；或第 4 步打包**中途被强停**（心跳没接）；或第 5 步读失败时子 agent **被误停**（`unwrap_or` 方向反了）。
- **零余量的口径**：本卡通过时，`PY_MAX_TIMEOUT_SECS` 与 `SUBAGENT_STALL_TIMEOUT_SECS` **必须同时是 600**——本批的注释明写「两个数必须一起动」（`constants.rs:105-107` / `:261-264`）。若你在本卡里把一个改了而另一个没改，**那本身就是缺陷**，登记。

**变异注入复验**：把 `shell.rs:279-283` 的标记翻转删掉（`tool_in_flight` 永远是 false）→ 重跑第 2 步应 **FAIL**（长工具被误杀）。
第二个变体：把 `runner.rs:557` 的 `.unwrap_or(false)` 改回 `.unwrap_or(true)` → 重跑第 5 步应 **FAIL**（读失败误停）。

**平台**：跨平台。

---

### M102 · 子 agent 的三道「直接拒不弹窗」：批准了也无效的窗不该弹

**靶点**：`src-tauri/src/bot/dispatch.rs:378-405`（规则闸 ask 分支子 agent 短路）+ `src-tauri/src/bot_web/destination.rs:165-173`（目的地闸子 agent 直拒）+ `src-tauri/src/bot/dispatch.rs:700-719`（执行计数挪到入参 JSON 闸之后）（提交 `c5be8f61` / `78485a8c`）

**bug 形态**：子 agent 是**后台会话**，弹窗**无人认领**（要等 600s 才被接替）。两种「弹了也没用」的空转窗：

1. **规则闸 ask 分支**：某工具按 per-tool 规则表需要用户确认 → 无条件弹窗 → 用户（或主 agent 转述）批准了 → 但**子 agent 白名单闸仍会拒** → 白弹一个「批准了也无效」的窗。
2. **目的地闸**：子 agent 请求白名单外域名 → 弹窗无人应答。

**三道「直接拒不弹窗」**（**勿混淆，这是三个不同的地方**）：

| # | 位置 | 语义 | 事件 |
|---|---|---|---|
| A | `dispatch.rs:382-405` | **规则闸 ask 分支**：子 agent 会话直接拒，提示「请主 agent 向用户说明后改由主会话执行」 | `tool_rule.hit`（**Info**，`action=ask_denied_subagent`）+ `pre_execute.deny` + `tool.return`（reason=`denied_by_rule`） |
| B | `destination.rs:165-173` | **目的地闸**：子 agent 一律按 strict 处理，白名单外直接拒 | `bot.http_request`（Warn，`dest=subagent`） |
| C | `dispatch.rs:700-719` | **子 agent 白名单外**（工具不在 profile ∩ 配置档的交集里） | `subagent_whitelist_deny` |

另有一处**不是**子 agent 直拒但常被误记：**规则闸 deny 分支**（`action=deny`，**Warn** 级，与 A 的 **Info** 级不同）。

**执行计数挪闸后**（`78485a8c`）：原先计数块在入参 JSON 闸**之前**——被闸拒掉的调用**也占** `used_tool_calls`。改后挪到闸**之后**，判据「计数守的是实际执行」，与熔断侧对 `Counted+Denied` 做回退的口径一致。这条影响 `read_own_card` 的实时余量与收尾时的预算触达判定。

**步骤**：

1. 在设置页给某个工具加一条 per-tool 规则（**action=ask**）。
2. 让主 agent 派一个**会调那个工具**的子 agent（`spawn_subagent`）。
3. 观察：**有没有弹窗**、子 agent 拿到什么回复、审计里 `tool_rule.hit` 的 `action` 是什么。
4. **对照半**：让**主会话**自己调同一个工具 → 应当**正常弹窗**（主会话弹窗有人认领）。
5. **计数半**：让子 agent 调一个**入参非法**的工具（触发 `tool_args_invalid_json`）→ 观察 `used_tool_calls` **不增加**（`read_own_card` 显示的余量不变）。

**判据**：

- PASS：第 3 步**没有弹窗**（60s 内无人应答的窗不出现）、子 agent 拿到一条**说人话的拒收说明**（含「请主 agent 向用户说明」）、`tool_rule.hit` 的 `action="ask_denied_subagent"` 且级别为 **Info**；第 4 步主会话**正常弹窗**；第 5 步**被 JSON 闸拒的调用不计数**。
- FAIL：子 agent 场景**弹了窗**（空转窗回来了）；或弹窗了但**用户批准后工具仍被拒**（这就是本卡要治的病）；或 `action` 值不是 `ask_denied_subagent`（接错分支）；或子 agent 与主会话**行为一样**（子 agent 也弹窗 = 没做短路）；或 JSON 闸拒掉的调用**照样计数**（计数块没挪）。
- **级别半**：`ask_denied_subagent` 是 **Info**、`deny` 是 **Warn**——两个同属 `tool_rule.hit` 但级别不同，判读时别混。

**变异注入复验**：把 `dispatch.rs:378-405` 的子 agent 短路判断删掉 → 重跑第 3 步应 **FAIL**（空转窗回来了，60s 后默认拒绝）。
第二个变体：把计数块挪回 JSON 闸之前 → 重跑第 5 步应 **FAIL**（余量被虚耗）。

**平台**：跨平台。**Windows 注意**：60s 默认拒绝在两个平台同口径，但驱动时人手节奏必须 < 60s。

---

### M103 · 三段式 stage 派发自动挂载配置档：没有 stage 就没有档

**靶点**：`src-tauri/src/bot_orchestrator.rs:124-137`（`SubagentStage::default_onion`）+ `src-tauri/src/bot/subagent_tools.rs:31-63`（`attach_stage_onion`）+ `src-tauri/resources/onions/*.yaml`（三档内置配置）（提交 `c30e3af1` / `1ac9834b`）

**bug 形态**：三段式派发（`recon` → `executor` → `verifier`）本来要按阶段收窄工具面，但**配置档（onion）得显式传 `onion` 参数才挂得上**。派发方（尤其是 LLM）不传 → 子 agent 工具面 = profile 白名单原样，**没收窄**。三段式的隔离设计等于没生效。

改后：**stage 派发自动挂载内置配置档**。

| stage | 默认 profile | **默认挂载的档** | 该档的 `allowed` 集合 |
|---|---|---|---|
| `recon` | research | **`probe`** | `web_search` / `fetch_url` / **`http_request`** / `list_files` / `read_text_file` / `grep_files` / `extract_document` / `write_artifact_file` / `read_own_card`（9 项） |
| `executor` | coder | **`writereditor`** | `read_text_file` / `list_files` / `grep_files` / `edit_file` / `write_file` / `run_python` / `write_artifact_file` / `read_own_card`（8 项） |
| `verifier` | research | **`validator`** | `read_text_file` / `list_files` / `grep_files` / `run_python` / `ast_inspect` / `write_artifact_file` / `read_own_card`（7 项） |

**优先级**：显式 `onion` 参数 > stage 自动挂载 > 不挂。**加载失败不阻断流水线**——不挂、派发继续、落 `onion.stage_attach_failed`（**Warn**）。

**三档同时退役 `max_turn`**：轮数改由 `MAX_ROUNDS = 200` 统一兜底（主/子/三段式共用同一安全网）。`max_turn` 字段在代码里**仍存在**（自定义配置档要快停时可自行声明），只是三内置档不再声明。

**`allowed` 是收窄清单**：实际生效 = **profile 白名单 ∩ 本列表**（只能收紧，yaml 不能放宽 profile）。

**派生副作用**（值得记一笔）：`verifier` 挂的 `validator` 档 profile 是 **`coder`**（不是 `SubagentStage::default_profile` 返回的 `research`）——**两条派生路径结果不同**，靠 onion 档覆盖。效果是 verifier 的档位从 `research` 翻到 `coder`（能用 `run_python` / `ast_inspect` 复核交付物），但工具面仍被 `validator.allowed` 收在只读集合内。

**步骤**：

1. 让主 agent 派一个 **`recon` 阶段**的子 agent（`spawn_subagent` 带 `stage=recon`），**不传 `onion`**。
2. 观察：`onion.stage_attached`（Info，`stage`/`onion_id` 两个 kv）出现、`onion_id="probe"`；子 agent 的可用工具**不含** `run_python`（probe 档没收窄到有 run_python？——**注意 probe 档确实不含 run_python**）。
3. 派一个 **`executor`** 阶段子 agent → `onion_id="writereditor"`，可用工具**含** `write_file`/`edit_file`/`run_python`。
4. 派一个 **`verifier`** 阶段子 agent → `onion_id="validator"`，可用工具**含** `run_python` 但**不含** `write_file`/`edit_file`（只读集合）。
5. **显式优先半**：派一个 `stage=recon` **且显式传了 `onion=writereditor`** 的子 agent → 应当**挂 writereditor**（显式优先）。
6. **无 stage 半**：派一个**不传 stage** 的子 agent → **不挂档**（`onion.stage_attached` 不出现）。
7. **失败降级半**：把用户数据目录里的 `onions/probe.yaml` 改坏（写成非法 YAML），派一个 `stage=recon` 子 agent → 应当**派发仍成功**、`onion.stage_attach_failed`（Warn）出现、子 agent 工具面 = profile 原样。

**判据**：

- PASS：第 1 步 `onion.stage_attached` 出现且 `onion_id="probe"`；第 3 步 `onion_id="writereditor"`；第 4 步 `onion_id="validator"` 且**工具面收窄正确**（能写不能写的边界对）；第 5 步显式 `onion` **覆盖** stage 默认；第 6 步**无** `onion.stage_attached`；第 7 步**派发成功** + `onion.stage_attach_failed`（Warn）+ 子 agent **正常跑完**。
- FAIL：stage 派发**没挂上档**（`onion.stage_attached` 不出现——自动挂载没生效）；或挂错档（`stage=recon` 挂了 writereditor）；或显式 `onion` 被 stage **覆盖**（优先级反了）；或第 7 步**派发被阻断**（降级路径没做，流水线断在这里）。
- **边界半**：观察子 agent 试图用**档外工具**（如 executor 阶段用 `web_search`）→ 应当被**白名单闸拒**（`subagent_whitelist_deny`）。这条与 M102 的 C 路同源。

**变异注入复验**：把 `attach_stage_onion` 首行的 `if let Some(cfg) = explicit { return Some(cfg); }` 删掉 → 重跑第 5 步应 **FAIL**（stage 覆盖了显式配置）。
第二个变体：把 `default_onion` 的 `Recon => "probe"` 改成 `"writereditor"` → 重跑第 1 步应 **FAIL**（recon 挂上了写档，工具面没收窄）。

**平台**：跨平台。

---

### M104 · 提示词与 schema 是第二真相源：三处文案必须与常量对得上

**靶点**：`src-tauri/src/prompts/system.rs:35`（规则 17）+ `src-tauri/src/bot/registry/schemas.rs:139/215`（`SCHEMA_RUN_PYTHON` / `SCHEMA_WEB_SEARCH`）+ `src-tauri/src/bot_chat/exec.rs:18` + `src-tauri/tests/fixtures/tools_baseline.json`（提交 `633ec41f`）

**bug 形态**：提示词与工具 schema 是**模型直接读的第二真相源**。本批把风控常量化了，但**三处文案还停在常量化前的旧值**——**模型读的是文案，不是常量**：

| 面 | 旧文案 | 新文案 | 账实来源 | 不对齐的后果 |
|---|---|---|---|---|
| 批量执行提示词 | 「50 轮工具循环」 | 「最多 **200** 轮工具循环」 | `MAX_ROUNDS = 200` | **模型按 50 自我节流**——安全网 200 轮形同虚设 |
| run_python schema | 「输出 **64KB** 封顶」 | 「输出约 **8KB** 封顶：超限部分转存文件并附指针」 | 回灌层 per-tool cap 8192 字节（**64KB 是沙箱层 stdout 口径，模型面差 8 倍**） | **模型按 64KB 规划 print 量**，实际 8KB 就转存 → 大清单被截成节选 |
| web_search count | 「默认 **8**」 | 「默认 **6**」 | `SEARCH_MAX_RESULTS_DEFAULT = 6`（Tavily 主流推荐 6） | 模型按 8 规划，实际只回 6 |

**这三条都是「静默失效」**：代码全对，只有模型的行为不对。

**步骤**：

1. **静态半**：`grep` 上面三处文案，确认已与常量一致（`tools_baseline.json` 夹具也随提交重生成）。
2. **动态半（轮数）**：让模型跑一个**多步任务**（如「读这个目录所有 `.log` 并汇总」）→ 观察它**自己说的步数规划**是否还按 50（提示词里现在写 200，模型应当按 200 规划）。
3. **动态半（run_python 输出）**：让模型写一段**会 print 大量内容**的 Python → 观察它的**自我约束**（schema 现在说 8KB + 转存指针，模型应当主动把大结果写进文件再用 `read_text_file` 读，而不是 `print` 全量）。
4. **动态半（web_search）**：让模型搜一次并**说明它请求了几条** → 应当是 6。

**判据**：

- PASS：三处文案**静态**已与常量一致（无「50 轮」「64KB 封顶」「默认 8」残留）；第 2 步模型**不再自述 50 轮**（按 200 规划）；第 3 步模型**主动把大输出落文件**而不是 print 全量（这是 schema 文案生效的直接证据）；第 4 步请求条数为 6。
- FAIL：任一处文案**仍是旧值**（模型按旧值行为）；或第 3 步模型**照 print 全量**、结果被转存成节选（模型没读到新约束）；或第 4 步实际请求 8 条 / 模型按 8 规划（默认值没生效）。
- **口径半**：`64KB` 在**沙箱层 stdout** 口径上仍然是真值（`py/runtime.rs` 的 `OUTPUT_CAP`）——**两处 64KB 不是同一个东西**，改文案时别把沙箱层也改了。

**变异注入复验**：把 `prompts/system.rs:35` 的「最多 200 轮」改回「50 轮」→ 重跑第 2 步应 **FAIL**（模型按 50 自我节流）。
第二个变体：把 `SCHEMA_RUN_PYTHON` 的「输出约 8KB 封顶」改回「输出 64KB 封顶」→ 重跑第 3 步应 **FAIL**（模型照 print 全量，被转存成节选）。

**平台**：跨平台。**这卡的动态半依赖模型是否真的读文案**——若某模型不遵守，按「模型不配合」登记观察项，不判 FAIL。

---

### M105 · 压缩分级量尺改全量序列化：带图的请求不再对硬顶失明

**靶点**：`src-tauri/src/bot_model_loop/request.rs:467-470`（`msgs_chars` 计算）+ `src-tauri/src/bot_model_loop/context.rs:180`（`msg_chars`）+ `src-tauri/src/bot/compaction.rs:45-55`（`classify_level`）+ `src-tauri/src/bot/constants.rs:59`（`CONTEXT_CHARS_HARD_CAP = 200_000`）（提交 `38cf3740`）

**bug 形态**：压缩阶梯（软压 70% / 硬压 85% / Fuse 100%）的分级量尺算的是「消息里 `content` 是**字符串**的那些条目的字符数」。**多模态图片（base64，单图至多 3MB）与 `tool_calls` 的 `arguments` 落在量尺外** → **带图请求对 200 000 字符的硬顶静默失明**：请求体实际比量尺说的大好几倍，上游要么 400 要么静默截断。

改成：每条消息**整体 `serde_json::to_string` 后计字符数**（`msgs.iter().map(msg_chars).sum::<usize>()`）。分级阈值不变（70% / 85% / 100%）。

**步骤**：

1. 让模型做一件**必然带图**的事（例如让它 `ocr_image` 或读一张截图），观察 `bot.log` 的 `loop.msgs.over_budget`（含 `chars` / `budget`）与随后的 `loop.msgs.compacted` / `loop.msgs.age_compacted`。
2. **对照半**：同样的任务但**不带图**，对比 `chars` 读数。
3. **量尺校准半**：在带图的会话里，人工构造一个「消息数组总字符数远超量尺读数」的场景（连续发几张图），观察**压缩阶梯是否按真实体量触发**。

**判据**：

- PASS：带图会话的 `loop.msgs.over_budget` 的 `chars` 读数**明显大于**「只数字符串 content」时的读数（说明图片进量尺了）；压缩阶梯**按真实体量触发**（带图的会话比不带图的**更早**进入压缩）；`loop.msgs.compacted` 出现且压缩后请求正常返回。
- FAIL：带图与不带图的 `chars` 读数**几乎相同**（图片仍不在量尺里——本卡的病没修）；或带图会话**一路不触发压缩**直到上游报错（硬顶失明的直接后果）。
- **口径半**：量尺口径从「字符串 content 字符数」变成「整条消息 JSON 序列化后的字符数」——**数值会整体变大**（JSON 转义与结构开销都计入）。这是预期的，别当成 bug。

**变异注入复验**：把 `request.rs:467-470` 改回 `msgs.iter().filter_map(|m| m["content"].as_str())...` → 重跑应 **FAIL**（图片退出量尺）。

**平台**：跨平台。

---

### T2 档：可感知语义

### M106 · ask_user 聊天放开 + 提问卡：模型能在聊天里问你了

**靶点**：`src-tauri/src/ask_engine.rs`（`AskOrigin` / `register_chat_ask_context` / `ask_event_payload` / `resolved_event_payload`）+ `src-tauri/src/bot_chat/flow.rs:648-657`（注册/注销）+ `src/components/ChatPanel/AskCard.tsx` + `src/components/ChatPanel/askStore.ts` + `src/components/ChatPanel/useChatStream.ts:462/476`（事件接线）（提交 `d7ec5c60` / `b504d006`）

**bug 形态**：此前 `ask_user` **只在任务执行中可用**——普通聊天里模型问了，后端直接回一句 warn「ask_user 仅任务执行中可用；聊天中请直接文字提问」。而「模型需要澄清再动手」是主流交互形态；业界五家（Claude Code 的 AskUserQuestion、Codex 的 request_user_input、Cursor、Cline/Roo、Kiro）**全都支持**。

改后：聊天会话**注册提问上下文**（`bot_chat_inner` 在 `run_model_loop` 前后成对注册/注销），模型可以直接问。**汇报轮不注册**（`TurnOrigin` 非 User）——保持无人值守路径不被提问打断。

**提问卡**（前端）：输入区上方出现一张卡——问题文本、`问这个：{why}`、`未回答时按：{assumption} 继续`、选项 chips（点选即填）、自由输入框（maxLength 200）、两个按钮「按假设继续」「回答并继续」。

**三个事件的载荷**（前端 ↔ 后端**同名**，无改名无别名）：

| 事件 | 载荷 | 终态 action |
|---|---|---|
| `bot-ask` | `{ questionId, question, why, options[], assumption, sessionId, seq }` | — |
| `bot-ask-resolved` | `{ questionId, action, answer, sessionId, seq }` | `answer` / `assume` / `dismiss` / `expired`（`answer` 字段**只在 `action=="answer"` 时携带**） |

**载荷里的三个设计细节**：

1. `sessionId` 与 `seq` 由 **`emit_recorded` 统一出口盖章**，调用方**不许自带同名键**（`stream_log.rs:246/250`）。
2. `questionId` 是本次新造的 uuid（通知 id 是 `wfq:{qid}`）。
3. 通知标题分叉：TaskCard → 「任务卡「{node_title}」提问」；Chat → 「聊天提问」。

**askStore 的写入纪律**（这是最容易写错的一处）：**必须整体替换**（`asks.set(sid, next)`），原地改引用里的对象会让 `useSyncExternalStore` 拿到同一引用而**不重渲染**；空态必须用**共享的 `EMPTY` 实例**（`askStore.ts:53`）。

**步骤**：

1. 在聊天里让模型问一个有歧义的问题（例如「这个任务要按 A 还是 B 口径做？A 是……B 是……」）。
2. 观察：**聊天内**出现提问卡；**通知中心**多一条「❓ 聊天提问」。
3. 点选项 chip → 自动填入输入框 → 点「回答并继续」→ 模型**按你的答案**继续。
4. **假设路径**：再问一次，**不答**、点「按假设继续」→ 模型按 `assumption` 继续。
5. **expired 路径**：这张卡等 24h 才过期——本卡**不验**（登记进未覆盖清单）。
6. **报告轮不注册**：让模型做一次**汇报型**任务（不需澄清）→ 不应出现提问卡。
7. **幂等半**：让模型**连问两次**同一问题 → 两张卡，各自独立应答。

**判据**：

- PASS：第 1 步**聊天内出现提问卡**（问题文本、`问这个`、`未回答时按` 三行都在）；通知中心**多一条聊天提问**；第 3 步模型**明确按你选的答案继续**（内容与选项对得上，不是按假设）；第 4 步模型按 `assumption` 继续且**卡上显示「已按假设继续」**；第 6 步**不出现提问卡**；第 7 步两张卡**互不影响**（答一张不影响另一张）。
- FAIL：提问卡**不出现**（事件没到前端——查 `bot-ask` 是否进回放缓冲）；或卡片出现但**输入框不生效**（`askStore` 原地改引用导致不重渲染）；或**答了却没生效**（应答没回到引擎）；或模型**无视答案按假设继续**；或第 6 步**汇报轮也弹卡**（注册口径错了）。

**变异注入复验**：把 `askStore.ts` 的 `applyAsk` 改成**原地改对象**（`asks.get(sid)[i].question = ...` 不整体替换）→ 重跑应 **FAIL**（卡片出现但不刷新）。
第二个变体：把 `bot_chat/flow.rs` 的注册条件从 `origin == TurnOrigin::User` 改成无条件 → 重跑第 6 步应 **FAIL**（汇报轮也弹卡）。

> **勘误（2026-11-09）**：汇报轮提问口径已反转——`ask_registered` 现覆盖 `TurnOrigin::User | TurnOrigin::Report`（提交 `485b5887`）。本卡「**汇报轮不注册**」段落、第 6 步「不出现提问卡」的 PASS 判据与「汇报轮也弹卡」的 FAIL 反例、以及上面第二个变异锚点（`origin == TurnOrigin::User` 代码已不存在）**全部作废**。复跑口径：第 6 步改为「汇报轮也**能**弹提问卡且可正常应答」（子 agent 终态续轮与用户轮同闸同预算）；变异注入改为「把 `matches!(origin, TurnOrigin::User | TurnOrigin::Report)` 退回只认 User → 汇报轮提问被拒回落假设，应 **FAIL**」。

**平台**：跨平台。

---

### M107 · 提问/确认等待环的三处竞态：用户的回答不许被扔掉

**靶点**：`src-tauri/src/ask_engine.rs:361`（`wait_ask_reply`）+ `src-tauri/src/bot_slash.rs:371`（`wait_confirm_reply`）+ `src-tauri/src/bot_model_loop/core.rs:1095-1102`（侧调停止前置闸）+ `src/components/ChatPanel/askStore.ts:69`（`hasUnresolvedAsk`）+ `src/components/ChatPanel/useChatStream.ts:531`（看门狗豁免）（提交 `fc3f4a25` / `d424c992` / `2364381c` / `20c0f919`）

**bug 形态**：四个独立的「用户操作被丢弃 / 被误判为死」问题，都在等待环里。

**① `select` 随机裁决**（`fc3f4a25`）：原先 `tokio::select! { timeout(timeout, rx) => ..., stop轮询 => ... }`——**超时与应答同时就绪时 select 随机选**。取到 `Expired` 就把用户**已经写进通知表的回答扔掉**，模型按假设继续 → **通知显示「已回答」而模型没收到，两层各说各话**。
改后：500ms 轮询环，顺序固定「**先收应答 → 后判停止 → 后判超时**」，应答在窗口内必达。

**② oneshot 关闭自旋**（`d424c992`）：两处等待环都只认 `Ok(Ok(reply))`——**sender 被 drop**（应答端在摘除与发送之间消失）时，关闭的通道**每次 poll 立即就绪**，环内再无让出点 → **退化成同步级自旋占满一个 tokio worker 到 deadline**（提问窗以小时计！）。
改后：`Ok(Err(_))`（通道关闭）分支**立即按超时口径退出**；确认侧关闭时回查一次停止标志以保住审计拒绝原因。

**③ 停止后白等一次侧调**（`2364381c`）：工具自检侧调**无条件发起**，一次最多烧 60s。用户在自检期间点了停止 → UI 停在「收尾中 · 校验答复」最长 60s，**看着像停止失灵**。
改后：`if stop.stopped() { None } else { 侧调 }`——已停止则不发侧调，回落 in-loop（这道闸不丢）。

**④ 提问等待期被看门狗判死**（`20c0f919`）：前端 dead-man 看门狗（660s 无流式事件即判回合失败）**只看 `lastActivityOf(sid)`**。提问卡挂着时（可等 **24h**）**零流式事件** → 660s 后被判死回收会话，UI 显示「⚠️ 长时间无任何流式事件，回合已按失败处理」——**用户只是还没回答问题，回合却被判死**。
改后：看门狗循环内先 `if (askStore.hasUnresolvedAsk(sid)) continue;`——**存在未答提问卡即豁免判死**；豁免随 `bot-ask-resolved` 终态解除（该事件本身刷新活动戳）。`hasUnresolvedAsk` 的判据是 `(asks.get(sid) ?? EMPTY).some((a) => !a.resolved)`——**多问串行时，前一张已答、后一张未答仍豁免**。

**步骤**：

1. **① 超时临界**：让模型提问，然后**在提问卡的等待窗口末尾**（接近 24h 过期点不可行）——**改用 `ask_user` 的短超时场景**：若能在测试配置里把 `ASK_TIMEOUT_SECS` 临时调小（例如 3s），则在 3s 临界点**同时**应答与等待超时 → 观察**应答是否被采纳**（模型收到答案，而不是按假设继续）。
2. **② 关闭通道**：删掉会话 / 中途让应答端消失（发完答案立刻删会话）→ 观察**提问卡不挂满 24h**（应立即回落假设或释放 worker）。
3. **③ 停止即退**：让模型在一个**长历史**的会话里调工具（触发侧调自检），在自检期间按 `/stop` → 观察 UI **立即**回到停止态，**不等 60s**。
4. **确认窗侧**：触发一个需要确认的工具（`bot-confirm`），在等待期按 `/stop` → 观察确认窗**立即关闭**（不是挂满 60s）。
5. **④ 提问不被判死**：把 `STREAM_DEAD_IDLE_MS` 临时调小（例如 **15s**，见 §附录 D 的 D3）→ 让模型提问并**不答** → 等超过阈值 → 观察**回合不被判死**（**没有**「长时间无任何流式事件，回合已按失败处理」），提问卡**仍在**。答完后再等一轮 → 豁免**解除**（下一轮该判死时仍会判死）。
6. **多问串行半**：让模型**连问两次**（两张卡），答掉第一张、第二张不答 → 观察**仍豁免**（豁免判据是「有任一张未答」）。

**判据**：

- PASS：第 1 步**应答被采纳**（模型收到答案）；第 2 步提问**立即回落**（不挂 24h、CPU 不打满）；第 3 步 `/stop` **立即生效**（UI 秒级回到停止态）；第 4 步确认窗**立即关闭**且审计里 `confirm_timeout` 带 `{reason}` 区分「/stop」与「60s 无响应」；第 5 步**不判死**（提问卡仍在、无失败文案）、答完后**豁免解除**；第 6 步**仍豁免**。
- FAIL：第 1 步**应答被扔**（模型按假设继续，而通知显示已回答——两层各说各话）；第 2 步提问**挂满 24h** 且 CPU 占用异常（自旋）；第 3 步停止后**仍等满 60s**；第 4 步确认窗**挂满 60s**（`/stop` 打不断）；第 5 步**提问卡被看门狗判死**（UI 出现「长时间无任何流式事件，回合已按失败处理」）；或第 5 步答完后**豁免没解除**（该判死时也不判——看门狗彻底失能）；或第 6 步**豁免被提前解除**（已答的那张让另一张失保）。
- **超时口径**：deadline 后**已开片内**的应答**仍被采纳**（deadline + 至多一个片长 = deadline + 500ms，**偏用户侧**）——这是有意的，不是 bug。

**变异注入复验**：把 `ask_engine.rs:361` 的 `wait_ask_reply` 改回 `tokio::select!` 随机裁决 → 重跑第 1 步应 **FAIL**。
第二个变体：把 `Ok(Err(_))` 分支的立即退出改回「继续循环」→ 重跑第 2 步应 **FAIL**（自旋回归）。
第三个变体：把 `useChatStream.ts:531` 的 `if (askStore.hasUnresolvedAsk(sid)) continue;` 删掉 → 重跑第 5 步应 **FAIL**（提问卡被看门狗判死）。

**平台**：跨平台。

---

### M108 · 前端回灌状态机 + 三条赛跑防线：判死后开新轮不能被旧轮清场

**靶点**：`src/components/ChatPanel/streamState.ts`（`beginTurn`/`endTurn`/`bumpTermEpoch`/`termEpochOf`）+ `src/components/ChatPanel/useChatHistory.ts:122`（快照代际）+ `src/components/ChatPanel/useChatStream.ts:528-572`（dead-man 看门狗）+ `src/components/ChatPanel/useChatSessionActions.ts:234/333`（回合世代）（提交 `4ce6a1ca` / `30852dbf`）

**bug 形态**：流式回灌与回合收尾**赛跑**。本批修了**七条**前端状态机缺陷，其中三条是「判死后开新轮」的连锁反应：

**① 快照代际作废**（`termEpoch`）：终态事件在 `fetchReplay` 往返期间先处理时，旧快照的 `inflight:true` 会把**已死轮重新标在途** → 会话卡 busy 直到切回自愈或看门狗兜底。
改后：终态事件 `bumpTermEpoch(sid)`；回灌发现**代际已变**即按收尾收口（`dropSession` + `loadFinalText` + 清已答卡）。**墓碑语义不随 `dropSession` 清除**（删了旧快照就会重新对上号）。

**② 回合世代守卫**（`turnSeq`）：看门狗判死放行后用户开新轮，**旧轮 `runChat` 迟到的 finally 会把新轮的 meta/在途一并清掉**。
改后：`beginTurn` 登记世代号、`endTurn` **仅在「仍是本世代」时**返回 true 才清场。counter 全进程单调——`dropSession` 清掉会话条目后新轮重新计数**不撞号**。

**③ 看门狗叫停复核**：判死放行后用户可能已开新轮，`bot_stop` 按 sessionId 置位**全部**实例 → 不复核会把新轮一并误停。
改后：叫停前 `if (!streamState.isInflight(sid))` 才 invoke。

**另四条**（同批）：`openExecSession` 不再无条件清 meta（**切走再切回执行会话不丢已流出正文**）；终态事件只对**非 invoke 轮**清场；回灌判据从 `seqOf(sid) > 0` 改成 **seq 连续性**（不连续则整轮重建）；合并窗按**增量所属会话**入队（**旧会话的尾巴不串进新会话**）。

**看门狗参数**：`STREAM_DEAD_IDLE_MS = 660_000`（660s = 后端最长静默 600s + 60s 余量；依据列在 `constants.ts:45` 的注释里）、tick 间隔 **15s**。判死文案：`⚠️ 长时间无任何流式事件，回合已按失败处理；若后端仍在运行，重新打开会话可查看最终结果`，并把气泡内容变成 `partial`。

**步骤**：

1. **切走再切回**：让模型跑一个**长任务**（中间有工具调用），**中途切到别的会话再切回来** → 正文**不丢**。
2. **合并窗串台**：会话 A 正在流式输出时切到会话 B → A 的尾巴**不进 B 的视图**。
3. **判死后开新轮**：把 `STREAM_DEAD_IDLE_MS` 临时调小（例如 **15s**，tick 也是 15s，所以最多 30s 内触发）→ 等判死 → **立刻开新轮** → 观察新轮**不被旧轮收尾清场**（新轮的内容还在、状态不被打回）。
4. **看门狗不误停新轮**：同上，判死后开新轮，等看门狗再触发一次 → 新轮**不被 `bot_stop` 停掉**。
5. **终态不重复提示**：判死场景下**只出现一条**失败文案（不叠两条）。
6. **改完记得复位** `STREAM_DEAD_IDLE_MS` 回 660000。

**判据**：

- PASS：第 1 步切走再切回**正文完整**；第 2 步**无串台**；第 3 步新轮**不被清场**（内容还在、状态正常）；第 4 步新轮**不被误停**（后端仍在跑、模型继续输出）；第 5 步**只有一条**失败文案。
- FAIL：切走再切回**正文丢失**；或**串台**（B 视图里出现 A 的尾巴）；或新轮**内容被清空** / 状态被打回（世代守卫失效）；或新轮**被 `bot_stop` 停掉**（叫停没复核在途）；或**出现两条**失败文案。
- **口径半**：看门狗判死后气泡内容是 `partial` + 提示，**不是最终答复**——提示里明说「若后端仍在运行，重新打开会话可查看最终结果」。**这是设计如此**，别当成「内容丢了」。

**变异注入复验**：把 `useChatSessionActions.ts:333` 的 `if (streamState.endTurn(sid, turn))` 改成无条件清场 → 重跑第 3 步应 **FAIL**（新轮被旧轮清场）。
第二个变体：把 `useChatStream.ts:559-568` 的 `if (!streamState.isInflight(sid))` 复核删掉 → 重跑第 4 步应 **FAIL**（新轮被误停）。
第三个变体：把 `useChatHistory.ts:125` 的代际比对分支删掉 → 重跑第 3 步的**前置**（终态先于回灌到达）应 **FAIL**（死轮重新标在途）。

**平台**：跨平台。前端行为与平台无关。

---

### M109 · bot-confirm 统一出口 + 重载还原带真实剩余时长

**靶点**：`src-tauri/src/bot_slash.rs:256`（`ask_confirm_inner`）+ `src-tauri/src/bot_slash.rs:397`（`bot_pending_confirms`）+ `src-tauri/src/app_state.rs:46`（`PendingConfirmRequest`）+ `src/components/ConfirmMap/ConfirmMap.tsx`（还原与倒计时）（提交 `969de3ca` / `8c0cd928`）

**bug 形态**：三个问题。

**① 裸 `app.emit` 游离在信封方案外**：`bot-confirm` 原先由调用方裸 `app.emit("bot-confirm", {..., "sessionId": session_id})`——**会话号由调用方自带**，与会话号「只在统一出口盖章」的约定冲突（`docs/BOT-EVENT-ENVELOPE-DESIGN-2026-10-27.md`）。
改后：走 `emit_recorded`——会话号由出口盖章、调用方不再自带同名键、事件**进回放缓冲**。

**② 60s 挂死打不断**：等待是 `tokio::time::timeout(60s, rx)` 一次性挂满 60s，**`/stop` 打不断**。
改后：500ms 粒度**同时盯应答与 `/stop`**（`is_session_stopped` 按会话号反查停止注册表——确认等待**不持 `StopGuard`**）。

**③ 重载还原把倒计时重置满 60s**：webview 重载丢掉挂在事件上的弹窗 → 重载后按 `bot_pending_confirms` 还原。但后端 deadline 从**提问时刻**起算、**不随还原重置** → 用户会在「看着还剩很多」时点到一条**后端早已超时拒绝**的请求上。且 `HashMap` 迭代序随机 → 「还原哪条」不确定。
改后：确认表补 `created_at_ms`；`bot_pending_confirms` 按**入表时刻 FIFO** 排序并带 `remaining_secs`；前端倒计时取 `min(60, remaining)`；**已到期（0）不还原、直接按拒收回收**；多条并发时还原**最早**那条、其余拒收；还原路径补 `busyRef` 守卫（不补刀用户正在飞的应答）。

**步骤**：

1. 触发一个确认弹窗（任何 danger 类工具）→ 观察弹窗出现、倒计时从 60s 开始。
2. **停止可打断**：在等待期按 `/stop` → 弹窗**立即关闭**（不是挂满 60s）。
3. **重载还原**：触发弹窗后**在 60s 内重载 webview**（DevTools reload 或重启前端）→ 观察弹窗**重现**且**倒计时是真实剩余**（如还剩 9s 就显示 9s，**不是重置 60s**）。
4. **过期不还原**：触发弹窗，**等它过期**（60s），然后重载 → **不弹窗**（直接按拒收回收）。
5. **FIFO**：同时触发**两个**确认弹窗 → 重载后**还原最早那一个**（`remaining_secs` 最大的那个），另一个被拒收。
6. **信封**：检查 `bot-confirm` 的 payload——**不应出现调用方自带的 `sessionId` 键**（由出口盖章）。

**判据**：

- PASS：第 2 步弹窗**立即关闭**（< 1s，不是 60s）；第 3 步弹窗重现且倒计时**等于真实剩余**（重载前后对比）；第 4 步**不还原**（不弹窗）；第 5 步**还原最早那条**、另一条被拒收；第 6 步 payload 里的 `sessionId` 是出口盖的章。
- FAIL：第 2 步**挂满 60s**；或第 3 步**倒计时重置 60s**（看着还剩很多但后端早超时了）；或第 4 步**仍然还原**（点下去必然失败）；或第 5 步**还原错那条**（FIFO 反了）；或 payload 里**出现两个 `sessionId`**（信封破了）。

**变异注入复验**：把 `bot_slash.rs:397` 的排序从 `remaining_secs` 升序改成**降序**（或去掉排序）→ 重跑第 5 步应 **FAIL**（还原的不是最早那条）。
第二个变体：把 `ConfirmMap.tsx:93` 的 `Math.min(TIMEOUT_SECS, first.remaining_secs ?? TIMEOUT_SECS)` 改成 `TIMEOUT_SECS`（重置满）→ 重跑第 3 步应 **FAIL**（倒计时重置）。

**平台**：跨平台。**Windows 注意**：`/stop` 的停止阶梯走 `taskkill`（见 §1.0 ③）——本卡验的是**等待环被打断**（后端侧），不是进程被杀。

---

### M110 · SSE 拆行 / 攒批上限 / 注释行：一条事件不许丢半段

**靶点**：`src-tauri/src/bot_model_loop/sse.rs:392`（`read_sse_stream`）+ `src-tauri/src/bot_model_loop/sse.rs:448/505-529/513-522`（攒批与边界）（提交 `bf228f2c` / `931e89fa`）

**bug 形态**：三个问题，都在 SSE 读流层。

**① 一条事件拆多条 `data:` 行**：逐行独立解析 `state.handle_line(line.trim())`——**一条载荷拆两行时两半都被当坏块跳过、丢正文**。代理按 token 边界断开重发超长行时就会触发。
改后：攒到**事件边界**（非 data 行）或流尾再拼成一条交给解析；跨 chunk 保留攒批（事件边界行可能与 data 行落在不同 chunk）。

**② 攒批缓冲无上限**：`bf228f2c` 引入攒批后，**切出的完整 data 行不占 `byte_buf` 的 2MB 配额** → 故障/恶意流持续发短 data 行（不 idle 即可保活）可绕过单行上限**无限攒内存**。
改后：攒批缓冲与单行同口径上限（`MAX_SSE_LINE_BYTES = 2MB`）；超限**截停 + 审计一次**（`llm.sse_data_batch_truncated`，Warn，带 `bytes`），**流不失败**（丢批保会话）。

**③ 注释行被当事件边界**：`: 开头的 keep-alive 注释行被当成边界 → 把一条事件的两半**拆丢**。
改后：注释行按 SSE 规范**忽略**——不终结正在攒的事件、也不产生输出。

**步骤**：

1. **正向半**：用一个**会把长行拆成多条 `data:` 行**的代理（本地起一个假 SSE 服务：把一个 JSON payload 按固定长度切开，每片一行 `data:`），让模型从它那儿流式输出 → 观察**正文完整**（两半都在，不丢）。
2. **注释行半**：假服务在一条事件的两半之间插一行 `: keep-alive` → 观察正文**仍然完整**。
3. **超限半**：假服务持续发大量无边界 data 行（累计 > 2MB） → 观察 `llm.sse_data_batch_truncated`（Warn）**恰好出现一次**、**流不失败**、会话**还能继续**。
4. **对照半**：正常服务（不拆行不插注释）→ 一切照旧。

**判据**：

- PASS：第 1 步正文**完整**（两半都在）；第 2 步注释行**不打断**事件；第 3 步 `llm.sse_data_batch_truncated` 出现**恰好一次**、`bytes` 接近 2MB 量级、**流未失败**（会话继续、后续消息正常）；第 4 步无回归。
- FAIL：第 1 步正文**丢一半**（拆行没接上）；或第 2 步注释行把事件**拆丢**；或第 3 步**无审计**（超限不留痕，内存可能被拖垮）；或第 3 步**审计出现多次**（每次切片都报，噪音）；或**流直接失败**（超限应当丢批保会话，不是断流）；或第 3 步**正文进了模型上下文**（超限那批内容不该进正文）。

**变异注入复验**：把 `sse.rs:513-522` 的超限截停分支删掉（攒批无上限）→ 重跑第 3 步应 **FAIL**（`llm.sse_data_batch_truncated` 不出现）。
第二个变体：把 `sse.rs:502-504` 的注释行 `continue` 删掉 → 重跑第 2 步应 **FAIL**（注释行又成了事件边界，事件被拆丢）。

**平台**：跨平台。**这卡需要一个本地假 SSE 服务**——用 Python 的 `http.server` 写几十行即可，见 §附录 D。

---

### M111 · 流式内核兜底：panic 不许永久转圈

**靶点**：`src-tauri/src/bot/stream_log.rs:187-205`（`record` / seq 盖章）+ `src-tauri/src/bot_model_loop/shell.rs:21-53`（`StreamFinishGuard` / `catch_model_loop_panic`）+ `src-tauri/src/bot_model_loop/finalize.rs:363`（`final_selfcheck.reply_unparsed`）（提交 `5a9a50f3` / `431ca487`）

**bug 形态**：四个「永久残渣 / 永久在途」。

| # | 原症状 | 根因 | 改法 |
|---|---|---|---|
| ① | 回放流整段绕前端去重闸（回放 + 实时重叠 → **双计**） | 缓冲副本**缺 `seq`** | seq 进缓冲副本（回放与实时同构） |
| ② | 每漏一条回合外事件多一条**永久残渣** | `record` 为**未 begin 的会话建条目** | `record` 不建孤儿条目（`m.get_mut(session_id)?`） |
| ③ | 会话**永久在途**（前端「正在思考」永不复位） | begin/finish 手工成对 → panic / 早退展开跳过收尾 | `StreamFinishGuard` **RAII** 收尾（正常路径显式 finish + defuse，时序不变） |
| ④ | 模型不按 Y/N 作答时正文**无痕消失** | 终稿自检返工抑制了该轮流式 | 留一条 Warn 痕 `final_selfcheck.reply_unparsed` |

**panic 归一**：`catch_model_loop_panic` 用 `AssertUnwindSafe(fut).catch_unwind()`——panic → 普通 `CommandError` + **补发失败终态**（`bot-turn-status`，`status:"failed"`，`detail:"内部错误：模型循环异常终止"`）。审计事件名 `model_loop_panic`（**Error** 级，kv `where` / `detail`；`where` 承载三种来源：主聊天 / 任务执行 / 逐步执行）。

**步骤**：

1. **正向半**（容易）：正常跑一轮 → 回放缓冲里的事件**带 seq**（`bot-ask` 的载荷里有 `seq`）。
2. **回放不双计半**：跑完一轮后**重新打开会话**（触发回放）→ 观察内容**不重复**（前端去重闸按 seq 生效）。
3. **panic 半**：注入一个 panic（临时在模型循环里 `panic!("test")`）→ 观察 UI **立即出失败终态**（不是永久转圈）、`model_loop_panic` 审计（Error）出现、**会话不再永久在途**（刷新后不显示「正在思考」）。
4. **孤儿条目半**：在模型循环**外**发一个回合事件（模拟漏 begin 的场景）→ 观察**不产生孤儿条目**（缓冲里不残留未收尾的条目）。
5. **终稿自检半**：让模型提交一个**不符合 Y/N 格式**的终稿 → 观察 `final_selfcheck.reply_unparsed`（Warn）留痕，且正文**不再无痕消失**。

**判据**：

- PASS：第 1 步回放缓冲的事件**带 seq**；第 2 步重开会话内容**不重复**；第 3 步 UI **立即出失败终态**（文案含「内部错误：模型循环异常终止」）、`model_loop_panic` 出现、刷新后**不显示「正在思考」**；第 4 步**无孤儿条目**残留；第 5 步 `final_selfcheck.reply_unparsed` 出现、正文**可追溯**。
- FAIL：panic 后**永久转圈**（RAII 收尾没接上）；或 `model_loop_panic` **没留痕**（panic 被吞）；或重开会话内容**重复一遍**（seq 没进缓冲副本）；或缓冲里**留孤儿条目**；或终稿自检失败**无痕**（正文静默消失）。

**变异注入复验**：把 `shell.rs:29` 的 `impl Drop` 里的 `stream_log::finish` 调用删掉 → 重跑第 3 步应 **FAIL**（panic 后会话永久在途）。
第二个变体：把 `stream_log.rs:193` 的 `m.get_mut(session_id)?` 改回 `or_insert_with(...)` → 重跑第 4 步应 **FAIL**（孤儿条目回归）。

**平台**：跨平台。**这卡要注入 panic**——按 §附录 D 的「变异注入前置」做，**不要**把 panic 留在工作区。

---

### M112 · 子 agent 等待环与终态落库：500ms 薄片 + 有界重试

**靶点**：`src-tauri/src/bot_orchestrator.rs:68/75/346-396`（等待阶梯与薄片）+ `src-tauri/src/bot/subagent_runner.rs:41-69`（`write_terminal_with_retry`）（提交 `07c94a55` / `2af5a6bc`）

**bug 形态**：两个「子 agent 永远卡着」。

**① `/stop` 打不断等待**：整步一把睡 `timeout(step_ms, notified)`——停止标志只在**每步醒后**检查，子 agent 长跑期间 `/stop` 最长要等满剩余等待（步长阶梯末档可达分钟级）才生效。
改后：整步切成 **≤500ms 薄片**睡，片间查停止标志；`notified` 复用已 enable 的那个（事件到达立刻破片重读）；**薄片到点只查停止标志不重读** → **DB 读频不随切片变密**（测试断言 `reads.get() <= 3`）。

**② 终态落库一次失败永挂**：四处兜底写各自 `db_locked(update_subagent_status)`，**一次失败只留审计、行永久停在 queued/running** → 父 agent 每次 `check_subagent` 空等、前端一直转圈，**唯一恢复靠重启清扫**。
改后：统一收进 `write_terminal_with_retry`——指数退避重试（`RETRY_DELAYS_MS = [1000, 2000, 4000, 8000, 16000]`，共 5 次，累计约 31s），重试幂等。

**等待阶梯**（`CHECK_POLL_LADDER_MS`）：`[150, 300, 600, 1500, 3000, 6000, 12000, 24000, 30000, 60000]` ms，出阶梯恒定封顶末档。总预算 `SUBAGENT_CHECK_WAIT_MS_CAP = 600_000`。

**步骤**：

1. **薄片半**：让主 agent 派一个**长跑**子 agent，主 agent 用 `check_subagent` 等待 → 在等待期间按 `/stop` → 观察 `/stop` **≤500ms 内生效**（不是等到整步走完）。
2. **读频半**：同上，观察等待期间**子 agent 行的 DB 读频不高**（不该因为切片而变密——这一步靠日志/DB 读计数或用变异对比）。
3. **落库重试半**：用 `BEGIN EXCLUSIVE` 短暂持锁制造写失败（或注入变异让第一次写必失败）→ 观察子 agent **最终仍落到终态**（不永挂 running）、审计 `subagent_terminal_state_write_failed` 留痕。
4. **恢复半**：让子 agent 正常跑完 → 父 agent 的 `check_subagent` **及时返回终态**（不是空等）。

**判据**：

- PASS：第 1 步 `/stop` **≤500ms** 生效；第 3 步子 agent **最终落终态**（行不再是 running）、`subagent_terminal_state_write_failed` 留痕；第 4 步 `check_subagent` **及时返回**终态。
- FAIL：第 1 步 `/stop` 要等**整步走完**（薄片没接上）；或第 3 步子 agent **永挂 running**（前端一直转圈、重启才恢复）；或落库失败**无留痕**；或第 3 步**重试打满 5 次仍失败**（没收敛）。
- **口径半**：`subagent_terminal_state_write_failed` 是 **`bot::audit_log` 自由文本**（不是 `write_event` 结构化事件名）——按 `docs/acceptance/event-names.md` 第 17 行的例外条款，它**不在白名单里是正常的**。判读时别去白名单里找。

**变异注入复验**：把 `bot_orchestrator.rs:383-396` 的薄片内层环改成**整步一把睡** → 重跑第 1 步应 **FAIL**（`/stop` 要等整步）。
第二个变体：把 `write_terminal_with_retry` 的重试次数改成 0（只写一次）→ 重跑第 3 步应 **FAIL**（写失败即永挂）。

**平台**：跨平台。

---

### M113 · run_python 许可排队响应 /stop + MCP 并行连接

**靶点**：`src-tauri/src/py/gate.rs:44-99`（`py_gate_acquire_with_stop`）+ `src-tauri/src/bot/mcp/manager.rs`（`ensure_tools_known` 的 `join_all`）（提交 `de53b5af` / `cef4dd96`）

**bug 形态**：两个「用户按了停止但没用」与「坏服务器拖慢整轮」。

**① 许可排队期不认停止**：许可闸的 Condvar 等待已是 200ms 粒度轮询，但**只查 EXITING 不查停止** → 两个许可被长任务占满时（各 600s 上限），**按了停止的排队者最坏要再等 2×600s** 才拿到闸、那时才检查停止。
改后：`py_gate_acquire_with_stop` 每轮唤醒**顺带查停止令牌**，停止即拒（**过闸即拒**）；`run_python` 把手头 `StopToken` 传进去。`document.rs` 的 doc 生成流**不传**（无会话停止语义，保持原路径）。

**② MCP 待发现服务器串行连接**：`for cfg in targets { ... discover_tools(app, &cfg).await; }` 串行——一台坏服务器拖 20s（连接 + 列工具各 10s 超时），**N 台全坏时回合首 token 被拖到分钟级**。
改后：先收集待发现的（`!tools_known && !discovery_cooling`），再 `join_all` 并行；各台发现互不依赖，每台内部超时与冷却结构不变。

**步骤**：

1. **排队半**：并发发起 **3 个**长时 run_python（许可数只有 **2**，`PY_RUN_MAX_CONCURRENT`）→ 第 3 个排队 → 在排队期间按 `/stop` → 观察**立即退出**（不是等 20 分钟）。
2. **许可半**：并发 3 个，确认**同时最多 2 个**在跑（第 3 个排队）。
3. **MCP 半**：配置 **2 台不可达的 MCP 服务器** + 1 台正常的 → 观察回合首 token **不被拖到分钟级**（正常那台的工具**在本轮内可用**）。
4. **冷却半**：一台发现失败的服务器**不立刻重试**（`DISCOVERY_RETRY_COOLDOWN_MS = 10 分钟`）——第二轮不再重复连接它。

**判据**：

- PASS：第 1 步排队中的 run_python **立即被拒**（错误信息含「已被用户停止，不再排队等待 Python 许可」）；第 2 步**同时最多 2 个**在跑；第 3 步正常 MCP 的工具**本轮可用**、首 token**未被拖到分钟级**；第 4 步失败服务器**不重复连接**。
- FAIL：第 1 步排队者**要等很久**才拿到闸才退出（停止没被认）；或**同时跑了 3 个**（许可闸失效）；或第 3 步首 token**被拖到分钟级**（还是串行）；或第 4 步**每轮都重连**失败服务器（冷却没生效）。
- **口径半**：MCP 发现有独立的 `MAX_STREAM_RETRIES = 1`（流层重试）与 `DISCOVERY_RETRY_COOLDOWN_MS = 10 分钟`（发现冷却）——**不是同一套**，别混。

**变异注入复验**：把 `py/gate.rs:70` 的停止复查分支删掉 → 重跑第 1 步应 **FAIL**（排队者不认停止）。
第二个变体：把 `manager.rs` 的 `join_all` 改回 `for` 循环串行 → 重跑第 3 步应 **FAIL**（首 token 被拖慢）。

**平台**：跨平台。**Windows 注意**：第 1 步的停止可能被 `taskkill` 树杀抢先——**要看后端日志的拒绝文案**，不能只看 UI 消失了。

---

### M114 · 设置页 Agent 参数卡退役：旋钮没了，行为照旧

**靶点**：`src/components/SettingsPage/BotCoreSection.tsx`（整卡删除）+ `src/components/SettingsPage/constants.ts`（`AGENT_PARAM_ROWS` 整表删除）+ `src/components/SettingsPage/TaskModeSelect.test.tsx`（组件删除）+ `src-tauri/src/bot/config/types.rs`（BotConfig 十键删除）+ `src-tauri/src/bot/params.rs:113`（`PARAMS_TABLE` 24 → 3 行）+ `src-tauri/src/bot/config/commands.rs`（`clamp_agent_params` 改三键）（提交 `387d6dbb` / `8f33a09a` / `6655ab52` / `e4087f9e` / `36ac4ce3`）

**bug 形态**：这不是「功能坏了」，是**用户能看到的配置面整个消失**。设过旋钮的用户会发现：

| 消失项 | 原类型 |
|---|---|
| 「Agent 运行参数」**整卡** | 设置页卡片（含 24 行参数） |
| 任务模式三档选择器（轻量/均衡/深度） | UI 组件（+ 145 行测试） |
| 「子 agent 墙钟预算（秒）」 | 参数行 + 输入框 + fallback label |
| 「子 agent 轮数预算」/「子 agent 工具调用上限」 | 参数行 + 输入框 |
| 「工具结果截断字节数」（含「填 0 = 不截断」第三态） | 参数行 + 输入框 |

**BotConfig 删除的十个键**（后端 snake_case / 前端 camelCase）：`max_rounds` / `max_function_calls` / `history_budget_chars` / `history_truncate_target_pct` / `reasoning_echo_enabled` / `in_turn_context_edit` / `search_max_results` / `ask_budget` / `max_tokens` / `reasoning_effort`。另三个键在同窗口的其他提交里删：`subagent_max_wall_secs` / `subagent_max_turns` / `subagent_max_tool_calls` / `max_tool_output_bytes` / `task_mode`。

**留下的三行**（§1.2 已列）：`tools.python_timeout_secs`（**可编辑**）、`trace.retention_days`、`memory.extract_min_interval_secs`（后两个**只读**）。

**写侧钳制改为三键**：`clamp_agent_params()` 只剩 `python_timeout_secs`（钳 1..600）、`final_selfcheck_round_gap`（钳 0..=100）、`verifier_retry_max`（钳 0..=2）。

**步骤**：

1. 截图设置页 → 确认「Agent 运行参数」卡**不在**、任务模式选择器**不在**、页面**无空容器 / 错位 / 残留标题**。
2. 改一下三行参数表里的 `tools.python_timeout_secs` → 保存 → 重开应用 → 值**还在**（可编辑的那行真生效）。
3. 尝试**手改** `bot-config.json` 塞回一个旧键（如 `maxRounds: 50`）→ 重启 → 确认**不报错**、旧键被忽略。
4. 尝试**手改** `final_selfcheck_round_gap` 为一个超范围的值（如 999）→ 保存 → 确认**被钳到 100**。
5. 对照：确认**仍可见**的三项（Python 超时在机器人设置卡、权限模式在 API 卡、Python 沙箱开关）**位置没变**。

**判据**：

- PASS：第 1 步整卡与选择器**都不在**、页面**无残留**；第 2 步 Python 超时值**持久且生效**；第 3 步旧键**被忽略、不报错**；第 4 步**被钳到 100**；第 5 步三项仍在原位。
- FAIL：整卡**还在**（或空了但标题还在）；或第 2 步值**没持久**（写侧钳制或存盘断了）；或第 3 步**启动失败**；或第 4 步**没钳**（999 写进去了）；或页面**布局错位**（删卡留下的空白容器）。
- **口径半**：设置页上**仍能看到**「Python 执行超时（秒）」「执行痕迹保留期（天）」「记忆抽取最小间隔（秒）」——这三行**不是** Agent 参数卡的一部分，它们在别的卡片上（卡外消费者）。

**变异注入复验**：把 `clamp_agent_params()` 里的 `final_selfcheck_round_gap` 钳制改成不钳 → 重跑第 4 步应 **FAIL**（999 写进去了）。
第二个变体：把 `params.rs` 的 `PARAMS_TABLE` 三行改回 24 行（引用已删的键）→ 重跑启动应 **FAIL**（或门禁红：参数表与常量表对拍）。

**平台**：跨平台。**Windows 注意**：便携包的设置页同样要确认无残留布局。

---

### M115 · anthropic 探活改打 POST /v1/messages：无 `/v1/models` 的兼容端点不再被 404 误杀

**靶点**：`src-tauri/src/bot/config/commands.rs:775`（`probe_connection`）+ `src/components/SettingsPage/ModelRow.tsx:137-138`（前端注释同步）（提交 `d959272a`）

**bug 形态**：厂商探活对 anthropic 协议打 `GET {base}/v1/models`，**只认 2xx 算通**。DeepSeek 的 `/anthropic` 兼容端点**只实现 Messages API，没有模型列表路由** → 探活**必 404** → 厂商**永远进不了 `verified_vendors`**。

改后：anthropic 协议打 **`POST {base}/v1/messages`** 发**空 JSON 体**（在参数校验层被拒，**不计费**），判读放宽：

| 协议 | 请求 | 判通 |
|---|---|---|
| anthropic | `POST /v1/messages` 空体 | **2xx** 或 **4xx 非 401/403/404**（400 空体不合法 / 422 / 429 限流 → 路由在即通） |
| openai | `GET /models`（**不变**） | 只认 2xx |

401/403（鉴权失败）/ 404（路径错）/ 5xx（服务端错）→ 判失败。openai 分支**完全不变**。

**步骤**：

1. 在设置页添加一个 **anthropic 协议**的模型条目（base_url 指向一个**没有 `/v1/models`** 的兼容端点，DeepSeek 的 `/anthropic` 即可）。
2. 点「测试连接」→ 观察**判通**（不是 404 误杀）。
3. **对照半**：加一个 **openai 协议**条目 → 仍然打 `GET /models`，行为不变。
4. **假失败半**：把 base_url 指向一个**不存在的路径**（`.../v1/nonexistent`）→ 应判**失败**（404 不算通）。
5. **假失败半二**：把 API key 填错 → 应判**失败**（401/403 不算通）。

**判据**：

- PASS：第 1 步**判通**（点测试连接显示成功）；第 2 步**没有 404 误杀**；第 3 步 openai 路径**不变**；第 4 步**判失败**（404）；第 5 步**判失败**（401/403）。
- FAIL：第 1 步**仍被 404 误杀**（探活还打 `/v1/models`）；或第 4 步**判通**（404 被当成路由可达——判据放宽过头）；或第 5 步**判通**（鉴权失败被当成可达）；或第 3 步 **openai 分支也被改**（不该动）。

**变异注入复验**：把 `commands.rs` 的 anthropic 判读条件从「4xx 非 401/403/404」改回 `status.is_success()` → 重跑第 1 步应 **FAIL**（404/400 误杀）。
第二个变体：把 404 也算通（条件写成「4xx 非 401/403」）→ 重跑第 4 步应 **FAIL**（路径错被当成通）。

**平台**：跨平台。**网络前置**：这一步需要外网（或本地假服务模拟 400/401/404 三种响应）。

---

## 三、Windows 专章

### W13 · Windows 上没有出网硬闸：明说断了，但实际没断

**靶点**：`src-tauri/src/py/runtime.rs:921-924`（非 macOS 分支）+ `src-tauri/src/py/mod.rs:27-28`（条件编译）+ `src-tauri/src/py/seatbelt.rs`（整文件仅 macOS 存在）

**背景**：M93 的 seatbelt 是 **macOS 专有**（`sandbox-exec`）。Windows 构建里 `crate::py::seatbelt` **根本不存在**，`runtime.rs` 走 `silent_cmd(py)` 分支——**无沙箱、无出网硬闸**，「run_python 不联网」只剩提示词层软约束。这是**拍板过的平台差异**（`docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md` 偏离登记），不是缺陷。

**本专章要验的不是「断了网」，而是「明说断了但实际没断」这件事被如实登记**——Windows 上不能假装 M93 通过。

**步骤**：

1. 确认 Windows 上 `src-tauri/src/py/seatbelt.rs` **不在构建里**（交叉编译或看构建产物）。
2. 在 Windows 真机上跑一段**带联网意图**的 `run_python`（`urllib.request` 打一个公网地址）→ 观察它**是否成功**。
3. 对照：同一段脚本在 macOS 上**失败**（M93 已验）。
4. 检查 SPEC 的已知缺口条目是否**明确写着** Windows 无出网硬闸。

**判据**：

- PASS（**按「诚实登记」判，不是按「断了网」判**）：第 2 步**如实记录**「Windows 上 run_python 能联网」这一事实；第 4 步 SPEC / 方案文档的**平台差异登记在案**；**没有人把 Windows 的结果写成「断网成功」**。
- FAIL：把 Windows 的结果**记成 PASS（断网生效）**（这是本专章要防的头号事故）；或 SPEC 里**没有**这条平台差异登记。

**平台**：Windows 专章。

---

### W14 · 目的地闸与凭据在 Windows：同一套逻辑，路径与凭据另有差异

**靶点**：`src-tauri/src/bot_web/destination.rs`（跨平台）+ `src-tauri/src/bot/tools/http.rs`（`resolve_secret` 的 `env:` 分支）+ `src-tauri/src/bot/config/types.rs:204`（`allowedDomains`）

**背景**：目的地闸逻辑**与平台无关**（纯函数 + keyring + 弹窗），Windows 上应当与 macOS **同口径**。差异在三处：

1. **`env:` 引用要求启动前设好环境变量**——Windows 上设环境变量的路径不同（系统属性 / PowerShell `$env:`），**中途设无效**（`std::env::var` 读的是进程环境）。
2. **白名单设置在设置页的同一处**（「联网工具目的地白名单」，textarea，一行一个域名）。
3. **文件路径反斜杠**——`saveAs` 落盘的返回路径在 Windows 上是 `C:\...\AI_Gen_Files\xxx.csv`。

**步骤**：

1. 在设置页加白名单域名 → 走 M94 的同一套流程（白名单内放行 / 白名单外弹窗 / 改配置后授权失效 / 302 到白名单外被拒）。
2. 用 `env:` 引用发一个带鉴权头的请求 → 确认**在启动应用前**设的变量能读到。
3. 用 `saveAs` 下载一个文件 → 确认落盘路径是**反斜杠**且文件真的在 `AI_Gen_Files` 里。

**判据**：

- PASS：第 1 步与 macOS **同口径**（白名单内不弹、白名单外弹、改配置后失效、302 被拒）；第 2 步 `env:` 引用**生效**；第 3 步文件**真实落盘**、路径形态为反斜杠。
- FAIL：Windows 上白名单**不生效**（或反过来，弹窗**永不出现**）；或 `env:` 引用读不到（多半是设环境变量的时机问题——**先排查这个再判 FAIL**）；或文件**没落盘**（路径拼接在 Windows 上坏了）。

**平台**：Windows 专章。

---

### W15 · 设置面与冒烟脚本的 Windows 人工段

**靶点**：`scripts/realmachine/smoke.sh`（**macOS 专用**：bash + `screencapture` + `osascript` + `lsof`）+ `src/components/SettingsPage/*`（参数卡退役后的设置面）+ `src-tauri/src/paths.rs`（`data_dir` 判定）

**背景**：两条 Windows 专属要求：

1. **冒烟脚本没有 Windows 版**——`scripts/realmachine/smoke.sh` 依赖 `screencapture` / `osascript` / `lsof`，Windows 上跑不了。**上一份的 W12（14 项人工过一遍）在本卡重列一遍**（本卡给 Windows 版的清单）。
2. **参数卡退役后设置页不能留残骸**——删掉一张整卡容易留下空容器、错位、或孤立的标题。

**步骤**：

1. **数据目录定位**（第一件事）：用 `regression-probe.ps1 locate -ExeDir "<exe 目录>"` 确认 `data_dir` 落在哪（exe 同目录 or `%APPDATA%\com.renshi.wmessage`）。
2. **冒烟 14 项人工版**（对应 macOS `smoke.sh` 的 A/B/C/D 四阶段）：
   - 启动无 panic（进程存活，stderr 无 panic）
   - `error.log` 零增量
   - 库完整性与表数（`sqlite3` 或 `PRAGMA integrity_check`）
   - 探针 `mark` / `report` 可读
   - 启动截图 + 四页渲染（首页 / 任务 / 聊天 / 设置）
   - 两轮真实往返（`trace.complete status=done` ×2）
   - Enter 发送口径（Shift+Enter 换行不发送）
   - 优雅退出清理链（`app_exit_cleanup`）
   - 文件面静态（`builtin_skills/` 无 `.tmp` 残渣；`runtime/flags/` 四旗标齐）
3. **设置页残骸检查**：打开设置页 → 确认「Agent 运行参数」卡**不在**、任务模式选择器**不在**、页面**无空容器 / 错位 / 孤立标题**。
4. **`data_dir` 跟随性**：确认 `bot.log` / `wmessage.db` / 技能目录都落在第 1 步定位到的目录里。

**判据**：

- PASS：第 1 步数据目录**定位明确**；第 2 步 14 项**逐项有结论**（红的当场查）；第 3 步设置页**无残骸**；第 4 步三类产物**都在同一目录**。
- FAIL：数据目录**定位不了**（后面所有观测都读错地方）；或 14 项里**有红项被跳过**（记「未覆盖」不算通过）；或设置页**有残留布局**；或产物**散在两个目录**（便携锚定没生效）。

**平台**：Windows 专章。

---

## 四、变异注入总表（V97–V135）

> 编号接上一份的 V76–V96。**每条变异 = 改一个字符/一行 → 重跑对应卡 → 判据应当 FAIL**。
> **注入前置**：改之前 `git stash` 或建 checkpoint 分支；**改完必须还原**（`git status` clean 才算收尾干净）。
> **注入位置以本文件列出的 `file:line` 为准**——行号是 HEAD（`42fe53e8`）现状，别的提交动过同一文件后要按符号名重新定位。

| 编号 | 卡 | 注入点（file:line） | 改动 | 重跑后应 FAIL 的现象 |
|---|---|---|---|---|
| V97 | M93 | **成立** | allow 注入→探测 SB_ERR 61→fail-closed 拒跑（seatbelt_unavailable） |
| V98 | M93 | `py/seatbelt.rs` 探测判定 | 「只认 `SB_ERR 1`」放宽成「任何 `SB_ERR` 都算生效」 | errno 61 被误判成闸生效 → 实际没拦却放行 |
| V99 | M94 | `bot_web/destination.rs:159` | 删掉白名单匹配那一行 | 白名单内外一个样，白名单**完全不生效** |
| V100 | M94 | `bot_web/http.rs:117` | `check_destination` 从循环内挪到循环外 | 302 到白名单外**被放行**（每跳判定失效） |
| V101 | M95 | `bot/dispatch.rs:21` | `sink_args_preview` 改回裸 `args.chars().take(80)` | 完整 URL 查询串回到 `tool.call` 审计面 |
| V102 | M95 | `bot/dispatch.rs:829` | trace span 从 `sink_args_preview` 改回 `clamp_span_text(...args...)` | **只有 trace 这一面漏**（Authorization 明文落盘） |
| V103 | M96 | `bot_fs/read.rs` `render_read_slice` | `line.chars().take(READ_MAX_LINE_CHARS)` → `line.chars()` | 超长单行原样返回、标注消失 |
| V104 | M96 | `bot_fs/read.rs` | 删掉字节预算分支（改回按行数） | 中文页不再变窄、撞回 64KB 转存 |
| V105 | M97 | `bot_model_loop/fuse.rs:59` | 收尾窗放行的写类调用从「不计」改回「照计」 | 写永远撞不到熔断线，收尾窗形同虚设 |
| V106 | M98 | `bot_model_loop/fuse.rs:75` | `round > r0 + WRAP_UP_ROUNDS` → `round > r0 + 99` | `fuse_wrap_up_rounds` 不出现，收尾窗无出口 |
| V107 | M98 | `bot/constants.rs:184` | `WRAP_UP_TOOLS` 删掉 `write_artifact_file` | 模型写不出产物，收尾窗变「只许读不许写」 |
| V108 | M99 | `bot_model_loop/core.rs` 盖章点 | 「最后一条 assistant 消息」改成「每一条都盖」 | 请求体暴涨、缓存前缀被改写 |
| V109 | M100 | `bot_model_loop/request.rs:692` | `attempt < MAX_LLM_ATTEMPTS` → `attempt < 5` | 上游收到 5 次请求而非 3 次 |
| V110 | M100 | `py/runtime.rs:48-54` | `None => (DEFAULT_TIMEOUT_SECS, ...)` → `None => (cap, ...)` | 不传 timeoutSecs 时默认变 600s 而非 120s |
| V111 | M101 | `bot_model_loop/shell.rs:279-283` | 删掉 `tool_in_flight` 标记翻转 | 长工具跑到 600s 被判停滞强停（误杀窗回归） |
| V112 | M101 | `bot/subagent_runner.rs:557` | `.unwrap_or(false)` → `.unwrap_or(true)` | DB 读失败时健康 run 被误停 |
| V113 | M102 | `bot/dispatch.rs:378-405` | 删掉子 agent 短路判断 | 空转确认窗回归（批准了也无效） |
| V114 | M102 | `bot/dispatch.rs:700-719` | 执行计数块挪回 JSON 闸之前 | 被闸拒的调用虚耗 `used_tool_calls` |
| V115 | M103 | `bot/subagent_tools.rs` `attach_stage_onion` | 删掉显式 `onion` 优先分支 | stage 覆盖显式配置档（优先级反了） |
| V116 | M103 | `bot_orchestrator.rs:130-136` | `Recon => "probe"` → `"writereditor"` | recon 挂上写档，工具面没收窄 |
| V117 | M104 | `prompts/system.rs:35` | 「最多 200 轮」改回「50 轮」 | 模型按 50 自我节流，安全网形同虚设 |
| V118 | M104 | `bot/registry/schemas.rs:139` | 「输出约 8KB 封顶」改回「64KB 封顶」 | 模型 print 全量 → 被转存成节选 |
| V119 | M105 | `bot_model_loop/request.rs:467-470` | 改回「只数字符串 content」 | 图片退出量尺，对 200K 硬顶失明 |
| V120 | M106 | `src/components/ChatPanel/askStore.ts` `applyAsk` | 整体替换改成原地改引用 | 提问卡出现但不刷新（`useSyncExternalStore` 拿到同一引用） |
| V121 | M107 | `src-tauri/src/ask_engine.rs:361` | 500ms 轮询环改回 `tokio::select!` | 超时与应答同时就绪时随机裁决 → 回答被扔 |
| V122 | M108 | `src/components/ChatPanel/useChatSessionActions.ts:333` | `if (streamState.endTurn(sid, turn))` 改无条件清场 | 旧轮 finally 清掉新轮（世代守卫失效） |
| V123 | M110 | `bot_model_loop/sse.rs:513-522` | 删掉攒批 2MB 超限截停 | `llm.sse_data_batch_truncated` 不出现，攒批无上限 |
| V124 | M115 | `bot/config/commands.rs` anthropic 判读 | 「4xx 非 401/403/404」改回 `status.is_success()` | 无 `/v1/models` 的兼容端点被 404/400 误杀 |
| V125 | M107 | **成立** | 豁免删除→提问挂起 28s 被判死（trace 442 stopped） |
| V126 | M109 | `bot_slash.rs:397` | 确认表排序从 `remaining_secs` 升序改成降序 / 去掉排序 | 重载还原**不是最早那条**（FIFO 反了） |
| V127 | M109 | `src/components/ConfirmMap/ConfirmMap.tsx:93` | `Math.min(TIMEOUT_SECS, remaining_secs)` 改成 `TIMEOUT_SECS` | 重载还原的倒计时**重置满 60s**（后端早已超时） |
| V128 | M111 | `bot_model_loop/shell.rs:29` | `impl Drop` 里删掉 `stream_log::finish` 调用 | panic 后会话**永久在途**（前端永转圈） |
| V129 | M111 | `bot/stream_log.rs:193` | `m.get_mut(session_id)?` 改回 `or_insert_with(...)` | 缓冲里**留孤儿条目**（每漏一条回合外事件多一条永久残渣） |
| V130 | M112 | `bot_orchestrator.rs:383-396` | 薄片内层环改成整步一把睡 | `/stop` 要等整步走完才生效 |
| V131 | M112 | `bot/subagent_runner.rs:41-69` | `write_terminal_with_retry` 重试次数改成 0 | 终态落库一次失败即**永挂 running** |
| V132 | M113 | `py/gate.rs:70` | 删掉停止复查分支 | 许可排队期间 `/stop` **不认**（最坏等 2×600s） |
| V133 | M113 | `bot/mcp/manager.rs` | `join_all` 改回 `for` 串行 | 坏 MCP 服务器把回合首 token **拖到分钟级** |
| V134 | M114 | `bot/config/commands.rs` `clamp_agent_params` | `final_selfcheck_round_gap` 的钳制去掉 | 越界值（999）**被写进配置** |
| V135 | M114 | `bot/params.rs:113` | `PARAMS_TABLE` 三行改回 24 行（引用已删的键） | 启动失败或门禁红（参数表 ↔ 常量表对拍） |

---

## 五、结果回填位

### 5.1 逐卡结论

> 执行者按卡逐条填。**PASS / FAIL / 未覆盖 三选一**，不要留空。**未覆盖必须写清为什么**（环境不具备 / 卡与卡之间没来得及复位 / 模型不配合 / 构造不出来）。

| 卡 | 结论 | 证据 | 备注 |
|---|---|---|---|
| M93 run_python 断网硬闸 | **PASS** | 探针行在案；联网失败(DNS gaierror)+纯计算正常+子进程 curl 000；apps 同享半未覆盖 | 系统性弹窗绕行后复验 |
| M94 目的地闸与 http_request | **PASS（核心半）** | dest=whitelist/asked/confirm_denied/session 全链在案 + saveAs 落盘 8090B | 每跳 302 与配置失效半未覆盖（无可控公网 302 源），见报告 §五 |
| M95 凭据引用与审计旁路面 | **FAIL（反向半）** | 四处 sink 面零 key 落盘 + 整值 env: 解析生效 | 发现 D-M95-1（Bearer 组合透传）+ D-M95-2（keyring 不存在厂商发空头），报告 §三 |
| M96 read_text_file 双闸 | **PASS** | 截断标注+模型知晓残段；en-3000 end=456 字节预算收页+续读 | — |
| M97 预算简化整套 | **PASS** | 参数卡整卡不在+旧键启动不报错+timeout_secs 告警 | 200/400 跑飞场景未覆盖 |
| M98 轮数轴收尾窗 | **PASS** | fuse_wrap_up_enter axis=rounds round=8 + deny read + 带正文硬停 | 工具轴对照半未覆盖 |
| M99 思考段无条件回灌 | **PASS + 发现** | reasoner msgs 6→8→11→13 全程 200 零 400 | 发现 D-M99-1：汇报路径 thinking 未回传 400×41，报告 §三 |
| M100 LLM 重试 3 次 + 超时三档 | **PASS** | retry×2+假服务恰 3 次+1.5s 固定间隔+recoverable 文案；/stop 打断生效 | 钳制半未单独驱动 |
| M101 子 agent 停滞判定三修 | **PASS（长工具半）** | sleep(40)>20s 未误杀 succeeded | 发现 D-M101-1（工具执行期停滞判定挂起，口径冲突）；pack 心跳/DB 读失败半未覆盖 |
| M102 子 agent 三道直接拒不弹窗 | **PASS（B 路）** | dest=subagent Warn 直拒不弹窗 | A/C 路与计数半未覆盖 |
| M103 三段式 stage 自动挂载 | **PASS（核心半）** | onion.stage_attached stage=recon onion_id=probe + profile=research | 其余四半未覆盖 |
| M104 提示词/schema 文案对齐 | | | |
| M105 压缩量尺全量序列化 | | | |
| M106 ask_user 聊天放开 + 提问卡 | **PASS** | 提问卡三行+chips+双按钮+通知角标；answer/assume 双路径闭环 | expired 路径由 M107 覆盖 |
| M107 提问/确认等待环竞态 | **PASS（expired 路径）** | 3s 过期→按假设继续+模型如实转告 | 临界同时应答与 oneshot 自旋半未覆盖 |
| M108 前端回灌状态机 + 赛跑防线 | **PASS（豁免半）** | 提问挂起 40s+ 未判死 | 判死-新轮/切走切回半未覆盖（单测兜底） |
| M109 bot-confirm 统一出口 + 还原 | **部分 PASS** | confirm_timeout 默认拒绝 fail-closed 在案 | /stop 打断/重载还原/FIFO/信封未覆盖 |
| M110 SSE 拆行/攒批/注释行 | | | |
| M111 流式内核兜底（panic） | **PASS** | 注入 panic→UI 立即失败终态+model_loop_panic+会话可继续 | — |
| M112 子 agent 等待环与终态落库 | **PASS（行为面）** | check_subagent 等待期 /stop→stopped | 薄片计量/落库重试半未覆盖 |
| M113 许可排队 /stop + MCP 并行 | | | |
| M114 设置页参数卡退役 | **PASS** | 整卡不在+无残骸+python_timeout 持久 | clamp 钳制半未覆盖 |
| M115 anthropic 探活改 POST | **未覆盖** | 插头按钮合成点击无响应（请求零发出） | 判定逻辑代码核读+5xx 判失败旁证 |
| W13 Windows 无出网硬闸 | 不适用 | 本机 macOS；Windows 实机待安排 | — |
| W14 Windows 目的地闸与凭据 | 不适用 | 同上 | — |
| W15 Windows 设置面与冒烟人工段 | 不适用 | 同上 | — |

### 5.2 变异注入结论

| 编号 | 卡 | 注入后判据 FAIL？ | 观测到的具体现象 |
|---|---|---|---|
| V97 | M93 | | |
| V98 | M93 | | |
| V99 | M94 | | |
| V100 | M94 | | |
| V101 | M95 | | |
| V102 | M95 | | |
| V103 | M96 | | |
| V104 | M96 | | |
| V105 | M97 | | |
| V106 | M98 | | |
| V107 | M98 | | |
| V108 | M99 | | |
| V109 | M100 | | |
| V110 | M100 | | |
| V111 | M101 | | |
| V112 | M101 | | |
| V113 | M102 | | |
| V114 | M102 | | |
| V115 | M103 | | |
| V116 | M103 | | |
| V117 | M104 | | |
| V118 | M104 | | |
| V119 | M105 | | |
| V120 | M106 | | |
| V121 | M107 | | |
| V122 | M108 | | |
| V123 | M110 | | |
| V124 | M115 | | |
| V125 | M107 | | |
| V126 | M109 | | |
| V127 | M109 | | |
| V128 | M111 | | |
| V129 | M111 | | |
| V130 | M112 | | |
| V131 | M112 | | |
| V132 | M113 | | |
| V133 | M113 | | |
| V134 | M114 | | |
| V135 | M114 | | |

### 5.3 未覆盖（诚实清单）

> **跑了什么、没跑什么，必须如实列**。空着不填 = 这份报告不可信。

| 卡 / 项 | 为什么没跑 |
|---|---|
| | |

### 5.4 环境操作台账

> 所有临时改动（常量、配置键、白名单、MCP 服务器、测试文件、注入的 bug）**逐条登记 + 还原**。收尾必须 `git status` clean + 干净启动验证通过。

| 改了什么 | 改在哪 | 还原了吗 |
|---|---|---|
| | | |

---

## 附录 A：窗口内 71 个提交的分类与兜底

> **出卡 23 条 = 卡片覆盖的 23 条产品行为变更。** 其余 48 个提交由下面的机制兜底，不出卡。

### A1. 出了卡的 23 条（对应 M93–M115）

| 卡 | 主要提交 | 提交标题 |
|---|---|---|
| M93 | `728d267d` | feat(py): macOS seatbelt 出网硬闸——自证探测 + fail-closed + apps 同享 |
| M94 | `cf57b52b` `73bdfd62` | feat(bot): 目的地闸基座 + http_request 工具 / http_request 支持下载到产物目录 |
| M95 | `fa794e24` | fix(bot): http_request 凭据口径堵齐三个审计旁路面 |
| M96 | `effedc6f` `0f10ebe6` | feat(agent): read_text_file 单行截断闸 / 默认页改字节预算 56KB |
| M97 | `8f33a09a` `6655ab52` `e4087f9e` `36ac4ce3` `82a180e7` `387d6dbb` `57aa6154` `705a4285` `b7e4f1fd` | refactor(agent): 撤墙钟整套 / 子 agent 独立预算退役 / 档位与比例派生退役 / 工具回填 128KB / 思考段「发了就回」 / 参数卡退役 / 工具数上限转发链全拆 / 风控常量单源收口 / refactor(skills): 技能墙钟退役 |
| M98 | `7e79b684` `6b589f0e` | feat(agent): 轮数轴收尾窗补实现 / 工具调用阶梯语义统一 |
| M99 | `82a180e7` | refactor(agent): 思考段回灌改「发了就回」 |
| M100 | `fb1bd0ac` `19d23d2b` `256fea25` | fix: Python 超时 120/600 + LLM 重试 3 次 / 超时天花板参数化 / 五条超时天花板收进单一真相源 |
| M101 | `0a4eae66` `20b3e3f5` `1c91a49d` | fix(bot): 子 agent 看门狗在工具执行期挂起停滞判定 / watcher 查询失败按未取消处理 / pack/unpack 执行期心跳 |
| M102 | `c5be8f61` `78485a8c` | fix(bot): 规则闸 ask 分支对子 agent 直接拒 / 子 agent 执行计数挪到入参 JSON 闸之后 |
| M103 | `c30e3af1` `1ac9834b` | feat(onions): 三段式 stage 派发自动挂载内置配置档 / 内置三档对齐三段式挂载前提 |
| M104 | `633ec41f` | docs(bot): 模型面文案三处对齐账实——50 轮/64KB/默认 8 |
| M105 | `38cf3740` | fix(bot_model_loop): 压缩分级量尺改全量序列化——图片与 arguments 计入 |
| M106 | `d7ec5c60` `b504d006` | feat(agent): ask_user 聊天放开 / feat(ui): ChatPanel 内嵌提问卡 |
| M107 | `fc3f4a25` `d424c992` `2364381c` `20c0f919` | fix(agent): ask_user 等待环先收应答后判超时 / 提问确认等待环 oneshot 关闭立即退出 / 工具自检侧调加「用户已停止」前置闸 / fix(chat): 提问等待期豁免 dead-man 看门狗 |
| M108 | `4ce6a1ca` `30852dbf` | fix(chat): 前端回灌状态机 / 回灌收尾的赛跑防线 |
| M109 | `969de3ca` `8c0cd928` | fix(agent): bot-confirm 统一出口 + /stop 可打断 + 重载还原 / 确认还原带后端剩余时长 + FIFO |
| M110 | `bf228f2c` `931e89fa` | fix(agent): SSE 一条事件拆多条 data 行 / SSE data 攒批补 2MB 上限 + 注释行 |
| M111 | `5a9a50f3` `431ca487` | fix(agent): 流式内核兜底 / check 等待守卫 + py 闸门族拆出 gate.rs |
| M112 | `07c94a55` `2af5a6bc` | fix(agent): check_subagent 等待切成 500ms 薄片 / 子 agent 终态落库加有界重试 |
| M113 | `de53b5af` `cef4dd96` | fix(agent): run_python 许可排队期间响应 /stop / perf(agent): MCP 待发现服务器并行连接 |
| M114 | `387d6dbb` `8f33a09a` `6655ab52` `e4087f9e` `36ac4ce3` | refactor(agent): 参数卡退役 + BotConfig 十键删除 / 撤墙钟 / 子 agent 预算退役 / 档位退役 / 工具回填 128KB |
| M115 | `d959272a` | fix(bot-config): anthropic 探活改打 POST /v1/messages |

### A2. 不出卡的 48 个（兜底机制）

| 类别 | 数量 | 提交 | 兜底机制 |
|---|---|---|---|
| **纯文档** | 19 | `436841d2` `47046397` `fe6d506c` `998fe975` `736b166b` `e39e80c4` `02e6fddc` `ac76f42c` `1d7c498e` `b28a1233` `073f5e72` `a87bd6a6` `be5e21d1` `3992a26c` `93b21e09` `3f9f99f8` `c9ab666d` `2176ef07` `d7b800cc` `42fe53e8` | 文档对拍门禁 `tests-audit/audit_doc_claims.py` + `audit_acceptance_matrix.py` + `audit_sop_baseline.py`；**零行为改动** |
| **纯重构（行为零变化）** | 6 | `705a4285` `57aa6154` `256fea25` `588aab54` `d7b800cc` `1ee1aca9` | 常量单源门禁 `tests-audit/audit_constants_single_source.py`（每个 `pub const` 须有**生产**引用 + 别处同名 const 只能是别名）+ 纯搬移核对器 `scripts/verify_pure_move.py`；`1ee1aca9` 是 vendor 补账（patch 数值本身是 10-27 的改动，只补注释标记与 `PATCHES.md` 登记） |
| **测试 / 门禁** | 4 | `736b166b` `cf5e3c21` `02e6fddc` `c9ab666d` | 单测与门禁脚本；`cf5e3c21` 把早退点锚点 7→8（对应 M102 的新早退点） |
| **出包记录** | 1 | `42fe53e8` | `docs/PACKAGING-WINDOWS-PORTABLE.md` 全流程日更出包；Windows 实机验收仍待人工（**登记进 §5.3**） |
| **设计稿落档** | 4 | `436841d2` `1d7c498e` `3992a26c` `93b21e09` | 纯文档，无代码改动 |
| **前端纯搬移** | 2 | `073f5e72` `2176ef07` | 前端纯搬移核对器 `scripts/verify_pure_move_tsx.py`（逐行 lstrip 后必须完全相同） |
| **其他已并入上表的提交** | 12 | — | 已在 A1 中作为主卡出现（如 `82a180e7` 同时服务 M97 与 M99、`387d6dbb` 同时服务 M97 与 M114） |

> **合计核对（已程序化校验）**：A1 覆盖的**唯一提交** 48 个（部分提交服务多张卡，如 `82a180e7` 同时服务 M97 与 M99、`387d6dbb` 同时服务 M97 与 M114、`20c0f919` 服务 M107），加 A2 的 23 个唯一提交 = **71**，与窗口提交数**完全相等**（`git log e6546f8a..42fe53e8 | wc -l` = 71）。**无遗漏、无多列**。

### A3. 本批删除的配置键总表（12 + 5 个）

| 后端 serde 键 | 前端 camelCase | 删除提交 |
|---|---|---|
| `subagent_max_wall_secs` | `subagentMaxWallSecs` | `8f33a09a` |
| `subagent_max_turns` | `subagentMaxTurns` | `6655ab52` |
| `subagent_max_tool_calls` | `subagentMaxToolCalls` | `6655ab52` |
| `max_tool_output_bytes` | `maxToolOutputBytes` | `36ac4ce3` |
| `task_mode` | `taskMode` | `e4087f9e` |
| `max_rounds` | `maxRounds` | `387d6dbb` |
| `max_function_calls` | `maxFunctionCalls` | `387d6dbb` |
| `history_budget_chars` | `historyBudgetChars` | `387d6dbb` |
| `history_truncate_target_pct` | `historyTruncateTargetPct` | `387d6dbb` |
| `reasoning_echo_enabled` | `reasoningEchoEnabled` | `387d6dbb` |
| `in_turn_context_edit` | `inTurnContextEdit` | `387d6dbb` |
| `search_max_results` | `searchMaxResults` | `387d6dbb` |
| `ask_budget` | `askBudget` | `387d6dbb` |
| `max_tokens`（全局） | `maxTokens` | `387d6dbb` |
| `reasoning_effort` | `reasoningEffort` | `387d6dbb` |

**新增的配置键**（本批唯一）：`allowed_domains` / `allowedDomains`（`Option<Vec<String>>`，设置页 textarea「联网工具目的地白名单」，`cf57b52b`）。

**废弃的 frontmatter 键**：技能的 `timeout_secs`（`b7e4f1fd`，解析忽略 + 告警一次）。

---

## 附录 B：本方案引用的审计事件名

> 权威清单见 `docs/acceptance/event-names.md`（250 条）。本表只列本方案用到的。

### B1. 本批**新增**的结构化事件名

| 事件名 | 卡 | 级别 | kv |
|---|---|---|---|
| `bot.http_request` | M94 / M95 | Info（成功）/ **Warn**（失败） | `host` / `method` / `status` / `bytes` / `dest` |
| `llm.sse_data_batch_truncated` | M110 | Warn | `bytes` |
| `model_loop_panic` | M111 | Error | `where` / `detail` |
| `onion.stage_attached` | M103 | Info | `stage` / `onion_id` |
| `onion.stage_attach_failed` | M103 | Warn | `stage` / `err` |
| `fuse_wrap_up_enter` | M98 | — | 轮数轴：`axis=rounds` / `round` / `max_rounds`；工具轴：`used` / `cap` / `round` |
| `fuse_wrap_up_deny` | M98 | — | `axis`（`rounds` \| `tool_calls`）/ `round` / `used` / `cap` / `tool` |
| `fuse_wrap_up_rounds` | M98 | — | `cap` / `entered_round` / `round` |

### B2. 本批**改动**的既有事件名

| 事件名 | 改动 | 卡 |
|---|---|---|
| `tool_rule.hit` | 新增 `action` 取值 `ask_denied_subagent`（**Info**；`deny` 分支是 **Warn**） | M102 |
| `tool.call` / `tool.return` | `http_request` 走 `sink_args_preview` / `sink_result_text` 裁剪 | M95 |
| `subagent_progress` | 发射位置从入参 JSON 闸前挪到闸后 | M102 |
| `bot-ask` / `bot-ask-resolved` | 本批**新引入**（`d7ec5c60`），走 `emit_recorded` 统一盖章 `sessionId`/`seq` | M106 |
| `bot-confirm` | 本批从裸 `app.emit` 改走 `emit_recorded`（调用方不再自带 `sessionId` 键） | M109 |
| `llm.retry` | `attempt` 上限 2 → 3 | M100 |
| `skill_budget_soft_warn` | kv 删除 `timeout_secs`（技能墙钟退役） | M97 |

### B3. 本批**删除**的事件名 / reason

| 事件名 / reason | 卡 |
|---|---|
| `subagent_budget_tool_calls_exceeded` | M97 |
| `tool.return` 的 reason `budget_wrap_up_deny` | M98 |
| `tool.return` 的 reason `budget_tool_calls_exceeded` | M98 |
| `WALL_FUSE_MARKER`（「⏹ 已熔断（墙钟上限）」）及其熔断出口 | M97 |

### B4. 自由文本日志行（**不是**结构化事件名，别去白名单里找）

| 字面量 | 卡 |
|---|---|
| `run_python \| seatbelt_probe \| 出网闸生效（sandbox 内 socket 被拒，探测证明）` | M93 |
| `run_python err \| kind=seatbelt_unavailable \| {why}` | M93 |
| `run_python \| timeout clamped \| requested={} cap={cap}` | M100 |
| `subagent_stalled \| id: {id} \| idle_ms: {n} \| 停滞超时，已强制停止` | M101 |
| `subagent_terminal_state_write_failed` | M112 |
| `confirm_timeout`（新增 `{reason}` 区分 `/stop` 与「60s 无响应」） | M107 / M109 |

---

## 附录 C：已知缺口与登记项（跑卡时别误判成缺陷）

### C-1 事件名白名单缺 5 条（本批新增，尚未对拍）——**跑卡前先补**

`docs/acceptance/event-names.md` 的 mtime 早于 `c30e3af1`（10-31 15:03），本批新增的 5 个结构化事件名**不在白名单里**：

| 事件名 | 发射点 | 卡 |
|---|---|---|
| `onion.stage_attached` | `src-tauri/src/bot/subagent_tools.rs:45` | M103 |
| `onion.stage_attach_failed` | `src-tauri/src/bot/subagent_tools.rs:57` | M103 |
| `fuse_wrap_up_enter` | `src-tauri/src/bot_model_loop/core.rs:734` / `:922` | M98 |
| `fuse_wrap_up_deny` | `src-tauri/src/bot_model_loop/core.rs:805` | M98 |
| `fuse_wrap_up_rounds` | `src-tauri/src/bot_model_loop/core.rs:748` | M98 |

现有门禁 `tests-audit/audit_acceptance_matrix.py` 只校验「卡里引用的名字在源码存在」，**不校验反向** → 这 5 条不会当场红。但白名单是「写卡的人可以引用的名字」的权威清单，**缺条就意味着下一个写卡的人不敢引用它们**。

**建议动作**：补进 `docs/acceptance/event-names.md`（表内 250 → 255 条）。**跑卡不阻塞**——本方案的判据直接引用源码里的字面量。

> 另有 `subagent_terminal_state_write_failed`（M112）是 `bot::audit_log` **自由文本**，按该文档第 17 行的例外条款**不进表是对的**，不是缺口。

### C-2 `fetch_url` 与 `web_search` 不走目的地闸——**拍板不做，不是缺陷**

D1 拍板（`docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md`）：**`fetch_url` 接目的地闸 → 不做**。理由是收益前提（提示词注入场景）碰上代价（读每个新网站都弹窗，审批疲劳的终点是把授权模式调成 yolo）。

SPEC 记了已知缺口，并用提示词防线兜底（`prompts/system.rs:77`「不得把本机文件内容或密钥拼进任何 URL 查询串或搜索词」）。

**跑卡时**：M94 只验 `http_request` 走闸。**不要**去验 `fetch_url` 弹窗——它不会弹，**那是拍板结果不是 bug**。`web_search` 同理（出口 `api.tavily.com` / `api.search.brave.com` 在 `PRODUCT_EGRESS_HOSTS` 里，但 `web_search` 根本不经目的地闸判定）。

### C-3 明文凭据仍有一条残留面——**已知，不在 M95 的 PASS 判据里**

`preview_args` 只裁 URL **不裁 headers** → **明文**（非引用形式）的 Authorization 会进 `tool.call` 的 `args_preview`（前 80 字符内）。schema 描述因此劝阻用引用形式。

**M95 的 PASS 判据只针对引用形式**（`keyring:` / `env:`）。明文路径**没有**被堵——若你用明文 key 去跑 M95，FAIL 是预期的，**按已知残留面登记**，不要当新发现。

### C-4 权限档在目的地维度是四档不是三档

`PermMode` 有四个变体：`Strict` / `Ask`（**默认**）/ `Auto` / `Yolo`。**目的地闸里 `Ask` 与 `Auto` 走同一分支**（`destination.rs:191`）——`auto` 档在目的地维度没有「白名单内自动」可言（上面已判过白名单）。

**跑卡时**：M94 说「三档」是简写，实际判据按 `dest` 字段走，**不要把 `ask` 与 `auto` 分开验**。

### C-5 `PRODUCT_EGRESS_HOSTS` 与 `web_search` 实际出口不完全对应

`PRODUCT_EGRESS_HOSTS`（8 条）里有 `api.tavily.com` / `api.search.brave.com`，但 `web_search` 的 **Bing / 百度抓取模式**走 `bing.com` / `baidu.com`——**不在清单内**。因 `web_search` 不经目的地闸（C-2），**不构成绕过**；但清单语义与实际出口不完全对应，登记备查。

### C-6 `verifier` 的 profile 派生有双源

`SubagentStage::default_profile` 对 verifier 返回 `research`，而 `default_onion` 挂的 `validator` 档 profile 是 `coder`。**两条派生路径结果不同**，由 `resolve_spawn_profile` 的优先级决定（本批未改该函数）。

**M103 的判据按实际生效面走**（工具面收窄正确），不要去断言 profile 到底是哪个。

### C-7 `Grants.gen` 是死字段

`destination.rs:56-59` 定义了 `gen: u64`，`invalidate_grants()` 会 `gen += 1`，但 `grants_contain` **只查 `entries`**——失效靠 `entries.clear()` 实现。`gen` 无读取点。**登记备查，不影响 M94 的判据**（失效行为是对的）。

### C-8 Windows 出包后实机验收仍待人工

`42fe53e8` 出了 Windows 绿色版 2026-10-31（`wmessage-portable-2026-10-31.zip`，SHA256 `9fbf1c645442862b8be54d6ec7700c90b50be2ff48a115a4e2666313bbcb8f1e`），但**解压、便携锚定、语义检索肉眼确认、ocr_image 四项实机验收仍待人工**——**本方案跑 W13–W15 时一并做掉**。

### C-9 Windows 上没有 seatbelt——**平台差异登记**

见 §1.0 ① 与 W13。**M93 在 Windows 上整卡跳过**；Windows 上的实际状态是「提示词说断网、实际没断」，这是**拍板过的平台差异**，SPEC 与整合方案里都有登记。**不要**把它写成 PASS。

---

## 附录 D：辅助材料与前置准备

### D1. 本地假 SSE 服务（M110 用）

```python
# /tmp/fake_sse.py —— 用法：python3 /tmp/fake_sse.py <模式> <端口>
# 模式 split   : 把一个 payload 拆成多条 data: 行
# 模式 comment : 在一条事件的两半之间插一行 ": keep-alive"
# 模式 flood   : 持续发无边界 data 行（累计 > 2MB）
import http.server, json, sys, time

MODE = sys.argv[1] if len(sys.argv) > 1 else "split"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8901

class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        payload = json.dumps({"choices":[{"delta":{"content":"这一行是完整的正文。"}}]})
        if MODE == "split":
            half = len(payload) // 2
            for part in (payload[:half], payload[half:]):
                self.wfile.write(f"data: {part}\n\n".encode()); self.wfile.flush()
        elif MODE == "comment":
            half = len(payload) // 2
            self.wfile.write(f"data: {payload[:half]}\n".encode()); self.wfile.flush()
            self.wfile.write(b": keep-alive\n"); self.wfile.flush()
            self.wfile.write(f"data: {payload[half:]}\n\n".encode()); self.wfile.flush()
        elif MODE == "flood":
            blob = "x" * 4096
            for _ in range(700):          # 约 2.8MB > 2MB 上限
                self.wfile.write(f"data: {blob}\n".encode())
            self.wfile.flush()
    def log_message(self, *a): pass

http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
```

> **注意**：`flood` 模式要把 `baseUrl` 指到这个本地服务——但 `127.0.0.1` 是环回，**公网闸会拒**（`check_public_url` 挡环回）。M110 需要一个**公网可达**的地址，或临时改闸；**改闸属于变异注入，必须登记与还原**。

### D2. 变异注入的前置与还原

1. **建 checkpoint**：`git checkout -b mutest/2026-10-29` 或 `git stash`（工作区当前有未提交改动，先处理掉）。
2. **一次只注入一个**：注入 → 重跑对应卡 → 记录 → **立即还原** → 再注下一个。**不要攒着一起注入**——攒着就分不清是哪个变异起的作用。
3. **还原检查**：全部跑完 `git status` 必须 clean；`git diff HEAD --stat` 必须为空。
4. **常量类变异**（M98 / M101 / M97 涉及 `bot/constants.rs`）改的是**生产常量**，跑完必须确认改回原值——**这批最容易漏**。

### D3. 需要临时改常量的卡（跑前跑后各对一次）

| 卡 | 常量 | 临时值 | 原值 |
|---|---|---|---|
| M98 | `MAX_ROUNDS` | 10 | 200 |
| M101 | `SUBAGENT_STALL_TIMEOUT_SECS` | 20 | 600 |
| M108 | `STREAM_DEAD_IDLE_MS`（前端 `constants.ts:45`） | 15 000 | 660 000 |
| M107 | `ASK_TIMEOUT_SECS`（后端 `ask_engine.rs:152`） | 3 | 24×3600 |

### D4. 可直接复制的观测命令

```bash
# 定位数据目录 + 打水位线
bash scripts/regression-probe.sh locate
bash scripts/regression-probe.sh mark

# 看水位线之后的事件（按域分组）
bash scripts/regression-probe.sh report

# 只看本批关心的几个事件名
grep -E 'bot\.http_request|fuse_wrap_up|onion\.stage|llm\.retry|llm\.sse_data_batch_truncated|model_loop_panic' "$DATA_DIR/bot.log"

# 只读 sqlite 看工具调用逐条参数与返回
sqlite3 "file:$DATA_DIR/wmessage.db?mode=ro" \
  "select turn, name, substr(args,1,120) from exec_spans order by id desc limit 30;"
```

---

## 附录 E：本份与前几份的关系

| 文档 | 覆盖窗口 | 卡片 | 状态 |
|---|---|---|---|
| `docs/realmachine-regression-test.md` | 10-19 ~ 10-22 | 34 项 + 9 项 Windows 专章 | 已执行（10-22 复验手册） |
| `docs/realmachine-mutation-test-2026-10-23.md` | 10-23 | 39 项 + 3 项 Windows | 已执行（发现 D-1 ~ D-4，均已修） |
| `docs/realmachine-mutation-test-2026-10-24.md` | 10-23 20:02 → 10-24 10:51 | 35 项 + 3 项 Windows | 已执行 |
| `docs/realmachine-smoke-test-2026-10-24.md` | 10-23 → 10-24 | — | 已执行 |
| `docs/realmachine-smoke-test-2026-10-25.md` | 10-24 10:51 → 10-25 15:36 | M75–M92 + W10–W12 | 已执行（报告 `realmachine-smoke-report-2026-10-25.md`） |
| **`docs/realmachine-smoke-test-2026-10-29.md`（本份）** | **10-28 20:54 → 10-31 16:27** | **M93–M115 + W13–W15** | **待执行** |
| `docs/ACCEPTANCE-PLAN-2026-10-30.md` | 按**功能条目**索引（全库 114 条） | `docs/acceptance/cards-*.md` | 与本份**互补**：按改动窗口 vs 按功能条目 |

> **两条验收面的关系**：`ACCEPTANCE-PLAN-2026-10-30.md` 按 SPEC 功能条目建卡（回答「这条功能验没验过」），本份按改动窗口建卡（回答「这批改动对不对」）。**本份出的卡不与验收卡重复**——本份只覆盖 10-28 之后的改动面；验收卡覆盖全库功能面。**跑完本份后，验收矩阵里「本窗口新增的行为」这一格才补齐**。
