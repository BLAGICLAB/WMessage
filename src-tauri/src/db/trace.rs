//! 执行痕迹三表（Agent 透明化改造设计 §4.1，docs/AGENT-TRANSPARENCY-DESIGN-2026-10-06.md）：
//! 一次执行 = 一条 `exec_traces`（run_task_in_chat / 主聊天 / 技能 run），
//! 每次工具调用 = 一条 `exec_spans`，每次文件落盘修改 = 一条 `file_changes`。
//!
//! 定位：**append-only 观测面**——只增与收尾，不参与任何业务判定；写路径全部
//! 经 `open_db` 幂等建表（`EXEC_TRACE_DDL`），采集侧（/c/d）按 span/change
//! 逐条 insert，收尾由 `trace_finish` 一次 UPDATE 汇总。
//!
//! 钳制纪律（与审计 KV 500 字符同思路：本地库也防膨胀）：
//! - `SPAN_TEXT_MAX`：args/result 单条 16KB（生成侧钳，本模块 `clamp_text` 单源）
//! - `MAX_DIFF_LINES`：unified diff 行数上限（ 生成侧引用）
//! - `TRACE_RETENTION_DAYS`：保留期默认 30 天（`retire_traces_before` 由设置页
//!   手动命令与自动清扫 `spawn_retention_sweep` 两条触发）

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::error::{CommandError, CommandResult};

// 拆出后的子模块转出：保持 `crate::db::trace::X` 与 `crate::db::X` 两条旧路径不变
pub use super::trace_query::{
    session_spans, session_tool_names, span_get_row, spans_for_trace, trace_get_row, trace_query,
    SessionTools,
};
pub(crate) use super::trace_session::session_file_changes_sync;
pub use super::trace_session::{
    file_changes_for_session, SessionFileChangeEntry, SessionFileChangeItem, SessionFileChanges,
    SessionFileDiff, SESSION_FILE_CHANGES_MAX,
};
pub use super::trace_usage::{
    usage_stats_by_model_conn, usage_stats_daily_by_model_conn, usage_stats_daily_conn,
    TraceDetail, UsageByModel, UsageDay, UsageDayModel,
};

/// args/result 单条文本钳制（16KB）
pub const SPAN_TEXT_MAX: usize = 16 * 1024;
/// unified diff 行数钳制（ 生成侧引用）
pub const MAX_DIFF_LINES: usize = 2000;
/// 执行痕迹保留期（天）；对齐 Claude Code checkpointing 的 cleanupPeriodDays 默认
pub const TRACE_RETENTION_DAYS: i64 = 30;
/// 保留期可填范围（天）：下限 1（0 天 = 删光，不解释成那个意思），上限 365。
/// 手动清理与自动清扫共用这同一组边界——设置页公示的「1–365」即这两个数。
pub const TRACE_RETENTION_MIN_DAYS: i64 = 1;
pub const TRACE_RETENTION_MAX_DAYS: i64 = 365;
/// 自动清扫间隔（秒）：启动即跑一次，此后每天一次。保留期以「天」计，
/// 每天一轮足够——早一轮晚一轮对 30 天的窗口没有可观察差别。
const RETENTION_SWEEP_INTERVAL_SECS: u64 = 24 * 3600;

/// 词元统计可查窗口上限（天）：热力图能拉多宽的历史由这个数决定。
///
/// 三个 `usage_stats_*` 命令共用这一份上界——命令里的 `unwrap_or(30)` 与
/// `clamp(1, 365)` 曾各写一份字面量，改一处漏一处不会编译失败也不会测试失败。
pub const USAGE_STATS_MAX_DAYS: i64 = 365;

/// 词元统计窗口解析：`None` = 取上界（要多少给多少），`Some(n)` = 钳到 1..=上界。
pub fn resolve_usage_window_days(days: Option<i64>) -> i64 {
    days.map(|d| d.clamp(1, USAGE_STATS_MAX_DAYS))
        .unwrap_or(USAGE_STATS_MAX_DAYS)
}

/// trace 收尾状态值域（status 列；running 为起始态，其余四态只能由 trace_finish 写入）
pub const TRACE_STATUS_RUNNING: &str = "running";
pub const TRACE_STATUS_DONE: &str = "done";
pub const TRACE_STATUS_FAILED: &str = "failed";
pub const TRACE_STATUS_STOPPED: &str = "stopped";

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
  prompt_tokens       INTEGER NOT NULL DEFAULT 0,
  completion_tokens   INTEGER NOT NULL DEFAULT 0,
  -- 缓存 token（Anthropic 协议四档计价之二）。单列而非塞进 prompt_tokens：
  -- cache read 是 0.1x、cache write 是 1.25x，与普通 input 混算等于把
  -- 长对话的缓存收益吃掉——一次长会话里 cache_read 常数倍于新增 input。
  cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
  cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
  error               TEXT,
  model               TEXT
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
CREATE INDEX IF NOT EXISTS idx_changes_trace ON file_changes(trace_id);
-- 路径同一性用小写归一（与仓库既有的路径查重口径一致：同名不同路径算不同条目，
-- 但大小写不同的同一个路径算同一条）。索引带 COLLATE NOCASE，查询侧同款写法
-- 才能走上索引——否则每次判定都要全表扫。
CREATE INDEX IF NOT EXISTS idx_changes_path ON file_changes(path COLLATE NOCASE);";

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
    /// 缓存读取 token（cache read 计价档）。旧行为 0。
    pub cache_read_tokens: i64,
    /// 缓存写入 token（cache write 计价档）。旧行为 0。
    pub cache_write_tokens: i64,
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

/// file_change 落盘入参（ 由 FileChangeReceipt 映射而来）
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
    ///  回滚按此名取 `data_dir/checkpoints/<before_ref>`。设计 §9.2-3 据实现修正）
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
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
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

pub(crate) fn trace_row_from(r: &rusqlite::Row) -> rusqlite::Result<TraceRow> {
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
        cache_read_tokens: r.get(13)?,
        cache_write_tokens: r.get(14)?,
        error: r.get(15)?,
        model: r.get(16)?,
    })
}

pub(crate) const TRACE_COLS: &str =
    "id, session_id, task_id, origin, title, status, started_at, finished_at, \
     turn_count, tool_calls, files_changed, prompt_tokens, completion_tokens, \
     cache_read_tokens, cache_write_tokens, error, model";

/// 幂等建三表（open_db 接线；测试直接 execute_batch EXEC_TRACE_DDL 同款）。
/// 旧库迁移：model 列（2026-10 词元统计按模型聚合）与 cache_*_tokens 两列
/// （缓存档计价不进 prompt_tokens）都用 pragma 探测后 ALTER 补齐。
pub fn ensure_trace_tables(conn: &rusqlite::Connection) -> Result<(), String> {
    conn.execute_batch(EXEC_TRACE_DDL)
        .map_err(|e| e.to_string())?;
    // 补列一律走 add_column_if_absent：**open_db 每次调用都跑本函数，而 open_db
    // 会在多线程并发（首装时 setup / 僵尸清扫 / 首条命令各开一个连接），
    // 两个连接会同时探测到「列不存在」再同时 ALTER，后到者吃 duplicate column。
    // 先查后改的写法挡不住这个竞态——只有让 ALTER 自己容忍重复才挡得住。
    add_column_if_absent(conn, "exec_traces", "model", "TEXT")?;
    // 缓存 token 两列（Anthropic 四档计价之二，理由见 EXEC_TRACE_DDL 内的注释）
    add_column_if_absent(
        conn,
        "exec_traces",
        "cache_read_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_absent(
        conn,
        "exec_traces",
        "cache_write_tokens",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}

/// 列不存在才 ALTER；已存在（含别的连接刚加上的）一律静默成功。
///
/// 不吞真实错误：只有「duplicate column name」这一条被容忍，其余
/// （库锁、SQL 语法错）照原样上抛——后者是代码 bug，静默会让缺列一路掩到查询失败。
fn add_column_if_absent(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), String> {
    let has: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if has != 0 {
        return Ok(());
    }
    // 表名/列名来自本文件的常量、decl 同源，不含外部输入；
    // 仍用 format! 而非参数绑定——SQLite 的 ALTER TABLE 不支持占位符。
    match conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}")) {
        Ok(()) => Ok(()),
        Err(e) if e.to_string().contains("duplicate column name") => Ok(()),
        Err(e) => Err(e.to_string()),
    }
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
           cache_read_tokens = ?8, cache_write_tokens = ?9,
           error = ?10,
           model = COALESCE(?11, model),
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
            f.cache_read_tokens,
            f.cache_write_tokens,
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

/// 这批路径里哪些有落盘凭证（被工具写过盘）。
///
/// 判据是「有没有工具写过这个路径」，不是「这次写没写」——痕迹表按 30 天保留期
/// 清理，所以这里的答案是「保留期内被写过」。路径比较大小写不敏感，与仓库既有
/// 的路径同一性口径（小写归一）一致。
///
/// 返回**有凭证的**那部分（调用方取差集即「无凭证的」）。
pub fn file_paths_with_receipt(
    conn: &rusqlite::Connection,
    paths: &[String],
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT path FROM file_changes WHERE path = ?1 COLLATE NOCASE LIMIT 1")
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for p in paths {
        let hit: Option<String> = stmt.query_row([p], |r| r.get::<_, String>(0)).ok();
        if let Some(found) = hit {
            out.push(found);
        }
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

pub(crate) fn file_change_from_row(r: &rusqlite::Row) -> rusqlite::Result<FileChangeRow> {
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

// 前端查询命令

#[tauri::command]
pub fn usage_stats_daily_by_model(
    app: tauri::AppHandle,
    days: Option<i64>,
) -> CommandResult<Vec<UsageDayModel>> {
    let conn = super::open_db(&app)?;
    let days = resolve_usage_window_days(days);
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

/// 保留期截止时间戳（纯函数单测锚点）：days 先钳 1..=365 再折算成毫秒，
/// 手动命令与自动清扫共用同一口径（钳制不一致会让「公示的保留期」与实际删的
/// 范围在边界上对不上）。
pub fn retention_cutoff_ms(days: i64, now_ms: i64) -> i64 {
    now_ms - days.clamp(TRACE_RETENTION_MIN_DAYS, TRACE_RETENTION_MAX_DAYS) * 86_400_000
}

/// 保留期清理内核：设置页手动命令与自动清扫共用。
/// `days = None` 走 `TRACE_RETENTION_DAYS`（设置页公示的那个值）。
pub fn retire_traces_before_days(
    app: &tauri::AppHandle,
    days: Option<i64>,
) -> CommandResult<usize> {
    // 全表 DELETE：与其它写口同一把写锁，避免和并发写者抢 busy_timeout
    // （2s）后整条命令失败
    let _g = super::lock_db_write();
    let conn = super::open_db(app)?;
    let days = days.unwrap_or(TRACE_RETENTION_DAYS);
    let before = retention_cutoff_ms(days, crate::db::now_ms());
    let removed = retire_traces_before(&conn, before)?;
    if removed > 0 {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "trace.retired",
            &[("removed", removed.to_string()), ("days", days.to_string())],
        );
    }
    Ok(removed)
}

/// 自动保留期清扫：启动即跑一次，此后每 24h 一次。
///
/// 为什么要有：设置页公示「执行痕迹保留期（天）30」，原先只有数据管理里的手动
/// 按钮会删——公示的值与实际行为不符（长期不重启的应用等于永不清理）。
/// 失败只记审计，不打断启动、不终止循环。
pub fn spawn_retention_sweep(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            // retire 内部是 lock_db_write + open_db + 全表 DELETE——同步磁盘
            // IO 不落在 tokio worker 上（持锁等主窗长事务时会拖住整个 worker）
            let h = handle.clone();
            let retired =
                tauri::async_runtime::spawn_blocking(move || retire_traces_before_days(&h, None))
                    .await
                    .unwrap_or_else(|e| {
                        Err(crate::error::CommandError::from(format!(
                            "清扫线程 join 失败：{e}"
                        )))
                    });
            if let Err(e) = retired {
                crate::audit::write_event(
                    &handle,
                    crate::audit::AuditLevel::Warn,
                    "trace.retire_fail",
                    &[("err", e.to_string())],
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                RETENTION_SWEEP_INTERVAL_SECS,
            ))
            .await;
        }
    });
}

/// 保留期清理（数据管理手动入口；自动清扫走 spawn_retention_sweep）
#[tauri::command]
pub fn trace_clear_before(app: tauri::AppHandle, days: Option<i64>) -> CommandResult<usize> {
    retire_traces_before_days(&app, days)
}

#[tauri::command]
pub fn usage_stats_daily(app: tauri::AppHandle, days: Option<i64>) -> CommandResult<Vec<UsageDay>> {
    let conn = super::open_db(&app)?;
    let days = resolve_usage_window_days(days);
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
    let days = resolve_usage_window_days(days);
    Ok(usage_stats_by_model_conn(
        &conn,
        days,
        chrono::Local::now().date_naive(),
    )?)
}

/// 回滚核心：conn + 数据目录齐全即可完成全部闸口判定与恢复写盘，成功返回
/// 目标路径。命令壳只负责开库、拼用户消息与审计（审计仅在成功路径写，与
/// 抽核前行为一致）。
/// 双闸防吞改：
/// 1. **漂移闸**——当前文件内容 sha 必须等于本变更落盘时的 after_sha，文件被
///    （人/其他流程）动过即拒绝（覆盖会吞掉后续修改）；
/// 2. **快照闸**——快照内容 sha 必须等于 before_sha（快照文件损坏/被换即拒绝）。
/// 仅支持 modify：create 的撤销=删文件，属危险动作待立项（界面隐藏按钮兜底）。
pub(crate) fn file_rollback_core(
    conn: &rusqlite::Connection,
    data_dir: &std::path::Path,
    change_id: i64,
) -> Result<String, String> {
    let Some(c) = file_change_get(conn, change_id)? else {
        return Err(format!("变更记录 {change_id} 不存在（可能已被保留期清理）"));
    };
    if c.kind != "modify" {
        return Err(format!(
            "暂不支持 {} 类型的回滚（仅 modify；新建文件的撤销=删文件，属危险动作待立项）",
            c.kind
        ));
    }
    let Some(before_ref) = c.before_ref.clone() else {
        return Err("该变更无 before 快照（写盘时快照失败降级），无法回滚".into());
    };
    // 快照名由写入侧 uuid 发号（bot_fs）；带目录分隔符/父目录引用/绝对路径的
    // 一律视为被篡改的 DB 行，拒绝——防快照读取越界到任意文件
    let snap_path = std::path::Path::new(&before_ref);
    if before_ref.is_empty() || snap_path.file_name() != Some(snap_path.as_os_str()) {
        return Err(format!(
            "快照名非法（应为纯文件名），拒绝回滚：{before_ref}"
        ));
    }
    // 纵深防御：c.path 同样来自 DB 行，不直接信。回滚是覆盖写，目标若是
    // 符号链接会顺着链接打到任意文件，先拒绝；再给全文读取设硬上限防 OOM。
    // （残余窗口：检查与读写之间目标被换成符号链接，单机桌面场景可接受。）
    // 与用户白名单工作区的归一化比对涉及 bot_fs 策略口径，另行走闸口方案。
    let path = std::path::PathBuf::from(&c.path);
    let meta =
        std::fs::symlink_metadata(&path).map_err(|_| format!("目标文件已不存在：{}", c.path))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "目标是符号链接，拒绝回滚（防止写到链接指向的路径）：{}",
            c.path
        ));
    }
    if !meta.is_file() {
        return Err(format!("目标文件已不存在：{}", c.path));
    }
    // 与 SPAN_TEXT_MAX 同思路：文本快照读写给个硬上限，防被指向超大文件拖爆内存
    const FILE_ROLLBACK_MAX_BYTES: u64 = 64 * 1024 * 1024;
    if meta.len() > FILE_ROLLBACK_MAX_BYTES {
        return Err(format!(
            "目标文件过大（{} 字节 > 上限 {FILE_ROLLBACK_MAX_BYTES}），拒绝回滚；请人工核对：{}",
            meta.len(),
            c.path
        ));
    }
    let current =
        std::fs::read_to_string(&path).map_err(|e| format!("当前文件不可读（{}）：{e}", c.path))?;
    if c.after_sha
        .as_deref()
        .is_none_or(|a| a != crate::bot::sha256_hex(&current))
    {
        return Err(format!(
            "文件自本次修改后已被改动（指纹不符），拒绝回滚以免吞掉后续修改；请人工核对：{}",
            c.path
        ));
    }
    let snapshot = data_dir.join("checkpoints").join(&before_ref);
    let before = std::fs::read_to_string(&snapshot)
        .map_err(|e| format!("快照不可读（{before_ref}）：{e}"))?;
    if c.before_sha
        .as_deref()
        .is_none_or(|b| b != crate::bot::sha256_hex(&before))
    {
        return Err("快照校验不符（before_sha），拒绝回滚".into());
    }
    crate::db::atomic_write(&path, &before)?;
    Ok(c.path)
}

#[tauri::command]
pub fn file_rollback(app: tauri::AppHandle, change_id: i64) -> CommandResult<String> {
    let conn = super::open_db(&app)?;
    let path = file_rollback_core(&conn, &crate::db::paths::data_dir(&app), change_id)
        .map_err(CommandError::Internal)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Warn,
        "file.rollback",
        &[
            ("change_id", change_id.to_string()),
            ("path", crate::bot::truncate_for_log(&path, 200)),
        ],
    );
    Ok(format!("已回滚 {path}（恢复到本次修改前）"))
}

/// 导出结果（壳只拿它拼审计与返回值，不再回查）
#[derive(Debug)]
pub(crate) struct ExportOutcome {
    pub path: String,
    pub spans: usize,
    pub files: usize,
}

/// 单次执行痕迹导出 JSONL（Codex rollout 本地文件化思想）——
/// 第 1 行 trace 摘要，随后 span 行（`{"type":"span",...}`）与文件变更行
/// （`{"type":"file_change",...}`）按 id 序混排。落 `data_dir/exports/`，
/// 返回绝对路径供前端打开/分享。只读导出，不改动痕迹数据。
/// 核心与命令壳分离：conn + 数据目录即可测，壳只管开库与审计。
pub(crate) fn trace_export_core(
    conn: &rusqlite::Connection,
    data_dir: &std::path::Path,
    trace_id: i64,
) -> Result<ExportOutcome, String> {
    let Some(trace) = trace_get_row(conn, trace_id)? else {
        return Err(format!("执行痕迹 {trace_id} 不存在（可能已被保留期清理）"));
    };
    let spans = spans_for_trace(conn, trace_id)?;
    let changes = file_changes_for_trace(conn, trace_id)?;

    let dir = data_dir.join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("trace-{trace_id}.jsonl"));
    let mut body =
        serde_json::to_string(&TraceExportRow::trace(&trace)).map_err(|e| e.to_string())?;
    body.push('\n');
    for s in &spans {
        body.push_str(&serde_json::to_string(&TraceExportRow::span(s)).map_err(|e| e.to_string())?);
        body.push('\n');
    }
    for c in &changes {
        body.push_str(
            &serde_json::to_string(&TraceExportRow::file_change(c)).map_err(|e| e.to_string())?,
        );
        body.push('\n');
    }
    crate::db::atomic_write(&path, &body)?;
    Ok(ExportOutcome {
        path: path.display().to_string(),
        spans: spans.len(),
        files: changes.len(),
    })
}

#[tauri::command]
pub fn trace_export(app: tauri::AppHandle, trace_id: i64) -> CommandResult<String> {
    let conn = super::open_db(&app)?;
    let out = trace_export_core(&conn, &crate::db::paths::data_dir(&app), trace_id)
        .map_err(CommandError::Internal)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "trace.export",
        &[
            ("trace_id", trace_id.to_string()),
            ("spans", out.spans.to_string()),
            ("files", out.files.to_string()),
        ],
    );
    Ok(out.path)
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

/// 本会话文件改动（聊天侧栏「文件」工作面）。
///
/// 给 `path` = 取该文件的详情；不给 = 取本会话改动文件清单。
/// 两种用法分开算：清单要的是「所有文件的累计 ±」，详情要的是「一个文件的 diff」，
/// 各算各的最省——否则点开一个文件要把全会话的文件重读一遍。
#[tauri::command]
pub async fn bot_session_file_changes(
    app: tauri::AppHandle,
    session_id: String,
    path: Option<String>,
) -> CommandResult<SessionFileChanges> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        session_file_changes_sync(&handle, &session_id, path.as_deref())
    })
    .await
    .map_err(|e| CommandError::Internal(format!("会话改动统计线程失败：{e}")))?
}

#[cfg(test)]
mod tests;
