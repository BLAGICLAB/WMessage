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

/// 预算默认值与硬顶（设计 §6 已）——常量与钳制实现在 db::SubagentBudget（写口
/// 强制：spawn 与 task_patch 都过 clamped()），此处 re-export 保持原路径可用。
pub use crate::db::{
    DEFAULT_MAX_TOOL_CALLS, DEFAULT_MAX_TURNS, DEFAULT_MAX_WALL_SECONDS, MAX_TURNS_HARD_CAP,
};
/// check_subagent 的 wait_ms 上限（设计 §5）。
pub const CHECK_WAIT_MS_CAP: u64 = 5_000;
/// wait 轮询步长。
const CHECK_POLL_INTERVAL_MS: u64 = 150;

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

/// 预算钳制：None → 默认值；其余交给 SubagentBudget::clamped（硬顶 50 / 下限 1）。
pub fn clamp_budget(budget: Option<SubagentBudget>) -> SubagentBudget {
    budget.unwrap_or_default().clamped()
}

/// 配置档的 `max_turn` 只收紧：派发给的轮数更宽时不许被配置档顶开。
///
/// 取 min 而非覆盖——配置档是权限上限，不是「优先于调用方的指令」。
fn apply_onion_turn_cap(budget: &mut SubagentBudget, cap: Option<u32>) {
    if let Some(cap) = cap {
        if cap < budget.max_turns {
            budget.max_turns = cap;
        }
    }
}

/// 派发 profile 的解析顺序：显式 `profile` > 配置档的 `profile` > `stage` 派生。
///
/// **不是「配置档说了算」**：让 yaml 的 profile 覆盖运行时 profile 就是放宽
/// （模型给 research、配置写 general → 白名单变大），与设计 §3.4「只有收紧、
/// 没有第二张 allow 表」冲突。显式 profile 优先＝把档位的决定权交回运行时，
/// yaml 的 `profile:` 只在调用方没表态时兜底。
///
/// 三者都给不出时返回 None：那是「没说清派个什么档位」，必须报错而不是猜。
pub(crate) fn resolve_spawn_profile(
    explicit: Option<SubagentProfile>,
    config_profile: Option<SubagentProfile>,
    stage: Option<SubagentStage>,
) -> Option<SubagentProfile> {
    explicit
        .or(config_profile)
        .or_else(|| stage.map(|s| s.default_profile()))
}

fn validate_request(req: &SpawnRequest) -> CommandResult<()> {
    if req.objective.trim().is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "objective".into(),
            value: req.objective.clone(),
            reason: "目标不能为空".into(),
        });
    }
    if req.acceptance_criteria.is_empty()
        || req.acceptance_criteria.iter().any(|c| c.trim().is_empty())
    {
        return Err(CommandError::InvalidArgument {
            field: "acceptance_criteria".into(),
            value: format!("{:?}", req.acceptance_criteria),
            reason: "验收标准必填且每条非空".into(),
        });
    }
    Ok(())
}

/// spawn 纯 DB 核心：校验 → 缺 parent 卡 TaskNotFound → 插 queued 行。
/// 子 agent 不再建任务卡——执行契约全在行上（objective / acceptance_json /
/// context_summary / budget_json），前端经 `list_subagents` 读行看进度。
pub(crate) fn spawn_subagent_locked(
    conn: &mut rusqlite::Connection,
    req: &SpawnRequest,
    now: i64,
) -> CommandResult<SpawnAck> {
    validate_request(req)?;
    if let Some(parent) = &req.parent_task_id {
        let exists = crate::db::task_exists(conn, parent).map_err(CommandError::DbError)?;
        if !exists {
            return Err(CommandError::TaskNotFound(parent.clone()));
        }
    }
    let budget = clamp_budget(req.budget.clone());
    let subagent_id = format!("sa_{}", uuid::Uuid::new_v4().simple());
    let trace_id = format!("trc_{}", uuid::Uuid::new_v4().simple());
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    let row = SubagentRow {
        id: subagent_id.clone(),
        status: SubagentStatus::Queued,
        profile: req.profile,
        model: req.model_profile.clone(),
        parent_session_id: req.parent_session_id.clone(),
        session_id: None,
        task_id: None,
        objective: req.objective.trim().to_string(),
        trace_id: trace_id.clone(),
        acceptance_json: Some(
            serde_json::to_string(&req.acceptance_criteria)
                .map_err(|e| CommandError::Internal(e.to_string()))?,
        ),
        context_summary: req
            .context_summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        budget_json: Some(
            serde_json::to_string(&budget).map_err(|e| CommandError::Internal(e.to_string()))?,
        ),
        result_json: None,
        error: None,
        created_at: now,
        started_at: None,
        finished_at: None,
    };
    crate::db::insert_subagent(&tx, &row).map_err(CommandError::DbError)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(SpawnAck {
        subagent_id,
        task_id: None,
        trace_id,
        status: SubagentStatus::Queued,
    })
}

/// 按 subagent_id / task_id / 子会话 id 解析行（设计 §5 双键 +  会话键：
/// 任务卡停止按钮与子会话停止键只持有会话 id）。
/// 前缀约定：subagent_id 恒以 `sa_` 开头；task_id 与 session_id 均为裸 uuid——
/// 先 task_id 后 session_id 双探测（ 批勘误）。
fn resolve_row(conn: &rusqlite::Connection, key: &str) -> CommandResult<SubagentRow> {
    let row = if key.starts_with("sa_") {
        crate::db::load_subagent(conn, key).map_err(CommandError::DbError)?
    } else {
        match crate::db::find_subagent_by_task(conn, key).map_err(CommandError::DbError)? {
            Some(r) => Some(r),
            None => {
                crate::db::find_subagent_by_session(conn, key).map_err(CommandError::DbError)?
            }
        }
    };
    row.ok_or_else(|| CommandError::InvalidArgument {
        field: "subagent_id".into(),
        value: key.to_string(),
        reason: "subagent 不存在（subagent_id / task_id / 会话 id 均未命中）".into(),
    })
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

/// spawn 异步包装：持锁执行核心 → 审计 subagent_spawned → 发 subagent-spawned
/// 事件（行建好即通知前端重拉，收尾前就可见可取消）→ 派发 runner。
pub async fn spawn_subagent(app: &AppHandle, req: SpawnRequest) -> CommandResult<SpawnAck> {
    // LLM 未显式给预算时用配置默认（config subagentMax* 三项，resolve 内钳制）；
    // 显式给了的仍走 clamp_budget 硬顶。Gate 并发数（max_running）保持常量不在本路径。
    let mut req = req;
    if req.budget.is_none() {
        let cfg = crate::bot::load_config(app);
        req.budget = Some(crate::bot::params::resolve_subagent_budget(&cfg));
    }
    // 配置档 max_turn 只收紧（规则见 apply_onion_turn_cap）
    if let Some(b) = req.budget.as_mut() {
        apply_onion_turn_cap(b, req.onion.as_ref().and_then(|o| o.max_turn));
    }
    // 审计字段先拷出（闭包要 move req 进 spawn_blocking）；校验只走 locked 核心
    //（wrapper 侧重复调用已删——）
    let audit_parent_session = req.parent_session_id.clone();
    let audit_profile = req.profile;
    let audit_model = req.model_profile.clone();
    // stage 走 in-memory 闭包——spawn 触发 runner 是同步启动，闭包 move 进 runner；
    // 不落库（详见 SubagentStage 注释）。SubagentStage 是 Copy，所以这里直接搬走。
    let runner_stage = req.stage;
    // 配置档同理走内存闭包（理由见 SpawnOnion 注释）
    let runner_onion = req.onion.clone();
    let app2 = app.clone();
    let ack = tauri::async_runtime::spawn_blocking(move || -> CommandResult<SpawnAck> {
        let _g = crate::db::lock_db_write();
        let mut conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        let now = chrono::Utc::now().timestamp_millis();
        spawn_subagent_locked(&mut conn, &req, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent spawn 线程 join 失败：{e}")))??;
    // 审计：结构化 + 文本行（设计 §3 事件字段）。task_id 恒空（新行无卡），
    // 字段仍留——老审计日志按该键反查的查询不受影响
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "subagent_spawned",
        &[
            ("subagent_id", ack.subagent_id.clone()),
            ("task_id", ack.task_id.clone().unwrap_or_default()),
            (
                "parent_session_id",
                audit_parent_session.clone().unwrap_or_default(),
            ),
            ("profile", audit_profile.as_str().to_string()),
            ("model", audit_model.unwrap_or_default()),
            ("trace_id", ack.trace_id.clone()),
        ],
    );
    crate::bot::audit_log(
        app,
        &format!(
            "subagent_spawned | id: {} | parent_session: {} | profile: {} | trace: {}",
            ack.subagent_id,
            audit_parent_session.as_deref().unwrap_or("-"),
            audit_profile.as_str(),
            ack.trace_id,
        ),
    );
    // 非阻塞派发 runner（设计 §5：spawn 立即返回；执行在后台推进状态机）。
    // spawned 事件在派发前发：不与 runner 的收尾事件竞争先后，行建好即告知
    emit_subagent_spawned(app, &ack.subagent_id, audit_parent_session.as_deref());
    let runner_app = app.clone();
    let runner_sid = ack.subagent_id.clone();
    tauri::async_runtime::spawn(async move {
        run_subagent(runner_app, runner_sid, runner_stage, runner_onion).await
    });
    Ok(ack)
}

/// check 异步包装：wait_ms=0 单次读；>0 轮询至终态或超时（幂等可轮询，设计 §5）。
pub async fn check_subagent(app: &AppHandle, key: &str, wait_ms: u64) -> CommandResult<CheckState> {
    let wait = wait_ms.min(CHECK_WAIT_MS_CAP);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait);
    loop {
        let app2 = app.clone();
        let key = key.to_string();
        let state = tauri::async_runtime::spawn_blocking(move || -> CommandResult<CheckState> {
            let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
            check_subagent_locked(&conn, &key)
        })
        .await
        .map_err(|e| CommandError::from(format!("subagent check 线程 join 失败：{e}")))??;
        if state.status.is_terminal() || std::time::Instant::now() >= deadline {
            return Ok(state);
        }
        tokio::time::sleep(std::time::Duration::from_millis(CHECK_POLL_INTERVAL_MS)).await;
    }
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

use crate::bot_chat::ChatGuard;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::Emitter;

/// 持 DB_WRITE_LOCK 的阻塞 DB 操作辅助（runner 链路各步共用）。
async fn db_locked<R, F, T>(app: &AppHandle<R>, f: F) -> CommandResult<T>
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

/// 从文本提取**最后一个**顶层 JSON 对象（设计 §7 解析策略）。
/// 字符串感知的花括号扫描：收集深度 1 的 {...} span，从末往前试 serde 解析，
/// 首个成功且为 object 的返回。找不到/全失败 → None。
pub(crate) fn extract_last_json_object(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *c == '\\' {
                escaped = true;
            } else if *c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    spans.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    spans.reverse();
    for (a, b) in spans {
        let candidate: String = chars[a..b].iter().collect();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&candidate) {
            if v.is_object() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 收尾结果判定（纯函数，设计 §6/§7）：
/// 墙钟触达 → budget_exceeded(max_wall_seconds)；轮数熔断 → budget_exceeded(max_turns)；
/// 其他循环错误 → failed；正文解析成功 → 模型自报 succeeded/failed（自报
/// cancelled/budget_exceeded 视为 failed + error 说明）；解析失败 →
/// failed + result_json_unparseable + 原文存 summary（部分结果不丢弃）。
pub(crate) fn classify_outcome(
    run_err: Option<&str>,
    wall_exceeded: bool,
    final_text: Option<&str>,
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    if wall_exceeded {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_wall_seconds".into()),
            None,
        );
    }
    if let Some(err) = run_err {
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(err, 300)),
            None,
        );
    }
    let text = final_text.unwrap_or_default();
    // 轮数熔断：熔断出口返回 Ok，正文带 ROUNDS_FUSE_MARKER（见 bot_model_loop）。
    // 终态仍是 budget_exceeded，但正文不是丢弃而是存进 result_json.summary——
    // 跑完的那十几轮里查到的东西对父 agent 仍有价值（判断该不该原目标重试、
    // 还是换 profile / 放宽验收），写 NULL 等于逼重试从零再来。
    if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_turns".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "轮数预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    let parsed = extract_last_json_object(text)
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok());
    match parsed {
        Some(v) => {
            // verifier 子 agent 输出独有 schema：含 checks 数组的 verified.json
            // ——只看 status 会漏掉「逐条 acceptance_criteria 验收」的真实语义。
            // 此处按结构识别走独立分支判定，不依赖 stage 字段（verifier 输出已经
            // 结构化，从结构出发比从 stage 出发的耦合面小）。
            if let Some(checks) = v.get("checks").and_then(|c| c.as_array()) {
                return classify_verifier_outcome(&v, checks);
            }
            let claimed = v
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("succeeded");
            match claimed {
                "succeeded" => (SubagentStatus::Succeeded, None, Some(v)),
                "failed" => (
                    SubagentStatus::Failed,
                    Some("model_reported_failure".into()),
                    Some(v),
                ),
                "cancelled" => (
                    SubagentStatus::Failed,
                    Some("model_reported_cancelled".into()),
                    Some(v),
                ),
                "budget_exceeded" => (
                    SubagentStatus::Failed,
                    Some("model_reported_budget_exceeded".into()),
                    Some(v),
                ),
                _ => (SubagentStatus::Succeeded, None, Some(v)),
            }
        }
        None => (
            SubagentStatus::Failed,
            Some("result_json_unparseable".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "收尾 JSON 解析失败，原文截断存此防丢（设计 §7）"
            })),
        ),
    }
}

/// verifier 路径判定：按 `checks[*].met` 综合决定子 agent 终态。
/// - checks 空：summary 含明确否决词（未通过/fail/reject/failed/否决）→ Failed；
///   否则视为 N/A 走 Succeeded（verifier 没排到检查项极少，fail-safe 默认放行）
/// - 全部 met=true → Succeeded
/// - 全部 met=false → Failed + 错误 reason 拼未通过条目的 criterion 列表（≤3 项）
///   给主 agent 看到「到底哪里没通过」的最小信号
/// - 部分 met=true → Succeeded（主 agent 看 result_json 自己决策下一步）
fn classify_verifier_outcome(
    v: &serde_json::Value,
    checks: &[serde_json::Value],
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    let total = checks.len();
    let passed = checks
        .iter()
        .filter(|c| c.get("met").and_then(|m| m.as_bool()) == Some(true))
        .count();
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
    if total == 0 {
        // checks 为空：按 summary 显式否决词判定
        let reject_keywords = ["未通过", "fail", "failed", "reject", "否决", "驳回"];
        let explicit_reject = reject_keywords
            .iter()
            .any(|kw| summary.to_lowercase().contains(&kw.to_lowercase()));
        if explicit_reject {
            return (
                SubagentStatus::Failed,
                Some(format!(
                    "verifier_summary_rejected: {}",
                    crate::bot::truncate_for_log(summary, 80)
                )),
                Some(v.clone()),
            );
        }
        return (SubagentStatus::Succeeded, None, Some(v.clone()));
    }
    if passed == total {
        return (SubagentStatus::Succeeded, None, Some(v.clone()));
    }
    if passed == 0 {
        // 全部未通过：把失败 criterion 列表（前 3 项）拼进 error reason 给主 agent 看
        let failed_list: Vec<String> = checks
            .iter()
            .take(3)
            .filter_map(|c| {
                let criterion = c.get("criterion").and_then(|s| s.as_str())?;
                Some(format!("{criterion}=false"))
            })
            .collect();
        let reason = if failed_list.is_empty() {
            "verifier_all_checks_failed".to_string()
        } else {
            format!("verifier_all_checks_failed: {}", failed_list.join("; "))
        };
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(&reason, 280)),
            Some(v.clone()),
        );
    }
    // 部分通过：判定 Succeeded（主 agent 看 result_json.checks 自行决策下一步）；
    // 这里保持纯函数不写审计——非关键信号，让 caller 在 run_subagent 收尾路径里
    // 决定是否打点（partical-pass 不是错误，重复审计反而拖累日志）。
    (SubagentStatus::Succeeded, None, Some(v.clone()))
}

/// 任务包装（设计 §8.5）：orchestrator 从 subagents 行渲染，不接受 LLM 自由拼接。
/// 目标 / 验收标准 / 上下文摘要 / 预算全部来自行上字段。
pub(crate) fn render_task_wrapper(
    row: &SubagentRow,
    budget: &SubagentBudget,
    artifact_dir: &std::path::Path,
) -> String {
    let criteria: Vec<String> = acceptance_criteria_of(row);
    let mut w = String::new();
    w.push_str("[子任务派发]\n");
    w.push_str(&format!("subagent_id: {}\n", row.id));
    w.push_str(&format!("objective: {}\n", row.objective));
    w.push_str("\nacceptance_criteria:\n");
    for c in &criteria {
        w.push_str(&format!("- {c}\n"));
    }
    if let Some(ctx) = row
        .context_summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        w.push_str("\n上下文摘要:\n");
        w.push_str(ctx);
        w.push('\n');
    }
    w.push_str(&format!(
        "\n预算:\nmax_turns={}, max_tool_calls={}, max_wall_seconds={}\n",
        budget.max_turns, budget.max_tool_calls, budget.max_wall_seconds
    ));
    w.push_str(&format!("\n产物目录:\n{}\n", artifact_dir.display()));
    w.push_str("\n完成后必须输出以下 JSON（confidence 取 0.0~1.0，按实际把握给值）：\n");
    w.push_str(crate::prompts::RESULT_SCHEMA_HINT);
    w
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
fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max).collect();
    out.push('…');
    out
}

/// 回报段的渲染上限：一次注入最多这几条 / 这么多字。
///
/// 上限不是洁癖：父会话的历史预算是共享的，一次注入十条长摘要就能把本轮
/// 对话挤掉。宁可少注入几条（下一轮还会补），也不要撑爆当轮上下文。
const DIGEST_MAX_ITEMS: usize = 8;
const DIGEST_MAX_SUMMARY_CHARS: usize = 600;
const DIGEST_MAX_ARTIFACTS: usize = 5;

/// 一段渲染好的子任务回报 + 本次注入后应记为「父上下文已吸收」的子 agent id。
pub(crate) struct SubagentDigest {
    pub text: String,
    pub absorb: Vec<String>,
}

/// 父侧装配（设计 §3.7「下一轮 assemble 检查」的落点）。
///
/// 口径三条，都是为了「齐了/没齐」在模型侧都看得见：
/// 1. **只注 `result_json`，绝不注子会话历史**——子 agent 的完整对话留在它自己
///    的会话行里，父上下文只吸收结构化收尾。这是 §3.7 要点 3 的硬要求。
/// 2. **终态即吸收**：终态行注入后记入 `absorb`，下一轮不再重复念同一份结果。
/// 3. **非终态不吸收**：还在跑的行下轮还要看它有没有完成，所以只报一句计数，
///    不逐条铺开。
///
/// 返回 None = 没有新东西可注入（该父会话的子任务都已汇报过，或一个都没有），
/// 调用方据此不加这段 system 消息。
pub(crate) fn build_subagent_digest(
    rows: &[SubagentRow],
    absorbed: &std::collections::HashSet<String>,
) -> Option<SubagentDigest> {
    let mut items = String::new();
    let mut absorb = Vec::new();
    let mut running = 0usize;
    let mut listed = 0usize;

    for row in rows {
        if absorbed.contains(&row.id) {
            continue;
        }
        let objective = clip(&row.objective, 40);
        if !row.status.is_terminal() {
            // 在途不逐条列：铺开只会挤占上下文，模型只需要知道「还有几个没回」
            running += 1;
            continue;
        }
        if listed >= DIGEST_MAX_ITEMS {
            continue;
        }
        listed += 1;
        absorb.push(row.id.clone());
        match row.status {
            SubagentStatus::Succeeded => {
                items.push_str(&format!("{listed}. {objective} → 成功\n"));
                if let Some(r) = row.result_json.as_deref().and_then(parse_result_json) {
                    if let Some(summary) = r
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        items.push_str(&format!(
                            "   摘要：{}\n",
                            clip(summary, DIGEST_MAX_SUMMARY_CHARS)
                        ));
                    }
                    if let Some(list) = r.get("artifacts").and_then(|v| v.as_array()) {
                        let shown: Vec<String> = list
                            .iter()
                            .take(DIGEST_MAX_ARTIFACTS)
                            .filter_map(|a| {
                                let path = a.get("path").and_then(|v| v.as_str())?;
                                let desc = a
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty());
                                Some(match desc {
                                    Some(d) => format!("{}（{}）", clip(path, 80), clip(d, 60)),
                                    None => clip(path, 80),
                                })
                            })
                            .collect();
                        if !shown.is_empty() {
                            items.push_str(&format!("   产物：{}\n", shown.join("、")));
                        }
                    }
                }
            }
            other => {
                // 失败/取消/超预算也必须回报：父 agent 否则会一直以为子任务还在跑，
                // 按「等齐」的预期一直等下去。
                let reason = row
                    .error
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("：{}", clip(s, 200)))
                    .unwrap_or_default();
                items.push_str(&format!(
                    "{listed}. {objective} → {}{reason}\n",
                    match other {
                        SubagentStatus::Failed => "失败",
                        SubagentStatus::Cancelled => "已取消",
                        SubagentStatus::BudgetExceeded => "超预算",
                        _ => unreachable!("非终态已在上分支返回"),
                    }
                ));
            }
        }
    }

    if listed == 0 && running == 0 {
        return None;
    }
    let mut text = String::from("[子任务回报] 以下是你此前派出的子任务结果，可直接据此继续：\n");
    if listed > 0 {
        text.push_str(&items);
        text.push_str("（以上已汇报，不必再用 check_subagent 重复查。）\n");
    }
    if running > 0 {
        text.push_str(&format!(
            "另有 {running} 个子任务仍在排队或执行中，完成后会在下一轮自动汇总；\
             现在可以先基于以上结果推进，不必空等。\n"
        ));
    }
    Some(SubagentDigest { text, absorb })
}

/// 解析收尾 JSON：解析失败不当成「没有结果」，而是当没有——宁可少报一条，
/// 也不要把半截 JSON 念进提示词（`finalize` 侧已保证 result_json 结构完整，
/// 这里只是不假设它一定完整）。
fn parse_result_json(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str(raw).ok()
}

/// 父会话 → 已经汇报过的子 agent id。
///
/// **进程内、不落库**：这是一次「父上下文是否已吸收某结果」的去重标记，不是
/// 业务事实——DB 里的终态才是事实。落库要多一列 + 迁移，换来的只是重启后
/// 不重复念一遍；而重启后重复念的后果是提示词里多几段旧结果（啰嗦，不是
/// 错误），重启一次也只发生一次。这个取舍是刻意的，重启后重复汇报的口子
/// 记在设计文档 §十。
static DIGESTED: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>,
> = std::sync::OnceLock::new();

fn digested(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>
{
    DIGESTED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 锁中毒按 into_inner 取数据（全仓 mutex 口径）：去重表坏了的后果只是
/// 「可能多汇报一次」，不该让整个对话发不出请求。
fn lock_digested() -> std::sync::MutexGuard<
    'static,
    std::collections::HashMap<String, std::collections::HashSet<String>>,
> {
    crate::db::lock::lock_or_recover(digested(), "bot_orchestrator::DIGESTED")
}

/// 本次会话要注入的子任务回报段（没有新回报时 None）。
///
/// 只读自己的父会话行，查库失败/无会话一律 None——父上下文缺一段汇报照样能
/// 发请求，用户也还能用 `check_subagent` 手动查。
pub(crate) async fn subagent_digest_block(
    app: &AppHandle,
    session_id: Option<&str>,
) -> Option<String> {
    let sid = session_id?;
    let app2 = app.clone();
    let sid_owned = sid.to_string();
    let rows = tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app2).ok()?;
        crate::db::load_subagents_by_parent(&conn, &sid_owned, DIGEST_SCAN_LIMIT as u32).ok()
    })
    .await
    .ok()
    .flatten()?;

    let (text, absorb) = {
        let mut guard = lock_digested();
        let set = guard.entry(sid.to_string()).or_default();
        let built = build_subagent_digest(&rows, set)?;
        for id in &built.absorb {
            set.insert(id.clone());
        }
        (built.text, built.absorb.len())
    };
    crate::bot::audit_log(
        app,
        &format!("subagent_digest | session={sid} absorbed={absorb}"),
    );
    Some(text)
}

/// 每次会话扫多少个子 agent 行（最新在前的 LIMIT）。
///
/// 与 `DIGEST_MAX_ITEMS`（一次渲染几条）不同：前者是「看多新」，后者是
/// 「说多少」。前者要够大，免得长期挂着的排队行把新完成的结果挤出候选。
const DIGEST_SCAN_LIMIT: usize = 30;

/// 机器可判定的验收谓词（相对子 agent 产物目录解析）：
/// - `file_exists:<相对路径>`：该文件存在
/// - `file_nonempty:<相对路径>`：该文件存在且非空
///
/// 为什么要有这一层：verifier 收尾里的 `met` 是**自述**，服务端无从核实——
/// 模型写 `met:true` 就显示通过。而「产物文件到底在不在」这类标准恰好能机械
/// 判定。不带前缀的条目维持原样由 verifier 自评（很多标准本来就得看内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MachineCheck {
    FileExists(String),
    FileNonEmpty(String),
}

/// 谓词前缀 → 判定种类
const MACHINE_PREFIXES: &[(&str, u8)] = &[("file_exists:", 0), ("file_nonempty:", 1)];

/// 相对路径闸：绝对路径、盘符、`..`/`.` 分量一律不解析——判定路径若能逃出
/// 产物目录，「验收」就成了任意文件存在性探测器。
pub(crate) fn safe_rel_path(raw: &str) -> Option<&str> {
    let p = raw.trim();
    if p.is_empty() || p.starts_with('/') || p.starts_with('\\') {
        return None;
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        return None; // C:\ / C:/ 盘符
    }
    if p.split(['/', '\\'])
        .any(|c| c == ".." || c == "." || c.is_empty())
    {
        return None;
    }
    Some(p)
}

/// 解析一条验收标准里的机器可判定谓词；None = 自由文本条目（交给 verifier 自评）
pub(crate) fn parse_machine_criterion(c: &str) -> Option<MachineCheck> {
    let t = c.trim();
    for (prefix, kind) in MACHINE_PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rel = safe_rel_path(rest)?;
            return Some(match kind {
                0 => MachineCheck::FileExists(rel.to_string()),
                _ => MachineCheck::FileNonEmpty(rel.to_string()),
            });
        }
    }
    None
}

/// 单条机器判定结论
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineVerdict {
    /// 原样保留已声明的 criteria 文本（回填/比对用）
    pub criterion: String,
    pub met: bool,
    pub reason: String,
}

/// 逐条执行机器可判定谓词（纯 FS 读，不写）。
/// 路径不合法（写了谓词但路径逃逸）判 false 并说明——fail-closed：
/// 宁可记未通过，也不能把一条不可核验的标准当通过。
pub(crate) fn eval_machine_criteria(
    criteria: &[String],
    dir: &std::path::Path,
) -> Vec<MachineVerdict> {
    let mut out = Vec::new();
    for c in criteria {
        let t = c.trim();
        let Some(check) = parse_machine_criterion(t) else {
            continue;
        };
        let (rel, want_nonempty) = match &check {
            MachineCheck::FileExists(r) => (r.as_str(), false),
            MachineCheck::FileNonEmpty(r) => (r.as_str(), true),
        };
        let path = dir.join(rel);
        let (met, reason) = match std::fs::metadata(&path) {
            Err(e) => (false, format!("产物不存在：{}", e.kind())),
            Ok(md) => {
                if md.is_dir() {
                    (false, "路径是目录，不是产物文件".to_string())
                } else if want_nonempty && md.len() == 0 {
                    (false, "产物存在但为空文件".to_string())
                } else {
                    (
                        true,
                        format!("服务端核验：{} 存在（{} 字节）", rel, md.len()),
                    )
                }
            }
        };
        out.push(MachineVerdict {
            criterion: t.to_string(),
            met,
            reason,
        });
    }
    out
}

/// 把机器判定并入收尾 JSON：以服务端结论覆盖 verifier 的自评，
/// 找不到对应条目就补一条（不依赖模型主动上报这些条目）。
pub(crate) fn merge_machine_verdicts(result: &mut serde_json::Value, verdicts: &[MachineVerdict]) {
    if verdicts.is_empty() {
        return;
    }
    let key = if result.get("checks").is_some() {
        "checks"
    } else {
        "acceptance_check"
    };
    let arr = result
        .as_object_mut()
        .map(|o| o.entry(key).or_insert_with(|| serde_json::json!([])));
    let Some(arr) = arr else { return };
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let arr = arr.as_array_mut().expect("上一步已归一成数组");
    for v in verdicts {
        let mut found = false;
        for item in arr.iter_mut() {
            let same = item
                .get("criterion")
                .and_then(|c| c.as_str())
                .map(|s| s.trim())
                == Some(v.criterion.as_str());
            if same {
                item["met"] = serde_json::Value::Bool(v.met);
                item["machine"] = serde_json::Value::Bool(true);
                item["machine_reason"] = serde_json::Value::String(v.reason.clone());
                found = true;
                break;
            }
        }
        if !found {
            arr.push(serde_json::json!({
                "criterion": v.criterion,
                "met": v.met,
                "machine": true,
                "machine_reason": v.reason,
            }));
        }
    }
}

/// 进度统计的交叉核对：只认**已声明过**的验收标准，且每条最多计一次。
/// 少了这层，模型报一条没派发过的 `met:true` 就能把 done 灌满
/// （min(total) 只是封顶，不挡虚报）。
fn count_met_against_declared(declared: &[String], reported: &[serde_json::Value]) -> usize {
    let mut hit: Vec<&str> = Vec::new();
    for item in reported {
        if item.get("met").and_then(|m| m.as_bool()) != Some(true) {
            continue;
        }
        let Some(c) = item
            .get("criterion")
            .and_then(|c| c.as_str())
            .map(str::trim)
        else {
            continue;
        };
        if declared.iter().any(|d| d.trim() == c) && !hit.contains(&c) {
            hit.push(c);
        }
    }
    hit.len()
}

/// 机器判定并入收尾产物（纯函数，单测锚点）：改写收尾 JSON 的验收结论，
/// 并在最终正文末尾点名未通过项——正文是模型与用户唯一同时看得到的地方，
/// 只写进 result_json 等于把结论藏起来。
pub(crate) fn apply_machine_verdicts(
    result: Option<serde_json::Value>,
    final_text: String,
    verdicts: &[MachineVerdict],
) -> (Option<serde_json::Value>, String) {
    if verdicts.is_empty() {
        return (result, final_text);
    }
    let mut result = result;
    if let Some(v) = result.as_mut() {
        merge_machine_verdicts(v, verdicts);
    }
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| !v.met)
        .map(|v| format!("{}（{}）", v.criterion, v.reason))
        .collect();
    if failed.is_empty() {
        return (result, final_text);
    }
    let mut t = final_text;
    t.push_str(&format!(
        "\n\n⚠️ 服务端机器验收未通过 {} 项：{}",
        failed.len(),
        failed.join("；")
    ));
    (result, t)
}

/// 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/。
pub(crate) fn artifact_dir_of(gen_root: &std::path::Path, subagent_id: &str) -> std::path::PathBuf {
    gen_root.join("subagents").join(subagent_id)
}

// 并发闸per-session 2 / global 3 FIFO）

/// 并发上限（设计 §6 已）：全局 3、每个派发主会话 2。超限排队不失败
///（status=queued），有槽位唤醒。
pub const MAX_GLOBAL_RUNNING: usize = 3;
pub const MAX_PER_SESSION_RUNNING: usize = 2;

fn gate() -> &'static SubagentGate {
    static GATE: std::sync::OnceLock<SubagentGate> = std::sync::OnceLock::new();
    GATE.get_or_init(|| SubagentGate {
        state: std::sync::Mutex::new(GateState {
            global_running: 0,
            per_session: HashMap::new(),
            waiting: HashMap::new(),
        }),
        notify: tokio::sync::Notify::new(),
    })
}

/// 进程级并发闸。FIFO 口径勘误 采纳后的简化）：
/// 持久队列 + notified().await 在 future 被 drop 时会泄漏队列项（取消不安全），
/// 改为「RAII 等待计数 + 轮询/唤醒重试」——同会话内先到先得（严格），
/// 跨会话近似公平（3 槽争用面极小，A 期接受）。
pub(crate) struct SubagentGate {
    state: std::sync::Mutex<GateState>,
    notify: tokio::sync::Notify,
}

struct GateState {
    global_running: usize,
    /// running 计数：parent_session → 数
    per_session: HashMap<String, usize>,
    /// 等待计数：parent_session → 数（WaitingGuard Drop 递减，取消安全）
    waiting: HashMap<String, usize>,
}

/// 排队票据：Drop 释放槽位 + 递减等待计数（全部 RAII，无泄漏面）。
pub(crate) struct GateTicket {
    parent_session: Option<String>,
}

impl Drop for GateTicket {
    fn drop(&mut self) {
        let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
        st.global_running = st.global_running.saturating_sub(1);
        if let Some(k) = &self.parent_session {
            if let Some(c) = st.per_session.get_mut(k) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    st.per_session.remove(k);
                }
            }
        }
        drop(st);
        gate().notify.notify_one();
    }
}

/// 等待席位：Drop 递减 waiting 计数（waiter future 被 drop 也安全）。
struct WaitingGuard {
    parent_session: Option<String>,
}

impl Drop for WaitingGuard {
    fn drop(&mut self) {
        if let Some(k) = &self.parent_session {
            let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
            if let Some(c) = st.waiting.get_mut(k) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    st.waiting.remove(k);
                }
            }
        }
    }
}

impl SubagentGate {
    /// 可取消排队——每轮 50ms 退避后调 `should_abort`，true 即退队返回
    /// None（waiting 计数由 WaitingGuard RAII 递减）。取消判定由调用方给
    ///（runner 查 DB 行终态），闸门本身不依赖存储。
    pub(crate) async fn wait_slot_cancellable(
        parent_session: Option<String>,
        mut should_abort: impl FnMut() -> bool,
    ) -> Option<GateTicket> {
        // 登记等待席位（RAII：无论从哪条路径退出，计数都会被递减）
        {
            let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
            if let Some(k) = &parent_session {
                *st.waiting.entry(k.clone()).or_insert(0) += 1;
            }
        }
        let _waiting = WaitingGuard {
            parent_session: parent_session.clone(),
        };
        loop {
            if should_abort() {
                return None;
            }
            let outcome = {
                let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
                let global_ok = st.global_running < MAX_GLOBAL_RUNNING;
                let sess_running = parent_session
                    .as_deref()
                    .and_then(|k| st.per_session.get(k).copied())
                    .unwrap_or(0);
                let sess_ok = sess_running < MAX_PER_SESSION_RUNNING;
                // 不做「等待序数」公平判定（waiting<=1 的判定
                // 在两个并发 waiter 同见 waiting=2 时会互相让行 → 集体饿死）。
                // 改为纯「试占」：有空槽即取，50ms 重试保证无饥饿。
                // FIFO 降级为 best-effort（waiting 计数仅供 check 展示排队位），
                // 勘误登记于批 spec。
                if global_ok && sess_ok {
                    st.global_running += 1;
                    if let Some(k) = &parent_session {
                        *st.per_session.entry(k.to_string()).or_insert(0) += 1;
                    }
                    Some(true)
                } else {
                    None
                }
            };
            if outcome == Some(true) {
                return Some(GateTicket { parent_session });
            }
            // 没轮到：等唤醒或 50ms 退避重试（无持久队列项，取消安全）
            gate().notify.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// 排队位次（check progress 用）：同会话前面还有几个等待者；
    /// None = 不在等待（运行中或无关会话）。
    pub(crate) fn queue_position(parent_session: Option<&str>) -> Option<usize> {
        let st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
        let n = parent_session
            .and_then(|k| st.waiting.get(k).copied())
            .unwrap_or(0);
        (n > 0).then(|| n - 1)
    }
}

// runner（执行引擎）

/// 子 agent 执行引擎：独立会话 + 工具循环 + 收尾落库。spawn 异步包装末尾派发
/// （非阻塞）。执行基建复用 exec 家族（ChatGuard/ExecGuard/StopGuard/tool_guard），
/// 但入口/存储/事件与 bot_execute_task 各自独立（设计 §2）。
/// 具体化 `tauri::AppHandle`（= Wry）：run_model_loop 生产壳即该签名
/// （mock 链路走 run_model_loop_core，同 llm_integration 分层）。
#[allow(clippy::too_many_lines)]
async fn run_subagent(
    app: AppHandle,
    subagent_id: String,
    stage: Option<SubagentStage>,
    onion: Option<SpawnOnion>,
) {
    let sid_for_load = subagent_id.clone();
    let row = match db_locked(&app, move |conn| {
        crate::db::load_subagent(conn, &sid_for_load)
            .map_err(CommandError::DbError)?
            .ok_or(CommandError::TaskNotFound(sid_for_load))
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // 加载失败也要落终态，防 ghost queued 行永挂
            let ghost_sid = subagent_id.clone();
            let ghost_err = e.message();
            let _ = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &ghost_sid,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&ghost_err),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: {}",
                    subagent_id,
                    e.message()
                ),
            );
            // parent 会话未知（行没读到），发不带归属的收尾事件——前端只重拉不弹提示
            emit_subagent_finished(&app, &subagent_id, None, SubagentStatus::Failed, "");
            return;
        }
    };
    // 派发后、开跑前已被取消/终态 → 不启动。行已终态但事件没人发过（cancel
    // 包装不 emit），这里补上，前端才不会把该行停在 queued
    if row.status != SubagentStatus::Queued {
        emit_subagent_finished(
            &app,
            &row.id,
            row.parent_session_id.as_deref(),
            row.status,
            "",
        );
        return;
    }
    let budget: SubagentBudget = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // 并发闸global 3 / per-session 2，超限等待不失败）。
    // 排队时间不计墙钟：max_wall_seconds 的 timeout 从 run_model_loop 起算（下方），
    // 天然满足设计 §6。
    // 排队可取消——每轮退避同时查行终态，取消即退队；Running 改在**拿到槽位并复核后**
    // 才写，排队中的行不再假挂运行中。
    let sid_gate_check = row.id.clone();
    let app_gate_check = app.clone();
    // 评审 M 采纳：节流 DB 读——open_db 含建目录+pragmas 文件 IO，不该 50ms
    // 一次压在 async worker 上；取消检测 500ms 粒度足够（槽后复核兜底）
    let gate_tick = std::cell::Cell::new(0u32);
    let mut cancelled_while_queued = move || {
        let n = gate_tick.get();
        gate_tick.set(n.wrapping_add(1));
        if !n.is_multiple_of(10) {
            return false;
        }
        crate::db::open_db(&app_gate_check)
            .ok()
            .and_then(|conn| {
                crate::db::load_subagent(&conn, &sid_gate_check)
                    .ok()
                    .flatten()
            })
            .map(|r| r.status.is_terminal())
            .unwrap_or(false) // DB 读失败不误判取消（拿到槽位后的复核兜底）
    };
    let ticket = crate::bot_orchestrator::SubagentGate::wait_slot_cancellable(
        row.parent_session_id.clone(),
        &mut cancelled_while_queued,
    )
    .await;
    let _ticket = match ticket {
        Some(t) => t,
        // 排队中被取消：行已被 cancel 写成终态，此处仅退出（守卫 RAII 清理）
        None => {
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: cancelled_while_queued",
                    row.id
                ),
            );
            emit_subagent_finished(
                &app,
                &row.id,
                row.parent_session_id.as_deref(),
                SubagentStatus::Cancelled,
                "",
            );
            return;
        }
    };
    {
        // 拿到槽位后复核（cancel-vs-runner 竞态闸）：行已终态 = cancel 先赢 →
        // 中止；DB 读失败保守中止（执行已取消的任务比不跑更糟
        let sid_still_queued = row.id.clone();
        let state_now = db_locked(&app, move |conn| {
            Ok(crate::db::load_subagent(conn, &sid_still_queued)
                .ok()
                .flatten()
                .map(|r| r.status))
        })
        .await
        .unwrap_or(None);
        match state_now {
            Some(s) if !s.is_terminal() => {}
            other => {
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "subagent_runner_abort | id: {} | err: aborted_while_queued | state: {}",
                        row.id,
                        other
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "db_read_failed".into()),
                    ),
                );
                // 拿到槽位后的复核撞上终态：行已由 cancel 写成 cancelled；
                // 读不到行（DB 失败/行已没）时不要谎报——只留审计，等启动清扫
                if let Some(s) = other {
                    emit_subagent_finished(&app, &row.id, row.parent_session_id.as_deref(), s, "");
                }
                return;
            }
        }
        // Running 写库挪到槽位拿到之后（排队窗口内行保持 queued 语义）。
        // 写失败必须中止（评审 HIGH 采纳）：recheck 与本写之间 cancel 可抢先写
        // Cancelled，update_subagent_status 因非法状态转移报错——吞错等于在
        // 已取消任务上继续跑
        let sid_running = row.id.clone();
        let run_write = db_locked(&app, move |conn| {
            crate::db::update_subagent_status(
                conn,
                &sid_running,
                SubagentStatus::Running,
                now_ms(),
                None,
            )
            .map_err(CommandError::DbError)
        })
        .await;
        if let Err(e) = run_write {
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: running_write_failed | {e}",
                    row.id
                ),
            );
            // 写 Running 被拒基本只有一种成因：cancel 已把行写成终态。回读
            // 真实终态上报（直接写 Cancelled 会在「DB 抖动」这类非 cancel
            // 失败上撒谎）；读不到行就只留审计，交给启动清扫收尸
            let st = load_subagent_status(&app, &row.id).await;
            if let Some(st) = st {
                emit_subagent_finished(&app, &row.id, row.parent_session_id.as_deref(), st, "");
            }
            return;
        }
    }
    let started = std::time::Instant::now();

    // 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/
    // gen_dir 解析失败必须 fail-closed：此前退到 "__gen_dir__" 相对路径兜底，
    // 会把产物目录钉到进程 CWD（打包应用 CWD 可能是 /），产物越过配置的隔离根
    #[allow(unused_mut)]
    let mut artifact_dir = match crate::db::gen_dir(&app) {
        Ok(g) => artifact_dir_of(&g, &row.id),
        Err(e) => {
            let msg = format!("产物目录不可用（{e}），子任务中止");
            let sid_failed = row.id.clone();
            let _ = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &sid_failed,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&msg),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: gen_dir_failed | {e}",
                    row.id
                ),
            );
            emit_subagent_finished(
                &app,
                &row.id,
                row.parent_session_id.as_deref(),
                SubagentStatus::Failed,
                "",
            );
            return;
        }
    };
    // create + canonicalize（写路径钉死规范化目录）。失败仅审计：
    // write_artifact_file 每次调用会再校验，目录坏了只影响产物不影响状态机
    let mk_dir = artifact_dir.clone();
    let mk_res = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        std::fs::create_dir_all(&mk_dir).map_err(|e| e.to_string())?;
        std::fs::canonicalize(&mk_dir).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match mk_res {
        Ok(c) => {
            // 覆盖为规范化路径（registry ctx 与包装渲染都用它）
            artifact_dir = c;
        }
        Err(e) => {
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_artifact_dir_warn | id: {} | 产物目录创建/规范化失败（{e}），产物写将被拒绝",
                    row.id
                ),
            );
        }
    }

    // 开跑：建会话 + 包装落库 + session/assignee 回填（单次锁内完成）。
    // 标题不带 emoji 前缀：子 agent 身份由 bot_sessions.is_subagent 结构化
    // 字段承载（U20D 批 5），前端按 Session.isSubagent 路由停止键
    let title = format!(
        "子任务：{}",
        crate::bot::truncate_for_log(row.objective.trim(), 30)
    );
    let setup = {
        let artifact_dir = artifact_dir.clone();
        let row = row.clone();
        let budget = budget.clone();
        let onion = onion.clone();
        db_locked(&app, move |conn| -> CommandResult<(String, String)> {
            // 建会话与回填 session_id 包同一事务，防出现「建了会话但行没记」的孤儿会话
            let tx = conn
                .transaction()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            let session = crate::db::bot_session_create_subagent_inner(&tx, Some(title.clone()))
                .map_err(CommandError::DbError)?;
            // 包装文本仅返回（渲染一次，交 runner 供 LLM 与 finalize 复用）——
            // 历史落库统一在 finalize 全量覆盖写（setup 的
            // bot_history_save_inner 是半截写，会被 finalize DELETE+INSERT 覆盖，
            // 中途崩溃还留「只有任务块没有结论」的误导历史）
            let wrapper = render_task_wrapper(&row, &budget, &artifact_dir);
            crate::db::set_subagent_session(&tx, &row.id, &session.id)
                .map_err(CommandError::DbError)?;
            // 配置档元列随会话一起建：配置正文在 `<数据目录>/onions/`，这里只存指针。
            // 不写的话 bind_session 读不到，子 agent 会拿到未收窄的整套 profile 工具。
            if let Some(o) = &onion {
                crate::db::bot_session_set_onion(
                    &tx,
                    &session.id,
                    Some(&o.config),
                    o.max_turn.map(|t| t as i64),
                )
                .map_err(CommandError::DbError)?;
            }
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok((session.id, wrapper))
        })
    }
    .await;
    let (session_id, wrapper) = match setup {
        Ok(v) => v,
        Err(e) => {
            let msg = e.message();
            let msg_for_db = msg.clone();
            let sid = row.id.clone();
            let _ = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &sid,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&msg_for_db),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: session_setup_failed | {msg}",
                    row.id
                ),
            );
            emit_subagent_finished(
                &app,
                &row.id,
                row.parent_session_id.as_deref(),
                SubagentStatus::Failed,
                "",
            );
            return;
        }
    };

    // per-onion 配置绑定（两层配置的 DB 层）：读会话的 onion_config 元列 + 落盘
    // yaml，解析校验后进进程内缓存。**必须在守卫注册之前**——守卫要拿
    // 「profile ∩ allowed」的交集作为白名单判据，配置没绑就没交集可算。
    crate::bot_onions::bind_session(&app, &session_id);
    // 白名单全集：配置档在就取交集，不在就是 profile 白名单原样（语义不变）
    let subagent_tools: Vec<&'static str> =
        match crate::bot_onions::cached_for_session(Some(session_id.as_str())) {
            Some(cfg) => crate::bot_onions::effective_tools(row.profile, &cfg.allowed),
            None => crate::bot::registry::profile_whitelist(row.profile).to_vec(),
        };

    // 工具守卫注册（白名单闸 + spawn 递归身份校验 + read_own_card/write_artifact 定位）。
    // RAII 守卫：Drop 反注册——runner 中途 panic 也不泄漏条目（ 采纳）
    let _session_guard = crate::tool_guard::SubagentSessionGuard::new(
        &session_id,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: row.id.clone(),
            task_id: row.task_id.clone(),
            profile: row.profile,
            tools: std::sync::Arc::new(subagent_tools),
            budget: budget.clone(),
            artifact_dir: artifact_dir.clone(),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        },
    );

    // 会话锁（用户向围观会话发消息 → 友好拒绝）+ 停止守卫（interactive=围观流式）
    let chat_guard = ChatGuard::acquire(&app, Some(session_id.as_str()));
    if chat_guard.is_err() {
        // 此处行已是 Running——中止必须落终态，不能悬挂
        let sid_failed = row.id.clone();
        if let Err(e) = db_locked(&app, move |conn| {
            crate::db::update_subagent_status(
                conn,
                &sid_failed,
                SubagentStatus::Failed,
                now_ms(),
                Some("session_busy"),
            )
            .map_err(CommandError::DbError)
        })
        .await
        {
            // 回滚留痕：行已 Cancelled 时 update_subagent_status 会拒绝置 Failed，
            // 终态由 watcher 兜底——失败可见非静默（围观/审计设计保留会话行）
            eprintln!(
                "[subagent] session_busy 中止回滚失败（subagent: {}）：{e}",
                row.id
            );
        }
        crate::bot::audit_log(
            &app,
            &format!("subagent_runner_abort | id: {} | err: session_busy", row.id),
        );
        emit_subagent_finished(
            &app,
            &row.id,
            row.parent_session_id.as_deref(),
            SubagentStatus::Failed,
            "",
        );
        return;
    }
    let stop = crate::bot_slash::StopGuard::new(&app, true, Some(session_id.clone()));
    // 取消令牌登记：cancel_subagent 持句柄远程停止。已知窗口（可接受：
    // ChatGuard 成功到令牌登记之间收到的 cancel 落不到令牌——由 2s watcher 兜底
    // （行已 cancelled → watcher 置 stop）。
    crate::db::lock::lock_or_recover(
        &crate::app_state::subagent_stops(&app),
        "app_state::subagent_stops",
    )
    .insert(row.id.clone(), stop.token());

    // 取消 watcher（设计 §4.3 硬停）：2s 轮询行状态，进终态即停工具循环。
    // 子 agent 无卡，不再有「软删卡 = 取消派发」这条信号——cancel 置行终态，
    // 这是唯一的外部取消入口
    let watcher_app = app.clone();
    let watcher_stop = stop.token();
    let watcher_sid = row.id.clone();
    struct AbortOnDrop(tauri::async_runtime::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let watcher = tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if watcher_stop.stopped() {
                return;
            }
            let sid = watcher_sid.clone();
            // 只读轮询：直接开连接，不持 DB_WRITE_LOCK（防多子 agent 写锁互饿）
            let watcher_app2 = watcher_app.clone();
            let aborted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
                let conn = crate::db::open_db(&watcher_app2)?;
                Ok(crate::db::load_subagent(&conn, &sid)
                    .ok()
                    .flatten()
                    .map(|r| r.status.is_terminal())
                    .unwrap_or(true))
            })
            .await
            .unwrap_or(Ok(false))
            .unwrap_or(false);
            if aborted {
                watcher_stop.stop();
                return;
            }
        }
    });

    // 消息装配：system = 子 agent 通用提示 + profile 段 + 产物目录规则；user = 任务包装
    // stage 在场：拼 PROFILE_RECON/EXECUTOR/VERIFIER 替代基线 PROFILE_*（设计 §8.5）。
    // profile 仍决定工具白名单档——stage 只换角色语义层。两层解耦，允许调用方：
    // 1) 仅给 stage（profile 自动按 default_profile 派生）；或
    // 2) 同时给 stage + profile（白名单与角色互不干扰，如 verifier=research 的只读够用）。
    let profile_prompt = match stage {
        Some(SubagentStage::Recon) => crate::prompts::PROFILE_RECON,
        Some(SubagentStage::Executor) => crate::prompts::PROFILE_EXECUTOR,
        Some(SubagentStage::Verifier) => crate::prompts::PROFILE_VERIFIER,
        None => match row.profile {
            SubagentProfile::Research => crate::prompts::PROFILE_RESEARCH,
            SubagentProfile::Coder => crate::prompts::PROFILE_CODER,
            SubagentProfile::General => crate::prompts::PROFILE_GENERAL,
        },
    };
    // wrapper 已由 setup 渲染返回（同一文本：LLM 输入 = 历史落库 = 收尾覆盖源）
    // 配置档段：角色 + 交集后的实际可用工具（§3.4「配置看着生效、实际落空」
    // 要让模型也看得见）；无配置档时是空串，不占位也不改既有提示词。
    let bound_onion = crate::bot_onions::cached_for_session(Some(session_id.as_str()));
    let onion_note = bound_onion
        .as_ref()
        .map(|cfg| {
            format!(
                "\n\n{}",
                crate::bot_onions::render_prompt_note(cfg, row.profile)
            )
        })
        .unwrap_or_default();
    let system = format!(
        "{}\n\n{}\n\n{}{}",
        crate::prompts::SUBAGENT_BASE,
        profile_prompt,
        gen_dir_rule_for(&artifact_dir),
        onion_note,
    );
    let msgs = vec![
        serde_json::json!({ "role": "system", "content": system }),
        serde_json::json!({ "role": "user", "content": wrapper }),
    ];

    // 工具循环：轮数预算 + 墙钟预算（超时硬停→budget_exceeded；排队不计——running 起算）
    let run = crate::bot_model_loop::run_model_loop(
        app.clone(),
        msgs,
        budget.max_turns as usize,
        &stop,
        None,
        None,
        // 子 agent 模型走自身预算/派发通道（model_profile），不挂每卡覆盖
        None,
        // 配置档的采样温度（f32 → f64 无损）。这是 yaml `temperature:` 字段的
        // 唯一消费者；None 时用模型条目自带的 temperature。
        bound_onion
            .as_ref()
            .and_then(|c| c.temperature)
            .map(f64::from),
    );
    let wall = tokio::time::timeout(
        std::time::Duration::from_secs(budget.max_wall_seconds.max(1)),
        run,
    )
    .await;

    // 墙钟触达先 stop 后收尾——run future 被 timeout 掐掉时，脱离 runtime
    // 的在飞 Python/MCP（spawn_blocking 不随 future 取消）靠 StopToken 自行退出；
    // 短 grace 给它们的退出清理/Drop 留时间，不留孤儿进程
    if wall.is_err() {
        stop.force_stop();
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }

    // 收尾清理：watcher 由 AbortOnDrop Drop 保证中止（含 panic/unwind 路径——
    // StopGuard::drop 不置位停止标志，单靠手动 abort 会泄漏）
    let _watcher = AbortOnDrop(watcher);
    // tool_guard 注册走 RAII 守卫（_session_guard Drop 反注册）
    crate::db::lock::lock_or_recover(
        &crate::app_state::subagent_stops(&app),
        "app_state::subagent_stops",
    )
    .remove(&row.id);
    drop(chat_guard);

    // 收尾判定（设计 §6/§7）
    let (status, error, result) = match &wall {
        Err(_elapsed) => classify_outcome(None, true, None),
        Ok(Err(e)) => classify_outcome(Some(&e.message()), false, None),
        Ok(Ok((text, _refs, _loop_trace))) => classify_outcome(None, false, Some(text)),
    };
    // 轮数熔断现在也走 Ok 出口（正文带 ROUNDS_FUSE_MARKER），机器验收必须把它
    // 排除掉：熔断行没有收尾 JSON，等于没交答卷，拿产物文件判它「未通过」只会
    // 在正文里凭空多出一段子 agent 没说过的话。
    let fused_on_rounds = matches!(&wall, Ok(Ok((text, _, _)))
        if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER));
    let final_text: String = match &wall {
        Ok(Ok((text, _, _))) => text.clone(),
        Ok(Err(e)) => format!("⚠️ 执行失败：{}", e.message()),
        Err(_) => "⏹ 墙钟预算触达，执行被强制停止（部分产物已保留）".into(),
    };
    // 机器可判定验收：verifier 收尾里的 met 是自述，服务端对「产物文件在不在」
    // 这类标准能自己判——判了才覆盖自评，并在正文里点名未通过项。
    // 只在跑完（有收尾 JSON）时判：预算触顶/异常退出的行没有验收可言。
    let machine_verdicts = if matches!(wall, Ok(Ok(_))) && !fused_on_rounds {
        eval_machine_criteria(&acceptance_criteria_of(&row), &artifact_dir)
    } else {
        Vec::new()
    };
    let failed_machine = machine_verdicts.iter().filter(|v| !v.met).count();
    if failed_machine > 0 {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Warn,
            "subagent.machine_check_failed",
            "subagent_id" => row.id.clone(),
            "failed" => failed_machine.to_string(),
            "total" => machine_verdicts.len().to_string(),
        );
    } else if !machine_verdicts.is_empty() {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "subagent.machine_check_passed",
            "subagent_id" => row.id.clone(),
            "total" => machine_verdicts.len().to_string(),
        );
    }
    // 收尾 JSON 与最终正文都带上机器结论——正文这行是模型和用户唯一同时
    // 看得到的地方，不能只躺在 result_json 里等人去查
    let (result, final_text) = apply_machine_verdicts(result, final_text, &machine_verdicts);
    let wrapper_for_db = wrapper.clone();
    let row_for_db = row.clone();
    let session_id_for_db = session_id.clone();
    let status_for_db = status;
    let result_for_db = result.clone();
    let error_for_db = error.clone();
    let final_for_db = final_text.clone();
    // 返回 fresh row——下游审计/事件直接用，免二次持锁查询
    let finalized: Option<SubagentRow> =
        db_locked(&app, move |conn| -> CommandResult<Option<SubagentRow>> {
            // 多写包同一事务 + cancel-vs-runner 竞态闸——
            // 行已终态（cancel 先赢）时不做任何状态/结果副作用，只落历史
            let current = crate::db::load_subagent(conn, &row_for_db.id)
                .map_err(CommandError::DbError)?
                .ok_or_else(|| CommandError::TaskNotFound(row_for_db.id.clone()))?;
            let already_terminal = current.status.is_terminal();
            // 历史落库（任务包装 + 收尾文本）无论终态与否都执行（围观/取证可见）
            crate::db::bot_history_save_inner(
                conn,
                &session_id_for_db,
                &[
                    crate::db::BotMsgRow {
                        role: "user".into(),
                        content: wrapper_for_db,
                        refs_json: None,
                        thinking: None,
                        tools_json: None,
                    },
                    crate::db::BotMsgRow {
                        role: "assistant".into(),
                        content: final_for_db,
                        refs_json: None,
                        thinking: None,
                        tools_json: None,
                    },
                ],
            )
            .map_err(CommandError::DbError)?;
            if already_terminal {
                // 终态已被 cancel 抢先写入：不做任何状态/结果副作用，但把行本身
                // 带回去——下游事件与教训要用行的真实终态（cancelled），不能用
                // classify_outcome 的「执行被打断 → Failed」观感
                return Ok(Some(current));
            }
            let tx = conn
                .transaction()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            crate::db::update_subagent_status(
                &tx,
                &row_for_db.id,
                status_for_db,
                now_ms(),
                error_for_db.as_deref(),
            )
            .map_err(CommandError::DbError)?;
            if let Some(v) = &result_for_db {
                crate::db::set_subagent_result(&tx, &row_for_db.id, &v.to_string())
                    .map_err(CommandError::DbError)?;
            }
            let fresh = crate::db::load_subagent(&tx, &row_for_db.id)
                .map_err(CommandError::DbError)?
                .ok_or_else(|| CommandError::TaskNotFound(row_for_db.id.clone()))?;
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok(Some(fresh))
        })
        .await
        .unwrap_or_else(|e| {
            let msg = crate::bot::truncate_for_log(&e.message(), 200);
            // 收尾失败行会永挂 Running——best-effort 落 Failed
            //（终态判定在收尾闭包内已做；此处只兜住闭包本身失败的情形）。
            // 后台任务持 owned handle（spawn 要求 'static，不能借用 app）。
            let sid_failed = row.id.clone();
            let app_for_fail = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = db_locked(&app_for_fail, move |conn| {
                    crate::db::update_subagent_status(
                        conn,
                        &sid_failed,
                        SubagentStatus::Failed,
                        now_ms(),
                        Some(&msg),
                    )
                    .map_err(CommandError::DbError)
                })
                .await;
            });
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_finalize_error | id: {} | err: {}",
                    row.id,
                    crate::bot::truncate_for_log(&e.message(), 200)
                ),
            );
            None
        });

    // 审计 + 广播 + 前端事件（设计 §3 事件全链按 subagent_id 串链）
    let elapsed = started.elapsed().as_millis() as u64;
    // 行的真实终态优先于 classify_outcome：用户取消走到这里时 classify 只会给出
    // Failed（执行被 StopToken 打断），照它上报 = 每次取消都记一条 subagent_failed
    // 并沉淀「失败原因：」的空教训。收尾闭包整体失败（finalized=None）才回落到观感值
    let effective_status = effective_status(finalized.as_ref().map(|r| r.status), status);
    let event = match effective_status {
        SubagentStatus::Succeeded => "subagent_done",
        SubagentStatus::BudgetExceeded => "subagent_budget_exceeded",
        SubagentStatus::Cancelled => "subagent_cancelled",
        _ => "subagent_failed",
    };
    // fresh 直接取自收尾闭包返回值（不再二次持锁查询）
    let summary = finalized
        .as_ref()
        .and_then(|r| r.result_json.as_deref())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("summary")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let row_error = finalized
        .as_ref()
        .and_then(|r| r.error.clone())
        .unwrap_or_default();
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        event,
        &[
            ("subagent_id", row.id.clone()),
            ("task_id", row.task_id.clone().unwrap_or_default()),
            (
                "parent_session_id",
                row.parent_session_id.clone().unwrap_or_default(),
            ),
            ("profile", row.profile.as_str().to_string()),
            ("trace_id", row.trace_id.clone()),
            ("elapsed_ms", elapsed.to_string()),
            ("error", row_error.clone()),
        ],
    );
    crate::bot::audit_log(
        &app,
        &format!(
            "{} | id: {} | status: {} | elapsed_ms: {} | error: {}",
            event,
            row.id,
            effective_status.as_str(),
            elapsed,
            crate::bot::truncate_for_log(&row_error, 120)
        ),
    );
    // Failed / BudgetExceeded 自动沉淀 lesson：与 run_task_in_chat 的
    // auto_lesson_on_task_failure 同口径（source=system, scenario=task_exec,
    // tags=[lesson, task_exec]）——store 语义去重自动合并同类失败教训。
    // 走 fire-and-forget，不阻塞 finalize 返回路径。
    // Cancelled 是用户选择不沉淀，Succeeded 无失败不沉淀。
    if matches!(
        effective_status,
        SubagentStatus::Failed | SubagentStatus::BudgetExceeded
    ) {
        let app_for_lesson = app.clone();
        let lesson_title = row.objective.clone();
        let lesson_error = row_error.clone();
        tauri::async_runtime::spawn(async move {
            crate::memory::auto_lesson_on_task_failure(
                &app_for_lesson,
                &lesson_title,
                &lesson_error,
            )
            .await;
        });
    }
    emit_subagent_finished(
        &app,
        &row.id,
        row.parent_session_id.as_deref(),
        effective_status,
        summary.as_deref().unwrap_or_default(),
    );
}

/// 启动清扫：进程重启后没有任何 runner 在跑，库里 running/queued 的行是
/// 上一世的僵尸——不收尾就在子 agent 面板里永远转圈（L1 排队恢复尚未接线，
/// 这里只保证不留假在途）。逐行落终态 + 审计，谁被清的、可追溯。
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

/// 上报用终态：行已写入终态时以行为准（cancel 抢先 → cancelled），
/// 收尾闭包自身失败拿不到行时才回落 classify 的观感值
fn effective_status(row: Option<SubagentStatus>, classified: SubagentStatus) -> SubagentStatus {
    row.unwrap_or(classified)
}

/// 回读一行子 agent 的状态：读失败或行已不存在都返 None（调用方据此决定
/// 「按真实状态上报」还是「只留审计」）
async fn load_subagent_status(app: &AppHandle, id: &str) -> Option<SubagentStatus> {
    let id = id.to_string();
    db_locked(app, move |conn| {
        Ok(crate::db::load_subagent(conn, &id)
            .ok()
            .flatten()
            .map(|r| r.status))
    })
    .await
    .unwrap_or(None)
}

/// `subagent-finished` 唯一收尾出口：主会话视图据此刷新子 agent 列表 + 打完成
/// 提示。所有把行写成终态的路径都必须走到这里（含排队期取消、启动中止、
/// 行加载失败的 ghost 路径）——前端只认事件，不发就永远停在 queued
fn emit_subagent_finished(
    app: &AppHandle,
    subagent_id: &str,
    parent_session_id: Option<&str>,
    status: SubagentStatus,
    summary: &str,
) {
    let _ = app.emit_to(
        "main",
        "subagent-finished",
        serde_json::json!({
            "subagentId": subagent_id,
            "parentSessionId": parent_session_id,
            "status": status.as_str(),
            "summary": summary,
        }),
    );
}

/// 行刚建好（queued）就发一条：子 agent 列表只靠事件驱动重拉，没有这条，
/// 新行要等收尾才可见——长跑任务期间无处取消
fn emit_subagent_spawned(app: &AppHandle, subagent_id: &str, parent_session_id: Option<&str>) {
    let _ = app.emit_to(
        "main",
        "subagent-spawned",
        serde_json::json!({
            "subagentId": subagent_id,
            "parentSessionId": parent_session_id,
        }),
    );
}

// final_text 由上面的 move 闭包直接捕获（历史落库后不再外用）

/// 子 agent 系统提示的产物目录规则段（gen_dir 硬约束，同 EXECUTE 提示口径）。
fn gen_dir_rule_for(artifact_dir: &std::path::Path) -> String {
    format!(
        "## 产物目录（硬性约束）\n你的全部文件写入必须落在：{}\n目录外写入会被拒绝；报告/数据文件写进该目录并在收尾 JSON 的 artifacts 里登记路径。",
        artifact_dir.display()
    )
}

// LLM 工具实现（registry 调用）

fn parse_tool_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or(serde_json::Value::Null)
}

/// spawn_subagent 工具（递归双保险②：子 agent 会话调用 → 服务端身份校验拒绝）。
pub async fn tool_spawn_subagent(
    app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if crate::tool_guard::is_subagent_session(session_id.as_deref()) {
        return ToolResult::error(
            "子 agent 不得派发子 agent（递归禁用）。请专注完成自己的任务包装目标。",
            Vec::new(),
        );
    }
    let v = parse_tool_args(args);
    let objective = v["objective"].as_str().unwrap_or_default().to_string();
    // stage 可选：recon/executor/verifier 三段式派发阶段（设计 §8.5）。
    // profile 可选：不给时按 配置档 → stage 派生（见 resolve_spawn_profile）。
    let stage = v["stage"].as_str().and_then(SubagentStage::from_str);
    // 配置档：名字非法 / 文件不在 / YAML 语法错 / 字段不合法都在这里报错。
    // 放在建行之前——加载失败的配置档若留到 runner 才炸，就是一条永远跑不起来的行。
    let onion_cfg = match v["onion"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => match crate::bot_onions::load(app, id) {
            Ok(cfg) => Some(cfg),
            Err(e) => return ToolResult::warn(format!("spawn 失败：{e}"), Vec::new()),
        },
        None => None,
    };
    let profile = match resolve_spawn_profile(
        v["profile"].as_str().and_then(SubagentProfile::from_str),
        onion_cfg.as_ref().and_then(|c| c.profile_enum()),
        stage,
    ) {
        Some(p) => p,
        None => {
            return ToolResult::warn(
                "spawn 失败：profile 必须是 research / coder / general 之一；或给 onion=配置档名；或给 stage=recon/executor/verifier 让系统按默认派生",
                Vec::new(),
            );
        }
    };
    let criteria: Vec<String> = v["acceptanceCriteria"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if criteria.is_empty() {
        return ToolResult::warn(
            "spawn 失败：acceptanceCriteria 必填且每条可检验",
            Vec::new(),
        );
    }
    let budget = match &v["budget"] {
        serde_json::Value::Object(_) => Some(
            SubagentBudget {
                max_turns: v["budget"]["maxTurns"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_TURNS as u64) as u32,
                max_tool_calls: v["budget"]["maxToolCalls"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_TOOL_CALLS as u64)
                    as u32,
                max_wall_seconds: v["budget"]["maxWallSeconds"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_WALL_SECONDS),
            }
            .clamped(),
        ),
        _ => None,
    };
    let req = SpawnRequest {
        objective,
        profile,
        stage,
        acceptance_criteria: criteria,
        context_summary: v["contextSummary"].as_str().map(str::to_string),
        budget,
        model_profile: v["modelProfile"].as_str().map(str::to_string),
        onion: onion_cfg.as_ref().map(|c| SpawnOnion {
            config: c.onion_id.clone(),
            max_turn: c.max_turn,
        }),
        parent_task_id: v["parentTaskId"].as_str().map(str::to_string),
        parent_session_id: session_id,
    };
    match spawn_subagent(app, req).await {
        Ok(ack) => ToolResult::ok(
            format!(
                "已派发子 agent（非阻塞）：subagentId={} status={}。用 check_subagent 轮询结果，先继续回应用户。",
                ack.subagent_id,
                ack.status.as_str()
            ),
            // 无卡：不挂 TaskRef（指向已不存在的卡，前端会渲染成死链）
            Vec::new(),
        ),
        Err(e) => ToolResult::warn(format!("spawn 失败：{}", e.message()), Vec::new()),
    }
}

/// check_subagent 工具（幂等可轮询；waitMs 上限 5000）。
pub async fn tool_check_subagent(app: &AppHandle, args: &str) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let v = parse_tool_args(args);
    let key = v["subagentId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string);
    let Some(key) = key else {
        return ToolResult::warn("check 失败：subagentId 与 taskId 至少提供一个", Vec::new());
    };
    let wait_ms = v["waitMs"].as_u64().unwrap_or(0).min(CHECK_WAIT_MS_CAP);
    match check_subagent(app, &key, wait_ms).await {
        Ok(state) => ToolResult::ok(
            serde_json::to_string(&state).unwrap_or_else(|_| state.status.as_str().to_string()),
            Vec::new(),
        ),
        Err(e) => ToolResult::warn(format!("check 失败：{}", e.message()), Vec::new()),
    }
}

/// cancel_subagent 工具（与任务卡停止按钮同 API；子 agent 会话禁调）。
pub async fn tool_cancel_subagent(
    app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if crate::tool_guard::is_subagent_session(session_id.as_deref()) {
        return ToolResult::error("子 agent 不得取消其他子 agent。", Vec::new());
    }
    let v = parse_tool_args(args);
    let key = v["subagentId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string);
    let Some(key) = key else {
        return ToolResult::warn("cancel 失败：subagentId 与 taskId 至少提供一个", Vec::new());
    };
    let reason = v["reason"].as_str().unwrap_or("主 agent 取消").to_string();
    match cancel_subagent_async(app, &key, &reason).await {
        Ok(ack) => ToolResult::ok(
            if ack.already_terminal {
                format!(
                    "子 agent {} 已是终态（{}），无需取消",
                    ack.subagent_id,
                    ack.status.as_str()
                )
            } else {
                format!("已取消子 agent {}", ack.subagent_id)
            },
            Vec::new(),
        ),
        Err(e) => ToolResult::warn(format!("cancel 失败：{}", e.message()), Vec::new()),
    }
}

/// write_artifact_file 工具：仅限子 agent 会话 + 产物目录内（设计 §6 产物目录隔离）。
pub async fn tool_write_artifact_file(
    _app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("write_artifact_file 仅子 agent 执行会话可用。", Vec::new());
    };
    let v = parse_tool_args(args);
    let filename = v["filename"].as_str().unwrap_or_default().trim();
    let content = v["content"].as_str().unwrap_or_default();
    // content 上限（ 采纳）：失控子 agent 循环写大文件可填满磁盘——
    // 8MB 与 registry 长输出工具软上限同量级，报告/数据文件足够
    const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
    if content.len() > MAX_ARTIFACT_BYTES {
        return ToolResult::warn(
            format!(
                "写产物失败：content {} 字节超上限 {}（8MB）",
                content.len(),
                MAX_ARTIFACT_BYTES
            ),
            Vec::new(),
        );
    }
    // 纯文件名校验：拒绝路径分隔符、NUL（POSIX 截断/Windows 路径组件）
    // 与 Windows 保留设备名；.. 按组件判定而非子串——
    // 不误伤 foo..bar / ...txt 这类合法名
    const WIN_RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem_upper = filename
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
        || filename.chars().any(|c| c.is_control())
        || WIN_RESERVED.contains(&stem_upper.as_str())
    {
        return ToolResult::warn(
            "写产物失败：filename 必须是纯文件名（不含路径分隔符、`.`/`..`、控制字符；Windows 保留设备名亦拒绝）",
            Vec::new(),
        );
    }
    // 产物目录 canonicalize（写路径钉死规范化目录，防 symlink 语义漂移）
    let mk_dir = ctx.artifact_dir.clone();
    let canonical = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        std::fs::create_dir_all(&mk_dir).map_err(|e| e.to_string())?;
        std::fs::canonicalize(&mk_dir).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let dir = match canonical {
        Ok(p) => p,
        Err(e) => {
            return ToolResult::warn(format!("写产物失败：产物目录不可用（{e}）"), Vec::new());
        }
    };
    let filename = filename.to_string();
    let content_len = content.len();
    let content = content.to_string();
    // FS 操作不持 DB 锁；同步小文件写放 spawn_blocking 防阻塞执行器
    let write_res = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut target = dir.join(&filename);
        let stem = std::path::Path::new(&filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let ext = std::path::Path::new(&filename)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut n = 1;
        while target.exists() {
            if n >= 1000 {
                return Err("同名产物过多（≥1000），停止追加写".into());
            }
            target = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::write(&target, &content).map_err(|e| e.to_string())?;
        Ok(target.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string());
    match write_res {
        Ok(Ok(path)) => ToolResult::ok(
            format!(
                "已写入产物 {path}（{content_len} 字节）。收尾 JSON 的 artifacts 里登记该路径。"
            ),
            Vec::new(),
        ),
        Ok(Err(e)) => ToolResult::warn(format!("写产物失败：{e}"), Vec::new()),
        Err(e) => ToolResult::warn(format!("写产物失败：{e}"), Vec::new()),
    }
}

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

/// 列某主会话派出的子 agent（新→旧）。聊天内嵌子 agent 区域与右侧栏
/// 「子 Agent」工作面的数据源——子 agent 不再建独立会话后，用户没有别的地方
/// 能看见自己派出去的那些还活着/刚跑完。
#[tauri::command]
pub async fn list_subagents(
    app: AppHandle,
    session_id: String,
    limit: Option<u32>,
) -> CommandResult<Vec<crate::db::SubagentRow>> {
    let limit = limit.unwrap_or(20).clamp(1, 100);
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        crate::db::load_subagents_by_parent(&conn, &session_id, limit)
            .map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent list 线程 join 失败：{e}")))?
}

/// read_own_card 工具：只读自己的执行契约。子 agent 不再建卡——契约在
/// subagents 行上（objective / acceptance_json / context_summary / budget_json），
/// 行不存在只可能是 runner 上下文串了会话，属真错误要响亮报出。
pub async fn tool_read_own_card(
    app: &AppHandle,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("read_own_card 仅子 agent 执行会话可用。", Vec::new());
    };
    let subagent_id = ctx.subagent_id.clone();
    let loaded = db_locked(app, move |conn| {
        crate::db::load_subagent(conn, &subagent_id).map_err(CommandError::DbError)
    })
    .await;
    let row = match loaded {
        Ok(Some(r)) => r,
        Ok(None) => {
            return ToolResult::warn("读契约失败：会话对应的 subagent 不在库中", Vec::new())
        }
        Err(e) => return ToolResult::warn(format!("读契约失败：{}", e.message()), Vec::new()),
    };
    let criteria: Vec<String> = row
        .acceptance_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let budget: Option<SubagentBudget> = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let cancelled = row.status == SubagentStatus::Cancelled;
    ToolResult::ok(
        serde_json::json!({
            "subagent_id": row.id,
            "objective": row.objective,
            "acceptanceCriteria": criteria,
            "contextSummary": row.context_summary,
            "budget": budget,
            "status": row.status.as_str(),
            "hint": if cancelled {
                "派发已被取消：立即停止新探索，输出收尾 JSON（status=cancelled）"
            } else {
                "上面的验收标准与上下文摘要是本任务的全部契约，逐条对照完成后收尾"
            }
        })
        .to_string(),
        Vec::new(),
    )
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;
    // 仅 parent_card 夹具用：子 agent 自己不再建卡，但派发仍可挂编排计划卡
    use crate::db::Task;

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

    /// U20D 批 5：前端以 `Session.isSubagent` 结构化字段判定子 agent 会话并
    /// 分流停止键（cancel_subagent / bot_stop，后者已作运行时兜底）——路由
    /// 契约从「标题 🧩 前缀」升级为 bot_sessions.is_subagent 字段。此处锁定：
    /// ① runner setup 必须经 subagent 变体建会话（漂移即测试红）；
    /// ② 标题已去 emoji 前缀（路由不再依赖标题）。
    #[test]
    fn subagent_sessions_flagged_structurally_not_by_title() {
        // concat! 拼接防本测试自匹配（dead_command_tests 同款手法）
        let wiring = concat!(
            "bot_session_create_",
            "subagent_inner(&tx, Some(title.clone()))"
        );
        assert!(
            include_str!("bot_orchestrator.rs").contains(wiring),
            "runner 建会话必须走 subagent 变体（is_subagent=1 结构化标记）"
        );
        assert!(
            !include_str!("bot_orchestrator.rs").contains(concat!("🧩 子", "任务：")),
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
        let body = include_str!("bot_orchestrator.rs");
        let runner = body
            .split_once("let run = crate::bot_model_loop::run_model_loop(")
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
            extract_last_json_object(r#"{"a":1}"#).as_deref(),
            Some(r#"{"a":1}"#)
        );
        // 散文夹 JSON + 多对象 → 取最后一个
        let t = r#"结论如下 {"status":"succeeded","summary":"第一步"} 中间 {"status":"succeeded","summary":"最终"}"#;
        let got = extract_last_json_object(t).unwrap();
        assert!(got.contains("最终"));
        // 字符串内花括号不破坏扫描
        let t2 = r#"{"summary":"带 } 花括号与 \" 引号"}"#;
        assert!(extract_last_json_object(t2).is_some());
        // 无 JSON
        assert_eq!(extract_last_json_object("没有任何大括号的普通回复"), None);
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
        assert_eq!(b.max_wall_seconds, 1);
        assert_eq!(
            clamp_budget(None),
            SubagentBudget {
                max_turns: 30,
                max_tool_calls: 100,
                max_wall_seconds: 600
            }
        );
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
        let body = include_str!("bot_orchestrator.rs");

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
        let body = include_str!("bot_orchestrator.rs");
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
