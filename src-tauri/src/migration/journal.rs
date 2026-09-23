//! B1: 文件迁移操作 journal（防 lost-update / 孤儿文件）。
//!
//! 状态机：pending → committed（成功完成）/ cleared（未启动或明确失败）。
//! 启动时 `journal_replay_pending` 检测 pending 条目修复 DB 一致性。
//!
//! NEW-B-2: journal 写纳入 `DB_WRITE_LOCK` 临界区——与 db_upsert 等持锁写串行，
//! 不再靠 2s busy_timeout 兜底并发冲突。

use std::path::Path;

use super::types::JournalEntry;

/// NEW-B-2: journal 写纳入 DB_WRITE_LOCK 临界区——与 db_upsert 等持锁写串行，
/// 不再靠 2s busy_timeout 兜底并发冲突。
/// 注意：不能在整个 run_migration_inner 入口持锁——其内部 block_on(db_upsert)
/// 会在 spawn_blocking 线程里再抢同一把锁，std Mutex 不可重入 → 死锁。
/// 因此只在每次 journal 写时短临界区持锁。
pub(crate) fn db_write_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        // C3-1：poison 事件必须可见（同 db::lock_db_write 形态），恢复行为不变
        eprintln!("[mutex_poisoned] migration::journal DB_WRITE_LOCK: {e:?}");
        e.into_inner()
    })
}

/// B1 inner: 记录一个 pending 操作，返回 row id。
/// 抽出来为方便单测（不需 AppHandle）。生产仍走 journal_pending 包一层。
/// MI-04a：同一 (task_id, src) 至多一条 pending——重入/重试复用既有行
/// （刷新 op/dst/created_at），不再制造 id DESC 下不可见、但仍被 replay 扫到的孤儿。
/// 去重原子性依赖调用方持 db_write_lock（journal_pending 已持锁，见同文件 db_write_lock 注释）。
pub(crate) fn journal_pending_inner(
    conn: &rusqlite::Connection,
    op: &str,
    src: &Path,
    dst: Option<&Path>,
    task_id: &str,
    now_ms: i64,
) -> Result<i64, String> {
    use rusqlite::OptionalExtension;
    let dst_s = dst.map(|p| p.to_string_lossy().into_owned());
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM migration_journal
             WHERE task_id = ?1 AND src = ?2 AND state = 'pending'
             ORDER BY id DESC LIMIT 1",
            rusqlite::params![task_id, src.to_string_lossy()],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(id) = existing {
        conn.execute(
            "UPDATE migration_journal
             SET op = ?2, dst = ?3, created_at = ?4
             WHERE id = ?1",
            rusqlite::params![id, op, dst_s, now_ms],
        )
        .map_err(|e| e.to_string())?;
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO migration_journal (op, src, dst, task_id, state, created_at)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
        rusqlite::params![op, src.to_string_lossy(), dst_s, task_id, now_ms,],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// B1: 记录一个 pending 操作，返回 row id（后续 committed/cleared 需用）。
/// pending 表示「即将开始」还未完成，启动时需要 replay 检测。
/// NEW-B-2: 复用调用方连接（不再每次 open_db 付全套 schema 检查），写持 DB_WRITE_LOCK。
pub(crate) fn journal_pending(
    conn: &rusqlite::Connection,
    op: &str,
    src: &Path,
    dst: Option<&Path>,
    task_id: &str,
) -> Result<i64, String> {
    let _g = db_write_lock();
    journal_pending_inner(conn, op, src, dst, task_id, crate::migration::ops::now_ms())
}

/// B1 inner: 标记 committed。replay 跳过该行。
pub(crate) fn journal_committed_inner(conn: &rusqlite::Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE migration_journal SET state = 'committed' WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// B1: 操作成功完成 → 标记 committed。replay 跳过该行。
/// NEW-B-2: 复用调用方连接，写持 DB_WRITE_LOCK。
pub(crate) fn journal_committed(conn: &rusqlite::Connection, id: i64) -> Result<(), String> {
    let _g = db_write_lock();
    journal_committed_inner(conn, id)
}

/// B1 inner: 清除（未启动 / 已明确失败）。
pub(crate) fn journal_cleared_inner(conn: &rusqlite::Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE migration_journal SET state = 'cleared' WHERE id = ?1",
        rusqlite::params![id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// B1: 操作未启动 / 已明确失败 → 清除该行（不需要 replay）。
/// NEW-B-2: 复用调用方连接，写持 DB_WRITE_LOCK。
pub(crate) fn journal_cleared(conn: &rusqlite::Connection, id: i64) -> Result<(), String> {
    let _g = db_write_lock();
    journal_cleared_inner(conn, id)
}

/// NEW-B-1 inner: 查某任务某源路径最新一条 pending journal（轮询中就地对账用）。
/// 抽出来为方便单测（不需 AppHandle）。
pub(crate) fn journal_find_pending_inner(
    conn: &rusqlite::Connection,
    task_id: &str,
    src: &Path,
) -> Result<Option<JournalEntry>, String> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT id, op, src, dst, task_id
         FROM migration_journal
         WHERE task_id = ?1 AND src = ?2 AND state = 'pending'
         ORDER BY id DESC LIMIT 1",
        rusqlite::params![task_id, src.to_string_lossy()],
        |r| {
            Ok(JournalEntry {
                id: r.get(0)?,
                op: r.get(1)?,
                src: r.get(2)?,
                dst: r.get(3)?,
                task_id: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// NEW-B-1: 生产包装（NEW-B-2 起复用调用方连接；纯 SELECT，WAL 下读写不互斥，无需持锁）。
pub(crate) fn journal_find_pending(
    conn: &rusqlite::Connection,
    task_id: &str,
    src: &Path,
) -> Result<Option<JournalEntry>, String> {
    journal_find_pending_inner(conn, task_id, src)
}
