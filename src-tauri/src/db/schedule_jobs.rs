//! 定时任务（定时任务模块，N-SCHED）：**内容型定时作业**存储。
//!
//! 与任务卡定时的本质区别：作业在到点前**没有**对应的任务卡——用户只写
//! 「要做什么」的内容，调度器到点才根据内容新建任务卡并交给机器人执行
//! （设计取舍见 docs 侧方案：「新建任务卡」而非「给现有卡设定时」）。
//! 工作流定时不在本表（沿用 workflows.schedule 列，到点跑整张已有工作流）。
//!
//! 老数据迁移：tasks.schedule 仍存旧的「现有卡定时」，由 open_db 的
//! `migrate_legacy_task_schedules` 一次性搬进本表（content = 卡标题）后清空
//! 任务卡字段，此后 UI 只经本模块读写。

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
   retry_count INTEGER
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
    for (col, ty) in [
        ("last_status", "TEXT"),
        ("last_error", "TEXT"),
        ("pause_on_failure", "INTEGER"),
        ("retry_max", "INTEGER"),
        ("retry_at", "INTEGER"),
        ("retry_count", "INTEGER"),
    ] {
        let has: bool = conn
            .prepare("PRAGMA table_info(scheduled_jobs)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == col))
            })
            .map_err(|e| e.to_string())?;
        if !has {
            conn.execute(
                &format!("ALTER TABLE scheduled_jobs ADD COLUMN {col} {ty}"),
                [],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 内容长度硬顶（与 workflow goal 同量级：一段「要做什么」的描述）
pub const MAX_JOB_CONTENT: usize = 500;

/// 失败重试上限（借鉴 XXL-JOB 失败重试次数；写入口硬顶，防配置成死循环重烧 LLM）
pub const MAX_RETRY: i64 = 5;

/// 失败重试延迟（固定 5 分钟退避：不做指数退避——本地单机场景固定间隔足够可预期）
pub const RETRY_DELAY_MS: i64 = 5 * 60 * 1000;

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
     last_status, last_error, pause_on_failure, retry_max, retry_at, retry_count \
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
        let conflict = if expected == super::tasks::BASELINE_NULL_ROW {
            cur != Some(None)
        } else {
            cur.flatten() != Some(expected)
        };
        if conflict {
            return Err(format!(
                "{}：定时作业 {} 读快照后已被其他写者修改，写回被拒",
                super::tasks::CONFLICT_ERR_PREFIX,
                j.id
            ));
        }
    }
    conn.execute(
        "INSERT INTO scheduled_jobs (id, content, schedule, sched_last, enabled, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(id) DO UPDATE SET
           content=excluded.content, schedule=excluded.schedule,
           sched_last=excluded.sched_last, enabled=excluded.enabled,
           updated_at=excluded.updated_at",
        rusqlite::params![
            j.id,
            j.content,
            j.schedule,
            j.sched_last,
            j.enabled.map(|b| b as i64),
            j.created_at,
            j.updated_at,
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

// ────────────── 执行历史（XXL-JOB 调度日志借鉴） ──────────────

/// 落一条执行历史并裁剪到 JOB_RUNS_KEEP 条（调用方持锁）
pub fn record_job_run(
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

pub fn delete_scheduled_job(conn: &rusqlite::Connection, id: &str) -> Result<bool, String> {
    let n = conn
        .execute("DELETE FROM scheduled_jobs WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(n > 0)
}

// ────────────── 老数据迁移：tasks.schedule → scheduled_jobs ──────────────

/// 一次性幂等迁移：把「现有卡定时」搬成内容型作业后清空任务卡调度字段。
/// 迁移前后行为等价（原卡到点由机器人执行 → 作业到点新建卡并执行，
/// 卡内容同为 title），搬完任务卡不再携带 schedule，UI 与调度器以本表单源。
/// 幂等：搬完即清，重跑找不到带 schedule 的卡 → 0。
pub fn migrate_legacy_task_schedules(conn: &rusqlite::Connection) -> Result<usize, String> {
    let legacy: Vec<(String, String, String, Option<i64>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, COALESCE(TRIM(title), ''), schedule, sched_last FROM tasks \
                 WHERE deleted_at IS NULL AND TRIM(COALESCE(schedule,'')) <> ''",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
    };
    if legacy.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().timestamp_millis();
    let mut moved = 0usize;
    for (task_id, title, schedule, sched_last) in legacy {
        if title.is_empty() {
            continue; // 无内容可搬（理论不可达：title NOT NULL），保守跳过
        }
        conn.execute(
            "INSERT INTO scheduled_jobs (id, content, schedule, sched_last, enabled, created_at, updated_at)
             VALUES (?1,?2,?3,?4,NULL,?5,?5)
             ON CONFLICT(id) DO NOTHING",
            rusqlite::params![format!("job-{task_id}"), title, schedule, sched_last, now],
        )
        .map_err(|e| e.to_string())?;
        moved += 1;
    }
    conn.execute(
        "UPDATE tasks SET schedule = NULL, sched_last = NULL, enabled = NULL \
         WHERE deleted_at IS NULL AND TRIM(COALESCE(schedule,'')) <> ''",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(moved)
}

// ────────────── Tauri commands ──────────────

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
    crate::bot_scheduler::validate_schedule(schedule.trim()).map_err(|reason| {
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
        let now = chrono::Utc::now().timestamp_millis();
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

/// 编辑内容型定时作业（内容 / 时间 / 重试与自动暂停；None 参数 = 不改该字段）
#[tauri::command]
pub async fn scheduled_job_update(
    app: AppHandle,
    id: String,
    content: Option<String>,
    schedule: Option<String>,
    retry_max: Option<i64>,
    pause_on_failure: Option<bool>,
) -> CommandResult<ScheduledJob> {
    let new_content = match &content {
        Some(c) => Some(validate_content(c)?),
        None => None,
    };
    if let Some(s) = &schedule {
        crate::bot_scheduler::validate_schedule(s.trim()).map_err(|reason| {
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
        // 配置变更 → 重试状态失效（下一周期从零计）
        job.retry_at = None;
        job.retry_count = None;
        job.expected_updated_at = job.updated_at; // RMW 基线 = 快照 updated_at
        job.updated_at = Some(chrono::Utc::now().timestamp_millis());
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
        let conn = super::open_db(&app)?;
        delete_scheduled_job(&conn, &id).map_err(CommandError::from)
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
        conn.execute_batch(crate::db::tasks::TASKS_DDL).unwrap();
        conn
    }

    #[test]
    fn legacy_task_schedule_migrates_to_job_and_clears() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        let now = 1_000_000i64;
        for (id, title, sched) in [
            ("t1", "每天写日报", "daily:09:00"),
            ("t2", "周一例会准备", "weekly:1:08:30"),
        ] {
            conn.execute(
                "INSERT INTO tasks (id, title, col, schedule, sched_last, updated_at) \
                 VALUES (?1, ?2, 'todo', ?3, 123, ?4)",
                rusqlite::params![id, title, sched, now],
            )
            .unwrap();
        }
        // 软删卡不参与迁移（其调度字段语义上已被删除流程清掉，防御性跳过）
        conn.execute(
            "INSERT INTO tasks (id, title, col, schedule, deleted_at, updated_at) \
             VALUES ('t3', '已删卡', 'todo', 'daily:10:00', 1, ?1)",
            [now],
        )
        .unwrap();

        let moved = migrate_legacy_task_schedules(&conn).unwrap();
        assert_eq!(moved, 2, "两张未删卡迁移");

        // 作业行：content=卡标题，schedule/sched_last 原样带过来
        let jobs = load_scheduled_jobs(&conn).unwrap();
        assert_eq!(jobs.len(), 2);
        let t1 = jobs.iter().find(|j| j.id == "job-t1").unwrap();
        assert_eq!(t1.content, "每天写日报");
        assert_eq!(t1.schedule, "daily:09:00");
        assert_eq!(t1.sched_last, Some(123));
        assert!(t1.sched_enabled());

        // 任务卡调度字段已清空（幂等前提），软删卡原样不动
        let dirty: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE deleted_at IS NULL \
                 AND COALESCE(schedule,'') <> ''",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dirty, 0);
        let t3: Option<String> = conn
            .query_row("SELECT schedule FROM tasks WHERE id='t3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(t3.as_deref(), Some("daily:10:00"));

        // 幂等：重跑 0
        assert_eq!(migrate_legacy_task_schedules(&conn).unwrap(), 0);
    }

    #[test]
    fn job_runs_trimmed_to_keep_limit() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        for i in 0..(JOB_RUNS_KEEP + 5) as i64 {
            record_job_run(&conn, "j1", 1_000 + i, "ok", Some(i), Some("ok"), Some("c")).unwrap();
        }
        let runs = load_job_runs(&conn, "j1", JOB_RUNS_KEEP).unwrap();
        assert_eq!(runs.len(), JOB_RUNS_KEEP, "只保留最近 N 条");
        assert_eq!(
            runs[0].fired_at,
            1_000 + JOB_RUNS_KEEP as i64 + 4,
            "倒序最新在前"
        );
        // 其他作业的历史互不干扰
        record_job_run(&conn, "j2", 9_999, "fail", None, Some("boom"), None).unwrap();
        assert_eq!(load_job_runs(&conn, "j2", 10).unwrap().len(), 1);
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
}
