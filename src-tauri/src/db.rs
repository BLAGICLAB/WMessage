//! WMessage 任务数据存储：SQLite（app_data_dir/wmessage.db）
//! 方案B（2026-08-14）：行级增量读写，取代方案2 的 data.json 全量覆盖。
//! 单写者架构：只有主窗口通过 db_upsert/db_delete 写库，挂件只读（db_load）。

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::Manager; // F-6：Runtime 给 data_dir 泛型化

use crate::error::{CommandError, CommandResult};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Subtask {
    pub id: String,
    pub text: String,
    pub done: bool,
}

/// 任务卡绑定文件条目（2026-08-19 多文件绑定，上限 MAX_TASK_FILES）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskFile {
    pub path: String,
    pub is_dir: bool,
}

/// 任务卡绑定文件数量上限（多文件绑定 2026-08-19 老板指令）
pub const MAX_TASK_FILES: usize = 10;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub due: Option<String>,
    pub note: Option<String>,
    pub tags: Option<Vec<String>>,
    /// 绑定文件列表（多文件绑定）；None/空 = 未绑定
    #[serde(default)]
    pub files: Option<Vec<TaskFile>>,
    /// 旧单绑定字段：迁移过渡保留（启动迁移进 files；新写入双写首条保持旧版本可读）
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
    /// 定时执行规则：daily:HH:MM / weekly:D:HH:MM（D=1..7 周一起）/ at:YYYY-MM-DDTHH:MM（一次性）
    #[serde(default)]
    pub schedule: Option<String>,
    /// 上次定时执行时间（epoch ms）
    #[serde(default)]
    pub sched_last: Option<i64>,
    /// 已交给机器人执行中（🤖 点击置真；执行结束无论成败清除；启动时残留清零）
    #[serde(default)]
    pub bot_assigned: Option<bool>,
}

impl Task {
    /// 有效绑定文件列表：files 非空优先；否则回退旧单绑定字段（迁移过渡兜底，
    /// 覆盖「旧数据还没跑启动迁移就被读」的窗口）。
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

/// 多文件绑定元数据解析（bind_files 命令内核）：fs::metadata 判定 isDir，
/// 去重保序、空路径丢弃、超 MAX_TASK_FILES 截断。metadata 失败（路径已消失等）按文件处理。
pub fn resolve_task_files(paths: Vec<String>) -> Vec<TaskFile> {
    let mut out: Vec<TaskFile> = Vec::new();
    for p in paths {
        let path = p.trim().to_string();
        if path.is_empty() || out.iter().any(|f| f.path == path) {
            continue;
        }
        let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
        out.push(TaskFile { path, is_dir });
        if out.len() >= MAX_TASK_FILES {
            break;
        }
    }
    out
}

/// Tauri 命令：多文件绑定元数据解析（前端选完文件后拿 isDir；上限 10 内截断）
#[tauri::command]
pub fn bind_files(paths: Vec<String>) -> Vec<TaskFile> {
    resolve_task_files(paths)
}

/// Tauri 命令：旧单文件调用方兼容——直接转 bind_files(vec![path])
#[tauri::command]
pub fn bind_file(path: String) -> Vec<TaskFile> {
    resolve_task_files(vec![path])
}

/// 便携模式：数据库优先放 exe 同目录（U盘/绿色目录随走随带）；
/// 目录不可写（如 Program Files）时兜底到系统应用数据目录。
/// P2-19：目录解析委托 audit::probe_log_dir（原与 profile::data_dir /
/// audit::generic_log_dir 三处拷贝，已合一）。
fn db_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    crate::audit::probe_log_dir(app)
}

/// 数据目录（供本地 HTTP API 存 token 等附属文件，便携模式跟随 exe）
pub fn data_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    db_dir(app)
}

/// 日志轮转：超过 size_limit 字节就改名 .old（旧 .old 覆盖）。写日志前调用。
pub fn rotate_log_if_large(path: &std::path::Path, size_limit: u64) {
    if let Ok(md) = std::fs::metadata(path) {
        if md.len() > size_limit {
            let old = path.with_extension("log.old");
            let _ = std::fs::rename(path, &old);
        }
    }
}

/// NEW-B-6: 原子写文件——先写同目录 tmp 再 rename 覆盖目标。
/// 崩溃在写中途只留 tmp 残件，目标文件要么是旧完整版、要么是新完整版，
/// 不会留半截 JSON。tmp 与目标同目录保证 rename 同卷原子。
/// pub(crate)：后续 P2-8 save_rules 原子化可复用。
pub(crate) fn atomic_write(path: &std::path::Path, contents: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("无效的目标路径：{}", path.display()))?;
    let tmp = path.with_file_name(format!("{}.tmp", file_name.to_string_lossy()));
    std::fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败：{e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("落盘重命名失败：{e}"))?;
    Ok(())
}

/// P2-4: 便携模式首启拷贝老库——checkpoint 失败记 warn 继续（不吞错）；
/// 同时拷 `-wal` / `-shm` 边车文件（如存在）——只拷 `.db` 时若 checkpoint 失败，
/// WAL 里最近写入会静默丢失。返回告警列表（调用方写审计日志）；
/// 拷贝整体失败不致命，open_db 会继续开新库。
fn copy_legacy_db(legacy_db: &std::path::Path, db_path: &std::path::Path) -> Vec<String> {
    let mut warns = Vec::new();
    match rusqlite::Connection::open(legacy_db) {
        Ok(conn) => {
            // PRAGMA wal_checkpoint 的 BUSY 不走 Err——返回 (busy, log, checkpointed) 行，
            // busy=1 表示有读者未放行，WAL 未完全落主库（此时 -wal/-shm 拷贝就是救命副本）
            match conn.query_row("PRAGMA wal_checkpoint(TRUNCATE);", [], |r| {
                r.get::<_, i64>(0)
            }) {
                Ok(0) => {} // checkpoint 完成
                Ok(_) => warns.push(
                    "老库 WAL checkpoint 被占（BUSY），WAL 未落主库——-wal/-shm 已一并拷贝"
                        .to_string(),
                ),
                Err(e) => warns.push(format!(
                    "老库 WAL checkpoint 失败（继续拷贝，-wal/-shm 一并带走）：{e}"
                )),
            }
        }
        Err(e) => warns.push(format!("老库打开失败（跳过 checkpoint 直接拷贝）：{e}")),
    }
    if let Err(e) = std::fs::copy(legacy_db, db_path) {
        warns.push(format!("老库拷贝失败：{e}"));
        return warns;
    }
    for ext in ["wal", "shm"] {
        let src = wal_sidecar(legacy_db, ext);
        if src.exists() {
            if let Err(e) = std::fs::copy(&src, wal_sidecar(db_path, ext)) {
                warns.push(format!("老库 -{ext} 边车拷贝失败：{e}"));
            }
        }
    }
    warns
}

/// SQLite WAL 边车路径：`wmessage.db` → `wmessage.db-wal` / `wmessage.db-shm`
fn wal_sidecar(db: &std::path::Path, ext: &str) -> std::path::PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push(format!("-{ext}"));
    std::path::PathBuf::from(s)
}

pub fn open_db(app: &tauri::AppHandle) -> Result<rusqlite::Connection, String> {
    let dir = db_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let db_path = dir.join("wmessage.db");
    // 一次性迁移：exe 目录无库时，从系统应用数据目录（老版本存放处）拷一份过来，
    // 先 checkpoint WAL 保证最近写入都落主库；老文件保留不动。
    if !db_path.exists() {
        if let Ok(legacy_dir) = app.path().app_data_dir() {
            let legacy_db = legacy_dir.join("wmessage.db");
            if legacy_db.exists() && legacy_db != db_path {
                // P2-4：拷贝失败/ checkpoint 失败不再 `let _` 全静默——记 WARN 审计继续
                for w in copy_legacy_db(&legacy_db, &db_path) {
                    crate::audit::write_event(
                        app,
                        crate::audit::AuditLevel::Warn,
                        "legacy_db_copy",
                        &[("warn", w)],
                    );
                }
            }
        }
    }
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
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
         -- B1: 迁移操作对账日志。pending = 操作已开始但未提交，committed/cleared = 终态
         -- 启动时 replay pending：检测 move/delete 是否实际完成，必要时修复 DB
         -- 表为幂等设计，多次启动不会重复修复
         CREATE TABLE IF NOT EXISTS migration_journal (
           id          INTEGER PRIMARY KEY AUTOINCREMENT,
           op          TEXT    NOT NULL,    -- 'move' | 'delete'
           src         TEXT    NOT NULL,
           dst         TEXT,                -- 仅 move 有值；delete 为 NULL
           task_id     TEXT    NOT NULL,
           state       TEXT    NOT NULL,    -- 'pending' | 'committed' | 'cleared'
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
         );
         -- 机器人长期记忆（2026-08-20 Phase 4.2）：key-value 事实/偏好，跨会话保留；
         -- 模型经 remember_fact 写入（同 key 覆盖，空 value=删除），recall_facts 全量读回
         CREATE TABLE IF NOT EXISTS bot_facts (
           key        TEXT PRIMARY KEY,
           value      TEXT NOT NULL,
           updated_at INTEGER NOT NULL
         );",
    )
    .map_err(|e| e.to_string())?;
    // 迁移：定时任务卡（2026-08-16）——tasks 补 schedule / sched_last 列
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
    // 迁移：机器人归属头像（2026-08-16）——tasks 补 bot_assigned 列
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
    // 迁移：老库补 ord 列（任务拖拽排序字段，2026-08-14 新增）
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
    // 迁移：老库补 updated_at 列（合并导入比较用，2026-08-14 新增）
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
    // 迁移：任务卡多文件绑定（2026-08-19）——tasks 补 files 列（JSON [{path,isDir}]），
    // 老 file_path/file_is_dir 单绑定回填进 files（仅 files 为空时；老列保留不清，过渡期旧版本可读）
    ensure_files_column(&conn)?;
    let migrated = migrate_legacy_file_bindings(&conn)?;
    if migrated > 0 {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "task_files_migration",
            &[("migrated", migrated.to_string())],
        );
    }
    // 迁移：多会话（2026-08-16）——bot_messages 补 session_id 列；老单会话消息归入「默认对话」
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
    // 迁移：聊天折叠行持久化（2026-08-16）——bot_messages 补 thinking/tools 列（思考过程 + 工具调用行，JSON）
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
    // 启动时清残留 bot_assigned（机器人执行不可能跨重启存活；每进程仅一次）
    // NEW-B-4: 不用 Once——首开遇库忙（busy_timeout 2s 兜底超时）时 Once 会把失败
    // 当「已做过」永久吞错，残留 🤖 标志要等下次重启才清。改为 AtomicBool 标志：
    // 仅 UPDATE 成功才置位，失败不消耗，下次 open_db 自动重试（UPDATE 幂等无副作用）。
    static BOT_ASSIGNED_RESET_DONE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    reset_bot_assigned_with(&BOT_ASSIGNED_RESET_DONE, || {
        conn.execute("UPDATE tasks SET bot_assigned = 0 WHERE bot_assigned = 1", [])
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    Ok(conn)
}

/// 多文件绑定（2026-08-19）：tasks 补 files 列（老库 ALTER 幂等）
fn ensure_files_column(conn: &rusqlite::Connection) -> Result<(), String> {
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

/// 多文件绑定（2026-08-19）：老单绑定 file_path/file_is_dir → files JSON。
/// 仅当 files 为空（NULL/''/'[]'）时回填，已迁移/新数据不动（幂等，可每启动重跑）。
/// 返回迁移条数。
fn migrate_legacy_file_bindings(conn: &rusqlite::Connection) -> Result<usize, String> {
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
        mapped.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };
    let mut n = 0;
    for (id, path, is_dir) in rows {
        let files = serde_json::to_string(&vec![TaskFile {
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

/// NEW-B-4: 可重试的一次性执行——done 未置位时跑 exec，仅成功才置位；
/// 失败返回 false 留待下次调用重试（不消耗 token）。已置位直接返回 true。
fn reset_bot_assigned_with<F: FnOnce() -> Result<(), String>>(
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

// ───────────────────────── 工作区（静态链接） ─────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLink {
    pub id: String,
    /// 展示别名，用户可自由自定义；兼容旧数据字段 label
    #[serde(alias = "label")]
    pub display_name: String,
    /// 底层真实路径 / 网页链接；兼容旧数据字段 target
    #[serde(alias = "target")]
    pub target_uri: String,
    /// url | file | folder
    pub kind: String,
}

/// 机器人聊天消息行（持久化）：role=user/assistant，refs 为任务引用 JSON（可空）；
/// thinking=思考过程文本，tools_json=工具调用行 JSON（折叠行装饰，可空）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotMsgRow {
    pub role: String,
    pub content: String,
    pub refs_json: Option<String>,
    pub thinking: Option<String>,
    pub tools_json: Option<String>,
}

/// 机器人会话（多会话，2026-08-16）
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BotSession {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceItem {
    pub id: String,
    pub title: String,
    pub collapsed: Option<bool>,
    pub links: Vec<WorkspaceLink>,
    pub order: Option<f64>,
    pub updated_at: Option<i64>,
}

// Skill outcome persistence (Phase 5 D 2026-08-18 08:00)
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PersistedSkillOutcome {
    pub skill_name: String,
    pub kind: String,
    pub reason: Option<String>,
    pub completed_summary: Option<String>,
    pub rollback_attempted: Option<bool>,
    pub last_at_ms: i64,
}

pub fn upsert_skill_outcome(
    conn: &rusqlite::Connection,
    o: &PersistedSkillOutcome,
) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO skill_outcomes (skill_name, kind, reason, completed_summary, rollback_attempted, last_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![o.skill_name, o.kind, o.reason, o.completed_summary, o.rollback_attempted.map(|b| if b { 1 } else { 0 }), o.last_at_ms],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_all_skill_outcomes(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, PersistedSkillOutcome>, String> {
    let mut stmt = conn.prepare("SELECT skill_name, kind, reason, completed_summary, rollback_attempted, last_at_ms FROM skill_outcomes").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(PersistedSkillOutcome {
                skill_name: r.get(0)?,
                kind: r.get(1)?,
                reason: r.get(2)?,
                completed_summary: r.get(3)?,
                rollback_attempted: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
                last_at_ms: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let o = row.map_err(|e| e.to_string())?;
        map.insert(o.skill_name.clone(), o);
    }
    Ok(map)
}

fn load_workspace(conn: &rusqlite::Connection) -> Result<Vec<WorkspaceItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, title, collapsed, links, ord, updated_at
             FROM workspace_items
             ORDER BY ord IS NULL, ord",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<f64>>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    for row in rows {
        let (id, title, collapsed, links, order, updated_at) = row.map_err(|e| e.to_string())?;
        items.push(WorkspaceItem {
            id,
            title,
            collapsed: collapsed.map(|v| v != 0),
            links: serde_json::from_str(&links).unwrap_or_default(),
            order,
            updated_at,
        });
    }
    Ok(items)
}

/// P2-5: 循环 execute 包事务（同 P0-1 / B1 模式）——中途失败整体回滚，不留半截写入。
fn upsert_workspace(
    conn: &mut rusqlite::Connection,
    items: &[WorkspaceItem],
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO workspace_items (id, title, collapsed, links, ord, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET
                   title=excluded.title, collapsed=excluded.collapsed,
                   links=excluded.links, ord=excluded.ord,
                   updated_at=excluded.updated_at",
            )
            .map_err(|e| e.to_string())?;
        for it in items {
            stmt.execute(rusqlite::params![
                it.id,
                it.title,
                it.collapsed.map(|v| v as i64),
                serde_json::to_string(&it.links).unwrap_or_else(|_| "[]".into()),
                it.order,
                it.updated_at,
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// P2-5: 同 upsert_workspace——批量 DELETE 包事务，中途失败回滚。
fn delete_workspace(conn: &mut rusqlite::Connection, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    {
        let mut stmt = tx
            .prepare("DELETE FROM workspace_items WHERE id = ?1")
            .map_err(|e| e.to_string())?;
        for id in ids {
            stmt.execute([id]).map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn workspace_load(app: tauri::AppHandle) -> CommandResult<Vec<WorkspaceItem>> {
    // NEW-B-3: B3 残留 sync 命令——与 db_load 同模式扔到 spawn_blocking，不阻塞 UI。
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&app)?;
        load_workspace(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区读取线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workspace_upsert(app: tauri::AppHandle, items: Vec<WorkspaceItem>) -> CommandResult<()> {
    // NEW-B-3: 数据量小但仍是磁盘 IO；包 async + spawn_blocking（内部循环 execute 逻辑不动）。
    tauri::async_runtime::spawn_blocking(move || {
        if items.is_empty() {
            return Ok(());
        }
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        upsert_workspace(&mut conn, &items).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区写入线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workspace_delete(app: tauri::AppHandle, ids: Vec<String>) -> CommandResult<()> {
    // NEW-B-3: 同 workspace_upsert——包 async + spawn_blocking（内部循环逻辑不动）。
    tauri::async_runtime::spawn_blocking(move || {
        if ids.is_empty() {
            return Ok(());
        }
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        delete_workspace(&mut conn, &ids).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区删除线程 join 失败：{e}")))?
}

// ───────────────────────── 机器人聊天记录 ─────────────────────────

/// 全部会话列表（按最近更新倒序）
#[tauri::command]
pub fn bot_sessions_load(app: tauri::AppHandle) -> CommandResult<Vec<BotSession>> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, created_at, updated_at FROM bot_sessions ORDER BY updated_at DESC",
        )
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BotSession {
                id: r.get::<_, String>(0)?,
                title: r.get::<_, String>(1)?,
                created_at: r.get::<_, i64>(2)?,
                updated_at: r.get::<_, i64>(3)?,
            })
        })
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| CommandError::DbError(e.to_string()))
}

/// 新建会话，返回新会话（title 缺省「新对话」）
#[tauri::command]
pub fn bot_session_create(
    app: tauri::AppHandle,
    title: Option<String>,
) -> CommandResult<BotSession> {
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "新对话".into());
    conn.execute(
        "INSERT INTO bot_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        rusqlite::params![id, title, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(BotSession {
        id,
        title,
        created_at: now,
        updated_at: now,
    })
}

/// 删除会话及其全部消息（原子：消息与会话同一事务，任一失败整体回滚）
#[tauri::command]
pub async fn bot_session_delete(app: tauri::AppHandle, id: String) -> CommandResult<()> {
    // B3: 高频写 + 跨表事务 → 主线程会阻塞；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        let tx = conn
            .transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        tx.execute("DELETE FROM bot_messages WHERE session_id = ?1", [&id])
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        tx.execute("DELETE FROM bot_sessions WHERE id = ?1", [&id])
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("会话删除线程 join 失败：{e}")))?
}

/// 会话改名
#[tauri::command]
pub fn bot_session_rename(app: tauri::AppHandle, id: String, title: String) -> CommandResult<()> {
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "UPDATE bot_sessions SET title = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![title.trim(), now, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 加载指定会话的消息（按写入顺序）
#[tauri::command]
pub async fn bot_history_load(
    app: tauri::AppHandle,
    session_id: String,
) -> CommandResult<Vec<BotMsgRow>> {
    // B3: 长会话（几千条消息）查询会被主线程阻塞；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&app)?;
        let mut stmt = conn
            .prepare("SELECT role, content, refs, thinking, tools FROM bot_messages WHERE session_id = ?1 ORDER BY id")
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        let rows = stmt
            .query_map([&session_id], |r| {
                Ok(BotMsgRow {
                    role: r.get::<_, String>(0)?,
                    content: r.get::<_, String>(1)?,
                    refs_json: r.get::<_, Option<String>>(2)?,
                    thinking: r.get::<_, Option<String>>(3)?,
                    tools_json: r.get::<_, Option<String>>(4)?,
                })
            })
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("历史读取线程 join 失败：{e}")))?
}

/// 保存指定会话的聊天记录：全量覆盖 + 更新会话活跃时间（原子：DELETE+INSERT+UPDATE 同一事务）
fn bot_history_save_inner(
    conn: &rusqlite::Connection,
    session_id: &str,
    messages: &[BotMsgRow],
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM bot_messages WHERE session_id = ?1",
        [session_id],
    )
    .map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp_millis();
    if !messages.is_empty() {
        let mut stmt = conn
            .prepare(
                "INSERT INTO bot_messages (role, content, refs, session_id, thinking, tools, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|e| e.to_string())?;
        for m in messages {
            stmt.execute(rusqlite::params![
                m.role,
                m.content,
                m.refs_json,
                session_id,
                m.thinking,
                m.tools_json,
                now
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    conn.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn bot_history_save(
    app: tauri::AppHandle,
    session_id: String,
    messages: Vec<BotMsgRow>,
) -> CommandResult<()> {
    // B3: 长会话全量覆盖写入 + fsync 重；主线程阻塞；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        let tx = conn
            .transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        bot_history_save_inner(&tx, &session_id, &messages)
            .map_err(CommandError::DbError)?;
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("历史保存线程 join 失败：{e}")))?
}

/// 清空指定会话的聊天记录（会话保留）
#[tauri::command]
pub async fn bot_history_clear(app: tauri::AppHandle, session_id: String) -> CommandResult<()> {
    // B3: DELETE 大量消息时仍可能阻塞；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = open_db(&app)?;
        conn.execute(
            "DELETE FROM bot_messages WHERE session_id = ?1",
            [&session_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| CommandError::from(format!("历史清空线程 join 失败：{e}")))?
}

fn upsert_tasks(conn: &rusqlite::Connection, tasks: &[Task]) -> Result<(), String> {
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
             -- B2: lost update 守卫 — 只允许新数据压过老数据
             -- current 为 NULL (老行) → 任何新数据胜出
             -- current 有值 且 incoming >= current → 更新
             -- current 有值 且 incoming < current → 跳过 (避免迁移中的旧快照回写覆盖用户新改)
             WHERE tasks.updated_at IS NULL OR excluded.updated_at >= tasks.updated_at",
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
        let subtasks = match subtasks {
            Some(s) => serde_json::from_str(&s).ok(),
            None => None,
        };
        let files = files.and_then(|s| serde_json::from_str(&s).ok());
        tasks.push(Task {
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
        });
    }
    Ok(tasks)
}

/// 迁移方案2 的 data.json：json 里有库里缺的任务就补回，导入成功后删除旧文件。
/// P2-7：原触发条件「库 count==0」——用户删任务后重启、老 data.json 还在时不再迁移，
/// 数据静默丢失。改为「json 任务数 > 库内任务数」即尝试，且只补库中缺失的 id
/// （已存在的 id 不用 json 旧值覆盖，防回滚用户的新编辑）。
fn migrate_data_json(app: &tauri::AppHandle, conn: &mut rusqlite::Connection) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = migrate_data_json_file(&dir.join("data.json"), conn);
}

/// P2-7 可测内核：返回是否执行了迁移（成功补回并删除旧文件）。
fn migrate_data_json_file(file: &std::path::Path, conn: &mut rusqlite::Connection) -> bool {
    if !file.exists() {
        return false;
    }
    let Ok(json) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(tasks) = serde_json::from_str::<Vec<Task>>(&json) else {
        return false;
    };
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap_or(0);
    if tasks.len() as i64 <= count {
        return false; // json 没有比库更多的任务，无可补
    }
    // 只补库中缺失的 id——INSERT OR REPLACE 会用 json 旧值覆盖已有行的新编辑
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
    let missing: Vec<Task> = tasks
        .into_iter()
        .filter(|t| !existing.contains(&t.id))
        .collect();
    if missing.is_empty() {
        return false;
    }
    let Ok(tx) = conn.transaction() else { return false };
    if upsert_tasks(&tx, &missing).is_ok() && tx.commit().is_ok() {
        let _ = std::fs::remove_file(file);
        return true;
    }
    false
}

/// 写操作全局锁：主窗口（db_upsert/db_delete/db_merge）与本地 API 线程共享同一把锁，
/// 避免 WAL 下并发写冲突（busy_timeout 只是兜底）。
/// pub(crate)：migration 的 journal 写也纳入同一把锁（NEW-B-2）。
pub(crate) static DB_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tauri::command]
pub async fn db_load(app: tauri::AppHandle) -> CommandResult<Vec<Task>> {
    // B3: 启动加载全部任务（可能有几千条 + migrate_data_json 读 JSON 文件）；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = open_db(&app)?;
        // P2-7：触发判定移入 migrate_data_json（json 任务数 > 库内任务数才补回），
        // 不再「库空才迁移」——用户删任务后重启、老 data.json 还在时也能补回
        migrate_data_json(&app, &mut conn);
        load_all(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库读取线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn db_upsert(app: tauri::AppHandle, tasks: Vec<Task>) -> CommandResult<()> {
    // B3: 高频写（挂件拖拽/编辑都走这里），批量事务含 fsync；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        if tasks.is_empty() {
            return Ok(());
        }
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
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
pub async fn db_delete(app: tauri::AppHandle, ids: Vec<String>) -> CommandResult<()> {
    // B3: 批量删（回收站多选 / 清空）；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        if ids.is_empty() {
            return Ok(());
        }
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = open_db(&app)?;
        delete_tasks(&conn, &ids).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("数据库删除线程 join 失败：{e}")))?
}

/// 只读读取外部数据库（可能是更老版本，缺 ord / updated_at 列时按 NULL 处理）
fn load_external(conn: &rusqlite::Connection) -> Result<Vec<Task>, String> {
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(tasks)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).collect())
        })
        .map_err(|e| e.to_string())?;
    let has_ord = cols.iter().any(|c| c == "ord");
    let has_ua = cols.iter().any(|c| c == "updated_at");
    // 定时任务卡 + 机器人归属（2026-08-16 新列）：外部库有就读，没有按 NULL（此前硬编码 NULL 会丢数据）
    let has_sched = cols.iter().any(|c| c == "schedule");
    let has_sched_last = cols.iter().any(|c| c == "sched_last");
    let has_ba = cols.iter().any(|c| c == "bot_assigned");
    // 多文件绑定（2026-08-19 新列）：外部库有就读，没有按 NULL
    let has_files = cols.iter().any(|c| c == "files");
    let sql = format!(
        "SELECT id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                completed_at, archived, deleted_at, collapsed, {}, {}, {}, {}, {}, {}
         FROM tasks",
        if has_ord { "ord" } else { "NULL" },
        if has_ua { "updated_at" } else { "NULL" },
        if has_sched { "schedule" } else { "NULL" },
        if has_sched_last { "sched_last" } else { "NULL" },
        if has_ba { "bot_assigned" } else { "NULL" },
        if has_files { "files" } else { "NULL" }
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
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
        let tags = tags.and_then(|s| serde_json::from_str(&s).ok());
        let subtasks = subtasks.and_then(|s| serde_json::from_str(&s).ok());
        let files = files.and_then(|s| serde_json::from_str(&s).ok());
        tasks.push(Task {
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
        });
    }
    Ok(tasks)
}

/// 合并导入：按 id 并集；同 id 内容分歧时保留 updated_at 更新（外部无 updated_at 视为最旧）。
/// 返回实际写入的任务条数。
#[tauri::command]
pub async fn db_merge(app: tauri::AppHandle, path: String) -> CommandResult<usize> {
    // B3: 长文件读 + 跨表事务；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        use rusqlite::{OpenFlags, OptionalExtension};

        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let src = rusqlite::Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("无法打开所选数据库：{e}"))?;
        let ext = load_external(&src)?;
        if ext.is_empty() {
            return Ok(0);
        }
        let mut conn = open_db(&app)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut merged = 0usize;
        for t in &ext {
            // B4: 读 Option<i64> 处理 NULL（2026-08-14 前老行 updated_at 为 NULL）
            // 原代码 r.get::<_, i64> 遇 NULL 直接报 InvalidColumnType → 整次导入崩溃
            let cur: Option<Option<i64>> = tx
                .query_row(
                    "SELECT updated_at FROM tasks WHERE id = ?1",
                    rusqlite::params![t.id],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let cur_ua = cur.flatten().unwrap_or(0); // NULL / 无行 都视为 0
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
    .map_err(|e| CommandError::from(format!("数据库合并线程 join 失败：{e}")))?
}

/// 导出任务卡数据：全量任务（含归档、回收站）序列化为 JSON 文件，返回条数
#[tauri::command]
pub async fn tasks_export(app: tauri::AppHandle, path: String) -> CommandResult<usize> {
    // B3: 大数据集导出（load_all + JSON 序列化 + 文件写）阻塞主线程；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&app)?;
        let tasks = load_all(&conn)?;
        let json = serde_json::to_string_pretty(&tasks).map_err(|e| e.to_string())?;
        // NEW-B-6: 原子写——崩溃不留半截 JSON（先写同目录 tmp 再 rename）
        atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(tasks.len())
    })
    .await
    .map_err(|e| CommandError::from(format!("任务导出线程 join 失败：{e}")))?
}

/// 从 JSON 文件导入任务卡数据：按 id 并集合并，同 id 保留 updated_at 更晚者。返回写入条数。
#[tauri::command]
pub async fn tasks_import(app: tauri::AppHandle, path: String) -> CommandResult<usize> {
    // B3: 大文件读 + 解析 + 长事务；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        use rusqlite::OptionalExtension;

        let raw = std::fs::read_to_string(&path).map_err(|e| format!("无法读取所选文件：{e}"))?;
        let ext: Vec<Task> =
            serde_json::from_str(&raw).map_err(|e| format!("不是有效的任务数据 JSON：{e}"))?;
        if ext.is_empty() {
            return Ok(0);
        }
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut merged = 0usize;
        for t in &ext {
            if t.id.trim().is_empty() {
                continue; // 跳过无 id 的脏数据
            }
            // B4: 同 db_merge — NULL updated_at 兼容
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
mod tests {
    use super::*;
    use std::fs;

    // ── P2-4：legacy WAL copy 不吞错 + 边车文件一并拷贝 ──

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
            writer
                .execute_batch("INSERT INTO t VALUES ('y');")
                .unwrap();
            let warns = copy_legacy_db(&legacy, &dst);
            drop(writer);
            warns
        };

        assert!(dst.exists(), "主库必须拷过来");
        assert!(
            wal_sidecar(&dst, "wal").exists(),
            "-wal 边车必须拷过来（否则 checkpoint 失败时 WAL 写入静默丢失）"
        );
        assert!(
            wal_sidecar(&dst, "shm").exists(),
            "-shm 边车必须拷过来"
        );
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
        let warns = copy_legacy_db(&legacy, &dst);
        assert!(warns.is_empty(), "happy path 不应有 warn；got: {warns:?}");
        let c = rusqlite::Connection::open(&dst).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "拷贝后的库应含老数据");
        fs::remove_dir_all(&dir).ok();
    }

    // ── P2-7：migrate_data_json 触发条件放宽（json 任务数 > 库内任务数即补回） ──

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
            column: "todo".into(),
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
        }
    }

    /// 删任务后重启场景：库里只剩 t1，老 data.json 还有 t1/t2/t3 → 补回 t2/t3，
    /// 且已存在的 t1 不得被 json 旧值覆盖（B2 守卫之外再加 id 过滤）。
    #[test]
    fn migrate_data_json_backfills_missing_after_user_delete() {
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
            "json 任务数(3) > 库内(1) 必须触发迁移补回"
        );
        let tasks = load_all(&conn).unwrap();
        assert_eq!(tasks.len(), 3, "t2/t3 应补回");
        let t1 = tasks.iter().find(|t| t.id == "t1").unwrap();
        assert_eq!(t1.title, "新标题", "已有 id 不得被 json 旧值覆盖");
        assert!(!file.exists(), "迁移成功后 data.json 应删除");
        fs::remove_dir_all(&dir).ok();
    }

    /// 库不比 json 少 → 不触发（防重复迁移）；文件保留原状
    #[test]
    fn migrate_data_json_skips_when_db_not_behind() {
        let (dir, mut conn) = setup_tasks_db();
        upsert_tasks(&conn, &[mk_task("t1", "a"), mk_task("t2", "b")]).unwrap();
        let file = dir.join("data.json");
        fs::write(&file, serde_json::to_string(&vec![mk_task("t1", "old")]).unwrap()).unwrap();

        assert!(
            !migrate_data_json_file(&file, &mut conn),
            "json 任务数(1) <= 库内(2) 不得触发"
        );
        assert!(file.exists(), "未迁移时文件保留");
        let tasks = load_all(&conn).unwrap();
        assert_eq!(tasks.len(), 2, "库内容不得变化");
        fs::remove_dir_all(&dir).ok();
    }

    // ── 多文件绑定（2026-08-19）：files 列迁移 + 老数据回填 + 上限 ──

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

    /// 老数据 → 新 schema 完整链路：ALTER 补 files 列 + file_path 回填 files + 读回解析。
    /// 覆盖：文件绑定、文件夹绑定（is_dir=1）、未绑定不动、幂等重跑不重复迁移。
    #[test]
    fn legacy_file_binding_migrates_to_files_column() {
        let (dir, conn) = setup_legacy_tasks_db();
        conn.execute_batch(
            "INSERT INTO tasks (id, title, file_path, file_is_dir, col) VALUES
               ('t-file', '绑文件', '/tmp/a.pdf', 0, 'todo'),
               ('t-dir',  '绑文件夹', '/tmp/dir', 1, 'todo'),
               ('t-none', '没绑', NULL, NULL, 'todo');",
        )
        .unwrap();

        ensure_files_column(&conn).unwrap();
        assert_eq!(migrate_legacy_file_bindings(&conn).unwrap(), 2, "两条老绑定应迁移");

        let tasks = load_all(&conn).unwrap();
        let tf = tasks.iter().find(|t| t.id == "t-file").unwrap();
        assert_eq!(
            tf.files.as_deref(),
            Some(vec![TaskFile { path: "/tmp/a.pdf".into(), is_dir: false }].as_slice()),
            "文件绑定应迁进 files（isDir=false）"
        );
        let td = tasks.iter().find(|t| t.id == "t-dir").unwrap();
        assert_eq!(
            td.files.as_deref(),
            Some(vec![TaskFile { path: "/tmp/dir".into(), is_dir: true }].as_slice()),
            "文件夹绑定应迁进 files（isDir=true）"
        );
        let tn = tasks.iter().find(|t| t.id == "t-none").unwrap();
        assert!(tn.files.is_none(), "未绑定任务不得产生 files");

        // 老列保留（迁移过渡期旧版本仍可读 file_path/file_is_dir）
        let fp: Option<String> = conn
            .query_row("SELECT file_path FROM tasks WHERE id='t-file'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fp.as_deref(), Some("/tmp/a.pdf"), "老列 file_path 保留不清");

        // 幂等：再跑一次迁移条数为 0，files 不变
        assert_eq!(migrate_legacy_file_bindings(&conn).unwrap(), 0, "重跑不得重复迁移");
        fs::remove_dir_all(&dir).ok();
    }

    /// 已有 files 的任务不被老列回填覆盖（files 非空 → 跳过）
    #[test]
    fn migration_skips_tasks_with_existing_files() {
        let (dir, conn) = setup_legacy_tasks_db();
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
        assert_eq!(migrate_legacy_file_bindings(&conn).unwrap(), 0);
        let t = &load_all(&conn).unwrap()[0];
        assert_eq!(
            t.files.as_deref().unwrap()[0].path, "/tmp/new.txt",
            "已有 files 不得被老 file_path 覆盖"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// files 列读写回环：upsert 多文件 → load_all 原样读回
    #[test]
    fn files_roundtrip_via_upsert_load() {
        let (dir, conn) = setup_tasks_db();
        let mut t = mk_task("t1", "多文件");
        t.files = Some(vec![
            TaskFile { path: "/a/1.pdf".into(), is_dir: false },
            TaskFile { path: "/a/2.docx".into(), is_dir: false },
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
            vec![TaskFile { path: "/tmp/legacy.pdf".into(), is_dir: true }],
            "files 空 → 回退旧字段"
        );
        t.files = Some(vec![TaskFile { path: "/tmp/new.pdf".into(), is_dir: false }]);
        assert_eq!(
            t.effective_files(),
            vec![TaskFile { path: "/tmp/new.pdf".into(), is_dir: false }],
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

    /// P0-1 核心回归：bot_history_save 内层报错时，DELETE 必须被事务回滚，
    /// 原会话聊天记录不得丢失（崩溃/强杀落在 INSERT 中间的场景）。
    #[test]
    fn bot_history_save_rolls_back_on_insert_failure() {
        let (dir, mut conn) = setup_bhs_db();
        conn.execute("INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000)", [])
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
            tx.execute(
                "DELETE FROM bot_messages WHERE session_id = 's1'",
                [],
            )
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
        conn.execute("INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 5000)", [])
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
            .query_row("SELECT updated_at FROM bot_sessions WHERE id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert!(updated_at >= 5000, "updated_at 应被 update 为 now >= 5000");

        fs::remove_dir_all(&dir).ok();
    }

    /// bot_session_delete 原子性：消息与会话同一事务，任一失败不留下半删状态。
    #[test]
    fn bot_session_delete_is_atomic() {
        let (dir, conn) = setup_bhs_db();
        conn.execute("INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', 'user', 'm1', 1000)",
            [],
        )
        .unwrap();

        let mut conn = conn;
        let tx = conn.transaction().unwrap();
        tx.execute("DELETE FROM bot_messages WHERE session_id = 's1'", [])
            .unwrap();
        tx.execute("DELETE FROM bot_sessions WHERE id = 's1'", [])
            .unwrap();
        tx.commit().unwrap();

        let sess_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_sessions WHERE id = 's1'", [], |r| r.get(0))
            .unwrap();
        let msg_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sess_count, 0, "提交后会话应被删除");
        assert_eq!(msg_count, 0, "提交后消息应被删除");

        fs::remove_dir_all(&dir).ok();
    }

    /// B4: 模拟 2026-08-14 前的任务行（updated_at IS NULL），验证 db_merge / tasks_import
    /// 读取不再崩。原代码 r.get::<_, i64>(0) 遇 NULL 报 InvalidColumnType，整次导入失败。
    /// 新代码 r.get::<_, Option<i64>>(0) + flatten + unwrap_or(0)。
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

        // NULL 行：原本 r.get::<_, i64>(0) 报 Err，现在 r.get::<_, Option<i64>>(0) → Some(None)
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
            .query_row(
                "SELECT updated_at FROM tasks WHERE id = 'new-1'",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
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

    /// B2: upsert WHERE 守卫防 lost update。
    /// 五场景：incoming>current / incoming<current / 相等 / 老 NULL 行 / incoming NULL
    #[test]
    fn upsert_where_guard_prevents_lost_update() {
        let dir = std::env::temp_dir().join(format!("wm-b2-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                updated_at INTEGER
            );
            INSERT INTO tasks (id, title, updated_at) VALUES ('t1', 'old-50', 50);
            INSERT INTO tasks (id, title) VALUES ('legacy', 'legacy-row');",
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "INSERT INTO tasks (id, title, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                   title=excluded.title,
                   updated_at=excluded.updated_at
                 WHERE tasks.updated_at IS NULL OR excluded.updated_at >= tasks.updated_at",
            )
            .unwrap();

        // 场景 1: incoming(100) > current(50) → 更新
        stmt.execute(rusqlite::params!["t1", "new-100", 100]).unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "new-100", "场景 1: 更新的应压过老的");

        // 场景 2: incoming(60) < current(100) → 跳过（lost update 防护）
        stmt.execute(rusqlite::params!["t1", "old-snapshot-60", 60]).unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "new-100", "场景 2: 更老的不应压过更新的");

        // 场景 3: 相等 timestamp → 允许更新
        stmt.execute(rusqlite::params!["t1", "equal-100", 100]).unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "equal-100", "场景 3: 相等 timestamp 仍允许更新");

        // 场景 4: 老 NULL 行被任何 incoming 覆盖
        stmt.execute(rusqlite::params!["legacy", "new-over-legacy", 5]).unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 'legacy'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "new-over-legacy", "场景 4: 老 NULL 行被任何 incoming 覆盖");

        // 场景 5: incoming NULL 不应覆盖 current 有值
        conn.execute("UPDATE tasks SET title='keep-me', updated_at=200 WHERE id='t1'", []).unwrap();
        stmt.execute(rusqlite::params!["t1", "incoming-null", Option::<i64>::None]).unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(title, "keep-me", "场景 5: incoming NULL 不应覆盖 current 有值");

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

    /// NEW-B-3: workspace_* 改 async + spawn_blocking 后，阻塞段逻辑与返回类型不变
    /// （CommandResult<Vec<WorkspaceItem>> / CommandResult<()>）。AppHandle 无法单测构造，
    /// 用临时库复刻命令体的 spawn_blocking 桥接结构，走 block_on 验证。
    #[test]
    fn workspace_commands_spawn_blocking_bridge() {
        let dir = std::env::temp_dir().join(format!("wm-ws-async-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("t.db");
        rusqlite::Connection::open(&db_path)
            .unwrap()
            .execute_batch(
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

        // upsert（桥接结构同 workspace_upsert 命令体）
        let (p, it) = (db_path.clone(), item.clone());
        let r: CommandResult<()> = tauri::async_runtime::block_on(async move {
            tauri::async_runtime::spawn_blocking(move || {
                let mut conn = rusqlite::Connection::open(&p)
                    .map_err(|e| CommandError::from(e.to_string()))?;
                upsert_workspace(&mut conn, &[it]).map_err(CommandError::from)
            })
            .await
            .map_err(|e| CommandError::from(format!("join 失败：{e}")))?
        });
        r.unwrap();

        // load
        let p = db_path.clone();
        let r: CommandResult<Vec<WorkspaceItem>> = tauri::async_runtime::block_on(async move {
            tauri::async_runtime::spawn_blocking(move || {
                let conn = rusqlite::Connection::open(&p)
                    .map_err(|e| CommandError::from(e.to_string()))?;
                load_workspace(&conn).map_err(CommandError::from)
            })
            .await
            .map_err(|e| CommandError::from(format!("join 失败：{e}")))?
        });
        let items = r.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "T");

        // delete
        let p = db_path.clone();
        let r: CommandResult<()> = tauri::async_runtime::block_on(async move {
            tauri::async_runtime::spawn_blocking(move || {
                let mut conn = rusqlite::Connection::open(&p)
                    .map_err(|e| CommandError::from(e.to_string()))?;
                delete_workspace(&mut conn, &["w1".to_string()]).map_err(CommandError::from)
            })
            .await
            .map_err(|e| CommandError::from(format!("join 失败：{e}")))?
        });
        r.unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert!(load_workspace(&conn).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    /// P2-5：upsert_workspace 中途失败必须整体回滚——已写入的前序行不得落库。
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
        let r = upsert_workspace(&mut conn, &[mk("w1", "new"), mk("boom", "x"), mk("w2", "y")]);
        assert!(r.is_err(), "trigger 注入失败必须返回 Err");
        let got = load_workspace(&conn).unwrap();
        assert_eq!(got.len(), 1, "半截写入必须被回滚（w2 不得落库）");
        assert_eq!(got[0].title, "old", "w1 必须保持旧值（事务回滚）");
        fs::remove_dir_all(&dir).ok();
    }

    /// P2-5：delete_workspace 中途失败同样整体回滚——前序已删行恢复。
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
}

#[cfg(test)]
mod reset_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// NEW-B-4: 首次 UPDATE 失败不消耗 token——下次重试成功后才置位，此后不再执行。
    #[test]
    fn reset_bot_assigned_retries_on_failure() {
        let done = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);

        // 第一次失败（模拟首开遇库忙）：不置位，留待重试
        let ok = reset_bot_assigned_with(&done, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err("database is locked".into())
        });
        assert!(!ok, "失败应返回 false");
        assert!(!done.load(Ordering::SeqCst), "失败不得消耗 token");

        // 第二次重试成功 → 置位
        let ok = reset_bot_assigned_with(&done, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(ok);
        assert!(done.load(Ordering::SeqCst));

        // 第三次：已置位 → 直接 true，不再执行 exec（保留「仅清一次」语义）
        let ok = reset_bot_assigned_with(&done, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(ok);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "置位后不应再执行 UPDATE");
    }

    /// NEW-B-4: 真实库——重试成功时确实清掉残留 bot_assigned 标志。
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
        let ok = reset_bot_assigned_with(&done, || {
            conn.execute("UPDATE tasks SET bot_assigned = 0 WHERE bot_assigned = 1", [])
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        assert!(ok);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks WHERE bot_assigned = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0, "残留 bot_assigned 应被清掉");
        fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod atomic_write_tests {
    use super::*;
    use std::fs;

    /// NEW-B-6: 原子写 happy path——内容完整落盘，tmp 不残留；覆盖已有文件也正常。
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

    /// NEW-B-6: tmp 写入失败（目标目录只读）→ 目标文件保持原状，不被半截覆盖。
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
