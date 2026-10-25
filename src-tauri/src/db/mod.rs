//!
//! 原 2746 行 db.rs 按 SRP 拆为 8 个子模块（paths / migrations / tasks /
//! workspace / bot_sessions / bot_history / skill_out）。本文件保留为 facade：
//! `pub use crate::db::{每子模块}::*;` 重导出所有 pub 项，`crate::db::X` 路径兼容。
//!
//! **安全纪律**：SQL / schema / 事务边界 / `busy_timeout = 2s` 一律不动。
//!
//! 测试模块（4 段共 2052 行）拆到同层子文件（`tests.rs` / `ws_tests.rs` /
//! `reset_tests.rs` / `atomic_write_tests.rs`）。它们仍是本模块的**直接子模块**
//! （`crate::db::tests` 等路径不变），所以 `use super::*` 仍解析为
//! `crate::db::*`，`super::X` 经 pub use 解析为子模块项——搬移不改变语义。

pub use crate::db::apps::*;
pub use crate::db::bot_history::*;
pub use crate::db::bot_sessions::*;
pub use crate::db::migrations::*;
pub use crate::db::paths::*;
pub use crate::db::people::*;
pub use crate::db::reminders::*;
pub use crate::db::schedule_jobs::*;
pub use crate::db::skill_out::*;
pub use crate::db::subagents::*;
pub use crate::db::tasks::*;
pub use crate::db::trace::*;
pub use crate::db::workspace::*;
use std::time::Duration;
use tauri::Manager;

pub mod bot_history;
pub mod bot_sessions;
pub mod lock;
pub mod migrations;
pub mod paths;
pub mod people;
pub mod reminders;
pub mod schedule_jobs;
pub mod skill_out;
pub mod subagents;
pub mod tasks;
pub mod trace;
pub mod workspace;

pub mod apps;

// RMW helpers

/// 当前 Unix 毫秒时间戳（全仓单一实现）：时钟拨到 1970 前返回负值，
/// 不伪造 0 哨兵——0 会跟合法 epoch-zero 记录撞时间戳、搅乱排序。
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 写库前的标准接线：把当前 `updated_at` 作为下次 RMW 读快照基线，
/// 然后把 `updated_at` 戳成"现在"。任何写库前的 mutate 路径都必须调。
///
/// 行为细节：
/// - `task.updated_at` 为 None 时（老行 / 新建），基线降级为
///   `BASELINE_NULL_ROW`（行存在性哨兵，防 NULL 老行被静默覆盖）
/// - 一律刷 `updated_at` 为当前时间戳
///
/// 用法：`db::prepare_for_upsert(&mut task)` 后接 `db::db_upsert(...)`。
pub fn prepare_for_upsert(t: &mut Task) {
    t.expected_updated_at = Some(t.updated_at.unwrap_or(BASELINE_NULL_ROW));
    t.updated_at = Some(chrono::Utc::now().timestamp_millis());
}

/// 打开数据库（泛型 Runtime：mock runtime 测试可直调）
///
/// 连接级 PRAGMA（每次建连都要设——PRAGMA 是 **per-connection** 的，漏一处就静默跑默认档）：
/// - `journal_mode=WAL`：读写不互相阻塞（本应用单写者 + 多读）
/// - `synchronous=NORMAL`：WAL 下的推荐档——掉电最多丢最近若干已提交事务
/// - `foreign_keys=ON`：SQLite 默认 **OFF**，显式打开
///
/// 幂等补列（schema 演进的共享内核）：查 `PRAGMA table_info(table)`，
/// 缺列则 `ALTER TABLE ADD COLUMN`。各 `ensure_*` 列补丁统一走这里。
/// 探测与 ALTER 之间没有库级锁，并发连接可能已把列加上：
/// ALTER 报「duplicate column」即目的已达成，返 Ok 不算错误。
pub(crate) fn ensure_columns(
    conn: &rusqlite::Connection,
    table: &str,
    cols: &[(&str, &str)],
) -> Result<(), String> {
    for (col, ty) in cols {
        let has: bool = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == *col))
            })
            .map_err(|e| e.to_string())?;
        if !has {
            let altered = conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {col} {ty}"), []);
            match altered {
                Ok(_) => {}
                Err(e) if e.to_string().contains("duplicate column") => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    Ok(())
}

/// 打开数据库（泛型 Runtime：mock runtime 测试可直调）
///
/// 连接级 PRAGMA（每次建连都要设——PRAGMA 是 **per-connection** 的，漏一处就静默跑默认档）：
/// - `journal_mode=WAL`：读写不互相阻塞（本应用单写者 + 多读）
/// - `synchronous=NORMAL`：WAL 下的推荐档——掉电最多丢最近若干已提交事务
/// - `foreign_keys=ON`：SQLite 默认 **OFF**，显式打开
///
/// 回归锁：`db::tests::conn_pragmas_are_applied`
#[allow(clippy::too_many_lines)]
pub fn open_db<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<rusqlite::Connection, String> {
    let dir = paths::db_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let db_path = dir.join("wmessage.db");
    if !db_path.exists() {
        // 进程内锁 + 双检：并发 open_db（首装多线程/多命令并发）
        // 可同时过外层 exists 检查并竞态拷贝 → 半成品 db/-wal/-shm 三元组。
        // 锁内复检查让后到者直接跳过；锁仅护首装一次性窗口，常态零代价
        // （外层 exists 不进锁）。照 BOT_ASSIGNED_RESET_DONE / DB_WRITE_LOCK 先例。
        static LEGACY_COPY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g =
            crate::db::lock::lock_or_recover(&LEGACY_COPY_LOCK, "db::open_db LEGACY_COPY_LOCK");
        if !db_path.exists() {
            match app.path().app_data_dir() {
                Ok(legacy_dir) => {
                    let legacy_db = legacy_dir.join("wmessage.db");
                    if legacy_db.exists() && legacy_db != db_path {
                        match paths::copy_legacy_db(&legacy_db, &db_path) {
                            Ok(warns) => {
                                for w in warns {
                                    crate::audit::write_event(
                                        app,
                                        crate::audit::AuditLevel::Warn,
                                        "legacy_db_copy",
                                        &[("warn", w)],
                                    );
                                }
                            }
                            Err(e) => {
                                crate::audit::write_event(
                                    app,
                                    crate::audit::AuditLevel::Error,
                                    "legacy_db_copy",
                                    &[("error", e.to_string())],
                                );
                                return Err(e.to_string());
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[db] app_data_dir 解析失败，跳过 legacy db 拷贝：{e}");
                    crate::audit::write_event(
                        app,
                        crate::audit::AuditLevel::Warn,
                        "legacy_db_copy",
                        &[(
                            "warn",
                            format!("app_data_dir 解析失败，跳过 legacy db 拷贝：{e}"),
                        )],
                    );
                }
            }
        }
    }
    let conn = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    // busy_timeout **有意保留 2s，不调 5000ms**：进程内写者已由 DB_WRITE_LOCK 串行化
    conn.busy_timeout(Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    migrations::apply_conn_pragmas(&conn)?;
    // tasks 表单源 DDL（从 tasks.rs 抽取）：完整 25 列 schema，老库缺列由下方幂等 ALTER 补齐
    conn.execute_batch(crate::db::tasks::TASKS_DDL)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspace_items (
           id         TEXT PRIMARY KEY,
           title      TEXT NOT NULL,
           collapsed  INTEGER,
           links      TEXT NOT NULL,
           ord        REAL,
           updated_at INTEGER
         );
         CREATE TABLE IF NOT EXISTS migration_journal (
           id          INTEGER PRIMARY KEY AUTOINCREMENT,
           op          TEXT    NOT NULL,
           src         TEXT    NOT NULL,
           dst         TEXT,
           task_id     TEXT    NOT NULL,
           state       TEXT    NOT NULL,
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
           turn_status TEXT,
           created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bot_sessions (
           id         TEXT PRIMARY KEY,
           title      TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           is_subagent INTEGER NOT NULL DEFAULT 0,
           onion_config TEXT,
           onion_max_turn INTEGER
         );
         CREATE TABLE IF NOT EXISTS tag_merge_history (
           run_id        TEXT PRIMARY KEY,
           merged_to     TEXT NOT NULL,
           merged_from   TEXT NOT NULL,
           previous_tags_json TEXT NOT NULL,
           affected_task_count INTEGER NOT NULL,
           rolled_back_at INTEGER,
           created_at    INTEGER NOT NULL
         );",
    )
    .map_err(|e| e.to_string())?;
    // skill_outcomes 表走单源 DDL（skill_out 模块），与集成测试建表共用
    conn.execute_batch(SKILL_OUTCOMES_DDL)
        .map_err(|e| e.to_string())?;
    // subagents 表走单源 DDL（含 task_id 索引 + status/parent 索引），与测试建表共用
    conn.execute_batch(subagents::SUBAGENTS_DDL)
        .map_err(|e| e.to_string())?;
    // 上下文摘要独立成列（不再从子卡 note 文本解析取回）。必须排在
    // ensure_task_id_nullable 之前——后者的重建会搬运该列
    crate::db::subagents::ensure_context_summary_column(&conn).map_err(|e| e.to_string())?;
    // 子 agent 不再建任务卡：task_id 的 NOT NULL 解除、UNIQUE 索引移除
    // （新行恒 NULL），老行保留原值（历史可按卡键反查）
    crate::db::subagents::ensure_task_id_nullable(&conn).map_err(|e| e.to_string())?;
    // token 用量三列（老库 NULL = 未记）。排在 task_id 重建迁移之后——
    // 重建搬运的是显式列清单，本列由这里补
    crate::db::subagents::ensure_usage_columns(&conn).map_err(|e| e.to_string())?;
    // 定时任务表（定时任务模块）：内容型定时作业，到点据此新建任务卡并执行
    conn.execute_batch(crate::db::schedule_jobs::SCHEDULED_JOBS_DDL)
        .map_err(|e| e.to_string())?;
    // v1 老库补状态/重试列 + 执行历史表（XXL-JOB 调度日志借鉴）
    crate::db::schedule_jobs::ensure_scheduled_jobs_columns(&conn).map_err(|e| e.to_string())?;
    conn.execute_batch(crate::db::schedule_jobs::SCHEDULED_JOB_RUNS_DDL)
        .map_err(|e| e.to_string())?;
    // 提醒表：到点发系统通知，不建卡不执行（与内容型作业分表存）
    conn.execute_batch(crate::db::reminders::REMINDERS_DDL)
        .map_err(|e| e.to_string())?;
    // 成员注册表（任务图谱设计 §1.2）：多人汇总的归属人字典，幂等
    conn.execute_batch(people::PEOPLE_DDL)
        .map_err(|e| e.to_string())?;
    // 程序模块（apps）：程序元数据 + 运行历史 + 版本快照，幂等建表
    conn.execute_batch(crate::db::apps::APPS_DDL)
        .map_err(|e| e.to_string())?;
    // 老库补列：程序声明的单次运行时长（apps.timeout_secs，NULL = 未声明）
    ensure_columns(&conn, "apps", &[("timeout_secs", "INTEGER")])?;
    conn.execute_batch(crate::db::apps::APP_RUNS_DDL)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(crate::db::apps::APP_VERSIONS_DDL)
        .map_err(|e| e.to_string())?;
    // 模型元数据双表（meta_provider/meta_model，meta 模块的存储面），幂等
    migrations::ensure_meta_tables(&conn)?;
    // 画布工作流退役清扫（幂等）：老库删遗留表与数据行，已清/新库零成本跳过
    migrations::cleanup_retired_workflow_tables(&conn)?;
    // 画布退役的 tasks 删列（幂等）：workflow_id / canvas_x / canvas_y
    migrations::drop_retired_workflow_task_columns(&conn)?;
    // 历次改版退役列清扫（幂等）：file_path/file_is_dir/schedule/sched_last/enabled/origin/acceptance
    migrations::drop_legacy_task_columns(&conn)?;
    // 执行痕迹三表（exec_traces/exec_spans/file_changes，Agent 透明化设计 §4.1），幂等
    // （含旧库 model 列迁移，词元统计按模型聚合依赖）
    crate::db::trace::ensure_trace_tables(&conn).map_err(|e| e.to_string())?;
    // 迁移：计划起止 plan_start/plan_end（周时间网格排期；NULL = 未排期）
    ensure_columns(&conn, "tasks", &crate::db::tasks::PLAN_TASK_COLUMNS)?;
    // 迁移：子任务卡出身 parent_id（子任务行一键生成的关联键；NULL = 普通卡）
    ensure_columns(&conn, "tasks", &crate::db::tasks::PARENT_ID_TASK_COLUMNS)?;
    // 迁移：子 agent 编排三字段设计 §4.1——assignee/budget/result 走
    // task_patch 既有通道；budget/result 存 JSON TEXT）
    ensure_columns(
        &conn,
        "tasks",
        &[("assignee", "TEXT"), ("budget", "TEXT"), ("result", "TEXT")],
    )?;
    // 迁移设计 §3.1）：工作流四字段 + 画布坐标。
    // origin 缺省 NULL 视作 "user"（读路径归一），不加 DEFAULT 避免 ALTER 语义分叉
    ensure_columns(&conn, "tasks", &crate::db::tasks::W1_TASK_COLUMNS)?;
    // 迁移（任务图谱设计 §1.1）：任务归属人 owner_id，NULL = 本人
    ensure_columns(&conn, "tasks", &crate::db::tasks::OWNER_TASK_COLUMNS)?;
    // 迁移：任务创建时间 created_at（epoch ms）。老数据 ALTER 后为 NULL（= 未知），不回填
    ensure_columns(&conn, "tasks", &crate::db::tasks::CREATED_AT_TASK_COLUMNS)?;
    // 迁移（U20D 批 5）：bot_sessions.is_subagent 结构化子 agent 标记——
    // 会话路由从「标题 🧩 前缀」升级为字段（runner 建会话时置 1；
    // 存量旧会话默认 0，前端按旧标题前缀兜底）
    ensure_columns(
        &conn,
        "bot_sessions",
        &[("is_subagent", "INTEGER NOT NULL DEFAULT 0")],
    )?;
    // 迁移（onions）：每会话 onion 配置两列。bot_sessions 的 DDL 是
    // CREATE TABLE IF NOT EXISTS——老库走不到那句 DDL，只剩补列这一条路。
    // 不补的后果不是「少个字段」，而是每会话读 onion 配置必炸
    // no such column: onion_config（配置永远不生效）。
    migrations::ensure_onion_config_columns(&conn)?;
    ensure_columns(&conn, "tasks", &[("bot_assigned", "INTEGER")])?;
    ensure_columns(&conn, "tasks", &[("ord", "REAL")])?;
    ensure_columns(&conn, "tasks", &[("updated_at", "INTEGER")])?;
    migrations::ensure_files_column(&conn)?;
    ensure_columns(
        &conn,
        "bot_messages",
        &[
            ("session_id", "TEXT"),
            ("thinking", "TEXT"),
            ("tools", "TEXT"),
            ("turn_status", "TEXT"),
        ],
    )?;
    {
        let orphan: i64 = match conn.query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id IS NULL",
            [],
            |r| r.get(0),
        ) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("[db] orphan session_id 统计失败（按 0 跳过回填）：{e}");
                0
            }
        };
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
    static BOT_ASSIGNED_RESET_DONE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    migrations::reset_bot_assigned_with(&BOT_ASSIGNED_RESET_DONE, || {
        conn.execute(
            "UPDATE tasks SET bot_assigned = 0 WHERE bot_assigned = 1",
            [],
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })?;
    Ok(conn)
}

/// 写操作全局锁：主窗口（db_upsert/db_delete）与本地 API 线程共享同一把锁
pub static DB_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// 当前线程是否持有 DB_WRITE_LOCK（供契约断言用）。
    /// `std::sync::Mutex` **线程无关**：`try_lock` 只能证明「锁被某线程持有」，不能证明
    /// 「被当前线程持有」（且 poisoned 也返回 Err）→ 故用线程本地标记精确判定。
    static HOLDING_DB_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `DB_WRITE_LOCK` 的守卫：取锁时置位线程本地标记，Drop 时**无条件**清位
/// （Drop 同时覆盖正常释放与 unwind，故不写手动清位——）。
pub struct DbWriteGuard {
    _g: std::sync::MutexGuard<'static, ()>,
}

impl Drop for DbWriteGuard {
    fn drop(&mut self) {
        HOLDING_DB_WRITE.with(|f| f.set(false));
    }
}

/// 取 `DB_WRITE_LOCK` 并标记「本线程持锁」。poison 走仓库既有 `[mutex_poisoned]` 约定
/// （`into_inner` 后照样置位，不让 poisoned 断掉标记语义。
pub fn lock_db_write() -> DbWriteGuard {
    let g = crate::db::lock::lock_or_recover(&DB_WRITE_LOCK, "db::DB_WRITE_LOCK");
    HOLDING_DB_WRITE.with(|f| f.set(true));
    DbWriteGuard { _g: g }
}

/// 当前线程是否持有 `DB_WRITE_LOCK`（供 `upsert_tasks` 的契约断言用）。
pub fn holding_db_write() -> bool {
    HOLDING_DB_WRITE.with(|f| f.get())
}

/// 测试用内存库连接：open_in_memory 后按序执行给定 DDL 批。
/// 各模块测试 `setup_conn` 的单源——表依赖（tasks 先于 people/workflows）
/// 由调用方排序；建表失败 unwrap 直接炸测试，不扩散错误路径。
#[cfg(test)]
pub(crate) fn test_conn(ddl: &[&str]) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    for d in ddl {
        conn.execute_batch(d).unwrap();
    }
    conn
}

// 抑制 unused warnings
#[cfg(test)]
mod tests;

#[cfg(test)]
mod ws_tests;

#[cfg(test)]
mod reset_tests;

#[cfg(test)]
mod atomic_write_tests;
