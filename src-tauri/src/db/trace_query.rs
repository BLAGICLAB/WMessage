//! 执行痕迹的查询面：摘要列表 / 单条详情 / 会话 span / 会话工具名。
//!
//! 从 `db/trace.rs` 拆出。本模块与 `crate::db::trace` **同层**，
//! 两者的 `super::` 同指 `crate::db`——所以搬移不改任何路径语义。

use super::trace::{trace_row_from, SpanRow, TraceRow, TRACE_COLS};

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

/// 本会话调用过的工具名（分「调用过」与「成功过」两档）。
///
/// 幻觉守卫「点名判据」的证据池。两档分开的理由是两道判据问的不是同一件事：
///
/// · **成功过**——终稿声称某个工具跑成了，而它从没成功返回过 → 编造。跨提问的
///   重述（「刚才 create_word 生成的那个」）是真话，所以这一档要覆盖本会话历史，
///   不止本次提问。
/// · **调用过**——本会话派过子 agent 的判断依据。子 agent 的调用不落我们的痕迹表
///   （见下），它跑的活在我们眼里永远「没成功过」；派过子 agent 时点名判据整档
///   让开，查不到不等于没跑过。
///
/// 只覆盖**主 agent**的调用：子 agent 的 span 记在 `subagents.trace_id` 下，而那个
/// 值是 `trc_xxx` 字符串、`exec_spans.trace_id` 是整数——join 永远不成立，子 agent
/// 的工具名在这张表里查不到。调用方遇到派过子 agent 的会话要整档让开。
#[derive(Clone, Debug, Default)]
pub struct SessionTools {
    /// 调用过（含失败）
    pub called: Vec<String>,
    /// 成功过
    pub succeeded: Vec<String>,
}

pub fn session_tool_names(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<SessionTools, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT s.name, s.ok FROM exec_spans s
             JOIN exec_traces t ON s.trace_id = t.id
             WHERE t.session_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
        })
        .map_err(|e| e.to_string())?;
    let mut out = SessionTools {
        called: Vec::new(),
        succeeded: Vec::new(),
    };
    for r in rows {
        let (name, ok) = r.map_err(|e| e.to_string())?;
        if !out.called.contains(&name) {
            out.called.push(name.clone());
        }
        if ok && !out.succeeded.contains(&name) {
            out.succeeded.push(name);
        }
    }
    Ok(out)
}
