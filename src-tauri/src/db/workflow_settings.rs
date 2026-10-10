//! 工作流偏好设置（设计 §4.3）：`workflow_settings` 键值表。
//!
//! 只放 **后端运行期要读** 的开关（验收开关、审计保留次数）——纯前端偏好
//! （画布偏好、指引段）继续走 localStorage，不进 DB。
//! 默认值在代码里（缺行/解析失败一律落默认），表只存"用户改过的值"。

use serde::Serialize;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

pub const WORKFLOW_SETTINGS_DDL: &str = "CREATE TABLE IF NOT EXISTS workflow_settings (
   key   TEXT PRIMARY KEY,
   value TEXT NOT NULL
 );";

pub fn ensure_workflow_settings(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(WORKFLOW_SETTINGS_DDL)
        .map_err(|e| e.to_string())
}

pub const KEY_NODE_ACCEPTANCE: &str = "node_acceptance";
pub const KEY_AUDIT_RETENTION: &str = "audit_retention_runs";
/// 轻量评审模型：模型库条目 id；空串/缺行 = 跟随全局 active
pub const KEY_REVIEW_MODEL: &str = "review_model";
/// ask_user 提问预算（全局：工作流节点与手动任务卡执行共用，注册时读一次）
pub const KEY_ASK_BUDGET: &str = "ask_budget";

/// 节点级验收默认开；审计保留默认 20 次 run；提问预算默认 3 问
pub const DEFAULT_NODE_ACCEPTANCE: bool = true;
pub const DEFAULT_AUDIT_RETENTION: u32 = 20;
/// 保留次数钳制范围
pub const RETENTION_MIN: u32 = 5;
pub const RETENTION_MAX: u32 = 100;
pub const DEFAULT_ASK_BUDGET: u8 = 3;
pub const ASK_BUDGET_MIN: u8 = 1;
pub const ASK_BUDGET_MAX: u8 = 5;

fn get(conn: &rusqlite::Connection, key: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT value FROM workflow_settings WHERE key = ?1",
        [key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn set(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO workflow_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 节点级验收开关（runner 每次 workflow_run 读一次）
pub fn node_acceptance_enabled(conn: &rusqlite::Connection) -> bool {
    match get(conn, KEY_NODE_ACCEPTANCE) {
        Ok(Some(v)) => v != "0",
        _ => DEFAULT_NODE_ACCEPTANCE,
    }
}

/// 审计保留次数（run_done 后 prune 用）；非法值落默认
pub fn audit_retention_runs(conn: &rusqlite::Connection) -> u32 {
    match get(conn, KEY_AUDIT_RETENTION) {
        Ok(Some(v)) => v.parse::<u32>().unwrap_or(DEFAULT_AUDIT_RETENTION),
        _ => DEFAULT_AUDIT_RETENTION,
    }
}

/// 轻量评审模型条目 id（异步壳：clarify / runner 验收直接调用，读失败降级 None）
pub async fn load_review_model(app: &AppHandle) -> Option<String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::db::open_db(&app)
            .ok()
            .and_then(|conn| review_model_id(&conn))
    })
    .await
    .ok()
    .flatten()
}

/// 轻量评审模型条目 id（clarify 与节点验收核查共用）；空/缺 = None（跟随全局）
pub fn review_model_id(conn: &rusqlite::Connection) -> Option<String> {
    get(conn, KEY_REVIEW_MODEL)
        .ok()
        .flatten()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// ask_user 提问预算（ask_contexts 注册时读一次）；非法值落默认
pub fn ask_budget_per_exec(conn: &rusqlite::Connection) -> u8 {
    match get(conn, KEY_ASK_BUDGET) {
        Ok(Some(v)) => v
            .parse::<u8>()
            .unwrap_or(DEFAULT_ASK_BUDGET)
            .clamp(ASK_BUDGET_MIN, ASK_BUDGET_MAX),
        _ => DEFAULT_ASK_BUDGET,
    }
}

/// 提问预算异步壳（bot_chat 注册提问上下文前读；读失败降级默认）
pub async fn load_ask_budget<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> u8 {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::db::open_db(&app)
            .ok()
            .and_then(|conn| Some(ask_budget_per_exec(&conn)))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(DEFAULT_ASK_BUDGET)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSettingsView {
    pub node_acceptance: bool,
    pub audit_retention_runs: u32,
    /// 模型库条目 id；空串 = 跟随全局 active
    pub review_model: String,
    /// ask_user 提问预算（每次任务执行/工作流节点，1–5）
    pub ask_budget: u8,
}

/// 读全量设置（设置页打开时）
pub fn settings_view(conn: &rusqlite::Connection) -> WorkflowSettingsView {
    WorkflowSettingsView {
        node_acceptance: node_acceptance_enabled(conn),
        audit_retention_runs: audit_retention_runs(conn),
        review_model: review_model_id(conn).unwrap_or_default(),
        ask_budget: ask_budget_per_exec(conn),
    }
}

/// 写设置；保留次数钳 5..=100，提问预算钳 1..=5。None = 不改。
pub fn settings_set(
    conn: &rusqlite::Connection,
    node_acceptance: Option<bool>,
    audit_retention_runs: Option<u32>,
    review_model: Option<String>,
    ask_budget: Option<u8>,
) -> CommandResult<WorkflowSettingsView> {
    // 三键写入同事务：部分失败不落半套设置
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    if let Some(v) = node_acceptance {
        set(&tx, KEY_NODE_ACCEPTANCE, if v { "1" } else { "0" }).map_err(CommandError::DbError)?;
    }
    if let Some(v) = audit_retention_runs {
        let v = v.clamp(RETENTION_MIN, RETENTION_MAX);
        set(&tx, KEY_AUDIT_RETENTION, &v.to_string()).map_err(CommandError::DbError)?;
    }
    if let Some(v) = review_model.clone() {
        // 条目存在性不校验：条目可后删，运行期 summarize 侧降级兜底（spec 红线）
        set(&tx, KEY_REVIEW_MODEL, v.trim()).map_err(CommandError::DbError)?;
    }
    if let Some(v) = ask_budget {
        let v = v.clamp(ASK_BUDGET_MIN, ASK_BUDGET_MAX);
        set(&tx, KEY_ASK_BUDGET, &v.to_string()).map_err(CommandError::DbError)?;
    }
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(settings_view(conn))
}

// tauri 命令

/// 读工作流设置（设置页 + 前端展示）
#[tauri::command]
pub async fn workflow_settings_get(app: AppHandle) -> CommandResult<WorkflowSettingsView> {
    tauri::async_runtime::spawn_blocking(move || -> CommandResult<WorkflowSettingsView> {
        let conn = crate::db::open_db(&app)?;
        Ok(settings_view(&conn))
    })
    .await
    .map_err(|e| CommandError::from(format!("读工作流设置线程 join 失败：{e}")))?
}

/// 写工作流设置；参数均可选（None = 不改）；返回写后全量
#[tauri::command]
pub async fn workflow_settings_set(
    app: AppHandle,
    node_acceptance: Option<bool>,
    audit_retention_runs: Option<u32>,
    review_model: Option<String>,
    ask_budget: Option<u8>,
) -> CommandResult<WorkflowSettingsView> {
    let app2 = app.clone();
    // review_model 要在闭包外（审计事件）再用——闭包 move 捕获前先克隆一份
    let review_model_for_db = review_model.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> CommandResult<WorkflowSettingsView> {
        let conn = crate::db::open_db(&app2)?;
        let view = settings_set(
            &conn,
            node_acceptance,
            audit_retention_runs,
            review_model_for_db,
            ask_budget,
        )?;
        // 保留次数变更立即生效：超期 run 就地清理（尽力而为）
        if audit_retention_runs.is_some() {
            if let Err(e) = super::workflow_audit::wa_prune(&conn, view.audit_retention_runs) {
                eprintln!("[workflow_settings] 保留清理失败（不阻断）：{e}");
            }
        }
        Ok(view)
    })
    .await
    .map_err(|e| CommandError::from(format!("写工作流设置线程 join 失败：{e}")))??;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "workflow_settings_set",
        &[
            (
                "nodeAcceptance",
                node_acceptance
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            (
                "auditRetention",
                // Some(_) 记钳制后的实际生效值（view）：原始入参可超范围（如传 1000 实际生效 100），
                // 审计轨迹必须与实际生效一致，不能记原始值失真
                if audit_retention_runs.is_some() {
                    r.audit_retention_runs.to_string()
                } else {
                    "-".to_string()
                },
            ),
            (
                "reviewModel",
                // None=未提交此字段（"-"）；Some("")=显式清除（"cleared"）——审计可区分
                match review_model.as_deref().map(str::trim) {
                    None => "-".to_string(),
                    Some("") => "cleared".to_string(),
                    Some(v) => v.to_string(),
                },
            ),
            (
                // 同 retention：记钳制后的实际生效值
                "askBudget",
                if ask_budget.is_some() {
                    r.ask_budget.to_string()
                } else {
                    "-".to_string()
                },
            ),
        ],
    );
    Ok(r)
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_workflow_settings(&conn).unwrap();
        conn
    }

    /// 锁定审计口径依赖的语义：超范围入参存库与 view 一律是钳制后的实际生效值
    /// （审计事件记 view.audit_retention_runs，必须等于真实生效值而非原始入参）
    #[test]
    fn retention_stored_and_viewed_clamped() {
        let conn = mem_conn();
        let view = settings_set(&conn, None, Some(1000), None, None).unwrap();
        assert_eq!(view.audit_retention_runs, RETENTION_MAX);
        assert_eq!(audit_retention_runs(&conn), RETENTION_MAX);
        let view = settings_set(&conn, None, Some(1), None, None).unwrap();
        assert_eq!(view.audit_retention_runs, RETENTION_MIN);
        assert_eq!(audit_retention_runs(&conn), RETENTION_MIN);
    }

    /// 提问预算：默认 3、写超界钳到 1..=5、缺行落默认（全局口径：工作流与任务卡共用）
    #[test]
    fn ask_budget_default_and_clamped() {
        let conn = mem_conn();
        assert_eq!(ask_budget_per_exec(&conn), DEFAULT_ASK_BUDGET);
        let view = settings_set(&conn, None, None, None, Some(9)).unwrap();
        assert_eq!(view.ask_budget, ASK_BUDGET_MAX);
        let view = settings_set(&conn, None, None, None, Some(0)).unwrap();
        assert_eq!(view.ask_budget, ASK_BUDGET_MIN);
        let view = settings_set(&conn, None, None, None, Some(4)).unwrap();
        assert_eq!(view.ask_budget, 4);
        assert_eq!(ask_budget_per_exec(&conn), 4);
    }
}
