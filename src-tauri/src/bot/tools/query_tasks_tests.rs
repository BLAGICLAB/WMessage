// query 工具族纯函数测试

use super::*;

fn task_with(
    column: TaskStatus,
    archived: Option<bool>,
    deleted_at: Option<i64>,
    title: &str,
) -> crate::db::Task {
    crate::db::Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: format!("id-{title}"),
        title: title.to_string(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column,
        subtasks: None,
        completed_at: None,
        created_at: None,
        archived,
        deleted_at,
        collapsed: None,
        order: None,
        updated_at: None,
        bot_assigned: None,
        assignee: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,
        expected_updated_at: None,
    }
}

#[test]
fn parse_view_defaults_follow_query_presence() {
    // 自动档：无 query=Active 清单；有 query=All 全库检索（对齐原 list/search 口径）
    assert_eq!(parse_view(None, false), TaskView::Active);
    assert_eq!(parse_view(None, true), TaskView::All);
    assert_eq!(parse_view(Some("active"), false), TaskView::Active);
    assert_eq!(parse_view(Some("done"), true), TaskView::Done);
    assert_eq!(parse_view(Some("archived"), false), TaskView::Archived);
    assert_eq!(parse_view(Some("trash"), true), TaskView::Trash);
    assert_eq!(parse_view(Some("all"), false), TaskView::All);
    // 非法值 fail-soft 回退 Active
    assert_eq!(parse_view(Some("bogus"), true), TaskView::Active);
}

#[test]
fn view_keep_partitions_four_card_states() {
    let active = task_with(TaskStatus::Todo, None, None, "a");
    let done = task_with(TaskStatus::Done, Some(false), None, "b");
    let archived = task_with(TaskStatus::Done, Some(true), None, "c");
    let trashed = task_with(TaskStatus::Todo, None, Some(1), "d");
    // Active：未完成未归档未删（原 active_tasks 口径）
    assert!(view_keep(TaskView::Active, &active));
    assert!(!view_keep(TaskView::Active, &done));
    assert!(!view_keep(TaskView::Active, &archived));
    assert!(!view_keep(TaskView::Active, &trashed));
    // Done：已完成未归档未删
    assert!(view_keep(TaskView::Done, &done));
    assert!(!view_keep(TaskView::Done, &archived));
    // Archived：已归档未删（跨状态列）
    assert!(view_keep(TaskView::Archived, &archived));
    assert!(!view_keep(TaskView::Archived, &done));
    // Trash：仅软删
    assert!(view_keep(TaskView::Trash, &trashed));
    assert!(!view_keep(TaskView::Trash, &active));
    // All：除回收站外全部
    assert!(view_keep(TaskView::All, &active));
    assert!(view_keep(TaskView::All, &done));
    assert!(view_keep(TaskView::All, &archived));
    assert!(!view_keep(TaskView::All, &trashed));
}

#[test]
fn keyword_hit_matches_title_note_tag_subtask() {
    let mut t = task_with(TaskStatus::Todo, None, None, "季度汇报 PPT");
    assert!(keyword_hit("汇报", &t));
    assert!(!keyword_hit("不存在词xyz", &t));
    t.note = Some("引用了 Q3 数据".to_string());
    assert!(keyword_hit("q3", &t));
    t.tags = Some(vec!["工作".to_string(), "周报".to_string()]);
    assert!(keyword_hit("周报", &t));
    t.subtasks = Some(vec![crate::db::Subtask {
        id: "s1".into(),
        text: "核对配色变量".into(),
        done: false,
        task_id: None,
    }]);
    assert!(keyword_hit("配色", &t));
    // 空关键词恒命中（清单模式）
    assert!(keyword_hit("", &t));
}

#[test]
fn tag_keep_is_exact_case_insensitive() {
    let mut t = task_with(TaskStatus::Todo, None, None, "a");
    t.tags = Some(vec!["Work".to_string()]);
    assert!(tag_keep("work", &t));
    assert!(!tag_keep("wor", &t), "标签过滤是精确匹配，不是 contains");
    assert!(tag_keep("", &t), "空标签恒命中");
}

#[test]
fn parse_limit_defaults_and_clamps() {
    assert_eq!(parse_limit(&serde_json::json!({})), 50);
    assert_eq!(parse_limit(&serde_json::json!({"limit": 10})), 10);
    assert_eq!(parse_limit(&serde_json::json!({"limit": 0})), 1);
    assert_eq!(parse_limit(&serde_json::json!({"limit": 9999})), 200);
}

#[test]
fn locate_subtask_prefers_id_falls_back_to_text() {
    let subs = vec![
        crate::db::Subtask {
            id: "sub-a".into(),
            text: "买牛奶".into(),
            done: false,
            task_id: None,
        },
        crate::db::Subtask {
            id: "sub-b".into(),
            text: "买面包".into(),
            done: true,
            task_id: None,
        },
    ];
    // id 精确优先（即使 text 关键词也能命中别的条）
    assert_eq!(locate_subtask(&subs, Some("sub-b"), Some("牛奶")), Ok(1));
    // 无 id 走文本 contains
    assert_eq!(locate_subtask(&subs, None, Some("奶")), Ok(0));
    // id 未命中：报 id 不存在（不静默回落文本，防误伤别的条）
    assert!(locate_subtask(&subs, Some("sub-x"), Some("奶")).is_err());
    // 两者都缺 / 空串
    assert!(locate_subtask(&subs, None, None).is_err());
    assert!(locate_subtask(&subs, Some("  "), None).is_err());
    assert!(locate_subtask(&subs, None, Some("  ")).is_err());
}

#[test]
fn render_task_line_basic_shape() {
    let t = task_with(TaskStatus::Doing, Some(true), None, "写周报");
    let line = render_task_line(&t);
    assert!(line.contains("[进行中]"), "{line}");
    assert!(line.contains("（已归档）"), "{line}");
    assert!(line.contains("（id=id-写周报）"), "{line}");
}
