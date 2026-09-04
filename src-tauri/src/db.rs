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
    /// T1-1（2026-09-03）：RMW 写回基线 = 调用方读快照时该行的 updated_at。
    /// upsert 写前比对现行行：不一致 → 冲突拒写（Err），防「读旧快照→整行写回」lost-update。
    /// 不映射 DB 列；skip_serializing = 后端事件/导出不下发（防前端 state 残留脏基线），
    /// 仅调用方上行携带。None = 无基线（新建/导入/未读快照），行为同原时间戳守卫。
    #[serde(default, skip_serializing)]
    pub expected_updated_at: Option<i64>,
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
    // 2026-08-28 批次2审计：先拷到临时文件再 rename——原先直拷目标路径，
    // 拷贝中断（磁盘满/断电）留下半拷贝文件，open_db 的 `!db_path.exists()` 守卫
    // 会让下轮永久跳过拷贝，rusqlite 打开截断文件报「database disk image is malformed」
    let tmp = db_path.with_extension("db.copying");
    let copied = std::fs::copy(legacy_db, &tmp)
        .map_err(|e| e.to_string())
        .and_then(|_| {
            std::fs::rename(&tmp, db_path).map_err(|e| e.to_string())
        });
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&tmp);
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
    // 迁移：记忆模块 Step 1（2026-09-04，设计 docs/BOT-MEMORY-DESIGN.md 第 3 节）——
    // bot_facts 补 kind/category/importance/source/access_count/accessed_at 六列
    ensure_bot_facts_memory_columns(&conn)?;
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

/// 迁移：记忆模块 Step 1（2026-09-04，设计第 3 节）——bot_facts 补 6 列
/// （PRAGMA 检查 + ALTER 逐列补齐，幂等，老库平滑迁移；与 ensure_files_column 同模式）。
/// pub：内存库单测与 tests/llm_integration.rs 全链路测试直用（与 bot::run_model_loop_core 同先例）。
pub fn ensure_bot_facts_memory_columns(conn: &rusqlite::Connection) -> Result<(), String> {
    for (col, def) in [
        // fact(事实/偏好) | summary(历史摘要) | reflection(阶段总结)
        ("kind", "TEXT NOT NULL DEFAULT 'fact'"),
        // profile(画像) | preference(偏好) | project(项目上下文) | general
        ("category", "TEXT NOT NULL DEFAULT 'general'"),
        ("importance", "INTEGER NOT NULL DEFAULT 3"), // 1-5
        // user_stated(用户明确说的) | model_inferred(模型/系统生成的)
        ("source", "TEXT NOT NULL DEFAULT 'user_stated'"),
        ("access_count", "INTEGER NOT NULL DEFAULT 0"), // 命中注入次数（Step 2 检索用）
        ("accessed_at", "INTEGER NOT NULL DEFAULT 0"), // 最近命中时间（0=从未命中）
    ] {
        let has: bool = conn
            .prepare("PRAGMA table_info(bot_facts)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == col))
            })
            .unwrap_or(false);
        if !has {
            conn.execute(&format!("ALTER TABLE bot_facts ADD COLUMN {col} {def}"), [])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
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

pub(crate) fn load_workspace(conn: &rusqlite::Connection) -> Result<Vec<WorkspaceItem>, String> {
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
        // 2026-08-28 批次2审计：links JSON 损坏留痕（读成空数组后 upsert 回写 = 静默丢链接）
        let links = match serde_json::from_str(&links) {
            Ok(l) => l,
            Err(_) => {
                eprintln!("[db] 工作区条目 {id} 的 links JSON 损坏，按空读取（原值未动）");
                Vec::new()
            }
        };
        items.push(WorkspaceItem {
            id,
            title,
            collapsed: collapsed.map(|v| v != 0),
            links,
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

/// bot_session_delete 的事务段（抽出供单测直调；锁与 open_db 留在命令层）。
/// 消息与会话同一事务删除，任一失败整体回滚，不留半删状态。
fn bot_session_delete_inner(conn: &mut rusqlite::Connection, id: &str) -> CommandResult<()> {
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.execute("DELETE FROM bot_messages WHERE session_id = ?1", [id])
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.execute("DELETE FROM bot_sessions WHERE id = ?1", [id])
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))
}

/// 删除会话及其全部消息（原子：消息与会话同一事务，任一失败整体回滚）
#[tauri::command]
pub async fn bot_session_delete(app: tauri::AppHandle, id: String) -> CommandResult<()> {
    // B3: 高频写 + 跨表事务 → 主线程会阻塞；扔到 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        bot_session_delete_inner(&mut conn, &id)
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
    // 2026-08-28 批次2审计：单会话历史上限 2000 条——原先全量覆盖写无上限，
    // 长会话每轮对话 O(n) 重写全表（写放大 + WAL 膨胀 + 长事务挤压其它写者）
    const MAX_HISTORY_MSGS: usize = 2000;
    let messages = if messages.len() > MAX_HISTORY_MSGS {
        &messages[messages.len() - MAX_HISTORY_MSGS..]
    } else {
        messages
    };
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

// ───────────────────────── 记忆模块 Step 1（2026-09-04，设计 docs/BOT-MEMORY-DESIGN.md） ─────────────────────────
//
// 所有长期记忆（fact/summary/reflection）统一存 bot_facts 一张表，kind 列区分。
// 纯函数取 &Connection（内存库可单测，与 bot.rs fact_* 同模式）；async 包装走
// DB_WRITE_LOCK + spawn_blocking（与 bot_history_save 同模式）。Step 1 只承接
// 「截断即摘要」写入路径；remember_fact 的 200 上限逻辑在 bot.rs，Step 2 才统一。

/// 记忆总条数上限（fact/summary/reflection 共享；超出走惰性淘汰）
pub const MAX_BOT_MEMORIES: i64 = 300;

/// Reflection 触发阈值/批量（设计 7.3）：summary 攒够 10 条合成一条 reflection
pub const REFLECTION_BATCH: i64 = 10;

/// 惰性淘汰（设计第 3 节）：插入新条目前检查，达 MAX_BOT_MEMORIES 时删淘汰分最低的
/// 一条（importance 升序 → accessed_at 升序）；importance>=4 且 kind='fact' 的画像
/// 不淘汰——没有可淘汰条目时报错，让写入方（模型）自己删。
fn memory_evict_if_needed(conn: &rusqlite::Connection) -> Result<(), String> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM bot_facts", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if count < MAX_BOT_MEMORIES {
        return Ok(());
    }
    let victim: Option<String> = conn
        .query_row(
            "SELECT key FROM bot_facts
             WHERE NOT (kind = 'fact' AND importance >= 4)
             ORDER BY importance ASC, accessed_at ASC
             LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    match victim {
        Some(key) => {
            conn.execute("DELETE FROM bot_facts WHERE key = ?1", [&key])
                .map_err(|e| e.to_string())?;
            Ok(())
        }
        None => Err(format!(
            "记忆已达 {MAX_BOT_MEMORIES} 条上限且剩余全是受保护的高重要度事实，请先删除不需要的"
        )),
    }
}

/// 写入一条 summary/reflection 记忆（同 key 覆盖不占新名额）。
/// key 由调用方按 `summary:{session_id}:{ts}` / `reflection:{ts}` 生成；
/// category 落库默认 'general'，source 固定 'model_inferred'（系统生成而非用户口述）。
/// pub：tests/llm_integration.rs 全链路测试直用。
pub fn memory_insert(
    conn: &rusqlite::Connection,
    key: &str,
    value: &str,
    kind: &str,
    importance: i64,
    now: i64,
) -> Result<(), String> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM bot_facts WHERE key = ?1",
            rusqlite::params![key],
            |_| Ok(()),
        )
        .is_ok();
    if !exists {
        memory_evict_if_needed(conn)?;
    }
    conn.execute(
        "INSERT INTO bot_facts (key, value, updated_at, kind, importance, source)
         VALUES (?1, ?2, ?3, ?4, ?5, 'model_inferred')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        rusqlite::params![key, value, now, kind, importance],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 指定 kind 的条数（Reflection 触发判定用）
pub fn memory_count_by_kind(conn: &rusqlite::Connection, kind: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM bot_facts WHERE kind = ?1",
        rusqlite::params![kind],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// 最旧的 n 条指定 kind 记忆（key, value），按写入时间升序——Reflection 取最旧 10 条 summary
pub fn memory_oldest_by_kind(
    conn: &rusqlite::Connection,
    kind: &str,
    n: i64,
) -> Result<Vec<(String, String)>, String> {
    conn.prepare("SELECT key, value FROM bot_facts WHERE kind = ?1 ORDER BY updated_at ASC LIMIT ?2")
        .and_then(|mut s| {
            s.query_map(rusqlite::params![kind, n], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map(|iter| iter.filter_map(|r| r.ok()).collect::<Vec<_>>())
        })
        .map_err(|e| e.to_string())
}

/// 删除指定 keys（Reflection 合成后清原摘要）；返回实际删除条数
pub fn memory_delete_keys(conn: &rusqlite::Connection, keys: &[String]) -> Result<usize, String> {
    let mut n = 0;
    for k in keys {
        n += conn
            .execute("DELETE FROM bot_facts WHERE key = ?1", rusqlite::params![k])
            .map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// 摘要落库（截断即摘要，设计 5.2）：kind=summary, importance=2，
/// key=summary:{session_id}:{ts}（session_id 缺失归 'default'，保留可追溯性）。
/// 返回落库后最旧的 REFLECTION_BATCH 条 summary（调用方据此判定是否触发 Reflection）。
pub async fn bot_memory_save_summary(
    app: tauri::AppHandle,
    session_id: Option<String>,
    summary: String,
) -> CommandResult<Vec<(String, String)>> {
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = open_db(&app)?;
        let now = chrono::Utc::now().timestamp_millis();
        let key = format!("summary:{}:{now}", session_id.as_deref().unwrap_or("default"));
        memory_insert(&conn, &key, &summary, "summary", 2, now).map_err(CommandError::DbError)?;
        memory_oldest_by_kind(&conn, "summary", REFLECTION_BATCH).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("记忆写入线程 join 失败：{e}")))?
}

/// Reflection 落库（设计 7.3）：插入 kind=reflection, importance=3（key=reflection:{ts}），
/// 与被合并的原 summary keys 的删除放在同一事务（半完成不留中间态）。
pub async fn bot_memory_apply_reflection(
    app: tauri::AppHandle,
    delete_keys: Vec<String>,
    reflection: String,
) -> CommandResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        let tx = conn
            .transaction()
            .map_err(|e| CommandError::DbError(e.to_string()))?;
        let now = chrono::Utc::now().timestamp_millis();
        memory_insert(&tx, &format!("reflection:{now}"), &reflection, "reflection", 3, now)
            .map_err(CommandError::DbError)?;
        memory_delete_keys(&tx, &delete_keys).map_err(CommandError::DbError)?;
        tx.commit()
            .map_err(|e| CommandError::DbError(e.to_string()))
    })
    .await
    .map_err(|e| CommandError::from(format!("Reflection 写入线程 join 失败：{e}")))?
}

// ───────────────────────── 记忆模块 Step 2（2026-09-05，设计第 4/6 节）：检索打分与注入取数 ─────────────────────────
//
// 零索引设施（设计第 1 节）：全表 ≤300 条，SELECT * + Rust 打分是微秒级开销。
// 打分与冲突提示共用同一套 extract_keywords/memory_score，保证「写入即检索」与
// 「读取注入」的相关性口径一致。

/// 检索注入条数（设计第 4 节：top-5 注入）
pub const MEMORY_TOP_N: usize = 5;

/// 近期摘要条数（设计第 4/6 节：最近 3 条 summary/reflection，兼作检索全 0 分的回退兜底）
pub const MEMORY_RECENT_N: usize = 3;

/// 记忆条目快照（检索打分与记忆块拼装用，全列）
#[derive(Clone, Debug)]
pub struct MemoryItem {
    pub key: String,
    pub value: String,
    pub kind: String,
    pub category: String,
    pub importance: i64,
    pub source: String,
    pub access_count: i64,
    pub accessed_at: i64,
    pub updated_at: i64,
}

/// 关键词提取（设计第 4 节，无依赖）：英文/数字连续段转小写成一个词；
/// 连续中日韩字符段取字符 bigram（"记忆模块" → {记忆, 忆模, 模块}）；
/// 单字 CJK 段保留单字（否则单字查询提取不出任何词，永远 0 分）。去重保序。
pub(crate) fn extract_keywords(text: &str) -> Vec<String> {
    fn is_cjk(c: char) -> bool {
        matches!(c,
            '\u{3400}'..='\u{4DBF}'   // CJK 扩展 A
            | '\u{4E00}'..='\u{9FFF}' // CJK 统一表意文字
            | '\u{3040}'..='\u{30FF}' // 日文假名
            | '\u{AC00}'..='\u{D7AF}' // 韩文音节
        )
    }
    fn flush_cjk(cjk: &mut Vec<char>, out: &mut Vec<String>) {
        if cjk.len() == 1 {
            out.push(cjk[0].to_string());
        } else {
            for w in cjk.windows(2) {
                out.push(w.iter().collect());
            }
        }
        cjk.clear();
    }
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut cjk: Vec<char> = Vec::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            flush_cjk(&mut cjk, &mut out);
            word.push(c.to_ascii_lowercase());
        } else {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            if is_cjk(c) {
                cjk.push(c);
            } else {
                flush_cjk(&mut cjk, &mut out);
            }
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    flush_cjk(&mut cjk, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|k| seen.insert(k.clone()));
    out
}

/// 单条打分（设计第 4 节）：
/// score = keyword_overlap × (importance/3) × exp(-age_days/90) × 访问强化
/// - keyword_overlap = 命中词数 / 提取词数（提取词数为 0 → 全 0 分，调用方回退兜底）
/// - age_days 从 accessed_at（为 0 用 updated_at）起算——被反复想起的记忆衰减更慢
/// - 访问强化取 1 + ln(1+access_count) 而非设计原文的 ln(1+access_count)：
///   后者在 access_count=0 时 ln(1)=0，新记忆全 0 分 → 永不注入 → 永不命中，
///   检索永久锁死。加 1 保底保持单调递增且新记忆无加成（详见 Step 2 报告）。
pub(crate) fn memory_score(query_kws: &[String], item: &MemoryItem, now: i64) -> f64 {
    if query_kws.is_empty() {
        return 0.0;
    }
    let item_kws = extract_keywords(&format!("{} {}", item.key, item.value));
    let hits = query_kws.iter().filter(|k| item_kws.contains(*k)).count();
    if hits == 0 {
        return 0.0;
    }
    let overlap = hits as f64 / query_kws.len() as f64;
    let base = if item.accessed_at > 0 {
        item.accessed_at
    } else {
        item.updated_at
    };
    let age_days = ((now - base) as f64 / 86_400_000.0).max(0.0);
    overlap
        * (item.importance as f64 / 3.0)
        * (-age_days / 90.0).exp()
        * (1.0 + (1.0 + item.access_count as f64).ln())
}

/// 全表快照（≤300 条全扫，微秒级）
pub fn memory_load_all(conn: &rusqlite::Connection) -> Result<Vec<MemoryItem>, String> {
    conn.prepare(
        "SELECT key, value, kind, category, importance, source, access_count, accessed_at, updated_at
         FROM bot_facts",
    )
    .and_then(|mut s| {
        s.query_map([], |r| {
            Ok(MemoryItem {
                key: r.get(0)?,
                value: r.get(1)?,
                kind: r.get(2)?,
                category: r.get(3)?,
                importance: r.get(4)?,
                source: r.get(5)?,
                access_count: r.get(6)?,
                accessed_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })
        .map(|iter| iter.filter_map(|r| r.ok()).collect::<Vec<_>>())
    })
    .map_err(|e| e.to_string())
}

/// 检索 top-n：打分后按分降序取前 n（score>0 即 keyword_overlap>0）。
/// only_kind 过滤可选（remember_fact 冲突提示只查 fact）。纯读，不刷新访问计数
/// （注入路径的访问强化在 memory_injection_snapshot 里做）。
pub fn memory_search(
    conn: &rusqlite::Connection,
    query: &str,
    top_n: usize,
    now: i64,
    only_kind: Option<&str>,
) -> Result<Vec<MemoryItem>, String> {
    let query_kws = extract_keywords(query);
    if query_kws.is_empty() {
        return Ok(Vec::new());
    }
    let mut scored: Vec<(f64, MemoryItem)> = memory_load_all(conn)?
        .into_iter()
        .filter(|m| only_kind.map_or(true, |k| m.kind == k))
        .map(|m| (memory_score(&query_kws, &m, now), m))
        .filter(|(s, _)| *s > 0.0)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(top_n).map(|(_, m)| m).collect())
}

/// 注入取数三段原料（设计第 6 节拼装顺序）
pub struct MemoryInjection {
    /// importance>=4 的 fact（无条件进「用户画像与偏好」段）
    pub pinned: Vec<MemoryItem>,
    /// 检索 top-5（进「相关记忆」段；已原子刷新 access_count+1 / accessed_at=now）
    pub hits: Vec<MemoryItem>,
    /// 最近 3 条 summary/reflection（进「近期摘要」段；检索全 0 分时的回退兜底）
    pub recent: Vec<MemoryItem>,
}

/// 注入取数内核（纯连接版，内存库可单测）：pinned / hits / recent 三段互不重复；
/// 命中条目在同一连接内原子刷新访问计数（设计 7.4 访问强化）。
pub fn memory_injection_snapshot(
    conn: &rusqlite::Connection,
    query: &str,
    now: i64,
) -> Result<MemoryInjection, String> {
    let all = memory_load_all(conn)?;
    let pinned: Vec<MemoryItem> = all
        .iter()
        .filter(|m| m.kind == "fact" && m.importance >= 4)
        .cloned()
        .collect();
    // 检索 top-5：已 pinned 的不重复进「相关记忆」段
    let query_kws = extract_keywords(query);
    let mut scored: Vec<(f64, MemoryItem)> = if query_kws.is_empty() {
        Vec::new()
    } else {
        all.iter()
            .filter(|m| !pinned.iter().any(|p| p.key == m.key))
            .map(|m| (memory_score(&query_kws, m, now), m.clone()))
            .filter(|(s, _)| *s > 0.0)
            .collect()
    };
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let hits: Vec<MemoryItem> = scored.into_iter().take(MEMORY_TOP_N).map(|(_, m)| m).collect();
    // 命中即访问强化：原子 access_count+1 / accessed_at=now（设计第 4 节）
    for m in &hits {
        conn.execute(
            "UPDATE bot_facts SET access_count = access_count + 1, accessed_at = ?1 WHERE key = ?2",
            rusqlite::params![now, m.key],
        )
        .map_err(|e| e.to_string())?;
    }
    // 近期摘要：最近 3 条 summary/reflection，已被检索命中的不重复
    let recent: Vec<MemoryItem> = {
        let mut rs: Vec<MemoryItem> = all
            .iter()
            .filter(|m| {
                (m.kind == "summary" || m.kind == "reflection")
                    && !hits.iter().any(|h| h.key == m.key)
            })
            .cloned()
            .collect();
        rs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        rs.truncate(MEMORY_RECENT_N);
        rs
    };
    Ok(MemoryInjection { pinned, hits, recent })
}

/// 注入取数 async 薄壳：DB_WRITE_LOCK + spawn_blocking（与 bot_memory_save_summary 同模式）
pub async fn bot_memory_injection(
    app: tauri::AppHandle,
    query: String,
) -> CommandResult<MemoryInjection> {
    tauri::async_runtime::spawn_blocking(move || {
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let conn = open_db(&app)?;
        let now = chrono::Utc::now().timestamp_millis();
        memory_injection_snapshot(&conn, &query, now).map_err(CommandError::DbError)
    })
    .await
    .map_err(|e| CommandError::from(format!("记忆检索线程 join 失败：{e}")))?
}

/// T1-1（2026-09-03）：写冲突错误前缀——RMW 基线比对失败（lost-update 防护拒写）。
/// 错误以 String 穿透多层（CommandError::from(String) → Internal），调用方按前缀分流
/// （如本地 API 映射 409；其余调用方按写失败处理，数据未被覆盖）。
pub const CONFLICT_ERR_PREFIX: &str = "写冲突";

/// 2026-09-04 审计 P2-4：「行存在性」基线哨兵。老行 updated_at 为 NULL 时 RMW 调用方
/// 无法做时间戳比对（原先 expected_updated_at=None = 跳过基线检查，最需要防
/// lost-update 的老行反而裸奔）。以此哨兵为基线表示「行必须仍存在且 updated_at
/// 仍为 NULL」——行被删（无行）或被改（任何写者落库必写非 NULL 时间戳）都判冲突。
/// 取 i64::MIN 保证与任何合法毫秒时间戳不撞。
pub const BASELINE_NULL_ROW: i64 = i64::MIN;

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
        // T1-1 写前重读比对（2026-09-03）：调用方给了读快照基线（expected_updated_at）时，
        // 现行行 updated_at 必须仍等于基线——否则「读旧快照→修改→整行写回」窗口内有
        // 其他写者改过/删过该行，整行写回会覆盖对方修改 → 拒写报错，不覆盖。
        // 比对与写入在同一事务（且写路径持 DB_WRITE_LOCK，进程内写者串行）→ 原子。
        // cur=None 含「行不存在（快照后被删）」与「老行 NULL updated_at」两种，均判冲突拒写：
        // 前者防复活已删行，后者因基线语义是「读到过的确定时间戳」，NULL 无从匹配。
        // 2026-09-04 审计 P2-4：不再 flatten——区分「行被删」（None）与「老行 NULL」
        // （Some(None)），后者配合 BASELINE_NULL_ROW 哨兵走「行存在性」基线。
        if let Some(expected) = t.expected_updated_at {
            use rusqlite::OptionalExtension;
            let cur: Option<Option<i64>> = conn
                .query_row(
                    "SELECT updated_at FROM tasks WHERE id = ?1",
                    [&t.id],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let conflict = if expected == BASELINE_NULL_ROW {
                // 行存在性基线：行仍在且 updated_at 仍为 NULL 才放行；
                // 被删（None）或被改（Some(Some(_))）都判冲突
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
                    "{CONFLICT_ERR_PREFIX}：任务 {} 读快照后已被其他写者{}，本次整行写回被拒（基线 updated_at={baseline_desc}，现行 {cur_flat:?}）",
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
        // 行内 JSON 字段损坏检测（2026-08-28 批次2审计）：原值非空但解析失败 → 留痕。
        // 读成 None 后任何整行 upsert 会把 NULL 写回 = 静默丢数据；彻底防护需字段级
        // 合并写入（架构改造，见 AUDIT-DATA 报告）——这里至少让损坏可见、可诊断。
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
            expected_updated_at: None, // 库读出的快照不自带基线；由 RMW 调用方写回前设置
        });
    }
    Ok(tasks)
}

/// 迁移方案2 的 data.json：json 里有库里缺的任务就补回。
/// P2-7：原触发条件「库 count==0」——用户删任务后重启、老 data.json 还在时不再迁移，
/// 数据静默丢失。只补库中缺失的 id（已存在的 id 不用 json 旧值覆盖，防回滚用户的新编辑）。
/// 2026-08-28 批次2审计 B2-P0 修正：
/// 1) 触发判定从「计数比较」（json 条数 > 库条数）改为「集合差」——计数比较在
///    「json ≤ 库但 json 含库缺失 id」时漏迁；
/// 2) 评估成功后无论是否补了内容，都把 data.json 改名退役（data.json.migrated，可人工找回）——
///    原先「不触发就保留文件」，而删除任务是硬删：库计数将来跌穿 json 计数时
///    陈年 json 会把已删除任务全部复活。
fn migrate_data_json(app: &tauri::AppHandle, conn: &mut rusqlite::Connection) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = migrate_data_json_file(&dir.join("data.json"), conn);
}

/// P2-7 可测内核：返回是否执行了迁移（成功补回缺失任务并退役旧文件）。
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
    // 集合差判定：只补库中缺失的 id——INSERT OR REPLACE 会用 json 旧值覆盖已有行的新编辑
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
    // 退役 = 改名而非删除（数据可人工找回）；改名失败保留原文件，下次重试
    let retire = |file: &std::path::Path| {
        let _ = std::fs::rename(file, file.with_extension("json.migrated"));
    };
    if missing.is_empty() {
        // json 内容已全部在库里 → 冗余残留，直接退役（防未来硬删后复活）
        retire(file);
        return false;
    }
    let Ok(tx) = conn.transaction() else { return false };
    if upsert_tasks(&tx, &missing).is_ok() && tx.commit().is_ok() {
        retire(file);
        return true;
    }
    false
}

/// 写操作全局锁：主窗口（db_upsert/db_delete）与本地 API 线程共享同一把锁，
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

/// 导出路径校验（2026-08-27 SEC-P1-3）：导出命令前端直达，路径限 .json——
/// 防任意路径写覆盖用户文件（正常路径经系统保存对话框取得，本就用户授权）
fn check_export_path(path: &str) -> CommandResult<()> {
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

/// 导出任务卡数据：全量任务（含归档、回收站）序列化为 JSON 文件，返回条数
#[tauri::command]
pub async fn tasks_export(app: tauri::AppHandle, path: String) -> CommandResult<usize> {
    check_export_path(&path)?;
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
            // B4: NULL updated_at 兼容（2026-08-14 前老行 updated_at 为 NULL）
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

/// 导出工作区链接数据：全量 WorkspaceItem 序列化为 JSON 文件，返回条数。
/// （与 tasks_export 风格一致；workspace 数据独立存于 workspace_items 表，与任务数据物理隔离）
#[tauri::command]
pub async fn workspace_export(app: tauri::AppHandle, path: String) -> CommandResult<usize> {
    check_export_path(&path)?;
    // B3: 同步读 DB + JSON 序列化 + 文件写 阻塞主线程；扔 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let conn = open_db(&app)?;
        let items = load_workspace(&conn)?;
        let json = serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?;
        // NEW-B-6: 原子写——崩溃不留半截 JSON（先写同目录 tmp 再 rename）
        atomic_write(std::path::Path::new(&path), &json)
            .map_err(|e| format!("写入文件失败：{e}"))?;
        Ok(items.len())
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区导出线程 join 失败：{e}")))?
}

/// workspace_import 的合并段（抽出供单测直调；读文件/解析/锁/open_db 留在命令层）。
/// 按 id 并集合并，同 id 保留 updated_at 更晚者；空 id 跳过；单事务，中途失败整体回滚。
/// 返回写入条数。
fn workspace_import_merge(
    conn: &mut rusqlite::Connection,
    ext: &[WorkspaceItem],
) -> Result<usize, String> {
    use rusqlite::OptionalExtension;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut merged = 0usize;
    for it in ext {
        if it.id.trim().is_empty() {
            continue; // 跳过无 id 的脏数据
        }
        // 同 tasks_import：NULL updated_at 兼容（库内 NULL = 0，外部 None = 0）
        let cur: Option<Option<i64>> = tx
            .query_row(
                "SELECT updated_at FROM workspace_items WHERE id = ?1",
                rusqlite::params![it.id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let cur_ua = cur.flatten().unwrap_or(0);
        let take = it.updated_at.unwrap_or(0) > cur_ua;
        if take {
            // inline upsert（与 upsert_workspace 同 SQL），不复用 fn 避免事务嵌套；
            // mid-loop 任何错整体回滚，事务不半截提交
            tx.execute(
                "INSERT INTO workspace_items (id, title, collapsed, links, ord, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET
                   title=excluded.title, collapsed=excluded.collapsed,
                   links=excluded.links, ord=excluded.ord,
                   updated_at=excluded.updated_at",
                rusqlite::params![
                    it.id,
                    it.title,
                    it.collapsed.map(|v| v as i64),
                    serde_json::to_string(&it.links).unwrap_or_else(|_| "[]".into()),
                    it.order,
                    it.updated_at,
                ],
            )
            .map_err(|e| e.to_string())?;
            merged += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(merged)
}

/// 从 JSON 文件导入工作区链接数据：按 id 并集合并，同 id 保留 updated_at 更晚者。返回写入条数。
/// （与 tasks_import 语义一致；workspace 数据独立存于 workspace_items 表）
#[tauri::command]
pub async fn workspace_import(app: tauri::AppHandle, path: String) -> CommandResult<usize> {
    // B3: 大文件读 + 解析 + 长事务；扔 spawn_blocking。
    tauri::async_runtime::spawn_blocking(move || {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("无法读取所选文件：{e}"))?;
        let ext: Vec<WorkspaceItem> =
            serde_json::from_str(&raw).map_err(|e| format!("不是有效的工作区链接 JSON：{e}"))?;
        if ext.is_empty() {
            return Ok(0);
        }
        let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut conn = open_db(&app)?;
        workspace_import_merge(&mut conn, &ext).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作区导入线程 join 失败：{e}")))?
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
            expected_updated_at: None,
        }
    }

    /// 删任务后重启场景：库里只剩 t1，老 data.json 还有 t1/t2/t3 → 首次评估补回 t2/t3，
    /// 且已存在的 t1 不得被 json 旧值覆盖（B2 守卫之外再加 id 过滤）。
    /// 2026-08-28 批次2：迁移/评估成功后 data.json 改名退役——再次评估不再复活已删任务。
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
            "json 含库缺失的 t2/t3 必须触发迁移补回"
        );
        let tasks = load_all(&conn).unwrap();
        assert_eq!(tasks.len(), 3, "t2/t3 应补回");
        let t1 = tasks.iter().find(|t| t.id == "t1").unwrap();
        assert_eq!(t1.title, "新标题", "已有 id 不得被 json 旧值覆盖");
        assert!(!file.exists(), "迁移成功后 data.json 应退役（改名）");
        assert!(
            dir.join("data.json.migrated").exists(),
            "退役文件保留为 data.json.migrated（可人工找回）"
        );
        // B2-P0 关键回归：退役后用户硬删 t2，再次评估不得复活
        delete_tasks(&conn, &["t2".to_string()]).unwrap();
        assert!(
            !migrate_data_json_file(&file, &mut conn),
            "文件已退役：不存在即不评估"
        );
        let tasks = load_all(&conn).unwrap();
        assert_eq!(tasks.len(), 2, "已删任务不得被残留 json 复活");
        fs::remove_dir_all(&dir).ok();
    }

    /// json 内容已全部在库里 → 不迁移，但文件同样退役（防未来硬删后计数复活）
    #[test]
    fn migrate_data_json_skips_when_db_not_behind() {
        let (dir, mut conn) = setup_tasks_db();
        upsert_tasks(&conn, &[mk_task("t1", "a"), mk_task("t2", "b")]).unwrap();
        let file = dir.join("data.json");
        fs::write(&file, serde_json::to_string(&vec![mk_task("t1", "old")]).unwrap()).unwrap();

        assert!(
            !migrate_data_json_file(&file, &mut conn),
            "json 无库缺失 id 不得迁移"
        );
        assert!(!file.exists(), "评估成功后文件退役（2026-08-28 语义变化）");
        assert!(dir.join("data.json.migrated").exists());
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
    /// 直调生产 bot_session_delete_inner（原来内联裸 SQL 复刻命令体，生产改动测试不红）。
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
        bot_session_delete_inner(&mut conn, "s1").unwrap();

        let sess_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_sessions WHERE id = 's1'", [], |r| r.get(0))
            .unwrap();
        let msg_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sess_count, 0, "提交后会话应被删除");
        assert_eq!(msg_count, 0, "提交后消息应被删除");

        // 原子性反向验证：第二条 DELETE（会话）注入失败 → 第一条（消息）也必须回滚
        conn.execute("INSERT INTO bot_sessions VALUES ('s2', 'T', 1000, 1000)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s2', 'user', 'm2', 1000)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_sess_del BEFORE DELETE ON bot_sessions
             WHEN OLD.id = 's2' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
        assert!(
            bot_session_delete_inner(&mut conn, "s2").is_err(),
            "trigger 注入失败必须返回 Err"
        );
        let sess_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_sessions WHERE id = 's2'", [], |r| r.get(0))
            .unwrap();
        let msg_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM bot_messages WHERE session_id = 's2'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sess_count, 1, "回滚后会话必须还在");
        assert_eq!(msg_count, 1, "回滚后消息必须还在（不留半删状态）");

        fs::remove_dir_all(&dir).ok();
    }

    /// B4: 模拟 2026-08-14 前的任务行（updated_at IS NULL），验证 tasks_import
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

    /// B2: upsert WHERE 守卫防 lost update——T1-1 起改为直接调生产 upsert_tasks
    /// （原测试内联复刻生产 SQL，守卫改动后测试不同步即失效的弱断言已顺手修掉）。
    /// 五场景：incoming>current / incoming<current / 相等 / 老 NULL 行 / incoming NULL
    #[test]
    fn upsert_where_guard_prevents_lost_update() {
        let (dir, conn) = setup_tasks_db();
        let title_of = |conn: &rusqlite::Connection, id: &str| -> String {
            conn.query_row(
                "SELECT title FROM tasks WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        // 老 NULL 行（2026-08-14 前 schema 遗留）：生产 upsert 写全列，NULL 行只能 SQL 直插
        conn.execute(
            "INSERT INTO tasks (id, title, col) VALUES ('legacy', 'legacy-row', 'todo')",
            [],
        )
        .unwrap();

        // 场景 1: incoming(100) > current(50) → 更新
        let mut t = mk_task("t1", "old-50");
        t.updated_at = Some(50);
        upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
        t.title = "new-100".into();
        t.updated_at = Some(100);
        upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
        assert_eq!(title_of(&conn, "t1"), "new-100", "场景 1: 更新的应压过老的");

        // 场景 2: incoming(60) < current(100) → 跳过（lost update 防护）
        t.title = "old-snapshot-60".into();
        t.updated_at = Some(60);
        upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
        assert_eq!(title_of(&conn, "t1"), "new-100", "场景 2: 更老的不应压过更新的");

        // 场景 3: 相等 timestamp → 允许更新
        t.title = "equal-100".into();
        t.updated_at = Some(100);
        upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
        assert_eq!(title_of(&conn, "t1"), "equal-100", "场景 3: 相等 timestamp 仍允许更新");

        // 场景 4: 老 NULL 行被任何 incoming 覆盖
        let mut l = mk_task("legacy", "new-over-legacy");
        l.updated_at = Some(5);
        upsert_tasks(&conn, &[l]).unwrap();
        assert_eq!(
            title_of(&conn, "legacy"),
            "new-over-legacy",
            "场景 4: 老 NULL 行被任何 incoming 覆盖"
        );

        // 场景 5: incoming NULL 不应覆盖 current 有值
        conn.execute("UPDATE tasks SET title='keep-me', updated_at=200 WHERE id='t1'", [])
            .unwrap();
        t.title = "incoming-null".into();
        t.updated_at = None;
        upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
        assert_eq!(title_of(&conn, "t1"), "keep-me", "场景 5: incoming NULL 不应覆盖 current 有值");

        fs::remove_dir_all(&dir).ok();
    }

    /// T1-1（2026-09-03）核心回归：两写者读同一快照后交错写回——
    /// 后写者基线比对失败被拒（Err 含 CONFLICT_ERR_PREFIX），先写者的字段修改不丢；
    /// 后写者重读刷新基线后重试可成功。另覆盖「快照后行被删 → 拒写防复活」。
    #[test]
    fn upsert_expected_baseline_rejects_stale_write() {
        let (dir, conn) = setup_tasks_db();
        let mut seed = mk_task("t1", "原始");
        seed.updated_at = Some(100);
        seed.note = Some("原始备注".into());
        upsert_tasks(&conn, &[seed]).unwrap();

        // 写者 A / B 读同一快照（updated_at=100）
        let snap_a = load_all(&conn).unwrap().into_iter().next().unwrap();
        let snap_b = snap_a.clone();
        assert_eq!(snap_a.updated_at, Some(100));

        // A 先写回：改标题，刷新时间戳，带基线 → 放行
        let mut a = snap_a.clone();
        a.expected_updated_at = a.updated_at; // RMW 调用方的标准接线：基线=快照 updated_at
        a.title = "A改的标题".into();
        a.updated_at = Some(200);
        upsert_tasks(&conn, std::slice::from_ref(&a)).unwrap();

        // B 后写回：基于同一旧快照改备注，时间戳更新（300>200，旧时间戳守卫会放行）→ 必须被基线拒
        let mut b = snap_b;
        b.expected_updated_at = b.updated_at;
        b.note = Some("B改的备注".into());
        b.updated_at = Some(300);
        let err = upsert_tasks(&conn, std::slice::from_ref(&b)).unwrap_err();
        assert!(
            err.starts_with(CONFLICT_ERR_PREFIX),
            "后写者基线过期必须拒写；got: {err}"
        );
        let cur = load_all(&conn).unwrap().into_iter().next().unwrap();
        assert_eq!(cur.title, "A改的标题", "先写者的字段修改不得被覆盖");
        assert_eq!(cur.note.as_deref(), Some("原始备注"), "被拒写者的修改不得落库");
        assert_eq!(cur.updated_at, Some(200));

        // B 重读刷新基线后重试 → 放行（冲突可见、可恢复，而非静默丢）
        let mut b2 = load_all(&conn).unwrap().into_iter().next().unwrap();
        b2.expected_updated_at = b2.updated_at;
        b2.note = Some("B改的备注".into());
        b2.updated_at = Some(300);
        upsert_tasks(&conn, std::slice::from_ref(&b2)).unwrap();
        let cur = load_all(&conn).unwrap().into_iter().next().unwrap();
        assert_eq!(cur.title, "A改的标题");
        assert_eq!(cur.note.as_deref(), Some("B改的备注"));

        // 快照后行被删：带基线写回 → 拒写（防复活已删行）
        delete_tasks(&conn, &["t1".to_string()]).unwrap();
        let err = upsert_tasks(&conn, std::slice::from_ref(&b2)).unwrap_err();
        assert!(err.starts_with(CONFLICT_ERR_PREFIX), "行已删必须拒写；got: {err}");
        assert!(load_all(&conn).unwrap().is_empty(), "被拒写不得复活已删行");

        // 无基线（expected_updated_at=None）保持原行为：新建直插、时间戳守卫兜底
        let mut fresh = mk_task("t2", "新建无基线");
        fresh.updated_at = Some(50);
        upsert_tasks(&conn, &[fresh]).unwrap();
        assert_eq!(load_all(&conn).unwrap().len(), 1);

        fs::remove_dir_all(&dir).ok();
    }

    /// 2026-09-04 审计 P2-4：老行 updated_at 为 NULL 时用「行存在性」哨兵基线
    /// （BASELINE_NULL_ROW）——行原样放行；行被改（updated_at 变非 NULL）或
    /// 被删都拒写。原先 NULL 行基线是 None → 跳过比对，老行无 lost-update 防护。
    #[test]
    fn upsert_null_row_existence_baseline() {
        let (dir, conn) = setup_tasks_db();
        // 模拟迁移前的老行：updated_at 为 NULL
        let mut seed = mk_task("t1", "老行");
        seed.updated_at = None;
        upsert_tasks(&conn, &[seed]).unwrap();

        // 行原样（仍在且仍 NULL）→ 放行
        let mut a = load_all(&conn).unwrap().into_iter().next().unwrap();
        assert_eq!(a.updated_at, None);
        a.expected_updated_at = Some(BASELINE_NULL_ROW);
        a.title = "放行".into();
        a.updated_at = Some(100);
        upsert_tasks(&conn, std::slice::from_ref(&a)).unwrap();
        assert_eq!(load_all(&conn).unwrap()[0].title, "放行");

        // 快照时行是 NULL（基线=行存在性），但窗口内其他写者已改（updated_at=100 非 NULL）→ 拒
        let mut b = mk_task("t1", "覆盖者");
        b.expected_updated_at = Some(BASELINE_NULL_ROW);
        b.updated_at = Some(200);
        let err = upsert_tasks(&conn, std::slice::from_ref(&b)).unwrap_err();
        assert!(
            err.starts_with(CONFLICT_ERR_PREFIX),
            "老行被改后行存在性基线必须拒写；got: {err}"
        );
        assert_eq!(
            load_all(&conn).unwrap()[0].title,
            "放行",
            "被拒写不得覆盖现行行"
        );

        // 快照后行被删 → 拒写（防复活）
        delete_tasks(&conn, &["t1".to_string()]).unwrap();
        let err = upsert_tasks(&conn, std::slice::from_ref(&b)).unwrap_err();
        assert!(
            err.starts_with(CONFLICT_ERR_PREFIX),
            "行已删必须拒写；got: {err}"
        );
        assert!(load_all(&conn).unwrap().is_empty(), "被拒写不得复活已删行");

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

    /// NEW-B-3: workspace_* 命令改 async + spawn_blocking 后，命令体内的数据路径语义不变。
    /// 桥接层（spawn_blocking + join 错误映射）是无逻辑薄壳且依赖 AppHandle 无法单测；
    /// 这里直调生产 helper（upsert/load/delete）覆盖命令体真正干活的部分
    /// （原测试内联复刻桥接结构，生产命令体改动不会让它变红，属弱断言，已去复刻）。
    #[test]
    fn workspace_commands_spawn_blocking_bridge() {
        let dir = std::env::temp_dir().join(format!("wm-ws-async-{}", uuid::Uuid::new_v4()));
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
            title: "T".into(),
            collapsed: None,
            links: vec![],
            order: None,
            updated_at: Some(1),
        };

        // upsert → load 回读（生产 helper，同 workspace_upsert / workspace_load 命令体所调）
        upsert_workspace(&mut conn, &[item]).unwrap();
        let items = load_workspace(&conn).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "T");

        // delete（同 workspace_delete 命令体所调）
        delete_workspace(&mut conn, &["w1".to_string()]).unwrap();
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

    /// workspace_import 合并语义 + 跳空 id 脏数据（与 tasks_import 语义一致）：
    /// 同 id 保留 updated_at 更晚者；新 id 直接写入；空 id 跳过。
    /// 直调生产 workspace_import_merge（原来 inline 复刻合并循环 SQL，生产改动测试不红）。
    #[test]
    fn workspace_import_merges_by_updated_at_and_skips_empty_id() {
        let dir = std::env::temp_dir().join(format!("wm-ws-imp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE workspace_items (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
               links TEXT NOT NULL, ord REAL, updated_at INTEGER);
             INSERT INTO workspace_items VALUES ('a','cur-50',NULL,'[]',NULL,50);
             INSERT INTO workspace_items VALUES ('b','cur-200',NULL,'[]',NULL,200);",
        )
        .unwrap();
        let mk = |id: &str, title: &str, ua: i64| WorkspaceItem {
            id: id.into(),
            title: title.into(),
            collapsed: None,
            links: vec![],
            order: None,
            updated_at: Some(ua),
        };
        let incoming = vec![
            mk("a", "in-100", 100), // 同 id · incoming > cur → UPDATE
            mk("b", "in-180", 180), // 同 id · incoming < cur → 跳过
            mk("c", "in-5", 5),     // 新 id → INSERT
            mk("", "blank", 999),   // 空 id → 跳过
        ];
        let merged = workspace_import_merge(&mut conn, &incoming).unwrap();

        assert_eq!(merged, 2, "应写 a（UPDATE）+ c（INSERT）；跳过 b + 空 id");
        let a: String = conn.query_row("SELECT title FROM workspace_items WHERE id='a'", [], |r| r.get(0)).unwrap();
        assert_eq!(a, "in-100", "a 被新覆盖");
        let b: String = conn.query_row("SELECT title FROM workspace_items WHERE id='b'", [], |r| r.get(0)).unwrap();
        assert_eq!(b, "cur-200", "b 未被覆盖（180<200）");
        let c: String = conn.query_row("SELECT title FROM workspace_items WHERE id='c'", [], |r| r.get(0)).unwrap();
        assert_eq!(c, "in-5", "c 已写入");
        // 空 id 跳过：不应有 id='' 的行（setup 也没创建，double-check）
        let blank_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM workspace_items WHERE id=''",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(blank_count, 0, "空 id 应被跳过");
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


#[cfg(test)]
mod memory_tests {
    //! 记忆模块 Step 1（2026-09-04）：迁移幂等 / 惰性淘汰 / 保护规则 / Reflection 辅助函数。
    //! 内存库与 bot.rs phase4_facts_tests 同模式——先建老 schema（3 列），
    //! 再走生产迁移函数 ensure_bot_facts_memory_columns 补列，覆盖「老库平滑迁移」路径。
    use super::*;

    /// 老 schema（2026-08-20 Phase 4.2 形态：key/value/updated_at 三列）
    fn mem_conn_old() -> rusqlite::Connection {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE bot_facts (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        c
    }

    /// 老 schema + 生产迁移 → 新列齐全
    fn mem_conn() -> rusqlite::Connection {
        let c = mem_conn_old();
        ensure_bot_facts_memory_columns(&c).unwrap();
        c
    }

    fn columns_of(conn: &rusqlite::Connection) -> Vec<String> {
        let mut stmt = conn.prepare("PRAGMA table_info(bot_facts)").unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    #[test]
    fn migration_adds_six_columns_idempotent() {
        let c = mem_conn_old();
        // 老数据先落一行（老 schema 形态写入）
        c.execute(
            "INSERT INTO bot_facts (key, value, updated_at) VALUES ('称呼', '老板', 100)",
            [],
        )
        .unwrap();
        ensure_bot_facts_memory_columns(&c).unwrap();
        ensure_bot_facts_memory_columns(&c).unwrap(); // 幂等：二次运行不报错
        let cols = columns_of(&c);
        for col in ["kind", "category", "importance", "source", "access_count", "accessed_at"] {
            assert!(cols.iter().any(|c| c == col), "缺列 {col}；实际：{cols:?}");
        }
        // 老行回填默认值
        let (kind, category, importance, source, access_count, accessed_at): (
            String,
            String,
            i64,
            String,
            i64,
            i64,
        ) = c
            .query_row(
                "SELECT kind, category, importance, source, access_count, accessed_at FROM bot_facts WHERE key = '称呼'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(kind, "fact", "老行 kind 默认 fact");
        assert_eq!(category, "general");
        assert_eq!(importance, 3);
        assert_eq!(source, "user_stated", "老行是 remember_fact 写入的，属用户口述");
        assert_eq!(access_count, 0);
        assert_eq!(accessed_at, 0);
        // 老行 value 不受影响
        let v: String = c
            .query_row("SELECT value FROM bot_facts WHERE key = '称呼'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "老板");
    }

    #[test]
    fn memory_insert_writes_kind_and_source() {
        let c = mem_conn();
        memory_insert(&c, "summary:s1:1000", "讨论了记忆模块", "summary", 2, 1000).unwrap();
        let (kind, importance, source): (String, i64, String) = c
            .query_row(
                "SELECT kind, importance, source FROM bot_facts WHERE key = 'summary:s1:1000'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((kind.as_str(), importance, source.as_str()), ("summary", 2, "model_inferred"));
        // 同 key 覆盖不占新名额（不触发淘汰）
        memory_insert(&c, "summary:s1:1000", "更新了", "summary", 2, 1001).unwrap();
        assert_eq!(memory_count_by_kind(&c, "summary").unwrap(), 1);
        let v: String = c
            .query_row("SELECT value FROM bot_facts WHERE key = 'summary:s1:1000'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, "更新了");
    }

    #[test]
    fn eviction_picks_lowest_importance_then_oldest_access() {
        let c = mem_conn();
        // 填满 300 条：1 条 importance=1（最该淘汰）、1 条 importance=2 但 accessed_at 更旧、
        // 其余 importance=2
        memory_insert(&c, "low", "v", "summary", 1, 0).unwrap();
        c.execute("UPDATE bot_facts SET accessed_at = 500 WHERE key = 'low'", []).unwrap();
        memory_insert(&c, "older-access", "v", "summary", 2, 0).unwrap();
        c.execute("UPDATE bot_facts SET accessed_at = 100 WHERE key = 'older-access'", []).unwrap();
        for i in 0..(MAX_BOT_MEMORIES - 2) {
            memory_insert(&c, &format!("s{i}"), "v", "summary", 2, i).unwrap();
            c.execute(
                "UPDATE bot_facts SET accessed_at = 900 WHERE key = ?1",
                [format!("s{i}")],
            )
            .unwrap();
        }
        assert_eq!(memory_count_by_kind(&c, "summary").unwrap(), MAX_BOT_MEMORIES);
        // 第 301 条 → 淘汰 importance 最低的 low
        memory_insert(&c, "new1", "v", "summary", 2, 9999).unwrap();
        assert_eq!(memory_count_by_kind(&c, "summary").unwrap(), MAX_BOT_MEMORIES);
        let gone: bool = c
            .query_row("SELECT 1 FROM bot_facts WHERE key = 'low'", [], |_| Ok(()))
            .is_err();
        assert!(gone, "importance 最低的应被淘汰");
        // new1 刚插入 accessed_at=0（从未命中，比谁都「旧」），先刷新再排——
        // 否则下轮淘汰的是 new1 而非 older-access
        c.execute("UPDATE bot_facts SET accessed_at = 950 WHERE key = 'new1'", []).unwrap();
        // 再来一条 → 同分按 accessed_at 升序，淘汰 older-access（100 < 900/950）
        memory_insert(&c, "new2", "v", "summary", 2, 10000).unwrap();
        let gone: bool = c
            .query_row("SELECT 1 FROM bot_facts WHERE key = 'older-access'", [], |_| Ok(()))
            .is_err();
        assert!(gone, "同 importance 时 accessed_at 最旧的应被淘汰");
    }

    #[test]
    fn eviction_protects_high_importance_facts() {
        let c = mem_conn();
        // 300 条全是 importance>=4 的 fact（受保护）→ 再插入报错，让模型自己删
        for i in 0..MAX_BOT_MEMORIES {
            memory_insert(&c, &format!("f{i}"), "v", "fact", 4, i).unwrap();
        }
        let err = memory_insert(&c, "one-more", "v", "summary", 2, 9999).unwrap_err();
        assert!(err.contains("上限"), "应报上限错误：{err}");
        assert_eq!(memory_count_by_kind(&c, "fact").unwrap(), MAX_BOT_MEMORIES, "受保护条目一条不动");
        // 混入一条低分 summary 后可淘汰它而非受保护 fact
        memory_delete_keys(&c, &["f0".to_string()]).unwrap();
        memory_insert(&c, "sacrifice", "v", "summary", 1, 0).unwrap();
        memory_insert(&c, "one-more", "v", "summary", 2, 9999).unwrap();
        let sacrifice_gone: bool = c
            .query_row("SELECT 1 FROM bot_facts WHERE key = 'sacrifice'", [], |_| Ok(()))
            .is_err();
        assert!(sacrifice_gone, "应淘汰低分 summary 而非受保护 fact");
        let protected: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM bot_facts WHERE kind = 'fact' AND importance >= 4",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(protected, MAX_BOT_MEMORIES - 1, "受保护 fact 不淘汰");
    }

    #[test]
    fn reflection_helpers_count_oldest_delete() {
        let c = mem_conn();
        // 10 条 summary + 1 条 fact（fact 不参与 summary 统计）
        memory_insert(&c, "称呼", "老板", "fact", 5, 0).unwrap();
        for i in 0..REFLECTION_BATCH {
            memory_insert(&c, &format!("summary:s1:{i}"), &format!("摘要{i}"), "summary", 2, i).unwrap();
        }
        assert_eq!(memory_count_by_kind(&c, "summary").unwrap(), REFLECTION_BATCH);
        // 最旧 10 条按写入时间升序
        let oldest = memory_oldest_by_kind(&c, "summary", REFLECTION_BATCH).unwrap();
        assert_eq!(oldest.len(), REFLECTION_BATCH as usize);
        assert_eq!(oldest[0].0, "summary:s1:0", "最旧的在前");
        assert_eq!(oldest[9].0, "summary:s1:9");
        // 合成 reflection：插入 + 删原 10 条（生产 bot_memory_apply_reflection 的同事务两步）
        memory_insert(&c, "reflection:100", "阶段总结", "reflection", 3, 100).unwrap();
        let keys: Vec<String> = oldest.into_iter().map(|(k, _)| k).collect();
        let deleted = memory_delete_keys(&c, &keys).unwrap();
        assert_eq!(deleted, REFLECTION_BATCH as usize);
        assert_eq!(memory_count_by_kind(&c, "summary").unwrap(), 0, "原摘要应被清掉");
        assert_eq!(memory_count_by_kind(&c, "reflection").unwrap(), 1);
        let (imp,): (i64,) = c
            .query_row("SELECT importance FROM bot_facts WHERE key = 'reflection:100'", [], |r| {
                Ok((r.get(0)?,))
            })
            .unwrap();
        assert_eq!(imp, 3, "reflection importance=3");
        // 删除不存在的 key → 0 条，不报错
        assert_eq!(memory_delete_keys(&c, &keys).unwrap(), 0);
    }

    // ── 记忆模块 Step 2（2026-09-05）：关键词提取 / 打分排序 / 检索 / 注入取数 ──

    #[test]
    fn extract_keywords_cjk_bigram_and_ascii_words() {
        // 连续 CJK 段取字符 bigram（设计第 4 节示例）
        assert_eq!(extract_keywords("记忆模块"), vec!["记忆", "忆模", "模块"]);
        // 英文词转小写 + 数字词；分隔符切段；CJK 与英文相邻时互不粘段
        let kws = extract_keywords("WMessage 挂件 V2.0");
        assert!(kws.contains(&"wmessage".to_string()), "英文词转小写：{kws:?}");
        assert!(kws.contains(&"挂件".to_string()), "CJK bigram：{kws:?}");
        assert!(kws.contains(&"v2".to_string()) && kws.contains(&"0".to_string()), "数字词：{kws:?}");
        // 单字 CJK 段保留单字（否则单字查询永远提取不出词）
        assert_eq!(extract_keywords("猫"), vec!["猫"]);
        // 全标点 → 空（调用方据此判「提取词数为 0 → 全部 0 分」）
        assert!(extract_keywords("！？，。").is_empty());
        // 去重保序
        assert_eq!(extract_keywords("上海 上海"), vec!["上海"]);
    }

    /// 打分因子构造器：同一份查询词下逐项变量对比
    fn score_item(importance: i64, access_count: i64, accessed_at: i64, updated_at: i64) -> MemoryItem {
        MemoryItem {
            key: "城市".into(),
            value: "上海".into(),
            kind: "fact".into(),
            category: "profile".into(),
            importance,
            source: "user_stated".into(),
            access_count,
            accessed_at,
            updated_at,
        }
    }

    #[test]
    fn memory_score_factors_importance_recency_access() {
        let day = 86_400_000i64;
        let now = 1000 * day;
        let kws = extract_keywords("上海");
        let base = score_item(3, 0, 0, now);
        assert!(memory_score(&kws, &base, now) > 0.0, "有命中应有正分");
        // 零命中 → 0 分；空查询词 → 全 0 分（设计第 4 节）
        assert_eq!(memory_score(&extract_keywords("北京"), &base, now), 0.0);
        assert_eq!(memory_score(&[], &base, now), 0.0);
        // 重要度因子：importance 5 > 1
        let hi = score_item(5, 0, 0, now);
        let lo = score_item(1, 0, 0, now);
        assert!(memory_score(&kws, &hi, now) > memory_score(&kws, &lo, now));
        // 时效因子：新 > 旧（age 从 updated_at 起算）
        let old = score_item(3, 0, 0, now - 180 * day);
        assert!(memory_score(&kws, &base, now) > memory_score(&kws, &old, now));
        // 访问强化：access_count 高 > 低；且 access_count=0 不为 0 分（1+ln(1+n) 保底）
        let hot = score_item(3, 10, now, 0);
        let cold = score_item(3, 0, now, 0);
        assert!(memory_score(&kws, &hot, now) > memory_score(&kws, &cold, now));
        // age 从 accessed_at（>0 时）起算：刚被想起的旧记忆衰减重置
        let refreshed = score_item(3, 0, now, now - 365 * day);
        assert!(
            (memory_score(&kws, &refreshed, now) - memory_score(&kws, &base, now)).abs() < 1e-9,
            "accessed_at=now 应与刚写入同分"
        );
    }

    #[test]
    fn memory_search_ranks_top_n_and_filters_kind() {
        let c = mem_conn();
        let now = 1_000_000_000_000i64;
        // 两条都命中「上海」，importance 5 的排前；无关条目不进结果
        c.execute(
            "INSERT INTO bot_facts (key, value, updated_at, kind, category, importance, source)
             VALUES ('城市', '上海', ?1, 'fact', 'profile', 3, 'user_stated')",
            [now],
        ).unwrap();
        c.execute(
            "INSERT INTO bot_facts (key, value, updated_at, kind, category, importance, source)
             VALUES ('定居地', '长期定居上海', ?1, 'fact', 'profile', 5, 'user_stated')",
            [now],
        ).unwrap();
        c.execute(
            "INSERT INTO bot_facts (key, value, updated_at, kind, category, importance, source)
             VALUES ('爱好', '摄影', ?1, 'fact', 'preference', 5, 'user_stated')",
            [now],
        ).unwrap();
        memory_insert(&c, "summary:s1:1", "聊到上海的天气", "summary", 2, now).unwrap();

        let hits = memory_search(&c, "上海", 5, now, None).unwrap();
        assert_eq!(hits.len(), 3, "三条命中（含 summary）");
        assert_eq!(hits[0].key, "定居地", "importance 高者排前");
        // top_n 截断
        assert_eq!(memory_search(&c, "上海", 1, now, None).unwrap().len(), 1);
        // kind 过滤：只查 fact（冲突提示路径）
        let facts = memory_search(&c, "上海", 5, now, Some("fact")).unwrap();
        assert!(facts.iter().all(|m| m.kind == "fact"));
        assert_eq!(facts.len(), 2, "summary 被过滤");
        // 纯读：不刷新访问计数
        let ac: i64 = c.query_row("SELECT access_count FROM bot_facts WHERE key='城市'", [], |r| r.get(0)).unwrap();
        assert_eq!(ac, 0, "memory_search 不得刷新访问计数");
        // 无命中 → 空；空查询 → 空
        assert!(memory_search(&c, "火星基地", 5, now, None).unwrap().is_empty());
        assert!(memory_search(&c, "！！！", 5, now, None).unwrap().is_empty());
    }

    #[test]
    fn memory_injection_snapshot_pinned_hits_recent_and_access_mark() {
        let c = mem_conn();
        let now = 1_000_000_000_000i64;
        // pinned：importance>=4 的 fact（即使也命中查询词，不重复进 hits）
        c.execute(
            "INSERT INTO bot_facts (key, value, updated_at, kind, category, importance, source)
             VALUES ('称呼', '老板', ?1, 'fact', 'profile', 5, 'user_stated')",
            [now],
        ).unwrap();
        // hit：普通 fact 命中查询词
        c.execute(
            "INSERT INTO bot_facts (key, value, updated_at, kind, category, importance, source)
             VALUES ('城市', '上海', ?1, 'fact', 'general', 3, 'user_stated')",
            [now],
        ).unwrap();
        // 近期摘要：4 条，取最新 3 条
        for i in 0..4 {
            memory_insert(&c, &format!("summary:s1:{i}"), &format!("第{i}段摘要"), "summary", 2, i).unwrap();
        }
        let inj = memory_injection_snapshot(&c, "老板在上海的项目", now).unwrap();
        assert_eq!(inj.pinned.len(), 1);
        assert_eq!(inj.pinned[0].key, "称呼");
        assert!(
            inj.hits.iter().any(|m| m.key == "城市") && !inj.hits.iter().any(|m| m.key == "称呼"),
            "pinned 不重复进 hits：{:?}",
            inj.hits.iter().map(|m| &m.key).collect::<Vec<_>>()
        );
        assert_eq!(inj.recent.len(), MEMORY_RECENT_N, "近期摘要取最新 3 条");
        assert_eq!(inj.recent[0].key, "summary:s1:3", "最新的在前");
        // 命中即访问强化：access_count+1 / accessed_at=now
        let (ac, at): (i64, i64) = c
            .query_row("SELECT access_count, accessed_at FROM bot_facts WHERE key='城市'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((ac, at), (1, now), "命中条目应原子刷新访问计数");
        let ac2: i64 = c.query_row("SELECT access_count FROM bot_facts WHERE key='称呼'", [], |r| r.get(0)).unwrap();
        assert_eq!(ac2, 0, "pinned 无条件注入不算命中，不刷新");

        // 回退兜底（设计第 4 节）：检索全 0 分时 hits 空，recent 仍有最近 3 条
        let inj2 = memory_injection_snapshot(&c, "完全不相关的闲聊", now).unwrap();
        assert!(inj2.hits.is_empty(), "全 0 分 → 无检索命中");
        assert_eq!(inj2.recent.len(), MEMORY_RECENT_N, "回退兜底 = 最近 3 条 summary/reflection");
        assert_eq!(inj2.pinned.len(), 1, "高重要度 fact 无条件保留");
    }
}
