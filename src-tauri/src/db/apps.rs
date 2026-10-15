//! 程序模块：apps 表 + app_runs 表
//!
//! 「程序」= 大模型开发时写好的确定性小程序（脚本 + 输入声明），运行态点一下
//! 跑、不连大模型。设计稿：`docs/APPS-DESIGN-2026-10-15.md`。
//!
//! 库内只存元数据；脚本文件落在 `{data_dir}/apps/<id>/app.py`（库存相对路径），
//! 与技能系统同思路——版本控制、备份、迁移都跟文件走。
//!
//! status 三态：
//! - `draft`     草稿，上架前 M2 由 AI 调用 `apps_create` 写入
//! - `enabled`   程序页可见可运行，M1 起就支持（手工/AI 都能上架）
//! - `archived`  主动隐藏，仍在数据目录里

use serde::{Deserialize, Serialize};

/// apps 表单源 DDL：生产建表（db::ensure）与集成测试建表共用，防手抄漂移
pub const APPS_DDL: &str = "CREATE TABLE IF NOT EXISTS apps (
   id              TEXT PRIMARY KEY,
   name            TEXT NOT NULL,
   description    TEXT NOT NULL,
   icon            TEXT NOT NULL,
   input_schema    TEXT NOT NULL,
   script_path    TEXT NOT NULL,
   manual          TEXT NOT NULL,
   status          TEXT NOT NULL,
   enabled_at     INTEGER,
   created_at     INTEGER NOT NULL,
   updated_at     INTEGER NOT NULL
 );";

/// app_runs 表单源 DDL：每次运行的元数据（产物路径在 `data_dir/apps/<id>/output/<run_id>/`）
pub const APP_RUNS_DDL: &str = "CREATE TABLE IF NOT EXISTS app_runs (
   id              INTEGER PRIMARY KEY AUTOINCREMENT,
   app_id         TEXT NOT NULL,
   started_at     INTEGER NOT NULL,
   finished_at    INTEGER,
   duration_ms    INTEGER,
   status         TEXT NOT NULL,
   summary        TEXT
 );";

/// app_versions 表单源 DDL：程序本体版本快照（每次创建/更新/回滚存一份完整内容）。
/// 选型理由（为何不用 git）：单脚本线性改、Windows 绿色版不带 git、
/// data/ 敏感数据不进历史；回滚 = 旧内容整体写回，本身也记为新版本
pub const APP_VERSIONS_DDL: &str = "CREATE TABLE IF NOT EXISTS app_versions (
   id             INTEGER PRIMARY KEY AUTOINCREMENT,
   app_id         TEXT NOT NULL,
   version        INTEGER NOT NULL,
   name           TEXT NOT NULL,
   description    TEXT NOT NULL,
   icon           TEXT NOT NULL,
   input_schema   TEXT NOT NULL,
   script         TEXT NOT NULL,
   manual         TEXT NOT NULL,
   source         TEXT NOT NULL,
   created_at     INTEGER NOT NULL,
   UNIQUE(app_id, version)
 );";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    /// 输入声明 JSON：
    /// - 文件夹模式 `{"kind":"folder"}`（约定 `data/ input/ output/`，无字段）
    /// - 表单模式 `{"kind":"form","fields":[{"key","label","type","required","placeholder"}]}`
    pub input_schema: String,
    /// `{data_dir}/apps/<id>/` 之下的相对路径（M1 恒为 `app.py`）
    pub script_path: String,
    pub manual: String,
    pub status: String,
    pub enabled_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppRun {
    pub id: i64,
    pub app_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub summary: Option<String>,
}

pub fn upsert_app(conn: &rusqlite::Connection, a: &App) -> Result<(), String> {
    conn.execute(
        "INSERT INTO apps (
            id, name, description, icon, input_schema, script_path, manual, status,
            enabled_at, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET
            name=excluded.name,
            description=excluded.description,
            icon=excluded.icon,
            input_schema=excluded.input_schema,
            script_path=excluded.script_path,
            manual=excluded.manual,
            status=excluded.status,
            enabled_at=excluded.enabled_at,
            updated_at=excluded.updated_at",
        rusqlite::params![
            a.id,
            a.name,
            a.description,
            a.icon,
            a.input_schema,
            a.script_path,
            a.manual,
            a.status,
            a.enabled_at,
            a.created_at,
            a.updated_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 非归档程序（草稿 + 已启用）：M2 程序页展示与 bot 工具 apps_list 共用
pub fn list_active_apps(conn: &rusqlite::Connection) -> Result<Vec<App>, String> {
    list_by_status(conn, "status != 'archived'")
}

fn list_by_status(conn: &rusqlite::Connection, where_clause: &str) -> Result<Vec<App>, String> {
    let sql = format!(
        "SELECT id, name, description, icon, input_schema, script_path, manual, status,
                enabled_at, created_at, updated_at
         FROM apps WHERE {where_clause} ORDER BY updated_at DESC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(App {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                icon: r.get(3)?,
                input_schema: r.get(4)?,
                script_path: r.get(5)?,
                manual: r.get(6)?,
                status: r.get(7)?,
                enabled_at: r.get(8)?,
                created_at: r.get(9)?,
                updated_at: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn get_app(conn: &rusqlite::Connection, id: &str) -> Result<Option<App>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, description, icon, input_schema, script_path, manual, status,
                    enabled_at, created_at, updated_at
             FROM apps WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([id], |r| {
            Ok(App {
                id: r.get(0)?,
                name: r.get(1)?,
                description: r.get(2)?,
                icon: r.get(3)?,
                input_schema: r.get(4)?,
                script_path: r.get(5)?,
                manual: r.get(6)?,
                status: r.get(7)?,
                enabled_at: r.get(8)?,
                created_at: r.get(9)?,
                updated_at: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.next().transpose().map_err(|e| e.to_string())
}

pub fn record_app_run(
    conn: &rusqlite::Connection,
    app_id: &str,
    started_at: i64,
    status: &str,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO app_runs (app_id, started_at, status) VALUES (?1, ?2, ?3)",
        rusqlite::params![app_id, started_at, status],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_app_run(
    conn: &rusqlite::Connection,
    run_id: i64,
    finished_at: i64,
    duration_ms: i64,
    status: &str,
    summary: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE app_runs SET finished_at = ?1, duration_ms = ?2, status = ?3, summary = ?4
         WHERE id = ?5",
        rusqlite::params![finished_at, duration_ms, status, summary, run_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn list_app_runs(
    conn: &rusqlite::Connection,
    app_id: &str,
    limit: i64,
) -> Result<Vec<AppRun>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, app_id, started_at, finished_at, duration_ms, status, summary
             FROM app_runs WHERE app_id = ?1
             ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![app_id, limit], |r| {
            Ok(AppRun {
                id: r.get(0)?,
                app_id: r.get(1)?,
                started_at: r.get(2)?,
                finished_at: r.get(3)?,
                duration_ms: r.get(4)?,
                status: r.get(5)?,
                summary: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

// ── 版本快照（app_versions） ──

/// 版本快照全文行（回滚/查看用；列表走 [`AppVersionMeta`] 防脚本大文本过 IPC）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppVersion {
    pub id: i64,
    pub app_id: String,
    /// 每程序单调递增（1 起），UNIQUE(app_id, version)
    pub version: i64,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub input_schema: String,
    /// app.py 全文快照
    pub script: String,
    pub manual: String,
    /// create / update / restore
    pub source: String,
    pub created_at: i64,
}

/// 版本列表行（不含 script/manual 大文本）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppVersionMeta {
    pub id: i64,
    pub app_id: String,
    pub version: i64,
    pub name: String,
    pub icon: String,
    pub source: String,
    pub created_at: i64,
}

/// 下一版本号 = 每程序当前最大版本 + 1（空 = 1）
pub fn next_app_version(conn: &rusqlite::Connection, app_id: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM app_versions WHERE app_id = ?1",
        [app_id],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

pub fn insert_app_version(conn: &rusqlite::Connection, v: &AppVersion) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_versions (
            app_id, version, name, description, icon, input_schema, script, manual,
            source, created_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        rusqlite::params![
            v.app_id,
            v.version,
            v.name,
            v.description,
            v.icon,
            v.input_schema,
            v.script,
            v.manual,
            v.source,
            v.created_at,
        ],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 版本列表（新→旧），不含脚本/说明书大文本
pub fn list_app_version_metas(
    conn: &rusqlite::Connection,
    app_id: &str,
    limit: i64,
) -> Result<Vec<AppVersionMeta>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, app_id, version, name, icon, source, created_at
             FROM app_versions WHERE app_id = ?1
             ORDER BY version DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![app_id, limit], |r| {
            Ok(AppVersionMeta {
                id: r.get(0)?,
                app_id: r.get(1)?,
                version: r.get(2)?,
                name: r.get(3)?,
                icon: r.get(4)?,
                source: r.get(5)?,
                created_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// 取某版本全文（无则 None）
pub fn get_app_version(
    conn: &rusqlite::Connection,
    app_id: &str,
    version: i64,
) -> Result<Option<AppVersion>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, app_id, version, name, description, icon, input_schema, script,
                    manual, source, created_at
             FROM app_versions WHERE app_id = ?1 AND version = ?2",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map(rusqlite::params![app_id, version], |r| {
            Ok(AppVersion {
                id: r.get(0)?,
                app_id: r.get(1)?,
                version: r.get(2)?,
                name: r.get(3)?,
                description: r.get(4)?,
                icon: r.get(5)?,
                input_schema: r.get(6)?,
                script: r.get(7)?,
                manual: r.get(8)?,
                source: r.get(9)?,
                created_at: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.next().transpose().map_err(|e| e.to_string())
}

/// 滚动保留每程序最近 `keep` 个版本（超出按版本号从小到大删）
pub fn trim_app_versions(
    conn: &rusqlite::Connection,
    app_id: &str,
    keep: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM app_versions WHERE app_id = ?1
         AND version <= (SELECT MAX(version) FROM app_versions WHERE app_id = ?1) - ?2",
        rusqlite::params![app_id, keep],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}
