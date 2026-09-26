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

/// 任务状态(三列看板：todo / doing / done)。
///
/// **单一来源**:所有后端「状态列」比较 / 赋值 / 序列化都走此 enum。
/// 序列化 / 反序列化都走裸字符串(serde 自定义实现),与前端
/// `src/types.ts` 的 `ColumnId = "todo" | "doing" | "done"` 保持字节级一致;
/// wire format 与 db::Task::column 改 enum 前的 String 完全兼容,不需要迁移。
/// 前端代码不需要改;改 enum 只是后端内部表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    Todo,
    Doing,
    Done,
}

impl TaskStatus {
    /// 裸字符串表示(wire / DB / 前端共享的 3 个值)
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Doing => "doing",
            Self::Done => "done",
        }
    }

    /// 全部 variant,声明顺序(用于 registry JSON schema / 校验枚举 / UI 渲染顺序)
    pub const ALL: &'static [TaskStatus] = &[Self::Todo, Self::Doing, Self::Done];
}

impl std::str::FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "todo" => Ok(Self::Todo),
            "doing" => Ok(Self::Doing),
            "done" => Ok(Self::Done),
            other => Err(format!("invalid task status: {other}")),
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// 字符串 ↔ enum 的 serde 手写实现:wire format 保持与原来 `String` 字段完全一致,
// 前端按字符串接收无感知。
impl serde::Serialize for TaskStatus {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for TaskStatus {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = <&str>::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

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
    pub column: TaskStatus,
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
    // 契约断言（OCR C3-1）：调用方必须持 DB_WRITE_LOCK（经 lock_db_write() 的守卫）。
    // 线程本地标记精确判定「当前线程持锁」—— try_lock 做不到（线程无关 + poisoned 也 Err）。
    // debug_assert：release 编译掉，不给生产路径加开销，同时把契约钉在 debug/CI 上。
    debug_assert!(
        super::holding_db_write(),
        "upsert_tasks 必须在持有 DB_WRITE_LOCK（lock_db_write()）时调用"
    );
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
    let mut affected_total: usize = 0;
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
        let affected = stmt
            .execute(rusqlite::params![
                t.id,
                t.title,
                t.due,
                t.note,
                tags,
                t.file_path,
                t.file_is_dir.map(|b| b as i64),
                t.column.as_str(),
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
        affected_total += affected;
    }
    // ON CONFLICT WHERE 子句不满足 → rows_affected=0（静默 no-op），
    // 视为冲突，返 CONFLICT_ERR_PREFIX（与 expected_updated_at 路径共用错误码）。
    if affected_total < tasks.len() {
        return Err(format!(
            "{CONFLICT_ERR_PREFIX}：ON CONFLICT WHERE 子句不满足，写入未生效（affected={affected_total}/{}）",
            tasks.len()
        ));
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
        // col 从 DB 读出仍是 String(列类型 TEXT),parse 到 TaskStatus enum。
        // 与本函数 subtasks/files JSON 损坏「warn + 按空读取」的契约对齐:
        // 单行 col 异常不应让整个 load_all 失败、把全部任务藏起来。
        // DB 列是 TEXT 无 CHECK 约束,历史数据 / 老 client / 未来 bug 都可能
        // 塞非法值进来 — 兜底 Todo 比炸整个看板安全得多。
        // 先绑定原始值再 parse:否则 shadow 后日志里只剩解析错误,
        // 定位不到 DB 里实际是哪个脏值(取证需要原值)。
        let raw_col = col;
        let col: TaskStatus = match raw_col.parse() {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[db] 任务 {id} 的 col 值「{raw_col}」无法识别为 TaskStatus，按 Todo 兜底读取（原值未动）：{e}"
                );
                TaskStatus::Todo
            }
        };
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
        // C3-1：legacy 迁移会在**读路径**上写库（upsert_tasks），故须持 DB_WRITE_LOCK。
        // 前置约束：exists() 早返回放在**锁外** —— 正常路径（无 data.json）根本不进临界区，
        // 零串行化代价（只一次 stat）；迁移路径才进锁，且是一次性窗口（迁后改名 .json.migrated）。
        if super::migrations::legacy_data_json_path(&app)
            .map(|p| p.exists())
            .unwrap_or(false)
        {
            let _g = super::lock_db_write();
            super::migrations::migrate_data_json(&app, &mut conn);
        }
        load_all(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库读取线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn db_upsert(app: AppHandle, tasks: Vec<super::Task>) -> CommandResult<()> {
    db_upsert_for(&app, tasks).await
}

/// task_set_column 的锁内段（纯 DB 逻辑，单测锚点）：**同一把写锁内**读现值 →
/// 应用列语义 → 打新基线（= 锁内现读 updated_at）→ 写。基线在锁内现读，
/// 对任何并发写者（规则定时器/迁移/其他实例）都不可能冲突。
/// 语义与前端 toggleDone 1:1：
/// → done：completed_at=now、archived=false、bot_assigned 清（用户完成显示用户头像）
/// → todo/doing：completed_at/archived 清（bot_assigned 保留）
pub(crate) fn task_set_column_locked(
    conn: &mut rusqlite::Connection,
    id: &str,
    status: TaskStatus,
    now: i64,
) -> CommandResult<super::Task> {
    let mut task = load_all(conn)?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    task.expected_updated_at = task.updated_at;
    task.updated_at = Some(now);
    match status {
        TaskStatus::Done => {
            task.column = TaskStatus::Done;
            task.completed_at = Some(now);
            task.archived = Some(false);
            task.bot_assigned = None;
        }
        TaskStatus::Todo | TaskStatus::Doing => {
            task.column = status;
            task.completed_at = None;
            task.archived = None;
        }
    }
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    upsert_tasks(&tx, std::slice::from_ref(&task)).map_err(CommandError::from)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(task)
}

/// TP-1：单任务列状态定向迁移（完成✅/取消✅ 专用）。前端传意图（id + 目标列），
/// 服务端锁内现读现写，**前端快照完全不参与**——彻底免除整行回写的 RMW 写冲突。
#[tauri::command]
pub async fn task_set_column(
    app: AppHandle,
    id: String,
    col: String,
) -> CommandResult<super::Task> {
    let status = match col.as_str() {
        "todo" => TaskStatus::Todo,
        "doing" => TaskStatus::Doing,
        "done" => TaskStatus::Done,
        _ => {
            return Err(CommandError::InvalidArgument {
                field: "col".into(),
                value: col,
                reason: "合法值 todo/doing/done".into(),
            })
        }
    };
    let app_emit = app.clone();
    let row = async_runtime::spawn_blocking(move || {
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        let now = chrono::Utc::now().timestamp_millis();
        task_set_column_locked(&mut conn, &id, status, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库列状态线程 join 失败：{e}")))??;
    // 广播：挂件 tasks-changed 重读收敛；主窗 tasks-updated（source=main 已落盘，只合并不回写）
    {
        use tauri::Emitter;
        let _ = app_emit.emit("tasks-changed", ());
        let _ = app_emit.emit_to(
            "main",
            "tasks-updated",
            serde_json::json!({
                "source": crate::mutation::MutationOrigin::Main.as_str(),
                "upserts": [row],
                "deletes": []
            }),
        );
    }
    Ok(row)
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
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        let tx = conn
            .transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        if let Err(e) = upsert_tasks(&tx, &tasks) {
            let msg = e.to_string();
            // TP-1：RMW 写冲突审计留痕（bot.log）——「其他写者」归因证据链
            if msg.starts_with(CONFLICT_ERR_PREFIX) {
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Warn,
                    "rmw_conflict",
                    &[(
                        "detail",
                        msg.trim_start_matches(CONFLICT_ERR_PREFIX)
                            .trim_start_matches('：')
                            .to_string(),
                    )],
                );
            }
            return Err(CommandError::from(msg));
        }
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
        let _g = super::lock_db_write();
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
            reason: "必须是 .json 文件".into(),
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

/// tasks_import 读入文件大小上限（防内存 DoS：用户误选多 GB 文件时 read_to_string 直接吃满内存）
const MAX_IMPORT_BYTES: u64 = 64 * 1024 * 1024;

#[tauri::command]
pub async fn tasks_import(app: AppHandle, path: String) -> CommandResult<usize> {
    check_export_path(&path)?;
    async_runtime::spawn_blocking(move || {
        use rusqlite::OptionalExtension;
        // 上限在读侧强制（bounded reader），不做 metadata 预检——check-then-act 之间
        // 文件可被换大（symlink swap），只有限制实际读入字节数才兜底。
        // 先读字节再转 String：Take 截断可能切断 UTF-8 码点边界，直接 read_to_string
        // 会把超限文件误报成编码错误。
        let f = std::fs::File::open(&path).map_err(|e| format!("无法读取所选文件：{e}"))?;
        let mut limited = std::io::Read::take(f, MAX_IMPORT_BYTES + 1);
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut limited, &mut buf)
            .map_err(|e| format!("无法读取所选文件：{e}"))?;
        if buf.len() as u64 > MAX_IMPORT_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "path".into(),
                value: path.clone(),
                reason: format!(
                    "任务数据文件超过大小上限（{} MB）",
                    MAX_IMPORT_BYTES / (1024 * 1024)
                ),
            });
        }
        let raw = String::from_utf8(buf).map_err(|e| format!("不是有效的 UTF-8 文本：{e}"))?;
        let ext: Vec<super::Task> =
            serde_json::from_str(&raw).map_err(|e| format!("不是有效的任务数据 JSON：{e}"))?;
        if ext.is_empty() {
            return Ok(0);
        }
        let _g = super::lock_db_write();
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

#[cfg(test)]
mod task_set_column_tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, due TEXT, note TEXT, tags TEXT,
               file_path TEXT, file_is_dir INTEGER, col TEXT NOT NULL, subtasks TEXT,
               completed_at INTEGER, archived INTEGER, deleted_at INTEGER, collapsed INTEGER,
               ord REAL, updated_at INTEGER, schedule TEXT, sched_last INTEGER,
               bot_assigned INTEGER, files TEXT );",
        )
        .unwrap();
        conn
    }

    fn insert_task(conn: &rusqlite::Connection, id: &str, col: &str, bot: Option<bool>) {
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at, bot_assigned) VALUES (?1, ?2, ?3, 1000, ?4)",
            rusqlite::params![id, format!("t-{id}"), col, bot],
        )
        .unwrap();
    }

    #[test]
    fn set_column_todo_clears_completion_keeps_bot_assigned() {
        let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求（测试模块的 super 是 tasks 非 db）
        let mut conn = setup_conn();
        insert_task(&conn, "a", "done", Some(true));
        let row = task_set_column_locked(&mut conn, "a", TaskStatus::Todo, 5000).unwrap();
        assert_eq!(row.column, TaskStatus::Todo);
        assert_eq!(row.completed_at, None);
        assert_eq!(row.archived, None);
        // bot_assigned 保留（挂件 toggleDone 语义）
        assert_eq!(row.bot_assigned, Some(true));
        assert_eq!(row.updated_at, Some(5000));
        // 基线 = 锁内现读值，写后重新加载一致
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "a")
            .unwrap();
        assert_eq!(reloaded.column, TaskStatus::Todo);
        assert_eq!(reloaded.updated_at, Some(5000));
    }

    #[test]
    fn set_column_done_records_completion_clears_bot_assigned() {
        let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求（测试模块的 super 是 tasks 非 db）
        let mut conn = setup_conn();
        insert_task(&conn, "b", "todo", Some(true));
        let row = task_set_column_locked(&mut conn, "b", TaskStatus::Done, 6000).unwrap();
        assert_eq!(row.column, TaskStatus::Done);
        assert_eq!(row.completed_at, Some(6000));
        assert_eq!(row.archived, Some(false));
        // 用户完成清机器人标记（显示用户头像）
        assert_eq!(row.bot_assigned, None);
    }

    #[test]
    fn set_column_missing_id_is_task_not_found() {
        let mut conn = setup_conn();
        let err = match task_set_column_locked(&mut conn, "ghost", TaskStatus::Done, 1) {
            Err(e) => e,
            Ok(_) => panic!("missing id 应返回 TaskNotFound"),
        };
        assert!(matches!(err, CommandError::TaskNotFound(_)), "got: {err:?}");
    }
}

#[cfg(test)]
mod task_status_tests {
    use super::TaskStatus;

    #[test]
    fn as_str_matches_wire_format() {
        assert_eq!(TaskStatus::Todo.as_str(), "todo");
        assert_eq!(TaskStatus::Doing.as_str(), "doing");
        assert_eq!(TaskStatus::Done.as_str(), "done");
    }

    #[test]
    fn display_matches_as_str() {
        for s in TaskStatus::ALL {
            assert_eq!(s.to_string(), s.as_str());
        }
    }

    #[test]
    fn from_str_accepts_valid_lowercase() {
        assert_eq!("todo".parse::<TaskStatus>().unwrap(), TaskStatus::Todo);
        assert_eq!("doing".parse::<TaskStatus>().unwrap(), TaskStatus::Doing);
        assert_eq!("done".parse::<TaskStatus>().unwrap(), TaskStatus::Done);
    }

    /// wire format 是大小写敏感的裸小写字符串(前端 `ColumnId` 契约)
    #[test]
    fn from_str_rejects_invalid_and_case_variants() {
        for bad in ["", "TODO", "Todo", "doing ", " archived", "archived", "0"] {
            assert!(bad.parse::<TaskStatus>().is_err(), "{bad:?} 不应被接受");
        }
    }

    /// 锁住「与改 enum 前的 String wire format 字节级兼容」这一契约
    #[test]
    fn serde_round_trips_wire_string() {
        assert_eq!(
            serde_json::from_str::<TaskStatus>("\"doing\"").unwrap(),
            TaskStatus::Doing
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Done).unwrap(),
            "\"done\""
        );
    }

    #[test]
    fn serde_rejects_unknown_variant() {
        assert!(serde_json::from_str::<TaskStatus>("\"archived\"").is_err());
    }

    #[test]
    fn all_lists_three_variants_in_declared_order() {
        assert_eq!(
            TaskStatus::ALL,
            &[TaskStatus::Todo, TaskStatus::Doing, TaskStatus::Done]
        );
    }
}
