//! 聊天记录（消息行）持久化

use tauri::async_runtime;
use tauri::AppHandle;

use super::bot_sessions::BotMsgRow;
use crate::error::{CommandError, CommandResult};

/// 加载指定会话的消息（按写入顺序：行序由自增 id 恢复——写入侧批内 created_at
/// 相同（覆写原子时刻），id 是唯一行序事实源）
pub fn bot_history_load_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<BotMsgRow>, String> {
    let mut stmt = conn
        .prepare("SELECT role, content, refs, thinking, tools, turn_status FROM bot_messages WHERE session_id = ?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok(BotMsgRow {
                role: r.get::<_, String>(0)?,
                content: r.get::<_, String>(1)?,
                refs_json: r.get::<_, Option<String>>(2)?,
                thinking: r.get::<_, Option<String>>(3)?,
                tools_json: r.get::<_, Option<String>>(4)?,
                turn_status: r.get::<_, Option<String>>(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn bot_history_load(app: AppHandle, session_id: String) -> CommandResult<Vec<BotMsgRow>> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        bot_history_load_inner(&conn, &session_id).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("历史读取线程 join 失败：{e}")))?
}

/// 保存指定会话的聊天记录：全量覆盖 + 更新会话活跃时间
///
/// 返回被丢弃的条数（0 = 没截断）：超上限时截断最旧的若干条，丢弃这件事
/// 必须留痕——调用方（`bot_history_save_audited`）据此写审计事件。
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
    conn.execute(
        "DELETE FROM bot_messages WHERE session_id = ?1",
        [session_id],
    )
    .map_err(|e| e.to_string())?;
    // created_at 取单次 now 系有意：整批 DELETE+INSERT 是一次原子覆写，created_at
    // 记录「本次覆写的原子时刻」（同批相同）。行序恢复不依赖 created_at——自增 id
    // 即 (created_at, id) 复合排序键的 row_sequence 分量；按 created_at 排
    // bot_messages 的消费方均属缺陷。
    let now = crate::db::now_ms();
    if !messages.is_empty() {
        let mut stmt = conn
            .prepare(
                "INSERT INTO bot_messages (role, content, refs, session_id, thinking, tools, turn_status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
                m.turn_status,
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
    Ok(dropped)
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
        bot_history_save_audited(&app, &tx, &session_id, &messages)
            .map_err(CommandError::DbError)?;
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
    tx.execute(
        "INSERT INTO bot_messages (role, content, refs, session_id, thinking, tools, turn_status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            message.role,
            message.content,
            message.refs_json,
            session_id,
            message.thinking,
            message.tools_json,
            message.turn_status,
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
                role: "user".into(),
                content: "问".into(),
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: None,
            },
            BotMsgRow {
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
}
