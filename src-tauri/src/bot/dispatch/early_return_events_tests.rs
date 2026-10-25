use super::*;

type AuditEvent = (
    crate::audit::AuditLevel,
    &'static str,
    Vec<(&'static str, String)>,
);

fn event_names(evs: &[AuditEvent]) -> Vec<&'static str> {
    evs.iter().map(|(_, e, _)| *e).collect()
}

fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
    kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
}

#[test]
fn rule_gate_deny_path_emits_deny_then_balanced_tool_return() {
    // 规则闸拦截路径：pre_execute.deny → tool.return（顺序敏感），
    // reason=denied_by_rule / exit_code=none / duration_ms，事件带 trace 可整轮回放
    let trace = ToolCallTrace {
        turn: Some(2),
        tool_call_id: Some("call_abc".into()),
    };
    let evs = early_return_events(
        "create_word_revisions",
        "denied_by_rule",
        3,
        None,
        Some("sess-1"),
        &trace,
    );
    let names: Vec<&str> = evs.iter().map(|(_, e, _)| *e).collect();
    assert_eq!(names, ["pre_execute.deny", "tool.return"]);
    assert!(evs
        .iter()
        .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
    assert_eq!(
        evs.iter()
            .find(|(_, e, _)| *e == "pre_execute.deny")
            .and_then(|(_, _, kv)| kv.iter().find(|(k, _)| *k == "tool"))
            .map(|(_, v)| v.as_str()),
        Some("create_word_revisions")
    );
    for ev in &evs {
        let kv = &ev.2;
        assert_eq!(
            kv.iter()
                .find(|(k, _)| *k == "session_id")
                .map(|(_, v)| v.as_str()),
            Some("sess-1")
        );
        assert_eq!(
            kv.iter()
                .find(|(k, _)| *k == "turn")
                .map(|(_, v)| v.as_str()),
            Some("2")
        );
    }
}

#[test]
fn skill_step_error_path_warns_then_balanced_tool_return() {
    // 熔断路径：skill_on_step_error（补 Warn 可见性）→ tool.return(reason=skill_step_failed)
    let evs = early_return_events(
        "run_python",
        "skill_step_failed",
        5,
        Some("超过最大步数上限（8 步）"),
        None,
        &ToolCallTrace::default(),
    );
    assert_eq!(event_names(&evs), ["skill_on_step_error", "tool.return"]);
    assert!(evs
        .iter()
        .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
    assert_eq!(kv_get(&evs[0].2, "err"), Some("超过最大步数上限（8 步）"));
    let ret = &evs[1].2;
    assert_eq!(kv_get(ret, "reason"), Some("skill_step_failed"));
    assert_eq!(kv_get(ret, "exit_code"), Some("none"));
    assert!(kv_get(ret, "duration_ms").is_some());
}

// ── trace_kv：有值才写，无会话上下文不产生误导性 turn=0 ──

#[test]
fn trace_kv_omits_absent_turn_and_tool_call_id() {
    let kv = trace_kv(&ToolCallTrace::default(), None);
    assert_eq!(kv_get(&kv, "session_id"), Some("-"));
    assert!(kv_get(&kv, "turn").is_none(), "无 turn 不应写 turn=0");
    assert!(kv_get(&kv, "tool_call_id").is_none());
}

#[test]
fn trace_kv_writes_present_fields_and_blank_ids_fall_back_to_dash() {
    let kv = trace_kv(
        &ToolCallTrace {
            turn: Some(0),
            tool_call_id: Some(String::new()),
        },
        Some(""),
    );
    assert_eq!(kv_get(&kv, "session_id"), Some("-"), "空会话按 - 记");
    assert_eq!(kv_get(&kv, "turn"), Some("0"), "第 0 轮是有效值");
    assert!(
        kv_get(&kv, "tool_call_id").is_none(),
        "空 id 不写（合成 id 前的畸形流不该出现空值行）"
    );
}
