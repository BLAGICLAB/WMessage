//! 用户纠正信号检测（Round 4 学习闭环的入口）。
//!
//! 用户消息含 push-back 模式 → 写 `user.correction` 审计事件到 bot.log；
//! 后续聚类任务读 bot.log 攒同类 → 同类 ≥3 自动 record_lesson。
//!
//! 范围（Commit 1）：仅做事件打点。聚类与自动 record_lesson 留作后续 commit，
//! 不在本模块引入调度/批处理——单点职责，「事件落盘」与「聚合」解耦。
//!
//! 模式选择：白名单多字短语（"错了"/"重做"/"搞反" 等），避单字歧义
//!（"不"/"别" 单独出现命中太宽）。模糊宁可漏检不要误判——审计信号噪
//! 声太大会让聚类阈值失效（见 Round 4 设计）。新增模式须补单测。

/// 推回模式白名单：命中其一即视为用户纠正信号。
/// 多字短语优先；单字歧义词（"不"/"别"）单独出现不在此列。
pub(crate) const CORRECTION_PATTERNS: &[&str] = &[
    "错了",
    "搞错",
    "弄错",
    "重做",
    "重来",
    "搞反",
    "弄反",
    "反了",
    "应该用",
    "应该是",
    "应该改",
    "别用",
    "别这样",
];

/// 命中返回首个命中的模式（按白名单顺序），未命中返回 None。
/// 纯函数、无副作用，便于单测。
pub(crate) fn detect_correction(text: &str) -> Option<&'static str> {
    CORRECTION_PATTERNS
        .iter()
        .find(|&pat| text.contains(pat))
        .copied()
}

/// 写入 `user.correction` 审计事件（Info 级，bot.log 单行追加）。
/// session_id 缺省时记空串（用户从挂件/匿名入口发来的会话常见）。
pub(crate) fn emit_user_correction<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Option<&str>,
    text: &str,
    pattern: &str,
) {
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "user.correction",
        "session" => session_id.unwrap_or(""),
        "pattern" => pattern,
        "text" => text.chars().take(200).collect::<String>(),
    );
}

#[cfg(test)]
mod correction_tests {
    use super::*;

    // 命中：常见 push-back 短语（按白名单顺序命中首个匹配）
    #[test]
    fn detects_unambiguous_push_backs() {
        // "搞错了" 含 "错了" 子串，先命中
        // 公共子串「错了」先匹配（"搞错了"/"弄错了吧" 都含此串）
        assert_eq!(detect_correction("你搞错了，应该是 X"), Some("错了"));
        assert_eq!(detect_correction("弄错了吧"), Some("错了"));
        // 公共子串「反了」先匹配（"弄反了" 含此串）
        assert_eq!(detect_correction("做反了，从头再来"), Some("反了"));
        assert_eq!(detect_correction("重做一次吧"), Some("重做"));
        // "不是" 单独不在白名单（中文里太宽，问句/否定都会命中）
        assert_eq!(detect_correction("用 Y，不是 X"), None);
        assert_eq!(detect_correction("别用 X 了"), Some("别用"));
        assert_eq!(detect_correction("应该是 X 才对"), Some("应该是"));
    }

    // 不命中：常见中性消息
    #[test]
    fn does_not_match_neutral_messages() {
        assert_eq!(detect_correction("帮我创建一个任务"), None);
        assert_eq!(detect_correction("查询一下今天的天气"), None);
        assert_eq!(detect_correction("你说的对不对？"), None); // 问句不视为纠正
        assert_eq!(detect_correction(""), None);
    }

    // 白名单与单字避歧义：单字 "不"/"别" 不在列
    #[test]
    fn single_char_avoided_to_reduce_false_positives() {
        for s in ["不要", "别说了", "不行啊", "不能这样"] {
            // 单字歧义短语不应命中（宁可漏检）
            assert_eq!(detect_correction(s), None, "{s} 不应误判");
        }
    }
}
