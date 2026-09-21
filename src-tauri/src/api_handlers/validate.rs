//! API handler 层的输入校验工具：把「trim + 长度校验」这套在
//! create_task / update_task 重复 5 次的模板抽成纯函数。

use super::util::over_limit;

/// trim 输入字符串。None / 全空白返回 None；否则返回 trim 后的 Owned。
pub(crate) fn take_trimmed_string(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}

/// 一次性 trim + 长度校验。返回 `Some(err_msg)` 当 trim 后非空且长度超
/// `max`，否则 `None`。行为与原 handlers.rs 中 5 处模板 1:1 等价：
/// - 输入 None / 全空白 → 不校验，返回 None（不是错误）
/// - 输入 trim 后非空 → 检查长度，超限返回 `over_limit` 的错误文案
///
/// 调用方拿到 `Some(e)` 通常走 `req.respond(json_err(400, &e)); return;`。
pub(crate) fn check_field(raw: Option<&str>, max: usize, label: &str) -> Option<String> {
    // 不经过 take_trimmed_string(to_owned 一次),直接在 borrow view 上跑
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    over_limit(trimmed, max, label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_trimmed_none_returns_none() {
        assert_eq!(take_trimmed_string(None), None);
    }

    #[test]
    fn take_trimmed_empty_returns_none() {
        assert_eq!(take_trimmed_string(Some("")), None);
    }

    #[test]
    fn take_trimmed_whitespace_only_returns_none() {
        assert_eq!(take_trimmed_string(Some("   \t\n")), None);
    }

    #[test]
    fn take_trimmed_preserves_inner_content() {
        assert_eq!(take_trimmed_string(Some("  hello  ")).unwrap(), "hello");
    }

    #[test]
    fn check_field_skips_when_empty() {
        // None / 全空白不应报错
        assert_eq!(check_field(None, 10, "备注"), None);
        assert_eq!(check_field(Some(""), 10, "备注"), None);
        assert_eq!(check_field(Some("   "), 10, "备注"), None);
    }

    #[test]
    fn check_field_skips_when_within_limit() {
        assert_eq!(check_field(Some("hi"), 10, "备注"), None);
    }

    #[test]
    fn check_field_returns_err_when_over_limit() {
        // max=2, 输入 3 字符 → 报错
        let e = check_field(Some("abc"), 2, "备注").unwrap();
        assert!(e.contains("备注"), "error should mention label: {e}");
        assert!(e.contains("2"), "error should mention max: {e}");
    }

    #[test]
    fn check_field_counts_chars_not_bytes() {
        // 中文 1 字 = 1 char; 3 中文字符对 max=2 应该超限
        let e = check_field(Some("你好世"), 2, "备注").unwrap();
        assert!(e.contains("备注"));
    }

    /// 边界用例:`chars().count() == max` 不得触发超限,
    /// 防 over_limit 从 `>` 退化成 `>=` 的 off-by-one 回归
    #[test]
    fn check_field_len_equals_max_is_within_limit() {
        // max=3, 输入 3 字符 → 不超限(None)
        assert_eq!(check_field(Some("abc"), 3, "备注"), None);
    }

    /// 紧邻边界:`chars().count() == max + 1` 必须触发超限,
    /// 防 off-by-one 回归
    #[test]
    fn check_field_len_equals_max_plus_one_is_over_limit() {
        // max=3, 输入 4 字符 → 超限(Some)
        assert!(check_field(Some("abcd"), 3, "备注").is_some());
    }
}
