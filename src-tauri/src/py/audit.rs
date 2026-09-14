//! 结构化审计日志（py_audit + py_audit_to）

use std::io::Write;
use std::path::Path;

use tauri::AppHandle;

pub fn py_audit(app: &AppHandle, line: &str) {
    let p = crate::db::data_dir(app).join("bot.log");
    py_audit_to(&p, line);
}

pub fn py_audit_to(path: &Path, line: &str) {
    let _g = crate::audit::BOT_LOG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::db::rotate_log_if_large(path, 5 * 1024 * 1024);
    if let Ok(mut f) = crate::audit::open_log_append(path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {line}");
    }
}
