//! 轮内工具结果滑动窗的单元测试（裁剪口径 + 护栏 + 一次性）。
//!
//! 四件验收对应四个用例：触发裁剪、护栏不越界、桩子可召回、未超阈值零改写。

use crate::bot_model_loop::{
    apply_tool_window, compact_before_round, tool_result_chars, ModelLoopDeps,
    TOOL_WINDOW_KEEP_RECENT, TOOL_WINDOW_TRIGGER_CHARS,
};
use crate::bot_slash::StopGuard;

/// 造一条「工具调用 → 工具结果」的消息对。
fn tool_pair(id: &str, name: &str, result: &str) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": [{"id": id, "type": "function",
                            "function": {"name": name, "arguments": "{}"}}]
        }),
        serde_json::json!({"role": "tool", "tool_call_id": id, "content": result}),
    ]
}

fn tool_content(m: &serde_json::Value) -> &str {
    m.get("content")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
}

/// ① 超阈值触发裁剪：最近 K 条之外的结果换成桩子。
#[test]
fn window_replaces_old_tool_results_with_stubs() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "干活"})];
    for i in 0..10 {
        msgs.extend(tool_pair(
            &format!("call_{i}"),
            "grep_files",
            &format!("第 {i} 次的结果"),
        ));
    }
    let trimmed = apply_tool_window(&mut msgs, TOOL_WINDOW_KEEP_RECENT);
    assert_eq!(trimmed, 10 - TOOL_WINDOW_KEEP_RECENT, "只裁窗口外的");
    // 最老的一条成了桩子
    assert!(
        tool_content(&msgs[2]).contains("⚠️ 已省略以控制上下文"),
        "老结果应换成桩子：{}",
        tool_content(&msgs[2])
    );
    // 最近 K 条原样
    let last_tool = msgs.len() - 1;
    assert_eq!(tool_content(&msgs[last_tool]), "第 9 次的结果");
}

/// 护栏②：最近 K 条永不裁（即使它们很旧）。
#[test]
fn window_keeps_the_most_recent_results() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "干活"})];
    for i in 0..8 {
        msgs.extend(tool_pair(
            &format!("call_{i}"),
            "read_text_file",
            "很长的结果",
        ));
    }
    apply_tool_window(&mut msgs, TOOL_WINDOW_KEEP_RECENT);
    let contents: Vec<&str> = msgs
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .map(tool_content)
        .collect();
    assert_eq!(contents.len(), 8, "桩子仍是 tool 消息（协议配对不能断）");
    let kept = &contents[contents.len() - TOOL_WINDOW_KEEP_RECENT..];
    assert!(
        kept.iter().all(|c| *c == "很长的结果"),
        "窗口内的结果必须原样：{kept:?}"
    );
    let cut = &contents[..contents.len() - TOOL_WINDOW_KEEP_RECENT];
    assert!(
        cut.iter().all(|c| c.contains("已省略")),
        "窗口外的必须换成桩子：{cut:?}"
    );
}

/// 护栏②：只裁 tool 结果消息，user/assistant 文本一个字节不动。
#[test]
fn window_never_touches_user_or_assistant_text() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "用户的原话"})];
    msgs.extend(tool_pair("call_1", "read_text_file", "结果一"));
    msgs.push(serde_json::json!({"role": "assistant", "content": "模型说的话"}));
    msgs.extend(tool_pair("call_2", "read_text_file", "结果二"));
    msgs.push(serde_json::json!({"role": "assistant", "content": "又说了一句"}));
    msgs.extend(tool_pair("call_3", "read_text_file", "结果三"));
    let texts_before: Vec<serde_json::Value> = msgs
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("tool"))
        .cloned()
        .collect();
    apply_tool_window(&mut msgs, 1);
    let texts_after: Vec<serde_json::Value> = msgs
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("tool"))
        .cloned()
        .collect();
    assert_eq!(texts_before, texts_after, "user/assistant 文本一律不动");
}

/// 护栏④：带图片的消息整体跳过（图片挂在 user 消息的多模态数组上）。
#[test]
fn window_skips_multimodal_messages() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "干活"})];
    msgs.extend(tool_pair("call_1", "screenshot", "结果一"));
    msgs.push(serde_json::json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "〔系统附图〕"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
        ]
    }));
    msgs.extend(tool_pair("call_2", "read_text_file", "结果二"));
    let image_before = msgs[3].clone();
    apply_tool_window(&mut msgs, 0);
    assert_eq!(msgs[3], image_before, "多模态消息不能被碰");
}

/// 桩子带工具名与调用 id，并指路 lookup_receipt。
#[test]
fn stub_carries_tool_name_and_call_id() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "干活"})];
    msgs.extend(tool_pair("call_xyz", "grep_files", "一堆匹配行"));
    apply_tool_window(&mut msgs, 0);
    let stub = tool_content(&msgs[2]);
    assert!(stub.contains("grep_files"), "桩子要写清是哪个工具：{stub}");
    assert!(stub.contains("call_xyz"), "桩子要给出调用 id：{stub}");
    assert!(
        stub.contains("lookup_receipt"),
        "桩子要指路怎么调回原文：{stub}"
    );
}

/// 桩子不带原文（原文只在痕迹表里）。
#[test]
fn stub_drops_the_original_text() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "干活"})];
    msgs.extend(tool_pair(
        "call_1",
        "read_text_file",
        "原始内容不该留在上下文里",
    ));
    apply_tool_window(&mut msgs, 0);
    assert!(!tool_content(&msgs[2]).contains("原始内容"));
}

/// ④ 未超阈值：累计量算出来不到阈值就是零改写（触发判定在调用方，这里测口径）。
#[test]
fn tool_result_chars_counts_only_tool_messages() {
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "0123456789"})];
    msgs.extend(tool_pair("call_1", "read_text_file", "12345"));
    // user 的 10 个字符不计入，tool 的 5 个字符计入
    assert_eq!(tool_result_chars(&msgs), 5);
    assert!(
        tool_result_chars(&msgs) < TOOL_WINDOW_TRIGGER_CHARS,
        "没超阈值就不该触发"
    );
}

/// 触发阈值落在实测 p90 附近（24K 字符 ≈ 9% 的提问会触发）。
#[test]
fn trigger_threshold_is_near_the_measured_p90() {
    assert_eq!(TOOL_WINDOW_TRIGGER_CHARS, 24_000);
    assert_eq!(TOOL_WINDOW_KEEP_RECENT, 6);
}

/// 一次性门不得被「跨阈值但裁无可裁」的轮空消费：首批大结果跨过阈值但
/// 候选 ≤ keep_recent（一条裁不了）时，flag 不应置位——否则同一 trace 里
/// 后续更大的批次永远失去裁剪机会（实机变异 V76 的返工路径）。
#[test]
fn one_shot_gate_survives_a_threshold_crossing_without_trim() {
    let app = tauri::test::mock_app();
    let stop = StopGuard::new(app.handle(), false, None);
    let emit = |_: &str, _: serde_json::Value| {};
    let audit = |_: crate::audit::AuditLevel, _: &str, _: Vec<(&'static str, String)>| {};
    let skill_finish = |_: bool, _: &str| String::new();
    let no_skill_run = |_: Option<&str>| None;
    let on_round = |_: usize| {};
    let no_keywords: Vec<String> = Vec::new();
    let deps = ModelLoopDeps {
        fuse_cap_override: None,
        emit: &emit,
        audit: &audit,
        skill_finish: &skill_finish,
        active_skill_run: &no_skill_run,
        conv_sidecar_dir: None,
        final_selfcheck_round_gap: Some(0),
        final_selfcheck_http: None,
        on_round: &on_round,
        turn_status: None,
        session_tools: None,
        guard_claim_keywords: &no_keywords,
        file_receipts: None,
    };

    let big = "x".repeat(9_000);
    let mut msgs = vec![serde_json::json!({"role": "user", "content": "干活"})];
    for i in 0..3 {
        msgs.extend(tool_pair(&format!("call_{i}"), "read_text_file", &big));
    }
    // 3 条 × 9000 字符 = 27000 > 阈值，但候选 3 ≤ keep_recent：裁无可裁
    let mut flag = false;
    compact_before_round(&mut msgs, &[], &deps, &stop, 0, &mut flag);
    assert!(!flag, "裁无可裁的跨阈值轮不得烧掉一次性门");
    assert!(
        !tool_content(&msgs[2]).contains("已省略"),
        "没裁就不该有桩子"
    );

    // 追加 4 条 → 候选 7 > keep_recent：这一轮必须能启动窗口
    for i in 3..7 {
        msgs.extend(tool_pair(
            &format!("call_{i}"),
            "read_text_file",
            &format!("第 {i} 次的结果"),
        ));
    }
    compact_before_round(&mut msgs, &[], &deps, &stop, 1, &mut flag);
    assert!(flag, "窗口真正裁剪后 flag 才置位");
    assert!(
        tool_content(&msgs[2]).contains("已省略"),
        "最老的结果应被裁成桩子：{}",
        tool_content(&msgs[2])
    );
}
