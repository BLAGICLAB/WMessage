//! 工具调用语义结果自检：变更类工具返回成功后让模型用最小 token 自评是否真达成意图。
//!
//! 节流：同一会话内每 5 轮最多一次。Y = 符合预期（静默放行，已 mutation_done 守卫）；
//! N = 未达预期（并入 PREVR 第 1 层「换策略」提示路径，按工具失败处理）。

pub(crate) const SELFCHECK_PROMPT: &str = "\
【自检】上一条工具调用 {tool_name} 返回成功（开头摘要：{snippet}）。\
请仅用一个字回答：Y=符合预期，N=未达预期。回答后不要解释。";

/// 终稿自检：主出口前置触发，让模型对 final_text 做一次「无幻觉/包含幻觉」判定。
/// 节流由 bot_model_loop 控制（设置项 final_selfcheck_round_gap，默认 8 轮一次，
/// 0 = 关闭；trigger 条件 final_text ≥ 80 字 + function_calls ≥ 3 硬编码常量）；
/// N 路径不改写 final_text，只追加 ⚠️ 警告 + 写 bot.log 审计。
/// 复用 parse_selfcheck_response（纯 Y/N 视觉提示同 SELFCHECK）。
pub(crate) const FINAL_SELFCHECK_PROMPT: &str = "\
【终稿自检】本轮回复（开头 500 字）：{snippet}。\
仅回答：Y=无幻觉，N=包含幻觉或虚构事实。回答后不要解释。";

/// final_text 整段 trim 后是否是纯 Y/N 字样。
/// - Some(true) = Y（自检通过）
/// - Some(false) = N（自检失败，按 PREVR 第 1 层失败路径处理）
/// - None = 不是自检回复（或模型附了解释），按正常对话流走
pub(crate) fn parse_selfcheck_response(trimmed: &str) -> Option<bool> {
    match trimmed {
        "Y" | "y" => Some(true),
        "N" | "n" => Some(false),
        _ => None,
    }
}

/// 宽松判定：取首个非空白字符是 Y/N 即认 verdict。终稿自检的 verdict 解析专用
/// ——模型常在单字判定后附加解释（"N，日期写错了"），整段严格匹配会把它当
/// 通过处理，幻觉自白被静默丢弃。空串 / 首字符非 Y/N → None（无法判定）。
/// 工具自检不用此函数：附解释的回复在那条链路里应当作正常答案展示给用户。
pub(crate) fn parse_selfcheck_reply_lenient(trimmed: &str) -> Option<bool> {
    match trimmed.chars().next() {
        Some('Y' | 'y') => Some(true),
        Some('N' | 'n') => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_required_placeholders() {
        assert!(SELFCHECK_PROMPT.contains("{tool_name}"));
        assert!(SELFCHECK_PROMPT.contains("{snippet}"));
    }

    #[test]
    fn prompt_chinese_uses_full_width_punctuation() {
        assert!(SELFCHECK_PROMPT.contains('（'));
        assert!(SELFCHECK_PROMPT.contains('）'));
        assert!(SELFCHECK_PROMPT.contains('：'));
        assert!(SELFCHECK_PROMPT.contains('。'));
    }

    #[test]
    fn prompt_stays_under_one_hundred_chars() {
        // 自检是每 5 轮一次的轻量 token 注入，长度过长会反噬省 token 的初衷
        let len = SELFCHECK_PROMPT.chars().count();
        assert!(
            len < 100,
            "SELFCHECK_PROMPT 过长（{len} 字符）应 < 100，疑似内容漂移"
        );
    }

    #[test]
    fn placeholder_substitution_works() {
        let out = SELFCHECK_PROMPT
            .replace("{tool_name}", "create_task")
            .replace("{snippet}", "taskId=42 title=完成报告");
        assert!(out.contains("create_task"));
        assert!(out.contains("taskId=42"));
        assert!(!out.contains("{tool_name}"));
        assert!(!out.contains("{snippet}"));
    }

    #[test]
    fn final_prompt_contains_required_placeholder() {
        assert!(FINAL_SELFCHECK_PROMPT.contains("{snippet}"));
    }

    #[test]
    fn final_prompt_chinese_uses_full_width_punctuation() {
        // 与 SELFCHECK_PROMPT 同锁：终稿自检也走中文标点规则，前端样式一致
        assert!(FINAL_SELFCHECK_PROMPT.contains('（'));
        assert!(FINAL_SELFCHECK_PROMPT.contains('）'));
        assert!(FINAL_SELFCHECK_PROMPT.contains('：'));
        assert!(FINAL_SELFCHECK_PROMPT.contains('。'));
    }

    #[test]
    fn final_prompt_stays_under_one_hundred_chars() {
        // 自检类 prompt 必须轻量：每 8 轮一次跑、每次 prompt 注入 + 字符增量 ×
        // 长对话的 N 次触发 = 直接放大 token 成本，长度上限锁死
        let len = FINAL_SELFCHECK_PROMPT.chars().count();
        assert!(
            len < 100,
            "FINAL_SELFCHECK_PROMPT 过长（{len} 字符）应 < 100，疑似内容漂移"
        );
    }

    #[test]
    fn final_prompt_placeholder_substitution_works() {
        let out = FINAL_SELFCHECK_PROMPT.replace("{snippet}", "已完成报告生成");
        assert!(out.contains("已完成报告生成"));
        assert!(!out.contains("{snippet}"));
    }

    #[test]
    fn parse_selfcheck_response_y() {
        assert_eq!(parse_selfcheck_response("Y"), Some(true));
        assert_eq!(parse_selfcheck_response("y"), Some(true));
    }

    #[test]
    fn parse_selfcheck_response_n() {
        assert_eq!(parse_selfcheck_response("N"), Some(false));
        assert_eq!(parse_selfcheck_response("n"), Some(false));
    }

    #[test]
    fn parse_selfcheck_response_non_pure_text_returns_none() {
        assert_eq!(parse_selfcheck_response(""), None);
        assert_eq!(parse_selfcheck_response("Y,文件已保存"), None);
        assert_eq!(parse_selfcheck_response(" N "), None); // 没 trim
        assert_eq!(parse_selfcheck_response("Yes"), None);
        assert_eq!(parse_selfcheck_response("Done"), None);
    }

    #[test]
    fn parse_selfcheck_reply_lenient_prefix_y_n() {
        assert_eq!(parse_selfcheck_reply_lenient("Y"), Some(true));
        assert_eq!(parse_selfcheck_reply_lenient("n"), Some(false));
        // 附解释：首字符即 verdict——幻觉自白不能因为不是纯 N 就被丢弃
        assert_eq!(
            parse_selfcheck_reply_lenient("N，报告中把日期写错了"),
            Some(false)
        );
        assert_eq!(
            parse_selfcheck_reply_lenient("Y —— 检查过路径，文件确实存在"),
            Some(true)
        );
        // 无法判定：空串 / 首字符非 Y/N
        assert_eq!(parse_selfcheck_reply_lenient(""), None);
        assert_eq!(parse_selfcheck_reply_lenient("我觉得还行"), None);
        // 首字符带空白：调用方负责 trim（与 parse_selfcheck_response 同约定）
        assert_eq!(parse_selfcheck_reply_lenient(" N "), None);
    }
}
