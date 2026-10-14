//! 标签整理端到端：scan → merge → list → rollback 全链路。
//!
//! 不依赖 Tauri runtime（tag_merge_history 表是普通 SQLite 表，core 函数
//! 都接受 &Connection / &AppHandle<MockRuntime>）；用 tauri::test::mock_app()
//! 拿 app handle 满足 rollback 签名。tags_consolidate 走 db::open_db 的
//! mock variant 在 mock app 上不工作——所以连接直接用 Connection::open
//! in-memory + 共享的建表 DDL。

use std::collections::BTreeSet;

use wmessage_lib::bot::tags_consolidate::{
    cluster_archived_tags, merge_tag_cluster, rollback_tag_merge,
};

#[test]
fn tag_consolidate_e2e_scan_merge_rollback() {
    // 1. 内存库 + 必要 DDL（tasks + tag_merge_history，与 db/mod.rs bootstrap 同步）
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL DEFAULT '',
            tags TEXT,
            archived INTEGER,
            deleted_at INTEGER
         );
         CREATE TABLE tag_merge_history (
            run_id TEXT PRIMARY KEY,
            merged_to TEXT NOT NULL,
            merged_from TEXT NOT NULL,
            previous_tags_json TEXT NOT NULL,
            affected_task_count INTEGER NOT NULL,
            rolled_back_at INTEGER,
            created_at INTEGER NOT NULL
         );",
    )
    .unwrap();

    // 2. 准备 3 个归档卡，tags 含明显同义
    let insert = |id: &str, tags: &[&str], archived: bool| {
        let json = serde_json::to_string(tags).unwrap();
        conn.execute(
            "INSERT INTO tasks (id, title, tags, archived) VALUES (?1, '', ?2, ?3)",
            rusqlite::params![id, json, archived as i64],
        )
        .unwrap();
    };
    insert("t1", &["买菜", "购菜", "groceries"], true);
    insert("t2", &["买菜"], true);
    insert("t3", &["购菜"], true);
    insert("active", &["买菜"], false); // 主面板卡——应被 WHERE 排除

    // 3. scan：宽松阈值 0.5 拿到簇（0.92 起步阈值对 "买菜/购菜/groceries"
    //    的具体 cos 取决于嵌入引擎；这里降到 0.5 保证至少 1 簇）
    let scan = cluster_archived_tags(&conn, Some(0.5)).unwrap();
    assert!(scan.archived_tasks_scanned >= 3, "应扫到 3 个归档卡");
    assert!(!scan.clusters.is_empty(), "应至少 1 个簇");
    let cluster = &scan.clusters[0];
    let member_set: BTreeSet<&str> = cluster.members.iter().map(String::as_str).collect();
    assert!(
        member_set.contains("买菜") || member_set.contains("购菜"),
        "簇成员应包含本测试标签之一：{:?}",
        cluster.members
    );

    // 4. merge：用簇的 suggested_main + 非 main 的成员
    let members_refs: Vec<&str> = cluster
        .members
        .iter()
        .map(String::as_str)
        .filter(|m| **m != cluster.suggested_main)
        .collect();
    assert!(!members_refs.is_empty(), "应至少 1 个别名可合并");
    let run = merge_tag_cluster(&conn, &cluster.suggested_main, &members_refs).unwrap();
    assert_eq!(run.merged_to, cluster.suggested_main);
    assert!(!run.affected_task_ids.is_empty(), "应至少影响 1 个归档卡");
    assert!(
        !run.affected_task_ids.contains(&"active".to_string()),
        "主面板卡不应被合并：{:?}",
        run.affected_task_ids
    );

    // 5. 验证：归档卡 tags 含主标签 + 不含别名
    for affected_id in &run.affected_task_ids {
        let raw: String = conn
            .query_row("SELECT tags FROM tasks WHERE id = ?1", [affected_id], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert!(
            parsed.contains(&run.merged_to),
            "归档卡 {affected_id} tags 应含主标签 #{}",
            run.merged_to,
        );
        for m in &members_refs {
            assert!(
                !parsed.contains(&m.to_string()),
                "归档卡 {affected_id} tags 不应含别名 #{m}",
            );
        }
    }

    // 6. history 表有 1 条
    let hist_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tag_merge_history WHERE run_id = ?1",
            [&run.run_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hist_count, 1);

    // 7. rollback：mock app 走 AppHandle<MockRuntime>
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let rb = rollback_tag_merge(&handle, &conn, &run.run_id).unwrap();
    assert_eq!(rb.run_id, run.run_id);
    assert_eq!(rb.restored_task_count, run.affected_task_ids.len());

    // 8. 验证：归档卡 tags 恢复原值
    for affected_id in &run.affected_task_ids {
        let raw: String = conn
            .query_row("SELECT tags FROM tasks WHERE id = ?1", [affected_id], |r| {
                r.get(0)
            })
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&raw).unwrap();
        let old = run.previous_tags.get(affected_id).unwrap();
        assert_eq!(
            &parsed, old,
            "归档卡 {affected_id} rollback 后应回到旧 tags",
        );
    }

    // 9. 二次 rollback 应 Err（rolled_back_at 已置）
    let rb2 = rollback_tag_merge(&handle, &conn, &run.run_id);
    assert!(rb2.is_err(), "二次 rollback 应被 rolled_back_at 拒绝");
}
