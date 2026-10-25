// 测试：BotConfig 序列化 + extract_document 输出格式化

//! BotConfig 序列化与默认值单测

use super::*;

/// 工具入参口的 count 必须与配置侧同一条钳制边界。
///
/// 钳制曾在这里手写第三份 `clamp(1, 10)`：写侧落盘、读侧 resolver 各一份，
/// 工具入参又一份。改 `SEARCH_MAX_RESULTS_CAP` 而漏改这里不会编译失败，
/// 只会让「设置页显示硬顶 10、模型传 12 就真给 12 条」。
#[test]
fn search_count_follows_the_params_clamp() {
    let cap = crate::bot::params::SEARCH_MAX_RESULTS_CAP;
    assert_eq!(
        parse_search_count(&serde_json::json!(0)),
        Some(1),
        "0 钳到 1"
    );
    assert_eq!(
        parse_search_count(&serde_json::json!(5)),
        Some(5),
        "区间内放行"
    );
    assert_eq!(
        parse_search_count(&serde_json::json!(99)),
        Some(cap),
        "越界钳到 params 的硬顶"
    );
    assert_eq!(
        parse_search_count(&serde_json::json!(u64::from(u32::MAX) + 1)),
        Some(cap),
        "超 u32 收口到硬顶，不得按截断值放行"
    );
    assert_eq!(
        parse_search_count(&serde_json::json!("5")),
        None,
        "非整数视作未给"
    );
    assert_eq!(
        parse_search_count(&serde_json::Value::Null),
        None,
        "缺省走配置解析"
    );
}
