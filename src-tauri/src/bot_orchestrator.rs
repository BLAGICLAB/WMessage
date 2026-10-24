//! 子 Agent 编排器（orchestrator）—— 方案 B（docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md）。
//!
//! 与 bot_execute_task 的边界（设计 §2）：bot_execute_task 是「单发子 agent」（用户点执行，
//! 无编排语义）；本模块是「受管子 agent」（主 agent 派发，orchestrator 管生命周期/预算/
//! 审计）。两者复用 exec 运行基建，但入口、存储、事件各自独立。
//!
//! 分批（设计 §11）：
//! - `subagents` 表 + 生命周期状态机 + spawn/check/cancel 内部实现 +
//!   parent_task_id 关联（行上存派发时的主卡 id，不建子卡）+ 预算字段落表 +
//!   审计 subagent_spawned。不暴露 LLM 工具、不注册 tauri 命令。
//! - 三工具进 registry + 递归双保险 + 任务包装渲染 + 提示词 + runner + 收尾 JSON 解析。
//! - 预算强制（turns/tool_calls/wall）+ 并发排队（per-session 2 / global 3）+ 前端。
//!
//! 结构约定（同 task_patch / task_patch_locked 先例）：纯 DB 核心 `*_locked` 可在
//! in-memory conn 上单测；异步包装负责 spawn_blocking + DB_WRITE_LOCK + 审计 + 广播。

use crate::db::{now_ms, SubagentBudget, SubagentProfile, SubagentRow, SubagentStatus};
use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

// ── subagent-split: begin ──
// 拆分后的子模块 re-export：旧路径 `crate::bot_orchestrator::<name>` 1:1 保持可用。
pub use crate::bot::subagent_spawn::clamp_budget;
pub use crate::bot::subagent_spawn::spawn_subagent;
pub use crate::bot::subagent_spawn::MAX_GLOBAL_RUNNING;
pub use crate::bot::subagent_spawn::MAX_PER_SESSION_RUNNING;
pub use crate::bot::subagent_tools::tool_cancel_subagent;
pub use crate::bot::subagent_tools::tool_check_subagent;
pub use crate::bot::subagent_tools::tool_read_own_card;
pub use crate::bot::subagent_tools::tool_spawn_subagent;
pub use crate::bot::subagent_tools::tool_write_artifact_file;
// 跨模块共用的私有项：门面内的实现与模块内测试都要经这里取。
pub(crate) use crate::bot::subagent_digest::apply_machine_verdicts;
pub(crate) use crate::bot::subagent_digest::count_met_against_declared;
pub(crate) use crate::bot::subagent_digest::eval_machine_criteria;
pub(crate) use crate::bot::subagent_digest::mark_digested;
pub(crate) use crate::bot::subagent_digest::reconcile_machine_override;
pub(crate) use crate::bot::subagent_digest::spawn_parent_report;
pub(crate) use crate::bot::subagent_digest::subagent_digest_block;
pub(crate) use crate::bot::subagent_digest::MACHINE_PREFIXES;
pub(crate) use crate::bot::subagent_digest::UNPARSEABLE_MARKER;
pub(crate) use crate::bot::subagent_repair::budget_force_needed;
pub(crate) use crate::bot::subagent_repair::classify_outcome;
pub(crate) use crate::bot::subagent_repair::parse_final_json;
pub(crate) use crate::bot::subagent_repair::repair_final_json_once;
pub(crate) use crate::bot::subagent_runner::run_subagent;
pub(crate) use crate::bot::subagent_spawn::budget_of_row;
pub(crate) use crate::bot::subagent_spawn::resolve_row;
pub(crate) use crate::bot::subagent_spawn::resolve_spawn_profile;
pub(crate) use crate::bot::subagent_spawn::SubagentGate;
pub(crate) use crate::bot::subagent_tools::artifact_dir_of;
pub(crate) use crate::bot::subagent_tools::gen_dir_rule_for;
pub(crate) use crate::bot::subagent_tools::safe_rel_path;
// ── subagent-split: end ──

/// 预算默认值与硬顶（设计 §6 已）——常量与钳制实现在 db::SubagentBudget（写口
/// 强制：spawn 与 task_patch 都过 clamped()），此处 re-export 保持原路径可用。
pub use crate::db::{
    DEFAULT_MAX_TOOL_CALLS, DEFAULT_MAX_TURNS, DEFAULT_MAX_WALL_SECONDS, MAX_TURNS_HARD_CAP,
};
/// check_subagent 的 wait_ms 上限（设计 §5；放宽理由见
/// docs/SUBAGENT-CHECK-BACKOFF-DESIGN-2026-10-18.md）。
///
/// 上限放到 10 分钟：每返回一次 check_subagent，主 agent 就要多走一整个
/// LLM 轮次（重发请求 + 重算上下文 + 把轮询结果再塞进历史），所以「早知道」
/// 的边际收益远小于「少醒几次」。短上限逼着模型反复短轮询——实测一个 3 分钟
/// 的调研子任务被拆成 4 次 30s 等待，每次都是一轮模型开销，最后还因为等不到
/// 而把「仍在进行中」当成了整轮答复。一次给足预算让它整个睡到终态，轮次降到 1。
pub const CHECK_WAIT_MS_CAP: u64 = 600_000;
/// wait 兜底轮询步长阶梯（毫秒）。首档 1500ms：终态另有事件唤醒
/// （见 `finish_notify`），这条阶梯只是漏通知时的安全网，所以起手就不必密——
/// 越密只是越频繁地开库跑全量 DDL。末档后恒定封顶：兜底延迟要有上界，
/// 否则退化成看不见的卡住。
const CHECK_POLL_LADDER_MS: [u64; 10] = [
    1_500, 3_000, 6_000, 12_000, 24_000, 30_000, 60_000, 120_000, 300_000, 600_000,
];

/// 三段式派发阶段（设计 §8.5）——主 agent 在 recon/executor/verifier 三段闭环中
/// 选定当前阶段的角色语义。**不破坏 SubagentProfile 枚举**——profile 仍是工具白名单
/// 档（Research/Coder/General），stage 在 prompt 装配层额外拼一段角色 prompt：
/// - Recon → 拼 PROFILE_RECON（只读 + 联网调研）
/// - Executor → 拼 PROFILE_EXECUTOR（写文件/跑脚本/生成文档）
/// - Verifier → 拼 PROFILE_VERIFIER（只读 + 逐条 acceptance_criteria 验收）
/// 阶段不落库——spawn 触发 runner 的 in-memory 闭包 move 进 runner，崩溃重启的
/// queued 行降级到基线 PROFILE_*（profile 仍存表），属可控降级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStage {
    Recon,
    Executor,
    Verifier,
}

impl SubagentStage {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentStage::Recon => "recon",
            SubagentStage::Executor => "executor",
            SubagentStage::Verifier => "verifier",
        }
    }

    // 有意不实现 std::str::FromStr：解析失败返回 Option 而非 Err
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "recon" => SubagentStage::Recon,
            "executor" => SubagentStage::Executor,
            "verifier" => SubagentStage::Verifier,
            _ => return None,
        })
    }

    /// stage → 默认 profile 档（设计 §8.5 映射）：
    /// - recon/verifier → research（只读工具足够）
    /// - executor → coder（写类工具必需）
    /// 调用方未显式给 profile 时按此派生；显式给了的 profile 仍优先（角色与白名单解耦）。
    pub fn default_profile(self) -> SubagentProfile {
        match self {
            SubagentStage::Recon | SubagentStage::Verifier => SubagentProfile::Research,
            SubagentStage::Executor => SubagentProfile::Coder,
        }
    }
}

/// 派发时已解析校验过的 per-onion 配置档。
///
/// **随内存闭包进 runner，不落 subagents 行**：这两项是「这次派发做了什么决定」，
/// 不是子 agent 的固有属性——重启后不会有第二个派发方重跑同一次 spawn
/// （行已终态）。真正需要长期留存的是 `bot_sessions` 的两列，runner 建会话时写。
#[derive(Debug, Clone)]
pub struct SpawnOnion {
    /// 配置档名，同时是 `<数据目录>/onions/<config>.yaml` 的文件名
    pub config: String,
    /// 配置档 `max_turn`：轮数上限，只收紧不覆盖
    pub max_turn: Option<u32>,
}

/// spawn 入参（设计 §5 工具签名的内部形态）。
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub objective: String,
    pub profile: SubagentProfile,
    /// 三段式派发阶段（设计 §8.5）：None = 走基线 PROFILE_*（单段派发兼容路径）。
    /// 拼装阶段：runner 启动时随闭包 in-memory 传过去（详见 `spawn_subagent`）。
    pub stage: Option<SubagentStage>,
    /// **spawn 必填**，每条可检验；落行上 acceptance_json，runner 渲染包装时现读
    pub acceptance_criteria: Vec<String>,
    /// 上下文摘要（设计 §8.5「上下文摘要」段）：只给必要背景，不倒主对话全文。
    /// 落行上 context_summary——runner 渲染包装时从行上现读。
    pub context_summary: Option<String>,
    pub budget: Option<SubagentBudget>,
    /// A 期参数透传：None/无效值回退当前 active model（设计 §5）
    pub model_profile: Option<String>,
    /// per-onion 配置档（设计 §3.7 三段流水线的权限载体）。
    /// None = 不套配置档，工具集就是 profile 白名单原样。
    ///
    /// 名字与上限都在派发时**已解析校验**：加载失败必须在建行之前就报错，
    /// 否则留下一条「派发成功但永远跑不起来」的幽灵行。
    pub onion: Option<SpawnOnion>,
    /// 主卡（编排计划卡）id；缺卡 → TaskNotFound
    pub parent_task_id: Option<String>,
    /// 派发者主会话 id（串链审计）
    pub parent_session_id: Option<String>,
}

/// spawn 应答：非阻塞立即返回（设计 §5）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnAck {
    pub subagent_id: String,
    /// 历史子任务卡 id：子 agent 不再建卡，新应答恒为 None（字段保留兼容旧消费方）
    pub task_id: Option<String>,
    pub trace_id: String,
    pub status: SubagentStatus,
    /// 命中幂等键：返回的是**已在跑**的同一个子 agent，本次没有新建行、
    /// 也没有再起一个 runner。消费方据此区分「派发成功」与「重复派发未新建」。
    pub deduped: bool,
    /// **生效**预算（钳制后的值，不是调用方给的值）。
    ///
    /// 回显给调用方是必须的：调用方给的墙钟可能被钳到 30..=3600，模型不知道
    /// 自己的子 agent 最多能跑多久，就会按「我给了一小时」安排后续动作。
    pub budget: SubagentBudget,
}

/// check 应答（设计 §5/§7：status/progress/result?/error?）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckState {
    pub subagent_id: String,
    /// 历史子任务卡 id：无卡行恒为 None（字段保留兼容旧消费方）
    pub task_id: Option<String>,
    /// 子 agent 执行会话（围观串链键；runner 建会话前为 None）
    pub session_id: Option<String>,
    pub status: SubagentStatus,
    /// 进度小计：验收标准逐条达成数 + 预算消耗（A3 起带 turns/tool_calls）
    pub progress: serde_json::Value,
    /// 收尾结构化结果（§7），未收尾为 None
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// cancel 应答：`already_terminal` = 目标已是终态（幂等 no-op，不报错）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelAck {
    pub ok: bool,
    pub subagent_id: String,
    pub status: SubagentStatus,
    pub already_terminal: bool,
}

/// check 纯 DB 核心：幂等读 + 进度小计（验收标准逐条达成比）。
/// 进度口径：total = 行上 acceptance_json 条数（派发时写死，全生命周期不变）；
/// done = 收尾 JSON 里 met=true 的条数——普通行读 `acceptance_check`
/// （RESULT_SCHEMA_HINT 要求的逐条验收），verifier 行读 `checks`
/// （verifier 独有 schema，与 classify 的结构识别同口径）。
/// 未收尾时 done 恒为 0。键名沿用 subtasksDone/subtasksTotal，前端契约不变。
pub(crate) fn check_subagent_locked(
    conn: &rusqlite::Connection,
    key: &str,
) -> CommandResult<CheckState> {
    let row = resolve_row(conn, key)?;
    let declared = acceptance_criteria_of(&row);
    let subtasks_total: usize = declared.len();
    let result: Option<serde_json::Value> = row
        .result_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let subtasks_done: usize = result
        .as_ref()
        .and_then(|v| {
            v.get("acceptance_check")
                .and_then(|c| c.as_array())
                .or_else(|| v.get("checks").and_then(|c| c.as_array()))
        })
        .map(|a| count_met_against_declared(&declared, a))
        // 交叉核对后仍封顶：重复条目已被去重，这里再兜一层防御
        // （子 agent 重复报同一条时 done 不得超过 total）
        .unwrap_or(0)
        .min(subtasks_total);
    // 排队位次：queued 状态时给出 FIFO 前面还有几个
    let queue_position = if row.status == SubagentStatus::Queued {
        SubagentGate::queue_position(row.parent_session_id.as_deref())
    } else {
        None
    };
    let mut progress = serde_json::json!({
        "subtasksDone": subtasks_done,
        "subtasksTotal": subtasks_total,
    });
    // 轮数读数：子 agent 在跑时 ctx 里有活的计数器，终态后守卫注销读不到。
    // 预算从行上 budget_json 取——轮数与预算放一起看才有意义（10/20 与 10/200
    // 是完全不同的紧迫度）。readOnly 消费，不参与任何强制。
    if let Some(budget) = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<SubagentBudget>(s).ok())
    {
        let used = crate::tool_guard::subagent_ctx(row.session_id.as_deref())
            .map(|c| c.used_rounds.load(std::sync::atomic::Ordering::Relaxed));
        if let Some(used) = used {
            progress["roundsUsed"] = serde_json::json!(used);
            progress["roundsBudget"] = serde_json::json!(budget.max_turns);
        }
    }
    if let Some(pos) = queue_position {
        progress["queuePosition"] = serde_json::json!(pos);
    }
    Ok(CheckState {
        subagent_id: row.id,
        task_id: row.task_id,
        session_id: row.session_id,
        status: row.status,
        progress,
        result,
        error: row.error,
    })
}

/// cancel 纯 DB 核心：queued/running → cancelled（error=原因）；终态 no-op。
/// 子 agent 无卡，取消只动行——前端经 subagent-finished 事件看状态。
pub(crate) fn cancel_subagent_locked(
    conn: &rusqlite::Connection,
    key: &str,
    reason: &str,
    now: i64,
) -> CommandResult<CancelAck> {
    let row = resolve_row(conn, key)?;
    if row.status.is_terminal() {
        return Ok(CancelAck {
            ok: true,
            subagent_id: row.id,
            status: row.status,
            already_terminal: true,
        });
    }
    crate::db::update_subagent_status(conn, &row.id, SubagentStatus::Cancelled, now, Some(reason))
        .map_err(CommandError::DbError)?;
    Ok(CancelAck {
        ok: true,
        subagent_id: row.id,
        status: SubagentStatus::Cancelled,
        already_terminal: false,
    })
}

/// 等待退出原因（审计事件 `subagent_check_wait` 的 exit 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitExit {
    /// 等到了终态
    Terminal,
    /// 预算耗尽仍未终态（wait_ms 到点）
    Timeout,
    /// 用户 /stop
    Stopped,
}

impl WaitExit {
    fn as_str(self) -> &'static str {
        match self {
            WaitExit::Terminal => "terminal",
            WaitExit::Timeout => "timeout",
            WaitExit::Stopped => "stopped",
        }
    }
}

/// 终态唤醒信号：任何子 agent 转入终态都经 `emit_subagent_finished` 这一个出口，
/// 在那里 notify_waiters 即可覆盖排队期取消 / 启动中止 / ghost 路径，不漏。
/// 用进程内 `Notify` 而非 Tauri 事件——等待方就在同一进程里，没必要过事件总线。
pub(crate) fn finish_notify() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// 兜底轮询步长：阶梯内递增，出阶梯后恒定封顶。
fn next_poll_step(step: usize) -> u64 {
    CHECK_POLL_LADDER_MS[step.min(CHECK_POLL_LADDER_MS.len() - 1)]
}

/// 等待核心：对「读一次状态」的取数函数泛型，因此可在没有 AppHandle 的单测里
/// 直接驱动（生产传入 spawn_blocking + open_db 的闭包）。
///
/// 唤醒策略 = 终态事件 **或** 兜底步长，先到先醒。正常路径下子 agent 一转终态
/// 就立刻返回，阶梯走不完；阶梯只兜「通知漏了」的情况。
async fn wait_until_terminal<F, Fut>(
    read: F,
    wait_ms: u64,
    stop: Option<&crate::bot_slash::StopToken>,
) -> Result<(CheckState, WaitExit), CommandError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = CommandResult<CheckState>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    let mut step = 0usize;
    loop {
        // 先把唤醒监听挂上、再读状态：顺序反了会出现「读完状态到开始等待之间
        // 恰好转入终态」的窗口，而 notify_waiters 不留 permit，那次通知就丢了
        //（丢一次不致命，兜底阶梯接得住，只是白等一个步长）。
        let notified = finish_notify().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let state = read().await?;
        if state.status.is_terminal() {
            return Ok((state, WaitExit::Terminal));
        }
        // 停止先于超时判定：用户按了 /stop 就该按「被叫停」记账，
        // 而不是记成「等够了没等到」。
        if stop.is_some_and(|t| t.stopped()) {
            return Ok((state, WaitExit::Stopped));
        }
        if std::time::Instant::now() >= deadline {
            return Ok((state, WaitExit::Timeout));
        }
        let step_ms = next_poll_step(step);
        step += 1;
        let _ = tokio::time::timeout(std::time::Duration::from_millis(step_ms), notified).await;
    }
}

/// check 异步包装：wait_ms=0 单次读；>0 等待至终态、预算耗尽或用户停止
/// （幂等可轮询，设计 §5；等待策略见 docs/SUBAGENT-CHECK-BACKOFF-DESIGN-2026-10-18.md）。
pub async fn check_subagent(
    app: &AppHandle,
    key: &str,
    wait_ms: u64,
    stop: Option<&crate::bot_slash::StopToken>,
    session_id: Option<&str>,
) -> CommandResult<CheckState> {
    let wait = wait_ms.min(CHECK_WAIT_MS_CAP);
    let started = std::time::Instant::now();
    let app2 = app.clone();
    let key2 = key.to_string();
    let (state, exit) = wait_until_terminal(
        || {
            let app = app2.clone();
            let key = key2.clone();
            async move {
                tauri::async_runtime::spawn_blocking(move || -> CommandResult<CheckState> {
                    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
                    check_subagent_locked(&conn, &key)
                })
                .await
                .map_err(|e| CommandError::from(format!("subagent check 线程 join 失败：{e}")))?
            }
        },
        wait,
        stop,
    )
    .await?;
    // 问到终态 = 父上下文已拿到这份结果：记进去重表，digest 不再重念，
    // 自动汇报轮也不会因为「没人提过」而重复讲同一件事。
    if state.status.is_terminal() {
        mark_digested(session_id, &state.subagent_id);
    }
    // 等待口径留痕：wait_ms=0 的单次读不记（否则审计被即时查询刷屏）。
    // 有等待才记——本次优化唯一的验证手段，「轮次少了」不能靠感觉，
    // 得能看实际等待时长与退出原因的分布。
    if wait > 0 {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "subagent_check_wait",
            &[
                ("wait_ms", wait.to_string()),
                ("waited_ms", started.elapsed().as_millis().to_string()),
                ("exit", exit.as_str().to_string()),
            ],
        );
    }
    Ok(state)
}

/// cancel 异步包装：置 cancelled + 审计 + 远程停 runner。终态事件由 runner
/// 补发（排队闸/槽后复核/收尾路径各自 emit）——本函数不发，避免与 runner
/// 对同一行收尾双发、前端重复弹「子任务已取消」。
pub async fn cancel_subagent_async(
    app: &AppHandle,
    key: &str,
    reason: &str,
) -> CommandResult<CancelAck> {
    let app2 = app.clone();
    let key = key.to_string();
    let reason = reason.to_string();
    let audit_reason = reason.clone();
    let ack = tauri::async_runtime::spawn_blocking(move || -> CommandResult<CancelAck> {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        let now = chrono::Utc::now().timestamp_millis();
        cancel_subagent_locked(&conn, &key, &reason, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent cancel 线程 join 失败：{e}")))??;
    if !ack.already_terminal {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "subagent_cancelled",
            &[
                ("subagent_id", ack.subagent_id.clone()),
                ("reason", audit_reason.clone()),
            ],
        );
        crate::bot::audit_log(
            app,
            &format!(
                "subagent_cancelled | id: {} | reason: {}",
                ack.subagent_id,
                crate::bot::truncate_for_log(&audit_reason, 120)
            ),
        );
        // 远程停止 runner 工具循环（令牌在 runner 注册；watcher 2s 兜底轮询）
        let token = crate::db::lock::lock_or_recover(
            &crate::app_state::subagent_stops(app),
            "app_state::subagent_stops",
        )
        .get(&ack.subagent_id)
        .cloned();
        if let Some(token) = token {
            token.stop();
        }
    }
    Ok(ack)
}

// 工具实现 + runner + 收尾解析

/// 持 DB_WRITE_LOCK 的阻塞 DB 操作辅助（runner 链路各步共用）。
pub(crate) async fn db_locked<R, F, T>(app: &AppHandle<R>, f: F) -> CommandResult<T>
where
    R: tauri::Runtime,
    F: FnOnce(&mut rusqlite::Connection) -> CommandResult<T> + Send + 'static,
    T: Send + 'static,
{
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let mut conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        f(&mut conn)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent db 线程 join 失败：{e}")))?
}

/// 行上已声明的验收标准（渲染包装 / 机器判定 / 进度统计三处共用一个读法）
pub(crate) fn acceptance_criteria_of(row: &SubagentRow) -> Vec<String> {
    row.acceptance_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// 提示词用的软截断（按字符，不转义控制字符）。
///
/// **不复用 `truncate_for_log`**：那个会把换行/制表转义成 `\n` 形态，那是日志
/// 专用形态；回报段是要给模型读的，多行摘要转义成一坨反而更难读。
pub(crate) fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max).collect();
    out.push('…');
    out
}

// 并发闸per-session 2 / global 3 FIFO）

// runner（执行引擎）

/// 启动清扫：进程重启后没有任何 runner 在跑，库里 running/queued 的行是
/// 上一世的僵尸——不收尾就在子 agent 面板里永远转圈。逐行落终态 + 审计，
/// 谁被清的、可追溯；取消原因写进行上的 error，`check` 回传给父 agent 与面板。
///
/// 排队中的行**不恢复**，是刻意的：恢复需要连 stage（角色语义层）与 onion
/// （per-agent 配置）一起还原，而这两项没落在行上（行只有 profile / model）。
/// 只按 profile 重投会跑出一个「用默认角色和默认配置完成」的成功结果，比取消
/// 更难发现。真要恢复，先把 stage / onion 持久化，再谈重投。
///
/// 前置前提：应用是单实例（tauri-plugin-single-instance），所以本进程启动时
/// 不可能有另一个进程正跑着同一份库的子 agent——否则这里会误杀活跑的。
pub fn sweep_zombie_subagents(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let sweep = {
            let conn = match crate::db::open_db(&app) {
                Ok(c) => c,
                Err(e) => {
                    crate::bot::audit_log(
                        &app,
                        &format!("subagent_zombie_sweep_db_fail | err: {e}"),
                    );
                    return;
                }
            };
            let _g = crate::db::lock_db_write();
            let mut cleaned = 0usize;
            // 终态按「事实」选：排队中的从未开跑，是被重启丢弃（cancelled）；
            // 已开跑的中途断掉才算失败（failed）。状态机也只放行这两条边
            for (st, to) in [
                (SubagentStatus::Queued, SubagentStatus::Cancelled),
                (SubagentStatus::Running, SubagentStatus::Failed),
            ] {
                let rows = match crate::db::load_subagents_by_status(&conn, st) {
                    Ok(r) => r,
                    Err(e) => {
                        crate::bot::audit_log(
                            &app,
                            &format!(
                                "subagent_zombie_sweep_load_fail | status: {} | err: {e}",
                                st.as_str()
                            ),
                        );
                        continue;
                    }
                };
                for row in rows {
                    let id = row.id.clone();
                    let res = crate::db::update_subagent_status(
                        &conn,
                        &id,
                        to,
                        now_ms(),
                        Some("应用重启，执行现场已不存在（僵尸行清理）"),
                    );
                    match res {
                        Ok(_) => {
                            cleaned += 1;
                            crate::bot::audit_log(
                                &app,
                                &format!(
                                    "subagent_zombie_cleared | id: {id} | was: {}",
                                    st.as_str()
                                ),
                            );
                        }
                        Err(e) => crate::bot::audit_log(
                            &app,
                            &format!("subagent_zombie_clear_fail | id: {id} | err: {e}"),
                        ),
                    }
                }
            }
            cleaned
        };
        if sweep > 0 {
            crate::bot::audit_log(
                &app,
                &format!("subagent_zombie_sweep_done | count: {sweep}"),
            );
        }
    });
}

// final_text 由上面的 move 闭包直接捕获（历史落库后不再外用）

// LLM 工具实现（registry 调用）

/// cancel_subagent tauri 命令任务卡停止按钮 / 子会话停止键的同 API 入口，
/// 设计 §4.3；薄壳转内部 cancel_subagent_async）。
#[tauri::command]
pub async fn cancel_subagent(
    app: AppHandle,
    key: String,
    reason: Option<String>,
) -> CommandResult<CancelAck> {
    cancel_subagent_async(&app, &key, reason.as_deref().unwrap_or("用户取消")).await
}

/// list_subagents 的行视图 = DB 行 + 内存态实时轮数。
///
/// 轮数只活在 tool_guard 注册表（runner 开跑注册、收尾注销），不落 DB：落库就得
/// 每轮写一次，与「轮数仪表共用同一个 Arc」的设计正好抵消。守卫注销后读不到就
/// 报 None——凭空造数比不报更坏（与 check_subagent 同口径）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunView {
    #[serde(flatten)]
    pub row: crate::db::SubagentRow,
    /// 已跑轮数；非在途（守卫已注销）为 None
    pub rounds_used: Option<u32>,
    /// 轮数硬顶；与轮数同源同报，10/20 与 10/200 的紧迫度完全不同
    pub rounds_budget: Option<u32>,
    /// 已执行工具调用数；与轮数同源（tool_guard 内存仪表），终态为 None
    pub tool_calls_used: Option<u32>,
    /// 工具调用硬顶；与已用数同报——收尾窗从 100/100 起开，只看轮数看不出
    pub tool_calls_budget: Option<u32>,
}

/// 列某主会话派出的子 agent（新→旧）。聊天内嵌子 agent 区域与右侧栏
/// 「子 Agent」工作面的数据源——子 agent 不再建独立会话后，用户没有别的地方
/// 能看见自己派出去的那些还活着/刚跑完。
#[tauri::command]
pub async fn list_subagents(
    app: AppHandle,
    session_id: String,
    limit: Option<u32>,
) -> CommandResult<Vec<SubagentRunView>> {
    let limit = limit.unwrap_or(20).clamp(1, 100);
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        let rows = crate::db::load_subagents_by_parent(&conn, &session_id, limit)
            .map_err(CommandError::DbError)?;
        Ok(rows.into_iter().map(attach_rounds).collect())
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent list 线程 join 失败：{e}")))?
}

/// 把内存态仪表（轮数 / 工具调用数）挂到 DB 行上。在途行取 tool_guard 的
/// 计数器，终态行留空。
fn attach_rounds(row: crate::db::SubagentRow) -> SubagentRunView {
    let ctx = crate::tool_guard::subagent_ctx(row.session_id.as_deref());
    SubagentRunView {
        rounds_used: ctx
            .as_ref()
            .map(|c| c.used_rounds.load(std::sync::atomic::Ordering::Relaxed) as u32),
        rounds_budget: ctx.as_ref().map(|c| c.budget.max_turns),
        tool_calls_used: ctx
            .as_ref()
            .map(|c| c.used_tool_calls.load(std::sync::atomic::Ordering::Relaxed) as u32),
        tool_calls_budget: ctx.as_ref().map(|c| c.budget.max_tool_calls),
        row,
    }
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;
    // ── subagent-split: begin ──
    use crate::bot::subagent_digest::{
        bot_session_exists_locked, build_subagent_digest, is_digested, merge_machine_verdicts,
        parse_machine_criterion, parse_result_json, should_report_to_parent, MachineCheck,
        MachineVerdict, DIGEST_MAX_ITEMS, DIGEST_MAX_SUMMARY_CHARS,
    };
    use crate::bot::subagent_repair::json_object_spans;
    use crate::bot::subagent_runner::{
        effective_status, panic_to_command_error, render_task_wrapper,
    };
    use crate::bot::subagent_spawn::{
        apply_onion_turn_cap, clamp_budget, gate, spawn_subagent_locked, GateTicket,
    };
    use crate::bot::subagent_tools::{own_card_payload, spawn_receipt_text};
    // ── subagent-split: end ──
    // 仅 parent_card 夹具用：子 agent 自己不再建卡，但派发仍可挂编排计划卡
    use crate::db::Task;

    /// 编排器源码全景：拆分后代码分布在门面与子模块里，源码守卫按函数定位时扫这一份。
    ///
    /// 顺序要紧：子模块在前、门面在后——守卫用 `split_once("#[cfg(test)]")`
    /// 取「首个测试模块之前」的生产切片，门面里的测试模块排在最后，
    /// 子模块的代码才落进生产切片。
    fn orch_prod_src() -> String {
        [
            include_str!("bot/subagent_digest.rs"),
            include_str!("bot/subagent_repair.rs"),
            include_str!("bot/subagent_runner.rs"),
            include_str!("bot/subagent_spawn.rs"),
            include_str!("bot/subagent_tools.rs"),
            include_str!("bot_orchestrator.rs"),
        ]
        .concat()
    }

    /// runner panic 必须归一成普通失败，收尾路径照常走。
    ///
    /// panic 会跳过整条收尾（取消令牌反注册、状态落库、subagent-finished
    /// 事件）——前端那一行永远转圈，行状态停在 running。
    #[test]
    fn runner_panic_becomes_ordinary_failure() {
        let e = panic_to_command_error("index out of bounds");
        let msg = e.message();
        assert!(
            msg.contains("panic") && msg.contains("index out of bounds"),
            "panic 信息要如实带进失败原因：{msg}"
        );
        // 超长信息截断（审计日志与失败原因都不该被一段长 payload 撑爆）
        let long = "x".repeat(5000);
        let e2 = panic_to_command_error(&long);
        assert!(
            e2.message().chars().count() < 400,
            "panic 信息应截断到 200 量级：{}",
            e2.message().chars().count()
        );
    }

    /// 接线守卫：runner future 必须包 catch_unwind，且取消令牌登记要有
    /// RAII 反注册——两者都是「panic 路径不漏」的前提。
    #[test]
    fn runner_is_panic_isolated_and_stop_token_has_raii_cleanup() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bot_orchestrator.rs"
        ))
        .expect("应能读到自己");
        let start = src
            .find("async fn run_subagent(")
            .expect("应能找到 run_subagent");
        let rest = &src[start..];
        // 函数体到顶格 "}" 为止（嵌套块的 "}" 都带缩进）
        let end = rest[1..].find("\n}\n").map(|i| i + 1).unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(
            body.contains("catch_unwind"),
            "runner future 必须包 catch_unwind（panic 要归一成普通失败走收尾）：{body}"
        );
        assert!(
            body.contains("StopTokenGuard"),
            "取消令牌登记必须配 RAII 反注册（panic 路径不能把条目留在 subagent_stops）：{body}"
        );
    }

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // tasks 表内联最小 schema（列序与 open_db 一致，含  新增三列）；
        // subagents 表与生产同源 DDL（单源真相）
        conn.execute_batch(crate::db::tasks::TASKS_DDL).unwrap();
        conn.execute_batch(crate::db::SUBAGENTS_DDL).unwrap();
        conn
    }

    fn parent_card(conn: &rusqlite::Connection) -> String {
        let card = Task {
            plan_start: None,
            plan_end: None,
            parent_id: None,
            id: "parent-1".into(),
            title: "主编排卡".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            column: crate::db::TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: Some(false),
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: Some(1_000),
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            depends_on: None,
            model: None,
            owner_id: None,
            created_at: None,
            expected_updated_at: None,
        };
        crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).unwrap();
        "parent-1".into()
    }

    // check 等待策略：阶梯递进 + 终态事件唤醒 + /stop 打断

    fn fake_state(status: SubagentStatus) -> CheckState {
        CheckState {
            subagent_id: "sa_wait_test".into(),
            task_id: None,
            session_id: None,
            status,
            progress: serde_json::json!({ "subtasksDone": 0, "subtasksTotal": 1 }),
            result: None,
            error: None,
        }
    }

    #[test]
    fn poll_step_ladder_is_monotonic_and_capped() {
        // 起手就稀疏（终态走事件唤醒，阶梯只兜漏通知）：每次读都要开一次库
        // 跑全量 DDL，越密只是越频繁地开库
        let steps: Vec<u64> = (0..12).map(next_poll_step).collect();
        assert_eq!(steps[0], 1_500, "首档 1500ms");
        assert_eq!(steps[9], 600_000, "阶梯末档 10 分钟");
        assert_eq!(steps[11], 600_000, "出阶梯后封顶不增长");
        for w in steps.windows(2) {
            assert!(w[0] <= w[1], "步长不递减：{steps:?}");
        }
        // 上限与末档同档：预算给满时兜底阶梯不会越过预算本身
        assert_eq!(
            next_poll_step(CHECK_POLL_LADDER_MS.len() - 1),
            CHECK_WAIT_MS_CAP
        );
    }

    #[tokio::test]
    async fn wait_returns_at_once_when_already_terminal() {
        let (state, exit) = wait_until_terminal(
            || async { Ok::<_, CommandError>(fake_state(SubagentStatus::Succeeded)) },
            CHECK_WAIT_MS_CAP,
            None,
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Terminal);
        assert_eq!(state.status, SubagentStatus::Succeeded);
    }

    #[tokio::test]
    async fn wait_wakes_on_finish_event_instead_of_burning_budget() {
        // 终态事件必须立刻唤醒等待方：预算给满 30s，若退化成纯兜底轮询，
        // 这个用例会白等一个封顶步长（3s）才返回。
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done2 = done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            done2.store(true, std::sync::atomic::Ordering::SeqCst);
            finish_notify().notify_waiters();
        });
        let started = std::time::Instant::now();
        let (state, exit) = wait_until_terminal(
            || async {
                let terminal = done.load(std::sync::atomic::Ordering::SeqCst);
                Ok::<_, CommandError>(fake_state(if terminal {
                    SubagentStatus::Succeeded
                } else {
                    SubagentStatus::Running
                }))
            },
            CHECK_WAIT_MS_CAP,
            None,
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Terminal);
        assert_eq!(state.status, SubagentStatus::Succeeded);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "没被终态事件唤醒，等了 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_returns_early_when_user_stops() {
        // 预算上限已到 30s，而 /stop 只在轮次边界检查——等待循环不自己查令牌，
        // 停止键就会锁死整个等待时长。
        let stop = crate::bot_slash::StopToken::new_for_test();
        stop.stop();
        let started = std::time::Instant::now();
        let (_, exit) = wait_until_terminal(
            || async { Ok::<_, CommandError>(fake_state(SubagentStatus::Running)) },
            CHECK_WAIT_MS_CAP,
            Some(&stop),
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Stopped);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "/stop 没能打断等待，等了 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn long_budget_does_not_reopen_db_on_every_step() {
        // 阶梯的意义就在这：每次读都要开一次库跑全量 DDL。旧的固定 150ms
        // 步长下 1.5s 预算要读 11 次，递进后只读 2 次（起手就 1500ms）。
        //
        // 预算取 1500ms（正好一个首档）而不是更大：阶梯末档已是分钟级，
        // 预算再大这个用例就得跑好几分钟，而 2 次 vs 11 次已足够判别。
        // 合并了原先「超时也带回当前状态」那条：断的仍是同一行预算判定。
        let reads = std::cell::Cell::new(0usize);
        let started = std::time::Instant::now();
        let (state, exit) = wait_until_terminal(
            || {
                reads.set(reads.get() + 1);
                async { Ok::<_, CommandError>(fake_state(SubagentStatus::Running)) }
            },
            1_500,
            None,
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Timeout);
        assert_eq!(
            state.status,
            SubagentStatus::Running,
            "超时也要带回当前状态"
        );
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(1_500),
            "预算没走满就返回了：{:?}",
            started.elapsed()
        );
        // 上界留 3 而不是 2：第二次读若因计时抖动落在 deadline 前几百毫秒，会
        // 多走一档 3000ms——那是无害的，判别力仍远强于固定步长的 11 次。
        // finish_notify 是进程级信号，别的子 agent 收尾也会唤醒本等待方。
        assert!(
            reads.get() <= 3,
            "1.5s 预算读了 {} 次，阶梯没生效",
            reads.get()
        );
    }

    // 并发闸（FIFO/上限/释放）

    /// 并发闸状态机（合并为单测：static GATE 进程级共享，cargo 并行跑两个测试
    /// 会互相踩计数；手动构造票据，不调 wait_slot——它会与其他并行测试竞争
    /// 全局槽导致排队挂死）。
    #[test]
    fn gate_state_machine_acquire_release_and_queue_position() {
        let before_running = gate().state.lock().unwrap().global_running;
        // ① 拿槽 → 计数 +1、per_session 登记、未排队
        {
            let t = Some(GateTicket {
                parent_session: Some("ps_drop".into()),
            });
            // 手动占槽（wait_slot 锁内路径的等价操作）
            {
                let mut st = gate().state.lock().unwrap();
                st.global_running += 1;
                *st.per_session.entry("ps_drop".into()).or_insert(0) += 1;
            }
            {
                let st = gate().state.lock().unwrap();
                assert_eq!(st.global_running, before_running + 1, "拿槽 +1");
                assert_eq!(st.per_session.get("ps_drop"), Some(&1));
            }
            // queue_position 也拿 gate 锁（非递归 mutex），必须在持锁块外调用，
            // 否则同线程二次加锁自死锁
            assert_eq!(SubagentGate::queue_position(Some("ps_drop")), None);
            // ② Drop → 释放：计数回基线、归零清键
            drop(t);
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running, "Drop 必须释放槽位");
            assert!(!st.per_session.contains_key("ps_drop"), "计数归零须清键");
        }
        // ③ 多持多放对称：两张票据只放一张 → 残 1；全放 → 清键
        {
            let t1 = Some(GateTicket {
                parent_session: Some("ps_rel".into()),
            });
            let t2 = GateTicket {
                parent_session: Some("ps_rel".into()),
            };
            {
                let mut st = gate().state.lock().unwrap();
                st.global_running += 2;
                *st.per_session.entry("ps_rel".into()).or_insert(0) += 2;
            }
            drop(t2);
            assert_eq!(
                gate().state.lock().unwrap().per_session.get("ps_rel"),
                Some(&1),
                "放一张残 1"
            );
            drop(t1);
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running, "全放回基线");
            assert!(!st.per_session.contains_key("ps_rel"), "归零清键");
        }
        // ④ queue_position：同会话等待计数语义——前面还有 waiting-1 个
        {
            let mut st = gate().state.lock().unwrap();
            *st.waiting.entry("ps_q1".into()).or_insert(0) += 2;
            *st.waiting.entry("ps_q2".into()).or_insert(0) += 1;
        }
        assert_eq!(SubagentGate::queue_position(Some("ps_q1")), Some(1));
        assert_eq!(SubagentGate::queue_position(Some("ps_q2")), Some(0));
        assert_eq!(SubagentGate::queue_position(Some("ps_not_waiting")), None);
        {
            let mut st = gate().state.lock().unwrap();
            st.waiting.clear();
        }
        assert_eq!(SubagentGate::queue_position(Some("ps_q1")), None, "清场");
    }

    /// U20D 批 5：子 agent 会话的身份由 bot_sessions.is_subagent 结构化字段
    /// 承载，路由契约从「标题 🧩 前缀」升级为该字段。此处锁定：
    /// ① runner setup 必须经 subagent 变体建会话（漂移即测试红）；
    /// ② 标题已去 emoji 前缀（路由不再依赖标题）。
    ///
    /// 这一列用于「会话列表不列子 agent 会话」与库内判定；活会话的判定走
    /// tool_guard 注册表，前端拿到的 Session.isSubagent 恒为 false。
    #[test]
    fn subagent_sessions_flagged_structurally_not_by_title() {
        // concat! 拼接防本测试自匹配（dead_command_tests 同款手法）
        let wiring = concat!(
            "bot_session_create_",
            "subagent_inner(&tx, Some(title.clone()))"
        );
        assert!(
            orch_prod_src().contains(wiring),
            "runner 建会话必须走 subagent 变体（is_subagent=1 结构化标记）"
        );
        assert!(
            !orch_prod_src().contains(concat!("🧩 子", "任务：")),
            "标题已去 emoji 前缀（路由不依赖标题；子 agent 身份由 is_subagent 字段承载）"
        );
    }

    // ── 父侧装配（设计 §3.7 L2 落点）──

    fn digest_row(id: &str, status: SubagentStatus, result: Option<&str>) -> SubagentRow {
        SubagentRow {
            id: id.into(),
            status,
            profile: SubagentProfile::Coder,
            model: None,
            parent_session_id: Some("main-sess".into()),
            session_id: Some(format!("sub-{id}")),
            task_id: None,
            objective: format!("目标-{id}"),
            trace_id: format!("trc-{id}"),
            acceptance_json: None,
            context_summary: None,
            budget_json: None,
            result_json: result.map(str::to_string),
            error: (status == SubagentStatus::Failed).then(|| "LLM 5xx".to_string()),
            created_at: 1_000,
            started_at: Some(1_001),
            finished_at: Some(1_002),
            prompt_tokens: None,
            completion_tokens: None,
            cache_read_tokens: None,
        }
    }

    const OK_RESULT: &str = r#"{"status":"succeeded","summary":"覆盖 5 个竞品，含官网 URL","artifacts":[{"path":"report.md","description":"竞品对比"},{"path":"data.csv"}]}"#;

    #[test]
    fn digest_reports_terminal_rows_and_only_absorbs_those() {
        let rows = vec![
            digest_row("sa_ok", SubagentStatus::Succeeded, Some(OK_RESULT)),
            digest_row("sa_running", SubagentStatus::Running, None),
            digest_row("sa_fail", SubagentStatus::Failed, None),
        ];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new())
            .expect("有未汇报的子任务就该出段");
        assert!(d.text.contains("覆盖 5 个竞品"), "摘要要进段：{}", d.text);
        assert!(
            d.text.contains("report.md（竞品对比）"),
            "产物路径与说明都要进段：{}",
            d.text
        );
        assert!(
            d.text.contains("→ 失败：LLM 5xx"),
            "失败必须回报（否则父 agent 会一直等）：{}",
            d.text
        );
        assert!(
            d.text.contains("1 个子任务仍在排队或执行中"),
            "在途要报计数：{}",
            d.text
        );
        assert!(
            !d.text.contains("目标-sa_running"),
            "在途不逐条铺开：{}",
            d.text
        );
        // 只有终态被吸收：在途行下轮还要看它完没完
        assert_eq!(d.absorb, vec!["sa_ok".to_string(), "sa_fail".to_string()]);
    }

    #[test]
    fn digest_is_idempotent_across_turns() {
        let rows = vec![digest_row(
            "sa_ok",
            SubagentStatus::Succeeded,
            Some(OK_RESULT),
        )];
        let first = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        let mut set = std::collections::HashSet::new();
        for id in &first.absorb {
            set.insert(id.clone());
        }
        assert!(
            build_subagent_digest(&rows, &set).is_none(),
            "同一份结果第二轮不得再念一遍"
        );
        assert!(
            build_subagent_digest(&[], &set).is_none(),
            "没有子任务时不该出段"
        );
    }

    #[test]
    fn digest_never_injects_child_conversation_history() {
        // §3.7 要点 3：父 payload 只注 result_json，不注子会话历史。
        // 段里出现「system」「assistant」这类角色标记或子会话 id 即为破线。
        let rows = vec![digest_row(
            "sa_ok",
            SubagentStatus::Succeeded,
            Some(OK_RESULT),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert!(
            !d.text.contains("sub-sa_ok"),
            "子会话 id 不该出现：{}",
            d.text
        );
        assert!(
            !d.text.contains("\"role\""),
            "不得把子会话消息行当 payload 注入：{}",
            d.text
        );
    }

    #[test]
    fn digest_caps_items_and_summary_length() {
        // 一次注入不能把本轮对话挤掉：条数与摘要长度都要封顶。
        let long = "长".repeat(DIGEST_MAX_SUMMARY_CHARS * 3);
        let result = format!(
            r#"{{"summary":"{long}","artifacts":[{{"path":"a.md"}},{{"path":"b.md"}},{{"path":"c.md"}},{{"path":"d.md"}},{{"path":"e.md"}},{{"path":"f.md"}}]}}"#
        );
        let rows: Vec<SubagentRow> = (0..DIGEST_MAX_ITEMS + 5)
            .map(|i| {
                digest_row(
                    &format!("sa_{i}"),
                    SubagentStatus::Succeeded,
                    Some(result.as_str()),
                )
            })
            .collect();
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert_eq!(
            d.absorb.len(),
            DIGEST_MAX_ITEMS,
            "超出上限的行不注入（也不吸收，否则它永远进不了下一轮）"
        );
        let summary_line = d
            .text
            .lines()
            .find(|l| l.trim_start().starts_with("摘要："))
            .expect("有摘要行");
        assert!(
            summary_line.chars().count() <= DIGEST_MAX_SUMMARY_CHARS + 8,
            "摘要必须被截断：{} 字",
            summary_line.chars().count()
        );
        let artifacts_line = d
            .text
            .lines()
            .find(|l| l.trim_start().starts_with("产物："))
            .expect("有产物行");
        assert!(
            !artifacts_line.contains("f.md"),
            "产物条数封顶：{artifacts_line}"
        );
    }

    /// 失败/超预算行必须摆出**残留产出**。
    ///
    /// 父 agent 此前只看到「失败」二字，判断不了「该不该带残留重试」——
    /// 于是从零重烧一遍。跑了几十分钟查到的东西就在 result_json.summary 里，
    /// 不摆出来等于没有。
    #[test]
    fn digest_shows_residual_output_for_failed_rows() {
        let rows = vec![digest_row(
            "sa_failed",
            SubagentStatus::BudgetExceeded,
            Some(r#"{"summary":"已核到 3 家供应商报价单，还差比价","note":"墙钟熔断"}"#),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert!(
            d.text.contains("残留产出："),
            "失败行必须带残留产出：{}",
            d.text
        );
        assert!(
            d.text.contains("3 家供应商"),
            "残留产出的内容要能看到：{}",
            d.text
        );
    }

    /// 成功行的**机检未过项**必须摆出来。
    ///
    /// 「成功」与「机检没过」两件事同时成立时，父 agent 只看到「成功」就会把
    /// 没过的项一起收下。只认服务端核验过的（machine=true 且 met=false）——
    /// 模型自报的 met:false 是自我评价，混进来会让父 agent 分不清。
    #[test]
    fn digest_shows_machine_unmet_criteria() {
        let result = r#"{"status":"succeeded","acceptance_check":[
            {"criterion":"file_exists:调研.md","met":true,"machine":true},
            {"criterion":"file_nonempty:数据.xlsx","met":false,"machine":true},
            {"criterion":"内容符合用户口径","met":false}
        ]}"#;
        let rows = vec![digest_row(
            "sa_machine",
            SubagentStatus::Succeeded,
            Some(result),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert!(
            d.text.contains("机检未过"),
            "成功行必须带机检未过项：{}",
            d.text
        );
        assert!(
            d.text.contains("file_nonempty:数据.xlsx"),
            "未过项要指名道姓：{}",
            d.text
        );
        assert!(
            !d.text.contains("内容符合用户口径"),
            "模型自报的未过项不摆（服务端没核验过）：{}",
            d.text
        );
    }

    #[test]
    fn digest_survives_malformed_result_json() {
        // 收尾 JSON 解析失败当「没有结果」：宁可少报一条，也不把半截 JSON
        // 念进提示词。行仍要被吸收——否则它每轮都重来一次。
        let rows = vec![digest_row(
            "sa_bad",
            SubagentStatus::Succeeded,
            Some("{半截"),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new())
            .expect("行本身仍要出段");
        assert!(!d.text.contains("半截"), "坏 JSON 不得进段：{}", d.text);
        assert!(d.text.contains("→ 成功"));
        assert_eq!(d.absorb, vec!["sa_bad".to_string()]);
    }

    #[test]
    fn malformed_result_json_is_not_wrapped_and_carried_on() {
        // 直接钉函数契约：解析失败返回 None。
        // 只在渲染输出上断言「文本里没有坏 JSON」是不够的——把失败包成
        // `{"raw": ...}` 继续往下走的写法同样能让输出里搜不到坏串，
        // 那样原始文本就还在数据流里（且总有一天会换个字段被渲染出来）。
        assert!(
            parse_result_json("{半截").is_none(),
            "解析失败必须返回 None，不得包装后继续"
        );
        assert!(
            parse_result_json(OK_RESULT).is_some(),
            "合法收尾 JSON 要能解析出来"
        );
        assert!(
            parse_result_json("[]").is_some(),
            "合法但结构不是对象也算解析成功（取字段时自然落空，不该当语法错）"
        );
    }

    /// 接线守卫：回报段的注入点必须在消息组装的**末尾**（历史之后）。
    ///
    /// 放在摘要/记忆块那段之前，模型看到的是「先回报后用户问题」，权重结构
    /// 反了；放在更早还会被后续逻辑改写。匹配串 `concat!` 拆分防自匹配。
    #[test]
    fn digest_is_injected_at_the_end_of_message_assembly() {
        let chat = include_str!("bot_chat.rs");
        let at = chat
            .find(concat!(
                "crate::bot_orchestrator::",
                "subagent_digest_block(&app, session_id.as_deref())"
            ))
            .expect("组装末尾必须调 subagent_digest_block");
        let history_end = chat
            .find("msgs.push(serde_json::json!({\"role\": role, \"content\": m.content}))")
            .expect("历史消息循环的收尾 push");
        // 模型循环锚点必须在注入点**之后**找：bot_chat.rs 前面还有若干处提到
        // run_model_loop 的注释/别名，全文件搜第一个会命中它们。
        let runs_loop = chat[at..]
            .find("run_model_loop(")
            .map(|i| i + at)
            .expect("注入点之后应有模型循环调用");
        assert!(
            history_end < at && at < runs_loop,
            "回报段必须夹在历史消息与模型循环之间（现 history_end={history_end} \
             digest={at} loop={runs_loop}）"
        );
        assert!(
            chat[at..].contains("\"role\": \"system\""),
            "回报段作为 system 消息追加"
        );
    }

    /// `temperature` 是 yaml 里长期没有消费者的字段（已解析、已落盘，就是没人用）。
    /// 这条把它钉成「真的进了模型循环」：优先级是配置档 > 模型条目。
    #[test]
    fn onion_temperature_reaches_the_model_loop() {
        let body = orch_prod_src();
        let runner = body
            .split_once("let first = crate::bot_model_loop::run_model_loop(")
            .expect("子 agent 调模型循环")
            .1
            .split(";")
            .next()
            .expect("调用应在此处收尾");
        // 两个 token 分开断言而不是拼成 `bound_onion.as_ref()`：rustfmt 会按行宽
        // 把链式调用拆行，拼出来的串匹配不到——守卫被格式器牵着走。
        assert!(
            runner.contains("bound_onion"),
            "子 agent 必须把配置档的温度传进模型循环：{runner}"
        );
        assert!(
            runner.contains("c.temperature"),
            "传的是配置档的 temperature 字段：{runner}"
        );
    }

    /// tool_calls 预算计数器（ctx 侧）——dispatch 累计语义的单测锚点
    #[test]
    fn tool_call_counter_accumulates() {
        let ctx = crate::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_c1".into(),
            task_id: None,
            profile: SubagentProfile::Coder,
            tools: std::sync::Arc::new(
                crate::bot::registry::profile_whitelist(SubagentProfile::Coder).to_vec(),
            ),
            budget: SubagentBudget {
                max_turns: 5,
                max_tool_calls: 2,
                max_wall_seconds: 60,
            },
            artifact_dir: std::path::PathBuf::from("/tmp/art_c1"),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
        };
        assert_eq!(
            ctx.used_tool_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            ctx.used_tool_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            1
        );
        // used(2) >= max(2) → dispatch 层下一次调用将拒绝
        assert!(2 >= ctx.budget.max_tool_calls as usize);
    }

    // 收尾解析 / 包装渲染

    #[test]
    fn extract_last_json_takes_the_last_object() {
        // 纯 JSON
        assert_eq!(
            json_object_spans(r#"{"a":1}"#),
            vec![r#"{"a":1}"#.to_string()],
            "纯 JSON 应切出一个顶层对象"
        );
        // 散文夹 JSON + 多对象 → 取最后一个
        let t = r#"结论如下 {"status":"succeeded","summary":"第一步"} 中间 {"status":"succeeded","summary":"最终"}"#;
        let got = parse_final_json(t).expect("多对象时取最后一个");
        assert_eq!(got["summary"], "最终");
        // 字符串内花括号不破坏扫描
        let t2 = r#"{"summary":"带 } 花括号与 \" 引号"}"#;
        assert!(parse_final_json(t2).is_some());
        // 无 JSON
        assert!(parse_final_json("没有任何大括号的普通回复").is_none());
    }

    #[test]
    fn classify_outcome_maps_all_paths() {
        use SubagentStatus::*;
        // 墙钟 → budget_exceeded
        let (st, err, _) = classify_outcome(None, true, None);
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_wall_seconds"));
        // 轮数熔断 → budget_exceeded（标记在正文里，出口见 bot_model_loop 熔断）
        let fused = crate::bot_model_loop::rounds_fuse_message("上轮正文", "", 20);
        let (st, err, _) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_turns"));
        // 其他错误 → failed
        let (st, _, _) = classify_outcome(Some("HTTP 500"), false, None);
        assert_eq!(st, Failed);
        // 解析成功 + 模型自报 succeeded
        let (st, err, res) = classify_outcome(
            None,
            false,
            Some(r#"结论 {"status":"succeeded","summary":"ok"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        assert_eq!(res.unwrap()["summary"], "ok");
        // 模型自报 failed → failed + 说明
        let (st, err, _) =
            classify_outcome(None, false, Some(r#"{"status":"failed","summary":"x"}"#));
        assert_eq!(st, Failed);
        assert_eq!(err.as_deref(), Some("model_reported_failure"));
        // 解析失败 → failed + result_json_unparseable + 原文存 summary（部分结果不丢）
        let (st, err, res) = classify_outcome(None, false, Some("纯文本收尾，没有 JSON"));
        assert_eq!(st, Failed);
        assert_eq!(err.as_deref(), Some("result_json_unparseable"));
        let res = res.unwrap();
        assert!(res["summary"].as_str().unwrap().contains("纯文本收尾"));
    }

    /// 触顶改判的四象限：交了卷的触顶是真实完成（收尾窗放行过写产物），
    /// 只有「触顶 + 没交卷」才强制 budget_exceeded。
    #[test]
    fn budget_force_needed_only_when_not_wrapped_up() {
        use SubagentStatus::*;
        // 触顶 + 交卷（succeeded / 自报 failed 都是交卷）→ 不改判
        assert!(!budget_force_needed(100, 100, Succeeded, true));
        assert!(!budget_force_needed(100, 100, Failed, true));
        // 触顶 + 没交卷 → 改判（真实原因是预算）
        assert!(budget_force_needed(100, 100, Succeeded, false));
        assert!(budget_force_needed(100, 100, Failed, false));
        // 未触顶 / 已是 budget_exceeded（熔断标记路径）→ 不改判
        assert!(!budget_force_needed(99, 100, Failed, false));
        assert!(!budget_force_needed(100, 100, BudgetExceeded, false));
    }

    /// read_own_card 的返回体：budgetUsage 一块含全部实时余量，静态 budget
    /// 键不再单独出现（两份会让模型不知道看哪个）。
    #[test]
    fn own_card_payload_reports_live_budget_usage() {
        let row = crate::db::subagents::SubagentRow {
            id: "sa_x".into(),
            status: SubagentStatus::Running,
            profile: SubagentProfile::Research,
            model: None,
            parent_session_id: None,
            session_id: None,
            task_id: None,
            objective: "调研 X".into(),
            trace_id: "trc_x".into(),
            acceptance_json: Some(r#"["覆盖 X"]"#.into()),
            context_summary: None,
            budget_json: None,
            result_json: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
            prompt_tokens: None,
            completion_tokens: None,
            cache_read_tokens: None,
        };
        let v = own_card_payload(
            &row,
            vec!["覆盖 X".into()],
            Some(SubagentBudget {
                max_turns: 50,
                max_tool_calls: 100,
                max_wall_seconds: 3600,
            }),
            47,
            12,
            1800,
        );
        assert_eq!(v["budgetUsage"]["toolCallsUsed"], 47);
        assert_eq!(v["budgetUsage"]["toolCallsMax"], 100);
        assert_eq!(v["budgetUsage"]["roundsUsed"], 12);
        assert_eq!(v["budgetUsage"]["roundsMax"], 50);
        assert_eq!(v["budgetUsage"]["wallRemainingSecs"], 1800);
        assert!(
            v.get("budget").is_none(),
            "静态 budget 键已并入 budgetUsage，不得再单独出现"
        );
        // 预算缺失（老行）时 max 为 null，used 仍要报——余量读数不依赖预算存在
        let v2 = own_card_payload(&row, vec![], None, 3, 1, 60);
        assert_eq!(v2["budgetUsage"]["toolCallsUsed"], 3);
        assert!(v2["budgetUsage"]["toolCallsMax"].is_null());
    }

    /// 轮数熔断必须保住已跑完的那十几轮产出。
    ///
    /// 回归背景：recon 子任务 20 轮用尽、28 次工具调用查完后熔断，终态
    /// budget_exceeded/max_turns 而 result_json 是 NULL——产出整段蒸发，父 agent
    /// 回灌拿不到 summary，重试只能从零再来（设计 §6「部分结果不丢弃」）。
    #[test]
    fn classify_outcome_keeps_partial_output_on_rounds_fuse() {
        use SubagentStatus::*;
        let partial = "查到 pp-ocr-v6/ 下有 det.onnx / cls.onnx / rec.onnx + keys.txt";
        let fused = crate::bot_model_loop::rounds_fuse_message(partial, "", 20);
        let (st, err, res) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_turns"));
        let res = res.expect("轮数熔断不得写 NULL result_json");
        let summary = res["summary"].as_str().expect("熔断行必须带 summary");
        assert!(
            summary.contains("det.onnx"),
            "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
        );
    }

    /// 工具熔断必须落 budget_exceeded(max_tool_calls)。
    ///
    /// 此前 classify_outcome 只认轮数熔断标记，工具熔断（另一个常量）落不进
    /// 预算分支：模型照办输出 {"status":"succeeded"} 就报成功，父 agent 因此
    /// 不重试——预算烧满、产出残缺，却记成一次干净的成功。
    #[test]
    fn classify_outcome_treats_tool_fuse_as_budget_exceeded() {
        use SubagentStatus::*;
        let partial = "查到 3 个候选包，还差最后一步没跑";
        let fused = crate::bot_model_loop::fuse_message(partial, "", 100);
        let (st, err, res) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded, "工具熔断必须落预算触达");
        assert_eq!(err.as_deref(), Some("max_tool_calls"));
        let res = res.expect("工具熔断不得写 NULL result_json");
        let summary = res["summary"].as_str().expect("熔断行必须带 summary");
        assert!(
            summary.contains("3 个候选包"),
            "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
        );

        // 轮数熔断仍归 max_turns（轮数标记是工具标记的前缀，判据不能反）
        let rounds = crate::bot_model_loop::rounds_fuse_message(partial, "", 20);
        let (st, err, _) = classify_outcome(None, false, Some(&rounds));
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_turns"));
    }

    /// 墙钟熔断必须落 budget_exceeded(max_wall_seconds) **并保留正文**。
    ///
    /// 两个坑：
    /// 1. 墙钟标记「⏹ 已熔断（墙钟上限）」**包含**通用熔断标记「⏹ 已熔断」——
    ///    判据顺序反了会被认成工具调用熔断，真实原因「墙钟触顶」又被丢掉。
    /// 2. 与轮数熔断同款留产：跑了几十分钟的部分产出不能归零。
    #[test]
    fn classify_outcome_treats_wall_fuse_as_budget_exceeded_and_keeps_output() {
        use SubagentStatus::*;
        let partial = "已核到 3 家供应商的报价单，还差最后一步比价";
        let fused = crate::bot_model_loop::wall_fuse_message(partial, "", 600);
        let (st, err, res) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded, "墙钟熔断必须落预算触达");
        assert_eq!(
            err.as_deref(),
            Some("max_wall_seconds"),
            "墙钟熔断不得被认成工具调用熔断（墙钟标记包含通用标记）"
        );
        let res = res.expect("墙钟熔断不得写 NULL result_json");
        let summary = res["summary"].as_str().expect("熔断行必须带 summary");
        assert!(
            summary.contains("3 家供应商"),
            "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
        );
    }

    /// 收尾 JSON 的本地修复（结构化输出三段修复的第 2 段）：零 token、零延迟，
    /// 必须排在补答之前——补答要再走一次 LLM 轮次。
    #[test]
    fn parse_final_json_repairs_trailing_comma_and_truncation() {
        // 尾逗号
        let v = parse_final_json(r#"{"status":"succeeded","summary":"好了",}"#)
            .expect("尾逗号应被本地修复救回");
        assert_eq!(v["status"], "succeeded");
        // 被截断（写到一半没了）
        let v = parse_final_json(r#"{"status":"succeeded","summary":"调研完成，报告"#)
            .expect("截断的 JSON 应被补闭合救回");
        assert_eq!(v["status"], "succeeded");
        // markdown 围栏（既有能力，顺带锁住）
        let v = parse_final_json("```json\n{\"status\":\"failed\"}\n```").expect("围栏应能取到");
        assert_eq!(v["status"], "failed");
    }

    /// 本地修复不得改坏字符串内容：模型写的正文里出现逗号/花括号是合法的。
    #[test]
    fn parse_final_json_does_not_corrupt_string_content() {
        let raw = r#"{"status":"succeeded","summary":"结论：a, b, c"}"#;
        let v = parse_final_json(raw).expect("正常 JSON 必须能解析");
        assert_eq!(v["summary"], "结论：a, b, c");
        // 字符串里的尾逗号形态不能被当成尾逗号删掉
        let raw = r#"{"status":"succeeded","summary":"结尾带逗号,"}"#;
        let v = parse_final_json(raw).expect("字符串里的逗号不得被改");
        assert_eq!(v["summary"], "结尾带逗号,");
    }

    /// 散文收尾仍然解析不出来——修复不是万能的，解析失败要如实落失败。
    #[test]
    fn parse_final_json_leaves_prose_unparseable() {
        assert!(parse_final_json("纯文本收尾，没有 JSON").is_none());
        assert!(parse_final_json("").is_none());
    }

    /// 截断的 JSON 修出来若**认不出收尾语义**（没有 status / checks），
    /// 不得认成收尾结果——判定里「没有 status」默认按 succeeded 算，
    /// 把「没说完」说成「说完了」。
    #[test]
    fn parse_final_json_rejects_truncated_fragment_without_status() {
        assert!(
            parse_final_json(r#"{"summary":"写到一半断了"#).is_none(),
            "半截 JSON 没有 status，不得被认成收尾结果"
        );
        // 有 status 的截断件认得出来（模型已经表明了结论）
        assert!(
            parse_final_json(r#"{"status":"succeeded","summary":"写到一半断了"#).is_some(),
            "已有 status 的截断件应被救回"
        );
    }

    #[test]
    fn machine_pass_overrides_unparseable_finish() {
        let verdicts = vec![
            MachineVerdict {
                criterion: "file_exists:调研.md".into(),
                met: true,
                reason: "服务端核验：存在（13780 字节）".into(),
            },
            MachineVerdict {
                criterion: "file_nonempty:调研.md".into(),
                met: true,
                reason: "服务端核验：存在（13780 字节）".into(),
            },
        ];
        let (st, err, res) = classify_outcome(None, false, Some("散文收尾，没写 JSON"));
        assert_eq!(st, SubagentStatus::Failed, "起点是解析失败");
        assert!(res
            .expect("原文必须留在 result_json")
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("散文收尾"));

        let (st2, err2, recovered) = reconcile_machine_override(st, err, &verdicts);
        assert_eq!(st2, SubagentStatus::Succeeded);
        assert!(recovered);
        // 推翻这件事本身要留痕：error 上保留解析失败标记并补出理由
        let e = err2.expect("推翻后仍须留标记");
        assert!(e.starts_with(UNPARSEABLE_MARKER), "{e}");
        assert!(e.contains("2 项"), "{e}");
    }

    /// 边界：机器全过只推翻「解析失败」这一种。模型自报失败说的可能是服务端
    /// 验不了的自由文本条目；一条机器结论都没有时服务端零证据；验了没过更是
    /// 实打实的不合格——这三种都必须维持失败。
    #[test]
    fn machine_pass_overrides_nothing_else() {
        let all_met = vec![MachineVerdict {
            criterion: "file_exists:a.md".into(),
            met: true,
            reason: "存在".into(),
        }];
        for (st, err) in [
            (
                SubagentStatus::Failed,
                Some("model_reported_failure".to_string()),
            ),
            (SubagentStatus::Failed, Some("HTTP 500".to_string())),
            (
                SubagentStatus::BudgetExceeded,
                Some("max_turns".to_string()),
            ),
        ] {
            let (s2, e2, recovered) = reconcile_machine_override(st, err.clone(), &all_met);
            assert_eq!(s2, st);
            assert_eq!(e2, err);
            assert!(!recovered);
        }
        // 零证据（一条机器标准都没声明）→ 维持失败
        let (s2, _, r2) = reconcile_machine_override(
            SubagentStatus::Failed,
            Some(UNPARSEABLE_MARKER.into()),
            &[],
        );
        assert_eq!(s2, SubagentStatus::Failed);
        assert!(!r2);
        // 验了没过 → 维持失败
        let unmet = vec![MachineVerdict {
            criterion: "file_exists:a.md".into(),
            met: false,
            reason: "产物不存在".into(),
        }];
        let (s3, _, r3) = reconcile_machine_override(
            SubagentStatus::Failed,
            Some(UNPARSEABLE_MARKER.into()),
            &unmet,
        );
        assert_eq!(s3, SubagentStatus::Failed);
        assert!(!r3);
    }

    /// 接线守卫：机器验收的修正必须落在终态快照之前。判定顺序反了的话，
    /// 「机器验收通过」与「行状态 failed」就会永远同时成立——那正是本次修的 bug。
    #[test]
    fn machine_override_runs_before_status_snapshot() {
        let src = orch_prod_src();
        let fix = src
            .find("reconcile_machine_override(status, error,")
            .expect("finalize 必须调用 reconcile_machine_override");
        let snapshot = src.find("let status_for_db = status;").expect("终态快照点");
        assert!(fix < snapshot, "修正必须早于落库快照");
    }

    /// 「问过即已吸收」：`check_subagent` 拿到终态后结果已在父上下文里，
    /// digest 与自动汇报轮都不该再讲一遍。去重表按会话隔离，别把 A 会话的
    /// 结果算到 B 会话头上。
    #[test]
    fn digest_mark_is_per_session() {
        let sid = format!("dm-{}", uuid::Uuid::new_v4());
        assert!(!is_digested(&sid, "sa_a"));
        mark_digested(Some(&sid), "sa_a");
        assert!(is_digested(&sid, "sa_a"));
        assert!(
            !is_digested(&format!("{sid}-other"), "sa_a"),
            "去重表按会话隔离"
        );
        mark_digested(None, "sa_b");
        assert!(!is_digested(&sid, "sa_b"), "无会话 id 不得污染任何会话");

        // 回报段的吸收是两步：先构造（不碰已吸收集合），模型循环成功后才提交。
        // 构造即提交是「digest 失败即丢」的根因——循环失败时这段回报压根没送进
        // 模型，却已被记成已吸收，汇报轮从此不再重提这条子任务结果。
        let sid2 = format!("dmc-{}", uuid::Uuid::new_v4());
        let rows = vec![digest_row(
            "sa_x",
            SubagentStatus::Succeeded,
            Some(OK_RESULT),
        )];
        let built = build_subagent_digest(&rows, &std::collections::HashSet::new())
            .expect("有未汇报的终态子任务就该出段");
        assert_eq!(built.absorb, vec!["sa_x".to_string()]);
        assert!(
            !is_digested(&sid2, "sa_x"),
            "构造回报段不构成吸收——模型循环还没成功"
        );
        for id in &built.absorb {
            mark_digested(Some(&sid2), id);
        }
        assert!(is_digested(&sid2, "sa_x"), "提交后才算已吸收");
    }

    /// 接线守卫：汇报轮失败不能直接放弃——那等于这条子任务结果永远没人汇报。
    ///
    /// 失败与 Busy 同款处理：deadline 内 sleep 后再试（digest 此时也没记成已
    /// 吸收，下一轮还有机会把结果说出去）。
    #[test]
    fn report_failure_is_retried_instead_of_given_up() {
        let src = orch_prod_src();
        // 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）
        assert!(
            !src.contains(concat!(
                "ReportOutcome::Ran | crate::bot_chat::",
                "ReportOutcome::Failed"
            )),
            "汇报失败不能和成功同款直接 return"
        );
        assert!(
            src.contains(concat!(
                "ReportOutcome::Failed | crate::bot_chat::",
                "ReportOutcome::Busy"
            )),
            "汇报失败要与 Busy 同款：deadline 内 sleep 后重试"
        );
    }

    /// 接线守卫：兜底写终态失败必须留痕。
    ///
    /// 兜底写的是「行停在 queued/running」的唯一补救；它自己失败时若不记，
    /// 那行就永远停在那里，面板上一直转圈，事后也查不到是哪一步失败的。
    /// 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）。
    #[test]
    fn terminal_state_fallback_write_leaves_a_trace_on_failure() {
        let src = orch_prod_src();
        assert!(
            !src.contains(concat!("let _ = db_", "locked(")),
            "兜底写终态不得吞掉失败——行会永远停在 queued/running"
        );
        let write_needle = concat!("let write = db_", "locked(");
        let trace_needle = concat!("subagent_terminal_state_write_", "failed");
        // 按「兜底写」切段：每段到下一个兜底写为止，每段都要有自己的失败留痕
        let segments: Vec<&str> = src.split(write_needle).skip(1).collect();
        assert!(
            segments.len() >= 4,
            "兜底写终态有四处（加载失败 / 产物目录不可用 / 会话建立失败 / 收尾失败），\
             现 {len} 处",
            len = segments.len()
        );
        for seg in &segments {
            assert!(
                seg.contains(trace_needle),
                "兜底写终态失败必须留痕——行永远停在 queued/running 时只有这条线索"
            );
        }
    }

    /// 接线守卫：`subagent_digest_block` 只造段，不得顺手记「已吸收」。
    ///
    /// 它被模型循环**之前**调用——在这里记，等于把「这段回报已经说过了」提前
    /// 宣布；循环失败时它压根没送进模型，汇报轮也会因此以为说过、从此不再
    /// 重提，子任务结果在本次进程内永久消失。
    #[test]
    fn digest_block_builds_without_absorbing() {
        let src = orch_prod_src();
        let start = src
            .find("pub(crate) async fn subagent_digest_block(")
            .expect("要有 subagent_digest_block");
        let rest = &src[start..];
        let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(
            !body.contains("set.insert"),
            "回报段的构造函数里不得记已吸收——记动作在模型循环成功之后"
        );
        assert!(
            body.contains("build_subagent_digest("),
            "构造函数仍要调 build_subagent_digest 出段"
        );
    }

    /// 接线守卫：「已吸收」的标记不在 `bot_chat_inner` 里做。
    ///
    /// 这一层既不知道送达结果、也不该替调用方决定：交互聊天（前端驱动）在
    /// 命令返回前记（模型确实看到了那段回报），汇报轮（后端驱动）要等落库
    /// 成功才记（它的历史下一轮从库里读，没落库的回复等于没说出去）。在这里
    /// 无条件记，等于把「已经说过了」提前宣布——落库失败时那段回报从此不再
    /// 进 digest，子任务结果在本次进程内永久消失。
    #[test]
    fn digest_absorb_mark_is_left_to_the_caller() {
        let chat = include_str!("bot_chat.rs");
        let start = chat
            .find("async fn bot_chat_inner(")
            .expect("bot_chat_inner 函数体起点");
        let body = &chat[start..];
        let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
        let inner = &body[..end];
        assert!(
            !inner.contains("mark_digested("),
            "bot_chat_inner 里不得记「已吸收」——标记由知道送达结果的调用方做"
        );
    }

    /// 派发方会话被删后不该再汇报：bot_messages 没有外键约束，硬跑一轮只会给
    /// 一个不存在的会话写一条永远读不到的孤儿消息。
    #[test]
    fn missing_parent_session_blocks_the_report() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                is_subagent INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        assert!(
            !bot_session_exists_locked(&conn, "s-gone"),
            "不存在的会话判为不在"
        );
        conn.execute(
            "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
            [],
        )
        .unwrap();
        assert!(bot_session_exists_locked(&conn, "s1"));
    }

    /// 接线守卫：汇报轮的轮次说明要排在 digest 注入之前。digest 放末尾是
    /// 刻意的（模型对末尾权重最高），说明插在它前面才不抢这份权重。
    #[test]
    fn report_turn_prompt_precedes_digest_injection() {
        let src = include_str!("bot_chat.rs");
        let prompt = src
            .find("REPORT_TURN_PROMPT}))")
            .expect("汇报轮必须注入轮次说明");
        let digest = src
            .find("subagent_digest_block(&app, session_id.as_deref())")
            .expect("digest 注入点");
        assert!(prompt < digest, "说明应在 digest 之前，末尾留给回报本身");
    }

    /// verifier 输出 verified.json schema 的判定分支覆盖。
    /// checks 数组结构而非 status 字段是 verifier 路径的真正信号源。
    #[test]
    fn classify_outcome_verifier_paths() {
        use SubagentStatus::*;
        // 全部 met=true → Succeeded
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"checks":[{"criterion":"c1","met":true,"evidence":"x"}],"summary":"all ok"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        // 全部 met=false → Failed + reason 列前 3 项 criterion
        let (st, err, res) = classify_outcome(
            None,
            false,
            Some(
                r#"{"checks":[{"criterion":"导出 PDF","met":false,"evidence":"缺依赖"},
                              {"criterion":"样式对齐","met":false,"evidence":"字体不一致"}],
                   "summary":"全部失败"}"#,
            ),
        );
        assert_eq!(st, Failed);
        let err_str = err.unwrap();
        assert!(
            err_str.starts_with("verifier_all_checks_failed:"),
            "actual: {err_str}"
        );
        assert!(err_str.contains("导出 PDF=false"), "actual: {err_str}");
        assert!(err_str.contains("样式对齐=false"), "actual: {err_str}");
        assert_eq!(res.unwrap()["summary"], "全部失败");
        // 部分 met=true → Succeeded（主 agent 看 checks 自己决策）
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(
                r#"{"checks":[{"criterion":"c1","met":true,"evidence":"x"},
                              {"criterion":"c2","met":false,"evidence":"y"}],
                   "summary":"部分通过"}"#,
            ),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        // checks 空 + summary 含「未通过」 → Failed
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"checks":[],"summary":"未通过：缺关键证据"}"#),
        );
        assert_eq!(st, Failed);
        let err_str = err.unwrap();
        assert!(
            err_str.starts_with("verifier_summary_rejected:"),
            "actual: {err_str}"
        );
        assert!(err_str.contains("缺关键证据"), "actual: {err_str}");
        // checks 空 + summary 不含否决词 → Succeeded（fail-safe 放行）
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"checks":[],"summary":"没有可检查项，跳过验证"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        // status=succeeded 的普通收尾无 checks → 走现有路径不受影响
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"status":"succeeded","summary":"ok"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
    }

    #[test]
    fn task_wrapper_contains_all_sections() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.context_summary = Some("上游已确认五个竞品清单".into());
        let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        let budget = crate::db::SubagentBudget::default();
        let w = render_task_wrapper(&row, &budget, std::path::Path::new("/tmp/art"));
        assert!(w.contains("[子任务派发]"));
        assert!(w.contains(&format!("subagent_id: {}", row.id)));
        assert!(w.contains("objective: 调研五个竞品并输出报告"));
        assert!(w.contains("- 覆盖至少 5 个产品"));
        // 上下文摘要取自行上 context_summary（不再从子卡 note 文本解析）
        assert!(w.contains("上下文摘要:\n上游已确认五个竞品清单"));
        assert!(w.contains("max_turns=30, max_tool_calls=100, max_wall_seconds=600"));
        assert!(w.contains("产物目录:"));
        assert!(w.contains("/tmp/art"));
        assert!(w.contains("完成后必须输出以下 JSON"));
        assert!(w.contains("\"succeeded|failed|cancelled|budget_exceeded\""));
    }

    /// 无上下文摘要时包装不带该段（不留空标题噪声）
    #[test]
    fn task_wrapper_omits_absent_context_summary() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        let w = render_task_wrapper(&row, &SubagentBudget::default(), std::path::Path::new("/a"));
        assert!(!w.contains("上下文摘要"));
    }

    fn req(parent: Option<String>) -> SpawnRequest {
        SpawnRequest {
            objective: "调研五个竞品并输出报告".into(),
            profile: SubagentProfile::Research,
            stage: None,
            acceptance_criteria: vec!["覆盖至少 5 个产品".into(), "每条含官网 URL".into()],
            context_summary: None,
            budget: None,
            model_profile: None,
            onion: None,
            parent_task_id: parent,
            parent_session_id: Some("main-sess".into()),
        }
    }

    #[test]
    fn spawn_creates_row_only_and_no_subcard() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let parent = parent_card(&conn);
        let ack = spawn_subagent_locked(&mut conn, &req(Some(parent)), 5_000).unwrap();
        assert!(ack.subagent_id.starts_with("sa_"));
        assert_eq!(ack.status, SubagentStatus::Queued);
        // 不再建卡：行上 task_id 为 NULL，tasks 表除 parent 外无新增
        assert_eq!(ack.task_id, None);
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.task_id, None);
        assert_eq!(row.parent_session_id.as_deref(), Some("main-sess"));
        let cards = crate::db::load_all(&conn).unwrap();
        assert_eq!(cards.len(), 1, "只应有 parent 卡，子 agent 不建卡");
        assert_eq!(cards[0].id, "parent-1");
        // 验收标准与上下文摘要落在行上（机器侧，不再双写 note）
        let criteria: Vec<String> =
            serde_json::from_str(row.acceptance_json.as_deref().unwrap()).unwrap();
        assert_eq!(criteria.len(), 2);
        assert!(row.context_summary.is_none());
        // 预算上表可见（默认值）
        let budget: SubagentBudget =
            serde_json::from_str(row.budget_json.as_deref().unwrap()).unwrap();
        assert_eq!(budget.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(budget.max_tool_calls, DEFAULT_MAX_TOOL_CALLS);
        assert_eq!(budget.max_wall_seconds, DEFAULT_MAX_WALL_SECONDS);
    }

    /// 空白上下文摘要不落库（否则包装会渲染一个空标题段）
    #[test]
    fn spawn_trims_blank_context_summary_to_none() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.context_summary = Some("   \n  ".into());
        let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.context_summary, None);
    }

    #[test]
    fn spawn_without_parent_card_is_allowed() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        assert!(ack.task_id.is_none());
        assert!(!ack.trace_id.is_empty());
    }

    /// 重复派发同一份请求不建第二条活行（幂等键：父会话 + 档位 + 目标 +
    /// 验收标准 + 指定模型）。
    ///
    /// 模型重复调用 spawn_subagent（同一轮发两次、或隔几轮忘了自己派过）原先
    /// 会建两条活行，各拿一份完整预算——同一份活干两遍。
    #[test]
    fn repeated_spawn_of_the_same_request_reuses_the_live_row() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        assert!(!first.deduped, "首次派发必须是新建");
        let second = spawn_subagent_locked(&mut conn, &req(None), 6_000).unwrap();
        assert!(second.deduped, "重复派发必须命中幂等键");
        assert_eq!(
            second.subagent_id, first.subagent_id,
            "重复派发必须返回同一个子 agent id"
        );
        let rows = crate::db::load_subagents_by_parent(&conn, "main-sess", 100).unwrap();
        assert_eq!(rows.len(), 1, "重复派发不得再建一条活行");
    }

    /// 已终态的不参与去重：「再跑一次」是重试语义，不能被幂等键吞掉。
    #[test]
    fn spawn_after_the_previous_one_finished_creates_a_new_row() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        crate::db::update_subagent_status(
            &conn,
            &first.subagent_id,
            SubagentStatus::Cancelled,
            6_000,
            None,
        )
        .unwrap();
        let second = spawn_subagent_locked(&mut conn, &req(None), 7_000).unwrap();
        assert!(!second.deduped, "已终态的旧行不该再命中幂等键");
        assert_ne!(second.subagent_id, first.subagent_id);
        let rows = crate::db::load_subagents_by_parent(&conn, "main-sess", 100).unwrap();
        assert_eq!(rows.len(), 2, "重试应另起一行");
    }

    /// 验收标准不同 = 不同的活，不去重（模型改需求后重派是正当的）。
    #[test]
    fn spawn_with_different_acceptance_criteria_is_not_deduped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let mut other = req(None);
        other.acceptance_criteria = vec!["覆盖至少 8 个产品".into()];
        let second = spawn_subagent_locked(&mut conn, &other, 6_000).unwrap();
        assert!(!second.deduped, "验收标准不同不该命中幂等键");
        assert_ne!(second.subagent_id, first.subagent_id);
    }

    /// 不同父会话的同目标派发是不同意图，不互相吞掉。
    #[test]
    fn spawn_from_a_different_parent_session_is_not_deduped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let mut other = req(None);
        other.parent_session_id = Some("other-sess".into());
        let second = spawn_subagent_locked(&mut conn, &other, 6_000).unwrap();
        assert!(!second.deduped, "跨会话不命中幂等键");
        assert_ne!(second.subagent_id, first.subagent_id);
    }

    /// 没有父会话（拿不到会话 id 的调用路径）时不去重：宁可多建一行，
    /// 也不要把别人的活吞掉。
    #[test]
    fn spawn_without_parent_session_is_not_deduped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.parent_session_id = None;
        let first = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let second = spawn_subagent_locked(&mut conn, &r, 6_000).unwrap();
        assert!(!first.deduped && !second.deduped);
        assert_ne!(first.subagent_id, second.subagent_id);
    }

    /// 幂等键挡住的是「建第二条活行」，挡不住「再起一个 runner」——后者照样
    /// 把同一份活跑两遍、各烧一份预算。deduped 的早退必须排在 runner 派发前。
    #[test]
    fn deduped_spawn_returns_before_the_runner_is_dispatched() {
        let src = orch_prod_src();
        let start = src
            .find("pub async fn spawn_subagent(")
            .expect("bot_orchestrator.rs 应有 spawn_subagent");
        let dedup = start
            + src[start..]
                .find("ack.deduped")
                .expect("spawn_subagent 应有 deduped 早退分支");
        let runner = start
            + src[start..]
                .find("run_subagent(")
                .expect("spawn_subagent 应派发 runner");
        assert!(
            dedup < runner,
            "命中幂等键必须早于 runner 派发返回（否则同一份活跑两遍）"
        );
        assert!(
            src[dedup..runner].contains("return Ok(ack);"),
            "deduped 分支必须直接返回，不得落到 runner 派发"
        );
    }

    /// 回执文案两种形态：新建说「已派发」，命中幂等键要说清「没新建、
    /// 还是那个」——模型照抄「已派发」会把一个子 agent 说成两个。
    #[test]
    fn spawn_receipt_text_distinguishes_new_from_deduped() {
        let ack = SpawnAck {
            subagent_id: "sa_x".into(),
            task_id: None,
            trace_id: "trc_x".into(),
            status: SubagentStatus::Queued,
            deduped: false,
            budget: SubagentBudget::default(),
        };
        let fresh = spawn_receipt_text(&ack);
        assert!(fresh.contains("已派发子 agent"), "{fresh}");
        assert!(fresh.contains("sa_x"), "{fresh}");
        // 回执必须带生效预算——模型不知道墙钟被钳过，就会按自己给的数安排动作
        assert!(fresh.contains("生效预算"), "{fresh}");

        let deduped = SpawnAck {
            deduped: true,
            ..ack
        };
        let dup = spawn_receipt_text(&deduped);
        assert!(dup.contains("未重复派发"), "{dup}");
        assert!(dup.contains("sa_x"), "{dup}");
        assert!(
            !dup.contains("已派发子 agent"),
            "命中幂等键的回执不得说成新建：{dup}"
        );
    }

    #[test]
    fn spawn_with_missing_parent_card_is_task_not_found() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        // Result 的 OK 侧 SpawnAck 无 Debug——用 match 拆错误，不 unwrap_err
        let err = match spawn_subagent_locked(&mut conn, &req(Some("ghost-card".into())), 5_000) {
            Err(e) => e,
            Ok(_) => panic!("缺卡 spawn 必须失败"),
        };
        match err {
            CommandError::TaskNotFound(id) => assert_eq!(id, "ghost-card"),
            other => panic!("应 TaskNotFound，实际 {other:?}"),
        }
    }

    /// 机器可判定条目带杂质要在**派发期**就拒。
    ///
    /// 模型写 `file_exists:调研.md 已完成`，服务端把路径当成「调研.md 已完成」，
    /// 文件当然不存在——假阴性。此前要等子 agent 跑完十几分钟才暴露，白烧一整轮。
    #[test]
    fn spawn_rejects_machine_criterion_with_trailing_prose() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.acceptance_criteria = vec!["file_exists:调研.md 已完成".into()];
        let err = match spawn_subagent_locked(&mut conn, &r, 5_000) {
            Ok(_) => panic!("带杂质的机器可判定条目必须当场拒"),
            Err(e) => e,
        };
        let msg = err.message();
        assert!(
            msg.contains("file_exists:report.md"),
            "报错要带正例（模型不知道对的长什么样）：{msg}"
        );
        // 干净的写法照常放行
        let mut ok = req(None);
        ok.acceptance_criteria = vec![
            "file_exists:调研.md".into(),
            "file_nonempty:output/数据.xlsx".into(),
            "内容符合用户口径".into(),
        ];
        assert!(
            spawn_subagent_locked(&mut conn, &ok, 5_000).is_ok(),
            "干净的机器可判定条目不得被拒"
        );
    }

    #[test]
    fn spawn_rejects_blank_objective_and_empty_acceptance() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.objective = "  ".into();
        assert!(spawn_subagent_locked(&mut conn, &r, 5_000).is_err());
        let mut r2 = req(None);
        r2.acceptance_criteria = Vec::new();
        assert!(spawn_subagent_locked(&mut conn, &r2, 5_000).is_err());
        let mut r3 = req(None);
        r3.acceptance_criteria = vec!["  ".into()];
        assert!(spawn_subagent_locked(&mut conn, &r3, 5_000).is_err());
    }

    #[test]
    fn budget_clamps_turns_to_hard_cap() {
        let b = clamp_budget(Some(SubagentBudget {
            max_turns: 9_999,
            max_tool_calls: 0,
            max_wall_seconds: 0,
        }));
        assert_eq!(b.max_turns, MAX_TURNS_HARD_CAP);
        assert_eq!(b.max_tool_calls, 1);
        // 墙钟下界与配置键同源（30s）——「1 秒墙钟」等于白派一个子 agent
        assert_eq!(b.max_wall_seconds, 30);
        assert_eq!(
            clamp_budget(None),
            SubagentBudget {
                max_turns: 30,
                max_tool_calls: 100,
                max_wall_seconds: 600
            }
        );
    }

    /// 墙钟的硬顶必须与配置键走同一组常量——两条路两个闸时，LLM 显式给的
    /// `maxWallSeconds=999999` 会原样生效，墙钟实际由调用方说了算。
    #[test]
    fn budget_wall_clock_is_capped_at_the_same_bound_as_config() {
        let too_big = clamp_budget(Some(SubagentBudget {
            max_turns: 30,
            max_tool_calls: 100,
            max_wall_seconds: 999_999,
        }));
        assert_eq!(
            too_big.max_wall_seconds,
            crate::bot::params::SUBAGENT_MAX_WALL_SECS_MAX
        );
        // 配置键路径给同一个数，钳完必须一模一样
        let via_config = crate::bot::params::clamp_subagent_max_wall_secs(999_999);
        assert_eq!(too_big.max_wall_seconds, via_config);
    }

    #[test]
    fn cancel_sets_cancelled_and_terminal_noop() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let c = cancel_subagent_locked(&conn, &ack.subagent_id, "用户取消", 6_000).unwrap();
        assert!(c.ok);
        assert!(!c.already_terminal);
        assert_eq!(c.status, SubagentStatus::Cancelled);
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, SubagentStatus::Cancelled);
        assert_eq!(row.error.as_deref(), Some("用户取消"));
        assert_eq!(row.finished_at, Some(6_000));
        // 取消不动 tasks 表（无卡可退）
        assert!(crate::db::load_all(&conn).unwrap().is_empty());
        // 终态再取消 = no-op 幂等
        let again = cancel_subagent_locked(&conn, &ack.subagent_id, "再取消", 7_000).unwrap();
        assert!(again.already_terminal);
        assert_eq!(again.status, SubagentStatus::Cancelled);
        assert_eq!(
            crate::db::load_subagent(&conn, &ack.subagent_id)
                .unwrap()
                .unwrap()
                .finished_at,
            Some(6_000),
            "幂等重入不得改写首收尾时刻"
        );
    }

    /// 有卡的历史行仍能按 task_id 解析（子会话键之外的第二把键，降级路径不炸）
    #[test]
    fn cancel_resolves_by_task_id_on_legacy_row() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        // 模拟老库遗留：给行补一个 task_id
        conn.execute(
            "UPDATE subagents SET task_id = 'legacy-card' WHERE id = ?1",
            [&ack.subagent_id],
        )
        .unwrap();
        let c = cancel_subagent_locked(&conn, "legacy-card", "卡片停止按钮", 6_000).unwrap();
        assert_eq!(c.subagent_id, ack.subagent_id);
        assert_eq!(c.status, SubagentStatus::Cancelled);
    }

    /// 新行 task_id 为 NULL，按卡键解析落到会话键探测后仍报「不存在」而非 panic
    #[test]
    fn cancel_by_task_key_on_cardless_row_is_invalid_argument() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let err = match cancel_subagent_locked(&conn, "no-such-card", "x", 6_000) {
            Err(e) => e,
            Ok(_) => panic!("无卡行的假卡键不得解析成功"),
        };
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
        assert_eq!(ack.task_id, None);
    }

    #[test]
    fn cancel_unknown_id_is_invalid_argument() {
        let conn = test_conn();
        // OK 侧 CancelAck 无 Debug——match 拆错误
        let err = match cancel_subagent_locked(&conn, "sa_ghost", "x", 1) {
            Err(e) => e,
            Ok(_) => panic!("未知 id 必须失败"),
        };
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
    }

    #[test]
    fn cancel_beats_classified_failure_when_reporting_terminal_state() {
        // 用户取消 → StopToken 打断 → classify 只会给 Failed；上报必须用行上的
        // cancelled，否则每次取消都记一条 subagent_failed + 空原因教训
        assert_eq!(
            effective_status(Some(SubagentStatus::Cancelled), SubagentStatus::Failed),
            SubagentStatus::Cancelled
        );
        // 行没拿到（收尾闭包整体失败）才回落到观感值
        assert_eq!(
            effective_status(None, SubagentStatus::Failed),
            SubagentStatus::Failed
        );
    }

    /// 用户取消的子 agent 不给父会话起汇报轮：用户刚点过停止，
    /// 不需要模型再解释一遍。
    #[test]
    fn cancelled_subagent_does_not_report_to_parent() {
        assert!(!should_report_to_parent(SubagentStatus::Cancelled));
        for st in [
            SubagentStatus::Succeeded,
            SubagentStatus::Failed,
            SubagentStatus::BudgetExceeded,
            SubagentStatus::Running,
            SubagentStatus::Queued,
        ] {
            assert!(should_report_to_parent(st), "{st:?} 应汇报");
        }
    }

    /// 接线守卫：汇报轮的门控必须用收尾时的真实终态，不能用行快照。
    ///
    /// 行是派发时建的，收尾前 status 恒为 Queued——门控拿 `row.status` 判取消
    /// 永远判不成立，用户取消后照样起汇报轮（违反设计「用户取消不汇报」）。
    /// 单测验不到调用点传的是哪个值，故这里扫源码钉住接线。
    #[test]
    fn parent_report_gate_is_wired_to_the_effective_status() {
        let src = orch_prod_src();
        let production = src
            .split_once("#[cfg(test)]")
            .expect("bot_orchestrator.rs 应有测试模块")
            .0;
        assert!(
            production.contains(concat!(
                "spawn_parent_report(&app, &row, effective_",
                "status)"
            )),
            "汇报轮必须传收尾时的真实终态（effective_status）"
        );
        // 只扫汇报轮函数体：别处读行快照是合法的（如 read_own_card 读的是
        // 库里当前行，不是收尾前的快照）
        let digest = include_str!("bot/subagent_digest.rs");
        let start = digest
            .find("fn spawn_parent_report")
            .expect("应能找到 spawn_parent_report");
        let rest = &digest[start..];
        let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
        let func = &rest[..end];
        assert!(
            func.contains("should_report_to_parent("),
            "汇报轮门控必须走 should_report_to_parent"
        );
        assert!(
            !func.contains("row.status"),
            "汇报轮门控不得读行快照判取消（快照恒为 Queued）"
        );
    }

    /// 在跑的子 agent 必须报得出「已跑 N / 预算 M 轮」。
    ///
    /// 回归背景：轮数此前完全不落任何地方，主 agent 靠 `check_subagent` 轮询
    /// 只看得到验收进度——一个跑满预算、在原地打转的子任务和一个刚启动的子任务
    /// 在 UI 上长得一模一样，只能干等 budget_exceeded 才知道它在烧钱。
    #[test]
    fn check_reports_rounds_while_subagent_runs() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let sid = format!("sa-test-rounds-{}", ack.subagent_id);
        // 注册子会话（行上 session_id 与之对应），模拟在跑的状态
        crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

        // 守卫注销时（终态）读不到计数器 → 不报轮数，字段整体不出现
        let queued = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert!(
            queued.progress.get("roundsUsed").is_none(),
            "无在跑计数器时不得报轮数（凭空造数比不报更坏）"
        );

        let _g2 = crate::tool_guard::SubagentSessionGuard::new(
            &sid,
            crate::tool_guard::SubagentSessionCtx {
                subagent_id: ack.subagent_id.clone(),
                task_id: None,
                profile: SubagentProfile::Research,
                tools: std::sync::Arc::new(vec![]),
                budget: SubagentBudget::default(),
                artifact_dir: std::path::PathBuf::from("/tmp/art_rounds"),
                used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(14)),
                wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
            },
        );
        let running = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(running.progress["roundsUsed"], 14);
        assert_eq!(
            running.progress["roundsBudget"],
            SubagentBudget::default().max_turns,
            "预算必须与轮数同报：10/20 与 10/200 紧迫度完全不同"
        );
    }

    /// list_subagents 的行视图与 check_subagent 同口径：在途报轮数与预算，
    /// 读不到仪表（终态/未开跑）就整段留空——两条链路口径必须一致，否则
    /// 「侧栏说跑了 14 轮、check 说没有」会让人怀疑数据。
    #[test]
    fn list_view_attaches_rounds_while_subagent_runs() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let sid = format!("sa-test-view-{}", ack.subagent_id);
        crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("spawn 后应有行");
        let queued = attach_rounds(row);
        assert_eq!(
            (queued.rounds_used, queued.rounds_budget),
            (None, None),
            "无在跑仪表时不得报轮数（凭空造数比不报更坏）"
        );

        let _g2 = crate::tool_guard::SubagentSessionGuard::new(
            &sid,
            crate::tool_guard::SubagentSessionCtx {
                subagent_id: ack.subagent_id.clone(),
                task_id: None,
                profile: SubagentProfile::Research,
                tools: std::sync::Arc::new(vec![]),
                budget: SubagentBudget::default(),
                artifact_dir: std::path::PathBuf::from("/tmp/art_view"),
                used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(14)),
                wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
            },
        );
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("行仍在");
        let running = attach_rounds(row);
        assert_eq!(running.rounds_used, Some(14));
        assert_eq!(
            running.rounds_budget,
            Some(SubagentBudget::default().max_turns),
            "预算必须与轮数同报：10/20 与 10/200 紧迫度完全不同"
        );
    }

    /// 守卫注销（子 agent 收尾）后轮数归空：列表要显示的是「这轮跑了多久」，
    /// 不是死后残留的最后一帧计数。终态行拿 started/finished 算时长即可。
    #[test]
    fn list_view_drops_rounds_after_guard_dropped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let sid = format!("sa-test-drop-{}", ack.subagent_id);
        crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

        {
            let _guard = crate::tool_guard::SubagentSessionGuard::new(
                &sid,
                crate::tool_guard::SubagentSessionCtx {
                    subagent_id: ack.subagent_id.clone(),
                    task_id: None,
                    profile: SubagentProfile::Research,
                    tools: std::sync::Arc::new(vec![]),
                    budget: SubagentBudget::default(),
                    artifact_dir: std::path::PathBuf::from("/tmp/art_drop"),
                    used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(7)),
                    wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
                },
            );
            let row = crate::db::load_subagent(&conn, &ack.subagent_id)
                .unwrap()
                .expect("行仍在");
            assert_eq!(attach_rounds(row).rounds_used, Some(7));
        }

        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("行仍在");
        let after = attach_rounds(row);
        assert_eq!(
            (after.rounds_used, after.rounds_budget),
            (None, None),
            "守卫注销后不得继续报轮数"
        );
    }

    /// 子 agent 预算默认值单源：db 侧常量必须引用 bot::params，改一处不漏两处。
    #[test]
    fn subagent_budget_defaults_have_single_source() {
        assert_eq!(
            crate::db::subagents::DEFAULT_MAX_TURNS,
            crate::bot::params::DEFAULT_SUBAGENT_MAX_TURNS
        );
        assert_eq!(
            crate::db::subagents::DEFAULT_MAX_TOOL_CALLS,
            crate::bot::params::DEFAULT_SUBAGENT_MAX_TOOL_CALLS
        );
        assert_eq!(
            crate::db::subagents::DEFAULT_MAX_WALL_SECONDS,
            crate::bot::params::DEFAULT_SUBAGENT_MAX_WALL_SECS
        );
    }

    #[test]
    fn check_counts_progress_from_acceptance_and_result() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        // 未收尾：total = 验收标准条数，done = 0
        let by_id = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(by_id.subagent_id, ack.subagent_id);
        assert_eq!(by_id.status, SubagentStatus::Queued);
        assert_eq!(by_id.progress["subtasksDone"], 0);
        assert_eq!(by_id.progress["subtasksTotal"], 2);
        assert!(by_id.result.is_none());
        assert_eq!(by_id.task_id, None);
        // 收尾后：done = result_json.acceptance_check 里 met=true 的条数
        crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"acceptance_check":[{"criterion":"覆盖至少 5 个产品","met":true},{"criterion":"每条含官网 URL","met":false}]}"#,
        )
        .unwrap();
        let done = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(done.progress["subtasksDone"], 1);
        assert_eq!(done.progress["subtasksTotal"], 2);
        // 幂等：两次 check 结果一致
        assert_eq!(
            check_subagent_locked(&conn, &ack.subagent_id)
                .unwrap()
                .progress,
            done.progress
        );
    }

    /// 机器可判定谓词：只有带前缀的条目进机器核验，自由文本原样留给 verifier
    #[test]
    fn machine_criterion_parsed_only_for_known_prefixes() {
        assert_eq!(
            parse_machine_criterion("file_exists:report.md"),
            Some(MachineCheck::FileExists("report.md".into()))
        );
        assert_eq!(
            parse_machine_criterion("  file_nonempty: out/数据.xlsx "),
            Some(MachineCheck::FileNonEmpty("out/数据.xlsx".into()))
        );
        assert_eq!(parse_machine_criterion("覆盖至少 5 个产品"), None);
        assert_eq!(parse_machine_criterion("每条含官网 URL"), None);
        assert_eq!(parse_machine_criterion(""), None);
    }

    /// 路径逃逸一律不解析：判定路径能跑出产物目录，验收就成了任意文件探测器
    #[test]
    fn machine_criterion_rejects_paths_escaping_artifact_dir() {
        for bad in [
            "file_exists:../../etc/passwd",
            "file_exists:/etc/passwd",
            "file_exists:C:\\Windows\\win.ini",
            "file_exists:./report.md",
            "file_exists:",
            "file_nonempty:a//b.md",
        ] {
            assert_eq!(
                parse_machine_criterion(bad),
                None,
                "逃逸路径必须不解析：{bad}"
            );
        }
        assert_eq!(safe_rel_path("a/b/c.md"), Some("a/b/c.md"));
    }

    /// 机器判定真去 FS 看：存在/不存在/空文件三种形态
    #[test]
    fn eval_machine_criteria_reads_real_files() {
        let dir = std::env::temp_dir().join(format!("wm-mc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.md"), "# 报告").unwrap();
        std::fs::write(dir.join("empty.md"), "").unwrap();

        let out = eval_machine_criteria(
            &[
                "file_exists:report.md".into(),
                "file_nonempty:report.md".into(),
                "file_nonempty:empty.md".into(),
                "file_exists:missing.md".into(),
                "自由文本标准".into(),
            ],
            &dir,
        );
        assert_eq!(out.len(), 4, "自由文本条目不进机器判定");
        assert!(out[0].met && out[1].met);
        assert!(!out[2].met, "空文件不满足 nonempty");
        assert!(!out[3].met, "缺失文件不通过");
        assert_eq!(out[3].criterion, "file_exists:missing.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 机器结论覆盖模型自评：模型说 met:true 而文件不存在 → 以服务端为准
    #[test]
    fn merge_machine_verdicts_overrides_self_report_and_fills_missing() {
        let mut result = serde_json::json!({
            "acceptance_check": [
                {"criterion": "file_exists:report.md", "met": true, "evidence": "我写好了"},
                {"criterion": "内容覆盖 5 个产品", "met": false}
            ]
        });
        merge_machine_verdicts(
            &mut result,
            &[MachineVerdict {
                criterion: "file_exists:report.md".into(),
                met: false,
                reason: "产物不存在：NotFound".into(),
            }],
        );
        let arr = result["acceptance_check"].as_array().unwrap();
        assert_eq!(arr[0]["met"], false, "自评必须被服务端结论覆盖");
        assert_eq!(arr[0]["machine"], true);
        assert!(arr[0]["machine_reason"]
            .as_str()
            .unwrap()
            .contains("产物不存在"));
        assert_eq!(arr[1]["met"], false, "非机器条目不受影响");

        // 模型压根没报这条 → 服务端补一条（不依赖模型主动上报）
        let mut only_free = serde_json::json!({"acceptance_check": []});
        merge_machine_verdicts(
            &mut only_free,
            &[MachineVerdict {
                criterion: "file_nonempty:out.xlsx".into(),
                met: true,
                reason: "服务端核验：out.xlsx 存在（12 字节）".into(),
            }],
        );
        let arr = only_free["acceptance_check"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["met"], true);

        // verifier 行用 checks 键，同样处理
        let mut verifier = serde_json::json!({"checks": []});
        merge_machine_verdicts(
            &mut verifier,
            &[MachineVerdict {
                criterion: "file_exists:v.md".into(),
                met: true,
                reason: "ok".into(),
            }],
        );
        assert_eq!(verifier["checks"].as_array().unwrap().len(), 1);
    }

    /// 机器判定落到用户可见的正文：未通过项必须点名，不能只躺在 result_json
    #[test]
    fn machine_failures_are_named_in_final_text() {
        let verdict = |c: &str, met: bool| MachineVerdict {
            criterion: c.into(),
            met,
            reason: "产物不存在：NotFound".into(),
        };
        // 全通过 → 正文不动
        let (r, text) = apply_machine_verdicts(
            Some(serde_json::json!({"acceptance_check": []})),
            "干完了".into(),
            &[verdict("file_exists:a.md", true)],
        );
        assert_eq!(text, "干完了");
        assert_eq!(r.unwrap()["acceptance_check"].as_array().unwrap().len(), 1);

        // 有未通过 → 正文点名 + 原因
        let (r, text) = apply_machine_verdicts(
            Some(serde_json::json!({"acceptance_check": []})),
            "干完了".into(),
            &[
                verdict("file_exists:a.md", true),
                verdict("file_exists:b.md", false),
            ],
        );
        assert!(text.contains("机器验收未通过 1 项"), "{text}");
        assert!(text.contains("file_exists:b.md"), "{text}");
        assert!(text.contains("产物不存在"), "{text}");
        assert!(text.starts_with("干完了"), "原正文必须保留：{text}");
        assert_eq!(r.unwrap()["acceptance_check"].as_array().unwrap().len(), 2);

        // 无机器条目 → 原样返回（不改 JSON、不动正文）
        let (r, text) = apply_machine_verdicts(
            Some(serde_json::json!({"acceptance_check": [{"criterion":"x","met":true}]})),
            "干完了".into(),
            &[],
        );
        assert_eq!(text, "干完了");
        assert_eq!(r.unwrap()["acceptance_check"][0]["met"], true);
        // 异常退出无收尾 JSON：不得凭空造一个
        let (r, _) = apply_machine_verdicts(
            None,
            "执行失败".into(),
            &[verdict("file_exists:a.md", false)],
        );
        assert!(r.is_none(), "没有收尾 JSON 时不应伪造");
    }

    /// 交叉核对：没派发过的标准、重复上报，都不得灌满进度
    #[test]
    fn progress_ignores_undeclared_and_duplicate_criteria() {
        let declared = vec![
            "覆盖至少 5 个产品".to_string(),
            "每条含官网 URL".to_string(),
        ];
        // 虚报一条从未派发的标准 + 重复报同一条 → 只应计 1
        let reported = vec![
            serde_json::json!({"criterion": "覆盖至少 5 个产品", "met": true}),
            serde_json::json!({"criterion": "覆盖至少 5 个产品", "met": true}),
            serde_json::json!({"criterion": "我其实做了 10 个产品", "met": true}),
        ];
        assert_eq!(count_met_against_declared(&declared, &reported), 1);
        // met 非 bool / 缺 criterion 的条目不计
        assert_eq!(
            count_met_against_declared(
                &declared,
                &[serde_json::json!({"criterion": "每条含官网 URL", "met": "yes"})]
            ),
            0
        );
        assert_eq!(
            count_met_against_declared(&declared, &[serde_json::json!({"met": true})]),
            0
        );
        // 白空格差异不算漏报（模型可能顺手加了空格）
        assert_eq!(
            count_met_against_declared(
                &declared,
                &[serde_json::json!({"criterion": " 每条含官网 URL ", "met": true})]
            ),
            1
        );
    }

    #[test]
    fn verifier_row_progress_counts_checks_array_not_zero() {
        // verifier 行的收尾 JSON 是 checks 数组（verifier 独有 schema）：
        // done 若只认 acceptance_check，验收全过的 verifier 行也显示 0/N，
        // 主 agent 会误判「没做成」而重派
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"checks":[{"criterion":"覆盖至少 5 个产品","met":true},{"criterion":"每条含官网 URL","met":true}]}"#,
        )
        .unwrap();
        let state = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(state.progress["subtasksDone"], 2);
        assert_eq!(state.progress["subtasksTotal"], 2);
    }

    #[test]
    fn check_unknown_key_is_invalid_argument() {
        let conn = test_conn();
        let err = check_subagent_locked(&conn, "sa_ghost").unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
    }

    #[test]
    fn check_returns_parsed_result_json() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"summary":"结论","confidence":0.9}"#,
        )
        .unwrap();
        crate::db::update_subagent_status(
            &conn,
            &ack.subagent_id,
            SubagentStatus::Running,
            5_100,
            None,
        )
        .unwrap();
        crate::db::update_subagent_status(
            &conn,
            &ack.subagent_id,
            SubagentStatus::Succeeded,
            5_200,
            None,
        )
        .unwrap();
        let state = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(state.status, SubagentStatus::Succeeded);
        assert_eq!(state.result.unwrap()["summary"], "结论");
    }

    // stage enum 字符串映射（设计 §8.5；调用方传入 stage=recon/executor/verifier）
    #[test]
    fn subagent_stage_roundtrips_string() {
        for (s, expected) in [
            (SubagentStage::Recon, "recon"),
            (SubagentStage::Executor, "executor"),
            (SubagentStage::Verifier, "verifier"),
        ] {
            assert_eq!(s.as_str(), expected);
            assert_eq!(SubagentStage::from_str(expected), Some(s));
        }
        assert_eq!(SubagentStage::from_str("unknown"), None);
    }

    // stage → 默认 profile 派生（设计 §8.5 映射）
    #[test]
    fn stage_default_profile_is_research_or_coder() {
        // recon/verifier 只读足够 → research
        assert_eq!(
            SubagentStage::Recon.default_profile(),
            SubagentProfile::Research
        );
        assert_eq!(
            SubagentStage::Verifier.default_profile(),
            SubagentProfile::Research
        );
        // executor 写类必需 → coder
        assert_eq!(
            SubagentStage::Executor.default_profile(),
            SubagentProfile::Coder
        );
    }

    // spawn 行上 profile 仍按显式值落库（stage 不进 DB 表）
    #[test]
    fn stage_is_not_persisted_and_profile_field_unaffected() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.profile = SubagentProfile::Coder;
        r.stage = Some(SubagentStage::Executor);
        let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        // 行 profile 是显式给的 Coder（stage 在内存里走闭包，不落表）
        assert_eq!(row.profile, SubagentProfile::Coder);
    }

    // ── per-onion 配置档接线（期 6）──

    /// profile 解析优先级：显式 > 配置档 > stage 派生，都没给则 None。
    ///
    /// 「配置档 > stage」这一档是配置档 profile 唯一的生效路径——它排在
    /// 显式之后，才不会让 yaml 覆盖运行时档位（那等于放宽）。
    #[test]
    fn spawn_profile_priority_is_explicit_then_config_then_stage() {
        let e = Some(SubagentProfile::General);
        let c = Some(SubagentProfile::Coder);
        // ① 显式压过配置档与 stage（防「配置覆盖运行时 profile = 放宽」）
        assert_eq!(
            resolve_spawn_profile(e, c, Some(SubagentStage::Executor)),
            Some(SubagentProfile::General)
        );
        // ② 没给显式 → 配置档兜底，压过 stage
        assert_eq!(
            resolve_spawn_profile(None, c, Some(SubagentStage::Executor)),
            Some(SubagentProfile::Coder)
        );
        // ③ 只有 stage → 按 stage 派生
        assert_eq!(
            resolve_spawn_profile(None, None, Some(SubagentStage::Verifier)),
            Some(SubagentProfile::Research)
        );
        // ④ 三者皆无 → None，调用方据此报错（不许猜一个默认档）
        assert_eq!(resolve_spawn_profile(None, None, None), None);
    }

    #[test]
    fn onion_turn_cap_only_tightens() {
        let mut b = SubagentBudget {
            max_turns: 30,
            max_tool_calls: 100,
            max_wall_seconds: 600,
        };
        // 配置档更紧 → 压到配置档的值
        apply_onion_turn_cap(&mut b, Some(4));
        assert_eq!(b.max_turns, 4);
        // 配置档更松 → 不许被它顶开（配置档是上限，不是覆盖）
        apply_onion_turn_cap(&mut b, Some(50));
        assert_eq!(b.max_turns, 4, "配置档比派发值宽时不应放宽轮数");
        // 没给上限 → 不动
        apply_onion_turn_cap(&mut b, None);
        assert_eq!(b.max_turns, 4);
    }

    /// 接线守卫：配置档从派发到执行的四段链路，一段都不能靠单测推断。
    ///
    /// 四段分别是「spawn 解析 → 内存闭包进 runner → 建会话写两列 →
    /// bind 出的交集进守卫白名单」。缺任一段的症状都是同一个：模型以为
    /// 派了个受限子 agent，实际跑的是全档位——纯函数单测全绿也照样漏。
    ///
    /// **所有匹配串都用 `concat!` 拆开**：本测试就住在被扫描的同一个文件里，
    /// 裸写字面量会被测试自己的源码匹配上，删掉真接线照样绿（本条守卫第一版
    /// 就是这么假绿的）。与 `tool_guard::dead_command_bind_file_not_in_dispatcher` 同款。
    #[test]
    fn onion_config_is_wired_from_spawn_to_guard_whitelist() {
        let body = orch_prod_src();

        // ① 派发层解析出配置档并装进 SpawnRequest
        let parse_arm = body
            .split_once(concat!("crate::bot_onions::", "load(app, id)"))
            .expect("派发层必须先加载配置档再建行")
            .1
            .split("parent_session_id: session_id,")
            .next()
            .expect("SpawnRequest 构造应在 session_id 之前");
        assert!(
            parse_arm.contains(concat!("onion: ", "onion_cfg")),
            "SpawnRequest 必须带上已解析的配置档：{parse_arm}"
        );

        // ② 配置档随内存闭包进 runner（不落 subagents 行）
        assert!(
            body.contains(concat!("let runner_onion = ", "req.onion.clone();")),
            "配置档必须从 req 摘出来交给 runner，否则 runner 拿不到配置名"
        );
        assert!(
            body.contains(concat!(
                "run_subagent(runner_app, runner_sid, ",
                "runner_stage, runner_onion)"
            )),
            "run_subagent 必须收到配置档参数"
        );

        // ③ 建会话时把配置写进 bot_sessions 两列——不写则 bind_session 无从读起
        let setup = body
            .split_once(concat!("bot_session_", "set_onion("))
            .expect("子会话建会话时必须写配置档两列")
            .1;
        assert!(
            setup.contains(concat!("Some(&", "o.config)")),
            "要写配置档名（否则 bind 读到空指针，配置静默失效）"
        );

        // ④ 守卫白名单取交集，且 bind 发生在守卫注册之前
        let bind_at = body
            .find(concat!(
                "crate::bot_onions::",
                "bind_session(&app, &session_id)"
            ))
            .expect("runner 必须绑定配置");
        let guard_at = body
            .find("SubagentSessionGuard::new(")
            .expect("runner 必须注册守卫");
        assert!(
            bind_at < guard_at,
            "bind_session 必须在守卫注册之前：守卫的白名单取自绑定后的交集，\
             顺序反了等于拿 profile 白名单当交集"
        );
        let guard_arm = body.split_at(guard_at).1;
        let guard_arm = &guard_arm[..guard_arm.find(");").unwrap_or(guard_arm.len())];
        assert!(
            guard_arm.contains(concat!("tools: std::sync::Arc::", "new(subagent_tools)")),
            "守卫必须带交集后的工具集，闸才是按收窄判的：{guard_arm}"
        );
    }

    /// 接线守卫：配置档的角色与实际可用工具要进子 agent 的 system 提示词。
    ///
    /// 少了这段，配置档对模型就是「工具莫名少了一半」——它会反复调被收掉的
    /// 工具、每次吃一次白名单拒绝，白烧轮数。匹配串同样 `concat!` 拆开。
    #[test]
    fn onion_prompt_note_is_injected_into_subagent_system() {
        let body = orch_prod_src();
        assert!(
            body.contains(concat!(
                "crate::bot_onions::",
                "render_prompt_note(cfg, row.profile)"
            )),
            "system 拼装必须注入配置档提示词段"
        );
        let system_at = body
            .find("let system = format!(")
            .expect("子 agent system 拼装点");
        let arm = &body[system_at..];
        let arm = &arm[..arm.find("let msgs = vec![").unwrap_or(arm.len())];
        assert!(
            arm.contains("onion_note"),
            "system 格式串必须带上配置档段，无配置档时为空串占位：{arm}"
        );
    }
}
