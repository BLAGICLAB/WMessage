# Agent 运行时改造方案（2026-11-02）

> **状态：方案，未落地。** 本文不含任何代码改动。
> 落地时每批一个提交，并按 [§九 文档同步清单](#九文档同步清单)同步 SPEC / 验收矩阵 / 验收卡。
>
> **依据**：
> 1. `deepseek-ai/dsh-superagent-prototype` 与 `deepseek-ai/deepseek-harness` 源码走查（2026-11-02）
> 2. 本仓三路静态审计：状态写入与副作用、执行单元与面板呈现、事务边界与幂等
> 3. `SPEC.md` 全文、`docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md`、`docs/acceptance/matrix.csv`
>
> **口径**：本文所有论断均带 `文件:行号` 证据。凡写「未确认」的，是本次审计没有取到证据，
> 不是「不存在」。行号以本文写作时的 `main` 为准。

---

## 〇、本方案的取舍原则

superagent 是面向**多用户团队、跨信道、跨设备**的系统设计；wmessage 是**单机、单人、本地优先**
的个人任务助手（`SPEC.md:38-40`）。照抄形态会引入不必要的复杂度。

> **借 superagent 的「状态机与幂等纪律」，不借它的「组织形态」。**

**落地的判据只有一条**：superagent 解决的问题，wmessage 是否**真的**有这个问题。
本文 [§二](#二已实现清单照抄-superagent-会重复造的东西) 先列「已经有了的」，
[§三](#三缺口清单带证据) 再列「真的缺的」——**只有 §三 里的才进方案**。

---

## 一、superagent 的能力盘点（供对照）

调研对象是两个仓库，不是同一个：

| 仓库 | 角色 | 规模 |
|---|---|---|
| `deepseek-ai/deepseek-harness`（DSH） | 底座：模型外的 Agent 运行时 | pnpm monorepo，60+ package group |
| `deepseek-ai/dsh-superagent-prototype` | 原型：跑在 DSH 上的一个 bundle | 1.56 万行 TS |

superagent 的核心机制与本仓的对照见 [§八 证据索引](#八证据索引)。

---

## 二、已实现清单（照抄 superagent 会重复造的东西）

> **本节是本文最重要的一节。** 下面的能力 wmessage **已经有了**，
> 对 superagent 方案的机械照搬会造出第二套实现、两套口径。

### 2.1 幂等

| superagent 的做法 | wmessage 的现状 | 证据 |
|---|---|---|
| Trigger ID 内容派生，天然幂等 | `spawn_subagent` 服务端派生语义幂等键：**父会话 id + 档位 profile + 目标 objective + 验收标准 acceptance_json + 指定模型 model** 五字段逐字节比较，且只对**活着**的行生效 | 判定 `src-tauri/src/bot/subagent_spawn.rs:123-142`；SQL `src-tauri/src/db/subagents.rs:339-344` |
| Outbox 回执 `INSERT OR IGNORE` 永久保留 | `notifications` 表 `INSERT OR IGNORE` + 业务派生 id（`artifact:{task_id}:{sid}` / `memory:{session}:{ts}` / `{serviceId}:{alertType}`） | `src-tauri/src/notifications.rs:73-99`；`src-tauri/src/bot_chat/exec.rs:532`；`src-tauri/src/memory/extract.rs:725` |
| Effects 的 `(kind,key)` 唯一 | `migration_journal` 的 `(task_id, src)` 至多一条 pending | `src-tauri/src/migration/journal.rs:28-70` |
| — | `app_versions` 的 `UNIQUE(app_id, version)` | `src-tauri/src/db/apps.rs:58` |

### 2.2 事务

**wmessage 有事务**，而且用在了关键处。`conn.transaction()` 生产代码 28 处 + `unchecked_transaction()` 10 处，
覆盖：`bot_history`（INSERT + `UPDATE bot_sessions.updated_at` 同事务，`db/bot_history.rs:159-190`）、
`apps` 三表同事务删（`db/apps.rs:149-161`）、`scheduled_job_runs` 插入 + 裁剪同事务（`db/schedule_jobs.rs:260-285`）、
建会话 + 任务块同事务（`bot_chat/exec.rs:185-204`）、子 agent 收尾多写同事务（`bot/subagent_runner.rs:783-810`）、
子 agent 插行同事务（`bot/subagent_spawn.rs:145-176`）。

**真正的缺口不是「没有事务」，而是「写串行 ≠ 写原子」**——见 §3.1。

### 2.3 「崩了能接着跑」

`migration_journal` 是**全仓唯一同时具备三要素**的结构：

1. **持久化**：DB 表 `src-tauri/src/db/mod.rs:191-199`
2. **幂等键**：`(task_id, src)` 至多一条 pending `src-tauri/src/migration/journal.rs:41-67`
3. **崩溃后续跑**：启动 replay 扫 pending `src-tauri/src/migration/recovery.rs:105-125`

这正是 superagent Effects 的内核。**本方案以它为原型抽通用队列，不是凭空造抽象。**

### 2.4 在途与恢复

| superagent | wmessage |
|---|---|
| 仅追加 session log + 恢复 / 分叉 / 重放 | **回合开始即落库**（用户消息先进库，崩了/退出也不丢）+ 回收集尾再写一次最终态（`SPEC.md:74`） |
| 事件序号 + 回放缓冲 | 会话级流式事件带单调递增 `seq` + 在途登记 + 回放缓冲（条数与字节双上限，游标老于窗口时如实报 `truncated`）（`SPEC.md:75`；`src-tauri/src/bot/stream_log.rs`） |
| 投递回执 | `notifications` 表 status 三态 pending/done/dismissed（`src-tauri/src/notifications.rs:29-31`） |

### 2.5 验收判据

`file_exists:` / `file_nonempty:` 机器可判定验收，**派发即验形状、跑完才验存在性**：

- 谓词解析 `src-tauri/src/bot/subagent_digest.rs:292-306`
- 判定 `src-tauri/src/bot/subagent_digest.rs:318-355`（fail-closed：目录判 false、空文件判 false）
- 派发期形状校验 `src-tauri/src/bot/subagent_spawn.rs:50-59`、`:67-100`
- 跑完判定 `src-tauri/src/bot/subagent_runner.rs:704-711`
- 服务端结论覆盖模型自评 `src-tauri/src/bot/subagent_digest.rs:358-400`

### 2.6 预算与熔断

轮数 / 工具调用两轴安全网（`MAX_ROUNDS=200` / `MAX_FUNCTION_CALLS=400`，主子共用，`src-tauri/src/bot/constants.rs:23,25`）、
90% 软告警 / 95% 收尾窗（只放行写产物类工具）、`read_own_card` 实时余量、token 四档记账、
轮数口径单源（`SPEC.md:152,162`）。

### 2.7 上下文

`Inline & Spill` 已有：未超预算原样内联、超预算落 `AI_Gen_Files` 溢出目录 + 回灌指针 + 30 天保留期清扫
（`src-tauri/src/bot_model_loop/output.rs:97-126`、`src-tauri/src/overflow.rs`）；
压缩侧另有 sidecar 旁路（`src-tauri/src/bot/compaction.rs:172-202`）；
一跳召回 `lookup_receipt`（`src-tauri/src/bot/tools/trace.rs:25-29`）。

### 2.8 权限与闸

8 级闸序固定在 `execute_tool_impl`（`src-tauri/src/bot/dispatch.rs:522-800`），
早退点 8 处由 `dispatch/pre_tool_hook_tests.rs` 计数断言锁住；
闸的分布与强制缺口已由 `docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md` 盘清（含 §五 强制缺口清单）。

---

## 三、缺口清单（带证据）

> **本节只列有证据的缺口。** 每条都注明「证据」与「性质」
> （性质分：**结构性缺口** = 缺一个概念；**语义缺口** = 概念在但口径不对；
> **可见性缺口** = 实现有但用户看不到）。

### 3.1 G1｜写串行 ≠ 写原子（结构性缺口）

**证据**：全仓 `lock_db_write()` 取锁点 **110 处**，开事务 **38 处**。
`DB_WRITE_LOCK`（`src-tauri/src/db/mod.rs:362`）保证的是**进程内写者互斥**，
`with_write_conn`（`src-tauri/src/db/lock.rs:43-58`）只做「锁 + 连接」，**不开事务**，
事务必须调用方自己在闭包里 `conn.transaction()`。

具体裂缝（全部已逐行核实）：

| # | 位置 | 裂缝 | 崩溃后留下什么 |
|---|---|---|---|
| 1 | `src-tauri/src/db/tasks.rs:306-426`（`:387` 逐条 `stmt.execute`） | `upsert_tasks` 批量卡**全程无事务**，RMW 冲突检查在每条 execute 之前 | 第 3 张卡冲突时前 2 张已提交、第 3 张未写；函数整体返 `Err`，调用方以为「全失败」——**实际是部分成功** |
| 2 | `src-tauri/src/bot_scheduler.rs:604-616` 与 `:649-661` | `record_job_run` 与 `update_job_run_state` 两次独立 `with_write_conn`（两次 open_db、两次事务） | `scheduled_job_runs` 有 fail 行，但 `scheduled_jobs.retry_at` 仍为空 → **失败不重试**，状态徽标还是上次的值 |
| 3 | `src-tauri/src/apps/mod.rs:104-108` 与 `:197-212` | `record_app_run` 与 `finish_app_run` 两次独立取锁 + 独立连接 | `app_runs` 行永远停在 `running`（用户以为还在跑），只能靠 `apps/mod.rs:287` `sweep_stale_runs` 时间阈值兜底 |
| 4 | `src-tauri/src/bot/subagent_spawn.rs:145-176` → `src-tauri/src/bot/subagent_runner.rs:245-251` → `:344-353` | 插行 / 置 Running / 建会话+回写 session_id **三段独立事务** | 行存在但无 `session_id`，父 agent `check` 拿到空会话，`check_subagent` 永远查不到进度 |
| 5 | `src-tauri/src/bot_reminders.rs:352-353` | `fire(app, &r)`（OS 通知 = 外部副作用）后 `delete_row(app, &r.id)` 分两步 | 崩溃在两者之间 → 下个 tick **重复发同一条提醒** |

> 裂缝 1 与 5 是本方案**批 1** 的直接目标；2/3/4 涉及「外部副作用 + 状态写」的两段式，
> 只能靠 [§五 主线二](#五主线二统一持久化副作用队列effects)的队列解决，不能靠事务——
> 事务管不了「通知发出去之后进程死了」。

### 3.2 G2｜副作用无统一持久化待办，6 个通知发送点全无重试（结构性缺口）

**证据**：

- 全仓 28 张表**没有 queue / outbox 类表**（唯一具备三要素的 `migration_journal` 只服务文件迁移一个子系统）。
- 系统通知 6 个发送点，**全部无重试**：

| 位置 | 失败处理 | 用户可见性 |
|---|---|---|
| `src-tauri/src/bot_scheduler.rs:37-45`（作业完成/失败） | 只 `eprintln!` | **不可见**（不进审计、不进 UI） |
| `src-tauri/src/bot_scheduler.rs:52-60`（错过补跑） | 只 `eprintln!` | **不可见** |
| `src-tauri/src/bot_reminders.rs:369-394` | 记审计 `reminder.fire_failed` | 仅审计 |
| `src-tauri/src/due_notify.rs:253-274` | 记审计 `due_notify_fail`；且标志已在 `plan_round` 置为「已发」 | **静默丢弃**（注释 `:263` 明说「避免每 30s 重试刷屏」） |
| `src-tauri/src/ask_engine.rs:518-523` | `let _ = ....show()` | **返回值直接丢弃，连审计都没有** |
| `src-tauri/src/apps/commands.rs:113-118` | `let _ = ....show()` | **静默丢弃** |

> `bot_scheduler.rs:49-50` 的注释写着「cron 监控核心教训：静默失败最危险」——
> **项目自己已经识别了这个风险**，但两条发送点仍只 `eprintln!`。
> 本方案不是新增风险，是把已识别的风险关掉。

### 3.3 G3｜主 agent 的活没有执行单元（结构性缺口）

**证据**：

- `exec_traces` 模块**自述定位**：「**append-only 观测面——只增与收尾，不参与任何业务判定**」
  （`src-tauri/src/db/trace.rs:1-12`）。
- 主聊天建 trace 时 `task_id` 与 `title` **均为 `None`**：
  `begin_trace(&trace_app, sid, None, "chat", None)`（`src-tauri/src/bot_chat/flow.rs:636-643`）。
  ActivityPage 只能显示 `t.title || t.sessionId`（`src/components/ActivityPage/index.tsx:145`）——
  **回落到会话 id**。
- `exec_traces` 无 `objective`、无 `acceptance`、无授权范围、无阶段列（DDL `src-tauri/src/db/trace.rs:68-95`）。
- 机器可判定验收**是子 agent 独占**：`eval_machine_criteria` 全仓唯一生产调用点是
  `src-tauri/src/bot/subagent_runner.rs:704-711`，其输入 `acceptance_criteria_of(&row)` 签名是 `&SubagentRow`
  （`src-tauri/src/bot_orchestrator.rs:525-530`）；`acceptance_json` 列只存在于 `subagents` 表
  （`src-tauri/src/db/subagents.rs:155`）。

**结论**：用户说「把这批 PDF 转成 Excel」，这件事在系统里**没有身份、没有目标、没有验收、没有阶段**，
只落成一条 `origin="chat"` 的 trace 加一串消息。

### 3.4 G4｜三套状态语义互不映射，「等人回答」不在工作行上（结构性缺口）

| 语义 | 取值 | 位置 |
|---|---|---|
| 子 agent 生命周期 | 六态 Queued/Running/Succeeded/Failed/Cancelled/BudgetExceeded + 合法转移表 | `src-tauri/src/db/subagents.rs:18-25`、`:97-105` |
| 用户看板 | 三列 Todo/Doing/Done | `src-tauri/src/db/tasks.rs:77-96` |
| 执行结局 | 四态 running/done/failed/stopped | `src-tauri/src/db/trace.rs:69-73`（唯一写口 `src-tauri/src/trace_sink.rs:274-281`） |

三者**无交叉映射代码**。且：

- 「等人回答」不是阶段，是旁路的一条 pending 通知
  （`src-tauri/src/ask_engine.rs:470-473`，`kind="task_question"`），**不挂在任何工作行上**。
- `SubagentStage`（recon/executor/verifier，`src-tauri/src/bot_orchestrator.rs:87-110`）**不落库**——
  `subagents` DDL 无 stage 列（`src-tauri/src/db/subagents.rs:145-168`）。

### 3.5 G5｜执行现场不可恢复，且已有代码写明了原因（结构性缺口）

`sweep_zombie_subagents`（`src-tauri/src/bot_orchestrator.rs:561`）在启动时把
`Queued → Cancelled`、`Running → Failed`（`:577-582`）。

**为什么不做恢复，代码里写得很清楚**（`src-tauri/src/bot_orchestrator.rs:555-557`）：

> 「…一起还原，而这两项没落在行上（行只有 profile / model）。只按 profile 重投会跑出一个
> 「用默认角色和默认配置完成」的成功结果，比取消更难发现。**真要恢复，先把 stage / onion 持久化，再谈重投。**」

这段注释是本方案的**直接依据**：恢复的阻塞点是「stage / onion 没落库」，
而 superagent 的 Effects + Ticket 契约正是解这个的。

### 3.6 G6｜「需人处理」的聚合面只覆盖通知表一个数据源（可见性缺口）

| 视图 | 是否按「需人介入」组织 | 证据 |
|---|---|---|
| 通知中心 | **是**（唯一）：pending 页签 + 角标 | `src/components/NotificationsPage/NotificationsPage.tsx:420-426`、`:405-409` |
| 首页「通知」面板 | 是（同一数据源，截 3 条） | `src/components/HomePage/HomePanels.tsx:165-193` |
| 活动页 | **否**：只有 origin 筛选，**无状态筛选** | `src/components/ActivityPage/index.tsx:22-28` |
| 右侧栏「子 Agent」 | 否：六态平铺，failed / budget_exceeded 与 succeeded 同列 | `src/components/ChatPanel/ContextRail.tsx:670+` |
| 定时作业页 | 否：失败徽标行内显示，无失败集合视图 | `src/components/SchedulePage/SchedulePage.tsx:47`、`:237-269` |
| 首页「机器人动态」 | 否：全 trace 列表 + 成功率 | `src/components/HomePage/HomePanels.tsx:100-145` |

通知中心只覆盖 4 类 kind：`memory_proposal` / `artifact_bind` / `task_question` / `service_alert`
（`src-tauri/src/notifications.rs:33-42`）。
**子 agent 的 failed / budget_exceeded、trace 的 failed / stopped、定时作业失败、待验收项——没有任何视图集中列出。**

### 3.7 G7｜定时作业与批量执行无并发闸（结构性缺口）

全仓并发闸只有三处：`py` 沙箱 `PY_RUN_MAX_CONCURRENT=2`（`src-tauri/src/py/gate.rs:16`）、
受管服务 `MAX_CONCURRENT_SERVICES=4`（`src-tauri/src/bot/services.rs:50`）、
子 agent 全局 3 / 每会话 2（`src-tauri/src/bot/constants.rs:250,252`）。

**定时作业没有闸**：`jobs_tick` 对每个到点作业调 `spawn_job_run`（`src-tauri/src/bot_scheduler.rs:853`），
后者是**非 async 函数**，内部 `tauri::async_runtime::spawn`（`src-tauri/src/bot_scheduler.rs:710-716`）。
调度循环本身不被阻塞（这点是对的），但**同一 tick 内可派发任意多个作业，每个都是一整套
`MAX_ROUNDS=200` / `MAX_FUNCTION_CALLS=400` 的模型循环**。到点集中时并发无上限。

### 3.8 G8｜没有干跑 / 预演（结构性缺口）

全仓 `dry_run` / `dryRun` / `observe_mode` / `would_accept` / `pretend` / `simulate` **零命中**。
定时作业到点直接执行（`src-tauri/src/bot_scheduler.rs:519-533` → `run_task_in_chat`）。

### 3.9 G9｜`tool_call_id` 无执行去重（语义缺口）

`exec_spans.tool_call_id` 是普通可空列，**无 UNIQUE**（`src-tauri/src/db/trace.rs:95-108`），
`span_insert` 是裸 INSERT（`:396-416`）；模型循环逐个执行 tool_calls
（`src-tauri/src/bot_model_loop/core.rs:762`），dispatch 侧无前置查重
（`src-tauri/src/bot/dispatch.rs:522-538`）。

同一 `tool_call_id` 执行两次 = 工具真跑两次 + `exec_spans` 落两行。

> **注意边界**：模型重发**不同** id 的同名工具**不该去重**（那是两次不同的意图）。
> 去重只在「同一 id 重复投递」时生效——那是重试/重放的场景。

---

## 四、方案总览

四条主线，按「证据强度 × 收益 ÷ 风险」排序：

| 主线 | 消解缺口 | 性质 | 批次 |
|---|---|---|---|
| **一、写路径原子化** | G1 | 结构性 | 批 1 |
| **二、统一持久化副作用队列（Effects）** | G2、G1-裂缝2/3/4 | 结构性 | 批 2–4 |
| **三、主 agent 执行单元（WorkItem）与阶段语义** | G3、G4、G5 | 结构性 | 批 5–6 |
| **四、「需人处理」聚合面与干跑模式** | G6、G8 | 可见性 + 结构性 | 批 7–8 |

**不动的**：G7（并发闸）、G9（tool_call_id 去重）单列为可选批次，见 [§七](#七不做清单与可选项)。

---

## 五、主线一：写路径原子化

### 5.1 目标

把「写串行」升级为「写原子」：**一次业务操作的多条写，要么都成，要么都不成。**

### 5.2 做法：给 `with_write_conn` 加一层事务包装

新增 `src-tauri/src/db/tx.rs`：

```rust
/// 在 DB_WRITE_LOCK + 连接之上再开一层事务：闭包返回 Err 即回滚。
/// 与 with_write_conn 的区别只在「是否开事务」——锁粒度、连接语义、错误透传一致。
/// 幂等/去重语义仍由调用方负责（事务不提供业务级幂等）。
pub(crate) async fn with_tx<R, F, T>(app: &AppHandle<R>, f: F) -> Result<T, String>
where
    R: tauri::Runtime,
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
```

**约束**：
- 事务里**不得** `await`（SQLite 事务不可跨 await 持有）——闭包是同步的。
- 事务里**不得**起外部副作用（发通知、起子进程）——那属于主线二。
- 事务时长上限建议 100ms；超过要拆。

> **为什么不用 superagent 的 `repo.atomic()` 名字**：本仓已有 `with_write_conn` 样板约定
> （`src-tauri/src/db/lock.rs:1-16` 记录的收敛历史），新包装按本仓命名走 `with_tx`，
> 不引入外来词汇。

### 5.3 逐条修裂缝

| 裂缝 | 修法 |
|---|---|
| 1 `upsert_tasks` 部分提交 | 改用 `with_tx`：整批卡一个事务。**冲突检查也要移进事务内**——现在检查在 execute 之前逐条做（`db/tasks.rs:344-365`），移进事务后仍是先查后写，但同事务内不会半提交 |
| 2 job_run 与 job 状态分家 | `finalize_job_run` 的「插 run 行」+「写 job 状态」合并为**一个事务**（`bot_scheduler.rs:604-616` + `:649-661`） |
| 3 `app_runs` 停在 running | 「建 run 行」+「起进程」是两段（进程是外部副作用）→ 进程启动失败时**在同一事务内把 run 行改 failed**，不留 running；真正的「进程起了但收尾没跑」仍靠 `sweep_stale_runs` 时间阈值 |
| 4 子 agent 行与会话脱钩 | 插行 / 置 Running / 建会话+回写 session_id 合并为一个事务（`subagent_spawn.rs:145` + `subagent_runner.rs:245` + `:344`） |
| 5 reminder 先发后删 | **顺序对调**：先删行（事务内）再发通知。语义从「可能重复发」换成「可能漏发」——**提醒重复发比漏发更扰民**，取漏发。丢失的提醒由「周期提醒下一轮照常」兜底 |

### 5.4 改造后提升

1. **「点了没反应」类问题根除**：`upsert_tasks` 的部分成功（函数返 Err 但前两张卡已写）
   是最难排查的一类不一致，收进事务后不再存在。
2. **定时作业的失败重试不再丢**：job_run 与 job 状态同事务后，
   「有 fail 行但 retry_at 为空 → 不重试」这个洞关闭。
3. **崩溃后不留半写状态**：主 agent、app 运行、子 agent 三处的「存在但残缺」行消失。
4. **提醒不再重复轰炸**：顺序对调后，一轮崩溃最多漏一条，不会每次开机补一条。

### 5.5 验收

- `upsert_tasks` 新增用例：批内第 N 条冲突 → **整批不落库**（当前行为是前 N-1 条已落）
- `finalize_job_run` 新增用例：run 行写入成功后 job 状态写入失败 → **两者都不留**
- `bot_reminders` 新增用例：`fire` 抛错 → 行已被删（不再下轮重复发）
- 子 agent：三段写中任一段失败 → 行不存在
- 全部走 `scripts/test-all.sh`

---

## 六、主线二：统一持久化副作用队列（Effects）

### 6.1 目标

**副作用必达**：任何「状态已写、但外部动作还没做」的中间态，都必须能被恢复。

### 6.2 做法：把 `migration_journal` 泛化成通用队列

**这是本方案里唯一「抽象」的引入，因此必须满足 AGENTS.md 的门槛：第二个调用方出现前不抽象。**
本方案的调用方序列是：

1. `migration_journal`（**已存在**，第一个调用方）
2. 系统通知投递（第二个）
3. 定时作业失败重试（第三个）
4. 子 agent 终态写库 / runner 派发（第四个）

即：抽象在**迁移 migration 时**就已经有第二个调用方，**不违反「不为以后可能预留抽象」**。

### 6.3 队列内核

新增 `src-tauri/src/db/effects.rs`，从 `migration/journal.rs` 抽出：

```sql
CREATE TABLE IF NOT EXISTS effect_queue (
  kind        TEXT NOT NULL,
  key         TEXT NOT NULL,
  payload     TEXT NOT NULL,
  seq         INTEGER NOT NULL,     -- 队列位置，同 kind+key 再入队不插队
  generation  INTEGER NOT NULL DEFAULT 1,  -- payload 变更计数，派发时 CAS
  attempts    INTEGER NOT NULL DEFAULT 0,
  due         INTEGER NOT NULL,     -- 到期时间戳（ms）
  error       TEXT,
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (kind, key)
);
CREATE INDEX IF NOT EXISTS idx_effects_due ON effect_queue(kind, due, seq);
```

**四条语义**（照搬 superagent `src/app/effects.ts`，那是最干净的一块）：

| # | 语义 | 说明 |
|---|---|---|
| 1 | 同 `kind+key` 只有一行；再次入队**保留队列位置**（不插队），payload 变了则 `generation+1`、`attempts` 归零 | 队列位置是「先来先做」的承诺，不能被后来的同 key 抢占 |
| 2 | 派发用 `kind+key+seq+generation` 四元组 CAS | 跑动中的行不会被新 payload 覆盖 |
| 3 | 抛出 `Later` 异常 = 「现在跑不了」（暂停中 / 无槽位 / 依赖未就绪）→ **只顺延 `due`，不计失败次数** | 暂停不该消耗重试配额 |
| 4 | 普通失败 → `attempts+1`，退避 `min(cap, base × 2^attempts)` | 指数退避 |

> **语义 3 是 superagent 最值得抄的一条**：wmessage 现在的重试把「暂停」和「真失败」混为一谈。

### 6.4 收编范围（按批次）

| 批次 | 迁入的 kind | 现状 | 迁后 |
|---|---|---|---|
| 批 2 | `migration` | `migration_journal`（自研三要素实现） | 改用通用队列内核，**行为不变**（含启动 replay） |
| 批 3 | `notify` | 6 个发送点全无重试、4 处静默 | 统一走队列：有回执、指数退避、失败进健康面板、**绝不静默** |
| 批 4 | `schedule_retry` / `subagent_finalize` / `subagent_dispatch` | 定时作业重试固定 5 分钟且写两处；子 agent 终态写库是**内存内**指数重试（`src-tauri/src/bot/subagent_runner.rs:41-77`，`RETRY_DELAYS_MS=[1s,2s,4s,8s,16s]`，**不持久化**）；runner 派发是 `spawn` 一次性 | 三者进队列：崩溃后自动续跑；`subagent_dispatch` 进队列后，G5 的「插行了但 runner 没起」自动消解 |

**「定时作业固定 5 分钟退避」不改**：那是明确的产品决策
（`src-tauri/src/db/schedule_jobs.rs:68-69`：「固定 5 分钟退避：不做指数退避——本地单机场景固定间隔足够可预期」）。
队列承载它但不改变间隔语义。**语义 4 的指数退避只用于新收编的 kind**。

### 6.5 改造后提升

1. **通知不再静默丢**：`ask_engine.rs:518` 那种 `let _ = ....show()` 的丢弃被消除——
   发送失败会重投，且最终失败会出现在健康面板（不是只进 stderr）。
2. **崩溃后副作用必达**：进程在「状态已写、动作未做」之间死掉，重启后自动补上。
   这直接消解 G1 的裂缝 2/3/4。
3. **重试语义统一且正确**：「暂停」不再消耗重试次数（语义 3）。
4. **一个视图看全后台工作**：`SELECT kind, count(*), max(attempts) FROM effect_queue GROUP BY kind`
   就是全部待办副作用的健康度。现在分散在 6 个发送点 + 2 个重试点，得分别查。
5. **子 agent 派发不再丢**：G5 的第一个分支（queued 行被清扫成 cancelled）根源消除。

---

## 七、主线三：主 agent 执行单元（WorkItem）与阶段语义

### 7.1 目标

让「主 agent 自己在干的活」有一等表达：**是什么活、多大授权、做到哪一步、机器怎么判完成、谁在等它。**

### 7.2 表设计

```sql
CREATE TABLE IF NOT EXISTS work_items (
  id             TEXT PRIMARY KEY,      -- wi_{uuid}
  kind           TEXT NOT NULL,         -- chat | task | scheduled
  origin_ref     TEXT,                  -- task_id / job_id / session_id
  session_id     TEXT,
  objective      TEXT,                  -- 这活要干什么
  acceptance     TEXT,                  -- JSON：机器可判据（复用 file_exists:/file_nonempty: 语法）
  stage          TEXT NOT NULL,         -- 见 7.3
  mandate        TEXT,                  -- JSON：授权范围与来源
  result         TEXT,                  -- JSON：结构化结论
  error          TEXT,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_work_items_stage   ON work_items(stage);
CREATE INDEX IF NOT EXISTS idx_work_items_session ON work_items(session_id);
```

### 7.3 阶段语义（person 族 / working 族二分）

照搬 superagent 的 `ticketStage` 思路（`docs/007-architecture.md`），但词汇对齐 wmessage：

| stage | 条件 | 面板中文 |
|---|---|---|
| `undelivered` | 输入投递到执行会话失败 | 输入未送达 |
| `awaiting_answer` | 有待答问题 | 待你回答 |
| `awaiting_review` | 报告就绪待验收 | 待验收 |
| `blocked` | 报告受阻，且此后无人消息 | 受阻 |
| `failed` | 报告失败，且此后无人消息 | 失败 |
| `queued` | 无派工 | 排队中 |
| `idle` | 报告 `waiting` | 等待新指令 |
| `working` | 其余 | 进行中 |

**`person` 族 = 没有用户介入就动不了**；`working` 族 = 自己在推进。
这个二分是「面板只显示需要人的」的基础。

### 7.4 与既有表的边界（**不合并**）

| 表 | 定位 | 与 work_items 的关系 |
|---|---|---|
| `subagents` | 派给**子 agent** 的活：目标 / 验收标准 / 授权档位 / 生命周期状态机 / 结构化结果（`db/subagents.rs:107-168`） | **并存，不合并**。子 agent 派工链已经很完整（幂等键、机器验收、digest 修复），动它风险高且无收益 |
| `tasks` | 用户的**看板卡片**（三列 Todo/Doing/Done） | **并存，不合并**。用 `origin_ref` 关联；UI 上卡片可显示工单摘要 |
| `exec_traces` | **观测面**（自述「不参与任何业务判定」，`db/trace.rs:1-12`） | **并存，不合并**。trace 的定位就是观测；升格它会破坏 30 天保留期清扫与活动页口径 |
| `notifications` | 待用户**决策**的消息 | **并存**。主线四在视图层聚合，不改它的表 |

### 7.5 复用而不是重造：机器可判据

**不自造第二套判据引擎**。把 `src-tauri/src/bot/subagent_digest.rs` 的判据部分从
「吃 `SubagentRow`」泛化为「吃 `&[String]`」：

```rust
// 现状：pub(crate) fn eval_machine_criteria(...) 输入是 &SubagentRow
// 改为：pub(crate) fn eval_machine_criteria(criteria: &[String], ...) -> ...
// 调用方：subagent_runner.rs:708（传 acceptance_criteria_of(&row)）
//         work_items 的验收判定（传 acceptance_json 解析结果）
```

判据语法（`file_exists:` / `file_nonempty:`）、fail-closed 语义、审计事件
（`subagent.machine_check_failed` / `subagent.machine_check_passed`）全部沿用。

### 7.6 开票与推进时机

| 时机 | 动作 |
|---|---|
| 主 agent 收到一条**判定为要干活**的消息 | 建 `work_items` 行（`kind=chat`），`objective` 取首条用户消息摘要；`stage=working` |
| 模型在首轮给出方向后 | 模型可补全 `acceptance`（复用现有 acceptance 语法，提示词侧加一句） |
| 每轮报告 | 更新 `stage`（受阻/失败/就绪等） |
| 轮结束 | 落 `result` JSON、`stage` 收尾 |
| `ask_user` 提问 | `stage=awaiting_answer`，**并把 `notifications` 那条 pending 与 work_item 关联**（解决 G4 的「不在工作行上」） |

### 7.7 改造后提升

1. **主 agent 的活有了身份与目标**：ActivityPage 不再回落到会话 id（`ActivityPage/index.tsx:145`），
   能显示「这活要干什么」。
2. **主 agent 平权获得机器可判定验收**：当前是子 agent 独占（`subagent_runner.rs:704-711` 唯一调用点），
   改后主 agent 也能声明 `file_exists:` 判据，交付有机器判据而不是只有模型自述。
3. **三套状态语义收敛为一套**：G4 的六态 / 三列 / 四态各自定义、互不映射问题得到缓解——
   `work_items.stage` 成为**唯一横跨「排队 / 进行中 / 需要人 / 结局」的语义**，
   既有三套保持原样（各自有用户界面），但新面板按 `stage` 组织。
4. **执行现场可恢复（解 G5）**：只要 `objective` + `acceptance` + `stage` + `mandate` 落库，
   崩溃后就能像 superagent 那样「按恢复提示重启」——
   这正是 `bot_orchestrator.rs:555-557` 注释里说的「先把 stage / onion 持久化，再谈重投」。
5. **「等人回答」挂在工作行上**：不再是一条孤立的 pending 通知。
6. **面板能只显示需要人的**：按 `stage ∈ {blocked, failed, awaiting_answer, awaiting_review, undelivered}` 聚合。

---

## 八、主线四：「需人处理」聚合面与干跑模式

### 8.1 聚合面（消解 G6）

**不新建表**，在视图层聚合：

| 位置 | 动作 |
|---|---|
| 通知中心 | 「待处理」页签**增补**一个来源：`work_items` 中 stage 需要人介入的行 |
| 活动页 | 增补**状态筛选**（现在只有 origin 筛选，`ActivityPage/index.tsx:22-28`） |
| 右侧栏「子 Agent」 | 失败的子 agent（failed / budget_exceeded）**提级**到通知中心，不再与 succeeded 同列 |
| 定时作业页 | 失败的作业提级到通知中心 |

### 8.2 干跑模式（消解 G8）

给定时作业加 `mode` 三档（照搬 superagent `src/policy.ts:84` 的 `off / observe / auto`）：

| 档 | 行为 |
|---|---|
| `off` | 不派发（现有 `enabled=false` 语义保留） |
| `observe`（新增，默认建议） | 到点**只生成提案不执行**：建一张卡 + 写清「本会做什么」，用户确认后才真跑 |
| `auto` | 现有行为：到点直接执行 |

**干跑不是「模拟执行」**——不跑模型、不跑工具、不产生副作用，只把「要做什么」显式化。
superagent 的 observe 模式记录的是「模型认为可以接单 + 接了会做什么」。

### 8.3 改造后提升

1. **一处看全需要人处理的事**：不再需要在活动页 / 子 agent 栏 / 作业页 / 通知中心之间来回切。
2. **定时作业上线前可先看后跑**：把「AI 每晚 2 点要做什么」从黑盒变成白纸。
3. **失败不再需要用户主动去查**：失败项自动出现在通知中心角标里。

---

## 九、批次切分与依赖

> 每批一个逻辑变更、一次提交。行为变更换测试。提交前人眼看 diff。

| 批 | 内容 | 依赖 | 用户可见行为变化 | 规模 |
|---|---|---|---|---|
| **1** | `db/tx.rs` 事务包装 + 修裂缝 1（`upsert_tasks`）、2（job 状态合并）、5（reminder 顺序对调） | — | 无 | 中 |
| **2** | 通用 Effects 队列内核（从 `migration_journal` 抽）+ migration 改用它 | 1 | 无（migration 行为不变） | 中 |
| **3** | 6 个通知发送点收进队列 `notify` kind + 失败进健康面板 | 2 | **有**（通知失败不再静默） | 中 |
| **4** | `schedule_retry` / `subagent_finalize` / `subagent_dispatch` 收进队列 | 2,3 | 无（间隔语义不变） | 中 |
| **5** | `work_items` 表 + `stage` 判定 + 主 agent 开票与推进 | 1 | 轻微（活动页多出工作项条目） | 大 |
| **6** | 机器判据引擎泛化 + 主 agent 支持 `acceptance` | 5 | 轻微（交付多出机器验收结论） | 中 |
| **7** | 面板聚合：通知中心增补来源 / 活动页状态筛选 / 失败提级 | 5 | **有**（面板重组） | 中 |
| **8** | 定时作业干跑模式（off/observe/auto） | 2,5 | **有**（作业页多一档） | 中 |

**批 1–4 是「不改用户可见行为」的加固**，可先合；
**批 5–6 是最大的结构改动**；**批 7–8 直接改善体感**。

---

## 十、不做清单与可选项

### 10.1 明确不做

| 不做 | 理由 |
|---|---|
| 拆 Router / Worker 双层 | wmessage 主 agent 本来就要动手做事；已有的子 agent 隔离（`SubagentProfile` 工具白名单 + 全局 3/每会话 2 闸）就是 Worker 层。再拆一层只增 token 与跳转 |
| 准入表（谁能派活） | 单人产品没有「群里谁能指挥 agent」的群体问题 |
| 链式召回 | 单人单库，历史量级与 superagent 差两个数量级；一跳 `lookup_receipt` 够用 |
| 存算分离 / 云端 Watchdog | 违背「数据全部落在本机」的定位（`SPEC.md:38-40`） |
| Cordis 式插件化 | 单体应用，插件化无收益 |
| 改「固定 5 分钟退避」 | 明确的产品决策（`db/schedule_jobs.rs:68-69`）；队列承载它但不改语义 |
| 合并 `subagents` 与 `work_items` | 子 agent 派工链已完整（幂等键 / 机器验收 / digest 三段修复），合并要动全链，风险高收益低 |
| 把 `exec_traces` 升格成工作单元 | trace 的定位就是观测面（自述「不参与任何业务判定」），改它破坏保留期清扫与活动页口径 |
| 改闸序 | 已有 FACT-SHEET 兜底，闸序改动风险高收益低 |
| 改 `MAX_ROUNDS` / `MAX_FUNCTION_CALLS` | 安全网常量，2026-10-28 简化方案刚收口 |

### 10.2 可选项（不做也不阻塞主线）

| 项 | 说明 |
|---|---|
| **G7 定时作业并发闸** | 给 `spawn_job_run` 加一个作业级信号量（建议初值 2）。**注意**：主线的 Effects 队列天然提供 kind+key 唯一，但**不提供并发上限**——同一 tick 多个作业仍会同时起。若要做，须独立于 Effects 另设闸。superagent 的 `WorkerSlots` 是对位物（`dsh-superagent-prototype/src/executor/dsh.ts:171-199`），其「人的追问可多占一槽」的优先级设计可参考 |
| **G9 `tool_call_id` 执行去重** | 在 dispatch 前加「同一 id 已执行过则返回上次结果」的表。**边界严格**：只对**同一 tool_call_id** 去重，不同 id 的同名工具**必须**各跑一次 |

---

## 十一、风险与回归面

| 风险 | 缓解 |
|---|---|
| 事务里混入 `await` 或外部副作用 → SQLite 事务跨 await 持有会出错 | `with_tx` 闭包是同步的；code review 盯这一条；CRATE 里禁止在事务闭包内调用 `spawn` |
| Effects 队列与既有重试逻辑双跑 | 迁移按批次逐个切，切完即删旧字段（`retry_at` / `retry_count` 降级为只读，下一批再删列） |
| `work_items` 与 `tasks` / `subagents` 语义打架 | 三张表**明确分工且不合并**（§7.4）；关联只用 `origin_ref`；UI 上卡片显示工单摘要 |
| 干跑模式让用户以为作业没跑 | `observe` 档必须在作业页与通知里显式标注「本轮未执行（预演）」，不能静默跳过 |
| 批 5–7 触碰主聊天链路 | 单独提交；主 agent 开票用**开关**（默认关）灰度一轮再放开 |
| 桌面前端多处依赖 `BotChatResult` 语义 | 批 7 需同步前端；活动页增补条目不能打乱既有 origin 筛选口径 |

---

## 十二、文档同步清单

**方案落地时**（不是现在）按 [AGENTS.md ](../AGENTS.md) 的硬要求同步：

| 文档 | 时机 | 动作 |
|---|---|---|
| `SPEC.md` | 批 3 / 5 / 7 / 8 落地时 | 功能进/改随同一提交更新「功能规格」；批 7 的面板重组要写进 §1/§2 |
| `docs/acceptance/matrix.csv` | 同上 | SPEC 功能条目变更**必须**同步追踪矩阵，否则 `tests-audit/audit_acceptance_matrix.py` 门禁红 |
| `docs/acceptance/cards-*.md` | 同上 | 每功能一张验收卡，新功能要补卡 |
| `docs/rust-bot-architecture.md` | 批 1/2/5 落地时 | 新模块落位后更新模块地图（`tests-audit/audit_module_map.py` 对拍） |
| `docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md` | 批 1 落地时 | 闸序或工具跑位有变时同步（该文档 §八 维护规则） |
| `docs/agent-audit-sop-zcode.md` | 新增风控常量时 | 常量基线表须与 `bot/constants.rs` 对拍（`tests-audit/audit_sop_baseline.py`） |
| `DEVLOG.md` | 批 1 落地时 | 只写「为什么换方向」：为什么引入事务包装、为什么把 migration_journal 泛化成队列 |
| `tests-audit/audit_file_size.py` | 全程 | 新文件 ≤1500 行；白名单已清空，超限当场 fail |

**本文现在**不需要同步任何文档——没有功能变更，只有方案。

---

## 十三、证据索引

按主题列出本文引用的位置，便于复核。

### 闸与工具

| 主题 | 位置 |
|---|---|
| 闸序 8 级 | `src-tauri/src/bot/dispatch.rs:522-800`；早退 8 处断言 `src-tauri/src/bot/dispatch/early_return_events_tests.rs` |
| 66 工具单一来源 | `src-tauri/src/bot/registry/schemas.rs:355-359` |
| 强制缺口清单（已有） | `docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md:184-199` |

### 状态与持久化

| 主题 | 位置 |
|---|---|
| `DB_WRITE_LOCK` | `src-tauri/src/db/mod.rs:362`、`:373-389` |
| `with_write_conn` | `src-tauri/src/db/lock.rs:43-58` |
| 事务实例（38 处，生产） | `memory/consolidate.rs:284`、`memory/store.rs:518`、`memory/extract.rs:514,643,807`、`meta/sync.rs:97`、`db/tasks_patch.rs:55,332`、`db/tasks.rs:808`、`db/subagents.rs:189`、`db/bot_sessions.rs:194`、`db/workspace.rs:112,143,223`、`db/tasks_transfer.rs:246`、`db/schedule_jobs.rs:324`、`db/bot_history.rs:138,164,196,281` |
| `upsert_tasks` 无事务 | `src-tauri/src/db/tasks.rs:306-426`（`:387` 逐条 execute） |
| `migration_journal` 三要素 | DDL `src-tauri/src/db/mod.rs:191-199`；幂等 `src-tauri/src/migration/journal.rs:28-70`；replay `src-tauri/src/migration/recovery.rs:105-125` |
| `exec_traces` 定位自述 | `src-tauri/src/db/trace.rs:1-12` |
| trace 四态 | `src-tauri/src/db/trace.rs:69-73`；唯一写口 `src-tauri/src/trace_sink.rs:274-281` |
| `subagents` 六态与转移表 | `src-tauri/src/db/subagents.rs:18-25`、`:97-105` |
| `tasks` 三列 | `src-tauri/src/db/tasks.rs:77-96` |

### 副作用与重试

| 主题 | 位置 |
|---|---|
| 定时作业重试（固定 5 分钟 + 理由注释） | `src-tauri/src/db/schedule_jobs.rs:66`、`:68-69`；计算 `src-tauri/src/bot_scheduler.rs:644-648` |
| job run 与 job 状态两处写 | `src-tauri/src/bot_scheduler.rs:604-616`、`:649-661` |
| 通知 6 发送点 | `bot_scheduler.rs:37-45`、`:52-60`；`bot_reminders.rs:369-394`；`due_notify.rs:253-274`；`ask_engine.rs:518-523`；`apps/commands.rs:113-118` |
| reminder 先发后删 | `src-tauri/src/bot_reminders.rs:352-353` |
| 子 agent 终态写库（内存指数重试） | `src-tauri/src/bot/subagent_runner.rs:41-77`（`:48` `RETRY_DELAYS_MS`） |
| 子 agent 三段写 | `bot/subagent_spawn.rs:145-176`；`bot/subagent_runner.rs:245-251`、`:344-353` |
| app run 两处写 | `src-tauri/src/apps/mod.rs:104-108`、`:197-212`；清扫 `:287` |

### 执行单元与判定

| 主题 | 位置 |
|---|---|
| 子 agent 幂等键 | `src-tauri/src/bot/subagent_spawn.rs:123-142`；SQL `src-tauri/src/db/subagents.rs:339-344` |
| 机器判据 | `src-tauri/src/bot/subagent_digest.rs:292-306`、`:318-355`、`:358-400`；派发形状校验 `bot/subagent_spawn.rs:50-59`、`:67-100`；跑完判定 `bot/subagent_runner.rs:704-711` |
| 僵尸清扫与恢复不可行的理由 | `src-tauri/src/bot_orchestrator.rs:555-557`、`:561`、`:577-582` |
| `SubagentStage` 不落库 | 定义 `src-tauri/src/bot_orchestrator.rs:87-110`；DDL 无该列 `src-tauri/src/db/subagents.rs:145-168` |
| 主聊天建 trace（无 task_id / title） | `src-tauri/src/bot_chat/flow.rs:636-643` |
| `tool_call_id` 无去重 | `src-tauri/src/db/trace.rs:95-108`、`:396-416`；`src-tauri/src/bot_model_loop/core.rs:762`；`src-tauri/src/bot/dispatch.rs:522-538` |

### 面板

| 主题 | 位置 |
|---|---|
| 通知 4 类 kind | `src-tauri/src/notifications.rs:33-42` |
| 通知中心待处理页签 | `src/components/NotificationsPage/NotificationsPage.tsx:405-409`、`:420-426` |
| 活动页只有 origin 筛选 | `src/components/ActivityPage/index.tsx:22-28`；标题回落 `:145` |
| 子 agent 六态标签 | `src/components/ChatPanel/constants.ts:111-121` |
| 并发闸三处 | `src-tauri/src/py/gate.rs:16`；`src-tauri/src/bot/services.rs:50`；`src-tauri/src/bot/constants.rs:250,252` |
| 定时作业无闸 | `src-tauri/src/bot_scheduler.rs:853`、`:710-716` |

### superagent 对位

| superagent | 位置 |
|---|---|
| 领域模型与阶段表 | `dsh-superagent-prototype/docs/001-domain.md`、`docs/007-architecture.md` |
| 授权与准入表 | `docs/002-authorization.md`；`src/policy.ts:167-241` |
| Effects 队列内核 | `src/app/effects.ts` |
| Worker 槽位 | `src/executor/dsh.ts:171-199` |
| 观察模式三档 | `src/policy.ts:84` |
| 事件与回执 | `src/app/outbox.ts` |

---

## 十四、结语

superagent 对 wmessage 最有价值的三样东西不是它的架构形态，而是它的**纪律**：

1. **副作用必达** —— 状态与动作之间不留无主中间态
2. **活要有载体** —— 一份被授权的、有阶段、有判据的工作，而不是一段聊天记录
3. **人永远优先** —— 需要人处理的事集中可见，不该做的能先看后跑

第一条本仓**已有局部答案**（`migration_journal`），把它泛化即可；
第二条**完全缺失**，是主线的核心；第三条**部分缺失**（通知中心只覆盖 4 类 kind），
补齐成本低、体感收益高。

**建议按批 1–4 → 5–6 → 7–8 推进**：前四批不改用户可见行为，能先把地基补平；
批 5–6 是最大的结构改动；批 7–8 才是用户能直接感知的改善。
