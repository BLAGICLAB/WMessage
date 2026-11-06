//! 聊天记录（消息行）持久化

use tauri::async_runtime;
use tauri::AppHandle;

use super::bot_sessions::BotMsgRow;
use crate::error::{CommandError, CommandResult};

/// bot_messages 单源 DDL（表 + 身份三列）：基线初始化与测试建表共用同一份，
/// 防「基线加列、测试漏列」口径漂移（同 subagents::SUBAGENTS_DDL 先例）。
/// 索引在 IDENTITY_INDEX_DDL——唯一索引依赖 msg_id 列，老库补列之前建不出来，
/// 必须与建表分开执行。
pub(crate) const BOT_MESSAGES_DDL: &str = "
         CREATE TABLE IF NOT EXISTS bot_messages (
           id         INTEGER PRIMARY KEY AUTOINCREMENT,
           role       TEXT NOT NULL,
           content    TEXT NOT NULL,
           refs       TEXT,
           session_id TEXT,
           thinking   TEXT,
           tools      TEXT,
           turn_status TEXT,
           created_at INTEGER NOT NULL,
           msg_id     TEXT,
           ord        INTEGER NOT NULL DEFAULT 0,
           trace_id   INTEGER,
           file_changes TEXT
         );";

/// 消息身份两索引：upsert 冲突目标与读序都靠它们。唯一索引用全列
/// `(session_id, msg_id)` 而非部分索引——SQLite 中 NULL 相异，存量 NULL id
/// 行彼此不冲突，无需 `WHERE msg_id IS NOT NULL`，也避开 ON CONFLICT 目标
/// 子句与部分索引 WHERE 匹配的版本兼容坑。
pub(crate) const BOT_MESSAGES_IDENTITY_INDEX_DDL: &str = "
         CREATE UNIQUE INDEX IF NOT EXISTS idx_bot_messages_sid_msgid
             ON bot_messages(session_id, msg_id);
         CREATE INDEX IF NOT EXISTS idx_bot_messages_sid_ord
             ON bot_messages(session_id, ord);";

/// bot_messages 身份三列 + 两索引的幂等迁移：新库 DDL 已带齐（补列探测全
/// 跳过），老库先补列再建索引（唯一索引依赖 msg_id 列存在，顺序不能反）。
pub(crate) fn ensure_bot_messages_identity(conn: &rusqlite::Connection) -> Result<(), String> {
    super::ensure_columns(
        conn,
        "bot_messages",
        &[
            ("msg_id", "TEXT"),
            ("ord", "INTEGER NOT NULL DEFAULT 0"),
            ("trace_id", "INTEGER"),
            ("file_changes", "TEXT"),
        ],
    )?;
    conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
        .map_err(|e| e.to_string())
}

/// 加载指定会话的消息（按快照序：行序由 `ord` 恢复，id 只作 ord 雷同的次级
/// 稳定键——存量行 ord 恒 0，落到 id 序，与旧行为一致）
pub fn bot_history_load_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<BotMsgRow>, String> {
    let mut stmt = conn
        .prepare("SELECT role, content, refs, thinking, tools, turn_status, msg_id, trace_id, file_changes FROM bot_messages WHERE session_id = ?1 ORDER BY ord, id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok(BotMsgRow {
                msg_id: r.get::<_, Option<String>>(6)?,
                role: r.get::<_, String>(0)?,
                content: r.get::<_, String>(1)?,
                refs_json: r.get::<_, Option<String>>(2)?,
                thinking: r.get::<_, Option<String>>(3)?,
                tools_json: r.get::<_, Option<String>>(4)?,
                turn_status: r.get::<_, Option<String>>(5)?,
                trace_id: r.get::<_, Option<i64>>(7)?,
                file_changes_json: r.get::<_, Option<String>>(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn bot_history_load<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> CommandResult<Vec<BotMsgRow>> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        bot_history_load_inner(&conn, &session_id).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("历史读取线程 join 失败：{e}")))?
}

/// 服务端消息身份生成（uuid v4 文本，与前端 crypto.randomUUID 同格式）。
/// msg_id 对存储层是不透明串，前后端各生成互不冲突。
fn new_msg_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 保存指定会话的聊天记录：快照 upsert + 差集删除 + 更新会话活跃时间。
///
/// 入参数组即权威全量：本批内的行按 `(session_id, msg_id)` upsert（内容列
/// 与 ord 覆盖，created_at 保首插时刻），库里不在本批的行——含未带 id 的
/// 存量行——全部删除。与旧的「DELETE 全部 + 重插」语义逐一对齐，只是身份
/// 跨保存存活：同一逻辑消息重存不换行、不换首插时刻。
///
/// 行序与身份在本内核定桩：ord = 数组下标（快照序唯一权威是入参数组），
/// 未带 msg_id 的行就地补服务端 id（exec/子会话整段重建不关心跨轮身份，
/// 每轮重建即得新身份，旧行由差集删除收走）。
///
/// 返回被丢弃的条数（0 = 没截断）：超上限时截断最旧的若干条，丢弃这件事
/// 必须留痕——调用方（`bot_history_save_audited`）据此写审计事件。被裁行
/// 不在本批，由差集删除自然收走。
pub fn bot_history_save_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
    messages: &[BotMsgRow],
) -> Result<usize, String> {
    const MAX_HISTORY_MSGS: usize = 2000;
    let dropped = messages.len().saturating_sub(MAX_HISTORY_MSGS);
    let messages = if dropped > 0 {
        &messages[dropped..]
    } else {
        messages
    };
    let now = crate::db::now_ms();
    let ids: Vec<String> = messages
        .iter()
        .map(|m| m.msg_id.clone().unwrap_or_else(new_msg_id))
        .collect();
    // created_at 故意不在 DO UPDATE 列清单里：同 id 命中旧行时保首插时刻
    let mut stmt = conn
        .prepare(
            "INSERT INTO bot_messages (session_id, msg_id, ord, role, content, refs, thinking, tools, turn_status, trace_id, file_changes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(session_id, msg_id) DO UPDATE SET
                ord = excluded.ord,
                role = excluded.role,
                content = excluded.content,
                refs = excluded.refs,
                thinking = excluded.thinking,
                tools = excluded.tools,
                turn_status = excluded.turn_status,
                trace_id = excluded.trace_id,
                file_changes = excluded.file_changes",
        )
        .map_err(|e| e.to_string())?;
    for (i, m) in messages.iter().enumerate() {
        stmt.execute(rusqlite::params![
            session_id,
            ids[i],
            i as i64,
            m.role,
            m.content,
            m.refs_json,
            m.thinking,
            m.tools_json,
            m.turn_status,
            m.trace_id,
            m.file_changes_json,
            now
        ])
        .map_err(|e| e.to_string())?;
    }
    drop(stmt);
    // 差集删除。id 清单经 json_each 单参数下发：2000 行也不撞 SQLite 变量上限
    let ids_json = serde_json::to_string(&ids).map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM bot_messages
          WHERE session_id = ?1
            AND (msg_id IS NULL OR msg_id NOT IN (SELECT value FROM json_each(?2)))",
        rusqlite::params![session_id, ids_json],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(dropped)
}

/// 按 `(session_id, msg_id)` upsert 单行：回合内用户行/终稿行的落行入口。
///
/// 新行 = 会话尾追加（ord = MAX+1，created_at = 当刻）；命中旧行只更新
/// 内容列——位置与首插时刻都不动（同 id 重写 = 同一条消息）。行带 None
/// id 时就地补服务端 id 并随返回值带回（调用方广播事件要用同一身份）。
/// 不自带事务：调用方需要原子语义时传入事务连接
/// （`&Transaction` 经 Deref 解引用可用）。
pub fn bot_history_upsert_row_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
    message: &BotMsgRow,
) -> Result<String, String> {
    let msg_id = message.msg_id.clone().unwrap_or_else(new_msg_id);
    let now = crate::db::now_ms();
    conn.execute(
        "INSERT INTO bot_messages (session_id, msg_id, ord, role, content, refs, thinking, tools, turn_status, trace_id, file_changes, created_at)
         VALUES (?1, ?2, (SELECT COALESCE(MAX(ord), -1) + 1 FROM bot_messages WHERE session_id = ?1), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(session_id, msg_id) DO UPDATE SET
            role = excluded.role,
            content = excluded.content,
            refs = excluded.refs,
            thinking = excluded.thinking,
            tools = excluded.tools,
            turn_status = excluded.turn_status,
            trace_id = excluded.trace_id,
            file_changes = excluded.file_changes",
        rusqlite::params![
            session_id,
            msg_id,
            message.role,
            message.content,
            message.refs_json,
            message.thinking,
            message.tools_json,
            message.turn_status,
            message.trace_id,
            message.file_changes_json,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(msg_id)
}

/// 整段覆盖写（留痕版）：超上限截断时把丢弃条数记进审计账本。
///
/// 截断原先只往 stderr 吐一行——stderr 不在日志体系里（bot.log / error.log
/// 才查得到），事后没人能看出「这段对话是不是被截过、丢了多少条」。
/// 事件名与「按 token 预算截上下文」那条区分开：那条是 `chat.history_truncated`。
///
/// 入参是事务句柄不是裸连接：整段覆盖写必须是事务里的一个单位。
pub fn bot_history_save_audited<R: tauri::Runtime>(
    app: &AppHandle<R>,
    tx: &rusqlite::Transaction,
    session_id: &str,
    messages: &[BotMsgRow],
) -> Result<(), String> {
    let dropped = bot_history_save_inner(tx, session_id, messages)?;
    if dropped > 0 {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "chat.history_save_truncated",
            "session_id" => session_id,
            "dropped" => dropped,
            "total" => messages.len(),
        );
    }
    Ok(())
}

/// 追加单条消息（不覆盖已有行）。
///
/// 给后端主动发起的轮次用：那种轮次没有前端快照可覆写，走全量覆盖写就得先
/// 把整段历史读回来再拼一遍，而读回来的快照会和并发落库的用户消息打架——
/// 谁后写谁赢，输的那条消息凭空消失。纯 INSERT 没有这个窗口。
///
/// 追加行就地生成服务端 id、排在会话尾（ord = MAX+1、created_at = 当刻）。
///
/// 追加与「会话活跃时间推进」写在同一事务里：两步必须同时成立。只 INSERT
/// 成功而 UPDATE 失败时，调用方会把这次追加当成「没写进去」（汇报轮据此
/// 不记「已吸收」并重试），可消息行其实已经在库里——重试就会多插一条一样
/// 的汇报，用户看到两条重复的子任务结果。
pub fn bot_history_append_inner(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    message: &BotMsgRow,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let now = crate::db::now_ms();
    let msg_id = message.msg_id.clone().unwrap_or_else(new_msg_id);
    tx.execute(
        "INSERT INTO bot_messages (session_id, msg_id, ord, role, content, refs, thinking, tools, turn_status, trace_id, file_changes, created_at) VALUES (?1, ?2, (SELECT COALESCE(MAX(ord), -1) + 1 FROM bot_messages WHERE session_id = ?1), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            session_id,
            msg_id,
            message.role,
            message.content,
            message.refs_json,
            message.thinking,
            message.tools_json,
            message.turn_status,
            message.trace_id,
            message.file_changes_json,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    tx.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

/// 截尾删除：删掉 msg_id 锚行之后（ord 更大）的所有行，锚行本身保留。
///
/// retry 的真源侧配套：上一轮的失败回复等残行不清掉，后端重建的上下文就会
/// 把它们摆回模型面前。锚行不存在 = 无事发生（调用方随后照常起轮，行内核
/// upsert 会按 client_msg_id 补上）。被删行里带身份的以清单返回（供
/// bot-msg-dropped 事件指名），NULL id 行不可指名，只计条数。
///
/// 删行与会话活跃时间推进同一事务（与 clear 同口径）。
pub fn bot_history_drop_after_inner(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    msg_id: &str,
) -> Result<(Vec<String>, usize), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let anchor: Option<i64> = match tx.query_row(
        "SELECT ord FROM bot_messages WHERE session_id = ?1 AND msg_id = ?2",
        rusqlite::params![session_id, msg_id],
        |r| r.get(0),
    ) {
        Ok(v) => Some(v),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e.to_string()),
    };
    let Some(anchor) = anchor else {
        return Ok((Vec::new(), 0));
    };
    let ids: Vec<String> = tx
        .prepare(
            "SELECT msg_id FROM bot_messages
              WHERE session_id = ?1 AND ord > ?2 AND msg_id IS NOT NULL",
        )
        .map_err(|e| e.to_string())?
        .query_map(rusqlite::params![session_id, anchor], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let n = tx
        .execute(
            "DELETE FROM bot_messages WHERE session_id = ?1 AND ord > ?2",
            rusqlite::params![session_id, anchor],
        )
        .map_err(|e| e.to_string())?;
    let now = crate::db::now_ms();
    tx.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((ids, n))
}

/// 删单行（removeMessage 的真源侧操作）：按 msg_id 精确删，他行不动。
/// 返回删除条数（0 = 行不存在，调用方提示即可）。删行与会话活跃时间推进
/// 同一事务（与 clear/截尾同口径）。
pub fn bot_history_drop_inner(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    msg_id: &str,
) -> Result<usize, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let n = tx
        .execute(
            "DELETE FROM bot_messages WHERE session_id = ?1 AND msg_id = ?2",
            rusqlite::params![session_id, msg_id],
        )
        .map_err(|e| e.to_string())?;
    let now = crate::db::now_ms();
    tx.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n)
}

/// 摘要行换血（compact 的真源侧操作）：把 msg_ids 集合的行删掉，换成一条
/// 服务端身份的摘要行，位置取被删集合的最小 ord（摘要代表那一段历史，
/// 不得挪到会话尾）。支持部分压缩：集合外的行 ord 不动。返回摘要行投影
/// （含服务端 id，供 bot-msg-upserted 载荷）+ 被删行身份清单。
///
/// msg_ids 为空 = 无事发生（前端不会这么调，防御性短路）。
pub fn bot_history_replace_summary_inner(
    conn: &mut rusqlite::Connection,
    session_id: &str,
    msg_ids: &[String],
    summary_content: &str,
) -> Result<(Vec<String>, crate::db::BotMsgRow), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let ids_json = serde_json::to_string(msg_ids).map_err(|e| e.to_string())?;
    let dropped: Vec<String> = tx
        .prepare(
            "SELECT msg_id FROM bot_messages
              WHERE session_id = ?1 AND msg_id IS NOT NULL
                AND msg_id IN (SELECT value FROM json_each(?2))",
        )
        .map_err(|e| e.to_string())?
        .query_map(rusqlite::params![session_id, ids_json], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if dropped.is_empty() {
        tx.commit().map_err(|e| e.to_string())?;
        return Ok((
            Vec::new(),
            crate::db::BotMsgRow {
                msg_id: None,
                role: "assistant".into(),
                content: String::new(),
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: None,
                file_changes_json: None,
                trace_id: None,
            },
        ));
    }
    let min_ord: i64 = tx
        .query_row(
            "SELECT MIN(ord) FROM bot_messages
              WHERE session_id = ?1 AND msg_id IN (SELECT value FROM json_each(?2))",
            rusqlite::params![session_id, ids_json],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM bot_messages
          WHERE session_id = ?1 AND msg_id IN (SELECT value FROM json_each(?2))",
        rusqlite::params![session_id, ids_json],
    )
    .map_err(|e| e.to_string())?;
    // 摘要行走行内核落行（就地补服务端 id），随后改盖 ord 到被删集合起点
    let summary = crate::db::BotMsgRow {
        msg_id: None,
        role: "assistant".into(),
        content: summary_content.to_string(),
        refs_json: None,
        thinking: None,
        tools_json: None,
        turn_status: None,
        file_changes_json: None,
        trace_id: None,
    };
    let msg_id = crate::db::bot_history_upsert_row_inner(&tx, session_id, &summary)
        .map_err(|e| e.to_string())?;
    // 行内核按 MAX+1 尾插——摘要行要占被删集合的起点，改盖 ord
    tx.execute(
        "UPDATE bot_messages SET ord = ?2 WHERE session_id = ?1 AND msg_id = ?3",
        rusqlite::params![session_id, min_ord, msg_id],
    )
    .map_err(|e| e.to_string())?;
    let now = crate::db::now_ms();
    tx.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok((
        dropped,
        crate::db::BotMsgRow {
            msg_id: Some(msg_id),
            ..summary
        },
    ))
}

/// 清空指定会话的聊天记录（会话保留），并把会话的活跃时间推到当前。
///
/// 两步写在同一事务里：删消息与更新时间要么都成、要么都不成。
pub fn bot_history_clear_inner(
    conn: &mut rusqlite::Connection,
    session_id: &str,
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "DELETE FROM bot_messages WHERE session_id = ?1",
        [session_id],
    )
    .map_err(|e| e.to_string())?;
    // 清空也是一次会话活动：会话列表按 updated_at 倒序，不更新的话
    // 清空过的对话仍挂在旧位置，看着像「没动过」
    let now = crate::db::now_ms();
    tx.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// 清空指定会话的聊天记录（会话保留）
#[tauri::command]
pub async fn bot_history_clear(app: AppHandle, session_id: String) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock::lock_or_recover(
            &super::DB_WRITE_LOCK,
            "db::bot_history DB_WRITE_LOCK",
        );
        let mut conn = super::open_db(&app)?;
        bot_history_clear_inner(&mut conn, &session_id).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("历史清空线程 join 失败：{e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 超上限截断要进审计账本：原先只往 stderr 吐一行——stderr 不在日志体系里，
    /// 事后没人能看出「这段对话是不是被截过、丢了多少条」。
    ///
    /// cargo test 下数据目录被钉在 temp（见 paths::probe_log_dir 的测试旁路），
    /// 写盘不会碰到用户真实数据目录。
    #[test]
    fn history_save_truncation_is_recorded_in_the_audit_log() {
        let _serial = crate::db::lock::lock_or_recover(
            &crate::audit::BOT_LOG_TEST_LOCK,
            "audit::BOT_LOG_TEST_LOCK",
        );
        let app = tauri::test::mock_app();
        let bot_log = crate::db::data_dir(app.handle()).join("bot.log");
        let _ = std::fs::remove_file(&bot_log);
        let mut conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(BOT_MESSAGES_DDL).expect("建表");
        conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
            .expect("建索引");
        conn.execute_batch(
            "
             CREATE TABLE bot_sessions (
                 id         TEXT PRIMARY KEY,
                 title      TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1000, 1000);",
        )
        .expect("建表");
        // 2001 条：超上限 1 条
        let rows: Vec<BotMsgRow> = (0..2001)
            .map(|i| BotMsgRow {
                msg_id: None,
                file_changes_json: None,
                trace_id: None,
                role: "user".into(),
                content: format!("m{i}"),
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: None,
            })
            .collect();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_audited(app.handle(), &tx, "s1", &rows).expect("保存");
        tx.commit().expect("提交");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(count, 2000, "超上限应截断到 2000 条（丢最旧的）");
        let content = std::fs::read_to_string(&bot_log).expect("审计应写出 bot.log");
        let last = content.lines().last().expect("bot.log 至少一行");
        assert!(
            last.contains("chat.history_save_truncated"),
            "截断事件名缺失: {last}"
        );
        assert!(last.contains("dropped=1"), "丢弃条数要记: {last}");
    }

    /// 清空历史要把会话的活跃时间推到当前：会话列表按 updated_at 倒序，
    /// 不更新的话清空过的对话仍挂在旧位置，看着像「没动过」。
    #[test]
    fn clearing_history_bumps_session_activity_time() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(BOT_MESSAGES_DDL).expect("建表");
        conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
            .expect("建索引");
        conn.execute_batch(
            "
             CREATE TABLE bot_sessions (
                 id         TEXT PRIMARY KEY,
                 title      TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1000, 1000);
             INSERT INTO bot_messages (role, content, session_id, created_at)
                 VALUES ('user', '旧消息', 's1', 1000);",
        )
        .expect("建表");
        bot_history_clear_inner(&mut conn, "s1").expect("清空");
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(left, 0, "消息行应被清空");
        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("读会话");
        assert!(
            updated_at > 1000,
            "清空也要推会话活跃时间（现 updated_at={updated_at}）"
        );
    }

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
        conn.execute_batch(BOT_MESSAGES_DDL).expect("建表");
        conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
            .expect("建索引");
        conn.execute_batch(
            "
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
            msg_id: None,
            file_changes_json: None,
            trace_id: None,
            role: "assistant".into(),
            content: content.into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
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

    /// 回合终态要能往返：写进去的 turn_status 读回来还得是它。
    ///
    /// 终态是「这一轮怎么结束的」的事实，只活在内存里的话，重载会话就丢——
    /// 库里才是这一轮的真源。None（用户消息 / 后端自己写的行 / 旧行）也必须
    /// 原样回来：None 是「没有回合概念」，不是「进行中」。
    #[test]
    fn turn_status_round_trips_through_history() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(BOT_MESSAGES_DDL).expect("建表");
        conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
            .expect("建索引");
        conn.execute_batch(
            "
             CREATE TABLE bot_sessions (
                 id         TEXT PRIMARY KEY,
                 title      TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1000, 1000);",
        )
        .expect("建表");
        let rows = vec![
            BotMsgRow {
                msg_id: None,
                file_changes_json: None,
                trace_id: None,
                role: "user".into(),
                content: "问".into(),
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: None,
            },
            BotMsgRow {
                msg_id: None,
                file_changes_json: None,
                trace_id: None,
                role: "assistant".into(),
                content: "答".into(),
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: Some("interrupted".into()),
            },
        ];
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &rows).expect("保存");
        tx.commit().expect("提交");
        let loaded = bot_history_load_inner(&conn, "s1").expect("读历史");
        assert_eq!(loaded.len(), 2, "两行都要读回来");
        assert_eq!(
            loaded[0].turn_status, None,
            "用户消息没有回合终态，None 不能被当成进行中"
        );
        assert_eq!(
            loaded[1].turn_status.as_deref(),
            Some("interrupted"),
            "回合终态要原样回来"
        );
    }

    /// 追加写必须是**一个单位**：插入消息与推进会话活跃时间要么都成、要么都不成。
    ///
    /// 只 INSERT 成功而 UPDATE 失败时，调用方会把这次追加当成「没写进去」
    /// （汇报轮据此不记「已吸收」并重试），可消息行其实已经在库里——重试就
    /// 会多插一条一样的汇报，用户看到两条重复的子任务结果。
    #[test]
    fn history_append_rolls_back_the_row_when_the_session_update_fails() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(BOT_MESSAGES_DDL).expect("建表");
        conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
            .expect("建索引");
        conn.execute_batch(
            "
             CREATE TABLE bot_sessions (
                 id         TEXT PRIMARY KEY,
                 title      TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1000, 1000);
             CREATE TRIGGER boom BEFORE UPDATE ON bot_sessions
             BEGIN SELECT RAISE(ABORT, 'boom'); END;",
        )
        .expect("建表");
        let r = bot_history_append_inner(
            &mut conn,
            "s1",
            &BotMsgRow {
                msg_id: None,
                file_changes_json: None,
                trace_id: None,
                role: "assistant".into(),
                content: "汇报轮回复".into(),
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: None,
            },
        );
        assert!(r.is_err(), "会话更新时间写失败时整次追加必须失败");
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(
            left, 0,
            "失败的追加不得留下消息行——否则重试会多插一条一样的汇报"
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
                let call = &src[at..];
                // 取第一个实参：必须是事务句柄（tx / &tx），不得是裸连接
                let args = &call[call.find('(').map(|i| i + 1).unwrap_or(0)..];
                let arg = args[..args.find(',').unwrap_or(0)].trim();
                assert!(
                    arg == "tx" || arg == "&tx",
                    "{name}: 历史整段覆盖写的调用点必须传事务句柄（tx / &tx），\
                     不得直接传 conn：{call}"
                );
            }
        }
    }

    // ===== 消息身份数据面锚点（快照 upsert + 差集删除 + 行内核）=====

    /// 身份测试夹具：单源 DDL 建消息表 + 两索引 + 最小会话表（s1，基线时刻 1000）
    fn identity_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("内存库");
        conn.execute_batch(BOT_MESSAGES_DDL).expect("建消息表");
        conn.execute_batch(BOT_MESSAGES_IDENTITY_INDEX_DDL)
            .expect("建索引");
        conn.execute_batch(
            "CREATE TABLE bot_sessions (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1000, 1000);",
        )
        .expect("建会话表");
        conn
    }

    /// id 传空串 = 不带身份（写内核就地补服务端 id）
    fn row(id: &str, content: &str) -> BotMsgRow {
        BotMsgRow {
            msg_id: if id.is_empty() { None } else { Some(id.into()) },
            file_changes_json: None,
            trace_id: None,
            role: "user".into(),
            content: content.into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        }
    }

    /// 锚点①：同 id upsert 命中旧行只更新内容，首插时刻与位置都不动
    #[test]
    fn upsert_row_keeps_first_insert_time_and_position() {
        let conn = identity_db();
        conn.execute(
            "INSERT INTO bot_messages (session_id, msg_id, ord, role, content, created_at)
             VALUES ('s1', 'a', 5, 'user', '旧文', 1000)",
            [],
        )
        .expect("seed");
        bot_history_upsert_row_inner(&conn, "s1", &row("a", "新文")).expect("upsert");
        let (created_at, ord, content): (i64, i64, String) = conn
            .query_row(
                "SELECT created_at, ord, content FROM bot_messages WHERE msg_id = 'a'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("读行");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE msg_id = 'a'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(count, 1, "同 id upsert 不得产生第二行");
        assert_eq!(created_at, 1000, "命中旧行必须保首插时刻，覆写不漂");
        assert_eq!(ord, 5, "命中旧行必须保位置");
        assert_eq!(content, "新文", "内容列要走 upsert 更新");
    }

    /// 新行 = 会话尾追加（ord = MAX+1）；无 id 行落地即有服务端身份
    #[test]
    fn upsert_row_appends_new_ids_at_session_tail() {
        let conn = identity_db();
        bot_history_upsert_row_inner(&conn, "s1", &row("a", "一")).expect("a");
        bot_history_upsert_row_inner(&conn, "s1", &row("b", "二")).expect("b");
        bot_history_upsert_row_inner(&conn, "s1", &row("", "三")).expect("c");
        let ords: Vec<i64> = conn
            .prepare("SELECT ord FROM bot_messages WHERE session_id = 's1' ORDER BY ord")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(ords, vec![0, 1, 2], "新行按会话尾递增排位");
        let null_ids: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE msg_id IS NULL",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(null_ids, 0, "行内核不得落地无身份的行");
    }

    /// 锚点②：快照是权威全量——库里不在本批的行（含 NULL id 存量行）被差集
    /// 删除收走，本批行按数组下标重排 ord
    #[test]
    fn snapshot_diff_removes_missing_and_unidentified_rows() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("a", "A"), row("b", "B")]).expect("首批");
        tx.commit().expect("提交");
        conn.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at)
             VALUES ('s1', 'user', '无id存量行', 1000)",
            [],
        )
        .expect("seed NULL 行");
        // 快照只剩 a 与新行 d：b 与 NULL 存量行都要被收走
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("a", "A2"), row("d", "D")]).expect("次批");
        tx.commit().expect("提交");
        let contents: Vec<String> = conn
            .prepare("SELECT content FROM bot_messages WHERE session_id = 's1' ORDER BY ord")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(
            contents,
            vec!["A2".to_string(), "D".to_string()],
            "差集删除后只剩本批两行且按快照序"
        );
        let null_ids: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE msg_id IS NULL",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(null_ids, 0, "NULL id 存量行必须被差集删除清走");
    }

    /// 空快照 = 清空会话（/clean 与整段重建空态同语义），且会话活跃时间照推进
    #[test]
    fn snapshot_diff_empty_snapshot_clears_session() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("a", "A"), row("b", "B")]).expect("首批");
        tx.commit().expect("提交");
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[]).expect("空快照");
        tx.commit().expect("提交");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(count, 0, "空快照必须清空会话");
        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("读会话");
        assert!(updated_at > 1000, "清空也要推会话活跃时间");
    }

    /// 未带 id 的行每轮重建都换新身份（exec/子会话整段重建语义），
    /// 旧行由差集删除收走，行数稳定
    #[test]
    fn snapshot_rows_without_ids_get_fresh_identity_each_rebuild() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("", "X"), row("", "Y")]).expect("首批");
        tx.commit().expect("提交");
        let ids1: Vec<String> = conn
            .prepare("SELECT msg_id FROM bot_messages WHERE session_id = 's1' ORDER BY ord")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert!(ids1.iter().all(|i| !i.is_empty()), "落地即有服务端身份");
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("", "X"), row("", "Y")]).expect("次批");
        tx.commit().expect("提交");
        let ids2: Vec<String> = conn
            .prepare("SELECT msg_id FROM bot_messages WHERE session_id = 's1' ORDER BY ord")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(ids2.len(), 2, "重建后行数稳定");
        assert_ne!(ids1, ids2, "每轮重建都是全新身份（旧行被差集收走）");
    }

    /// 锚点⑥：老库形态（无三列）迁出 NULL id 行不炸唯一索引（SQLite NULL
    /// 相异），迁移幂等重跑，列与索引齐备；下一次保存把 NULL 行自然换血
    #[test]
    fn migration_tolerates_legacy_null_id_rows_and_is_idempotent() {
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
                 turn_status TEXT,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE bot_sessions (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 is_subagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO bot_sessions (id, title, created_at, updated_at)
                 VALUES ('s1', '会话', 1000, 1000);
             INSERT INTO bot_messages (session_id, role, content, created_at)
                 VALUES ('s1', 'user', '旧一', 1000), ('s1', 'user', '旧二', 1000), ('s1', 'user', '旧三', 1000);",
        )
        .expect("老库建表");
        ensure_bot_messages_identity(&conn).expect("迁移");
        ensure_bot_messages_identity(&conn).expect("迁移幂等重跑不得报错");
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_index_list('bot_messages')
                 WHERE name IN ('idx_bot_messages_sid_msgid', 'idx_bot_messages_sid_ord')",
                [],
                |r| r.get(0),
            )
            .expect("读索引");
        assert_eq!(idx_count, 2, "身份两索引必须就位");
        let legacy: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_messages", [], |r| r.get(0))
            .expect("数行");
        assert_eq!(legacy, 3, "迁移不得动存量数据");
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("a", "新血")]).expect("首次保存");
        tx.commit().expect("提交");
        let contents: Vec<String> = conn
            .prepare("SELECT content FROM bot_messages WHERE session_id = 's1'")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(
            contents,
            vec!["新血".to_string()],
            "NULL 存量行在首次保存时被差集换血"
        );
    }

    /// 锚点⑤：追加行排会话尾（ord = MAX+1）且落地即有服务端身份
    #[test]
    fn append_assigns_tail_ord_and_server_identity() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("a", "A"), row("b", "B")]).expect("首批");
        tx.commit().expect("提交");
        bot_history_append_inner(&mut conn, "s1", &row("", "汇报一")).expect("append 1");
        bot_history_append_inner(&mut conn, "s1", &row("", "汇报二")).expect("append 2");
        let rows: Vec<(Option<String>, i64)> = conn
            .prepare("SELECT msg_id, ord FROM bot_messages WHERE session_id = 's1' ORDER BY ord")
            .expect("prepare")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        let ords: Vec<i64> = rows.iter().map(|(_, o)| *o).collect();
        assert_eq!(ords, vec![0, 1, 2, 3], "追加行依次排在会话尾");
        assert!(
            rows.iter()
                .all(|(id, _)| id.as_deref().is_some_and(|s| !s.is_empty())),
            "追加行落地即有服务端身份，不得留 NULL"
        );
        let distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT msg_id) FROM bot_messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(distinct, 4, "四行身份互异");
    }

    /// 锚点⑱：drop 单行——删指定行、他行保留、无锚无事发生、活跃时间推进
    #[test]
    fn drop_removes_exactly_one_row_and_keeps_others() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &[row("a", "问题"), row("b", "答复")]).expect("首批");
        tx.commit().expect("提交");
        let n = bot_history_drop_inner(&mut conn, "s1", "b").expect("drop");
        assert_eq!(n, 1, "精确删一行");
        let contents: Vec<String> = conn
            .prepare("SELECT content FROM bot_messages WHERE session_id = 's1'")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(contents, vec!["问题".to_string()], "他行保留");
        let n2 = bot_history_drop_inner(&mut conn, "s1", "锚不存在").expect("无锚");
        assert_eq!(n2, 0, "行不存在 = 无事发生");
        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("读会话");
        assert!(updated_at > 1000, "删行也推会话活跃时间");
    }

    /// 锚点⑳：摘要行换血——集合行删净、摘要行占被删集合起点（ord=最小）、
    /// 服务端身份随行返回；集合外行不动
    #[test]
    fn replace_summary_bloods_out_set_and_lands_summary_at_its_start() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(
            &tx,
            "s1",
            &[row("a", "旧一"), row("b", "旧二"), row("c", "保留行")],
        )
        .expect("首批");
        tx.commit().expect("提交");
        let (dropped, summary) = bot_history_replace_summary_inner(
            &mut conn,
            "s1",
            &["a".into(), "b".into()],
            "摘要：早前聊了两句",
        )
        .expect("换血");
        assert_eq!(dropped, vec!["a".to_string(), "b".to_string()]);
        assert!(
            summary.msg_id.as_deref().is_some_and(|s| !s.is_empty()),
            "摘要行带服务端身份（事件载荷用）"
        );
        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT ord, content FROM bot_messages WHERE session_id = 's1' ORDER BY ord")
            .expect("prepare")
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(
            rows,
            vec![
                (0, "摘要：早前聊了两句".to_string()),
                (2, "保留行".to_string())
            ],
            "摘要行占被删集合起点（ord=0），集合外行 ord 不动"
        );
        // 空集合防御性短路
        let (dropped2, s2) =
            bot_history_replace_summary_inner(&mut conn, "s1", &[], "x").expect("空集合");
        assert!(dropped2.is_empty() && s2.msg_id.is_none(), "空集合无事发生");
    }

    /// 锚点⑰：终稿行聚合字段（thinking/tools/file_changes/trace_id）落库
    /// 读回原样往返——重启后 thinking/tools 从库还原走的就是这条链
    #[test]
    fn final_row_aggregate_fields_round_trip_through_db() {
        let conn = identity_db();
        let row = BotMsgRow {
            msg_id: Some("fin".into()),
            role: "assistant".into(),
            content: "答复".into(),
            refs_json: None,
            thinking: Some("想了".into()),
            tools_json: Some(r#"[{"id":"t1","name":"read"}]"#.into()),
            turn_status: Some("completed".into()),
            file_changes_json: Some(
                r#"[{"path":"/a","kind":"create","added":1,"deleted":0}]"#.into(),
            ),
            trace_id: Some(9),
        };
        bot_history_upsert_row_inner(&conn, "s1", &row).expect("落行");
        let loaded = bot_history_load_inner(&conn, "s1").expect("读回");
        assert_eq!(loaded.len(), 1);
        let r = &loaded[0];
        assert_eq!(r.thinking.as_deref(), Some("想了"));
        assert_eq!(
            r.tools_json.as_deref(),
            Some(r#"[{"id":"t1","name":"read"}]"#)
        );
        assert_eq!(
            r.file_changes_json.as_deref(),
            Some(r#"[{"path":"/a","kind":"create","added":1,"deleted":0}]"#)
        );
        assert_eq!(r.trace_id, Some(9));
        assert_eq!(r.turn_status.as_deref(), Some("completed"));
    }

    /// retry 截尾：锚行后的行（含 NULL id 存量行）全删、锚行保留、活跃时间推进；
    /// 锚行不存在 = 无事发生
    #[test]
    fn drop_after_truncates_rows_after_anchor_and_keeps_it() {
        let mut conn = identity_db();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(
            &tx,
            "s1",
            &[row("a", "问题"), row("b", "坏答复"), row("c", "残行")],
        )
        .expect("首批");
        tx.commit().expect("提交");
        let (ids, n) = bot_history_drop_after_inner(&mut conn, "s1", "a").expect("截尾");
        assert_eq!(n, 2, "锚行后的两行全删");
        assert_eq!(
            ids,
            vec!["b".to_string(), "c".to_string()],
            "被删行身份按序返回"
        );
        let contents: Vec<String> = conn
            .prepare("SELECT content FROM bot_messages WHERE session_id = 's1'")
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("map")
            .collect::<Result<Vec<_>, _>>()
            .expect("收集");
        assert_eq!(contents, vec!["问题".to_string()], "锚行保留");
        let updated_at: i64 = conn
            .query_row(
                "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
                [],
                |r| r.get(0),
            )
            .expect("读会话");
        assert!(
            updated_at > 1000,
            "截尾也要推会话活跃时间（与 clear 同口径）"
        );
        let (_, n2) = bot_history_drop_after_inner(&mut conn, "s1", "锚不存在").expect("无锚");
        assert_eq!(n2, 0, "锚行不存在 = 无事发生");
    }

    /// 差集删除的 id 清单走 json_each 单参数：2000 行（截断上限）整批进出
    /// 不撞 SQLite 变量上限，ord 无断裂
    #[test]
    fn snapshot_diff_handles_2000_rows_without_variable_limit() {
        let mut conn = identity_db();
        let first: Vec<BotMsgRow> = (0..2000)
            .map(|i| row(&format!("m{i}"), &format!("内容{i}")))
            .collect();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &first).expect("首批 2000 行");
        tx.commit().expect("提交");
        let second: Vec<BotMsgRow> = (0..2000).map(|i| row("", &format!("换血{i}"))).collect();
        let tx = conn.transaction().expect("开事务");
        bot_history_save_inner(&tx, "s1", &second).expect("次批 2000 行");
        tx.commit().expect("提交");
        let (count, ord_max): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(ord), -1) FROM bot_messages WHERE session_id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("读");
        assert_eq!(count, 2000, "重建后行数稳定（旧 2000 行全被差集收走）");
        assert_eq!(ord_max, 1999, "ord 连续无断裂");
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE content LIKE '内容%'",
                [],
                |r| r.get(0),
            )
            .expect("数行");
        assert_eq!(stale, 0, "首批旧行不得残留");
    }
}
