//! allowed-tools 交互路径闸的行为测试：会话内活动技能声明了 allowed-tools
//! 时，集合外的工具调用被当场拒绝（与 DSL 调度器步骤路径同口径）。
//!
//! 判定打在 `allowed_tools_violation` 纯查询上（mock handle + 真注册表）；
//! 接线（execute_tool_impl 是否真调它、顺序对不对）由源码守卫钉住——
//! 纯函数测试钉不住「闸没接上」这类回归，变异实测反复证明这一点。

use super::*;

/// 注入独立 AppState 的 mock handle（SKILL_RUNS 随 AppState 隔离）
fn test_handle() -> AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    tauri::Manager::manage(&app, crate::app_state::AppState::default());
    app.handle().clone()
}

/// 插入一个带 allowed_tools 的 Running run（挂在指定会话）
fn insert_run(app: &AppHandle<tauri::test::MockRuntime>, session: &str, allowed: &[&str]) {
    let mut run = crate::bot_skills::test_run(8, 180);
    run.name = "limited-skill".into();
    run.state = crate::bot::SkillState::Running;
    run.session_id = Some(session.into());
    run.allowed_tools = allowed.iter().map(|s| s.to_string()).collect();
    let registry = crate::bot_skills::skill_runs(app);
    crate::db::lock::lock_or_recover(&registry, "dispatch::allowed_tools_tests")
        .insert("limited-skill".into(), run);
}

#[test]
fn tool_outside_declared_set_violates() {
    let app = test_handle();
    insert_run(&app, "sess-at", &["read_text_file", "grep_files"]);
    let v =
        allowed_tools_violation(&app, "write_file", Some("sess-at")).expect("集合外工具应命中闸");
    assert_eq!(v.0, "limited-skill");
    assert_eq!(
        v.1,
        vec!["read_text_file".to_string(), "grep_files".to_string()]
    );
    let text = allowed_tools_denial_text(&v.0, &v.1, "write_file");
    assert!(
        text.contains("allowed-tools") && text.contains("write_file"),
        "拒绝文案要点名技能、声明集合与被拒工具：{text}"
    );
    crate::bot_skills::test_remove_skill_run(&app, "limited-skill");
}

#[test]
fn tool_inside_declared_set_passes() {
    let app = test_handle();
    insert_run(&app, "sess-at", &["read_text_file", "grep_files"]);
    assert!(
        allowed_tools_violation(&app, "read_text_file", Some("sess-at")).is_none(),
        "集合内工具不拦"
    );
    crate::bot_skills::test_remove_skill_run(&app, "limited-skill");
}

#[test]
fn empty_allowed_tools_means_unrestricted() {
    let app = test_handle();
    insert_run(&app, "sess-at", &[]);
    assert!(
        allowed_tools_violation(&app, "write_file", Some("sess-at")).is_none(),
        "未声明集合 = 不限制"
    );
    crate::bot_skills::test_remove_skill_run(&app, "limited-skill");
}

#[test]
fn other_sessions_run_does_not_leak_into_this_session() {
    let app = test_handle();
    insert_run(&app, "sess-at", &["read_text_file"]);
    assert!(
        allowed_tools_violation(&app, "write_file", Some("sess-other")).is_none(),
        "别的会话的技能边界不得外溢"
    );
    crate::bot_skills::test_remove_skill_run(&app, "limited-skill");
}

/// 接线守卫：execute_tool_impl 必须真调 `allowed_tools_violation`，且排在
/// per-tool 规则表闸之前（作者自限是更上游的契约）。变异实测反复证明
/// 「纯函数全绿但闸没接线」是常态，这里钉语句存在 + 相对位置。
#[test]
fn allowed_tools_gate_is_wired_before_rule_table() {
    let body = include_str!("../dispatch.rs")
        .split_once("#[cfg(test)]")
        .expect("dispatch.rs 应有测试模块")
        .0;
    let body = body
        .split_once("async fn execute_tool_impl(")
        .expect("找不到 execute_tool_impl")
        .1
        .split("\n}\n")
        .next()
        .expect("execute_tool_impl 应以顶格 } 收尾");
    let gate = body
        .find("allowed_tools_violation(app, name, session_id)")
        .expect("execute_tool_impl 必须调用 allowed-tools 闸");
    let rule_table = body.find("tool_rule_gate(").expect("找不到规则表闸调用点");
    assert!(
        gate < rule_table,
        "allowed-tools 闸必须排在 per-tool 规则表之前（作者契约在上游）"
    );
    assert!(
        body.contains("skill.allowed_tools_violation"),
        "命中必须留审计（与 DSL 调度器路径同一事件名，排查可串线）"
    );
}
