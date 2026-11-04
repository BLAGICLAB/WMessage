//! 工作单元（work_items 表）：主 agent 自己在干的活的一等表达——是什么活、
//! 做到哪一步（stage）、结果如何。与 `subagents`（子 agent 派工）、`tasks`
//! （用户看板）、`exec_traces`（观测面）并存不合并，用 `session_id` 关联。
//!
//! 粒度：**每会话一行**——session_id partial UNIQUE 撑起 upsert，
//! 开票是机械动作（每轮模型循环 upsert），objective 随轮次更新。
//!
//! 阶段二分：person 族 = 没有用户介入就动不了（面板「需要人」聚合的依据）；
//! working 族 = 自己在推进。批 5 产出 working / awaiting_answer / idle / failed
//! 四态；undelivered / awaiting_review / blocked / queued 为预留词，尚无产出路径。

use rusqlite::OptionalExtension;
use serde::Serialize;

/// 单源 DDL：open_db 与测试建表共用（老库打开即建空表，无迁移）
pub const WORK_ITEMS_DDL: &str = "CREATE TABLE IF NOT EXISTS work_items (
   id             TEXT PRIMARY KEY,
   kind           TEXT NOT NULL,
   origin_ref     TEXT,
   session_id     TEXT,
   objective      TEXT,
   acceptance     TEXT,
   stage          TEXT NOT NULL,
   result         TEXT,
   error          TEXT,
   created_at     INTEGER NOT NULL,
   updated_at     INTEGER NOT NULL
 );
 CREATE UNIQUE INDEX IF NOT EXISTS idx_work_items_session
   ON work_items(session_id) WHERE session_id IS NOT NULL;
 CREATE INDEX IF NOT EXISTS idx_work_items_stage ON work_items(stage);";

pub const STAGE_WORKING: &str = "working";
pub const STAGE_AWAITING_ANSWER: &str = "awaiting_answer";
pub const STAGE_IDLE: &str = "idle";
pub const STAGE_FAILED: &str = "failed";

/// person 族：没有用户介入就动不了——「需要人处理」聚合面的数据源
pub const PERSON_STAGES: [&str; 5] = [
    "undelivered",
    STAGE_AWAITING_ANSWER,
    "awaiting_review",
    "blocked",
    STAGE_FAILED,
];

pub fn is_person_stage(stage: &str) -> bool {
    PERSON_STAGES.contains(&stage)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemRow {
    pub id: String,
    pub kind: String,
    pub origin_ref: Option<String>,
    pub session_id: Option<String>,
    pub objective: Option<String>,
    pub acceptance: Option<String>,
    pub stage: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<WorkItemRow> {
    Ok(WorkItemRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        origin_ref: r.get(2)?,
        session_id: r.get(3)?,
        objective: r.get(4)?,
        acceptance: r.get(5)?,
        stage: r.get(6)?,
        result: r.get(7)?,
        error: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

const COLS: &str = "SELECT id, kind, origin_ref, session_id, objective, acceptance, \
                    stage, result, error, created_at, updated_at FROM work_items";

/// 按会话开票/更新（机械动作，每轮模型循环调）：有行则更新 objective（给了
/// 非空值才覆盖）与 updated_at，没有则建行（stage=working）。
/// acceptance 每轮重写：开票先清空（acceptance 是「本轮计划声明」的随行判据，
/// 无计划轮不留旧账——机器判定不得拿上一轮的过期判据下结论），计划轮由
/// set_acceptance 紧随其后写入。
pub fn upsert_for_session(
    conn: &rusqlite::Connection,
    session_id: &str,
    kind: &str,
    objective: Option<&str>,
    origin_ref: Option<&str>,
) -> Result<WorkItemRow, String> {
    let now = super::now_ms();
    let existing: Option<WorkItemRow> = conn
        .query_row(
            &format!("{COLS} WHERE session_id = ?1"),
            [session_id],
            row_from,
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(mut row) = existing {
        let has_objective = objective.is_some_and(|o| !o.trim().is_empty());
        conn.execute(
            "UPDATE work_items SET \
               objective = CASE WHEN ?2 THEN ?3 ELSE objective END, \
               stage = CASE WHEN stage = 'failed' THEN ?4 ELSE stage END, \
               acceptance = NULL, \
               updated_at = ?5 \
             WHERE session_id = ?1",
            rusqlite::params![
                session_id,
                has_objective,
                objective.unwrap_or_default(),
                STAGE_WORKING,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        if has_objective {
            row.objective = objective.map(str::to_string);
        }
        row.acceptance = None;
        // 失败行被新一轮唤醒 → 回到 working（重试语义）
        if row.stage == STAGE_FAILED {
            row.stage = STAGE_WORKING.to_string();
        }
        row.updated_at = now;
        return Ok(row);
    }
    let row = WorkItemRow {
        id: format!("wi_{}", uuid::Uuid::new_v4().simple()),
        kind: kind.to_string(),
        origin_ref: origin_ref.map(str::to_string),
        session_id: Some(session_id.to_string()),
        objective: objective
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        acceptance: None,
        stage: STAGE_WORKING.to_string(),
        result: None,
        error: None,
        created_at: now,
        updated_at: now,
    };
    conn.execute(
        "INSERT INTO work_items \
           (id, kind, origin_ref, session_id, objective, acceptance, stage, result, error, created_at, updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        rusqlite::params![
            row.id,
            row.kind,
            row.origin_ref,
            row.session_id,
            row.objective,
            row.acceptance,
            row.stage,
            row.result,
            row.error,
            row.created_at,
            row.updated_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(row)
}

/// 阶段定点更新（ask_user 挂起等场景）
pub fn set_stage(
    conn: &rusqlite::Connection,
    session_id: &str,
    stage: &str,
    error: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE work_items SET stage = ?2, error = COALESCE(?3, error), updated_at = ?4 \
         WHERE session_id = ?1",
        rusqlite::params![session_id, stage, error, super::now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 轮收尾：成功 → idle（等待新指令），失败/停止 → failed；result/eerror 随写
pub fn finish(
    conn: &rusqlite::Connection,
    session_id: &str,
    ok: bool,
    result: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let stage = if ok { STAGE_IDLE } else { STAGE_FAILED };
    conn.execute(
        "UPDATE work_items SET stage = ?2, result = COALESCE(?3, result), \
           error = COALESCE(?4, error), updated_at = ?5 \
         WHERE session_id = ?1",
        rusqlite::params![session_id, stage, result, error, super::now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 写验收标准（批 6 的机器判据管线读它）
pub fn set_acceptance(
    conn: &rusqlite::Connection,
    session_id: &str,
    acceptance: &str,
) -> Result<(), String> {
    conn.execute(
        "UPDATE work_items SET acceptance = ?2, updated_at = ?3 WHERE session_id = ?1",
        rusqlite::params![session_id, acceptance, super::now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 启动清扫：working / awaiting_answer 行的执行现场随进程消失——重启后一律 failed。
/// 待答行一并收：ask 等待器在内存里，重启后那个等待已无意义，留着会永远挂在
/// 「后台待办」。清扫只在启动时跑（tick 不收待答——提问等待可达 24h 属合法长挂）。
/// 与僵尸子 agent 清扫同口径（只认事实）。
pub fn sweep_all_working(conn: &rusqlite::Connection) -> Result<usize, String> {
    conn.execute(
        "UPDATE work_items SET stage = ?2, \
           error = COALESCE(error, '进程重启，执行现场已不存在'), updated_at = ?3 \
         WHERE stage = ?1 OR stage = 'awaiting_answer'",
        rusqlite::params![STAGE_WORKING, STAGE_FAILED, super::now_ms()],
    )
    .map_err(|e| e.to_string())
}

/// tick 重估：working 行超过 stale_ms 无更新（轮死残留，正常轮收尾必写一次）
/// → failed。awaiting_answer 不参与（提问等待可达 24h，属合法长挂）。
pub fn sweep_stale_working(conn: &rusqlite::Connection, stale_ms: i64) -> Result<usize, String> {
    let cutoff = super::now_ms() - stale_ms;
    conn.execute(
        "UPDATE work_items SET stage = ?2, \
           error = COALESCE(error, '轮超时无更新，按中断收尾'), updated_at = ?3 \
         WHERE stage = ?1 AND updated_at < ?4",
        rusqlite::params![STAGE_WORKING, STALE_SWEEP_STAGE, super::now_ms(), cutoff],
    )
    .map_err(|e| e.to_string())
}

const STALE_SWEEP_STAGE: &str = STAGE_FAILED;

/// 「需要人处理」聚合面数据源（批 7 查询时聚合用）：person 族的行
pub fn list_needing_human(
    conn: &rusqlite::Connection,
    limit: usize,
) -> Result<Vec<WorkItemRow>, String> {
    list_by_stages(conn, &PERSON_STAGES, limit)
}

/// 最近工作项（活动页/面板数据源）
pub fn list_recent(conn: &rusqlite::Connection, limit: usize) -> Result<Vec<WorkItemRow>, String> {
    list_by_stages(
        conn,
        &[
            STAGE_WORKING,
            STAGE_AWAITING_ANSWER,
            STAGE_IDLE,
            STAGE_FAILED,
        ],
        limit,
    )
}

fn list_by_stages(
    conn: &rusqlite::Connection,
    stages: &[&str],
    limit: usize,
) -> Result<Vec<WorkItemRow>, String> {
    let placeholders = stages.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "{COLS} WHERE stage IN ({placeholders}) ORDER BY updated_at DESC, rowid DESC LIMIT ?{}",
        stages.len() + 1
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut params: Vec<rusqlite::types::Value> = stages
        .iter()
        .map(|s| rusqlite::types::Value::Text((*s).to_string()))
        .collect();
    params.push(rusqlite::types::Value::Integer(limit as i64));
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), row_from)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn load_by_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<WorkItemRow>, String> {
    conn.query_row(
        &format!("{COLS} WHERE session_id = ?1"),
        [session_id],
        row_from,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 轮收尾 + 机器判据管线：行上有验收标准（JSON 数组，`file_exists:` /
/// `file_nonempty:` 语法，相对 AI_Gen_Files）就逐条跑服务端判定——fail-closed，
/// 结论进 result JSON 并留审计；阶段语义不变（判据只记录、不改判，改判推翻
/// 语义仍归子 agent 的收尾契约）。无验收标准的行走普通 finish。
pub(crate) async fn finish_with_machine_check<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
    ok: bool,
    error: Option<&str>,
) {
    let app2 = app.clone();
    let sid2 = session_id.to_string();
    let checked =
        tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
            let conn = crate::db::open_db(&app2)?;
            let Some(row) = load_by_session(&conn, &sid2)? else {
                return Ok(None);
            };
            let Some(acc) = row.acceptance.as_deref() else {
                return Ok(None);
            };
            let Ok(criteria) = serde_json::from_str::<Vec<String>>(acc) else {
                return Ok(None);
            };
            if criteria.is_empty() {
                return Ok(None);
            }
            let dir = crate::db::gen_dir(&app2).map_err(|e| e.to_string())?;
            let verdicts = crate::bot::subagent_digest::eval_machine_criteria(&criteria, &dir);
            if verdicts.is_empty() {
                return Ok(None);
            }
            let total = verdicts.len();
            let passed = verdicts.iter().filter(|v| v.met).count();
            let failed_items: Vec<&str> = verdicts
                .iter()
                .filter(|v| !v.met)
                .map(|v| v.criterion.as_str())
                .collect();
            crate::audit_event!(
                &app2,
                crate::audit::AuditLevel::Info,
                "work_item.machine_check",
                "session" => sid2,
                "passed" => passed,
                "total" => total,
                "failed" => failed_items.join(" | ")
            );
            Ok(Some(
                serde_json::json!({ "machineCheck": { "passed": passed, "total": total } })
                    .to_string(),
            ))
        })
        .await;
    let result_json = match checked {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            eprintln!("[work_items] 机器判据失败（跳过，只落阶段）：{e}");
            None
        }
        Err(e) => {
            eprintln!("[work_items] 机器判据线程失败（跳过，只落阶段）：{e}");
            None
        }
    };
    let sid3 = session_id.to_string();
    let err3 = error.map(str::to_string);
    let _ = crate::db::lock::with_write_conn(app, move |conn| {
        finish(conn, &sid3, ok, result_json.as_deref(), err3.as_deref())
    })
    .await;
}

// Tauri commands

/// 「需要人处理」聚合项（查询时聚合，不新增 notifications kind）：
/// 三源合并——工作项 person 族 / 失败的子 agent / 失败且启用的定时作业
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NeedsAttentionItem {
    /// work_item | subagent | scheduled_job
    pub source: String,
    pub id: String,
    pub title: String,
    pub detail: Option<String>,
    pub at: i64,
}

/// 查询时聚合：一处看全需要人的事（不改 notifications 表）
#[tauri::command]
pub async fn needs_attention_list(
    app: tauri::AppHandle,
    limit: Option<usize>,
) -> Result<Vec<NeedsAttentionItem>, crate::error::CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let limit = limit.unwrap_or(50).min(200);
        let conn = super::open_db(&app).map_err(crate::error::CommandError::DbError)?;
        let mut out: Vec<NeedsAttentionItem> = Vec::new();
        // ① 工作项 person 族
        for r in list_needing_human(&conn, limit)? {
            let fallback = r
                .objective
                .clone()
                .or_else(|| r.session_id.clone())
                .unwrap_or_else(|| r.id.clone());
            out.push(NeedsAttentionItem {
                source: "work_item".into(),
                id: r.id,
                title: fallback,
                detail: r.error,
                at: r.updated_at,
            });
        }
        // ② 失败/超预算的子 agent
        for st in [
            crate::db::subagents::SubagentStatus::Failed,
            crate::db::subagents::SubagentStatus::BudgetExceeded,
        ] {
            for r in crate::db::subagents::load_subagents_by_status(&conn, st)? {
                out.push(NeedsAttentionItem {
                    source: "subagent".into(),
                    id: r.id,
                    title: crate::bot::truncate_for_log(r.objective.trim(), 40),
                    detail: r.error,
                    at: r.finished_at.unwrap_or(r.created_at),
                });
            }
        }
        // ③ 失败且启用的定时作业
        for j in crate::db::load_scheduled_jobs(&conn)? {
            if j.sched_enabled() && j.last_status.as_deref() == Some("fail") {
                out.push(NeedsAttentionItem {
                    source: "scheduled_job".into(),
                    id: j.id,
                    title: crate::bot::truncate_for_log(j.content.trim(), 40),
                    detail: j.last_error,
                    at: j.updated_at.unwrap_or(0),
                });
            }
        }
        out.sort_by_key(|item| std::cmp::Reverse(item.at));
        out.truncate(limit);
        Ok(out)
    })
    .await
    .map_err(|e| crate::error::CommandError::Internal(format!("聚合读取线程 join 失败：{e}")))?
}

/// 最近工作项（前端面板/活动页增补数据源）
#[tauri::command]
pub async fn work_items_list(
    app: tauri::AppHandle,
    limit: Option<usize>,
) -> Result<Vec<WorkItemRow>, crate::error::CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app).map_err(crate::error::CommandError::DbError)?;
        list_recent(&conn, limit.unwrap_or(50).min(200))
            .map_err(crate::error::CommandError::DbError)
    })
    .await
    .map_err(|e| crate::error::CommandError::Internal(format!("工作项读取线程 join 失败：{e}")))?
}

/// 「需要人处理」聚合查询（批 7 查询时聚合：不新增 notifications kind）
#[tauri::command]
pub async fn work_items_needing_human(
    app: tauri::AppHandle,
    limit: Option<usize>,
) -> Result<Vec<WorkItemRow>, crate::error::CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app).map_err(crate::error::CommandError::DbError)?;
        list_needing_human(&conn, limit.unwrap_or(50).min(200))
            .map_err(crate::error::CommandError::DbError)
    })
    .await
    .map_err(|e| crate::error::CommandError::Internal(format!("工作项读取线程 join 失败：{e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(WORK_ITEMS_DDL).unwrap();
        conn
    }

    fn row(conn: &rusqlite::Connection, session: &str) -> WorkItemRow {
        load_by_session(conn, session).unwrap().unwrap()
    }

    #[test]
    fn upsert_is_one_row_per_session_and_updates_objective() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        let a = upsert_for_session(&conn, "s1", "chat", Some("第一轮目标"), None).unwrap();
        let b = upsert_for_session(&conn, "s1", "chat", Some("第二轮目标"), None).unwrap();
        assert_eq!(a.id, b.id, "同会话只一行");
        assert_eq!(row(&conn, "s1").objective.as_deref(), Some("第二轮目标"));
        assert_eq!(row(&conn, "s1").stage, STAGE_WORKING);
        // 唯一索引兜底：绕过 upsert 的直插必败
        let dup = conn.execute(
            "INSERT INTO work_items (id, kind, session_id, stage, created_at, updated_at) \
             VALUES ('wi_x', 'chat', 's1', 'working', 1, 1)",
            [],
        );
        assert!(dup.is_err(), "session_id 唯一索引必须挡住第二行");
    }

    #[test]
    fn stage_transitions_and_finish() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_for_session(&conn, "s1", "chat", Some("目标"), None).unwrap();
        set_stage(&conn, "s1", STAGE_AWAITING_ANSWER, None).unwrap();
        assert_eq!(row(&conn, "s1").stage, STAGE_AWAITING_ANSWER);
        assert!(is_person_stage(STAGE_AWAITING_ANSWER));
        finish(&conn, "s1", true, Some(r#"{"ok":1}"#), None).unwrap();
        let r = row(&conn, "s1");
        assert_eq!(r.stage, STAGE_IDLE);
        assert_eq!(r.result.as_deref(), Some(r#"{"ok":1}"#));
        // 失败后新一轮 upsert 回到 working（重试语义）
        finish(&conn, "s1", false, None, Some("boom")).unwrap();
        upsert_for_session(&conn, "s1", "chat", None, None).unwrap();
        assert_eq!(row(&conn, "s1").stage, STAGE_WORKING);
        assert_eq!(
            row(&conn, "s1").error.as_deref(),
            Some("boom"),
            "失败原因保留"
        );
    }

    #[test]
    fn sweeps_cover_restart_and_stale_turns() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_for_session(&conn, "s1", "chat", Some("目标"), None).unwrap();
        // 启动清扫：working → failed
        assert_eq!(sweep_all_working(&conn).unwrap(), 1);
        assert_eq!(row(&conn, "s1").stage, STAGE_FAILED);
        // 新一轮把它唤醒回 working，再走陈旧清扫（老行按 cutoff 判）
        upsert_for_session(&conn, "s1", "chat", None, None).unwrap();
        conn.execute(
            "UPDATE work_items SET updated_at = 1 WHERE session_id = 's1'",
            [],
        )
        .unwrap();
        assert_eq!(
            sweep_stale_working(&conn, 60_000).unwrap(),
            1,
            "陈旧轮按中断收"
        );
        assert_eq!(row(&conn, "s1").stage, STAGE_FAILED);
        // awaiting_answer 不被陈旧清扫碰（合法长挂）
        upsert_for_session(&conn, "s2", "chat", Some("目标"), None).unwrap();
        set_stage(&conn, "s2", STAGE_AWAITING_ANSWER, None).unwrap();
        conn.execute(
            "UPDATE work_items SET updated_at = 1 WHERE session_id = 's2'",
            [],
        )
        .unwrap();
        assert_eq!(sweep_stale_working(&conn, 60_000).unwrap(), 0);
        assert_eq!(row(&conn, "s2").stage, STAGE_AWAITING_ANSWER);
    }

    #[test]
    fn needing_human_lists_only_person_stages() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_for_session(&conn, "s1", "chat", Some("挂着"), None).unwrap();
        set_stage(&conn, "s1", STAGE_AWAITING_ANSWER, None).unwrap();
        upsert_for_session(&conn, "s2", "chat", Some("进行中"), None).unwrap();
        upsert_for_session(&conn, "s3", "chat", Some("失败"), None).unwrap();
        finish(&conn, "s3", false, None, Some("x")).unwrap();
        let rows = list_needing_human(&conn, 50).unwrap();
        let sessions: Vec<&str> = rows
            .iter()
            .map(|r| r.session_id.as_deref().unwrap())
            .collect();
        assert_eq!(sessions, vec!["s3", "s1"], "只列 person 族，按更新时间倒序");
        assert!(list_needing_human(&conn, 50)
            .unwrap()
            .iter()
            .all(|r| is_person_stage(&r.stage)));
    }

    #[test]
    fn startup_sweep_collects_awaiting_answer_too() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_for_session(&conn, "s1", "chat", Some("长轮"), None).unwrap();
        upsert_for_session(&conn, "s2", "chat", Some("提问"), None).unwrap();
        upsert_for_session(&conn, "s3", "chat", Some("已完成"), None).unwrap();
        set_stage(&conn, "s2", STAGE_AWAITING_ANSWER, None).unwrap();
        finish(&conn, "s3", true, None, None).unwrap();
        // 待答行一并收：ask 等待器在内存里，重启后等待已无意义
        assert_eq!(sweep_all_working(&conn).unwrap(), 2);
        assert_eq!(row(&conn, "s1").stage, STAGE_FAILED);
        assert_eq!(row(&conn, "s2").stage, STAGE_FAILED);
        assert!(
            row(&conn, "s2")
                .error
                .as_deref()
                .unwrap_or("")
                .contains("进程重启"),
            "待答行带进程重启类 error"
        );
        assert_eq!(row(&conn, "s3").stage, STAGE_IDLE, "idle 不被误伤");
    }

    #[test]
    fn upsert_clears_acceptance_for_the_new_turn() {
        let conn = mem_conn();
        let _g = crate::db::lock_db_write();
        upsert_for_session(&conn, "s1", "chat", Some("有计划的轮"), None).unwrap();
        set_acceptance(&conn, "s1", r#"["file_exists:a.txt"]"#).unwrap();
        assert!(row(&conn, "s1").acceptance.is_some(), "计划轮判据在行");
        // 下一轮无计划：开票即清——机器判定不得拿上一轮的过期判据下结论
        upsert_for_session(&conn, "s1", "chat", Some("无计划的轮"), None).unwrap();
        assert!(row(&conn, "s1").acceptance.is_none(), "上一轮判据必须被清");
        // 计划轮：开票清空后由 set_acceptance 紧随写入，不受清空影响
        set_acceptance(&conn, "s1", r#"["file_exists:b.txt"]"#).unwrap();
        assert_eq!(
            row(&conn, "s1").acceptance.as_deref(),
            Some(r#"["file_exists:b.txt"]"#)
        );
    }
}
