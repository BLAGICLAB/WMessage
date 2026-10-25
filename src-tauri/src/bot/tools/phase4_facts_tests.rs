use super::*;

#[test]
fn get_current_time_format_has_date_and_weekday() {
    let result = tool_get_current_time();
    let refs = &result.refs;
    let text = &result.text;
    assert!(refs.is_empty());
    assert!(text.starts_with("现在："), "实际：{text}");
    assert!(
        [
            "星期一",
            "星期二",
            "星期三",
            "星期四",
            "星期五",
            "星期六",
            "星期日"
        ]
        .iter()
        .any(|w| text.contains(w)),
        "应含中文星期，实际：{text}"
    );
}
