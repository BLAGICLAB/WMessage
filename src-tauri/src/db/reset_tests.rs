use super::*;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 首次 UPDATE 失败不消耗 token——下次重试成功后才置位，此后不再执行。
#[test]
fn reset_bot_assigned_retries_on_failure() {
    let done = AtomicBool::new(false);
    let calls = AtomicUsize::new(0);

    // 第一次失败（模拟首开遇库忙）：不置位，留待重试
    let r = reset_bot_assigned_with(&done, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Err("database is locked".into())
    });
    assert!(r.is_err(), "失败应返回 Err");
    assert!(!done.load(Ordering::SeqCst), "失败不得消耗 token");

    // 第二次重试成功 → 置位
    let r = reset_bot_assigned_with(&done, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    assert!(r.is_ok());
    assert!(done.load(Ordering::SeqCst));

    // 第三次：已置位 → 直接 true，不再执行 exec（保留「仅清一次」语义）
    let r = reset_bot_assigned_with(&done, || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    assert!(r.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2, "置位后不应再执行 UPDATE");
}

/// 真实库——重试成功时确实清掉残留 bot_assigned 标志。
#[test]
fn reset_bot_assigned_real_db_clears_flag() {
    let dir = std::env::temp_dir().join(format!("wm-reset-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, bot_assigned INTEGER);")
        .unwrap();
    conn.execute(
        "INSERT INTO tasks (id, bot_assigned) VALUES ('t1', 1), ('t2', 0)",
        [],
    )
    .unwrap();
    let done = AtomicBool::new(false);
    let r = reset_bot_assigned_with(&done, || {
        conn.execute(
            "UPDATE tasks SET bot_assigned = 0 WHERE bot_assigned = 1",
            [],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    });
    assert!(r.is_ok());
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tasks WHERE bot_assigned = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "残留 bot_assigned 应被清掉");
    fs::remove_dir_all(&dir).ok();
}
