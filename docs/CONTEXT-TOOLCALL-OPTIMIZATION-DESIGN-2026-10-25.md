# 上下文成本与工具调用优化方案（2026-10-25）

目标：在**不降低工具调用与产出质量**的前提下，显著降低 token 成本。
两条正交的省钱线 + 三条提质线，全部有业界实证数据背书（来源见 §三）。

- 省钱线 A（缓存）：让历史消息进入可缓存前缀 —— 针对跨轮重发
- 省钱线 B（轮次）：减少推理轮数与每轮思考输出 —— 针对轮内累积
- 提质线：旧结果合法召回（防编造/免重跑）、参数示例（减少重试）、思考段回灌（官方口径）

状态：**待拍板，未实施**。本文只写方案，不改代码。

---

## 一、token 到底花在哪（现状账本）

| 成本项 | 现状 | 计价 | 结论 |
|---|---|---|---|
| 静态 system | 槽位组装（prompt_builder.rs），静态段在前、动态推尾（`VerifierRetryTail`，prompt_builder.rs:40-44） | 一次全价，后续缓存命中 | 无油水，已优化到位 |
| 跨轮历史重发 | 历史是纯 user/assistant 文本对（ChatMsg 只有 user/assistant），工具消息不跨轮持久 | **每轮全价** | **最大浪费点**，见 P1 |
| 轮内工具消息累积 | append-only，逐迭代全量重发；溢出漏斗挡超大结果 | 重发部分缓存折扣价，新消息全价 | 次级浪费点，见 P7 |
| 每轮思考输出 | M2 每轮生成 reasoning（output token，全价，约为 input 价的数倍） | 按轮数线性 | 见 P4 |
| 工具重跑 | 旧轮工具结果不在上下文，模型需要时只能重新调工具（重读 500 行文件、重跑 OCR） | 一次完整工具往返 + output | 见 P3 |

**关键事实（P1 的依据）**：记忆检索块（按本轮用户消息做语义检索，几乎每轮内容都变）和
`[早前对话摘要]` 现在插在主 system 之后、全部历史之前（bot_chat.rs:1128-1143）。
MiniMax 隐式前缀缓存按「tool 列表 → system → user 消息」顺序做最长前缀匹配（官方文档口径），
这两块一变，**后面整段历史的缓存前缀全部作废**——长会话历史每轮都在付全价。

---

## 二、现状盘点（已有机制，避免重复建设）

| 已有 | 位置 | 与业界对应 |
|---|---|---|
| 缓存前缀稳定组装（静态在前/动态推尾） | prompt_builder.rs | MiniMax 官方建议的组装顺序 |
| 历史字符预算 + 截断即摘要落库 | bot_chat.rs:239-258（`historyBudgetChars` 钳 20K..=500K，默认 100K） | 上下文压缩（compaction） |
| 工具大结果溢出转存 + 指针回灌 + 30 天清扫 | overflow.rs | PTC「中间结果不进上下文」的穷人版 |
| cache_read/cache_creation 全链路采集 + 按天按模型统计 | bot_model_loop.rs:138-330、db/trace.rs usage_* | 缓存效果观测（本文验收的仪表盘） |
| 并行工具调用引擎（循环遍历 tool_calls 逐个执行） | bot_model_loop.rs:3062 | OpenAI/Anthropic 原生并行 |
| 结构化对账 + 终稿自检（独立模型、500 字 snippet、8 轮节流） | bot_model_loop.rs、prompts/selfcheck.rs | 幻觉校验（SemIf 思想的等价物） |
| exec_traces 全量落库（span args/result 钳制存储）+ UI 痕迹页 | db/trace.rs | 「receipt 永真源在 DB」 |
| 技能渐进披露（目录层先行，正文按需注入） | prompt_builder.rs SkillCatalog/SkillBody | Tool Search 的低配 |
| 子 agent 隔离上下文 | spawn_subagent | Claude Code context isolation |

**明确不缺的**：图片只在最后 3 条消息内保留（bot_chat.rs:279-284）；M2 思考段不重复生成
（只是是否回灌的问题，见 P6）；时间不走 system 注入（走 `get_current_time` 工具）。

---

## 三、业界口径（2026-10 调研摘要）

| 厂商/产品 | 机制 | 实测数据 |
|---|---|---|
| Anthropic | Programmatic Tool Calling：模型写 Python 在沙箱编排 20+ 次工具调用，中间结果不回上下文 | token 43,588→27,297（**-37%**）；GIA 基准 46.5%→51.2% |
| Anthropic | Tool Search Tool：工具定义按需加载（defer_loading） | 工具定义场景上下文 -85%；准确率 49%→74%（门槛：工具定义 >10K token、10+ 工具） |
| Anthropic | Context Editing：逼近阈值自动把旧工具结果替换占位符，保留最近 N 次 | 单用 **+29%**、配合 memory tool **+39%**（agentic 基准） |
| Anthropic | Tool Use Examples：工具定义内嵌真实调用示例 | 复杂参数准确率 72%→90% |
| OpenAI | strict mode（约束解码保参数合法）；原生并行调用，教程第一条：必须遍历 tool_calls 数组 | — |
| Google | 并行调用 + thought signature 必须回传（丢了直接报错） | — |
| MiniMax | 隐式缓存：前缀顺序 tool→system→user；命中价约输入价 1 折（写入可能上浮，以价目为准）；输入 ≥512 token 起 | 多轮对话是官方首推受益场景 |
| MiniMax | **M2/M3 系列要求把 `reasoning_content` 连同 tool_calls 回灌历史**，原话「otherwise subsequent conversation will lose context」「essential for achieving the model's best performance」 | 官方口径，与省 token 相悖但有缓存兜底 |

来源：[Anthropic advanced tool use](https://www.anthropic.com/research/advanced-tool-use)、
[Anthropic context editing](https://platform.claude.com/docs/en/build-with-claude/context-editing)、
[hermes-agent 记录的 29%/39%](https://github.com/NousResearch/hermes-agent/issues/526)、
[OpenAI function calling](https://developers.openai.com/api/docs/guides/function-calling)、
[Gemini thought signatures](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)、
[MiniMax prompt caching](https://platform.minimax.io/docs/api-reference/text-prompt-caching)、
[MiniMax interleaved thinking](https://platform.minimax.io/docs/guides/text-m3-function-call)、
[MiniMax-M2 保留思考段讨论](https://huggingface.co/MiniMaxAI/MiniMax-M2/discussions/38)。

---

## 四、改进项总览

| 编号 | 名称 | 线 | 收益 | 质量风险 | 改动量 | 批次 |
|---|---|---|---|---|---|---|
| P1 | 记忆块/摘要挪到消息末尾 | 省钱 A | 大（长会话 input 从全价变 1 折） | 近零 | 小 | 批 1 |
| P2 | 截断滞回 | 省钱 A | 中（保住 P1 的稳定窗口） | 近零 | 极小 | 批 1 |
| P3 | `lookup_receipt` 工具 | 提质 | 中（防编造 + 免重跑） | 无 | 小 | 批 2 |
| P4 | 并行调用提示词引导 + 热点统计 | 省钱 B | 中（每并一轮省一轮思考 output） | 低 | 小 | 批 2 |
| P5 | 高频工具 schema 加真实示例 | 提质 | 小→中（减少参数重试轮） | 无 | 极小 | 批 3 |
| P6 | M2 思考段回灌开关 | 提质 | 官方口径的 agent 性能 | token 上升（缓存兜底） | 小 | 批 4（先实测） |
| P7 | 轮内工具结果滑动窗 | 省钱 B | 大（重工具轮；Anthropic 实证 +29%） | 中（召回闭环依赖 P3） | 中 | 批 5（二期，看数据） |
| P8 | 工具自检挪出主循环 | 省钱 B | 小（每 5 轮省一次全价重发） | 低 | 小 | 可选 |

依赖关系：P7 依赖 P3（桩子召回闭环）；其余互相独立。

---

## 五、逐项设计

### P1 记忆块/摘要挪到消息末尾（解封跨轮缓存）

**问题**：每轮都变的记忆块与摘要卡在历史之前，长会话历史永远缓存不命中（§一）。

**方案**：装配顺序从
`[system][摘要?][记忆?][历史...][本轮 user]`
改为
`[system][历史...][摘要?][记忆?][本轮 user]`
（摘要与记忆块紧跟在本轮 user 消息之前或并入其中，二选一，以实测缓存命中定）。

语义上更对：记忆块按「本轮用户消息」检索、摘要服务「本轮对早前的记忆」——都是本轮视野的
前置材料，放末尾（recency 注意力最强位置）不损失也理顺。

**改动点**：bot_chat.rs 装配段（1120-1150 附近）两处 push 挪位；注入仍是请求装配时拼接，
**不落库、不进 ChatMsg、不污染历史**（与现状同约定）。

**实现形态二选一**（批 1 内实测后定，默认 a）：
- a. 末尾独立 system 消息：改动最小；现状已证明该端点接受多条 system 消息；
- b. 并入本轮 user 消息 content：前缀稳定性最通用（任何厂商口径都安全），
  需注意只在装配时拼接、历史持久化不含注入块。

**预期收益**：50 轮会话、每轮历史约 2 万 token：现状约 100 万 token 全价 input；
改后 ≈ 一次全价 + 其余 1 折 ≈ 12 万。**input 费用降约 85%**（会话越长越接近此上限；
间隔超过缓存 TTL 退回全价属厂商行为，非本方案可控）。

**护栏/断言**：新增「前缀稳定性」单测——同一会话连续两轮的请求消息数组，
除末尾三段（摘要/记忆/本轮 user）外，前 N 条消息必须 byte 级一致。

**验收**：单测（前缀稳定 + 注入位置）；实机固定会话改前后对比
`usage_stats_daily` 的 cache_read/input 占比（目标：长会话 cache_read 占 input ≥60%）。

### P2 截断滞回（hysteresis）

**问题**：现贴边裁——超预算丢最老的，可能每轮裁 1-2 条 → 每轮一次缓存全断，
P1 省下的窗口被裁剪本身打碎。

**方案**：超预算时**一次裁到预算的 70%**（目标比例可配），留 30% 余量；
两次裁剪之间前缀稳定多轮。最后一条（本轮 user）仍永不裁，行为不变。

**改动点**：`truncate_split_point` 加目标比例参数；config 加
`historyTruncateTargetRatio`（默认 0.7，钳 0.5..=0.9）。

**验收**：单测三态——未超预算不动；超预算一次裁到 ≤70%；最后一条恒保留。

### P3 `lookup_receipt` 召回工具

**问题**：工具消息不跨轮持久，旧结果模型只能「重跑工具」（贵、有副作用）或编造
（现有对账闸拦编造，但没有给合法召回渠道）。

**方案**：新增只读内置工具，按 trace 查历史工具调用的完整凭证：

```
lookup_receipt({ "trace_id": 123 }) ->
{ "trace_id": 123, "tool_name": "...", "arguments": {...},
  "result": "存档原文（SPAN_TEXT_MAX 钳制内）", "success": true, "finished_at": ... }
```

- 数据源：`db::trace` 现成查询（trace_get_row / spans_for_trace），**只读，零新表**；
- 入参用 `trace_id`（UI 痕迹页可见）而非网传方案的 turn 序号——痕迹表没有 turn 概念，
  trace_id 是既有主键；溢出转存过的大结果，回执里带溢出文件路径与读法（复用 overflow 指针文案）；
- 查不到：返回明确「无此痕迹」提示，禁止模型脑补（与对账闸口径一致）；
- system 静态段加一条规则：引用旧轮工具结果必须先 `lookup_receipt`，不得凭记忆复述。

**改动点**：bot/tools.rs 加 SCHEMA const（静态，不破缓存前缀）；bot/registry.rs TOOLS_TABLE
注册；执行分支接 trace 查询；tool_guard 只读白名单；prompts/system.rs 加一条静态规则。

**验收**：单测（查回内容与落库一致 / 查不到返回提示 / 无写副作用）；实机——
把痕迹页某 trace_id 喂给模型，验证引用内容与 UI 显示一致。

### P4 并行调用提示词引导 + 同类工具热点统计

**问题**：引擎已支持一条消息多个 tool_calls（bot_model_loop.rs:3062），但模型默认一次一个；
每多一轮 = 多一次思考 output（全价）+ 轮次延迟。

**方案（层次 1，零基建）**：system 静态段加一条规则——「同类独立操作（批量建卡、批量读文件、
批量改名）必须合并到同一轮并行发出；仅当后一步依赖前一步结果时才分轮」。

**配套观测**：先做统计再做判断——从 exec_traces 统计真实会话里「同类工具连续独占轮数」
分布（如连续 8 轮都是 read_text_file），作为本项与 P7/P8 的收益依据。产出为一次性
分析查询（或 api_handlers 只读统计口），不改写库。

**安全网不变**：每个调用仍逐个过 tool_guard 全量网关；写类工具确认弹窗仍是每次一弹。

**验收**：单测（多 tool_calls 单轮执行回归，已有行为防回归）；实机对比引导前后
同类工具轮数与 function_calls_total。

### P5 高频工具 schema 加真实调用示例（Tool Use Examples 低配）

**问题**：JSON schema 只表达结构合法性，不表达使用惯例（日期格式、id 从哪取、
可选参数何时传）——参数写错 = 整轮重试（思考 output + 工具重跑双倍浪费）。

**方案**：不给模型加 fewshot 段（占前缀），只给 3-5 个高频易错工具的 SCHEMA const 的
description 里追加一行真实示例（如 create_task 的 when/schedule 格式、
read_text_file 的 offset 语义、link_file_to_task 的 taskId 来源）。静态 const，缓存无损。

**选点依据**：先用 P4 的热点统计选真实高频工具，不从名单拍脑袋。

**验收**：单测（schema 仍是合法 JSON，regen_tools_baseline_fixture 流程重生成）；
实机观察对应工具的参数错误率。

### P6 M2 思考段回灌开关（实测拍板）

**问题**：MiniMax 官方要求把 `reasoning_content` 随 tool_calls 回灌历史（思考链不能断），
当前实现只回灌 content+tool_calls（bot_model_loop.rs `with_thinking` 仅 Anthropic 协议）。
省 token 与官方质量口径冲突。

**方案**：加 config 开关 `reasoningEchoEnabled`（**默认关**，维持现状）；
开启时 OpenAI-compat 路径的 assistant(tool_calls) 消息附带本轮 `reasoning_content` 回灌。
回灌内容是 append-only 稳定消息，缓存可兜底大部分成本；
若与 P7 同开，老轮次的思考段随工具结果一并入桩（Anthropic CE 的 thinking clearing 同思想）。

**实测口径**：同一批真实会话 开/关 各跑一遍，对比工具调用返工率（同工具连续失败、
replan 触发）与最终答案质量；**用数据定默认值，不拍脑袋**。

**验收**：单测（开关两态的消息形状断言：带/不带 reasoning_content 字段）。

### P7 轮内工具结果滑动窗（二期，Anthropic Context Editing 对标）

**问题**：重工具轮 20 次迭代，早期工具结果在轮内被反复重发；溢出漏斗只挡超大结果，
中等结果（几百行 grep、整页 extract_document）全量滞留。

**方案**（阈值触发 + 一次批量 + 之后前缀重归稳定，避免反复改写破缓存）：
- 触发：轮内累计工具结果超过 `inTurnEditTriggerEstTokens`（默认建议 24K 估算 token，
  字符 × 系数估，不引 tokenizer）；
- 动作：**一次性**把「最近 `keepRecentToolResults`（默认 6）条之外」的工具结果替换为桩子：
  `⚠️ 已省略以控制上下文。工具 {name}（trace_id={id}）的完整结果已存档，
  需要时调用 lookup_receipt(trace_id={id}) 调取。` 之后不再改写，前缀重归 append-only；
- 强制护栏（对齐网传方案禁令，写断言）：
  ① 本轮最近 K 条永不裁；② 只裁 tool 结果消息，user/assistant 文本不裁；
  ③ **只改 prompt，DB 记录零改动**（exec_traces/溢出文件原样）；
  ④ 带图片的消息整体跳过（多模态形状不碰）。

**依赖**：P3 必须先上（桩子必须给得出可召回的 trace_id）。

**预期收益**：Anthropic 实证同机制 agentic 基准 +29%（质量口径），token 双降；
我们的实际收益取决于重工具轮占比（P4 统计先行）。

**验收**：单测四件——超阈值触发裁剪且一次性；最近 K 条与图片消息完整；
桩子含 trace_id 且对应 DB 记录可查回原文；未超阈值零改写。

### P8 工具自检挪出主循环（可选）

**问题**：每 5 轮一次的工具 Y/N 自检占主循环一整轮（全上下文重发 + 一轮延迟）。

**方案**：历史较长（超过阈值）时改走 `selfcheckModelId` 独立请求通道
（终稿自检的现成基建），发「工具名 + 结果 snippet」小请求；历史短时保持原样。

**取舍**：独立模型只看 snippet，判「是否达意」弱于主模型——所以只在长历史场景切换，
短历史场景主模型上下文里判得更准，不值得为省小钱降判定质量。

**验收**：单测（阈值两态的路由断言）。

---

## 六、分批实施与验收口径

| 批次 | 内容 | 提交切分 | 验收 |
|---|---|---|---|
| 批 1 | P1 + P2 | 一个 commit | 前缀稳定单测 + 滞回三态单测 + 实机 cache_read 占比对比 |
| 批 2 | P3 + P4（含热点统计） | 各一个 commit | lookup_receipt 单测 + 并行回归 + 实机召回/热点数据 |
| 批 3 | P5 | 一个 commit | schema fixture 重生成 + 参数错误率观察 |
| 批 4 | P6 | 一个 commit（默认关） | 开关两态形状单测 + 实机 A/B 后定默认值 |
| 批 5 | P7 | 独立 commit（依赖批 2） | 滑动窗四件单测 + 重工具轮实机对拍 |
| 可选 | P8 | 独立 commit | 路由单测 |

每批走既有门禁（`test-fast.sh`：cargo nextest + clippy -D warnings + vitest；
新文件 ≤1500 行；配置项同步设置面板可后续批做）。

---

## 七、不做清单（本次明确不做）

1. **exec_traces/凭证注入 prompt 的热/冷窗口**——凭证真源在 DB 是现行架构的刻意设计，
   注入再裁剪 = 先造成本再治理；P3+P7 已覆盖真实需求；
2. **Programmatic Tool Calling**——需要沙箱编排基建，单人应用不划算（YAGNI）；
3. **Tool Search Tool**——我们的静态工具定义远低于官方 10K token 门槛，收不回复杂度；
4. **固定 fewshot 段**——占前缀且无证据当前缺示例；用 P5 的 schema 内嵌示例替代；
5. **精确 tokenizer 依赖**——字符 × 系数估算够预算用；
6. **跨轮 PlanList 持久化/归档**——Plan 是单轮临时对象（bot_plan.rs:17），无归档对象；
7. **通用 batch_tools 批量壳**——第二个真实场景出现前不抽象；
8. **修改既有 SemIf/自检判定逻辑**——本方案只动装配与路由，判定逻辑零改动。

---

## 八、风险与回滚

| 风险 | 缓解 | 回滚 |
|---|---|---|
| P1 末尾 system 消息在个别端点行为差异 | 批 1 内 a/b 两形态实测后定 | 纯装配挪位，revert 即回原状 |
| P1 记忆召回质量回退（位置敏感） | 实机固定问题集对比召回命中 | 同上 |
| P7 桩子导致模型引用被裁内容 | P3 召回闭环 + 桩子文案显式指路 + 对账闸仍兜底 | config 关闭即整段旁路 |
| P6 token 上升超预期 | 默认关；实测数据定默认值 | config 开关 |
| P2 比例配置写错 | 钳制 0.5..=0.9 + 单测 | config 默认值 |

---

## 九、偏离登记

（实施时逐条回填：与本文档的偏离、原因、影响面。）

### 9.1 P1 形态从「末尾独立 system 消息」改为「并进本轮用户消息」

本文 §五 P1 定形态 a（末尾独立 system 消息）为默认。实施时改为形态 b（并进本轮
用户消息），原因：`bot_anthropic::openai_msgs_to_anthropic` 会把消息数组里**所有**
role=system 的消息抽出来合并进顶层 system 字段（Anthropic 协议的 system 是顶层字段）——
形态 a 在 Anthropic 路径上等于没挪。形态 b 对两种协议都成立。

代价：并进去的那条用户消息字节每轮都变，缓存前缀比形态 a 少命中一条（O(N) 里的
O(1)）。取舍理由是「两种协议都成立」值这点。

### 9.2 实测数据推翻了两处默认假设

用 `scripts/tool-hotspots.py` 对真实痕迹库（124 个 trace / 861 条 span / 51 个工具）
跑了一次分析，两处与本文假设不符：

| 项 | 本文假设 | 实测 |
|---|---|---|
| P7 触发阈值 24K 估算 token | 「重工具轮」常见 | 按 24K **字符** 算只有 8.9% 的提问触发；按 24K token（≈72K 字符）算只有 **0.8%**。实测 p50=1071 / p75=9704 / p90=22637 / p95=32764 / max=359810 字符 |
| P4 的收益空间 | 「模型默认一次一个」 | 已经不全是：75.7% 的轮只有 1 个调用，18.9% 有 2 个，3.7% 有 3 个。**多调用轮已占 24.3%**——并行能力本来就有，缺的是「什么时候该并行」的判断 |

**第三处：P5 选点依据不是频次，是失败率。** 本文 P5 写「先用 P4 的热点统计选真实
高频工具」。但实测里高频工具恰恰**不是**易错的——`read_text_file` 66 次只失败 1 次
（1.5%）、`grep_files` 56 次 0 失败、`list_files` 53 次 0 失败、`run_python` 156 次
失败 6 次（3.8%）。真正烧轮次的是另一批：

| 工具 | 调用 | 失败 | 失败率 | 固定失败原因 |
|---|---|---|---|---|
| `ui_type` | 21 | 16 | **76.2%** | 目标窗口不在前台（要先 `activate_window`）／控件不支持按压（要先 `ui_click` 聚焦） |
| `create_task` | 11 | 5 | **45.5%** | `due` 写成「2026-10-24 09:00」或「2026-10」／`column` 填了 `archived` |
| `fetch_url` | 15 | 6 | **40.0%** | 站点返 403/429/404／Content-Type 不是网页文本 |
| `edit_task` | 12 | 3 | 25.0% | `column` 填 `archived`／卡已完成或已归档 |
| `complete_task` | 14 | 3 | 21.4% | 对已完成/已归档的卡调用 |

故 P5 的示例加在这五个上（`ui_type` / `create_task` / `fetch_url` / `edit_task` /
`complete_task`），不是加在频次榜前列的读类工具上——给本来就不错的工具加示例是
白占前缀。

按实测重定的两个值：

- **P7 触发阈值 = 24,000 字符**（不是 24K token）。命中 8.9% 的提问，覆盖 p90 以上的
  尾部（含 359K 字符那条）。阈值再高一档（48K）只剩 1.6% 的提问，等于不做。
- **P4 的引导话术点名「互不依赖的读类操作」**：实测连续独占段最长的三个是
  `check_subagent`（19 段，最长 28 轮）、`run_python`（54 段，最长 16 轮）、
  `ui_inspect`（36 段，最长 4 轮）。前两个是「等子 agent / 看脚本输出」——**天然
  串行**，提示词合并不了；真正能并行的是读类（`read_text_file` 66 次 / `grep_files`
  56 次 / `list_files` 53 次）。故规则写「批量读文件、批量 grep、批量列目录…」，
  不写笼统的「同类操作」。

### 9.3 P3 的入参多了 `tool_call_id`，且支持「不传参 = 列清单」

本文 P3 写的是 `lookup_receipt({trace_id})`。实施时改为三个可选入参
（`tool_call_id` / `span_id` / `trace_id`），且三者都不传时列出本会话最近的调用凭证。

原因：痕迹表里**一次执行（trace）含多条工具调用（span）**，只给 trace_id 无法
定位到具体那一次；而模型手上唯一逐次唯一的标识是 LLM 签发的 `tool_call_id`——
它同时也是 P7 的桩子能带上的那个 id（「调取请用 tool_call_id=xxx」）。
「不传参 = 列清单」是为了让模型在不知道 id 的情况下也能找到要召回的那一条。

### 9.4 P3 的回执不含溢出文件路径

本文 P3 写「溢出转存过的大结果，回执里带溢出文件路径与读法」。实施时**未带**：
溢出文件的路径只存在于模型循环的指针文案与审计日志（bot.log）里，痕迹表没有
这一列；为它加一列就得给 `exec_spans` 加写路径（破坏该表 append-only 的观测面
定位）。回执改为给「存档原文按 16KB 上限截断」并如实说明——原文的前 16KB 仍在，
超出部分确实取不到。属于已知能力缺口，不是遗漏。
