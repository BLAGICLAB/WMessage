//! 共享的日志转义 + 字符级截断 helper（2026-10-14 收敛点）。
//!
//! 历史：8 份具名实现 + 同名 `escape_for_log` 行为分叉（audit.rs 弱版 vs
//! bot/config/audit.rs 强版，调用方按文件随机吃到哪个），加上 paths.rs 第
//! 三份内联拷贝 + api_handlers/ratelimit.rs 第四份 `sanitize_log_line`
//! + memory/evolution 同名 `truncate_chars` 不同签名（有无省略号）。
//! 详见冗余审计账本（docs/ 下 REDUNDANCY 前缀文件）。
//!
//! 此模块收敛到两件事：
//! - [`escape_for_log`]：单源强版（控制字符全集 + U+2028/2029 + `|` 倍增 +
//!   `| ` 双空格补位 + 字符数截断 + 省略号）
//! - [`truncate_chars`]：单签名 `truncate_chars(s, n, ellipsis)`，按字符
//!   计数（中文友好），可选追加 `…`。
//!
//! 调用方：
//! - `crate::bot::log_sanitize::escape_for_log` / `truncate_chars`
//! - 通过 `crate::bot::log_sanitize` 命名空间使用；旧路径
//!   `crate::bot::escape_for_log` / `crate::bot::truncate_for_log`
//!   仍由 `bot.rs` facade 转出，无外部改动。
//!
//! weak → strong 行为变更要点（已写进 commit message）：
//! - `\0` / 其他控制字符 / `ESC` / `BEL` / `DEL` / C1 区 / U+2028 / U+2029：
//!   弱版悄悄不处理 → 强版以 `\u{N}` 显式转义，避免伪造日志行的同时也避免
//!   截断后误以为是省略号吞掉的内容。这是审计修复，非纯重构。

/// 审计日志安全转义 + 字符级截断（bot 侧日志写入统一出口）：
/// 剥换行 / 管道符 / 控制字符，防伪造「INFO |」前缀与多行撕裂，
/// 同时规避终端注入序列（ESC / BEL 等）。
///
/// 规则（按字符臂逐个匹配）：
/// - `|` → `||`，前置 `"| "` → "|  " 预替换再走字符臂保持既有的 `|` 倍增 + 空格补位
/// - `\n` → `\\n`，`\r` → `\\r`
/// - U+2028 / U+2029 → `\u{2028}` / `\u{2029}`（行 / 段分隔符，多数行导向工具视为换行）
/// - 其余控制字符（U+0000-U+001F（除 \n/\r 已处理）+ U+0080-U+009F C1 区）→ `escape_default` 输出 `\u{N}`
/// - 普通字符（CJK / ASCII 可打印 / 中文标点）原样保留
///
/// 转义后按 **字符数** 截到 `max_chars` 加省略号：`max_chars` 含省略号位
/// （即实际返回长度 = `min(count, max_chars + 1)`）。中文一字 3 字节，按字计算才一致。
///
/// 注意：弱版（仅 `\n\r|`）的 4 个既有用例现已自动覆盖：
/// - `"a\nb| c"` → `"a\\nb||  c"`
/// - `"x\ry"` → `"x\\ry"`
/// - `"plain"` → `"plain"`
/// - `"evil\n[2026-01-01 00:00:00] INFO | fake"` → 转义同理
/// 弱版的 `escape_for_log` 单测在 `audit.rs` 仍命中（可作为回归锁）。
pub fn escape_for_log(s: &str, max_chars: usize) -> String {
    // 「| 」 → 「|  」：保留弱版「双空格补位」的表格列语义；字符臂下 `|`→`||`
    // 自身已覆盖 99% 场景，"| "→"|  " 仅针对弱版两空格补位的历史行为
    let mut escaped = String::with_capacity(s.len() + 8);
    for ch in s.replace("| ", "|  ").chars() {
        match ch {
            '|' => escaped.push_str("||"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\u{2028}' => escaped.push_str("\\u{2028}"),
            '\u{2029}' => escaped.push_str("\\u{2029}"),
            c if c.is_control() => escaped.extend(c.escape_default()),
            c => escaped.push(c),
        }
    }
    let count = escaped.chars().count();
    if count <= max_chars {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// 字符级截断（中文友好）：中文一字 3 字节，按 `chars()` 计数。
///
/// - `s.chars().count() <= n` → 原样返回（不复制：直接 `s.to_string()`）
/// - 否则：取前 `n` 个字符 + 可选追加 `…`（仅当 `ellipsis == true`）
///
/// 与历史两版 `truncate_chars` 的关系：
/// - `memory::truncate_chars(s, n)`（无省略号）→ 现在 `truncate_chars(s, n, false)`
/// - `evolution::truncate_chars(s, max)`（有省略号）→ 现在 `truncate_chars(s, max, true)`
pub fn truncate_chars(s: &str, n: usize, ellipsis: bool) -> String {
    let count = s.chars().count();
    if count <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n).collect();
    if ellipsis {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 弱版历史用例（保持行为不变，回归锁）──

    /// 弱版 `escape_for_log` 的四组历史断言（audit.rs:710）现在仍命中——本次
    /// 重构保证：弱版来话处理结果不变（其它控制字符还会出现，见下）。
    #[test]
    fn escape_for_log_preserves_weak_version_cases() {
        assert_eq!(escape_for_log("a\nb| c", 300), "a\\nb||  c");
        assert_eq!(escape_for_log("x\ry", 300), "x\\ry");
        assert_eq!(escape_for_log("plain", 300), "plain");
        assert_eq!(
            escape_for_log("evil\n[2026-01-01 00:00:00] INFO | fake", 300),
            "evil\\n[2026-01-01 00:00:00] INFO ||  fake"
        );
    }

    /// 弱版 `escape_for_log_truncates_after_escape`（audit.rs:722）：
    /// 长字符串按字符数截到 max_chars + 1（含省略号位）。
    #[test]
    fn escape_for_log_truncates_after_escape() {
        let long = "x".repeat(600);
        let out = escape_for_log(&long, 500);
        assert_eq!(out.chars().count(), 501);
        assert!(out.ends_with('…'));
    }

    // ── 强版新增 4 类弱版忽视的控制字符 ──

    /// NUL → `\u{0}`（避免被截断后看起来像被吞掉）
    #[test]
    fn escape_for_log_handles_null_byte() {
        assert_eq!(escape_for_log("a\0b", 100), "a\\u{0}b");
    }

    /// U+2028 / U+2029 行 / 段分隔符（非 control 但多数工具视为换行）
    #[test]
    fn escape_for_log_handles_line_separator_u2028_u2029() {
        assert_eq!(escape_for_log("a\u{2028}b", 100), "a\\u{2028}b");
        assert_eq!(escape_for_log("a\u{2029}b", 100), "a\\u{2029}b");
    }

    /// `|` 倍增 + 弱版双空格补位
    #[test]
    fn escape_for_log_pipe_doubling_and_space_padding() {
        assert_eq!(escape_for_log("a|b", 100), "a||b");
        assert_eq!(escape_for_log("a| b", 100), "a||  b");
        // 已 escaped 的 `||` 不会再倍增（按字符臂匹配单字符）
        assert_eq!(escape_for_log("a||b", 100), "a||||b");
    }

    /// ESC / BEL / DEL 等控制字符走 escape_default
    #[test]
    fn escape_for_log_handles_esc_bel_del() {
        assert_eq!(escape_for_log("a\u{1b}b", 100), "a\\u{1b}b");
        assert_eq!(escape_for_log("a\u{7}b", 100), "a\\u{7}b");
        assert_eq!(escape_for_log("a\u{7f}b", 100), "a\\u{7f}b");
        // C1 区 U+0085 也走 escape_default
        assert_eq!(escape_for_log("a\u{85}b", 100), "a\\u{85}b");
    }

    /// 普通文本与中文文本不应被转义
    #[test]
    fn escape_for_log_keeps_plain_and_cjk_text() {
        assert_eq!(escape_for_log("普通文本 + ascii", 100), "普通文本 + ascii");
        assert_eq!(escape_for_log("", 100), "");
    }

    // ── truncate_chars ──

    /// 边界 ≤ n：原样返回（无省略号）
    #[test]
    fn truncate_chars_no_ellipsis_under_or_equal() {
        assert_eq!(truncate_chars("abc", 5, false), "abc");
        assert_eq!(truncate_chars("abc", 3, false), "abc");
        // 中文一字 3 字节按字符数算
        assert_eq!(truncate_chars("中文测试", 4, false), "中文测试");
    }

    /// > n：截前 n 字符
    #[test]
    fn truncate_chars_no_ellipsis_truncates() {
        assert_eq!(truncate_chars("abcdef", 3, false), "abc");
        // 中文友好：取前 2 字 = 前 6 字节
        assert_eq!(truncate_chars("中文测试", 2, false), "中文");
    }

    /// ellipsis=true + > n：截前 n + `…`
    #[test]
    fn truncate_chars_with_ellipsis_truncates() {
        assert_eq!(truncate_chars("abcdef", 3, true), "abc…");
        assert_eq!(truncate_chars("中文测试", 2, true), "中文…");
    }

    /// ellipsis=true + ≤ n：不追加省略号（边界正确）
    #[test]
    fn truncate_chars_with_ellipsis_under_boundary() {
        assert_eq!(truncate_chars("abc", 3, true), "abc");
        assert_eq!(truncate_chars("abc", 10, true), "abc");
    }

    /// 空字符串 / n=0
    #[test]
    fn truncate_chars_empty_and_zero() {
        assert_eq!(truncate_chars("", 0, false), "");
        assert_eq!(truncate_chars("", 0, true), "");
        assert_eq!(truncate_chars("", 5, true), "");
        assert_eq!(truncate_chars("abc", 0, false), "");
        assert_eq!(truncate_chars("abc", 0, true), "…");
    }
}
