//! 导入与 upsert 冲突：lost update 守卫、created_at 不覆盖、NULL 行基线。

use super::*;

/// 模拟旧 schema 的任务行（updated_at IS NULL），验证 tasks_import
/// 读取不崩：r.get::<_, Option<i64>>(0) + flatten + unwrap_or(0)。
/// 直接 r.get::<_, i64>(0) 遇 NULL 报 InvalidColumnType，整次导入失败。
#[test]
fn null_updated_at_handled_gracefully_in_import() {
    use rusqlite::OptionalExtension;

    let dir = std::env::temp_dir().join(format!("wm-b4-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (id TEXT PRIMARY KEY, updated_at INTEGER);
         INSERT INTO tasks (id) VALUES ('legacy-1');             -- NULL updated_at
         INSERT INTO tasks (id, updated_at) VALUES ('new-1', 100); -- 有时间戳",
    )
    .unwrap();

    // NULL 行：r.get::<_, Option<i64>>(0) → Some(None)（直接读 i64 会报 Err）
    let cur_legacy: Option<Option<i64>> = conn
        .query_row(
            "SELECT updated_at FROM tasks WHERE id = 'legacy-1'",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .unwrap();
    assert_eq!(cur_legacy, Some(None), "NULL 应读为 Some(None) 而非报错");
    assert_eq!(cur_legacy.flatten().unwrap_or(0), 0, "NULL 抹平为 0");

    // 有值行：仍正常读出
    let cur_new: Option<Option<i64>> = conn
        .query_row("SELECT updated_at FROM tasks WHERE id = 'new-1'", [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .optional()
        .unwrap();
    assert_eq!(cur_new, Some(Some(100)));
    assert_eq!(cur_new.flatten().unwrap_or(0), 100);

    // 模拟 merge 取舍：外部 updated_at=50 vs 老行 NULL → 外部胜出（cur_ua=0, 50>0）
    let cur_ua = cur_legacy.flatten().unwrap_or(0);
    assert!(50_i64 > cur_ua, "外部带时间戳的应压过老行");
    // 外部 updated_at=0 vs 老行 NULL → 老行不被动（0 不大于 0）
    assert!(!(0_i64 > cur_ua), "外部无时间戳时不应压过老行");

    // 无该 id 行：cur=None → flatten 后 unwrap_or(0) → 0 → 外部压入
    let cur_none: Option<Option<i64>> = conn
        .query_row(
            "SELECT updated_at FROM tasks WHERE id = 'missing'",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .unwrap();
    assert_eq!(cur_none, None);
    assert_eq!(cur_none.flatten().unwrap_or(0), 0);

    fs::remove_dir_all(&dir).ok();
}

/// upsert WHERE 守卫防 lost update——直接调生产 upsert_tasks
/// （内联复刻生产 SQL 会在守卫改动后测试不同步，属弱断言）。
/// 五场景：incoming>current / incoming<current / 相等 / 老 NULL 行 / incoming NULL
#[test]
fn upsert_where_guard_prevents_lost_update() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, conn) = setup_tasks_db();
    let title_of = |conn: &rusqlite::Connection, id: &str| -> String {
        conn.query_row("SELECT title FROM tasks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap()
    };
    // 老 NULL 行（早期 schema 遗留）：生产 upsert 写全列，NULL 行只能 SQL 直插
    conn.execute(
        "INSERT INTO tasks (id, title, col) VALUES ('legacy', 'legacy-row', 'todo')",
        [],
    )
    .unwrap();

    // 场景 1: incoming(100) > current(50) → 更新
    let mut t = mk_task("t1", "old-50");
    t.updated_at = Some(50);
    upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    t.title = "new-100".into();
    t.updated_at = Some(100);
    upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    assert_eq!(title_of(&conn, "t1"), "new-100", "场景 1: 更新的应压过老的");

    // 场景 2: incoming(60) < current(100) → 拒绝（显式 lost update 防护）
    t.title = "old-snapshot-60".into();
    t.updated_at = Some(60);
    let err = upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap_err();
    assert!(
        err.starts_with(super::tasks::CONFLICT_ERR_PREFIX),
        "场景 2: 更老的写入应被显式拒，err={err}"
    );
    assert_eq!(
        title_of(&conn, "t1"),
        "new-100",
        "场景 2: 更老的不应压过更新的（数据不变量保留）"
    );

    // 场景 3: 相等 timestamp → 允许更新
    t.title = "equal-100".into();
    t.updated_at = Some(100);
    upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        title_of(&conn, "t1"),
        "equal-100",
        "场景 3: 相等 timestamp 仍允许更新"
    );

    // 场景 4: 老 NULL 行被任何 incoming 覆盖
    let mut l = mk_task("legacy", "new-over-legacy");
    l.updated_at = Some(5);
    upsert_tasks(&conn, &[l]).unwrap();
    assert_eq!(
        title_of(&conn, "legacy"),
        "new-over-legacy",
        "场景 4: 老 NULL 行被任何 incoming 覆盖"
    );

    // 场景 5: incoming NULL 不应覆盖 current 有值
    conn.execute(
        "UPDATE tasks SET title='keep-me', updated_at=200 WHERE id='t1'",
        [],
    )
    .unwrap();
    t.title = "incoming-null".into();
    t.updated_at = None;
    let err = upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap_err();
    assert!(
        err.starts_with(super::tasks::CONFLICT_ERR_PREFIX),
        "场景 5: incoming NULL 应被显式拒，err={err}"
    );
    assert_eq!(
        title_of(&conn, "t1"),
        "keep-me",
        "场景 5: incoming NULL 不应覆盖 current 有值（数据不变量保留）"
    );

    fs::remove_dir_all(&dir).ok();
}

/// created_at 回归锁：INSERT 时落库；UPDATE（ON CONFLICT 分支）**不覆盖**已有值
/// （照 workflows.created_at 先例——UPDATE SET 刻意不含 created_at）。
/// 老行 NULL 也不被后续 upsert 回填。
#[test]
fn upsert_insert_stamps_created_at_update_never_overwrites() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, conn) = setup_tasks_db();
    let created_at_of = |conn: &rusqlite::Connection, id: &str| -> Option<i64> {
        conn.query_row("SELECT created_at FROM tasks WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    };

    // 新行 INSERT：created_at 正确落库
    let mut t = mk_task("t1", "新建");
    t.updated_at = Some(1000);
    t.created_at = Some(1000);
    upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        created_at_of(&conn, "t1"),
        Some(1000),
        "INSERT 应落 created_at"
    );

    // 同 id 再 upsert（created_at=2000）→ UPDATE 分支不得覆盖
    t.created_at = Some(2000);
    t.updated_at = Some(2000);
    t.title = "更新".into();
    upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        created_at_of(&conn, "t1"),
        Some(1000),
        "UPDATE 不得覆盖已有 created_at"
    );

    // 老行（created_at IS NULL，模拟 ALTER 后未回填）：UPDATE 不回填、保持 NULL
    let mut legacy = mk_task("legacy", "老行");
    legacy.updated_at = Some(3000);
    legacy.created_at = None;
    upsert_tasks(&conn, std::slice::from_ref(&legacy)).unwrap();
    assert_eq!(
        created_at_of(&conn, "legacy"),
        None,
        "老行应保持 NULL 不回填"
    );

    fs::remove_dir_all(&dir).ok();
}

/// 核心回归：两写者读同一快照后交错写回——
/// 后写者基线比对失败被拒（Err 含 CONFLICT_ERR_PREFIX），先写者的字段修改不丢；
/// 后写者重读刷新基线后重试可成功。另覆盖「快照后行被删 → 拒写防复活」。
#[test]
fn upsert_expected_baseline_rejects_stale_write() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, conn) = setup_tasks_db();
    let mut seed = mk_task("t1", "原始");
    seed.updated_at = Some(100);
    seed.note = Some("原始备注".into());
    upsert_tasks(&conn, &[seed]).unwrap();

    // 写者 A / B 读同一快照（updated_at=100）
    let snap_a = load_all(&conn).unwrap().into_iter().next().unwrap();
    let snap_b = snap_a.clone();
    assert_eq!(snap_a.updated_at, Some(100));

    // A 先写回：改标题，刷新时间戳，带基线 → 放行
    let mut a = snap_a.clone();
    prepare_for_upsert(&mut a);
    a.title = "A改的标题".into();
    a.updated_at = Some(200);
    upsert_tasks(&conn, std::slice::from_ref(&a)).unwrap();

    // B 后写回：基于同一旧快照改备注，时间戳更新（300>200，旧时间戳守卫会放行）→ 必须被基线拒
    let mut b = snap_b;
    prepare_for_upsert(&mut b);
    b.note = Some("B改的备注".into());
    b.updated_at = Some(300);
    let err = upsert_tasks(&conn, std::slice::from_ref(&b)).unwrap_err();
    assert!(
        err.starts_with(CONFLICT_ERR_PREFIX),
        "后写者基线过期必须拒写；got: {err}"
    );
    let cur = load_all(&conn).unwrap().into_iter().next().unwrap();
    assert_eq!(cur.title, "A改的标题", "先写者的字段修改不得被覆盖");
    assert_eq!(
        cur.note.as_deref(),
        Some("原始备注"),
        "被拒写者的修改不得落库"
    );
    assert_eq!(cur.updated_at, Some(200));

    // B 重读刷新基线后重试 → 放行（冲突可见、可恢复，而非静默丢）
    let mut b2 = load_all(&conn).unwrap().into_iter().next().unwrap();
    prepare_for_upsert(&mut b2);
    b2.note = Some("B改的备注".into());
    b2.updated_at = Some(300);
    upsert_tasks(&conn, std::slice::from_ref(&b2)).unwrap();
    let cur = load_all(&conn).unwrap().into_iter().next().unwrap();
    assert_eq!(cur.title, "A改的标题");
    assert_eq!(cur.note.as_deref(), Some("B改的备注"));

    // 快照后行被删：带基线写回 → 拒写（防复活已删行）
    delete_tasks(&conn, &["t1".to_string()]).unwrap();
    let err = upsert_tasks(&conn, std::slice::from_ref(&b2)).unwrap_err();
    assert!(
        err.starts_with(CONFLICT_ERR_PREFIX),
        "行已删必须拒写；got: {err}"
    );
    assert!(load_all(&conn).unwrap().is_empty(), "被拒写不得复活已删行");

    // 无基线（expected_updated_at=None）保持原行为：新建直插、时间戳守卫兜底
    let mut fresh = mk_task("t2", "新建无基线");
    fresh.updated_at = Some(50);
    upsert_tasks(&conn, &[fresh]).unwrap();
    assert_eq!(load_all(&conn).unwrap().len(), 1);

    fs::remove_dir_all(&dir).ok();
}

/// 老行 updated_at 为 NULL 时用「行存在性」哨兵基线
/// （BASELINE_NULL_ROW）——行原样放行；行被改（updated_at 变非 NULL）或
/// 被删都拒写。NULL 行基线无法做时间戳比对，哨兵补齐老行的 lost-update 防护。
#[test]
fn upsert_null_row_existence_baseline() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, conn) = setup_tasks_db();
    // 模拟迁移前的老行：updated_at 为 NULL
    let mut seed = mk_task("t1", "老行");
    seed.updated_at = None;
    upsert_tasks(&conn, &[seed]).unwrap();

    // 行原样（仍在且仍 NULL）→ 放行
    let mut a = load_all(&conn).unwrap().into_iter().next().unwrap();
    assert_eq!(a.updated_at, None);
    prepare_for_upsert(&mut a);
    a.title = "放行".into();
    a.updated_at = Some(100);
    upsert_tasks(&conn, std::slice::from_ref(&a)).unwrap();
    assert_eq!(load_all(&conn).unwrap()[0].title, "放行");

    // 快照时行是 NULL（基线=行存在性），但窗口内其他写者已改（updated_at=100 非 NULL）→ 拒
    let mut b = mk_task("t1", "覆盖者");
    b.updated_at = None; // 显式 None，触发 prepare_for_upsert 的 BASELINE_NULL_ROW 哨兵分支
    prepare_for_upsert(&mut b);
    b.updated_at = Some(200);
    let err = upsert_tasks(&conn, std::slice::from_ref(&b)).unwrap_err();
    assert!(
        err.starts_with(CONFLICT_ERR_PREFIX),
        "老行被改后行存在性基线必须拒写；got: {err}"
    );
    assert_eq!(
        load_all(&conn).unwrap()[0].title,
        "放行",
        "被拒写不得覆盖现行行"
    );

    // 快照后行被删 → 拒写（防复活）
    delete_tasks(&conn, &["t1".to_string()]).unwrap();
    let err = upsert_tasks(&conn, std::slice::from_ref(&b)).unwrap_err();
    assert!(
        err.starts_with(CONFLICT_ERR_PREFIX),
        "行已删必须拒写；got: {err}"
    );
    assert!(load_all(&conn).unwrap().is_empty(), "被拒写不得复活已删行");

    fs::remove_dir_all(&dir).ok();
}
