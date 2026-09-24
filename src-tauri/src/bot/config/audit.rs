//! Bot 审计日志（bot.log）：写一行 / 读尾部 + escape_for_log。
//!
//! 与 crate::audit 共享 BOT_LOG_LOCK 防并发 append 交错错行；
//! 真实失败 eprintln 带 path，不静默吞（与 audit::append_line 同做法）。

use std::io::Write;
use std::path::Path;

use tauri::AppHandle;

use crate::db;
use crate::error::{CommandError, CommandResult};

// ───────────────────────── 写 ─────────────────────────

/// 追加机器人审计日志：用户指令、工具名、入参、结果全部留痕（数据目录 bot.log）
/// 审计日志外部钩子（bot_skills 调度器用；bot.rs 内部仍用 audit_log）
pub fn audit_log_hook<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    audit_log(app, line);
}

pub fn audit_log<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    // data_dir 解析 + rotation（stat+rename 数 MB）在锁外跑：慢 IO 不饿死其它
    // audit 写者。rotate_log_if_large 容错竞争（metadata 失败跳过 / rename 失败
    // 静默）——双写者并发 rotation 最坏 = 当次不轮转，下次调用补上，无数据破坏。
    let p = db::data_dir(app).join("bot.log");
    crate::db::rotate_log_if_large(&p, 5 * 1024 * 1024);
    // 与 audit::write_event 共用同一把写锁，锁内只剩 open+append 防交错错行
    let _g = crate::audit::BOT_LOG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _ = append_bot_log_line(&p, line);
}

/// bot.log 写一行内核：open/write 失败 eprintln 带路径并返回 false，
/// 不 `if let Ok … { let _ = writeln! }` 全静默（对齐 audit::append_line 的做法）。
/// 抽成路径参数版便于单测（与 bot_py.rs 同先例）。
pub(crate) fn append_bot_log_line(p: &Path, line: &str) -> bool {
    let mut f = match crate::audit::open_log_append(p) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[audit] write failed: {} path={}", e, p.display());
            return false;
        }
    };
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Err(e) = writeln!(f, "[{ts}] {line}") {
        eprintln!("[audit] write failed: {} path={}", e, p.display());
        return false;
    }
    true
}

// ───────────────────────── 转义 + 截断 ─────────────────────────

/// 审计日志安全转义 + 截断：剥换行/管道符，防伪造「INFO |」前缀与多行撕裂。
/// 规则：`| ` → `|  `（双空格），剩余裸 `|` → `||`，`\n` → `\\n`，`\r` → `\\r`；
/// 转义后按字符数截到 max 加省略号。与 bot_py::escape_for_log 同一规则。
pub(crate) fn escape_for_log(s: &str, max: usize) -> String {
    let escaped = s
        .replace("| ", "|  ")
        .replace('|', "||")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let count = escaped.chars().count();
    if count <= max {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 向后兼容别名：bot_chat / bot_model_loop / bot_scheduler 仍用旧名，
/// 行为即 escape_for_log（同族日志伪造问题一并修复）；新代码请直接用 escape_for_log。
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    escape_for_log(s, max)
}

// ───────────────────────── 读 ─────────────────────────

/// 读取机器人审计日志（倒序，最新在前；默认 200 行，上限 2000）
/// 读失败（权限/磁盘/损坏）返回 Err(IoError) + ERROR 审计，
/// 不静默吞成「暂无日志」；仅「文件不存在」返回占位文案。
#[tauri::command]
pub fn bot_log_read(app: AppHandle, limit: Option<usize>) -> CommandResult<String> {
    let p = db::data_dir(&app).join("bot.log");
    match read_log_tail(&p, limit) {
        Ok(s) => Ok(s),
        Err(e) => {
            // 带 code 写审计（err => 宏臂自动展开 code/recoverable/err）：
            // `grep 'code=IO_ERROR' bot.log` 可直接统计读取失败
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Error,
                "bot_log_read_fail",
                err => &e
            );
            Err(e)
        }
    }
}

/// 纯路径参数版便于单测（tauri command 绑定 Wry AppHandle）。
/// NotFound → Ok("（暂无日志）")（日志确实没东西）；其他 IO 错误 → Err(IoError)。
pub(crate) fn read_log_tail(path: &Path, limit: Option<usize>) -> CommandResult<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("（暂无日志）".into()),
        Err(e) => return Err(CommandError::IoError(format!("读取日志失败：{e}"))),
    };
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }
    lines.reverse();
    Ok(lines.join("\n"))
}

// 抑制 unused 警告：Write / tauri::AppHandle / db 都已通过 trait/参数使用
#[allow(dead_code)]
fn _write_marker(_w: &mut dyn Write) {}
