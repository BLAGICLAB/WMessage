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
    super::ensure_columns(conn, "tasks", &[("files", "TEXT")])
}

/// 历次改版退役的 tasks 列清扫（幂等）：file_path/file_is_dir（单绑定 → files JSON）、
/// schedule/sched_last/enabled（定时单源 → scheduled_jobs）、origin（画布归属）、
/// acceptance（画布验收）。逐列按 pragma 实际存在性删，跨版本老库缺哪列补删哪列。
/// 前置：legacy 绑定/定时数据迁移已随列删除一并移除（在库数据均已迁移完毕）。
pub fn drop_legacy_task_columns(conn: &rusqlite::Connection) -> Result<(), String> {
    const RETIRED: [&str; 7] = [
        "file_path",
        "file_is_dir",
        "schedule",
        "sched_last",
        "enabled",
        "origin",
        "acceptance",
    ];
    let existing: std::collections::HashSet<String> = {
        let mut stmt = conn
            .prepare("SELECT name FROM pragma_table_info('tasks')")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    for col in RETIRED {
        if existing.contains(col) {
            conn.execute_batch(&format!("ALTER TABLE tasks DROP COLUMN {col}"))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 模型元数据双表（meta_provider / meta_model）：models.dev 同步缓存 + 用户自建，
/// key 即主键（去自增 id），幂等建表。CRUD/命令见 `crate::meta`。
pub fn ensure_meta_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta_provider (
           provider_key     TEXT PRIMARY KEY,
           provider_name    TEXT NOT NULL,
           logo_url         TEXT,
           fallback_color   TEXT NOT NULL,
           fallback_char    TEXT NOT NULL,
           default_base_url TEXT,
           timeout          INTEGER,
           source           TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS meta_model (
           model_key             TEXT PRIMARY KEY,
           provider_key          TEXT NOT NULL,
           display_name          TEXT NOT NULL,
           context_length        INTEGER,
           temperature           REAL,
           top_p                 REAL,
           max_tokens            INTEGER,
           default_system_prompt TEXT,
           source                TEXT NOT NULL
         );
         -- by-provider 查询（models_by_provider / JOIN）全按 provider_key 过滤
         CREATE INDEX IF NOT EXISTS idx_meta_model_provider ON meta_model(provider_key);",
    )
    .map_err(|e| e.to_string())
}

/// 画布工作流退役清扫（幂等）：以 `workflows` 表存在为闸——老库一次性删掉四张
/// 遗留表、`origin='workflow'` 的任务卡和旧提问通知；已清库/新装库无此表，查一次
/// sqlite_master 即返回，零成本。任务卡走硬删（绑定的文件清单存在行内 files 列，
/// 无独立绑定表，不存在孤儿）。
pub fn cleanup_retired_workflow_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    let had_workflows: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'workflows'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false);
    if !had_workflows {
        return Ok(());
    }
    conn.execute_batch(
        "DROP TABLE IF EXISTS workflows;
         DROP TABLE IF EXISTS brief_entries;
         DROP TABLE IF EXISTS workflow_audit;
         DROP TABLE IF EXISTS workflow_settings;
         DELETE FROM tasks WHERE origin = 'workflow';
         DELETE FROM notifications WHERE kind = 'workflow_question';",
    )
    .map_err(|e| e.to_string())
}

/// 画布退役的 tasks 删列（幂等）：老库 DROP 掉 workflow_id / canvas_x / canvas_y
/// 三列（数据已随行清理，列恒 NULL）；新库/已清库 pragma 查无此列即返回。
/// 三列由同一次 W1 ALTER 成对添加，存在性以 workflow_id 一列代表。
pub fn drop_retired_workflow_task_columns(conn: &rusqlite::Connection) -> Result<(), String> {
    let has_col: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'workflow_id'")
        .and_then(|mut stmt| stmt.query_row([], |r| r.get::<_, i64>(0)))
        .map(|v| v == 1)
        .unwrap_or(false);
    if !has_col {
        return Ok(());
    }
    conn.execute_batch(
        "ALTER TABLE tasks DROP COLUMN workflow_id;
         ALTER TABLE tasks DROP COLUMN canvas_x;
         ALTER TABLE tasks DROP COLUMN canvas_y;",
    )
    .map_err(|e| e.to_string())
}

/// 可重试的一次性执行——done 未置位时跑 exec，仅成功才置位
pub fn reset_bot_assigned_with<F: FnOnce() -> Result<(), String>>(
    done: &std::sync::atomic::AtomicBool,
    exec: F,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    // CAS 抢占执行权：load/exec/store 三步分离时，两个并发调用可能都观察到
    // false 而双双执行。compare_exchange 保证只有一个调用 false→true 成功；
    // exec 失败要回滚标记——保留「失败可重试」语义（成功才消耗执行权）
    if done
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        eprintln!("[migrate] reset_bot_assigned_with：执行权已被并发调用持有，本次跳过");
        return Ok(());
    }
    match exec() {
        Ok(()) => Ok(()),
        Err(e) => {
            done.store(false, Ordering::SeqCst);
            Err(e)
        }
    }
}

/// 迁移方案2 的 data.json：json 里有库里缺的任务就按 id 集合差补回
/// legacy `data.json` 路径（供调用方在**取写锁前**做「是否需要迁移」的锁外早返回）。
/// 与 `migrate_data_json` 共用同一路径推导，避免两处重复。
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

#[cfg(test)]
mod retired_cleanup_tests {
    use super::{cleanup_retired_workflow_tables, drop_retired_workflow_task_columns};

    /// 清扫涉及的最小表结构（tasks/notifications 是老库必有的共享表）
    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
               id     TEXT PRIMARY KEY,
               title  TEXT NOT NULL,
               col    TEXT NOT NULL,
               origin TEXT
             );
             CREATE TABLE notifications (
               id         TEXT PRIMARY KEY,
               kind       TEXT NOT NULL,
               title      TEXT NOT NULL,
               body       TEXT NOT NULL,
               payload    TEXT NOT NULL,
               status     TEXT NOT NULL,
               created_at INTEGER
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn drops_legacy_tables_and_rows_once() {
        let conn = setup_conn();
        // 老库形态：四表 + workflow 卡 + 旧 kind 通知 + 一张普通卡
        conn.execute_batch(
            "CREATE TABLE workflows (id TEXT PRIMARY KEY);
             CREATE TABLE brief_entries (id TEXT PRIMARY KEY);
             CREATE TABLE workflow_audit (id TEXT PRIMARY KEY);
             CREATE TABLE workflow_settings (key TEXT PRIMARY KEY);
             INSERT INTO workflows (id) VALUES ('wf1');
             INSERT INTO notifications (id, kind, title, body, payload, status, created_at)
               VALUES ('n1', 'workflow_question', 't', '', '{}', 'pending', 1);
             INSERT INTO tasks (id, title, col, origin) VALUES ('t-wf', '画布卡', 'todo', 'workflow');
             INSERT INTO tasks (id, title, col, origin) VALUES ('t-user', '普通卡', 'todo', NULL);",
        )
        .unwrap();
        cleanup_retired_workflow_tables(&conn).unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('workflows','brief_entries','workflow_audit','workflow_settings')"), 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM tasks WHERE origin = 'workflow'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM notifications WHERE kind = 'workflow_question'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM tasks WHERE id = 't-user'"),
            1,
            "普通卡不受影响"
        );
        // 幂等：二跑不再有可清对象，也不报错
        cleanup_retired_workflow_tables(&conn).unwrap();
    }

    #[test]
    fn fresh_db_without_workflows_table_is_noop() {
        let conn = setup_conn();
        cleanup_retired_workflow_tables(&conn).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn drops_retired_task_columns_once_and_keeps_other_data() {
        let conn = setup_conn();
        conn.execute_batch(
            "ALTER TABLE tasks ADD COLUMN workflow_id TEXT;
             ALTER TABLE tasks ADD COLUMN canvas_x REAL;
             ALTER TABLE tasks ADD COLUMN canvas_y REAL;
             ALTER TABLE tasks ADD COLUMN depends_on TEXT;
             INSERT INTO tasks (id, title, col, depends_on) VALUES ('t1', '普通卡', 'todo', '[\"a\"]');",
        )
        .unwrap();
        drop_retired_workflow_task_columns(&conn).unwrap();
        let col_names: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM pragma_table_info('tasks')")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(!col_names.contains(&"workflow_id".to_string()));
        assert!(!col_names.contains(&"canvas_x".to_string()));
        assert!(!col_names.contains(&"canvas_y".to_string()));
        assert!(
            col_names.contains(&"depends_on".to_string()),
            "依赖列必须保留（图谱/KPI 数据源）"
        );
        let (title, deps): (String, String) = conn
            .query_row(
                "SELECT title, depends_on FROM tasks WHERE id = 't1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "普通卡");
        assert_eq!(deps, "[\"a\"]", "既有列数据不受删列影响");
        // 幂等：二跑列已不存在，直接返回
        drop_retired_workflow_task_columns(&conn).unwrap();
    }
}
