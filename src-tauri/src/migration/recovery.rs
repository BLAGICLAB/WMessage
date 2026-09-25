//! B1: 启动时 replay + 任务 DB 修复。
//!
//! 当 journal 仍 pending 但文件操作已完成（move/delete 成功但 db_upsert 失败），
//! 启动 replay 检测 dst 存在 + src 不存在 → 重新落 DB 绑定修复一致。
//!
//! NEW-B-1: 修复前必须确认任务 file_path 仍指向 src（未被用户重绑 / 已解绑），
//! 否则跳过修复避免旧 journal 覆盖新绑定。

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::db;

use super::journal::{journal_cleared_inner, journal_committed_inner};
use super::ops::{log_line, now_ms};
use super::types::JournalEntry;

/// NEW-B-1: replay 修复前提——任务 file_path 仍指向 journal 记录的 src，
/// 即 DB 自该文件操作后未被碰过。若用户期间重新绑定了别的文件 / 已解绑，
/// 必须跳过修复，防止旧 journal 覆盖新绑定。
pub(crate) fn file_path_untouched(current: Option<&str>, expected_src: &str) -> bool {
    current == Some(expected_src)
}

/// NEW-B-1: 「源已消失」时的处置决策（纯函数，可单测）。
/// pending = 该 (task, src) 最新 pending journal；dst_exists 仅对 move 有意义。
pub(crate) enum SrcMissingAction {
    /// 上轮 move 成功但 DB 未更新 → 重新绑定到 dst，并提交该 journal
    RepairMove { dst: String, journal_id: i64 },
    /// 确认解绑；Some(id) 的 journal 在解绑落盘成功后提交（关闭对账环路）
    Unbind { commit_journal: Option<i64> },
}

pub(crate) fn decide_src_missing(
    pending: Option<JournalEntry>,
    dst_exists: bool,
) -> SrcMissingAction {
    match pending {
        Some(e) if e.op == "move" && dst_exists => match e.dst {
            Some(dst) => SrcMissingAction::RepairMove {
                dst,
                journal_id: e.id,
            },
            None => SrcMissingAction::Unbind {
                commit_journal: Some(e.id),
            },
        },
        Some(e) => SrcMissingAction::Unbind {
            commit_journal: Some(e.id),
        },
        None => SrcMissingAction::Unbind {
            commit_journal: None,
        },
    }
}

/// B1: 启动时 replay pending 条目。
///
/// 语义：
/// - move + src 不存在 + dst 存在 → 文件已迁但 DB 未更新 → 重跑 db_upsert 改 file_path
/// - move + src 存在 + dst 不存在 → 文件未迁（操作未执行或失败）→ clear 行
/// - move + 两边都在 / 都不在 → 异常状态 → clear 行 + 记错误
/// - delete + src 不存在 → 文件已删但 DB 未清 file_path → 重跑 db_upsert 置 None
/// - delete + src 存在 → 文件未删 → clear 行
///
/// 返回（恢复条数, 错误条数）供调用者记日志。
pub fn journal_replay_pending(app: &AppHandle) -> Result<(usize, usize), String> {
    let conn = db::open_db(app).map_err(|e| e.to_string())?;
    // 孤儿 pending 清理（拍板 #8=A）：MI-04a 修复前产生的同 (task_id, src) 多条
    // pending（id DESC 下不可见、仍被 replay 扫描 → 同 key 重复处置）。启动重放前
    // 一次性清理：只留 id 最大（最新尝试），其余删除 + 留痕。
    let purged = purge_orphan_pending(&conn).map_err(|e| e.to_string())?;
    if purged > 0 {
        log_line(
            app,
            &format!("journal replay: 清理孤儿 pending {purged} 条（同任务同源仅保留最新一条）"),
        );
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, op, src, dst, task_id
             FROM migration_journal WHERE state = 'pending'
             ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<JournalEntry> = stmt
        .query_map([], |r| {
            Ok(JournalEntry {
                id: r.get(0)?,
                op: r.get(1)?,
                src: r.get(2)?,
                dst: r.get(3)?,
                task_id: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut recovered = 0usize;
    let mut errors = 0usize;
    for entry in rows {
        let src = PathBuf::from(&entry.src);
        let dst = entry.dst.as_ref().map(PathBuf::from);
        match entry.op.as_str() {
            "move" => match (&dst, src.exists()) {
                (Some(d), false) if d.exists() => {
                    // move 成功但 DB 未更新
                    match recover_move_db(app, &entry.task_id, &entry.src, d) {
                        Ok(true) => {
                            journal_committed_inner(&conn, entry.id).ok();
                            recovered += 1;
                            log_line(
                                app,
                                &format!("journal replay: 修复 {} → {}", entry.src, d.display()),
                            );
                        }
                        Ok(false) => {
                            // NEW-B-1: 任务 file_path 已不指向 src（用户重绑 / 已解绑）
                            // → 跳过修复，避免旧 journal 覆盖新绑定
                            journal_cleared_inner(&conn, entry.id).ok();
                            log_line(
                                app,
                                &format!(
                                    "journal replay: 跳过 {}（任务附件已变更，不覆盖）",
                                    entry.src
                                ),
                            );
                        }
                        Err(e) => {
                            errors += 1;
                            log_line(
                                app,
                                &format!("journal replay: 修复 {} 失败：{}", entry.src, e),
                            );
                        }
                    }
                }
                _ => {
                    // 其他异常 / 未开始 / 异常状态 → clear 行
                    journal_cleared_inner(&conn, entry.id).ok();
                }
            },
            "delete" => {
                if !src.exists() {
                    // delete 成功但 DB 未清 file_path
                    match recover_delete_db(app, &entry.task_id, &entry.src) {
                        Ok(true) => {
                            journal_committed_inner(&conn, entry.id).ok();
                            recovered += 1;
                            log_line(
                                app,
                                &format!("journal replay: 清除 {} 的 file_path", entry.src),
                            );
                        }
                        Ok(false) => {
                            // NEW-B-1: 任务 file_path 已变更（用户重绑）→ 跳过，不清空新绑定
                            journal_cleared_inner(&conn, entry.id).ok();
                            log_line(
                                app,
                                &format!(
                                    "journal replay: 跳过 {}（任务附件已变更，不覆盖）",
                                    entry.src
                                ),
                            );
                        }
                        Err(e) => {
                            errors += 1;
                            log_line(
                                app,
                                &format!("journal replay: 清除 {} 失败：{}", entry.src, e),
                            );
                        }
                    }
                } else {
                    journal_cleared_inner(&conn, entry.id).ok();
                }
            }
            _ => {
                journal_cleared_inner(&conn, entry.id).ok();
            }
        }
    }

    // 限制日志表大小：最近 1000 条保留，剩余 cleared/committed 的清理
    // （避免 journal 表无限增长）
    let _ = conn.execute(
        "DELETE FROM migration_journal
         WHERE id NOT IN (
             SELECT id FROM migration_journal ORDER BY id DESC LIMIT 1000
         ) AND state IN ('committed', 'cleared')",
        [],
    );

    Ok((recovered, errors))
}

/// 返回值：Ok(true)=已修复；Ok(false)=任务 file_path 已不指向 expected_src（用户重绑/已解绑），
/// 跳过修复防止旧 journal 覆盖新绑定（NEW-B-1）。
fn recover_move_db(
    app: &AppHandle,
    task_id: &str,
    expected_src: &str,
    dst: &Path,
) -> Result<bool, String> {
    // B3: db_load/db_upsert 改 async 了；recover_* 在 spawn_polling 的 std::thread 里跑，
    // 不在 tokio runtime 上 → 用 block_on 安全桥接（不会死锁）。
    let tasks = tauri::async_runtime::block_on(async { db::db_load(app.clone()).await })
        .map_err(|e| e.to_string())?;
    let Some(mut t) = tasks.into_iter().find(|x| x.id == task_id) else {
        return Err(format!("task {task_id} 不存在"));
    };
    if !file_path_untouched(t.file_path.as_deref(), expected_src) {
        return Ok(false);
    }
    t.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
    t.file_path = Some(dst.to_string_lossy().to_string());
    t.updated_at = Some(now_ms());
    tauri::async_runtime::block_on(async { db::db_upsert(app.clone(), vec![t]).await })
        .map_err(|e| e.to_string())?;
    Ok(true)
}

fn recover_delete_db(app: &AppHandle, task_id: &str, expected_src: &str) -> Result<bool, String> {
    let tasks = tauri::async_runtime::block_on(async { db::db_load(app.clone()).await })
        .map_err(|e| e.to_string())?;
    let Some(mut t) = tasks.into_iter().find(|x| x.id == task_id) else {
        return Err(format!("task {task_id} 不存在"));
    };
    if !file_path_untouched(t.file_path.as_deref(), expected_src) {
        return Ok(false);
    }
    t.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
    t.file_path = None;
    t.file_is_dir = None;
    t.updated_at = Some(now_ms());
    tauri::async_runtime::block_on(async { db::db_upsert(app.clone(), vec![t]).await })
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// 孤儿 pending 清理内核（纯函数供单测）：同 (task_id, src) 的多条 pending
/// 仅保留 id 最大的一条，其余删除，返回删除行数。含 committed 共存的 pending
/// 不动（可能是新一轮中断尝试，replay 自身能处置）。
pub(crate) fn purge_orphan_pending(conn: &rusqlite::Connection) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM migration_journal
         WHERE state = 'pending'
           AND EXISTS (
             SELECT 1 FROM migration_journal newer
             WHERE newer.task_id = migration_journal.task_id
               AND newer.src = migration_journal.src
               AND newer.state = 'pending'
               AND newer.id > migration_journal.id
           )",
        [],
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_journal() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE migration_journal (
               id          INTEGER PRIMARY KEY AUTOINCREMENT,
               op          TEXT    NOT NULL,
               src         TEXT    NOT NULL,
               dst         TEXT,
               task_id     TEXT    NOT NULL,
               state       TEXT    NOT NULL,
               created_at  INTEGER NOT NULL
             );",
        )
        .unwrap();
        conn
    }

    fn insert(conn: &rusqlite::Connection, task_id: &str, src: &str, state: &str) -> i64 {
        conn.execute(
            "INSERT INTO migration_journal (op, src, dst, task_id, state, created_at)
             VALUES ('move', ?1, NULL, ?2, ?3, 1)",
            rusqlite::params![src, task_id, state],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    /// 同 (task_id, src) 多条 pending → 只留 id 最大，其余删除；返回删除数
    #[test]
    fn purge_orphan_pending_keeps_latest_per_key() {
        let conn = setup_journal();
        insert(&conn, "t1", "/a/file.txt", "pending");
        insert(&conn, "t1", "/a/file.txt", "pending");
        let latest = insert(&conn, "t1", "/a/file.txt", "pending");
        insert(&conn, "t2", "/b/other.txt", "pending");

        let purged = purge_orphan_pending(&conn).unwrap();

        assert_eq!(purged, 2, "同 key 前两条 pending 应删除");
        let left: Vec<i64> = conn
            .prepare("SELECT id FROM migration_journal WHERE state = 'pending' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            left,
            vec![latest, 4],
            "同 key 仅保留 id 最大的 pending；其他 key 不动"
        );
    }

    /// committed 共存的 pending 不删（可能是新一轮中断尝试，replay 自身能处置）
    #[test]
    fn purge_orphan_pending_leaves_committed_coexistence() {
        let conn = setup_journal();
        insert(&conn, "t1", "/a/file.txt", "committed");
        let pending = insert(&conn, "t1", "/a/file.txt", "pending");

        // 补组合：committed + 同 key 多 pending → 同 key 重复处置风险仍在，仍留最新
        insert(&conn, "t1", "/a/file.txt", "pending");
        let purged = purge_orphan_pending(&conn).unwrap();
        assert_eq!(
            purged, 1,
            "多 pending 时仍清旧留新（committed 不参与删除判定）"
        );
        let left: Vec<i64> = conn
            .prepare("SELECT id FROM migration_journal WHERE state = 'pending' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(left.len(), 1, "committed 共存场景同 key 也仅留最新一条");
        // 终态：committed(1) 与最新 pending(3) 存活；孤儿 pending(2) 已删
        let final_states: Vec<(i64, String)> = conn
            .prepare("SELECT id, state FROM migration_journal ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            final_states,
            vec![(1, "committed".to_string()), (3, "pending".to_string())],
            "committed 保留；同 key pending 仅留最新"
        );
        assert_eq!(pending, 2);
    }
}
