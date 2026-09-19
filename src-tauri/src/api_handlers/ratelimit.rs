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
    crate::db::rotate_log_if_large(p, 5 * 1024 * 1024);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .open(p)
    {
        // 防 log injection:caller 控制内容(handlers.rs:93 传的 path 是
        // HTTP 请求路径,query string 等)可能含 `\n` / `\r`,会伪造日志行、
        // 隐藏真实活动、或在行导向工具里错位。控制字符全部 escape 成可见
        // 表示后写。
        let sanitized = line
            .replace('\r', "\\r")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
            .replace('\0', "\\0");
        let _ = writeln!(
            f,
            "[{}] {sanitized}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}
