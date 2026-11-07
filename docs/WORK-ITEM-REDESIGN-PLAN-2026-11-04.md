# 工作项粒度重构方案（每回合一行 + 全来源接入）

> **状态：方案，未实施。** 本文不含代码改动。
> **拍板来源**：2026-11-04 用户拍板——批 5–8 的四个诉求全收（① 同会话两件独立的活分别
> 可追踪 ② 一条消息一条流水用于审计/统计/回看 ③ 失败信号不被新消息抹掉 ④ 聚合面一次
> 看到会话内所有未决事项），**并把任务卡执行与定时作业执行一并纳入开票**。
>
> **依据**：
> 1. `docs/AGENT-RUNTIME-REFIT-PLAN-2026-11-02.md` §七（批 5 的设计原文与四项拍板）
> 2. 批 5–8 的实际落地代码（`3f801143..e0f23813`）
> 3. 本仓四类执行入口的真实调用链（主聊天 / 任务卡 / 定时作业 / 汇报轮 / 子 agent）
>
> **口径**：本文所有论断均带 `file:line` 证据，行号以 `e0f23813` 为准。
> **本文取代**方案 §7.2（粒度拍板）、§7.3（failed 的「此后无人消息」口径）、§7.6（开票时机）
> 三节；偏离登记见 §十。

---

## 〇、要解决的四件事 + 一处范围扩展

| # | 诉求 | 现状根因（证据） | 本方案怎么解 |
|---|---|---|---|
| G1 | 同会话两件独立的活要分别可追踪 | 粒度=每会话一行，两活共用一行、objective 互相覆盖（`db/work_items.rs:106-119`） | 行 = **一次干活的尝试**（回合），两活分属两行；`origin_ref` 指向各自的卡/作业 |
| G2 | 一条消息一条流水，用于审计/统计/回看 | 粒度=每会话一行，历史只剩最后一次 objective | 每回合一行，`objective` / `acceptance` / `result` / `error` 按回合留存 |
| G3 | 失败信号不被新消息抹掉 | 新一轮开票把 `failed` 翻回 `working`（`db/work_items.rs:109`） | stage = **那一回合的结局**，永不翻转；失败行永久留库 |
| G4 | 聚合面一次看到会话内所有未决事项 | 聚合只按 stage 取行（`db/work_items.rs:246-251`），同会话多行会被压成一条 | 聚合按「未决 / 已作废」两组呈现，同会话可列多条 |
| G5 | **范围扩展**：任务卡执行、定时作业执行也开工作项 | 这两条链路**根本不开票**——它们不走 `bot_chat_inner` | 开票点下沉到 `run_task_in_chat_with`（`bot_chat/exec.rs:344`），覆盖任务卡 + 定时作业 + 批量 |

> **G3 与 G4 的一处内在张力**：§7.3 原定义「failed = 报告失败，**且此后无人消息**」，
> 隐含「用户又说话了 → 不用再管了」。G3 要求**不抹除**，G4 要求**全量呈现**。
> 本方案的解法是把二者拆成**两个派生概念**——数据层不删（保 G3），报警层保留原意
> 但被覆盖的项单列一组（保 G4 且不制造噪声）。详见 §五。

---

## 一、现状盘点（带证据）

### 1.1 表与寻址：四个写口全按 session_id

| 写口 | 位置 | 寻址 |
|---|---|---|
| 开票 | `db/work_items.rs:88-167` `upsert_for_session` | 按 session 查 → 有则 UPDATE 无则 INSERT |
| 挂起 | `db/work_items.rs:170-183` `set_stage` | `WHERE session_id = ?1` |
| 写判据 | `db/work_items.rs:205-216` `set_acceptance` | `WHERE session_id = ?1` |
| 轮收尾 | `db/work_items.rs:186-202` `finish` | `WHERE session_id = ?1` |
| 机器判据读行 | `db/work_items.rs:293-304` `load_by_session` | `WHERE session_id = ?1` |

唯一性靠 `CREATE UNIQUE INDEX idx_work_items_session ON work_items(session_id) WHERE session_id IS NOT NULL`
（`db/work_items.rs:29-30`）。

**要害**：这四条语句都是「按会话改**所有**匹配行」。粒度换成多行后，若不改寻址，
`finish` 会把同会话的所有行一起刷成 idle/failed——这是本方案**必须先解决**的问题，
不是细节。

### 1.2 写入点只有主聊天；任务卡与定时作业根本不开票

模型循环有 5 个调用点：

| 调用点 | 位置 | 今天是否开票 |
|---|---|---|
| 主聊天 | `bot_chat/flow.rs:713` | **是**（开票在 `flow.rs:653-681`） |
| 任务卡 / 定时作业 / 批量执行 | `bot_chat/exec.rs:312` | **否** |
| 任务卡逐步执行模式 | `exec_steps.rs:262` | **否** |
| 子 agent | `bot/subagent_runner.rs:607` | 否（`subagents` 表并存，见 §7.4） |
| 子 agent 修复轮 | `bot/subagent_repair.rs:272` | 否（同上） |

任务卡与定时作业走 `run_task_in_chat_with`（`bot_chat/exec.rs:344`）→ 自己建会话
（`exec.rs:405` `create_exec_session`）→ 自己拼消息 → 直接调 `run_model_loop`，
**不进 `bot_chat_inner`**。所以批 5 的开票代码对它们完全不生效。

### 1.3 汇报轮与用户轮共用同一行

`bot_chat/report.rs:26-33`：汇报轮把**同一个 `session_id`** 传进 `bot_chat_inner`
（`TurnOrigin::Report`）。今天因为一行只有一条，汇报轮的收尾
（`flow.rs:770` `finish_with_machine_check`）会把**用户轮那一行**刷成 idle/failed——
一个没人设计过的副作用：子 agent 回报一到，主聊天的工作项就被关掉了。

### 1.4 失败被新消息抹掉

`db/work_items.rs:109`：`stage = CASE WHEN stage = 'failed' THEN 'working' ELSE stage END`。
用户再发一句话 → 失败行被翻成 working → `failed` 不再是 person 族 → 聚合面里那条
失败**当场消失**（`error` 列还在，但没人看）。

### 1.5 聚合面压平了会话维度

`db/work_items.rs:246-251` `list_needing_human` → `list_by_stages`：按 stage IN (person 族)
取行、`ORDER BY updated_at DESC` 截断。同会话多行会被**平铺**成多条独立事项
（今天因为一行只有一条，不存在这个问题）。

### 1.6 可抄的先例：`exec_traces` 已经带 task_id 与 origin

`bot_chat/exec.rs:434-442`：任务/作业执行建 trace 时传了
`begin_trace(&app, &sid, Some(task.id.clone()), origin.as_str(), Some(task.title.clone()))`。
**trace 早就是「一次执行 = 一行 + 带来源」**。工作项照抄这个模型即可，不必另发明
来源体系（`exec_traces` DDL 见 `db/trace.rs:68-95`）。

---

## 二、目标模型：一次「干活的尝试」= 一行

### 2.1 行是什么

**行 = 一次干活的尝试**（一次模型循环的完整生命周期）。具体对应：

| 来源 | 一次尝试 = | kind | origin_ref | session |
|---|---|---|---|---|
| 主聊天 | 用户发的一条消息引发的回合 | `chat` | —（可留空） | 用户会话 |
| 任务卡执行 | 点 🤖 执行这张卡一次 | `task` | `task_id` | **新建**执行会话 |
| 定时作业执行 | 作业到点跑一次 | `scheduled` | `job_id` | **新建**执行会话 |
| 聊天批量执行 | 批量里的一张卡跑一次 | `batch` | `task_id` | **新建**执行会话 |
| 任务卡逐步执行 | 逐步执行模式跑一轮 | `task` | `task_id` | **新建**执行会话 |
| 汇报轮（子 agent 回报） | **不开票** | — | — | — |
| 子 agent | **不开票**（`subagents` 表并存） | — | — | — |

**汇报轮不开票**是本方案的一项修正：它不是「用户的事」，且今天它会关掉用户轮的行
（§1.3）。

### 2.2 统一键：`(session_id, turn_seq)`

因为任务卡与定时作业**每次执行都新建会话**（`exec.rs:405`），`(session_id, turn_seq)`
对全部来源通用：

- 主聊天：同一会话内多条消息 → `turn_seq` 递增；
- 任务卡/作业：每次执行新会话 → `turn_seq` 恒为 1。

`turn_seq` 取值：同一会话内 `MAX(turn_seq) + 1`，在**同一个写事务**里算
（`DB_WRITE_LOCK` 已保证进程内写者互斥，`db/mod.rs:372`）。

### 2.3 与既有表的边界（**不合并**，沿用方案 §7.4）

| 表 | 定位 | 与 work_items 的关系 |
|---|---|---|
| `subagents` | 派给子 agent 的活（含机器验收、digest 修复） | **并存，不合并**。子 agent 有自己的生命周期状态机与幂等键，动它风险高收益低 |
| `tasks` | 用户看板卡片（三列） | **并存，不合并**。用 `origin_ref` 关联；工作项记录「执行尝试」，任务卡记录「要做的事」 |
| `exec_traces` | 观测面（自述「不参与任何业务判定」） | **并存，不合并**。trace 是观测，工作项是执行单元——但**行模型对齐**（一次执行一行 + 带来源） |
| `notifications` | 待用户决策的消息 | **并存**。聚合在视图层做，不改它的表 |

---

## 三、表设计（DDL + 迁移）

### 3.1 新 DDL

```sql
CREATE TABLE IF NOT EXISTS work_items (
   id             TEXT PRIMARY KEY,      -- wi_{uuid}
   kind           TEXT NOT NULL,         -- chat | task | scheduled | batch
   origin_ref     TEXT,                  -- task_id / job_id
   session_id     TEXT,
   turn_seq       INTEGER NOT NULL DEFAULT 1,
   objective      TEXT,                  -- 这一回合要干什么（用户输入摘要 ≤40 字）
   acceptance     TEXT,                  -- JSON 数组：机器可判据
   stage          TEXT NOT NULL,         -- 这一回合的阶段/结局
   result         TEXT,                  -- JSON：结构化结论（含 machineCheck）
   error          TEXT,
   created_at     INTEGER NOT NULL,
   updated_at     INTEGER NOT NULL
 );
-- 同一会话内按回合唯一（不再是「会话唯一」）
CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_session_turn
  ON work_items(session_id, turn_seq) WHERE session_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_items_stage ON work_items(stage);
-- 按来源查（任务卡/作业的执行历史）
CREATE INDEX IF NOT EXISTS idx_work_items_origin ON work_items(kind, origin_ref);
```

### 3.2 迁移（批 5 的表是「打开即建空表，无迁移」，改结构必须补迁移）

**先例**：`db/schedule_jobs.rs:48-62` `ensure_scheduled_jobs_columns` 用
`ALTER TABLE ... ADD COLUMN` 补列（批 8 加 `mode` 列就是这么做的）。本方案沿用同一手法，
**复用既有助手** `db::ensure_columns`（`db/mod.rs:86-109`，幂等 + 类型串可带约束）：

```rust
// ensure_work_items_columns：幂等，老库打开即补
ensure_columns(conn, "work_items", &[("turn_seq", "INTEGER NOT NULL DEFAULT 1")])?;
conn.execute_batch("
  DROP INDEX IF EXISTS idx_work_items_session;                               -- 旧「会话唯一」索引退役
  CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_session_turn
    ON work_items(session_id, turn_seq) WHERE session_id IS NOT NULL;
  CREATE INDEX IF NOT EXISTS idx_work_items_origin ON work_items(kind, origin_ref);
")?;
```

> 幂等性由助手自带：已存在的列直接跳过，「duplicate column」错误也吞掉
> （`db/mod.rs:99-105`）——调用方不必自己判存在。索引部分用 `IF NOT EXISTS` 保证重复执行安全。

**存量行的归属**：老库里每会话一行 → 迁成 `turn_seq = 1`（等价于「该会话的第 1 回合」）。
复合唯一索引仍然成立（老行 session_id 唯一 → (session_id, 1) 唯一）。

**迁移风险评估**：低。批 5 于 2026-11-04 07:50 落地，**实机验收尚未执行**，
用户库里 `work_items` 基本无行；且老行即使存在也只是「一个会话一行」的粗粒度快照，
迁成 turn_seq=1 不丢信息。

### 3.3 迁移口径三条

1. **幂等**：重复打开应用不得重复加列（用 `PRAGMA table_info` 判存在，或捕获重复列错误）；
2. **不删数据**：只加列、建索引、退役旧索引，**不重建表**（避免丢行）；
3. **失败即上抛**：迁移失败必须让启动报错，**不许**「跳过迁移照常启动」——
   照常启动的后果是唯一索引缺失 → 同一会话多行不被拦 → G1 的粒度保证失效。

---

## 四、寻址模型：三处穿线（本方案的硬骨头）

### 4.1 行 id 的来源

开票函数改为**只插不更新**，并返回新行的 id：

```rust
/// 开一行（一次干活的尝试）。同会话内 turn_seq 递增。
pub fn open_turn(
    conn: &rusqlite::Connection,
    session_id: &str,
    kind: &str,
    objective: Option<&str>,
    origin_ref: Option<&str>,
) -> Result<WorkItemRow, String>
```

原 `upsert_for_session`（`db/work_items.rs:88-167`）的「有则更新」分支**整体退役**
（不再有「刷新 objective」这回事——每回合自带 objective）。

### 4.2 轮收尾：局部变量直传

`bot_chat/flow.rs:653-681` 的开票块改成拿住 `work_item_id`，收尾处
（`flow.rs:761-775`）直接传 id：

```rust
let work_item_id: Option<String> = /* 开票返回值 */;
// …模型循环…
crate::db::work_items::finish_with_machine_check(&app, work_item_id.as_deref(), ok, err)
```

`finish_with_machine_check` 内部改按 id 读行（不再 `load_by_session`），
判据仍读该行的 `acceptance` 列（批 6 的管线不变）。

### 4.3 `ask_user` 挂起：走 StopGuard 穿线

挂起点在 `ask_engine.rs:516-532`，它只拿得到 `session_id`。改法：

1. `StopGuard` 加字段 `work_item_id: Option<String>`（`bot_slash.rs:29-37`），
   配一个 builder：`stop.with_work_item_id(id)`（不动 `StopGuard::new` 的现有 6 个调用点）；
2. 开票后立刻 `let stop = stop.with_work_item_id(id);`（`flow.rs:310` 建 guard 的地方）；
3. `engine_ask_user` 已有 `stop: Option<&StopGuard>` 参数
   （`ask_engine.rs:394`）→ 直接读 `stop.and_then(|s| s.work_item_id())`；
   工具调用点 `bot/registry/handlers.rs:255` **不用改**（`ctx.stop` 已经透传）。

**为什么走 StopGuard 而不是全局注册表**：StopGuard 本来就是「本次执行」的令牌，
已带 `session_id`（`bot_slash.rs:33`）并流经模型循环到工具上下文
（`ctx.stop`）。全局表要处理并发与清理，StopGuard 不用。

### 4.4 汇报轮不开票（顺带修掉刷行隐患）

`flow.rs:653-681` 的开票块加条件：`origin == TurnOrigin::User` 才开票。
汇报轮（`TurnOrigin::Report`）**不开票、不改行、不收尾**——
今天它把用户轮的行刷成 idle 的副作用（§1.3）随之消失。

### 4.5 寻址漏点清单（改造时逐条对拍，防漏）

| 漏点 | 位置 | 处置 |
|---|---|---|
| 逐步执行模式的三处 guard | `exec_steps.rs:378` / `:470` / `:486` | 逐步执行也开票（kind=task），guard 带 id |
| 任务卡/作业执行的 guard | `exec.rs:448` | 开票后 `with_work_item_id` |
| 主聊天 guard | `flow.rs:310` | 同上 |
| 子 agent guard | `bot/subagent_runner.rs:475` | **不传**（子 agent 不开票） |
| eval 框架 guard | `bot/eval.rs:200` | **不传**（评测跑批，不入业务面） |
| 其它 guard | `lib.rs:1133`、`apps/commands.rs:86` | **不传**（非工作项场景） |

---

## 五、失败语义与聚合口径

### 5.1 stage 语义修订：stage = 那一回合的结局

- 翻转唤醒**删除**：`db/work_items.rs:109` 的 `CASE WHEN stage='failed' THEN 'working'` 整条退役
  （失败是那一回合的结局，新回合是新行——没有「唤醒」这回事了）；
- `set_stage` 改 `set_stage_by_id`（按行 id）；
- `finish` 改 `finish_by_id`（按行 id），`COALESCE` 语义保留。

### 5.2 两个派生概念：未决 / 已作废

| 概念 | 定义 | 用途 |
|---|---|---|
| **未决** | `stage ∈ person 族` ∧ 该行之后**没有更新的用户回合** | 报警（要人处理） |
| **已作废** | `stage ∈ person 族` ∧ 该行之后**有更新的用户回合** | 留痕，不报警 |

「之后有没有更新的用户回合」用 `EXISTS (SELECT 1 FROM work_items w2 WHERE w2.session_id = w.session_id AND w2.turn_seq > w.turn_seq AND w2.kind = 'chat')` 判定。

> 这条判定保住了方案 §7.3 的原意（**失败后用户又说话了 → 不用再管**），
> 同时数据一行不删（G3）。

### 5.3 聚合面新口径

`needs_attention_list`（`db/work_items.rs:396-452`）的三源不变
（工作项 person 族 / 失败子 agent / 失败作业），工作项源改为带 `superseded` 标记：

```
NeedsAttentionItem { source, id, title, detail, at, session_id, superseded }
```

- `superseded=false` → 前端渲染「**待你处理**」组；
- `superseded=true` → 渲染「**历史未决**」组（折叠，默认收起）。

**去重要点**：定时作业的失败会**同时**命中两个源——
① 作业源按 `scheduled_jobs.last_status='fail'`（`db/work_items.rs:435-445`）
② 工作项源按 kind='scheduled' 的失败行。
**同一 `job_id` 必须合并成一条**（取最新），否则批 7 的聚合面会重复报警。

### 5.4 与方案 §7.3 的偏离登记

方案 §7.3 的 stage 表里 `failed` 定义为「报告失败，**且此后无人消息**」。
本方案把它拆成「stage=failed（数据层，不含时间条件）」+「未决/已作废（派生层，含时间条件）」——
**stage 列的取值域不变**（仍是那 8 个词），只是「是否需要人管」不再由 stage 单值决定。
§10 记这条偏离。

---

## 六、任务卡与定时作业接入（G5）

### 6.1 开票点下沉到 `run_task_in_chat_with`

`bot_chat/exec.rs:344-470` 已有三样现成的东西：

1. `origin: TaskExecOrigin` 参数（`exec.rs:347`）——`Manual` / `Scheduled` / `Batch`；
2. `create_exec_session` 建的新会话 id（`exec.rs:405`）；
3. trace 已带 `task_id` + `origin`（`exec.rs:434-442`）。

**改造点**：在 `create_exec_session` 之后、模型循环之前开一行：

```rust
let work_item_id = open_turn(
    &conn, &sid,
    kind_from_origin(origin),      // manual → task / scheduled → scheduled / batch → batch
    Some(&task.title),              // objective = 任务卡标题
    Some(&task.id),                 // origin_ref = task_id
)?;
```

### 6.2 `origin_ref` 的两种来源

| kind | origin_ref | 说明 |
|---|---|---|
| `task` / `batch` | `task_id` | 已有（`run_task_in_chat_with` 的 `task_id` 参数） |
| `scheduled` | **`job_id`** | **需新穿一个参数** |

定时作业的现状：`bot_scheduler.rs:594-598` 调
`run_task_in_chat(&app, &card.id, TaskExecOrigin::Scheduled)`——
传进去的是**作业建的卡片 id**，不是 `job_id`，两者无法互推（卡是作业跑出来的产物）。
**处置**：`run_task_in_chat` / `run_task_in_chat_impl` / `run_task_in_chat_with`
三个函数加一个 `origin_ref: Option<&str>` 参数（`bot_scheduler.rs` 传 `Some(&job.id)`，
其余传 `None` 即回落到 `task_id`）。

### 6.3 逐步执行模式

`exec_steps.rs:262` 跑模型循环、`:378/:470/:486` 建 guard。逐步执行也是「任务卡的一次
执行」→ 开票（kind=task，origin_ref=task_id），guard 带 id。
（逐步执行的「挂起等确认」语义与 `awaiting_answer` 不同，不复用 stage 词——留 `working`。）

### 6.4 子 agent 不开票

沿用方案 §7.4：`subagents` 表并存不合并。子 agent 的失败/超预算**已经**在批 7 的
聚合面里（源②），不重复进工作项源。

---

## 七、批次切分与依赖

> 每批一个逻辑变更、一次提交。行为变更换测试。提交前人眼看 diff。

| 批 | 内容 | 依赖 | 用户可见行为变化 | 规模 |
|---|---|---|---|---|
| **A** | 表结构迁移（加 `turn_seq`、换唯一索引、加 origin 索引）+ 寻址改行 id（`open_turn` / `finish_by_id` / `set_stage_by_id` / `load_by_id`） | — | 无（数据面） | 中 |
| **B** | 主聊天按回合开票 + 失败语义改口径（删唤醒）+ 汇报轮不开票 + StopGuard 穿线（`ask_user` 挂起按行 id） | A | 轻微（工作项不再按会话合并） | 中 |
| **C** | 任务卡 / 定时作业 / 批量 / 逐步执行接入开票（kind + origin_ref + `origin_ref` 参数穿线） | A, B | **有**（执行历史可查） | 中 |
| **D** | 聚合面新口径（未决 / 已作废两组 + 作业与工作项去重）+ 前端渲染 | B, C | **有**（通知中心多一组） | 中 |
| **E** | 文档同步（SPEC 6-118 修订 / 追踪矩阵 / ACC-P-13 卡 / 模块地图）+ 冒烟卡改写 | A–D | 无 | 小 |

**与批 5–8 实机验收的关系**：批 5–8 的冒烟卡（`docs/realmachine-smoke-test-2026-11-04.md`）
里 **M137 / M138 / M144 / M151 四张判据直接反转或作废**（每会话一行 → 每回合一行）。
建议顺序：**先跑完批 5–8 的 24 张卡拿到实机结论**（顺带验证批 8 的 R-1 缺陷面），
**再动粒度**——否则等于按读码预判改设计，且改完连「改之前是什么样」都没有基线。
若要先改，则需把上述四张卡改写为新语义，并补批 A–E 的新卡。

---

## 八、风险与回归面

| 风险 | 缓解 |
|---|---|
| 寻址漏改 → `finish` 把同会话多行一起刷 | §4.5 漏点清单逐条对拍；单测断言「同会话两行，收尾只改一行」 |
| 汇报轮 / 子 agent 误开票 → 行数暴涨、聚合面噪声 | §4.4 条件开票 + §6.4 不开票；单测断言汇报轮不产生行 |
| 迁移漏做 → 唯一索引缺失，粒度保证失效 | §3.3 迁移失败即上抛，不许静默跳过 |
| 聚合面重复报警（作业源 × 工作项源） | §5.3 同 `job_id` 合并 |
| 「已作废」判定过宽 → 失败永远不报警 | 判定只用「之后有更新的**用户**回合」，作业/任务回合不覆盖它 |
| 行数膨胀（长会话多轮、作业天天跑） | 表小；列表一律带 limit；保留期清扫**本批不做**（YAGNI，量大了再说） |
| 与批 5–8 冒烟卡冲突 | §七 顺序建议；先跑完再改 |
| 前端两组的交互没定 | §十一 P1 待拍板 |
| **开票与收尾的落地次序竞态**（现状缺陷，粒度无关）——两者都是 fire-and-forget 的 spawn，收尾 UPDATE 若先于开票 INSERT 落地则静默 no-op，行停在 `working` | 本方案批 B 改写时把开票/收尾的次序钉死（收尾按行 id 写，不再依赖「行已存在」）；冒烟卡 M141（V156 反向锚点）复验 |
| **崩溃后 `awaiting_answer` 行永久报警**（现状缺陷，粒度无关）——启动清扫只收 `working`，而 ask waiter 在内存里，重启后等待器全失 | 修法：启动清扫把 `awaiting_answer` 一并按中断收（重启后没有活着的 waiter）；只在启动做、不进 tick（tick 里它可能是合法长挂）；冒烟卡 M142（V157 修复型锚点）复验 |

---

## 九、文档同步清单

落地时按 [AGENTS.md](../AGENTS.md) 的硬要求同步：

| 文档 | 时机 | 动作 |
|---|---|---|
| `SPEC.md` | 批 A–D 落地时 | 6-118 条目改写：粒度从「每会话一行」改为「每回合一行」，补任务卡/作业接入与未决/已作废口径 |
| `docs/acceptance/matrix.csv` | 同上 | 6-118 行状态与「无实机卡」备注更新 |
| `docs/acceptance/cards-06-orchestration.md` | 批 E | ACC-P-13 卡按新语义改写（步骤 1/2/3 的判据逐条改） |
| `docs/rust-bot-architecture.md` | 批 A/C 落地时 | 模块地图（`tests-audit/audit_module_map.py` 对拍） |
| `docs/AGENT-RUNTIME-REFIT-PLAN-2026-11-02.md` | 批 E | 补「修订 9」：本文取代 §7.2 / §7.3 / §7.6 |
| `docs/realmachine-smoke-test-2026-11-04.md` | 批 E | M137/M138/M144/M151 改写；新增批 A–E 的卡 |
| `DEVLOG.md` | 批 A / B 落地时 | 只写「为什么换方向」：为什么从每会话一行改成每回合一行 |
| `tests-audit/audit_file_size.py` | 全程 | 新文件 ≤1500 行 |

---

## 十、偏离登记

| # | 偏离 | 依据 | 处置 |
|---|---|---|---|
| 1 | 粒度从「每会话一行」改为「每回合一行」 | 用户 2026-11-04 拍板（四诉求全收） | 方案 §7.2 的粒度拍板（1A）被本文取代；SPEC 6-118 改写 |
| 2 | `failed` 的「此后无人消息」条件从 stage 里拆出，改为派生层 | 用户拍板 G3（失败不被抹）+ G4（全量呈现） | 方案 §7.3 的 stage 语义改口径；stage 取值域不变 |
| 3 | 汇报轮不开票 | 汇报轮不是用户的事，且今天会关掉用户轮的行 | 方案 §7.6「每轮模型循环开票」加条件 |
| 4 | `mandate` 列仍不建 | 用户拍板 3A；本方案不引入授权语义 | 无变化 |
| 5 | 保留期清扫本批不做 | YAGNI：量小 | 量大了再议 |

---

## 十一、待拍板项

| # | 待拍板 | 推荐 | 影响 |
|---|---|---|---|
| P1 | 「已作废」组在通知中心怎么摆：同一区块内折叠分组，还是单独页签/入口？ | 同一区块内折叠（默认收起），标题写「历史未决（N）」 | 只影响前端布局，改起来便宜 |
| P2 | 任务卡/作业的工作项要不要进「后台待办」的**待你处理**组？作业失败时同一 job 已有作业源和工作项源 | 合并成一条（按 job_id），来源标签显示「定时作业」 | 影响 §5.3 的去重实现 |
| P3 | 「已作废」的判定用「之后有更新的用户回合」还是「之后有更新的任何回合（含作业/任务）」？ | 只认用户回合（作业跑一轮不代表用户那边的事被办了） | 影响 §5.2 的 SQL 谓词 |
| P4 | 是否给 `work_items` 加保留期清扫（执行历史会一直长） | 本批不做 | 后续量大了再议 |

---

## 十二、结语

本方案的核心不是「把一行拆成多行」，而是**把「会话」从工作单元的身份换成工作单元的容器**：
身份落到「一次干活的尝试」上（回合 / 执行），会话只用来把同一条线上的尝试串起来。
这一换，G1–G4 四个诉求同时成立；G5（任务卡与作业接入）只是把开票点下沉到本来就有的
执行链路上——`exec_traces` 早就是这么做的（§1.6），工作项只是把同一套行模型用在
**可判定**的那一层。

代价集中在两处：**寻址要穿线**（§4，三处漏点已列）与**聚合面要分两组**（§5.3）。
前者是机械活（加一个 StopGuard 字段 + 改五个函数签名），后者是口径活（去重 + 分组渲染）。
都不大，但都必须一次做对——寻址漏一处就是「收尾刷了别的行」这种难查的错。
