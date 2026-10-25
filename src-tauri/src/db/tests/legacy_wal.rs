//! legacy 库拷贝：WAL 边车一并拷贝、错误不吞、孤儿边车清场。

use super::*;

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
