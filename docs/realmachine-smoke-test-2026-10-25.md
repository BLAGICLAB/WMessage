# 真机冒烟测试方案：2026-10-24 → 10-25 变更回归

针对 **2026-10-24 10:51 → 2026-10-25 15:36** 这批改动的实机验收方案。**macOS 与 Windows 双平台**，两平台的测法有实质差异，见 §1.0。

与 `docs/realmachine-fulltest-2026-10-24.md`（覆盖到 `8887e50e`）**首尾相接、不重叠**：上一份验「10-23 夜里到 10-24 上午」，这份验「10-24 上午之后到 10-25 下午」。骨架同构——**每张卡都写清「把 bug 注回去，重跑应当 FAIL」的锚点**，证明这套测试有牙齿。

| 项 | 值 |
|---|---|
| 覆盖窗口 | `8887e50e`（10-24 10:51）→ `be529600`（10-25 15:36，HEAD），**116 个提交 / 55 个文件 / +3 250 −150 行** |
| 基线 | `8887e50e`（上一份冒烟与全量报告验的最后一版） |
| 提交构成 | refactor 66 / docs 20 / test 8 / fix 8 / feat 8 / perf 5 / chore 1 |
| 行为变更 | **17 条产品面**（perf 5 + feat 6 + fix 4 + 2 条改了审计写侧的 refactor）；另有 6 条 feat/fix 是**开发工具与门禁脚本**（eval 层、冒烟脚本、纯搬移核对器、冻结白名单）→ 不出卡，归附录 A。其中 **3 条已在上一份的报告里实机验过**（`721aaff9` 见 D-12、`c430bb7e` 见 D-11、`b5b996bf` 见 D-9）→ 本份不重复出卡，**1 条只补了提示词、行为面没验**（`bef13ba3`）→ 出卡 M89 |
| 卡片 | **M75–M92（18 张主卡）+ W10–W12（3 张 Windows 专章）**，覆盖 14 条未验过的产品行为变更 |
| 变异注入 | **21 条（V76–V96）**，总表见 §四（编号接上一份的 V26–V75） |
| 自动化基线 | `bash scripts/realmachine/smoke.sh`（macOS 14 项）——**先跑一遍**：它是「这版能不能起来、链路通不通」的地基，卡片跑在它之后 |
| 配套探针 | macOS/Linux → `scripts/regression-probe.sh`；**Windows → `scripts/regression-probe.ps1`**（与前几份同一套） |
| 兄弟文档 | `docs/realmachine-smoke-test-2026-10-24.md`（上一轮冒烟报告）· `docs/realmachine-fulltest-2026-10-24.md`（上一轮全量报告）· `docs/realmachine-mutation-test-2026-10-24.md`（上一份方案）· `docs/realmachine-regression-test.md`（10-19~10-22）· `docs/realmachine-rerun-2026-10-22.md`（复验手册）· `docs/CONTEXT-TOOLCALL-OPTIMIZATION-DESIGN-2026-10-25.md`（本批 P1–P8 的设计） |

---

## 〇、这份文档是什么

**是**：一份「怎么在真机上证明 10-24 上午之后这批改动是对的」的逐项清单。每一项给一条可直接粘贴的提示词（或一个可复制的 UI 操作）、一个能判 PASS/FAIL 的观测点、以及一次变异注入复验（把 bug 注回去，应当 FAIL）。

**不是**：不是单测的替代，也不是「再跑一遍功能」。单测证明纯函数在被喂进特定入参时表现正确；这里撬的是**跨模块接缝**——单测结构性够不到的地方：

| 接缝 | 坏了会怎样 | 单测能不能看见 |
|---|---|---|
| 注入块位置 ↔ provider 协议转换 | 记忆块挪回 system 消息，Anthropic 路径上整段历史的缓存前缀当场作废 | 不能（Anthropic 转换只在真请求里发生） |
| 工具结果裁剪 ↔ 痕迹表 ↔ 召回工具 | 早期结果被裁成桩子，而桩子里的 id 调不回原文 → 模型永久丢失那段输出 | 不能（要真跑一轮重工具才裁） |
| 侧调通道 ↔ 主循环判定 | 侧调拿不到用户意图 → 只会答 Y → 自检这道闸静默失效 | 不能（侧调是独立请求，独立模型） |
| 思考段 ↔ 上游 400 | 缺 `reasoning_content`，DeepSeek 在工具轮直接 400 | 不能（400 只在真上游发生） |
| 拆分波 ↔ 可见性 / 初始化序 | 文件拆开后某处 `pub(super)` 收窄、某模块晚初始化 → 运行时才炸 | 不能（纯搬移核对器只证文本相同，证不了运行时接线） |
| 审计写入 ↔ 探针解析 | `bot.log` 换了行格式，探针的 `report` 读不出来 → 判据全部落空 | 不能（行格式是运行时产物） |

**选样原则**：只收「错了会静默发生、且当场看不出来」的项。纯文档对拍、纯门禁脚本、纯格式化不在此列（它们归 `bash scripts/test-fast.sh` 管）。**这批 116 个提交里有 66 个是 refactor**——它们由两个纯搬移核对器兜底（Rust 用 `scripts/verify_pure_move.py`、前端用 `scripts/verify_pure_move_tsx.py`，逐行比对搬移前后是否完全相同），核对器不认的那部分（可见性改写、接线、初始化序）落到 **M91 / M92 两张功能面抽样卡**上。剩下的 14 条未验过的产品行为变更落进 18 张卡。

> **上一份已经验过的不在这里重复出卡**：`721aaff9`（LLM 错误说人话，上一份 D-12）、`c430bb7e`（脚本失败原因进 stderr，上一份 D-11）、`b5b996bf`（Replan 步骤兼容对象形态，上一份 D-9）——三条都已实机复验过，见上一份报告的附录。**复跑时不必重跑这三条**；但如果换过 exe，按上一份手册的顺序先复验它们。

**与「单测变异脚本」的关系**：仓库里已有 `scripts/mutation-trace-funnel.sh`——那是**单元测试层**的变异（注入 → 跑 `cargo test` → 看 KILLED / SURVIVED）。本文是**真机层**的同构物：注入 → 跑真实场景 → 看判据是否 FAIL。两者互补：单测变异能证明「有约束在守」，真机变异能证明「约束在真实环境里真的生效」。

---

## 一、前提条件与通用判据

### 1.0 Windows 实机适配（先读这一节）

Windows 上有五处平台差异会**直接改写测法**，不是「注意一下」的程度。本批**每张卡都要看这一节**。

#### ① 数据目录可能不在 `D:\Program Files\...`

便携包的 `data_dir` 判定顺序（`src-tauri/src/paths.rs`）：exe 同目录可写 → 锚定 exe 目录；不可写（如 `Program Files`）→ 回退 `%APPDATA%\com.renshi.wmessage`，并记一条 WARN `data_dir_fallback`。

> **第一件事：确认数据到底落在哪。**
> ```powershell
> powershell -ExecutionPolicy Bypass -File D:\wmessage-probe\regression-probe.ps1 locate -ExeDir "D:\Program Files\wmessage-portable-2026-10-25"
> ```
>
> **本批卡片的判据几乎全落在数据目录里**（`bot.log` / `wmessage.db` / 技能目录 / 摘要与记忆文件）——**不知道 `data_dir` 在哪，后面所有观测都读错地方**。

#### ② 端口闸在 Windows 上**不生效**——这不是 bug

`listening_supported()` = `cfg!(linux|macos)`。Windows 上没有进程快照与监听 socket 的采集路径，所以面板会明说「本平台暂不支持监听端口采集」。

**M85（端口闸采集金丝雀）整卡在 Windows 上跳过**——金丝雀的判据是「采样自称成功却看不见宿主必在监听的端口」，Windows 上根本没有采样这条路。其余卡片不依赖端口闸；M85 需要用「应用还活着吗」当反证时，Windows 上改用任务管理器看 CPU 与「应用无响应」状态。

#### ③ 停止阶梯走 `taskkill`

Windows 上没有 unix 那套信号语义，停止走 `taskkill /PID <pid> /T /F`。**本批没改停止阶梯**，但 W12（冒烟脚本的 Windows 人工段）要用 `Get-Process` / `tasklist` 对照——**子进程树是否真被收掉**，在 Windows 上要看 `/T`（树杀）有没有生效。

#### ④ 技能物化目录在 Windows 上是 `%APPDATA%` 下的兄弟目录

`builtin_skills/` 是用户技能目录 `skills/` 的**兄弟**（不是子目录）。M84（create_skill 会话档）要看技能文件落点，Windows 上如果数据目录回退到 `%APPDATA%\com.renshi.wmessage`，物化副本就在那里——**按回退后的真实目录找**，不能照 macOS 的路径猜。

#### ⑤ 其余平台差异速查

| 项 | macOS / Linux | Windows |
|---|---|---|
| 键盘 / 鼠标注入 | AX（辅助功能）+ CGEvent | UIAutomation + SendInput（**UIPI 会拦**） |
| 窗口 id | `pid:AXWindows下标`（**下标会复用**） | `pid:hwnd`（hwnd 不复用） |
| 前台判定 | 两级（应用 pid + 窗口标题），标题读不到就退化为「同应用即可」 | `GetForegroundWindow() == hwnd`（精确匹配） |
| 行尾 | `\n` | `\r\n`（**读 `bot.log` 找行要带 `\r` 容忍**） |
| 路径分隔 | `/` | `\`（**桩子与回执里的路径**在 Windows 上是反斜杠） |

### 1.1 这批的通用判据（每张卡都按这三条取证）

1. **审计行**：探针 `report` 读水位线之后的事件（`scripts/regression-probe.sh mark` / `report`）。事件名与本卡的「判据」一栏逐字比对——**行格式换了要照 §1.0 ⑤ 容忍 `\r`**。
2. **库里的地面真相**：只读 sqlite（`file:<db>?mode=ro`，不碰应用状态）。本批常用 `exec_spans`（工具调用逐条：`trace_id` / `turn` / `tool_call_id` / `name` / `args` / `result`）与 `exec_traces`（一次执行：`turn_count` / `tool_calls` / `prompt_tokens` / `cache_read_tokens`）。
3. **界面/气泡的可见结果**：截图。用户能看见的东西才叫「可感知」。

### 1.2 前置：跑之前先备好三样

1. **水位线**：`bash scripts/regression-probe.sh mark`（Windows：`regression-probe.ps1 mark`）——之后所有 `report` 只看这之后的行。
2. **基线冒烟**：`bash scripts/realmachine/smoke.sh`（macOS；**Windows 无对应脚本**，见 W12）。14 项全绿再往下跑；有任何一项红了，**先停下查那项**，不要带着地基问题跑卡片。
3. **一份「本轮要验的配置」清单**：本批有 4 个新参数（`loop.history_truncate_target_pct` / `loop.reasoning_echo_enabled` / `loop.in_turn_context_edit`，加上既有 `loop.history_budget_chars`）与 2 个只读行（`loop.tool_window_trigger_chars` / `loop.tool_window_keep_recent`）。**每张卡开跑前先记下当前这 6 个值**——卡与卡之间会改开关，改完不复位，后面的卡就全废了。

### 1.3 驱动方式（本批沿用上一轮的三件套）

| 手段 | 用途 |
|---|---|
| `scripts/regression-probe.sh`（`locate`/`mark`/`report`/`services`/`orphans`/`keyring`/`fixes`） | 日志水位、事件分组、进程与端口地面真相 |
| 只读 sqlite（`file:<db>?mode=ro`） | 工具调用逐条参数与返回、trace 汇总的 token 记账 |
| 截图 + 剪贴板回车 | 界面与气泡的可见结果；**驱动中文输入用剪贴板 + 回车，别用合成 `kp:return`**（上一轮已见输入法层把它吃掉，伪影不是应用问题） |

**弹窗 60s 窗口**：授权弹窗 / 工具确认弹窗 60 秒无响应即默认拒绝（fail-closed，符合预期）。**自动化驱动时，人手节奏必须 < 60s**，否则会吃到一次「拒绝」并误判成应用缺陷。

### 1.4 观测点速查

| 看什么 | 去哪看 |
|---|---|
| 审计事件 | `bot.log`（探针 `report`；结构化行是 `[时间戳] LEVEL \| 事件 \| kv`，无级别行是 `[时间戳] 事件 \| kv`，**两种都要认**） |
| 工具调用明细 | `exec_spans`（`args` / `result` / `turn` / `tool_call_id`） |
| 一次执行的汇总 | `exec_traces`（`turn_count` / `tool_calls` / `prompt_tokens` / `completion_tokens` / `cache_read_tokens`） |
| 缓存命中 | `llm.usage` 的 `cache_hit_pct` / `cache_read_tokens` / `input_tokens` |
| 每轮的请求体规模 | `llm.request` 的 `msgs_count` / `model` / `provider` / `reasoning` |
| 截断 | `chat.history_truncated`（`dropped` / `budget`） |
| 轮内裁剪 | `loop.tool_window.applied`（`chars` / `trimmed` / `keep_recent`） |
| 侧调自检 | `tool_selfcheck.sidecall`（`chars` / `verdict`）/ `tool_selfcheck.sidecall_failed`（`err`） |
| 注入失败 | `memory.injection_failed` |
| 端口采集 | `service.listening_canary_failed`（`canary_port`）/ `service.listening_sample_failed` |

---

## 二、场景卡

### T0 档：安全边界与数据破坏

### M75 · 轮内工具结果滑动窗：四条护栏一条都不能破

**靶点**：`src-tauri/src/bot_model_loop/context.rs`（`apply_tool_window` / `tool_result_chars`）+ `src-tauri/src/bot_model_loop/request.rs`（`compact_before_round`）（提交 `d753b125`）

**bug 形态**：重工具轮里，早期的工具结果在后续每一轮都被整段重发。实测一次提问的工具结果总量 p50=1071 / p75=9704 / p90=22637 / p95=32764 / max=359810 **字符**——尾部那档（一次 grep 几百行、一次整页文档提取）能在后面每一轮里反复付一遍。

改成：轮内累计工具结果超过 **24,000 字符**时，**一次性**把「最近 6 条之外」的工具结果换成桩子：`⚠️ 已省略以控制上下文。工具 {name} 的完整结果已存档（调用 id={id}），需要时调用 lookup_receipt(tool_call_id="{id}") 调取原文。`

四条护栏：**最近 6 条永不裁**（少一条模型就可能忘了上一轮刚做过什么，重跑工具 = 重跑副作用）、**只裁 tool 结果消息**（user/assistant 文本一个字节不动）、**带图片的消息整体跳过**（多模态形状不碰）、**只改 prompt，DB 记录零改动**（痕迹表与溢出文件原样保留，桩子给的是「调回原文」的入口不是「原文没了」）。

**一次性是硬要求**：改写会改掉请求数组的缓存前缀，反复改写等于把前缀打碎，省下的 token 又被缓存失效吃回去——触发之后窗口不再右移，前缀重归 append-only。

**步骤**：

1. 记下当前 `loop.in_turn_context_edit`（默认 **1** = 开）与 `loop.tool_window_trigger_chars`（**24000**）、`loop.tool_window_keep_recent`（**6**）。
2. 在聊天里让模型做一次**重读**：例如「读一下 `<某个大目录>` 下所有 `.log` 文件，把每个文件的行数告诉我」（用自己项目里的大目录，产出要够大）。
3. 继续追问，让它**基于刚才的结果再问几个问题**（把结果留在上下文里，逼后续轮次重发）。
4. 观察：`bot.log` 的 `loop.tool_window.applied`；被裁掉的那几条 tool 结果在库里是否还是原文；模型有没有把最近 6 条的结果搞混。

**判据**：

- PASS：`loop.tool_window.applied` 出现，且 kv 里 `keep_recent=6`、`trimmed` = 被裁条数 ≥ 0、`chars` ≥ 24000；**库里 `exec_spans` 的 `result` 列仍是原文**（桩子只活在 prompt 里）；模型引用最近 6 条里的内容时**内容正确**（没把被裁的那段当成最近一次的结果）；**用户消息与助手消息一个字没被裁**（模型能准确复述你前面说过的话）。
- FAIL：库里 `result` 也被改成桩子（DB 被动过）；用户消息被裁；被裁之后模型把旧结果当新结果用（说明「最近 6 条」这条护栏破了）。
- **不触发也算 PASS**：`loop.tool_window.applied` 没出现，说明这一轮没超 24,000 字符——换个更重的读再试，或临时把 `loop.tool_window_trigger_chars` 调到 5000 强制触发（**调完记得复位**）。

**变异注入复验**：把 `apply_tool_window` 的候选集从「只 `role=tool`」改成「`role=tool` 与 `role=user` 都算」→ 重跑应 **FAIL**（你的话被裁掉，模型开始记错你说过什么）。
同款变异的单测侧由 `bot_model_loop/tests/tool_window.rs` 兜着。

**平台**：跨平台；桩子里的 `tool_call_id` 与路径写法在 Windows 上不同（见 §1.0 ⑤），但判据不变。

---

### M76 · 旧轮工具结果的召回闭环：桩子指的路要真能走通

**靶点**：`src-tauri/src/bot/tools/trace.rs`（`call_lookup_receipt`）+ `src-tauri/src/bot/registry/schemas.rs`（`SCHEMA_LOOKUP_RECEIPT`）（提交 `eced677e`，与 M75 配套）

**bug 形态**：工具消息**不跨轮持久**——上一轮 `read_text_file` 的返回、上一轮 OCR 的整页文字，出了那一轮就不在上下文里了。模型想引用旧结果只有两条路：重跑工具（贵，写类工具有副作用）或凭记忆复述（编造）。

新增只读工具 `lookup_receipt`：不传参数 = 列出本会话最近的工具调用凭证；传 `tool_call_id` / `span_id` / `trace_id` = 调回那一次的完整返回。三条口径：**会话内限定**（别家会话的 id 一律按「查不到」处理，不泄露）、**查不到就说查不到**（明确提示 + 禁止凭记忆复述）、**回执不含溢出文件路径**（大结果的溢出文件路径不进回执，模型要自己按指针去读）。

**步骤**：

1. 第一轮：让模型读一个具体文件并把某个特定行号的内容报出来（例如「读 `<仓库路径>/AGENTS.md`，把第 42 行原样贴给我」）。
2. 第二轮（**新的一轮**）：问「刚才那个文件第 42 行是什么？不要重新读文件」。
3. 观察：模型是**调 `lookup_receipt`** 取回，还是重新读了一遍，还是凭印象编。

**判据**：

- PASS：第二轮的 `exec_spans` 里出现 `name='lookup_receipt'` 的行，`args` 带 `tool_call_id`（或 `span_id` / `trace_id`），`result` 里**逐字含**第 42 行的原文；模型贴出来的行与文件里那一行**逐字一致**。
- FAIL：模型直接复述但内容与文件对不上（编造）；或者模型重读了文件（没走召回，白花一次工具调用）；或者 `lookup_receipt` 返回空串 / 「查不到」而那条结果**确实在库里**（查不到就说查不到的判据反过来破了——该查得到的查不到）。
- **反向半**：让模型查一个不存在的 id（例如「用 `tool_call_id="call_does_not_exist"` 调回原文」），应当看到明确的「查不到」提示，**不能编一段原文出来**。

**变异注入复验**：把 `lookup_receipt` 查不到时的返回从「明确提示」改成空串 → 重跑应 **FAIL**（模型无法区分「没查到」与「结果是空」）。

**平台**：跨平台；回执内容是库里的原文，Windows 上路径分隔符不同（`args` 里的 `path` 参数）。

---

### M77 · 截断滞回：一次裁到 70%，末条永远留着

**靶点**：`src-tauri/src/bot_chat/summary.rs`（`truncate_split_point`）+ `src-tauri/src/bot_chat/flow.rs`（装配段）（提交 `67c50b2f`）

**bug 形态**：历史裁剪原先是「超预算就砍到刚好不超」——裁完那一刻的体量贴着预算线，下一条用户消息一进来又超，于是**每轮都裁一次**，每裁一次都动整段历史的缓存前缀。

改成**滞回**：一次裁到 **目标比例**（`loop.history_truncate_target_pct`，默认 **70**，钳 50..=90），裁完离预算线留出一段余量。配合 `loop.history_budget_chars`（默认 **100000**）用。

**步骤**：

1. 记下 `loop.history_budget_chars` 与 `loop.history_truncate_target_pct`。
2. 为了让本轮能触发裁剪，把 `loop.history_budget_chars` **临时调小**（例如 20000），让正常对话就能超。
3. 在同一个会话里连续聊 **5 轮以上**（每轮问一句能引出回答的话）。
4. 观察：`chat.history_truncated` 的 `dropped` / `budget`；**这条事件出现几次**；每轮之后的助手回复是否还记得**本轮**你说了什么。

**判据**：

- PASS：`chat.history_truncated` 出现的次数**远少于**对话轮数（滞回生效：裁一次管几轮，不是每轮都裁）；裁完之后**本轮用户输入仍在**（模型这一轮明确回应了你刚说的那件事，不是答非所问）；模型对更早的内容答不上来时**如实说「记不清/已被裁掉」**，不编。
- FAIL：每轮都出现一次 `chat.history_truncated`（滞回没生效，回到「贴着线裁」的老行为）；或者裁完**本轮输入被砍掉**（模型在答上一轮的问题）；或者模型对被裁掉的旧内容**编造出具体内容**。
- **参数档位**：把 `loop.history_truncate_target_pct` 改成 **50** 与 **90** 各跑一次，裁掉的条数应当随档位变化（50 裁得更狠、90 留得更多）——**参数不生效**（两个档位裁出一样多）算 FAIL。

**变异注入复验**：把目标比例改成 100（等于不裁）→ 重跑应 **FAIL**（裁剪后仍然超预算，`chat.history_truncated` 之后历史体量没降）。
同款变异的单测侧在 `bot_chat` 的 summary 段。

**平台**：跨平台。

---

### M78 · 思考段回灌：开与关是两副面孔

**靶点**：`src-tauri/src/bot_model_loop/stream.rs`（`stamp_last_assistant_reasoning`）+ `src-tauri/src/bot_model_loop/core.rs`（盖章点）+ `src-tauri/src/bot/params.rs`（`loop.reasoning_echo_enabled`）（提交 `3e60c46a`）

**bug 形态**：带思考模式的模型把思维链放在 `reasoning_content` 字段里，**带 tools 的请求必须把它随 assistant 消息完整回传**，缺了上游直接 400。DeepSeek 官方原话：「携带了 tools 参数的请求，在后续所有请求中，必须完整回传 reasoning_content 给 API……若未正确回传，API 会返回 400 报错」，且明确「即使该轮模型未实际进行工具调用」也要回传。

原先思考段只走前端展示出口、历史里一点不留，等于每次工具调用后的下一轮请求都在裸奔。

改成：设置页开关 `loop.reasoning_echo_enabled`（**默认 0 = 关**）打开后，OpenAI 兼容路径的 assistant 消息带上本轮 `reasoning_content` 原文。覆盖面比「工具轮那一条」宽——自检返工、幻觉守卫返工、长度续跑这三条路径都会再发一次请求，它们推回的 assistant 消息同样要带（盖章点统一落在「消息栈最后一条 assistant 消息」上，更早的内容一个字节不动，隐式前缀缓存不受影响）。**Anthropic 协议走 thinking 块原样回传，不受此开关影响。**

**步骤**：

1. 找一个**开思考模式**且走 OpenAI 兼容端点的模型（模型库里已 enabled 的 DeepSeek V4 Flash 即可）。
2. 在聊天里让它**调一次工具**（例如「列出我的待办任务」）。
3. 先在**开关关闭**状态下跑一轮，记下上游是否 400；再把 `loop.reasoning_echo_enabled` 改成 **1**，重跑同一句。
4. 观察：上游错误正文；`bot.log` 的 `llm.request`（`provider` / `reasoning` 两列）与 `llm.request_failed`。

**判据**：

- PASS（开）：带工具的那一轮**不报错**，模型正常给出调用结果；后续轮次也正常（说明回灌没有把协议打歪）。
- PASS（关）：**若该模型在思考模式下确实要求回传**，应当复现上游 400（错误体点名 reasoning / reasoning_content 之类）；**若上游不要求**，则开关开与关行为一致——**这一档没有反证也算 PASS**，把「上游到底要不要」记进报告（这正是默认取「关」的原因：省 token 与兼容 400 两条路各有一头）。
- FAIL：开关**开着**仍然 400（回灌没盖上）；或者开关**开着**时请求体明显变大而模型行为变差（回灌盖到了不该盖的消息上）。
- **Anthropic 路径不受影响**：用 MiniMax-M3（anthropic 端点）跑同一句，开关开与关都应当正常——变不了就是回归。

**变异注入复验**：把盖章点从「消息栈最后一条 assistant 消息」改成「给每一条 assistant 消息都盖」→ 重跑应 **FAIL**（请求体暴涨、缓存前缀被改写；`llm.request` 的 `msgs_count` 不变但流量大增）。

**复验修正（V76–V96 实测回填）**：V82/V83 两条变异（盖章点漂移 / 回灌开关反转）在本机不可激活——变异面要求模型真实产出 `reasoning_content`，DeepSeek 需显式开思考参数、MiniMax 走 Anthropic thinking 块根本不进该字段。跑这两条前先确认上游真的在回 `reasoning_content`（`llm.request` 的 `reasoning` 列 + 上游文档），否则如实记「不可激活」而不是 FAIL。

**平台**：跨平台（协议面，与操作系统无关）。

---

### M79 · 长历史的工具自检走侧调：三条硬条件与一道回落

**靶点**：`src-tauri/src/bot_model_loop/guard.rs`（`side_selfcheck_verdict` / `side_selfcheck_routes_to_sidecall`）+ `src-tauri/src/bot_model_loop/context.rs`（`user_intent_of`）+ `src-tauri/src/prompts/selfcheck.rs`（`SIDE_SELFCHECK_PROMPT`）（提交 `8cefaeb3`）

**bug 形态**：工具语义自检（变更类工具成功后的 Y/N 自评）每 5 轮占主循环一整轮，那一轮的请求体是**整段历史**——长会话里这是最贵的一次输入。改成长历史时判定挪到独立通道（`selfcheckModelId`）发一次小请求（几百字符），主循环省掉那一整轮。

**三条硬条件缺一不可**：① 配了侧调模型（没配就没有这条路）；② 消息栈够长（**超过消息预算的 1/5 = 40,000 字符**）；③ **拿得到用户意图**——侧调模型看不到主对话，不带诉求它只会答 Y，那道闸等于白设（比不做还糟：in-loop 至少偶尔能答 N 触发换策略）。意图取消息栈里最早与最近两条真人用户消息（首条取头、末条取尾：摘要与记忆块拼在用户原话**前面**，取尾才拿得到原话），各留 200 字。

**回落优先于判定**：侧调请求失败、返回空答、或首字符不是 Y/N——一律回落 in-loop 路径，**那道闸不丢**。

**步骤**：

1. **前置**：在设置页配一个自检侧调模型（`selfcheckModelId`，指向模型库里的一条已启用条目）。**没配的话整张卡跳过**——如实记「侧调未配，本卡未跑」。
2. 让模型做一次**变更类**工具调用（`create_task` 之类）。
3. 制造长上下文：让模型**一轮里读几个大文件**（每个几万字符的日志/文档），紧接着再调一次**变更类**工具——此时消息栈里那几条工具结果是全量的（滑窗只裁「最近 6 条之外」的），字符量能过 40,000。
   **怎么量**：只读查 `exec_spans` 的 `result` 列长度之和（工具结果是消息栈的大头），`SELECT SUM(LENGTH(result)) FROM exec_spans WHERE trace_id=?`；加上系统提示与对话文本，够 40,000 就能触发。
4. 观察：`tool_selfcheck` 相关审计行（`tool_selfcheck.sidecall` / `tool_selfcheck.sidecall_failed`）、主服务器的请求次数。

**判据**：

- PASS：`tool_selfcheck.sidecall` 出现，kv 里 `verdict` 是 `pass` / `fail`（不是 `unknown`）；主循环**少了一轮**（与同场景 in-loop 路径比，请求数少 1）；主循环里**没有**出现「【自检】」注入（走了侧调就不该再问一遍）；侧调失败时出现 `tool_selfcheck.sidecall_failed` 且**主循环照常继续**（没卡死、没把「判不出来」当「通过」）。
- FAIL：侧调判出来 `unknown` 却被当成通过；侧调失败后主循环**不再问自检**（闸被吞）；短历史也被换到侧调（判定力白让出去）。
- **短历史对照**：同一个会话里，历史没到 40,000 字符时应当**不**出现 `tool_selfcheck.sidecall`（留在主循环判）。

**变异注入复验**：
1. 把侧调 prompt 里的 `{intent}` 去掉（只留工具名 + 摘要）→ 重跑应 **FAIL**（侧调只会答 Y，闸静默失效）。
2. 把「侧调失败回落 in-loop」改成「侧调失败即视为通过」→ 重跑应 **FAIL**（闸被吞）。

**复验修正（V76–V96 实测回填）**：V86（侧调失败即视为通过）**不能用轮数当锚点**——侧调失败回落与侧调成功两条臂的主循环请求数相同（侧调不占主循环轮次），数请求分不出谁是谁。改锚结构断言：mock 侧调服务器**可控地返回失败**，断言主循环随后照常注入「【自检】」问句（回落闸仍能消费 verdict），或在 `tool_selfcheck.sidecall_failed` 事件后紧跟 in-loop 的判定事件。

**平台**：跨平台。

---

### T1 档：静默失效与状态谎报

### M80 · 记忆块与摘要并进本轮用户消息：功能不丢，缓存要回来

**靶点**：`src-tauri/src/bot_chat/inject.rs`（`build_injection` / `merge_injection` / `build_history_msgs`）+ `src-tauri/src/bot_chat/flow.rs`（装配顺序）（提交 `67c50b2f`）

**bug 形态**：隐式前缀缓存按「工具列表 → system → 消息」顺序做**最长前缀匹配**。摘要与记忆块每轮都变（摘要按截断生成、记忆块按本轮用户消息做语义检索），它们原先卡在主 system 之后、全部历史之前——一变，后面整段历史的缓存前缀当场作废，长会话每轮都在付全价 input。

改成**并进本轮用户消息**（不是另起 system 消息：Anthropic 协议转换会把数组里**所有** `role=system` 的消息抽出来提到顶层 system 字段，另起 system 消息在 Anthropic 路径上等于没挪）。并进去之后，用户原话留在消息**最后**（recency 权重最高的位置留给真正的指令）。

**步骤**：

1. 记下 `llm.usage` 的 `cache_hit_pct`（上一轮两轮对话是 100，**长会话才是这批改动的靶子**）。
2. 造一段**够长的会话**（≥ 20 轮，每轮都带工具调用，让历史涨到几万字符）。
3. 在会话**中段**问一句需要回忆的事：「我一开始让你做的是什么？」
4. 在会话**末段**再问一次同一件事。
5. 观察：模型的回答（记忆块有没有起作用）、`llm.usage` 的 `cache_hit_pct` / `cache_read_tokens` / `input_tokens`。

**判据**：

- PASS：模型**能引用早前的事实**（记忆块与摘要在起作用——这条不成立的话 P1 是把功能改坏了）；同一件事在中段与末段问出的答案**一致**；长会话的 `cache_hit_pct` 明显高于 0 且 `cache_read_tokens` 相对 `input_tokens` 占比高（**目标：长会话 cache_read 占 input ≥ 60%**——这是设计文档给 P1 定的验收线）。
- FAIL：模型**答不出**早前的事（记忆块没注入进去或注入了但没生效）；或中段与末段答案**不一致**；或长会话 `cache_read_pct` 长期为 0（缓存没回来——说明注入块仍卡在历史之前，或者改动被别的提交顶回去了）。
- **对照**：把 `bot_chat/flow.rs` 的装配顺序临时改回「注入块另起 system 消息放历史之前」，同一段会话重跑，`cache_hit_pct` 应**明显下降**——这一步做与不做都记录（做了要复原）。

**变异注入复验**：把注入块从「并进本轮用户消息」改回「另起 system 消息」→ 重跑应 **FAIL**（长会话缓存命中率掉下来；Anthropic 端点上尤其明显——转换层会把 system 提到最前面，等于没挪）。

**复验修正（V76–V96 实测回填）**：V88（注入块拼到用户原话之后）的 recency 探针**对 M3 级模型无牙齿**——强模型不看位置也能答对早前事实，行为面测不出退化。要么换弱模型采样，要么承认这条护栏只有结构保障（单测钉装配顺序），实机卡面按「弱模型保险」记账，本机不可证伪。

**平台**：跨平台；**Anthropic 与 OpenAI 兼容两条路径各跑一遍**（前者是这条改动的原始动机）。

---

### M81 · 记忆块注入失败要留痕，不能静默

**靶点**：`src-tauri/src/bot_chat/flow.rs`（注入段）+ `src-tauri/src/memory/*`（提交 `67c50b2f` 之后的连带面）

**bug 形态**：记忆块是「可选增强」——取不到就不注入，不该因此打断对话。但**静默**取不到会让人以为「它记不住我」，而真因（嵌入服务没起 / 库锁了）没人知道。

**步骤**：

1. 正常聊一轮（记忆块可用）——作为对照。
2. **制造注入失败**：停掉本地嵌入服务（或把记忆库文件临时改名，让读路径报错）。
3. 再聊一轮，看这一轮的 `bot.log`。

**判据**：

- PASS：对话**照常进行**（模型仍能回答，只是没有记忆块）；`memory.injection_failed` 出现（**有留痕**）；用户侧没有报错弹窗、没有卡住。
- FAIL：注入失败**没有留痕**（静默）；或者注入失败**打断了对话**（抛错到用户面前）。

**变异注入复验**：把注入失败分支里的审计调用删掉 → 重跑应 **FAIL**（静默失效）。

**平台**：跨平台。

---

### M82 · 易错工具的 schema 示例：首轮参数就该对

**靶点**：`src-tauri/src/bot/registry/schemas.rs`（`SCHEMA_CREATE_TASK` 等 5 条 schema）+ `src-tauri/tests/fixtures/tools_baseline.json`（提交 `0ae2e53c`）

**bug 形态**：JSON schema 只表达结构合法性，不表达使用惯例——参数写错 = 整轮重试（思考输出 + 工具重跑双倍浪费）。给**实测失败率最高**的五个工具在 description 里追加一行真实示例：`ui_type`（76% 失败）、`create_task`（45%）、`fetch_url`（40%）、`edit_task`（25%）、`complete_task`（21%）。

选点按**失败率**不按频次——高频的读类工具失败率本来就低（`read_text_file` 1.5%、`grep_files` / `list_files` 0%），给它们加示例是白占前缀。

**步骤**：

1. 让模型建一张带截止时间的任务卡（「建一张待办『季度复盘』，截止 2026-12-31 下午三点」）。
2. 看**第一次** `create_task` 调用的参数（`exec_spans` 的 `args` 列）。
3. 再让它建一张**不合法**的卡（列名写错之类），看它被闸拒绝后的**下一个动作**。

**判据**：

- PASS：第一次调用的 `args` 里 `due` 是 `2026-12-31T15:00` 形态（**T 分隔、分钟补零**）、`column` 是 `todo` 或 `doing`、日期没被写成空格分隔；**没有**因为参数错而重试一轮；卡片在库里建成、标题与截止时间都对。
- FAIL：第一次就填错格式（闸拒绝 → 重试 → 浪费一轮）；或者模型被闸拒绝后**自行去掉参数重试**（那是另一道纪律，见 M89）。
- **静态半**：`src-tauri/tests/fixtures/tools_baseline.json` 里这 5 条 schema 的 description 含示例行（夹具与源码同源，schema 改了要重生成夹具）。

**变异注入复验**：把 `SCHEMA_CREATE_TASK` 里的 `due` 示例删掉 → 重跑应 **FAIL**（模型更容易把时间写成空格分隔，被闸拒绝一次）。

**复验修正（V76–V96 实测回填）**：V89（删 due 示例）对 M3 级模型**无牙齿**——示例删掉后首轮参数照样对（强模型不靠示例）。这条卡的价值声明改为「对弱模型 / 其他厂商的保险」：结构面由 tools_baseline 夹具对拍钉死（示例在场），行为面本机不可证伪，不据此判 FAIL。

**平台**：跨平台（schema 面与操作系统无关）。

---

### M83 · 并行调用引导：同类读类操作该同轮发出

**靶点**：`src-tauri/src/prompts/system.rs`（规则 28）+ `src-tauri/tests/llm_integration/core.rs`（提交 `ac497156`）

**bug 形态**：一条 assistant 消息里带多个 `tool_calls`，引擎逐个执行、逐个回填 tool 消息，然后才发下一次请求——一轮发几个互不依赖的调用，成本几乎一样；一个一个轮次地发，每一轮都要重发整段上下文并多付一次思考输出。模型默认一次一个。

提示词按实测点名**读类**操作（批量读文件 / 批量 grep / 批量列目录 / 批量查任务卡 / 批量建卡 / 批量改名）。实盘 861 次调用里「同类工具连续独占 ≥3 轮」占 54.7%，而真正能并行的是读类（`read_text_file` 66 次 / `grep_files` 56 次 / `list_files` 53 次）；`check_subagent`（最长连续 28 轮）与 `run_python`（最长连续 16 轮）看着像同类，实则是「等子 agent」「看脚本输出」的天然串行，**合并不了也不该合并**。

**步骤**：

1. 让模型**一次读多个文件**：「读一下 `A.md`、`B.md`、`C.md`，各自第一段标题给我」（三个文件事先放好）。
2. 再让模型**一次列多个目录**。
3. 观察：`exec_spans` 里同一 `trace_id` 下**同一个 `turn` 有几条**。

**判据**：

- PASS：三个读操作落在**同一个 `turn`**（`SELECT turn, count(*) FROM exec_spans WHERE trace_id=? GROUP BY turn` 出现 count ≥ 2 的行）；模型**没有**为每个文件各发一轮。
- FAIL：三个读分三轮（`turn` 各 1）——引导没起作用（**注意**：模型偶尔不并行不算失败，要看**多次同类请求的分布**，别用单次判）；或者出现了**不该并行的合并**（把 `check_subagent` 与别的调用塞进同一轮）。

**变异注入复验**：把 system prompt 里的并行引导规则删掉 → 重跑应 **FAIL**（同类读类调用退回一轮一个）。

**复验修正（V76–V96 实测回填）**：V90（删并行引导规则）对 M3 级模型**无牙齿**——规则删掉后同类读类照样并行（引擎本就支持多 tool_calls，强模型自发用）。价值声明同 M82/V89：引导是「对弱模型的保险」，结构面由单测钉 prompt 内容，行为面本机不可证伪，不据此判 FAIL。

**平台**：跨平台。

---

### M84 · create_skill 的会话档：跨会话不许劫持

**靶点**：`src-tauri/src/bot_skills/parse.rs`（`scope: session` 强制）+ `src-tauri/src/bot_skills/manage/create.rs` + `src-tauri/src/bot_skills/manage.rs`（会话档不进全局路由表）（提交 `d403cc86`）

**bug 形态**：模型 `create_skill` 建出来的技能，触发路由**只在本会话生效**——重启后不再自动触发，用户可在设置页把它提升为全局。不这么做的后果是：一次会话里随口建的东西，**永久劫持**以后所有会话的路由（用户随口说的关键词从此被拉进一个陌生流程）。

强制点有两处：frontmatter 被强制写成 `scope: session`（模型自己写 `scope: global` 也会被覆盖），以及全局扫描**过滤会话档**（不进全局路由表）。

**步骤**：

1. 会话 A 里让模型建一个技能：「把这个流程存成技能：以后我说『测试触发词』就帮我做 X」（`intents` 里带一个冷门触发词）。
2. **同一会话**内再说一遍那个触发词 → 应当路由进这个技能（`skill.start` 出现）。
3. **新开一个会话 B**，说同一个触发词 → **不应**被劫持。
4. 完全退出应用并重启，再说一次触发词 → 仍**不应**被路由（会话档不跨重启）。
5. 去技能目录看文件落点与 frontmatter。

**判据**：

- PASS：步骤 2 有 `skill.start`（本会话立即生效）；步骤 3 与 4 **没有** `skill.start`（跨会话不劫持、重启失效）；技能文件里 frontmatter 含 `scope: session`（**哪怕模型自己写的是 global**）；设置页的技能列表里这个技能**还在**（文件在场）但**模型在别的会话里不被推荐它**（会话档不进全局路由表）。
- FAIL：会话 B 被同一个触发词劫持（跨会话劫持）；或者 frontmatter 里留着 `scope: global`（强制没生效）；或者重启后仍被路由。
- **副作用半**：会话档技能**不该**出现在全局技能路由的广告里（模型不该在别的会话里被推荐它）。

**变异注入复验**：把 `scope: session` 的强制注入去掉 → 重跑应 **FAIL**（模型写 global 就变 global，跨会话劫持）。

**平台**：跨平台；技能目录落点见 §1.0 ④（Windows 上是 `%APPDATA%` 下的兄弟目录）。

---

### M85 · 端口闸的采集金丝雀：采样自称成功却看不见，就是失明

**靶点**：`src-tauri/src/bot/services/gate.rs`（`api_canary_port` / `canary_blind` / `note_canary_blind`）（提交 `70c16321`）

**bug 形态**：端口闸靠「采样本机监听端口」判断受管服务有没有乱监听。采样路径是 `lsof` / `/proc/net/*`——**采集失败**与「确实没人在监听」在旧口径下混在一起：采不到就当没违规（放行），或者采不到就当有违规（误伤）。更糟的一种是「采样自称成功，但看不见**宿主自己**必在监听的 API 端口」——**采集失明**（如 lsof 权限不足），闸据此判「合规」，实际根本没在看。

金丝雀用宿主自己的已知事实当基准：本地 HTTP API 活着 → 它必在监听 `API_PORT`；采样说成功却看不见这个端口 = 失明。

**步骤**：

1. **开着**本地 HTTP API（设置里启用），确认宿主确实在监听那个端口。
2. 让服务巡检跑一轮（每 30s 一轮，等一分钟）。
3. 观察：面板的端口采集状态；`bot.log` 里有没有 `service.listening_canary_failed` / `service.listening_sample_failed`。
4. **反向注入**：macOS 上用「PATH 前置一个假 `lsof`」法——建一个临时目录，放一个立即退出且无输出的假 `lsof` 脚本，把它所在目录垫到巡检进程的 PATH 最前面（比「摘掉 lsof / 改系统文件权限」侵入小、可复原）→ 采样应当**明确失败**而不是装作成功。V92 实测已用此法复现成功。
   （Linux 走的是 `/proc/net/*` 那条路，破坏它要更狠——这条反向半**在 Linux 上可跳过**，如实记「未跑」即可。）

**判据**：

- PASS（正常）：采样成功且**看得见** API 端口 → 无金丝雀告警；面板的端口采集状态是「已采集」。
- PASS（反向半）：假 `lsof` 生效后，`service.listening_sample_failed` 出现（**采集失败有留痕**），闸**本轮不判**（既不判违规也不判合规）。
- FAIL：采集失败**没有任何留痕**；或者金丝雀失明时仍然判「合规」而没有 `service.listening_canary_failed`。
- **Windows 整卡跳过**（见 §1.0 ②：Windows 没有采集路径，面板明说不支持）。

**变异注入复验**：把 `canary_blind` 恒返回 `false` → 重跑应 **FAIL**（采集失明不再报警，闸在瞎报合规）。

**平台**：**macOS / Linux 专有**（Windows 跳过）。

---

### M86 · MCP 超限结果要留痕，原始字节数要能查

**靶点**：`src-tauri/src/bot/registry/*`（超限结果的回灌整形）（提交 `73f10b5c`）

**bug 形态**：外部 MCP 工具返回的结果超限时会被整形后回灌给模型，**原始字节数**如果不留痕，事后没人知道「这个工具到底吐了多少」——只能看到被截短的那一截。留痕的字段有两个：原始字节数、`server` 字段（区分来源服务器）。

**步骤**：

1. 配一台会返回大结果的外部 MCP 服务器（上一份文档 M50 的配方：`npx -y @modelcontextprotocol/server-everything` 一类）。
2. 让模型调它的返回型工具，让结果超限。
3. 观察 `bot.log` 与 `exec_spans`。

**判据**：

- PASS：出现 `tool.output.over_budget`，kv 里带**原始字节数**与 `server` 字段（来源可辨）；`exec_spans` 的 `result` 列里模型看到的是整形后的内容（不是全文）；模型**如实转述**了「结果被截短」这件事，没有编造后半段。
- FAIL：超限了但没有留痕；留痕里没有 `server`（分不清来源）；或者模型拿到的是全文（超限没生效）。

**变异注入复验**：把超限分支里的 `tool.output.over_budget` 审计调用删掉 → 重跑应 **FAIL**（无留痕）。

**平台**：跨平台（MCP 子进程的启停在 Windows 上见上一份 W04）。

---

### M87 · 审计写入统一行格式：探针还得读得出来

**靶点**：`src-tauri/src/audit/*`（`write_event` 单一写侧）+ `src-tauri/src/bot_model_loop/types.rs`（`ModelLoopDeps` 撤 `audit_log` 注入口）（提交 `645a9edf` / `227fdb2b`）

**bug 形态**：`bot.log` 此前有两种写法（free-form 行 + 结构化行），探针的 `report` 要同时认两种才读得全。统一到**一种写侧**（结构化：事件名 + kv）之后，判据「日志里出现某条事件」才真正可查。free-form 那批事件被拆成 `(event, detail)` 迁进结构化回调，守卫/自检失败/熔断记 Warn 进 `error.log` 镜像，工具流水记 Info。

**这条卡是本批所有卡片的观测底座**——判据全落在审计行上，行格式变了整批卡片的判据全部落空。

**步骤**：

1. 跑一轮带工具调用的对话（触发多类事件：请求、工具、守卫、熔断）。
2. `bash scripts/regression-probe.sh report` 看事件分组。
3. 直接 `grep` 一次 `bot.log`，看有没有**两种行格式并存**。

**判据**：

- PASS：`bot.log` 里**只有一种**行格式（结构化行：`[时间戳] LEVEL | 事件 | kv`；无级别事件是 `[时间戳] 事件 | kv`）；探针 `report` **列出的事件条数**与 `grep` 出来的条数一致（不漏读）；告警级事件在 `error.log` 里有镜像。
- FAIL：两种行格式并存；探针漏掉了某类事件（`report` 条数 < `grep` 条数）；或者 free-form 行里仍有未拆解的内容。

**变异注入复验**：让某处再写一条 free-form 行 → 重跑应 **FAIL**（格式分叉，探针读不全）。

**平台**：跨平台；Windows 上 `bot.log` 是 `\r\n` 行尾（见 §1.0 ⑤），`grep` 出来的行尾会多一个 `\r`——**比对时容忍**。

---

### T2 档：可感知语义

### M88 · 机器人气泡与用户气泡同一条限宽

**靶点**：`src-tauri/src/components/ChatPanel*`（气泡容器样式）（提交 `cdabd05e`）

**bug 形态**：机器人气泡原先通栏伸到头像底下，和用户气泡的限宽不一致——同一屏里两种宽度，读起来像两个来源。

**步骤**：

1. 聊天页发一条消息，让机器人回一段长文（**长到能看出限宽**）。
2. 截图。

**判据**：

- PASS：机器人气泡与用户气泡的**右边界对齐**（同一条限宽），气泡不再伸到头像下方。
- FAIL：机器人气泡通栏（比用户气泡宽出一截）。

**变异注入复验**：把机器人气泡容器上的限宽样式去掉 → 重跑应 **FAIL**（截图上看得出来）。

**平台**：跨平台（前端与操作系统无关，但**字体与缩放**在 Windows 上可能差几像素——按「右边界对齐」判，不按像素数判）。

---

### M89 · 参数被闸拒绝不许自行降级重试

**靶点**：`src-tauri/src/prompts/*`（底座提示词）+ `src-tauri/src/bot/registry/*`（参数闸）（提交 `bef13ba3`；上一份报告的 O-2 只补了提示词，**行为面没验**）

**bug 形态**：参数被闸拒绝时，模型若**自行改参重试**（把 `due` 从空串换成别的、把列名改掉、把整个参数丢掉），产出的东西**背离用户原始要求**——用户要「截止 2026-12-31」，模型建出来一张没截止时间的卡，而闸只拦了第一次。

底座提示词已补「参数被闸拒绝时转告原因并问用户，禁止换参/丢参降级重试」。**上一轮只验了提示词在位，这张卡验行为。**

**步骤**：

1. 提一个**带非法参数**的要求，让闸必然拒绝（例如「建一张卡，截止时间写成『下个月初』」——`due` 只认 `YYYY-MM-DDTHH:mm`）。
2. 看模型被拒之后的**下一个动作**。

**判据**：

- PASS：模型**如实转告**闸的原因并**问用户**要合法值（不自行改参、不丢参、不换个工具硬来）。
- FAIL：模型自行去掉 `due` 重试建卡（用户要的截止时间没了）；或者自行把「下个月初」猜成一个具体日期（编造用户没说的值）；或者换个写类工具绕过（破坏性替代）。

**变异注入复验**：把底座提示词里那条「禁止自行降级重试」删掉 → 重跑应 **FAIL**（模型开始换参重试）。

**平台**：跨平台。

---

### M90 · 终稿自检与工具自检共用侧调通道，两条线不许串味

**靶点**：`src-tauri/src/bot_model_loop/finalize.rs`（终稿自检侧调）+ `src-tauri/src/bot_model_loop/guard.rs`（工具自检侧调）（提交 `8cefaeb3` 与既有 `finalize.rs` 共用 `final_selfcheck_http`）

**bug 形态**：P8 让**工具自检**也走 `selfcheckModelId` 通道了。同一个侧调模型现在会同时接到两种请求：终稿自检（判「这稿有没有幻觉」）与工具自检（判「这次工具调用达没达意」）。两者的判定语义、事件名、失败处理都不同——串味的后果是**判据归错类**（拿终稿的 Y/N 当工具的结论，或反之）。

**步骤**：

1. 配好侧调模型。
2. 跑一轮会触发**终稿自检**的对话（长回复 + 多次工具调用）。
3. 再跑一轮会触发**工具自检**的对话（变更类工具成功 + 长上下文）。
4. 观察两类审计事件。

**判据**：

- PASS：终稿自检的事件是 `final_selfcheck.fail` / `final_selfcheck.sidecall_failed` 一族；工具自检的事件是 `tool_selfcheck.sidecall` / `tool_selfcheck.sidecall_failed` 一族；**两族互不出现对方的名字**；侧调模型的两次请求内容不同（终稿那次的 prompt 里有「终稿」字样，工具那次的里有「用户要求」字样）。
- FAIL：工具自检的结果被当成终稿自检的 verdict（终稿凭空多出一条 ⚠️ 或凭空少了一条）；或者终稿自检的失败把工具自检也带崩了（两个判定互相污染）。

**变异注入复验**：让工具自检复用终稿自检的判定分支 → 重跑应 **FAIL**（终稿事件里出现工具自检的名字）。

**平台**：跨平台。

---

### M91 · 拆分波功能面抽样 A：聊天、任务卡、技能

**靶点**：`bot_model_loop/` 13 个子模块、`bot/registry.rs` 三个子模块、`bot/services.rs`、`db/trace.rs`、`db/tasks.rs`、`bot_chat.rs`、`bot/tools.rs`、`bot_fs.rs`、`bot_py.rs`、`bot_archive.rs`、`bot_skills/*`、`db/mod.rs`、`memory/extract.rs`、`py/document.rs`、`tests/llm_integration.rs` → 目录形态（**66 个 refactor 提交**，10-25 05:38 → 13:58）

**bug 形态**：这批提交的主体会被两个纯搬移核对器兜住（`scripts/verify_pure_move.py` / `scripts/verify_pure_move_tsx.py`，逐行比对搬移前后完全相同）。核对器**证不了**的三件事：**可见性收窄**（`pub(super)` 改写后某处该能看见的看不见）、**接线顺序**（模块初始化序变了，某处读到未初始化的状态）、**测试专用导出的可见性**（门面转出一个只被测试引用的项，非测试构建下 unused import 报错）。

这三样只有真机能看见。

**步骤**（每条都是一分钟量级的真实操作）：

1. **聊天往返**：新建会话，发两句，看回复 + 用量条 + 右侧「任务 / 文件 / 子Agent」三页签。
2. **任务卡写口**：建一张卡 → 改标题 → 拖到「进行中」→ 标记完成 → 归档 → 删除。
3. **子任务**：给一张卡加两个子任务，勾掉一个。
4. **标签**：加两个标签（**上限 10 个 / 单个 30 字**），再加第 11 个看是否被拒。
5. **技能**：设置页技能列表渲染正常；点一次「恢复默认」看词表回填；`use_skill` 跑一个技能到收尾。
6. **长期记忆**：让模型 `remember_fact` 一条，第二轮问它记不记得。

**判据**：

- PASS：六条全部功能正常，**没有**「编译过但运行时报未初始化 / 不可见 / 类型不匹配」这类只在运行时才炸的错误；`error.log` 零新增（对比水位）。
- FAIL：任一条功能异常（尤其「按钮在但点了没反应」「列表少一项」「保存后不刷新」）；或者 `error.log` 新增 panic。

**变异注入复验**：把某个被拆出去的函数挪到**另一个模块**里（制造一次「不是纯搬移」的越界）→ `scripts/verify_pure_move.py` 应当先 FAIL（**这就是核对器的牙齿**），实机卡是第二道。

**平台**：跨平台；Windows 上见 W11。

---

### M92 · 拆分波功能面抽样 B：定时、图谱、设置页、子 agent

**靶点**：`src/components/SettingsPage*` → `types.ts` / `constants.ts` / 三个 hook / 五个 section 组件、`src/components/ChatPanel*` → 11 个自定义 hook + 纯解析函数外移、`graph-materials.ts`（着色器与材质工厂）、`bot_orchestrator/*` 五个子模块、`bot/subagent_*`（提交同上，**前端与编排两段**）

**bug 形态**：前端那 13 个提交搬的是**状态与 handler**——搬错了会出现「保存了不生效」「切分区丢状态」「事件订阅没解绑（重复渲染）」。编排那 5 个提交搬的是**子 agent 的派发/摘要/修复/收尾**——搬错了会出现「子 agent 不回摘要」「收尾 JSON 修不好」「派发参数错位」。

**步骤**：

1. **设置页**：改三个设置（推理强度、最大轮数、思考段回灌开关）→ 保存 → **完全退出应用重开** → 看三个值是否还在（`bot-config.json` 里的值与界面一致）。
2. **设置页分区切换**：在「基础 / 模型 / 工具 / 技能 / 记忆」之间来回切 5 次，观察有没有输入框内容丢失、列表重复。
3. **图谱**：打开图谱页，缩放 + 点一个节点看详情卡（**着色器与材质工厂被搬进 `graph-materials.ts`**——图渲染不出来是最直观的失败）。
4. **定时任务**：建一个 2 分钟后的作业，等它跑完，看「上次」时间更新 + 执行历史有行。
5. **子 agent**：让模型派一个子 agent（`spawn_subagent`）跑一个小任务，看右侧栏的动作流与收尾摘要。
6. **聊天输入**：Enter 发送 / Shift+Enter 换行 / 组合态不发送。

**判据**：

- PASS：六条全部正常；**设置保存后重启还在**（`bot-config.json` 与界面一致——这直接关系本批 P2/P6/P7 三个新参数能不能被用户真正打开）；图谱能渲染（着色器没丢）；子 agent 有动作流与摘要。
- FAIL：任一条异常；尤其「设置保存后丢字段」——**前端整份 config 回写会丢后端新增但前端没接的字段**，本批三个新参数若在设置页保存一次就消失，说明接线漏了。

**变异注入复验**：在设置页保存一次后，对比 `bot-config.json` 里 `historyTruncateTargetPct` / `reasoningEchoEnabled` / `inTurnContextEdit` 三个字段是否还在——**缺一个就是接线漏了**（可用改前改后的文件对比复核）。

**平台**：跨平台；Windows 上见 W11。

---

## 三、Windows 专章

### W10 · 本批落点全在数据目录：先把 `data_dir` 定位对

**靶点**：`src-tauri/src/paths.rs`（数据目录判定）+ 本批全部新面（`bot.log` / `wmessage.db` / 技能目录 / 摘要与记忆文件）

**为什么单独成章**：本批所有卡的判据都落在数据目录里，而 Windows 便携包**可能**落在 `%APPDATA%\com.renshi.wmessage` 而不是 exe 目录（`Program Files` 下不可写时回退，并记 WARN `data_dir_fallback`）。**读错地方 = 全部判据落空**，且现象是「什么都没发生」而不是「报错」。

**步骤**：

1. 跑 `regression-probe.ps1 locate`，**记下 `data_dir` 的真实值**。
2. 确认 `bot.log` / `wmessage.db` / 技能目录在那个位置（`data_dir` 与技能目录是**兄弟关系**，见 §1.0 ④）。
3. 跑一轮带工具调用的对话，回到 `data_dir` 看 `bot.log` 长了、库里多了 span。

**判据**：

- PASS：`bot.log` 与 `wmessage.db` 都在探针报出的 `data_dir` 里；对话之后两者都有新内容；`data_dir_fallback`（若有）只有一条且解释得通。
- FAIL：文件在别处（说明定位口径变了）；或者对话之后 `bot.log` 没长（**审计没写进去**——本批 M87 改的就是这条写侧）。

**平台**：Windows 专章（macOS 上 `data_dir` 恒在 App Support，无回退分支）。

---

### W11 · 前端拆分波与气泡限宽的 Windows UI 冒烟

**靶点**：`src/components/SettingsPage*`（13 个前端提交）+ `src/components/ChatPanel*` + `graph-materials.ts`

**为什么单独成章**：前端拆的是状态与 handler，**只有真机点得出来**；而 Windows 的字体与缩放和 macOS 不同，同一段 JSX 的实际排版可能差几像素——限宽类判据（M88）在两平台要分开看。

**步骤**：

1. 照 M92 的 1–3 条与 M88 跑一遍（设置页三保存 / 分区切换 / 图谱渲染 / 气泡限宽截图）。
2. 额外看**文字截断与滚动**：长标题的任务卡、长回复的气泡、设置页的长参数行（`loop.history_truncate_target_pct` 之类的行）。

**判据**：

- PASS：与 M92 / M88 同一套判据；**没有**横向滚动条、没有文字重叠、没有元素被挤出容器（Windows 的 DPI 缩放最常见的失败形态）。
- FAIL：出现横向滚动 / 重叠 / 元素溢出——这类问题在 macOS 上可能看不出来。

**平台**：Windows 专章。

---

### W12 · 冒烟脚本没有 Windows 版：14 项人工过一遍

**靶点**：`scripts/realmachine/smoke.sh`（**macOS 专用**：bash + `screencapture` + `osascript` + `lsof`）

**为什么单独成章**：`scripts/realmachine/smoke.sh` 把 14 项冒烟里可脚本化的部分自动化了，**Windows 上没有对应实现**（依赖 macOS 的辅助功能 / 屏幕录制 / `lsof`）。出 Windows 包之前，这 14 项必须**人工**过一遍。

**步骤**（照脚本的 14 项逐条人工做）：

| # | 检查项 | Windows 怎么做 |
|---|---|---|
| 1 | 应用启动无 panic | 启动后看进程活着、stderr 无 panic（便携包没有 stderr 时看任务管理器） |
| 2 | `error.log` 零增量 | 水位前后比行数 |
| 3 | `subagents` usage 三列在位 | `sqlite3` 或用只读连接查 `PRAGMA table_info(subagents)` |
| 4 | 库完整性 | `PRAGMA integrity_check` = ok |
| 5 | 探针 mark/report 闭环 | `regression-probe.ps1 mark` / `report` |
| 6 | 启动截图 | 截图工具手工截 |
| 7–9 | 四个页签渲染（首页 / 任务 / 聊天 / 设置） | 逐个切页看渲染 |
| 10–11 | 两轮真实对话往返 | 各发一句，看回复 + `trace.complete` 两条 |
| 12 | Enter 发送口径 | 手工敲 |
| 13 | 优雅退出清理链 | 关闭窗口后看 `app_exit_cleanup`（`api_stopped` / `mcp_stopped` / `exec_drained` 几列） |
| 14 | 文件面静态核验 | `builtin_skills/` 无退役残留、无 `.tmp` 残渣、`runtime/flags/` 四旗标齐 |

**判据**：14 项逐条 PASS / FAIL 记进报告（格式照上一份冒烟报告的两列表）。**有 FAIL 先停下查那项**，别带着地基问题跑卡片。

**平台**：Windows 专章。

---

## 四、变异注入总表（V76–V96）

编号接 `docs/realmachine-mutation-test-2026-10-24.md` 的 V26–V75。**每条都要求：注回去 → 重跑对应卡 → 应当 FAIL**；不 FAIL 的那条说明卡没有牙齿（判据太松），要改卡而不是改代码。

| 编号 | 注入点 | 对应卡 | 期望的 FAIL 现象 |
|---|---|---|---|
| V76 | `apply_tool_window` 的候选集从「只 `role=tool`」改成「含 `role=user`」 | M75 | 你的话被裁掉，模型开始记错你说过什么 |
| V77 | 去掉 `tool_window_applied` 的一次性门（每轮重裁） | M75 | 缓存前缀被反复改写，请求体在轮间跳动 |
| V78 | `lookup_receipt` 查不到时返回空串 | M76 | 模型分不清「没查到」与「结果是空」 |
| V79 | 桩子里的 `tool_call_id` 换成 `trace_id` | M76 | 桩子承诺的一步直达失效（强模型会多跳/降级自愈，看召回路径可观测性）——原判据「调不回原文」过强 |
| V80 | 截断目标比例改成 100（等于不裁） | M77 | 裁剪后仍超预算，`chat.history_truncated` 之后体量没降 |
| V81 | 去掉「最后一条本轮输入不可砍」 | M77 | 模型在答上一轮的问题 |
| V82 | 盖章点从「最后一条 assistant」改成「每条 assistant 都盖」 | M78 | 请求体暴涨、缓存前缀被改写 |
| V83 | Anthropic 分支也塞 `reasoning_content` | M78 | Anthropic 路径 400（转换层不认这个键） |
| V84 | 侧调路由阈值改成 0（永远侧调） | M79 | 短历史也换侧调，判定力白让出去 |
| V85 | 侧调 prompt 去掉 `{intent}` | M79 | 侧调只会答 Y，自检闸静默失效 |
| V86 | 侧调失败即视为通过 | M79 | 闸被吞（判不出来被当成「达意」）；锚点不能落在轮数（两臂同请求数），用结构断言——见 M79 卡复验修正 |
| V87 | 注入块改回「另起 system 消息」 | M80 | 长会话 `cache_hit_pct` 掉下来 |
| V88 | 注入块拼到用户原话**之后** | M80 | recency 探针对 M3 级模型无牙齿（行为面不退化）；护栏靠单测结构保障 |
| V89 | 删掉 `SCHEMA_CREATE_TASK` 的 `due` 示例 | M82 | 对 M3 级模型无牙齿（示例删掉首轮照样对）；价值 = 弱模型保险，结构面靠 tools_baseline 对拍 |
| V90 | 删掉 system prompt 的并行引导规则 | M83 | 对 M3 级模型无牙齿（引擎本就支持多 tool_calls，强模型自发并行）；价值 = 弱模型保险 |
| V91 | 去掉 `scope: session` 的强制注入 | M84 | 跨会话劫持 / 重启后仍被路由 |
| V92 | `canary_blind` 恒返回 `false` | M85 | 采集失明不再报警，闸在瞎报合规（反向半用 PATH 假 lsof 法复现成功） |
| V93 | 删掉超限分支的 `tool.output.over_budget` 审计 | M86 | 超限无留痕 |
| V94 | 让 free-form 行与结构化行并存 | M87 | 探针 `report` 漏读，判据全部落空 |
| V95 | 去掉机器人气泡的限宽样式 | M88 | 气泡通栏，与用户气泡不齐 |
| V96 | 把某个被拆出去的函数挪到另一个模块 | M91 / M92 | 纯搬移核对器先 FAIL；实机侧是「功能不响应」 |

**注入时的纪律**：

1. 一次只注**一条**，跑**对应那一张卡**，不扩散。
2. 注完 → 跑卡 → 记 FAIL 现象 → **立刻 `git checkout` 复原** → 再跑一次确认回到 PASS。
3. 变异体不提交。跑完 `git status` 必须是 clean（除报告文档外）。

---

## 五、结果回填位

跑完把本节填满，另起一份报告文档（`docs/realmachine-fulltest-2026-10-25.md` 一类），**不要就地改本方案**。

### 5.1 逐卡结论

| 卡 | 结论 | 关键证据 |
|---|---|---|
| M75 滑动窗四护栏 | | |
| M76 召回闭环 | | |
| M77 截断滞回 | | |
| M78 思考段回灌 | | |
| M79 侧调自检 | | |
| M80 注入并入本轮消息 | | |
| M81 注入失败留痕 | | |
| M82 易错工具示例 | | |
| M83 并行引导 | | |
| M84 会话档技能 | | |
| M85 采集金丝雀 | | |
| M86 MCP 超限留痕 | | |
| M87 审计行格式 | | |
| M88 气泡限宽 | | |
| M89 参数被拒不降级 | | |
| M90 两条自检不串味 | | |
| M91 抽样 A | | |
| M92 抽样 B | | |
| W10 数据目录定位 | | |
| W11 Windows UI | | |
| W12 Windows 14 项 | | |

### 5.2 变异注入结论

| 编号 | 注入后是否 FAIL | 判据够不够硬 |
|---|---|---|
| V76–V96（逐条） | | |

### 5.3 未覆盖（诚实清单）

跑完把**没跑到 / 跑不到**的逐条写下来，注明原因（环境不具备 / 编排不到 / 依赖未配置），并注明该面**由哪条单测或守卫兜着**。

---

## 附录 A：窗口内 116 个提交的分类与兜底

| 类别 | 条数 | 兜底方式 |
|---|---|---|
| `refactor`（纯搬移/拆分） | 66 | `scripts/verify_pure_move.py`（Rust）/ `scripts/verify_pure_move_tsx.py`（前端）逐行比对 + 全量门禁；**不可见性与接线**落到 M91 / M92 |
| `docs` | 20 | `tests-audit/audit_doc_claims.py` 对拍（引用的事件名必须在源码里存在）+ 人眼读 |
| `test`（测试与门禁脚本） | 8 | `scripts/test-fast.sh` / `test-all.sh` |
| `fix` | 8 | 其中 4 条上一份报告已验（D-9 / D-11 / D-12 + 卡片修正），其余入卡 |
| `feat` | 8 | 入卡（M79 / M80 / M82 / M84 / M85 / M86 等） |
| `perf` | 5 | P1–P8 全部入卡 |
| `chore` | 1 | 门禁 |

**行为类 21 条的归属**（17 条产品面 + 4 条工具/门禁面）：

| 提交 | 归属卡 | 备注 |
|---|---|---|
| `67c50b2f` P1 记忆/摘要并入本轮消息 | M77 / M80 / M81 | 三个面：截断、缓存、注入失败 |
| `eced677e` P3 `lookup_receipt` | M76 | 与 P7 配套 |
| `ac497156` P4 并行引导 | M83 | 附带 `scripts/tool-hotspots.py`（只读分析脚本，不入卡） |
| `0ae2e53c` P5 schema 示例 | M82 | 夹具需重生成 |
| `3e60c46a` P6 思考段回灌 | M78 | 默认关，待拍板 |
| `d753b125` P7 滑动窗 | M75 | 与 P3 配套 |
| `8cefaeb3` P8 侧调自检 | M79 / M90 | 与终稿自检共用通道 |
| `d403cc86` create_skill 会话档 | M84 | |
| `70c16321` 端口闸金丝雀 | M85 | Windows 跳过 |
| `73f10b5c` MCP 超限留痕 | M86 | |
| `cdabd05e` 气泡限宽 | M88 | |
| `645a9edf` / `227fdb2b` 审计写侧统一 | M87 | 本批所有卡的观测底座 |
| `bef13ba3` 参数被拒不降级 | M89 | 上一份只补了提示词，行为面本卡验 |
| `721aaff9` / `c430bb7e` / `b5b996bf` | — | 上一份报告已实机验过（D-12 / D-11 / D-9） |
| `f2c105f0` 评估层 / `ebb26c79` 冒烟脚本 | — | 开发工具，不是产品行为面（`ebb26c79` 本身是本方案 §1.2 的基线） |
| `de581426` / `ed6b9a7d` / `d51e77b4` / `a588ab7a` | — | 门禁脚本与冻结白名单（纯搬移核对器、审计守卫），由 `scripts/test-fast.sh` 兜 |

---

## 附录 B：本方案引用的审计事件名

| 事件名 | 出处 |
|---|---|
| `chat.history_truncated` | `bot_chat/flow.rs`（`dropped` / `budget`） |
| `llm.request` | `bot_model_loop/request.rs`（`model` / `msgs_count` / `provider` / `reasoning`） |
| `llm.usage` | `bot_model_loop/sse.rs`（`input_tokens` / `output_tokens` / `cache_read_tokens` / `cache_hit_pct`） |
| `llm.request_failed` | `bot_model_loop/request.rs` |
| `loop.tool_window.applied` | `bot_model_loop/request.rs`（`chars` / `trimmed` / `keep_recent`） |
| `loop.msgs.compacted` | `bot_model_loop/request.rs` |
| `tool_selfcheck.sidecall` | `bot_model_loop/guard.rs`（`chars` / `verdict`） |
| `tool_selfcheck.sidecall_failed` | `bot_model_loop/guard.rs`（`err`） |
| `final_selfcheck.fail` | `bot_model_loop/finalize.rs` |
| `final_selfcheck.sidecall_failed` | `bot_model_loop/finalize.rs` |
| `memory.injection_failed` | 记忆注入段 |
| `tool.output.over_budget` | 工具回灌漏斗 |
| `service.listening_canary_failed` | `bot/services/gate.rs`（`canary_port` / `detail`） |
| `service.listening_sample_failed` | `bot/services/gate.rs`（`detail` / `impact`） |
| `skill.start` | 技能启动 |
| `skill.enabled_changed` | 技能启用开关 |
| `user.message` / `trace.start` / `trace.complete` | 提问与执行审计链 |
| `app_exit_cleanup` | 退出清理链 |

> 事件名与源码同源这件事有门禁兜着：`tests-audit/audit_doc_claims.py` 逐个核对本方案里形如 `a.b` 的引用在源码里真的存在。**文档写了从未存在的事件，执行者在机器上 grep 不到那一行，只能靠猜**——这道闸就是防这个的。

---

## 附录 C：已知缺口与登记项（跑卡时别误判成缺陷）

| 项 | 状态 | 说明 |
|---|---|---|
| MiniMax OpenAI 原生格式的 `<think>` 段仍丢 | 登记未做 | 官方原生格式把思考段塞在 content 的 `<think>` 标签里，本仓库的 `feed_think` 会剥出做展示、不留进历史。走 **Anthropic 端点不受影响**。M78 用 DeepSeek 验，别用 MiniMax 的 OpenAI 端点验（会误判） |
| P6 思考段回灌**默认关** | 待拍板 | 受益（DeepSeek 不 400）与代价（多付一份思考段 input）落在同一批模型上。设置页可开；M78 的「关」那一档没有反证也算 PASS |
| P8 侧调**当前是惰性的** | 需要配置 | 复用终稿自检的 `selfcheckModelId` 通道；**没配侧调模型就没有侧调这条路**，M79 整卡跳过。如实记「侧调未配，本卡未跑」 |
| P7 阈值 24,000 字符 / 保留 6 条 | 代码常量 | 没做成设置项（收益本来就小，阈值挪一档远小于「配错导致质量下降」的代价）。改它要过 review |
| 工具自检**每 5 轮最多一次** | 既有口径 | 长会话里未必每轮都触发自检——M79 造长上下文时要有耐心，别以为「没看到侧调」就是坏了 |
| `error.log` 里的老错误不属于本窗口 | 既有 | 冒烟前的老日志行不是这批引入的，比对增量要看水位线前后 |
