//! Agent 通知中心（持久化消息）。
//!
//! 三类「需要用户决策」的 Agent 事件统一落 `notifications` 表，以消息形式
//! 在主窗口通知页呈现——每条消息独立成行、持久化、互不覆盖：
//! - `memory_proposal`：记忆待确认提案（`memory::extract` confirm 档入队）
//! - `evolution_proposal`：自进化提案入池（`evolution` 反思派生、未被自动应用的）
//! - `artifact_bind`：任务卡执行产物待绑定（原挂件 ArtifactBatchDialog 弹窗迁移）
//!
//! 幂等：id 由各来源生成（`memory:{session}:{ts}` / `evo:{proposal_id}` /
//! `artifact:{task_id}:{sid}`），`INSERT OR IGNORE` 保证同 id 不重复。
//! 操作面：通知页按钮 + 设置页面板（MemoryPanel / EvolutionPanel）双入口，
//! 既有命令尾部挂 resolution 回写，保证任一处操作后消息状态一致；
//! 每次变更广播 `notifications-changed`，前端刷新列表与导航角标。

use crate::error::{CommandError, CommandResult};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

pub const EVENT_CHANGED: &str = "notifications-changed";

/// 待处理状态（通知页默认页签）
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_DONE: &str = "done";
pub const STATUS_DISMISSED: &str = "dismissed";

pub const KIND_MEMORY: &str = "memory_proposal";
pub const KIND_EVOLUTION: &str = "evolution_proposal";
pub const KIND_ARTIFACT: &str = "artifact_bind";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NotificationView {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub payload: Value,
    pub status: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn ensure_table(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS notifications(
           id          TEXT PRIMARY KEY,
           kind        TEXT NOT NULL,
           title       TEXT NOT NULL,
           body        TEXT NOT NULL DEFAULT '',
           payload     TEXT NOT NULL,
           status      TEXT NOT NULL DEFAULT 'pending',
           created_at  TEXT NOT NULL,
           resolved_at TEXT
         );",
    )
    .map_err(|e| e.to_string())
}

/// 幂等插入：同 id 已存在则跳过（含已处理的——操作过的消息不被同源事件复活）
pub fn notif_insert(
    conn: &rusqlite::Connection,
    id: &str,
    kind: &str,
    title: &str,
    body: &str,
    payload: &Value,
) -> Result<bool, String> {
    ensure_table(conn)?;
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO notifications (id, kind, title, body, payload, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                id,
                kind,
                title,
                body,
                payload.to_string(),
                STATUS_PENDING,
                now_ms().to_string()
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

fn row_to_view(r: &rusqlite::Row) -> rusqlite::Result<NotificationView> {
    let payload_raw: String = r.get(4)?;
    Ok(NotificationView {
        id: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        body: r.get(3)?,
        payload: serde_json::from_str(&payload_raw).unwrap_or(Value::Null),
        status: r.get(5)?,
        created_at: r.get::<_, String>(6)?.parse::<i64>().unwrap_or_default(),
        resolved_at: r
            .get::<_, Option<String>>(7)?
            .and_then(|s| s.parse::<i64>().ok()),
    })
}

/// 列表（新→旧）；status=None 全量
fn notif_list(
    conn: &rusqlite::Connection,
    status: Option<&str>,
) -> Result<Vec<NotificationView>, String> {
    ensure_table(conn)?;
    let (sql, param): (&str, Vec<&str>) = match status {
        Some(s) => (
            "SELECT id, kind, title, body, payload, status, created_at, resolved_at
             FROM notifications WHERE status = ?1 ORDER BY created_at DESC, rowid DESC",
            vec![s],
        ),
        None => (
            "SELECT id, kind, title, body, payload, status, created_at, resolved_at
             FROM notifications ORDER BY created_at DESC, rowid DESC",
            vec![],
        ),
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(param), row_to_view)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 解决一条通知（done / dismissed）；不存在时静默成功（幂等）。
/// pub(crate)：evolution 面板命令的操作回写也走这里。
pub(crate) fn notif_resolve(
    conn: &rusqlite::Connection,
    id: &str,
    status: &str,
) -> Result<(), String> {
    ensure_table(conn)?;
    conn.execute(
        "UPDATE notifications SET status = ?2, resolved_at = ?3
         WHERE id = ?1 AND status = 'pending'",
        rusqlite::params![id, status, now_ms().to_string()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 按 taskId 解决 pending 的 artifact_bind 通知（confirm_artifact_batch 成功后调用）
pub fn notif_resolve_artifact(conn: &rusqlite::Connection, task_id: &str) -> Result<(), String> {
    ensure_table(conn)?;
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, payload FROM notifications
                 WHERE kind = ?1 AND status = 'pending'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([KIND_ARTIFACT], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(_, p)| {
                serde_json::from_str::<Value>(p)
                    .ok()
                    .and_then(|v| v.get("taskId").and_then(Value::as_str).map(String::from))
                    .as_deref()
                    == Some(task_id)
            })
            .map(|(id, _)| id)
            .collect()
    };
    for id in ids {
        notif_resolve(conn, &id, STATUS_DONE)?;
    }
    Ok(())
}

/// 记忆提案通知同步（`mem_pending_approve` / `mem_pending_reject` 尾部调用）：
/// 遍历 pending 的 memory_proposal 通知，把 payload.ids 中已不在 mem_pending 表的
/// id 剔除；剔完为空则整条按 `resolved_status`（approve→done / reject→dismissed）
/// 解决。局部收下/忽略同样正确。
pub fn notif_sync_memory(conn: &rusqlite::Connection, resolved_status: &str) -> Result<(), String> {
    ensure_table(conn)?;
    let entries: Vec<(String, Value)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, payload FROM notifications
                 WHERE kind = ?1 AND status = 'pending'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([KIND_MEMORY], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(id, p)| (id, serde_json::from_str::<Value>(&p).unwrap_or(Value::Null)))
            .collect()
    };
    for (id, payload) in entries {
        let Some(ids) = payload.get("ids").and_then(Value::as_array) else {
            continue;
        };
        let mut remaining: Vec<i64> = Vec::new();
        for v in ids {
            let Some(pid) = v.as_i64() else { continue };
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM mem_pending WHERE id = ?1",
                    [pid],
                    |r| r.get::<_, i64>(0),
                )
                .map(|c| c > 0)
                .unwrap_or(false);
            if exists {
                remaining.push(pid);
            }
        }
        if remaining.is_empty() {
            notif_resolve(conn, &id, resolved_status)?;
        } else if remaining.len() != ids.len() {
            let new_payload = serde_json::json!({ "ids": remaining });
            conn.execute(
                "UPDATE notifications SET payload = ?2 WHERE id = ?1",
                rusqlite::params![id, new_payload.to_string()],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 变更广播（插入/解决后调用；无 AppHandle 场景静默跳过——测试环境）
pub fn emit_changed<R: tauri::Runtime>(app: &AppHandle<R>) {
    let _ = app.emit(EVENT_CHANGED, serde_json::json!({}));
}

// ───────────────────────── tauri 命令 ─────────────────────────

/// 通知列表（新→旧）；status 传 "pending"/"done"/"dismissed"，缺省全量
#[tauri::command]
pub async fn notifications_list(
    app: AppHandle,
    status: Option<String>,
) -> CommandResult<Vec<NotificationView>> {
    let r =
        tauri::async_runtime::spawn_blocking(move || -> Result<Vec<NotificationView>, String> {
            let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
            notif_list(&conn, status.as_deref())
        })
        .await;
    r.map_err(|e| CommandError::from(format!("通知列表线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 待处理通知数（导航栏角标）
#[tauri::command]
pub async fn notifications_pending_count(app: AppHandle) -> CommandResult<usize> {
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let conn = crate::db::open_db(&app).map_err(|e| e.to_string())?;
        ensure_table(&conn)?;
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE status = 'pending'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(n as usize)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("通知计数线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)
}

/// 解决一条通知：status 仅允许 done / dismissed（通知页「忽略/跳过」走这里）
#[tauri::command]
pub async fn notifications_resolve(
    app: AppHandle,
    id: String,
    status: String,
) -> CommandResult<()> {
    if status != STATUS_DONE && status != STATUS_DISMISSED {
        return Err(CommandError::from(format!(
            "非法通知状态：{status}（仅允许 done/dismissed）"
        )));
    }
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        notif_resolve(&conn, &id, &status)
    })
    .await;
    r.map_err(|e| CommandError::from(format!("通知解决线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)?;
    emit_changed(&app);
    Ok(())
}

/// 清空已处理消息（done + dismissed）
#[tauri::command]
pub async fn notifications_clear_done(app: AppHandle) -> CommandResult<usize> {
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        ensure_table(&conn)?;
        conn.execute(
            "DELETE FROM notifications WHERE status IN ('done','dismissed')",
            [],
        )
        .map_err(|e| e.to_string())
    })
    .await;
    let n = r
        .map_err(|e| CommandError::from(format!("清空通知线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)?;
    emit_changed(&app);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // mem_pending 表供 sync 用例使用（同 memory::extract 建表语句）
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS mem_pending(
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               content TEXT NOT NULL, kind TEXT NOT NULL,
               importance INTEGER NOT NULL DEFAULT 2,
               session_id TEXT NOT NULL, created_at TEXT NOT NULL);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn insert_idempotent_same_id() {
        let conn = mem_conn();
        let payload = serde_json::json!({ "ids": [1, 2] });
        assert!(notif_insert(&conn, "memory:s1:1", KIND_MEMORY, "t", "b", &payload).unwrap());
        // 同 id 二次插入被 IGNORE，且已处理状态也不复活
        assert!(!notif_insert(&conn, "memory:s1:1", KIND_MEMORY, "t", "b", &payload).unwrap());
        notif_resolve(&conn, "memory:s1:1", STATUS_DONE).unwrap();
        assert!(!notif_insert(&conn, "memory:s1:1", KIND_MEMORY, "t", "b", &payload).unwrap());
        assert_eq!(notif_list(&conn, None).unwrap().len(), 1);
        assert_eq!(notif_list(&conn, None).unwrap()[0].status, STATUS_DONE);
    }

    #[test]
    fn resolve_only_pending() {
        let conn = mem_conn();
        notif_insert(&conn, "evo:p1", KIND_EVOLUTION, "t", "", &Value::Null).unwrap();
        notif_resolve(&conn, "evo:p1", STATUS_DISMISSED).unwrap();
        // 已解决后再 resolve（幂等保护：只改 pending 行）不覆盖 resolved_at 语义
        notif_resolve(&conn, "evo:p1", STATUS_DONE).unwrap();
        let v = notif_list(&conn, None).unwrap();
        assert_eq!(v[0].status, STATUS_DISMISSED);
    }

    #[test]
    fn sync_memory_resolves_when_all_processed() {
        let conn = mem_conn();
        conn.execute(
            "INSERT INTO mem_pending (content, kind, session_id, created_at) VALUES ('a','fact','s','1')",
            [],
        )
        .unwrap();
        let pid: i64 = conn.last_insert_rowid();
        notif_insert(
            &conn,
            "memory:s1:1",
            KIND_MEMORY,
            "t",
            "",
            &serde_json::json!({ "ids": [pid] }),
        )
        .unwrap();
        // 队列行被收下删除 → 通知整条 done
        conn.execute("DELETE FROM mem_pending WHERE id = ?1", [pid])
            .unwrap();
        notif_sync_memory(&conn, STATUS_DONE).unwrap();
        assert_eq!(notif_list(&conn, None).unwrap()[0].status, STATUS_DONE);
    }

    #[test]
    fn sync_memory_trims_partial_and_keeps_pending() {
        let conn = mem_conn();
        for c in ["a", "b"] {
            conn.execute(
                "INSERT INTO mem_pending (content, kind, session_id, created_at) VALUES (?1,'fact','s','1')",
                [c],
            )
            .unwrap();
        }
        let (p1, p2) = {
            let mut stmt = conn
                .prepare("SELECT id FROM mem_pending ORDER BY id")
                .unwrap();
            let rows: Vec<i64> = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            (rows[0], rows[1])
        };
        notif_insert(
            &conn,
            "memory:s1:1",
            KIND_MEMORY,
            "t",
            "",
            &serde_json::json!({ "ids": [p1, p2] }),
        )
        .unwrap();
        // 只收下 p1：通知保留 pending，payload 收缩为 [p2]
        conn.execute("DELETE FROM mem_pending WHERE id = ?1", [p1])
            .unwrap();
        notif_sync_memory(&conn, STATUS_DONE).unwrap();
        let v = notif_list(&conn, Some(STATUS_PENDING)).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].payload["ids"], serde_json::json!([p2]));
    }

    #[test]
    fn resolve_artifact_by_task_id() {
        let conn = mem_conn();
        notif_insert(
            &conn,
            "artifact:t1:s1",
            KIND_ARTIFACT,
            "t",
            "",
            &serde_json::json!({ "taskId": "t1", "paths": ["/a"] }),
        )
        .unwrap();
        notif_insert(
            &conn,
            "artifact:t2:s2",
            KIND_ARTIFACT,
            "t",
            "",
            &serde_json::json!({ "taskId": "t2", "paths": ["/b"] }),
        )
        .unwrap();
        notif_resolve_artifact(&conn, "t1").unwrap();
        let pending = notif_list(&conn, Some(STATUS_PENDING)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "artifact:t2:s2");
        let done = notif_list(&conn, Some(STATUS_DONE)).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].id, "artifact:t1:s1");
    }
}
