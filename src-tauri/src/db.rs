//! WMessage 任务数据存储：SQLite（app_data_dir/wmessage.db）
//! 方案B（2026-08-14）：行级增量读写，取代方案2 的 data.json 全量覆盖。
//! 单写者架构：只有主窗口通过 db_upsert/db_delete 写库，挂件只读（db_load）。

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::Manager; // F-6：Runtime 给 data_dir 泛型化

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

/// 便携模式：数据库优先放 exe 同目录（U盘/绿色目录随走随带）；
/// 目录不可写（如 Program Files）时兜底到系统应用数据目录。
fn db_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let probe = dir.join(".wm-write-probe");
            if std::fs::File::create(&probe).is_ok() {
                let _ = std::fs::remove_file(&probe);
                return dir.to_path_buf();
            }
        }
    }
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// 数据目录（供本地 HTTP API 存 token 等附属文件，便携模式跟随 exe）
pub fn data_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
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
                if let Ok(conn) = rusqlite::Connection::open(&legacy_db) {
                    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
                }
                let _ = std::fs::copy(&legacy_db, &db_path);
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
    static RESET_ONCE: std::sync::Once = std::sync::Once::new();
    RESET_ONCE.call_once(|| {
        let _ = conn.execute(
            "UPDATE tasks SET bot_assigned = 0 WHERE bot_assigned = 1",
            [],
        );
    });
    Ok(conn)
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

fn upsert_workspace(conn: &rusqlite::Connection, items: &[WorkspaceItem]) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
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
    Ok(())
}

fn delete_workspace(conn: &rusqlite::Connection, ids: &[String]) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("DELETE FROM workspace_items WHERE id = ?1")
        .map_err(|e| e.to_string())?;
    for id in ids {
        stmt.execute([id]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn workspace_load(app: tauri::AppHandle) -> Result<Vec<WorkspaceItem>, String> {
    let conn = open_db(&app)?;
    load_workspace(&conn)
}

#[tauri::command]
pub fn workspace_upsert(app: tauri::AppHandle, items: Vec<WorkspaceItem>) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    upsert_workspace(&conn, &items)
}

#[tauri::command]
pub fn workspace_delete(app: tauri::AppHandle, ids: Vec<String>) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    delete_workspace(&conn, &ids)
}

// ───────────────────────── 机器人聊天记录 ─────────────────────────

/// 全部会话列表（按最近更新倒序）
#[tauri::command]
pub fn bot_sessions_load(app: tauri::AppHandle) -> Result<Vec<BotSession>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, title, created_at, updated_at FROM bot_sessions ORDER BY updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BotSession {
                id: r.get::<_, String>(0)?,
                title: r.get::<_, String>(1)?,
                created_at: r.get::<_, i64>(2)?,
                updated_at: r.get::<_, i64>(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 新建会话，返回新会话（title 缺省「新对话」）
#[tauri::command]
pub fn bot_session_create(
    app: tauri::AppHandle,
    title: Option<String>,
) -> Result<BotSession, String> {
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

/// 删除会话及其全部消息
#[tauri::command]
pub fn bot_session_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    conn.execute("DELETE FROM bot_messages WHERE session_id = ?1", [&id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM bot_sessions WHERE id = ?1", [&id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 会话改名
#[tauri::command]
pub fn bot_session_rename(app: tauri::AppHandle, id: String, title: String) -> Result<(), String> {
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
pub fn bot_history_load(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<Vec<BotMsgRow>, String> {
    let conn = open_db(&app)?;
    let mut stmt = conn
        .prepare("SELECT role, content, refs, thinking, tools FROM bot_messages WHERE session_id = ?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
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
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 保存指定会话的聊天记录：全量覆盖 + 更新会话活跃时间
#[tauri::command]
pub fn bot_history_save(
    app: tauri::AppHandle,
    session_id: String,
    messages: Vec<BotMsgRow>,
) -> Result<(), String> {
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    conn.execute(
        "DELETE FROM bot_messages WHERE session_id = ?1",
        [&session_id],
    )
    .map_err(|e| e.to_string())?;
    if !messages.is_empty() {
        let now = chrono::Utc::now().timestamp_millis();
        let mut stmt = conn
            .prepare(
                "INSERT INTO bot_messages (role, content, refs, session_id, thinking, tools, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|e| e.to_string())?;
        for m in &messages {
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
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "UPDATE bot_sessions SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 清空指定会话的聊天记录（会话保留）
#[tauri::command]
pub fn bot_history_clear(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    conn.execute(
        "DELETE FROM bot_messages WHERE session_id = ?1",
        [&session_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn upsert_tasks(conn: &rusqlite::Connection, tasks: &[Task]) -> Result<(), String> {
    if tasks.is_empty() {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(
            "INSERT INTO tasks
               (id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                completed_at, archived, deleted_at, collapsed, ord, updated_at, schedule, sched_last, bot_assigned)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
             ON CONFLICT(id) DO UPDATE SET
               title=excluded.title, due=excluded.due, note=excluded.note,
               tags=excluded.tags, file_path=excluded.file_path,
               file_is_dir=excluded.file_is_dir, col=excluded.col,
               subtasks=excluded.subtasks, completed_at=excluded.completed_at,
               archived=excluded.archived, deleted_at=excluded.deleted_at,
               collapsed=excluded.collapsed, ord=excluded.ord,
               updated_at=excluded.updated_at,
               schedule=excluded.schedule, sched_last=excluded.sched_last,
               bot_assigned=excluded.bot_assigned",
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
            t.order,
            t.updated_at,
            t.schedule,
            t.sched_last,
            t.bot_assigned.map(|b| b as i64),
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
                    completed_at, archived, deleted_at, collapsed, ord, updated_at, schedule, sched_last, bot_assigned
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
        ) = r.map_err(|e| e.to_string())?;
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
            order,
            updated_at,
            schedule,
            sched_last,
            bot_assigned: bot_assigned.map(|v| v != 0),
        });
    }
    Ok(tasks)
}

/// 库为空时迁移方案2 的 data.json：导入全部任务后删除旧文件
fn migrate_data_json(app: &tauri::AppHandle, conn: &mut rusqlite::Connection) {
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let file = dir.join("data.json");
    if !file.exists() {
        return;
    }
    let Ok(json) = std::fs::read_to_string(&file) else {
        return;
    };
    let Ok(tasks) = serde_json::from_str::<Vec<Task>>(&json) else {
        return;
    };
    let Ok(tx) = conn.transaction() else { return };
    if upsert_tasks(&tx, &tasks).is_ok() {
        if tx.commit().is_ok() {
            let _ = std::fs::remove_file(&file);
        }
    }
}

/// 写操作全局锁：主窗口（db_upsert/db_delete/db_merge）与本地 API 线程共享同一把锁，
/// 避免 WAL 下并发写冲突（busy_timeout 只是兜底）。
static DB_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let _g = DB_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let conn = open_db(&app)?;
    delete_tasks(&conn, &ids)
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
    let sql = format!(
        "SELECT id, title, due, note, tags, file_path, file_is_dir, col, subtasks,
                completed_at, archived, deleted_at, collapsed, {}, {}, {}, {}, {}
         FROM tasks",
        if has_ord { "ord" } else { "NULL" },
        if has_ua { "updated_at" } else { "NULL" },
        if has_sched { "schedule" } else { "NULL" },
        if has_sched_last { "sched_last" } else { "NULL" },
        if has_ba { "bot_assigned" } else { "NULL" }
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
        ) = r.map_err(|e| e.to_string())?;
        let tags = tags.and_then(|s| serde_json::from_str(&s).ok());
        let subtasks = subtasks.and_then(|s| serde_json::from_str(&s).ok());
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
pub fn db_merge(app: tauri::AppHandle, path: String) -> Result<usize, String> {
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
        let cur: Option<i64> = tx
            .query_row(
                "SELECT updated_at FROM tasks WHERE id = ?1",
                rusqlite::params![t.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let take = match cur {
            // 当前库没有这条 → 直接并入
            None => true,
            // 同 id：保留最后修改时间更晚的
            Some(cur_ua) => t.updated_at.unwrap_or(0) > cur_ua,
        };
        if take {
            upsert_tasks(&tx, std::slice::from_ref(t))?;
            merged += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(merged)
}

/// 导出任务卡数据：全量任务（含归档、回收站）序列化为 JSON 文件，返回条数
#[tauri::command]
pub fn tasks_export(app: tauri::AppHandle, path: String) -> Result<usize, String> {
    let conn = open_db(&app)?;
    let tasks = load_all(&conn)?;
    let json = serde_json::to_string_pretty(&tasks).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入文件失败：{e}"))?;
    Ok(tasks.len())
}

/// 从 JSON 文件导入任务卡数据：按 id 并集合并，同 id 保留 updated_at 更晚者。返回写入条数。
#[tauri::command]
pub fn tasks_import(app: tauri::AppHandle, path: String) -> Result<usize, String> {
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
        let cur: Option<i64> = tx
            .query_row(
                "SELECT updated_at FROM tasks WHERE id = ?1",
                rusqlite::params![t.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let take = match cur {
            // 当前库没有这条 → 直接并入
            None => true,
            // 同 id：保留最后修改时间更晚的
            Some(cur_ua) => t.updated_at.unwrap_or(0) > cur_ua,
        };
        if take {
            upsert_tasks(&tx, std::slice::from_ref(t))?;
            merged += 1;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
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
        upsert_workspace(&conn, &[item]).unwrap();
        let got = load_workspace(&conn).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].collapsed, Some(true));
        assert_eq!(got[0].links[0].display_name, "别名");
        fs::remove_dir_all(&dir).ok();
    }
}
