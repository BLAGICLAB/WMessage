//! 任务卡 CRUD：upsert / delete / load + db_* tauri command + tasks_export/import

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::async_runtime;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

// 拆出后的子模块转出：保持 `crate::db::tasks::X` 与 `crate::db::X` 两条旧路径不变
pub(crate) use super::tasks_patch::{apply_task_patch, task_patch_locked, task_set_column_locked};
pub use super::tasks_transfer::{
    check_export_path, export_tasks_json, import_tasks_conn, ExportProfileCard, TasksExportFileV2,
    TasksImportEnvelope, TasksImportPayload,
};
pub(crate) use super::tasks_transfer::{
    check_export_path_gate, read_import_bounded, MAX_IMPORT_BYTES,
};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Subtask {
    pub id: String,
    pub text: String,
    pub done: bool,
    /// 该子任务行一键生成的任务卡 id（行上跳转标记的指向）；None = 未生成。
    /// 必须随结构体落库，否则前端写入的标记会在 task_patch/db_upsert 往返中被 serde 丢弃
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    pub path: String,
    pub is_dir: bool,
}

pub const MAX_TASK_FILES: usize = 10;

/// tasks 表单源 DDL：open_db 建表用。
/// 含全部 25 列——老库缺列由 open_db 的幂等 ALTER 迁移补齐，此处即最新完整 schema。
pub const TASKS_DDL: &str = "CREATE TABLE IF NOT EXISTS tasks (
   id           TEXT PRIMARY KEY,
   title        TEXT NOT NULL,
   due          TEXT,
   note         TEXT,
   tags         TEXT,
   col          TEXT NOT NULL,
   subtasks     TEXT,
   completed_at INTEGER,
   archived     INTEGER,
   deleted_at   INTEGER,
   collapsed    INTEGER,
   ord          REAL,
   updated_at   INTEGER,
   bot_assigned INTEGER,
   files        TEXT,
   assignee     TEXT,
   result       TEXT,
   depends_on   TEXT,
   model        TEXT,
   owner_id     TEXT,
   created_at   INTEGER,
   plan_start   TEXT,
   plan_end     TEXT,
   parent_id    TEXT
 );";

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

///  新增列清单（open_db 幂等迁移与 legacy 迁移测试 fixture 共用，防两处漂移——）
pub(crate) const W1_TASK_COLUMNS: [(&str, &str); 2] = [("depends_on", "TEXT"), ("model", "TEXT")];

/// 任务归属人列（任务图谱设计 §1.1）：NULL 恒等于本人；
/// 外来任务存导入信封里的 personId，渲染层据 `ownerId == null` 过滤自己的任务。
pub(crate) const OWNER_TASK_COLUMNS: [(&str, &str); 1] = [("owner_id", "TEXT")];

/// 任务创建时间列（epoch ms）：新建时打戳，UPDATE 永不覆盖；老数据 ALTER 后该列为
/// NULL（= 未知），不回填。
pub(crate) const CREATED_AT_TASK_COLUMNS: [(&str, &str); 1] = [("created_at", "INTEGER")];

/// 计划起止列（周时间网格排期）：NULL = 未排期。
/// 格式 "YYYY-MM-DDTHH:mm"（与 due 同）；plan_end 恒 > plan_start。
pub(crate) const PLAN_TASK_COLUMNS: [(&str, &str); 2] =
    [("plan_start", "TEXT"), ("plan_end", "TEXT")];

/// 子任务卡出身列：子任务行一键生成的任务卡记母卡 id；普通卡 NULL。
/// 仅作关联键（详情面板跳转标记、母卡删除连带询问），无级联/归属语义
pub(crate) const PARENT_ID_TASK_COLUMNS: [(&str, &str); 1] = [("parent_id", "TEXT")];

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
    pub column: TaskStatus,
    pub subtasks: Option<Vec<Subtask>>,
    pub completed_at: Option<i64>,
    pub archived: Option<bool>,
    pub deleted_at: Option<i64>,
    pub collapsed: Option<bool>,
    pub order: Option<f64>,
    pub updated_at: Option<i64>,
    #[serde(default)]
    pub bot_assigned: Option<bool>,
    /// 子 agent 编排设计 §4.1）：子卡上 = 派发子 agent 的串链标识；
    /// 普通卡恒为 None。经 task_patch 通道读写（服务端编排写）。
    #[serde(default)]
    pub assignee: Option<String>,
    /// 收尾结构化结果（设计 §7 schema）；存 JSON TEXT，卡片折叠展示
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// 上游任务 id 列表（DAG 依赖）；存 JSON TEXT
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    /// 执行用大模型：模型库条目 id；None = 跟随全局 active 模型
    #[serde(default)]
    pub model: Option<String>,
    /// 归属人 personId（任务图谱设计 §1.1）：None = 本人（库内统一 NULL 存储）；
    /// 导入外来数据时由信封盖章。前端写入路径不感知（serde default），零改动兼容
    #[serde(default)]
    pub owner_id: Option<String>,
    /// 任务创建时间（epoch ms）：新建时与 updated_at 同值打戳，此后 UPDATE 永不覆盖；
    /// 老数据为 NULL（= 未知），不回填。序列化进导出信封，serde default 兼容旧信封。
    #[serde(default)]
    pub created_at: Option<i64>,
    /// 计划开始（周时间网格排期）：None = 未排期。
    /// 格式 "YYYY-MM-DDTHH:mm"（与 due 同）；task_patch 写口校验单字段格式，
    /// null = 清除排期；end > start 的跨字段一致性由 UI 成对提交保证。
    #[serde(default)]
    pub plan_start: Option<String>,
    /// 计划结束，恒 > plan_start；None = 未排期
    #[serde(default)]
    pub plan_end: Option<String>,
    /// 子任务卡出身：指向母卡 id；普通卡 None。仅作关联键，无级联语义
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing)]
    pub expected_updated_at: Option<i64>,
}

impl Task {
    pub fn effective_files(&self) -> Vec<TaskFile> {
        self.files.clone().unwrap_or_default()
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

/// 统一绑定写入口（所有绑定渠道共用内核）：文件追加制 + 文件夹槽替换制。
/// 查重主键 = 路径（小写归一——Windows 大小写差异/调用方拼写差异不产生重复绑定）；
/// 同名不同路径视为不同条目，名字不拦截。文件夹绑定替换现有文件夹槽（占原位，
/// 文件条目与列表顺序不动），同路径重复绑无操作。追加永不清除既有绑定，
/// 空追加 = 无操作——清空/减少绑定是 UI 专属操作，模型不可达。
/// 返回 (实际生效条数, 超上限溢出条数)。
pub fn upsert_files_to_task(t: &mut Task, additions: Vec<TaskFile>) -> (usize, usize) {
    let mut files = t.effective_files();
    let mut applied = 0usize;
    let mut overflow = 0usize;
    for f in additions {
        let path = f.path.trim().to_string();
        if path.is_empty() {
            continue;
        }
        let key = path.to_lowercase();
        if files.iter().any(|e| e.path.to_lowercase() == key) {
            continue; // 同路径已绑定：无操作
        }
        if f.is_dir {
            match files.iter_mut().find(|e| e.is_dir) {
                // 替换不占新名额：满员也可换槽
                Some(slot) => {
                    *slot = TaskFile { path, is_dir: true };
                    applied += 1;
                }
                None => {
                    if files.len() >= MAX_TASK_FILES {
                        overflow += 1;
                    } else {
                        files.push(TaskFile { path, is_dir: true });
                        applied += 1;
                    }
                }
            }
        } else if files.len() >= MAX_TASK_FILES {
            overflow += 1;
        } else {
            files.push(TaskFile {
                path,
                is_dir: false,
            });
            applied += 1;
        }
    }
    if applied > 0 {
        // 只增不减：applied>0 时 files 必非空，绝不写回 None
        t.files = Some(files);
    }
    (applied, overflow)
}

/// 写冲突错误前缀——RMW 基线比对失败（lost-update 防护拒写）
pub const CONFLICT_ERR_PREFIX: &str = "写冲突";

/// 「行存在性」基线哨兵：i64::MIN 保证与任何合法毫秒时间戳不撞
pub const BASELINE_NULL_ROW: i64 = i64::MIN;

/// RMW 冲突判定（tasks / scheduled_jobs / api 内存层三处共用）：expected 为
/// 行存在性哨兵时要求行仍在且 updated_at 仍为 NULL 才放行；否则要求
/// updated_at 等于基线。`cur`：None = 行不存在，Some(None) = 行在但无基线时间戳。
pub fn rmw_conflict(expected: i64, cur: Option<Option<i64>>) -> bool {
    if expected == BASELINE_NULL_ROW {
        cur != Some(None)
    } else {
        cur.flatten() != Some(expected)
    }
}

pub fn upsert_tasks(conn: &rusqlite::Connection, tasks: &[Task]) -> Result<(), String> {
    // 契约断言：调用方必须持 DB_WRITE_LOCK（经 lock_db_write() 的守卫）。
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
               (id, title, due, note, tags, col, subtasks,
                completed_at, archived, deleted_at, collapsed, ord, updated_at, bot_assigned, files,
                assignee, result, depends_on, model, owner_id, created_at,
                plan_start, plan_end, parent_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)
             ON CONFLICT(id) DO UPDATE SET
               title=excluded.title, due=excluded.due, note=excluded.note,
               tags=excluded.tags,
               col=excluded.col,
               subtasks=excluded.subtasks, completed_at=excluded.completed_at,
               archived=excluded.archived, deleted_at=excluded.deleted_at,
               collapsed=excluded.collapsed, ord=excluded.ord,
               updated_at=excluded.updated_at,
               bot_assigned=excluded.bot_assigned, files=excluded.files,
               assignee=excluded.assignee, result=excluded.result,
               depends_on=excluded.depends_on,
               model=excluded.model, owner_id=excluded.owner_id,
               plan_start=excluded.plan_start, plan_end=excluded.plan_end,
               parent_id=excluded.parent_id
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
            let conflict = rmw_conflict(expected, cur);
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
        let result = match &t.result {
            Some(v) => Some(serde_json::to_string(v).map_err(|e| e.to_string())?),
            None => None,
        };
        let depends_on = match &t.depends_on {
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
                t.column.as_str(),
                subtasks,
                t.completed_at,
                t.archived.map(|b| b as i64),
                t.deleted_at,
                t.collapsed.map(|b| b as i64),
                t.order,
                t.updated_at,
                t.bot_assigned.map(|b| b as i64),
                files,
                t.assignee,
                result,
                depends_on,
                t.model,
                t.owner_id,
                t.created_at,
                t.plan_start,
                t.plan_end,
                t.parent_id,
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

/// 行 → Task 的共享解析（load_all 全表 / load_task 单行两条读路径共用，防漂移）。
/// 损坏容错契约：col/subtasks/files/budget/result 任一解析失败 → warn + 兜底值，
/// 行仍可读（单行脏不让整表藏起来；原值未动，取证看 eprintln）。
fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<super::Task> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let due: Option<String> = row.get(2)?;
    let note: Option<String> = row.get(3)?;
    let tags: Option<String> = row.get(4)?;
    let col: String = row.get(5)?;
    let subtasks: Option<String> = row.get(6)?;
    let completed_at: Option<i64> = row.get(7)?;
    let archived: Option<i64> = row.get(8)?;
    let deleted_at: Option<i64> = row.get(9)?;
    let collapsed: Option<i64> = row.get(10)?;
    let order: Option<f64> = row.get(11)?;
    let updated_at: Option<i64> = row.get(12)?;
    let bot_assigned: Option<i64> = row.get(13)?;
    let files: Option<String> = row.get(14)?;
    let assignee: Option<String> = row.get(15)?;
    let result: Option<String> = row.get(16)?;
    let depends_on: Option<String> = row.get(17)?;
    let model: Option<String> = row.get(18)?;
    let owner_id: Option<String> = row.get(19)?;
    let created_at: Option<i64> = row.get(20)?;
    let plan_start: Option<String> = row.get(21)?;
    let plan_end: Option<String> = row.get(22)?;
    let parent_id: Option<String> = row.get(23)?;
    // col 从 DB 读出仍是 String(列类型 TEXT),parse 到 TaskStatus enum。
    // 与 subtasks/files JSON 损坏「warn + 按空读取」的契约对齐:
    // 单行 col 异常不应让整个读失败、把全部任务藏起来。
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
    // result 存 JSON TEXT；损坏按空读取（同 subtasks/files 契约）
    let result = match &result {
        Some(s) => match serde_json::from_str(s) {
            Ok(v) => Some(v),
            Err(_) => {
                eprintln!("[db] 任务 {id} 的 result JSON 损坏，按空读取（原值未动）");
                None
            }
        },
        None => None,
    };
    // depends_on 存 JSON TEXT；损坏按空读取（同 subtasks/files 契约）
    let depends_on = match &depends_on {
        Some(s) => match serde_json::from_str(s) {
            Ok(v) => Some(v),
            Err(_) => {
                eprintln!("[db] 任务 {id} 的 depends_on JSON 损坏，按空读取（原值未动）");
                None
            }
        },
        None => None,
    };
    Ok(super::Task {
        id,
        title,
        due,
        note,
        tags,
        files,
        column: col,
        subtasks,
        completed_at,
        archived: archived.map(|v| v != 0),
        deleted_at,
        collapsed: collapsed.map(|v| v != 0),
        order,
        updated_at,
        bot_assigned: bot_assigned.map(|v| v != 0),
        assignee,
        result,
        depends_on,
        model,
        owner_id,
        created_at,
        plan_start,
        plan_end,
        parent_id,
        expected_updated_at: None,
    })
}

const TASK_SELECT_COLS: &str = "SELECT id, title, due, note, tags, col, subtasks, \
     completed_at, archived, deleted_at, collapsed, ord, updated_at, \
     bot_assigned, files, assignee, result, depends_on, \
     model, owner_id, created_at, plan_start, plan_end, parent_id FROM tasks";

pub fn load_all(conn: &rusqlite::Connection) -> Result<Vec<super::Task>, String> {
    let sql = format!("{TASK_SELECT_COLS} ORDER BY ord, rowid");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], task_from_row)
        .map_err(|e| e.to_string())?;
    let mut tasks = Vec::new();
    for r in rows {
        tasks.push(r.map_err(|e| e.to_string())?);
    }
    Ok(tasks)
}

/// 单卡定点读orchestrator 的 check 进度小计用；PK 索引直查，
/// 不走 load_all 全表扫——）。
pub fn load_task(conn: &rusqlite::Connection, id: &str) -> Result<Option<super::Task>, String> {
    let sql = format!("{TASK_SELECT_COLS} WHERE id = ?1");
    conn.query_row(&sql, [id], task_from_row)
        .optional()
        .map_err(|e| e.to_string())
}

/// 行存在性定点查spawn 的 parent 卡校验用——）。
pub fn task_exists(conn: &rusqlite::Connection, id: &str) -> Result<bool, String> {
    conn.query_row("SELECT 1 FROM tasks WHERE id = ?1", [id], |_| Ok(()))
        .optional()
        .map(|o| o.is_some())
        .map_err(|e| e.to_string())
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

/// 删除非本人的任务卡（数据管理卡入口）：
/// 这些卡是从别人程序导出的统计用数据，**点删除直接硬删**，不放回收站、无二次确认，
/// 范围含回收站/归档中的非本人卡）：owner_id 非本人且非 NULL 即删
/// （owner_id NULL = 本人——任务图谱设计 §1.1 读路径归一，NULL 永不命中）。
/// 锁内单条 DELETE，随后广播 tasks-updated(source="api", deletes=[...])——
/// 后端已落盘，主窗口只合并 UI 不回写（协议防回写循环）。
#[tauri::command]
pub async fn tasks_delete_non_self(app: AppHandle) -> CommandResult<usize> {
    let (self_pid, _) = crate::profile::ensure_person_id(&app);
    let app2 = app.clone();
    let (count, deleted_ids) =
        async_runtime::spawn_blocking(move || -> CommandResult<(usize, Vec<String>)> {
            let _g = super::lock_db_write();
            let conn = super::open_db(&app2)?;
            delete_non_self_locked(&conn, &self_pid)
        })
        .await
        .map_err(|e| CommandError::from(format!("删除线程 join 失败：{e}")))??;
    if !deleted_ids.is_empty() {
        // 后端已落盘 → source="api"（App.tsx 只合并 UI 不回写）；tasks-changed 让各页重读
        use tauri::Emitter;
        let _ = app.emit("tasks-changed", ());
        let _ = app.emit(
            "tasks-updated",
            serde_json::json!({
                "source": crate::mutation::MutationOrigin::Api.as_str(),
                "upserts": [],
                "deletes": deleted_ids,
            }),
        );
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "tasks.delete_non_self",
            &[("count", count.to_string())],
        );
    }
    Ok(count)
}

/// 删除非本人的锁内段（纯 DB 逻辑，单测锚点）：硬删全部 owner_id 非本人且非 NULL 的卡
///（含回收站/归档——统计用数据一次清干净）。返回 (删除数, 被删 id 列表)。
pub(crate) fn delete_non_self_locked(
    conn: &rusqlite::Connection,
    self_pid: &str,
) -> CommandResult<(usize, Vec<String>)> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM tasks WHERE owner_id IS NOT NULL AND owner_id != ?1")
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        let rows = stmt
            .query_map([self_pid], |r| r.get::<_, String>(0))
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| CommandError::DbError(e.to_string()))?);
        }
        out
    };
    if ids.is_empty() {
        return Ok((0, ids));
    }
    let n = conn
        .execute(
            "DELETE FROM tasks WHERE owner_id IS NOT NULL AND owner_id != ?1",
            [self_pid],
        )
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok((n, ids))
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
        let now = crate::db::now_ms();
        task_set_column_locked(&mut conn, &id, status, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库列状态线程 join 失败：{e}")))??;
    // 广播：tasks-changed 让各页重读收敛；tasks-updated（source=main 已落盘，只合并不回写）
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

/// 截止时间格式校验（task_patch / bot 工具 / HTTP API 三写口共用）：
/// 两种合法形态——"YYYY-MM-DD"（仅日期，当天 23:59 截止）与
/// "YYYY-MM-DDTHH:mm"（带时刻）。与 due_notify::parse_due_dt 同一口径。
///
/// 其余一律拒：打字过程中的半截值（"2026-10"）、越界日期（2026-02-31）、
/// 越界时分（25:00）落进库里，due_notify 解析失败是「视为无截止」——
/// 静默不发通知，任务就这么无声地过期了。空白串同样不合法（清除用 null）。
pub(crate) fn due_format_error(s: &str) -> Option<&'static str> {
    let s = s.trim();
    let ok = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").is_ok()
        || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok();
    if ok {
        None
    } else {
        Some("截止时间格式必须是 YYYY-MM-DD 或 YYYY-MM-DDTHH:mm")
    }
}

/// TP-2：单任务定向补丁（通用状态变更：软删/恢复/归档/编辑/折叠/日程……）。
/// 与 task_set_column 同架构：**同一把写锁内**读现值 → 白名单逐键应用（null=清空，
/// 缺键=不动）→ 基线 = 锁内现读 → 写 → 广播。前端快照不参与。
#[tauri::command]
pub async fn task_patch(
    app: AppHandle,
    id: String,
    patch: serde_json::Value,
) -> CommandResult<super::Task> {
    let app_emit = app.clone();
    let row = async_runtime::spawn_blocking(move || {
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        let now = crate::db::now_ms();
        task_patch_locked(&mut conn, &id, &patch, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库补丁线程 join 失败：{e}")))??;
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
            return Err(upsert_failure(msg));
        }
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库 upsert 线程 join 失败：{e}")))?
}

/// upsert 失败 → CommandError。
///
/// RMW 写冲突（别的写者先改了这条）报 `TaskInvalidState`——用户重读基线后
/// 重试即可，不是内部故障；降级成 Internal 会让它 recoverable=false，前端
/// 只让用户「反馈日志」而不是给重试入口（API 侧同一情形已映射 409，语义
/// 必须一致）。其余失败报 `DbError`。
pub(crate) fn upsert_failure(msg: String) -> CommandError {
    if msg.starts_with(CONFLICT_ERR_PREFIX) {
        CommandError::TaskInvalidState { reason: msg }
    } else {
        CommandError::DbError(msg)
    }
}

#[tauri::command]
pub async fn db_delete(app: AppHandle, ids: Vec<String>) -> CommandResult<()> {
    let app_emit = app.clone();
    let ids_len = ids.len();
    async_runtime::spawn_blocking(move || {
        if ids.is_empty() {
            return Ok(());
        }
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        delete_tasks(&conn, &ids).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库删除线程 join 失败：{e}")))??;
    // 落库必广播（协议与 task_patch / autotag / tags 一致）：当前前端调用方
    // 自己补发闭环成立，但任何后端复用直调本命令都会静默漏发
    if ids_len > 0 {
        use tauri::Emitter;
        let _ = app_emit.emit("tasks-changed", ());
    }
    Ok(())
}

#[tauri::command]
pub async fn tasks_export(app: AppHandle, path: String) -> CommandResult<usize> {
    // 路径闸门：fs 调用进阻塞线程（口径见 check_export_path_gate）
    crate::db::tasks::check_export_path_gate(path.clone()).await?;
    let path_for_audit = path.clone();
    let app_audit = app.clone();
    let count = async_runtime::spawn_blocking(move || -> CommandResult<usize> {
        // 导出读全表 + people 表，拿写锁保证快照一致——避免与 task_patch /
        // 调度器并发写产生「半新半旧」的撕裂信封（导出低频，锁竞争可接受）
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        let (pid, name, avatar_id) = crate::profile::export_profile_card(&app);
        let (json, count) = export_tasks_json(&conn, &pid, &name, avatar_id.as_deref())?;
        super::paths::atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(count)
    })
    .await
    .map_err(|e| CommandError::from(format!("任务导出线程 join 失败：{e}")))?;
    // 双 ? ：外层剥 join 错，内层剥闭包自己的 CommandResult
    let count = count?;
    // 整库外流留审计：导出可把全部任务+people 写到任意过闸路径
    crate::audit::write_event(
        &app_audit,
        crate::audit::AuditLevel::Info,
        "tasks_export",
        &[
            ("count", count.to_string()),
            ("path", crate::bot::escape_for_log(&path_for_audit, 120)),
        ],
    );
    Ok(count)
}

#[tauri::command]
pub async fn tasks_import(app: AppHandle, path: String) -> CommandResult<usize> {
    // 路径闸门：fs 调用进阻塞线程（口径见 check_export_path_gate）
    crate::db::tasks::check_export_path_gate(path.clone()).await?;
    let path_for_audit = path.clone();
    let app_audit = app.clone();
    let imported = async_runtime::spawn_blocking(move || -> CommandResult<usize> {
        let raw = read_import_bounded(&path, MAX_IMPORT_BYTES)?;
        if raw.trim().is_empty() {
            return Ok(0);
        }
        // 本人 personId 先行（people 本人行落库 + 占位判定基准）
        let (self_pid, _name) = crate::profile::ensure_person_id(&app);
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        import_tasks_conn(&mut conn, &raw, &self_pid).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("任务导入线程 join 失败：{e}")))?;
    let imported = imported?;
    // 数据面变更留审计（谁何时导入了多少条）+ 落库必广播（同 db_delete 口径）
    if imported > 0 {
        crate::audit::write_event(
            &app_audit,
            crate::audit::AuditLevel::Info,
            "tasks_import",
            &[
                ("count", imported.to_string()),
                ("path", crate::bot::escape_for_log(&path_for_audit, 120)),
            ],
        );
        use tauri::Emitter;
        let _ = app_audit.emit("tasks-changed", ());
    }
    Ok(imported)
}

#[cfg(test)]
mod task_set_column_tests;

#[cfg(test)]
mod task_patch_tests;

#[cfg(test)]
mod task_status_tests;

// 任务图谱：归属与导入导出信封测试

#[cfg(test)]
mod owner_graph_tests;

#[cfg(test)]
mod import_bounded_tests;
