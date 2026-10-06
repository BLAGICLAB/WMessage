//! 结构化审计日志（py_audit + py_audit_to）

use std::io::Write;
use std::path::Path;

use tauri::AppHandle;

pub fn py_audit(app: &AppHandle, line: &str) {
    let p = crate::db::data_dir(app).join("bot.log");
    py_audit_to(&p, line);
}

pub fn py_audit_to(path: &Path, line: &str) {
    let _g = crate::audit::BOT_LOG_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] py::audit BOT_LOG_LOCK: {e:?}");
        e.into_inner()
    });
    crate::db::rotate_log_if_large(path, 5 * 1024 * 1024);
    if let Ok(mut f) = crate::audit::open_log_append(path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        // 控制字符转义后再落盘：调用方常拼脚本 stdout/stderr（document.rs 的
        // `out` 等），原文含 \n 时可伪造审计行；与 api_handlers 访问日志同一 sanitize 口径
        let safe = crate::api_handlers::ratelimit::sanitize_log_line(line);
        let _ = writeln!(f, "[{ts}] {safe}");
    }
}
