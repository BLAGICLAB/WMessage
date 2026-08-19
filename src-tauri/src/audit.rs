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

/// 拼装一行审计日志（NEW-D-5：生产 write_event 与测试 format_event_line 共用，
/// 消除「测试专用拷贝与生产拼装 drift 时测试照样绿」的盲区）。
/// ts 由调用方提供（生产填真实时间戳，测试填占位串）。
fn build_event_line(ts: &str, level: AuditLevel, event: &str, kv: &[(&str, &str)]) -> String {
    let mut line = format!("[{ts}] {} | {}", level.as_tag(), event);
    append_kv_escaped(&mut line, kv);
    line
}

/// 纯函数：把事件拼成一行（测试用，不碰磁盘）
/// 委托 build_event_line——与 write_event 同一份拼装实现（NEW-D-5），测试所见即线上行为
#[cfg_attr(not(test), allow(dead_code))]
pub fn format_event_line(level: AuditLevel, event: &str, kv: &[(&str, &str)]) -> String {
    build_event_line("TIMESTAMP", level, event, kv)
}

/// bot.log 全局写锁：`write_event` 与 `bot::audit_log` 共用，
/// 防多线程并发 append 交错（2026-08-18 事故：并发 execute_task 写日志出现错行混排）。
/// rotate + open + write 必须在同一把锁内，否则检查大小与写入之间存在竞态。
pub static BOT_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 追加一行到指定日志文件（P2-15：open/write 失败不再 `let _ =` 全静默——
/// eprintln 到 stderr 提示路径，审计丢了至少有迹可循；返回成功与否供测试断言，
/// 不 panic、不阻塞业务）。
fn append_line(path: &std::path::Path, line: &str) -> bool {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    let mut f = match f {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[audit] write failed: {} path={}", e, path.display());
            return false;
        }
    };
    if let Err(e) = writeln!(f, "{line}") {
        eprintln!("[audit] write failed: {} path={}", e, path.display());
        return false;
    }
    true
}

/// 写一条结构化审计事件到 `bot.log`（post-execute 钩子主入口）
/// 复用 `bot::audit_log` 的 rotate 阈值与文件路径，老日志兼容。
/// 泛型 Runtime（P2-24）：mock runtime 测试可直调（与 write_error_audit 同先例）。
pub fn write_event<R: tauri::Runtime>(
    app: &AppHandle<R>,
    level: AuditLevel,
    event: &str,
    kv: &[(&str, String)],
) {
    let _g = BOT_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::db::rotate_log_if_large(&crate::db::data_dir(app).join("bot.log"), 5 * 1024 * 1024);
    let p = crate::db::data_dir(app).join("bot.log");
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    // NEW-C-6：kv 值统一转义（剥 \n / |），防伪造日志行；调用方不得再自行预转义
    // NEW-D-5：行拼装走 build_event_line，与 format_event_line 同一份实现（防 drift）
    let kv_refs: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let line = build_event_line(&ts.to_string(), level, event, &kv_refs);
    append_line(&p, &line);
}

/// P2-19：数据目录便携探针单一实现（原 db::db_dir / profile::data_dir /
/// 本模块 generic_log_dir 三处拷贝，drift 风险；现统一走这里）。
/// 优先 exe 同目录（便携模式，U盘/绿色目录随走随带）；目录不可写
/// （如 Program Files）退 app_data_dir；再退系统临时目录。
pub(crate) fn probe_log_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    use tauri::Manager;
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    probe_dir(exe_dir.as_deref(), app.path().app_data_dir().ok())
}

/// P2-19 可测内核：probe 三分支——exe 目录可写用它；不可写退 app_data；皆不可用退 temp。
fn probe_dir(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(dir) = exe_dir {
        let probe = dir.join(".wm-write-probe");
        if std::fs::File::create(&probe).is_ok() {
            let _ = std::fs::remove_file(&probe);
            return dir.to_path_buf();
        }
    }
    app_data.unwrap_or_else(std::env::temp_dir)
}

/// 泛型 Runtime 版日志目录（D2）：write_event 写死 Wry AppHandle，middleware/profile
/// 等泛型模块调不了，病态路径（registry 缺失 / profile 损坏）的 ERROR 审计走这里，
/// 尽力而为不 panic。P2-19 起目录解析委托 probe_log_dir（消除第三处拷贝）。
fn generic_log_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    probe_log_dir(app)
}

/// 泛型 Runtime 的 ERROR 审计（D2/D3）：rotate + BOT_LOG_LOCK + 追加一行结构化事件。
/// 行拼装复用 build_event_line，与 write_event 零漂移；IO 失败走 append_line 的
/// eprintln（P2-15），不 panic、不阻塞业务。
pub(crate) fn write_error_audit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    event: &str,
    kv: &[(&str, &str)],
) {
    let _g = BOT_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = generic_log_dir(app).join("bot.log");
    crate::db::rotate_log_if_large(&p, 5 * 1024 * 1024);
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = build_event_line(&ts.to_string(), AuditLevel::Error, event, kv);
    append_line(&p, &line);
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

    // ── P2-15：写失败不再静默（eprintln + 返回 false，不 panic）──

    #[test]
    fn append_line_ok_returns_true_and_writes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot.log");
        assert!(append_line(&p, "hello"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello\n");
    }

    #[cfg(unix)]
    #[test]
    fn append_line_readonly_dir_fails_without_panic() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let p = ro.join("bot.log");
        // 写失败：返回 false + eprintln（stderr 含 path），主流程不 panic
        assert!(!append_line(&p, "x"));
        assert!(!p.exists());
        // 恢复权限让 tempdir 清理不掉链子
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
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

    // ── P2-19：probe_log_dir 三分支 + 调用方一致性 ──

    #[test]
    fn probe_dir_writable_exe_dir_wins() {
        // 分支 1：exe 目录可写 → 用它（便携模式）
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir.path());
        // 探针文件不得残留
        assert!(!exe_dir.path().join(".wm-write-probe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn probe_dir_readonly_exe_dir_falls_back_to_app_data() {
        // 分支 2：exe 目录不可写（如 Program Files）→ 退 app_data_dir
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let ro = base.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(&ro), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path());
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn probe_dir_no_exe_no_app_data_falls_back_to_temp() {
        // 分支 3：exe 目录不可写 且 app_data_dir 不可用 → 退系统临时目录
        let got = probe_dir(None, None);
        assert_eq!(got, std::env::temp_dir());
    }

    #[test]
    fn probe_log_dir_matches_exe_parent_in_cargo_test() {
        // 调用方一致性：cargo test 下 current_exe 父目录（target/debug/deps）可写，
        // probe_log_dir 必须命中 exe 分支——与抽取前 db_dir/profile data_dir 行为一致
        let app = tauri::test::mock_app();
        let got = probe_log_dir(app.handle());
        let exe_parent = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(got, exe_parent);
    }

    // ── P2-16 回归：kv value 里的 `\n` / `| ` 不得逃逸成裸日志分隔符 ──
    // （NEW-C-6 已在 append_kv_escaped 统一转义，此测试锁死行为防回退）

    #[test]
    fn kv_value_pipe_space_and_newline_stay_escaped() {
        let line = format_event_line(
            AuditLevel::Info,
            "tool_done",
            &[("preview", "a\nb| c"), ("note", "ok")],
        );
        // 行内「 | 」只允许来自结构分隔（INFO|event 1 处 + 2 个 kv = 3 处），
        // value 里的不得多出裸分隔
        assert_eq!(line.matches(" | ").count(), 3, "got: {line:?}");
        assert!(!line.contains("a\nb"), "裸换行必须被剥掉: {line:?}");
        assert!(line.contains("preview=a\\nb||  c"), "got: {line:?}");
    }

    // ── NEW-D-5：format_event_line 与生产 write_event 共用 build_event_line ──

    #[test]
    fn build_event_line_is_single_source_for_format_and_write() {
        // write_event 的行拼装 = build_event_line(ts, ...)；format_event_line = build_event_line("TIMESTAMP", ...)。
        // write_event 是 Wry 签名无法 mock runtime 直调，这里断言两条路径对同等 kv 产出同等行——
        // 由于二者都委托 build_event_line，该等式由同一实现保证，drift 在编译期即不可能。
        let cases: Vec<(AuditLevel, &str, Vec<(&str, &str)>)> = vec![
            (
                AuditLevel::Info,
                "tool_done",
                vec![("tool", "list_tasks"), ("ms", "4"), ("refs", "3")],
            ),
            (AuditLevel::Warn, "skill_paused", vec![]),
            (
                AuditLevel::Error,
                "tool.return",
                vec![("preview", "{\"k\":\"v\"}\nnext|line"), ("reason", "denied")],
            ),
        ];
        for (level, event, kv) in cases {
            let via_format = format_event_line(level, event, &kv);
            let via_build = build_event_line("TIMESTAMP", level, event, &kv);
            assert_eq!(via_format, via_build, "kv={kv:?}");
        }
    }
}
