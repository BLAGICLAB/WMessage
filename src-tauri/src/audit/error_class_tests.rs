use super::classify_error_class;

/// 成功文本 → None（与 tool_call_failed 同源口径）
#[test]
fn success_text_is_none() {
    assert_eq!(classify_error_class("已标记完成"), None);
    assert_eq!(classify_error_class("已修改 /a/x.py（+3 行）"), None);
    assert_eq!(classify_error_class(""), None);
}

/// 可识别失败模式各归其类
#[test]
fn recognized_patterns_classified() {
    assert_eq!(
        classify_error_class("未知工具 nope，可用工具见列表"),
        Some("unknown_tool")
    );
    assert_eq!(
        classify_error_class("⚠️ 原子工具裸调被拦截"),
        Some("gate_denied")
    );
    assert_eq!(
        classify_error_class("用户拒绝写入该路径"),
        Some("user_denied")
    );
    assert_eq!(
        classify_error_class("技能已暂停，step_check 拒绝"),
        Some("skill_paused")
    );
    assert_eq!(
        classify_error_class("已强制终止（步数超限）"),
        Some("fused")
    );
    assert_eq!(classify_error_class("读取失败：连接超时"), Some("timeout"));
    // 「任务卡不存在」在既有 tool_call_failed 口径下不算失败（模型换策略即可）
    // → 分类器同口径返回 None，不私自扩失败面
    assert_eq!(classify_error_class("任务卡不存在或已在回收站"), None);
    assert_eq!(
        classify_error_class("读取失败：权限不足"),
        Some("permission")
    );
}

/// 未识别失败 → exec_error 兜底
#[test]
fn unrecognized_failure_falls_back() {
    assert_eq!(
        classify_error_class("生成失败, exit code 1"),
        Some("exec_error")
    );
    assert_eq!(classify_error_class("Error: 500"), Some("exec_error"));
}
