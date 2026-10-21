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

#[cfg(test)]
mod tests {
    use super::*;

    /// 整段覆盖写在事务里必须是**一个单位**：中途失败不能留下「历史被清空、
    /// 新行只写进去一半」的状态。
    ///
    /// 用触发器让第二条 INSERT 失败：DELETE 已经执行过了，不在事务里的话
    /// 这条旧消息就真没了。（`db::tests` 里那条 `bot_history_save_rolls_back_on_insert_failure`
    /// 用裸 SQL 复现同一场景；这条直接调被测函数本身，验的是函数里的 DELETE
    /// 也被事务盖住。）
    #[test]
    fn history_save_in_a_transaction_rolls_back_as_one_unit() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(
            "CREATE TABLE bot_messages (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 role       TEXT NOT NULL,
                 content    TEXT NOT NULL,
                 refs       TEXT,
                 session_id TEXT,
                 thinking   TEXT,
                 tools      TEXT,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE bot_sessions (
                 id         TEXT PRIMARY KEY,
                 title      TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1, 1);
             INSERT INTO bot_messages (role, content, session_id, created_at)
                 VALUES ('user', '旧消息', 's1', 1);
             CREATE TRIGGER boom BEFORE INSERT ON bot_messages
                 WHEN NEW.content = 'BOOM' BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .expect("建表");
        let row = |content: &str| BotMsgRow {
            role: "assistant".into(),
            content: content.into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
        };
        // 事务里跑到一半失败 → 整个覆盖写回滚
        let tx = conn.transaction().expect("开事务");
        let r = bot_history_save_inner(&tx, "s1", &[row("新消息"), row("BOOM")]);
        assert!(r.is_err(), "第二条 INSERT 应被触发器打断");
        drop(tx); // 不提交 → 回滚
        let left: Vec<String> = conn
            .prepare("SELECT content FROM bot_messages WHERE session_id = 's1'")
            .expect("读")
            .query_map([], |r| r.get(0))
            .expect("取行")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(
            left,
            vec!["旧消息".to_string()],
            "覆盖写失败后旧历史必须完好——不能被 DELETE 掏空"
        );
    }

    /// 接线守卫：历史整段覆盖写的每个调用点都必须传事务句柄。
    ///
    /// 上面那条测试说明为什么：不在事务里就是半截写。匹配串用 `concat!` 拆分
    /// 防自匹配（本测试自身的源码不算命中）。
    #[test]
    fn every_history_save_call_runs_inside_a_transaction() {
        let needle = concat!("bot_history_save_", "inner(");
        for (name, src) in [
            ("db/bot_history.rs", include_str!("bot_history.rs")),
            ("bot_chat.rs", include_str!("../bot_chat.rs")),
            (
                "bot_orchestrator.rs",
                include_str!("../bot_orchestrator.rs"),
            ),
        ] {
            let mut from = 0usize;
            while let Some(rel) = src[from..].find(needle) {
                let at = from + rel;
                from = at + 1;
                // 跳过函数定义本身
                if src[..at].ends_with("pub fn ") {
                    continue;
                }
                // 跳过注释里的提及（说明文字，不是调用点）
                let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
                if src[line_start..at].trim_start().starts_with("//") {
                    continue;
                }
                let call = &src[at..(at + 60).min(src.len())];
                assert!(
                    call.contains("&tx"),
                    "{name}: 历史整段覆盖写的调用点必须传事务句柄（&tx），不得直接传 conn：{call}"
                );
            }
        }
    }
}
