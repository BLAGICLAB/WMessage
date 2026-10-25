//! 连接级 PRAGMA 与老库补列的回归锁。

use super::*;

// ── 连接级 PRAGMA 回归锁 ──

/// PRAGMA 是 **per-connection** 的：漏设一处就静默跑 SQLite 默认档
/// （默认 rollback journal + synchronous=FULL + foreign_keys=OFF），
/// 而默认档在小写入场景下每次 commit 都 fsync、并发读写互相阻塞——症状只是「莫名变慢」。
/// 这里断言三档**真的生效**，而不只是「SQL 写对了」。
#[test]
fn conn_pragmas_are_applied() {
    let dir = std::env::temp_dir().join(format!("wm-pragma-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    apply_conn_pragmas(&conn).unwrap();

    let mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal", "journal_mode 必须是 WAL");
    let sync: i64 = conn
        .query_row("PRAGMA synchronous;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sync, 1, "synchronous 应为 NORMAL(1)，实际 {sync}");
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1, "foreign_keys 应打开（默认 OFF）");

    let _ = fs::remove_dir_all(&dir);
}

/// 老库没有 turn_status 列时必须幂等补上。
///
/// bot_messages 的 DDL 是 CREATE TABLE IF NOT EXISTS——老库走不到那句 DDL，
/// 只剩 ensure_columns 补列这一条路。缺列的后果不是「少个字段」，而是写
/// 历史时直接炸「no such column: turn_status」。
#[test]
fn ensure_columns_adds_turn_status_to_legacy_bot_messages() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
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
         );",
    )
    .unwrap();
    let has_turn_status = |c: &rusqlite::Connection| -> bool {
        c.prepare("PRAGMA table_info(bot_messages)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .any(|n| n == "turn_status")
    };
    ensure_columns(&conn, "bot_messages", &[("turn_status", "TEXT")]).unwrap();
    assert!(has_turn_status(&conn), "老库必须补上 turn_status 列");
    // 幂等：每次 open_db 都会走这一遍，重复补列不得报错
    ensure_columns(&conn, "bot_messages", &[("turn_status", "TEXT")]).unwrap();
    assert!(has_turn_status(&conn), "重复补列不得报错");
}

/// 接线守卫：bot_messages 的幂等补列清单里必须带 turn_status。
///
/// 上面的测试证明的是「机制能补」；这条证明的是「真接上了」——两件事
/// 少一件，老库就永远缺这一列。匹配串用 `concat!` 拆分防自匹配
/// （本测试自身的源码不算命中）。
#[test]
fn bot_messages_ensure_columns_covers_turn_status() {
    let src = include_str!("../mod.rs");
    let needle_call = concat!("ensure_", "columns(");
    let needle_table = concat!("\"bot_", "messages\"");
    let needle_col = concat!("turn_", "status");
    let mut from = 0usize;
    let mut checked = 0usize;
    while let Some(rel) = src[from..].find(needle_call) {
        let at = from + rel;
        from = at + 1;
        let rest = &src[at..];
        let call = &rest[..rest.find(';').unwrap_or(rest.len())];
        if !call.contains(needle_table) {
            continue;
        }
        checked += 1;
        assert!(
            call.contains(needle_col),
            "bot_messages 的幂等补列清单里必须有 turn_status：{call}"
        );
    }
    assert!(checked > 0, "没找到 bot_messages 的幂等补列调用");
}

/// 老库没有 onion 两列时必须补上。
///
/// bot_sessions 的 DDL 是 CREATE TABLE IF NOT EXISTS——老库走不到那句 DDL，
/// 只剩 ensure_onion_config_columns 补列这一条路。缺列的后果不是「少个字段」，
/// 而是每会话读 onion 配置必炸 no such column: onion_config（配置永远不生效）。
#[test]
fn ensure_onion_config_columns_adds_columns_to_legacy_bot_sessions() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE bot_sessions (
             id         TEXT PRIMARY KEY,
             title      TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             updated_at INTEGER NOT NULL
         );",
    )
    .unwrap();
    let cols = |c: &rusqlite::Connection| -> Vec<String> {
        c.prepare("PRAGMA table_info(bot_sessions)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect()
    };
    assert!(
        !cols(&conn).contains(&"onion_config".to_string()),
        "老库本来就没有这两列"
    );
    migrations::ensure_onion_config_columns(&conn).unwrap();
    let after = cols(&conn);
    assert!(
        after.contains(&"onion_config".to_string()),
        "老库必须补上 onion_config 列"
    );
    assert!(
        after.contains(&"onion_max_turn".to_string()),
        "老库必须补上 onion_max_turn 列"
    );
    // 幂等：每次 open_db 都会走这一遍，重复补列不得报错
    migrations::ensure_onion_config_columns(&conn).unwrap();
}

/// 接线守卫：open_db 必须真的调 ensure_onion_config_columns。
///
/// 上面的测试证明的是「机制能补」；这条证明的是「真接上了」——
/// 函数定义了却没人调，老库照样缺这两列。匹配串用 `concat!` 拆分防自匹配。
#[test]
fn open_db_calls_ensure_onion_config_columns() {
    let src = include_str!("../mod.rs");
    let needle = concat!("ensure_onion_", "config_columns(");
    assert!(
        src.contains(needle),
        "open_db 里必须调 ensure_onion_config_columns——定义了没人调等于没有"
    );
}
