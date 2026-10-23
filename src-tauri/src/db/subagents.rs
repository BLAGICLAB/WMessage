//! 子 agent 编排持久化
//!
//! 设计基准：docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md §3（存储）§6（预算默认值）。
//! 一行 = 一个受管子 agent：orchestrator spawn 时插入（status=queued），runner 推进
//! 状态机，check_subagent 幂等可轮询（事件丢失可查表、重启状态不丢）。
//!
//! 列对齐设计 §3，另加三列（增列不违设计，勘误已在批 spec 登记）：
//! - `session_id`：子 agent 自己的执行会话（串链围观/审计；parent_session_id 是派发者主会话）
//! - `trace_id`：贯穿 bot.log 的串链键
//! - `acceptance_json`：验收标准机器侧副本（子 agent 不再建任务卡，行是唯一契约载体）

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// 预算默认值与硬顶（设计 §6 已；钳制实现见 [SubagentBudget::clamped]）。
///
/// 默认值单源在 `bot::params`（DEFAULT_SUBAGENT_MAX_*），这里直接引用而不是
/// 各写一份字面量：两处曾同为 30/100/600，改一处漏一处就是静默漂移——
/// bot 读侧按 30 放行、行上按 100 记账这类偏差会一路掩到熔断才暴露。
pub const DEFAULT_MAX_TURNS: u32 = crate::bot::params::DEFAULT_SUBAGENT_MAX_TURNS;
pub const MAX_TURNS_HARD_CAP: u32 = 50;
/// 轮数下界（0 轮 = 子 agent 一句话都说不完，等于白派）
pub const MIN_SUBAGENT_TURNS: u32 = 1;
pub const DEFAULT_MAX_TOOL_CALLS: u32 = crate::bot::params::DEFAULT_SUBAGENT_MAX_TOOL_CALLS;
pub const DEFAULT_MAX_WALL_SECONDS: u64 = crate::bot::params::DEFAULT_SUBAGENT_MAX_WALL_SECS;

/// 子 agent 轮数钳制（1..=硬顶 50）——单一来源：config 落盘（bot_set_config）、
/// bot 读侧 resolver、预算持久化（[SubagentBudget::clamped]）三处共用，
/// 边界只认这里的 `MAX_TURNS_HARD_CAP`，防改一处漏两处。
pub fn clamp_subagent_max_turns(n: u32) -> u32 {
    n.clamp(MIN_SUBAGENT_TURNS, MAX_TURNS_HARD_CAP)
}

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

    // 有意不实现 std::str::FromStr：解析失败返回 Option 而非 Err
    #[allow(clippy::should_implement_trait)]
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

    // 有意不实现 std::str::FromStr：解析失败返回 Option 而非 Err
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "research" => SubagentProfile::Research,
            "coder" => SubagentProfile::Coder,
            "general" => SubagentProfile::General,
            _ => return None,
        })
    }
}

/// 预算三硬顶（设计 §6 默认值）。落行上 budget_json（check 可查），不是隐藏参数。
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
    /// 预算钳制：turns 硬顶 50、tool_calls 下限 1、wall 钳 30..=3600。
    /// **所有持久化写口（spawn / task_patch）必须过本函数**——字段是 pub 的，
    /// 构造侧不受限，钳制契约靠两个写口强制（防 task_patch 旁路）。
    ///
    /// 墙钟的上界原先只钳「≥1 秒」，于是**两条路两个闸**：配置键走
    /// `clamp_subagent_max_wall_secs`（30..=3600），LLM 在 `spawn_subagent` 里
    /// 显式给的 `maxWallSeconds` 只钳下限——墙钟的实际硬顶由调用方说了算。
    /// 现在两路同源：不管预算从配置、调用方还是默认值来，**在校验边界处钳一次**。
    pub fn clamped(mut self) -> Self {
        self.max_turns = clamp_subagent_max_turns(self.max_turns);
        self.max_tool_calls = self.max_tool_calls.max(1);
        self.max_wall_seconds =
            crate::bot::params::clamp_subagent_max_wall_secs(self.max_wall_seconds);
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
    /// 历史子任务卡 id：子 agent 不再建卡，新行为 NULL；老行保留原值供反查
    pub task_id: Option<String>,
    pub objective: String,
    pub trace_id: String,
    /// 验收标准 Vec<String> 的 JSON（任务包装与 check 进度小计的唯一来源）
    pub acceptance_json: Option<String>,
    /// 上下文摘要（设计 §8.5）：此前只落子卡 note，文本解析取回；无卡后独立成列
    pub context_summary: Option<String>,
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
/// ）。子 agent 不再建任务卡，故 task_id 可空（无卡行写 NULL，老行保留原值）；
/// status / parent_session_id 加索引（按状态扫队列、按主会话串链）。
pub(crate) const SUBAGENTS_DDL: &str = "CREATE TABLE IF NOT EXISTS subagents (
           id                TEXT PRIMARY KEY,
           status            TEXT NOT NULL,
           profile           TEXT NOT NULL,
           model             TEXT,
           parent_session_id TEXT,
           session_id        TEXT,
           task_id           TEXT,
           objective         TEXT NOT NULL,
           trace_id          TEXT NOT NULL,
           acceptance_json   TEXT,
           context_summary   TEXT,
           budget_json       TEXT,
           result_json       TEXT,
           error             TEXT,
           created_at        INTEGER NOT NULL,
           started_at        INTEGER,
           finished_at       INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_subagents_status ON subagents(status);
         CREATE INDEX IF NOT EXISTS idx_subagents_parent ON subagents(parent_session_id);
         CREATE INDEX IF NOT EXISTS idx_subagents_task ON subagents(task_id);";

/// 幂等迁移：task_id 从 `NOT NULL UNIQUE` 放宽为可空。
/// SQLite 不支持 ALTER COLUMN，只能重建表——数据全量搬运，老行 task_id 原样保留
/// （历史子 agent 仍可反查它当初那张卡是否还在），新行写 NULL。
/// 先查 pragma：已是可空就直接返回，避免每次启动都重建。
/// **前置条件**：context_summary 列已存在（重建的 INSERT..SELECT 搬运该列）；
/// open_db 里必须先跑 [ensure_context_summary_column]。
pub fn ensure_task_id_nullable(conn: &rusqlite::Connection) -> Result<(), String> {
    let notnull: i64 = conn
        .query_row(
            "SELECT \"notnull\" FROM pragma_table_info('subagents') WHERE name = 'task_id'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if notnull == 0 {
        return Ok(());
    }
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF;
         BEGIN;
         CREATE TABLE subagents_new (
           id                TEXT PRIMARY KEY,
           status            TEXT NOT NULL,
           profile           TEXT NOT NULL,
           model             TEXT,
           parent_session_id TEXT,
           session_id        TEXT,
           task_id           TEXT,
           objective         TEXT NOT NULL,
           trace_id          TEXT NOT NULL,
           acceptance_json   TEXT,
           context_summary   TEXT,
           budget_json       TEXT,
           result_json       TEXT,
           error             TEXT,
           created_at        INTEGER NOT NULL,
           started_at        INTEGER,
           finished_at       INTEGER
         );
         INSERT INTO subagents_new
           SELECT id, status, profile, model, parent_session_id, session_id, task_id,
                  objective, trace_id, acceptance_json, context_summary, budget_json,
                  result_json, error, created_at, started_at, finished_at
           FROM subagents;
         DROP TABLE subagents;
         ALTER TABLE subagents_new RENAME TO subagents;
         CREATE INDEX IF NOT EXISTS idx_subagents_status ON subagents(status);
         CREATE INDEX IF NOT EXISTS idx_subagents_parent ON subagents(parent_session_id);
         CREATE INDEX IF NOT EXISTS idx_subagents_task ON subagents(task_id);
         COMMIT;
         PRAGMA foreign_keys=ON;",
    )
    .map_err(|e| e.to_string())
}

/// 幂等迁移：补 context_summary 列。老库该列不存在时新增（SQLite 的 ADD COLUMN
/// 恒追加到列尾，故老行的 context_summary 为 NULL，与「摘要未回填」同形）。
/// 先查 pragma：列已在就直接返回，避免每次启动都跑一遍 ALTER。
pub fn ensure_context_summary_column(conn: &rusqlite::Connection) -> Result<(), String> {
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('subagents') WHERE name = 'context_summary'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists != 0 {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE subagents ADD COLUMN context_summary TEXT")
        .map_err(|e| e.to_string())
}

pub fn insert_subagent(conn: &rusqlite::Connection, r: &SubagentRow) -> Result<(), String> {
    conn.execute(
        "INSERT INTO subagents (id, status, profile, model, parent_session_id, session_id,
             task_id, objective, trace_id, acceptance_json, context_summary, budget_json,
             result_json, error, created_at, started_at, finished_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
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
            r.context_summary,
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

const SQL_SELECT_BY_ID: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
     trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE id = ?1";
const SQL_SELECT_BY_TASK: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
     trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE task_id = ?1 \
     ORDER BY created_at DESC LIMIT 1";
const SQL_SELECT_BY_SESSION: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
     trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE session_id = ?1 \
     ORDER BY created_at DESC LIMIT 1";
const SQL_SELECT_BY_STATUS: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
     trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE status = ?1 ORDER BY created_at";
/// 按派发者主会话取全部子 agent（新→旧）。聊天内嵌子 agent 区域与右侧栏
/// 「子 Agent」工作面的数据源：子 agent 不再建独立会话后，用户只能从这里
/// 看到自己派出去的那些还活着/刚跑完。
const SQL_SELECT_BY_PARENT: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
     trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE parent_session_id = ?1 \
     ORDER BY created_at DESC LIMIT ?2";

/// 按「同一份派发请求」找**还活着**的行（幂等去重用）。
///
/// 只认 queued / running：已终态的允许重派——「再跑一次」是重试语义，不能
/// 被去重吞掉。`model IS ?5` 用 IS 而不是 =，NULL 行（未指定模型）也要能匹配上。
const SQL_SELECT_LIVE_BY_REQUEST: &str =
    "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
     trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
     created_at, started_at, finished_at FROM subagents WHERE parent_session_id = ?1 \
     AND profile = ?2 AND objective = ?3 AND acceptance_json = ?4 AND model IS ?5 \
     AND status IN ('queued','running') ORDER BY created_at DESC LIMIT 1";

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
        context_summary: row.get(10)?,
        budget_json: row.get(11)?,
        result_json: row.get(12)?,
        error: row.get(13)?,
        created_at: row.get(14)?,
        started_at: row.get(15)?,
        finished_at: row.get(16)?,
    })
}

pub fn load_subagent(conn: &rusqlite::Connection, id: &str) -> Result<Option<SubagentRow>, String> {
    conn.query_row(SQL_SELECT_BY_ID, [id], row_to_subagent)
        .optional()
        .map_err(|e| e.to_string())
}

/// 按子任务卡 id 反查。子 agent 不再建卡，新行 task_id 为 NULL 查不到——
/// 该路径只服务有卡的历史行；`task_id = ?` 遇 NULL 永不为真，故无需特判。
pub fn find_subagent_by_task(
    conn: &rusqlite::Connection,
    task_id: &str,
) -> Result<Option<SubagentRow>, String> {
    conn.query_row(SQL_SELECT_BY_TASK, [task_id], row_to_subagent)
        .optional()
        .map_err(|e| e.to_string())
}

/// 按子 agent 执行会话 id 反查行（前端停止键只持有会话 id——
/// runner 的 set_subagent_session 回填后即可反查）。
pub fn find_subagent_by_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<SubagentRow>, String> {
    conn.query_row(SQL_SELECT_BY_SESSION, [session_id], row_to_subagent)
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

/// 按派发者主会话列子 agent（新→旧）。`limit` 防长会话一次拉全量历史。
pub fn load_subagents_by_parent(
    conn: &rusqlite::Connection,
    parent_session_id: &str,
    limit: u32,
) -> Result<Vec<SubagentRow>, String> {
    let mut stmt = conn
        .prepare(SQL_SELECT_BY_PARENT)
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params![parent_session_id, limit], row_to_subagent)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 按「同一份派发请求」找还活着的子 agent 行（spawn 幂等去重的查表口）。
///
/// 请求口径 = 父会话 + 档位 + 目标 + 验收标准 + 指定模型：全同即视为同一次
/// 派发。`acceptance_json` 与 `objective` 都取行里的原样字符串（写行时就是
/// 这么存的），比较是逐字节的——目标差一个字就不是同一次派发。
///
/// 只认活着的行（queued / running）：已终态的允许重派。
pub fn find_live_subagent_by_request(
    conn: &rusqlite::Connection,
    parent_session_id: &str,
    profile: &str,
    objective: &str,
    acceptance_json: &str,
    model: Option<&str>,
) -> Result<Option<SubagentRow>, String> {
    conn.query_row(
        SQL_SELECT_LIVE_BY_REQUEST,
        rusqlite::params![
            parent_session_id,
            profile,
            objective,
            acceptance_json,
            model
        ],
        row_to_subagent,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// 按会话（作为父会话 **或** 子 agent 自己的会话）取**在途**行。
///
/// 会话删除时用：父会话没了，它派出去的子 agent 就成了没人收的孤儿——看不见、
/// 停不掉，还照烧完整份预算。
pub fn load_live_subagents_by_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Vec<SubagentRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, status, profile, model, parent_session_id, session_id, task_id, objective, \
             trace_id, acceptance_json, context_summary, budget_json, result_json, error, \
             created_at, started_at, finished_at FROM subagents \
             WHERE (parent_session_id = ?1 OR session_id = ?1) \
             AND status IN ('queued','running') ORDER BY created_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], row_to_subagent)
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
    // 空串 reason 视同 None——防 Some("") 经 COALESCE 覆盖既有失败原因
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
    // 同态重入保留原 finished_at——重试不改写首收尾时刻，审计时长口径稳定
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
/// （防 runner 拿过期 id 静默 no-op。
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
            task_id: None,
            objective: "调研五个竞品".into(),
            trace_id: format!("trc-{id}"),
            acceptance_json: Some(r#"["覆盖5个产品"]"#.into()),
            context_summary: None,
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
        // 无卡行 task_id 落 NULL（老行仍保留原值，由迁移保证）
        assert!(got.task_id.is_none());
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
        // 有卡的是历史行：新行 task_id 为 NULL，该路径只服务它们
        let mut legacy = sample_row("sa_2", SubagentStatus::Running);
        legacy.task_id = Some("card-sa_2".into());
        insert_subagent(&conn, &legacy).unwrap();
        let got = find_subagent_by_task(&conn, "card-sa_2").unwrap().unwrap();
        assert_eq!(got.id, "sa_2");
        assert!(find_subagent_by_task(&conn, "no-such-card")
            .unwrap()
            .is_none());
        // 无卡行按 NULL 天然查不到（SQL 里 `task_id = ?` 对 NULL 永不为真），
        // 也不会污染老行的反查结果
        insert_subagent(&conn, &sample_row("sa_2b", SubagentStatus::Running)).unwrap();
        assert_eq!(
            find_subagent_by_task(&conn, "card-sa_2")
                .unwrap()
                .unwrap()
                .id,
            "sa_2"
        );
        assert_eq!(
            load_subagent(&conn, "sa_2b").unwrap().unwrap().task_id,
            None
        );
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

    /// 回填目标行不存在必须响亮失败（防 runner 拿过期 id 静默 no-op）
    #[test]
    fn session_and_result_backfill_missing_id_errors() {
        let conn = test_conn();
        assert!(set_subagent_session(&conn, "sa_ghost", "sess-x").is_err());
        assert!(set_subagent_result(&conn, "sa_ghost", "{}").is_err());
    }

    /// 同态重入保留原 finished_at（重试不改写首收尾时刻）
    #[test]
    fn same_state_terminal_reentry_preserves_finished_at() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_7", SubagentStatus::Running)).unwrap();
        update_subagent_status(&conn, "sa_7", SubagentStatus::Succeeded, 5_000, None).unwrap();
        let retry =
            update_subagent_status(&conn, "sa_7", SubagentStatus::Succeeded, 9_999, None).unwrap();
        assert_eq!(retry.finished_at, Some(5_000), "重试不得改写首收尾时刻");
    }

    /// 终态自环在转移表上必须判非法（同态放行只走 update 的幂等短路）
    #[test]
    fn terminal_self_loops_are_not_legal_transitions() {
        use SubagentStatus::*;
        for s in [Succeeded, Failed, Cancelled, BudgetExceeded] {
            assert!(!transition_allowed(s, s), "{} 自环应非法", s.as_str());
        }
    }

    /// 无卡行 task_id 全为 NULL：多子 agent 并存不得因撞键互斥（旧 DDL 的
    /// UNIQUE 约束已随「不再建卡」移除，这是新行为而非回归）
    #[test]
    fn multiple_cardless_rows_coexist_without_unique_conflict() {
        let conn = test_conn();
        insert_subagent(&conn, &sample_row("sa_a", SubagentStatus::Queued)).unwrap();
        insert_subagent(&conn, &sample_row("sa_b", SubagentStatus::Queued)).unwrap();
        assert!(insert_subagent(&conn, &sample_row("sa_c", SubagentStatus::Queued)).is_ok());
        let by_parent = load_subagents_by_parent(&conn, "parent-1", 10).unwrap();
        assert_eq!(by_parent.len(), 3);
    }

    /// 老库（有卡、task_id NOT NULL UNIQUE）升级后：行数据全留，task_id 可空，
    /// 新行能写 NULL
    #[test]
    fn migration_relaxes_task_id_and_keeps_legacy_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE subagents (
               id TEXT PRIMARY KEY, status TEXT NOT NULL, profile TEXT NOT NULL, model TEXT,
               parent_session_id TEXT, session_id TEXT, task_id TEXT NOT NULL UNIQUE,
               objective TEXT NOT NULL, trace_id TEXT NOT NULL, acceptance_json TEXT,
               budget_json TEXT, result_json TEXT, error TEXT,
               created_at INTEGER NOT NULL, started_at INTEGER, finished_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO subagents (id, status, profile, task_id, objective, trace_id, created_at)
             VALUES ('sa_old','queued','research','card-old','老任务','trc-old',1)",
            [],
        )
        .unwrap();
        ensure_context_summary_column(&conn).unwrap();
        ensure_task_id_nullable(&conn).unwrap();
        let legacy = load_subagent(&conn, "sa_old").unwrap().unwrap();
        assert_eq!(legacy.task_id.as_deref(), Some("card-old"));
        assert_eq!(
            find_subagent_by_task(&conn, "card-old")
                .unwrap()
                .unwrap()
                .id,
            "sa_old"
        );
        insert_subagent(&conn, &sample_row("sa_new", SubagentStatus::Queued)).unwrap();
        assert!(load_subagent(&conn, "sa_new")
            .unwrap()
            .unwrap()
            .task_id
            .is_none());
        // 幂等：再跑一次迁移不得炸也不得丢数据
        ensure_task_id_nullable(&conn).unwrap();
        assert_eq!(
            load_subagent(&conn, "sa_old").unwrap().unwrap().id,
            "sa_old"
        );
    }

    #[test]
    fn migration_adds_context_summary_column_idempotently() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE subagents (
               id TEXT PRIMARY KEY, status TEXT NOT NULL, profile TEXT NOT NULL, model TEXT,
               parent_session_id TEXT, session_id TEXT, task_id TEXT,
               objective TEXT NOT NULL, trace_id TEXT NOT NULL, acceptance_json TEXT,
               budget_json TEXT, result_json TEXT, error TEXT,
               created_at INTEGER NOT NULL, started_at INTEGER, finished_at INTEGER);",
        )
        .unwrap();
        ensure_context_summary_column(&conn).unwrap();
        ensure_context_summary_column(&conn).unwrap();
        let row = SubagentRow {
            context_summary: Some("只给必要背景".into()),
            ..sample_row("sa_ctx", SubagentStatus::Queued)
        };
        insert_subagent(&conn, &row).unwrap();
        assert_eq!(
            load_subagent(&conn, "sa_ctx")
                .unwrap()
                .unwrap()
                .context_summary,
            Some("只给必要背景".into())
        );
    }

    /// serde 线协议对齐 DB as_str 小写 snake（CheckState.status 给前端）
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
