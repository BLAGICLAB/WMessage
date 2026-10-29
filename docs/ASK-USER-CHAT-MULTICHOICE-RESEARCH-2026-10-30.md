# ask_user 聊天放开 + 选择题提问 —— 业界调研与方案（待拍板）

> 2026-10-30。用户诉求原文：「Ask user 工具普通聊天 agent 也可以用，以选择题的形式提问」。
> 本文只做调研与方案对比，**未动任何代码**；拍板后按 §九 实施。

## 一、结论先行

1. 「结构化选择题提问」已经是编码 agent 的事实标配：Claude Code（`AskUserQuestion`）、
   OpenAI Codex（`request_user_input`）、Cursor（`AskQuestion`）、Cline/Roo
   （`ask_followup_question`）、AWS Kiro（`userInput`）全都有；没做的
   （Copilot、Gemini CLI、Warp）被用户在 issue 里追着要。
2. 业界形状高度趋同：**单次 1–4 问 × 2–4 选项 + 「其他」自由输入兜底**，选项
   label 配一句话说明，答案作为 tool result 回灌。
3. 我们的 `ask_user` **已经具备选择题骨架**：schema 里就有 `options`（≤4×40 字），
   通知页问题卡已经把选项渲染成按钮 + 自由输入。真正的差距只有两个：
   **聊天场景被硬闸挡住**（schema 描述写死「聊天中不要用」+ 会话未注册被引擎拒绝）、
   **问题卡长在通知页而不是聊天流里**。
4. 推荐方案 B（§六）：聊天解禁 + 新增 `bot-ask` 事件 + ChatPanel 消息内嵌问题卡
   （选项点选 + 其他输入 + 「按假设继续」），通知中心照旧双写兜底。
   多问 / 多选 / 选项描述（Claude Code 完整口径）列为二期。
5. 子 agent 维持禁用——与 Claude Code 官方口径一致（其子 agent 明确不可用该工具）。

## 二、大厂口径

### 2.1 Claude Code — `AskUserQuestion`（本赛道的定义者）

2025-10 v2.0.21 引入，社区称「hidden gem」。双源核对：官方 Agent SDK 文档 + 本机工具定义。

| 维度 | 口径 |
|---|---|
| 数量 | 每次调用 1–4 问，每问 2–4 个选项 |
| 选项结构 | `{label, description}`；另有 `header`（≤12 字符的小标签）；SDK 侧可选 `preview`（markdown/html 分栏预览，HTML 过滤 script/style/DOCTYPE） |
| 多选 | `multiSelect` 布尔，默认单选 |
| 自由输入 | UI 自动在选项后追加「Other」文本框（SDK 指南明确要求宿主自实现此项），自定义文本作为答案值 |
| 回灌 | `answers` 对象（key=问题文本，value=所选 label；多选传数组或逗号串）；UI 直接关闭问题卡时走 `response` 自由文本，模型收到「The user responded: …」 |
| 超时 | **无**。SDK 回调可以无限期 pending；文档建议怕进程退出的场景改用 hook + 持久化恢复 |
| 默认答案 | 无（官方文档无此机制） |
| 子 agent | **不可用**——官方原文 "AskUserQuestion is not currently available in subagents spawned via the Agent tool" |
| 使用纪律（系统提示词） | 只在「无法从请求/代码/常理默认值解决、真正属于用户的决策」时问；推荐项置顶且 label 加 "(Recommended)"；不问代码里能查证的事实 |

来源：[Agent SDK 用户输入文档](https://code.claude.com/docs/en/agent-sdk/user-input)、
[claudelog FAQ](https://www.claudelog.com/faqs/what-is-ask-user-question-tool-in-claude-code)、
[v2.0.21 发布帖](https://www.reddit.com/r/ClaudeCode/comments/1oesyrc/)、
[spec-kit 对其 schema 的引用](https://github.com/github/spec-kit/issues/2181)。

### 2.2 OpenAI Codex — `request_user_input`（同构 twins）

- 社区共识「与 AskUserQuestion 几乎完全同构」：1–3 问，每问可选项 + 自由文本
  "other"（[issue #9926](https://github.com/openai/codex/issues/9926) 原话：
  "they are literally identical and can be used at any time, just like in Claude"）。
- **主要痛点是可用面被锁在 plan mode**：#11536、#11565 连续要求放开到普通对话模式。
  ——这正是「聊天 agent 也可以用」这个方向最有力的业界佐证：锁死模式是 Codex 用户
  最大的不满，而不是工具本身。
- 来源：[codex#9926](https://github.com/openai/codex/issues/9926)、
  [codex#11536](https://github.com/openai/codex/issues/11536)。

### 2.3 Cursor — `AskQuestion`

- Agent 内置多选题机制，问题以选项按钮呈现（[论坛功能请求 → 已实现](https://forum.cursor.com/t/enhancing-ais-ability-to-seek-clarification/15894)）。
- 有超时，且超时行为出过事：论坛报告「AskQuestion 超时后后续 addition request 被消费」
  （[帖子](https://forum.cursor.com/t/addition-request-consumed-on-askquestion-timeout/152799)）。
  **教训：做超时就必须把超时后的副作用定义干净**（吞请求是 Cursor 踩过的坑）。

### 2.4 Cline / Roo Code — `ask_followup_question`（聊天 agent 全程可用的先例）

- 参数：`question`（必填）+ `follow_up`（可选，2–4 个建议答案）。**每次只问一个问题**。
- UI：建议答案渲染为可点按钮，同时保留自由输入（"selecting or typing a response"）。
- **不强制结构化**："Cannot force structured responses – users can still respond freely"、
  无答案校验（Roo 官方文档原文）。
- 可用面：**所有模式全程可用**（"always available"，不限 plan/执行）。
- 时机纪律：关键信息缺失、多个有效方案二选一、技术细节/偏好必需时才问；
  官方警告 "Excessive use can slow down task completion and create a fragmented experience"。
- 多选是社区未满足需求（[cline#3808](https://github.com/cline/cline/discussions/3808)）。
- 来源：[Cline 工具参考](https://docs.cline.bot/tools-reference/all-cline-tools)、
  [Roo 文档](https://roocodeinc.github.io/Roo-Code/advanced-usage/available-tools/ask-followup-question)。

### 2.5 AWS Kiro — `userInput`（系统提示强制的提问工具）

- Spec 工作流中，系统提示要求 agent 在阶段评审点**必须**用 `userInput` 工具提问，
  UI 呈现为选项按钮；需求→设计→任务逐阶段都要用户确认后才前进。
- 特点：不是「模型想问就问」，而是**流程强制的结构化确认点**——与我们任务卡
  「逐步执行确认」的 park/resume 机制同族，但用工具+按钮实现了它。
- 来源：[Kiro Specs 文档](https://kiro.dev/docs/specs)、
  [Kiro 博客（前置澄清问题）](https://kiro.dev/blog/faster-smarter-specs)、
  [系统提示词流出帖](https://www.reddit.com/r/kiroIDE/comments/1mucxak/)。

### 2.6 MCP Elicitation（协议层的「表单化提问」，重武器）

2025-06-18 引入 form mode，2025-11-25 增 URL mode。是「工具/服务器向用户要输入」的
协议级标准，但定位比聊天选择题重得多：

- form mode：`requestedSchema` 只允许**扁平对象 + 原始类型**（string 含
  email/uri/date/date-time 格式与长度约束、number/integer、boolean、
  enum 单选（`enum` 或 `oneOf[{const,title}]`）、enum 多选（array+items）），显式
  禁嵌套结构与对象数组——为了客户端表单体验做的刻意裁剪。
- 响应三态：**accept**（带 content）/ **decline**（明确拒绝）/ **cancel**（未决策关闭）。
- 安全红线：表单模式禁止索取密码/API key/token/支付凭据，敏感场景必须走 URL mode
  （带外交互）；客户端必须显示请求方、允许拒答、允许修改后再提交。
- 实现面：VS Code 已落地（[实装记](https://den.dev/blog/vscode-mcp-elicitations-stop-guessing)）；
  Gemini CLI 尚未实现（[feature request #7056](https://github.com/google-gemini/gemini-cli/issues/7056)）。
- 规范原文：[Elicitation — MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation)。

**定位判断**：这套是给 MCP 外部服务器用的表单引擎。我们聊天链路的选择题不需要上
这套；将来若 MCP 外部工具需要 elicitation，再另立方案对接。

### 2.7 没做这件事的（反面对照）

| 产品 | 现状 | 用户声音 |
|---|---|---|
| GitHub Copilot agent 模式 | 无结构化提问工具，Plan agent 只用纯文本问；官方文档确认其「提问澄清」为文本形态 | [community#169482](https://github.com/orgs/community/discussions/169482)（高热度请求：强制/允许 Copilot 主动问澄清问题） |
| Gemini CLI | 无内置 ask 工具；shell 确认弹窗是权限语义不是问答语义 | elicitation 工作流 [feature request #7056](https://github.com/google-gemini/gemini-cli/issues/7056) |
| Warp | 无（agent 澄清靠纯文本） | [warp#8519](https://github.com/warpdotdev/warp/issues/8519) 点名要对齐 Claude Code `AskUserQuestion` 与 Cursor Plan 模式的可点选 UI |

## 三、共识提炼（六条）

1. **选择题提问是标配新原语**：2025-10 Claude Code 起步，一年内 Codex / Cursor /
   Cline 系 / Kiro 跟进；缺席者（Copilot、Gemini CLI、Warp）被用户追着要。
2. **形状趋同**：单次 1–4 问 × 2–4 选项；选项 = 短 label +（可选）一句话说明；
   **必配「其他」自由输入**——没有任何一家强制结构化。Cline 系证明「单问 2–4 选项」
   就是可用下限。
3. **回灌与阻塞**：答案作为 tool result 回灌是唯一主流口径。超时要么不给
   （Claude Code SDK 无限等），要么把超时副作用定义干净（Cursor 吞请求的教训）。
4. **节制靠提示词，不靠硬闸**：Claude Code 系统提示「只在真正卡住时问」、Cline
   官方警告过度使用。业界普遍**没有**预算闸和默认答案；我们的 assumption（假设）兜底
   + 提问预算反而是超出业界的保守设计，是独有优势不是包袱。
5. **子 agent 一律不可用**：Claude Code 官方明说子 agent 拿不到该工具。
6. **更重的结构化输入归 MCP elicitation**：schema 校验、三态应答、URL 带外——协议层
   的事，不进聊天主链路。

## 四、我们的现状（调研发现：比预想近得多）

`ask_user` 已存在，且选择题骨架齐全：

- **schema 已有 `options`**：≤4 个、每个 ≤40 字，「用户可点选也可自由回答」
  （`src-tauri/src/bot/registry/schemas.rs:267`）。
- 解析与截断完备：question ≤200 / why ≤100 / options ≤4×40 / assumption ≤200
  （`src-tauri/src/ask_engine.rs:206-254`）。
- 阻塞引擎完备：oneshot waiter + 24h 超时 + /stop 每 500ms 轮询打断
  （`ask_engine.rs:280-448`）。
- **选择题 UI 已存在，只是长在通知页**：通知页问题卡已把 payload 里的 options
  渲染成按钮，输入框占位文案「或输入其他回答」
  （`src/components/NotificationsPage/NotificationsPage.tsx:250-302`）。
- 回灌走标准漏斗：`ToolResult::ok("用户回答了你的提问：{answer}")` → 8192 字节
  per-tool 预算 → `role:"tool"` 进下一轮，无专用旁路。
- 应答三动作：answer（富回答）/ assume（按假设继续）/ dismiss（忽略，等同按假设）
  （`ask_engine.rs:30-82`）。

但有两道闸把聊天场景挡死：

1. **schema 描述写死**：「仅任务执行中可用：工作流节点与手动执行的任务卡；
   聊天中不要用本工具，直接文字提问即可」。
2. **引擎硬闸**：聊天会话没有注册 `AskRegistration`（注册只发生在手动执行任务卡的
   执行会话上），engine 直接 warn 拒绝：「ask_user 仅任务执行中可用……请按你的假设
   继续执行」（`ask_engine.rs:292-297, 306-311`）。

另外：子 agent 的三份 profile 白名单都不含 ask_user，天然禁用（与业界口径一致）；
提问预算为全局设置 1–5 默认 3，工作流与任务卡共用。

## 五、差距清单

| # | 差距 | 说明 |
|---|---|---|
| G1 | 聊天解禁 | 双处解锁：schema 描述改写 + 聊天会话注册轻量提问上下文 |
| G2 | 选择题一等化 | 问题呈现进聊天流（新增事件 + ChatPanel 内嵌卡），不再只靠通知中心；现状是「AI 在聊天里问了一句，卡片却在通知页」的割裂体验 |
| G3 | （可选）选项语义增强 | label+description、multiSelect、单次多问——业界已有，我们可后置 |
| G4 | 聊天模式语义适配 | assumption/24h 超时/预算三个任务模式的兜底，在同步聊天里是否原样保留需拍板 |

## 六、方案选项与推荐

### 方案 A —— 最小改动（只解锁）

聊天会话注册轻量 `AskRegistration`（无 task_id）+ schema 描述解禁；问题照旧进通知
中心应答。约一天工作量。
缺点：体验割裂——聊天中 AI 提问，用户要去通知页点选项；业界主流（全部五家）都是
聊天流内呈现。

### 方案 B —— 聊天内嵌问题卡（推荐）

方案 A 之上再加呈现层：

1. 后端 emit 新事件（如 `bot-ask`）经 `emit_recorded` 统一出口盖 sessionId（遵守
   BOT-EVENT-ENVELOPE-2026-10-27 的盖章纪律）；payload 直接复用现有通知 payload
   结构（question/why/options/assumption/questionId），零新增字段。
2. ChatPanel 新增内嵌问题卡组件：问题 + why + 选项按钮（点选即 answer）+ 自由输入
   （「其他」，Roo/Cline 口径）+ 「按假设继续」按钮（把 assume 显式交给用户）；
   组件可直接参考 NotificationsPage 现有选项按钮实现。
3. 通知中心照旧双写（跨窗口/后答兜底，现状机制零改动）；晚到回答仍走 ask_respond
   → waiter 唤醒，同一把锁纪律不变。
4. 回放：重开会话时问题卡显示终态（已答/已按假设/已忽略）——通知已有状态可查。
5. 聊天模式超时/预算/assumption 语义按 §七 拍板项执行（推荐全部沿用现状，机制
   零改动）。

工作量：后端事件 + 前端一卡 + 注册/描述两处解锁 + 测试，中等规模一个批次。

### 方案 C —— Claude Code 完整口径（二期）

方案 B 之上加：单次 1–4 问、`multiSelect`、每选项 `description`、推荐项置顶标注。
不建议一期做：我们模型栈以小模型为主，schema 越复杂工具调用越易碎（业界可用下限
已被 Cline 系证明是「单问 2–4 选项」）；且多选/多问会连带前端卡片与回放形态复杂化。
等一期实机跑出真实需求再升级。

### 推荐：B，理由三条

1. 业界共识起步形状就是「单问 2–4 选项 + 其他」，B 正好落在这个形状上。
2. 我们已有的骨架（options 参数 + 通知页按钮 + oneshot 回灌）让 B 的增量最小。
3. assumption 兜底是业界少有的安全网——保留它，聊天放开闸就没有「AI 干等/死循环
   追问」的风险。

## 七、待拍板清单

| # | 拍板项 | 选项 | 推荐 |
|---|---|---|---|
| P1 | 聊天模式超时与假设 | ① 沿用 24h+按假设继续（引擎零改动）② 缩短（如 10 分钟）③ 无超时一直等 | ①——用户离席时 AI 不停摆，回来仍可从通知页应答；Cursor 的教训说明长等待必须配明确的回落语义，我们已有 |
| P2 | 聊天提问预算 | ① 沿用全局 1–5 默认 3 ② 聊天单独预算 ③ 不设闸靠提示词 | ①——一致性；业界虽无预算闸，但多一层「按假设继续」的软着陆无害 |
| P3 | UI 形态 | ① 消息内嵌卡（业界同款）② 复用 ConfirmMap 弹窗 | ①——弹窗打断心流，且 ConfirmMap 是「批准」语义不是「问答」语义 |
| P4 | 通知中心是否双写 | ① 双写（现状机制）② 只在聊天内 | ①——跨窗口/晚到兜底，成本为零 |
| P5 | description/multiSelect/多问 | ① 二期 ② 一期就上 | ①——见方案 C 论证 |
| P6 | 子 agent 是否放开 | ① 维持禁用 ② 加进某 profile 白名单 | ①——与 Claude Code 官方口径一致；子 agent 阻塞点应写进收尾 JSON 的 blockers 由主 agent 转问 |
| P7 | 「按假设继续」是否显式成按钮 | ① 显式按钮（用户可主动选）② 隐藏，只做超时兜底 | ①——把 assume 从「系统暗箱」变成用户可见的第三个选项，与 answer/dismiss 同权 |

## 八、不做清单

- **MCP elicitation 全量表单引擎**（schema 校验 / URL mode / 三态应答）：协议层重武器，
  聊天选择题用不上；将来 MCP 外部服务器要 elicitation 时另立方案。
- **preview 分栏预览**（选项附代码/ASCII 渲染）：编码 agent 场景，聊天产品用不上。
- **强制结构化**（拒绝自由输入）：五家没有一家这么做，Roo 文档明确反对。
- **HTML 富选项**：注入面大收益小。

## 九、实施备注（拍板后用）

- `SCHEMA_ASK_USER` 在 `TOOLS_TABLE` 基线前缀（前 28 项）之外，改描述/加参数不动
  `tools_baseline.json` fixture；`BUILTIN_TOOL_COUNT` 不变（不增减条目）。
- 新事件走 `emit_recorded`（`src-tauri/src/bot/stream_log.rs:229-247`）统一盖
  sessionId；是否进回放缓冲随 P4 一并定。
- 前端监听加在 `src/components/ChatPanel/useChatStream.ts`；问题卡组件放 ChatPanel/。
- 注册解禁：普通聊天会话建立时注册轻量 `AskRegistration`（task_id 置空或加 origin
  字段区分），与 unregister 成对；`register_ask_context`（`ask_engine.rs:168`）
  现有签名可复用。
- 回灌 per-tool 预算 8192 字节对答案文本绰绰有余，无需调整。

## 十、参考链接

- Claude Code：[Agent SDK 用户输入](https://code.claude.com/docs/en/agent-sdk/user-input) ·
  [claudelog FAQ](https://www.claudelog.com/faqs/what-is-ask-user-question-tool-in-claude-code) ·
  [v2.0.21 发布帖](https://www.reddit.com/r/ClaudeCode/comments/1oesyrc/) ·
  [spec-kit#2181](https://github.com/github/spec-kit/issues/2181)
- Codex：[codex#9926](https://github.com/openai/codex/issues/9926) ·
  [codex#11536](https://github.com/openai/codex/issues/11536)
- Cursor：[澄清能力请求](https://forum.cursor.com/t/enhancing-ais-ability-to-seek-clarification/15894) ·
  [超时吞请求报告](https://forum.cursor.com/t/addition-request-consumed-on-askquestion-timeout/152799)
- Cline/Roo：[Cline 工具参考](https://docs.cline.bot/tools-reference/all-cline-tools) ·
  [Roo ask_followup_question](https://roocodeinc.github.io/Roo-Code/advanced-usage/available-tools/ask-followup-question) ·
  [cline#3808 多选请求](https://github.com/cline/cline/discussions/3808)
- Kiro：[Specs 文档](https://kiro.dev/docs/specs) ·
  [faster-smarter-specs](https://kiro.dev/blog/faster-smarter-specs) ·
  [系统提示词流出](https://www.reddit.com/r/kiroIDE/comments/1mucxak/)
- MCP：[Elicitation 规范 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/client/elicitation) ·
  [VS Code 实装记](https://den.dev/blog/vscode-mcp-elicitations-stop-guessing) ·
  [gemini-cli#7056](https://github.com/google-gemini/gemini-cli/issues/7056)
- 缺席者：[copilot community#169482](https://github.com/orgs/community/discussions/169482) ·
  [warp#8519](https://github.com/warpdotdev/warp/issues/8519)
