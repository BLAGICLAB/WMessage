//! 凭证召回工具：把历史工具调用的完整结果调回模型上下文。
//!
//! 为什么需要它：工具消息**不跨轮持久**——上一轮 `read_text_file` 的返回、
//! 上一轮 OCR 的整页文字，出了那一轮就不在上下文里了。模型想引用旧结果时
//! 只有两条路：重跑工具（贵，且写类工具有副作用）或凭记忆复述（编造）。
//! 痕迹表 `exec_spans` 里存着每次调用的完整返回，于是给它一条**只读**的
//! 合法召回渠道。
//!
//! 三条口径：
//! - **只读**：不碰任何业务表，零新表、零写副作用；
//! - **会话内限定**：痕迹表是全局的，但「本会话跑过什么」只该在本会话里
//!   答——别的会话的 trace id 传进来一律按「查不到」处理，不泄露；
//! - **查不到就说查不到**：返回明确提示，绝不脑补一份结果。

use crate::bot::registry::ToolResult;
use crate::db::trace::SpanRow;

/// 清单模式最多列几条（一条一行，太多就淹掉上下文）
const LIST_LIMIT: usize = 30;
/// 清单里每条的结果节选字数
const SNIPPET_CHARS: usize = 120;
/// 单次调用的完整返回最多回灌多少字（痕迹表本身按 16KB 存，这里再收一道）
const DETAIL_CHARS: usize = 8000;

/// 凭证召回：按 tool_call_id / span_id / trace_id 调回历史工具调用的完整结果。
///
/// 不传任何参数 → 列出本会话最近的工具调用凭证（工具名 / 轮次 / 成败 /
/// 结果节选 / 可用于精确调取的 id），供模型挑出要哪一条。
pub(crate) async fn tool_lookup_receipt<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    args: &str,
    session_id: Option<&str>,
) -> ToolResult {
    let Some(session_id) = session_id.filter(|s| !s.is_empty()) else {
        return ToolResult::denied(
            "lookup_receipt 需要会话上下文才能定位历史工具结果；本轮没有会话 id，无法召回。"
                .to_string(),
        );
    };
    let v = crate::bot::dispatch::parse_args(args);
    let tool_call_id = v["tool_call_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let span_id = v["span_id"].as_i64();
    let trace_id = v["trace_id"].as_i64();

    let conn = match crate::db::open_db(app) {
        Ok(c) => c,
        Err(e) => {
            return ToolResult::error(format!("lookup_receipt 读取执行痕迹失败：{e}"), Vec::new());
        }
    };

    // 精确调取：span_id 最准（一次调用一条 span），tool_call_id 次之
    // （LLM 签发的调用 id），trace_id 最粗（一次执行里的全部调用）。
    if let Some(sid) = span_id {
        return match crate::db::trace::span_get_row(&conn, sid) {
            Ok(Some(row)) => {
                if !belongs_to_session(&conn, &row, session_id) {
                    return not_found("span_id");
                }
                ToolResult::ok(render_detail(&row), Vec::new())
            }
            Ok(None) => not_found("span_id"),
            Err(e) => ToolResult::error(format!("lookup_receipt 查询失败：{e}"), Vec::new()),
        };
    }
    if let Some(tcid) = tool_call_id {
        let spans = match crate::db::trace::session_spans(&conn, session_id, 500) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult::error(format!("lookup_receipt 查询失败：{e}"), Vec::new());
            }
        };
        let hits: Vec<&SpanRow> = spans
            .iter()
            .filter(|s| s.tool_call_id.as_deref() == Some(tcid))
            .collect();
        if hits.is_empty() {
            return not_found("tool_call_id");
        }
        if hits.len() == 1 {
            return ToolResult::ok(render_detail(hits[0]), Vec::new());
        }
        return ToolResult::ok(render_list(&hits, session_id), Vec::new());
    }
    if let Some(tid) = trace_id {
        let spans = match crate::db::trace::session_spans(&conn, session_id, 500) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult::error(format!("lookup_receipt 查询失败：{e}"), Vec::new());
            }
        };
        let hits: Vec<&SpanRow> = spans.iter().filter(|s| s.trace_id == tid).collect();
        if hits.is_empty() {
            return not_found("trace_id");
        }
        return ToolResult::ok(render_list(&hits, session_id), Vec::new());
    }

    // 清单模式：本会话最近的工具调用凭证
    let spans = match crate::db::trace::session_spans(&conn, session_id, LIST_LIMIT) {
        Ok(s) => s,
        Err(e) => {
            return ToolResult::error(format!("lookup_receipt 查询失败：{e}"), Vec::new());
        }
    };
    if spans.is_empty() {
        return ToolResult::ok(
            "本会话没有可召回的工具调用凭证（痕迹表里没有本会话的记录）。".to_string(),
            Vec::new(),
        );
    }
    let refs: Vec<&SpanRow> = spans.iter().collect();
    ToolResult::ok(render_list(&refs, session_id), Vec::new())
}

/// 这条 span 是否属于本会话（trace_id → exec_traces.session_id）。
fn belongs_to_session(conn: &rusqlite::Connection, span: &SpanRow, session_id: &str) -> bool {
    match crate::db::trace::trace_get_row(conn, span.trace_id) {
        Ok(Some(t)) => t.session_id == session_id,
        _ => false,
    }
}

/// 查不到：明确说查不到，不给任何「可能是这样」的内容。
fn not_found(which: &str) -> ToolResult {
    ToolResult::ok(
        format!(
            "查不到 {which} 对应的工具调用凭证（不在本会话的执行痕迹里，或已被 30 天保留期清理）。\
             不要凭记忆复述这次调用的结果——如实说拿不到即可。"
        ),
        Vec::new(),
    )
}

/// 清单模式渲染：一条一行，带足以精确调取的 id。
fn render_list(spans: &[&SpanRow], session_id: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "本会话（{session_id}）的工具调用凭证，共 {} 条（按时间顺序）：\n",
        spans.len()
    ));
    for (i, s) in spans.iter().enumerate() {
        out.push_str(&format!(
            "{}. [tool_call_id={}, span_id={}, trace_id={}, turn={}] {} {} {}\n",
            i + 1,
            s.tool_call_id.as_deref().unwrap_or("-"),
            s.id,
            s.trace_id,
            s.turn,
            s.name,
            if s.ok { "✅" } else { "❌" },
            snippet(s.result.as_deref().unwrap_or(""))
        ));
    }
    out.push_str(
        "要某一条的完整返回，用它的 tool_call_id（或 span_id / trace_id）再调一次本工具。",
    );
    out
}

/// 单条渲染：元信息 + 入参 + 完整返回。
fn render_detail(s: &SpanRow) -> String {
    let mut out = String::new();
    out.push_str("工具调用凭证\n");
    out.push_str(&format!(
        "- tool_call_id: {}\n",
        s.tool_call_id.as_deref().unwrap_or("-")
    ));
    out.push_str(&format!("- span_id: {}\n", s.id));
    out.push_str(&format!("- trace_id: {}\n", s.trace_id));
    out.push_str(&format!("- 轮次: {}\n", s.turn));
    out.push_str(&format!("- 工具: {}\n", s.name));
    out.push_str(&format!(
        "- 结果: {}{}\n",
        if s.ok { "成功" } else { "失败" },
        s.duration_ms
            .map(|d| format!("（耗时 {d}ms）"))
            .unwrap_or_default()
    ));
    if let Some(err) = s.error_class.as_deref().filter(|e| !e.is_empty()) {
        out.push_str(&format!("- 错误分类: {err}\n"));
    }
    if let Some(a) = s.args.as_deref().filter(|a| !a.trim().is_empty()) {
        out.push_str(&format!("- 入参: {a}\n"));
    }
    out.push_str("- 完整返回:\n");
    let result = s.result.as_deref().unwrap_or("");
    if result.trim().is_empty() {
        out.push_str("（无返回内容）\n");
    } else {
        out.push_str(&truncate_chars(result, DETAIL_CHARS));
        out.push('\n');
    }
    out.push_str(&format!(
        "（存档原文按 {} 上限截断，超出部分不在此处。）",
        crate::db::trace::SPAN_TEXT_MAX
    ));
    out
}

/// 结果节选：单行化 + 截断（清单里一行放得下）。
fn snippet(text: &str) -> String {
    let flat: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut s: String = flat.chars().take(SNIPPET_CHARS).collect();
    if flat.chars().count() > SNIPPET_CHARS {
        s.push('…');
    }
    if s.is_empty() {
        "（无返回内容）".to_string()
    } else {
        s
    }
}

/// 按字符截断（中文按字算，不切碎多字节）。
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}
