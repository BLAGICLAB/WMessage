//! 数据库层的单测。
//!
//! 按主题拆到 `db/tests/` 子文件（每个子文件一个主题面），这里只留共用辅助
//! 与 mod 声明。

use super::*;
use crate::migration::ops::copy_dir_recursive;
use std::fs;

#[cfg(test)]
mod files;
#[cfg(test)]
mod history;
#[cfg(test)]
mod legacy_wal;
#[cfg(test)]
mod migrate_json;
#[cfg(test)]
mod pragmas;
#[cfg(test)]
mod session;
#[cfg(test)]
mod upsert;

// ── 共用辅助 ──
fn setup_tasks_db() -> (std::path::PathBuf, rusqlite::Connection) {
    let dir = std::env::temp_dir().join(format!("wm-mig-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    // 单源 DDL：migrate_data_json 走 upsert_tasks 写路径，需全列 schema
    conn.execute_batch(TASKS_DDL).unwrap();
    (dir, conn)
}

fn mk_task(id: &str, title: &str) -> Task {
    Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: id.into(),
        title: title.into(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column: crate::db::TaskStatus::Todo,
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(1),
        bot_assigned: None,
        assignee: None,
        budget: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,
        created_at: None,
        expected_updated_at: None,
    }
}
/// 构造一个临时带 bot_sessions / bot_messages 表的 Connection
fn setup_bhs_db() -> (std::path::PathBuf, rusqlite::Connection) {
    let dir = std::env::temp_dir().join(format!("wm-bhs-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE bot_sessions (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            is_subagent INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE bot_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            refs TEXT,
            thinking TEXT,
            tools TEXT,
            turn_status TEXT,
            created_at INTEGER NOT NULL
        );",
    )
    .unwrap();
    // 会话删除会顺带清子 agent 终态行，夹具得带上这张表（单源 DDL）
    conn.execute_batch(subagents::SUBAGENTS_DDL).unwrap();
    (dir, conn)
}
