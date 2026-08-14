//! WMessage 任务数据存储：SQLite（app_data_dir/wmessage.db）
//! 方案B（2026-08-14）：行级增量读写，取代方案2 的 data.json 全量覆盖。
//! 单写者架构：只有主窗口通过 db_upsert/db_delete 写库，挂件只读（db_load）。

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::Manager;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Subtask {
    pub id: String,
    pub text: String,
    pub done: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub due: Option<String>,
    pub note: Option<String>,
    pub tags: Option<Vec<String>>,
    pub file_path: Option<String>,
    pub file_is_dir: Option<bool>,
    pub column: String,
    pub subtasks: Option<Vec<Subtask>>,
    pub completed_at: Option<i64>,
    pub archived: Option<bool>,
    pub deleted_at: Option<i64>,
    pub collapsed: Option<bool>,
}

fn open_db(app: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let conn = rusqlite::Connection::open(dir.join("wmessage.db")).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_secs(2)).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS tasks (
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
           collapsed    INTEGER
         );",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn upsert_tasks(conn: &rusqlite::Connection, tasks: &[Task]) -> Result<(), String> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(
            "INSERT INTO tasks
               (id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                completed_at, archived, deleted_at, collapsed)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(id) DO UPDATE SET
               title=excluded.title, due=excluded.due, note=excluded.note,
               tags=excluded.tags, file_path=excluded.file_path,
               file_is_dir=excluded.file_is_dir, col=excluded.col,
               subtasks=excluded.subtasks, completed_at=excluded.completed_at,
               archived=excluded.archived, deleted_at=excluded.deleted_at,
               collapsed=excluded.collapsed",
        )
        .map_err(|e| e.to_string())?;
    for t in tasks {
        let tags = match &t.tags {
            Some(v) => Some(serde_json::to_string(v).map_err(|e| e.to_string())?),
            None => None,
        };
        let subtasks = match &t.subtasks {
            Some(v) => Some(serde_json::to_string(v).map_err(|e| e.to_string())?),
            None => None,
        };
        stmt.execute(rusqlite::params![
            t.id,
            t.title,
            t.due,
            t.note,
            tags,
            t.file_path,
            t.file_is_dir.map(|b| b as i64),
            t.column,
            subtasks,
            t.completed_at,
            t.archived.map(|b| b as i64),
            t.deleted_at,
            t.collapsed.map(|b| b as i64),
        ])
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn delete_tasks(conn: &rusqlite::Connection, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("DELETE FROM tasks WHERE id IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, rusqlite::params_from_iter(params.iter()))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn load_all(conn: &rusqlite::Connection) -> Result<Vec<Task>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                    completed_at, archived, deleted_at, collapsed
             FROM tasks ORDER BY rowid",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<i64>>(12)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut tasks = Vec::new();
    for r in rows {
        let (id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
             completed_at, archived, deleted_at, collapsed) = r.map_err(|e| e.to_string())?;
        let tags = match tags {
            Some(s) => serde_json::from_str(&s).ok(),
            None => None,
        };
        let subtasks = match subtasks {
            Some(s) => serde_json::from_str(&s).ok(),
            None => None,
        };
        tasks.push(Task {
            id,
            title,
            due,
            note,
            tags,
            file_path,
            file_is_dir: file_is_dir.map(|v| v != 0),
            column: col,
            subtasks,
            completed_at,
            archived: archived.map(|v| v != 0),
            deleted_at,
            collapsed: collapsed.map(|v| v != 0),
        });
    }
    Ok(tasks)
}

/// 库为空时迁移方案2 的 data.json：导入全部任务后删除旧文件
fn migrate_data_json(app: &tauri::AppHandle, conn: &mut rusqlite::Connection) {
    let Ok(dir) = app.path().app_data_dir() else { return };
    let file = dir.join("data.json");
    if !file.exists() {
        return;
    }
    let Ok(json) = std::fs::read_to_string(&file) else { return };
    let Ok(tasks) = serde_json::from_str::<Vec<Task>>(&json) else { return };
    let Ok(tx) = conn.transaction() else { return };
    if upsert_tasks(&tx, &tasks).is_ok() {
        if tx.commit().is_ok() {
            let _ = std::fs::remove_file(&file);
        }
    }
}

#[tauri::command]
pub fn db_load(app: tauri::AppHandle) -> Result<Vec<Task>, String> {
    let mut conn = open_db(&app)?;
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count == 0 {
        migrate_data_json(&app, &mut conn);
    }
    load_all(&conn)
}

#[tauri::command]
pub fn db_upsert(app: tauri::AppHandle, tasks: Vec<Task>) -> Result<(), String> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut conn = open_db(&app)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    upsert_tasks(&tx, &tasks)?;
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn db_delete(app: tauri::AppHandle, ids: Vec<String>) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let conn = open_db(&app)?;
    delete_tasks(&conn, &ids)
}
