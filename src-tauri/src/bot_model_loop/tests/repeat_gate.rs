//! 工具重复调用闸：指纹规范化、连击状态机、豁免表与接线守卫。

use super::*;
use crate::bot::constants::REPEAT_GATE_MAX_IDENTICAL;

/// 指纹按规范化 JSON 计算：键序不同的同参调用算同一次调用。
#[test]
fn fingerprint_ignores_key_order() {
    let a = call_fingerprint("write_file", r#"{"path":"x","content":"y"}"#);
    let b = call_fingerprint("write_file", r#"{"content":"y","path":"x"}"#);
    assert_eq!(a, b, "键序不同的同参调用应算同一次调用");
}

/// 指纹必须同时区分工具与参数：同工具不同参数、不同工具同参数都算不同调用。
#[test]
fn fingerprint_separates_tool_and_args() {
    assert_ne!(
        call_fingerprint("write_file", r#"{"path":"a"}"#),
        call_fingerprint("write_file", r#"{"path":"b"}"#),
        "同工具不同参数应算不同调用"
    );
    assert_ne!(
        call_fingerprint("write_file", r#"{"path":"a"}"#),
        call_fingerprint("edit_file", r#"{"path":"a"}"#),
        "不同工具同参数应算不同调用"
    );
}

/// 参数不是合法 JSON 时按原文串哈希：原文相同算同一次调用。
#[test]
fn fingerprint_falls_back_to_raw_string_on_invalid_json() {
    let a = call_fingerprint("mcp__x__tool", "{not json");
    assert_eq!(
        a,
        call_fingerprint("mcp__x__tool", "{not json"),
        "同样的坏参数应算同一次调用"
    );
    assert_ne!(
        a,
        call_fingerprint("mcp__x__tool", "{not json "),
        "原文不同应算不同调用"
    );
}

/// 连击状态机：第 3 次相同调用到达上限（与常量同钉，防两边漂移）。
#[test]
fn gate_trips_on_third_identical_call() {
    assert_eq!(REPEAT_GATE_MAX_IDENTICAL, 3, "业界收敛值是 3（连击口径）");
    let mut st = RepeatGateState::new();
    assert_eq!(st.observe("web_search", r#"{"query":"q"}"#), 1);
    assert_eq!(st.observe("web_search", r#"{"query":"q"}"#), 2);
    assert_eq!(st.observe("web_search", r#"{"query":"q"}"#), 3);
}

/// 状态机：参数一变或工具一换即重新计数——闸只拦「原地打转」，
/// 不拦「换了参数仍在同一方向推进」的正常迭代。
#[test]
fn gate_resets_on_different_call() {
    let mut st = RepeatGateState::new();
    st.observe("web_search", r#"{"query":"q"}"#);
    st.observe("web_search", r#"{"query":"q"}"#);
    assert_eq!(
        st.observe("web_search", r#"{"query":"q2"}"#),
        1,
        "参数一变即重新计数"
    );
    assert_eq!(
        st.observe("edit_file", r#"{"path":"q"}"#),
        1,
        "换工具也重新计数"
    );
}

/// 豁免表语义：纯读/轮询/探测类不拦，副作用与出网类默认受闸。
///
/// submit_result 不需要豁免——它是终局提交工具，第一次成功即结束回合，
/// 不存在连击面。
#[test]
fn exempt_table_separates_reads_from_effectful_tools() {
    for name in [
        "ask_user",
        "check_subagent",
        "wait_for_element",
        "screenshot",
        "read_text_file",
        "grep_files",
        "lookup_receipt",
        "query_tasks",
        "list_windows",
    ] {
        assert!(is_exempt_tool(name), "轮询/纯读工具必须豁免：{name}");
    }
    for name in [
        "web_search",
        "fetch_url",
        "http_request",
        "run_python",
        "write_file",
        "edit_file",
        "ui_click",
        "spawn_subagent",
        "create_word",
    ] {
        assert!(!is_exempt_tool(name), "副作用/出网工具必须受闸：{name}");
    }
}

/// 拒绝文案必须自带出路：点名工具、连击数、未执行事实与禁令。
#[test]
fn reject_text_names_tool_and_gives_a_way_out() {
    let msg = reject_text("web_search", 3);
    assert!(msg.contains("web_search"), "文案应点名工具：{msg}");
    assert!(msg.contains("3 次"), "文案应带连击数：{msg}");
    assert!(msg.contains("未执行"), "文案应说明本次未执行：{msg}");
    assert!(msg.contains("不得再以相同参数调用"), "文案应带禁令：{msg}");
}

/// 接线守卫：拦截点必须在收尾窗拒绝之后、计数/执行之前——被拦调用不得
/// 消耗预算（照抄收尾窗「只计实际执行」的口径），且闸拒绝分支不得触碰
/// consec_failures（不进失败链，避免与 fail_hint 双重提示）。
///
/// 与 post_turn 那条守卫同款理由：模块的纯函数测试验不到调用点是否存在、
/// 位置是否正确——把接线删掉或挪到计数之后，纯函数测试照样全绿。
#[test]
fn repeat_gate_is_wired_between_wrap_up_and_counted() {
    let src = prod_file_with("repeat_gate.observe");
    let gate = src
        .find("repeat_gate.observe")
        .expect("模型循环必须调用重复闸 observe");
    let counted = src
        .find("if gate == CallGate::Counted {")
        .expect("找不到计数点");
    assert!(
        gate < counted,
        "重复闸必须拦在计数之前：被拦调用不得消耗预算"
    );
    let block = &src[gate..counted];
    assert!(
        !block.contains("consec_failures"),
        "闸拒绝不得计入失败链（避免与 fail_hint 双重提示）"
    );
    assert!(
        src.contains("\"repeat_gate\""),
        "闸触发必须落审计事件 repeat_gate"
    );
}
