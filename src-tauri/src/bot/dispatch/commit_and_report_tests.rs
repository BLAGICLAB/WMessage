/// 写库失败时 status 与文本必须给出同一个答案。
///
/// status=Error 是新的成败口径；文本 `{fail_prefix}：{err}` 要能被
/// `classify_tool_failure` 认出，两者才不打架（此前 status=Ok 而文本判失败，
/// 同一个结果两种答案：审计说成功、调度器说失败）。
/// 7 个任务卡工具的 fail_prefix 必须都含「失败」，否则 error_class 落空、
/// 转换历史消息的 Anthropic 层不会设 is_error。
#[test]
fn every_commit_and_report_fail_prefix_is_classifiable() {
    // 逐行扫：进入 commit_and_report( 后，收集形参区里最后一个「独占一行的
    // 字符串字面量」——那就是 fail_prefix（成功/失败消息都是 format! 或
    // 多行闭包，只有 fail_prefix 是单行字面量）
    let mut prefixes: Vec<String> = Vec::new();
    let mut in_call = false;
    let mut pending: Option<String> = None;
    for line in include_str!("../tools/task.rs").lines() {
        let t = line.trim();
        if t.starts_with("commit_and_report(") {
            in_call = true;
            pending = None;
            continue;
        }
        if in_call {
            if t.starts_with(".await") {
                if let Some(p) = pending.take() {
                    prefixes.push(p);
                }
                in_call = false;
                continue;
            }
            if let Some(rest) = t.strip_suffix(',') {
                let s = rest.trim();
                if s.len() > 1 && s.starts_with('"') && s.ends_with('"') {
                    pending = Some(s.trim_matches('"').to_string());
                }
            }
        }
    }
    assert_eq!(
        prefixes.len(),
        7,
        "任务卡工具应恰好 7 处 commit_and_report，实参解析出 {} 个: {prefixes:?}",
        prefixes.len()
    );
    for p in &prefixes {
        assert!(
            p.contains("失败"),
            "fail_prefix 应含「失败」二字（当前 {p:?}）"
        );
        assert!(
            crate::audit::tool_call_failed("complete_task", &format!("{p}：磁盘只读")),
            "fail_prefix 必须能被 classify_tool_failure 认出，否则 status 与文本口径分裂：{p}"
        );
        assert!(
            crate::audit::classify_error_class(&format!("{p}：磁盘只读")).is_some(),
            "error_class 不应为空：{p}"
        );
    }
}

/// 写库失败走 Error 不是 Denied：工具跑过了，是它自己没做完。
#[test]
fn commit_failure_status_is_error_not_denied() {
    // 直接验映射：Error → ERROR 审计；Denied 是「没执行」，语义不同
    assert_eq!(
        crate::audit::AuditLevel::from_tool_status(crate::bot::registry::ToolStatus::Error),
        crate::audit::AuditLevel::Error
    );
    assert_eq!(
        crate::audit::AuditLevel::from_tool_status(crate::bot::registry::ToolStatus::Denied),
        crate::audit::AuditLevel::Warn
    );
}
