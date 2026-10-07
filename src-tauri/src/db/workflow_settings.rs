//! 工作流偏好设置（W10-QA-AUDIT 设计 §4.3）：`workflow_settings` 键值表。
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

/// 节点级验收默认开（拍板 10）；审计保留默认 20 次 run
pub const DEFAULT_NODE_ACCEPTANCE: bool = true;
pub const DEFAULT_AUDIT_RETENTION: u32 = 20;
/// 保留次数钳制范围
pub const RETENTION_MIN: u32 = 5;
pub const RETENTION_MAX: u32 = 100;

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSettingsView {
    pub node_acceptance: bool,
    pub audit_retention_runs: u32,
}

/// 读全量设置（设置页打开时）
pub fn settings_view(conn: &rusqlite::Connection) -> WorkflowSettingsView {
    WorkflowSettingsView {
        node_acceptance: node_acceptance_enabled(conn),
        audit_retention_runs: audit_retention_runs(conn),
    }
}

/// 写设置；保留次数钳 5..=100。node_acceptance=None = 不改。
pub fn settings_set(
    conn: &rusqlite::Connection,
    node_acceptance: Option<bool>,
    audit_retention_runs: Option<u32>,
) -> CommandResult<WorkflowSettingsView> {
    if let Some(v) = node_acceptance {
        set(conn, KEY_NODE_ACCEPTANCE, if v { "1" } else { "0" }).map_err(CommandError::DbError)?;
    }
    if let Some(v) = audit_retention_runs {
        let v = v.clamp(RETENTION_MIN, RETENTION_MAX);
        set(conn, KEY_AUDIT_RETENTION, &v.to_string()).map_err(CommandError::DbError)?;
    }
    Ok(settings_view(conn))
}

// ────────────── tauri 命令 ──────────────

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

/// 写工作流设置；两参均可选（None = 不改）；返回写后全量
#[tauri::command]
pub async fn workflow_settings_set(
    app: AppHandle,
    node_acceptance: Option<bool>,
    audit_retention_runs: Option<u32>,
) -> CommandResult<WorkflowSettingsView> {
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> CommandResult<WorkflowSettingsView> {
        let conn = crate::db::open_db(&app2)?;
        let view = settings_set(&conn, node_acceptance, audit_retention_runs)?;
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
                audit_retention_runs
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
        ],
    );
    Ok(r)
}

use rusqlite::OptionalExtension;
