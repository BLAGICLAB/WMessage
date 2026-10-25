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

use crate::error::CommandResult;

/// 审计日志安全转义 + 截断，单源在 [`crate::bot::log_sanitize::escape_for_log`]：
/// `|` 倍增 + `| ` 双空格补位 + \n\r 短形式 + 控制字符全集（含 ESC/BEL/DEL/C1）
/// 与 U+2028/2029 显式转义 + 转义后按字符数截断加省略号。
/// 经此 re-export 保持 `crate::audit::escape_for_log` 路径不变
/// （ask_engine / task_autotag 等按此引用）。
pub(crate) use crate::bot::log_sanitize::escape_for_log;

/// kv 值写入日志前的长度上限
const KV_VALUE_MAX: usize = 500;

/// CommandError → 审计标准三键：`code` / `recoverable` / `err`。
///
/// 用途：bot.log 里失败事件带机器可读 code（`grep 'code=LLM_API_ERROR'` 即可统计失败分布），
/// 不必再解析人读文案。核心模型循环拿不到 AppHandle（审计走注入回调），
/// 所以这里提供 kv 级 helper，与 `write_event_with_error` / `audit_event!(..., err => &e)`
/// 共用同一份拼装，避免两条路径的键名 drift。
/// 从 catch_unwind payload 提取 panic 信息（&str / String / 其他三种情况）。
/// 全仓唯一实现（middleware / api_server / migration 轮询 / bot_scheduler 共用）。
pub(crate) fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    panic_payload_msg(payload.as_ref())
}

/// 引用版 panic 信息提取（panic 钩子的 PanicHookInfo 只给 &dyn Any，
/// 不包装 Box）；Box 版委托这里，两种形态永不 drift。
/// `pub(crate)`：catch_unwind 的 `Box<dyn Any + Send>` 取 `.as_ref()` 复用本函数，
/// 不在钩子运行时另抄一份 payload 解析（两份各自 downcast 会漂移）。
pub(crate) fn panic_payload_msg(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

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

    /// 从工具 status 派生 audit 级别，是唯一从 status → audit 级别的入口。
    ///
    /// 职责边界（status 与文本分工，写死以免日后又长出第二套口径）：
    /// - **status 管「成没成」**——审计级别、trace span 的 ok、调度器的步骤成败判定。
    /// - **文本管「模型读到啥」**——给模型的说明句，以及 `classify_tool_failure`
    ///   提供的错误子类（error_class）。文本不承担成败判定。
    pub fn from_tool_status(s: crate::bot::registry::ToolStatus) -> Self {
        use crate::bot::registry::ToolStatus;
        match s {
            ToolStatus::Ok => AuditLevel::Info,
            ToolStatus::Warn => AuditLevel::Warn,
            ToolStatus::Error => AuditLevel::Error,
            // 被拒是预期内的控制流（闸拦下 / 用户点了否 / 预算触顶），不是故障
            ToolStatus::Denied => AuditLevel::Warn,
        }
    }
}

/// 工具文本失败分类器：`tool_call_failed` 与 `classify_error_class` 共用同一份判定，
/// 杜绝「判定一处、归类另一处各测一遍前缀」的口径漂移。
///
/// **谁在用**：外部 MCP 工具（返回文本不受我们控制、拿不到 `ToolStatus`）、以及
/// 内置工具的 `error_class` 细分（要区分 timeout / permission / gate_denied…）。
/// 内置工具的成败判定走 `ToolStatus`，**不再走这里**——否则同一个结果会同时被
/// 审计判成功、被调度器判失败。
/// 判定臂依序：未知工具 / 门禁「⚠️」/ 用户拒绝 / 技能已暂停 / 已强制终止（contains）/
/// 「失败/错误」关键词后置分隔符 / error: 系行首；都不命中 = 非失败。
fn classify_tool_failure(text: &str) -> Option<ToolFailureClass> {
    if text.starts_with("未知工具") {
        Some(ToolFailureClass::UnknownTool)
    } else if text.starts_with("⚠️") {
        // 门禁拦截（原子工具裸调等）
        Some(ToolFailureClass::GateDenied)
    } else if text.starts_with("用户拒绝") {
        // 确认弹窗被拒绝/超时
        Some(ToolFailureClass::UserDenied)
    } else if text.starts_with("技能已暂停") {
        // step_check 暂停态拒绝
        Some(ToolFailureClass::SkillPaused)
    } else if text.contains("已强制终止") {
        // step_check 步数/超时熔断
        Some(ToolFailureClass::ForcedTerminated)
    } else if failure_keyword_after_separator(text) {
        Some(ToolFailureClass::Keyword)
    } else if text.starts_with("error:") || text.starts_with("Error:") || text.starts_with("ERROR:")
    {
        Some(ToolFailureClass::ErrorPrefix)
    } else {
        None
    }
}

/// 工具失败类别（[classify_tool_failure] 的判定臂一一对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolFailureClass {
    UnknownTool,
    GateDenied,
    UserDenied,
    SkillPaused,
    ForcedTerminated,
    /// 「失败/错误」关键词后紧跟分隔符
    Keyword,
    /// error: / Error: / ERROR: 行首
    ErrorPrefix,
}

/// 工具调用文本是否「看起来像失败/被拒」。**只服务拿不到 `ToolStatus` 的路径**
/// （外部 MCP 工具），以及作为内置工具 `error_class` 子类的推导入口。
///
/// 内置工具的成败判定走 `registry::ToolStatus`（见 `dispatch::span_ok`），
/// 不要在这里给内置工具的成败兜底——两套口径同时判，早晚会给同一个结果
/// 两种答案：之前就发生过——5 处闸拦/拒绝/预算触顶的文案既不带「⚠️」也不带
/// 「失败：」，审计记 WARN 而 Skill DSL 调度器把被拒步骤记成成功。
///
/// 熔断「已强制终止」、暂停「技能已暂停」、门禁拦截「⚠️」、用户拒绝「用户拒绝」
/// 四类文案与常规失败前缀都要判失败；否则会出现末步熔断误报「✅ 完成」、门禁拦截
/// 对 PREVR 隐身、被拒删除当成功。
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
    classify_tool_failure(text).is_some()
}

/// 错误分类器（与 tool_call_failed 同走 [classify_tool_failure] 单一来源，供
/// trace span error_class 用，两处不再各写
/// 一份口径）。成功文本返回 None；五个具名失败直接映射，关键词/错误前缀两类
/// 失败再按内容细分（超时/不存在/权限），未识别失败统一 "exec_error"。
pub fn classify_error_class(text: &str) -> Option<&'static str> {
    match classify_tool_failure(text)? {
        ToolFailureClass::UnknownTool => Some("unknown_tool"),
        ToolFailureClass::GateDenied => Some("gate_denied"),
        ToolFailureClass::UserDenied => Some("user_denied"),
        ToolFailureClass::SkillPaused => Some("skill_paused"),
        ToolFailureClass::ForcedTerminated => Some("fused"),
        ToolFailureClass::Keyword | ToolFailureClass::ErrorPrefix => {
            if text.contains("超时") || text.contains("timeout") || text.contains("Timeout") {
                Some("timeout")
            } else if text.contains("不存在")
                || text.contains("没有找到")
                || text.contains("not found")
            {
                Some("not_found")
            } else if text.contains("权限") || text.contains("白名单") || text.contains("未授权")
            {
                Some("permission")
            } else {
                Some("exec_error")
            }
        }
    }
}

#[cfg(test)]
mod error_class_tests;

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
/// 同步专用契约：持锁跨越整段阻塞文件 IO（rotate + open + chmod + write），
/// 不得在 tokio worker / 延迟敏感路径上直接调用；这类调用方应先投递到
/// 普通 std::thread 再写。
pub static BOT_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 日志文件打开：先建父目录，再创建即 0600 + 已有文件补 chmod——
/// bot.log 含用户指令/文件路径/工具输出，umask 默认 0644 下同机其他用户可读。
/// 三处写入点（audit::append_line / bot::audit_log / bot_py::py_audit_to）共用。
///
/// 父目录必须在这里建：生产里数据目录由 open_db 建好，但审计写入不依赖 DB
/// （故意如此：库坏了也要留痕），一旦目录不存在，写入会静默变成一行
/// `[audit] write failed: No such file or directory`——审计在最该留痕的时候
/// 恰恰没留痕。
pub(crate) fn open_log_append(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
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
        // 已存在文件 mode() 不生效，补 chmod（幂等）。失败不能静默：
        // bot.log 含用户指令/路径/工具输出，权限没收紧等于敏感数据裸奔
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            eprintln!("[audit] chmod 0o600 failed: {} path={}", e, path.display());
        }
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

// 写入公共内核（三个写入点共用）

/// 日志 rotate 阈值：超过 5 MB 轮转让位（保留两代历史，见
/// rotate_log_if_large）。bot.log / error.log 镜像共用同一阈值。
/// 三个写入点（write_event / write_error_audit / write_warn_audit_to）共用，
/// 由 `write_at` 统一调用，不再各自传常量。
const LOG_ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// error.log 文件名（与所写 bot.log 同目录）：WARN/ERROR 结构化事件的镜像账本。
/// 机器人工具流水（bot/py 写入点，无级别自由行）**不**镜像——那是上下文不是
/// 信号；信号账本不被流水稀释，5MB 能覆盖数月，诊断卡与问题反推以它为准。
const ERROR_LOG_NAME: &str = "error.log";

/// error.log 写锁：与 BOT_LOG_LOCK 分离——镜像不必与 Info 流水互斥，
/// 两文件独立 append 互不影响。rotate + write 同锁（同 bot.log 契约）。
static ERROR_LOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 写入公共内核：rotate + BOT_LOG_LOCK + 时间戳 + 行拼装 + 追加一行结构化事件，
/// WARN/ERROR 镜像一份到同目录 error.log。
///
/// 三个写入点（write_event / write_error_audit / write_warn_audit_to）都走这里，
/// 行格式 `[ts] LEVEL | event | k=v` 字符级一致——加新写入点不需要再复制
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
    let _g = crate::db::lock::lock_or_recover(&BOT_LOG_LOCK, "audit::BOT_LOG_LOCK");
    crate::db::rotate_log_if_large(log_path, LOG_ROTATE_BYTES);
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = build_event_line(&ts.to_string(), level, event, kv);
    let ok = append_line(log_path, &line);
    // 镜像失败不回滚主文件写入（append_line 内已 eprintln 留痕）
    if matches!(level, AuditLevel::Warn | AuditLevel::Error) {
        let _ = mirror_error_log(log_path, &line);
    }
    ok
}

/// WARN/ERROR 行镜像到同目录 error.log（独立锁 + 独立轮转，见 ERROR_LOG_NAME）。
/// 返回成功与否供测试断言。
fn mirror_error_log(bot_log_path: &std::path::Path, line: &str) -> bool {
    let error_path = bot_log_path.with_file_name(ERROR_LOG_NAME);
    let _g = crate::db::lock::lock_or_recover(&ERROR_LOG_LOCK, "audit::ERROR_LOG_LOCK");
    crate::db::rotate_log_if_large(&error_path, LOG_ROTATE_BYTES);
    append_line(&error_path, line)
}

/// 读 error.log 尾部（倒序最新在前）——**路径参数化**：单测走 tempdir 隔离，
/// 不受并行测试的审计写入污染（tauri command 本身读的是共享数据目录）。
fn error_log_tail_from(path: &std::path::Path, limit: Option<usize>) -> CommandResult<String> {
    crate::bot::config::audit::read_log_tail(path, limit)
}

/// 诊断卡读 error.log 尾部（倒序最新在前；默认 200 行上限 2000）。
/// NotFound → 占位文案（日志确实没东西）；其他 IO 错误 → Err + ERROR 审计。
/// 复用 bot_log 的 read_log_tail（同一行导向格式，同一容错语义）。
/// 泛型 Runtime：mock runtime 测试可直调（与 write_event 同先例）。
#[tauri::command]
pub fn error_log_read<R: tauri::Runtime>(
    app: AppHandle<R>,
    limit: Option<usize>,
) -> CommandResult<String> {
    let p = crate::db::data_dir(&app).join(ERROR_LOG_NAME);
    match error_log_tail_from(&p, limit) {
        Ok(s) => Ok(s),
        Err(e) => {
            let err = e.to_string();
            crate::audit_event!(
                &app,
                AuditLevel::Error,
                "error_log_read_fail",
                "err" => err
            );
            Err(e)
        }
    }
}

/// 「复制全部日志」单文件尾部上限：剪掉超出部分（按行对齐），防 12 份文件
/// 全量拼接出几 MB 剪贴板文本
const BUNDLE_FILE_CAP_BYTES: usize = 256 * 1024;

/// 打包内核（路径参数化便于单测，tauri command 绑定 Runtime 后 mock 直调仍共享
/// 数据目录，精确的顺序/截断断言需要 tempdir 隔离）
fn bundle_text_from_dir(dir: &std::path::Path) -> String {
    let names = [
        "error.log",
        "error.log.old",
        "error.log.old.1",
        "bot.log",
        "bot.log.old",
        "bot.log.old.1",
        "api.log",
        "api.log.old",
        "api.log.old.1",
        "migration.log",
        "migration.log.old",
        "migration.log.old.1",
    ];
    let mut out = String::new();
    for name in names {
        let p = dir.join(name);
        let Ok(raw) = std::fs::read(&p) else {
            continue; // 缺失（从未写过/刚轮转掉）直接跳过
        };
        out.push_str(&format!("===== {name} =====\n"));
        let content = String::from_utf8_lossy(&raw);
        if content.len() > BUNDLE_FILE_CAP_BYTES {
            // 尾部按行对齐 + 字符边界对齐（日志含中文，字节切会劈开 UTF-8 码点）
            let mut cut = content.len() - BUNDLE_FILE_CAP_BYTES;
            while cut < content.len() && !content.is_char_boundary(cut) {
                cut += 1;
            }
            let start = content[cut..]
                .find('\n')
                .map(|i| cut + i + 1)
                .unwrap_or(content.len());
            out.push_str("[已截断，仅含尾部]\n");
            out.push_str(&content[start..]);
        } else {
            out.push_str(&content);
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if out.is_empty() {
        return "（暂无日志）".into();
    }
    out
}

/// 「复制全部日志」打包文本（诊断卡一键给开发者的全量账本）：
/// error 镜像在前（信号优先），bot/api/migration 流水随后；文件缺失跳过。
/// 泛型 Runtime：mock runtime 测试可直调。
#[tauri::command]
pub fn logs_bundle_text<R: tauri::Runtime>(app: AppHandle<R>) -> CommandResult<String> {
    Ok(bundle_text_from_dir(&crate::db::data_dir(&app)))
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

/// 前端上报去重闸：同签名（event+detail 指纹）只记首条，签名集上限 32——
/// 防 WebView 内违规风暴刷爆审计日志（重复违规对排查无增量价值）。
/// Vec 而非 HashSet：static 初始化需 const（HashSet::new 非 const），
/// 容量 32 内线性查找代价可忽略。
static FRONTEND_REPORT_SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
const FRONTEND_REPORT_MAX_SIGNATURES: usize = 32;

/// 前端事件上报 → 审计日志。
/// 目前唯一来源：WebView 的 securitypolicyviolation（CSP 违规，生产 csp 已
/// 收紧，运行时违规必须可感知）。签名去重防风暴；容量满后新签名静默丢弃
///（最早的重复依然会被去重，丢弃的是「全新违例」，可接受——审计非计量）。
#[tauri::command]
pub fn frontend_event_report(app: AppHandle, event: String, detail: String) -> Result<(), String> {
    const MAX_DETAIL: usize = 200;
    // 指纹按字符截断（40 字符）：detail 常含中文，按字节切会切在多字节字符
    // 中间直接 panic——审计路径禁 panic
    let fingerprint = format!("{event}|{}", detail.chars().take(40).collect::<String>());
    {
        let mut seen = FRONTEND_REPORT_SEEN
            .lock()
            .map_err(|e| format!("前端上报去重表锁失败：{e}"))?;
        if seen.len() >= FRONTEND_REPORT_MAX_SIGNATURES && !seen.contains(&fingerprint) {
            return Ok(()); // 签名集满：静默丢弃全新违例（防风暴，审计非计量）
        }
        if !seen.contains(&fingerprint) {
            seen.push(fingerprint);
        }
    }
    write_event(
        &app,
        AuditLevel::Warn,
        &format!("frontend.{event}"),
        &[("detail", escape_for_log(&detail, MAX_DETAIL))],
    );
    Ok(())
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

/// 全局 panic 钩子（lib setup 安装一次）：panic 写 ERROR 审计（随 write_at
/// 镜像 error.log），线程静默死亡从此可反推。目录在安装时解析固化——钩子内
/// 不碰 AppHandle / 目录探测缓存：panic 时任何锁都可能正被持有，钩子只做
/// 「拼行 + append」最小动作（锁被毒化时 lock_or_recover 自恢复）。
/// 原默认钩子链式保留：dev 终端 stderr 输出行为不变。
pub fn install_panic_hook(log_dir: std::path::PathBuf) {
    let bot_log = log_dir.join("bot.log");
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = escape_for_log(&panic_payload_msg(info.payload()), 2000);
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        write_at(
            &bot_log,
            AuditLevel::Error,
            "panic",
            &[("thread", &thread), ("loc", &loc), ("msg", &msg)],
        );
        prev(info);
    }));
}

/// `log` crate 门面（lib setup 安装一次）：新代码用标准宏 `log::warn!` 等
/// 直接落 bot.log（行格式 `[ts] LEVEL | <target> | msg=<消息>`），不必再找
/// 需要 AppHandle 的 audit_event!；WARN/ERROR 随 write_at 自动镜像 error.log。
/// 两个纪律：
/// - 门面在 setup 首行就位，之前的 log 宏调用被静默丢弃（log crate 未初始化
///   语义）——启动最早期路径仍用 audit_event! / write_at；
/// - 本实现写失败走 append_line 的 eprintln（日志自身失败禁止再发 log 宏，
///   否则递归）。audit 自身的写失败 eprintln 因此永不迁移。
pub struct AuditLogger {
    dir: std::path::PathBuf,
}

impl AuditLogger {
    pub fn new(dir: std::path::PathBuf) -> Self {
        Self { dir }
    }
}

impl log::Log for AuditLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => AuditLevel::Error,
            log::Level::Warn => AuditLevel::Warn,
            log::Level::Info => AuditLevel::Info,
            // max_level Info 已挡，这里双保险（防 enabled 语义被放宽）
            log::Level::Debug | log::Level::Trace => return,
        };
        let msg = escape_for_log(&record.args().to_string(), 2000);
        write_at(
            &self.dir.join("bot.log"),
            level,
            record.target(),
            &[("msg", &msg)],
        );
    }

    fn flush(&self) {}
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

/// 测试串行锁：凡「删除/读全量共享 bot.log」（target/debug/deps 下探针目录）
/// 的用例必须持有——进程内并行下两个用例会互相删对方正在读的文件
///（NotFound 竞态实测复现）。对齐 skill_e2e 的 SKILL_SCHED_TEST_LOCK 模式。
#[cfg(test)]
pub(crate) static BOT_LOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod freeform_freeze;

#[cfg(test)]
mod tests;
