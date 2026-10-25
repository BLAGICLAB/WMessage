use super::*;

/// create/edit 的 due 参数闸：长度 + 格式一起判。格式口径与 task_patch /
/// HTTP 三写口共用（db::tasks::due_format_error）——schema 曾教空格分隔
/// 格式，照旧传值落库后 due_notify 按「无截止」处理，通知静默不发。
#[test]
fn due_param_denied_rejects_space_format_and_half_typed() {
    for bad in [
        "2026-10-24 09:00",
        "2026-10",
        "2026-02-31",
        "2026-10-24T25:00",
        "明天下午",
    ] {
        let r = due_param_denied(bad).unwrap_or_else(|| panic!("该拒: {bad}"));
        assert!(
            r.text.contains("YYYY-MM-DDTHH:mm"),
            "拒因要点明合法格式：{r:?}"
        );
        // 拒绝回执必须自带「勿丢参/换参重试」指引：底座提示词拦不住紧邻的
        // 下一跳，回执是模型当下唯一盯着的信息
        assert!(
            r.text.contains("丢参或改填别的值重试"),
            "拒因要带行动指引：{r:?}"
        );
    }
}

#[test]
fn due_param_denied_accepts_both_valid_shapes_and_over_length() {
    assert!(due_param_denied("2026-10-24").is_none());
    assert!(due_param_denied("2026-10-24T09:30").is_none());
    let r = due_param_denied("x".repeat(MAX_DUE + 1).as_str()).unwrap();
    assert!(r.text.contains("截止时间"), "超长走长度闸：{r:?}");
}
