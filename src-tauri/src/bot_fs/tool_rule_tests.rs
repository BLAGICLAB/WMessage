use super::*;
use crate::bot::config::types::{PermMode, ToolRule};

fn rule(tool: &str, action: &str) -> ToolRule {
    ToolRule {
        tool: tool.into(),
        action: action.into(),
    }
}

/// 评估序：deny/ask/allow 各自命中；同工具多规则**首中即停**
#[test]
fn lookup_first_match_wins() {
    let rules = vec![rule("edit_file", "allow"), rule("edit_file", "deny")];
    assert_eq!(
        lookup_tool_rule(Some(&rules), "edit_file"),
        Some(ToolRuleAction::Allow),
        "首中即停：allow 在前按 allow"
    );
    let rules = vec![rule("run_python", "deny"), rule("run_python", "allow")];
    assert_eq!(
        lookup_tool_rule(Some(&rules), "run_python"),
        Some(ToolRuleAction::Deny)
    );
}

/// 无命中/无表/非法 action → None（走全局 perm_mode 兜底）
#[test]
fn lookup_miss_and_dirty_data() {
    assert_eq!(lookup_tool_rule(None, "edit_file"), None);
    assert_eq!(lookup_tool_rule(Some(&[]), "edit_file"), None);
    let rules = vec![rule("web_search", "ask")];
    assert_eq!(lookup_tool_rule(Some(&rules), "edit_file"), None);
    // 脏数据：同工具首条 action 非法 → 视为无命中（清洗层已防，双保险）
    let rules = vec![rule("edit_file", "bogus")];
    assert_eq!(lookup_tool_rule(Some(&rules), "edit_file"), None);
}

/// 退役档解析："strict"/"auto" 与非法/缺省一律回退 Ask（安全默认不变）
#[test]
fn perm_mode_retired_values_fall_back_to_ask() {
    assert_eq!(PermMode::from_cfg(Some("strict")), PermMode::Ask);
    assert_eq!(PermMode::from_cfg(Some("auto")), PermMode::Ask);
    assert_eq!(PermMode::from_cfg(Some(" auto ")), PermMode::Ask);
    assert_eq!(PermMode::from_cfg(Some("bogus")), PermMode::Ask);
    assert_eq!(PermMode::from_cfg(None), PermMode::Ask);
    assert_eq!(PermMode::Ask.as_str(), "ask");
}
