//! 任务卡 CRUD：upsert / delete / load + db_* tauri command + tasks_export/import

use serde::{Deserialize, Serialize};
use tauri::async_runtime;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Subtask {
    pub id: String,
    pub text: String,
    pub done: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    pub path: String,
    pub is_dir: bool,
}

pub const MAX_TASK_FILES: usize = 10;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub due: Option<String>,
    pub note: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub files: Option<Vec<TaskFile>>,
    pub file_path: Option<String>,
    pub file_is_dir: Option<bool>,
    pub column: String,
    pub subtasks: Option<Vec<Subtask>>,
    pub completed_at: Option<i64>,
    pub archived: Option<bool>,
    pub deleted_at: Option<i64>,
    pub collapsed: Option<bool>,
    pub order: Option<f64>,
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub sched_last: Option<i64>,
    #[serde(default)]
    pub bot_assigned: Option<bool>,
    #[serde(default, skip_serializing)]
    pub expected_updated_at: Option<i64>,
}

impl Task {
    pub fn effective_files(&self) -> Vec<TaskFile> {
        if let Some(files) = &self.files {
            if !files.is_empty() {
                return files.clone();
            }
        }
        match &self.file_path {
            Some(p) if !p.is_empty() => vec![TaskFile {
                path: p.clone(),
                is_dir: self.file_is_dir.unwrap_or(false),
            }],
            _ => Vec::new(),
        }
    }
}

/// 多文件绑定元数据解析：fs::metadata 判定 isDir，去重保序、空路径丢弃、超 MAX_TASK_FILES 截断
pub fn resolve_task_files(paths: Vec<String>) -> Vec<TaskFile> {
    let mut out: Vec<TaskFile> = Vec::new();
    for p in paths {
        let path = p.trim().to_string();
        if path.is_empty() || out.iter().any(|f| f.path == path) {
            continue;
        }
        let is_dir = std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        out.push(TaskFile { path, is_dir });
        if out.len() >= MAX_TASK_FILES {
            break;
        }
    }
    out
}

#[tauri::command]
pub fn bind_files(paths: Vec<String>) -> Vec<TaskFile> {
    resolve_task_files(paths)
}

/// Reflection 触发阈值/批量：summary 攒够 10 条合成一条 reflection
pub const REFLECTION_BATCH: i64 = 10;

/// 写冲突错误前缀——RMW 基线比对失败（lost-update 防护拒写）
pub const CONFLICT_ERR_PREFIX: &str = "写冲突";

/// 「行存在性」基线哨兵：i64::MIN 保证与任何合法毫秒时间戳不撞
pub const BASELINE_NULL_ROW: i64 = i64::MIN;

pub fn upsert_tasks(conn: &rusqlite::Connection, tasks: &[Task]) -> Result<(), String> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(
            "INSERT INTO tasks
               (id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                completed_at, archived, deleted_at, collapsed, ord, updated_at, schedule, sched_last, bot_assigned, files)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
             ON CONFLICT(id) DO UPDATE SET
               title=excluded.title, due=excluded.due, note=excluded.note,
               tags=excluded.tags, file_path=excluded.file_path,
               file_is_dir=excluded.file_is_dir, col=excluded.col,
               subtasks=excluded.subtasks, completed_at=excluded.completed_at,
               archived=excluded.archived, deleted_at=excluded.deleted_at,
               collapsed=excluded.collapsed, ord=excluded.ord,
               updated_at=excluded.updated_at,
               schedule=excluded.schedule, sched_last=excluded.sched_last,
               bot_assigned=excluded.bot_assigned, files=excluded.files
             WHERE tasks.updated_at IS NULL OR excluded.updated_at >= tasks.updated_at",
        )
        .map_err(|e| e.to_string())?;
    for t in tasks {
        if let Some(expected) = t.expected_updated_at {
            use rusqlite::OptionalExtension;
            let cur: Option<Option<i64>> = conn
                .query_row("SELECT updated_at FROM tasks WHERE id = ?1", [&t.id], |r| {
                    r.get::<_, Option<i64>>(0)
                })
                .optional()
                .map_err(|e| e.to_string())?;
            let conflict = if expected == BASELINE_NULL_ROW {
                cur != Some(None)
            } else {
                cur.flatten() != Some(expected)
            };
            if conflict {
                let cur_flat = cur.flatten();
                let baseline_desc = if expected == BASELINE_NULL_ROW {
                    "NULL（行存在性）".to_string()
                } else {
                    expected.to_string()
                };
                return Err(format!(
                    "{CONFLICT_ERR_PREFIX}：任务 {} 读快照后已被其他写者 {}，本次整行写回被拒（基线 updated_at={baseline_desc}，现行 {cur_flat:?}）",
                    t.id,
                    if cur_flat.is_some() { "修改" } else { "删除" },
                ));
            }
        }
        let tags = match &t.tags {
            Some(v) => Some(serde_json::to_string(v).map_err(|e| e.to_string())?),
            None => None,
        };
        let subtasks = match &t.subtasks {
            Some(v) => Some(serde_json::to_string(v).map_err(|e| e.to_string())?),
            None => None,
        };
        let files = match &t.files {
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
            t.order,
            t.updated_at,
            t.schedule,
            t.sched_last,
            t.bot_assigned.map(|b| b as i64),
            files,
        ])
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn delete_tasks(conn: &rusqlite::Connection, ids: &[String]) -> Result<(), String> {
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

pub fn load_all(conn: &rusqlite::Connection) -> Result<Vec<super::Task>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                    completed_at, archived, deleted_at, collapsed, ord, updated_at, schedule, sched_last, bot_assigned, files
             FROM tasks ORDER BY ord, rowid",
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
                row.get::<_, Option<f64>>(13)?,
                row.get::<_, Option<i64>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<i64>>(17)?,
                row.get::<_, Option<String>>(18)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut tasks = Vec::new();
    for r in rows {
        let (
            id,
            title,
            due,
            note,
            tags,
            file_path,
            file_is_dir,
            col,
            subtasks,
            completed_at,
            archived,
            deleted_at,
            collapsed,
            order,
            updated_at,
            schedule,
            sched_last,
            bot_assigned,
            files,
        ) = r.map_err(|e| e.to_string())?;
        let tags = match tags {
            Some(s) => serde_json::from_str(&s).ok(),
            None => None,
        };
        let subtasks = match &subtasks {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => Some(v),
                Err(_) => {
                    eprintln!("[db] 任务 {id} 的 subtasks JSON 损坏，按空读取（原值未动）");
                    None
                }
            },
            None => None,
        };
        let files = match &files {
            Some(s) => match serde_json::from_str(s) {
                Ok(v) => Some(v),
                Err(_) => {
                    eprintln!("[db] 任务 {id} 的 files JSON 损坏，按空读取（原值未动）");
                    None
                }
            },
            None => None,
        };
        tasks.push(super::Task {
            id,
            title,
            due,
            note,
            tags,
            files,
            file_path,
            file_is_dir: file_is_dir.map(|v| v != 0),
            column: col,
            subtasks,
            completed_at,
            archived: archived.map(|v| v != 0),
            deleted_at,
            collapsed: collapsed.map(|v| v != 0),
            order,
            updated_at,
            schedule,
            sched_last,
            bot_assigned: bot_assigned.map(|v| v != 0),
            expected_updated_at: None,
        });
    }
    Ok(tasks)
}

#[tauri::command]
pub async fn db_load(app: AppHandle) -> CommandResult<Vec<super::Task>> {
    db_load_for(&app).await
}

pub async fn db_load_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> CommandResult<Vec<super::Task>> {
    let app = app.clone();
    async_runtime::spawn_blocking(move || {
        let mut conn = super::open_db(&app)?;
        super::migrations::migrate_data_json(&app, &mut conn);
        load_all(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库读取线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn db_upsert(app: AppHandle, tasks: Vec<super::Task>) -> CommandResult<()> {
    db_upsert_for(&app, tasks).await
}

pub async fn db_upsert_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tasks: Vec<super::Task>,
) -> CommandResult<()> {
    let app = app.clone();
    async_runtime::spawn_blocking(move || {
        if tasks.is_empty() {
            return Ok(());
        }
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = super::open_db(&app)?;
        let tx = conn
            .transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        upsert_tasks(&tx, &tasks).map_err(CommandError::from)?;
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库 upsert 线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn db_delete(app: AppHandle, ids: Vec<String>) -> CommandResult<()> {
    async_runtime::spawn_blocking(move || {
        if ids.is_empty() {
            return Ok(());
        }
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = super::open_db(&app)?;
        delete_tasks(&conn, &ids).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库删除线程 join 失败：{e}")))?
}

pub fn check_export_path(path: &str) -> CommandResult<()> {
    let ok = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    if !ok {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path.to_string(),
            reason: "导出路径必须是 .json 文件".into(),
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn tasks_export(app: AppHandle, path: String) -> CommandResult<usize> {
    check_export_path(&path)?;
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        let tasks = load_all(&conn)?;
        let json = serde_json::to_string_pretty(&tasks).map_err(|e| e.to_string())?;
        super::paths::atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(tasks.len())
    })
    .await
    .map_err(|e| CommandError::from(format!("任务导出线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn tasks_import(app: AppHandle, path: String) -> CommandResult<usize> {
    async_runtime::spawn_blocking(move || {
        use rusqlite::OptionalExtension;
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("无法读取所选文件：{e}"))?;
        let ext: Vec<super::Task> =
            serde_json::from_str(&raw).map_err(|e| format!("不是有效的任务数据 JSON：{e}"))?;
        if ext.is_empty() {
            return Ok(0);
        }
        let _g = super::DB_WRITE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = super::open_db(&app)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut merged = 0usize;
        for t in &ext {
            if t.id.trim().is_empty() {
                continue;
            }
            let cur: Option<Option<i64>> = tx
                .query_row(
                    "SELECT updated_at FROM tasks WHERE id = ?1",
                    rusqlite::params![t.id],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let cur_ua = cur.flatten().unwrap_or(0);
            let take = t.updated_at.unwrap_or(0) > cur_ua;
            if take {
                upsert_tasks(&tx, std::slice::from_ref(t))?;
                merged += 1;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(merged)
    })
    .await
    .map_err(|e| CommandError::from(format!("任务导入线程 join 失败：{e}")))?
}
