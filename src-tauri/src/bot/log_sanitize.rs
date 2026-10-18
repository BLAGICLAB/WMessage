//! 共享的日志转义 + 字符级截断 helper（单源收敛点）。
//!
//! 转义分两级，语义刻意不同——按调用形态选，不可混用：
//! - [`escape_for_log`]：**值级**。`|` 倍增 + `| ` 双空格补位（value 不得伪装
//!   bot.log 的 ` | ` 字段分隔）+ 控制字符全集 + U+2028/2029 + 截断加省略号。
//!   audit / config / paths 的 kv value 走这级。
//! - [`sanitize_log_line`]：**行级**。只转义控制字符与 U+2028/2029，不碰 `|`、
//!   不截断——用于「整行已在调用方按 ` | ` 拼好结构、value 可能已自行转义」的
//!   场景（py_audit / api 访问日志），管道倍增会打弯结构分隔符或二次倍增。
//! - [`truncate_chars`]：纯截断，不带转义（memory 内容字段与文案）。
//!
//! 值级的强版与更早的弱版（仅 `\n\r|` 三件套）差异只在「含控制字符 / U+2028 /
//! U+2029」的输入：弱版悄悄放行，强版显式 `\u{N}` 转义——既防伪造日志行也防
//! 终端注入序列。audit.rs 保留的同名单测是弱版历史值的回归锁。

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
/// - 带省略号的那一版 → 现在 `truncate_chars(s, n, true)`
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

/// **行级**日志消毒（见模块头「两级转义」）：只把控制字符与 U+2028/2029 渲染成
/// 可见转义，不碰 `|`、不截断。用于整行已由调用方按 ` | ` 拼好结构、value 可能
/// 已自行过 [`escape_for_log`] 的写入路径（py_audit / api 访问日志）——
/// 管道倍增会把结构分隔符打弯、把已转义的 value 二次倍增。
pub fn sanitize_log_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len() * 2);
    for c in line.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            '\u{2028}' => out.push_str("\\u{2028}"),
            '\u{2029}' => out.push_str("\\u{2029}"),
            c if c.is_control() => out.extend(c.escape_default()),
            c => out.push(c),
        }
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

    // ── sanitize_log_line（行级：控制字符转义，不碰 | 不截断）──

    #[test]
    fn sanitize_line_escapes_all_control_chars() {
        assert_eq!(sanitize_log_line("a\u{1b}[31mb"), "a\\u{1b}[31mb");
        assert_eq!(sanitize_log_line("\u{7}"), "\\u{7}");
        assert_eq!(sanitize_log_line("\u{7f}"), "\\u{7f}");
        assert_eq!(sanitize_log_line("\u{85}"), "\\u{85}");
    }

    #[test]
    fn sanitize_line_keeps_short_forms_pipes_and_plain_text() {
        // \r\n\t\0 保留短形式；`|` 原样（结构分隔符归调用方管）
        assert_eq!(sanitize_log_line("a\rb\nc\td\0e"), "a\\rb\\nc\\td\\0e");
        assert_eq!(sanitize_log_line("a | b | c"), "a | b | c");
        assert_eq!(
            sanitize_log_line("正常中文 /path?q=1"),
            "正常中文 /path?q=1"
        );
        assert_eq!(sanitize_log_line(""), "");
        // 不变式:输出绝不残留任何控制字符或行/段分隔符
        assert!(
            !sanitize_log_line("a\u{1b}\u{7}\u{7f}\u{85}\u{2028}\u{2029}\rb")
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        );
    }
}
