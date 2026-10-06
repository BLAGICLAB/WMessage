//! 执行痕迹三表（Agent 透明化改造设计 §4.1，docs/AGENT-TRANSPARENCY-DESIGN-2026-10-06.md）：
//! 一次执行 = 一条 `exec_traces`（run_task_in_chat / 主聊天 / 技能 run），
//! 每次工具调用 = 一条 `exec_spans`，每次文件落盘修改 = 一条 `file_changes`。
//!
//! 定位：**append-only 观测面**——只增与收尾，不参与任何业务判定；写路径全部
//! 经 `open_db` 幂等建表（`EXEC_TRACE_DDL`），采集侧（P1-b/c/d）按 span/change
//! 逐条 insert，收尾由 `trace_finish` 一次 UPDATE 汇总。
//!
//! 钳制纪律（与审计 KV 500 字符同思路：本地库也防膨胀）：
//! - `SPAN_TEXT_MAX`：args/result 单条 16KB（生成侧钳，本模块 `clamp_text` 单源）
//! - `MAX_DIFF_LINES`：unified diff 行数上限（P1-b 生成侧引用）
//! - `TRACE_RETENTION_DAYS`：保留期默认 30 天（`retire_traces_before` 由命令/定时触发）

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::error::{CommandError, CommandResult};

/// args/result 单条文本钳制（16KB）
pub const SPAN_TEXT_MAX: usize = 16 * 1024;
/// unified diff 行数钳制（P1-b 生成侧引用）
pub const MAX_DIFF_LINES: usize = 2000;
/// 执行痕迹保留期（天）；对齐 Claude Code checkpointing 的 cleanupPeriodDays 默认
pub const TRACE_RETENTION_DAYS: i64 = 30;

/// trace 收尾状态值域（status 列；running 为起始态，其余四态只能由 trace_finish 写入）
pub const TRACE_STATUS_RUNNING: &str = "running";
pub const TRACE_STATUS_DONE: &str = "done";
pub const TRACE_STATUS_FAILED: &str = "failed";
pub const TRACE_STATUS_STOPPED: &str = "stopped";
pub const TRACE_STATUS_TIMEOUT: &str = "timeout";

pub const EXEC_TRACE_DDL: &str = "
CREATE TABLE IF NOT EXISTS exec_traces (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id        TEXT    NOT NULL,
  task_id           TEXT,
  origin            TEXT    NOT NULL,
  title             TEXT,
  status            TEXT    NOT NULL DEFAULT 'running',
  started_at        INTEGER NOT NULL,
  finished_at       INTEGER,
  turn_count        INTEGER NOT NULL DEFAULT 0,
  tool_calls        INTEGER NOT NULL DEFAULT 0,
  files_changed     INTEGER NOT NULL DEFAULT 0,
  prompt_tokens     INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  error             TEXT,
  model             TEXT
);
CREATE INDEX IF NOT EXISTS idx_traces_task     ON exec_traces(task_id);
CREATE INDEX IF NOT EXISTS idx_traces_session  ON exec_traces(session_id);
-- 保留期清理主扫描列（retire_traces_before）
CREATE INDEX IF NOT EXISTS idx_traces_finished ON exec_traces(finished_at);

CREATE TABLE IF NOT EXISTS exec_spans (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  trace_id     INTEGER NOT NULL,
  turn         INTEGER NOT NULL,
  tool_call_id TEXT,
  name         TEXT    NOT NULL,
  args         TEXT,
  result       TEXT,
  ok           INTEGER NOT NULL DEFAULT 1,
  error_class  TEXT,
  duration_ms  INTEGER,
  created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_spans_trace ON exec_spans(trace_id);

CREATE TABLE IF NOT EXISTS file_changes (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  trace_id   INTEGER NOT NULL,
  span_id    INTEGER,
  path       TEXT    NOT NULL,
  kind       TEXT    NOT NULL,
  added      INTEGER NOT NULL DEFAULT 0,
  deleted    INTEGER NOT NULL DEFAULT 0,
  diff       TEXT,
  truncated  INTEGER NOT NULL DEFAULT 0,
  before_ref TEXT,
  before_sha TEXT,
  after_sha  TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_changes_trace ON file_changes(trace_id);";

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceRow {
    pub id: i64,
    pub session_id: String,
    pub task_id: Option<String>,
    pub origin: String,
    pub title: Option<String>,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub turn_count: i64,
    pub tool_calls: i64,
    pub files_changed: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub error: Option<String>,
    /// 本轮实际模型名（trace_finish 时写入；旧行/未发请求为 NULL）
    pub model: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpanRow {
    pub id: i64,
    pub trace_id: i64,
    pub turn: i64,
    pub tool_call_id: Option<String>,
    pub name: String,
    pub args: Option<String>,
    pub result: Option<String>,
    pub ok: bool,
    pub error_class: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileChangeRow {
    pub id: i64,
    pub trace_id: i64,
    pub span_id: Option<i64>,
    pub path: String,
    pub kind: String,
    pub added: i64,
    pub deleted: i64,
    pub diff: Option<String>,
    pub truncated: bool,
    pub before_ref: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    pub created_at: i64,
}

/// trace_start 入参（started_at 由采集侧传——收尾要算耗时，别在本层偷换 now）
pub struct NewTrace<'a> {
    pub session_id: &'a str,
    pub task_id: Option<&'a str>,
    pub origin: &'a str,
    pub title: Option<&'a str>,
    pub started_at: i64,
}

/// span 落盘入参；args/result 由调用方经 `clamp_text` 钳后传入（钳没钳本层不拦，
/// 超限审计 `trace.span_overflow` 在采集侧——DB 层只保底 `span_insert` 不再钳二次）。
pub struct NewSpan<'a> {
    pub trace_id: i64,
    pub turn: i64,
    pub tool_call_id: Option<&'a str>,
    pub name: &'a str,
    pub args: Option<&'a str>,
    pub result: Option<&'a str>,
    pub ok: bool,
    pub error_class: Option<&'a str>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}

/// file_change 落盘入参（P1-b 由 FileChangeReceipt 映射而来）
pub struct NewFileChange<'a> {
    pub trace_id: i64,
    pub span_id: Option<i64>,
    pub path: &'a str,
    /// create / modify / delete（值域由生成侧保证）
    pub kind: &'a str,
    pub added: i64,
    pub deleted: i64,
    pub diff: Option<&'a str>,
    pub truncated: bool,
    /// before 全文快照文件名（uuid 发号，非 DB row id——row id 落库前不可知；
    /// P2 回滚按此名取 `data_dir/checkpoints/<before_ref>`。设计 §9.2-3 据实现修正）
    pub before_ref: Option<&'a str>,
    pub before_sha: Option<&'a str>,
    pub after_sha: Option<&'a str>,
    pub created_at: i64,
}

/// trace_finish 入参；files_changed 不由调用方传——以 file_changes 表计数为准（单一事实源）。
/// model 可空：None 保留原值（COALESCE），失败在发请求前时行内维持 NULL。
pub struct TraceFinish<'a> {
    pub status: &'a str,
    pub finished_at: i64,
    pub turn_count: i64,
    pub tool_calls: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub error: Option<&'a str>,
    pub model: Option<&'a str>,
}

/// 文本按字节钳制，UTF-8 字符边界回退（多字节中文不能拦腰截断）。
/// 生成侧统一走本函数，保证「钳后必是合法 String」只有一份实现。
pub fn clamp_text(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn trace_row_from(r: &rusqlite::Row) -> rusqlite::Result<TraceRow> {
    Ok(TraceRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        task_id: r.get(2)?,
        origin: r.get(3)?,
        title: r.get(4)?,
        status: r.get(5)?,
        started_at: r.get(6)?,
        finished_at: r.get(7)?,
        turn_count: r.get(8)?,
        tool_calls: r.get(9)?,
        files_changed: r.get(10)?,
        prompt_tokens: r.get(11)?,
        completion_tokens: r.get(12)?,
        error: r.get(13)?,
        model: r.get(14)?,
    })
}

const TRACE_COLS: &str =
    "id, session_id, task_id, origin, title, status, started_at, finished_at, \
     turn_count, tool_calls, files_changed, prompt_tokens, completion_tokens, error, model";

/// 幂等建三表（open_db 接线；测试直接 execute_batch EXEC_TRACE_DDL 同款）。
/// 旧库迁移：model 列（2026-10 词元统计按模型聚合）用 pragma 探测后 ALTER 补齐。
pub fn ensure_trace_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(EXEC_TRACE_DDL)
        .map_err(|e| e.to_string())?;
    let has_model: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('exec_traces') WHERE name = 'model'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if has_model == 0 {
        conn.execute_batch("ALTER TABLE exec_traces ADD COLUMN model TEXT")
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn trace_start(conn: &rusqlite::Connection, t: &NewTrace) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO exec_traces (session_id, task_id, origin, title, status, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            t.session_id,
            t.task_id,
            t.origin,
            t.title,
            TRACE_STATUS_RUNNING,
            t.started_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// 收尾一次 UPDATE 汇总；files_changed 从 file_changes 表反计（防两头记账漂移）。
/// 幂等性：重复 finish 只是覆盖同值（调用方保证只收一次），running 行误收不拦——观测面不设闸。
pub fn trace_finish(
    conn: &rusqlite::Connection,
    trace_id: i64,
    f: &TraceFinish,
) -> Result<(), String> {
    conn.execute(
        "UPDATE exec_traces SET
           status = ?2, finished_at = ?3,
           turn_count = ?4, tool_calls = ?5,
           prompt_tokens = ?6, completion_tokens = ?7,
           error = ?8,
           model = COALESCE(?9, model),
           files_changed = (SELECT COUNT(*) FROM file_changes WHERE trace_id = ?1)
         WHERE id = ?1",
        rusqlite::params![
            trace_id,
            f.status,
            f.finished_at,
            f.turn_count,
            f.tool_calls,
            f.prompt_tokens,
            f.completion_tokens,
            f.error,
            f.model,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn span_insert(conn: &rusqlite::Connection, s: &NewSpan) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO exec_spans
           (trace_id, turn, tool_call_id, name, args, result, ok, error_class, duration_ms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            s.trace_id,
            s.turn,
            s.tool_call_id,
            s.name,
            s.args,
            s.result,
            s.ok as i64,
            s.error_class,
            s.duration_ms,
            s.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

pub fn file_change_insert(conn: &rusqlite::Connection, c: &NewFileChange) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO file_changes
           (trace_id, span_id, path, kind, added, deleted, diff, truncated, before_ref, before_sha, after_sha, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            c.trace_id,
            c.span_id,
            c.path,
            c.kind,
            c.added,
            c.deleted,
            c.diff,
            c.truncated as i64,
            c.before_ref,
            c.before_sha,
            c.after_sha,
            c.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// 摘要列表（不含 span 正文）：可选过滤 task/session/origin，started_at 倒序。
/// limit 钳 1..=500（防止前端一拉全表）。
pub fn trace_query(
    conn: &rusqlite::Connection,
    task_id: Option<&str>,
    session_id: Option<&str>,
    origin: Option<&str>,
    limit: i64,
) -> Result<Vec<TraceRow>, String> {
    let mut sql = format!("SELECT {TRACE_COLS} FROM exec_traces");
    let mut vals: Vec<String> = Vec::new();
    let mut conds: Vec<String> = Vec::new();
    if let Some(t) = task_id {
        conds.push("task_id = ?".into());
        vals.push(t.to_string());
    }
    if let Some(s) = session_id {
        conds.push("session_id = ?".into());
        vals.push(s.to_string());
    }
    if let Some(o) = origin {
        conds.push("origin = ?".into());
        vals.push(o.to_string());
    }
    if !conds.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conds.join(" AND "));
    }
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT ?");
    vals.push(limit.clamp(1, 500).to_string());

    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(vals.iter()), trace_row_from)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn trace_get_row(
    conn: &rusqlite::Connection,
    trace_id: i64,
) -> Result<Option<TraceRow>, String> {
    let sql = format!("SELECT {TRACE_COLS} FROM exec_traces WHERE id = ?1");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query_map([trace_id], trace_row_from)
        .map_err(|e| e.to_string())?;
    match rows.next() {
        Some(r) => Ok(Some(r.map_err(|e| e.to_string())?)),
        None => Ok(None),
    }
}

pub fn spans_for_trace(conn: &rusqlite::Connection, trace_id: i64) -> Result<Vec<SpanRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, trace_id, turn, tool_call_id, name, args, result, ok, error_class, duration_ms, created_at
             FROM exec_spans WHERE trace_id = ?1 ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([trace_id], |r| {
            Ok(SpanRow {
                id: r.get(0)?,
                trace_id: r.get(1)?,
                turn: r.get(2)?,
                tool_call_id: r.get(3)?,
                name: r.get(4)?,
                args: r.get(5)?,
                result: r.get(6)?,
                ok: r.get::<_, i64>(7)? != 0,
                error_class: r.get(8)?,
                duration_ms: r.get(9)?,
                created_at: r.get(10)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn file_changes_for_trace(
    conn: &rusqlite::Connection,
    trace_id: i64,
) -> Result<Vec<FileChangeRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, trace_id, span_id, path, kind, added, deleted, diff, truncated, before_ref, before_sha, after_sha, created_at
             FROM file_changes WHERE trace_id = ?1 ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([trace_id], file_change_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 单条文件变更定点读（file_rollback 入口用）
pub fn file_change_get(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<FileChangeRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, trace_id, span_id, path, kind, added, deleted, diff, truncated, before_ref, before_sha, after_sha, created_at
             FROM file_changes WHERE id = ?1",
        )
        .map_err(|e| e.to_string())?;
    stmt.query_row([id], file_change_from_row)
        .optional()
        .map_err(|e| e.to_string())
}

fn file_change_from_row(r: &rusqlite::Row) -> rusqlite::Result<FileChangeRow> {
    Ok(FileChangeRow {
        id: r.get(0)?,
        trace_id: r.get(1)?,
        span_id: r.get(2)?,
        path: r.get(3)?,
        kind: r.get(4)?,
        added: r.get(5)?,
        deleted: r.get(6)?,
        diff: r.get(7)?,
        truncated: r.get::<_, i64>(8)? != 0,
        before_ref: r.get(9)?,
        before_sha: r.get(10)?,
        after_sha: r.get(11)?,
        created_at: r.get(12)?,
    })
}

/// 保留期清理：删除「已收尾且 finished_at 早于 before_ms」或「一直挂着 running 且
/// started_at 早于 before_ms」（崩溃残留的僵尸 trace 永远等不到收尾）。
/// 子表行按 trace_id 显式删除（无外键，事务内三删保原子）。返回删除的 trace 条数。
pub fn retire_traces_before(conn: &rusqlite::Connection, before_ms: i64) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let ids: Vec<i64> = {
        let mut stmt = tx
            .prepare(
                "SELECT id FROM exec_traces
                 WHERE (finished_at IS NOT NULL AND finished_at < ?1)
                    OR (finished_at IS NULL AND started_at < ?1)",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([before_ms], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        out
    };
    for id in &ids {
        tx.execute("DELETE FROM exec_spans WHERE trace_id = ?1", [id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM file_changes WHERE trace_id = ?1", [id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM exec_traces WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ids.len())
}

// ───────────────────────── 前端查询命令（P1-d） ─────────────────────────

/// 单次执行的完整痕迹（trace 摘要 + 全部 span + 全部文件变更；diff 已按 MAX_DIFF_LINES 钳）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TraceDetail {
    #[serde(flatten)]
    pub trace: TraceRow,
    pub spans: Vec<SpanRow>,
    pub file_changes: Vec<FileChangeRow>,
}

/// 词元统计单日聚合（设置页「词元统计」卡数据源，P3 实装消费）。
/// 升序返回、窗口内缺日补零（趋势图/热力图 x 轴必须连续）；
/// 执行次数只计已收尾行（running 僵尸不算一次执行，token 仍计入）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageDay {
    /// 本地日期 YYYY-MM-DD
    pub day: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub runs: i64,
    pub tool_calls: i64,
}

/// 聚合核心（conn + today 注入，测试可定时）：窗口 = today 往前 days 天（含两端）。
pub fn usage_stats_daily_conn(
    conn: &rusqlite::Connection,
    days: i64,
    today_local: chrono::NaiveDate,
) -> Result<Vec<UsageDay>, String> {
    let start_date = today_local - chrono::Duration::days(days - 1);
    let since_ms = start_date
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS day,
                    SUM(prompt_tokens), SUM(completion_tokens),
                    SUM(CASE WHEN finished_at IS NOT NULL THEN 1 ELSE 0 END),
                    SUM(tool_calls)
             FROM exec_traces WHERE started_at >= ?1
             GROUP BY day",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut by_day: std::collections::BTreeMap<String, [i64; 4]> =
        std::collections::BTreeMap::new();
    for r in rows {
        let (day, p, c, runs, tools) = r.map_err(|e| e.to_string())?;
        by_day.insert(day, [p, c, runs, tools]);
    }
    let mut out = Vec::with_capacity(days as usize);
    let mut d = start_date;
    while d <= today_local {
        let key = d.format("%Y-%m-%d").to_string();
        let v = by_day.remove(&key).unwrap_or([0; 4]);
        out.push(UsageDay {
            day: key,
            prompt_tokens: v[0],
            completion_tokens: v[1],
            runs: v[2],
            tool_calls: v[3],
        });
        d += chrono::Duration::days(1);
    }
    Ok(out)
}

/// 词元统计按模型聚合（「模型用量」榜数据源）：只计已收尾行（与 daily 口径一致），
/// 按总 tokens 降序；旧行/未发请求 model 为 NULL，由前端显示「未知模型」。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageByModel {
    pub model: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub runs: i64,
}

/// 聚合核心（conn + today 注入，测试可定时）
pub fn usage_stats_by_model_conn(
    conn: &rusqlite::Connection,
    days: i64,
    today_local: chrono::NaiveDate,
) -> Result<Vec<UsageByModel>, String> {
    let start_date = today_local - chrono::Duration::days(days - 1);
    let since_ms = start_date
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "SELECT model, SUM(prompt_tokens), SUM(completion_tokens),
                    SUM(CASE WHEN finished_at IS NOT NULL THEN 1 ELSE 0 END)
             FROM exec_traces WHERE started_at >= ?1
             GROUP BY model
             ORDER BY SUM(prompt_tokens + completion_tokens) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since_ms], |r| {
            Ok(UsageByModel {
                model: r.get(0)?,
                prompt_tokens: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                completion_tokens: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                runs: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// 词元统计按模型×按日聚合（「每日 Token 趋势图」按模型分线数据源）：
/// 只返回有数据的 (day, model) 组合（缺日由前端以 0 补齐），day 升序。
/// 只计已收尾行口径与 daily/by_model 一致；model NULL 保留（前端显示「未知模型」）。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageDayModel {
    /// 本地日期 YYYY-MM-DD
    pub day: String,
    pub model: Option<String>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// 聚合核心（conn + today 注入，测试可定时）
pub fn usage_stats_daily_by_model_conn(
    conn: &rusqlite::Connection,
    days: i64,
    today_local: chrono::NaiveDate,
) -> Result<Vec<UsageDayModel>, String> {
    let start_date = today_local - chrono::Duration::days(days - 1);
    let since_ms = start_date
        .and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .map(|t| t.timestamp_millis())
        .unwrap_or(0);
    let mut stmt = conn
        .prepare(
            "SELECT date(started_at / 1000, 'unixepoch', 'localtime') AS day, model,
                    SUM(prompt_tokens), SUM(completion_tokens)
             FROM exec_traces WHERE started_at >= ?1
             GROUP BY day, model ORDER BY day ASC, model ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([since_ms], |r| {
            Ok(UsageDayModel {
                day: r.get(0)?,
                model: r.get(1)?,
                prompt_tokens: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                completion_tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[tauri::command]
pub fn usage_stats_daily_by_model(
    app: tauri::AppHandle,
    days: Option<i64>,
) -> CommandResult<Vec<UsageDayModel>> {
    let conn = super::open_db(&app)?;
    let days = days.unwrap_or(30).clamp(1, 365);
    Ok(usage_stats_daily_by_model_conn(
        &conn,
        days,
        chrono::Local::now().date_naive(),
    )?)
}

#[tauri::command]
pub fn trace_list(
    app: tauri::AppHandle,
    task_id: Option<String>,
    session_id: Option<String>,
    origin: Option<String>,
    limit: Option<i64>,
) -> CommandResult<Vec<TraceRow>> {
    let conn = super::open_db(&app)?;
    Ok(trace_query(
        &conn,
        task_id.as_deref(),
        session_id.as_deref(),
        origin.as_deref(),
        limit.unwrap_or(50),
    )?)
}

#[tauri::command]
pub fn trace_detail(app: tauri::AppHandle, trace_id: i64) -> CommandResult<Option<TraceDetail>> {
    let conn = super::open_db(&app)?;
    let Some(trace) = trace_get_row(&conn, trace_id)? else {
        return Ok(None);
    };
    let spans = spans_for_trace(&conn, trace_id)?;
    let file_changes = file_changes_for_trace(&conn, trace_id)?;
    Ok(Some(TraceDetail {
        trace,
        spans,
        file_changes,
    }))
}

/// 保留期清理（数据管理入口，后续可挂定时器）：默认 TRACE_RETENTION_DAYS 天，钳 1..=365
#[tauri::command]
pub fn trace_clear_before(app: tauri::AppHandle, days: Option<i64>) -> CommandResult<usize> {
    let conn = super::open_db(&app)?;
    let days = days.unwrap_or(TRACE_RETENTION_DAYS).clamp(1, 365);
    let before = chrono::Utc::now().timestamp_millis() - days * 86_400_000;
    let removed = retire_traces_before(&conn, before)?;
    if removed > 0 {
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "trace.retired",
            &[("removed", removed.to_string()), ("days", days.to_string())],
        );
    }
    Ok(removed)
}

#[tauri::command]
pub fn usage_stats_daily(app: tauri::AppHandle, days: Option<i64>) -> CommandResult<Vec<UsageDay>> {
    let conn = super::open_db(&app)?;
    let days = days.unwrap_or(30).clamp(1, 365);
    Ok(usage_stats_daily_conn(
        &conn,
        days,
        chrono::Local::now().date_naive(),
    )?)
}

#[tauri::command]
pub fn usage_stats_by_model(
    app: tauri::AppHandle,
    days: Option<i64>,
) -> CommandResult<Vec<UsageByModel>> {
    let conn = super::open_db(&app)?;
    let days = days.unwrap_or(30).clamp(1, 365);
    Ok(usage_stats_by_model_conn(
        &conn,
        days,
        chrono::Local::now().date_naive(),
    )?)
}

/// 文件级回滚（P2-a，设计 §9.2-3）：把一次 AI 修改恢复到修改前快照。
/// 双闸防吞改：
/// 1. **漂移闸**——当前文件内容 sha 必须等于本变更落盘时的 after_sha，文件被
///    （人/其他流程）动过即拒绝（覆盖会吞掉后续修改）；
/// 2. **快照闸**——快照内容 sha 必须等于 before_sha（快照文件损坏/被换即拒绝）。
/// 仅支持 modify：create 的撤销=删文件，属危险动作待立项（界面隐藏按钮兜底）。
#[tauri::command]
pub fn file_rollback(app: tauri::AppHandle, change_id: i64) -> CommandResult<String> {
    let conn = super::open_db(&app)?;
    let Some(c) = file_change_get(&conn, change_id)? else {
        return Err(CommandError::Internal(format!(
            "变更记录 {change_id} 不存在（可能已被保留期清理）"
        )));
    };
    if c.kind != "modify" {
        return Err(CommandError::Internal(format!(
            "暂不支持 {} 类型的回滚（仅 modify；新建文件的撤销=删文件，属危险动作待立项）",
            c.kind
        )));
    }
    let Some(before_ref) = c.before_ref.clone() else {
        return Err(CommandError::Internal(
            "该变更无 before 快照（写盘时快照失败降级），无法回滚".into(),
        ));
    };
    // 快照名由写入侧 uuid 发号（bot_fs）；带目录分隔符/父目录引用/绝对路径的
    // 一律视为被篡改的 DB 行，拒绝——防快照读取越界到任意文件
    let snap_path = std::path::Path::new(&before_ref);
    if before_ref.is_empty() || snap_path.file_name() != Some(snap_path.as_os_str()) {
        return Err(CommandError::Internal(format!(
            "快照名非法（应为纯文件名），拒绝回滚：{before_ref}"
        )));
    }
    // 纵深防御：c.path 同样来自 DB 行，不直接信。回滚是覆盖写，目标若是
    // 符号链接会顺着链接打到任意文件，先拒绝；再给全文读取设硬上限防 OOM。
    // （残余窗口：检查与读写之间目标被换成符号链接，单机桌面场景可接受。）
    // 与用户白名单工作区的归一化比对涉及 bot_fs 策略口径，另行走闸口方案。
    let path = std::path::PathBuf::from(&c.path);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| CommandError::Internal(format!("目标文件已不存在：{}", c.path)))?;
    if meta.file_type().is_symlink() {
        return Err(CommandError::Internal(format!(
            "目标是符号链接，拒绝回滚（防止写到链接指向的路径）：{}",
            c.path
        )));
    }
    if !meta.is_file() {
        return Err(CommandError::Internal(format!(
            "目标文件已不存在：{}",
            c.path
        )));
    }
    // 与 SPAN_TEXT_MAX 同思路：文本快照读写给个硬上限，防被指向超大文件拖爆内存
    const FILE_ROLLBACK_MAX_BYTES: u64 = 64 * 1024 * 1024;
    if meta.len() > FILE_ROLLBACK_MAX_BYTES {
        return Err(CommandError::Internal(format!(
            "目标文件过大（{} 字节 > 上限 {FILE_ROLLBACK_MAX_BYTES}），拒绝回滚；请人工核对：{}",
            meta.len(),
            c.path
        )));
    }
    let current = std::fs::read_to_string(&path)
        .map_err(|e| CommandError::Internal(format!("当前文件不可读（{}）：{e}", c.path)))?;
    if c.after_sha
        .as_deref()
        .is_none_or(|a| a != crate::bot_fs::sha256_hex(&current))
    {
        return Err(CommandError::Internal(format!(
            "文件自本次修改后已被改动（指纹不符），拒绝回滚以免吞掉后续修改；请人工核对：{}",
            c.path
        )));
    }
    let snapshot = crate::db::paths::data_dir(&app)
        .join("checkpoints")
        .join(&before_ref);
    let before = std::fs::read_to_string(&snapshot)
        .map_err(|e| CommandError::Internal(format!("快照不可读（{before_ref}）：{e}")))?;
    if c.before_sha
        .as_deref()
        .is_none_or(|b| b != crate::bot_fs::sha256_hex(&before))
    {
        return Err(CommandError::Internal(
            "快照校验不符（before_sha），拒绝回滚".into(),
        ));
    }
    crate::db::atomic_write(&path, &before).map_err(CommandError::Internal)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Warn,
        "file.rollback",
        &[
            ("change_id", change_id.to_string()),
            ("path", crate::bot::truncate_for_log(&c.path, 200)),
        ],
    );
    Ok(format!("已回滚 {}（恢复到本次修改前）", c.path))
}

/// P4：单次执行痕迹导出 JSONL（Codex rollout 本地文件化思想）——
/// 第 1 行 trace 摘要，随后 span 行（`{"type":"span",...}`）与文件变更行
/// （`{"type":"file_change",...}`）按 id 序混排。落 `data_dir/exports/`，
/// 返回绝对路径供前端打开/分享。只读导出，不改动痕迹数据。
#[tauri::command]
pub fn trace_export(app: tauri::AppHandle, trace_id: i64) -> CommandResult<String> {
    let conn = super::open_db(&app)?;
    let Some(trace) = trace_get_row(&conn, trace_id)? else {
        return Err(CommandError::Internal(format!(
            "执行痕迹 {trace_id} 不存在（可能已被保留期清理）"
        )));
    };
    let spans = spans_for_trace(&conn, trace_id)?;
    let changes = file_changes_for_trace(&conn, trace_id)?;

    let dir = crate::db::paths::data_dir(&app).join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::Internal(e.to_string()))?;
    let path = dir.join(format!("trace-{trace_id}.jsonl"));
    let mut body = serde_json::to_string(&TraceExportRow::trace(&trace))
        .map_err(|e| CommandError::Internal(e.to_string()))?;
    body.push('\n');
    for s in &spans {
        body.push_str(
            &serde_json::to_string(&TraceExportRow::span(s))
                .map_err(|e| CommandError::Internal(e.to_string()))?,
        );
        body.push('\n');
    }
    for c in &changes {
        body.push_str(
            &serde_json::to_string(&TraceExportRow::file_change(c))
                .map_err(|e| CommandError::Internal(e.to_string()))?,
        );
        body.push('\n');
    }
    crate::db::atomic_write(&path, &body).map_err(CommandError::Internal)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "trace.export",
        &[
            ("trace_id", trace_id.to_string()),
            ("spans", spans.len().to_string()),
            ("files", changes.len().to_string()),
        ],
    );
    Ok(path.display().to_string())
}

/// 导出行包装（type 字段区分 trace/span/file_change，消费方按行解析）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceExportRow<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    trace: Option<&'a TraceRow>,
    #[serde(flatten)]
    span: Option<&'a SpanRow>,
    #[serde(flatten)]
    file_change: Option<&'a FileChangeRow>,
}

impl<'a> TraceExportRow<'a> {
    fn trace(t: &'a TraceRow) -> Self {
        Self {
            kind: "trace",
            trace: Some(t),
            span: None,
            file_change: None,
        }
    }
    fn span(s: &'a SpanRow) -> Self {
        Self {
            kind: "span",
            trace: None,
            span: Some(s),
            file_change: None,
        }
    }
    fn file_change(c: &'a FileChangeRow) -> Self {
        Self {
            kind: "file_change",
            trace: None,
            span: None,
            file_change: Some(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(EXEC_TRACE_DDL).unwrap();
        conn
    }

    fn mk_trace<'a>(
        conn: &'a rusqlite::Connection,
        session: &'a str,
        task: Option<&'a str>,
        origin: &'a str,
        started_at: i64,
    ) -> i64 {
        trace_start(
            conn,
            &NewTrace {
                session_id: session,
                task_id: task,
                origin,
                title: Some("测试卡"),
                started_at,
            },
        )
        .unwrap()
    }

    /// 建表幂等：EXEC_TRACE_DDL 连跑两遍不炸（open_db 每次启动都跑）
    #[test]
    fn ddl_is_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ensure_trace_tables(&conn).unwrap();
        ensure_trace_tables(&conn).unwrap();
    }

    /// 索引存在（保留期扫描列 + 三表查询列）
    #[test]
    fn indexes_exist() {
        let conn = setup_conn();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name IN
                 ('idx_traces_task','idx_traces_session','idx_traces_finished','idx_spans_trace','idx_changes_trace')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 5, "5 个索引必须全部建出");
    }

    /// start→finish 往返：files_changed 从 file_changes 反计，不由调用方传
    #[test]
    fn trace_start_finish_roundtrip_counts_files_from_table() {
        let conn = setup_conn();
        let id = mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
        file_change_insert(
            &conn,
            &NewFileChange {
                trace_id: id,
                span_id: None,
                path: "/a/x.py",
                kind: "modify",
                added: 3,
                deleted: 1,
                diff: None,
                truncated: false,
                before_ref: None,
                before_sha: None,
                after_sha: None,
                created_at: 1001,
            },
        )
        .unwrap();
        trace_finish(
            &conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: 2000,
                turn_count: 5,
                tool_calls: 8,
                prompt_tokens: 100,
                completion_tokens: 200,
                error: None,
                model: None,
            },
        )
        .unwrap();
        let t = trace_get_row(&conn, id).unwrap().unwrap();
        assert_eq!(t.status, "done");
        assert_eq!(t.finished_at, Some(2000));
        assert_eq!(t.turn_count, 5);
        assert_eq!(t.tool_calls, 8);
        assert_eq!(
            t.files_changed, 1,
            "files_changed 必须从 file_changes 表反计"
        );
        assert_eq!(t.prompt_tokens, 100);
        assert_eq!(t.error, None);
    }

    /// trace_get 查无此行 → None（不 Err，前端 404 语义留给命令层）
    #[test]
    fn trace_get_missing_returns_none() {
        let conn = setup_conn();
        assert!(trace_get_row(&conn, 999).unwrap().is_none());
    }

    /// span 往返：字段全量保真（ok/error_class/duration_ms）
    #[test]
    fn span_insert_roundtrip() {
        let conn = setup_conn();
        let id = mk_trace(&conn, "s1", None, "Scheduled", 1000);
        let span_id = span_insert(
            &conn,
            &NewSpan {
                trace_id: id,
                turn: 2,
                tool_call_id: Some("call_abc"),
                name: "edit_file",
                args: Some("{\"path\":\"/a/x.py\"}"),
                result: Some("已修改"),
                ok: false,
                error_class: Some("fs_not_found"),
                duration_ms: Some(123),
                created_at: 1001,
            },
        )
        .unwrap();
        let spans = spans_for_trace(&conn, id).unwrap();
        assert_eq!(spans.len(), 1);
        let s = &spans[0];
        assert_eq!(s.id, span_id);
        assert_eq!(s.turn, 2);
        assert_eq!(s.tool_call_id.as_deref(), Some("call_abc"));
        assert_eq!(s.name, "edit_file");
        assert!(!s.ok);
        assert_eq!(s.error_class.as_deref(), Some("fs_not_found"));
        assert_eq!(s.duration_ms, Some(123));
        // 行序 = id 序（时间线渲染依赖）
        span_insert(
            &conn,
            &NewSpan {
                trace_id: id,
                turn: 3,
                tool_call_id: None,
                name: "web_search",
                args: None,
                result: None,
                ok: true,
                error_class: None,
                duration_ms: None,
                created_at: 1002,
            },
        )
        .unwrap();
        let spans = spans_for_trace(&conn, id).unwrap();
        assert!(spans[0].id < spans[1].id);
    }

    /// clamp_text：按字节钳 + UTF-8 边界回退（中文多字节不拦腰）
    #[test]
    fn clamp_text_respects_char_boundary() {
        let short = "abc";
        assert_eq!(clamp_text(short, 100), "abc", "未超限原样返回");
        // 8 字节上限：三个 3 字节汉字只能装下两个
        let zh = "一二三四";
        let clamped = clamp_text(zh, 8);
        assert_eq!(clamped, "一二");
        assert!(clamped.len() <= 8);
        // 全 ASCII 精确钳
        assert_eq!(clamp_text("abcdefghij", 4), "abcd");
    }

    /// file_change 往返：before/after 证据链字段全量保真
    #[test]
    fn file_change_roundtrip_with_rollback_evidence() {
        let conn = setup_conn();
        let id = mk_trace(&conn, "s1", None, "Workflow", 1000);
        file_change_insert(
            &conn,
            &NewFileChange {
                trace_id: id,
                span_id: Some(7),
                path: "/a/x.py",
                kind: "modify",
                added: 12,
                deleted: 4,
                diff: Some("--- a/x.py\n+++ b/x.py\n@@ -1,3 +1,4 @@"),
                truncated: true,
                before_ref: Some("42"),
                before_sha: Some("aaa"),
                after_sha: Some("bbb"),
                created_at: 1001,
            },
        )
        .unwrap();
        let changes = file_changes_for_trace(&conn, id).unwrap();
        assert_eq!(changes.len(), 1);
        let c = &changes[0];
        assert_eq!(c.span_id, Some(7));
        assert_eq!(c.kind, "modify");
        assert_eq!(c.added, 12);
        assert_eq!(c.deleted, 4);
        assert_eq!(
            c.diff.as_deref(),
            Some("--- a/x.py\n+++ b/x.py\n@@ -1,3 +1,4 @@")
        );
        assert!(c.truncated);
        assert_eq!(c.before_ref.as_deref(), Some("42"));
        assert_eq!(c.before_sha.as_deref(), Some("aaa"));
        assert_eq!(c.after_sha.as_deref(), Some("bbb"));
    }

    /// trace_list：过滤组合 + started_at 倒序 + limit 钳制
    #[test]
    fn trace_list_filters_order_and_limit() {
        let conn = setup_conn();
        mk_trace(&conn, "s1", Some("t1"), "Manual", 1000);
        mk_trace(&conn, "s1", Some("t1"), "Manual", 3000);
        mk_trace(&conn, "s2", Some("t1"), "Scheduled", 2000);
        mk_trace(&conn, "s2", Some("t2"), "Workflow", 4000);

        // started_at 倒序
        let all = trace_query(&conn, None, None, None, 100).unwrap();
        let starts: Vec<i64> = all.iter().map(|t| t.started_at).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(starts, sorted, "必须 started_at 倒序");

        // origin 过滤
        let sched = trace_query(&conn, None, None, Some("Scheduled"), 100).unwrap();
        assert_eq!(sched.len(), 1);
        assert_eq!(sched[0].session_id, "s2");
        assert_eq!(sched[0].origin, "Scheduled");

        // task + origin 组合过滤
        let t2_wf = trace_query(&conn, Some("t2"), None, Some("Workflow"), 100).unwrap();
        assert_eq!(t2_wf.len(), 1);
        assert_eq!(t2_wf[0].task_id.as_deref(), Some("t2"));

        // session 过滤
        let s1 = trace_query(&conn, None, Some("s1"), None, 100).unwrap();
        assert_eq!(s1.len(), 2);

        // limit 钳制：请求 0 → 至少给 1；请求超量 → 全量
        assert_eq!(trace_query(&conn, None, None, None, 0).unwrap().len(), 1);
        assert_eq!(trace_query(&conn, None, None, None, 2).unwrap().len(), 2);
        assert_eq!(trace_query(&conn, None, None, None, 9999).unwrap().len(), 4);
    }

    /// 保留期：只删「已收尾且过期」与「僵尸 running」，未过期/活跃行保留；子表级联删
    #[test]
    fn retire_traces_before_cascades_and_keeps_live_rows() {
        let conn = setup_conn();
        // ① 已收尾、过期（finished_at=100 < 5000）→ 删
        let old = mk_trace(&conn, "s1", None, "Manual", 50);
        span_insert(
            &conn,
            &NewSpan {
                trace_id: old,
                turn: 1,
                tool_call_id: None,
                name: "edit_file",
                args: None,
                result: None,
                ok: true,
                error_class: None,
                duration_ms: None,
                created_at: 60,
            },
        )
        .unwrap();
        file_change_insert(
            &conn,
            &NewFileChange {
                trace_id: old,
                span_id: None,
                path: "/a",
                kind: "create",
                added: 1,
                deleted: 0,
                diff: None,
                truncated: false,
                before_ref: None,
                before_sha: None,
                after_sha: None,
                created_at: 61,
            },
        )
        .unwrap();
        trace_finish(
            &conn,
            old,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: 100,
                turn_count: 1,
                tool_calls: 1,
                prompt_tokens: 0,
                completion_tokens: 0,
                error: None,
                model: None,
            },
        )
        .unwrap();
        // ② 已收尾、未过期（finished_at=9000）→ 留
        let fresh = mk_trace(&conn, "s2", None, "Manual", 8000);
        trace_finish(
            &conn,
            fresh,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: 9000,
                turn_count: 1,
                tool_calls: 1,
                prompt_tokens: 0,
                completion_tokens: 0,
                error: None,
                model: None,
            },
        )
        .unwrap();
        // ③ 僵尸 running（started_at=50 < 5000，永远等不到收尾）→ 删
        mk_trace(&conn, "s3", None, "Batch", 50);
        // ④ 活跃 running（started_at=8000）→ 留
        mk_trace(&conn, "s4", None, "Batch", 8000);

        let removed = retire_traces_before(&conn, 5000).unwrap();
        assert_eq!(removed, 2, "过期收尾 + 僵尸 running 各删一条");

        assert!(
            trace_get_row(&conn, old).unwrap().is_none(),
            "过期收尾必须删"
        );
        assert!(
            spans_for_trace(&conn, old).unwrap().is_empty(),
            "子表 spans 必须级联删"
        );
        assert!(
            file_changes_for_trace(&conn, old).unwrap().is_empty(),
            "子表 file_changes 必须级联删"
        );
        assert!(
            trace_get_row(&conn, fresh).unwrap().is_some(),
            "未过期必须留"
        );
        assert_eq!(trace_query(&conn, None, None, None, 100).unwrap().len(), 2);
    }

    /// 保留期空转：无可删行返回 0，不 Err（定时器空跑是常态路径）
    #[test]
    fn retire_traces_before_noop_returns_zero() {
        let conn = setup_conn();
        mk_trace(&conn, "s1", None, "Manual", 8000);
        assert_eq!(retire_traces_before(&conn, 5000).unwrap(), 0);
        assert_eq!(trace_query(&conn, None, None, None, 100).unwrap().len(), 1);
    }

    /// 旧库迁移：无 model 列的 exec_traces（升级前建的库）经 ensure_trace_tables
    /// 补齐 model 列；新库（DDL 已含列）跑第二遍不重复加（幂等）
    #[test]
    fn ensure_trace_tables_migrates_model_column_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // 旧 schema：手工建不含 model 的表（模拟升级前的库）
        conn.execute_batch(
            "CREATE TABLE exec_traces (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               session_id TEXT NOT NULL, task_id TEXT, origin TEXT NOT NULL, title TEXT,
               status TEXT NOT NULL DEFAULT 'running', started_at INTEGER NOT NULL,
               finished_at INTEGER, turn_count INTEGER NOT NULL DEFAULT 0,
               tool_calls INTEGER NOT NULL DEFAULT 0, files_changed INTEGER NOT NULL DEFAULT 0,
               prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0,
               error TEXT);",
        )
        .unwrap();
        ensure_trace_tables(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('exec_traces') WHERE name = 'model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "旧库必须补出 model 列");
        ensure_trace_tables(&conn).unwrap(); // 幂等：再跑一遍不炸不加
                                             // 迁移后可正常写入读取
        let id = mk_trace(&conn, "s1", None, "chat", 1000);
        trace_finish(
            &conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: 2000,
                turn_count: 1,
                tool_calls: 0,
                prompt_tokens: 10,
                completion_tokens: 20,
                error: None,
                model: Some("glm-5.3"),
            },
        )
        .unwrap();
        let t = trace_get_row(&conn, id).unwrap().unwrap();
        assert_eq!(t.model.as_deref(), Some("glm-5.3"), "finish 必须写入模型名");
    }

    /// model COALESCE 语义：None 收尾保留原值（无→NULL），Some 覆盖
    #[test]
    fn trace_finish_model_coalesce() {
        let conn = setup_conn();
        let id = mk_trace(&conn, "s1", None, "chat", 1000);
        trace_finish(
            &conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_FAILED,
                finished_at: 1100,
                turn_count: 0,
                tool_calls: 0,
                prompt_tokens: 0,
                completion_tokens: 0,
                error: Some("boom"),
                model: None,
            },
        )
        .unwrap();
        assert_eq!(
            trace_get_row(&conn, id).unwrap().unwrap().model,
            None,
            "None 收尾不写模型"
        );
    }

    /// 词元按日聚合：缺日补零 + 升序 + 僵尸 running 不计执行次数（token 计入）
    #[test]
    fn usage_stats_daily_zero_fills_and_counts_finished_only() {
        let conn = setup_conn();
        // 今天：一次完整执行（done）+ 一次僵尸 running——runs 只应算 1
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let today_ms = today
            .and_hms_opt(4, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let id = mk_trace(&conn, "s1", None, "chat", today_ms);
        trace_finish(
            &conn,
            id,
            &TraceFinish {
                status: TRACE_STATUS_DONE,
                finished_at: today_ms + 100,
                turn_count: 2,
                tool_calls: 3,
                prompt_tokens: 1000,
                completion_tokens: 500,
                error: None,
                model: Some("glm-5.3"),
            },
        )
        .unwrap();
        // 僵尸：不 finish，手工点 prompt_tokens 模拟半路用量
        let zombie = mk_trace(&conn, "s2", None, "chat", today_ms + 200);
        conn.execute(
            "UPDATE exec_traces SET prompt_tokens = 100 WHERE id = ?1",
            [zombie],
        )
        .unwrap();
        // 昨天：一次 stopped 执行
        let y_ms = today_ms - 86_400_000;
        let yid = mk_trace(&conn, "s1", None, "manual", y_ms);
        trace_finish(
            &conn,
            yid,
            &TraceFinish {
                status: TRACE_STATUS_STOPPED,
                finished_at: y_ms + 50,
                turn_count: 1,
                tool_calls: 1,
                prompt_tokens: 200,
                completion_tokens: 300,
                error: None,
                model: None,
            },
        )
        .unwrap();

        let days = usage_stats_daily_conn(&conn, 7, today).unwrap();
        assert_eq!(days.len(), 7, "窗口 7 天必须逐日补零返回");
        assert_eq!(days.last().unwrap().day, "2026-10-06");
        assert_eq!(days[0].prompt_tokens, 0, "窗口外沿必须补零");
        let today_row = days.last().unwrap();
        assert_eq!(today_row.prompt_tokens, 1100, "token 含僵尸行");
        assert_eq!(today_row.completion_tokens, 500);
        assert_eq!(today_row.runs, 1, "僵尸 running 不算执行次数");
        let y_row = &days[days.len() - 2];
        assert_eq!((y_row.prompt_tokens, y_row.completion_tokens), (200, 300));
        assert_eq!(y_row.runs, 1, "stopped 也算已收尾");
    }

    /// 按模型聚合：分组求和 + 总 tokens 降序 + NULL model 参与分组（前端显示未知）
    #[test]
    fn usage_stats_by_model_groups_orders_and_keeps_null() {
        let conn = setup_conn();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let t0 = today
            .and_hms_opt(4, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let finish = |conn: &rusqlite::Connection, id: i64, p: i64, c: i64, model: Option<&str>| {
            trace_finish(
                conn,
                id,
                &TraceFinish {
                    status: TRACE_STATUS_DONE,
                    finished_at: t0 + 100,
                    turn_count: 1,
                    tool_calls: 0,
                    prompt_tokens: p,
                    completion_tokens: c,
                    error: None,
                    model,
                },
            )
            .unwrap();
        };
        finish(
            &conn,
            mk_trace(&conn, "s1", None, "chat", t0),
            100,
            100,
            Some("glm-5.3"),
        );
        finish(
            &conn,
            mk_trace(&conn, "s1", None, "chat", t0 + 1),
            400,
            400,
            Some("glm-5.3"),
        );
        finish(
            &conn,
            mk_trace(&conn, "s2", None, "manual", t0 + 2),
            300,
            100,
            Some("kimi-k3"),
        );
        finish(
            &conn,
            mk_trace(&conn, "s3", None, "manual", t0 + 3),
            50,
            50,
            None,
        );

        let rows = usage_stats_by_model_conn(&conn, 7, today).unwrap();
        assert_eq!(rows.len(), 3, "glm-5.3 / kimi-k3 / NULL 三组");
        assert_eq!(rows[0].model.as_deref(), Some("glm-5.3"));
        assert_eq!(
            (rows[0].prompt_tokens, rows[0].completion_tokens),
            (500, 500)
        );
        assert_eq!(rows[1].model.as_deref(), Some("kimi-k3"));
        assert_eq!(rows[2].model, None, "NULL 模型保留（前端显示未知）");
        assert!(
            rows[0].prompt_tokens + rows[0].completion_tokens
                >= rows[1].prompt_tokens + rows[1].completion_tokens,
            "必须按总 tokens 降序"
        );
    }

    /// 按模型×按日聚合：day 升序、(day, model) 分组、NULL model 保留、缺日不返回（前端补零）
    #[test]
    fn usage_stats_daily_by_model_groups_and_orders() {
        let conn = setup_conn();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let t0 = today
            .and_hms_opt(4, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .earliest()
            .unwrap()
            .timestamp_millis();
        let finish =
            |conn: &rusqlite::Connection, id: i64, at: i64, p: i64, model: Option<&str>| {
                trace_finish(
                    conn,
                    id,
                    &TraceFinish {
                        status: TRACE_STATUS_DONE,
                        finished_at: at + 100,
                        turn_count: 1,
                        tool_calls: 0,
                        prompt_tokens: p,
                        completion_tokens: p,
                        error: None,
                        model,
                    },
                )
                .unwrap();
            };
        // 今天：glm-5.3 两行 + kimi 一行；昨天：NULL 模型一行
        finish(
            &conn,
            mk_trace(&conn, "s1", None, "chat", t0),
            t0,
            100,
            Some("glm-5.3"),
        );
        finish(
            &conn,
            mk_trace(&conn, "s1", None, "chat", t0 + 1),
            t0 + 1,
            200,
            Some("glm-5.3"),
        );
        finish(
            &conn,
            mk_trace(&conn, "s2", None, "chat", t0 + 2),
            t0 + 2,
            50,
            Some("kimi-k3"),
        );
        finish(
            &conn,
            mk_trace(&conn, "s3", None, "manual", t0 - 86_400_000),
            t0 - 86_400_000,
            30,
            None,
        );

        let rows = usage_stats_daily_by_model_conn(&conn, 7, today).unwrap();
        assert_eq!(
            rows.len(),
            3,
            "10-05 NULL + 10-06 两模型 = 3 行；缺日不补零"
        );
        let days: Vec<&str> = rows.iter().map(|r| r.day.as_str()).collect();
        assert_eq!(days, ["2026-10-05", "2026-10-06", "2026-10-06"], "day 升序");
        assert_eq!(rows[0].model, None, "NULL 模型排在同日最前（NULLs first）");
        assert_eq!((rows[0].prompt_tokens, rows[0].completion_tokens), (30, 30));
        assert_eq!(rows[1].model.as_deref(), Some("glm-5.3"));
        assert_eq!(
            (rows[1].prompt_tokens, rows[1].completion_tokens),
            (300, 300),
            "同日同模型求和"
        );
        assert_eq!(rows[2].model.as_deref(), Some("kimi-k3"));
    }
}
