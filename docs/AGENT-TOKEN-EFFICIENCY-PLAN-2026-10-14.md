# Agent 省 Token 不降性能——调研对照与 wmessage 改进方案（2026-10-14）

> 依据：两路外部调研（技术手法盘点 + 成熟产品工程实践，来源见文末）+ 本仓
> 代码现状核实。核心事实：Agent 输入/输出比约 100:1（Manus 实测）——每一轮
> 都重付整段历史的输入费，所以省 token 的主战场是**输入侧**；而「先缓存后减法」
> 是次序铁律（压缩会打穿缓存前缀）。

## 实施进度（2026-10-14）

| 项 | 状态 | 提交 | 备注 |
|---|---|---|---|
| P0-1 工具输出默认截断 30K | ✅ | `078e545` | cap_tool_output 头 24K + 尾 6K，按字符计 |
| P0-2 终稿自检侧调小模型 | ✅ | `e5291dc` + `fc978d6` | selfcheck_model_id 配置；侧调走 summarize_http |
| P0-3 历史滚动降密度 | ✅ | `263109f` | AGE_SOFT_ROUNDS=5 / HARD=15，compact_one 单条核 |
| P1-4 Anthropic 第三断点 + 动段位 | ✅ | `dc11dc1` + `986567c` | 断点 2/2 → 3/3；VerifierRetryTail 末位槽 |
| P1-5 cache 命中/写入观测 | ✅ | 随 P0-2/P1-4 落库 | AnthropicUsage 4 字段 + cache_hit_pct 审计 + 挂件 bot-usage-delta 扩字段 |
| P2-2 复诵计划 | ⏸ 待拍板 | — | 等真机观察长任务目标漂移再启 |
| P2-3/4/5 不做 | ✅ | — | LLMLingua/并行多 agent 写/激进压缩已附理由 |
| Wave2 escape/truncate + lock_ok | ✅ | `6d8b526` + `33a386d` | log_sanitize 单源 + 同名 escape_for_log 分叉修复；lock_or_recover 收敛 140 处 mutex_poisoned |

## 标签整理功能（2026-10-14 单独决策）

按归档页「整理标签」入口，扫归档卡 tags 按余弦相似度聚类（union-find 复用
corrections 闭环模式），用户选主后改源 + 落 `tag_merge_history` 表 + 设置页
操作历史可按 run_id 撤销。拍板阈值 0.92 起步 / 仅归档卡 / 0 后台 / 完整
保留撤销机制在设置页「操作历史」tab。

| 阶段 | 提交 | 备注 |
|---|---|---|
| 后端内核 + DDL + 7 单元测 | `bca9d12` | cluster_archived_tags / merge_tag_cluster / rollback_tag_merge；AND/OR 优先级踩坑（fix 注释） |
| 4 个 tauri command 注册 | `a2d18fc` | tags_consolidate_scan / merge / rollback / list_runs |
| 前端 Modal + ArchivePage 按钮 + chips 折叠 + Settings tab | (本批) | portal 渲染 + select 选主 + 「合并 N 个簇」动态文案；details 折叠 |
| e2e 串联测试 | (本批) | 9 步 scan→merge→rollback→二次拒绝 |

**全量测试 1630 测全绿（基线 1619 + 新增 11 测）。**

## 一、wmessage 现状 vs 业界对照

| 业界实践 | wmessage 现状 | 判定 |
|---|---|---|
| 阈值触发 compaction（Claude Code ~85-92%、Codex ~90%） | 70%/85% 软硬压缩，只动 tool 消息 | ✅ 比业界更早，方向对 |
| 工具输出落盘 + 上下文留指针（Manus「文件系统即上下文」） | sidecar conv-history + 原文见路径 | ✅ 已对齐 |
| 子 agent 上下文隔离（只回 1-2K 结论） | recon/executor/verifier 三段式，只传 objective/验收/摘要 | ✅ 已对齐 |
| 渐进式披露（技能只给 name+description） | 技能目录轻量注入 + use_skill 按需读 | ✅ 已对齐 |
| 极小 verdict 判定（Y/N 省 token） | 工具自检 + 终稿自检，单字判定 | ✅ 已对齐（注意业界 recall 警告） |
| Prompt caching：稳定前缀 + append-only + 显式断点 | Anthropic 适配层已打 system+末工具两个 ephemeral 断点 | 🟡 有一半：断点少、动态段位置未审计 |
| 工具输出硬顶（Claude Code 默认 30K 字符） | `maxToolOutputChars` **默认 None = 不截断** | ❌ 最大缺口 |
| 陈旧工具结果持续清除（Anthropic context editing：token -84%、表现 +29%） | 纯阈值触发，70% 以下一点不动 | 🟡 有机制缺策略 |
| 固定槽位模型路由（Haiku 级做摘要/判定，RouteLLM 省 85% 保 95%） | 全部用同一主模型 | ❌ 未做 |
| 记忆外置 + 保留失败痕迹 | memory 模块 + lesson + fail hint 注入 | ✅ 已对齐 |

结论：wmessage 的机制层已经把业界共识的「四大件」（压缩/落盘/隔离/渐进披露）
都建齐了，差距集中在**三个策略缺口**（默认截断、持续降密度、模型路由）和
**缓存精细化**上——都是小改动大收益的位置，不需要新架构。

## 二、改进方案

### P0-1：工具输出默认截断（最低成本，当天可做）

- **做什么**：`maxToolOutputChars` 默认从 None（不截断）改为 30,000 字符；
  截断方式从「只留头部」改为 头 24K + 尾 6K（复用 bot/compaction 的头尾
  保留思路，错误堆栈/总结性尾部不丢）。
- **依据**：一次大 fetch / 大文件读取现在可以无上限吃进上下文；业界全线设
  硬顶——Claude Code 文件读 25K token、Bash 30K 字符，Gemini CLI 40K 字符
  外加蒸馏（10K token）/摘要（20K token）两道二级闸。机制已存在
  （708-740 行），只补默认值和头尾保留。
- **收益**：单条大输出的预算占比从「上不封顶」锁到 ~15%（200K 预算口径）；
  配合已有 compaction，长会话峰值显著下降。
- **风险**：极低——截断带说明文字 + trace span 存原文（已实现），需要全文走
  sidecar/read_text_file 回看。

### P0-2：终稿自检改「侧调小模型」，省掉主循环一整轮

- **做什么**：FINAL_SELFCHECK 从「主循环内注入 prompt 多跑一轮」改为独立
  小请求：只带 500 字 snippet + 判定 prompt，路由到廉价模型槽位（复用现有
  models_by_provider 配置机制加一个 `selfcheck` 槽位；未配置 = 保持现状）。
- **依据**：输入/输出比 100:1 → 主循环里多一轮 = **整段历史重付一次输入费**；
  而判定 prompt 本来就只看 500 字 snippet（循环内外信息量几乎相同）。
  业界把「摘要/分类/判定」固定槽位路由到小模型是标准做法：Claude Code 用
  Haiku 跑后台子任务（`SUBAGENT_MODEL`），Cognition 为「历史压缩」专门微调
  了小模型（「hard to get right but rewards the investment」），OpenHands
  摘要 LLM 明说「often cheaper model than reasoning LLM」；RouteLLM 实测
  省 85% 成本保 95% 表现。
- **收益**：每 8 轮一次的全史输入重付变为一次几百字符的请求；长会话下这是
  本方案里省得最多的一项。Y 判定不写审计的现有语义不变。
- **风险**：小模型判定略弱——业界数据二元判定对「不一致」样本漏报 30-60%，
  但本判定漏报的代价只是「少一条 ⚠️」（N 不改写正文），可接受；主循环轮数
  减少，长会话还有速度收益。

### P0-3：历史降密度从「阈值触发」改为「滚动降密度」

- **做什么**：compaction 保留 70%/85% 阈值作为兜底，另加滚动策略：距今
  >N 轮（N=5，对齐 Claude Code microcompact「只保留最近 5 条工具结果」）
  的 tool 结果自动软压（头尾保留 + sidecar 指针），更早的硬压。
- **依据**（三条独立实证，都指向「老结果降密度不但省钱还提效」）：
  - Anthropic context editing（清陈旧工具结果）：token -84%、agentic
    search 表现 +29%；官方称这是「最安全的轻量压缩」；
  - SWE-agent 消融（SWE-bench Lite）：只保留最近 5 条观察 **18.0%** vs
    全历史 **15.0%**——丢老观察反而高 3 分；文件视窗 100 行 > 30 行 >
    全文，太多太少都掉分（context rot 的直接证据）；
  - OpenHands condenser：质量 54% vs 53% 基本持平，每轮成本 **不到一半**，
    成本曲线从二次方变线性。
- **收益**：上下文曲线从锯齿悬崖变缓坡；每轮输入都更小（不只告急轮）；
  模型注意力集中在最近 K 轮的高信号内容上。
- **风险**：模型需要回看老结果时多一次 read_text_file——sidecar 指针 +
  已修好的硬压缩保指针（f9d03f5）就是为这条路铺的。滚动参数做成配置项；
  借鉴 Claude Code 的防抖设计：连续几轮压完又满则停止并报错，不空转。

### P1-4：缓存精细化（省 50-90% 输入费的零质量损失项）

- **做什么**：
  1. 审计 system prompt 的动态段位置：`## 当前设置` 等每会话可变内容固定
     注入到 system 尾部（OpenAI 官方 best practice 原话「Place stable
     content at the beginning, dynamic content at the end, keep context
     append-only」；Claude Code 甚至为此默认关闭插件后台刷新——「changes
     the system prompt mid-session, invalidates prompt caching」）；
  2. Anthropic 断点从 2 个加到 3 个：最后一个 user 消息再打一个 ephemeral
     （SWE-agent 的 CacheControlHistoryProcessor 默认就在最后 2 条消息上
     打断点），实现「system+工具+旧历史」增量缓存；
  3. compaction 原地改写历史消息会作废其后全部缓存前缀——Claude Code 把
     这笔账记为「expected rebuild」，属低频一次性代价，可接受；Anthropic
     API 侧已有 context editing 能力可按 tool_use_id 删结果而不动前缀
     （cache_edits），若走 Anthropic 原生协议可评估切换到该机制。
- **依据**：Anthropic 缓存命中 0.1×（省 90% 成本 + 85% 延迟）、OpenAI
  GPT-5 命中价 0.1×（GPT-5.6 起写价 1.25×）、Gemini 隐式缓存 90% 折扣、
  DeepSeek ~90-98%；Manus 把命中率列为「生产级 agent 的唯一最重要指标」
  （实测 10× 价差）。
- **收益**：多轮会话的大头输入费直接打折；不改任何行为语义。
- **风险**：断点位置不当反而降低命中（写入费 1.25×）——需要 P1-5 的观测
  数据先行验证。

### P1-5：每轮 token/缓存观测（先有数，再优化）

- **做什么**：llm.request 审计事件追加 input chars（已有 msgs_chars）+
  响应侧 usage（Anthropic 的 cache_read_input_tokens / OpenAI 的
  prompt_tokens_details.cached_tokens）落 trace；前端设置页加一行
  「本轮会话输入/缓存命中」。
- **依据**：各家工程博客一致把缓存命中率与每轮成本作为 agent 的第一观测
  指标；没有数就无法验证 P0-2/P0-3/P1-4 的实际收益。
- **风险**：纯观测，零行为风险。

### P1-6：主提示瘦身（与审计报告 P2-5 同一项，从 token 视角再记账）

- MAIN_AGENT_ADDENDUM 里的三份角色定义逐字节拷贝（~1.16KB）删除，改为
  「spawn 时给 stage 字段即可」一句指引——角色文本由运行时按 stage 拼装
  （已实现）。每次主 agent 对话省 1.16KB 系统提示，且消除双维护漂移。

### P2（暂缓/不做，记录理由）

- **计划复诵（recitation）**：候选。Manus 平均任务 ~50 次工具调用，靠不断
  重写 todo.md 把目标推到上下文末端对抗 lost-in-the-middle；wmessage 已有
  plan_state 与 PREVR replan，但计划只在重规划时重注入——若真机观测到
  长任务目标漂移，把「当前计划/剩余步骤」周期性追加为 user 提示是现成
  机制的轻量扩展。
- **LLMLingua 类提示压缩**：不做。研究一致结论——对代码/配置等事实密集
  内容不可靠（丢一个标识符即错），2026 生产风向是 compaction+外置记忆，
  不是压缩重写（ACE 论文更点名「压缩式重写」导致 context collapse）。
- **并行多 agent 写操作**：不做。Cognition《Don't Build Multi-Agents》：
  编码任务每个动作隐含设计决策，缺共享上下文的并行写产出的集成结果差；
  wmessage 三段式串行 + recon/verifier 只读隔离已是共识形态（Claude Code
  自身也只把子代理用于「回答边界清晰的问题」，并给出多代理 15× token、
  agent teams 7× token 的成本警示）。
- **无差别提高 compaction 频率**：等 P1-5 数据说话；Gemini CLI 默认 0.5
  触发线说明激进派存在，但 wmessage 的滚动策略（P0-3）已覆盖其收益主体。

## 三、预期合计

P0 三项全做：长会话峰值上下文预计降 40-60%（截断 + 持续降密度），自检
开销从「每 8 轮一次全史重付」降两个数量级（侧调小模型）；P1 缓存精细化
对多轮会话输入费再打 50-90% 折扣（零质量损失）。全部不需要新架构——
机制层 wmessage 已建齐，缺的只是策略与默认值。

## 来源（节选）

- Anthropic：Effective context engineering for AI agents / Context management
  （context editing -84% token、+29% 表现；memory tool +39%）/ 多代理研究系统
  （90.2% 提升但 15× token）/ Code execution with MCP（工具定义 150K→2K token）
  / Agent Skills 三级加载 / Prompt caching 文档（0.1× 命中价）
- Manus：Context Engineering for AI Agents（100:1 比例、KV 缓存第一指标、
  mask 勿移除、文件系统即上下文、todo 复诵、保留失败痕迹）
- Cognition：Don't Build Multi-Agents（单代理 + 共享上下文）
- OpenAI：Prompt caching guide（50-95% 折扣）/ Structured Outputs（100% 遵从）
- Codex CLI 压缩阈值 ~90-94.7%；Claude Code 缓冲 33-45K token、bash 输出 30K 字符
- RouteLLM（-85% 成本保 95% 表现）/ FrugalGPT（级联省 98%）；Haiku 4.5 = Sonnet 1/3 价
- LLMLingua-2（2-5× 压缩）及其局限；ACE（+10.6%，反对压缩式重写）
- Chroma：Context Rot（长窗口非单调退化）；Eugene Yan / Judge's Verdict
  （二元判定 precision >95% 但不一致样本 recall 仅 30-60%——verdict 按漏报代价校准）

*完整 URL 清单见调研底稿；价格为 2026-10 检索值，落地前核对官方定价页。*
