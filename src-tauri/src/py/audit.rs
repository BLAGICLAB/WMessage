//! 结构化审计日志（py_audit + py_audit_to）

use std::io::Write;
use std::path::Path;

use tauri::AppHandle;

pub fn py_audit(app: &AppHandle, line: &str) {
    let p = crate::db::data_dir(app).join("bot.log");
    py_audit_to(&p, line);
}

pub fn py_audit_to(path: &Path, line: &str) {
    let _g =
        crate::db::lock::lock_or_recover(&crate::audit::BOT_LOG_LOCK, "py::audit BOT_LOG_LOCK");
    crate::db::rotate_log_if_large(path, 5 * 1024 * 1024);
    if let Ok(mut f) = crate::audit::open_log_append(path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        // 控制字符转义后再落盘：调用方常拼脚本 stdout/stderr（document.rs 的
        // `out` 等），原文含 \n 时可伪造审计行。行级消毒——整行已按 ` | ` 拼好
        // 结构、value 可能已自行过 escape_for_log，管道倍增会打弯结构分隔符
        let safe = crate::bot::log_sanitize::sanitize_log_line(line);
        let _ = writeln!(f, "[{ts}] {safe}");
    }
}
