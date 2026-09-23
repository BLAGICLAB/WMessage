//! 速率限制（每分钟 N 次）+ 访问/变更日志追加。
//!
//! 仅作用于合法请求（鉴权后）；健康检查与限流外的 401 风暴由鉴权前置兜底。
//! 窗口基于毫秒时间戳。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 每分钟请求上限（仅回环，防失控脚本）
pub(crate) const RATE_LIMIT_PER_MIN: u32 = 120;

static RATE: Mutex<(u64, u32)> = Mutex::new((0, 0));

/// 串行化 access/change 日志追加:多线程同写同一日志文件时,
/// 无锁的 rotate+append 组合会让两行内容撕裂交织。
static LOG_WRITE: Mutex<()> = Mutex::new(());

/// 每分钟最多 `RATE_LIMIT_PER_MIN` 次；窗口基于毫秒时间戳
pub(crate) fn rate_check() -> bool {
    let now = crate::api_handlers::util::now_ms() as u64;
    // 锁 poisoning:持有 RATE 的线程 panic 会让 state 半修改。
    // 仍然 into_inner() 恢复锁访问(stop-the-world 不接受),
    // 但 eprintln! 一行标记,日志聚合 / 监控能据此告警。
    let mut g = RATE.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] api_handlers::ratelimit::RATE: {e:?}");
        e.into_inner()
    });
    if now.saturating_sub(g.0) > 60_000 {
        *g = (now, 0);
    }
    if g.1 >= RATE_LIMIT_PER_MIN {
        return false;
    }
    g.1 += 1;
    true
}

/// 追加一行访问/变更日志（`None` 或写入失败则静默忽略）
pub(crate) fn log_line(path: &Option<PathBuf>, line: &str) {
    let Some(p) = path else { return };
    let _g = LOG_WRITE.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] api_handlers::ratelimit::LOG_WRITE: {e:?}");
        e.into_inner()
    });
    crate::db::rotate_log_if_large(p, 5 * 1024 * 1024);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .open(p)
    {
        // 防 log injection:caller 控制内容(handlers.rs:93 传的 path 是
        // HTTP 请求路径,query string 等)可能含 `\n` / `\r` / ESC 等控制
        // 字符,会伪造日志行、隐藏真实活动、或在行导向工具里错位(ESC 还
        // 能注入终端转义序列)。控制字符全部 escape 成可见表示后写。
        let sanitized = sanitize_log_line(line);
        let _ = writeln!(
            f,
            "[{}] {sanitized}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}

/// 把控制字符渲染成可见转义形式;`\r\n\t\0` 保留短形式,其余控制字符
/// (含 ESC、BEL、DEL、C1 区)走 `escape_default`,普通字符(含中文)原样。
/// U+2028/U+2029 非 control 但行导向工具视为换行,一并转义。
pub(crate) fn sanitize_log_line(line: &str) -> String {
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

    #[test]
    fn sanitize_escapes_all_control_chars() {
        assert_eq!(sanitize_log_line("a\u{1b}[31mb"), "a\\u{1b}[31mb");
        assert_eq!(sanitize_log_line("\u{7}"), "\\u{7}");
        assert_eq!(sanitize_log_line("\u{7f}"), "\\u{7f}");
        assert_eq!(sanitize_log_line("\u{85}"), "\\u{85}");
    }

    #[test]
    fn sanitize_keeps_short_forms_and_plain_text() {
        assert_eq!(sanitize_log_line("a\rb\nc\td\0e"), "a\\rb\\nc\\td\\0e");
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
