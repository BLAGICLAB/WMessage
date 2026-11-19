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
        ("db/bot_history.rs", include_str!("../bot_history.rs")),
        ("bot_chat.rs", include_str!("../../bot_chat.rs")),
        (
            "bot_orchestrator.rs",
            include_str!("../../bot_orchestrator.rs"),
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
        file_changes_json: Some(r#"[{"path":"/a","kind":"create","added":1,"deleted":0}]"#.into()),
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
