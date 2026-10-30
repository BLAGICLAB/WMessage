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
//! - 预算强制（turns/tool_calls）+ 并发排队（per-session 2 / global 3）+ 前端。
//!
//! 结构约定（同 task_patch / task_patch_locked 先例）：纯 DB 核心 `*_locked` 可在
//! in-memory conn 上单测；异步包装负责 spawn_blocking + DB_WRITE_LOCK + 审计 + 广播。

use crate::db::{now_ms, SubagentProfile, SubagentRow, SubagentStatus};
use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

// ── subagent-split: begin ──
// 拆分后的子模块 re-export：旧路径 `crate::bot_orchestrator::<name>` 1:1 保持可用。
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
pub(crate) use crate::bot::subagent_repair::classify_outcome;
pub(crate) use crate::bot::subagent_repair::repair_final_json_once;
pub(crate) use crate::bot::subagent_runner::run_subagent;
pub(crate) use crate::bot::subagent_spawn::resolve_row;
// 只有测试在用的私有项：非测试构建里没人消费，按构建期切换避免 unused import
#[cfg(test)]
pub(crate) use crate::bot::subagent_repair::parse_final_json;
pub(crate) use crate::bot::subagent_spawn::resolve_spawn_profile;
pub(crate) use crate::bot::subagent_spawn::SubagentGate;
pub(crate) use crate::bot::subagent_tools::artifact_dir_of;
pub(crate) use crate::bot::subagent_tools::gen_dir_rule_for;
pub(crate) use crate::bot::subagent_tools::safe_rel_path;
// ── subagent-split: end ──

/// check_subagent 的 wait_ms 上限（设计 §5；放宽理由见
/// docs/SUBAGENT-CHECK-BACKOFF-DESIGN-2026-10-18.md）。
///
/// 上限放到 10 分钟：每返回一次 check_subagent，主 agent 就要多走一整个
/// LLM 轮次（重发请求 + 重算上下文 + 把轮询结果再塞进历史），所以「早知道」
/// 的边际收益远小于「少醒几次」。短上限逼着模型反复短轮询——实测一个 3 分钟
/// 的调研子任务被拆成 4 次 30s 等待，每次都是一轮模型开销，最后还因为等不到
/// 而把「仍在进行中」当成了整轮答复。一次给足预算让它整个睡到终态，轮次降到 1。
pub const CHECK_WAIT_MS_CAP: u64 = crate::bot::constants::SUBAGENT_CHECK_WAIT_MS_CAP;
/// wait 兜底轮询步长阶梯（毫秒）。首档 1500ms：终态另有事件唤醒
/// （见 `finish_notify`），这条阶梯只是漏通知时的安全网，所以起手就不必密——
/// 越密只是越频繁地开库跑全量 DDL。末档后恒定封顶：兜底延迟要有上界，
/// 否则退化成看不见的卡住。
const CHECK_POLL_LADDER_MS: [u64; 10] = [
    1_500, 3_000, 6_000, 12_000, 24_000, 30_000, 60_000, 120_000, 300_000, 600_000,
];

/// 步长内的停止检查切片（毫秒）：每个步长切成这么薄的片睡，片间查一次
/// 停止标志——/stop 的响应延迟以此为上界，不再随步长阶梯走到分钟级。
/// 只影响睡眠粒度，DB 重读仍只在事件唤醒或整步走完时发生。
const CHECK_STOP_SLICE_MS: u64 = 500;

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
    // readOnly 消费，不参与任何强制（轮数上限与主会话同一套安全网）。
    if let Some(used) = crate::tool_guard::subagent_ctx(row.session_id.as_deref())
        .map(|c| c.used_rounds.load(std::sync::atomic::Ordering::Relaxed))
    {
        progress["roundsUsed"] = serde_json::json!(used);
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
        // 整步切成薄片睡：/stop 落在步长中间时 ≤500ms 内醒（一把睡过去的
        // 话末档 600s 要等满整个预算）。监听复用上面已 enable 的那个
        // notified——事件到达时立刻破片回去重读状态；薄片到点只查停止
        // 标志，不重读——DB 读频不随切片变密。
        let step_end = std::time::Instant::now() + std::time::Duration::from_millis(step_ms);
        loop {
            let now = std::time::Instant::now();
            if now >= step_end {
                break;
            }
            let slice = std::time::Duration::from_millis(CHECK_STOP_SLICE_MS).min(step_end - now);
            if tokio::time::timeout(slice, notified.as_mut()).await.is_ok() {
                break;
            }
            if stop.is_some_and(|t| t.stopped()) {
                break;
            }
        }
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
        let now = crate::db::now_ms();
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
    /// 已执行工具调用数；与轮数同源（tool_guard 内存仪表），终态为 None
    pub tool_calls_used: Option<u32>,
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
        tool_calls_used: ctx
            .as_ref()
            .map(|c| c.used_tool_calls.load(std::sync::atomic::Ordering::Relaxed) as u32),
        row,
    }
}

#[cfg(test)]
mod orchestrator_tests;
