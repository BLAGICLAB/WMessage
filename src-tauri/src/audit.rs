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

/// CommandError → 审计标准三键：`code` / `recoverable` / `err`。
///
/// 用途：bot.log 里失败事件带机器可读 code（`grep 'code=LLM_API_ERROR'` 即可统计失败分布），
/// 不必再解析人读文案。核心模型循环拿不到 AppHandle（审计走注入回调），
/// 所以这里提供 kv 级 helper，与 `write_event_with_error` / `audit_event!(..., err => &e)`
/// 共用同一份拼装，避免两条路径的键名 drift。
pub fn error_kv(err: &crate::error::CommandError) -> Vec<(&'static str, String)> {
    vec![
        ("code", err.code().as_str().to_string()),
        ("recoverable", err.is_recoverable().to_string()),
        ("err", err.message()),
    ]
}

/// 带 CommandError 的结构化审计事件：标准三键 + 调用方附加 kv。
/// `audit_event!(app, level, "evt", err => &e, "k" => v)` 宏臂直接扩到这里。
pub fn write_event_with_error<R: tauri::Runtime>(
    app: &AppHandle<R>,
    level: AuditLevel,
    event: &str,
    err: &crate::error::CommandError,
    extra_kv: &[(&str, String)],
) {
    let mut kv = error_kv(err);
    kv.extend(extra_kv.iter().map(|(k, v)| (*k, v.clone())));
    write_event(app, level, event, &kv);
}

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
///
/// 「失败/错误」改为 markdown 前缀 + 关键词后接分隔符才判失败：
/// 1. markdown 前缀（#/-/*/数字列表）→ 过滤文档小标题「# 失败案例分析」
/// 2. 关键词后必须紧跟分隔符（：：，、、；； 空格 ()） → 过滤正文里裸出现的
///    「失败」「错误」（如「失败重试策略」），保留真错误消息「删除失败：权限不足」
/// 3. 首行 ≤120 **字符**（非字节，中文一字 3 字节，按字计算才一致；之前用 len()
///    实际只容 40 个汉字，过严误杀中文错误消息）
/// 4. 英文 error/Error/ERROR 都要带冒号（Error: 404 / ERROR: timeout），
///    避免「Error handling guide」这种标题被误判
pub fn tool_call_failed(name: &str, text: &str) -> bool {
    let _ = name;
    text.starts_with("未知工具")
        || text.starts_with("⚠️") // 门禁拦截（原子工具裸调等）
        || text.starts_with("用户拒绝") // 确认弹窗被拒绝/超时
        || text.starts_with("技能已暂停") // step_check 暂停态拒绝
        || text.contains("已强制终止") // step_check 步数/超时熔断
        || failure_keyword_after_separator(text)
        || text.starts_with("error:")
        || text.starts_with("Error:")
        || text.starts_with("ERROR:")
}

/// 失败/错误关键词后置分隔符判定：仅在首行 ≤120 **字符**、markdown 前缀开头
/// 或无前缀、关键词后紧跟分隔符（：：，、；； 空格 ()）时判失败。
///
/// 分隔符集合是中英文常见标点 + 空格，覆盖绝大多数真错误消息
/// （「删除失败：权限不足」「运行失败, exit code 1」「生成失败 磁盘只读」
/// 「失败重试（最多3次）」）；不包含「的」「了」等普通连词，避免「失败的请求」
/// 这种语义中性文本被误判。
fn failure_keyword_after_separator(text: &str) -> bool {
    // 首行 + 字符数限制（120 字 = 120 chars，按字算中文友好）
    let Some(first) = text.lines().next().map(str::trim) else {
        return false;
    };
    if first.is_empty() || first.chars().count() > 120 {
        return false;
    }
    // markdown 前缀判定：# 标题 / - 项目符号 / * 项目符号 / 数字. 列表
    // 容许纯文本（无前缀）也通过——错误消息本身就不带 markdown 符号
    let body = strip_markdown_prefix(first);
    let body = body.trim_start();
    if body.is_empty() {
        return false;
    }
    // 关键词后置分隔符集合
    const SEPARATORS: &[char] = &['：', ':', '，', ',', '、', '；', ';', ' ', '（', '('];
    has_keyword_after(body, "失败", SEPARATORS) || has_keyword_after(body, "错误", SEPARATORS)
}

/// 剥掉 markdown 列表/标题前缀：
/// - `# ` / `## ` / `### ` 标题
/// - `- ` / `* ` / `+ ` 无序列表
/// - `1. ` / `123. ` 有序列表
fn strip_markdown_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    // 标题前缀
    let after_hashes = trimmed.trim_start_matches('#').trim_start();
    if after_hashes.len() < trimmed.len() {
        // 真的剥了 #，说明是标题（允许 1~6 个 #）
        return after_hashes;
    }
    // 无序列表
    if let Some(rest) = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('*'))
        .or_else(|| trimmed.strip_prefix('+'))
    {
        if rest.starts_with(' ') {
            return rest;
        }
    }
    // 有序列表 `1. ` / `12. `
    let mut chars = trimmed.chars();
    let mut digits = String::new();
    while let Some(c) = chars.clone().next() {
        if c.is_ascii_digit() {
            digits.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if !digits.is_empty() {
        let rest = &trimmed[digits.len()..];
        if rest.starts_with(". ") {
            return rest;
        }
    }
    // 无前缀，原样返回
    line
}

/// 检查 body 中 keyword 后是否紧跟 SEPARATORS 之一。
/// 「失败重试」不命中（后跟「重」）；「失败：」命中；「失败 」命中。
fn has_keyword_after(body: &str, keyword: &str, separators: &[char]) -> bool {
    let mut start = 0;
    while let Some(pos) = body[start..].find(keyword) {
        let abs = start + pos;
        let after = &body[abs + keyword.len()..];
        match after.chars().next() {
            Some(c) if separators.contains(&c) => return true,
            // 关键词位于行尾（body 末尾）也计命中——「失败」单独一行
            None => return true,
            _ => {}
        }
        start = abs + keyword.len();
    }
    false
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
// 数据目录便携探针 + 探测结果缓存 + 辅助判定函数已全部搬到 paths.rs：
//   - probe_log_dir / cached_probe_dir
//   - probe_dir / probe_dir_cached / probe_dir_cached_in
//   - is_under_system_temp / is_macos_app_bundle_dir
//   - PROBE_CACHE
// audit 模块只负责「目录定了之后日志怎么写」。

// ─────────────────────── 写入公共内核（三个写入点共用） ───────────────────────

/// bot.log rotate 阈值：超过 5 MB 改名为 .old（保留一份历史）。
/// 三个写入点（write_event / write_error_audit / write_warn_audit_to）共用，
/// 由 `write_at` 统一调用，不再各自传常量。
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// 写入公共内核：rotate + BOT_LOG_LOCK + 时间戳 + 行拼装 + 追加一行结构化事件。
///
/// 三个写入点（write_event / write_error_audit / write_warn_audit_to）都走这里，
/// 行格式 `[ts] LEVEL | event | k=v | k=v` 字符级一致——加新写入点不需要再复制
/// rotate/lock/format 那一坨，也不存在「忘了过 escape_for_log」漏路径。
///
/// 返回 bool：成功 true / IO 失败 false。调用方一般不关心（IO 失败已 eprintln
/// 不阻塞业务）；返回给测试断言用。
fn write_at(
    log_path: &std::path::Path,
    level: AuditLevel,
    event: &str,
    kv: &[(&str, &str)],
) -> bool {
    let _g = BOT_LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::db::rotate_log_if_large(log_path, LOG_ROTATE_BYTES);
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = build_event_line(&ts.to_string(), level, event, kv);
    append_line(log_path, &line)
}

/// 写一条结构化审计事件到 `bot.log`（post-execute 钩子主入口）。
/// 复用 `bot::audit_log` 的 rotate 阈值与文件路径，老日志兼容。
/// 泛型 Runtime：mock runtime 测试可直调（与 write_error_audit 同先例）。
pub fn write_event<R: tauri::Runtime>(
    app: &AppHandle<R>,
    level: AuditLevel,
    event: &str,
    kv: &[(&str, String)],
) {
    let p = crate::db::data_dir(app).join("bot.log");
    // 重新分配成 (&str, &str) — build_event_line 拿的是 (&str, &str)，
    // write_event 入口签名故意是 String（macro audit_event! 直接 format!("{}", v)），
    // 一次 to_str 转换代价远小于把整个写入逻辑抄一份
    let kv_refs: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (*k, v.as_str())).collect();
    write_at(&p, level, event, &kv_refs);
}

/// 泛型 Runtime 的 ERROR 审计（profile/middleware 病态路径用）：
/// 目录解析走 paths::probe_log_dir，写入走 write_at——与 write_event 同格式。
pub(crate) fn write_error_audit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    event: &str,
    kv: &[(&str, &str)],
) {
    let p = crate::paths::probe_log_dir(app).join("bot.log");
    write_at(&p, AuditLevel::Error, event, kv);
}

/// 无 AppHandle 场景的 WARN 审计（keyring 降级明文存储路径用）。
/// 目录由调用方解析（与降级 key 文件同目录，保证同一便携位置）。
pub(crate) fn write_warn_audit_to(dir: &std::path::Path, event: &str, kv: &[(&str, &str)]) {
    let p = dir.join("bot.log");
    write_at(&p, AuditLevel::Warn, event, kv);
}

/// 结构化审计事件宏（post-execute 钩子用）
/// 用法：
///   audit_event!(app, AuditLevel::Info, "tool_done",
///       "tool" => "list_tasks", "ms" => 4u64, "refs" => 3usize);
///
/// 带 CommandError 的形态（自动展开 `code` / `recoverable` / `err` 三键）：
///   audit_event!(app, AuditLevel::Error, "llm.request_failed", err => &e, "attempt" => 2u32);
/// 注意 `err` 是关键字位上的标识符，普通 kv 键都是字符串字面量，两条宏臂不会混淆；
/// 但请勿用裸标识符 `err` 当普通 kv 键（会命中本臂）。
#[macro_export]
macro_rules! audit_event {
    ($app:expr, $level:expr, $event:expr, err => $err:expr $(, $k:expr => $v:expr)* $(,)?) => {
        $crate::audit::write_event_with_error($app, $level, $event, $err, &[
            $(($k, format!("{}", $v))),*
        ])
    };
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
        // 「失败」出现在正文而非错误消息（「成功完成任务，含失败回滚说明」），
        // 新规则要求关键词后接分隔符才判失败——「失败」后跟「回」不在 SEPARATORS，
        // 不应误判为 Warn。这正是修复目标
        assert_eq!(
            classify_text("foo", "成功完成任务，含失败回滚说明"),
            AuditLevel::Info
        );
        // 真错误消息「操作失败：磁盘只读」仍判 Warn（首行带分隔符）
        assert_eq!(classify_text("foo", "操作失败：磁盘只读"), AuditLevel::Warn);
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
    fn tool_call_failed_passes_long_read_with_embedded_keywords() {
        // SPEC.md / DEVLOG.md 这类正文中提到「失败/错误」的文档读取结果——
        // 第一行是路径 + 行号摘要（短，无关键词），后续行是正文（长，含关键词）。
        // 旧 contains 全文匹配会把整个读取结果误判为失败，skill_step_post 立即
        // 把 Skill 标 Failed，run_python / build_pptx 等工作步骤还没跑就先死。
        // 新规则：只判短首行（≤120 字符）含「失败/错误」→ 跳过正文里的关键词。
        let spec_md_read = "/Users/renshi/Projects/wmessage/SPEC.md（第 1-127 行 / 共 127 行）\
\n1: # WMessage — SPEC v1\
\n2: \
\n3: > 唯一依据。来源：老板 2026-08-13 发…\
\n…（正文里讨论异常处理、错误码、失败重试）";
        assert!(
            !tool_call_failed("read_text_file", spec_md_read),
            "read_text_file 读到的正文里出现「失败/错误」不应被误判为失败"
        );
        // 第一行 >120 字符（比如长路径）也不误判
        let long_first_line = format!(
            "{}(第 1-500 行 / 共 500 行)\n失败回滚说明：…",
            "/very/long/path/".repeat(20)
        );
        assert!(
            !tool_call_failed("read_text_file", &long_first_line),
            "第一行超 120 字符不应被误判"
        );
        // 真失败消息仍要命中：首行是错误文案、含「失败」/「错误」
        assert!(
            tool_call_failed("x", "执行失败：磁盘只读"),
            "短首行含「失败」应判失败"
        );
        assert!(
            tool_call_failed("x", "读取错误：路径不存在"),
            "短首行含「错误」应判失败"
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

    // ── CommandError → 审计三键（code / recoverable / err）：bot.log 可按 code grep 统计 ──

    fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn error_kv_carries_code_recoverable_and_message() {
        let err = crate::error::CommandError::LlmApiError {
            status: 502,
            body_preview: "bad gateway".into(),
        };
        let kv = error_kv(&err);
        assert_eq!(kv_get(&kv, "code"), Some("LLM_API_ERROR"));
        assert_eq!(kv_get(&kv, "recoverable"), Some("true"));
        assert!(
            kv_get(&kv, "err").unwrap().contains("502"),
            "err 应带人读信息: {kv:?}"
        );
    }

    #[test]
    fn error_event_line_is_greppable_by_code() {
        // 走生产同一份拼装（build_event_line）：行里必然出现 `code=INTERNAL`
        let err = crate::error::CommandError::Internal("boom".into());
        let kv = error_kv(&err);
        let kv_refs: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let line = build_event_line("TIMESTAMP", AuditLevel::Error, "tool.return", &kv_refs);
        assert!(line.contains("ERROR | tool.return"), "{line}");
        assert!(line.contains("code=INTERNAL"), "{line}");
        assert!(line.contains("recoverable=false"), "{line}");
    }
}
