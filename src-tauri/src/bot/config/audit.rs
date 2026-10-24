//! Bot 审计日志（bot.log）：free-form 兼容入口（桥接到结构化写侧）/ 读尾部 + escape_for_log。
//!
//! 写侧已收敛：`audit_log` 只做 free-form 行拆解，rotate / 写锁 / 转义 / 行格式
//! 全部走 `crate::audit::write_event` 单一内核——bot.log 不再有第二种写法。
//! 新代码一律 `audit_event!`（结构化 kv）；本入口为存量调用点保留，冻结守卫
//! 拦新增（见 audit.rs 的 `audit_log_frozen_to_migration_whitelist`）。

use std::path::Path;

use tauri::AppHandle;

use crate::db;
use crate::error::{CommandError, CommandResult};

// 写

/// 追加机器人审计日志（free-form 兼容入口，已桥接到结构化写侧）。
/// 行文按仓库惯例「事件名 | 细节」拆成 (event, detail) 交给 write_event：
///   `skill_create | name: x` → `[ts] INFO | skill_create | detail=name: x`
/// 拆不开「 | 」的整行进事件名（空白归一成 `_`，截 64 字符）。
/// 审计日志外部钩子（bot_skills 调度器用；bot.rs 内部仍用 audit_log）
pub fn audit_log_hook<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    audit_log(app, line);
}

pub fn audit_log<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    let (event, detail) = split_freeform_line(line);
    match detail {
        Some(d) => crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            &event,
            &[("detail", d)],
        ),
        None => crate::audit::write_event(app, crate::audit::AuditLevel::Info, &event, &[]),
    }
}

/// free-form 行拆解（纯函数，可单测）：首个 `" | "` 前是事件名（空白归一成
/// `_`、截 64 字符），其后整体是细节；没有 `" | "` 的整行就是事件名。
/// 事件名为空（行以 `" | "` 开头）退回 `bot_note`，不给空事件名进日志。
pub(crate) fn split_freeform_line(line: &str) -> (String, Option<String>) {
    let line = line.trim();
    let (raw_event, detail) = match line.split_once(" | ") {
        Some((e, d)) => (e, Some(d.trim().to_string())),
        None => (line, None),
    };
    let mut event: String = raw_event
        .chars()
        .take(64)
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    if event.is_empty() {
        event = "bot_note".to_string();
    }
    let detail = detail.filter(|d| !d.is_empty());
    (event, detail)
}

// 转义 + 截断

// 强版转义单源在 crate::bot::log_sanitize（逐字符臂与本地旧实现完全一致：
// "| "→"|  " 预替换 + |倍增 + \n\r 短形式 + 控制字符/U+2028/2029 escape_default）。
pub(crate) use crate::bot::log_sanitize::escape_for_log;

/// 向后兼容别名：bot_chat / bot_model_loop / bot_scheduler 仍用旧名，
/// 行为即 escape_for_log（同族日志伪造问题一并修复）；新代码请直接用 escape_for_log。
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    escape_for_log(s, max)
}

// 读

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

#[cfg(test)]
mod escape_tests {
    use super::*;

    /// 控制字符全集（\t/\0/ESC）与 U+2028/2029 显式转义；`|` 倍增；普通文本不变
    #[test]
    fn escape_for_log_covers_control_chars() {
        assert_eq!(escape_for_log("a\tb", 100), "a\\tb");
        // escape_default 对 NUL 输出 \u{0} 形态
        assert_eq!(escape_for_log("a\0b", 100), "a\\u{0}b");
        assert_eq!(escape_for_log("a\u{1b}b", 100), "a\\u{1b}b");
        assert_eq!(escape_for_log("a\u{2028}b", 100), "a\\u{2028}b");
        assert_eq!(escape_for_log("a\u{2029}b", 100), "a\\u{2029}b");
        assert_eq!(escape_for_log("a|b", 100), "a||b");
        assert_eq!(escape_for_log("a\nb", 100), "a\\nb");
        assert_eq!(escape_for_log("普通文本", 100), "普通文本");
        // 截断仍生效
        assert_eq!(escape_for_log("abcdef", 3), "abc…");
    }
}
