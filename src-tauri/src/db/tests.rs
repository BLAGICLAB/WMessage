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

/// 状态机守卫（数据层兜底）：回收站/归档卡不接受内容编辑与列迁移；
/// 恢复操作（patch 带状态位）放行。绕 UI 闸直调 task_patch / 拖拽迁移
/// 时，这里是最后一道闸。
#[test]
fn sealed_cards_reject_content_edits_and_column_moves() {
    let (_dir, mut conn) = setup_tasks_db();
    let now = 1_000;

    // 回收站卡：内容编辑被拒
    let _g = crate::db::lock_db_write();
    let mut trashed = mk_task("t1", "回收站卡");
    trashed.deleted_at = Some(now);
    upsert_tasks(&conn, std::slice::from_ref(&trashed)).unwrap();
    let err = tasks_patch::task_patch_locked(
        &mut conn,
        "t1",
        &serde_json::json!({"title": "改"}),
        now + 1,
    )
    .err()
    .expect("回收站卡内容编辑应被拒");
    assert!(err.to_string().contains("回收站"), "{err}");
    // 状态位 patch（恢复）放行
    let restored = tasks_patch::task_patch_locked(
        &mut conn,
        "t1",
        &serde_json::json!({"deletedAt": null}),
        now + 1,
    );
    let restored = restored.expect("恢复操作必须放行");
    assert!(restored.deleted_at.is_none());

    // 归档卡：列迁移被拒、内容编辑被拒、状态位 patch 放行
    let mut archived = mk_task("t2", "归档卡");
    archived.column = crate::db::TaskStatus::Done;
    archived.completed_at = Some(now);
    archived.archived = Some(true);
    upsert_tasks(&conn, std::slice::from_ref(&archived)).unwrap();
    let err =
        tasks_patch::task_patch_locked(&mut conn, "t2", &serde_json::json!({"note": "x"}), now + 1)
            .err()
            .expect("归档卡内容编辑应被拒");
    assert!(err.to_string().contains("归档"), "{err}");
    let err =
        tasks_patch::task_set_column_locked(&mut conn, "t2", crate::db::TaskStatus::Doing, now + 1);
    assert!(err.is_err(), "归档卡列迁移应被拒");
}

/// `collapsed` 是折叠视图状态，不在内容编辑闸里：归档/回收站卡的折叠键
/// 必须能点开（点开才能看/打开/复制绑定的文件与文件夹）。混合补丁里只要
/// 携带任一真内容键，整体仍须被拒——放行不得变成绕行口。
#[test]
fn sealed_cards_allow_collapsed_view_state_but_still_reject_content() {
    let (_dir, mut conn) = setup_tasks_db();
    let now = 1_000;
    let _g = crate::db::lock_db_write();

    for (id, trashed) in [("t1", true), ("t2", false)] {
        let mut task = mk_task(id, "封存卡");
        if trashed {
            task.deleted_at = Some(now);
        } else {
            task.column = crate::db::TaskStatus::Done;
            task.completed_at = Some(now);
            task.archived = Some(true);
        }
        upsert_tasks(&conn, std::slice::from_ref(&task)).unwrap();

        // 纯视图补丁放行：展开（collapsed=false）落库可往返
        let row = tasks_patch::task_patch_locked(
            &mut conn,
            id,
            &serde_json::json!({"collapsed": false}),
            now + 1,
        )
        .unwrap_or_else(|e| panic!("封存卡折叠态应放行，got {e:?}"));
        assert_eq!(row.collapsed, Some(false), "{id} 展开态应落库");
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        assert_eq!(reloaded.collapsed, Some(false), "{id} 展开态应可往返");

        // 混合补丁：夹带真内容键 → 整体拒（collapsed 不得当绕行口）
        let err = tasks_patch::task_patch_locked(
            &mut conn,
            id,
            &serde_json::json!({"collapsed": true, "title": "改"}),
            now + 1,
        )
        .err()
        .unwrap_or_else(|| panic!("{id} 混合内容补丁应被拒"));
        assert!(err.to_string().contains("不能编辑内容"), "{err}");
        // 拒后无半写：collapsed 保持上一笔的 false
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        assert_eq!(reloaded.collapsed, Some(false), "{id} 拒后不得半写");
    }
}
