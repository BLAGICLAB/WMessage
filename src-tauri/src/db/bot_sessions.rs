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
}

#[tauri::command]
pub fn bot_sessions_load(app: AppHandle) -> CommandResult<Vec<BotSession>> {
    let conn = super::open_db(&app)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, created_at, updated_at FROM bot_sessions ORDER BY updated_at DESC",
        )
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BotSession {
                id: r.get::<_, String>(0)?,
                title: r.get::<_, String>(1)?,
                created_at: r.get::<_, i64>(2)?,
                updated_at: r.get::<_, i64>(3)?,
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
    let id = uuid::Uuid::new_v4().simple().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "新对话".into());
    conn.execute(
        "INSERT INTO bot_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        rusqlite::params![id, title, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(BotSession {
        id,
        title,
        created_at: now,
        updated_at: now,
    })
}

#[tauri::command]
pub fn bot_session_create(app: AppHandle, title: Option<String>) -> CommandResult<BotSession> {
    let _g = super::DB_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let conn = super::open_db(&app)?;
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
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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
    let _g = super::DB_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let conn = super::open_db(&app)?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "UPDATE bot_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![title, now, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
