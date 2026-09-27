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

/// spawn 入参（设计 §5 工具签名的内部形态；工具层参数解析在 SUBA-2）。
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub objective: String,
    pub profile: SubagentProfile,
    /// **spawn 必填**，每条可检验（设计 §4.2 双写：子卡 note + 任务包装）
    pub acceptance_criteria: Vec<String>,
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

fn acceptance_note(criteria: &[String]) -> String {
    let mut note = String::from("【验收标准】\n");
    for c in criteria {
        note.push_str(&format!("- {c}\n"));
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
        id: task_id.clone(),
        title: format!(
            "🧩 子任务：{}",
            crate::bot::truncate_for_log(req.objective.trim(), 30)
        ),
        due: None,
        note: Some(acceptance_note(&req.acceptance_criteria)),
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

/// 按 subagent_id 或 task_id 解析行（设计 §5：check 接受双键；cancel/stop 按钮同款）。
/// 前缀约定：subagent_id 恒以 `sa_` 开头、task_id 恒为裸 uuid（无前缀）——
/// 若未来 task_id 引入前缀，此处路由必须同步改。
fn resolve_row(conn: &rusqlite::Connection, key: &str) -> CommandResult<SubagentRow> {
    let row = if key.starts_with("sa_") {
        crate::db::load_subagent(conn, key).map_err(CommandError::DbError)?
    } else {
        crate::db::find_subagent_by_task(conn, key).map_err(CommandError::DbError)?
    };
    row.ok_or_else(|| CommandError::InvalidArgument {
        field: "subagent_id".into(),
        value: key.to_string(),
        reason: "subagent 不存在（subagent_id 或 task_id 均未命中）".into(),
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
    Ok(CheckState {
        subagent_id: row.id,
        task_id: row.task_id,
        session_id: row.session_id,
        status: row.status,
        progress: serde_json::json!({
            "subtasksDone": subtasks_done,
            "subtasksTotal": subtasks_total,
        }),
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
pub async fn spawn_subagent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    req: SpawnRequest,
) -> CommandResult<SpawnAck> {
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
    Ok(ack)
}

/// check 异步包装：wait_ms=0 单次读；>0 轮询至终态或超时（幂等可轮询，设计 §5）。
pub async fn check_subagent<R: tauri::Runtime>(
    app: &AppHandle<R>,
    key: &str,
    wait_ms: u64,
) -> CommandResult<CheckState> {
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
pub async fn cancel_subagent<R: tauri::Runtime>(
    app: &AppHandle<R>,
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
    }
    Ok(ack)
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // tasks 表内联最小 schema（列序与 open_db 一致，含 SUBA-1 新增三列）；
        // subagents 表与生产同源 DDL（单源真相）
        conn.execute_batch(
            "CREATE TABLE tasks (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, due TEXT, note TEXT, tags TEXT,
               file_path TEXT, file_is_dir INTEGER, col TEXT NOT NULL, subtasks TEXT,
               completed_at INTEGER, archived INTEGER, deleted_at INTEGER, collapsed INTEGER,
               ord REAL, updated_at INTEGER, schedule TEXT, sched_last INTEGER,
               bot_assigned INTEGER, files TEXT, assignee TEXT, budget TEXT, result TEXT );",
        )
        .unwrap();
        conn.execute_batch(crate::db::SUBAGENTS_DDL).unwrap();
        conn
    }

    fn parent_card(conn: &rusqlite::Connection) -> String {
        let card = Task {
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
            expected_updated_at: None,
        };
        crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).unwrap();
        "parent-1".into()
    }

    fn req(parent: Option<String>) -> SpawnRequest {
        SpawnRequest {
            objective: "调研五个竞品并输出报告".into(),
            profile: SubagentProfile::Research,
            acceptance_criteria: vec!["覆盖至少 5 个产品".into(), "每条含官网 URL".into()],
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
        assert!(card.title.starts_with("🧩 子任务："));
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
