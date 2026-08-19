//! 结构化审计事件（Koa 洋葱管线 post-execute 钩子主用，2026-08-17 22:17 第一块落地）
//!
//! 老 `bot::audit_log` 仍走 free-form 文本；新 `audit_event!` 走结构化键值对。
//! 两类都写同一个 `bot.log`，解析端靠首段 `INFO/WARN/ERROR` 区分：
//!   老: `[2026-08-17 18:14:00] skill_start | name: x`
//!   新: `[2026-08-17 22:17:42.123] INFO | tool_done | tool=list_tasks | ms=4 | refs=3`
//!
//! 设计动机：洋葱管线补「出」钩子时，结果需要可结构化查询（工具耗时/失败率/技能步骤耗时），
//! 单靠 free-form 文本做不了统计面板。

use std::io::Write;
use tauri::AppHandle; // F-6：Runtime 给 write_event 泛型化

/// 审计日志安全转义 + 截断（P2-11 原生于 bot_py，NEW-C-6 上提本模块共享）：
/// 剥换行/管道符，防伪造「INFO |」前缀与多行撕裂。
/// 规则：`| ` → `|  `（双空格），剩余裸 `|` → `||`，`\n` → `\\n`，`\r` → `\\r`；
/// 转义后按字符数截到 max 加省略号。
/// 例：`"a\nb| c"` → `"a\\nb||  c"`
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

/// kv 值写入日志前的长度上限（NEW-C-6：write_event 统一转义 + 截断）
const KV_VALUE_MAX: usize = 500;

/// 拼装 kv 段（write_event 与 format_event_line 共用）：值统一过 escape_for_log，
/// 防用户输入 / 工具输出里的 `\n` / `|` 伪造日志行（NEW-C-6）。
/// 键保持原样（调用方均为硬编码字面量）。
fn append_kv_escaped(line: &mut String, kv: &[(&str, &str)]) {
    for (k, v) in kv {
        line.push_str(&format!(" | {k}={}", escape_for_log(v, KV_VALUE_MAX)));
    }
}

/// 审计事件级别（post-execute 钩子分类用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditLevel {
    Info,
    Warn,
    Error,
}

impl AuditLevel {
    pub fn as_tag(self) -> &'static str {
        match self {
            AuditLevel::Info => "INFO",
            AuditLevel::Warn => "WARN",
            AuditLevel::Error => "ERROR",
        }
    }
}

/// 工具返回文本 → 审计级别（post-execute 钩子分类用）
/// - 未知工具 → Error
/// - 含「失败/错误/error:」→ Warn
/// - 其他 → Info
pub fn classify_text(name: &str, text: &str) -> AuditLevel {
    if text.starts_with("未知工具") {
        return AuditLevel::Error;
    }
    if text.contains("失败")
        || text.contains("错误")
        || text.starts_with("error:")
        || text.starts_with("Error")
    {
        return AuditLevel::Warn;
    }
    let _ = name;
    AuditLevel::Info
}

/// 纯函数：把事件拼成一行（测试用，不碰磁盘）
/// 与 write_event 同一套 kv 转义规则（NEW-C-6），测试所见即线上行为
#[cfg_attr(not(test), allow(dead_code))]
pub fn format_event_line(level: AuditLevel, event: &str, kv: &[(&str, &str)]) -> String {
    let ts = "TIMESTAMP"; // 测试时占位；真实调用由 write_event 填时间
    let mut line = format!("[{ts}] {} | {}", level.as_tag(), event);
    append_kv_escaped(&mut line, kv);
    line
}

/// bot.log 全局写锁：`write_event` 与 `bot::audit_log` 共用，
/// 防多线程并发 append 交错（2026-08-18 事故：并发 execute_task 写日志出现错行混排）。
/// rotate + open + write 必须在同一把锁内，否则检查大小与写入之间存在竞态。
pub static BOT_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 写一条结构化审计事件到 `bot.log`（post-execute 钩子主入口）
/// 复用 `bot::audit_log` 的 rotate 阈值与文件路径，老日志兼容。
pub fn write_event(app: &AppHandle, level: AuditLevel, event: &str, kv: &[(&str, String)]) {
    let _g = BOT_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::db::rotate_log_if_large(&crate::db::data_dir(app).join("bot.log"), 5 * 1024 * 1024);
    let p = crate::db::data_dir(app).join("bot.log");
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    else {
        return;
    };
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let mut line = format!("[{ts}] {} | {}", level.as_tag(), event);
    // NEW-C-6：kv 值统一转义（剥 \n / |），防伪造日志行；调用方不得再自行预转义
    let kv_refs: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (*k, v.as_str())).collect();
    append_kv_escaped(&mut line, &kv_refs);
    let _ = writeln!(f, "{line}");
}

/// 结构化审计事件宏（post-execute 钩子用，2026-08-17 22:17）
/// 用法：
///   audit_event!(app, AuditLevel::Info, "tool_done",
///       "tool" => "list_tasks", "ms" => 4u64, "refs" => 3usize);
#[macro_export]
macro_rules! audit_event {
    ($app:expr, $level:expr, $event:expr $(, $k:expr => $v:expr)* $(,)?) => {
        $crate::audit::write_event($app, $level, $event, &[
            $(($k, format!("{}", $v))),*
        ])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_as_tag_three_levels() {
        assert_eq!(AuditLevel::Info.as_tag(), "INFO");
        assert_eq!(AuditLevel::Warn.as_tag(), "WARN");
        assert_eq!(AuditLevel::Error.as_tag(), "ERROR");
    }

    #[test]
    fn classify_text_unknown_tool_returns_error() {
        assert_eq!(classify_text("foo", "未知工具：bar"), AuditLevel::Error);
        assert_eq!(classify_text("foo", "未知工具："), AuditLevel::Error);
    }

    #[test]
    fn classify_text_fail_keyword_returns_warn() {
        assert_eq!(classify_text("foo", "删除失败：权限不足"), AuditLevel::Warn);
        assert_eq!(classify_text("foo", "操作错误：参数缺失"), AuditLevel::Warn);
        assert_eq!(classify_text("foo", "error: timeout"), AuditLevel::Warn);
        assert_eq!(classify_text("foo", "Error: 404"), AuditLevel::Warn);
    }

    #[test]
    fn classify_text_normal_returns_info() {
        assert_eq!(
            classify_text("foo", "当前没有未完成的任务"),
            AuditLevel::Info
        );
        assert_eq!(
            classify_text("foo", "已新建任务 id=abc123"),
            AuditLevel::Info
        );
        // "失败" 关键词的误报：接受这种 trade-off（合法场景罕见）
        assert_eq!(
            classify_text("foo", "成功完成任务，含失败回滚说明"),
            AuditLevel::Warn
        );
    }

    #[test]
    fn format_event_line_kv_concat_order() {
        let line = format_event_line(
            AuditLevel::Info,
            "tool_done",
            &[("tool", "list_tasks"), ("ms", "4"), ("refs", "3")],
        );
        assert_eq!(
            line,
            "[TIMESTAMP] INFO | tool_done | tool=list_tasks | ms=4 | refs=3"
        );
    }

    #[test]
    fn format_event_line_empty_kv_ok() {
        let line = format_event_line(AuditLevel::Warn, "skill_paused", &[]);
        assert_eq!(line, "[TIMESTAMP] WARN | skill_paused");
    }

    #[test]
    fn format_event_line_equals_value_unescaped() {
        // 参数预览可能含「=」（JSON 片段），「=」不转义，解析端按 | 分隔再 splitn(2, '=') 即可
        let line = format_event_line(
            AuditLevel::Info,
            "tool_done",
            &[("preview", "{\"k\":\"v\"}")],
        );
        assert!(line.contains("preview={\"k\":\"v\"}"));
    }

    // ── NEW-C-6：kv 值统一转义（剥换行/管道符）──

    #[test]
    fn write_event_escapes_kv_newlines() {
        // write_event 的拼装走 format_event_line 同一 append_kv_escaped：
        // 传 ("k", "a\nb| c")，日志行必须含转义后字符串且不含原始换行
        let line = format_event_line(AuditLevel::Info, "tool_done", &[("k", "a\nb| c")]);
        assert!(line.contains("k=a\\nb||  c"), "got: {line}");
        assert!(!line.contains("a\nb"), "原始换行必须被剥掉: {line:?}");
    }

    #[test]
    fn escape_for_log_strips_newlines_and_pipes() {
        // 迁移自 bot_py（P2-11），行为保持一致
        assert_eq!(escape_for_log("a\nb| c", 300), "a\\nb||  c");
        assert_eq!(escape_for_log("x\ry", 300), "x\\ry");
        assert_eq!(escape_for_log("plain", 300), "plain");
        assert_eq!(
            escape_for_log("evil\n[2026-01-01 00:00:00] INFO | fake", 300),
            "evil\\n[2026-01-01 00:00:00] INFO ||  fake"
        );
    }

    #[test]
    fn escape_for_log_truncates_after_escape() {
        let long = "x".repeat(600);
        let out = escape_for_log(&long, KV_VALUE_MAX);
        assert_eq!(out.chars().count(), KV_VALUE_MAX + 1);
        assert!(out.ends_with('…'));
    }
}
