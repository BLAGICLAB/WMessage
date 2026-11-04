//! 定时任务（定时任务模块，N-SCHED）：**内容型定时作业**存储。
//!
//! 与任务卡定时的本质区别：作业在到点前**没有**对应的任务卡——用户只写
//! 「要做什么」的内容，调度器到点才根据内容新建任务卡并交给机器人执行
//! （设计取舍见 docs 侧方案：「新建任务卡」而非「给现有卡设定时」）。

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

/// 单源 DDL：open_db 与测试建表共用
pub const SCHEDULED_JOBS_DDL: &str = "CREATE TABLE IF NOT EXISTS scheduled_jobs (
   id         TEXT PRIMARY KEY,
   content    TEXT NOT NULL,
   schedule   TEXT NOT NULL,
   sched_last INTEGER,
   enabled    INTEGER,
   created_at INTEGER,
   updated_at INTEGER,
   last_status TEXT,
   last_error TEXT,
   pause_on_failure INTEGER,
   retry_max  INTEGER,
   retry_at   INTEGER,
   retry_count INTEGER,
   mode       TEXT
 );";

/// 每作业保留的执行历史条数（借鉴 XXL-JOB 调度日志：查得到前几次跑得怎样即可，
/// 本地单机不做全量审计——审计走 audit 事件）
pub const JOB_RUNS_KEEP: usize = 20;

/// 单次执行历史 DDL：借鉴 XXL-JOB/Temporal 的 recent activity
pub const SCHEDULED_JOB_RUNS_DDL: &str = "CREATE TABLE IF NOT EXISTS scheduled_job_runs (
   id         INTEGER PRIMARY KEY AUTOINCREMENT,
   job_id     TEXT NOT NULL,
   fired_at   INTEGER NOT NULL,
   status     TEXT NOT NULL,
   duration_ms INTEGER,
   summary    TEXT,
   card_id    TEXT
 );
 CREATE INDEX IF NOT EXISTS idx_job_runs_job ON scheduled_job_runs(job_id, id DESC);";

/// 幂等补列：v1 已建表（无状态/重试列）的老库补齐，与 open_db 的 tasks 补列同模式
pub fn ensure_scheduled_jobs_columns(conn: &rusqlite::Connection) -> Result<(), String> {
    super::ensure_columns(
        conn,
        "scheduled_jobs",
        &[
            ("last_status", "TEXT"),
            ("last_error", "TEXT"),
            ("pause_on_failure", "INTEGER"),
            ("retry_max", "INTEGER"),
            ("retry_at", "INTEGER"),
            ("retry_count", "INTEGER"),
            ("mode", "TEXT"),
        ],
    )
}

/// 内容长度硬顶（一段「要做什么」的描述）
pub const MAX_JOB_CONTENT: usize = 500;

/// 失败重试上限（借鉴 XXL-JOB 失败重试次数；写入口硬顶，防配置成死循环重烧 LLM）
pub const MAX_RETRY: i64 = 5;

/// 失败重试延迟（固定 5 分钟退避：不做指数退避——本地单机场景固定间隔足够可预期）
pub const RETRY_DELAY_MS: i64 = 5 * 60 * 1000;

/// 执行档（mode 列）：auto = 到点直接执行（默认/存量）；preview = 预演
pub const JOB_MODE_AUTO: &str = "auto";
pub const JOB_MODE_PREVIEW: &str = "preview";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJob {
    pub id: String,
    /// 要做什么（到点据此新建任务卡）
    pub content: String,
    /// 定时规则：daily:HH:MM / weekly:D:HH:MM / monthly:DD:HH:MM / at:YYYY-MM-DDTHH:MM
    pub schedule: String,
    /// 上次触发时间（epoch ms）
    pub sched_last: Option<i64>,
    /// 定时启用开关：None/Some(true) = 启用，Some(false) = 暂停（保留配置）
    #[serde(default)]
    pub enabled: Option<bool>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    /// 最近一次最终执行结果：None=从未跑，Some("ok")/Some("fail")（借鉴 K8s status）
    #[serde(default)]
    pub last_status: Option<String>,
    /// 最近一次失败的错误摘要（成功时清空）
    #[serde(default)]
    pub last_error: Option<String>,
    /// 失败后自动暂停（Temporal pauseOnFailure 借鉴）：最终失败 → enabled=0 + 通知
    #[serde(default)]
    pub pause_on_failure: Option<bool>,
    /// 失败重试次数上限（XXL-JOB 失败重试借鉴）：0 = 不重试
    #[serde(default)]
    pub retry_max: Option<i64>,
    /// 待重试触发时间（epoch ms）；非空且到点 → jobs_tick 直接触发（不等下一周期）
    #[serde(default)]
    pub retry_at: Option<i64>,
    /// 已重试次数（成功清零）
    #[serde(default)]
    pub retry_count: Option<i64>,
    /// 执行档：None/Some("auto") = 到点直接执行；Some("preview") = 预演（只建提案卡+通知，不执行）
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default, skip_serializing)]
    pub expected_updated_at: Option<i64>,
}

impl ScheduledJob {
    pub fn sched_enabled(&self) -> bool {
        self.enabled != Some(false)
    }
    pub fn retry_max_or_zero(&self) -> i64 {
        self.retry_max.unwrap_or(0).clamp(0, MAX_RETRY)
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledJobRun {
    pub id: i64,
    pub job_id: String,
    pub fired_at: i64,
    /// "ok" | "fail" | "retrying"
    pub status: String,
    pub duration_ms: Option<i64>,
    /// 执行摘要（成功=结果文本截断，失败=错误信息截断）
    pub summary: Option<String>,
    /// 本次触发新建的任务卡 id（前端「查看执行」跳转用）
    pub card_id: Option<String>,
}

const JOB_COLS: &str =
    "SELECT id, content, schedule, sched_last, enabled, created_at, updated_at, \
     last_status, last_error, pause_on_failure, retry_max, retry_at, retry_count, mode \
     FROM scheduled_jobs";

fn job_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledJob> {
    Ok(ScheduledJob {
        id: r.get(0)?,
        content: r.get(1)?,
        schedule: r.get(2)?,
        sched_last: r.get(3)?,
        enabled: r.get::<_, Option<i64>>(4)?.map(|v| v != 0),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        last_status: r.get(7)?,
        last_error: r.get(8)?,
        pause_on_failure: r.get::<_, Option<i64>>(9)?.map(|v| v != 0),
        retry_max: r.get(10)?,
        retry_at: r.get(11)?,
        retry_count: r.get(12)?,
        mode: r.get(13)?,
        expected_updated_at: None,
    })
}

pub fn load_scheduled_jobs(conn: &rusqlite::Connection) -> Result<Vec<ScheduledJob>, String> {
    let mut stmt = conn
        .prepare(&format!("{JOB_COLS} ORDER BY created_at DESC, rowid"))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], job_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn load_scheduled_job(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Option<ScheduledJob>, String> {
    conn.query_row(&format!("{JOB_COLS} WHERE id = ?1"), [id], job_from_row)
        .optional()
        .map_err(|e| e.to_string())
}

/// 新建/覆盖写（调用方持 lock_db_write；RMW 基线检查同 upsert_tasks）
pub fn upsert_scheduled_job(conn: &rusqlite::Connection, j: &ScheduledJob) -> Result<(), String> {
    debug_assert!(
        super::holding_db_write(),
        "upsert_scheduled_job 必须在持有 DB_WRITE_LOCK（lock_db_write()）时调用"
    );
    // RMW：基线不匹配（行已被改/删）拒绝写，防覆盖并发编辑
    if let Some(expected) = j.expected_updated_at {
        let cur: Option<Option<i64>> = conn
            .query_row(
                "SELECT updated_at FROM scheduled_jobs WHERE id = ?1",
                [&j.id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let conflict = super::tasks::rmw_conflict(expected, cur);
        if conflict {
            return Err(format!(
                "{}：定时作业 {} 读快照后已被其他写者修改，写回被拒",
                super::tasks::CONFLICT_ERR_PREFIX,
                j.id
            ));
        }
    }
    conn.execute(
        "INSERT INTO scheduled_jobs (id, content, schedule, sched_last, enabled, created_at, updated_at, pause_on_failure, retry_max, mode)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(id) DO UPDATE SET
           content=excluded.content, schedule=excluded.schedule,
           sched_last=excluded.sched_last, enabled=excluded.enabled,
           pause_on_failure=excluded.pause_on_failure, retry_max=excluded.retry_max,
           mode=excluded.mode, updated_at=excluded.updated_at",
        rusqlite::params![
            j.id,
            j.content,
            j.schedule,
            j.sched_last,
            j.enabled.map(|b| b as i64),
            j.created_at,
            j.updated_at,
            j.pause_on_failure.map(|b| b as i64),
            j.retry_max,
            j.mode,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 执行收尾的状态/重试字段定点更新（调度器专用：只动状态列，不碰 content/schedule——
/// 用户在执行期间改配置不应被回滚）。不带 RMW：状态列最后写者胜。
pub fn update_job_run_state(
    conn: &rusqlite::Connection,
    id: &str,
    last_status: Option<&str>,
    last_error: Option<&str>,
    retry_at: Option<i64>,
    retry_count: Option<i64>,
    disable: bool,
    bump_updated_at: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE scheduled_jobs SET \
           last_status = ?1, last_error = ?2, retry_at = ?3, retry_count = ?4, \
           enabled = CASE WHEN ?5 THEN 0 ELSE enabled END, \
           updated_at = ?6 \
         WHERE id = ?7",
        rusqlite::params![
            last_status,
            last_error,
            retry_at,
            retry_count,
            disable as i64,
            bump_updated_at,
            id
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// 执行历史（XXL-JOB 调度日志借鉴）

/// 落一条执行历史并裁剪到 JOB_RUNS_KEEP 条。本函数无自有事务——
/// 调用方持锁并负责事务边界（与作业状态写同事务收口，见 bot_scheduler 收尾路径）。
pub fn record_job_run_in_tx(
    conn: &rusqlite::Connection,
    job_id: &str,
    fired_at: i64,
    status: &str,
    duration_ms: Option<i64>,
    summary: Option<&str>,
    card_id: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO scheduled_job_runs (job_id, fired_at, status, duration_ms, summary, card_id) \
         VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![job_id, fired_at, status, duration_ms, summary, card_id],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM scheduled_job_runs WHERE job_id = ?1 AND id NOT IN (\
           SELECT id FROM scheduled_job_runs WHERE job_id = ?1 ORDER BY id DESC LIMIT ?2)",
        rusqlite::params![job_id, JOB_RUNS_KEEP as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn run_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledJobRun> {
    Ok(ScheduledJobRun {
        id: r.get(0)?,
        job_id: r.get(1)?,
        fired_at: r.get(2)?,
        status: r.get(3)?,
        duration_ms: r.get(4)?,
        summary: r.get(5)?,
        card_id: r.get(6)?,
    })
}

pub fn load_job_runs(
    conn: &rusqlite::Connection,
    job_id: &str,
    limit: usize,
) -> Result<Vec<ScheduledJobRun>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, job_id, fired_at, status, duration_ms, summary, card_id \
             FROM scheduled_job_runs WHERE job_id = ?1 ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![job_id, limit as i64], run_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 删作业连同执行历史（同一事务，参照 bot_session_delete_inner 的级联写法）：
/// 只删父行会让 scheduled_job_runs 变成永久孤儿——它既没有外键约束，也不会
/// 被任何保留期清扫回收
pub fn delete_scheduled_job(conn: &mut rusqlite::Connection, id: &str) -> Result<bool, String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM scheduled_job_runs WHERE job_id = ?1", [id])
        .map_err(|e| e.to_string())?;
    let n = tx
        .execute("DELETE FROM scheduled_jobs WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(n > 0)
}

// Tauri commands

fn validate_content(content: &str) -> CommandResult<String> {
    let t = content.trim();
    if t.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "content".into(),
            value: content.to_string(),
            reason: "定时任务内容不能为空".into(),
        });
    }
    if t.chars().count() > MAX_JOB_CONTENT {
        return Err(CommandError::InvalidArgument {
            field: "content".into(),
            value: content.to_string(),
            reason: format!("定时任务内容超过 {MAX_JOB_CONTENT} 字上限"),
        });
    }
    Ok(t.to_string())
}

/// 新建内容型定时作业（定时任务模块「新建定时任务」入口）
#[tauri::command]
pub async fn scheduled_job_create(
    app: AppHandle,
    content: String,
    schedule: String,
    retry_max: Option<i64>,
    pause_on_failure: Option<bool>,
) -> CommandResult<ScheduledJob> {
    let content = validate_content(&content)?;
    crate::bot::validate_schedule(schedule.trim()).map_err(|reason| {
        CommandError::InvalidArgument {
            field: "schedule".into(),
            value: schedule.clone(),
            reason,
        }
    })?;
    let retry_max = retry_max.map(|n| n.clamp(0, MAX_RETRY));
    let app_emit = app.clone();
    let row = tauri::async_runtime::spawn_blocking(move || -> CommandResult<ScheduledJob> {
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        let now = crate::db::now_ms();
        let job = ScheduledJob {
            id: uuid::Uuid::new_v4().simple().to_string(),
            content,
            schedule: schedule.trim().to_string(),
            sched_last: None,
            enabled: None,
            created_at: Some(now),
            updated_at: Some(now),
            last_status: None,
            last_error: None,
            pause_on_failure,
            retry_max,
            retry_at: None,
            retry_count: None,
            mode: None,
            expected_updated_at: None,
        };
        upsert_scheduled_job(&conn, &job).map_err(CommandError::from)?;
        Ok(job)
    })
    .await
    .map_err(|e| CommandError::from(format!("定时作业创建线程 join 失败：{e}")))??;
    crate::audit::write_event(
        &app_emit,
        crate::audit::AuditLevel::Info,
        "scheduled_job_create",
        &[
            ("jobId", row.id.clone()),
            ("schedule", row.schedule.clone()),
        ],
    );
    Ok(row)
}

/// 编辑内容型定时作业（内容 / 时间 / 重试 / 自动暂停 / 执行档；None 参数 = 不改该字段）
#[tauri::command]
pub async fn scheduled_job_update(
    app: AppHandle,
    id: String,
    content: Option<String>,
    schedule: Option<String>,
    retry_max: Option<i64>,
    pause_on_failure: Option<bool>,
    mode: Option<String>,
) -> CommandResult<ScheduledJob> {
    let new_content = match &content {
        Some(c) => Some(validate_content(c)?),
        None => None,
    };
    if let Some(m) = &mode {
        if m != JOB_MODE_AUTO && m != JOB_MODE_PREVIEW {
            return Err(CommandError::InvalidArgument {
                field: "mode".into(),
                value: m.clone(),
                reason: format!("mode 只接受 {JOB_MODE_AUTO} / {JOB_MODE_PREVIEW}"),
            });
        }
    }
    if let Some(s) = &schedule {
        crate::bot::validate_schedule(s.trim()).map_err(|reason| {
            CommandError::InvalidArgument {
                field: "schedule".into(),
                value: s.clone(),
                reason,
            }
        })?;
    }
    let retry_max = retry_max.map(|n| n.clamp(0, MAX_RETRY));
    let app_emit = app.clone();
    let row = tauri::async_runtime::spawn_blocking(move || -> CommandResult<ScheduledJob> {
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        let mut job =
            load_scheduled_job(&conn, &id)?.ok_or(CommandError::TaskNotFound(id.clone()))?;
        if let Some(c) = new_content {
            job.content = c;
        }
        if let Some(s) = schedule {
            job.schedule = s.trim().to_string();
        }
        if let Some(n) = retry_max {
            job.retry_max = Some(n);
        }
        if let Some(p) = pause_on_failure {
            job.pause_on_failure = Some(p);
        }
        if let Some(m) = mode {
            job.mode = Some(m);
        }
        // 配置变更 → 重试状态失效（下一周期从零计）
        job.retry_at = None;
        job.retry_count = None;
        job.expected_updated_at = job.updated_at; // RMW 基线 = 快照 updated_at
        job.updated_at = Some(crate::db::now_ms());
        upsert_scheduled_job(&conn, &job).map_err(CommandError::from)?;
        Ok(job)
    })
    .await
    .map_err(|e| CommandError::from(format!("定时作业更新线程 join 失败：{e}")))??;
    crate::audit::write_event(
        &app_emit,
        crate::audit::AuditLevel::Info,
        "scheduled_job_update",
        &[("jobId", row.id.clone())],
    );
    Ok(row)
}

/// 删除内容型定时作业（取消定时；二次确认在 UI 侧）
#[tauri::command]
pub async fn scheduled_job_delete(app: AppHandle, id: String) -> CommandResult<bool> {
    let app_emit = app.clone();
    let id_for_audit = id.clone();
    let ok = tauri::async_runtime::spawn_blocking(move || -> CommandResult<bool> {
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        delete_scheduled_job(&mut conn, &id).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("定时作业删除线程 join 失败：{e}")))??;
    crate::audit::write_event(
        &app_emit,
        crate::audit::AuditLevel::Info,
        "scheduled_job_delete",
        &[("jobId", id_for_audit)],
    );
    Ok(ok)
}

/// 作业执行历史（最近 limit 条，倒序；借鉴 XXL-JOB 调度日志）
#[tauri::command]
pub async fn scheduled_job_history(
    app: AppHandle,
    id: String,
    limit: Option<usize>,
) -> CommandResult<Vec<ScheduledJobRun>> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        load_job_runs(
            &conn,
            &id,
            limit.unwrap_or(JOB_RUNS_KEEP).min(JOB_RUNS_KEEP),
        )
        .map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("作业历史线程 join 失败：{e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEDULED_JOBS_DDL).unwrap();
        conn.execute_batch(SCHEDULED_JOB_RUNS_DDL).unwrap();
        conn
    }

    #[test]
    fn job_runs_trimmed_to_keep_limit() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        for i in 0..(JOB_RUNS_KEEP + 5) as i64 {
            record_job_run_in_tx(&conn, "j1", 1_000 + i, "ok", Some(i), Some("ok"), Some("c"))
                .unwrap();
        }
        let runs = load_job_runs(&conn, "j1", JOB_RUNS_KEEP).unwrap();
        assert_eq!(runs.len(), JOB_RUNS_KEEP, "只保留最近 N 条");
        assert_eq!(
            runs[0].fired_at,
            1_000 + JOB_RUNS_KEEP as i64 + 4,
            "倒序最新在前"
        );
        // 其他作业的历史互不干扰
        record_job_run_in_tx(&conn, "j2", 9_999, "fail", None, Some("boom"), None).unwrap();
        assert_eq!(load_job_runs(&conn, "j2", 10).unwrap().len(), 1);
    }

    /// 收尾两笔写必须同事务：状态写失败时历史行也不能留——否则留下
    /// 「有 fail 行但 retry_at 为空」的中间态，失败不重试、徽标停在旧值。
    #[test]
    fn finalize_writes_rollback_together() {
        let conn = mem_conn();
        // 状态写的目标表不存在 → 第二笔必败，第一笔必须被回滚
        conn.execute_batch("DROP TABLE scheduled_jobs;").unwrap();
        let _g = crate::db::lock_db_write();
        let tx = conn.unchecked_transaction().unwrap();
        let outcome = record_job_run_in_tx(&tx, "j1", 1_000, "fail", None, Some("boom"), None)
            .and_then(|_| {
                update_job_run_state(
                    &tx,
                    "j1",
                    Some("fail"),
                    Some("boom"),
                    Some(2_000),
                    Some(1),
                    false,
                    1_500,
                )
            });
        assert!(outcome.is_err(), "scheduled_jobs 已删，状态写应失败");
        drop(tx); // Err 路径不 commit → 回滚
        let n: i64 = conn
            .query_row("SELECT count(*) FROM scheduled_job_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "历史行必须随状态写一起回滚，不留单边");
    }

    #[test]
    fn upsert_rmw_rejects_stale_write() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        let mut job = ScheduledJob {
            id: "j1".into(),
            content: "内容".into(),
            schedule: "daily:09:00".into(),
            sched_last: None,
            enabled: None,
            created_at: Some(1),
            updated_at: Some(1),
            last_status: None,
            last_error: None,
            pause_on_failure: None,
            retry_max: None,
            retry_at: None,
            retry_count: None,
            mode: None,
            expected_updated_at: None,
        };
        upsert_scheduled_job(&conn, &job).unwrap();
        // 模拟并发修改：updated_at 已推进
        conn.execute(
            "UPDATE scheduled_jobs SET updated_at = 999 WHERE id = 'j1'",
            [],
        )
        .unwrap();
        job.expected_updated_at = Some(1); // 基线 = 旧快照
        job.content = "并发编辑".into();
        assert!(upsert_scheduled_job(&conn, &job).is_err(), "过期基线应被拒");
    }

    /// 失败重试/自动暂停策略必须随 upsert 落库——INSERT 与 ON CONFLICT 任一处
    /// 漏写这两列，UI 保存的策略就写库即丢（失败后不重试、不自动暂停）。
    #[test]
    fn upsert_persists_retry_policy_columns() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        let job = ScheduledJob {
            id: "j1".into(),
            content: "内容".into(),
            schedule: "daily:09:00".into(),
            sched_last: None,
            enabled: None,
            created_at: Some(1),
            updated_at: Some(1),
            last_status: None,
            last_error: None,
            pause_on_failure: Some(true),
            retry_max: Some(2),
            retry_at: None,
            retry_count: None,
            mode: None,
            expected_updated_at: None,
        };
        upsert_scheduled_job(&conn, &job).unwrap();
        let loaded = load_scheduled_job(&conn, "j1").unwrap().unwrap();
        assert_eq!(loaded.retry_max, Some(2));
        assert_eq!(loaded.pause_on_failure, Some(true));

        // 编辑路径（ON CONFLICT 更新分支）：改策略后原样写回
        let mut j2 = loaded;
        j2.retry_max = Some(0);
        j2.pause_on_failure = Some(false);
        j2.expected_updated_at = j2.updated_at;
        j2.updated_at = Some(999);
        upsert_scheduled_job(&conn, &j2).unwrap();
        let loaded2 = load_scheduled_job(&conn, "j1").unwrap().unwrap();
        assert_eq!(loaded2.retry_max, Some(0));
        assert_eq!(loaded2.pause_on_failure, Some(false));

        // NULL 语义：未配置（None）→ 列保持 NULL
        let mut j3 = loaded2;
        j3.retry_max = None;
        j3.pause_on_failure = None;
        j3.expected_updated_at = j3.updated_at;
        upsert_scheduled_job(&conn, &j3).unwrap();
        let loaded3 = load_scheduled_job(&conn, "j1").unwrap().unwrap();
        assert_eq!(loaded3.retry_max, None);
        assert_eq!(loaded3.pause_on_failure, None);
    }
}
