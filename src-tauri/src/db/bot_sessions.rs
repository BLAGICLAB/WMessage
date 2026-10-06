//! 机器人多会话元数据

use serde::{Deserialize, Serialize};
use tauri::async_runtime;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotMsgRow {
    pub role: String,
    pub content: String,
    pub refs_json: Option<String>,
    pub thinking: Option<String>,
    pub tools_json: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotSession {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// 子 agent 执行会话标记（U20D 批 5：会话路由从「标题 🧩 前缀」升级为
    /// 结构化字段；runner 建会话时置 1，普通会话/存量旧会话为 0）
    pub is_subagent: bool,
}

#[tauri::command]
pub fn bot_sessions_load(app: AppHandle) -> CommandResult<Vec<BotSession>> {
    let conn = super::open_db(&app)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, created_at, updated_at, is_subagent FROM bot_sessions ORDER BY updated_at DESC",
        )
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BotSession {
                id: r.get::<_, String>(0)?,
                title: r.get::<_, String>(1)?,
                created_at: r.get::<_, i64>(2)?,
                updated_at: r.get::<_, i64>(3)?,
                is_subagent: r.get::<_, i64>(4)? != 0,
            })
        })
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| CommandError::DbError(e.to_string()))
}

//
pub fn bot_session_create_inner(
    conn: &rusqlite::Connection,
    title: Option<String>,
) -> Result<BotSession, String> {
    bot_session_create_with_kind_inner(conn, title, false)
}

/// 子 agent 执行会话创建（U20D 批 5）：is_subagent=1 结构化标记——
/// 前端停止键/斜杠 /stop 按 `Session.isSubagent` 路由 cancel_subagent，
/// 不再依赖标题 emoji 前缀。
pub fn bot_session_create_subagent_inner(
    conn: &rusqlite::Connection,
    title: Option<String>,
) -> Result<BotSession, String> {
    bot_session_create_with_kind_inner(conn, title, true)
}

fn bot_session_create_with_kind_inner(
    conn: &rusqlite::Connection,
    title: Option<String>,
    is_subagent: bool,
) -> Result<BotSession, String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "新对话".into());
    conn.execute(
        "INSERT INTO bot_sessions (id, title, created_at, updated_at, is_subagent) VALUES (?1, ?2, ?3, ?3, ?4)",
        rusqlite::params![id, title, now, is_subagent as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(BotSession {
        id,
        title,
        created_at: now,
        updated_at: now,
        is_subagent,
    })
}

#[tauri::command]
pub fn bot_session_create(app: AppHandle, title: Option<String>) -> CommandResult<BotSession> {
    // open_db（建连 + PRAGMA + 幂等 DDL）是同步磁盘 I/O，放在写锁外——
    // 临界区只覆盖 SQL 写，别让建连耗时串行化其他写者（同 evolution::apply 先例）
    let conn = super::open_db(&app)?;
    let _g = super::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] db::bot_sessions DB_WRITE_LOCK: {e:?}");

        e.into_inner()
    });
    bot_session_create_inner(&conn, title).map_err(CommandError::DbError)
}

pub fn bot_session_delete_inner(conn: &mut rusqlite::Connection, id: &str) -> CommandResult<()> {
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.execute("DELETE FROM bot_messages WHERE session_id = ?1", [id])
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.execute("DELETE FROM bot_sessions WHERE id = ?1", [id])
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))
}

#[tauri::command]
pub async fn bot_session_delete(app: AppHandle, id: String) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        let _g = super::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] db::bot_sessions DB_WRITE_LOCK: {e:?}");

            e.into_inner()
        });
        let mut conn = super::open_db(&app)?;
        bot_session_delete_inner(&mut conn, &id)
    })
    .await
    .map_err(|e| CommandError::from(format!("会话删除线程 join 失败：{e}")))?
}

#[tauri::command]
pub fn bot_session_rename(app: AppHandle, id: String, title: String) -> CommandResult<()> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "title".into(),
            value: String::new(),
            reason: "会话标题不能为空（或全为空白字符）".into(),
        });
    }
    // 同 bot_session_create：open_db 在写锁外，临界区只覆盖 UPDATE
    let conn = super::open_db(&app)?;
    let _g = super::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] db::bot_sessions DB_WRITE_LOCK: {e:?}");

        e.into_inner()
    });
    let now = chrono::Utc::now().timestamp_millis();
    let rows = conn
        .execute(
            "UPDATE bot_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![title, now, id],
        )
        .map_err(|e| e.to_string())?;
    if rows == 0 {
        return Err(CommandError::InvalidArgument {
            field: "id".into(),
            value: id.to_string(),
            reason: "会话不存在（重命名未命中任何行）".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_sessions (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                updated_at  INTEGER NOT NULL,
                is_subagent INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        conn
    }

    /// U20D 批 5：subagent 变体写 is_subagent=1，普通创建默认 0——
    /// 前端停止键路由（cancel_subagent vs bot_stop）的数据源
    #[test]
    fn subagent_flag_persists_and_loads() {
        let conn = mem_db();
        let sub = bot_session_create_subagent_inner(&conn, Some("子任务：调研".into())).unwrap();
        let normal = bot_session_create_inner(&conn, Some("普通会话".into())).unwrap();
        assert!(sub.is_subagent, "subagent 变体必须置 1");
        assert!(sub.title.starts_with("子任务："), "标题无 emoji 前缀");
        assert!(!normal.is_subagent, "普通创建默认 0");

        let mut stmt = conn
            .prepare("SELECT id, title, created_at, updated_at, is_subagent FROM bot_sessions")
            .unwrap();
        let rows: Vec<BotSession> = stmt
            .query_map([], |r| {
                Ok(BotSession {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    updated_at: r.get(3)?,
                    is_subagent: r.get::<_, i64>(4)? != 0,
                })
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 2, "两条会话都落库");
        assert_eq!(
            rows.iter().filter(|s| s.is_subagent).count(),
            1,
            "恰好一条标记为子 agent 会话"
        );
    }
}
