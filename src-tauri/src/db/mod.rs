//!
//! 原 2746 行 db.rs 按 SRP 拆为 8 个子模块（paths / migrations / tasks /
//! workspace / bot_sessions / bot_history / skill_out）。本文件保留为 facade：
//! `pub use crate::db::{每子模块}::*;` 重导出所有 pub 项，`crate::db::X` 路径兼容。
//!
//! **安全纪律**：SQL / schema / 事务边界 / `busy_timeout = 2s` 一律不动；
//! 测试模块（4 段共 1218 行）保留在本文件，`super::X` 经 pub use 解析为子模块项。

pub use crate::db::bot_history::*;
pub use crate::db::bot_sessions::*;
pub use crate::db::migrations::*;
pub use crate::db::paths::*;
pub use crate::db::skill_out::*;
pub use crate::db::tasks::*;
pub use crate::db::workspace::*;
use std::time::Duration;
use tauri::Manager;

use serde::{Deserialize, Serialize};

use crate::error::CommandError;

pub mod bot_history;
pub mod bot_sessions;
pub mod migrations;
pub mod paths;
pub mod skill_out;
pub mod tasks;
pub mod workspace;

// ──────────────────── RMW helpers ────────────────────

/// 写库前的标准接线：把当前 `updated_at` 作为下次 RMW 读快照基线，
/// 然后把 `updated_at` 戳成"现在"。任何写库前的 mutate 路径都必须调。
///
/// 行为细节：
/// - `task.updated_at` 为 None 时（老行 / 新建），基线降级为
///   `BASELINE_NULL_ROW`（行存在性哨兵，防 NULL 老行被静默覆盖）
/// - 一律刷 `updated_at` 为当前时间戳
///
/// 用法：`db::prepare_for_upsert(&mut task)` 后接 `db::db_upsert(...)`。
pub fn prepare_for_upsert(t: &mut Task) {
    t.expected_updated_at = Some(t.updated_at.unwrap_or(BASELINE_NULL_ROW));
    t.updated_at = Some(chrono::Utc::now().timestamp_millis());
}

/// 打开数据库（泛型 Runtime：mock runtime 测试可直调）
///
/// 连接级 PRAGMA（每次建连都要设——PRAGMA 是 **per-connection** 的，漏一处就静默跑默认档）：
/// - `journal_mode=WAL`：读写不互相阻塞（本应用单写者 + 多读）
/// - `synchronous=NORMAL`：WAL 下的推荐档——掉电最多丢最近若干已提交事务
/// - `foreign_keys=ON`：SQLite 默认 **OFF**，显式打开
///
/// 回归锁：`db::tests::conn_pragmas_are_applied`
pub fn open_db<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<rusqlite::Connection, String> {
    let dir = paths::db_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let db_path = dir.join("wmessage.db");
    if !db_path.exists() {
        if let Ok(legacy_dir) = app.path().app_data_dir() {
            let legacy_db = legacy_dir.join("wmessage.db");
            if legacy_db.exists() && legacy_db != db_path {
                match paths::copy_legacy_db(&legacy_db, &db_path) {
                    Ok(warns) => {
                        for w in warns {
                            crate::audit::write_event(
                                app,
                                crate::audit::AuditLevel::Warn,
                                "legacy_db_copy",
                                &[("warn", w)],
                            );
                        }
                    }
                    Err(e) => {
                        return Err(e.to_string());
                    }
                }
            }
        }
    }
    let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    // busy_timeout **有意保留 2s，不调 5000ms**：进程内写者已由 DB_WRITE_LOCK 串行化
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    migrations::apply_conn_pragmas(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
           id           TEXT PRIMARY KEY,
           title        TEXT NOT NULL,
           due          TEXT,
           note         TEXT,
           tags         TEXT,
           file_path    TEXT,
           file_is_dir  INTEGER,
           col          TEXT NOT NULL,
           subtasks     TEXT,
           completed_at INTEGER,
           archived     INTEGER,
           deleted_at   INTEGER,
           collapsed    INTEGER,
           ord          REAL,
           updated_at   INTEGER,
           schedule     TEXT,
           sched_last   INTEGER,
           bot_assigned INTEGER
         );
         CREATE TABLE IF NOT EXISTS workspace_items (
           id         TEXT PRIMARY KEY,
           title      TEXT NOT NULL,
           collapsed  INTEGER,
           links      TEXT NOT NULL,
           ord        REAL,
           updated_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS migration_journal (
           id          INTEGER PRIMARY KEY AUTOINCREMENT,
           op          TEXT    NOT NULL,
           src         TEXT    NOT NULL,
           dst         TEXT,
           task_id     TEXT    NOT NULL,
           state       TEXT    NOT NULL,
           created_at  INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_messages (
           id         INTEGER PRIMARY KEY AUTOINCREMENT,
           role       TEXT NOT NULL,
           content    TEXT NOT NULL,
           refs       TEXT,
           session_id TEXT,
           thinking   TEXT,
           tools      TEXT,
           created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_sessions (
           id         TEXT PRIMARY KEY,
           title      TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS skill_outcomes (
           skill_name         TEXT PRIMARY KEY,
           kind               TEXT NOT NULL,
           reason             TEXT,
           completed_summary  TEXT,
           rollback_attempted INTEGER,
           last_at_ms         INTEGER NOT NULL
         );",
    )
    .map_err(|e| e.to_string())?;
    // 迁移：定时任务卡
    for (col, ty) in [("schedule", "TEXT"), ("sched_last", "INTEGER")] {
        let has: bool = conn
            .prepare("PRAGMA table_info(tasks)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == col))
            })
            .unwrap_or(false);
        if !has {
            conn.execute(&format!("ALTER TABLE tasks ADD COLUMN {col} {ty}"), [])
                .map_err(|e| e.to_string())?;
        }
    }
    let has_ba: bool = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).any(|n| n == "bot_assigned"))
        })
        .unwrap_or(false);
    if !has_ba {
        conn.execute("ALTER TABLE tasks ADD COLUMN bot_assigned INTEGER", [])
            .map_err(|e| e.to_string())?;
    }
    let has_ord: bool = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).any(|n| n == "ord"))
        })
        .unwrap_or(false);
    if !has_ord {
        conn.execute("ALTER TABLE tasks ADD COLUMN ord REAL", [])
            .map_err(|e| e.to_string())?;
    }
    let has_ua: bool = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).any(|n| n == "updated_at"))
        })
        .unwrap_or(false);
    if !has_ua {
        conn.execute("ALTER TABLE tasks ADD COLUMN updated_at INTEGER", [])
            .map_err(|e| e.to_string())?;
    }
    migrations::ensure_files_column(&conn)?;
    let migrated = migrations::migrate_legacy_file_bindings(&mut conn)?;
    if migrated > 0 {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "task_files_migration",
            &[("migrated", migrated.to_string())],
        );
    }
    let has_sid: bool = conn
        .prepare("PRAGMA table_info(bot_messages)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).any(|n| n == "session_id"))
        })
        .unwrap_or(false);
    if !has_sid {
        conn.execute("ALTER TABLE bot_messages ADD COLUMN session_id TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    for col in ["thinking", "tools"] {
        let has: bool = conn
            .prepare("PRAGMA table_info(bot_messages)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == col))
            })
            .unwrap_or(false);
        if !has {
            conn.execute(
                &format!("ALTER TABLE bot_messages ADD COLUMN {col} TEXT"),
                [],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    {
        let orphan: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM bot_messages WHERE session_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if orphan > 0 {
            let now = chrono::Utc::now().timestamp_millis();
            conn.execute(
                "INSERT OR IGNORE INTO bot_sessions (id, title, created_at, updated_at) VALUES ('default', '默认对话', ?1, ?1)",
                [now],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE bot_messages SET session_id='default' WHERE session_id IS NULL",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    static BOT_ASSIGNED_RESET_DONE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    migrations::reset_bot_assigned_with(&BOT_ASSIGNED_RESET_DONE, || {
        conn.execute(
            "UPDATE tasks SET bot_assigned = 0 WHERE bot_assigned = 1",
            [],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })?;
    Ok(conn)
}

/// 写操作全局锁：主窗口（db_upsert/db_delete）与本地 API 线程共享同一把锁
pub static DB_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// 当前线程是否持有 DB_WRITE_LOCK（供契约断言用）。
    /// `std::sync::Mutex` **线程无关**：`try_lock` 只能证明「锁被某线程持有」，不能证明
    /// 「被当前线程持有」（且 poisoned 也返回 Err）→ 故用线程本地标记精确判定（OCR C3-1）。
    static HOLDING_DB_WRITE: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// `DB_WRITE_LOCK` 的守卫：取锁时置位线程本地标记，Drop 时**无条件**清位
/// （Drop 同时覆盖正常释放与 unwind，故不写手动清位——OCR C3-1）。
pub struct DbWriteGuard {
    _g: std::sync::MutexGuard<'static, ()>,
}

impl Drop for DbWriteGuard {
    fn drop(&mut self) {
        HOLDING_DB_WRITE.with(|f| f.set(false));
    }
}

/// 取 `DB_WRITE_LOCK` 并标记「本线程持锁」。poison 走仓库既有 `[mutex_poisoned]` 约定
/// （`into_inner` 后照样置位，不让 poisoned 断掉标记语义；OCR C3-1）。
pub fn lock_db_write() -> DbWriteGuard {
    let g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] db::DB_WRITE_LOCK: {e:?}");
        e.into_inner()
    });
    HOLDING_DB_WRITE.with(|f| f.set(true));
    DbWriteGuard { _g: g }
}

/// 当前线程是否持有 `DB_WRITE_LOCK`（供 `upsert_tasks` 的契约断言用）。
pub fn holding_db_write() -> bool {
    HOLDING_DB_WRITE.with(|f| f.get())
}

// 抑制 unused warnings
#[allow(dead_code)]
fn _unused(_e: CommandError) {}
#[cfg(test)]
mod tests {
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
        assert!(
            warns.is_empty()
                || warns
                    .iter()
                    .any(|w| w.contains("checkpoint") || w.contains("BUSY"))
        );
        fs::remove_dir_all(&dir).ok();
    }

    // ── migrate_data_json 触发条件：json 含库缺失 id 即补回 ──

    fn setup_tasks_db() -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("wm-mig-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, due TEXT, note TEXT,
               tags TEXT, file_path TEXT, file_is_dir INTEGER, col TEXT NOT NULL,
               subtasks TEXT, completed_at INTEGER, archived INTEGER,
               deleted_at INTEGER, collapsed INTEGER, ord REAL, updated_at INTEGER,
               schedule TEXT, sched_last INTEGER, bot_assigned INTEGER, files TEXT
             );",
        )
        .unwrap();
        (dir, conn)
    }

    fn mk_task(id: &str, title: &str) -> Task {
        Task {
            id: id.into(),
            title: title.into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: crate::db::TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: Some(1),
            schedule: None,
            sched_last: None,
            bot_assigned: None,
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

    // ── 多文件绑定：files 列迁移 + 老数据回填 + 上限 ──

    /// 老 schema（无 files 列）建库：模拟 2026-08-19 前的真实老库
    fn setup_legacy_tasks_db() -> (std::path::PathBuf, rusqlite::Connection) {
        let dir = std::env::temp_dir().join(format!("wm-files-mig-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, due TEXT, note TEXT,
               tags TEXT, file_path TEXT, file_is_dir INTEGER, col TEXT NOT NULL,
               subtasks TEXT, completed_at INTEGER, archived INTEGER,
               deleted_at INTEGER, collapsed INTEGER, ord REAL, updated_at INTEGER,
               schedule TEXT, sched_last INTEGER, bot_assigned INTEGER
             );",
        )
        .unwrap();
        (dir, conn)
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

    /// 老数据 → 新 schema 完整链路：ALTER 补 files 列 + file_path 回填 files + 读回解析。
    /// 覆盖：文件绑定、文件夹绑定（is_dir=1）、未绑定不动、幂等重跑不重复迁移。
    #[test]
    fn legacy_file_binding_migrates_to_files_column() {
        let (dir, mut conn) = setup_legacy_tasks_db();
        conn.execute_batch(
            "INSERT INTO tasks (id, title, file_path, file_is_dir, col) VALUES
               ('t-file', '绑文件', '/tmp/a.pdf', 0, 'todo'),
               ('t-dir',  '绑文件夹', '/tmp/dir', 1, 'todo'),
               ('t-none', '没绑', NULL, NULL, 'todo');",
        )
        .unwrap();

        ensure_files_column(&conn).unwrap();
        assert_eq!(
            migrate_legacy_file_bindings(&mut conn).unwrap(),
            2,
            "两条老绑定应迁移"
        );

        let tasks = load_all(&conn).unwrap();
        let tf = tasks.iter().find(|t| t.id == "t-file").unwrap();
        assert_eq!(
            tf.files.as_deref(),
            Some(
                vec![TaskFile {
                    path: "/tmp/a.pdf".into(),
                    is_dir: false
                }]
                .as_slice()
            ),
            "文件绑定应迁进 files（isDir=false）"
        );
        let td = tasks.iter().find(|t| t.id == "t-dir").unwrap();
        assert_eq!(
            td.files.as_deref(),
            Some(
                vec![TaskFile {
                    path: "/tmp/dir".into(),
                    is_dir: true
                }]
                .as_slice()
            ),
            "文件夹绑定应迁进 files（isDir=true）"
        );
        let tn = tasks.iter().find(|t| t.id == "t-none").unwrap();
        assert!(tn.files.is_none(), "未绑定任务不得产生 files");

        // 老列保留（迁移过渡期旧版本仍可读 file_path/file_is_dir）
        let fp: Option<String> = conn
            .query_row("SELECT file_path FROM tasks WHERE id='t-file'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(fp.as_deref(), Some("/tmp/a.pdf"), "老列 file_path 保留不清");

        // 幂等：再跑一次迁移条数为 0，files 不变
        assert_eq!(
            migrate_legacy_file_bindings(&mut conn).unwrap(),
            0,
            "重跑不得重复迁移"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// 已有 files 的任务不被老列回填覆盖（files 非空 → 跳过）
    #[test]
    fn migration_skips_tasks_with_existing_files() {
        let (dir, mut conn) = setup_legacy_tasks_db();
        conn.execute_batch(
            "INSERT INTO tasks (id, title, file_path, file_is_dir, col) VALUES
               ('t1', '已迁移过', '/tmp/old.txt', 0, 'todo');",
        )
        .unwrap();
        ensure_files_column(&conn).unwrap();
        // 模拟已迁移/新写的数据：files 已有值
        conn.execute(
            "UPDATE tasks SET files = '[{\"path\":\"/tmp/new.txt\",\"isDir\":false}]' WHERE id='t1'",
            [],
        )
        .unwrap();
        assert_eq!(migrate_legacy_file_bindings(&mut conn).unwrap(), 0);
        let t = &load_all(&conn).unwrap()[0];
        assert_eq!(
            t.files.as_deref().unwrap()[0].path,
            "/tmp/new.txt",
            "已有 files 不得被老 file_path 覆盖"
        );
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

    /// effective_files 兜底：files 空时回退旧 file_path/file_is_dir（迁移窗口内读旧数据）
    #[test]
    fn effective_files_falls_back_to_legacy_fields() {
        let mut t = mk_task("t1", "x");
        assert!(t.effective_files().is_empty(), "都没绑 → 空");
        t.file_path = Some("/tmp/legacy.pdf".into());
        t.file_is_dir = Some(true);
        assert_eq!(
            t.effective_files(),
            vec![TaskFile {
                path: "/tmp/legacy.pdf".into(),
                is_dir: true
            }],
            "files 空 → 回退旧字段"
        );
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
            "files 非空 → 优先 files"
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
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE bot_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                refs TEXT,
                thinking TEXT,
                tools TEXT,
                created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        (dir, conn)
    }

    /// 核心回归：bot_history_save 内层报错时，DELETE 必须被事务回滚，
    /// 原会话聊天记录不得丢失（崩溃/强杀落在 INSERT 中间的场景）。
    #[test]
    fn bot_history_save_rolls_back_on_insert_failure() {
        let (dir, mut conn) = setup_bhs_db();
        conn.execute(
            "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000)",
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
            "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 5000)",
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
            },
            BotMsgRow {
                role: "assistant".into(),
                content: "hello".into(),
                refs_json: None,
                thinking: None,
                tools_json: None,
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

    /// bot_session_delete 原子性：消息与会话同一事务，任一失败不留下半删状态。
    /// 直调生产 bot_session_delete_inner（内联复刻命令体裸 SQL 会让生产改动测试不红）。
    #[test]
    fn bot_session_delete_is_atomic() {
        let (dir, conn) = setup_bhs_db();
        conn.execute(
            "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000)",
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

        // 原子性反向验证：第二条 DELETE（会话）注入失败 → 第一条（消息）也必须回滚
        conn.execute(
            "INSERT INTO bot_sessions VALUES ('s2', 'T', 1000, 1000)",
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

    /// 模拟 2026-08-14 前的任务行（updated_at IS NULL），验证 tasks_import
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
        // 老 NULL 行（2026-08-14 前 schema 遗留）：生产 upsert 写全列，NULL 行只能 SQL 直插
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
}

#[cfg(test)]
mod ws_tests {
    use super::*;
    use std::fs;

    /// 折叠状态往返：upsert collapsed=true → load 读回一致
    #[test]
    fn workspace_collapsed_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wm-ws-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
        )
        .unwrap();
        let item = WorkspaceItem {
            id: "w1".into(),
            title: "测试".into(),
            collapsed: Some(true),
            links: vec![WorkspaceLink {
                id: "l1".into(),
                display_name: "别名".into(),
                target_uri: "https://x.com".into(),
                kind: "url".into(),
            }],
            order: Some(1.0),
            updated_at: Some(123),
        };
        upsert_workspace(&mut conn, &[item]).unwrap();
        let got = load_workspace(&conn).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].collapsed, Some(true));
        assert_eq!(got[0].links[0].display_name, "别名");
        fs::remove_dir_all(&dir).ok();
    }

    /// workspace_* 命令的桥接层（spawn_blocking + join 错误映射）是无逻辑薄壳且依赖
    /// AppHandle 无法单测；这里直调生产 helper（upsert/load/delete）覆盖命令体真正
    /// 干活的部分（内联复刻桥接结构会让生产命令体改动测试不红，属弱断言）。
    #[test]
    fn workspace_commands_spawn_blocking_bridge() {
        let dir = std::env::temp_dir().join(format!("wm-ws-async-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
        )
        .unwrap();
        let item = WorkspaceItem {
            id: "w1".into(),
            title: "T".into(),
            collapsed: None,
            links: vec![],
            order: None,
            updated_at: Some(1),
        };

        // upsert → load 回读（生产 helper，同 workspace_upsert / workspace_load 命令体所调）
        upsert_workspace(&mut conn, &[item]).unwrap();
        let items = load_workspace(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "T");

        // delete（同 workspace_delete 命令体所调）
        delete_workspace(&mut conn, &["w1".to_string()]).unwrap();
        assert!(load_workspace(&conn).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    /// OCR C2b #2 簇B：写入侧 fail-closed —— upsert 遇非法 kind 返回可辨识错误
    /// InvalidWorkspaceLinkKind（带 kind / 来源 / 目标），不静默丢、不降级 url。
    /// 白名单内的 kind（url/file/folder）正常落库。
    #[test]
    fn upsert_rejects_invalid_link_kind() {
        let dir = std::env::temp_dir().join(format!("wm-ws-badkind-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
        )
        .unwrap();
        let mk = |id: &str, kind: &str| WorkspaceItem {
            id: id.into(),
            title: "T".into(),
            collapsed: None,
            links: vec![WorkspaceLink {
                id: "l1".into(),
                display_name: "x".into(),
                target_uri: "/tmp/x.app".into(),
                kind: kind.into(),
            }],
            order: None,
            updated_at: Some(1),
        };

        // 危险 kind + 未来 kind 全部拒（fail-closed）
        for bad in ["app", "command", "future_foo"] {
            match upsert_workspace(&mut conn, &[mk(&format!("w-{bad}"), bad)]).unwrap_err() {
                crate::error::CommandError::InvalidWorkspaceLinkKind { kind, source, link } => {
                    assert_eq!(kind, bad);
                    assert_eq!(source, "workspace_upsert");
                    assert_eq!(link, "/tmp/x.app");
                }
                other => panic!("应为 InvalidWorkspaceLinkKind，实得 {other:?}"),
            }
        }
        // 白名单内的 kind 正常落库
        for ok in ["url", "file", "folder"] {
            upsert_workspace(&mut conn, &[mk(&format!("ok-{ok}"), ok)]).unwrap();
        }
        assert_eq!(load_workspace(&conn).unwrap().len(), 3);

        // 混合批次（合法 + 非法同批）：整批拒绝且**零写入** —— 直接证明
        // validate_link_kinds 在事务之前跑（否则合法项会先落库）。
        let before = load_workspace(&conn).unwrap().len();
        let mixed = vec![mk("ok-mixed", "file"), mk("bad-mixed", "app")];
        assert!(matches!(
            upsert_workspace(&mut conn, &mixed),
            Err(crate::error::CommandError::InvalidWorkspaceLinkKind { .. })
        ));
        assert_eq!(
            load_workspace(&conn).unwrap().len(),
            before,
            "混合批次不得部分写入（fail-closed 必须在事务之前）"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// OCR C2b #2 簇B：import 路径同样 fail-closed（workspace_import_merge），
    /// 且拒绝时不产生任何写入。
    #[test]
    fn import_merge_rejects_invalid_link_kind() {
        let dir = std::env::temp_dir().join(format!("wm-ws-impbad-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
        )
        .unwrap();
        let item = WorkspaceItem {
            id: "w1".into(),
            title: "T".into(),
            collapsed: None,
            links: vec![WorkspaceLink {
                id: "l1".into(),
                display_name: "x".into(),
                target_uri: "/tmp/evil.command".into(),
                kind: "command".into(),
            }],
            order: None,
            updated_at: Some(1),
        };
        match workspace_import_merge(&mut conn, &[item]).unwrap_err() {
            crate::error::CommandError::InvalidWorkspaceLinkKind { kind, source, link } => {
                assert_eq!(kind, "command");
                assert_eq!(source, "workspace_import_merge");
                assert_eq!(link, "/tmp/evil.command");
            }
            other => panic!("应为 InvalidWorkspaceLinkKind，实得 {other:?}"),
        }
        // fail-closed：无任何写入
        assert!(load_workspace(&conn).unwrap().is_empty());

        // 混合批次（合法 + 非法）针对**已有行**：整批拒绝且不污染/删除既有数据。
        // 先种一行合法数据，再交混合批次，验证拒绝是全局的（在事务之前）。
        let seed = WorkspaceItem {
            id: "seed".into(),
            title: "seed".into(),
            collapsed: None,
            links: vec![WorkspaceLink {
                id: "l0".into(),
                display_name: "s".into(),
                target_uri: "/tmp/ok.txt".into(),
                kind: "file".into(),
            }],
            order: None,
            updated_at: Some(1),
        };
        workspace_import_merge(&mut conn, &[seed]).unwrap();
        assert_eq!(load_workspace(&conn).unwrap().len(), 1);
        let ok_item = WorkspaceItem {
            id: "w-ok".into(),
            title: "ok".into(),
            collapsed: None,
            links: vec![WorkspaceLink {
                id: "l1".into(),
                display_name: "x".into(),
                target_uri: "/tmp/ok.txt".into(),
                kind: "file".into(),
            }],
            order: None,
            updated_at: Some(2),
        };
        let bad_item = WorkspaceItem {
            id: "w-bad".into(),
            title: "bad".into(),
            collapsed: None,
            links: vec![WorkspaceLink {
                id: "l2".into(),
                display_name: "b".into(),
                target_uri: "/tmp/evil.command".into(),
                kind: "command".into(),
            }],
            order: None,
            updated_at: Some(3),
        };
        assert!(matches!(
            workspace_import_merge(&mut conn, &[ok_item, bad_item]),
            Err(crate::error::CommandError::InvalidWorkspaceLinkKind { .. })
        ));
        let after = load_workspace(&conn).unwrap();
        assert_eq!(after.len(), 1, "混合批次不得合并任何行（fail-closed）");
        assert_eq!(after[0].id, "seed");
        fs::remove_dir_all(&dir).ok();
    }

    /// upsert_workspace 中途失败必须整体回滚——已写入的前序行不得落库。
    /// 用 BEFORE INSERT trigger 在第 2 条注入失败（RAISE ABORT），模拟「中途 panic/磁盘错」。
    #[test]
    fn upsert_workspace_rolls_back_on_mid_loop_failure() {
        let dir = std::env::temp_dir().join(format!("wm-ws-tx-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);
             CREATE TRIGGER fail_boom BEFORE INSERT ON workspace_items
               WHEN NEW.id = 'boom' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
        // 预置旧数据：失败回滚后必须保持原状
        conn.execute(
            "INSERT INTO workspace_items VALUES ('w1', 'old', NULL, '[]', NULL, 1)",
            [],
        )
        .unwrap();
        let mk = |id: &str, title: &str| WorkspaceItem {
            id: id.into(),
            title: title.into(),
            collapsed: None,
            links: vec![],
            order: None,
            updated_at: Some(2),
        };
        // w1 更新 + boom（触发失败）+ w2：无事务时 w1 会被半截更新
        let r = upsert_workspace(
            &mut conn,
            &[mk("w1", "new"), mk("boom", "x"), mk("w2", "y")],
        );
        assert!(r.is_err(), "trigger 注入失败必须返回 Err");
        let got = load_workspace(&conn).unwrap();
        assert_eq!(got.len(), 1, "半截写入必须被回滚（w2 不得落库）");
        assert_eq!(got[0].title, "old", "w1 必须保持旧值（事务回滚）");
        fs::remove_dir_all(&dir).ok();
    }

    /// delete_workspace 中途失败同样整体回滚——前序已删行恢复。
    #[test]
    fn delete_workspace_rolls_back_on_mid_loop_failure() {
        let dir = std::env::temp_dir().join(format!("wm-ws-tx-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);
             INSERT INTO workspace_items VALUES ('w1', 'a', NULL, '[]', NULL, 1);
             INSERT INTO workspace_items VALUES ('w2', 'b', NULL, '[]', NULL, 1);
             INSERT INTO workspace_items VALUES ('boom', 'c', NULL, '[]', NULL, 1);
             CREATE TRIGGER fail_del BEFORE DELETE ON workspace_items
               WHEN OLD.id = 'boom' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
        // w1 删除成功 → boom 行存在，删它触发失败 → 无事务时 w1 已被半截删掉
        let r = delete_workspace(
            &mut conn,
            &["w1".to_string(), "boom".to_string(), "w2".to_string()],
        );
        assert!(r.is_err(), "trigger 注入失败必须返回 Err");
        let got = load_workspace(&conn).unwrap();
        assert_eq!(got.len(), 3, "回滚后 w1/boom/w2 都必须还在：{got:?}");
        fs::remove_dir_all(&dir).ok();
    }

    /// workspace_import 合并语义 + 跳空 id 脏数据（与 tasks_import 语义一致）：
    /// 同 id 保留 updated_at 更晚者；新 id 直接写入；空 id 跳过。
    /// 直调生产 workspace_import_merge（inline 复刻合并循环 SQL 会让生产改动测试不红）。
    #[test]
    fn workspace_import_merges_by_updated_at_and_skips_empty_id() {
        let dir = std::env::temp_dir().join(format!("wm-ws-imp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);
             INSERT INTO workspace_items VALUES ('a','cur-50',NULL,'[]',NULL,50);
             INSERT INTO workspace_items VALUES ('b','cur-200',NULL,'[]',NULL,200);",
        )
        .unwrap();
        let mk = |id: &str, title: &str, ua: i64| WorkspaceItem {
            id: id.into(),
            title: title.into(),
            collapsed: None,
            links: vec![],
            order: None,
            updated_at: Some(ua),
        };
        let incoming = vec![
            mk("a", "in-100", 100), // 同 id · incoming > cur → UPDATE
            mk("b", "in-180", 180), // 同 id · incoming < cur → 跳过
            mk("c", "in-5", 5),     // 新 id → INSERT
            mk("", "blank", 999),   // 空 id → 跳过
        ];
        let merged = workspace_import_merge(&mut conn, &incoming).unwrap();

        assert_eq!(merged, 2, "应写 a（UPDATE）+ c（INSERT）；跳过 b + 空 id");
        let a: String = conn
            .query_row("SELECT title FROM workspace_items WHERE id='a'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(a, "in-100", "a 被新覆盖");
        let b: String = conn
            .query_row("SELECT title FROM workspace_items WHERE id='b'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(b, "cur-200", "b 未被覆盖（180<200）");
        let c: String = conn
            .query_row("SELECT title FROM workspace_items WHERE id='c'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(c, "in-5", "c 已写入");
        // 空 id 跳过：不应有 id='' 的行（setup 也没创建，double-check）
        let blank_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM workspace_items WHERE id=''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(blank_count, 0, "空 id 应被跳过");
        fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod reset_tests {
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
        assert!(matches!(r, Err(_)), "失败应返回 Err");
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
}

#[cfg(test)]
mod atomic_write_tests {
    use super::*;
    use std::fs;

    /// 原子写 happy path——内容完整落盘，tmp 不残留；覆盖已有文件也正常。
    #[test]
    fn atomic_write_success_and_overwrite() {
        let dir = std::env::temp_dir().join(format!("wm-aw-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("export.json");

        atomic_write(&p, "{\"v\":1}").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\"v\":1}");
        assert!(!dir.join("export.json.tmp").exists(), "tmp 不应残留");

        // 覆盖已有目标（导出到已存在的文件）
        atomic_write(&p, "{\"v\":2}").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\"v\":2}");
        fs::remove_dir_all(&dir).ok();
    }

    /// tmp 写入失败（目标目录只读）→ 目标文件保持原状，不被半截覆盖。
    #[test]
    fn atomic_write_failure_preserves_target() {
        let dir = std::env::temp_dir().join(format!("wm-aw-ro-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("export.json");
        fs::write(&p, "original-complete").unwrap();

        // 目录只读 → 无法创建 tmp → 写失败
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms).unwrap();

        let r = atomic_write(&p, "partial-json-that-must-not-land");

        // 恢复可写以便清理与读回校验
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&dir, perms).unwrap();

        assert!(r.is_err(), "只读目录下 tmp 写入应失败");
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "original-complete",
            "失败时目标文件必须保持原状"
        );
        fs::remove_dir_all(&dir).ok();
    }
}
