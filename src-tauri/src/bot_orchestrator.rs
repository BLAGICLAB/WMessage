//! 子 Agent 编排器（orchestrator）—— 方案 B（docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md）。
//!
//! 与 bot_execute_task 的边界（设计 §2）：bot_execute_task 是「单发子 agent」（用户点执行，
//! 无编排语义）；本模块是「受管子 agent」（主 agent 派发，orchestrator 管生命周期/预算/
//! 审计/子卡绑定）。两者复用 exec 运行基建，但入口、存储、事件各自独立。
//!
//! 分批（设计 §11）：
//! - SUBA-1（本批）：`subagents` 表 + 生命周期状态机 + spawn/check/cancel 内部实现 +
//!   子卡创建（一子 agent 一子卡）+ parent_task_id 关联 + acceptance 双写（note 侧）+
//!   审计 subagent_spawned；预算字段落表。不暴露 LLM 工具、不注册 tauri 命令。
//! - SUBA-2：三工具进 registry + 递归双保险 + 任务包装渲染 + 提示词 + runner + 收尾 JSON 解析。
//! - SUBA-3：预算强制（turns/tool_calls/wall）+ 并发排队（per-session 2 / global 3）+ 前端。
//!
//! 结构约定（同 task_patch / task_patch_locked 先例）：纯 DB 核心 `*_locked` 可在
//! in-memory conn 上单测；异步包装负责 spawn_blocking + DB_WRITE_LOCK + 审计 + 广播。

use crate::db::{SubagentBudget, SubagentProfile, SubagentRow, SubagentStatus, Task};
use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

/// 预算默认值与硬顶（设计 §6 已拍板）——常量与钳制实现在 db::SubagentBudget（写口
/// 强制：spawn 与 task_patch 都过 clamped()），此处 re-export 保持原路径可用。
pub use crate::db::{
    DEFAULT_MAX_TOOL_CALLS, DEFAULT_MAX_TURNS, DEFAULT_MAX_WALL_SECONDS, MAX_TURNS_HARD_CAP,
};
/// check_subagent 的 wait_ms 上限（设计 §5）。
pub const CHECK_WAIT_MS_CAP: u64 = 5_000;
/// wait 轮询步长。
const CHECK_POLL_INTERVAL_MS: u64 = 150;

/// spawn 入参（设计 §5 工具签名的内部形态）。
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub objective: String,
    pub profile: SubagentProfile,
    /// **spawn 必填**，每条可检验（设计 §4.2 双写：子卡 note + 任务包装）
    pub acceptance_criteria: Vec<String>,
    /// 上下文摘要（设计 §8.5「上下文摘要」段）：只给必要背景，不倒主对话全文。
    /// 落在子卡 note 的【上下文摘要】段——与验收标准同卡，用户改 note 即改需求
    ///（卡即契约：runner 渲染包装时从卡上现读）。
    pub context_summary: Option<String>,
    pub budget: Option<SubagentBudget>,
    /// A 期参数透传：None/无效值回退当前 active model（设计 §5）
    pub model_profile: Option<String>,
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
    pub task_id: String,
    pub trace_id: String,
    pub status: SubagentStatus,
}

/// check 应答（设计 §5/§7：status/progress/result?/error?）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckState {
    pub subagent_id: String,
    pub task_id: String,
    /// 子 agent 执行会话（围观串链键；runner 建会话前为 None）
    pub session_id: Option<String>,
    pub status: SubagentStatus,
    /// 进度小计：子卡 subtasks 勾选比 + 预算消耗（A3 起带 turns/tool_calls）
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
            reason: "验收标准必填且每条非空（spawn 双写契约，设计 §4.2）".into(),
        });
    }
    Ok(())
}

fn acceptance_note(criteria: &[String], context_summary: Option<&str>) -> String {
    let mut note = String::from("【验收标准】\n");
    for c in criteria {
        note.push_str(&format!("- {c}\n"));
    }
    if let Some(ctx) = context_summary.map(str::trim).filter(|s| !s.is_empty()) {
        note.push_str(&format!("\n【上下文摘要】\n{ctx}\n"));
    }
    note.push_str(
        "\n（子 agent 编排派发的执行契约卡：改 subtasks/补 note 即修改需求，软删本卡 = 取消派发）",
    );
    note
}

/// spawn 纯 DB 核心：校验 → 缺 parent 卡 TaskNotFound → 建子卡 → 插 queued 行。
/// 返回 (ack, 子卡) —— 广播由异步包装做（锁内不做 IO 之外的事）。
pub(crate) fn spawn_subagent_locked(
    conn: &rusqlite::Connection,
    req: &SpawnRequest,
    now: i64,
) -> CommandResult<(SpawnAck, Task)> {
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
    let task_id = uuid::Uuid::new_v4().simple().to_string();
    let card = Task {
        acceptance: None,
        id: task_id.clone(),
        title: format!(
            "子任务：{}",
            crate::bot::truncate_for_log(req.objective.trim(), 30)
        ),
        due: None,
        note: Some(acceptance_note(
            &req.acceptance_criteria,
            req.context_summary.as_deref(),
        )),
        tags: None,
        files: None,
        file_path: None,
        file_is_dir: None,
        column: crate::db::TaskStatus::Doing,
        subtasks: None,
        completed_at: None,
        archived: Some(false),
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(now),
        schedule: None,
        sched_last: None,
        bot_assigned: None,
        assignee: None,
        budget: Some(budget.clone()),
        result: None,
        origin: None,
        workflow_id: None,
        depends_on: None,
        canvas_pos: None,
        model: None,
        owner_id: None,        // 子 agent 派发卡 = 本人（任务图谱设计 §1.1）
        created_at: Some(now), // 创建时间打戳（与 updated_at 同值；此后 UPDATE 不覆盖）
        enabled: None,         // 派发卡无定时配置
        expected_updated_at: None,
    };
    crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).map_err(CommandError::DbError)?;
    let row = SubagentRow {
        id: subagent_id.clone(),
        status: SubagentStatus::Queued,
        profile: req.profile,
        model: req.model_profile.clone(),
        parent_session_id: req.parent_session_id.clone(),
        session_id: None,
        task_id: task_id.clone(),
        objective: req.objective.trim().to_string(),
        trace_id: trace_id.clone(),
        acceptance_json: Some(
            serde_json::to_string(&req.acceptance_criteria)
                .map_err(|e| CommandError::Internal(e.to_string()))?,
        ),
        budget_json: Some(
            serde_json::to_string(&budget).map_err(|e| CommandError::Internal(e.to_string()))?,
        ),
        result_json: None,
        error: None,
        created_at: now,
        started_at: None,
        finished_at: None,
    };
    crate::db::insert_subagent(conn, &row).map_err(CommandError::DbError)?;
    Ok((
        SpawnAck {
            subagent_id,
            task_id,
            trace_id,
            status: SubagentStatus::Queued,
        },
        card,
    ))
}

/// 按 subagent_id / task_id / 子会话 id 解析行（设计 §5 双键 + SUBA-3 会话键：
/// 任务卡停止按钮与子会话停止键只持有会话 id）。
/// 前缀约定：subagent_id 恒以 `sa_` 开头；task_id 与 session_id 均为裸 uuid——
/// 先 task_id 后 session_id 双探测（SUBA-3 批勘误）。
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

/// check 纯 DB 核心：幂等读 + 进度小计（子卡 subtasks 勾选比）。
pub(crate) fn check_subagent_locked(
    conn: &rusqlite::Connection,
    key: &str,
) -> CommandResult<CheckState> {
    let row = resolve_row(conn, key)?;
    let (subtasks_done, subtasks_total) = crate::db::load_task(conn, &row.task_id)
        .map_err(CommandError::DbError)?
        .and_then(|t| t.subtasks)
        .map(|s| (s.iter().filter(|x| x.done).count(), s.len()))
        .unwrap_or((0, 0));
    let result = row
        .result_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    // 排队位次（SUBA-3）：queued 状态时给出 FIFO 前面还有几个
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
pub(crate) fn cancel_subagent_locked(
    conn: &rusqlite::Connection,
    key: &str,
    reason: &str,
    now: i64,
) -> CommandResult<(CancelAck, Option<Task>)> {
    let row = resolve_row(conn, key)?;
    if row.status.is_terminal() {
        return Ok((
            CancelAck {
                ok: true,
                subagent_id: row.id,
                status: row.status,
                already_terminal: true,
            },
            None,
        ));
    }
    crate::db::update_subagent_status(conn, &row.id, SubagentStatus::Cancelled, now, Some(reason))
        .map_err(CommandError::DbError)?;
    // 子卡同步回退 doing → todo：取消后卡片不能永远停在「进行中」
    //（OCR r2 采纳：取消必须让前端可见，否则卡片悬挂在 doing）
    let card = crate::db::load_task(conn, &row.task_id).map_err(CommandError::DbError)?;
    let card = match card {
        Some(mut c) if c.column == crate::db::TaskStatus::Doing => {
            c.column = crate::db::TaskStatus::Todo;
            c.expected_updated_at = c.updated_at;
            c.updated_at = Some(now);
            crate::db::upsert_tasks(conn, std::slice::from_ref(&c))
                .map_err(CommandError::DbError)?;
            Some(c)
        }
        other => other,
    };
    Ok((
        CancelAck {
            ok: true,
            subagent_id: row.id,
            status: SubagentStatus::Cancelled,
            already_terminal: false,
        },
        card,
    ))
}

/// spawn 异步包装：持锁执行核心 → 审计 subagent_spawned → 广播子卡。
/// SUBA-1 不启动执行（runner 在 SUBA-2 接入）；行停在 queued 由后续批推进。
pub async fn spawn_subagent(app: &AppHandle, req: SpawnRequest) -> CommandResult<SpawnAck> {
    // P3-a：LLM 未显式给预算时用配置默认（config subagentMax* 三项，resolve 内钳制）；
    // 显式给了的仍走 clamp_budget 硬顶。Gate 并发数（max_running）保持常量不在本路径。
    let mut req = req;
    if req.budget.is_none() {
        let cfg = crate::bot::load_config(app);
        req.budget = Some(crate::bot::params::resolve_subagent_budget(&cfg));
    }
    // 审计字段先拷出（闭包要 move req 进 spawn_blocking）；校验只走 locked 核心
    //（wrapper 侧重复调用已删——OCR r2 采纳）
    let audit_parent_session = req.parent_session_id.clone();
    let audit_profile = req.profile;
    let audit_model = req.model_profile.clone();
    let app2 = app.clone();
    let (ack, card) =
        tauri::async_runtime::spawn_blocking(move || -> CommandResult<(SpawnAck, Task)> {
            let _g = crate::db::lock_db_write();
            let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
            let now = chrono::Utc::now().timestamp_millis();
            spawn_subagent_locked(&conn, &req, now)
        })
        .await
        .map_err(|e| CommandError::from(format!("subagent spawn 线程 join 失败：{e}")))??;
    // 审计：结构化 + 文本行（设计 §3 事件字段）
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "subagent_spawned",
        &[
            ("subagent_id", ack.subagent_id.clone()),
            ("task_id", ack.task_id.clone()),
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
            "subagent_spawned | id: {} | task: {} | parent_session: {} | profile: {} | trace: {}",
            ack.subagent_id,
            ack.task_id,
            audit_parent_session.as_deref().unwrap_or("-"),
            audit_profile.as_str(),
            ack.trace_id,
        ),
    );
    crate::bot::broadcast_after_mutation(app, vec![card], Vec::new());
    // 非阻塞派发 runner（设计 §5：spawn 立即返回；执行在后台推进状态机）
    let runner_app = app.clone();
    let runner_sid = ack.subagent_id.clone();
    tauri::async_runtime::spawn(async move { run_subagent(runner_app, runner_sid).await });
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

/// cancel 异步包装：置 cancelled + 子卡回退 + 审计 + 广播（前端即时感知，
/// OCR r2 采纳）。SUBA-2 接 runner 后在此叠加 force_stop。
pub async fn cancel_subagent_async(
    app: &AppHandle,
    key: &str,
    reason: &str,
) -> CommandResult<CancelAck> {
    let app2 = app.clone();
    let key = key.to_string();
    let reason = reason.to_string();
    let audit_reason = reason.clone();
    let (ack, card) = tauri::async_runtime::spawn_blocking(
        move || -> CommandResult<(CancelAck, Option<Task>)> {
            let _g = crate::db::lock_db_write();
            let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
            let now = chrono::Utc::now().timestamp_millis();
            cancel_subagent_locked(&conn, &key, &reason, now)
        },
    )
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
        if let Some(card) = card {
            crate::bot::broadcast_after_mutation(app, vec![card], Vec::new());
        }
        // 远程停止 runner 工具循环（令牌在 runner 注册；watcher 2s 兜底轮询）
        let token = crate::app_state::subagent_stops(app)
            .lock()
            .unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] app_state::subagent_stops: {e:?}");
                e.into_inner()
            })
            .get(&ack.subagent_id)
            .cloned();
        if let Some(token) = token {
            token.stop();
        }
    }
    Ok(ack)
}

// ═══════════════════════ SUBA-2：工具实现 + runner + 收尾解析 ═══════════════════════

use crate::bot_chat::{ChatGuard, ExecGuard};
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::Emitter;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

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
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        spans.push((start, i + 1));
                    }
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
        if err.contains("对话轮数超限") {
            return (
                SubagentStatus::BudgetExceeded,
                Some("max_turns".into()),
                None,
            );
        }
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(err, 300)),
            None,
        );
    }
    let text = final_text.unwrap_or_default();
    let parsed = extract_last_json_object(text)
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok());
    match parsed {
        Some(v) => {
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

/// 任务包装（设计 §8.5）：orchestrator 从子卡渲染，不接受 LLM 自由拼接。
/// 上下文摘要现读卡上 note（用户改 note = 改需求，卡即契约）。
pub(crate) fn render_task_wrapper(
    row: &SubagentRow,
    card: &Task,
    budget: &SubagentBudget,
    artifact_dir: &std::path::Path,
) -> String {
    let criteria: Vec<String> = row
        .acceptance_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let mut w = String::new();
    w.push_str("[子任务派发]\n");
    w.push_str(&format!("subagent_id: {}\n", row.id));
    w.push_str(&format!("objective: {}\n", row.objective));
    w.push_str("\nacceptance_criteria:\n");
    if criteria.is_empty() {
        w.push_str("- （子卡 note 中的验收标准）\n");
    }
    for c in &criteria {
        w.push_str(&format!("- {c}\n"));
    }
    // 上下文摘要：从卡 note 摘出【上下文摘要】段（无则跳过）
    let note = card.note.as_deref().unwrap_or_default();
    if let Some(begin) = note.find("【上下文摘要】") {
        let rest = &note[begin + "【上下文摘要】".len()..];
        let end = rest.find("\n（").unwrap_or(rest.len());
        let ctx = rest[..end].trim();
        if !ctx.is_empty() {
            w.push_str("\n上下文摘要:\n");
            w.push_str(ctx);
            w.push('\n');
        }
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

/// 收尾：解析结果 → 代勾 subtask → 产物 bind 回子卡 → result 写卡（锁内）。
/// 返回更新后的子卡供广播。
fn finalize_card_locked(
    conn: &rusqlite::Connection,
    row: &SubagentRow,
    artifact_dir: &std::path::Path,
    result: Option<serde_json::Value>,
) -> CommandResult<Task> {
    let mut card = crate::db::load_task(conn, &row.task_id)
        .map_err(CommandError::DbError)?
        .ok_or_else(|| CommandError::TaskNotFound(row.task_id.clone()))?;
    // 1. 代勾 subtask（设计 §6：仅 orchestrator 代勾；按 id 对账，done 才勾）
    if let Some(value) = &result {
        if let Some(reported) = value.get("subtasks").and_then(|s| s.as_array()) {
            if let Some(subtasks) = card.subtasks.as_mut() {
                for st in subtasks.iter_mut() {
                    let hit = reported.iter().any(|r| {
                        r.get("id").and_then(|i| i.as_str()) == Some(st.id.as_str())
                            && r.get("status").and_then(|s| s.as_str()) == Some("done")
                    });
                    if hit {
                        st.done = true;
                    }
                }
            }
        }
    }
    // 2. 产物 bind 回卡（产物目录实存文件 + result.artifacts 路径，与既有 files
    //    合并去重，上限 MAX_TASK_FILES——设计 §13）
    let mut files = card.effective_files();
    if let Ok(entries) = std::fs::read_dir(artifact_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                let path = p.to_string_lossy().to_string();
                if !files.iter().any(|f| f.path == path) {
                    files.push(crate::db::TaskFile {
                        path,
                        is_dir: false,
                    });
                }
            }
        }
    }
    if let Some(value) = &result {
        if let Some(arts) = value.get("artifacts").and_then(|a| a.as_array()) {
            for a in arts {
                if let Some(path) = a.get("path").and_then(|p| p.as_str()) {
                    if !files.iter().any(|f| f.path == path) {
                        files.push(crate::db::TaskFile {
                            path: path.to_string(),
                            is_dir: false,
                        });
                    }
                }
            }
        }
    }
    files.truncate(crate::db::MAX_TASK_FILES);
    card.files = if files.is_empty() { None } else { Some(files) };
    // 3. result 写卡
    card.result = result;
    card.expected_updated_at = card.updated_at;
    card.updated_at = Some(now_ms());
    crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).map_err(CommandError::DbError)?;
    Ok(card)
}

/// 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/。
pub(crate) fn artifact_dir_of(gen_root: &std::path::Path, subagent_id: &str) -> std::path::PathBuf {
    gen_root.join("subagents").join(subagent_id)
}

// ───────────────────────── 并发闸（SUBA-3：per-session 2 / global 3 FIFO） ─────────────────────────

/// 并发上限（设计 §6 已拍板）：全局 3、每个派发主会话 2。超限排队不失败
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

/// 进程级并发闸。FIFO 口径勘误（SUBA-3，OCR r1 high 采纳后的简化）：
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
        let mut st = gate().state.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] subagent gate: {e:?}");
            e.into_inner()
        });
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
            let mut st = gate().state.lock().unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] subagent gate: {e:?}");
                e.into_inner()
            });
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
    /// 等到槽位。同会话内按等待先后放行（waiting 计数=1 者优先），
    /// 跨会话近似公平。所有状态变更 RAII 化：future 被 drop 无任何残留。
    pub(crate) async fn wait_slot(parent_session: Option<String>) -> GateTicket {
        Self::wait_slot_cancellable(parent_session, || false)
            .await
            .expect("不可取消的 wait_slot 不会返回 None")
    }

    /// B3-1：可取消排队——每轮 50ms 退避后调 `should_abort`，true 即退队返回
    /// None（waiting 计数由 WaitingGuard RAII 递减）。取消判定由调用方给
    ///（runner 查 DB 行终态），闸门本身不依赖存储。
    pub(crate) async fn wait_slot_cancellable(
        parent_session: Option<String>,
        mut should_abort: impl FnMut() -> bool,
    ) -> Option<GateTicket> {
        // 登记等待席位（RAII：无论从哪条路径退出，计数都会被递减）
        {
            let mut st = gate().state.lock().unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] subagent gate: {e:?}");
                e.into_inner()
            });
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
                let mut st = gate().state.lock().unwrap_or_else(|e| {
                    eprintln!("[mutex_poisoned] subagent gate: {e:?}");
                    e.into_inner()
                });
                let global_ok = st.global_running < MAX_GLOBAL_RUNNING;
                let sess_running = parent_session
                    .as_deref()
                    .and_then(|k| st.per_session.get(k).copied())
                    .unwrap_or(0);
                let sess_ok = sess_running < MAX_PER_SESSION_RUNNING;
                // OCR r2 high 采纳：不做「等待序数」公平判定（waiting<=1 的判定
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
        let st = gate().state.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] subagent gate: {e:?}");
            e.into_inner()
        });
        let n = parent_session
            .and_then(|k| st.waiting.get(k).copied())
            .unwrap_or(0);
        (n > 0).then(|| n - 1)
    }
}

// ───────────────────────── runner（执行引擎） ─────────────────────────

/// 子 agent 执行引擎：独立会话 + 工具循环 + 收尾落库。spawn 异步包装末尾派发
/// （非阻塞）。执行基建复用 exec 家族（ChatGuard/ExecGuard/StopGuard/tool_guard），
/// 但入口/存储/事件与 bot_execute_task 各自独立（设计 §2）。
/// 具体化 `tauri::AppHandle`（= Wry）：run_model_loop 生产壳即该签名
/// （mock 链路走 run_model_loop_core，同 llm_integration 分层）。
async fn run_subagent(app: AppHandle, subagent_id: String) {
    let sid_for_load = subagent_id.clone();
    let row = match db_locked(&app, move |conn| {
        crate::db::load_subagent(conn, &sid_for_load)
            .map_err(CommandError::DbError)?
            .ok_or_else(|| CommandError::TaskNotFound(sid_for_load))
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // OCR r2 high 采纳：加载失败也要落终态，防 ghost queued 行永挂
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
            return;
        }
    };
    // 派发后、开跑前已被取消/终态 → 不启动
    if row.status != SubagentStatus::Queued {
        return;
    }
    let budget: SubagentBudget = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    // 子卡防重入（与 🤖 手动执行互斥）；被占用 → failed（罕见：spawn 后立刻手动执行）
    let Some(_exec_guard) = ExecGuard::acquire(&app, &row.task_id) else {
        let sid = row.id.clone();
        let _ = db_locked(&app, move |conn| {
            crate::db::update_subagent_status(
                conn,
                &sid,
                SubagentStatus::Failed,
                now_ms(),
                Some("subcard_busy"),
            )
            .map_err(CommandError::DbError)
        })
        .await;
        return;
    };
    // 并发闸（SUBA-3：global 3 / per-session 2，超限等待不失败）。
    // 排队时间不计墙钟：max_wall_seconds 的 timeout 从 run_model_loop 起算（下方），
    // 天然满足设计 §6。
    // B3-1：排队可取消——每轮退避同时查行终态，取消即退队（ExecGuard RAII
    // 释放）；Running 改在**拿到槽位并复核后**才写，排队中的行不再假挂运行中。
    let sid_gate_check = row.id.clone();
    let app_gate_check = app.clone();
    // 评审 M 采纳：节流 DB 读——open_db 含建目录+pragmas 文件 IO，不该 50ms
    // 一次压在 async worker 上；取消检测 500ms 粒度足够（槽后复核兜底）
    let gate_tick = std::cell::Cell::new(0u32);
    let mut cancelled_while_queued = move || {
        let n = gate_tick.get();
        gate_tick.set(n.wrapping_add(1));
        if n % 10 != 0 {
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
            return;
        }
    };
    {
        // 拿到槽位后复核（cancel-vs-runner 竞态闸）：行已终态 = cancel 先赢 →
        // 中止；DB 读失败保守中止（执行已取消的任务比不跑更糟，OCR r2 口径保留）
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
                return;
            }
        }
        // B3-1：Running 写库挪到槽位拿到之后（排队窗口内行保持 queued 语义）。
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
            return;
        }
    };
    // create + canonicalize（OCR r2 high 采纳：写路径钉死规范化目录）。失败仅审计：
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
        db_locked(&app, move |conn| -> CommandResult<(String, Task, String)> {
            // OCR r1 high 采纳：四写包同一事务，防部分成功留半成品
            let tx = conn
                .transaction()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            let session = crate::db::bot_session_create_subagent_inner(&tx, Some(title.clone()))
                .map_err(CommandError::DbError)?;
            let card = crate::db::load_task(&tx, &row.task_id)
                .map_err(CommandError::DbError)?
                .ok_or_else(|| CommandError::TaskNotFound(row.task_id.clone()))?;
            // 包装文本仅返回（渲染一次，交 runner 供 LLM 与 finalize 复用）——
            // 历史落库统一在 finalize 全量覆盖写（OCR r3 high 采纳：setup 的
            // bot_history_save_inner 是半截写，会被 finalize DELETE+INSERT 覆盖，
            // 中途崩溃还留「只有任务块没有结论」的误导历史）
            let wrapper = render_task_wrapper(&row, &card, &budget, &artifact_dir);
            crate::db::set_subagent_session(&tx, &row.id, &session.id)
                .map_err(CommandError::DbError)?;
            // assignee = 子 agent 会话 id（设计 §4.1 串链围观/审计）
            let mut card2 = card;
            card2.assignee = Some(session.id.clone());
            card2.expected_updated_at = card2.updated_at;
            card2.updated_at = Some(now_ms());
            crate::db::upsert_tasks(&tx, std::slice::from_ref(&card2))
                .map_err(CommandError::DbError)?;
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok((session.id, card2, wrapper))
        })
    }
    .await;
    let (session_id, _card, wrapper) = match setup {
        Ok(v) => v,
        Err(e) => {
            let msg = e.message();
            let sid = row.id.clone();
            let _ = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &sid,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&msg),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            return;
        }
    };

    // 工具守卫注册（白名单闸 + spawn 递归身份校验 + read_own_card/write_artifact 定位）。
    // RAII 守卫：Drop 反注册——runner 中途 panic 也不泄漏条目（OCR r2 high 采纳）
    let _session_guard = crate::tool_guard::SubagentSessionGuard::new(
        &session_id,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: row.id.clone(),
            task_id: row.task_id.clone(),
            profile: row.profile,
            budget: budget.clone(),
            artifact_dir: artifact_dir.clone(),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        },
    );

    // 会话锁（用户向围观会话发消息 → 友好拒绝）+ 停止守卫（interactive=围观流式）
    let chat_guard = ChatGuard::acquire(&app, Some(session_id.as_str()));
    if chat_guard.is_err() {
        // OCR r1 high 采纳：此处行已是 Running——中止必须落终态，不能悬挂
        let sid_failed = row.id.clone();
        let _ = db_locked(&app, move |conn| {
            crate::db::update_subagent_status(
                conn,
                &sid_failed,
                SubagentStatus::Failed,
                now_ms(),
                Some("session_busy"),
            )
            .map_err(CommandError::DbError)
        })
        .await;
        crate::bot::audit_log(
            &app,
            &format!("subagent_runner_abort | id: {} | err: session_busy", row.id),
        );
        return;
    }
    let stop = crate::bot_slash::StopGuard::new(&app, true, Some(session_id.clone()));
    // 取消令牌登记：cancel_subagent 持句柄远程停止。已知窗口（可接受，OCR r2 留痕）：
    // ChatGuard 成功到令牌登记之间收到的 cancel 落不到令牌——由 2s watcher 兜底
    // （行已 cancelled → watcher 置 stop）。
    crate::app_state::subagent_stops(&app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] app_state::subagent_stops: {e:?}");
            e.into_inner()
        })
        .insert(row.id.clone(), stop.token());

    // 软删/取消 watcher（设计 §4.3 硬停）：2s 轮询子卡 deletedAt 与行状态
    let watcher_app = app.clone();
    let watcher_stop = stop.token();
    let watcher_sid = row.id.clone();
    let watcher_task_id = row.task_id.clone();
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
            let task_id = watcher_task_id.clone();
            let sid = watcher_sid.clone();
            // 只读轮询：直接开连接，不持 DB_WRITE_LOCK（防多子 agent 写锁互饿——OCR r1 采纳）
            let watcher_app2 = watcher_app.clone();
            let aborted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
                let conn = crate::db::open_db(&watcher_app2).map_err(|e| e)?;
                let card_deleted = crate::db::load_task(&conn, &task_id)
                    .ok()
                    .flatten()
                    .and_then(|t| t.deleted_at)
                    .is_some();
                let row_terminal = crate::db::load_subagent(&conn, &sid)
                    .ok()
                    .flatten()
                    .map(|r| r.status.is_terminal())
                    .unwrap_or(true);
                Ok(card_deleted || row_terminal)
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
    let profile_prompt = match row.profile {
        SubagentProfile::Research => crate::prompts::PROFILE_RESEARCH,
        SubagentProfile::Coder => crate::prompts::PROFILE_CODER,
        SubagentProfile::General => crate::prompts::PROFILE_GENERAL,
    };
    // wrapper 已由 setup 渲染返回（同一文本：LLM 输入 = 历史落库 = 收尾覆盖源）
    let system = format!(
        "{}\n\n{}\n\n{}",
        crate::prompts::SUBAGENT_BASE,
        profile_prompt,
        gen_dir_rule_for(&artifact_dir),
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
        // 子 agent 模型走自身预算/派发通道（model_profile），不挂每卡覆盖（W6-MODEL）
        None,
    );
    let wall = tokio::time::timeout(
        std::time::Duration::from_secs(budget.max_wall_seconds.max(1)),
        run,
    )
    .await;

    // B3-2：墙钟触达先 stop 后收尾——run future 被 timeout 掐掉时，脱离 runtime
    // 的在飞 Python/MCP（spawn_blocking 不随 future 取消）靠 StopToken 自行退出；
    // 短 grace 给它们的退出清理/Drop 留时间，不留孤儿进程
    if wall.is_err() {
        stop.force_stop();
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }

    // 收尾清理：watcher 由 AbortOnDrop Drop 保证中止（含 panic/unwind 路径——
    // OCR r3 high 采纳：StopGuard::drop 不置位停止标志，单靠手动 abort 会泄漏）
    let _watcher = AbortOnDrop(watcher);
    // tool_guard 注册走 RAII 守卫（_session_guard Drop 反注册）
    crate::app_state::subagent_stops(&app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] app_state::subagent_stops: {e:?}");
            e.into_inner()
        })
        .remove(&row.id);
    drop(chat_guard);
    drop(_exec_guard);

    // 收尾判定（设计 §6/§7）
    let (status, error, result) = match &wall {
        Err(_elapsed) => classify_outcome(None, true, None),
        Ok(Err(e)) => classify_outcome(Some(&e.message()), false, None),
        Ok(Ok((text, _refs, _loop_trace))) => classify_outcome(None, false, Some(text)),
    };
    let final_text: String = match &wall {
        Ok(Ok((text, _, _))) => text.clone(),
        Ok(Err(e)) => format!("⚠️ 执行失败：{}", e.message()),
        Err(_) => "⏹ 墙钟预算触达，执行被强制停止（部分产物已保留）".into(),
    };
    let wrapper_for_db = wrapper.clone();
    let row_for_db = row.clone();
    let session_id_for_db = session_id.clone();
    let artifact_dir_for_db = artifact_dir.clone();
    let status_for_db = status;
    let result_for_db = result.clone();
    let error_for_db = error.clone();
    let final_for_db = final_text.clone();
    // 返回 (fresh row, card)——下游审计/广播直接用，免二次持锁查询（OCR r1 采纳）
    let finalized: Option<(SubagentRow, Task)> = db_locked(
        &app,
        move |conn| -> CommandResult<Option<(SubagentRow, Task)>> {
            // OCR r1 high 采纳：多写包同一事务 + cancel-vs-runner 竞态闸——
            // 行已终态（cancel 先赢）时不做任何状态/结果/子卡副作用，只落历史
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
                return Ok(None);
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
            let card =
                finalize_card_locked(&tx, &fresh, &artifact_dir_for_db, result_for_db.clone())?;
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok(Some((fresh, card)))
        },
    )
    .await
    .unwrap_or_else(|e| {
        let msg = crate::bot::truncate_for_log(&e.message(), 200);
        // OCR r2 high 采纳：收尾失败行会永挂 Running——best-effort 落 Failed
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

    // 审计 + 广播 + widget 事件（设计 §3 事件全链按 subagent_id 串链）
    let elapsed = started.elapsed().as_millis() as u64;
    let event = match status {
        SubagentStatus::Succeeded => "subagent_done",
        SubagentStatus::BudgetExceeded => "subagent_budget_exceeded",
        SubagentStatus::Cancelled => "subagent_cancelled",
        _ => "subagent_failed",
    };
    // fresh/card 直接取自收尾闭包返回值（OCR r1 采纳：不再二次持锁查询）
    let fresh = finalized.as_ref().map(|(r, _)| r.clone());
    let card = finalized.as_ref().map(|(_, c)| c.clone());
    let summary = fresh
        .as_ref()
        .and_then(|r| r.result_json.as_deref())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("summary")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let row_error = fresh
        .as_ref()
        .and_then(|r| r.error.clone())
        .unwrap_or_default();
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        event,
        &[
            ("subagent_id", row.id.clone()),
            ("task_id", row.task_id.clone()),
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
            "{} | id: {} | task: {} | status: {} | elapsed_ms: {} | error: {}",
            event,
            row.id,
            row.task_id,
            status.as_str(),
            elapsed,
            crate::bot::truncate_for_log(&row_error, 120)
        ),
    );
    if let Some(card) = card {
        crate::bot::broadcast_after_mutation(&app, vec![card], Vec::new());
    }
    let _ = app.emit_to(
        "widget",
        "subagent-finished",
        serde_json::json!({
            "subagentId": row.id,
            "taskId": row.task_id,
            "parentSessionId": row.parent_session_id,
            "sessionId": session_id,
            "status": status.as_str(),
            "summary": summary.unwrap_or_default(),
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

// ───────────────────────── LLM 工具实现（registry 调用） ─────────────────────────

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
    let profile = match v["profile"].as_str().and_then(SubagentProfile::from_str) {
        Some(p) => p,
        None => {
            return ToolResult::warn(
                "spawn 失败：profile 必须是 research / coder / general 之一",
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
            "spawn 失败：acceptanceCriteria 必填且每条可检验（设计 §4.2 双写契约）",
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
        acceptance_criteria: criteria,
        context_summary: v["contextSummary"].as_str().map(str::to_string),
        budget,
        model_profile: v["modelProfile"].as_str().map(str::to_string),
        parent_task_id: v["parentTaskId"].as_str().map(str::to_string),
        parent_session_id: session_id,
    };
    match spawn_subagent(app, req).await {
        Ok(ack) => ToolResult::ok(
            format!(
                "已派发子 agent（非阻塞）：subagentId={} taskId={} status={}。用 check_subagent 轮询结果，先继续回应用户。",
                ack.subagent_id, ack.task_id, ack.status.as_str()
            ),
            vec![crate::bot_chat::TaskRef {
                id: ack.task_id.clone(),
                title: format!("子任务 {}", ack.subagent_id),
            }],
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
    // content 上限（OCR r3 high 采纳）：失控子 agent 循环写大文件可填满磁盘——
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
    // 与 Windows 保留设备名（OCR r1 采纳）；.. 按组件判定而非子串——
    // 不误伤 foo..bar / ...txt 这类合法名（OCR r2 采纳）
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
    // 产物目录 canonicalize（OCR r2 high 采纳：写路径钉死规范化目录，防 symlink 语义漂移）
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

/// cancel_subagent tauri 命令（SUBA-3：任务卡停止按钮 / 子会话停止键的同 API 入口，
/// 设计 §4.3；薄壳转内部 cancel_subagent_async）。
#[tauri::command]
pub async fn cancel_subagent(
    app: AppHandle,
    key: String,
    reason: Option<String>,
) -> CommandResult<CancelAck> {
    cancel_subagent_async(&app, &key, reason.as_deref().unwrap_or("用户取消")).await
}

/// read_own_card 工具：只读自己的子卡（卡即契约——重读 note/subtasks/deletedAt）。
pub async fn tool_read_own_card(
    app: &AppHandle,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("read_own_card 仅子 agent 执行会话可用。", Vec::new());
    };
    let task_id = ctx.task_id.clone();
    let card = db_locked(app, move |conn| {
        crate::db::load_task(conn, &task_id)
            .map_err(CommandError::DbError)?
            .ok_or_else(|| CommandError::TaskNotFound(task_id.clone()))
    })
    .await;
    match card {
        Ok(c) => {
            let deleted = c.deleted_at.is_some();
            ToolResult::ok(
                serde_json::json!({
                    "title": c.title,
                    "note": c.note,
                    "subtasks": c.subtasks,
                    "budget": c.budget,
                    "deletedAt": c.deleted_at,
                    "deleted": deleted,
                    "hint": if deleted { "卡片已被软删：立即停止新探索，输出收尾 JSON（status=cancelled）" } else { "" }
                })
                .to_string(),
                Vec::new(),
            )
        }
        Err(e) => ToolResult::warn(format!("读卡失败：{}", e.message()), Vec::new()),
    }
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // tasks 表内联最小 schema（列序与 open_db 一致，含 SUBA-1 新增三列）；
        // subagents 表与生产同源 DDL（单源真相）
        conn.execute_batch(crate::db::tasks::TASKS_DDL).unwrap();
        conn.execute_batch(crate::db::SUBAGENTS_DDL).unwrap();
        conn
    }

    fn parent_card(conn: &rusqlite::Connection) -> String {
        let card = Task {
            acceptance: None,
            id: "parent-1".into(),
            title: "主编排卡".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: crate::db::TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: Some(false),
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: Some(1_000),
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            origin: None,
            workflow_id: None,
            depends_on: None,
            canvas_pos: None,
            model: None,
            owner_id: None,
            created_at: None,
            enabled: None,
            expected_updated_at: None,
        };
        crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).unwrap();
        "parent-1".into()
    }

    // ─────────────────── SUBA-3：并发闸（FIFO/上限/释放） ───────────────────

    /// 并发闸状态机（合并为单测：static GATE 进程级共享，cargo 并行跑两个测试
    /// 会互相踩计数；手动构造票据，不调 wait_slot——它会与其他并行测试竞争
    /// 全局槽导致排队挂死）。
    #[test]
    fn gate_state_machine_acquire_release_and_queue_position() {
        let before_running = gate().state.lock().unwrap().global_running;
        // ① 拿槽 → 计数 +1、per_session 登记、未排队
        {
            let mut t = Some(GateTicket {
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
            t = None;
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running, "Drop 必须释放槽位");
            assert!(st.per_session.get("ps_drop").is_none(), "计数归零须清键");
        }
        // ③ 多持多放对称：两张票据只放一张 → 残 1；全放 → 清键
        {
            let mut t1 = Some(GateTicket {
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
            t1 = None;
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running, "全放回基线");
            assert!(st.per_session.get("ps_rel").is_none(), "归零清键");
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

    /// SUBA-3：tool_calls 预算计数器（ctx 侧）——dispatch 累计语义的单测锚点
    #[test]
    fn tool_call_counter_accumulates() {
        let ctx = crate::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_c1".into(),
            task_id: "card_c1".into(),
            profile: SubagentProfile::Coder,
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

    // ─────────────────── SUBA-2：收尾解析 / 包装渲染 ───────────────────

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
        // 轮数熔断 → budget_exceeded
        let (st, err, _) = classify_outcome(Some("对话轮数超限"), false, None);
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

    #[test]
    fn task_wrapper_contains_all_sections() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let (ack, card) = spawn_subagent_locked(&conn, &req(None), 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        let budget = crate::db::SubagentBudget::default();
        let w = render_task_wrapper(&row, &card, &budget, std::path::Path::new("/tmp/art"));
        assert!(w.contains("[子任务派发]"));
        assert!(w.contains(&format!("subagent_id: {}", row.id)));
        assert!(w.contains("objective: 调研五个竞品并输出报告"));
        assert!(w.contains("- 覆盖至少 5 个产品"));
        assert!(w.contains("max_turns=30, max_tool_calls=100, max_wall_seconds=600"));
        assert!(w.contains("产物目录:"));
        assert!(w.contains("/tmp/art"));
        assert!(w.contains("完成后必须输出以下 JSON"));
        assert!(w.contains("\"succeeded|failed|cancelled|budget_exceeded\""));
    }

    fn req(parent: Option<String>) -> SpawnRequest {
        SpawnRequest {
            objective: "调研五个竞品并输出报告".into(),
            profile: SubagentProfile::Research,
            acceptance_criteria: vec!["覆盖至少 5 个产品".into(), "每条含官网 URL".into()],
            context_summary: None,
            budget: None,
            model_profile: None,
            parent_task_id: parent,
            parent_session_id: Some("main-sess".into()),
        }
    }

    #[test]
    fn spawn_creates_row_and_subcard_with_acceptance_note() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let parent = parent_card(&conn);
        let (ack, card) = spawn_subagent_locked(&conn, &req(Some(parent)), 5_000).unwrap();
        assert!(ack.subagent_id.starts_with("sa_"));
        assert_eq!(ack.status, SubagentStatus::Queued);
        // 一子 agent 一子卡，且 parent_task_id 关联落在行上
        let row = crate::db::find_subagent_by_task(&conn, &ack.task_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.id, ack.subagent_id);
        assert_eq!(row.parent_session_id.as_deref(), Some("main-sess"));
        // 双写人可见侧：子卡 note 含每条验收标准
        let note = card.note.unwrap_or_default();
        assert!(note.contains("覆盖至少 5 个产品"));
        assert!(note.contains("每条含官网 URL"));
        // 预算上卡可见（默认值）
        let budget = card.budget.expect("子卡必须带预算");
        assert_eq!(budget.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(budget.max_tool_calls, DEFAULT_MAX_TOOL_CALLS);
        assert_eq!(budget.max_wall_seconds, DEFAULT_MAX_WALL_SECONDS);
        assert!(card.title.starts_with("子任务："));
        assert_eq!(card.column, crate::db::TaskStatus::Doing);
        // 行侧 acceptance_json（机器侧双写）
        let criteria: Vec<String> =
            serde_json::from_str(row.acceptance_json.as_deref().unwrap()).unwrap();
        assert_eq!(criteria.len(), 2);
    }

    #[test]
    fn spawn_without_parent_card_is_allowed() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let (ack, _card) = spawn_subagent_locked(&conn, &req(None), 5_000).unwrap();
        assert!(ack.task_id != ack.subagent_id);
    }

    #[test]
    fn spawn_with_missing_parent_card_is_task_not_found() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        // Result 的 OK 侧 (SpawnAck, Task) 无 Debug——用 match 拆错误，不 unwrap_err
        let err = match spawn_subagent_locked(&conn, &req(Some("ghost-card".into())), 5_000) {
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
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.objective = "  ".into();
        assert!(spawn_subagent_locked(&conn, &r, 5_000).is_err());
        let mut r2 = req(None);
        r2.acceptance_criteria = Vec::new();
        assert!(spawn_subagent_locked(&conn, &r2, 5_000).is_err());
        let mut r3 = req(None);
        r3.acceptance_criteria = vec!["  ".into()];
        assert!(spawn_subagent_locked(&conn, &r3, 5_000).is_err());
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
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let (ack, _card) = spawn_subagent_locked(&conn, &req(None), 5_000).unwrap();
        let (c, card) = cancel_subagent_locked(&conn, &ack.subagent_id, "用户取消", 6_000).unwrap();
        assert!(c.ok);
        assert!(!c.already_terminal);
        assert_eq!(c.status, SubagentStatus::Cancelled);
        // OCR r2 采纳：取消必须让子卡离开 doing（前端可见），并返回卡片供广播
        let card = card.expect("取消运行中的子 agent 必须回退其子卡");
        assert_eq!(card.column, crate::db::TaskStatus::Todo);
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, SubagentStatus::Cancelled);
        assert_eq!(row.error.as_deref(), Some("用户取消"));
        assert_eq!(row.finished_at, Some(6_000));
        // 终态再取消 = no-op 幂等，不返回卡片（无变更可广播）
        let (again, no_card) =
            cancel_subagent_locked(&conn, &ack.subagent_id, "再取消", 7_000).unwrap();
        assert!(again.already_terminal);
        assert_eq!(again.status, SubagentStatus::Cancelled);
        assert!(no_card.is_none());
    }

    #[test]
    fn cancel_resolves_by_task_id_too() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let (ack, _card) = spawn_subagent_locked(&conn, &req(None), 5_000).unwrap();
        let (c, _) = cancel_subagent_locked(&conn, &ack.task_id, "卡片停止按钮", 6_000).unwrap();
        assert_eq!(c.subagent_id, ack.subagent_id);
        assert_eq!(c.status, SubagentStatus::Cancelled);
    }

    #[test]
    fn cancel_unknown_id_is_invalid_argument() {
        let conn = test_conn();
        // OK 侧 (CancelAck, Option<Task>) 无 Debug——match 拆错误
        let err = match cancel_subagent_locked(&conn, "sa_ghost", "x", 1) {
            Err(e) => e,
            Ok(_) => panic!("未知 id 必须失败"),
        };
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
    }

    #[test]
    fn check_resolves_both_keys_and_counts_progress() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let (ack, _card) = spawn_subagent_locked(&conn, &req(None), 5_000).unwrap();
        // 给子卡补两个 subtask（勾一个）→ progress 小计 1/2
        let mut card = crate::db::load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == ack.task_id)
            .unwrap();
        card.subtasks = Some(vec![
            crate::db::Subtask {
                id: "st1".into(),
                text: "第一步".into(),
                done: true,
            },
            crate::db::Subtask {
                id: "st2".into(),
                text: "第二步".into(),
                done: false,
            },
        ]);
        card.expected_updated_at = card.updated_at;
        card.updated_at = Some(5_500);
        crate::db::upsert_tasks(&conn, std::slice::from_ref(&card)).unwrap();

        let by_id = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        let by_task = check_subagent_locked(&conn, &ack.task_id).unwrap();
        assert_eq!(by_id.subagent_id, ack.subagent_id);
        assert_eq!(by_task.subagent_id, ack.subagent_id);
        assert_eq!(by_id.status, SubagentStatus::Queued);
        assert_eq!(by_id.progress["subtasksDone"], 1);
        assert_eq!(by_id.progress["subtasksTotal"], 2);
        assert!(by_id.result.is_none());
        // 幂等：两次 check 结果一致
        assert_eq!(by_id.status, by_task.status);
    }

    #[test]
    fn check_unknown_key_is_invalid_argument() {
        let conn = test_conn();
        let err = check_subagent_locked(&conn, "sa_ghost").unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
    }

    #[test]
    fn check_returns_parsed_result_json() {
        let conn = test_conn();
        let _g = crate::db::lock_db_write();
        let (ack, _card) = spawn_subagent_locked(&conn, &req(None), 5_000).unwrap();
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
}
