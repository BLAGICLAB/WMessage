//! LLM 输出文本小工具：剥离 markdown 代码围栏。
//! 使用方：task_autotag / memory::extract / memory::consolidate——
//! 它们向模型要「裸 JSON」，模型偏爱包一层 ```json 围栏。

/// 剥离首尾代码围栏（```json …```）；无围栏原样返回（已 trim）。
pub(crate) fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // 跳过语言标记行（json / JSON …）
        let rest = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim();
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::strip_fences;

    #[test]
    fn strips_json_fence_and_language_tag() {
        assert_eq!(strip_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fences("```\n[{\"a\":1}]\n```"), "[{\"a\":1}]");
    }

    #[test]
    fn passes_plain_text_through_trimmed() {
        assert_eq!(strip_fences("  {\"a\":1}  "), "{\"a\":1}");
        // 只有头没有尾——不是围栏，原样返回
        assert_eq!(strip_fences("```json\n{\"a\":1}"), "```json\n{\"a\":1}");
    }
}
