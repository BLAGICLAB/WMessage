//! 子 agent 编排持久化（SUBA-1）
//!
//! 设计基准：docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md §3（存储）§6（预算默认值）。
//! 一行 = 一个受管子 agent：orchestrator spawn 时插入（status=queued），runner 推进
//! 状态机，check_subagent 幂等可轮询（事件丢失可查表、重启状态不丢）。
//!
//! 列对齐设计 §3，另加三列（增列不违设计，勘误已在批 spec 登记）：
//! - `session_id`：子 agent 自己的执行会话（串链围观/审计；parent_session_id 是派发者主会话）
//! - `trace_id`：贯穿 bot.log 的串链键
//! - `acceptance_json`：验收标准机器侧副本（双写的另一面在子卡 note）

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// 预算默认值与硬顶（设计 §6 已拍板；钳制实现见 [SubagentBudget::clamped]）。
pub const DEFAULT_MAX_TURNS: u32 = 30;
pub const MAX_TURNS_HARD_CAP: u32 = 50;
pub const DEFAULT_MAX_TOOL_CALLS: u32 = 100;
pub const DEFAULT_MAX_WALL_SECONDS: u64 = 600;

/// 子 agent 生命周期六态（设计 §3 状态机）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    BudgetExceeded,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentStatus::Queued => "queued",
            SubagentStatus::Running => "running",
            SubagentStatus::Succeeded => "succeeded",
            SubagentStatus::Failed => "failed",
            SubagentStatus::Cancelled => "cancelled",
            SubagentStatus::BudgetExceeded => "budget_exceeded",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => SubagentStatus::Queued,
            "running" => SubagentStatus::Running,
            "succeeded" => SubagentStatus::Succeeded,
            "failed" => SubagentStatus::Failed,
            "cancelled" => SubagentStatus::Cancelled,
            "budget_exceeded" => SubagentStatus::BudgetExceeded,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            SubagentStatus::Succeeded
                | SubagentStatus::Failed
                | SubagentStatus::Cancelled
                | SubagentStatus::BudgetExceeded
        )
    }
}

/// 子 agent 工具白名单档（设计 §5）。A 期三档；映射到具体工具清单在 registry（A2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentProfile {
    Research,
    Coder,
    General,
}

impl SubagentProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentProfile::Research => "research",
            SubagentProfile::Coder => "coder",
            SubagentProfile::General => "general",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "research" => SubagentProfile::Research,
            "coder" => SubagentProfile::Coder,
            "general" => SubagentProfile::General,
            _ => return None,
        })
    }
}

/// 预算三硬顶（设计 §6 默认值）。上卡可见（子卡 budget 字段），不是隐藏参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentBudget {
    /// 工具循环轮数；默认 30，硬顶 50（spawn 时钳制）
    pub max_turns: u32,
    /// 累计工具调用次数；默认 100（强制点在 dispatch 层，A3）
    pub max_tool_calls: u32,
    /// 墙钟秒数；默认 600，排队时间不计入（从 running 起算）
    pub max_wall_seconds: u64,
}

impl Default for SubagentBudget {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_TURNS,
            max_tool_calls: DEFAULT_MAX_TOOL_CALLS,
            max_wall_seconds: DEFAULT_MAX_WALL_SECONDS,
        }
    }
}

impl SubagentBudget {
    /// 预算钳制：turns 硬顶 50、tool_calls/wall 下限 1。
    /// **所有持久化写口（spawn / task_patch）必须过本函数**——字段是 pub 的，
    /// 构造侧不受限，钳制契约靠两个写口强制（OCR r1 high 采纳：防 task_patch 旁路）。
    pub fn clamped(mut self) -> Self {
        self.max_turns = self.max_turns.clamp(1, MAX_TURNS_HARD_CAP);
        self.max_tool_calls = self.max_tool_calls.max(1);
        self.max_wall_seconds = self.max_wall_seconds.max(1);
        self
    }
}

/// 状态机合法转移表（设计 §3）：
/// queued → running | cancelled；running → 4 终态；终态一律禁移（含自环——
/// 同态重入的放行走 [update_subagent_status] 的同态短路，不经本表）。
pub fn transition_allowed(from: SubagentStatus, to: SubagentStatus) -> bool {
    use SubagentStatus::*;
    match from {
        Queued => matches!(to, Running | Cancelled),
        Running => matches!(to, Succeeded | Failed | Cancelled | BudgetExceeded),
        // 终态：事实已发生，不允许改写（部分结果已落库，重跑 = 重新 spawn）
        Succeeded | Failed | Cancelled | BudgetExceeded => false,
    }
}

/// subagents 表一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRow {
    pub id: String,
    pub status: SubagentStatus,
    pub profile: SubagentProfile,
    /// per-spawn 透传的模型名；None = 回退当前 active model（A 期不映射，B 期分层）
    pub model: Option<String>,
    /// 派发者主会话（主 agent 的会话 id）
    pub parent_session_id: Option<String>,
    /// 子 agent 自己的执行会话 id（runner 建会话后回填）
    pub session_id: Option<String>,
    /// 子任务卡 id（一子 agent 一子卡）
    pub task_id: String,
    pub objective: String,
    pub trace_id: String,
    /// 验收标准 Vec<String> 的 JSON（双写机器侧；人可见侧在子卡 note）
    pub acceptance_json: Option<String>,
    /// SubagentBudget 的 JSON
    pub budget_json: Option<String>,
    /// 收尾结构化结果（设计 §7 schema）的 JSON；解析失败时缺省（原文在 error 侧链路）
    pub result_json: Option<String>,
    /// 失败/取消/预算触达原因
    pub error: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// subagents 表 DDL（单源真相：open_db 与各测试建表共用同一份，防 schema 漂移——
/// OCR r1 采纳）。task_id UNIQUE 钉死「一子 agent 一子卡」不变式（并发 spawn 重试
/// 也不可能双插）；status / parent_session_id 加索引（按状态扫队列、按主会话串链）。
pub(crate) const SUBAGENTS_DDL: &str = "CREATE TABLE IF NOT EXISTS subagents (
           id                TEXT PRIMARY KEY,
           status            TEXT NOT NULL,
           profile           TEXT NOT NULL,
           model             TEXT,
           parent_session_id TEXT,
           session_id        TEXT,
           task_id           TEXT NOT NULL UNIQUE,
           objective         TEXT NOT NULL,
           trace_id          TEXT NOT NULL,
           acceptance_json   TEXT,
           budget_json       TEXT,
           result_json       TEXT,
           error             TEXT,
           created_at        INTEGER NOT NULL,
           started_at        INTEGER,
           finished_at       INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_subagents_status ON subagents(status);
         CREATE INDEX IF NOT EXISTS idx_subagents_parent ON subagents(parent_session_id);";

pub fn insert_subagent(conn: &rusqlite::Connection, r: &SubagentRow) -> Result<(), String> {
    conn.execute(
        "INSERT INTO subagents (id, status, profile, model, parent_session_id, session_id,
             task_id, objective, trace_id, acceptance_json, budget_json, result_json,
             error, created_at, started_at, finished_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        rusqlite::params![
            r.id,
            r.status.as_str(),
            r.profile.as_str(),
            r.model,
            r.parent_session_id,
            r.session_id,
            r.task_id,
            r.objective,
            r.trace_id,
            r.acceptance_json,
            r.budget_json,
            r.result_json,
            r.error,
            r.created_at,
            r.started_at,
            r.finished_at,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

const SQL_SELECT_BY_ID: &str = "SELECT id, status, profile, model, parent_session_id, session_id, \
     task_id, objective, trace_id, acceptance_json, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE id = ?1";
const SQL_SELECT_BY_TASK: &str = "SELECT id, status, profile, model, parent_session_id, session_id, \
     task_id, objective, trace_id, acceptance_json, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1";
const SQL_SELECT_BY_STATUS: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, \
     task_id, objective, trace_id, acceptance_json, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE status = ?1 ORDER BY created_at";

fn row_to_subagent(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubagentRow> {
    let id: String = row.get(0)?;
    let status_s: String = row.get(1)?;
    let profile_s: String = row.get(2)?;
    // 未知枚举串 = schema 漂移 / 外部改库：warn + 兜底读取（原值未动），
    // 与 load_all 的 JSON 损坏 warn 契约同款，不让整表读炸也不静默吞。
    let status = SubagentStatus::from_str(&status_s).unwrap_or_else(|| {
        eprintln!(
            "[db] subagent {id} 的 status 值「{status_s}」无法识别，按 failed 兜底读取（原值未动）"
        );
        SubagentStatus::Failed
    });
    let profile = SubagentProfile::from_str(&profile_s).unwrap_or_else(|| {
        eprintln!("[db] subagent {id} 的 profile 值「{profile_s}」无法识别，按 general 兜底读取（原值未动）");
        SubagentProfile::General
    });
    Ok(SubagentRow {
        id,
        status,
        profile,
        model: row.get(3)?,
        parent_session_id: row.get(4)?,
        session_id: row.get(5)?,
        task_id: row.get(6)?,
        objective: row.get(7)?,
        trace_id: row.get(8)?,
        acceptance_json: row.get(9)?,
        budget_json: row.get(10)?,
        result_json: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        started_at: row.get(14)?,
        finished_at: row.get(15)?,
    })
}

pub fn load_subagent(conn: &rusqlite::Connection, id: &str) -> Result<Option<SubagentRow>, String> {
    conn.query_row(SQL_SELECT_BY_ID, [id], row_to_subagent)
        .optional()
        .map_err(|e| e.to_string())
}

/// 按子任务卡 id 反查（一子 agent 一子卡；task_id 带 UNIQUE 约束兜底防双插）。
pub fn find_subagent_by_task(
    conn: &rusqlite::Connection,
    task_id: &str,
) -> Result<Option<SubagentRow>, String> {
    conn.query_row(SQL_SELECT_BY_TASK, [task_id], row_to_subagent)
        .optional()
        .map_err(|e| e.to_string())
}

/// 按状态取全部行（A3 排队恢复 / 串链审计用）。
pub fn load_subagents_by_status(
    conn: &rusqlite::Connection,
    status: SubagentStatus,
) -> Result<Vec<SubagentRow>, String> {
    let mut stmt = conn
        .prepare(SQL_SELECT_BY_STATUS)
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([status.as_str()], row_to_subagent)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 推进状态机：校验转移合法 → 打 started/finished 戳 → 回写。非法转移响亮失败。
/// 允许同态重入（幂等收敛：runner 重试同一终态写不炸）。
pub fn update_subagent_status(
    conn: &rusqlite::Connection,
    id: &str,
    to: SubagentStatus,
    now: i64,
    error: Option<&str>,
) -> Result<SubagentRow, String> {
    let row = load_subagent(conn, id)?.ok_or_else(|| format!("subagent 不存在：{id}"))?;
    // 空串 reason 视同 None——防 Some("") 经 COALESCE 覆盖既有失败原因（OCR r2 采纳）
    let error = error.filter(|s| !s.trim().is_empty());
    if row.status != to && !transition_allowed(row.status, to) {
        return Err(format!(
            "subagent {id} 非法状态转移：{} → {}",
            row.status.as_str(),
            to.as_str()
        ));
    }
    let started_at = match to {
        SubagentStatus::Running if row.started_at.is_none() => Some(now),
        _ => row.started_at,
    };
    // 同态重入保留原 finished_at——重试不改写首收尾时刻，审计时长口径稳定（OCR r1 采纳）
    let finished_at = if row.status == to {
        row.finished_at
    } else if to.is_terminal() {
        Some(now)
    } else {
        row.finished_at
    };
    conn.execute(
        "UPDATE subagents SET status=?2, started_at=?3, finished_at=?4, error=COALESCE(?5, error) WHERE id=?1",
        rusqlite::params![id, to.as_str(), started_at, finished_at, error],
    )
    .map_err(|e| e.to_string())?;
    load_subagent(conn, id)?.ok_or_else(|| format!("subagent 回读失败：{id}"))
}

/// 回填子 agent 执行会话 id（runner 建会话后调）。目标行不存在 → 响亮失败
/// （防 runner 拿过期 id 静默 no-op，OCR r1 采纳）。
pub fn set_subagent_session(
    conn: &rusqlite::Connection,
    id: &str,
    session_id: &str,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE subagents SET session_id=?2 WHERE id=?1",
            rusqlite::params![id, session_id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("subagent 不存在：{id}（session_id 回填未生效）"));
    }
    Ok(())
}

/// 回填收尾结果 JSON（§7 schema；部分结果不丢弃语义）。目标行不存在 → 响亮失败。
pub fn set_subagent_result(
    conn: &rusqlite::Connection,
    id: &str,
    result_json: &str,
) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE subagents SET result_json=?2 WHERE id=?1",
            rusqlite::params![id, result_json],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("subagent 不存在：{id}（result 回填未生效）"));
    }
    Ok(())
}

#[cfg(test)]
mod subagent_db_tests {
    use super::*;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 与 open_db 同源 DDL（单源真相，防测试 schema 与生产漂移）
        conn.execute_batch(SUBAGENTS_DDL).unwrap();
        conn
    }

    fn sample_row(id: &str, status: SubagentStatus) -> SubagentRow {
        SubagentRow {
            id: id.into(),
            status,
            profile: SubagentProfile::Research,
            model: None,
            parent_session_id: Some("parent-1".into()),
            session_id: None,
            task_id: format!("card-{id}"),
            objective: "调研五个竞品".into(),
            trace_id: format!("trc-{id}"),
            acceptance_json: Some(r#"["覆盖5个产品"]"#.into()),
            budget_json: Some(r#"{"maxTurns":30,"maxToolCalls":100,"maxWallSeconds":600}"#.into()),
            result_json: None,
            error: None,
            created_at: 1_000,
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn transition_table_covers_all_legal_moves() {
        use SubagentStatus::*;
        let legal = [
            (Queued, Running),
            (Queued, Cancelled),
            (Running, Succeeded),
            (Running, Failed),
            (Running, Cancelled),
            (Running, BudgetExceeded),
        ];
        for (from, to) in legal {
            assert!(
                transition_allowed(from, to),
                "{} → {} 应合法",
                from.as_str(),
                to.as_str()
            );
        }
    }

    #[test]
    fn transition_table_rejects_illegal_moves() {
        use SubagentStatus::*;
        let illegal = [
            (Queued, Succeeded),
            (Queued, Failed),
            (Queued, BudgetExceeded),
            (Queued, Queued),
            (Running, Queued),
            (Running, Running),
            (Succeeded, Failed),
            (Succeeded, Running),
            (Failed, Succeeded),
            (Cancelled, Running),
            (BudgetExceeded, Succeeded),
        ];
        for (from, to) in illegal {
            assert!(
                !transition_allowed(from, to),
                "{} → {} 应非法",
                from.as_str(),
                to.as_str()
            );
        }
    }

    #[test]
    fn insert_load_roundtrip_preserves_all_columns() {
        let conn = test_conn();
        let row = sample_row("sa_1", SubagentStatus::Queued);
        insert_subagent(&conn, &row).unwrap();
        let got = load_subagent(&conn, "sa_1").unwrap().unwrap();
        assert_eq!(got.id, "sa_1");
        assert_eq!(got.status, SubagentStatus::Queued);
        assert_eq!(got.profile, SubagentProfile::Research);
        assert_eq!(got.parent_session_id.as_deref(), Some("parent-1"));
        assert_eq!(got.task_id, "card-sa_1");
        assert_eq!(got.trace_id, "trc-sa_1");
        assert!(got.acceptance_json.is_some());
        assert!(got.budget_json.is_some());
        assert!(got.session_id.is_none());
        assert!(got.started_at.is_none());
    }

    #[test]
    fn load_missing_row_is_none_not_error() {
        let conn = test_conn();
        assert!(load_subagent(&conn, "sa_missing").unwrap().is_none());
    }

    #[test]
    fn find_by_task_returns_the_row() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_2", SubagentStatus::Running)).unwrap();
        let got = find_subagent_by_task(&conn, "card-sa_2").unwrap().unwrap();
        assert_eq!(got.id, "sa_2");
        assert!(find_subagent_by_task(&conn, "no-such-card")
            .unwrap()
            .is_none());
    }

    #[test]
    fn update_status_stamps_started_and_finished() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_3", SubagentStatus::Queued)).unwrap();
        let running =
            update_subagent_status(&conn, "sa_3", SubagentStatus::Running, 2_000, None).unwrap();
        assert_eq!(running.started_at, Some(2_000));
        assert!(running.finished_at.is_none());
        let done =
            update_subagent_status(&conn, "sa_3", SubagentStatus::Succeeded, 3_000, None).unwrap();
        assert_eq!(done.started_at, Some(2_000), "started_at 不被后续转移覆盖");
        assert_eq!(done.finished_at, Some(3_000));
        assert_eq!(done.status, SubagentStatus::Succeeded);
    }

    #[test]
    fn update_status_rejects_illegal_transition() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_4", SubagentStatus::Queued)).unwrap();
        let err = update_subagent_status(&conn, "sa_4", SubagentStatus::Succeeded, 2_000, None)
            .unwrap_err();
        assert!(err.contains("非法状态转移"), "实际错误：{err}");
    }

    #[test]
    fn update_status_writes_error_reason() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_5", SubagentStatus::Running)).unwrap();
        let _ = update_subagent_status(
            &conn,
            "sa_5",
            SubagentStatus::Cancelled,
            2_000,
            Some("用户取消"),
        )
        .unwrap();
        let got = load_subagent(&conn, "sa_5").unwrap().unwrap();
        assert_eq!(got.error.as_deref(), Some("用户取消"));
        assert_eq!(got.finished_at, Some(2_000));
    }

    #[test]
    fn update_missing_id_errors() {
        let conn = test_conn();
        assert!(update_subagent_status(&conn, "sa_x", SubagentStatus::Running, 1, None).is_err());
    }

    #[test]
    fn load_by_status_filters_and_orders() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_a", SubagentStatus::Queued)).unwrap();
        insert_subagent(&conn, &sample_row("sa_b", SubagentStatus::Running)).unwrap();
        let queued = load_subagents_by_status(&conn, SubagentStatus::Queued).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, "sa_a");
    }

    #[test]
    fn session_and_result_backfill() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_6", SubagentStatus::Running)).unwrap();
        set_subagent_session(&conn, "sa_6", "sess-42").unwrap();
        set_subagent_result(&conn, "sa_6", r#"{"summary":"ok"}"#).unwrap();
        let got = load_subagent(&conn, "sa_6").unwrap().unwrap();
        assert_eq!(got.session_id.as_deref(), Some("sess-42"));
        assert_eq!(got.result_json.as_deref(), Some(r#"{"summary":"ok"}"#));
    }

    /// OCR r1 采纳：回填目标行不存在必须响亮失败（防 runner 拿过期 id 静默 no-op）
    #[test]
    fn session_and_result_backfill_missing_id_errors() {
        let conn = test_conn();
        assert!(set_subagent_session(&conn, "sa_ghost", "sess-x").is_err());
        assert!(set_subagent_result(&conn, "sa_ghost", "{}").is_err());
    }

    /// OCR r1 采纳：同态重入保留原 finished_at（重试不改写首收尾时刻）
    #[test]
    fn same_state_terminal_reentry_preserves_finished_at() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_7", SubagentStatus::Running)).unwrap();
        update_subagent_status(&conn, "sa_7", SubagentStatus::Succeeded, 5_000, None).unwrap();
        let retry =
            update_subagent_status(&conn, "sa_7", SubagentStatus::Succeeded, 9_999, None).unwrap();
        assert_eq!(retry.finished_at, Some(5_000), "重试不得改写首收尾时刻");
    }

    /// OCR r1 采纳：终态自环在转移表上必须判非法（同态放行只走 update 的幂等短路）
    #[test]
    fn terminal_self_loops_are_not_legal_transitions() {
        use SubagentStatus::*;
        for s in [Succeeded, Failed, Cancelled, BudgetExceeded] {
            assert!(!transition_allowed(s, s), "{} 自环应非法", s.as_str());
        }
    }

    /// OCR r1 采纳：task_id UNIQUE 钉死一子 agent 一子卡（并发/重试双插必炸）
    #[test]
    fn duplicate_task_id_insert_is_rejected() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_a", SubagentStatus::Queued)).unwrap();
        let mut dup = sample_row("sa_b", SubagentStatus::Queued);
        dup.task_id = "card-sa_a".into();
        assert!(
            insert_subagent(&conn, &dup).is_err(),
            "同 task_id 第二次插入必须失败"
        );
    }

    /// OCR r1 采纳：serde 线协议对齐 DB as_str 小写 snake（CheckState.status 给前端）
    #[test]
    fn status_serde_wire_is_snake_case() {
        assert_eq!(
            serde_json::to_value(SubagentStatus::BudgetExceeded).unwrap(),
            "budget_exceeded"
        );
        assert_eq!(
            serde_json::to_value(SubagentStatus::Queued).unwrap(),
            "queued"
        );
    }
}
