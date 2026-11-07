# Agent 运行时改造方案（2026-11-02）

> **状态：批 1–8 全部落地（批 5–8 于 2026-11-04 收口）。** 本文是方案原文，
> 落地时的实机验收卡见 `docs/realmachine-smoke-test-2026-11-03.md`（批 1–4）
> 与 `docs/realmachine-smoke-test-2026-11-04.md`（批 5–8）。
> 每批一个提交，并按 [§九 文档同步清单](#九文档同步清单)同步 SPEC / 验收矩阵 / 验收卡。
>
> **依据**：
> 1. `deepseek-ai/dsh-superagent-prototype` 与 `deepseek-ai/deepseek-harness` 源码走查（2026-11-02）
> 2. 本仓三路静态审计：状态写入与副作用、执行单元与面板呈现、事务边界与幂等
> 3. `SPEC.md` 全文、`docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md`、`docs/acceptance/matrix.csv`
>
> **口径**：本文所有论断均带 `文件:行号` 证据。凡写「未确认」的，是本次审计没有取到证据，
> 不是「不存在」。行号以本文写作时的 `main` 为准。
>
> **修订（2026-11-03 逐条对拍复核后落盘，随改造推进更新）**：
> 1. 裂缝 1 重定性：「部分提交」在现行生产路径不存在（主入口 `db_upsert_for` 与其余多批量调用方均已包事务，唯一裸连接调用批大小恒 1），降为**函数级防呆**，批 1 仍做但不再是修活跃 bug。
> 2. 通知发送点 6 → **8 处**：补 `bot_scheduler.rs:692-700`（自动暂停）与 `bot/services/observe.rs:274-282`（服务告警，走 `try_state`，按 `notification()` 搜会漏）。
> 3. `migration_journal` **不迁**通用内核：唯一实战恢复路径，重写零收益纯风险；且内核落地时它之外暂无第二调用方，踩 YAGNI。journal 原样保留为模式原型。批 2+3 合并（内核与 notify 同批落地）。
> 4. 裂缝 5（reminder 顺序对调）废弃：对一次性提醒等于永久丢失（「周期提醒下一轮」兜底对 one-shot 不成立），批 2 队列化时以「删行+入队同事务」正解。
> 5. Effects 内核补规格（对拍 `effects.ts` 实源）：成功=删行（CAS 四元组）、per-kind 并发槽 `limit()`（G7 随批 3 收编）、max_attempts 限次、派发器 at-least-once + 消费方幂等要求。
> 6. `with_tx` 补红线：闭包内禁调会再取锁的路径（`DB_WRITE_LOCK` 是 std Mutex 非重入，嵌套即死锁）；拿锁必须走 `lock_db_write()`。
> 7. 新增快赢批 4：主聊天 `begin_trace` 传 title（活动页先不显示裸会话 id，不等 work_items）。
> 8. **批 5–8 拍板（2026-11-03，四项全按推荐）**：开票粒度 = **每会话一行**（session_id upsert，等真出现「独立授权工作项」再迁粒度）；阶段重估 = **挂现有 30s tick**（不新增后台任务）；mandate 列 = **不建**（YAGNI，有授权语义再加）；批 7 聚合 = **查询时聚合**（不新增 notifications kind）；批 8 干跑档命名 = **preview/shadow**（避撞 `bot/services/observe.rs`）。批 1–4+审计修复先行实机验收，过了再推批 5–8。 批 5–8 落地偏离（2026-11-04）：批 7 的**活动页状态筛选延后**（trace_list 需加 status 参数，与 50 条截断口径联动，单独小批再做），聚合命令 + 通知中心「后台待办」区块已落地。
> 9. **粒度重构（2026-11-04 用户拍板，方案见 `docs/WORK-ITEM-REDESIGN-PLAN-2026-11-04.md`）**：本文 §7.2（粒度=每会话一行）、§7.3（failed 的「此后无人消息」口径）、§7.6（每轮模型循环开票）三节**被该方案取代**——粒度改为**每回合一行**（一次干活的尝试 = 一行），任务卡与定时作业执行一并纳入开票（今天这两条链路根本不开票），失败行不再被后续消息翻转，聚合面拆「未决 / 已作废」两组。落地前须先跑完批 5–8 的实机冒烟（`docs/realmachine-smoke-test-2026-11-04.md`）拿基线。

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

**wmessage 有事务**，而且用在了关键处。`conn.transaction()` 生产代码 25 处 + `unchecked_transaction()` 10 处（2026-11-03 复核；统计须排除 `src-tauri/src/.mimosa/` 快照副本），
覆盖：`bot_history`（INSERT + `UPDATE bot_sessions.updated_at` 同事务，`db/bot_history.rs:159-190`）、
`apps` 三表同事务删（`db/apps.rs:149-161`）、`scheduled_job_runs` 插入 + 裁剪同事务（`db/schedule_jobs.rs:260-285`）、
建会话 + 任务块同事务（`bot_chat/exec.rs:185-204`）、子 agent 收尾多写同事务（`bot/subagent_runner.rs:783-810`）、
子 agent 插行同事务（`bot/subagent_spawn.rs:145-176`）。

**真正的缺口不是「没有事务」，而是「写串行 ≠ 写原子」**——见 §3.1。

### 2.3 「崩了能接着跑」

`migration_journal` 是**全仓唯一同时具备三要素**的结构：

1. **持久化**：DB 表 `src-tauri/src/db/mod.rs:191-199`
2. **幂等键**：`(task_id, src)` 至多一条 pending `src-tauri/src/migration/journal.rs:41-67`
3. **崩溃后续跑**：启动 replay 扫 pending `src-tauri/src/migration/recovery.rs:111-130`（其前 `:98-110` 是孤儿 pending 清理）

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
| 1 | `src-tauri/src/db/tasks.rs:306-426`（`:387` 逐条 `stmt.execute`） | `upsert_tasks` 批量卡**函数内无事务**，RMW 冲突检查在每条 execute 之前 | **（2026-11-03 复核改判）现行无活跃部分提交路径**：主入口 `db_upsert_for`（`db/tasks.rs:807-810`）与其余多批量调用方（tasks_patch / tasks_transfer / migrations）均已外层包事务，唯一裸连接调用 `task_autotag.rs:225` 批大小恒 1。定性为**函数级防呆缺口**：未来新调用方拿裸 conn 直调即踩 |
| 2 | `src-tauri/src/bot_scheduler.rs:604-616` 与 `:649-661` | `record_job_run` 与 `update_job_run_state` 两次独立 `with_write_conn`（两次 open_db、两次事务） | `scheduled_job_runs` 有 fail 行，但 `scheduled_jobs.retry_at` 仍为空 → **失败不重试**，状态徽标还是上次的值 |
| 3 | `src-tauri/src/apps/mod.rs:104-108` 与 `:197-212` | `record_app_run` 与 `finish_app_run` 两次独立取锁 + 独立连接 | `app_runs` 行永远停在 `running`（用户以为还在跑），只能靠 `apps/mod.rs:287` `sweep_stale_runs` 时间阈值兜底 |
| 4 | `src-tauri/src/bot/subagent_spawn.rs:145-176` → `src-tauri/src/bot/subagent_runner.rs:245-251` → `:344-353` | 插行 / 置 Running / 建会话+回写 session_id **三段独立事务** | 行存在但无 `session_id`，父 agent `check` 拿到空会话，`check_subagent` 永远查不到进度 |
| 5 | `src-tauri/src/bot_reminders.rs:352-353` | `fire(app, &r)`（OS 通知 = 外部副作用）后 `delete_row(app, &r.id)` 分两步 | 崩溃在两者之间 → 下个 tick **重复发同一条提醒** |

> **（2026-11-03 修订）批 1 收裂缝 1（防呆）/ 2 / 3**：裂缝 2 的两笔写都是纯库内写（run 行 + job 状态），可直接合并事务；裂缝 3 的「起进程」是外部副作用，批 1 只做 fail-close（起失败即改 failed，不留 running）。裂缝 4 与裂缝 5 涉及「外部副作用 + 状态写」的两段式（runner 派发 / OS 通知），事务管不了「通知发出去之后进程死了」——归主线二的队列，裂缝 5 在批 2 队列化时以「删行+入队同事务」正解，**不做顺序对调**（对一次性提醒等于永久丢失）。

### 3.2 G2｜副作用无统一持久化待办，8 个通知发送点全无重试（结构性缺口）

**证据**：

- 全仓 28 张表**没有 queue / outbox 类表**（唯一具备三要素的 `migration_journal` 只服务文件迁移一个子系统）。
- 系统通知 8 个发送点（2026-11-03 复核由 6 修正为 8），**全部无重试**：

| 位置 | 失败处理 | 用户可见性 |
|---|---|---|
| `src-tauri/src/bot_scheduler.rs:37-45`（作业完成/失败） | 只 `eprintln!` | **不可见**（不进审计、不进 UI） |
| `src-tauri/src/bot_scheduler.rs:52-60`（错过补跑） | 只 `eprintln!` | **不可见** |
| `src-tauri/src/bot_reminders.rs:369-394` | 记审计 `reminder.fire_failed` | 仅审计 |
| `src-tauri/src/due_notify.rs:253-274` | 记审计 `due_notify_fail`；且标志已在 `plan_round` 置为「已发」 | **静默丢弃**（注释 `:263` 明说「避免每 30s 重试刷屏」） |
| `src-tauri/src/ask_engine.rs:518-523` | `let _ = ....show()` | **返回值直接丢弃，连审计都没有** |
| `src-tauri/src/apps/commands.rs:113-118` | `let _ = ....show()` | **静默丢弃** |
| `src-tauri/src/bot_scheduler.rs:692-700`（自动暂停） | 只 `eprintln!` | **不可见** |
| `src-tauri/src/bot/services/observe.rs:274-282`（服务资源告警） | 只 `eprintln!`；走 `try_state` 而非 `notification()`，按后者搜会漏 | **不可见** |

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
| 首页「机器人动态」 | 否：最新 4 条 trace（`:98` `slice(0,4)`）+ 近 7 天成功率 | `src/components/HomePage/HomePanels.tsx:80-147` |

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
- 闭包内**不得**再走任何取锁路径（`with_write_conn` / `db_locked` / journal 写）——`DB_WRITE_LOCK` 是 `std::sync::Mutex`（`db/mod.rs:362`）**不可重入**，嵌套即死锁（`migration/journal.rs:16` 已记载）。需要业务 SQL 就内联，或抽收 `&Transaction` 的 `*_in_tx` helper。仓里有两套并行持锁辅助（`db/lock.rs:43` `with_write_conn` 与 `bot_orchestrator.rs:508` `db_locked`），`with_tx` 落地要明说覆盖谁，避免出现第三种写法。
- 拿锁必须走 `lock_db_write()` 而非裸锁——契约函数 debug_assert 的是 `HOLDING_DB_WRITE` 线程本地标记（`db/lock.rs:33-35`），裸锁「锁是真的、标记是假的」。
- 事务时长上限建议 100ms；超过要拆。

> **为什么不用 superagent 的 `repo.atomic()` 名字**：本仓已有 `with_write_conn` 样板约定
> （`src-tauri/src/db/lock.rs:1-16` 记录的收敛历史），新包装按本仓命名走 `with_tx`，
> 不引入外来词汇。

### 5.3 逐条修裂缝

| 裂缝 | 修法 |
|---|---|
| 1 `upsert_tasks` 函数级防呆 | 函数内加契约断言：`tasks.len() > 1` 时要求 `!conn.is_autocommit()`（在事务内），违反即 debug panic——把「批量必须包事务」钉死在函数上；既有调用方不动。批内原子性继续由调用方事务保证（tasks_patch 等把 upsert 与其他写绑同一事务，**事务不能下沉进函数**，下沉会拆散它们的原子性） |
| 2 job_run 与 job 状态分家 | `finalize_job_run` 的「插 run 行」+「写 job 状态」合并为**一个事务**（`bot_scheduler.rs:604-616` + `:649-661`；两笔都是纯库内写，可以合并） |
| 3 `app_runs` 停在 running | 「建 run 行」+「起进程」是两段（进程是外部副作用）→ 进程启动失败时把 run 行改 failed（复用 `mark_run_failed`），不留 running；真正的「进程起了但收尾没跑」仍靠 `sweep_stale_runs` 时间阈值 |
| 4 子 agent 行与会话脱钩 | 维持批 3 队列化处理（`subagent_dispatch` 进队列后「插行了但 runner 没起」根源消除）；三段写里前两段（插行 / 置 Running）间的崩溃窗口由队列续跑兜底 |
| 5 reminder 先发后删 | **批 2 队列化时正解**：删行与入队（`notify` kind）同事务，投递失败指数退避重试、限次后进健康面板。~~顺序对调~~废弃——对一次性提醒是永久丢失（「周期提醒下一轮」兜底对 one-shot 不成立），且批 2 反正要重修这里 |

### 5.4 改造后提升

1. **批量写不再可能半提交**：`upsert_tasks` 的「批量必须包事务」从口头约定变成机器断言（2026-11-03 复核：现行调用方均已包事务，此条从修活跃 bug 降为防呆）。
2. **定时作业的失败重试不再丢**：job_run 与 job 状态同事务后，
   「有 fail 行但 retry_at 为空 → 不重试」这个洞关闭。
3. **崩溃后不留半写状态**：主 agent、app 运行两处的「存在但残缺」行消失（子 agent 一处归批 3 队列化）。
4. ~~提醒不再重复轰炸~~ → 移交批 2（裂缝 5 队列化正解），批 1 不再涉及 reminder。

### 5.5 验收

- `upsert_tasks` 新增用例：批内第 N 条冲突 → **整批不落库**（锁定既有契约：主入口本就包事务）；len>1 且 autocommit → debug 断言红
- `finalize_job_run` 新增用例：run 行写入成功后 job 状态写入失败 → **两者都不留**
- `app_runs` 新增用例：进程启动失败 → run 行落 failed（不留 running）
- reminder 用例移至批 2；子 agent 三段写维持批 3 队列化处理
- 全部走 `scripts/test-all.sh`

---

## 六、主线二：统一持久化副作用队列（Effects）

### 6.1 目标

**副作用必达**：任何「状态已写、但外部动作还没做」的中间态，都必须能被恢复。

### 6.2 做法：把 `migration_journal` 泛化成通用队列

**这是本方案里唯一「抽象」的引入，因此必须满足 AGENTS.md 的门槛：第二个调用方出现前不抽象。**
（2026-11-03 修订后）内核**与首个真实调用方 `notify` 同批落地**（不单独先行），批 3 紧接着再收编
三个 kind——抽象落地即有真实调用方，不违反「不为以后可能预留抽象」：

1. 系统通知投递（`notify`，内核落地时的首个调用方）
2. 定时作业失败重试（`schedule_retry`）
3. 子 agent 终态写库（`subagent_finalize`）
4. 子 agent runner 派发（`subagent_dispatch`）

`migration_journal` 不迁（见 §6.4），原样保留为模式原型。

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

**内核补充规格（对拍 `dsh-superagent-prototype/src/app/effects.ts` 实源后定稿，2026-11-03）**：

| # | 语义 | 说明 |
|---|---|---|
| 5 | **成功 = 删行**，CAS `kind+key+seq+generation` 四元组命中才删 | 队列即待办，done 不留行；superagent 同款（`effects.ts` attempt：无 failure 即 DELETE） |
| 6 | 失败 = `attempts+1` + `error`（截断落列）+ `due` 顺延 `min(cap, base × 2^attempts)` | 同语义 4；superagent 还把 error 截 1000 字符防膨胀 |
| 7 | 每 kind 可配 `max_attempts`：超限**停投递、留行、健康面板标红**；同 key 再入队（payload 变更）时 `attempts` 归零复活 | `notify` 限次（OS 通知权限被拒时无限重试无意义）；不配则按退避上限持续重试（superagent 无限流 + 健康板 `attempts>=3` 标红） |
| 8 | 每 kind 可配并发槽 `limit()`（返回 0 = 整 kind 暂停） | 对位 superagent `Handler.limit` / `WorkerSlots`；G7 的作业并发闸就是派发器对 `schedule` kind 的槽 |
| 9 | 派发器 = 单 tokio 任务：轮询 due + `kick` 唤醒 + 启动即扫（无需单独 replay 步骤） | in-flight 崩溃：行留原 due（已过期），重启自然重跑——**at-least-once**，消费方必须幂等（notify 的业务派生 id + `INSERT OR IGNORE` 天然满足） |

### 6.4 收编范围（按批次）

| 批次 | 迁入的 kind | 现状 | 迁后 |
|---|---|---|---|
| ~~原批 2~~ | `migration` | `migration_journal`（自研三要素实现） | **不迁**（2026-11-03 修订）：唯一实战恢复路径，重写零收益纯风险；effect_queue 无 state 列，replay 靠 `state='pending'`（`recovery.rs:111-130`）+ 孤儿清理（`:98-110`），「行为不变」做不到。journal 原样保留为模式原型，是否收编另立拍板 |
| 批 2 | `notify` | 8 个发送点全无重试、4 处静默 | **内核与首个真实调用方同批落地**：统一走队列、有回执、指数退避、限次、失败进健康面板、**绝不静默**；裂缝 5 以「删行+入队同事务」正解 |
| 批 3 | `subagent_finalize` / `subagent_dispatch` | 子 agent 终态写库是**内存内**指数重试（`src-tauri/src/bot/subagent_runner.rs:41-77`，`RETRY_DELAYS_MS=[1s,2s,4s,8s,16s]`，**不持久化**）；runner 派发是 `spawn` 一次性 | 两者进队列：崩溃后自动续跑；`subagent_dispatch` 进队列后，G5 的「插行了但 runner 没起」自动消解（僵尸清扫对「派发待办还在队列」的 queued 行豁免）。~~`schedule_retry`~~ **不做**（2026-11-03 落地时砍）：批 1 已把 run 行与 job 状态合并为同事务，`retry_at` 本就持久化且被 jobs_tick 扫描——崩溃续跑已有载体，再入队列是重复实现（YAGNI）。G7 的并发闸按 §10.2 原案独立成作业级信号量 |

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
6. **后台并发有了统一闸**：per-kind `limit()` 落地后，G7 的「定时作业无并发闸」随 `schedule` kind 收编一并关闭（批 3），通知发送也顺带限速。

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
  result         TEXT,                  -- JSON：结构化结论
  error          TEXT,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_session
  ON work_items(session_id) WHERE session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_stage ON work_items(stage);
```

> **粒度拍板（2026-11-03）：每会话一行**——session_id partial UNIQUE 撑起
> upsert；开票是**机械动作**（每轮模型循环 upsert，不靠提示词自觉），objective
> 随轮次更新、轮结束收尾 stage。同一会话先后两个任务共享一行是已知取舍，
> 真出现「独立授权工作项」需求时再迁粒度。mandate 列不建（主聊天今天没有
> 授权范围概念，永远 NULL 的死列违反 YAGNI）。

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

> **重估驱动器拍板（2026-11-03）：挂现有 30s 调度 tick**（与提醒/作业同一循环），
> 阶段收敛最多 30s 延迟、面板无感，不新增后台任务。

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
| 每轮模型循环（**机械开票**，不靠提示词判断） | 按会话 upsert `work_items` 行（`kind=chat`，粒度拍板见 §7.2）；`objective` 更新为本轮用户输入摘要；`stage=working` |
| 模型在首轮给出方向后 | 模型可补全 `acceptance`（复用现有 acceptance 语法，提示词侧加一句） |
| 每轮报告 | 更新 `stage`（受阻/失败/就绪等） |
| 轮结束 | 落 `result` JSON、`stage` 收尾 |
| `ask_user` 提问 | `stage=awaiting_answer`，**并把 `notifications` 那条 pending 与 work_item 关联**（解决 G4 的「不在工作行上」） |
| 30s tick（拍板驱动器） | 重估静默期阶段：blocked / failed 的「此后无人消息」判定 |

### 7.7 改造后提升

1. **主 agent 的活有了身份与目标**：ActivityPage 不再回落到会话 id（`ActivityPage/index.tsx:145`），
   能显示「这活要干什么」。
2. **主 agent 平权获得机器可判定验收**：当前是子 agent 独占（`subagent_runner.rs:704-711` 唯一调用点），
   改后主 agent 也能声明 `file_exists:` 判据，交付有机器判据而不是只有模型自述。
3. **三套状态语义收敛为一套**：G4 的六态 / 三列 / 四态各自定义、互不映射问题得到缓解——
   `work_items.stage` 成为**唯一横跨「排队 / 进行中 / 需要人 / 结局」的语义**，
   既有三套保持原样（各自有用户界面），但新面板按 `stage` 组织。
4. **执行现场可恢复（解 G5）**：只要 `objective` + `acceptance` + `stage` 落库，
   崩溃后就能像 superagent 那样「按恢复提示重启」——
   这正是 `bot_orchestrator.rs:555-557` 注释里说的「先把 stage / onion 持久化，再谈重投」。
5. **「等人回答」挂在工作行上**：不再是一条孤立的 pending 通知。
6. **面板能只显示需要人的**：按 `stage ∈ {blocked, failed, awaiting_answer, awaiting_review, undelivered}` 聚合。

---

## 八、主线四：「需人处理」聚合面与干跑模式

### 8.1 聚合面（消解 G6）

**不新建表**，在视图层聚合。**口径拍板（2026-11-03）：查询时聚合**——失败的
子 agent / 作业 / 需人的 work_item 在渲染时从各自表查询合并，**不新增
notifications kind、不改 notifications 表**（4 类 kind 白名单与既有口径不动）：

| 位置 | 动作 |
|---|---|
| 通知中心 | 「待处理」页签**增补**一个来源：`work_items` 中 stage 需要人介入的行 |
| 活动页 | 增补**状态筛选**（现在只有 origin 筛选，`ActivityPage/index.tsx:22-28`） |
| 右侧栏「子 Agent」 | 失败的子 agent（failed / budget_exceeded）**提级**到通知中心，不再与 succeeded 同列 |
| 定时作业页 | 失败的作业提级到通知中心 |

### 8.2 干跑模式（消解 G8）

给定时作业加 `mode` 三档（照搬 superagent `src/policy.ts:84` 的 `off / preview / auto`；
**命名拍板（2026-11-03）：用 `preview`**——`observe` 已被 `bot/services/observe.rs`
（资源巡检）占用，撞名会毒化全仓搜索）：

| 档 | 行为 |
|---|---|
| `off` | 不派发（现有 `enabled=false` 语义保留） |
| `preview`（新增，默认建议） | 到点**只生成提案不执行**：建一张卡 + 写清「本会做什么」，用户确认后才真跑 |
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
| **1** | `db/tx.rs` 事务包装 + 裂缝 1（契约断言）、2（job 状态合并）、3（app run fail-close） | — | 无 | 中 |
| **2** | Effects 内核（终态/限次/槽位/派发器）+ `notify` 8 处收编 + 裂缝 5 队列化正解 + 失败健康面 | 1 | **有**（通知失败不再静默） | 大 |
| **3** | `schedule_retry` / `subagent_finalize` / `subagent_dispatch` 收进队列（G7 随 `schedule` kind 槽位收编） | 2 | 无（间隔语义不变） | 中 |
| **4** | 快赢：主聊天 `begin_trace` 传 title（活动页不再回落裸会话 id） | — | 轻微 | 小 |
| **5** | `work_items` 表 + `stage` 判定 + 主 agent 开票与推进（拍板已定：每会话一行 / 30s tick 重估 / 不建 mandate，见修订 8） | 1 | 轻微（活动页多出工作项条目） | 大 |
| **6** | 机器判据引擎泛化 + 主 agent 支持 `acceptance` | 5 | 轻微（交付多出机器验收结论） | 中 |
| **7** | 面板聚合：通知中心增补来源 / 活动页状态筛选 / 失败提级（口径拍板：查询时聚合 vs 失败时写通知行，推荐前者） | 5 | **有**（面板重组） | 中 |
| **8** | 定时作业干跑模式（off/observe/auto；`observe` 撞名 `bot/services/observe.rs`，实现换词 `preview`/`shadow`） | 2,5 | **有**（作业页多一档） | 中 |

**批 1–3 是「不改用户可见行为」的加固**（批 2 的可见变化仅限失败场景），可先合；
**批 4 是一行级快赢**；**批 5–6 是最大的结构改动**；**批 7–8 直接改善体感**。

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
| 迁 `migration_journal` 上通用内核 | 唯一实战验证过的崩溃恢复路径，重写零收益纯风险；effect_queue 无 state 列，replay+孤儿清理语义「行为不变」做不到（2026-11-03 修订，原批 2 废弃） |

### 10.2 可选项（不做也不阻塞主线）

| 项 | 说明 |
|---|---|
| ~~**G7 定时作业并发闸**~~ | **已收编进批 3**（2026-11-03 修订）：不另设闸，由 Effects 派发器的 per-kind `limit()` 承担（`schedule` kind 槽位建议初值 2）；同作业防重入已有 `SchedGuard`（`bot_scheduler.rs:411`），缺的只是跨作业上限。superagent 的 `WorkerSlots`（`dsh-superagent-prototype/src/executor/dsh.ts:171-199`）其「人的追问可多占一槽」的优先级设计可后续参考 |
| **G9 `tool_call_id` 执行去重** | 在 dispatch 前加「同一 id 已执行过则返回上次结果」的表。**边界严格**：只对**同一 tool_call_id** 去重，不同 id 的同名工具**必须**各跑一次 |

---

## 十一、风险与回归面

| 风险 | 缓解 |
|---|---|
| 事务里混入 `await` 或外部副作用 → SQLite 事务跨 await 持有会出错 | `with_tx` 闭包是同步的；code review 盯这一条；CRATE 里禁止在事务闭包内调用 `spawn` |
| Effects 队列与既有重试逻辑双跑 | 迁移按批次逐个切，切完即删旧字段（`retry_at` / `retry_count` 降级为只读，下一批再删列） |
| `work_items` 与 `tasks` / `subagents` 语义打架 | 三张表**明确分工且不合并**（§7.4）；关联只用 `origin_ref`；UI 上卡片显示工单摘要 |
| 干跑模式让用户以为作业没跑 | `preview` 档必须在作业页与通知里显式标注「本轮未执行（预演）」，不能静默跳过 |
| 批 5–7 触碰主聊天链路 | 单独提交；主 agent 开票用**开关**（默认关）灰度一轮再放开 |
| 桌面前端多处依赖 `BotChatResult` 语义 | 批 7 需同步前端；活动页增补条目不能打乱既有 origin 筛选口径 |

---

## 十二、文档同步清单

**方案落地时**（不是现在）按 [AGENTS.md ](../AGENTS.md) 的硬要求同步：

| 文档 | 时机 | 动作 |
|---|---|---|
| `SPEC.md` | 批 2 / 5 / 7 / 8 落地时 | 功能进/改随同一提交更新「功能规格」；批 7 的面板重组要写进 §1/§2 |
| `docs/acceptance/matrix.csv` | 同上 | SPEC 功能条目变更**必须**同步追踪矩阵，否则 `tests-audit/audit_acceptance_matrix.py` 门禁红 |
| `docs/acceptance/cards-*.md` | 同上 | 每功能一张验收卡，新功能要补卡 |
| `docs/rust-bot-architecture.md` | 批 1/2/3/5 落地时 | 新模块落位后更新模块地图（`tests-audit/audit_module_map.py` 对拍） |
| `docs/TOOL-EXECUTION-FACT-SHEET-2026-10-30.md` | 批 1 落地时 | 闸序或工具跑位有变时同步（该文档 §八 维护规则） |
| `docs/agent-audit-sop-zcode.md` | 新增风控常量时 | 常量基线表须与 `bot/constants.rs` 对拍（`tests-audit/audit_sop_baseline.py`） |
| `DEVLOG.md` | 批 1 / 批 2 落地时 | 只写「为什么换方向」：为什么引入事务包装、为什么建通用 Effects 队列而**不迁** migration_journal |
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
| 事务实例（生产 35 处：25 transaction + 10 unchecked） | `memory/consolidate.rs:284`、`memory/store.rs:518`、`memory/extract.rs:514,643,807`、`meta/sync.rs:97`、`db/tasks_patch.rs:55,332`、`db/tasks.rs:808`、`db/subagents.rs:189`、`db/bot_sessions.rs:194`、`db/workspace.rs:112,143,223`、`db/tasks_transfer.rs:246`、`db/schedule_jobs.rs:324`、`db/bot_history.rs:138,164,196,281` |
| `upsert_tasks` 无事务 | `src-tauri/src/db/tasks.rs:306-426`（`:387` 逐条 execute） |
| `migration_journal` 三要素 | DDL `src-tauri/src/db/mod.rs:191-199`；幂等 `src-tauri/src/migration/journal.rs:28-70`；replay `src-tauri/src/migration/recovery.rs:111-130`（孤儿清理 `:98-110`） |
| `exec_traces` 定位自述 | `src-tauri/src/db/trace.rs:1-12` |
| trace 四态 | `src-tauri/src/db/trace.rs:61-65`（running 是 DDL 默认值 `:74`，三个终态才走写口 `src-tauri/src/trace_sink.rs:274-281`） |
| `subagents` 六态与转移表 | `src-tauri/src/db/subagents.rs:18-25`、`:97-105` |
| `tasks` 三列 | `src-tauri/src/db/tasks.rs:77-96` |

### 副作用与重试

| 主题 | 位置 |
|---|---|
| 定时作业重试（固定 5 分钟 + 理由注释） | `src-tauri/src/db/schedule_jobs.rs:66`、`:68-69`；计算 `src-tauri/src/bot_scheduler.rs:644-648` |
| job run 与 job 状态两处写 | `src-tauri/src/bot_scheduler.rs:604-616`、`:649-661` |
| 通知 8 发送点 | `bot_scheduler.rs:37-45`、`:52-60`、`:692-700`；`bot_reminders.rs:369-394`；`due_notify.rs:253-274`；`ask_engine.rs:518-523`；`apps/commands.rs:113-118`；`bot/services/observe.rs:274-282`（走 `try_state`） |
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
| 通知中心待处理页签 | `src/components/NotificationsPage/NotificationsPage.tsx:405-409`、`:420-426`（页内计数/页签）；导航角标在 `src/App.tsx:135-136`、`:188-190`、`:302` |
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
