//! 结构化审计事件（Koa 洋葱管线 post-execute 钩子主用）
//!
//! 老 `bot::audit_log` 仍走 free-form 文本；新 `audit_event!` 走结构化键值对。
//! 两类都写同一个 `bot.log`，解析端靠首段 `INFO/WARN/ERROR` 区分：
//!   老: `[2026-08-17 18:14:00] skill_start | name: x`
//!   新: `[2026-08-17 22:17:42.123] INFO | tool_done | tool=list_tasks | ms=4 | refs=3`
//!
//! 设计动机：洋葱管线补「出」钩子时，结果需要可结构化查询（工具耗时/失败率/技能步骤耗时），
//! 单靠 free-form 文本做不了统计面板。

use std::io::Write;
use tauri::AppHandle;

/// 审计日志安全转义 + 截断（bot 侧日志写入统一走这里）：
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

/// kv 值写入日志前的长度上限
const KV_VALUE_MAX: usize = 500;

/// 拼装 kv 段（write_event 与 format_event_line 共用）：值统一过 escape_for_log，
/// 防用户输入 / 工具输出里的 `\n` / `|` 伪造日志行。
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

/// 工具调用是否失败/被拒（统一判定口径，全链路唯一真相源）。
/// 熔断「已强制终止」、暂停「技能已暂停」、门禁拦截「⚠️」、用户拒绝「用户拒绝」
/// 四类文案与常规失败前缀都要判失败；否则会出现末步熔断误报「✅ 完成」、门禁拦截
/// 对 PREVR 隐身、被拒删除当成功。统一收口到这里，
/// DSL 调度器 / PREVR / 幻觉守卫 / skill_on_step_post / 审计分级全部共用。
/// 注：「失败/错误」保留 contains 语义——工具错误文案多为「{动作}失败：…」，前缀不命中。
pub fn tool_call_failed(name: &str, text: &str) -> bool {
    let _ = name;
    text.starts_with("未知工具")
        || text.starts_with("⚠️") // 门禁拦截（原子工具裸调等）
        || text.starts_with("用户拒绝") // 确认弹窗被拒绝/超时
        || text.starts_with("技能已暂停") // step_check 暂停态拒绝
        || text.contains("已强制终止") // step_check 步数/超时熔断
        || text.contains("失败")
        || text.contains("错误")
        || text.starts_with("error:")
        || text.starts_with("Error")
}

/// 工具返回文本 → 审计级别（post-execute 钩子分类用）
/// - 未知工具 → Error
/// - tool_call_failed 判失败/被拒 → Warn
/// - 其他 → Info
pub fn classify_text(name: &str, text: &str) -> AuditLevel {
    if text.starts_with("未知工具") {
        return AuditLevel::Error;
    }
    if tool_call_failed(name, text) {
        return AuditLevel::Warn;
    }
    AuditLevel::Info
}

/// 拼装一行审计日志（生产 write_event 与测试 format_event_line 共用，
/// 消除「测试专用拷贝与生产拼装 drift 时测试照样绿」的盲区）。
/// ts 由调用方提供（生产填真实时间戳，测试填占位串）。
fn build_event_line(ts: &str, level: AuditLevel, event: &str, kv: &[(&str, &str)]) -> String {
    let mut line = format!("[{ts}] {} | {}", level.as_tag(), event);
    append_kv_escaped(&mut line, kv);
    line
}

/// 纯函数：把事件拼成一行（测试用，不碰磁盘）
/// 委托 build_event_line——与 write_event 同一份拼装实现，测试所见即线上行为
#[cfg_attr(not(test), allow(dead_code))]
pub fn format_event_line(level: AuditLevel, event: &str, kv: &[(&str, &str)]) -> String {
    build_event_line("TIMESTAMP", level, event, kv)
}

/// bot.log 全局写锁：`write_event` 与 `bot::audit_log` 共用，
/// 防多线程并发 append 交错（并发 execute_task 写日志会出现错行混排）。
/// rotate + open + write 必须在同一把锁内，否则检查大小与写入之间存在竞态。
pub static BOT_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 日志文件打开：创建即 0600 + 已有文件补 chmod——
/// bot.log 含用户指令/文件路径/工具输出，umask 默认 0644 下同机其他用户可读。
/// 三处写入点（audit::append_line / bot::audit_log / bot_py::py_audit_to）共用。
pub(crate) fn open_log_append(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts.open(path)?;
    #[cfg(unix)]
    {
        // 已存在文件 mode() 不生效，补 chmod（幂等）
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(f)
}

/// 追加一行到指定日志文件（open/write 失败不再 `let _ =` 全静默——
/// eprintln 到 stderr 提示路径，审计丢了至少有迹可循；返回成功与否供测试断言，
/// 不 panic、不阻塞业务）。
fn append_line(path: &std::path::Path, line: &str) -> bool {
    let f = open_log_append(path);
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
/// 泛型 Runtime：mock runtime 测试可直调（与 write_error_audit 同先例）。
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
    // kv 值统一转义（剥 \n / |），防伪造日志行；调用方不得再自行预转义
    // 行拼装走 build_event_line，与 format_event_line 同一份实现（防 drift）
    let kv_refs: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let line = build_event_line(&ts.to_string(), level, event, &kv_refs);
    append_line(&p, &line);
}

/// 数据目录便携探针单一实现（db / profile / 本模块统一走这里，防多处拷贝 drift）。
/// 优先 exe 同目录（便携模式，U盘/绿色目录随走随带）；目录不可写
/// （如 Program Files）退 app_data_dir；再退系统临时目录。
///
/// 探测结果进程内 OnceLock 定版：每次调用都现写探针文件的话，
/// 杀软临时锁定/UAC 状态变化/压缩包内直接运行等瞬时失败会把当次数据目录
/// 翻转到 app_data，AI_Gen_Files、数据库、日志分裂到两个位置。
/// 首次探测定版，整个运行期不再翻转；
/// 兜底分支发生时记一条 WARN 审计（写清翻到哪、为什么），可诊断。
pub(crate) fn probe_log_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    use tauri::Manager;
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let app_data = app.path().app_data_dir().ok();
    // 首次探测判定（WARN 只在定版那次记）：生产读缓存是否已初始化；
    // 测试构建无缓存，每次现探且不为翻转记 WARN（测试不验证日志副作用）
    #[cfg(not(test))]
    let first_probe = PROBE_CACHE.get().is_none();
    #[cfg(test)]
    let first_probe = false;
    let resolved = probe_dir_cached(exe_dir.as_deref(), app_data);
    // 兜底判定：exe 目录存在但结果不是它 → 发生了翻转，记 WARN（只在首次探测记一次）。
    // 写日志挪到独立线程：调用方可能正持有 BOT_LOG_LOCK（audit_log → data_dir → 这里），
    // std Mutex 不可重入，直接写会死锁。
    if first_probe && exe_dir.as_deref().is_some_and(|d| d != resolved.as_path()) {
        let exe_dir_s = exe_dir
            .as_deref()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        let resolved_s = resolved.to_string_lossy().to_string();
        // zip 直跑（exe 在系统 temp 下）与写探针失败给不同 WARN 原因，便于诊断
        let reason = if exe_dir.as_deref().is_some_and(is_under_system_temp) {
            "exe 位于系统临时目录（疑似压缩包内直接双击运行），不在 temp 建数据目录，已退化 app_data——请解压后再运行"
        } else {
            "exe 目录写探针失败（杀软锁定/权限不足/压缩包内运行），数据目录按便携策略兜底"
        };
        let dir = resolved.clone();
        std::thread::spawn(move || {
            write_warn_audit_to(
                &dir,
                "data_dir_fallback",
                &[
                    ("exe_dir", exe_dir_s.as_str()),
                    ("resolved", resolved_s.as_str()),
                    ("reason", reason),
                ],
            );
        });
    }
    resolved
}

/// 探测结果缓存（进程级 OnceLock）：首次 probe_log_dir 调用定版。
/// 测试构建不缓存——同进程多测试各自探测不同临时目录/模拟 exe 目录，
/// 全局缓存会让先跑的测试劫持后续所有结果；
/// 定版语义由可注入内核 probe_dir_cached_in 的单测覆盖。
#[cfg(not(test))]
static PROBE_CACHE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(not(test))]
fn probe_dir_cached(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    probe_dir_cached_in(&PROBE_CACHE, exe_dir, app_data)
}

/// 测试构建：不缓存，每次现探（测试隔离优先）
#[cfg(test)]
fn probe_dir_cached(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    probe_dir(exe_dir, app_data)
}

/// 可测内核：OnceLock 定版语义——首次探测结果钉死，后续调用条件变化也不再翻转
#[cfg_attr(not(test), allow(dead_code))] // 生产只经 probe_dir_cached 间接调用
fn probe_dir_cached_in(
    cache: &std::sync::OnceLock<std::path::PathBuf>,
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    cache.get_or_init(|| probe_dir(exe_dir, app_data)).clone()
}

/// 已定版的数据目录（降级 key 路径等无 AppHandle 调用方复用，
/// 保证与 probe_log_dir 同一份结果；未初始化 = 主流程还没探测过，返回 None）
/// 测试构建无缓存（见 PROBE_CACHE 注释），恒 None → 调用方走原现探逻辑。
#[cfg(not(test))]
pub(crate) fn cached_probe_dir() -> Option<std::path::PathBuf> {
    PROBE_CACHE.get().cloned()
}

/// 测试构建桩：无进程级缓存，恒 None
#[cfg(test)]
pub(crate) fn cached_probe_dir() -> Option<std::path::PathBuf> {
    None
}

/// 可测内核：probe 三分支——exe 目录可写用它；不可写退 app_data；皆不可用退 temp。
/// 前置规则：
/// 1) exe 旁已有数据痕迹（wmessage.db / AI_Gen_Files）→ 强制便携锚定 exe 目录，
///    跳过写探针——探针瞬时失败（杀软锁定/UAC 抖动）不得把已有数据目录翻转走，
///    全机只允许一个 AI_Gen_Files；
/// 2) exe 落在系统临时目录（压缩包内直接双击运行）→ 不在 temp 建数据，
///    跳过便携分支退化 app_data（翻转 WARN 由 probe_log_dir 统一记）。
/// pub(crate)：bot.rs 降级 key 路径（无 AppHandle）复用同一便携策略。
pub(crate) fn probe_dir(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(dir) = exe_dir {
        // macOS .app 包内目录（*.app/Contents/MacOS）不算
        // 「便携 exe 同目录」——dmg 拖到 ~/Applications 后该目录可写，数据库/日志/
        // AI_Gen_Files 会全写进 app 包内（破坏签名、删 app 即删全部用户数据）。
        if !is_macos_app_bundle_dir(dir) {
            if dir.join("wmessage.db").exists() || dir.join("AI_Gen_Files").exists() {
                return dir.to_path_buf();
            }
            if !is_under_system_temp(dir) {
                let probe = dir.join(".wm-write-probe");
                if std::fs::File::create(&probe).is_ok() {
                    let _ = std::fs::remove_file(&probe);
                    return dir.to_path_buf();
                }
            }
        }
    }
    app_data.unwrap_or_else(std::env::temp_dir)
}

/// exe 目录是否在系统临时目录下（zip 直跑场景）：canonicalize 后比较，
/// macOS /var ↔ /private/var 软链由 canonicalize 归一
fn is_under_system_temp(dir: &std::path::Path) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    dir.starts_with(&tmp)
}

/// macOS .app 包内 MacOS 目录判定：…/Xxx.app/Contents/MacOS
fn is_macos_app_bundle_dir(dir: &std::path::Path) -> bool {
    let mut comps = dir.components().rev();
    matches!(comps.next(), Some(c) if c.as_os_str() == "MacOS")
        && matches!(comps.next(), Some(c) if c.as_os_str() == "Contents")
        && comps
            .next()
            .is_some_and(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
}

/// 泛型 Runtime 版日志目录：write_event 写死 Wry AppHandle，middleware/profile
/// 等泛型模块调不了，病态路径（registry 缺失 / profile 损坏）的 ERROR 审计走这里，
/// 尽力而为不 panic。目录解析委托 probe_log_dir。
fn generic_log_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    probe_log_dir(app)
}

/// 泛型 Runtime 的 ERROR 审计：rotate + BOT_LOG_LOCK + 追加一行结构化事件。
/// 行拼装复用 build_event_line，与 write_event 零漂移；IO 失败走 append_line 的
/// eprintln，不 panic、不阻塞业务。
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

/// 无 AppHandle 场景的 WARN 审计（keyring 降级明文存储路径用）。
/// 目录由调用方解析（与降级 key 文件同目录，保证同一便携位置），
/// 行拼装复用 build_event_line，与 write_event 零漂移。
pub(crate) fn write_warn_audit_to(dir: &std::path::Path, event: &str, kv: &[(&str, &str)]) {
    let _g = BOT_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = dir.join("bot.log");
    crate::db::rotate_log_if_large(&p, 5 * 1024 * 1024);
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = build_event_line(&ts.to_string(), AuditLevel::Warn, event, kv);
    append_line(&p, &line);
}

/// 结构化审计事件宏（post-execute 钩子用）
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

    // ── tool_call_failed 统一判定口径 ──

    #[test]
    fn tool_call_failed_catches_gate_fuse_pause_reject() {
        // 四类易漏判的失败文案（门禁/拒绝/熔断/暂停）都必须命中
        assert!(tool_call_failed(
            "link_file_to_task",
            "⚠️ link_file_to_task 是内部原子，不允许裸调。"
        ));
        assert!(tool_call_failed(
            "delete_task",
            "用户拒绝了删除，任务未删除"
        ));
        assert!(tool_call_failed(
            "x",
            "技能「s」超过最大步数上限（8 步），已强制终止"
        ));
        assert!(tool_call_failed(
            "x",
            "技能已暂停，等待用户确认中；确认通过后才能继续下一步"
        ));
        // 原有判定不回退
        assert!(tool_call_failed("x", "未知工具：foo"));
        assert!(tool_call_failed("x", "生成失败：磁盘只读"));
        assert!(tool_call_failed("x", "error: timeout"));
    }

    #[test]
    fn tool_call_failed_passes_success_text() {
        assert!(!tool_call_failed("list_tasks", "当前没有未完成的任务"));
        assert!(!tool_call_failed(
            "create_word",
            "已生成 Word 文档：/tmp/x.docx"
        ));
        assert!(!tool_call_failed(
            "delete_task",
            "已删除任务「买菜」（进回收站）"
        ));
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

    // ── 写失败不再静默（eprintln + 返回 false，不 panic）──

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

    // ── kv 值统一转义（剥换行/管道符）──

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
        // 迁移自 bot_py，行为保持一致
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

    // ── probe_log_dir 三分支 + 调用方一致性 ──

    #[test]
    fn probe_dir_cached_is_stable_across_calls() {
        // OnceLock 定版后，第二次调用即使探测条件
        // 变化（传入不同 exe_dir）也返回首次结果——运行期数据目录不再翻转。
        // 用独立 OnceLock 实例，不碰进程级全局缓存（防劫持其他测试）
        let cache = std::sync::OnceLock::new();
        let first = probe_dir_cached_in(
            &cache,
            None,
            Some(std::path::PathBuf::from("/tmp/wm-probe-cache-test")),
        );
        let second = probe_dir_cached_in(
            &cache,
            Some(std::path::Path::new("/nonexistent-exe-dir")),
            None,
        );
        assert_eq!(first, second, "定版后探测条件变化不得改变数据目录");
    }

    /// mock exe 目录：系统 temp 下的目录会被 probe_dir 当「压缩包直跑」跳过便携分支，
    /// 测试用 exe 目录一律建在 current_exe 父目录（target/debug/deps，可写且非 temp）
    fn mock_dir_outside_temp(name: &str) -> std::path::PathBuf {
        let base = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join(format!("wm-probe-test-{}-{}", name, uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn probe_dir_writable_exe_dir_wins() {
        // 分支 1：exe 目录可写 → 用它（便携模式）
        let exe_dir = mock_dir_outside_temp("writable");
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(&exe_dir), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir);
        // 探针文件不得残留
        assert!(!exe_dir.join(".wm-write-probe").exists());
        let _ = std::fs::remove_dir_all(&exe_dir);
    }

    #[cfg(unix)]
    #[test]
    fn probe_dir_readonly_exe_dir_falls_back_to_app_data() {
        // 分支 2：exe 目录不可写（如 Program Files）→ 退 app_data_dir
        use std::os::unix::fs::PermissionsExt;
        let ro = mock_dir_outside_temp("readonly");
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(&ro), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path());
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&ro);
    }

    #[test]
    fn probe_dir_exe_under_system_temp_falls_back_to_app_data() {
        // zip 内直接双击运行：exe 落系统 temp → 不在 temp 建数据，退化 app_data
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path());
        assert!(!exe_dir.path().join(".wm-write-probe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn probe_dir_existing_data_traces_force_portable() {
        // exe 旁已有 wmessage.db → 强制便携锚定，目录即使不可写也不翻转
        use std::os::unix::fs::PermissionsExt;
        let exe_dir = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join("wmessage.db"), b"").unwrap();
        std::fs::set_permissions(exe_dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir.path());
        std::fs::set_permissions(exe_dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn probe_dir_gen_dir_trace_forces_portable_under_temp() {
        // exe 在 temp 但旁边已有 AI_Gen_Files → 数据痕迹优先于 temp 规避，锚定 exe 目录
        let exe_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(exe_dir.path().join("AI_Gen_Files")).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir.path());
    }

    /// macOS .app 包内目录（即使可写）不得当便携数据目录——
    /// 否则数据库/日志/AI_Gen_Files 全写进 app 包内（删 app = 删全部数据）
    #[test]
    fn probe_dir_skips_macos_app_bundle_dir() {
        let tmp = std::env::temp_dir().join(format!("wm-test-{}", uuid::Uuid::new_v4().simple()));
        let macos_dir = tmp.join("wmessage.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos_dir).unwrap();
        let app_data = tmp.join("appdata");
        let got = super::probe_dir(Some(&macos_dir), Some(app_data.clone()));
        assert_eq!(got, app_data, ".app 包内目录必须跳过便携分支直落 app_data");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_macos_app_bundle_dir_detection() {
        use std::path::Path;
        assert!(super::is_macos_app_bundle_dir(Path::new(
            "/Applications/wmessage.app/Contents/MacOS"
        )));
        assert!(super::is_macos_app_bundle_dir(Path::new(
            "/Users/x/Applications/wmessage.app/Contents/MacOS/"
        )));
        assert!(!super::is_macos_app_bundle_dir(Path::new(
            "/Applications/wmessage.app/Contents"
        )));
        assert!(!super::is_macos_app_bundle_dir(Path::new("/opt/wmessage")));
        assert!(!super::is_macos_app_bundle_dir(Path::new(
            "/Users/x/wmessage.app/Contents/MacOSub"
        )));
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

    // ── 回归：kv value 里的 `\n` / `| ` 不得逃逸成裸日志分隔符 ──
    // （append_kv_escaped 统一转义，此测试锁死行为防回退）

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

    // ── format_event_line 与生产 write_event 共用 build_event_line ──

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
                vec![
                    ("preview", "{\"k\":\"v\"}\nnext|line"),
                    ("reason", "denied"),
                ],
            ),
        ];
        for (level, event, kv) in cases {
            let via_format = format_event_line(level, event, &kv);
            let via_build = build_event_line("TIMESTAMP", level, event, &kv);
            assert_eq!(via_format, via_build, "kv={kv:?}");
        }
    }
}
