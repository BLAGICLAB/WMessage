//! 凭证召回工具单测：查回内容与落库一致 / 查不到给提示 / 只读无副作用。

use super::trace::tool_lookup_receipt;
use crate::db::trace::{NewSpan, NewTrace};

/// 本组用例共用同一份临时库（cargo test 下数据目录旁路到 temp 固定子目录），
/// 并行跑会互撞 SQLite 写锁（建表 DDL 要写锁）。串行化即可——它们本就共享
/// 同一个库文件。
static DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db_guard() -> tokio::sync::MutexGuard<'static, ()> {
    DB_LOCK.lock().await
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// 落一条痕迹 + 一条 span，返回 (trace_id, span_id)。
fn seed(
    conn: &rusqlite::Connection,
    session_id: &str,
    tool_call_id: &str,
    name: &str,
    result: &str,
) -> (i64, i64) {
    let trace_id = crate::db::trace_start(
        conn,
        &NewTrace {
            session_id,
            task_id: None,
            origin: "chat",
            title: None,
            started_at: now_ms(),
        },
    )
    .expect("落 trace");
    let span_id = crate::db::span_insert(
        conn,
        &NewSpan {
            trace_id,
            turn: 2,
            tool_call_id: Some(tool_call_id),
            name,
            args: Some(r#"{"path":"/tmp/a.txt"}"#),
            result: Some(result),
            ok: true,
            error_class: None,
            duration_ms: Some(12),
            created_at: now_ms(),
        },
    )
    .expect("落 span");
    (trace_id, span_id)
}

/// 造一个带数据的 app（mock runtime；cargo test 下数据目录旁路到 temp 固定子目录）
fn mock_app() -> tauri::AppHandle<tauri::test::MockRuntime> {
    let app = Box::leak(Box::new(tauri::test::mock_app()));
    let handle = app.handle().clone();
    std::fs::create_dir_all(crate::db::data_dir(&handle)).unwrap();
    handle
}

/// 调回的内容与落库一致：查得到时给的是原文，不是摘要也不是模型改写
#[tokio::test]
async fn lookup_returns_the_stored_result_verbatim() {
    let _g = db_guard().await;
    let app = mock_app();
    let conn = crate::db::open_db(&app).unwrap();
    let sid = format!("lr-{}", uuid::Uuid::new_v4().simple());
    let (_, span_id) = seed(&conn, &sid, "call_a1", "read_text_file", "第一行\n第二行");

    let out = tool_lookup_receipt(&app, &format!("{{\"span_id\":{span_id}}}"), Some(&sid)).await;
    assert!(out.text.contains("第一行"), "应含落库原文：{}", out.text);
    assert!(out.text.contains("第二行"), "应含落库原文：{}", out.text);
    assert!(
        out.text.contains("read_text_file"),
        "应带工具名：{}",
        out.text
    );
    assert!(
        out.text.contains("call_a1"),
        "应带 tool_call_id：{}",
        out.text
    );

    // 清理
    conn.execute("DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)", [&sid]).ok();
    conn.execute("DELETE FROM exec_traces WHERE session_id = ?1", [&sid])
        .ok();
}

/// 查不到就说查不到：不脑补内容
#[tokio::test]
async fn lookup_missing_span_reports_not_found() {
    let _g = db_guard().await;
    let app = mock_app();
    // 建库（工具自己要开库）；本用例不落数据
    let _conn = crate::db::open_db(&app).unwrap();
    let sid = format!("lr-{}", uuid::Uuid::new_v4().simple());

    let out = tool_lookup_receipt(&app, "{\"span_id\":999999999}", Some(&sid)).await;
    assert!(out.text.contains("查不到"), "应明确说查不到：{}", out.text);
    assert!(
        !out.text.contains("第一行"),
        "查不到时不得给任何像结果的内容：{}",
        out.text
    );
}

/// 会话隔离：别的会话的 span 传进来一律按「查不到」处理，不泄露
#[tokio::test]
async fn lookup_refuses_spans_of_other_sessions() {
    let _g = db_guard().await;
    let app = mock_app();
    let conn = crate::db::open_db(&app).unwrap();
    let mine = format!("lr-{}", uuid::Uuid::new_v4().simple());
    let other = format!("lr-{}", uuid::Uuid::new_v4().simple());
    let (_, span_id) = seed(&conn, &other, "call_x", "read_text_file", "别人的秘密内容");

    let out = tool_lookup_receipt(&app, &format!("{{\"span_id\":{span_id}}}"), Some(&mine)).await;
    assert!(out.text.contains("查不到"), "应按查不到处理：{}", out.text);
    assert!(
        !out.text.contains("别人的秘密内容"),
        "不得泄露别的会话的内容：{}",
        out.text
    );

    conn.execute("DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)", [&other]).ok();
    conn.execute("DELETE FROM exec_traces WHERE session_id = ?1", [&other])
        .ok();
}

/// 清单模式：列出本会话的调用凭证，并带上可精确调取的 id
#[tokio::test]
async fn list_mode_lists_session_receipts_with_ids() {
    let _g = db_guard().await;
    let app = mock_app();
    let conn = crate::db::open_db(&app).unwrap();
    let sid = format!("lr-{}", uuid::Uuid::new_v4().simple());
    let (_, span_id) = seed(&conn, &sid, "call_list1", "grep_files", "命中 3 处");

    let out = tool_lookup_receipt(&app, "{}", Some(&sid)).await;
    assert!(
        out.text.contains("grep_files"),
        "应列出工具名：{}",
        out.text
    );
    assert!(out.text.contains("命中 3 处"), "应带结果节选：{}", out.text);
    assert!(out.text.contains(&span_id.to_string()), "应带 span_id");
    assert!(out.text.contains("call_list1"), "应带 tool_call_id");

    conn.execute("DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)", [&sid]).ok();
    conn.execute("DELETE FROM exec_traces WHERE session_id = ?1", [&sid])
        .ok();
}

/// 没有会话上下文时拒绝：痕迹表是全局的，没有会话就没法限定范围
#[tokio::test]
async fn lookup_without_session_is_denied() {
    let _g = db_guard().await;
    let app = mock_app();
    let out = tool_lookup_receipt(&app, "{}", None).await;
    assert_eq!(out.status, crate::bot::registry::ToolStatus::Denied);
    assert!(
        out.text.contains("会话"),
        "应说明缺会话上下文：{}",
        out.text
    );
}

/// 只读：本工具不写任何表（调用前后痕迹行数不变）
#[tokio::test]
async fn lookup_has_no_write_side_effects() {
    let _g = db_guard().await;
    let app = mock_app();
    let conn = crate::db::open_db(&app).unwrap();
    let sid = format!("lr-{}", uuid::Uuid::new_v4().simple());
    let (_, span_id) = seed(&conn, &sid, "call_ro", "read_text_file", "内容");

    let count_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    tool_lookup_receipt(&app, &format!("{{\"span_id\":{span_id}}}"), Some(&sid)).await;
    tool_lookup_receipt(&app, "{}", Some(&sid)).await;
    let count_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)",
            [&sid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count_before, count_after, "只读工具不得增删 span 行");

    conn.execute("DELETE FROM exec_spans WHERE trace_id IN (SELECT id FROM exec_traces WHERE session_id = ?1)", [&sid]).ok();
    conn.execute("DELETE FROM exec_traces WHERE session_id = ?1", [&sid])
        .ok();
}
