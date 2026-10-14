//! 速率限制（每分钟 N 次）+ 访问/变更日志追加。
//!
//! 仅作用于合法请求（鉴权后）；健康检查与限流外的 401 风暴由鉴权前置兜底。
//! 窗口基于毫秒时间戳。

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 每分钟请求上限（仅回环，防失控脚本）。
/// 固定窗口语义：窗口拼接处最坏可突发 2 倍（2s 内 240 次）——对手是失控脚本
/// 而非定向攻击，可接受；需严格 ≤N/60s 时再改滑动窗口。
pub(crate) const RATE_LIMIT_PER_MIN: u32 = 120;

static RATE: Mutex<(u64, u32)> = Mutex::new((0, 0));

/// 串行化 access/change 日志追加:多线程同写同一日志文件时,
/// 无锁的 rotate+append 组合会让两行内容撕裂交织。
static LOG_WRITE: Mutex<()> = Mutex::new(());

/// 每分钟最多 `RATE_LIMIT_PER_MIN` 次；窗口基于毫秒时间戳
pub(crate) fn rate_check() -> bool {
    let now = crate::db::now_ms() as u64;
    // 锁 poisoning:持有 RATE 的线程 panic 会让 state 半修改。
    // 仍然 into_inner() 恢复锁访问(stop-the-world 不接受),
    // 但 eprintln! 一行标记,日志聚合 / 监控能据此告警。
    let mut g = crate::db::lock::lock_or_recover(&RATE, "api_handlers::ratelimit::RATE");
    // 时钟回拨（NTP 步进/手动改时）时 now < g.0，saturating_sub 得 0 会把窗口
    // 冻死（计数只增不清，回环 API 全被拒直到时钟追回）；回拨视同窗口过期直接重开
    if now.checked_sub(g.0).is_none_or(|d| d > 60_000) {
        *g = (now, 0);
    }
    if g.1 >= RATE_LIMIT_PER_MIN {
        return false;
    }
    g.1 += 1;
    true
}

/// 追加一行访问/变更日志（`None` 时不写；open/writeln 失败 eprintln 留痕）
pub(crate) fn log_line(path: &Option<PathBuf>, line: &str) {
    let Some(p) = path else { return };
    let _g = crate::db::lock::lock_or_recover(&LOG_WRITE, "api_handlers::ratelimit::LOG_WRITE");
    crate::db::rotate_log_if_large(p, 5 * 1024 * 1024);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        Ok(mut f) => {
            // 防 log injection:caller 控制内容(handlers.rs:93 传的 path 是
            // HTTP 请求路径,query string 等)可能含 `\n` / `\r` / ESC 等控制
            // 字符,会伪造日志行、隐藏真实活动、或在行导向工具里错位(ESC 还
            // 能注入终端转义序列)。行级消毒（控制字符全集，不碰 | 不截断）
            let sanitized = crate::bot::log_sanitize::sanitize_log_line(line);
            // 审计行写失败可见化（磁盘满/权限）——访问日志静默丢失不可接受
            if let Err(e) = writeln!(
                f,
                "[{}] {sanitized}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            ) {
                eprintln!("[ratelimit] 访问日志写入失败（{}）：{e}", p.display());
            }
        }
        Err(e) => {
            eprintln!("[ratelimit] 访问日志打开失败（{}）：{e}", p.display());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn access_line_sanitized_via_log_sanitize() {
        // 归单源后的接线锁：访问行走行级消毒（控制字符转义、| 原样、不截断）
        assert_eq!(
            crate::bot::log_sanitize::sanitize_log_line("GET /a?b|c\n"),
            "GET /a?b|c\\n"
        );
    }
}
