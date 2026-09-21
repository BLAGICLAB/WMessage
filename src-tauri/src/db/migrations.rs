//! 启动迁移 + 连接级 PRAGMA

use tauri::Manager;

/// 连接级 PRAGMA（每次建连都要设——PRAGMA 是 **per-connection** 的）
///
/// 回归锁：`db::tests::conn_pragmas_are_applied`
pub fn apply_conn_pragmas(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
",
    )
    .map_err(|e| e.to_string())
}

/// 多文件绑定：tasks 补 files 列（老库 ALTER 幂等）
pub fn ensure_files_column(conn: &rusqlite::Connection) -> Result<(), String> {
    let has: bool = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).any(|n| n == "files"))
        })
        .unwrap_or(false);
    if !has {
        conn.execute("ALTER TABLE tasks ADD COLUMN files TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 多文件绑定：老单绑定 file_path/file_is_dir → files JSON
pub fn migrate_legacy_file_bindings(conn: &rusqlite::Connection) -> Result<usize, String> {
    let rows: Vec<(String, String, Option<i64>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, file_path, file_is_dir FROM tasks
                 WHERE file_path IS NOT NULL AND file_path <> ''
                   AND (files IS NULL OR files = '' OR files = '[]')",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        mapped
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    let mut n = 0;
    for (id, path, is_dir) in rows {
        let files = serde_json::to_string(&vec![super::TaskFile {
            path,
            is_dir: is_dir.map(|v| v != 0).unwrap_or(false),
        }])
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE tasks SET files = ?1 WHERE id = ?2",
            rusqlite::params![files, id],
        )
        .map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

/// 可重试的一次性执行——done 未置位时跑 exec，仅成功才置位
pub fn reset_bot_assigned_with<F: FnOnce() -> Result<(), String>>(
    done: &std::sync::atomic::AtomicBool,
    exec: F,
) -> bool {
    use std::sync::atomic::Ordering;
    if done.load(Ordering::SeqCst) {
        return true;
    }
    match exec() {
        Ok(()) => {
            done.store(true, Ordering::SeqCst);
            true
        }
        Err(_) => false,
    }
}

/// 迁移方案2 的 data.json：json 里有库里缺的任务就按 id 集合差补回
/// legacy `data.json` 路径（供调用方在**取写锁前**做「是否需要迁移」的锁外早返回）。
/// 与 `migrate_data_json` 共用同一路径推导，避免两处重复（OCR C3-1）。
pub fn legacy_data_json_path<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("data.json"))
}

pub fn migrate_data_json<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    conn: &mut rusqlite::Connection,
) {
    let Some(path) = legacy_data_json_path(app) else {
        return;
    };
    let _ = migrate_data_json_file(&path, conn);
}

/// 可测内核：返回是否执行了迁移
pub fn migrate_data_json_file(file: &std::path::Path, conn: &mut rusqlite::Connection) -> bool {
    if !file.exists() {
        return false;
    }
    let Ok(json) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(tasks) = serde_json::from_str::<Vec<super::Task>>(&json) else {
        return false;
    };
    let existing: std::collections::HashSet<String> = {
        let mut stmt = match conn.prepare("SELECT id FROM tasks") {
            Ok(s) => s,
            Err(_) => return false,
        };
        let rows = match stmt.query_map([], |r| r.get::<_, String>(0)) {
            Ok(r) => r,
            Err(_) => return false,
        };
        rows.filter_map(|r| r.ok()).collect()
    };
    let missing: Vec<super::Task> = tasks
        .into_iter()
        .filter(|t| !existing.contains(&t.id))
        .collect();
    let retire = |file: &std::path::Path| {
        let _ = std::fs::rename(file, file.with_extension("json.migrated"));
    };
    if missing.is_empty() {
        retire(file);
        return false;
    }
    let Ok(tx) = conn.transaction() else {
        return false;
    };
    if super::tasks::upsert_tasks(&tx, &missing).is_ok() && tx.commit().is_ok() {
        retire(file);
        return true;
    }
    false
}
