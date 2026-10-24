# 子 agent 效率与透明度改造 — 2026-10-24

> 状态：**方案待拍板**（本文只设计不动手；落地时在 §9 登记 commit）。
> 前置：`SUBAGENT-HANDOFF-BUDGET-REDESIGN-2026-10-23.md`（已落地）。那份管
> 「产出不丢」（墙钟 / 收尾格式 / digest），这份管「花得省 + 看得见」（token /
> 轮次 / 效率 / 运行透明度），互补不重叠，机制全部复用前者的形态。
> 北极星（用户口径）：**以节省 token 为原则，提高效率，不把简单事情复杂化。**

---

## 0. 一页结论

实机账（2026-10-24 凌晨「用 ppt-generator 技能做一个」跑批）：子 agent 对一个
**173 行**的 SPEC.md 发起 91 次 read_text_file（窗口 20→10→8→6→4→2→1 行，
34% 行次是重读），烧满 100 次工具调用预算被硬熔断，产物零字节，4 分 05 秒
几乎全花在模型轮次上。

三个病，三个修：

| 病 | 实机账 | 修法 | 改动量 |
|---|---|---|---|
| 小文件碎读 | 173 行读 91 次、32 轮、4 分钟 | 提示词红线：≤500 行一次读完 + 打转自检 | 零代码 |
| 收尾路不通 | 70% 软警告被无视；熔断硬杀，写产物机会为零 | 收尾窗：预算触顶后只拒读类、放行写类 | ~30 行 |
| 黑账 | 子 agent token 用量无处记，省没省无法验证 | subagents 行落 usage 三列 | ~20 行 |
| 运行黑箱 | 右侧栏只有转圈 + 秒表，4 分钟打转全程不可见 | 动作流实时上屏（dispatch 审计点旁发事件） | 后端 ~25 行 + 前端 ~60 行 |

最大的单点浪费是**轮次**：每轮都要把增长中的全上下文重发一遍（MiniMax 前缀
缓存命中只是变便宜，不是变免费，且每轮都付首 token 延迟）。32 轮压成 ~6 轮，
token 与墙钟同时省一个量级——这就是「不把简单事情复杂化」的量化含义。

---

## 1. 业界口径

### 1.1 Anthropic 多 agent 研究系统：token 是第一约束

单 agent 用量 ≈ chat 的 4×，多 agent 系统 ≈ 15×——他们因此把「教 lead agent
量力拆分、subagent 只回**压缩后的结论**（不回原文）」当作设计核心，而不是
调预算数值。省 token 的主战场在**轮次与回传内容**，不在闸门。

### 1.2 Manus：KV-cache 命中率是生产 agent 的第一指标

cached 与 uncached token 差约一个数量级；上下文 append-only（不修改历史）；
并明确记录了 agent 会掉进 **few-shot rut**——照抄自己先前的调用模式、原地打转，
解法是主动打破重复。本案例 91 次碎读正是 rut 的活体样本：模型读到 173 行的
文件后仍按前几轮的小窗口惯性滑行。业界解法落在提示词红线 + 打转自检，
不是加机制。

### 1.3 Anthropic 工具设计：价值密度

工具结果追求「有用信息 / token」高；多步操作合并为一次调用；**工具描述本身就
是提示词**，且是模型每次调用都会重读的最近因。读文件该怎么读，写进
read_text_file 的 description 比写进 profile 更见效。

### 1.4 Claude Agent SDK：子 agent = 上下文隔离仓

子 agent 拿新鲜上下文干活、只回最终文本，重工具结果留在子上下文里不污染主
会话；平台侧另有陈旧工具结果清扫（一次回收 30–70k token）。本项目子 agent
隔离形态已对齐；「清扫」不做（见不做清单）。

### 1.5 Cognition 反方（上轮已登记，不展开）

单线程 + 上下文压缩优于多 agent 并行。本项目不因 token 问题换架构——
这份方案全部是**把现有架构跑省**，不是换掉它。

### 1.6 UI 透明度：动作流，不是全文流

各家给最终用户看的都是**动作级摘要流**，没有一个产品给用户看原始 JSON 或
token 级全文：

- **ChatGPT Deep Research**：研究过程实时渲染成「Researching…」可展开步骤
  列表——每条搜索 / 每次读页一行、短语式标题，底层是一条动作事件流，
  跑完仍可回看。透明度靠短而准的条目，不靠吐字。
- **Manus「Manus's Computer」**：实时面板展示浏览、命令、中间文件——
  重形态，服务于「看它操作电脑」的产品叙事。
- **Devin**：整机 session 实时回放（VM 屏幕级），最重，依赖 VM 源。
- **Claude Code**：终端里每个工具一行 spinner + 短描述，子 agent 是嵌套任务行；
  PreToolUse / PostToolUse / SubagentStop 钩子就是官方动作事件流，
  第三方 HUD 全靠它长出来。

对项目的推论：wmessage 的 dispatch 层**已经有**这两个审计点（tool.call /
tool.return，带工具名 / 耗时 / preview 截断），透明度缺的不是数据源，
是把审计点旁路一份到 UI 事件通道。

---

## 2. 实机账（sa_0f0127…，03:06:59–03:11:04）

| 项 | 值 |
|---|---|
| 工具调用 | 100 / 100：91 读 + 7 搜 + 1 extract_document + 1 read_own_card，**0 写** |
| 读的对象 | SPEC.md（173 行）：90 次分页共 263 行次，唯一覆盖 173 行，**重读 34%** |
| 轮次 | 32 / 50，每轮 4–46s；工具执行每次 5–8ms，**墙钟 244.7s 几乎全是模型轮次** |
| 收尾提醒 | 70 次时注入一次（03:09:40「最多还能调 30 个，请尽快收尾」），被无视，之后又 30 次单行读 |
| 终局 | **模型循环层熔断先于 dispatch 层拒绝**（audit「fuse \| 单轮 Function 调用超过 100 次」；全日志 `budget_tool_calls_exceeded` 零次触发）——dispatch 层「停止新探索，整理当前结果并输出收尾 JSON」的拒绝文案是死路径，模型没有收尾轮 |
| 产出 | wmessage-spec-digest.md 不存在；机检 6 条 0 过；result_json 只剩熔断前正文片段 |
| 主 agent | 全程阻塞在 check_subagent(waitMs=240s)，4 分钟无响应 |

注：read_text_file 默认一次可读 500 行、上限 2000 行，173 行的文件一次调用
就该读完；工具描述只写了「大文件用 offset/limit 分页」，没说小文件怎么办。

---

## 3. 现状断点

| # | 断点 | 位置 | 后果 |
|---|---|---|---|
| 1 | 工具描述与 profile 都没说「小文件一次读完」 | `bot/registry.rs` read_text_file schema；`prompts/subagent.rs` 三 profile | 模型自选 limit=20 起步，越读越碎（rut） |
| 2 | 模型循环层熔断在 dispatch 预算闸**之前**判定（`function_calls_total > cap` 即 return） | `bot_model_loop.rs` should_fuse 分支 | 收尾拒绝路径永远轮不到触发，模型被硬杀 |
| 3 | 收尾手段只有 70% 一次性软警告（可无视） | `bot_model_loop.rs` soft_warn | 模型磨到最后一滴预算，产物零字节 |
| 4 | read_own_card 只回静态预算 | `bot_orchestrator.rs` tool_read_own_card | ctx 里 used_tool_calls / used_rounds / wall_deadline 都有，只是没给模型看 |
| 5 | 子 agent 不落 exec_traces，subagents 表无 usage 列 | `bot_orchestrator.rs`（子 agent 免 trace 注释）、db schema | cache 命中率（业界第一指标）无从算起 |
| 6 | 右侧栏子 Agent 页只有聚合摘要，无动作流 | `ChatPanel.tsx`（3s 轮询 + spawned/finished 重拉）、`ContextRail.tsx` | 「正在干什么 / 卡没卡 / 打没打转」全程不可见——PPT 案例 4 分钟打转在 UI 上就是一个安静的转圈 |

---

## 4. 改造清单

### ① 提示词红线：小文件一次读完 + 产物早落盘 + 打转自检（零代码，P1）

三处文案，共 ~6 行：

a. `read_text_file` schema description 补一句：
   「≤500 行的文件不传 limit 一次读完；分页只用于 >1000 行的大文件。」
   落点是 description 而非 profile——模型每次调工具都会重读它（§1.3）。
   schema 变了要重生成 `tools_baseline.json` fixture（regen 脚本，--ignored）。

b. 三个 profile（research / coder / general）各补两行：
   - 读文件红线：同 a 口径，加半句「把一个文件拆成碎片逐段读 = 白烧预算」。
   - 打转自检：「连续多轮重复同类调用（重读已读内容、原地打转）→ 停，
     换策略或直接收尾。」（Manus rut 解法，§1.2）

c. 产物纪律一行（三 profile 共用）：
   「带文件产物的任务尽早落盘：先写骨架文件，再 edit_file 增量补齐——
   预算或时间触顶时留下半成品，不是零。」

### ② 收尾窗：预算触顶后「只拒读、放行写」（~30 行，P1）

与已落地的软墙钟（dispatch 层边界拒绝）完全同构，搬到工具数轴上：

- dispatch 预算闸（`bot/dispatch.rs` acquire 失败分支）：
  - 首次触顶：置 ctx 新增 `wrap_up: AtomicBool`，拒绝本次调用，文案
    「⚠️ 工具调用预算已触顶（used / max）：停止新探索。现在只放行写产物
    工具——把已有发现写成产物文件，然后输出收尾 JSON。」
  - 后续调用：`wrap_up` 为真且工具 ∈ {write_file, write_artifact_file,
    edit_file, read_own_card}（∩ ctx.tools）→ 放行（不走 acquire，不再计数）；
    其余拒绝（同文案短版）。
- 模型循环层熔断：**子 agent 会话**的硬杀阈值从 `cap` 挪到 `cap + 5`，
  退化为纯兜底（防模型在收尾窗里无限刷被拒调用；5 = 1 次写产物 + 裕量，
  write_artifact_file 单调用写全文，正常一次就够）。主 agent 会话不变。
- 墙钟 / 轮数闸不动：收尾窗内照常生效。

效果：PPT 案例里第 101 次调用被拒并被告知「只许写」，模型有一整轮把已读到的
SPEC 内容写进 digest 文件——机检第一条（file_exists）就有救；即使之后仍被
兜底硬杀，盘上已有产物。

### ③ read_own_card 回实时余量（~10 行，P1）

`tool_read_own_card` 返回 JSON 增一块（数据全部现成：ctx 三个 Arc / Instant），
静态 budget 并入同一块，不再两份：

```json
"budgetUsage": {
  "toolCallsUsed": 47, "toolCallsMax": 100,
  "roundsUsed": 12, "roundsMax": 50,
  "wallRemainingSecs": 1800
}
```

### ④ 子 agent token 记账（~20 行，P2）

- `subagents` 表加三列：`prompt_tokens / completion_tokens / cache_read_tokens`
  （迁移走 db/mod.rs 既有模式）。
- run 结束时从模型循环已累计的 usage 原子量写入——主 chat 写 exec_traces 用的是
  同一套数，不新增埋点。
- 用途：验证 ①② 的真实收益；算 cache 命中率；给预算默认值调参留依据。
  只落库可查，不做面板。

### ⑤ 透明度：子 agent 动作流实时上屏（后端 ~25 行 + 前端 ~60 行，P1）

现状：右侧栏「子 Agent」页有单行摘要（状态 + 目标 + N/M 轮 + 秒表；3s 轮询 +
spawned / finished 事件重拉列表），详情面板有验收 / 结果——但**看不到正在
干什么**：没有动作流，打转和卡死在 UI 上都长成一个安静的转圈（§3 断点 6）。

业界口径（§1.6）：动作级一行流（Deep Research 步骤列表 / Claude Code
spinner 行），不做全文流。数据源已齐——dispatch 的 tool.call / tool.return
审计点每调用都过，缺的只是旁路一条 UI 事件。

改法（不新增 IPC 命令、不落库）：

- 后端：dispatch 两个审计点旁对子 agent 会话各加一条
  `emit_to("main", "subagent-activity")`：
  `{subagentId, turn, tool, phase: call|return, ms?, ok, preview, usedToolCalls, maxToolCalls}`。
  preview 复用审计已有的截断串（同一 sanitize 管道，不新开脱敏面，≤160 字）。
  事件即焚——历史回看走详情面板已有的验收 / 结果 / digest，不另存流。
- 前端：ContextRail 子 Agent 页，运行中的行下挂**最近 6 条动作**（新到旧，
  一行一条：`read_text_file · SPEC.md 第 47-50 行 · 8ms`；call/return 合并
  展示，return 覆盖同 id 的 call 行，不双行刷屏）。列表命令补
  `toolCallsUsed / maxToolCalls` 字段（与 roundsUsed 读同一组 Arc 仪表），
  行上「N/M 轮」旁加「N/M 次」。② 的预算拒绝也走这条流——「只许写」收尾窗
  在 UI 上可见。
- 内嵌 strip 与详情面板不动。

---

## 5. 优先级与预期效果

| 级 | 项 | 理由 |
|---|---|---|
| P1 | ① 提示词红线 | 零代码零风险；直接对着 PPT 案例复演：91 次读 → 1 次 |
| P1 | ② 收尾窗 | 同构复用软墙钟形态；把「硬杀零产出」变「半成品保底」 |
| P1 | ③ 实时余量 | 模型第一次有「还剩多少」的读数，软警告从孤响变成可查的账 |
| P1 | ⑤ 动作流上屏 | 数据源全现成（审计点旁路）；透明度同时是 ①② 的验收观测窗——打转、收尾窗触发都能亲眼看到 |
| P2 | ④ token 记账 | 验收本方案自身效果的尺子 |

PPT 案例复演推演（①+②+③ 全上）：1 次读全文（约 4–5k token 一次付清）→
逐域提炼 → 1–3 次写 → 收尾 JSON。轮次 32 → ~6，墙钟 4 分钟 → ~40s，
prompt token 降一个量级（省的大头是每轮增长重发，不是读到的字节本身）。

---

## 6. 不做清单

| 不做 | 理由 |
|---|---|
| 服务端重读去重（比对读历史、返回「已读过」） | 治标。① + ② 之后碎读动机大幅消失；④ 的账上真复现再议 |
| 多文件批量读 / 读搜合一等新工具 | 新工具 = 新 schema = 新测试；read 单文件 500 行已够密，YAGNI |
| 子 agent 上下文压缩 / 陈旧工具结果清扫 | MiniMax 走 openai 兼容流式，无平台级 context editing；自研清扫是投机抽象，等 ④ 的账上真出现「上下文爆」再议 |
| 85% 二次升级提醒 | ② 的拒绝本身就是升级（强制、不可无视），再加提醒是复杂化 |
| token 第四预算轴（max_tokens 钳制 + 熔断） | 轮数 × 工具数已近似约束 token 总量；第四根轴牵出整套钳制/熔断/提示词面 |
| 强制并行工具调用 | MiniMax-M3 已自发按批并行（4–5 个/轮）；提示词教不会的事不用机制硬掰 |
| 改 KV-cache 策略 | 消息栈已是 append-only（业界最佳形态）；provider 侧缓存已生效（主 trace cache_read=139k） |
| 子 agent 全文流式上屏 | 业界没有一家给最终用户吐 token 级全文（§1.6）；动作流已回答「在干什么」，全文是噪音 |
| 动作流持久化 / replay | Devin 式会话重放、Manus 式 Computer 面板都依赖 VM/屏幕源；本项目工具是本地函数调用，无此源无此需求。历史回看 = 详情面板已有的结果与 digest |
| LangSmith 式 trace 树 UI | audit 日志已是全量 span trace，UI 树是第二消费面（YAGNI）；调试场景直接看库 |

---

## 7. 口径变更登记

- 「工具调用预算触顶」从硬熔断改为「收尾窗（只拒读、放行写）+ 5 次硬兜底」。
- 子 agent 模型循环硬杀阈值 = `max_tool_calls + 5`（仅子 agent 会话；主会话不变）。
- read_own_card 返回新增 `budgetUsage` 实时块，静态 budget 并入同一块。
- subagents 行新增 usage 三列（只记不展示，查询走 sqlite）。
- 新增 UI 事件 `subagent-activity`（仅子 agent 会话，事件即焚不落库；
  preview 与审计同源同截断）。

---

## 8. 验收（实机）

- 复跑 PPT 同款 spawn（objective / 验收标准不变）：read_text_file 对 SPEC.md
  ≤ 3 次；产物文件存在且非空；status=succeeded；④ 的 usage 列有数。
- 故意触顶一次（预算钳到 10）：模型收到「只许写」拒绝后能产出产物文件 +
  收尾 JSON；若它确实一次没写才落 budget_exceeded，且 digest 带残留 summary。
- 复跑全程盯右侧栏：动作行实时滚动、能看到「读什么 / 搜什么 / 几 ms」；
  触顶瞬间看到「只许写」拒绝行；终态后动作流停住、摘要行转终态图标。

---

## 9. 落地进度

（待拍板后回填）
