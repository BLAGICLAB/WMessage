//! 回合聚合器锚点（真源化批 3 ⑫-⑮）：载荷形状与发射点逐字段一致——
//! 形状漂移即静默丢列，这里用 emit 收口的原文 payload 钉住。

use super::*;

fn sid(tag: &str) -> String {
    format!("tr-{}-{tag}", uuid::Uuid::new_v4())
}

/// 锚点⑫：thinking 逐段拼接；空回合聚合全 None（锚点⑮ 同测）
#[test]
fn think_deltas_join_and_empty_turn_yields_all_none() {
    let s = sid("think");
    begin(&s);
    push_event(
        &s,
        "bot-think-delta",
        &serde_json::json!({ "text": "先想" }),
    );
    push_event(
        &s,
        "bot-think-delta",
        &serde_json::json!({ "text": "再做" }),
    );
    let rec = take(&s);
    assert_eq!(rec.thinking.as_deref(), Some("先想再做"), "逐段拼接");
    assert_eq!(rec.tools_json, None, "没推工具就是 None，不是空数组");
    assert_eq!(rec.file_changes_json, None);
    assert_eq!(
        rec.turn_status, None,
        "没到终态就是 None，不凭空造 completed"
    );

    // 纯空回合（锚点⑮）：begin 后直接 take
    let s2 = sid("empty");
    begin(&s2);
    let rec2 = take(&s2);
    assert_eq!(rec2.thinking, None);
    assert_eq!(rec2.tools_json, None);
    assert_eq!(rec2.file_changes_json, None);
    assert_eq!(rec2.turn_status, None);
}

/// 锚点⑬：running/done 合并出完整工具行（id/name/argsPreview/args/result/ms/ok/done），
/// 保持发射序；无 running 直接 done 的行不凭空造（丢弃）
#[test]
fn tool_events_merge_into_full_rows_in_emission_order() {
    let s = sid("tool");
    begin(&s);
    push_event(
        &s,
        "bot-tool-running",
        &serde_json::json!({ "id": "t1", "name": "read", "args_preview": "/a" }),
    );
    push_event(
        &s,
        "bot-tool-running",
        &serde_json::json!({ "id": "t2", "name": "edit_file", "args_preview": "/b" }),
    );
    push_event(
        &s,
        "bot-tool-done",
        &serde_json::json!({ "id": "t2", "name": "edit_file", "args": "{b}", "result": "ok", "ms": 12, "ok": true }),
    );
    push_event(
        &s,
        "bot-tool-done",
        &serde_json::json!({ "id": "t1", "name": "read", "args": "{a}", "result": "内容", "ms": 3, "ok": true }),
    );
    let rec = take(&s);
    let tools: serde_json::Value =
        serde_json::from_str(rec.tools_json.as_deref().unwrap()).unwrap();
    let arr = tools.as_array().unwrap();
    assert_eq!(arr.len(), 2, "两行工具保持发射序");
    assert_eq!(arr[0]["name"], "read", "顺序 = running 顺序而非 done 顺序");
    assert_eq!(arr[0]["argsPreview"], "/a");
    assert_eq!(arr[0]["args"], "{a}");
    assert_eq!(arr[0]["result"], "内容");
    assert_eq!(arr[0]["ms"], 3);
    assert_eq!(arr[0]["ok"], true);
    assert_eq!(arr[0]["done"], true);
    assert_eq!(arr[1]["ok"], true);
    // 无 running 的孤儿 done：不凭空造行
    let s2 = sid("orphan");
    begin(&s2);
    push_event(
        &s2,
        "bot-tool-done",
        &serde_json::json!({ "id": "x", "name": "n", "args": "", "result": "r", "ms": 1, "ok": false }),
    );
    let rec2 = take(&s2);
    assert_eq!(rec2.tools_json, None, "孤儿 done 不落行");
}

/// 锚点⑭：file-changed 聚合保持发射序，形状 = 前端 FileChangeLite
#[test]
fn file_changes_accumulate_in_order() {
    let s = sid("file");
    begin(&s);
    push_file_change(&s, "/a.md", "create", 3, 0);
    push_file_change(&s, "/b.md", "modify", 1, 2);
    let rec = take(&s);
    let files: serde_json::Value =
        serde_json::from_str(rec.file_changes_json.as_deref().unwrap()).unwrap();
    let arr = files.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["path"], "/a.md");
    assert_eq!(arr[0]["kind"], "create");
    assert_eq!(arr[0]["added"], 3);
    assert_eq!(arr[1]["deleted"], 2);
}

/// 终态最后写者胜（循环唯一出口必发终态）；begin 清掉上一轮残留（锚点⑯ 的
/// 数据面半边：跨轮不得串账）
#[test]
fn begin_clears_stale_record_from_previous_turn() {
    let s = sid("stale");
    begin(&s);
    push_event(
        &s,
        "bot-think-delta",
        &serde_json::json!({ "text": "上一轮的思考" }),
    );
    push_event(
        &s,
        "bot-turn-status",
        &serde_json::json!({ "status": "completed", "detail": "" }),
    );
    // 上一轮异常收尾没 take（崩溃路径）→ 本轮 begin 清场
    begin(&s);
    push_event(
        &s,
        "bot-turn-status",
        &serde_json::json!({ "status": "interrupted", "detail": "" }),
    );
    let rec = take(&s);
    assert_eq!(
        rec.turn_status.as_deref(),
        Some("interrupted"),
        "终态最后写者胜"
    );
    assert_eq!(rec.thinking, None, "上一轮残留被 begin 清掉，不串账");
}
