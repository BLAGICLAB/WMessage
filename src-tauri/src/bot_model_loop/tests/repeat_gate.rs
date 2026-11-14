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

/// 转问草稿必须在引擎参数红线内且三件齐：三选项、最保守假设（无人应答
/// 按「就此打住」收尾）、问题点名工具与连击数。
#[test]
fn ask_draft_fits_engine_limits_and_carries_assumption() {
    let d = ask_draft("http_request", 3);
    assert!(
        d.question.chars().count() <= 200 && d.why.chars().count() <= 100,
        "question/why 必须在引擎红线内：{} / {}",
        d.question,
        d.why
    );
    assert_eq!(d.options.len(), 3, "选项给三条：换方法 / 再试 / 打住");
    for o in &d.options {
        assert!(o.chars().count() <= 40, "选项超 40 字：{o}");
    }
    assert!(
        d.assumption.chars().count() <= 200 && d.assumption.contains("就此打住"),
        "假设必须是最保守的「就此打住」：{}",
        d.assumption
    );
    assert!(d.question.contains("http_request") && d.question.contains("3 次"));
}

/// 应答回填文案透传用户裁决；回落文案与拒文同向（换路或收尾）。
#[test]
fn answered_and_fallback_texts_carry_the_verdict_or_way_out() {
    let a = answered_text("web_search", 3, "用户回答了你的提问：换个关键词再搜");
    assert!(a.contains("换个关键词再搜"), "裁决应原样透传：{a}");
    assert!(a.contains("请按裁决继续"), "应指示按裁决继续：{a}");
    let f = fallback_text("web_search", 3);
    assert!(
        f.contains("未获应答") && f.contains("不得再以相同参数调用"),
        "回落文案应说明未获应答并带禁令：{f}"
    );
}

/// 每回合至多一问；应答后重置连击（答「继续」时重试必须放行）。
#[test]
fn ask_once_per_turn_and_answer_resets_streak() {
    let mut st = RepeatGateState::new();
    assert!(st.can_ask(), "新回合应可转问");
    st.mark_ask_used();
    assert!(!st.can_ask(), "问过之后本回合不得再问");
    st.observe("web_search", r#"{"query":"q"}"#);
    st.observe("web_search", r#"{"query":"q"}"#);
    st.observe("web_search", r#"{"query":"q"}"#);
    st.reset_tracking();
    assert_eq!(
        st.observe("web_search", r#"{"query":"q"}"#),
        1,
        "应答重置后同参重试应从 1 重新计数"
    );
}

/// 接线守卫（转问分支）：闸块必须先尝试转问（出口在场且本回合未问过才发
/// 起，且在征询前置位 ask_used），用户应答（Ok）重置连击、其余回落拒文；
/// 三种 action 键（answered / fallback / reject）必须都存在。
#[test]
fn ask_branch_is_wired_in_the_gate_block() {
    let src = prod_file_with("repeat_gate.observe");
    let gate = src.find("repeat_gate.observe").expect("找不到闸块");
    let block = &src[gate..];
    assert!(
        block.contains("match deps.repeat_ask") && block.contains("if repeat_gate.can_ask()"),
        "闸块必须先尝试转问（出口在场且本回合未问过）"
    );
    let ask = block
        .find("mark_ask_used")
        .expect("转问前必须先置位 ask_used（每回合至多一问）");
    let call = block
        .find("ask_draft(name, consec)")
        .expect("找不到转问草稿构造");
    assert!(ask < call, "置位必须在发起征询之前");
    assert!(
        block.contains("reset_tracking"),
        "用户应答后必须重置连击（「继续再试一次」要能放行）"
    );
    for action in ["answered", "fallback", "reject"] {
        assert!(
            block.contains(&format!("\"{action}\"")),
            "action 键缺 {action}：审计分不了支"
        );
    }
}

/// 接线守卫（薄壳出口）：转问闭包必须闭在薄壳里直调 ask_engine，等待用
/// 闸的短超时常量（不挂 ask_user 的 24h）。
#[test]
fn repeat_ask_closure_lives_in_shell_and_uses_short_timeout() {
    // needle 必须唯一指向薄壳：core 也有 repeat_ask 形参，用 engine_ask_user
    // （只有薄壳直调引擎）锁定文件。
    let src = prod_file_with("engine_ask_user");
    assert!(
        src.contains("engine_ask_user"),
        "转问出口必须直调 ask_engine"
    );
    assert!(
        src.contains("REPEAT_GATE_ASK_TIMEOUT_SECS"),
        "转问等待必须用闸的短超时常量，不得复用 ask_user 的 24h"
    );
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
