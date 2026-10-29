# Agent 预算与风控简化方案（按业界口径重做，无墙钟无档位版）

> 状态：**方案稿，拍板已齐（§10），未动代码**。取代《Agent 风控参数最终定稿总表》的「三档 × 六预算 × 60/20/15 切分 + 墙钟」模型。
> 产品前提：**没有无人值守场景**——所有执行（聊天、手动、定时、批量）都在用户会看的环境里跑，`/stop` 随时可用。因此不设任何会话级墙钟。
> 业界口径来源：Claude Code 官方文档（env-vars / cli-reference / sub-agents / skills）、Cursor Agent 文档、Gemini CLI 配置参考、OpenAI Agents SDK 源码、GitHub Copilot coding agent 文档、LangGraph / AutoGen / Aider 源码；调研全文见 2026-10-28 会话记录，关键数字在 §2 表内注明。

## 1. 一句话结论

**把「预算」拆成两件互不相干的事，各用业界的办法管；时间维度只留在工具级：**

| 事 | 业界怎么管 | 本方案 |
|---|---|---|
| 跑飞了要能停 | 轮数/工具数**安全网**，默认高、不分档（Claude Code 交互无上限，LangGraph 1000，AutoGen 100） | 一套固定安全网，主会话与子 agent **共用**：200 轮 / 400 工具调用 |
| 单个工具卡死 | **工具级空闲/停滞超时**（Claude Bash 2 分钟默认 / 10 分钟上限，MCP 5 分钟空闲；Gemini shell 5 分钟无输出；Claude 子 agent 停滞 10 分钟） | Python 120s / 600s，LLM 首包 60s、流空闲 120s，子 agent 停滞 600s |

**没有会话墙钟、没有档位、没有子 agent 独立预算。** 交互式产品（Claude Code / Cursor / Gemini CLI）都不设会话墙钟；业界唯一的会话硬墙钟是 Copilot 云端 59 分钟——那是「用户不在场」的场景，本产品不存在。共享信封在业界要么是钱（Claude Code `--max-budget-usd`、Devin ACU）要么是时间（Copilot），从来不是轮数——一轮的成本相差十倍，轮数不是可加总的资源。

## 2. 设计原则（每条对应一个业界事实）

1. **轮数上限是安全网不是产品功能。** 四家交互式产品默认都不限轮数（Claude Code / Cursor / Gemini CLI / Codex），Cursor 取消了 25 次/轮，LangGraph 把递归上限从 25 提到 1000。→ 安全网固定一套、主会话与子 agent 共用，用户基本感知不到。
2. **不设会话墙钟；时间兜底全在工具级。** Claude Code 交互无墙钟，用 Bash 2/10 分钟、MCP 5/30 分钟、子 agent 停滞 10 分钟兜底。→ 撤掉现 `WallBudget` 全链路（主聊天、任务执行、子 agent、技能），新增子 agent 停滞超时。
3. **触顶留产可续。** Claude Code 子 agent 撞 `maxTurns` 返回 partial 可 resume；LangGraph 给 `RemainingSteps` 让节点提前收尾。→ 保留 90% / 95% / 100% 三段阶梯（两轴：轮数、工具数），用「收尾轮数」替代「+5 次写文件裕量」。
4. **上下文用比例触发 + 保留最近 N 条 + 固定截断 + 溢出转存。** Gemini 50% 压缩，API context editing 保留最近 3 条，Bash 30K / Gemini 40K 字符截断，超大输出给「文件路径 + 预览」。→ 历史预算 128K 与上下文硬顶 200K 两层常量，压缩阶梯挂在硬顶上；工具回填固定 128KB 并转存，不分档、不可配。
5. **用户面零拨杆，工程常量全隐藏且单源。** Cursor 几乎零拨杆，Claude Code 只给 env 变量与两个 headless 标志。→ 设置页**不再有「Agent 运行参数」卡**；权限模式留在原处（API 卡），Python 沙箱开关与超时留在「机器人设置」卡；其余全部是 `constants.rs` 里的常量，没有配置键。

## 3. 新模型：设置页零 Agent 参数 + 工程常量

### 设置页

「Agent 运行参数」卡**整张删除**（档位选择器、三预设、全部参数行）。与 Agent 相关、仍留在设置页的只有两样，且都不在这张卡上：

| 控件 | 位置 | 说明 |
|---|---|---|
| **权限模式**（严格 / 询问 / 自动 / yolo） | API 卡（`BotApiSection`，现状） | 工具执行是否弹窗；旧「保守/标准/放开」预设随卡退役，不再有任何地方顺手改它 |
| **Python 沙箱开关 + 单次超时** | 「机器人设置」卡（现状） | 工具级设置，不是 Agent 预算；默认 120s、上限 600s（§4.5） |

`maxRounds` / `maxFunctionCalls` 两个高级覆盖键**也不保留**——没有 UI 的配置键对非开发者用户就是不存在，留着只是一条没人走的旁路。

### 工程常量（`bot/constants.rs` 单源）

全部数值见 §4。原则：**一个数只在一处**——现在 `constants.rs` 里四对零引用的死副本（`SUBAGENT_WRAP_UP_FUSE_MARGIN` / `HISTORY_TRUNCATE_TARGET_PCT_MIN·MAX` / `MAX_REPLANS` / `MAX_PLAN_STEPS`）与别处字面量合并；`request.rs` 的 `MSGS_BUDGET_CHARS = 200_000` 改名 `CONTEXT_CHARS_HARD_CAP` 挪进 `constants.rs`，与 `HISTORY_BUDGET_CHARS = 128_000` 并列为上下文的两层常量（§4.4）。没有用户输入就没有钳制——各项「硬顶」随可配性一起退役，只有模型条目级 `max_tokens` 这条仍有用户输入的路径保留钳制；上下文那个「硬顶」是请求体的绝对上限，不是钳制边界。

### 参数注册表的去留

`PARAMS_TABLE` / `bot_effective_params` 是为渲染 Agent 卡而生的（Agent 透明化设计 §3）。卡删了，注册表只剩三个卡外消费者：数据管理卡的保留期默认与可填范围（`trace.retention_days`）、记忆区的抽取限频文案（`memory.extract_min_interval_secs`）、Python 开启确认框的超时文案（`tools.python_timeout_secs`）。**表缩到这三行**，命令与 `ParamView` 形状不变（不另造新命令）；`AgentParams` trait、`cfg_resolver!` 宏、全部 `resolve_*` / `clamp_*` 与写侧 `clamp_agent_params` 一并删除。

## 4. 数值表

### 4.1 固定值（原档位映射退役）

| | 值 | 硬顶 | 依据 |
|---|---|---|---|
| 会话历史字符预算 | 128K（`128_000`） | 200K（`200_000`，上下文硬顶） | 两层：**历史预算**管持久化的对话历史——超过 128K 丢最旧 + 摘要、裁至 70%（≈90K）；**上下文硬顶**管单次请求 msgs 总字符（历史 + 轮内工具结果 + 系统段）——压缩阶梯 70% / 85% / 100% 挂在它上（§4.4）。字符数没有 2 的幂对齐的理由，两者都取十进制 K，与现 `200_000` 同口径。无配置键 |
| 单轮输出 max_tokens | 16384 | 32768（仅钳模型条目级输入） | 现均衡档值；全局键删除，模型条目级 `max_tokens` 覆盖保留 |

### 4.2 安全网（固定常量，无配置键；主会话与子 agent 共用）

| | 值 | 依据 |
|---|---|---|
| 轮数 | 200 | 落在 AutoGen 100 与 LangGraph 1000 之间，比交互「无上限」保守；主会话与子 agent 同一个数。技能 frontmatter `max_rounds` 只许往下钳 |
| 工具调用 | 400 | 独立于轮数的一轴：并行调用多的会话先撞工具闸、串行长链的先撞轮数闸，两轴各管各的，不由「每轮几个工具」的配比派生 |
| 收尾轮数（进入 95% 收尾窗后最多再跑） | 3 轮 | LangGraph `RemainingSteps` 思路；替代「+5 次写文件裕量」 |

原 `MAX_ROUNDS_CAP = 500` / `MAX_FUNCTION_CALLS_CAP = 1200` 是给用户输入钳制用的，随配置键一起删除。

### 4.3 子 agent（无独立预算）

子 agent **不再有独立的预算三轴**（`SubagentBudget` 退役）：轮数/工具数与主会话共用同一套安全网（§4.2），派发时 LLM 自报的 budget 参数从工具 schema 与提示词中删除。循环终结论上仍需要轮数上限，但它就是主安全网那份，不是单独一套值。

| | 值 | 依据 |
|---|---|---|
| 停滞超时（无 LLM / 工具进展判停） | **600s** | Claude Code `CLAUDE_ASYNC_AGENT_STALL_TIMEOUT_MS` 同值；是所有工具单次最长时长（Python 600s）的上界，工具在跑不会被误判 |
| 并发 | 全局 3 / 每会话 2 | 桌面单用户保守值（Claude Code 默认 20，桌面端不需要） |
| 嵌套 | 禁止（现状） | Claude Code 默认 3 层，本产品暂不开放 |
| 触顶行为 | 留产（现状）；resume 二期 | Claude Code partial + resume |

子 agent 的闸从五个（轮数/工具数/墙钟/并发/停滞预告）收敛为四个：**共享安全网两轴 + 并发 + 停滞超时**。

### 4.4 上下文治理（固定）

| | 值 | 依据 |
|---|---|---|
| 历史裁剪 | 超过历史预算 128K → 丢最旧 + 摘要，裁至 70%（≈90K） | 现机制不变，仅基数 100K → 128K |
| 请求体压缩阶梯：软压 / 硬压 / Fuse | 上下文硬顶 200K 的 70% / 85% / 100%（140K / 170K / 200K） | 现机制不变（`MSGS_BUDGET_CHARS` 改名挪入 `constants.rs`，值不变）；软压/硬压只降工具消息密度，Fuse 拒收新工具调用 |
| 裁剪回落目标 | 70%（隐藏，不可配） | 定稿表 §二 本就列为隐藏常量 |
| 轮内工具结果滑动窗 | 24K 字符触发，保留最近 6 条 | API context editing 保留最近 3 条同路 |
| 单条工具回填 payload | **128KB 固定即硬顶**，不分档、不可配；超限转存文件 + 节选 + 指针；MCP 挂载用同一个数 | 业界截断按字符计：Claude Bash 30,000 字符、Gemini 40,000 字符。本产品按字节计且以中文为主（UTF-8 每字 3 字节），128KB ≈ 4.3 万字，恰在 Gemini 40,000 字符的量级；也是现均衡档与 MCP 挂载的现值，默认用户零变化。「文件路径 + 预览」正是 Claude Code 的做法 |
| 证据（Receipt）单条 | 64KB | 不变 |

### 4.5 工具级超时与重试（固定，全产品唯一的时间兜底）

| | 现值 | 新值 | 依据 |
|---|---|---|---|
| Python 沙箱 | 60s 默认 / 1800s 上限 | **120s / 600s** | Claude Code Bash 默认 2 分钟、模型可设上限 10 分钟。输入框留在「机器人设置」卡，是唯一保留用户输入的时长项 |
| LLM 首包 | 60s | 60s | 不变（Claude Code 自身 10 分钟是对自家 API；多厂商网关 60s 够用且更早暴露故障） |
| 流式空闲 | 120s | 120s | 不变（界于 Claude 300s 与「更快发现假死」之间，是现值） |
| LLM 重试 | 2 次尝试 / 1.5s | **3 次尝试 / 1.5s 退避** | Claude Code 默认重试 10 次；3 次是便宜的健壮性 |
| MCP 连接 | 10s | 10s | 不变 |
| 子 agent 停滞 | （无） | **600s** | Claude Code `CLAUDE_ASYNC_AGENT_STALL_TIMEOUT_MS` |

### 4.6 流程常量（值不变，全部变常量）

搜索 6、Verifier 重试 1 / 2、RePlan 2、计划 8 步、自检间隔 8 轮、ask_user 3、文件闸五项、归档闸七项——这些不是预算，值照旧。其中搜索条数与 ask_user 原是可配项（`searchMaxResults` / `askBudget`），随 Agent 卡退役变为常量，配套硬顶 10 / 5 与钳制函数一并删除（web_search 工具参数 `count` 仍需上限，`SEARCH_MAX_RESULTS_CAP = 10` 作为工具入参钳制保留）。技能配额见 §5.4。

## 5. 行为规则

### 5.1 全链路无墙钟

聊天、手动执行、定时、批量四条链路统一：无 `WallBudget`、无截止时间。防失控靠三层——工具级超时（§4.5）+ 安全网（§4.2）+ 用户 `/stop`。`flow.rs` / `exec.rs` 刚加的主会话墙钟构造撤回。

### 5.2 熔断阶梯（主会话与子 agent 同一套，一处判定，两轴：轮数、工具数）

- **计数口径**：只计**实际执行**的工具调用——被 core 或 dispatch 任一处拒绝的调用不计（现 `core.rs:757-777` 拒绝后不回退，违反这条；修）。
- **90%**：软告警一次（轮数 / 工具数各一轴）。
- **95%**：收尾窗——只放行 `WRAP_UP_TOOLS`（写产物 + 读自己的卡），放行的调用**不计数**；其余拒绝且不计数；从进入收尾窗起**最多再跑 3 轮**。
- **100% 或收尾轮数耗尽**：硬停，返回已产正文 + 已落盘产物清单。（**2026-10-29 审计澄清**：收尾轮数用尽是唯一正常出口——写类不计数后 100% 分支正常路径不可达；它作为派生链变化时的硬兜底保留，业界双层惯例——LangGraph 在 RemainingSteps 优雅降级层之下仍保留 GraphRecursionError 硬上限。）
- 删除 `+5` 裕量：写类不计数后它永远触不到，是个没有牙的数字（现 `core.rs:851-853` 已经把它废掉了，只是没删）。
- 子 agent 的 `dispatch.rs` 收尾旗标与 core 的收尾判定合并为同一状态（session 级），不再两道闸各算各的。

（**2026-10-29 审计后补拍**：轮数轴也设收尾窗。原方案只在工具轴做了三段阶梯，轮数轴只有 90% 软告警与 100% 硬停，与本节「两轴」表述不符。补实口径：剩最后 `WRAP_UP_ROUNDS` 轮时进窗，与工具轴共用同一个 `gate_call` 白名单判定，出口就是 max_rounds 自然硬停（带正文）——轮数每轮必 +1，不需要工具轴那套窗口轮数出口；LangGraph `RemainingSteps` 同思路，不用百分比派生。）
- 子 agent 走同一阶梯、同一默认值——`fuse_cap_for` 的「预算 + 裕量」分支随 `SubagentBudget` 一起退役，只留 `cfg_override` 与默认两条。

### 5.3 子 agent

- **无独立预算**：`SubagentBudget` / `clamp_budget` / `apply_onion_turn_cap` / spawn 自报 budget、`subagents` 行的 `budget_json` 列、`db::subagents` 的 `DEFAULT_*` / `MAX_TURNS_HARD_CAP` / `clamp_subagent_max_turns` 全链退役；`budget_exceeded` 结局分类与 digest / repair / prompts 里的预算话术同步拆除。
- 时间：无墙钟；停滞超时 600s——`subagents` 行加 `last_progress_at` 字段，runner 每次拿到 LLM 流 chunk 或工具返回时刷新；超过 600s 无刷新判停（Claude Code 停滞超时口径），留产退出。现「软墙钟 10% 预留窗 / `subagent_wall_soft_stop`」整套退役。
- `check_subagent` 的等待上限（现 600s cap）不变，与停滞超时无关。
- 失控面评估：取消独立预算后，子 agent 最坏烧到共享工具闸 400 次 + 共享轮数 200 + 停滞 600s 为止，且并发闸 3/2 限总量——10-24 那种「100 次调用全读一个文件」仍在工具闸范围内可拦，用户已拍板接受。

### 5.4 技能

- **无独立预算与时长**（Claude Code / Codex 口径）：技能运行计入所在会话的安全网。现技能墙钟（默认 1800s / 上限 3600s）整套退役。
- frontmatter `timeout_secs` **废弃**（解析时忽略，启动日志告警一次；测试阶段不做兼容）。
- frontmatter `max_steps` 只在 **auto（DSL 调度器）模式**用作死循环防护：默认 100，硬顶 200；interactive 模式不设步数闸。
- frontmatter `max_rounds` 只许**往下**钳主会话轮数，不得突破安全网（现在能越过）。
- 连续失败判死（auto 一步 / interactive 三次）不变——这是技能真正需要的「快速失败」闸。

### 5.5 上下文

- 历史裁剪基数 `HISTORY_BUDGET_CHARS = 128_000`（原配置默认 100K）；请求体压缩阶梯基数 `CONTEXT_CHARS_HARD_CAP = 200_000`（原 `request.rs::MSGS_BUDGET_CHARS` 改名挪入 `constants.rs`，值不变）；`guard.rs` 的 `SIDE_SELFCHECK_MIN_CHARS` 继续从硬顶派生（1/5 = 40K），行为不变。
- 工具回填上限：`max_tool_output_bytes` 配置键删除，常量 `TOOL_OUTPUT_BYTES = 128 * 1024`（同时是硬顶，没有第二个数）；`ModePreset.tool_output_bytes` 字段、`GLOBAL_TOOL_OUTPUT_BYTES_CAP` / `MAX_TOOL_OUTPUT_BYTES_CAP` / `clamp_max_tool_output_bytes` 一并删除；`mcp/mount.rs` 同源；`Some(0) = 不截断` 这个第三态随键一起退役（溢出转存已能保全原文）。
- 轮内工具结果滑动窗开关 `inTurnContextEdit` 删除，恒开（10-25 落地时默认即开，且 `lookup_receipt` 召回闭环兜着）。

### 5.6 模型行为开关（原 Agent 卡上的两项，去掉拨杆后的归宿）

- **思考段回灌**（`reasoningEchoEnabled`）：这是**协议正确性**问题不是预算——MiniMax M2/M3、DeepSeek reasoner、Moonshot K2 要求把上一轮 `reasoning_content` 回灌，缺了 400。现状是读配置、默认关。去掉拨杆后规则改为**「模型发了就回」**：上一轮流里出现了 `reasoning_content` 就原样盖回那条 assistant 消息，没出现就没有——不查模型 id、不设表、不加配置键。代码上是删掉 `core.rs` 里「开关是否打开」的判断，`echo_reasoning: Option<&str>` 本来就只在流里真有思考段时才是 `Some`。要求回灌的模型必发思考段，因此不可能再 400；只发不要求的模型（GLM 思考模式、Qwen thinking）会多付思考段的 input token，但盖在历史上的段是稳定前缀、走缓存读，代价小。Anthropic 的 thinking 块另有带签名的协议路径，始终原样回传，与此无关。
- **推理强度默认档**（`reasoningEffort`）：设置页默认档删除，常量 `medium`；聊天输入栏 ⚡ 的按会话覆盖**保留**（那是用户在场时的即时决定，符合「能决定且改完即生效」）。

## 6. 与现状的差异

### 删除

| 类别 | 项 |
|---|---|
| 档位整套 | `TaskMode` / `ModePreset` / `LIGHT·BALANCED·DEEP_PRESET` / `preset_for` / `resolve_preset` / `resolve_global_*` / `task_mode` 配置键与写侧归一、前端三档选择器与 `TaskModeSelect` 测试、参数表 `mode.task_mode` 行 |
| 配置键 | `taskMode`、`maxRounds`、`maxFunctionCalls`、`historyBudgetChars`、`historyTruncateTargetPct`、`subagentMaxTurns`、`subagentMaxToolCalls`、`subagentMaxWallSecs`、`maxToolOutputBytes`、`searchMaxResults`、`askBudget`、`reasoningEchoEnabled`、`inTurnContextEdit`、全局 `maxTokens`、`reasoningEffort` 默认档——`BotConfig` / `BotConfigView` 同步删字段，写侧 `clamp_agent_params` 整个删除（模型条目级 `max_tokens`、`permMode`、`pythonTimeoutSecs` 保留） |
| 设置页与注册表 | 「Agent 运行参数」整张卡（档位选择器 / 三预设 / 全部参数行 / `AGENT_PARAM_ROWS` / `AGENT_PARAM_FALLBACK_LABELS`）；`PARAMS_TABLE` 缩到三行只读（§3），`AgentParams` trait、`cfg_resolver!` 宏、全部 `resolve_*` / `clamp_*`、`ParamDef.editable` 为 true 的条目全部删除；Agent 透明化设计 §3「参数注册表」条目标注退役 |
| 子 agent 预算 | `SubagentBudget` / `clamp_budget` / `apply_onion_turn_cap` / spawn `budget` 字段与工具 schema 参数、`budget_json` 列（open_db 清扫式 DROP，无迁移）、`db::subagents` 的 `DEFAULT_*` / `MAX_TURNS_HARD_CAP` / `MIN_SUBAGENT_TURNS` / `clamp_subagent_max_turns`、`budget_exceeded` 话术、前端 `types.ts` / `subagentActivity.ts` 的预算显示 |
| 常量 / 函数 | `MAIN_SESSION_BUDGET_PCT`、`SUBAGENT_BUDGET_PCT`、`SKILL_BUDGET_PCT`、`budget_share`、`wall_share`、`SUBAGENT_WRAP_UP_FUSE_MARGIN`（两处）、`subagent_wall_reserve_secs`、`ModePreset` 系、`GLOBAL_TOOL_OUTPUT_BYTES_CAP` / `MAX_TOOL_OUTPUT_BYTES_CAP` / `clamp_max_tool_output_bytes`、`HISTORY_BUDGET_MIN` / `HISTORY_BUDGET_MAX` / `clamp_history_budget_chars`、`resolve_subagent_budget`、四对死副本中的重复项 |
| 墙钟整套 | `WallBudget` / `wall_budget`（11 个生产文件的引用全清）、`WALL_FUSE_MARKER` / `wall_fuse_message` / `fuse_wall`、`check_fuses` 墙钟轴、`subagent_wall_soft_stop`、技能 `SKILL_DEFAULT_TIMEOUT_SECS` / `SKILL_MAX_TIMEOUT_SECS_LIMIT` 与 frontmatter `timeout_secs` |
| UI | 「Agent 运行参数」整张卡；「保守 / 标准 / 放开」三预设（权限模式本就在 API 卡有独立控件，不受影响） |

### 保留

`permMode`（API 卡）、`pythonTimeoutSecs`（机器人设置卡，默认 120s / 上限 600s）、模型条目级 `max_tokens`、聊天输入栏 ⚡ 按会话推理强度覆盖、三段阶梯（两轴）、留产机制、溢出转存、并发闸 3 / 2、`check_subagent` 等待上限、`bot_effective_params` 命令（三行只读）。

### 新增

`WRAP_UP_ROUNDS = 3`；`TOOL_OUTPUT_BYTES = 128 * 1024`（单一常量，兼默认与硬顶）；`HISTORY_BUDGET_CHARS = 128_000`；`CONTEXT_CHARS_HARD_CAP = 200_000`（原 `MSGS_BUDGET_CHARS` 改名挪入 `constants.rs`）；`MAX_ROUNDS = 200` / `MAX_FUNCTION_CALLS = 400`（主/子共用）；子 agent 停滞超时 600s（`subagents.last_progress_at` + runner 刷新点）。思考段回灌不新增任何常量或表——只删开关判断。

测试阶段口径：旧配置里的废弃键与旧 `budget_json` 列**直接忽略/DROP**，不写迁移。

## 7. 实施批次与验收

| 批 | 内容 | 验收 |
|---|---|---|
| 1 | `constants.rs` 单源收口：删四对死副本、`MSGS_BUDGET_CHARS` 改名 `CONTEXT_CHARS_HARD_CAP` 挪入、七处过期注释、重复 `#[cfg(test)]` | 行为零变化；clippy 绿；新增「常量只在一处定义」的 grep 守卫测试 |
| 2 | 阶梯语义统一：拒绝不计数、写类收尾不计数、`WRAP_UP_ROUNDS`、删 `+5`、dispatch / core 收尾状态合一 | 新增叠加场景测试：收尾窗内连发 10 次读 + 5 次写，断言计数不动、第 4 轮硬停、正文保留 |
| 3 | 预算模型：删档位与比例派生整套；安全网常量 200 / 400（主/子共用，无配置键）；**撤墙钟整套** | `grep -rn "TaskMode\|ModePreset\|WallBudget" src-tauri/src` 零命中；熔断只剩轮数/工具数两轴 |
| 4 | 子 agent：`SubagentBudget` 全链退役（db 列 / spawn / schema / prompts / digest / repair / 前端显示）；**停滞超时 600s**（`last_progress_at` + 刷新点 + 判停留产） | spawn 请求无 budget 字段；停滞测试：600s 无刷新判停、产物保留、流式长回复中途不误杀 |
| 5 | 技能：`timeout_secs` 废弃；`max_steps` 仅 auto 防护；`max_rounds` 只许下钳；技能墙钟退役 | 解析测试 + 主循环轮数不被 frontmatter 抬高的守卫测试 |
| 6 | 上下文：历史预算常量 128K 接线（删配置键与钳制）；上下文硬顶 200K 保持；payload 固定 128KB（删预设字段、硬顶常量、钳制函数、配置键、参数表行）；MCP 同源；删第三态 | 历史 >128K 触发裁剪 + 摘要并裁至 ≈90K 的单测；请求体 140K / 170K / 200K 三级阈值守卫测试；MCP 与模型循环截断值相等的守卫测试；`grep -rn "tool_output_bytes\|TOOL_OUTPUT_BYTES_CAP" src-tauri/src` 只剩 `TOOL_OUTPUT_BYTES` 一处定义 |
| 7 | 设置页与注册表收尾：删「Agent 运行参数」整张卡与三预设；15 个配置键从 `BotConfig` / `BotConfigView` / 前端 `SettingsConfig` 删除；`PARAMS_TABLE` 缩到三行只读、删 `AgentParams` 与 `resolve_*` / `clamp_*`；思考段回灌删开关判断改「发了就回」；SPEC 的 2026-10-19 / 10-20 / 10-28 三条「参数卡同源」条目改写为「Agent 参数全为常量」、Agent 透明化设计 §3 标注退役；DEVLOG / 架构图 / AGENTS 地图同步 | 前端测试更新；`audit_doc_claims` 绿；`bot_effective_params` 返回恰好三行；回灌两态单测：上一轮流含 `reasoning_content` → 下一轮请求体带回，不含 → 不带；GLM / Qwen 实机各跑一次多轮工具链确认接受历史里的 `reasoning_content` |

批 1–2 先行（修 P1-A，零用户面变化），批 3–6 一个批次一个提交，批 7 收尾。全程不做数据迁移（`budget_json` 列走 open_db 清扫式 DROP，与 workflows 表退役同款）。

## 8. 不做清单

- **任何会话级墙钟**：包括未来若出现无人值守场景——到那时再按 Copilot `timeout-minutes` 的形态加回（一条链路一个截止时间），现在不加占位抽象。
- **任何档位/预设**：三档、两档都不做；`TaskMode` 概念不保留占位。
- **子 agent 独立预算**：不做；子 agent 与主会话共用安全网。
- **Token / 费用预算**（Claude Code `--max-budget-usd` 的等价物）：`exec_traces` 已按轮记账，技术上可做 Σ(非缓存输入 + 输出) 上限；但要先用 usage 统计的 p90 校准初值，列为二期。
- **子 agent resume 续跑**：二期。
- **子 agent 嵌套**：不开放。
- **共享递减轮数池**：不做（见 §1）。
- **按模型窗口百分比压缩**（Gemini 50% 口径）：需要 tokenizer，字符口径够用，不做。

## 9. 风险

- **定时/批量长任务没有时间兜底**：调度器自身无超时，墙钟撤掉后一个「每轮都有进展但永远做不完」的循环只能被 200 轮安全网拦住（按每轮一次模型往返估，十几分钟到半小时量级）；若嫌不够，二期用 token/费用预算补（§8）。业界交互式产品同此口径，可接受。
- **子 agent 取消独立预算后失控面变大**：最坏情况烧到共享工具闸 400 + 轮数 200 + 停滞 600s；10-24 那类「大量调用全读一个文件」仍可拦，但「大量调用各读不同文件」会烧满 400 次才停。用户已拍板接受；若日后反悔，恢复路径是给子 agent 加回一对固定小闸（不走配置）。
- **子 agent 停滞超时是新机制**：`last_progress_at` 的刷新点要盖全（LLM 流任意 chunk、工具返回），漏一个刷新点就会误杀健康子 agent——批 4 的测试要覆盖「流式长回复中途不误杀」。
- **思考段「发了就回」对只发不要求的模型多付 input token**：GLM 思考模式、Qwen thinking 会发 `reasoning_content` 但不要求回灌，现在也会被带回。代价走缓存前缀（历史上的段不再变动），预计小；若 usage 统计显示明显，再加一张「已知不需要」的厂商表做剔除——那张表漏一条的后果只是多花钱，不是 400，方向是安全的。另需实机确认这些厂商接受历史消息里带该字段（批 7 验收）。
- **默认值整体放宽，需实机过一遍 10-24 那份子 agent 场景卡**：主会话轮数 60 → 200、工具 150 → 400；子 agent 轮数 20 → 200、工具 50 → 400（与主会话共用）；历史预算 100K → 128K（多留约 28% 历史，input 成本略升，走缓存前缀）。payload 对均衡档（默认）零变化；只有手选过轻量（64KB）/ 深度（200KB）档的配置会统一回到 128KB。
- 删除一批配置键与 `budget_json` 列会让老配置/旧行静默失效——测试阶段口径允许，但 DEVLOG 要写明。

## 10. 拍板记录（2026-10-28 ～ 10-29，全部已定）

1. **墙钟**：全链路取消，无无人值守场景。
2. **工具回填 payload**：固定 128KB 即硬顶，不分档，设置页无此项。
3. **子 agent 预算**：不设独立预算闸——与主会话共用安全网，LLM 自报 budget 取消，停滞超时 600s 成为子 agent 唯一专属闸。
4. **档位**：取消。max_tokens 16384 固定为常量，设置页不露档位选择器。
5. **安全网默认值**：主会话与子 agent 轮数 **200**、工具调用 **400**。
6. **设置页取消 Agent 参数**：「Agent 运行参数」整张卡删除，两项高级覆盖键也不保留，全部 Agent 运行参数变为常量；权限模式（API 卡）与 Python 沙箱开关/超时（机器人设置卡）不受影响。
7. **思考段回灌由模型决定**：不设开关、不查模型表——上一轮发了 `reasoning_content` 就原样带回，没发就没有。
8. **会话历史字符预算**：常量 **128K**（`128_000`），上下文硬顶 **200K**（`200_000`，单次请求 msgs 总字符绝对上限，压缩阶梯基数）；两者都是常量，无配置键。
9. **（2026-10-29 补拍）轮数轴收尾窗**：剩最后 3 轮进窗（`max_rounds − WRAP_UP_ROUNDS`），共用 gate_call 白名单，出口 max_rounds 硬停带正文；同日补落地 §4.5 的 Python 120s/600s（apps 域拆独立 1800s 天花板）与 LLM 重试 3 次尝试（OpenAI/Anthropic SDK 默认同值）。
