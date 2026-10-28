//! migrate_data_json 的触发条件：json 含库缺失 id 即补回。

use super::*;

// ── migrate_data_json 触发条件：json 含库缺失 id 即补回 ──

/// prepare_for_upsert: updated_at=Some(x) → 基线=Some(x), updated_at 刷成 now
#[test]
fn prepare_for_upsert_with_some_timestamp_uses_current_as_baseline() {
    let mut t = mk_task("t1", "test");
    t.updated_at = Some(12345);
    t.expected_updated_at = None;
    prepare_for_upsert(&mut t);
    assert_eq!(t.expected_updated_at, Some(12345));
    assert!(t.updated_at.unwrap() > 12345, "updated_at 必须被刷新为 now");
}

/// prepare_for_upsert: updated_at=None → 基线=Some(BASELINE_NULL_ROW), updated_at 刷成 now
#[test]
fn prepare_for_upsert_with_none_timestamp_uses_baseline_null_row() {
    let mut t = mk_task("t1", "test");
    t.updated_at = None;
    t.expected_updated_at = None;
    let before_ms = crate::db::now_ms();
    prepare_for_upsert(&mut t);
    assert_eq!(t.expected_updated_at, Some(BASELINE_NULL_ROW));
    // 防止 `Some(0)` 这种"非 now 但 is_some"的回归
    let stamped = t.updated_at.expect("updated_at 必须被刷新");
    assert!(
        stamped >= before_ms,
        "updated_at 必须 >= 调用前时间戳; got {stamped}, pre-call {before_ms}"
    );
}

/// 删任务后重启场景：库里只剩 t1，老 data.json 还有 t1/t2/t3 → 首次评估补回 t2/t3，
/// 且已存在的 t1 不得被 json 旧值覆盖（时间戳守卫之外再加 id 过滤）。
/// 迁移/评估成功后 data.json 改名退役——再次评估不再复活已删任务。
#[test]
fn migrate_data_json_backfills_missing_after_user_delete() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, mut conn) = setup_tasks_db();
    upsert_tasks(&conn, &[mk_task("t1", "新标题")]).unwrap();

    let json_tasks = vec![
        mk_task("t1", "旧标题"),
        mk_task("t2", "被删的任务2"),
        mk_task("t3", "被删的任务3"),
    ];
    let file = dir.join("data.json");
    fs::write(&file, serde_json::to_string(&json_tasks).unwrap()).unwrap();

    assert!(
        migrate_data_json_file(&file, &mut conn),
        "json 含库缺失的 t2/t3 必须触发迁移补回"
    );
    let tasks = load_all(&conn).unwrap();
    assert_eq!(tasks.len(), 3, "t2/t3 应补回");
    let t1 = tasks.iter().find(|t| t.id == "t1").unwrap();
    assert_eq!(t1.title, "新标题", "已有 id 不得被 json 旧值覆盖");
    assert!(!file.exists(), "迁移成功后 data.json 应退役（改名）");
    assert!(
        dir.join("data.json.migrated").exists(),
        "退役文件保留为 data.json.migrated（可人工找回）"
    );
    // 关键回归：退役后用户硬删 t2，再次评估不得复活
    delete_tasks(&conn, &["t2".to_string()]).unwrap();
    assert!(
        !migrate_data_json_file(&file, &mut conn),
        "文件已退役：不存在即不评估"
    );
    let tasks = load_all(&conn).unwrap();
    assert_eq!(tasks.len(), 2, "已删任务不得被残留 json 复活");
    fs::remove_dir_all(&dir).ok();
}

/// json 内容已全部在库里 → 不迁移，但文件同样退役（防未来硬删后计数复活）
#[test]
fn migrate_data_json_skips_when_db_not_behind() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, mut conn) = setup_tasks_db();
    upsert_tasks(&conn, &[mk_task("t1", "a"), mk_task("t2", "b")]).unwrap();
    let file = dir.join("data.json");
    fs::write(
        &file,
        serde_json::to_string(&vec![mk_task("t1", "old")]).unwrap(),
    )
    .unwrap();

    assert!(
        !migrate_data_json_file(&file, &mut conn),
        "json 无库缺失 id 不得迁移"
    );
    assert!(!file.exists(), "评估成功后文件应退役");
    assert!(dir.join("data.json.migrated").exists());
    let tasks = load_all(&conn).unwrap();
    assert_eq!(tasks.len(), 2, "库内容不得变化");
    fs::remove_dir_all(&dir).ok();
}
