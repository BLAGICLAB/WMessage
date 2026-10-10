//! run 级结构化审计（设计 §4.2）：`workflow_audit` 表按
//! `(workflow_id, run_started_at)` 分组——**不建 runs 实体表**，二元组即 run 分组键。
//!
//! 红线：
//! - 写入尽力而为：单条失败由调用方降级（eprintln），绝不阻断执行
//! - bot.log 行式事件保持双写（兼容既有 grep 工具流），本表是可查询面
//! - 保留策略：最近 N 个 distinct run（默认 20，`workflow_settings` 可调）

use serde::Serialize;
use tauri::AppHandle;

use crate::error::CommandResult;

/// 单源 DDL：open_db 与测试建表共用
pub const WORKFLOW_AUDIT_DDL: &str = "CREATE TABLE IF NOT EXISTS workflow_audit (
   id              INTEGER PRIMARY KEY AUTOINCREMENT,
   workflow_id     TEXT NOT NULL,
   run_started_at  INTEGER NOT NULL,
   node_task_id    TEXT,
   kind            TEXT NOT NULL,
   level           TEXT NOT NULL DEFAULT 'info',
   payload         TEXT NOT NULL DEFAULT '{}',
   created_at      INTEGER NOT NULL
 );
CREATE INDEX IF NOT EXISTS idx_workflow_audit_run
   ON workflow_audit(workflow_id, run_started_at);";

pub fn ensure_workflow_audit(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(WORKFLOW_AUDIT_DDL)
        .map_err(|e| e.to_string())
}

/// kind 取值（11 种，设计 §4.2）
pub const KIND_RUN_START: &str = "run_start";
pub const KIND_NODE_START: &str = "node_start";
pub const KIND_NODE_RESULT: &str = "node_result";
pub const KIND_ACCEPTANCE_CHECK: &str = "acceptance_check";
pub const KIND_REWORK: &str = "rework";
pub const KIND_REVIEW: &str = "review";
pub const KIND_REWORK_ROUND: &str = "rework_round";
pub const KIND_RUN_DONE: &str = "run_done";
pub const KIND_STOP: &str = "stop";
pub const KIND_QUESTION_ASKED: &str = "question_asked";
pub const KIND_QUESTION_ANSWERED: &str = "question_answered";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: i64,
    pub workflow_id: String,
    pub run_started_at: i64,
    pub node_task_id: Option<String>,
    pub kind: String,
    pub level: String,
    pub payload: serde_json::Value,
    pub created_at: i64,
}

fn chars_truncate(s: &str, cap: usize) -> String {
    s.chars().take(cap).collect()
}

/// 落一条审计（Result 返回，调用方尽力而为）；payload 序列化失败 → 存错误占位。
/// kind/level 限 40 字符（枚举值本就短，防误传长文本）。
pub fn wa_insert(
    conn: &rusqlite::Connection,
    workflow_id: &str,
    run_started_at: i64,
    node_task_id: Option<&str>,
    kind: &str,
    level: &str,
    payload: &serde_json::Value,
) -> Result<i64, String> {
    if workflow_id.trim().is_empty() {
        return Err("workflow_id 不能为空".into());
    }
    let payload_str = serde_json::to_string(payload)
        .unwrap_or_else(|e| format!("{{\"_serialize_error\":\"{e}\"}}"));
    conn.execute(
        "INSERT INTO workflow_audit (workflow_id, run_started_at, node_task_id, kind, level, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            workflow_id,
            run_started_at,
            node_task_id,
            chars_truncate(kind, 40),
            chars_truncate(level, 40),
            payload_str,
            chrono::Utc::now().timestamp_millis()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    let payload_raw: String = r.get(6)?;
    let id: i64 = r.get(0)?;
    // 单行 payload 损坏降级不炸列表（同 notifications 口径：stderr 留痕可诊断）
    let payload = serde_json::from_str(&payload_raw).unwrap_or_else(|e| {
        eprintln!("[workflow_audit] payload JSON 损坏（id={id}）：{e}");
        serde_json::Value::Null
    });
    Ok(AuditEntry {
        id,
        workflow_id: r.get(1)?,
        run_started_at: r.get(2)?,
        node_task_id: r.get(3)?,
        kind: r.get(4)?,
        level: r.get(5)?,
        payload,
        created_at: r.get(7)?,
    })
}

const ENTRY_COLS: &str =
    "id, workflow_id, run_started_at, node_task_id, kind, level, payload, created_at";

/// 列表：某工作流全部 run，时间倒序（新→旧），limit 钳 1..=500
pub fn wa_list(
    conn: &rusqlite::Connection,
    workflow_id: &str,
    limit: u32,
) -> Result<Vec<AuditEntry>, String> {
    let limit = limit.clamp(1, 500);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ENTRY_COLS} FROM workflow_audit
             WHERE workflow_id = ?1
             ORDER BY id DESC LIMIT {limit}"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([workflow_id], row_to_entry)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 导出专用：全量读取，不带 LIMIT——列表接口的 1..=500 钳制是为 UI 分页，
/// 导出是落盘全量，多 run × 多节点事件可超 500 条，复用列表会静默截断导出文件。
/// 体量由保留策略兜底（wa_prune 只留最近 5..=100 个 run）。
pub fn wa_export_list(
    conn: &rusqlite::Connection,
    workflow_id: &str,
) -> Result<Vec<AuditEntry>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ENTRY_COLS} FROM workflow_audit
             WHERE workflow_id = ?1
             ORDER BY id DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([workflow_id], row_to_entry)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 保留清理：只留最近 keep_runs 个 distinct run（跨工作流全局计）。
/// 返回删除行数；keep=0 视为清空（settings 钳制在 5..=100，此分支仅测试用）。
pub fn wa_prune(conn: &rusqlite::Connection, keep_runs: u32) -> Result<usize, String> {
    if keep_runs == 0 {
        return conn
            .execute("DELETE FROM workflow_audit", [])
            .map_err(|e| e.to_string());
    }
    let n = conn
        .execute(
            "DELETE FROM workflow_audit WHERE run_started_at NOT IN (
             SELECT DISTINCT run_started_at FROM workflow_audit
             ORDER BY run_started_at DESC LIMIT ?1
         )",
            [keep_runs],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// 清空某工作流全部审计（设置页「清空审计」）；返回删除行数
pub fn wa_clear_workflow(conn: &rusqlite::Connection, workflow_id: &str) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM workflow_audit WHERE workflow_id = ?1",
        [workflow_id],
    )
    .map_err(|e| e.to_string())
}

/// 清空全部工作流审计（设置页「清空全部」；不按工作流区分）
pub fn wa_clear_all(conn: &rusqlite::Connection) -> Result<usize, String> {
    conn.execute("DELETE FROM workflow_audit", [])
        .map_err(|e| e.to_string())
}

/// 清空全部工作流审计；返回删除行数
#[tauri::command]
pub async fn workflow_audit_clear_all(app: AppHandle) -> CommandResult<usize> {
    let app2 = app.clone();
    let n = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        wa_clear_all(&conn)
    })
    .await
    .map_err(|e| crate::error::CommandError::from(format!("清空审计线程 join 失败：{e}")))?
    .map_err(crate::error::CommandError::DbError)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "workflow_audit_cleared",
        &[("scope", "all".into()), ("rows", n.to_string())],
    );
    Ok(n)
}

// tauri 命令

/// 审计列表（新→旧，limit 缺省 200 上限 500）
#[tauri::command]
pub async fn workflow_audit_list(
    app: AppHandle,
    workflow_id: String,
    limit: Option<u32>,
) -> CommandResult<Vec<AuditEntry>> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<AuditEntry>, String> {
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        wa_list(&conn, &workflow_id, limit.unwrap_or(200))
    })
    .await;
    r.map_err(|e| crate::error::CommandError::from(format!("审计列表线程 join 失败：{e}")))?
        .map_err(crate::error::CommandError::DbError)
}

/// 清空某工作流审计（设置页手动）；返回删除行数
#[tauri::command]
pub async fn workflow_audit_clear(app: AppHandle, workflow_id: String) -> CommandResult<usize> {
    let app2 = app.clone();
    let wid2 = workflow_id.clone();
    let n = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        wa_clear_workflow(&conn, &wid2)
    })
    .await
    .map_err(|e| crate::error::CommandError::from(format!("清空审计线程 join 失败：{e}")))?
    .map_err(crate::error::CommandError::DbError)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "workflow_audit_cleared",
        &[("workflowId", workflow_id), ("rows", n.to_string())],
    );
    Ok(n)
}

/// 审计导出 JSON（画布工具栏「审计导出」→ save dialog 路径）
#[tauri::command]
pub async fn workflow_audit_export(
    app: AppHandle,
    workflow_id: String,
    path: String,
) -> CommandResult<usize> {
    use crate::error::CommandResult;
    // 路径闸门的 fs 调用进阻塞线程（不占 Tokio worker）
    let path_gate = path.clone();
    crate::py::document::spawn_blocking_map(move || {
        crate::db::tasks::check_export_path(&path_gate).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| crate::error::CommandError::from(format!("审计路径校验线程 join 失败：{e}")))?;
    let app2 = app.clone();
    let wid2 = workflow_id.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> CommandResult<(usize, String)> {
        let conn = crate::db::open_db(&app2)?;
        let rows = wa_export_list(&conn, &wid2).map_err(crate::error::CommandError::DbError)?;
        let n = rows.len();
        let json = serde_json::to_string_pretty(&rows)
            .map_err(|e| crate::error::CommandError::from(e.to_string()))?;
        Ok((n, json))
    })
    .await
    .map_err(|e| crate::error::CommandError::from(format!("审计导出线程 join 失败：{e}")))??;
    let (n, json) = r;
    std::fs::write(&path, json).map_err(|e| crate::error::CommandError::from(e.to_string()))?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "workflow_audit_exported",
        &[
            ("workflowId", workflow_id),
            ("rows", n.to_string()),
            ("path", crate::audit::escape_for_log(&path, 120)),
        ],
    );
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_workflow_audit(&conn).unwrap();
        conn
    }

    #[test]
    fn insert_and_list_roundtrip() {
        let conn = mem_conn();
        wa_insert(
            &conn,
            "wf1",
            1000,
            Some("t1"),
            KIND_NODE_RESULT,
            "info",
            &serde_json::json!({ "status": "success", "attempt": 2, "ms": 1234 }),
        )
        .unwrap();
        wa_insert(
            &conn,
            "wf1",
            1000,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        wa_insert(
            &conn,
            "wf2",
            2000,
            None,
            KIND_RUN_DONE,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        let rows = wa_list(&conn, "wf1", 50).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, KIND_RUN_START); // id 倒序
        assert_eq!(rows[1].payload["attempt"], 2);
        assert_eq!(rows[1].node_task_id.as_deref(), Some("t1"));
        assert!(wa_list(&conn, "wf1", 0).is_ok() && wa_list(&conn, "wf1", 1).unwrap().len() == 1);
        // 空 workflow_id 拒绝
        assert!(wa_insert(
            &conn,
            "  ",
            1,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({})
        )
        .is_err());
    }

    #[test]
    fn payload_serialize_failure_degrades_to_placeholder() {
        let conn = mem_conn();
        // 非 stringify-able payload 现实不可达（json! 宏产物都可序列化），
        // 这里锁的是降级路径本身：损坏 payload 读回为 Null 不炸列表
        wa_insert(
            &conn,
            "wf1",
            1,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({ "a": 1 }),
        )
        .unwrap();
        conn.execute(
            "UPDATE workflow_audit SET payload = 'not-json' WHERE id = 1",
            [],
        )
        .unwrap();
        let rows = wa_list(&conn, "wf1", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].payload.is_null());
    }

    #[test]
    fn prune_keeps_latest_n_distinct_runs() {
        let conn = mem_conn();
        // 3 个 run：1000(2 条) / 2000(1 条) / 3000(1 条)
        wa_insert(
            &conn,
            "wf1",
            1000,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        wa_insert(
            &conn,
            "wf1",
            1000,
            None,
            KIND_RUN_DONE,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        wa_insert(
            &conn,
            "wf1",
            2000,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        wa_insert(
            &conn,
            "wf1",
            3000,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(wa_prune(&conn, 2).unwrap(), 2); // 1000 的 2 条被清
        let rows = wa_list(&conn, "wf1", 50).unwrap();
        assert_eq!(rows.len(), 2);
        let runs: Vec<i64> = rows.iter().map(|r| r.run_started_at).collect();
        assert!(runs.contains(&2000) && runs.contains(&3000));
        assert_eq!(wa_prune(&conn, 0).unwrap(), 2); // 全清
        assert!(wa_list(&conn, "wf1", 50).unwrap().is_empty());
    }

    /// 导出全量回归：超 500 条时 wa_list 钳制截断，wa_export_list 必须全量返回，
    /// 导出文件不得静默丢行
    #[test]
    fn export_list_not_clamped_by_list_limit() {
        let conn = mem_conn();
        for i in 0..505 {
            wa_insert(
                &conn,
                "wf1",
                1000 + i,
                None,
                KIND_NODE_START,
                "info",
                &serde_json::json!({ "i": i }),
            )
            .unwrap();
        }
        assert_eq!(wa_list(&conn, "wf1", 500).unwrap().len(), 500, "列表钳 500");
        assert_eq!(
            wa_export_list(&conn, "wf1").unwrap().len(),
            505,
            "导出全量不截断"
        );
    }

    #[test]
    fn clear_scoped_to_workflow() {
        let conn = mem_conn();
        wa_insert(
            &conn,
            "wf1",
            1,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        wa_insert(
            &conn,
            "wf2",
            2,
            None,
            KIND_RUN_START,
            "info",
            &serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(wa_clear_workflow(&conn, "wf1").unwrap(), 1);
        assert_eq!(wa_list(&conn, "wf1", 10).unwrap().len(), 0);
        assert_eq!(wa_list(&conn, "wf2", 10).unwrap().len(), 1);
    }
}
