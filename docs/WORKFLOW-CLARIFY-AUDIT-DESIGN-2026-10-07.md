# 工作流改造:双层档案 + 执行问答 + 验收/审计(设计定稿 v2)

> 2026-10-07 · 状态:已批,分片实施
> 关联:`docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md`(决策 6 沿用)、`docs/AGENT-TRANSPARENCY-DESIGN-2026-10-06.md`、W-QA(W5-W8 批次)

## 0. 拍板记录

1. **执行架构维持现状(A 方案)**:纯代码拓扑调度(就绪即跑)、节点 agent 对等、无主 agent。
   否决 LLM 主 agent 调度(B):主 agent 须吞下全部节点结果(上下文爆炸,现有 600/2400 有界简报机制失效)、
   无人值守整夜 LLM 循环风险、对话态断点续跑推翻现有指纹机制、动态改道导致"画布=执行计划"失真。
   LLM 异常处置提案(C 方案)列入后续候选,不在本改造。
2. **澄清发生在拆解前**(独立 `workflow_clarify` 调用),不做拆解中打断——不动一次性调用骨架,零新事件通道。
3. WORKFLOW-CANVAS-DESIGN 决策 6 维持:改 goal → 整图重拆,不做增量重拆;**新增:重拆保留澄清答案**。
4. 验收语义:**status(执行成败)与 acceptanceVerdict(质量裁决)分离**;下游放行仍只看 status。
5. **执行提问(2026-10-07 二次拍板)**:节点执行中允许阻塞式提问(缺关键信息/需确认时),
   **红线:每个问题必须自带 agent 假设值**——未答/超时 = 按假设继续 + 卡上标记,忽略问题工作流也能走。
   预算 ≤2 问/节点;工作流级提问模式设置(从不 / 关键决策才问(默认) / 重要节点每步确认)。
   调度仍是代码拓扑,提问≠LLM 调度,A 方案不推翻。
6. **双层档案替代"每卡一份孤立文件"**:跑偏多为跨卡,单卡档案互相看不见。
   上层 = **工作流决策摘要**(全局,有界,注入每个后续节点);下层 = **卡片档案**(本卡,重跑时注入)。
   形式为 DB 结构化记录(UI 呈现为文档、可导出 Markdown),不做散落物理文件
   (文件与任务生命周期脱钩、注入长度不可控、agent 直改无校验)。
   条目来源全走代码路径(问答/返工/手动编辑),agent 不可自由写档案——继续守"LLM 提案,代码裁决"。
7. **通知即队列**:待答问题落 `notifications` 表(新 kind `workflow_question`),应用内通知页富回答
   (选项 chips + 输入框 + "按假设继续"),系统通知只做门铃(OS 通知放不下自由文本,点击跳转)。
   生命周期:run 停止/应用重启 → 未决问题失效;run 已死后才送达的回答 → 落档案,重跑时生效,不自动拉起旧 run。
8. **问答机制统一**:clarify(拆解前)与执行提问共用同一张待答队列(notifications)、同一套
   "必带假设/超时按假设"语义;前台拆解走 hero 内联卡(用户在场),后台执行走通知(用户不在场)。

## 1. 现状基线(2026-10-07 核实)

- 拆解:`workflow_decompose` 一次性调用 + 校验链(≤30 节点/限长/剥 fences/Kahn 环检测/拓扑重排),
  失败带错误反馈重试 1 次;审计事件 `workflow_decompose` 写 bot.log;附件直调 doc_extract(12k/48k 封顶)。
- 执行:`workflow_runner` 拓扑就绪即跑;节点执行注入总目标(W-QA B1)+ 上游简报(600/2400)+ **本卡验收标准**
  (`bot_chat.rs:1856`);收尾引擎写证据 `build_node_result`;失败有界重试(`should_retry`,熔断不重试)。
- 收尾评审:W-QA rubric 裁决 pass|partial|fail + issues + 有界返工环;report 落 `last_report`。
- 审计:`audit::write_event` 行式写 bot.log,**无 run 级可查询结构**。
- 通知:`notifications` 表已具持久队列形态(id/kind/title/body/payload/status/created_at/resolved_at,
  pending/resolve 语义 + `notifications-changed` 广播 + 导航角标)——问答队列直接长在其上,不建新表。
- 确认通道样板:`ask_confirm_inner`(bot_slash.rs:263)oneshot + AppState.confirm_requests + 60s 超时默认拒;
  后台执行整条不可用(已知缺口,AGENT-TRANSPARENCY §1.4)——本次用"持久队列 + 假设兜底"补上。
- 缺口:①拆解前无澄清;②节点级验收核查缺失;③审计不可查询;④执行教训不回流拆解;⑤后台执行无法向用户提问。

## 2. 总体结构

| 批次 | 内容 | 依赖 |
|---|---|---|
| **W9-ASK** | 双层档案(§3.1)+ 执行提问通道(§3.2)+ 通知问答(§3.3)+ clarify 统一(§3.4)+ decompose 扩展(§3.5) | — |
| **W9b 小件** | 附件透明化/粒度开关/模型分层/度量埋点/重拆预填(§3.6) | W9-ASK |
| **W10-QA-AUDIT** | 节点级验收 check(§4.1)+ run 级结构化审计(§4.2) | W9-ASK 的模型分层与埋点约定 |

## 3. W9-ASK:双层档案 + 执行问答

### 3.1 双层档案(db/brief.rs)

- **新表 `brief_entries`**:`id INTEGER PK AUTOINCREMENT / workflow_id TEXT NOT NULL / task_id TEXT NULL /
  kind TEXT / source TEXT / text TEXT / reason TEXT / created_at INTEGER`;`task_id IS NULL` = 工作流级。
  索引 `(workflow_id)`。单源 DDL 常量 + ensure 调用,照 WORKFLOWS_DDL 模式。
- **kind/source 取值**:kind = `qa`(问答)/`rework`(返工原因)/`manual`(手动批注)/`clarify`(拆解澄清继承);
  source = `user`/`agent`/`qa`/`system`。text ≤200 字(入库 trim 截断),reason ≤80 字。
- **注入有界**:工作流级取最近 6 条(≤1200 字),卡片级取最近 8 条(≤800 字);
  注入段标题"工作流决策摘要——用户已确认的方向,优先级高于任务卡原文"。
- **写入口(全部代码路径)**:问答应答端(§3.3)、W-QA 返工(W10 接)、用户手动编辑(卡详情面板)、
  澄清答案继承(拆解成功后每卡落一条 clarify 摘要?否——clarify 是 goal 级,只落工作流级,卡片档案从执行期开始积累)。
- **生命周期**:工作流删除 → 档案级联删除;任务卡删除 → 卡片条目删除(workflow_id 保留条目为全局层,不动)。

### 3.2 执行提问通道(ask 端)

- **AppState 增 `question_waiters`**:`Mutex<HashMap<String /*questionId*/, oneshot::Sender<String>>>`
  (照 confirm_requests 形态;值 = 回答文本,assumption 兜底由 ask 端在超时分支处理)。
- **ask_user 工具**(仅 workflow origin 的节点执行注册):入参 `{question, why?, options?, assumption}`;
  契约要求 assumption 必填(红线);引擎侧预算 ≤2 问/节点(runner 传入计数,超预算工具直接返回
  "提问预算已用尽,请按假设继续")。
- **等待语义**:落通知队列(§3.3)→ `oneshot` 等待,超时可配(默认 24h;定时场景下一轮调度前也可);
  超时/通道关闭 → 返回 assumption 作为工具结果 + 卡上备注"1 问未答,按假设执行" + 审计 Warn。
- **交互豁免**:提问模式 = 从不 → 工具直接返回 assumption,不落队列。

### 3.3 问答应答端(通知集成)

- **notifications 新 kind `workflow_question`**:id = `wfq:{questionId}`;payload =
  `{questionId, workflowId, workflowName, taskId, nodeTitle, question, why, options, assumption, runStartedAt, createdAt}`;
  title = "工作流「X」任务「Y」提问",body = 问题文本。
- **新 command `workflow_question_respond(question_id, action, answer?)`**:action = `answer`(富回答)/
  `assume`(按假设继续)/`dismiss`;处理顺序:①resolve 通知(done/dismissed)②落档案条目
  (answer 原文,或"未回答,按假设继续:{assumption}")③若 waiter 存活 → take sender 发送
  (answer 原文;assume → assumption;dismiss → assumption)④审计 + `notifications-changed` 广播。
- **系统通知门铃**:落队列时发 OS 通知(复用 `notify_workflow_done` 的 plugin 通道),点击跳主窗口通知页。
- **失效回收**:`workflow_stop` / run 收尾 / 工作流删除时,把该 run 的 pending 问题批量 resolve 为 dismissed;
  run 存活与否不影响应答端写入(见拍板 7)。

### 3.4 澄清统一(workflow_clarify)

- **command**:`workflow_clarify(goal, attachments) -> ClarifyResult { questions, attempts }`;
  `ClarifyQuestion { id, question, why?, options, default }`;失败重试 1 次,再失败返回空 questions
  (outcome=degraded 审计,前端无缝直拆)——澄清是增强不是闸门。
- **契约段硬拼**:questions 为空 = 信息足够直接拆;≤3 问;**每问必带 default**(红线同源);
  why 一句话;不问 goal 已写明的事;只问一轮不得追问;选项 ≤4 个每个 ≤20 字。
- **校验纯函数 `parse_and_validate_clarify`**:剥 fences → JSON → 字段超限**截断**而非整体失败
  (结构坏才失败);default 缺失 → 取 options[0];default 与 options 双缺 → 丢弃该问(无法保证红线)。
- **前台展示**:hero 内联澄清卡组(ClarifyCard,四件套/折叠摘要/「开始拆解」永远可点/未答落 default);
  后台统一走通知(§3.3),同一队列两种门面。
- **模型**:轻量评审模型(W9b 落地设置项,过渡期跟随 active)。

### 3.5 workflow_decompose 扩展

- 签名 + `clarifications: Option<Vec<{question, answer}>>`:prompt 三段 = 指引 + 澄清记录(逐行 Q/A)+ 契约段;
  澄清记录同时落工作流档案(kind=clarify, source=user)。
- 输出契约 + `assumptions: Vec<String>`(≤5 × ≤60 字);`DecomposeSubtask` 不动;缺失不拒整包;
  GoalNode 下折叠条 `🤖 拆解假设`。
- 附件抽取块提为 `pub(crate) build_attachment_blocks` 共享 helper(clarify 复用同一封顶与占位口径)。
- **clarify_meta 持久化**:`workflows` 表 + `clarify_meta TEXT`(JSON:`{granularity, answers[], extra}`);
  重拆预填上次答案(拍板 3)。

### 3.6 W9b 小件

附件透明化(读取成功数/字符数回显)、粒度开关(粗/中/细,hero 三档 chip,随 clarify_meta 保存)、
模型分层设置(轻量评审模型:clarify 与 W10 验收核查共用)、度量埋点(§7)、重拆预填澄清答案。

## 4. W10-QA-AUDIT(不变,审计来源加强)

### 4.1 节点级验收 check

节点执行成功且 acceptance 非空 → 追加轻量评审调用(输入:标题+acceptance+summary+artifacts;
输出 `{"verdict":"pass|partial|fail","evidence":"≤100字"}`,解析降级 unknown 不阻断)。
fail → 带 evidence 返工 ≤2 次(独立预算,不与失败重试混用),用尽仍 fail → 终态 failed(下游跳过)+ 审计 Warn;
partial → 带徽标继续。verdict 并入 node result(`{status, acceptanceVerdict, acceptanceEvidence}`),
`node_is_success` 仍只看 status。用户主动停止不做验收。TaskNode 徽标 + evidence 进 TracePanel。
设置开关默认开;走轻量评审模型。

### 4.2 run 级结构化审计

新表 `workflow_audit`(id/workflow_id/run_started_at/node_task_id NULL/kind/level/payload JSON/created_at,
索引 (workflow_id, run_started_at));**不建 runs 实体表**,`workflow_id + run_started_at` 即 run 分组键。
kinds:run_start/node_start/node_result/acceptance_check/rework/review/rework_round/run_done/stop
**+ question_asked/question_answered(问答日志天然入审计)**。
写入尽力而为不阻断;bot.log 行式事件保持双写;保留最近 20 次 run(设置项)。
`workflow_audit_list` command + TracePanel「运行审计」页签 + 导出 JSON。

## 5. 数据 / API 变更汇总

| 层 | 变更 |
|---|---|
| command | +`workflow_clarify`、+`workflow_question_respond`、+`workflow_audit_list`(W10);`workflow_decompose`(+clarifications/attachment_texts/granularity,出参 +assumptions);`workflow_save/load`(+clarify_meta) |
| DB | +表 `brief_entries`(W9)、+表 `workflow_audit`(W10);`workflows` +`clarify_meta TEXT NULL` |
| AppState | +`question_waiters`(oneshot 注册表,照 confirm_requests 形态) |
| notifications | +kind `workflow_question` |
| 事件 | 无新增必须(问答走 notifications-changed;节点 verdict 走 tasks-updated) |
| 前端 | WorkflowPage hero 状态机、ClarifyCard.tsx、通知页问题卡、TaskNode/GoalNode/TracePanel、设置页、types.ts |

## 6. 校验与安全(上限表)

| 项 | 上限 |
|---|---|
| clarify 问题数 / question / why / options / default | ≤3 / 100 / 60 字 / ≤4×20 字 / 100 字 |
| 档案条目 text / reason;注入条数(全局/卡)/ 注入字数(全局/卡) | 200 / 80 字;6 / 8 条;1200 / 800 字 |
| 节点提问预算 / 等待超时 | ≤2 问 / 默认 24h(可配) |
| assumptions / clarify 重试 / 验收返工 | ≤5×60 字 / 1 次 / ≤2 次(独立预算) |
| 降级路径 | clarify 失败→直拆;提问超预算→返回假设;超时未答→按假设;验收解析失败→unknown;档案注入超限→截断;审计写失败→不阻断 |

## 7. 度量埋点(audit 事件)

`workflow_clarify {outcome, questions, answered, skipped}`、`workflow_question_asked {taskId, hasOptions}`、
`workflow_question_answered {action, latency}`、`workflow_decompose` 扩 `{granularity}`、
`workflow_acceptance {verdict, reworked}`(W10)、`workflow_lesson_recorded/recalled {count}`(教训闭环)。
跑两周期看:提问触发率/按假设继续率、澄清跳过率、验收返工率——用数据决定下一轮取舍。

## 8. 工程验收口径

- Rust 纯函数单测锚点:`parse_and_validate_clarify`(截断/缺 default 兜底/双缺丢弃/坏 JSON/空数组)、
  `brief_insert`(trim 截断)、`brief_for_injection`(有界/全局卡分层/空档 None)、
  `workflow_question_respond`(答→档案+唤醒;run 死→档案仍落;dismiss→assumption)、
  `parse_acceptance_verdict`(W10)、审计 payload 构造。
- vitest:hero 状态机、ClarifyCard(选项/假设/折叠/未答提示)、通知页问题卡回答流。
- 门禁 `scripts/test-fast.sh`;批次 spec 按 docs/batches 制度(W9-ASK 先行),OCR 复审照旧。

## 9. 非目标(本轮不做)

主 agent 调度(拍板否决)、LLM 异常重排(C 方案)、节点级输出契约、"需人工"卡标记、重拆 diff 视图、
档案导出 Markdown(UI 后置)、token 成本汇总(现成基建不满足则后置)、大纲视图(独立排期)。

## 10. 待拍板默认值

1. 提问等待超时 **24h**(定时场景下一轮调度前未答即按假设,不阻塞下轮)。
2. 提问模式默认 **关键决策才问**;验收 fail 用尽返工 → **终态 failed、下游跳过**(与现有失败语义一致)。
3. 审计保留 **20 次 run**;档案注入 **全局 6 条/卡 8 条**。
