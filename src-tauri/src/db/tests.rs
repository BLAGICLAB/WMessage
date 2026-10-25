use super::*;
use crate::migration::ops::copy_dir_recursive;
use std::fs;

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
    let src = include_str!("mod.rs");
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
    let src = include_str!("mod.rs");
    let needle = concat!("ensure_onion_", "config_columns(");
    assert!(
        src.contains(needle),
        "open_db 里必须调 ensure_onion_config_columns——定义了没人调等于没有"
    );
}

// ── legacy WAL copy 不吞错 + 边车文件一并拷贝 ──

/// 老库 WAL 被占（另一连接持读锁）→ checkpoint TRUNCATE 必失败（SQLITE_BUSY）、
/// WAL 完好——拷后三份都到位；checkpoint 出错路径产生 warn（不静默）
#[test]
fn copy_legacy_db_copies_wal_shm_and_warns_on_checkpoint_failure() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let legacy = dir.join("wmessage.db");
    // 真实 WAL 库 + 未 checkpoint 的写入 → -wal/-shm 存在且非空
    let keeper = rusqlite::Connection::open(&legacy).unwrap();
    keeper
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE t (id TEXT);
             INSERT INTO t VALUES ('x');",
        )
        .unwrap();
    assert!(wal_sidecar(&legacy, "wal").exists(), "setup: WAL 应存在");
    assert!(wal_sidecar(&legacy, "shm").exists(), "setup: SHM 应存在");
    let dst = dir.join("new").join("wmessage.db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    let warns = {
        // keeper 持「未读完的读游标」：读标记一直挂在 wal 上（已完结的 SELECT 会释放标记，
        // TRUNCATE 就不阻塞了）；writer 追加新帧使 wal 尾继续前进 →
        // copy_legacy_db 内 checkpoint TRUNCATE 必 BUSY 失败且 wal 不被截断
        let mut stmt = keeper.prepare("SELECT id FROM t").unwrap();
        let mut rows = stmt.query([]).unwrap();
        let _ = rows.next().unwrap(); // 游标保持打开 = 读标记存活
        let writer = rusqlite::Connection::open(&legacy).unwrap();
        writer.execute_batch("INSERT INTO t VALUES ('y');").unwrap();
        let warns = copy_legacy_db(&legacy, &dst).unwrap();
        drop(writer);
        warns
    };

    assert!(dst.exists(), "主库必须拷过来");
    assert!(
        wal_sidecar(&dst, "wal").exists(),
        "-wal 边车必须拷过来（否则 checkpoint 失败时 WAL 写入静默丢失）"
    );
    assert!(wal_sidecar(&dst, "shm").exists(), "-shm 边车必须拷过来");
    assert!(
        warns.iter().any(|w| w.contains("checkpoint")),
        "checkpoint 失败必须产生 warn，不得 let _ 吞掉；got: {warns:?}"
    );
    drop(keeper);
    fs::remove_dir_all(&dir).ok();
}

/// 正常老库（真 sqlite）→ checkpoint 成功，无 warn，主库内容一致
#[test]
fn copy_legacy_db_happy_path_no_warns() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let legacy = dir.join("wmessage.db");
    {
        let c = rusqlite::Connection::open(&legacy).unwrap();
        c.execute_batch("CREATE TABLE t (id TEXT); INSERT INTO t VALUES ('x');")
            .unwrap();
    }
    let dst = dir.join("wmessage-copy.db");
    let warns = copy_legacy_db(&legacy, &dst).unwrap();
    assert!(warns.is_empty(), "happy path 不应有 warn；got: {warns:?}");
    let c = rusqlite::Connection::open(&dst).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "拷贝后的库应含老数据");
    fs::remove_dir_all(&dir).ok();
}

/// staging 失败：legacy_db 路径不存在 → fs::copy 失败 → Err + tmp-* 无残留。
/// 本函数 trust caller 前置 `if !db_path.exists()`，本测试验证 staging 阶段本身不漏 tmp。
#[test]
fn copy_legacy_db_returns_err_when_legacy_db_missing() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-missing-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let legacy = dir.join("nonexistent-dir").join("wmessage.db"); // parent 不存在 → open 失败 + copy 失败
    let dst = dir.join("new").join("wmessage.db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    let result = copy_legacy_db(&legacy, &dst);

    assert!(result.is_err(), "legacy_db 不存在应返 Err，实际 {result:?}");
    assert!(
        !dst.exists(),
        "dst 不应被创建（Phase 1 fail 已 clean tmp-*）"
    );
    fs::remove_dir_all(&dir).ok();
}

/// 源库 open 失败（损坏/占用/加密类）→ fail-closed：Err 拒拷贝，
/// dst 主库/-wal/-shm 一概不产生，无 staging 残留。
#[test]
fn copy_legacy_db_open_failure_fails_closed() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-openfail-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    // 目标路径是目录 → Connection::open 必败（open(2) EISDIR）
    let legacy = dir.join("not-a-db");
    fs::create_dir_all(&legacy).unwrap();
    let dst = dir.join("new").join("wmessage.db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    let result = copy_legacy_db(&legacy, &dst);

    let err = result.expect_err("源库打不开必须拒拷贝（fail-closed）");
    let msg = err.to_string();
    assert!(
        msg.contains("拒绝拷贝"),
        "错误消息应可操作（指明已拒绝拷贝且原库未动），got: {msg}"
    );
    assert!(!dst.exists(), "dst 主库不得产生");
    assert!(!wal_sidecar(&dst, "wal").exists(), "dst -wal 不得产生");
    assert!(!wal_sidecar(&dst, "shm").exists(), "dst -shm 不得产生");
    fs::remove_dir_all(&dir).ok();
}

/// 损坏源库（非 SQLite 文件：open 惰性成功、checkpoint 报 NOTADB）→ fail-closed：
/// 与 open 失败同治，Err 拒拷贝、dst 三件套一概不产生（BUSY 之外的错误不裸拷）。
#[test]
fn copy_legacy_db_corrupt_source_fails_closed() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-corrupt-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let legacy = dir.join("wmessage.db");
    // >100 字节垃圾：SQLite 头校验必败（NOTADB）；零长度才会被当空库
    std::fs::write(&legacy, [b'x'; 512]).unwrap();
    let dst = dir.join("new").join("wmessage.db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    let result = copy_legacy_db(&legacy, &dst);

    let err = result.expect_err("损坏源库必须拒拷贝（fail-closed）");
    assert!(
        err.to_string().contains("拒绝拷贝"),
        "错误消息应可操作，got: {err}"
    );
    assert!(!dst.exists(), "dst 主库不得产生");
    assert!(!wal_sidecar(&dst, "wal").exists(), "dst -wal 不得产生");
    assert!(!wal_sidecar(&dst, "shm").exists(), "dst -shm 不得产生");
    fs::remove_dir_all(&dir).ok();
}

/// 成功路径：3 文件落地（main + -wal + -shm）+ warns 返回 Ok(Vec)。
/// happy_path_no_warns 已覆盖零 warn 路径；本测试确认三件套全到位 + Result 结构。
#[test]
fn copy_legacy_db_writes_three_files_returns_warns() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-3files-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let legacy = dir.join("wmessage.db");
    let keeper = rusqlite::Connection::open(&legacy).unwrap();
    keeper
        .execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE t (id TEXT); INSERT INTO t VALUES ('x');",
        )
        .unwrap();
    assert!(
        wal_sidecar(&legacy, "wal").exists(),
        "setup: legacy -wal 应存在"
    );
    assert!(
        wal_sidecar(&legacy, "shm").exists(),
        "setup: legacy -shm 应存在"
    );
    let dst = dir.join("new").join("wmessage.db");
    fs::create_dir_all(dst.parent().unwrap()).unwrap();

    let warns = copy_legacy_db(&legacy, &dst).unwrap();
    drop(keeper);

    assert!(dst.exists(), "主库必须落地");
    assert!(wal_sidecar(&dst, "wal").exists(), "-wal 边车必须落地");
    assert!(wal_sidecar(&dst, "shm").exists(), "-shm 边车必须落地");
    // BUSY warn 是否出现取决于 sqlite checkpoint 对并发读游标的行为（实测本环境
    // 可为空=checkpoint 成功）——契约 = Ok + 三件套到位（上方断言），warn 值域只有
    // 两种合法形态；任何其他 warn 内容都算回退。禁改回恒真或断言 warn 必现。
    assert!(
        warns.is_empty() || warns.iter().any(|w| w.contains("checkpoint")),
        "warn 只能为空或 checkpoint 容错（BUSY）；got: {warns:?}"
    );
    fs::remove_dir_all(&dir).ok();
}

/// 上次拷贝在边车 rename（2a/2b）后、主库提交（2c）前崩溃会在目标路径留下
/// 孤儿 -wal/-shm；新主库落位前必须清场，否则新库恰与无关 WAL 配对（回放即
/// 损坏）。本测试预置孤儿边车，走完拷贝断言：无残留边车、主库可用且数据完整。
#[test]
fn copy_legacy_db_clears_orphan_sidecars_at_target() {
    let dir = std::env::temp_dir().join(format!("wm-legacy-orphan-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    // 老 journal 模式库：checkpoint 干净，拷贝本身不产生边车
    let legacy = dir.join("wmessage.db");
    {
        let c = rusqlite::Connection::open(&legacy).unwrap();
        c.execute_batch("CREATE TABLE t (id TEXT); INSERT INTO t VALUES ('x');")
            .unwrap();
    }
    let dst = dir.join("wmessage-copy.db");
    // 预置孤儿边车（主库不在、边车在 = 2a/2b 后崩溃的现场）
    fs::write(wal_sidecar(&dst, "wal"), b"stale-wal").unwrap();
    fs::write(wal_sidecar(&dst, "shm"), b"stale-shm").unwrap();

    let warns = copy_legacy_db(&legacy, &dst).unwrap();

    assert!(warns.is_empty(), "干净老库不应产生 warn；got: {warns:?}");
    assert!(dst.exists(), "主库必须落地");
    assert!(
        !wal_sidecar(&dst, "wal").exists(),
        "孤儿 -wal 必须在主库落位前清场"
    );
    assert!(
        !wal_sidecar(&dst, "shm").exists(),
        "孤儿 -shm 必须在主库落位前清场"
    );
    let c = rusqlite::Connection::open(&dst).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "清场后主库可用且数据完整");
    fs::remove_dir_all(&dir).ok();
}

// ── migrate_data_json 触发条件：json 含库缺失 id 即补回 ──

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
    let before_ms = chrono::Utc::now().timestamp_millis();
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

// ── APW-02a: copy_dir_recursive 防护 ──

/// dst 已存在 → 返 Err（保守语义，不改现有“dst 不存在”行为）
#[test]
fn copy_dir_recursive_rejects_existing_dst() {
    let dir = std::env::temp_dir().join(format!("wm-copy-dst-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src");
    let dst = dir.join("dst");
    fs::create_dir_all(&src).unwrap();
    fs::create_dir_all(&dst).unwrap();
    fs::write(src.join("file.txt"), b"x").unwrap();
    let result = copy_dir_recursive(&src, &dst);
    assert!(result.is_err(), "dst 已存在应返 Err，实际 {result:?}");
    assert!(dst.exists(), "dst 路径应原样未变（无半截内容）");
    fs::remove_dir_all(&dir).ok();
}

/// src 是 symlink → 返 Err（read_dir 跟读目标 = move 语义失控）
///
/// symlink 的创建是 unix-only API——Windows 上没有对应物，这个用例不适用。
#[cfg(unix)]
#[test]
fn copy_dir_recursive_rejects_symlink_src() {
    let dir = std::env::temp_dir().join(format!("wm-copy-sym-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let real_src = dir.join("real_src");
    fs::create_dir_all(&real_src).unwrap();
    fs::write(real_src.join("file.txt"), b"x").unwrap();
    let sym_src = dir.join("sym_src");
    std::os::unix::fs::symlink(&real_src, &sym_src).unwrap();
    let dst = dir.join("dst");
    let result = copy_dir_recursive(&sym_src, &dst);
    assert!(result.is_err(), "src 是 symlink 应返 Err，实际 {result:?}");
    fs::remove_dir_all(&dir).ok();
}

/// files 列读写回环：upsert 多文件 → load_all 原样读回
#[test]
fn files_roundtrip_via_upsert_load() {
    let _g = super::lock_db_write(); // C3-1 契约：upsert_tasks 调用方须持锁
    let (dir, conn) = setup_tasks_db();
    let mut t = mk_task("t1", "多文件");
    t.files = Some(vec![
        TaskFile {
            path: "/a/1.pdf".into(),
            is_dir: false,
        },
        TaskFile {
            path: "/a/2.docx".into(),
            is_dir: false,
        },
    ]);
    upsert_tasks(&conn, &[t]).unwrap();
    let loaded = &load_all(&conn).unwrap()[0];
    assert_eq!(loaded.files.as_deref().unwrap().len(), 2);
    assert_eq!(loaded.files.as_deref().unwrap()[1].path, "/a/2.docx");
    fs::remove_dir_all(&dir).ok();
}

/// effective_files：返回 files 绑定；未绑定为空
#[test]
fn effective_files_returns_bound_files() {
    let mut t = mk_task("t1", "x");
    assert!(t.effective_files().is_empty(), "都没绑 → 空");
    t.files = Some(vec![TaskFile {
        path: "/tmp/new.pdf".into(),
        is_dir: false,
    }]);
    assert_eq!(
        t.effective_files(),
        vec![TaskFile {
            path: "/tmp/new.pdf".into(),
            is_dir: false
        }],
        "files 非空 → 原样返回"
    );
}

/// resolve_task_files：去重保序、空路径丢弃、超 MAX_TASK_FILES 截断、
/// 目录经 fs::metadata 判 isDir、不存在路径按文件处理
#[test]
fn resolve_task_files_dedup_cap_and_isdir() {
    let dir = std::env::temp_dir().join(format!("wm-resolve-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let dir_str = dir.to_string_lossy().to_string();

    // 目录判定 + 去重保序 + 不存在路径按文件
    let out = resolve_task_files(vec![
        dir_str.clone(),
        "/no/such/file.txt".into(),
        dir_str.clone(), // 重复
        "  ".into(),     // 空白丢弃
    ]);
    assert_eq!(out.len(), 2, "去重 + 空白丢弃");
    assert!(out[0].is_dir, "真实目录 isDir=true");
    assert_eq!(out[1].path, "/no/such/file.txt");
    assert!(!out[1].is_dir, "不存在路径按文件处理");

    // 上限截断：12 个 → 10 个，保序
    let many: Vec<String> = (0..12).map(|i| format!("/f/{i}.txt")).collect();
    let capped = resolve_task_files(many);
    assert_eq!(capped.len(), MAX_TASK_FILES, "超 10 截断");
    assert_eq!(capped[9].path, "/f/9.txt", "保序截断前 10 个");
    fs::remove_dir_all(&dir).ok();
}

/// upsert_files_to_task 统一绑定内核：文件追加、文件夹槽替换、小写路径去重、
/// 10 封顶、空追加无操作、只增不减
#[test]
fn upsert_files_to_task_additive_with_folder_slot() {
    let tf = |path: &str, is_dir: bool| TaskFile {
        path: path.to_string(),
        is_dir,
    };
    let mut t = mk_task("t1", "x");

    // 空追加 = 无操作（files 仍 None，只增不减）
    let (applied, overflow) = upsert_files_to_task(&mut t, vec![]);
    assert_eq!((applied, overflow), (0, 0));
    assert!(t.files.is_none(), "空追加不得写回空绑定");

    // 文件追加 + 小写路径去重（同路径不同大小写 = 同一条目）
    let (applied, overflow) = upsert_files_to_task(
        &mut t,
        vec![tf("/d/报.docx", false), tf("/D/报.DOCX", false)],
    );
    assert_eq!((applied, overflow), (1, 0), "小写归一去重");
    assert_eq!(t.files.as_deref().unwrap().len(), 1);

    // 文件夹槽：首个文件夹入列
    let (applied, _) = upsert_files_to_task(&mut t, vec![tf("/d/归档", true)]);
    assert_eq!(applied, 1);
    // 不同路径文件夹 → 替换占原位，文件条目与顺序不动
    let (applied, _) = upsert_files_to_task(&mut t, vec![tf("/e/其他", true)]);
    assert_eq!(applied, 1, "文件夹替换计为生效");
    let files = t.files.as_deref().unwrap();
    assert_eq!(files.len(), 2, "文件夹替换不增长列表");
    assert_eq!(files[1].path, "/e/其他", "新文件夹占原槽位");
    assert_eq!(files[0].path, "/d/报.docx", "文件条目不动");
    // 同路径文件夹重复绑 → 无操作
    let (applied, overflow) = upsert_files_to_task(&mut t, vec![tf("/E/其他", true)]);
    assert_eq!((applied, overflow), (0, 0));

    // 10 封顶：文件溢出计数；满员时文件夹替换仍生效
    let mut full = mk_task("t2", "y");
    let fill: Vec<TaskFile> = (0..9)
        .map(|i| tf(&format!("/f/{i}.txt", i = i), false))
        .collect();
    upsert_files_to_task(&mut full, fill);
    upsert_files_to_task(&mut full, vec![tf("/f/-dir", true)]);
    let (applied, overflow) = upsert_files_to_task(&mut full, vec![tf("/f/new.txt", false)]);
    assert_eq!((applied, overflow), (0, 1), "满员文件溢出");
    let (applied, overflow) = upsert_files_to_task(&mut full, vec![tf("/f/替换目录", true)]);
    assert_eq!((applied, overflow), (1, 0), "满员文件夹替换仍生效");
    assert_eq!(full.files.as_deref().unwrap().len(), 10, "替换不增长");
    assert_eq!(
        full.files
            .as_deref()
            .unwrap()
            .iter()
            .find(|f| f.is_dir)
            .unwrap()
            .path,
        "/f/替换目录"
    );
}

/// 旧版本工作区链接存 label/target 字段，新版本 displayName/targetUri，
/// serde alias 保证旧数据无缝读取。
#[test]
fn workspace_link_legacy_fields_compat() {
    let old = r#"[{"id":"l1","label":"Desktop","target":"/Users/x/Desktop","kind":"folder"}]"#;
    let links: Vec<WorkspaceLink> = serde_json::from_str(old).unwrap();
    assert_eq!(links[0].display_name, "Desktop");
    assert_eq!(links[0].target_uri, "/Users/x/Desktop");
    assert_eq!(links[0].kind, "folder");

    let new = r#"[{"id":"l2","displayName":"别名","targetUri":"https://x.com","kind":"url"}]"#;
    let links2: Vec<WorkspaceLink> = serde_json::from_str(new).unwrap();
    assert_eq!(links2[0].display_name, "别名");
    assert_eq!(links2[0].target_uri, "https://x.com");

    // 序列化输出新字段名（camelCase）
    let out = serde_json::to_string(&links2).unwrap();
    assert!(out.contains("displayName") && out.contains("targetUri"));
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

/// 追加写不能吃掉既有历史：后端主动发起的汇报轮没有前端快照可覆写，
/// 只能自己 INSERT；一旦退化成覆盖写，前端那份并发快照就会把它抹掉。
#[test]
fn bot_history_append_keeps_existing_rows_and_order() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    let mk = |c: &str| BotMsgRow {
        role: "user".into(),
        content: c.into(),
        refs_json: None,
        thinking: None,
        tools_json: None,
        turn_status: None,
    };
    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &[mk("第一条"), mk("第二条")]).unwrap();
    tx.commit().unwrap();

    bot_history_append_inner(
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
    )
    .unwrap();

    let mut stmt = conn
        .prepare("SELECT role, content FROM bot_messages WHERE session_id = 's1' ORDER BY id")
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(std::result::Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 3, "追加不能删除既有行");
    assert_eq!(rows[0].1, "第一条");
    assert_eq!(rows[2].1, "汇报轮回复");
    assert_eq!(rows[2].0, "assistant");
    let updated: i64 = conn
        .query_row(
            "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(updated >= 1000, "会话活跃时间要跟着动");
    drop(rows);
    drop(stmt);
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// 核心回归：bot_history_save 内层报错时，DELETE 必须被事务回滚，
/// 原会话聊天记录不得丢失（崩溃/强杀落在 INSERT 中间的场景）。
#[test]
fn bot_history_save_rolls_back_on_insert_failure() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    for i in 0..3 {
        conn.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', 'user', ?1, 1000)",
            [format!("msg-{}", i)],
        )
        .unwrap();
    }
    let count_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count_before, 3, "setup: 应有 3 条原消息");

    // 模拟 wrapper 包裹模式：tx + inner + commit。inner 期间遇 NOT NULL 违约。
    let tx_result: Result<(), String> = (|| {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM bot_messages WHERE session_id = 's1'", [])
            .map_err(|e| e.to_string())?;
        // 强制失败：role 为 NULL → NOT NULL 约束违反
        tx.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', NULL, 'x', 1000)",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })();
    assert!(tx_result.is_err(), "tx 必须覆盖推制失败的 insert");

    // 验证：事务被 Drop → 自动 rollback，原 3 条消息仍存在
    let count_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count_after, 3,
        "事务未 commit 必须回滚 DELETE，原 3 条消息应保留"
    );

    fs::remove_dir_all(&dir).ok();
}

/// 同样覆盖：bot_history_save_inner 成功路径在事务中被提交。
#[test]
fn bot_history_save_inner_commits_in_tx() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 5000, 0)",
        [],
    )
    .unwrap();

    let new_msgs = vec![
        BotMsgRow {
            role: "user".into(),
            content: "hi".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
        BotMsgRow {
            role: "assistant".into(),
            content: "hello".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
    ];

    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &new_msgs).unwrap();
    tx.commit().unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "提交后应有 2 条新消息");

    let updated_at: i64 = conn
        .query_row(
            "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(updated_at >= 5000, "updated_at 应被 update 为 now >= 5000");

    fs::remove_dir_all(&dir).ok();
}

/// 覆写行序不变量：全量覆写后 load 行序 == 写入序，批内 created_at 相等
/// （覆写原子时刻），id 单调递增——行序唯一由自增 id 恢复，与 created_at 无关。
#[test]
fn bot_history_overwrite_restores_order_and_batch_created_at() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 5000, 0)",
        [],
    )
    .unwrap();
    let mk = |content: &str| BotMsgRow {
        role: "user".into(),
        content: content.into(),
        refs_json: None,
        thinking: None,
        tools_json: None,
        turn_status: None,
    };

    // 第一批 3 条 → 整批覆写为 2 条（内容可辨识顺序）
    let first = vec![mk("a"), mk("b"), mk("c")];
    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &first).unwrap();
    tx.commit().unwrap();
    let second = vec![mk("d"), mk("e")];
    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &second).unwrap();
    tx.commit().unwrap();

    // load（生产查询同款 ORDER BY id）行序 == 写入序
    let mut stmt = conn
        .prepare("SELECT role, content, refs, thinking, tools FROM bot_messages WHERE session_id = 's1' ORDER BY id")
        .unwrap();
    let loaded: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        loaded,
        vec!["d".to_string(), "e".to_string()],
        "覆写后行序应恢复写入序"
    );
    drop(stmt);

    // 批内 created_at 全等（覆写原子时刻）+ id 单调递增
    let rows: Vec<(i64, i64)> = conn
        .prepare("SELECT id, created_at FROM bot_messages WHERE session_id = 's1' ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2, "覆写后应只剩第二批 2 条");
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let cats: Vec<i64> = rows.iter().map(|(_, ca)| *ca).collect();
    assert!(ids[0] < ids[1], "id 应单调递增：{ids:?}");
    assert_eq!(
        cats[0], cats[1],
        "批内 created_at 应全等（覆写原子时刻）：{cats:?}"
    );

    fs::remove_dir_all(&dir).ok();
}

/// bot_session_delete 原子性：消息与会话同一事务，任一失败不留下半删状态。
/// 直调生产 bot_session_delete_inner（内联复刻命令体裸 SQL 会让生产改动测试不红）。
#[test]
fn bot_session_delete_is_atomic() {
    let (dir, conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', 'user', 'm1', 1000)",
        [],
    )
    .unwrap();

    let mut conn = conn;
    bot_session_delete_inner(&mut conn, "s1").unwrap();

    let sess_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_sessions WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let msg_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sess_count, 0, "提交后会话应被删除");
    assert_eq!(msg_count, 0, "提交后消息应被删除");

    // 子 agent 行级联：终态随会话走，在途行留着（runner 还要靠它收尾）
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_done', 'succeeded', 'research', 's1', 'o1', 't1', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_live', 'running', 'research', 's1', 'o2', 't2', 1000)",
        [],
    )
    .unwrap();
    bot_session_delete_inner(&mut conn, "s1").unwrap();
    let done_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM subagents WHERE id = 'sa_done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let live_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM subagents WHERE id = 'sa_live'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done_left, 0, "会话删除后终态子 agent 行不应残留");
    assert_eq!(live_left, 1, "在途子 agent 行不能删——runner 还要靠它收尾");
    let live_status: String = conn
        .query_row(
            "SELECT status FROM subagents WHERE id = 'sa_live'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        live_status, "cancelled",
        "在途子 agent 必须随父会话一起取消——父会话没了，它的产物没人收、\
         它的行没人看得见，再跑下去纯烧预算"
    );

    // 原子性反向验证：第二条 DELETE（会话）注入失败 → 第一条（消息）也必须回滚
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s2', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s2', 'user', 'm2', 1000)",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_sess_del BEFORE DELETE ON bot_sessions
         WHEN OLD.id = 's2' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
    )
    .unwrap();
    assert!(
        bot_session_delete_inner(&mut conn, "s2").is_err(),
        "trigger 注入失败必须返回 Err"
    );
    let sess_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_sessions WHERE id = 's2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let msg_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sess_count, 1, "回滚后会话必须还在");
    assert_eq!(msg_count, 1, "回滚后消息必须还在（不留半删状态）");

    fs::remove_dir_all(&dir).ok();
}

/// 删会话时在途子 agent 一并取消：排队中与执行中各一条，外加一条以
/// `session_id` 挂在这个会话下的行——三个入口都得收干净。
///
/// 父会话没了 → 产物没人收、行没人看得见、还照烧完整份预算。
#[test]
fn deleting_a_session_cancels_its_live_subagents() {
    let (dir, conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_q', 'queued', 'research', 's1', 'o1', 't1', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_r', 'running', 'research', 's1', 'o2', 't2', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, session_id, objective, trace_id, created_at)
         VALUES ('sa_own', 'running', 'research', 's1', 'o3', 't3', 1000)",
        [],
    )
    .unwrap();

    let mut conn = conn;
    bot_session_delete_inner(&mut conn, "s1").unwrap();

    for id in ["sa_q", "sa_r", "sa_own"] {
        let status: String = conn
            .query_row(
                &format!("SELECT status FROM subagents WHERE id = '{id}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "cancelled", "{id} 应随父会话一起取消");
        let error: String = conn
            .query_row(
                &format!("SELECT error FROM subagents WHERE id = '{id}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            error.contains("父会话已删除"),
            "{id} 应记明取消原因：{error}"
        );
    }

    fs::remove_dir_all(&dir).ok();
}

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
