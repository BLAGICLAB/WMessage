use super::*;

fn setup_conn() -> rusqlite::Connection {
    super::super::test_conn(&[TASKS_DDL])
}

fn insert_task(conn: &rusqlite::Connection, id: &str, col: &str, bot: Option<bool>) {
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at, bot_assigned) VALUES (?1, ?2, ?3, 1000, ?4)",
        rusqlite::params![id, format!("t-{id}"), col, bot],
    )
    .unwrap();
}

fn insert_owned_task(conn: &rusqlite::Connection, id: &str, owner: Option<&str>) {
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at, owner_id) VALUES (?1, ?2, 'todo', 1000, ?3)",
        rusqlite::params![id, format!("t-{id}"), owner],
    )
    .unwrap();
}

// ── 删除非本人任务卡（数据管理卡入口；硬删含回收站） ──

/// 筛选口径：owner 非本人且非 NULL 的卡全部硬删（含回收站/归档——统计用数据
/// 一次清干净）；本人卡与 NULL 归属卡一概不动
#[test]
fn delete_non_self_filters_owner_hard_deletes() {
    let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求
    let conn = setup_conn();
    insert_owned_task(&conn, "mine", Some("self-pid"));
    insert_owned_task(&conn, "null-owner", None); // NULL = 本人（图谱设计 §1.1）
    insert_owned_task(&conn, "other-1", Some("someone-else"));
    insert_owned_task(&conn, "other-2", Some("another"));
    // 回收站里的非本人卡：统计用数据一次清干净，同样硬删
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at, owner_id, deleted_at) VALUES ('other-trashed', 't', 'todo', 1000, 'someone-else', 900)",
        [],
    )
    .unwrap();

    let (count, ids) = delete_non_self_locked(&conn, "self-pid").unwrap();
    assert_eq!(count, 3, "other-1/other-2/other-trashed 全删");
    assert_eq!(ids.len(), 3);

    let remaining: Vec<String> = load_all(&conn)
        .unwrap()
        .iter()
        .map(|t| t.id.clone())
        .collect();
    assert!(remaining.contains(&"mine".to_string()), "本人卡不动");
    assert!(
        remaining.contains(&"null-owner".to_string()),
        "NULL 归属 = 本人，不动"
    );
    assert!(
        !remaining.iter().any(|i| i.starts_with("other")),
        "非本人卡已硬删"
    );
}

/// 无命中空转返回 0
#[test]
fn delete_non_self_noop() {
    let _g = crate::db::lock_db_write();
    let conn = setup_conn();
    insert_owned_task(&conn, "mine", Some("self-pid"));
    insert_owned_task(&conn, "null-owner", None);
    let (count, ids) = delete_non_self_locked(&conn, "self-pid").unwrap();
    assert_eq!((count, ids.len()), (0, 0));
    assert_eq!(load_all(&conn).unwrap().len(), 2);
}

#[test]
fn set_column_todo_clears_completion_keeps_bot_assigned() {
    let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求（测试模块的 super 是 tasks 非 db）
    let mut conn = setup_conn();
    insert_task(&conn, "a", "done", Some(true));
    let row = task_set_column_locked(&mut conn, "a", TaskStatus::Todo, 5000).unwrap();
    assert_eq!(row.column, TaskStatus::Todo);
    assert_eq!(row.completed_at, None);
    assert_eq!(row.archived, None);
    // bot_assigned 保留（机器人代办的完成语义）
    assert_eq!(row.bot_assigned, Some(true));
    assert_eq!(row.updated_at, Some(5000));
    // 基线 = 锁内现读值，写后重新加载一致
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "a")
        .unwrap();
    assert_eq!(reloaded.column, TaskStatus::Todo);
    assert_eq!(reloaded.updated_at, Some(5000));
}

#[test]
fn set_column_done_records_completion_clears_bot_assigned() {
    let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求（测试模块的 super 是 tasks 非 db）
    let mut conn = setup_conn();
    insert_task(&conn, "b", "todo", Some(true));
    let row = task_set_column_locked(&mut conn, "b", TaskStatus::Done, 6000).unwrap();
    assert_eq!(row.column, TaskStatus::Done);
    assert_eq!(row.completed_at, Some(6000));
    assert_eq!(row.archived, Some(false));
    // 用户完成清机器人标记（显示用户头像）
    assert_eq!(row.bot_assigned, None);
}

#[test]
fn set_column_missing_id_is_task_not_found() {
    let mut conn = setup_conn();
    let err = match task_set_column_locked(&mut conn, "ghost", TaskStatus::Done, 1) {
        Err(e) => e,
        Ok(_) => panic!("missing id 应返回 TaskNotFound"),
    };
    assert!(matches!(err, CommandError::TaskNotFound(_)), "got: {err:?}");
}
