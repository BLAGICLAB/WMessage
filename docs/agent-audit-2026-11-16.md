# wmessage Agent 全量功能审计报告（2026-11-16）

> **性质**：现状审计（只读）。本报告只记录**当前代码的事实**，不修代码、不改门禁。
> **范围**：Agent 核心链路全量——模型循环、聊天流、工具层与注册表、子 agent 编排、
> 技能、MCP 与外部接入、持久化与数据层、调度与自动化、应用与平台横切、前端全量。
> **口径**：历史审计（10-16 / 10-19 / 10-21 / 10-22 四轮）已修项不在本报告重复出现；
> 所有发现都是对着 `f35f3608` 这一版代码实读得出的。

---

## 一、怎么审的

### 1.1 方法

四条腿并行，结论互相印证：

| 腿 | 做法 | 覆盖 |
|---|---|---|
| **自动化门禁** | 跑仓库自带的全部门禁 + 全量测试 | 客观基线，零主观 |
| **模块簇实读** | 6 个只读审计子代理，按模块簇分工，逐条给 `file:line` 证据 | 人工深读 |
| **横切对拍** | 自写脚本做全仓级机械对拍（事件名、孤岛符号、常量副本、孤儿命令） | 跨模块一致性 |
| **主审复核** | 每条 P0/P1 由我本人回到代码逐行复验，不采信子代理的单方结论 | 定级与定性 |

### 1.2 客观基线（2026-11-16 实跑，全绿）

| 检查 | 命令 | 结果 |
|---|---|---|
| Rust 单元/集成测试 | `cargo nextest run` | **2595 通过 / 0 失败 / 12 跳过** |
| Rust lint | `cargo clippy --all-targets` | **0 警告** |
| Rust 未用依赖 | `cargo machete` | **0 个** |
| 前端单测 | `vitest run` | **1252 通过 / 125 文件全绿** |
| 前端类型 | `tsc --noEmit` | **0 错** |
| 前端死代码 | `knip` | **0 条**（无死文件/死导出/死依赖） |
| 前端 lint | `oxlint src` | 0 error，10 条 warning（8×set-state-in-effect、1×array-index-key、1×await-in-loop） |
| 一致性对拍门禁 | `tests-audit/` 9 个 pytest + 1 个独立守卫 | **58 通过**，1 条 warn |
| 前端动态求值守卫 | `tests-audit/audit_no_eval.py` | **通过**（192 文件零命中） |

> **结论先行**：这不是一个「到处是坑」的仓库。自动化面全绿、无孤儿符号、
> 无未用依赖、事件契约双向闭合。真正的问题集中在**少数几条跨模块接缝上**——
> 它们恰好是自动化门禁看不见的地方（编译通过、测试全绿，但用户仍会踩到）。

### 1.3 横切对拍结果（自写脚本）

| 对拍 | 口径 | 结果 |
|---|---|---|
| 事件名注册表 ↔ 前后端 | `docs/acceptance/event-names.md` 33 条注册事件 | **33/33 双向存在**，无「前端收后端不发」也无「后端发前端不收」 |
| Rust 孤岛 pub 符号 | 全仓词频扫描（定义处 = 唯一出现处） | 3 个候选，**全部确认在用**（`tests/skill_e2e.rs` 的集成测试钩子），**Rust 侧无死代码符号** |
| 前端死代码 | knip + 自查 | **无** |
| 风控常量手写副本 | 扫描 `constants.rs` 外的数值副本 | 2 处候选，均为**显式声明的「同值不同域」**（见 P3-4），非缺陷 |
| 已注册未消费命令 | `tests-audit/audit_tauri_bridge.py` | **4 条**（门禁已报警，但设计成 warn-only 不拦）——见 P1-4 |

---

## 二、结论速览

| 级别 | 条数 | 说明 |
|---|---|---|
| **P0** | **0 条** | 未发现数据损坏、凭据泄露、死锁挂死级缺陷 |
| **P1** | **8 条** | 用户可见的错误行为 / 凭据丢失 / 契约断裂 / 运行时整体阻塞 |
| **P2** | **20 条** | 边界、一致性、运行限制类风险 |
| **P3** | **14 条** | 死代码、文档漂移、代码卫生 |

**一句话结论**：仓库的自动化纪律（门禁/测试/单源真相）执行得很到位，
死代码几乎为零（前端 1 个、Rust 侧 0 个）；风险集中在**接缝**——
凭据迁移链、终稿自检链、子 agent 状态收敛链、总开关门控链、流式去重链、
异步阻塞边界，都是「每个模块单独看都对，合起来就出事」的类型。

**P1 分布**（按用户可感知程度排序）：

| # | 一句话 | 面 |
|---|---|---|
| P1-1 | MCP 凭据迁移被 4 条写路径绕过 → 凭据永久丢失 | 配置/凭据 |
| P1-2 | 终稿自检轮被幻觉守卫劫持 → 模型改口后的答复被吞 | 模型循环 |
| P1-3 | 续跑成功的子 agent 残留旧 error → 前端渲染成失败态 | 子 agent |
| P1-4 | 4 个已注册命令无消费方（2 个是已写好但 UI 无入口的能力） | IPC/UI |
| P1-5 | 一次性 `at:` 作业在总开关关闭期间被**静默删除** | 调度 |
| P1-6 | 总开关在作业/提醒/截止通知三条链路口径不一致 | 自动化 |
| P1-7 | `lookup_receipt` 裸跑同步 SQLite → 阻塞整个 Tauri 运行时 | 工具层 |
| P1-8 | `bot-think-delta` 漏掉去重闸 → 回灌重放时思考文本重复 | 前端流式 |

---

## 三、P1 发现（8 条）

### P1-1 · MCP 机密迁移的不变式被 4 条配置写路径绕过 → 用户 MCP 凭据永久丢失

**位置**
- `src-tauri/src/bot/mcp/builtin.rs:287-288`（启动补写内置条目）
- `src-tauri/src/bot/config/commands.rs:400`（`bot_set_config`）
- `src-tauri/src/bot/config/commands.rs:489`（`bot_set_active_model`）
- `src-tauri/src/bot/config/commands.rs:530`（`bot_set_perm_mode`）

**证据**

`env` / `headers` 是机密字段，任何序列化都会把它们从文件里剥掉：

```rust
// src-tauri/src/bot/mcp/config.rs:101 / :108
#[serde(skip_serializing, default)]
pub env: BTreeMap<String, String>,
...
#[serde(skip_serializing, default)]
pub headers: BTreeMap<String, String>,
```

启动时的迁移**失败只告警，不中止启动**：

```rust
// src-tauri/src/lib.rs:529-537
{
    let g = crate::bot::config::io::lock_config_write();
    if let Err(e) = crate::bot::config::io::migrate_mcp_server_secrets_locked(&handle) {
        log::warn!(target: "bot::migrate", "mcp secret migration failed: {e}");
    }
    drop(g);
}
```

而写回路径只保护了 api/tavily/brave/deepseek 四个 key 槽（`config/io.rs:483-488`
的 `ensure_legacy_plaintext_backed`），**不覆盖 MCP 的 env/headers**。
上述 4 条写路径都直接调 `write_bot_config_file_locked`，**写前没有先跑迁移**。

代码自己写明了这条不变量——而 builtin 这条路径恰恰违反了它：

```rust
// src-tauri/src/bot/mcp/builtin.rs:274-288
/// 失败只审计不阻断启动——内置服务器是增强项，写不进去只让它缺席。
///
/// 必须在启动的配置迁移**之后**调用：明文 env/headers 未迁进凭据存储就写回，
/// 会被 `skip_serializing` 剥掉等于静默丢配置。
pub fn materialize_at_startup(app: &tauri::AppHandle) {
    ...
    let outcome =
        crate::bot::config::io::write_bot_config_file_locked(&crate::db::data_dir(app), cfg);
```

**现象 / 影响**
启动迁移因 keyring 不可用而失败（只 warn）→ 配置文件里仍是老明文 env/headers
→ 用户随后点「保存设置 / 切模型 / 切授权档」任一动作，整份配置回写
→ `skip_serializing` 把 env/headers 从文件剥掉，而 keyring 里从没有过副本
→ **MCP 服务器的 API 凭据永久消失，且零审计事件**（用户查不到发生了什么）。
`bot_set_perm_mode` 尤其隐蔽：它宣称「窄口径改授权模式、不做整份配置写回」，
实际仍走整份 `write_bot_config_file_locked`。

**建议**
这 4 条路径写前先 `migrate_mcp_server_secrets_locked`，失败即中止本次写
（与 `mcp/commands.rs:47` 的 `with_locked_config` 同款：失败中止，明文原样保留）。
另外把 `audit_tauri_bridge` 那类「写路径必须先迁移」的不变量做成门禁，
否则下次新增写路径还会漏。

---

### P1-2 · 终稿自检轮被幻觉守卫劫持 → 模型改口后的答复被快照回滚吞掉

**位置**
- `src-tauri/src/bot_model_loop/guard.rs:212-223`（守卫先跑）
- `src-tauri/src/bot_model_loop/finalize.rs:337-379`（verdict 后解析）
- `src-tauri/src/bot/registry/claims.rs:27-42`（声称判定无否定词处理）
- `src-tauri/src/prompts/selfcheck.rs:24-26`（自检提示词）

**证据**

收尾判决的顺序是「自检回复 → **幻觉守卫** → 出口段 → 终稿自检」：

```rust
// guard.rs:189-257（finalize_without_tools 内）
match finalize_selfcheck_reply(...)? { ... }          // ① 工具自检回复
let ev = guard_collect(...)?;                        // ② 幻觉守卫取证
match guard_verdict(...)? {                          // ③ 守卫判决（先于终稿自检）
    FinalizeOutcome::Proceed => {}
    other => return Ok(other),
}
match finalize_exit(...)? { ... }                    // ④ 出口段
match finalize_selfcheck(...).await? { ... }         // ⑤ 终稿自检 verdict 解析
```

守卫的触发条件之一是「变更类声称」，判据是**纯子串匹配，无否定词豁免**：

```rust
// stream.rs:31-37
pub(crate) fn claim_mutation_fires(mutation_done: bool, unverifiable_ran: bool, text: &str) -> bool {
    !mutation_done && !unverifiable_ran && claims_mutation(text)
}
```

而终稿自检的提示词恰恰要求模型「回答后不要解释」：

```rust
// prompts/selfcheck.rs:24-26
pub(crate) const FINAL_SELFCHECK_PROMPT: &str = "\
【终稿自检】本轮回复（开头 500 字）：{snippet}。\
仅回答：Y=无幻觉，N=包含幻觉或虚构事实。回答后不要解释。";
```

但仓库自己知道模型不听这句——宽松解析器就是为此加的：

```rust
// prompts/selfcheck.rs:29-31
/// 宽松判定：取首个非空白字符是 Y/N 即认 verdict。
/// 终稿自检的 verdict 解析专用
/// ——模型常在单字判定后附加解释（"N，日期写错了"），整段严格匹配会把它当
/// 通过处理，幻觉自白被静默丢弃。
```

**现象 / 影响**
自检轮模型答「N，我刚才说已创建了任务卡」这类**带解释的 N** →
文本含「已创建」→ 守卫命中变更声称 → 返回 `Continue`（第一次返工）
→ **verdict 从未被消费**，`expecting_final_selfcheck` 仍为 true
→ 下一轮模型的正式答复被流式抑制（`sse.rs:376`）并再次当作 verdict 解析
→ 首字符非 Y/N → `finalize.rs:360` 的 `None` 分支**还原旧快照**
→ 用户看到的仍是自检前的旧答复，新答复与「⚠️ 自检未达预期」警告双双消失，
只留一条 `final_selfcheck.reply_unparsed` 的 Warn 审计。

净效果：**终稿自检这条「防幻觉」机制，在模型照实解释时反而把模型改口后的正确答复吞掉**，
且用户无感。

**建议**
把 verdict 解析提到守卫之前；或在守卫命中时先消费 verdict 并清
`expecting_final_selfcheck`，使守卫只在「非自检答复」文本上判定。
配套锚点测试：喂一条含变更声称的「N，……」答复，断言走 N 分支而不是 Continue。

---

### P1-3 · 续跑成功的子 agent 行残留上一轮 error → 前端渲染成「失败」态

**位置**
- `src-tauri/src/db/subagents.rs:557-560`（状态更新 SQL）
- `src-tauri/src/db/subagents.rs:570-593`（续跑重开）
- `src/components/ChatPanel/SubagentRowLine.tsx:49`（前端状态映射）

**证据**

状态更新用 `COALESCE`，成功时传 `None` → **旧 error 保留**：

```rust
// db/subagents.rs:557-560
conn.execute(
    "UPDATE subagents SET status=?2, started_at=?3, finished_at=?4, error=COALESCE(?5, error) WHERE id=?1",
    rusqlite::params![id, to.as_str(), started_at, finished_at, error],
)
```

续跑重开**只清 status/时间，不清 error**（注释里是刻意的：保留熔断原因作历史）：

```rust
// db/subagents.rs:565-593
/// 跨进程续跑的唯一行状态入口（resume_subagent 工具调用）：预算熔断行翻回
/// queued 重入派发管线。…error 字段保留作历史（熔断原因不抹）…
conn.execute(
    "UPDATE subagents SET status='queued', started_at=NULL, finished_at=NULL WHERE id=?1",
    rusqlite::params![id],
)
```

前端把「有 error 且不在跑」直接映射成失败态：

```tsx
// src/components/ChatPanel/SubagentRowLine.tsx:47-52
function statusOf(s: SubagentRun): keyof typeof STATUS_ICON {
  if (s.status === "cancelled") return "cancelled";
  if (s.status === "failed" || (s.error && !s.running)) return "failed";
  if (s.running) return "running";
  return "done";
}
```

**现象 / 影响**
resume 一个 `budget_exceeded`（error = `max_tool_calls`）的子 agent →
它跑成功了 → 行 status = `succeeded`，error 仍是 `max_tool_calls`
→ 前端渲染成 ❌ 失败态，hover 提示写「（max_tool_calls）」
→ `check_subagent` 同时回传 `status=succeeded` + `error=max_tool_calls`
（`bot_orchestrator.rs:273-281`），主 agent 按 addendum「先读 error 与 summary」的
指引会把**一次成功的续跑误判为失败**。

**建议**
终态为 `Succeeded` 时显式写 `error=NULL`（或在 `update_subagent_status` 对成功终态
单开分支）；`COALESCE` 保留语义只应覆盖「同态重入」，不覆盖「换终态」。

---

### P1-4 · 4 个已注册的 tauri 命令无任何前端消费方（死代码 + 功能缺口）

**位置**
- `src-tauri/src/lib.rs:819`、`:821`、`:876`、`:877`（注册）
- `src-tauri/src/db/work_items.rs:463`、`:478`（命令实现）
- `src-tauri/src/bot/mcp/oauth.rs:614`、`:629`（命令实现）

**证据**

仓库自带的门禁**已经报警**，但设计成 warn-only 不拦：

```
tests-audit/audit_tauri_bridge.py::test_registered_but_uncalled_warn_only
  UserWarning: 已注册但前端未调用的命令（仅提示，可能有内部/测试用途）：
  mcp_oauth_logout, mcp_oauth_status, work_items_list, work_items_needing_human
```

前端只有「OAuth 登录」一个入口，**没有登出、没有状态显示**：

```tsx
// src/components/SettingsPage/McpPanel.tsx:630-638
{s.transport === "http" && (
  <button … onClick={() => void oauthLogin(s)}>
    {oauthBusyId === s.id ? "等待授权…" : "OAuth 登录"}
  </button>
)}
```

**现象 / 影响**

| 命令 | 性质 | 影响 |
|---|---|---|
| `mcp_oauth_logout` | 完整实现，**UI 无入口** | 用户无法手动撤销授权；`bot-mcp-oauth.json` 里的令牌只能被下一次登录覆盖 |
| `mcp_oauth_status` | 完整实现，**UI 无入口** | 用户看不到某台服务器是否已授权、何时过期；只能靠调用报错反推 |
| `work_items_list` | 完整实现 + 专属 helper `list_recent`（`db/work_items.rs:261`，**只被它调用**） | 纯死代码 |
| `work_items_needing_human` | 死壳——底层查询 `list_needing_human` 被聚合面 `needs_attention_list` 复用（`db/work_items.rs:412`），活的 | 纯死壳 |

连带影响：11-07 冒烟方案里的验收卡要求「登出（`mcp_oauth_logout`）后调用 →
明确的『授权已过期，请重新登录』类人话错误」，**用户根本无法从 UI 触发登出**——
该卡的判据在产品里不可达。

**建议**
- `mcp_oauth_logout` / `mcp_oauth_status`：接进 McpPanel（状态角标 + 登出按钮），
  它们是**已写好的能力**，接上就能用；同时解掉一条验收卡不可达。
- `work_items_list` / `work_items_needing_human`：**接线还是删除二选一**。
  若接（活动页补工作项），需要连 `list_recent` 一起留；若删，三个符号一起删
  （含 `list_recent` 与 `lib.rs` 的注册行）。
- 把 `audit_tauri_bridge` 的这条 warn **升级为失败门禁**，或至少给出白名单机制
  ——否则「注册了没人用」会持续增量堆积。

---

### P1-5 · 一次性 `at:` 作业在「机器人总开关关闭」期间被**静默删除**

**位置**
- `src-tauri/src/bot_scheduler.rs:934-936`（`at_expired` → `Delete`，判定在 `bot_on` 之前）
- `src-tauri/src/bot_scheduler.rs:999-1005`（`Delete` 分支：只有一条审计 + 删行）
- `src-tauri/src/bot_scheduler.rs:928-947`（`bot_on` 只在 retry 与 classify_due 分支被读）

**证据**

```rust
// bot_scheduler.rs:928-947
return if bot_on { JobAction::RetryDispatch } else { JobAction::Held };
}
if at_expired(&job.schedule, job.sched_last, now) {
    return JobAction::Delete;          // ← 不看 bot_on
}
match classify_due(&job.schedule, job.sched_last, now) {
    DueVerdict::Run => {
        if bot_on { JobAction::Dispatch } else { JobAction::Held }
    }
    DueVerdict::Missed => JobAction::Missed,
    DueVerdict::NotDue => JobAction::Idle,
}
```

`Delete` 分支**不发事件、不发通知**，只有一条审计：

```rust
// bot_scheduler.rs:998-1005
// 过期一次性（从未执行且已过点）→ 删除作业（防重启后补跑过期任务）
JobAction::Delete => {
    crate::bot::audit_log(app, &format!("sched_job_stale | id: {} | 过期一次性作业，删除", job.id));
    delete_job_row(app, &job.id).await;
}
```

对比同一函数里的 `Missed` 分支——它**既发 `sched-status` 事件、也发用户通知**：

```rust
// bot_scheduler.rs:1016-1022
// 与 started/done/failed 同一事件面：错过也要让定时页的徽标
// 即时翻「已错过」，等下次 reload 才出现就是静默
let _ = app.emit("sched-status", serde_json::json!({ "jobId": job.id, "phase": "skipped" }));
notify_schedule_missed(app, &job.id, &job.content).await;
```

**现象 / 影响**
`Held` 的语义承诺是「不派发、水位不推进，恢复后仍在补跑窗口内」
（`bot_scheduler.rs:891-893`）。但 `at:` 作业走的是 `Delete` 分支——
总开关关闭期间到点的一次性作业，**60~90 秒后（`AT_FIRST_RUN_GRACE` = 2×30s）就被删行**：
没跑过、没通知、没徽章，用户的作业从定时页**凭空消失**。周期作业不受影响
（走 Held）→ 同一个开关对两类作业给出两种结果。

**建议**
`at_expired` 命中时先看 `bot_on`，关闭时改判 `Held`（与 `classify_due` 同口径）；
并给 `Delete` 分支补一条用户可见通知（至少与 `Missed` 同级），
让「过期被删」不成为静默失败。

---

### P1-6 · 机器人总开关在三条自动化链路上口径不一致：作业 Held，提醒与截止通知照常发

**位置**
- `src-tauri/src/bot_scheduler.rs:961`（作业读开关 ✅）
- `src-tauri/src/bot_reminders.rs:319-375`（提醒链路**不读**）
- `src-tauri/src/due_notify.rs:220-294`（截止通知链路**不读**）

**证据**

全仓 `bot_get_enabled` 的读取点只有 4 处，**提醒与截止通知两处都没有**：

```text
src-tauri/src/bot_slash.rs:620        定义
src-tauri/src/bot_scheduler.rs:458    作业链路 ✅
src-tauri/src/bot_scheduler.rs:961    作业链路 ✅
src-tauri/src/bot_chat.rs:220 / :659  聊天链路 ✅
（bot_reminders.rs 与 due_notify.rs 零命中）
```

```rust
// bot_reminders.rs:319  reminders_tick —— 全程不读开关
let due = ... load_due_reminders(&conn, now_ms) ...;
advance_and_enqueue(app, &r, now_ms, next.map(...)).await;   // 发通知 + 推进 remind_at
```

**现象 / 影响**
关掉「机器人总开关」后：机器人不执行作业，但用户**仍会收到提醒**与**截止前通知**；
且提醒的 `remind_at` 被**推进**（水位前进），与作业链路的「水位不推进、恢复后补跑」
恰好相反——同一个开关，一边是「记住并补跑」，一边是「照常执行并翻篇」。

SPEC.md:79 只规定了聊天页行为，对这三条链路**无口径**。

**建议**
拍板一种口径并写进 SPEC + 两个模块注释。若认定「发通知不算机器人动作」，
至少在 `bot_reminders.rs` / `due_notify.rs` 模块头注明「总开关不门控本链路」——
否则下一个人一定会按作业链路的直觉去改这两处。

---

### P1-7 · `lookup_receipt` 在 async 上下文裸跑同步 SQLite + fs IO → 阻塞整个 Tauri 运行时

**位置**：`src-tauri/src/bot/tools/trace.rs:29`（`async fn`）、`:48`（`open_db`）、
`:70`、`:89`、`:103`（同步查询）

**证据**

```rust
// bot/tools/trace.rs:29-53
pub(crate) async fn tool_lookup_receipt<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>, args: &str, session_id: Option<&str>,
) -> ToolResult {
    ...
    let conn = match crate::db::open_db(app) {     // ← 同步：建目录 + PRAGMA + 全量 DDL 迁移
        Ok(c) => c,
        Err(e) => return ToolResult::error(...),
    };
```

```rust
// bot/tools/trace.rs:70
let spans = match crate::db::trace::session_spans(&conn, session_id, 500) {   // ← 同步 rusqlite
```

而仓库自己的约定写得非常死——**同一目录下的其它工具全都包了 `spawn_blocking`**：

```rust
// src-tauri/src/bot_fs.rs:327-329
// canonicalize 包 spawn_blocking：同步 syscall 在 async runtime 会阻塞全部
// Tauri command / event（performance critical）。
```

```text
spawn_blocking 使用点：bot/tools/task.rs:1004、service.rs:172/210、
doc.rs:52/83/210/375、common.rs:63 —— 唯独 trace.rs 没有
```

**现象 / 影响**
`open_db` 做 `create_dir_all` + PRAGMA + 十余条 `CREATE TABLE IF NOT EXISTS`
+ 多个 `ensure_*` 迁移；`session_spans` 是 join `exec_spans`×`exec_traces` 的同步查询
（`result` 列按 16KB 存 → 最坏 ~8MB 行数据在运行时线程上物化）。
这段时间内**全部 Tauri command 与 event 被阻塞**（不只是这一个工具）。

放大面：`lookup_receipt` 在 `constants.rs:262` 的重复调用闸豁免表里
（设计上就是给模型反复调用的），阻塞面比普通工具大。

现有守卫测试 `bot/tools/sync_syscall_off_runtime_tests.rs:18-25` **只扫**
`tools.rs/common.rs/doc.rs/service.rs/task.rs`，**不含 `trace.rs`**——
所以这条约定没被门禁拦住。

**建议**
把 `open_db` + 查询整体包进 `spawn_blocking`；
**守卫测试的扫描清单补上 `trace.rs` / `http.rs` / `plan.rs`**（见 P2-10），
否则下一条同类违例还是能溜过去。

### P1-8 · `bot-think-delta` 是唯一漏掉序号去重闸的流式事件 → 回灌重放时思考文本重复拼接

**位置**：`src/components/ChatPanel/useChatStream.ts:133-153`

**证据**

```ts
// useChatStream.ts:133-152（bot-think-delta）
const unThink = reg<{ text?: string; sessionId?: string | null; seq?: number }>(
  "bot-think-delta", (p) => {
    const sid = p?.sessionId ?? null;
    const t = p?.text;
    if (!t) return;
    if (sid !== null && (streamState.isInflight(sid) || sid === sessionIdRef.current)) {
      const m = streamState.ensureMeta(sid);
      m.thinking = (m.thinking ?? "") + t;      // ← 无 noteSeq 闸，直接累加
    }
    if (sid !== sessionIdRef.current) return;
    queue(sid, "", true);
  },
);
```

同文件**其余 10 个流式事件全部有这道闸**：

```text
useChatStream.ts:126  bot-chat-delta
              :165  bot-tool
              :191  bot-tool-name
              :222  bot-tool-done
              :260  bot-tool-running
              :293  bot-turn-status
              :386  bot-msg-upserted
              :424  bot-skill-failed
              :467  bot-ask
              :481  bot-msg-dropped
              :496  bot-turn-restart
```

而后端的 `bot-think-delta` **确实经 `emit_recorded` 出口**——会被盖章 `sessionId`+`seq`，
**并且会进回放缓冲**：

```rust
// src-tauri/src/bot_model_loop/shell.rs:301
crate::bot::stream_log::emit_recorded(&app, session_id, event, payload);
```

```rust
// src-tauri/src/bot/stream_log.rs:203-206
// 序号同时写进缓冲副本：回放事件与实时广播同构（都带 seq），前端的
// 序号去重闸对两路同一口径——缓冲副本缺 seq，回放流就整段绕过去重闸。
let mut stored = payload.clone();
if let Some(obj) = stored.as_object_mut() {
    obj.insert("seq".into(), serde_json::json!(seq));
}
```

回灌用的是**同一批处理器**：

```ts
// useChatHistory.ts:202-204
handlers[ev.event]?.(ev.payload)
```

**现象 / 影响**
同一条 think 增量在「实时广播」与「回放补齐」各来一次时，
`m.thinking` 被累加两次 → **思考折叠行里内容重复**（`思考A思考A`）。
触发条件：切换会话（`switchSession` → `resumeInflight`）或面板重挂载时，
该会话正在输出思考增量。

附带一层：这道闸同时承担**刷新看门狗活跃戳**的职责（`streamState.ts:210-212`），
缺闸意味着纯思考期不刷新活跃戳 → 看门狗可能把「正在思考」误判为死。

**建议**
与其它事件对齐，在 `m.thinking` 累加前加
`if (!streamState.noteSeq(sid, p?.seq)) return;`。
配套锚点测试：同一条带 seq 的 think 事件连续投递两次，断言 `m.thinking` 只累加一次。

---

## 四、P2 发现

### P2-1 · 「上下文硬顶 200K」在超顶档只记审计，请求照发——名为硬顶，实为告警线

- **位置**：`src-tauri/src/bot_model_loop/request.rs:470-529`；`bot/constants.rs:59`
- **证据**：

```rust
// request.rs:516-529
crate::bot::compaction::CompactionLevel::Fuse => {
    // 100% 熔断：仅 audit，soft_warn_queued 由 70% soft_warn_at 路径另起一行
    (deps.audit)(…, "loop.msgs.over_budget", …);
}
```

- **现象 / 影响**：`CONTEXT_CHARS_HARD_CAP`（200_000）名为「硬顶」、级别名为 `Fuse`，
  超顶后唯一动作是发一条审计，请求原样发出。真正的兜底是上游返回 400/413 后
  `shed_oldest_history`（至多 2 次、保留 50%），而砍史只砍「首条非 system 之后、
  末条之前」的消息（`context.rs:201-226`）；轮内滑动窗也**只裁 `role=tool` 的
  字符串 content**（`context.rs:52-63`），**碰不到 assistant(tool_calls) 的
  arguments**。膨胀若来自最近的 tool_calls 参数（如大图 base64、写文件内容），
  软压/硬压/砍史三道全部够不着 → 请求直接被上游拒掉。
- **建议**：要么在 Fuse 档真做一次裁剪（至少覆盖 tool_calls arguments），
  要么把常量与级别改名为「告警线」并在 SPEC 写清真实语义。**名字骗人比没设限更糟**。

### P2-2 · 子 agent profile 提示词与实际工具白名单双向对不上

- **位置**：`src-tauri/src/prompts/subagent.rs:172`、`:187`、`:194-195`；
  `src-tauri/resources/onions/validator.yaml`、`writereditor.yaml`
- **证据**：

```rust
// prompts/subagent.rs:193-195（verifier）
可用工具：read_text_file、list_files、grep_files、extract_document、write_artifact_file、read_own_card。
- 禁止调用 edit_file / write_file / run_python / web_search / fetch_url / http_request 等任何写或联网工具——只读验证。
```

```yaml
# resources/onions/validator.yaml（实际白名单）
profile: coder
allowed: [read_text_file, list_files, grep_files, run_python, ast_inspect,
          write_artifact_file, read_own_card]
```

```rust
// prompts/subagent.rs:187（executor）
- 动手前用 ast_inspect 摸清 recon 指到的 Python 代码结构
```

```yaml
# resources/onions/writereditor.yaml
# v1 不给 ast_inspect：撰写者按大纲写，不需要符号级结构查询。
allowed: [read_text_file, list_files, grep_files, edit_file, write_file,
          run_python, write_artifact_file, read_own_card]
```

- **现象 / 影响**：三处双向错位——
  ① verifier 提示词说 `extract_document` 可用，实际白名单里**没有** → 调用被拒；
  ② verifier 提示词说 `run_python` 禁用，实际白名单里**有** → 「只读验收」
  这条对外承诺在代码闸层并不成立（`run_python` 能写盘）；
  ③ executor 提示词让「动手前用 `ast_inspect`」，而 writereditor 明确**不给**它
  → 每次都被拒。
  此外两份注释对同一事实给出相反结论：`prompts/subagent.rs:172` 说
  「verifier 段 → profile=research（只读足够）」，`bot_orchestrator.rs:132-134` 说
  「validator 的 profile 是 coder——verifier 的派生档位随之从 research 翻到 coder」。
- **建议**：把三处提示词与 yaml 白名单**改成同一份真相**（最好由 yaml 生成提示词
  里的工具清单）；`profile: coder` 但只读白名单这个设计本身是合理的（上限+收窄），
  需要改的是**提示词**与**注释**。

### P2-3 · 「需要人处理」聚合把全部失败子 agent 行一次性读进内存

- **位置**：`src-tauri/src/db/work_items.rs:426-440`；`src-tauri/src/db/subagents.rs:432-447`
- **证据**：`load_subagents_by_status(&conn, st)` **无 LIMIT**（全表按 status 取），
  `limit` 只在末尾 `out.truncate(limit)` 生效；同一函数的工作项分支反倒有 limit。
- **现象 / 影响**：长期使用的库里失败子 agent 行线性增长，聚合查询每次全量读入 +
  排序，内存与延迟随历史增长（前端 limit 只挡输出不挡读）。
- **建议**：子 agent 分支加 LIMIT，或先按 `finished_at` 取最近 N 条再合并。

### P2-4 · 排队期取消检测在 async worker 上同步跑 open_db（含全量 DDL）

- **位置**：`src-tauri/src/bot/subagent_runner.rs:157-172`（对比同文件 watcher 的
  `spawn_blocking` 写法 `:543-553`）
- **证据**：闸门里的 `cancelled_while_queued` 直接 `crate::db::open_db(&app_gate_check)`；
  `open_db` 做 `create_dir_all` + PRAGMA + 十余条 `CREATE TABLE IF NOT EXISTS`
  + 多个 `ensure_*` 迁移。代码注释自己承认「open_db 含建目录+pragmas 文件 IO，
  不该压在 async worker 上」，但节流只把频率从 50ms 降到 500ms，**线程没换**。
- **现象 / 影响**：每个排队中的子 agent 每 500ms 在 tokio worker 上跑一次整套 DDL，
  排队数一多就压占 worker，拖慢同线程上的心跳/流式任务。
- **建议**：与 watcher 一致改 `spawn_blocking`，或开一条不跑 DDL 的只读连接。

### P2-5 · 技能 references 清单无条数上限

- **位置**：`src-tauri/src/bot_skills/runtime.rs:419-447`；`src-tauri/src/bot_skills/remote.rs:24`
- **证据**：`use_skill` 第三层披露把 `references/` 下所有 `.md` 拼进工具结果，
  `files` 无 `.take(...)`；远程安装上限为 2000 文件 / 64MB。
- **现象 / 影响**：装一个带 2000 个 `references/*.md` 的远程技能后，一次 `use_skill`
  就把约 2000 条绝对路径（≈上百 KB）注入模型上下文，**内容来自第三方仓库**。
- **建议**：清单加条数/字符上限（前 N 条 + 「其余按需 read_dir」）。

### P2-6 · SKILL.md 读取无上限，与模块文档宣称的「有大小上限」不符

- **位置**：`src-tauri/src/bot_skills/state.rs:265-277`；`src-tauri/src/bot_skills/mod.rs:6`
- **证据**：`std::fs::read_to_string(&path)` 无闸；`MAX_SKILL_BODY` 只截「保留下来的
  body」，不限制实际读入量。
- **现象 / 影响**：手工放进 skills 目录的大文件被整文件读进内存；
  `mod.rs:6` 的安全说明宣称有上限，**契约不成立**。
- **建议**：read 前加 `metadata` 字节闸（复用 `IMPORT_MAX_SKILL_MD_BYTES` 口径）。

### P2-7 · MCP stdio 错误路径只杀直接子进程，进程树可能留孤儿

- **位置**：`src-tauri/src/bot/mcp/stdio_transport.rs:190-199`；对照 `manager.rs:629-643`
- **证据**：错误路径只 `child.start_kill()`（SIGKILL 直接子进程，即 npx/uvx 包装器）；
  正常回收走 `manager.rs:631-635` 的 `stop_stdio_tree`（信号阶梯 + 递归子孙回收）。
- **现象 / 影响**：`initialize` / `tools-list` 超时 → RunningService 被 drop
  → 传输 drop → 只杀直接子进程；包装器拉起的真实服务器进程（npx → node）
  被 reparent 后继续跑、**占端口且宿主管不到**。
- **建议**：错误路径也走 `stop_stdio_tree`。

### P2-8 · 受管服务巡检在 async 运行时线程上做阻塞 IO，且每轮重复拍全机进程表

- **位置**：`src-tauri/src/bot/services/logs.rs:180-188`；`bot/services/observe.rs:90`；
  `bot/services/gate.rs:623`
- **证据**：`sweep_round` 直接在 tokio worker 上跑；`observe.rs:90` 的 `take_snapshot()`
  阻塞执行 `ps -ax`；`gate.rs:623` 又独立拍一次 `take_snapshot()`。
  MCP 面板的 5s TTL 采样（`manager.rs:832-895`）另拍一套快照，两套口径窗口不对齐。
- **现象 / 影响**：每 30s 占住一个 runtime worker 做阻塞进程枚举。
- **建议**：`sweep_round` 挪 `spawn_blocking`；一轮内共用一张 snapshot。

### P2-9 · MCP `Slot::Connecting` 无兜底超时 → 工具整个会话不可见

- **位置**：`src-tauri/src/bot/mcp/manager.rs:101-104`、`:338-377`、`:406-414`
- **证据**：`manager.rs:353` 写入 `Slot::Connecting` 后只有成功/失败两条落定路径；
  future 在 await 处被 drop（模型循环中止）则槽位**永停 Connecting**；
  `manager.rs:411` 的 `tools_known` 对 `Connecting` 返 true → `ensure_tools_known`
  因此**永久跳过**该服务器的发现；`status_of` 恒报 "pending"。
- **现象 / 影响**：该服务器的工具整个会话不再被发现/挂载（模型看不见），面板恒显
  「待发现」；只有下一次配置写（`apply_config` 重置槽位）才自愈。
- **建议**：`Connecting` 记起始时刻，超连接预算仍停 Connecting 即按 Down 处理。

### P2-10 · `http_request`(saveAs) 实际落盘却登记为 `mutating: false` → 守卫逼模型重复下载

- **位置**：`src-tauri/src/bot/registry/schemas.rs:617-623`；`src-tauri/src/bot/tools/http.rs:320-327`；
  `src-tauri/src/bot_model_loop/stream.rs:16-18`；`src-tauri/src/bot_model_loop/guard.rs:380-383`
- **证据**：

```rust
// bot/registry/schemas.rs:617-623
name: "http_request", schema: SCHEMA_HTTP_REQUEST,
mutating: false, claims_patterns: &[], max_output_bytes: 65536,
```

```rust
// bot/tools/http.rs:320-327  —— 工具真的把文件落盘了
[已保存] {path}
ToolResult::ok(out, …).with_file_change(build_binary_file_receipt(...))
```

守卫的「动过手」只认两样：`MUTATING_TOOLS()` 成员资格（`http_request` 不在里面），
或 `unverifiable_ran`——后者只认 `run_python` / `mcp_*` / `spawn_subagent`
（`guard.rs:380-383`），**不含 `http_request`**。而工具自身输出的「已保存 /path/x.csv」
会撞上 `record_lesson` 的 pattern `"已保存"`。

- **现象 / 影响**
模型如实汇报「已下载并保存到 X」→ `claim_mutation=true` 而 `mutation_done=false`
→ 守卫第一次**推回模型**（理由：「你刚才声称完成了变更，但本轮没有任何变更类工具调用成功」）
→ 模型**重新下载**（重复副作用 + 重复计费 + 重复打洞）→ 第二次不给推回，
改为在终稿末尾挂一条「上面声称的变更没有实际发生」——**对用户构成不实指控**。
- **建议**：`http_request` 登记为 `mutating: true`（附 claims_patterns），
或把它纳入 `guard.rs:380-383` 的 `unverifiable_ran` 集合。
注意 `registry_tests.rs:303-346` 锁死了 35 个 mutating 名字，改动需同步该基线。

### P2-11 · `update_plan` 同样落在「非 mutating 但话术撞 mutating pattern」的缝里

- **位置**：`src-tauri/src/bot/registry/schemas.rs:995-1001`；`src-tauri/src/bot/tools/plan.rs:58-62`
- **证据**：工具输出 `"计划已更新：共 {n} 步（已完成 {done} / …）"` 撞 `edit_task` 的
  pattern `"已更新"`；而 `update_plan` 不在 `MUTATING_TOOLS` → `mutation_done` 恒 false。
  按 `claims.rs` 的两阶段匹配，「计划已更新：共 3 步」「已更新计划，共 3 步」均命中。
- **现象 / 影响**：纯规划回合（只调 `update_plan`）被守卫判成「声称变更但未调工具」
  → 推回重跑 → 第二次终稿挂「上面声称的变更没有实际发生」。无破坏性副作用，
  但用户会看到对计划更新的不实指控。
- **建议**：`update_plan` 登记为 mutating（它确实写了会话计划状态），
或给 `claims_mutation` 改成「按工具归属」的判定而非全局合并 pattern 池。

### P2-12 · `deferred_gate` 早退不补 `tool.return` → 统计面板出现悬挂调用

- **位置**：`src-tauri/src/bot/dispatch.rs:586-588`（对照同函数其余 6 个早退点）
- **证据**：

```rust
// dispatch.rs:586-588
if let Some(denied) = crate::bot::registry::groups::deferred_gate(name, session_id) {
    return denied;          // ← 入口已在 :541 发出 tool.call，此处无 tool.return 配平
}
```

而 `dispatch.rs:132-139` 自己写着「否则统计面板出现『悬挂调用』（call > return）」。
- **现象 / 影响**：`tool.call` 计数 > `tool.return` 计数；工具组开关开启后，
  每次调用休眠组工具都留一条悬挂记录。
- **建议**：改走 `early_return_events(name, "deferred_group", …)` 或补一条
  `tool.return(reason=deferred_group)`。

### P2-13 · `extract_document` 的回灌预算（8KB）远小于自身一页（30000 字符）→ offset 续读会跳过没读过的中间段

- **位置**：`src-tauri/src/bot/registry/schemas.rs:521-527`；
  `src-tauri/src/bot/tools/doc.rs:269-270`；`src-tauri/src/bot_model_loop/output.rs:39-47`
- **证据**：`max_output_bytes: 8192`（schema）vs `EXTRACT_DEFAULT_LIMIT = 30000`（按**字符**切页）；
  回灌漏斗按**字节**截（`cap = min(8192, 131072)` = 8192）。中文文档一页 30000 字 ≈ 90KB，
  **11 倍于预算**。`DOCUMENT_OUTPUT_TOOLS` 只豁免 `use_skill`。
- **现象 / 影响**：模型按 schema 传 `limit=30000`，实际只拿回头尾各约 4KB + 溢出指针；
  若照 schema 的「用 offset 续读」去请求 `offset=30000`，会**跳过从未看到的中间约 22000 字**。
  同族 `fetch_url` / `http_request`（30000 字符切片 vs 65536 字节预算）也有 1.37 倍同类溢出；
  只有 `read_text_file`（56KB 页 / 64KB 预算）对齐。
- **建议**：把 `extract_document` 的 `max_output_bytes` 提到与页面上限同量级，
  或把页大小改成字节预算口径并在 schema 写明。

### P2-14 · `activate_groups` 对部分非法组名「先激活后报错」→ 状态已改但回执失败

- **位置**：`src-tauri/src/bot/registry/groups.rs:171-189`
- **证据**：

```rust
for g in groups {
    if !DEFERRED_GROUPS.contains(&g.as_str()) {
        return Err(format!("未知或非休眠组：{g}（…）"));   // ← 前面的合法组已入 store
    }
    entry.insert(leak_group_name(g)?);
}
```

- **现象 / 影响**：`groups=["desktop","bogus"]` 时 `desktop` 已写进 `active_groups()`，
  随后返回 Err → 工具回 `ToolResult::error`，模型看到「失败」，
  但下一轮 `session_tools_json` 会把 desktop 组挂上——**schema/状态与回执分叉**。
- **建议**：先整体校验再写入（两趟），或失败时回滚本轮已插入的组名。

### P2-15 · `at:` 一次性在作业与提醒两条链路的补触发宽限差 **1440 倍**（60s vs 24h）

- **位置**：`src-tauri/src/bot_scheduler.rs:306-310` vs `src-tauri/src/bot_reminders.rs:20,341-353`
- **证据**：

```rust
// bot_scheduler.rs:309
const AT_FIRST_RUN_GRACE: chrono::Duration = chrono::Duration::seconds((SCHED_TICK_SECS * 2) as i64); // 60s
// bot_reminders.rs:20
const ONE_SHOT_GRACE: Duration = Duration::hours(24);
```

- **现象 / 影响**：两者共用同一套触发点计算与同一个 `at:` 串
  （`bot_reminders.rs:4-7`、SPEC.md:312 明确「共用定时串格式与触发点计算」），
  但同一个 `at:2026-11-17T09:00` 在「关机 10 分钟后开机」下：**作业被删，提醒照响**。
  SPEC 只声明共用「格式与触发点」，未声明宽限期共用 → 口径分叉未登记。
- **建议**：统一成一个 `ONE_SHOT_GRACE` 常量，或在两处注释里写明「宽限期故意不同，理由是 X」。

### P2-16 · 提醒写路径不广播 `reminders-changed`，与文档口径相反

- **位置**：`src-tauri/src/bot_reminders.rs:372-374`（唯一的广播点，在 tick 里）；
  `src-tauri/src/db/reminders.rs:252/315/383/406`（四个写命令）
- **证据**：`grep -rn "reminders-changed" src-tauri/src` → **只有 1 处**（在 tick 里）。
  四个写命令只写审计事件后 `Ok(row)`，无 `app.emit`。
  文档侧：`docs/realmachine-smoke-test-2026-11-15.md:597` 明写
  「前端事件：`reminders-changed`（**提醒写路径广播**）」。
- **现象 / 影响**：模型经聊天建一条未来触发的提醒时，tick 的 `changed` 恒为 false
  （`load_due_reminders` 只取已到点的行）→ 广播永不发 → 定时页在当前挂载期不刷新
  （重新挂载会自愈，窗口有限，但多窗口/同窗未切换时存在）。
  **实机卡若按「写路径广播」判据跑会红。**
- **建议**：四个写命令在落库成功后补 `app.emit("reminders-changed", …)`，或修正文档口径。

### P2-17 · `http_request` saveAs 分支在 async 上下文同步写最多 50MB 落盘

- **位置**：`src-tauri/src/bot/tools/http.rs:295`（`gen_dir`）、`:315`（`atomic_write_bytes`）
- **证据**：`MAX_BODY_BYTES = 50MB`（`http.rs:18`）、默认 10MB（`:19`）；
  写盘走 `db/paths.rs:72` 的 `std::fs::write` + rename，全程在 runtime 线程。
- **现象 / 影响**：50MB 落盘期间 UI 事件/命令全停；与 `bot_fs.rs:328` 的约定同源。
- **建议**：`spawn_blocking` 包住写盘 + 目录创建，或复用 `bot_fs` 已有的原子写路径。

### P2-18 · 死循环看门狗里 `bot_stop` 失败被静默吞掉 → 前端判死但后端仍在跑并计费

- **位置**：`src/components/ChatPanel/useChatStream.ts:606-608`
- **证据**：

```tsx
// useChatStream.ts:603-608
// 后端若真还挂着（判死只回收前端视图），顺手叫停。叫停前复核一次
// 在途：判死放行后用户可能已开新轮（markInflight 已重新置位），
// bot_stop 按 sessionId 置位全部实例——不复核会把新轮一并误停
if (!streamState.isInflight(sid)) {
  invoke("bot_stop", { sessionId: sid }).catch(() => {});
}
```

- **现象 / 影响**：看门狗判定「后端无流式活动」→ 前端已把该轮标为 `failed`
  并回收视图 → 但 `bot_stop` 这次 invoke **失败时没有任何处理**：
  后端那一轮**仍在继续生成**（继续烧 token 与计费），用户界面上却已是终态。
  「判死只回收前端视图」的注释说明这是有意设计，但**叫停失败没有任何补救**
  （无重试、无审计、无提示）。
- **建议**：失败时重试一次或保留在途标记让下一轮看门狗继续看；
  至少留一条前端可观测痕迹。**「已停止」与「实际没停」必须区分开。**

### P2-19 · `ServicesPanel` 的 5s 轮询缺少可见性闸，隐藏时仍在后台轮询

- **位置**：`src/components/SettingsPage/ServicesPanel.tsx:111-118`；
  对照 `src/components/SettingsPage/McpPanel.tsx:297-314`；`src/components/SettingsPage/SettingsPage.tsx:551-561`
- **证据**：

```ts
// ServicesPanel.tsx:111-118
useEffect(() => {
  refresh();
  const timer = setInterval(refresh, 5000);   // ← 无 active 条件
  return () => clearInterval(timer);
}, []);
```

```ts
// McpPanel.tsx:297-314（同目录对照，有闸）
if (!active) return;
```

而设置页自己的注释已经把这条规矩写明了：

```ts
// SettingsPage.tsx:551-553
// active：分类隐藏时面板不卸载（惰性挂载），可见性要传下去，
// 否则它在后台一直 5s 一次地拍 ps / lsof
```

但 `<ServicesPanel />`（`SettingsPage.tsx:561`）**未传 `active`**。
- **现象 / 影响**：用户访问过一次「受管服务」分类后离开，该面板不再卸载但继续每 5 秒
  调 `services_list`（`bot/services.rs:415`），持续产生 IPC 与快照序列化开销。
  属后台无效负载，非功能错误（后端快照是缓存采样，严重度有限）。
- **建议**：给 `ServicesPanel` 加 `active` prop，轮询 effect 加 `if (!active) return;`。

### P2-20 · `cycleSetting` 是死代码：三态循环入口无任何生产调用方

- **位置**：`src/theme.ts:44-50`
- **证据**：

```ts
/** 三态循环：浅色 → 深色 → 跟随系统 → 浅色 */
export function cycleSetting(): ThemeSetting {
  const cur = getSetting();
  const next: ThemeSetting = cur === "light" ? "dark" : cur === "dark" ? "system" : "light";
  applySetting(next);
  return next;
}
```

全仓检索 `cycleSetting` → **只有定义处 + `src/theme.test.ts:84-89`**。
生产侧主题切换走的是 `toggleTheme`（`src/theme.ts:53`，由 `App.tsx:398` 的
`toggle-theme` 事件驱动）。
- **现象 / 影响**：knip 零告警是因为它被测试引用——**这正是 knip 的盲区
  （「只在测试里被引用」不算 unused）**。附带后果：三态中的「跟随系统」在 UI 上
  没有入口，用户无法主动切到 system。
- **建议**：删除 `cycleSetting` 及其测试，或在设置页接入三态切换 UI。
  **这是本次全仓唯一一个「真死代码」**（Rust 侧 0 个）。

---

## 五、P3 发现（死代码与卫生）

### P3-1 · `work_items_list` / `work_items_needing_human` 是纯死代码（详见 P1-4）

- **位置**：`src-tauri/src/db/work_items.rs:461-489`；`src-tauri/src/lib.rs:819`、`:821`
- 配套：`list_recent`（`db/work_items.rs:261`）**只被 `work_items_list` 调用**，
  删命令时必须一起删，否则留下新的孤儿函数。

### P3-2 · `ServiceState::Failed` 是死状态：没有任何路径把条目置为 Failed

- **位置**：`src-tauri/src/bot/services.rs:126-127`（定义）；`src-tauri/src/bot/services/start.rs:103-119`
- **证据**：spawn 失败走 `start.rs:107` `lock_table(&table).remove(&id)`（条目直接摘掉），
  不写 Failed；全仓无任何 `state = ServiceState::Failed` 赋值。
- **影响**：面板与模型永远看不到 "failed"；真实形态是「条目消失 + 报错文案」，
  与枚举注释语义不符 → 后续按 `as_str` 写前端分支会留永不触达的分支。

### P3-3 · `prompts::ALL` 登记表漏了 `FINAL_SELFCHECK_PROMPT`

- **位置**：`src-tauri/src/prompts/mod.rs:33-36`、`:49-75`
- **证据**：`assert_eq!(ALL.len(), 21, "prompt 清单与实际常量数不一致（新增请登记）")`，
  而该常量已在 re-export 却不在 `ALL` → 它的两条通用断言（非空 + ≥30 字）漏检；
  且长度写死成 21，将来补登记反而会红。

### P3-4 · 摘要请求的字符上限是 `CONTEXT_CHARS_HARD_CAP` 的手写副本

- **位置**：`src-tauri/src/bot_chat.rs:623`
- **证据**：

```rust
/// 与 constants::CONTEXT_CHARS_HARD_CAP 同值不同域（摘要请求 vs 主请求），
/// 改其中一处前先看另一处。
const SUMMARIZE_MAX_CHARS: usize = 200_000;
```

- **影响**：两处各自硬写 200_000，无编译期约束；改 `constants.rs:59` 不会同步此处。
  注释已承认靠人盯。**建议**直接引用常量（同值不同域就显式命名区分），
  让漂移在编译期暴露。

### P3-5 · `RouteAction::audit_kv` 无生产调用方，它声称服务的命中率统计并不存在

- **位置**：`src-tauri/src/intent_router.rs:33-45`
- **证据**：全仓只有本文件单测调用；主流程发的是 `pre_step.route_skill` /
  `pre_step.bypass_off` / `chat_execute.routed`，均不带 `action` 键 →
  注释里的统计口径无处可查。

### P3-6 · `body_outcome` 的 `"interrupted"` 分支不可达

- **位置**：`src-tauri/src/bot_model_loop/core.rs:671-677`
- **证据**：`body_outcome` 实际只被赋过 `"await_confirm"/"skill_finish"/"skill_fail"/
  "skill_terminate"/"tool_fuse"/"submitted"/"fused"`，无任何处写 `"interrupted"`
  → 该臂恒不命中；中断态实际由 `(Ok(_), _, true)`（`stop.stopped()`）兜住。

### P3-7 · `flow.rs` 的注释「50 轮工具循环」与实际 200 轮不符

- **位置**：`src-tauri/src/bot_chat/flow.rs:474`；对照 `bot_chat/exec.rs:18`、
  `bot/constants.rs:23`
- **影响**：同一概念两处口径不同（50 vs 200），按 `flow.rs` 注释估时**低估 4 倍**。

### P3-8 · 上下文量测每轮整栈序列化（O(轮数 × 上下文) 的重复复制）

- **位置**：`src-tauri/src/bot_model_loop/request.rs:470`；`guard.rs:80`；`evidence.rs:53-63`
- **证据**：`let msgs_chars: usize = msgs.iter().map(msg_chars).sum::<usize>();`
  其中 `msg_chars = serde_json::to_string(m)`（每条一次分配）。
- **影响**：单轮成本随消息栈线性增长，轮数上限 200 → 整轮合计 O(轮数×上下文)。
  受 200K 硬顶与滑动窗约束，量级为几十 MB 级 churn（非卡死），但**每轮重复付**。

### P3-9 · `deferred_gate` 的拒绝文案缺 `⚠️` 前缀 → trace span 的 error_class 与其它闸不一致

- **位置**：`src-tauri/src/bot/registry/groups.rs:224-226`；对照 `src-tauri/src/audit.rs:130`
- **证据**：`audit.rs:130` 的门禁臂是 `text.starts_with("⚠️")` → `GateDenied`；
  `dispatch.rs:336-338` 明写「⚠️ 前缀不是装饰」。而 `deferred_gate` 的文案没有该前缀
  → `classify_error_class` 返回 None → 执行痕迹面板回落通用「失败」。
- **影响**：无法与规则闸/白名单闸的「门禁拦截」区分。
- **建议**：文案前缀对齐 `dispatch.rs:377-379` 的 `⚠️ `。

### P3-10 · 模块头注释的数字漂移（4 处）

- **位置**：`src-tauri/src/bot/registry/handlers.rs:1`（`65 个 call_*` → 实为 **69**）；
  `src-tauri/src/bot/tools.rs:1`（`22 个 tool_*` → 实为 **26**）；
  `src-tauri/src/bot_model_loop.rs:122`（`15 个 mutating` → 实为 **35**）；
  `src-tauri/src/bot/tools/doc.rs:783-785`（`超时 60s`、`最长 300s` → 实际**默认 120s、上限 600s**）
- **影响**：与 `BUILTIN_TOOL_COUNT = 69`（`schemas.rs:373`）、
  `PYTHON_TIMEOUT_SECS = 120` / `PY_MAX_TIMEOUT_SECS = 600`（`constants.rs:109/115`）矛盾。
  **该 SOP 假发现模式已在 `docs/agent-audit-sop-zcode.md` 记录过**——照抄过期数字
  正是那次 12 条假发现的来源。
- **建议**：改为引用常量，或直接删掉数字。

### P3-11 · `active_groups()` 的会话条目永不回收

- **位置**：`src-tauri/src/bot/registry/groups.rs:160-163`、`:174`
- **证据**：进程级 `HashMap` 按 `session_id` 累积，无淘汰路径。
- **影响**：开关默认关，但一旦开启并被多会话使用，条目只增不减。
- **建议**：随会话结束清理，或改用有界 LRU。

### P3-12 · 新建周期作业「首跑立即生效」与列表页展示的 `nextRunAt` 不一致

- **位置**：`src-tauri/src/bot_scheduler.rs:1225` vs `src-tauri/src/bot_scheduler.rs:332`
- **证据**：列表用 `next_fire_at(sched, now)`；真实派发用
  `occurrence_after(sched, sched_last_dt(sched_last))`，而 `sched_last=None` → epoch0
  → 算出「已到点」→ 首跑立即派发（`sched_tests.rs:215-220` 钉住，属有意设计）。
- **影响**：列表页对同一条作业显示 `nextRunAt = 明天 09:00`，实际 ≤30s 后就真跑。
  窗口小，但属同一响应内的自相矛盾。
- **建议**：列表侧对 `sched_last=None` 按同一基准算，或在前端文案上区分「首次立即执行」。

### P3-13 · `tag_similar` 缓存逐出判断含生产不可达分支

- **位置**：`src-tauri/src/tag_similar.rs:46-54`、`:56-67`
- **证据**：`cache_insert` 的 `if state.map.len() >= CACHE_LIMIT && !state.map.contains_key(&tag)`
  ——唯一生产调用方 `cache_get_or_embed` **只在未命中时**调它，命中时提前 return，
  故 `contains_key` 恒为 false。
- **影响**：无功能危害，但该分支与其单测只覆盖了生产走不到的场景，
  掩盖了「重复入队会向 order 追加重复项」这一潜在坑。
- **建议**：删掉 `contains_key` 判断，或把单测改为走 `cache_get_or_embed` 真实路径。

### P3-14 · `useChatStream` 返回空对象，hook 出口为无效契约

- **位置**：`src/components/ChatPanel/useChatStream.ts:637-638`；调用方 `src/components/ChatPanel/ChatPanel.tsx:486-493`
- **证据**：

```ts
  return {
  };
}
```

调用方也不接收返回值。
- **影响**：无功能影响，但函数签名暗示有出口值、实际为空，属拆分重构的接口残骸。

---

## 六、运行限制清单（用户视角）

这些不是缺陷，是**当前产品的真实边界**。列出来是因为「运行限制」是本次审计的
明确维度——用户需要知道自己会在哪些地方撞墙。

### 6.1 算力与预算

| 限制 | 值 | 位置 |
|---|---|---|
| 单会话轮数安全网 | **200 轮**（主/子 agent 共用） | `bot/constants.rs:23` |
| 单会话工具调用安全网 | **400 次**（主/子 agent 共用） | `bot/constants.rs:25` |
| 熔断阶梯 | 90% 软告警 → 95% 收尾窗（只放行 5 个写产物类工具）→ 100% 硬停 | `bot/constants.rs:176-199` |
| 收尾窗剩余轮数 | **3 轮** | `bot/constants.rs:186` |
| 会话历史预算 | **128,000 字符**（超预算丢最旧 + 摘要，裁至 70%） | `bot/constants.rs:34` |
| 单次请求上下文「硬顶」 | **200,000 字符**（⚠️ 实为告警线，见 P2-1） | `bot/constants.rs:59` |
| 单条工具回填上限 | **128 KB**（超限转存溢出文件 + 节选 + 指针） | `bot/constants.rs:49` |
| 轮内工具结果滑动窗 | 累计 > **24,000 字符** 开始裁，保留最近 **6** 条 | `bot/constants.rs:64-69` |
| 单轮输出 max_tokens | 默认 **16,384** / 硬顶 **32,768** | `bot/constants.rs:40-43` |

### 6.2 时间闸

| 限制 | 值 | 位置 |
|---|---|---|
| LLM 首包超时 | **60s** | `bot/constants.rs:81` |
| 流式空闲超时 | **120s**（无总时长上限——只要每 <120s 有数据，单轮可无限长） | `bot_model_loop/sse.rs:511` |
| LLM 重试 | 429/5xx/网络错误共 **3 次**尝试；流内重发 **1 次** | `bot/constants.rs:86,94` |
| run_python | 默认 **120s**，上限 **600s** | `bot/constants.rs:109,115` |
| MCP 工具调用 | 上限 **600s** | `bot/constants.rs:123` |
| http_request | **120s**（下载放宽到 300s） | `bot/constants.rs:103,106` |
| apps 程序运行 | 上限 **1800s** | `bot/constants.rs:119` |
| 子 agent 停滞超时 | **600s**（无流量即判假死） | `bot/constants.rs:319` |
| `check_subagent` 单次等待 | 上限 **600,000ms** | `bot/constants.rs:310` |
| 技能绝对寿命 | **3600s**（含 Paused 等确认的时间） | `bot/constants.rs:295` |

### 6.3 并发与规模

| 限制 | 值 | 位置 |
|---|---|---|
| 内置工具数 | **69**（主 agent 默认可见数 = 69 − 子 agent 专属工具数） | `bot/registry/schemas.rs:373` |
| 子 agent 全局并发 | **3** | `bot/constants.rs:305` |
| 单主会话子 agent 并发 | **2** | `bot/constants.rs:307` |
| 技能步数 | 默认 **100**，声明上限 **200**（只对 auto/DSL 模式生效） | `bot/constants.rs:286,289` |
| 重复调用闸 | 同参连击 **3** 次即拦截（豁免表 21 个纯读/轮询工具） | `bot/constants.rs:241,254` |
| ask_user 预算 | 默认 **3** 次/回合 | `bot/constants.rs:234` |
| RePlan 次数 / 计划步数 | **2** 次 / **8** 步 | `bot/constants.rs:206,211` |
| 联网搜索条数 | **1–10**，默认 6 | `bot/constants.rs:227-231` |

### 6.4 文件与 IO

| 限制 | 值 | 位置 |
|---|---|---|
| 读文件 / 单行 / 单页 | 100 KB / 2,000 字符 / 56 KB | `bot/constants.rs:126-135` |
| 读文件行数 / grep 命中 / 单文件扫描 | 2,000 行 / 50 条 / 2 MB | `bot/constants.rs:137-141` |
| 目录列举 / 递归深度 | 200 条 / 5 层 | `bot/constants.rs:143-145` |
| 编辑 / 写入文件体积 | 1 MB / 2 MB | `bot/constants.rs:147-149` |
| 解压 | 单文件 64 MB / 总量 256 MB / 5,000 条 / 放大比 200× | `bot/constants.rs:152-158` |
| 打包 | 单文件 512 MB / 总量 2 GB / 20,000 条 | `bot/constants.rs:162-166` |

### 6.5 平台差异（真实边界，非缺陷）

| 能力 | macOS | Windows | 依据 |
|---|---|---|---|
| run_python 出网隔离 | ✅ seatbelt 硬闸（fail-closed） | ❌ 仅提示词软约束 | `py/runtime.rs:878-925` |
| 监听 socket 枚举 | ✅ lsof | ❌ **平台不支持**（判定层返 `Unsupported`，不假装没事） | `bot/procs.rs` + `bot/services/gate.rs:299-318` |
| 宿主死亡看门狗 | ✅ 管道 EOF | ✅ Job Object（KILL_ON_JOB_CLOSE） | `bot/services/watchdog.rs` |
| 凭据存储 | 系统钥匙串 | 系统钥匙串 | `bot/config/keyring.rs` |
| 端侧模型（OCR/向量） | 打包版**不随包**（解析不命中，已在案） | 随包装 | `docs/WINDOWS-INSTALLER-DESIGN-2026-11-05.md` |

### 6.6 已登记在案的设计缺口

- `fetch_url` **未纳入目的地闸**——白名单/按次授权对它不生效，GET 查询串可外传到
  任意公网域（Jina/DeepSeek 回退还会把原 URL 转给第三方）。D1 拍板「不做」，
  SPEC 已记缺口（`docs/RUN-PYTHON-SECURITY-REFIT-PLAN-2026-10-31.md:48-50`）。
  **别处不要再宣称 fetch_url 走目的地闸。**

### 6.7 自动化链路（调度 / 提醒 / 作业）

| 限制 | 值 | 位置 |
|---|---|---|
| 调度扫描周期 | **30s**（作业、提醒、截止通知、工作项清扫共用同一个 tick） | `bot_scheduler.rs` |
| 作业并发 | **2**（`JOB_RUN_MAX_CONCURRENT`）+ 同作业去重 + `SchedGuard` 防重入 | `bot_scheduler.rs:791-810` |
| 一次性作业宽限 | **60s**（⚠️ 与提醒的 24h 差 1440 倍，见 P2-15） | `bot_scheduler.rs:309` |
| 提醒一次性宽限 | **24h** | `bot_reminders.rs:20` |
| 作业错过补跑窗口 | **2h** | `bot_scheduler.rs` `CATCHUP_WINDOW` |
| 工作项陈旧清扫 | **2h** 无更新的 working 行按中断收 failed | `bot_scheduler.rs:1105` |
| 总开关覆盖 | **只覆盖聊天与作业**（⚠️ 提醒/截止通知不覆盖，见 P1-6） | `bot_slash.rs:614` |

---

## 七、已查证无问题（点名，避免漏审误读）

1. **Rust 侧无死代码符号**——全仓词频扫描「定义处即唯一出现处」，3 个候选
   (`bot_skills/state.rs:292,300,307` 的 `test_hook_*`) 确认是
   `src-tauri/tests/skill_e2e.rs` 用的集成测试钩子，**在用**。
2. **前端无死代码**——knip 零告警（无死文件/死导出/死依赖）；组件/常量/hook 全部有引用。
3. **事件契约双向闭合**——33 条注册事件全部前后端双向存在，无孤儿事件、无空订阅。
4. **提示词常量无死码**——22 个常量逐一 grep 计数，最少 2 处引用；无「拼了但拼不进
   任何路径」的提示词。
5. **轮内工具结果回灌顺序与模型顺序一致**——`core.rs:769-772` 的
   assistant(tool_calls) 数组按索引序生成，`:821-1164` 按同序逐个执行并 push
   `role=tool`，`tool_msg_ages` 同步记下标，无错序/错标。
6. **收尾窗拒绝与重复闸拒绝都按协议回填 tool 消息**——`core.rs:888-894`、
   `:941-947` 均 push `role=tool` 拒文后 continue，既不破坏 tool_calls↔tool 配对，
   也不消耗配额。
7. **请求重试全部有界**——429/5xx 受 `MAX_LLM_ATTEMPTS`、stream_options 降级与
   thinking 补救各自一次性、砍史至多 2 次，**不存在重试风暴**。
8. **会话/任务防重入是原子的**——`flow.rs:27-32` 与 `exec.rs:102-108` 在同一把锁内
   做 check+insert，无 check-then-act 窗；两把锁不交叉持锁，无锁序反转。
9. **MCP 发现与调用互斥**——两条路径共用同一把 per-server `call_lock`
   （`manager.rs:485-486`、`:1142-1143`）。
10. **MCP 机密明文不落 bot-config.json**——`skip_serializing` + 单测
    `server_serialization_never_emits_secrets`（`secrets.rs:634-659`）；掩码还原在
    `store_server_secrets`。
11. **MCP 应答脱敏 sink/model 面一致**——sink 面统一 `redact_answer_echoes`
    （`dispatch.rs:40` → tool.return preview / 动作流 / trace span），模型面保留原值。
12. **目的地闸 fail-closed**——子 agent / 无人值守一律拒（`destination.rs:166-183`）；
    会话授权粒度 host×会话；配置写入即失效。
13. **公网闸每跳校验 + 钉 IP + 禁重定向**，https→http 降级拒。
14. **端口闸「没判 ≠ 没事」**——`judge_sockets` 返 `Unsupported`、金丝雀失明单独审计、
    采样失败保留集不清空。
15. **技能 `enabled` 三闸口径一致**（广告/路由/预审），拨杆落盘后重建路由表。
16. **技能 `allowed-tools` 两层都真生效**——DSL 逐步校验 + dispatch 交互路径闸。
17. **子 agent 六态状态机终态不可再转**，有表驱动单测。
18. **spawn 建行与派发待办同事务**——「有行无待办」的中间态不存在。
19. **终态落库走 Effects 队列**（崩溃可恢复）且 handler 幂等。
20. **计划「至多一条 in_progress」在解析期硬校验并有单测**。
21. **子 agent 白名单在 dispatch 层强制**（schema 过滤之外的兜底）。
22. **架构文档的数字与代码一致**——「338 个文件 / 约 14.9 万行」与实测
    （338 个 / 148,742 行）相符；`tests-audit/audit_module_map.py` 通过。
23. **工具表 ↔ handler 一一对应，无死代码**——`TOOLS_TABLE` 69 项与 69 个 `call_*`
    双向零差集；69 个 `SCHEMA_*` 内嵌 name 与表项 name 全部相同、JSON 全合法、
    无未被表引用的常量。
24. **claims_patterns 配置自洽**——35 个 `mutating: true` 工具全部填了 patterns，
    0 个非 mutating 工具带 patterns。
25. **闸序与文档一致**——`tool.call` → L4 → allowed-tools → 休眠组 → 规则表 →
    skill 熔断 → 子 agent 白名单 → 入参 JSON → 工具本体，共 9 级，
    与 `dispatch.rs` 的实现顺序逐级对应。
26. **子 agent 白名单与全局表同源**——闸用 `ctx.tools`（= profile ∩ per-onion allowed），
    schema 层用 `bot_onions::effective_tools` 同一交集，两层不会互相脱节。
27. **allowed-tools 双闸口径一致**——两处都取「空集合 = 不限制」。
28. **per-tool 规则表单一来源**——`tool_rule_for`（`bot_fs.rs:70`）是唯一查询点。
29. **写库失败不再返回 Ok**——`commit_and_report` 统一走 `ToolResult::error` + 审计。
30. **字段校验跨层同源**——`due` 格式判据复用 `db::tasks::due_format_error`；
    `query_tasks` limit、`extract_document` limit、`MAX_TASK_FILES` 三处与 schema 一致。
31. **工具名字面量零悬空**——重复闸豁免表、收尾窗白名单、三个 profile 白名单、
    `SUBAGENT_ONLY_TOOLS`、`TOOL_GROUPS`、`TOOL_ALIASES` 全部命中 `TOOLS_TABLE`。
32. **无「拒绝/失败返 Ok」**——早退一律 `ToolResult::denied`（status=Denied），
    `is_success()` 判否；工具本体失败走 `ToolResult::error`。
33. **定时作业与提醒的触发点计算真同源**——唯一实现 `occurrence_after`
    （`bot_scheduler.rs:156`），提醒侧全部走它，作业侧走同一函数，无第二套解析器。
34. **UI 平台分支在判定层如实返回「不支持」而非 Ok**——`ui/mod.rs` 的 7 个平台分支
    全部 `Err("当前平台不支持 UI 自动化")`；OCR / 剪贴板 / 截图的兜底同样如实。
35. **本地 HTTP API 只绑回环**——`api_server.rs:225` `Server::http(("127.0.0.1", port))`，
    注释「安全红线：只绑回环地址，绝不 0.0.0.0」。
36. **API 鉴权与限流到位**——仅健康检查免鉴权；无 token/错 token → 401（有测试）；
    token 文件 0600 + 原子首建 + symlink 不跟随；比对用恒定时间 `ct_eq`；
    SSE 有连接上限 + 死连接收割。
37. **单实例插件在位，30s tick 不会多实例重复跑**——调度器只在 setup 启动一次。
38. **退出清理幂等且覆盖两条退出路径**——一次性门闩，`ExitRequested` + `Exit` 各调一次。
39. **启动期僵尸清扫覆盖了有 running 态的表**——app_runs、py-runs 目录、subagents、
    work_items、trace 均在清扫范围内。
40. **迁移 / journal 锁纪律正确**——不持锁跨 `block_on`。
41. **Windows OCR 模型目录三级回退 + ort 预检防 panic 毒化**。
42. **前端事件契约双向完整**——逐事件点名核实 28 个前端订阅事件，
    全部有发射点或有明确的「前端自发自收」来源（`bot-changed` / `bot-config-changed`
    / `edit-task` / `execute-task` / `workspace-changed` 均属后者）。
43. **会话号信封与序号统一盖章**——`stream_log.rs:237-258` 的 `emit_recorded`
    统一注入 `sessionId` 与 `seq`，缓冲副本也写 seq，与前端 `noteSeq` 闸口径一致。
44. **真源化投影对账有兜底**——`bot-msg-upserted` 无 id 命中且尾行非本回合时整段
    `loadFinalText` 重读；`bot-msg-dropped` 遇到投影外行同样整段重读，正确性以库为准。
45. **竞态守卫齐备**——`switchSession` / `openExecSession` / `refreshSubagents` /
    `ActivityPage.reload` 均有 `sessionIdRef` / `race` 令牌，旧响应不覆盖新状态；
    `useRaceSeq` 工具覆盖设置页与痕迹面板。
46. **定时器清理完整**——全仓 11 处 `setInterval` / `setTimeout` 全部有配对清理
    （`App.tsx:606`、`DueTicker.tsx:31`、`HomePage.tsx:133/153`、`ServicesPanel.tsx:117`、
    `McpPanel.tsx:313`、`useChatStream.ts:612`、`useChatSubagents.ts:81`、
    `ActivityPage/index.tsx:68`、`useNowTick.ts:11`、`useElapsed.ts:12`）。
47. **模块 store 与组件 state 未双份维护**——`planStore` / `askStore` / `streamState`
    三份外部状态统一走 `useSyncExternalStore`，无组件级副本。
48. **世代守卫防旧轮清场**——`beginTurn`/`endTurn` + `termEpochs` 墓碑语义确保
    看门狗判死后新开一轮不被旧轮 `finally` 带走。
49. **localStorage 写入失败均有兜底**——`theme.ts` / `bubbleStyle.ts` / `homeKpi.ts`
    全部 try/catch 包裹，不抛错阻塞 UI。
50. **空 catch 已清零**——全仓 `src/**` 无 `catch {}` 空块。
51. **Windows 路径处理正确**——`format.ts:5-8` 的 `basename` 用 `/[\\/]/` 同时切分两种分隔符。
52. **消息列表性能边界有实测保障**——生产入口走 `memo` 版气泡，
    `MessageList.perf.test.tsx:78-89` 断言 memo 版每帧耗时显著低于对照。
53. **在途态「三者同生共死」约束已落实**——`streamState.ts:120-129` 的 `dropSession`
    同删 meta / 游标 / 在途标记，不留半份状态。



---

## 八、修复优先级建议

### 批 A（凭据与数据安全，建议最先做）

| # | 事项 | 类型 |
|---|---|---|
| A-1 | P1-1：4 条配置写路径写前先跑 MCP 机密迁移，失败即中止 | 修复 |
| A-2 | P1-1 的配套：把「写路径必须先迁移」做成门禁（扫 `write_bot_config_file_locked` 的调用方是否都在迁移之后） | 门禁 |
| A-3 | P1-4：`mcp_oauth_logout` / `mcp_oauth_status` 接进 McpPanel | 接线 |

### 批 B（用户可见的错误行为）

| # | 事项 | 类型 |
|---|---|---|
| B-1 | P1-2：verdict 解析提到守卫之前（或守卫命中时先消费 verdict） | 修复 + 锚点测试 |
| B-2 | P1-3：终态 Succeeded 时显式清 `error` | 修复 + 锚点测试 |
| B-3 | P1-5：`at_expired` 先看总开关 → `Held`；`Delete` 分支补用户可见通知 | 修复 + 锚点测试 |
| B-4 | P1-6：拍板总开关对提醒/截止通知的口径，写进 SPEC + 两个模块注释 | 拍板 + 文档 |
| B-5 | P2-10 / P2-11：`http_request`(saveAs) 与 `update_plan` 登记为 mutating | 修复 + 基线同步 |
| B-6 | P2-1：`CONTEXT_CHARS_HARD_CAP` 要么真裁剪、要么改名改语义 | 语义对齐 |
| B-7 | **P1-8**：`bot-think-delta` 补 `noteSeq` 去重闸（同时修复纯思考期看门狗误判死） | 修复 + 锚点测试 |
| B-8 | P2-18：看门狗 `bot_stop` 失败不得静默吞掉 | 修复 |

### 批 C（接线与清理）

| # | 事项 | 类型 |
|---|---|---|
| C-1 | P1-4：`work_items_list` / `work_items_needing_human` 接线或删除（含 `list_recent`） | 二选一 |
| C-2 | P3-2 / P3-3 / P3-6：死状态、漏登记的提示词常量、不可达分支 | 清理 |
| C-3 | P2-2：三处提示词工具清单与 yaml 白名单对齐（最好由 yaml 生成） | 对齐 |
| C-4 | `audit_tauri_bridge` 的「已注册未调用」从 warn 升级为失败（或加白名单机制） | 门禁 |
| C-5 | P3-10：4 处模块头注释的数字漂移（照抄即假发现，已有前车之鉴） | 清理 |
| C-6 | P2-20：`cycleSetting` 删除或接三态 UI（**本次全仓唯一一个真死代码**） | 清理 |
| C-7 | P2-19：`ServicesPanel` 补 `active` 可见性闸（对齐 McpPanel） | 性能 |

### 批 D（运行不顺与限制）

| # | 事项 | 类型 |
|---|---|---|
| D-1 | **P1-7**：`lookup_receipt` 包 `spawn_blocking`；**守卫测试扫描清单补 `trace.rs`/`http.rs`/`plan.rs`** | 修复 + 门禁 |
| D-2 | P2-17：`http_request` saveAs 写盘挪 `spawn_blocking` | 性能 |
| D-3 | P2-4 / P2-8：阻塞 IO 挪 `spawn_blocking`，一轮共用一张进程快照 | 性能 |
| D-4 | P2-3：聚合查询加 LIMIT | 性能 |
| D-5 | P2-9：`Slot::Connecting` 加兜底超时 | 健壮性 |
| D-6 | P2-7：stdio 错误路径走 `stop_stdio_tree` | 健壮性 |
| D-7 | P2-5 / P2-6：技能 references 与 SKILL.md 加大小闸 | 限制 |
| D-8 | P2-13：`extract_document` 回灌预算与页面上限对齐 | 限制 |
| D-9 | P2-12 / P2-14：`deferred_gate` 补 `tool.return`；`activate_groups` 两趟校验 | 一致性 |
| D-10 | P2-15 / P2-16：一次性宽限统一；提醒写路径补广播（并同步文档口径） | 一致性 |

> **不建议**现在动的：P3-8（每轮整栈序列化）是真实开销但量级不致命，
> 等有性能实测数据再动；P1-4 的两条 work_items 命令，**先定「接线还是删除」再动手**——
> 两条路都合理，但不许两条路都不选（现状就是）。


---

## 九、复核记录（我本人逐条回到代码复验过的高危项）

| 发现 | 复验方式 | 结论 |
|---|---|---|
| P1-1 | 读 `skip_serializing` 定义 → 启动迁移的 warn 分支 → 4 条写路径 → `builtin.rs` 自陈注释 | **成立** |
| P1-2 | 读 `finalize_without_tools` 四段顺序 → `claims_mutation` 无否定词 → `parse_selfcheck_reply_lenient` 的存在理由 | **成立** |
| P1-3 | 读 `COALESCE` SQL → 续跑 UPDATE → `SubagentRowLine.tsx:49` 的 `statusOf` | **成立**（P1-2 之外的独立链） |
| P1-4 | 跑门禁拿 warn → 读 `McpPanel.tsx` 确认只有登录入口 → grep `list_recent` 的唯一调用方 | **成立** |
| P2-1 | 读 `request.rs` Fuse 分支 → `context.rs` 滑动窗只裁 role=tool → 砍史只砍中段 | **成立** |
| P2-2 | 读三处提示词 → 读两个 yaml 的 allowed → 读 `bot_orchestrator.rs:132-134` 注释 | **成立**（设计合理，错在提示词与注释） |
| P1-5 | 读 `bot_scheduler.rs:928-947` 的分支顺序 → 读 `Delete`/`Missed` 两个处理块 | **成立** |
| P1-6 | 全仓 grep `bot_get_enabled` 的 4 个读取点；确认 reminders/due_notify 零命中 | **成立** |
| P1-7 | 读 `trace.rs:29-53` → grep 同目录 7 处 `spawn_blocking` → 读 `bot_fs.rs:327-329` 约定原文 → 读守卫测试的扫描清单 | **成立** |
| P2-10 | 读 `schemas.rs` 的 `mutating: false` → 读 `http.rs:320-327` 的落盘 → 读 `guard.rs:380-383` 的 `unverifiable_ran` 集合 | **成立**（机制链完整） |
| P1-8 | 读 `useChatStream.ts:133-153` → 对比同文件 10 处 `noteSeq` 闸 → 读 `shell.rs:301` 与 `stream_log.rs:203-206` 确认 think 事件进缓冲且带 seq → 读 `useChatHistory.ts:202-204` 确认回灌复用同一批处理器 | **成立** |
| P2-20 | grep `cycleSetting` 全仓 → 只有定义 + `theme.test.ts`；生产走 `toggleTheme`（`App.tsx:398`） | **成立**（全仓唯一真死代码） |
| MCP OAuth 降级文件缺 0600 | 读 `secrets.rs` 两条分支 → 读 `keyring.rs:53-56` 的平台判定 | **降级为 P3**——`PlaintextFile` 后端只在 Linux 启用，而产品只支持 Win/macOS |
| Linux 无出网硬闸 | 读 `py/runtime.rs:900-924` 的 cfg 分支 | **降级为 P3**——同上，Linux 非目标平台 |

### 存疑（未下结论，需实机或设计意图确认）

1. **单轮无总时长上限**——唯一时间闸是 120s 逐块 idle，只要每 <120s 有数据，
   单轮可持续任意久。`sse.rs:13-14` 的注释显示 300s 总超时是被**有意去掉的**
   （无墙钟设计的延续），但该延续是否覆盖到「单轮」这一层，**无设计文档可查**。
2. **`plan_acceptance` 在无 session_id 时被丢弃**——`bot_chat/flow.rs:543` 定义、
   `:689` 取走，取走在 `if let Some(sid) = session_id.as_ref()` 内。若存在
   session_id=None 的生产入口，计划声明的验收标准会静默丢失。需确认所有生产入口
   是否都带 session_id。
3. **`effects.pending` 不过滤 attempts 能否造成永久幽灵行**——需 dispatch 待办
   连续 5 次失败的实机复现（payload 解析失败 / 行不存在 / handler panic）。
   「行不存在」时子 agent 行也不存在故不成幽灵，其余路径未构造出复现。
4. **`Slot::Connecting` 覆盖 `apply_config` 新槽位**——`manager.rs:516-523` 发现完成
   后无条件 insert Idle，理论上可覆盖 `apply_config` 刚写入的槽位；因 `call_lock`
   串行 + `apply_config` 对同指纹保留 Connected，未构造出可复现路径。
5. **`bot-think-delta` 重复拼接的实机复现**——P1-8 的完整链路依赖「回灌重放期间
   该会话正在输出思考增量」这一时序。代码路径与后端盖章均已核实，
   但**未做实机复现**（需后端在特定窗口内持续输出 thinking 并触发会话切换）。
6. **`MessageList` 无虚拟化**——`messages.map` 全量渲染（`MessageList.tsx:712`），
   长会话（数千条）下 DOM 节点数无上界。`MsgBubble` 的 memo 只解决重渲染成本、
   不解决节点数量。当前无实测数据支撑其构成实际瓶颈，故未列为发现。
7. **`useChatSend` 的 `messages` 闭包陈旧**——`send`（`useChatSend.ts:210`）用参数
   `messages` 构造 history，该函数每次 render 重建并作为 prop 下传给 `InputArea`。
   看起来安全，但 `InputArea` 内 `onKeyDown` → `send()` 的调用链未显式冻结引用，
   未做实机验证。

---

*本报告由主审 + 6 个只读审计子代理协作产出；所有 P0/P1 均经主会话回代码复验。*
*门禁与测试数据取自 2026-11-16 实跑。未修改任何业务代码。*

---

## 十、修复状态回填（2026-11-17，主会话逐条修复后登记）

> 本报告原文为只读审计。修复轮对 42 条发现逐条二次核实（P1 主会话亲核，
> P2/P3 由 5 个只读子代理并行核实），**42/42 全部成立无误报**。修复按 §八
> 批 A–D 落地，全部提交已过全量门禁（Rust 2385 + 前端 1252 + 对拍门禁全绿）。
> 两处细节修正：P2-9「只有 apply_config 才重置槽位」不准确（disconnect/再次
> ensure_connected 也可，但均需显式触发，自动路径确实无人兜底）；P2-15 的
> SPEC 两侧各自如实记载宽限值，属设计口径分裂而非文档失实。

| 批 | 提交 | 覆盖 |
|---|---|---|
| A | `ea108952` | P1-1（4 条写路径迁移前置 + 新门禁 audit_config_write_migration.py）+ P1-4 OAuth 半（McpPanel 接 mcp_oauth_status 角标与 mcp_oauth_logout） |
| B1 | `6726b75a` | P1-2（verdict 消费提前到守卫之前）+ P1-3（成功终态清 error + 前端防御 + 完整链锚点） |
| B2/B3 | `e8a7e7d1` | P1-5（at_expired 先看开关 → Held，Delete 补用户通知）+ P2-10/11（http_request/update_plan 登记 mutating，基线 35→37）+ P1-8（think 流补 noteSeq）+ P2-18（看门狗叫停失败不静默）+ P2-1（硬顶语义诚实化） |
| C | `90f1b61e` | P1-4/P3-1（work_items 双命令删除）+ P2-20（cycleSetting 删）+ P3-2/3/5/6 + P2-2（提示词↔白名单对齐）+ P2-19（ServicesPanel 闸）+ 门禁升级 + P3-7/10 注释漂移 |
| D | `7cd21f33` | P1-7（trace spawn_blocking + 守卫接线锁）+ P2-17/4/8（其余阻塞 IO）+ P2-9（Connecting 自愈）+ P2-7（stdio 树杀）+ P2-3/5/6/12/14/16 + P1-6/P2-15 口径注释 + P3-4/11/12/13/14 |

**拍板偏离登记**（用户未应答拍板询问，按推荐+保守执行，均可低成本回滚）：
- P1-4/P3-1：work_items 双命令按仓库「不留死代码」硬禁令**删除**（YAGNI；
  needs_attention_list 保留）。
- P2-20：cycleSetting 按**删除**处理（三态若要支持单独立项）。
- P1-6：**保持现状 + 注释登记**（总开关对提醒/截止通知不门控——改口径须先
  改 SPEC 再动代码，两模块头注释已指路）。
- P2-15：**保持现状 + 注释登记**（60s vs 24h 为有意差异，理由进两常量注释）。
- P2-1：**语义诚实化**（零行为变更；Fuse 档真裁剪单独立项）。
- P2-8：sweep_round 已挪 spawn_blocking；**单轮共用快照**需改 observe/gate 两
  函数签名，登记待有实测压力再做。
- P3-8：按报告建议**不动**（等性能实测数据）。

**修法偏离登记**（与报告建议的差异，理由在对应提交说明）：
- P2-4：报告首选 spawn_blocking，但闸门闭包是同步 FnMut 放不进 await——改用
  报告备选「不跑 DDL 的轻连接」（直接 open + busy_timeout，单行 SELECT）。
- P1-2：报告建议「verdict 解析整段提前」会改变触发轮守卫语义（守卫推回会被
  当 verdict 解析，绕回原坑）——改为「触发段留原位 + verdict 消费段拆出提前」。
- P1-5：旧测试锁死的正是缺陷行为，按新语义改写（开关开启才 Delete）。

**遗留**：本批全部提交未推送（`f35f3608..7cd21f33` 共 6 笔），等实机验收后
推送。待立项三件：P2-1 真裁剪、P2-8 快照共享、三态主题 UI。

