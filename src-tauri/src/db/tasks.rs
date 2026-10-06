//! 任务卡 CRUD：upsert / delete / load + db_* tauri command + tasks_export/import

use rusqlite::OptionalExtension;
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

/// tasks 表单源 DDL（W1-CANVAS 抽取）：open_db 建表与 db::workflow 测试共用。
/// 含全部 30 列——老库缺列由 open_db 的幂等 ALTER 迁移补齐，此处即最新完整 schema。
pub const TASKS_DDL: &str = "CREATE TABLE IF NOT EXISTS tasks (
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
   bot_assigned INTEGER,
   files        TEXT,
   assignee     TEXT,
   budget       TEXT,
   result       TEXT,
   origin       TEXT,
   workflow_id  TEXT,
   depends_on   TEXT,
   canvas_x     REAL,
   canvas_y     REAL,
   model        TEXT,
   owner_id     TEXT,
   created_at   INTEGER,
   enabled      INTEGER,
   acceptance   TEXT
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPos {
    pub x: f64,
    pub y: f64,
}

/// 工作流卡归属值（task_patch 白名单校验用）：缺省视作用户看板卡
pub const TASK_ORIGIN_USER: &str = "user";
pub const TASK_ORIGIN_WORKFLOW: &str = "workflow";

/// W1-CANVAS 新增列清单（open_db 幂等迁移与 legacy 迁移测试 fixture 共用，防两处漂移——OCR r1）
pub(crate) const W1_TASK_COLUMNS: [(&str, &str); 6] = [
    ("origin", "TEXT"),
    ("workflow_id", "TEXT"),
    ("depends_on", "TEXT"),
    ("canvas_x", "REAL"),
    ("canvas_y", "REAL"),
    ("model", "TEXT"),
];

/// 任务归属人列（任务图谱设计 §1.1）：NULL 恒等于本人；
/// 外来任务存导入信封里的 personId，渲染层据 `ownerId == null` 过滤自己的任务。
pub(crate) const OWNER_TASK_COLUMNS: [(&str, &str); 1] = [("owner_id", "TEXT")];

/// 任务创建时间列（epoch ms）：新建时打戳，UPDATE 永不覆盖（照 workflows.created_at
/// 先例）；老数据 ALTER 后该列为 NULL（= 未知），不回填。
pub(crate) const CREATED_AT_TASK_COLUMNS: [(&str, &str); 1] = [("created_at", "INTEGER")];

/// 定时启用开关列（定时任务模块）：NULL 恒等于启用；0 = 暂停（保留 schedule 配置不删）
pub(crate) const SCHED_ENABLED_TASK_COLUMNS: [(&str, &str); 1] = [("enabled", "INTEGER")];

/// 每卡验收标准列（W-QA 卡即契约）：AI 拆解生成的一行可验证完成标准；
/// 执行时注入提示词并要求对照自检。NULL = 无（旧卡/手动卡）。
pub(crate) const ACCEPTANCE_TASK_COLUMNS: [(&str, &str); 1] = [("acceptance", "TEXT")];

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
    /// 子 agent 编排（SUBA-1，设计 §4.1）：子卡上 = 派发子 agent 的串链标识；
    /// 普通卡恒为 None。经 task_patch 通道读写（服务端编排写）。
    #[serde(default)]
    pub assignee: Option<String>,
    /// 预算三硬顶（上卡可见，非隐藏参数）；存 JSON TEXT
    #[serde(default)]
    pub budget: Option<super::subagents::SubagentBudget>,
    /// 收尾结构化结果（设计 §7 schema）；存 JSON TEXT，卡片折叠展示
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    /// 工作流画布归属（W1-CANVAS，设计 §3.1）：None/"user" = 看板任务，"workflow" = 工作流节点卡
    #[serde(default)]
    pub origin: Option<String>,
    /// 所属工作流 id（origin="workflow" 时有值）
    #[serde(default)]
    pub workflow_id: Option<String>,
    /// 上游任务 id 列表（DAG 依赖 = 画布连线）；存 JSON TEXT
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
    /// 画布坐标（仅工作流卡使用）；DB 拆 canvas_x/canvas_y 两列
    #[serde(default)]
    pub canvas_pos: Option<CanvasPos>,
    /// 执行用大模型（W6-MODEL）：模型库条目 id；None = 跟随全局 active 模型
    #[serde(default)]
    pub model: Option<String>,
    /// 每卡验收标准（W-QA 卡即契约）：AI 拆解生成的一行可验证完成标准（≤120 字）；
    /// 执行时注入提示词并要求对照自检。None = 无（旧卡/手动卡）。
    #[serde(default)]
    pub acceptance: Option<String>,
    /// 归属人 personId（任务图谱设计 §1.1）：None = 本人（库内统一 NULL 存储）；
    /// 导入外来数据时由信封盖章。前端写入路径不感知（serde default），零改动兼容
    #[serde(default)]
    pub owner_id: Option<String>,
    /// 任务创建时间（epoch ms）：新建时与 updated_at 同值打戳，此后 UPDATE 永不覆盖；
    /// 老数据为 NULL（= 未知），不回填。序列化进导出信封，serde default 兼容旧信封。
    #[serde(default)]
    pub created_at: Option<i64>,
    /// 定时启用开关（定时任务模块）：None/Some(true) = 启用，Some(false) = 暂停
    /// （保留 schedule 配置不删）。读路径 None 视为启用。
    #[serde(default)]
    pub enabled: Option<bool>,
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
                completed_at, archived, deleted_at, collapsed, ord, updated_at, schedule, sched_last, bot_assigned, files,
                assignee, budget, result, origin, workflow_id, depends_on, canvas_x, canvas_y, model, owner_id, created_at, enabled, acceptance)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32)
             ON CONFLICT(id) DO UPDATE SET
               title=excluded.title, due=excluded.due, note=excluded.note,
               tags=excluded.tags, file_path=excluded.file_path,
               file_is_dir=excluded.file_is_dir, col=excluded.col,
               subtasks=excluded.subtasks, completed_at=excluded.completed_at,
               archived=excluded.archived, deleted_at=excluded.deleted_at,
               collapsed=excluded.collapsed, ord=excluded.ord,
               updated_at=excluded.updated_at,
               schedule=excluded.schedule, sched_last=excluded.sched_last,
               bot_assigned=excluded.bot_assigned, files=excluded.files,
               assignee=excluded.assignee, budget=excluded.budget, result=excluded.result,
               origin=excluded.origin, workflow_id=excluded.workflow_id,
               depends_on=excluded.depends_on,
               canvas_x=excluded.canvas_x, canvas_y=excluded.canvas_y,
               model=excluded.model, owner_id=excluded.owner_id, enabled=excluded.enabled,
               acceptance=excluded.acceptance
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
        let budget = match &t.budget {
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
                t.assignee,
                budget,
                result,
                t.origin,
                t.workflow_id,
                depends_on,
                t.canvas_pos.as_ref().map(|p| p.x),
                t.canvas_pos.as_ref().map(|p| p.y),
                t.model,
                t.owner_id,
                t.created_at,
                t.enabled.map(|b| b as i64),
                t.acceptance,
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
    let file_path: Option<String> = row.get(5)?;
    let file_is_dir: Option<i64> = row.get(6)?;
    let col: String = row.get(7)?;
    let subtasks: Option<String> = row.get(8)?;
    let completed_at: Option<i64> = row.get(9)?;
    let archived: Option<i64> = row.get(10)?;
    let deleted_at: Option<i64> = row.get(11)?;
    let collapsed: Option<i64> = row.get(12)?;
    let order: Option<f64> = row.get(13)?;
    let updated_at: Option<i64> = row.get(14)?;
    let schedule: Option<String> = row.get(15)?;
    let sched_last: Option<i64> = row.get(16)?;
    let bot_assigned: Option<i64> = row.get(17)?;
    let files: Option<String> = row.get(18)?;
    let assignee: Option<String> = row.get(19)?;
    let budget: Option<String> = row.get(20)?;
    let result: Option<String> = row.get(21)?;
    let origin: Option<String> = row.get(22)?;
    let workflow_id: Option<String> = row.get(23)?;
    let depends_on: Option<String> = row.get(24)?;
    let canvas_x: Option<f64> = row.get(25)?;
    let canvas_y: Option<f64> = row.get(26)?;
    let model: Option<String> = row.get(27)?;
    let owner_id: Option<String> = row.get(28)?;
    let created_at: Option<i64> = row.get(29)?;
    let enabled: Option<i64> = row.get(30)?;
    let acceptance: Option<String> = row.get(31)?;
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
    // SUBA-1：budget/result 存 JSON TEXT；损坏按空读取（同 subtasks/files 契约）
    let budget = match &budget {
        Some(s) => match serde_json::from_str(s) {
            Ok(v) => Some(v),
            Err(_) => {
                eprintln!("[db] 任务 {id} 的 budget JSON 损坏，按空读取（原值未动）");
                None
            }
        },
        None => None,
    };
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
    // W1-CANVAS：depends_on 存 JSON TEXT；损坏按空读取（同 subtasks/files 契约）
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
    // 画布坐标：x/y 任一缺失视为无坐标（半写入不构成合法位置）
    let canvas_pos = match (canvas_x, canvas_y) {
        (Some(x), Some(y)) => Some(CanvasPos { x, y }),
        _ => None,
    };
    Ok(super::Task {
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
        assignee,
        budget,
        result,
        origin,
        workflow_id,
        depends_on,
        canvas_pos,
        model,
        owner_id,
        created_at,
        enabled: enabled.map(|v| v != 0),
        acceptance,
        expected_updated_at: None,
    })
}

const TASK_SELECT_COLS: &str =
    "SELECT id, title, due, note, tags, file_path, file_is_dir, col, subtasks, \
     completed_at, archived, deleted_at, collapsed, ord, updated_at, schedule, sched_last, \
     bot_assigned, files, assignee, budget, result, origin, workflow_id, depends_on, \
     canvas_x, canvas_y, model, owner_id, created_at, enabled, acceptance FROM tasks";

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

/// 单卡定点读（SUBA-1：orchestrator 的 check 进度小计用；PK 索引直查，
/// 不走 load_all 全表扫——OCR r2 采纳）。
pub fn load_task(conn: &rusqlite::Connection, id: &str) -> Result<Option<super::Task>, String> {
    let sql = format!("{TASK_SELECT_COLS} WHERE id = ?1");
    conn.query_row(&sql, [id], task_from_row)
        .optional()
        .map_err(|e| e.to_string())
}

/// 工作流的节点卡定点读（W1-CANVAS：指纹 diff 保存用）。
/// 只取未软删的行——回收站里的旧节点卡视为已消失，不参与指纹匹配。
pub fn load_tasks_by_workflow(
    conn: &rusqlite::Connection,
    workflow_id: &str,
) -> Result<Vec<super::Task>, String> {
    let sql = format!("{TASK_SELECT_COLS} WHERE workflow_id = ?1 AND deleted_at IS NULL");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([workflow_id], task_from_row)
        .map_err(|e| e.to_string())?;
    let mut tasks = Vec::new();
    for r in rows {
        tasks.push(r.map_err(|e| e.to_string())?);
    }
    Ok(tasks)
}

/// 行存在性定点查（SUBA-1：spawn 的 parent 卡校验用——OCR r2 采纳）。
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

/// 删除非本人的任务卡（2026-10-06 老板需求，数据管理卡入口；同日拍板简化——
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
        // 后端已落盘 → source="api"（App.tsx 只合并 UI 不回写）；tasks-changed 让挂件重读
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

/// TP-2：task_patch 的字段白名单应用器（纯逻辑，单测锚点）。
/// **null = 清空**、缺键 = 不动；未知键/受保护键/空标题 → InvalidArgument 响亮失败。
/// 受保护字段：id（主键）、updatedAt/expectedUpdatedAt（服务端统一打戳，前端快照不许带）、
/// createdAt（创建时间不可篡改）。
pub(crate) fn apply_task_patch(
    task: &mut super::Task,
    patch: &serde_json::Value,
) -> CommandResult<()> {
    let obj = patch
        .as_object()
        .ok_or_else(|| CommandError::InvalidArgument {
            field: "patch".into(),
            value: "非对象".into(),
            reason: "patch 必须是 JSON 对象".into(),
        })?;
    fn set_from<T: serde::de::DeserializeOwned>(
        slot: &mut T,
        v: &serde_json::Value,
        field: &str,
    ) -> CommandResult<()> {
        *slot = serde_json::from_value(v.clone()).map_err(|e| CommandError::InvalidArgument {
            field: field.into(),
            value: v.to_string(),
            reason: format!("类型不匹配: {e}"),
        })?;
        Ok(())
    }
    for (k, v) in obj {
        match k.as_str() {
            // TaskStatus 的自定义 Deserialize 只支持借用字符串，from_value 走不通——
            // 特判经 FromStr 解析（wire = todo/doing/done）
            "column" => {
                let s = v.as_str().ok_or_else(|| CommandError::InvalidArgument {
                    field: k.into(),
                    value: v.to_string(),
                    reason: "column 必须是字符串".into(),
                })?;
                task.column = <TaskStatus as std::str::FromStr>::from_str(s).map_err(|e| {
                    CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: e,
                    }
                })?;
            }
            "completedAt" => set_from(&mut task.completed_at, v, k)?,
            "archived" => set_from(&mut task.archived, v, k)?,
            "deletedAt" => set_from(&mut task.deleted_at, v, k)?,
            "collapsed" => set_from(&mut task.collapsed, v, k)?,
            "note" => set_from(&mut task.note, v, k)?,
            "tags" => set_from(&mut task.tags, v, k)?,
            "subtasks" => set_from(&mut task.subtasks, v, k)?,
            "due" => set_from(&mut task.due, v, k)?,
            "files" => set_from(&mut task.files, v, k)?,
            "filePath" => set_from(&mut task.file_path, v, k)?,
            "fileIsDir" => set_from(&mut task.file_is_dir, v, k)?,
            "order" => set_from(&mut task.order, v, k)?,
            "schedule" => set_from(&mut task.schedule, v, k)?,
            "schedLast" => set_from(&mut task.sched_last, v, k)?,
            "botAssigned" => set_from(&mut task.bot_assigned, v, k)?,
            // 定时启用开关（定时任务模块）：null/缺省 = 启用
            "enabled" => set_from(&mut task.enabled, v, k)?,
            // SUBA-1（设计 §4.1）：子 agent 编排三字段走 task_patch 既有通道；
            // null = 清空。assignee/budget/result 由服务端编排写，前端仅投影展示。
            // budget 落库前必须过 clamped()——硬顶契约在写口强制，防 task_patch
            // 旁路 maxTurns（OCR r1 high 采纳）。assignee 拒绝空串（它是串链
            // subagents 表的 join 键，空串会产生孤儿指向——OCR r2 采纳）。
            "assignee" => {
                set_from(&mut task.assignee, v, k)?;
                if task
                    .assignee
                    .as_deref()
                    .is_some_and(|s| s.trim().is_empty())
                {
                    return Err(CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: "assignee 不能为空串（子 agent 串链键）".into(),
                    });
                }
            }
            "budget" => {
                set_from(&mut task.budget, v, k)?;
                task.budget = task.budget.take().map(|b| b.clamped());
            }
            "result" => set_from(&mut task.result, v, k)?,
            // W1-CANVAS（设计 §3.1）：工作流四字段走 task_patch 既有通道，null = 清空。
            // origin 限枚举值；workflowId 拒空串（它和 workflows 表的 join 键，同 assignee 理由）
            "origin" => {
                set_from(&mut task.origin, v, k)?;
                if let Some(o) = &task.origin {
                    if o != TASK_ORIGIN_USER && o != TASK_ORIGIN_WORKFLOW {
                        return Err(CommandError::InvalidArgument {
                            field: k.into(),
                            value: v.to_string(),
                            reason: format!(
                                "origin 只能是 {TASK_ORIGIN_USER}/{TASK_ORIGIN_WORKFLOW}"
                            ),
                        });
                    }
                    if o == TASK_ORIGIN_USER {
                        // "user" 归一为 None（缺省语义），避免两种写法表示同一状态
                        task.origin = None;
                    }
                }
            }
            "workflowId" => {
                set_from(&mut task.workflow_id, v, k)?;
                if task
                    .workflow_id
                    .as_deref()
                    .is_some_and(|s| s.trim().is_empty())
                {
                    return Err(CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: "workflowId 不能为空串（workflows 表 join 键）".into(),
                    });
                }
            }
            "dependsOn" => {
                set_from(&mut task.depends_on, v, k)?;
                if let Some(deps) = &task.depends_on {
                    if deps.iter().any(|d| d.trim().is_empty()) {
                        return Err(CommandError::InvalidArgument {
                            field: k.into(),
                            value: v.to_string(),
                            reason: "dependsOn 元素不能为空串".into(),
                        });
                    }
                }
            }
            "canvasPos" => set_from(&mut task.canvas_pos, v, k)?,
            "model" => {
                set_from(&mut task.model, v, k)?;
                if task.model.as_deref().is_some_and(|s| s.trim().is_empty()) {
                    return Err(CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: "model 不能为空串（清空用 null）".into(),
                    });
                }
            }
            "title" => {
                let t: String = serde_json::from_value(v.clone()).map_err(|e| {
                    CommandError::InvalidArgument {
                        field: k.into(),
                        value: v.to_string(),
                        reason: format!("类型不匹配: {e}"),
                    }
                })?;
                if t.trim().is_empty() {
                    return Err(CommandError::InvalidArgument {
                        field: "title".into(),
                        value: v.to_string(),
                        reason: "标题不能为空".into(),
                    });
                }
                task.title = t;
            }
            "id" | "updatedAt" | "expectedUpdatedAt" | "createdAt" => {
                return Err(CommandError::InvalidArgument {
                    field: k.into(),
                    value: v.to_string(),
                    reason: "受保护字段：id/updated_at/created_at 由服务端管理，不可经 patch 修改"
                        .into(),
                })
            }
            other => {
                return Err(CommandError::InvalidArgument {
                    field: other.into(),
                    value: v.to_string(),
                    reason: "未知 patch 字段".into(),
                })
            }
        }
    }
    Ok(())
}

/// task_patch 的锁内段（纯 DB 逻辑，单测锚点）：读现值 → 白名单应用 →
/// 基线 = 锁内现读 updated_at → 服务端打戳 → 写。
pub(crate) fn task_patch_locked(
    conn: &mut rusqlite::Connection,
    id: &str,
    patch: &serde_json::Value,
    now: i64,
) -> CommandResult<super::Task> {
    let mut task = load_all(conn)?
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| CommandError::TaskNotFound(id.to_string()))?;
    apply_task_patch(&mut task, patch)?;
    task.expected_updated_at = task.updated_at;
    task.updated_at = Some(now);
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    upsert_tasks(&tx, std::slice::from_ref(&task)).map_err(CommandError::from)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(task)
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
        let now = chrono::Utc::now().timestamp_millis();
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

/// TP-3：批量排序条目（ord-only 写）
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderItem {
    pub id: String,
    pub order: f64,
}

/// task_reorder 的锁内段（纯 DB 逻辑，单测锚点）：逐条读现值 → **只改 order**
/// （内容/updated_at 零改动）→ 基线 = 锁内现读（等值通过）→ 缺失行跳过。
pub(crate) fn task_reorder_locked(
    conn: &mut rusqlite::Connection,
    items: &[ReorderItem],
) -> CommandResult<Vec<super::Task>> {
    let all = load_all(conn)?;
    let mut out = Vec::with_capacity(items.len());
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    for it in items {
        if let Some(mut task) = all.iter().find(|t| t.id == it.id).cloned() {
            task.expected_updated_at = task.updated_at;
            task.order = Some(it.order);
            upsert_tasks(&tx, std::slice::from_ref(&task)).map_err(CommandError::from)?;
            out.push(task);
        }
    }
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(out)
}

/// TP-3：批量排序（拖拽/列内重排专用）。**只改 order，内容与 updated_at 零改动**——
/// RMW 守卫防的是「旧内容压新内容」，排序写不携带内容，ord-only 对任何并发写者
/// 天然安全；缺失行（他端已删）跳过不报错。广播同 TP-1/2。
#[tauri::command]
pub async fn task_reorder(
    app: AppHandle,
    items: Vec<ReorderItem>,
) -> CommandResult<Vec<super::Task>> {
    let app_emit = app.clone();
    let rows = async_runtime::spawn_blocking(move || {
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        task_reorder_locked(&mut conn, &items)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库排序线程 join 失败：{e}")))??;
    {
        use tauri::Emitter;
        let _ = app_emit.emit("tasks-changed", ());
        let _ = app_emit.emit_to(
            "main",
            "tasks-updated",
            serde_json::json!({
                "source": crate::mutation::MutationOrigin::Main.as_str(),
                "upserts": rows,
                "deletes": []
            }),
        );
    }
    Ok(rows)
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

/// 导出信封 v2（任务图谱设计 §1.4）：本人资料卡 + 已知成员 + 全量任务（owner 已盖章）
/// 同时是导入信封的 profile 字段类型（往返同构）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportProfileCard {
    pub person_id: String,
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksExportFileV2 {
    pub version: u8,
    pub exported_at: i64,
    pub profile: ExportProfileCard,
    pub people: Vec<super::PeopleCard>,
    pub tasks: Vec<Task>,
}

/// 导入载荷：untagged 按序尝试——对象 = v2 信封，数组 = v1 裸任务（旧版导出永久可导）
#[derive(Deserialize)]
#[serde(untagged)]
pub enum TasksImportPayload {
    V2(Box<TasksImportEnvelope>),
    V1(Vec<Task>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksImportEnvelope {
    #[serde(default)]
    pub version: Option<u8>,
    #[serde(default)]
    pub profile: Option<ExportProfileCard>,
    #[serde(default)]
    pub people: Vec<super::PeopleCard>,
    #[serde(default)]
    pub tasks: Vec<Task>,
}

/// 组装 v2 导出 JSON（纯逻辑，单测锚点）：NULL owner 盖章为本人 pid；
/// people = people 表已知成员（除本人）+ 任务里引用但未注册的占位成员
pub fn export_tasks_json(
    conn: &rusqlite::Connection,
    self_pid: &str,
    self_name: &str,
) -> Result<(String, usize), String> {
    let mut tasks = load_all(conn)?;
    for t in tasks.iter_mut() {
        if t.owner_id
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            t.owner_id = Some(self_pid.to_string());
        }
    }
    let mut people: Vec<super::PeopleCard> = super::people::people_load(conn)?
        .into_iter()
        .filter(|p| p.id != self_pid)
        .map(|p| super::PeopleCard {
            id: p.id,
            name: p.name,
        })
        .collect();
    let mut seen: std::collections::HashSet<String> = people.iter().map(|p| p.id.clone()).collect();
    for t in &tasks {
        if let Some(pid) = t
            .owner_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if pid != self_pid && seen.insert(pid.to_string()) {
                people.push(super::PeopleCard {
                    id: pid.to_string(),
                    name: "未知成员".into(),
                });
            }
        }
    }
    let count = tasks.len();
    let file = TasksExportFileV2 {
        version: 2,
        exported_at: chrono::Utc::now().timestamp_millis(),
        profile: ExportProfileCard {
            person_id: self_pid.to_string(),
            name: self_name.to_string(),
        },
        people,
        tasks,
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    Ok((json, count))
}

/// 归属归一（任务图谱设计 §1.3）：有效 pid = 任务自带 ownerId 非空者，
/// 否则信封本人 pid；等于自己 → 库内 NULL，否则 Some(pid)
fn normalize_owner(t: &Task, envelope_pid: Option<&str>, self_pid: &str) -> Option<String> {
    let eff = t
        .owner_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| envelope_pid.map(str::to_string))
        .unwrap_or_else(|| self_pid.to_string());
    if eff == self_pid {
        None
    } else {
        Some(eff)
    }
}

/// 导入核心（纯逻辑 + 事务，单测锚点）：people 与 tasks 同事务写入；
/// 归并规则不变（按 id，同 id 取 updatedAt 更晚者）。调用方须持 DB_WRITE_LOCK。
pub fn import_tasks_conn(
    conn: &mut rusqlite::Connection,
    raw: &str,
    self_pid: &str,
) -> Result<usize, String> {
    let payload: TasksImportPayload =
        serde_json::from_str(raw).map_err(|e| format!("不是有效的任务数据 JSON：{e}"))?;
    let (tasks, envelope, people_cards) = match payload {
        TasksImportPayload::V1(v) => (v, None, Vec::new()),
        TasksImportPayload::V2(env) => {
            if let Some(v) = env.version {
                if v != 2 {
                    return Err(format!("不支持的导出格式版本：{v}（本应用支持 1/2）"));
                }
            }
            let profile = env.profile.ok_or_else(|| {
                "信封缺少 profile 资料卡，无法确定任务归属（文件可能被手改损坏）".to_string()
            })?;
            let card = (profile.person_id, profile.name);
            (env.tasks, Some(card), env.people)
        }
    };
    // 本人 personId 若为空（防御）——信封 pid 也不该等于空串
    if self_pid.trim().is_empty() {
        return Err("本人 personId 未初始化".into());
    }
    let envelope_pid = envelope.as_ref().map(|(pid, _)| pid.clone());
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    // 成员注册表先行落库（任务图谱设计 §1.4）：信封本人卡（= 对方；若对方 pid 恰为
    // 本人——导自己的旧信封——is_self 只升不降语义保住本人行）+ 信封成员卡
    if let Some((pid, name)) = &envelope {
        super::people::people_upsert_entry(&tx, pid, name, false)?;
    }
    super::people::people_upsert_cards(&tx, &people_cards)?;
    // 任务引用但信封未携带资料的归属 → 占位行，图谱不出现悬空归属
    for pid in tasks
        .iter()
        .filter_map(|t| normalize_owner(t, envelope_pid.as_deref(), self_pid))
    {
        super::people::people_ensure_placeholder(&tx, &pid)?;
    }
    let mut merged = 0usize;
    for t in &tasks {
        if t.id.trim().is_empty() {
            continue;
        }
        let mut t2 = t.clone();
        t2.owner_id = normalize_owner(t, envelope_pid.as_deref(), self_pid);
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
            upsert_tasks(&tx, std::slice::from_ref(&t2))?;
            merged += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(merged)
}

#[tauri::command]
pub async fn tasks_export(app: AppHandle, path: String) -> CommandResult<usize> {
    check_export_path(&path)?;
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        let (pid, name) = crate::profile::export_profile_card(&app);
        let (json, count) = export_tasks_json(&conn, &pid, &name)?;
        super::paths::atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(count)
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
    .map_err(|e| CommandError::from(format!("任务导入线程 join 失败：{e}")))?
}

#[cfg(test)]
mod task_set_column_tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(TASKS_DDL).unwrap();
        conn
    }

    fn insert_task(conn: &rusqlite::Connection, id: &str, col: &str, bot: Option<bool>) {
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at, bot_assigned) VALUES (?1, ?2, ?3, 1000, ?4)",
            rusqlite::params![id, format!("t-{id}"), col, bot],
        )
        .unwrap();
    }

    fn insert_owned_task(conn: &rusqlite::Connection, id: &str, owner: Option<&str>) {
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at, owner_id) VALUES (?1, ?2, 'todo', 1000, ?3)",
            rusqlite::params![id, format!("t-{id}"), owner],
        )
        .unwrap();
    }

    // ── 删除非本人任务卡（数据管理卡入口，2026-10-06；硬删含回收站） ──

    /// 筛选口径：owner 非本人且非 NULL 的卡全部硬删（含回收站/归档——统计用数据
    /// 一次清干净）；本人卡与 NULL 归属卡一概不动
    #[test]
    fn delete_non_self_filters_owner_hard_deletes() {
        let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求
        let mut conn = setup_conn();
        insert_owned_task(&conn, "mine", Some("self-pid"));
        insert_owned_task(&conn, "null-owner", None); // NULL = 本人（图谱设计 §1.1）
        insert_owned_task(&conn, "other-1", Some("someone-else"));
        insert_owned_task(&conn, "other-2", Some("another"));
        // 回收站里的非本人卡：统计用数据一次清干净，同样硬删
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at, owner_id, deleted_at) VALUES ('other-trashed', 't', 'todo', 1000, 'someone-else', 900)",
            [],
        )
        .unwrap();

        let (count, ids) = delete_non_self_locked(&conn, "self-pid").unwrap();
        assert_eq!(count, 3, "other-1/other-2/other-trashed 全删");
        assert_eq!(ids.len(), 3);

        let remaining: Vec<String> = load_all(&conn)
            .unwrap()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        assert!(remaining.contains(&"mine".to_string()), "本人卡不动");
        assert!(
            remaining.contains(&"null-owner".to_string()),
            "NULL 归属 = 本人，不动"
        );
        assert!(
            !remaining.iter().any(|i| i.starts_with("other")),
            "非本人卡已硬删"
        );
    }

    /// 无命中空转返回 0
    #[test]
    fn delete_non_self_noop() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        insert_owned_task(&conn, "mine", Some("self-pid"));
        insert_owned_task(&conn, "null-owner", None);
        let (count, ids) = delete_non_self_locked(&conn, "self-pid").unwrap();
        assert_eq!((count, ids.len()), (0, 0));
        assert_eq!(load_all(&conn).unwrap().len(), 2);
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
mod task_patch_tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(TASKS_DDL).unwrap();
        conn
    }

    fn run_patch(
        conn: &mut rusqlite::Connection,
        id: &str,
        patch: serde_json::Value,
    ) -> CommandResult<super::Task> {
        let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求
        task_patch_locked(conn, id, &patch, 9000)
    }

    #[test]
    fn task_patch_applies_fields_and_null_clears() {
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, completed_at, archived, schedule, sched_last, updated_at)
             VALUES ('a', '任务A', 'done', 111, 1, 'daily:09:00', 222, 1000)",
            [],
        )
        .unwrap();
        // 归档恢复语义：archived=false + completedAt 重置为 now（服务端打戳）
        let row = run_patch(&mut conn, "a", serde_json::json!({"archived": false})).unwrap();
        assert_eq!(row.archived, Some(false));
        assert_eq!(row.updated_at, Some(9000));
        // null = 清空：deletedAt/schedule/schedLast（软删恢复/清调度语义）
        let row = run_patch(
            &mut conn,
            "a",
            serde_json::json!({"deletedAt": 123, "schedule": null, "schedLast": null, "column": "todo"}),
        )
        .unwrap();
        assert_eq!(row.deleted_at, Some(123));
        assert_eq!(row.schedule, None);
        assert_eq!(row.sched_last, None);
        assert_eq!(row.column, TaskStatus::Todo);
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "a")
            .unwrap();
        assert_eq!(reloaded.schedule, None);
        assert_eq!(reloaded.deleted_at, Some(123));
    }

    #[test]
    fn task_patch_rejects_unknown_and_protected_keys() {
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at) VALUES ('b', '任务B', 'todo', 1000)",
            [],
        )
        .unwrap();
        for bad in ["foo", "id", "updatedAt", "expectedUpdatedAt"] {
            let err = match run_patch(&mut conn, "b", serde_json::json!({ bad: 1 })) {
                Err(e) => e,
                Ok(_) => panic!("{bad} 应被拒绝"),
            };
            assert!(
                matches!(err, CommandError::InvalidArgument { .. }),
                "{bad} 应 InvalidArgument，got {err:?}"
            );
        }
    }

    #[test]
    fn task_patch_rejects_empty_title_and_missing_id() {
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at) VALUES ('c', '任务C', 'todo', 1000)",
            [],
        )
        .unwrap();
        assert!(matches!(
            run_patch(&mut conn, "c", serde_json::json!({"title": "  "})),
            Err(CommandError::InvalidArgument { .. })
        ));
        assert!(matches!(
            run_patch(&mut conn, "ghost", serde_json::json!({"title": "x"})),
            Err(CommandError::TaskNotFound(_))
        ));
    }

    /// SUBA-1：子 agent 编排三字段（assignee/budget/result）——设值、null 清空、
    /// 往返落库一致（JSON TEXT 序列化）。
    #[test]
    fn task_patch_orchestration_fields_set_and_null_clears() {
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at) VALUES ('suba', '子任务卡', 'doing', 1000)",
            [],
        )
        .unwrap();
        let row = run_patch(
            &mut conn,
            "suba",
            serde_json::json!({
                "assignee": "sess-child-1",
                "budget": {"maxTurns": 30, "maxToolCalls": 100, "maxWallSeconds": 600},
                "result": {"status": "succeeded", "summary": "结论"}
            }),
        )
        .unwrap();
        assert_eq!(row.assignee.as_deref(), Some("sess-child-1"));
        assert_eq!(row.budget.as_ref().unwrap().max_turns, 30);
        assert_eq!(row.result.as_ref().unwrap()["status"], "succeeded");
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "suba")
            .unwrap();
        assert_eq!(reloaded.assignee.as_deref(), Some("sess-child-1"));
        // 往返后按值复核（不只 is_some）：JSON TEXT 序列化无损
        let rb = reloaded.budget.as_ref().unwrap();
        assert_eq!(rb.max_turns, 30);
        assert_eq!(rb.max_tool_calls, 100);
        assert_eq!(rb.max_wall_seconds, 600);
        assert_eq!(reloaded.result.as_ref().unwrap()["summary"], "结论");
        // null = 清空三字段
        let cleared = run_patch(
            &mut conn,
            "suba",
            serde_json::json!({"assignee": null, "budget": null, "result": null}),
        )
        .unwrap();
        assert_eq!(cleared.assignee, None);
        assert_eq!(cleared.budget, None);
        assert_eq!(cleared.result, None);
    }

    /// OCR r1 high 采纳：budget 走 task_patch 也必须被硬顶钳制（写口强制，防旁路）
    #[test]
    fn task_patch_budget_is_clamped_to_hard_cap() {
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at) VALUES ('clamp', '预算钳制', 'doing', 1000)",
            [],
        )
        .unwrap();
        let row = run_patch(
            &mut conn,
            "clamp",
            serde_json::json!({"budget": {"maxTurns": 9999, "maxToolCalls": 0, "maxWallSeconds": 0}}),
        )
        .unwrap();
        let b = row.budget.as_ref().unwrap();
        assert_eq!(b.max_turns, 50, "上超钳到硬顶");
        assert_eq!(b.max_tool_calls, 1, "下超钳到下限");
        assert_eq!(b.max_wall_seconds, 1, "下超钳到下限");
    }

    /// OCR r1 采纳：budget/result 列损坏 → 兜底 None + 行仍可读（同 subtasks/files 契约）
    #[test]
    fn load_all_tolerates_corrupted_budget_result_json() {
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at) VALUES ('bad', '损坏行', 'doing', 1000)",
            [],
        )
        .unwrap();
        run_patch(
            &mut conn,
            "bad",
            serde_json::json!({"assignee": "keep-me", "budget": {"maxTurns": 10, "maxToolCalls": 10, "maxWallSeconds": 60}}),
        )
        .unwrap();
        conn.execute("UPDATE tasks SET result = '{not-json' WHERE id = 'bad'", [])
            .unwrap();
        let row = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "bad")
            .unwrap();
        assert_eq!(row.result, None, "损坏 result 必须兜底 None");
        assert_eq!(
            row.assignee.as_deref(),
            Some("keep-me"),
            "同行其他字段不受影响"
        );
        assert!(
            row.budget.is_some(),
            "完好的 budget 不受同行的损坏 result 影响"
        );
    }
}

#[cfg(test)]
mod task_reorder_tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(TASKS_DDL).unwrap();
        conn
    }

    #[test]
    fn task_reorder_sets_ord_only_keeps_content_and_updated_at() {
        let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, note, updated_at, ord) VALUES ('a', '任务A', 'todo', '备注', 1000, 1.0)",
            [],
        )
        .unwrap();
        let rows = task_reorder_locked(
            &mut conn,
            &[ReorderItem {
                id: "a".into(),
                order: 7.5,
            }],
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        // 只动 order：内容与 updated_at 零改动
        assert_eq!(rows[0].order, Some(7.5));
        assert_eq!(rows[0].note.as_deref(), Some("备注"));
        assert_eq!(rows[0].updated_at, Some(1000));
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "a")
            .unwrap();
        assert_eq!(reloaded.order, Some(7.5));
        assert_eq!(reloaded.updated_at, Some(1000));
    }

    #[test]
    fn task_reorder_skips_missing_rows_and_supports_batch() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        conn.execute(
            "INSERT INTO tasks (id, title, col, updated_at, ord) VALUES ('a', '任务A', 'todo', 1000, 1.0)",
            [],
        )
        .unwrap();
        let rows = task_reorder_locked(
            &mut conn,
            &[
                ReorderItem {
                    id: "ghost".into(),
                    order: 0.0,
                },
                ReorderItem {
                    id: "a".into(),
                    order: 2.0,
                },
            ],
        )
        .unwrap();
        // 缺失行跳过（他端已删不报错），存在的行照常更新
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a");
        assert_eq!(rows[0].order, Some(2.0));
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

// ──────────────── 任务图谱：归属与导入导出信封测试 ────────────────

#[cfg(test)]
mod owner_graph_tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(TASKS_DDL).unwrap();
        conn.execute_batch(super::super::people::PEOPLE_DDL)
            .unwrap();
        conn
    }

    const SELF: &str = "self-pid";

    fn task_json(id: &str, owner: Option<&str>, updated: i64) -> String {
        let owner_part = match owner {
            Some(o) => format!(r#""ownerId":"{o}","#),
            None => String::new(),
        };
        format!(
            r#"{{"id":"{id}","title":"t-{id}","column":"todo",{owner_part}"updatedAt":{updated}}}"#
        )
    }

    /// 最小合法 Task 构造（全 None 缺省）
    fn min_task(owner: Option<&str>) -> Task {
        Task {
            acceptance: None,
            id: "x".into(),
            title: "t".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: None,
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            origin: None,
            workflow_id: None,
            depends_on: None,
            canvas_pos: None,
            model: None,
            owner_id: owner.map(str::to_string),
            created_at: None,
            enabled: None,
            expected_updated_at: None,
        }
    }

    #[test]
    fn normalize_owner_rules() {
        // 显式外来 pid → Some
        assert_eq!(
            normalize_owner(&min_task(Some("p-a")), Some("env-pid"), SELF),
            Some("p-a".into())
        );
        // 显式本人 pid → None（库内 NULL = 本人）
        assert_eq!(
            normalize_owner(&min_task(Some(SELF)), Some("env"), SELF),
            None
        );
        // 空/空白 ownerId → 回退信封 pid
        assert_eq!(
            normalize_owner(&min_task(Some("  ")), Some("env"), SELF),
            Some("env".into())
        );
        // 信封也是本人 → None
        assert_eq!(normalize_owner(&min_task(Some("")), Some(SELF), SELF), None);
        // 无 ownerId → 信封 pid；信封缺失 → 本人 → None
        assert_eq!(
            normalize_owner(&min_task(None), Some("env"), SELF),
            Some("env".into())
        );
        assert_eq!(normalize_owner(&min_task(None), None, SELF), None);
    }

    #[test]
    fn import_v1_bare_array_treated_as_self() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        let raw = format!(
            "[{},{}]",
            task_json("a", None, 100),
            task_json("b", Some(SELF), 100)
        );
        let merged = import_tasks_conn(&mut conn, &raw, SELF).unwrap();
        assert_eq!(merged, 2);
        for t in load_all(&conn).unwrap() {
            assert_eq!(t.owner_id, None, "v1 裸数组全部归属本人");
        }
    }

    #[test]
    fn import_v2_stamps_foreign_owner_and_upserts_people() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        let raw = format!(
            r#"{{"version":2,"exportedAt":1,
                "profile":{{"personId":"zhang","name":"张三"}},
                "people":[{{"id":"li","name":"李四"}}],
                "tasks":[{},{},{}]}}"#,
            task_json("t1", None, 100),          // 张三的卡 → owner=zhang
            task_json("t2", Some("li"), 100),    // 张三库里李四的卡 → owner=li
            task_json("t3", Some("ghost"), 100), // 未注册归属 → 占位行
        );
        import_tasks_conn(&mut conn, &raw, SELF).unwrap();
        let by_id: std::collections::HashMap<String, Task> = load_all(&conn)
            .unwrap()
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        assert_eq!(by_id["t1"].owner_id.as_deref(), Some("zhang"));
        assert_eq!(by_id["t2"].owner_id.as_deref(), Some("li"));
        assert_eq!(by_id["t3"].owner_id.as_deref(), Some("ghost"));
        let names: std::collections::HashMap<String, String> =
            super::super::people::people_load(&conn)
                .unwrap()
                .into_iter()
                .map(|p| (p.id, p.name))
                .collect();
        assert_eq!(names["zhang"], "张三");
        assert_eq!(names["li"], "李四");
        assert_eq!(names["ghost"], "未知成员");
    }

    #[test]
    fn import_v2_version_mismatch_and_missing_profile_rejected() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        let bad_ver = r#"{"version":3,"profile":{"personId":"p","name":"n"},"tasks":[]}"#;
        assert!(import_tasks_conn(&mut conn, bad_ver, SELF).is_err());
        let no_profile = r#"{"version":2,"tasks":[]}"#;
        assert!(import_tasks_conn(&mut conn, no_profile, SELF).is_err());
    }

    #[test]
    fn import_garbage_rejected_with_readable_error() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        let err = import_tasks_conn(&mut conn, "这不是 JSON", SELF).unwrap_err();
        assert!(err.contains("不是有效的任务数据 JSON"), "{err}");
    }

    #[test]
    fn export_stamps_self_owner_and_carries_people() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        // 本人的卡（NULL owner）+ 已知外来卡（people 表有注册）
        let raw = format!(
            r#"{{"version":2,"profile":{{"personId":"zhang","name":"张三"}},"tasks":[{}]}}"#,
            task_json("foreign", None, 100),
        );
        import_tasks_conn(&mut conn, &raw, SELF).unwrap();
        super::super::people::people_upsert_entry(&conn, "zhang", "张三", false).unwrap();
        let mut mine = min_task(None);
        mine.id = "mine".into();
        mine.title = "我的卡".into();
        mine.updated_at = Some(100);
        upsert_tasks(&conn, &[mine]).unwrap();
        let (json, count) = export_tasks_json(&conn, SELF, "我").unwrap();
        assert_eq!(count, 2);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["profile"]["personId"], SELF);
        let tasks = v["tasks"].as_array().unwrap();
        let mine = tasks.iter().find(|t| t["id"] == "mine").unwrap();
        assert_eq!(mine["ownerId"], SELF, "NULL owner 导出时盖章为本人 pid");
        let foreign = tasks.iter().find(|t| t["id"] == "foreign").unwrap();
        assert_eq!(foreign["ownerId"], "zhang");
        let people = v["people"].as_array().unwrap();
        assert!(
            people
                .iter()
                .any(|p| p["id"] == "zhang" && p["name"] == "张三"),
            "people 应携带已知成员；实际 {people:?}"
        );
        assert!(
            !people.iter().any(|p| p["id"] == SELF),
            "people 不含本人（本人在 profile 里）"
        );
    }

    #[test]
    fn import_then_export_roundtrip_preserves_attribution() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        let raw = format!(
            r#"{{"version":2,"profile":{{"personId":"zhang","name":"张三"}},"tasks":[{}]}}"#,
            task_json("t1", None, 100),
        );
        import_tasks_conn(&mut conn, &raw, SELF).unwrap();
        let (json, _) = export_tasks_json(&conn, SELF, "我").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["tasks"][0]["ownerId"], "zhang", "往返后归属不串档");
    }

    #[test]
    fn import_same_id_takes_later_updated_at_with_owner() {
        let _g = crate::db::lock_db_write();
        let mut conn = setup_conn();
        // 先导旧版（本人、ua=100），再导新版同 id（张三、ua=200）→ 更新胜出且换归属
        let raw1 = format!("[{}]", task_json("dup", None, 100));
        import_tasks_conn(&mut conn, &raw1, SELF).unwrap();
        let raw2 = format!(
            r#"{{"version":2,"profile":{{"personId":"zhang","name":"张三"}},"tasks":[{}]}}"#,
            task_json("dup", None, 200),
        );
        import_tasks_conn(&mut conn, &raw2, SELF).unwrap();
        let t = &load_all(&conn).unwrap()[0];
        assert_eq!(t.owner_id.as_deref(), Some("zhang"));
        assert_eq!(t.updated_at, Some(200));
    }
}
