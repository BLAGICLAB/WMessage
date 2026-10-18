//! 聊天记录（消息行）持久化

use tauri::async_runtime;
use tauri::AppHandle;

use super::bot_sessions::BotMsgRow;
use crate::error::{CommandError, CommandResult};

/// 加载指定会话的消息（按写入顺序：行序由自增 id 恢复——写入侧批内 created_at
/// 相同（覆写原子时刻），id 是唯一行序事实源）
#[tauri::command]
pub async fn bot_history_load(app: AppHandle, session_id: String) -> CommandResult<Vec<BotMsgRow>> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        let mut stmt = conn
            .prepare("SELECT role, content, refs, thinking, tools FROM bot_messages WHERE session_id = ?1 ORDER BY id")
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        let rows = stmt
            .query_map([&session_id], |r| {
                Ok(BotMsgRow {
                    role: r.get::<_, String>(0)?,
                    content: r.get::<_, String>(1)?,
                    refs_json: r.get::<_, Option<String>>(2)?,
                    thinking: r.get::<_, Option<String>>(3)?,
                    tools_json: r.get::<_, Option<String>>(4)?,
                })
            })
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("历史读取线程 join 失败：{e}")))?
}

/// 保存指定会话的聊天记录：全量覆盖 + 更新会话活跃时间
pub fn bot_history_save_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
    messages: &[BotMsgRow],
) -> Result<(), String> {
    const MAX_HISTORY_MSGS: usize = 2000;
    let messages = if messages.len() > MAX_HISTORY_MSGS {
        // 超限截断留痕：让日志能区分「干净保存」与「丢了最旧 N 条」
        eprintln!(
            "[bot_history_save] 会话 {session_id} 消息数 {} 超上限 {MAX_HISTORY_MSGS}，截断丢弃最旧 {} 条",
            messages.len(),
            messages.len() - MAX_HISTORY_MSGS
        );
        &messages[messages.len() - MAX_HISTORY_MSGS..]
    } else {
        messages
    };
    conn.execute(
        "DELETE FROM bot_messages WHERE session_id = ?1",
        [session_id],
    )
    .map_err(|e| e.to_string())?;
    // created_at 取单次 now 系有意：整批 DELETE+INSERT 是一次原子覆写，created_at
    // 记录「本次覆写的原子时刻」（同批相同）。行序恢复不依赖 created_at——自增 id
    // 即 (created_at, id) 复合排序键的 row_sequence 分量；按 created_at 排
    // bot_messages 的消费方均属缺陷。
    let now = chrono::Utc::now().timestamp_millis();
    if !messages.is_empty() {
        let mut stmt = conn
            .prepare(
                "INSERT INTO bot_messages (role, content, refs, session_id, thinking, tools, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|e| e.to_string())?;
        for m in messages {
            stmt.execute(rusqlite::params![
                m.role,
                m.content,
                m.refs_json,
                session_id,
                m.thinking,
                m.tools_json,
                now
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    conn.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn bot_history_save(
    app: AppHandle,
    session_id: String,
    messages: Vec<BotMsgRow>,
) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock::lock_or_recover(
            &super::DB_WRITE_LOCK,
            "db::bot_history DB_WRITE_LOCK",
        );
        let mut conn = super::open_db(&app)?;
        let tx = conn
            .transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        bot_history_save_inner(&tx, &session_id, &messages).map_err(CommandError::DbError)?;
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("历史保存线程 join 失败：{e}")))?
}

/// 追加单条消息（不覆盖已有行）。
///
/// 给后端主动发起的轮次用：那种轮次没有前端快照可覆写，走全量覆盖写就得先
/// 把整段历史读回来再拼一遍，而读回来的快照会和并发落库的用户消息打架——
/// 谁后写谁赢，输的那条消息凭空消失。纯 INSERT 没有这个窗口。
pub fn bot_history_append_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
    message: &BotMsgRow,
) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO bot_messages (role, content, refs, session_id, thinking, tools, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            message.role,
            message.content,
            message.refs_json,
            session_id,
            message.thinking,
            message.tools_json,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 清空指定会话的聊天记录（会话保留）
#[tauri::command]
pub async fn bot_history_clear(app: AppHandle, session_id: String) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock::lock_or_recover(
            &super::DB_WRITE_LOCK,
            "db::bot_history DB_WRITE_LOCK",
        );
        let conn = super::open_db(&app)?;
        conn.execute(
            "DELETE FROM bot_messages WHERE session_id = ?1",
            [&session_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| CommandError::from(format!("历史清空线程 join 失败：{e}")))?
}
