use super::*;

/// 工具状态 → 审计级别：被拒是预期内控制流（闸拦下 / 用户点了否 / 预算触顶），
/// 记 ERROR 会把「闸拦下」写进 error.log，让正常控制流看起来像故障。
#[test]
fn tool_status_denied_is_warn_and_error_is_error() {
    use crate::bot::registry::ToolStatus;
    assert_eq!(
        AuditLevel::from_tool_status(ToolStatus::Denied),
        AuditLevel::Warn,
        "被闸拦下不是故障：级别必须是 WARN"
    );
    assert_eq!(
        AuditLevel::from_tool_status(ToolStatus::Error),
        AuditLevel::Error,
        "执行报错才是 ERROR"
    );
}

// ── 工具状态守卫：失败/拒绝语义的文案不得构造 ok / warn ──

/// 构造器名（源码里以字面量出现，扫描时按「落在字符串/注释里」跳过——
/// 所以这里写全名不会让守卫扫到自己）。
const OK_CTOR: &str = "ToolResult::ok(";
const WARN_CTOR: &str = "ToolResult::warn(";

/// 失败语标记：命中即视为「这段文案不该配 ok / warn」。
/// 「失败 / 错误」要求后接分隔符（见 `failure_reason`），其余是直查。
const FAILURE_MARKERS: &[&str] = &[
    "拒绝",
    "缺少",
    "不能为空",
    "未找到",
    "不存在",
    "非法",
    "无效",
    "未获确认",
    "没有可用",
    "不可用",
    "禁用",
];

/// 良性命中：文案里带失败语词但不是失败事件。
/// 每条都带理由——加新条目时必须写清为什么不是失败。
const BENIGN_FAILURE_TEXT: &[(&str, &str)] = &[
    ("逐条接受/拒绝", "Word 修订模式给用户的操作指引（成功文案）"),
    (
        "提问队列暂不可用",
        "提问队列故障是设计内降级：按模型声明的假设继续，不是失败事件",
    ),
];

/// 「失败 / 错误」后接分隔符才算失败——避免「失败把报错发回聊天」这类正常文案。
const FAILURE_SEPARATORS: &str = "：:，,、；;（(）) \n";

/// 判定一段文案是否带失败/拒绝语义；返回命中的标记名。
fn failure_reason(lit: &str) -> Option<&'static str> {
    for (needle, _why) in BENIGN_FAILURE_TEXT {
        if lit.contains(needle) {
            return None;
        }
    }
    for kw in ["失败", "错误"] {
        let mut from = 0;
        while let Some(rel) = lit[from..].find(kw) {
            let at = from + rel;
            let next = lit[at + kw.len()..].chars().next();
            if next.is_some_and(|c| FAILURE_SEPARATORS.contains(c)) {
                return Some(kw);
            }
            from = at + kw.len();
        }
    }
    FAILURE_MARKERS.iter().copied().find(|m| lit.contains(m))
}

/// 源码词法：切出字符串字面量与注释的区间。
/// 返回 (字面量区间, 注释区间)，每个字面量区间是 (起, 内容起, 止)。
fn lex_source(text: &str) -> (Vec<(usize, usize, usize)>, Vec<(usize, usize)>) {
    let b = text.as_bytes();
    let mut i = 0usize;
    let mut literals: Vec<(usize, usize, usize)> = Vec::new();
    let mut comments: Vec<(usize, usize)> = Vec::new();
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let start = i;
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                comments.push((start, i));
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let start = i;
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                comments.push((start, i));
            }
            b'r' if matches!(b.get(i + 1), Some(b'"') | Some(b'#')) => {
                let mut j = i + 1;
                while b.get(j) == Some(&b'#') {
                    j += 1;
                }
                if b.get(j) == Some(&b'"') {
                    let hashes = j - i - 1;
                    let marker = format!("\"{}", "#".repeat(hashes));
                    let mut k = j + 1;
                    while k < text.len() {
                        // 只在字符边界上切（raw string 里常有中文）
                        if text.is_char_boundary(k) && text[k..].starts_with(&marker) {
                            k += marker.len();
                            break;
                        }
                        k += 1;
                    }
                    literals.push((i, j + 1, k));
                    i = k;
                    continue;
                }
                i += 1;
            }
            b'"' => {
                let start = i;
                let content = i + 1;
                let mut j = content;
                while j < b.len() {
                    if b[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if b[j] == b'"' {
                        break;
                    }
                    j += 1;
                }
                let end = (j + 1).min(b.len());
                literals.push((start, content, end));
                i = end;
            }
            _ => i += 1,
        }
    }
    (literals, comments)
}

fn span_contains(spans: &[(usize, usize)], pos: usize) -> bool {
    spans.iter().any(|(s, e)| pos >= *s && pos < *e)
}

/// 找构造器调用的**首个字符串实参**：实参是字面量或 `format!("…")` 时返回其内容区间。
fn first_literal_arg(
    text: &str,
    literals: &[(usize, usize, usize)],
    after_paren: usize,
) -> Option<(usize, usize)> {
    let mut p = after_paren;
    let bytes = text.as_bytes();
    while p < bytes.len() && bytes[p].is_ascii_whitespace() {
        p += 1;
    }
    if text[p..].starts_with("format!") {
        p += "format!".len();
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
        if bytes.get(p) != Some(&b'(') {
            return None;
        }
        p += 1;
        while p < bytes.len() && bytes[p].is_ascii_whitespace() {
            p += 1;
        }
    }
    literals
        .iter()
        .find(|(s, _, _)| *s == p)
        .map(|(_, content, end)| (*content, *end))
}

/// 扫一段源码里所有「构造器 + 首个字符串实参」，返回带失败语义的文案与其位置。
fn scan_text_for_failure(text: &str, ctor: &str) -> Vec<(usize, String)> {
    let (literals, comments) = lex_source(text);
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(ctor) {
        let pos = from + rel;
        from = pos + ctor.len();
        // 源码里「提到」构造器名不等于调用它（注释 / 字面量里的不算）
        if span_contains(&comments, pos) || literals.iter().any(|(s, _, e)| pos >= *s && pos < *e) {
            continue;
        }
        let Some((cs, ce)) = first_literal_arg(text, &literals, pos + ctor.len()) else {
            continue;
        };
        let lit = &text[cs..ce];
        if let Some(reason) = failure_reason(lit) {
            out.push((cs, format!("{reason}：{lit}")));
        }
    }
    out
}

/// 回归锁：文案带失败/拒绝语义的返回点不得构造 `ok` / `warn`。
///
/// status 是成败唯一真相源（见 `registry::ToolStatus::is_success`）：`Warn`
/// 也算成功，所以「做成了但有告警」这条留白不适用于失败文案——被拒 /
/// 执行失败的返回点一旦记成成功，Skill DSL 调度器会把失败步骤记成技能
/// 正常完成，trace span 记 ok，幻觉守卫的凭证表里还会留下
/// 「delete_task 成功过」，模型据此汇报「已删除任务卡」也不会被拦。
///
/// 判定对象是构造器的**首个字符串实参**：注释、代码、非首参都不参与。
/// 变量实参（`ToolResult::ok(e, …)`）静态扫不到——那些点的成败由
/// 调用方按错误类型给，文案不承载语义。
#[test]
fn failure_texted_results_never_use_ok_or_warn() {
    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_rs(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&root, &mut files);
    assert!(!files.is_empty(), "应至少扫到一个 .rs 文件");

    let mut offenders: Vec<String> = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for ctor in [OK_CTOR, WARN_CTOR] {
            for (at, hit) in scan_text_for_failure(&text, ctor) {
                offenders.push(format!(
                    "{}:{}: {}",
                    f.strip_prefix(&root).unwrap_or(&f).display(),
                    text[..at].lines().count() + 1,
                    hit
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "文案带失败/拒绝语义的返回点不得构造 ok/warn，应改 ToolResult::denied / error：\n{}",
        offenders.join("\n")
    );
}

/// 守卫自测：扫描器本身必须抓到违例、放过正常文案。
///
/// 上一版守卫长期空转（把生产代码切反了，只扫测试代码；且只认
/// 「失败：/错误：」两种形态）。没有正例夹具就看不出它其实什么也没扫——
/// 这类静态守卫必须自带自测。
#[test]
fn ok_warn_failure_scanner_catches_violations() {
    // 违例：参数缺失 / 执行失败 / 拒绝 / 目标不存在
    for bad in [
        "ToolResult::ok(\"create_task 缺少 title\".to_string(), Vec::new());",
        "ToolResult::ok(\"路径不存在，拒绝绑定：{path}\", Vec::new());",
        "ToolResult::ok(\"未找到 id={id} 的任务卡\", Vec::new());",
    ] {
        assert!(
            !scan_text_for_failure(bad, OK_CTOR).is_empty(),
            "应被抓到：{bad}"
        );
    }
    assert!(
        !scan_text_for_failure(
            "ToolResult::ok(format!(\"删除失败：{e}\"), Vec::new())",
            OK_CTOR
        )
        .is_empty(),
        "多行 format! 的失败文案应被抓到"
    );
    assert!(
        !scan_text_for_failure("ToolResult::warn(\"用户拒绝了删除\", vec![]);", WARN_CTOR)
            .is_empty(),
        "warn 同样是成功态，失败文案不得用 warn"
    );
    // 正常文案：成功 / 空结果 / 变量实参 / 注释与字面量里的同形文本
    for good in [
        "ToolResult::ok(\"已删除任务「x」（进回收站）\", Vec::new());",
        "ToolResult::ok(\"没有找到相关记忆\", Vec::new());",
        "ToolResult::ok(lines.join(\"\\n\"), refs);",
        "// ToolResult::ok(\"失败：注释里的不算\")",
        "let s = \"ToolResult::ok(\\\"失败：字面量里的不算\\\")\";",
    ] {
        assert!(
            scan_text_for_failure(good, OK_CTOR).is_empty(),
            "不应被抓到：{good}"
        );
    }
    // 良性清单：带失败语词但不是失败事件
    assert!(
        scan_text_for_failure(
            "ToolResult::ok(\"已生成修订版 Word（逐条接受/拒绝）：{out}\", vec![]);",
            OK_CTOR
        )
        .is_empty(),
        "良性清单里的文案不应被抓到"
    );
}

#[test]
fn level_as_tag_three_levels() {
    assert_eq!(AuditLevel::Info.as_tag(), "INFO");
    assert_eq!(AuditLevel::Warn.as_tag(), "WARN");
    assert_eq!(AuditLevel::Error.as_tag(), "ERROR");
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
    // 长文档（README/规范等）正文里出现「失败/错误」的文档读取结果——
    // 第一行是路径 + 行号摘要（短，无关键词），后续行是正文（长，含关键词）。
    // 旧 contains 全文匹配会把整个读取结果误判为失败，skill_step_post 立即
    // 把 Skill 标 Failed，run_python / build_pptx 等工作步骤还没跑就先死。
    // 新规则：只判短首行（≤120 字符）含「失败/错误」→ 跳过正文里的关键词。
    let spec_md_read = "/Users/renshi/Projects/wmessage/SPEC.md（第 1-127 行 / 共 127 行）\
 # WMessage — SPEC v1\
 \
 > 唯一依据。来源：老板 2026-08-13 发…\
正文里讨论异常处理、错误码、失败重试）";
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

// ── error.log 镜像（WARN/ERROR 结构化事件，Info 不镜像）──

#[test]
fn warn_and_error_mirror_to_error_log() {
    let tmp = tempfile::tempdir().unwrap();
    let bot = tmp.path().join("bot.log");
    assert!(write_at(&bot, AuditLevel::Info, "tool_done", &[("k", "v")]));
    assert!(write_at(&bot, AuditLevel::Warn, "degraded", &[]));
    assert!(write_at(&bot, AuditLevel::Error, "boom", &[("err", "x")]));

    let err_log = tmp.path().join("error.log");
    let content = std::fs::read_to_string(&err_log).expect("WARN 应镜像出 error.log");
    let mirrored: Vec<&str> = content.lines().collect();
    assert_eq!(mirrored.len(), 2, "Info 不得进 error.log：{content}");
    assert!(mirrored[0].contains("WARN | degraded"), "{}", mirrored[0]);
    assert!(mirrored[1].contains("ERROR | boom"), "{}", mirrored[1]);

    // bot.log 与 error.log 的对应行字符级一致（同一行文本两次落盘）
    let bot_content = std::fs::read_to_string(&bot).unwrap();
    let bot_lines: Vec<&str> = bot_content.lines().collect();
    assert_eq!(bot_lines.len(), 3);
    assert_eq!(bot_lines[1], mirrored[0]);
    assert_eq!(bot_lines[2], mirrored[1]);
}

#[test]
fn mirror_error_log_rotates_with_two_generations() {
    // KV 值有 500 字符上限，超大行走 mirror_error_log 直测（append_line 不截断）。
    // 注意轮转时机：某笔写入使文件过限后，**下一笔**写入时才改名让位
    let tmp = tempfile::tempdir().unwrap();
    let bot = tmp.path().join("bot.log");
    let small = |tag: &str| format!("{tag}-{}", "x".repeat(60));
    let flood = || "f".repeat(5 * 1024 * 1024 + 16);

    assert!(mirror_error_log(&bot, &small("a")));
    assert!(!tmp.path().join("error.log.old").exists(), "未超限不轮转");

    // 灌过限大行（此时只追加不轮转），下一笔才触发第一次轮转
    assert!(mirror_error_log(&bot, &flood()));
    assert!(mirror_error_log(&bot, &small("b")), "过限后的下一笔应轮转");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("error.log.old")).unwrap(),
        format!("{}\n{}\n", small("a"), flood()),
        ".old = 第一代（a + flood）"
    );

    // 第二次过限 → 下一笔把第一代挤到 .old.1
    assert!(mirror_error_log(&bot, &flood()));
    assert!(mirror_error_log(&bot, &small("c")));
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("error.log.old.1")).unwrap(),
        format!("{}\n{}\n", small("a"), flood()),
        ".old.1 = 第一代（被挤出）"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("error.log.old")).unwrap(),
        format!("{}\n{}\n", small("b"), flood()),
        ".old = 第二代"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("error.log")).unwrap(),
        format!("{}\n", small("c")),
        "error.log 从头长"
    );
    assert!(!tmp.path().join("error.log.old.2").exists(), "只保两代");
}

// ── 全局 panic 钩子 ──

#[test]
fn panic_hook_writes_error_audit_and_mirrors() {
    let tmp = tempfile::tempdir().unwrap();
    // 钩子是进程级全局：先存默认钩子，测完立即恢复（cargo test 并行线程
    // 期间若有他测 panic，最坏多写本 tempdir 一行，无害）
    let prev = std::panic::take_hook();
    install_panic_hook(tmp.path().to_path_buf());
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        panic!("hook-test-boom");
    }));
    std::panic::set_hook(prev);
    assert!(r.is_err(), "panic 应被 catch_unwind 捕获");

    // 按内容查找而非取最后一行：cargo test 并行下，他测的故意 panic
    // （如 bot_py spawn_blocking panic 映射用例）也可能被钩子记进同一 tempdir
    let content = std::fs::read_to_string(tmp.path().join("bot.log")).unwrap();
    let line = content
        .lines()
        .find(|l| l.contains("msg=hook-test-boom"))
        .expect("panic 行应写入 bot.log");
    assert!(line.contains("ERROR | panic"), "{line}");
    assert!(line.contains("thread="), "{line}");
    assert!(line.contains("loc="), "{line}");

    // ERROR 级随 write_at 镜像 error.log
    let mirrored = std::fs::read_to_string(tmp.path().join("error.log")).unwrap();
    assert!(
        mirrored.contains("msg=hook-test-boom"),
        "panic 行应镜像 error.log：{mirrored}"
    );
}

#[test]
fn panic_payload_msg_covers_three_payload_kinds() {
    assert_eq!(panic_payload_msg(&"str-payload"), "str-payload");
    assert_eq!(
        panic_payload_msg(&String::from("string-payload")),
        "string-payload"
    );
    assert_eq!(
        panic_payload_msg(&42u32),
        "non-string panic payload",
        "非字符串 payload 不 panic、给占位文案"
    );
}

// ── log crate 门面（AuditLogger）──

#[test]
fn audit_logger_maps_levels_targets_and_mirrors() {
    use log::Log as _;
    let tmp = tempfile::tempdir().unwrap();
    let logger = AuditLogger::new(tmp.path().to_path_buf());

    // Record 借用 args，不能跨语句返回——宏保证构造与使用同语句
    macro_rules! rec {
        ($level:expr, $msg:expr) => {
            log::Record::builder()
                .args(format_args!($msg))
                .level($level)
                .target("my_mod::path")
                .build()
        };
    }
    logger.log(&rec!(log::Level::Info, "info-msg"));
    logger.log(&rec!(log::Level::Warn, "warn-msg"));
    logger.log(&rec!(log::Level::Error, "error-msg"));
    logger.log(&rec!(log::Level::Debug, "debug-msg"));

    let bot = std::fs::read_to_string(tmp.path().join("bot.log")).unwrap();
    assert!(bot.contains("INFO | my_mod::path | msg=info-msg"), "{bot}");
    assert!(bot.contains("WARN | my_mod::path | msg=warn-msg"), "{bot}");
    assert!(
        bot.contains("ERROR | my_mod::path | msg=error-msg"),
        "{bot}"
    );
    assert!(!bot.contains("debug-msg"), "Debug 不得落盘: {bot}");

    // WARN/ERROR 镜像 error.log，Info 不镜像
    let err_log = std::fs::read_to_string(tmp.path().join("error.log")).unwrap();
    let lines: Vec<&str> = err_log.lines().collect();
    assert_eq!(lines.len(), 2, "Info 不镜像: {err_log}");
    assert!(lines[0].contains("warn-msg"));
    assert!(lines[1].contains("error-msg"));

    let md = |level: log::Level| log::Metadata::builder().level(level).target("t").build();
    assert!(logger.enabled(&md(log::Level::Info)), "Info 在门限内");
    assert!(!logger.enabled(&md(log::Level::Debug)), "Debug 被门限挡下");
}

// ── 诊断卡命令（error_log_read / logs_bundle_text）──

#[test]
fn error_log_read_returns_tail_newest_first() {
    // tempdir 隔离（与 logs_bundle_text 那条同一口径）：共享 data_dir 会被
    // **并行测试**的审计写入镜像进同一个 error.log——「删掉后读回占位文案」
    // 这条断言会被挤掉（别的模块正好写了一行 ERROR），与本用例要验的
    // 「倒序最新在前」毫无关系，却会让门禁随机红。
    let tmp = tempfile::tempdir().unwrap();
    let bot = tmp.path().join("bot.log");
    let err_log = tmp.path().join("error.log");

    // 无文件 → 占位文案
    assert_eq!(error_log_tail_from(&err_log, None).unwrap(), "（暂无日志）");

    // WARN 先、ERROR 后 → 读回来是倒序（最新在前）
    write_at(&bot, AuditLevel::Warn, "w1-ord", &[]);
    write_at(&bot, AuditLevel::Error, "e2-ord", &[]);
    let s = error_log_tail_from(&err_log, None).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    let i_w = lines
        .iter()
        .rposition(|l| l.contains("WARN | w1-ord"))
        .expect("w1 应在");
    let i_e = lines
        .iter()
        .rposition(|l| l.contains("ERROR | e2-ord"))
        .expect("e2 应在");
    assert!(i_e < i_w, "倒序最新在前: {s}");
}

#[test]
fn logs_bundle_text_orders_signals_first_caps_and_skips_missing() {
    let tmp = tempfile::tempdir().unwrap(); // tempdir 隔离：精确断言不受并行审计写入污染
    let dir = tmp.path();

    // 全缺 → 占位
    assert_eq!(bundle_text_from_dir(dir), "（暂无日志）");

    // error.log（信号）+ bot.log + api.log（流水）；migration.log 保持缺失
    std::fs::write(dir.join("error.log"), "[t] ERROR | boom-main\n").unwrap();
    std::fs::write(dir.join("bot.log"), "[t] tool line\n").unwrap();
    std::fs::write(dir.join("api.log"), "[2026-01-01 00:00:00] GET /tasks\n").unwrap();

    let s = bundle_text_from_dir(dir);
    let err_pos = s.find("===== error.log =====").expect("error 段应在");
    let bot_pos = s.find("===== bot.log =====").expect("bot 段应在");
    let api_pos = s.find("===== api.log =====").expect("api 段应在");
    assert!(
        err_pos < bot_pos && bot_pos < api_pos,
        "信号（error）先于流水（bot/api）: {s}"
    );
    assert!(
        !s.contains("migration.log ====="),
        "缺失文件跳过无段头: {s}"
    );
    assert!(s.contains("boom-main"));

    // 截断路径：超 256KB 的文件只保留尾部（按行对齐）+ 注明截断
    let big = "b".repeat(300 * 1024);
    std::fs::write(dir.join("error.log"), format!("{big}\nlast-line\n")).unwrap();
    let s2 = bundle_text_from_dir(dir);
    let seg = s2.split("===== bot.log =====").next().unwrap();
    assert!(seg.contains("[已截断，仅含尾部]"), "应注明截断");
    assert!(seg.contains("last-line"), "尾部应完整保留");
    assert!(!seg.contains(&big), "超界部分应剪掉");

    // 多字节内容：按字节截断不得劈开 UTF-8 码点（panic 即回归）
    let multi = "汉".repeat(200 * 1024);
    std::fs::write(dir.join("migration.log"), format!("{multi}\n尾行\n")).unwrap();
    let s3 = bundle_text_from_dir(dir);
    assert!(s3.contains("尾行"), "多字节文件截断后尾部完整");
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

/// frontend_event_report 的 detail 只转义一次：转义统一发生在 write_event
/// 的拼装内核（append_kv_escaped），入口只做截断——入口再转一遍会把 `\n`
/// 落成字面反斜杠 + n（双重转义），日志里读不回真实内容。
#[test]
fn frontend_event_report_escapes_detail_exactly_once() {
    // bot.log 与其他用例共享：持测试串行锁 + 清残留（与
    // p2_6_1_zero_text_audit_kv_writes_to_bot_log 同纪律）
    let _serial = crate::db::lock::lock_or_recover(&BOT_LOG_TEST_LOCK, "audit::BOT_LOG_TEST_LOCK");
    let app = tauri::test::mock_app();
    let bot_log = crate::db::data_dir(app.handle()).join("bot.log");
    let _ = std::fs::remove_file(&bot_log);
    frontend_event_report(
        app.handle().clone(),
        "csp_violation".to_string(),
        "evil\n[2026-01-01] forged | line".to_string(),
    )
    .expect("前端上报应成功");
    let content = std::fs::read_to_string(&bot_log).expect("上报应写出 bot.log");
    let last_line = content.lines().last().expect("至少一行");
    assert!(
        last_line.contains("WARN | frontend.csp_violation"),
        "事件名缺失: {last_line}"
    );
    assert!(
        !last_line.contains('\n'),
        "detail 里的裸换行必须被转义: {last_line:?}"
    );
    // 单次转义形态：\n → 字面 \n 两字符、| → ||
    assert!(
        last_line.contains("evil\\n[2026-01-01] forged ||  line"),
        "detail 应恰好转义一次: {last_line:?}"
    );
    // 双重转义形态（\\n）不得出现
    assert!(
        !last_line.contains("\\\\n"),
        "detail 不得双重转义: {last_line:?}"
    );
}

// ── format_event_line 与生产 write_event 共用 build_event_line ──

#[test]
fn build_event_line_is_single_source_for_format_and_write() {
    // write_event 的行拼装 = build_event_line(ts, ...)；format_event_line = build_event_line("TIMESTAMP", ...)。
    // write_event 可用 tauri::test::mock_app() + MockRuntime 直调（见 p2_6_1_zero_text_audit_kv_writes_to_bot_log），
    // 这里断言两条路径对同等 kv 产出同等行
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
        detail: "bad gateway".into(),
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

// ── P2-6'.1 零原文设计验证：reply_preview 必须不存在，reply_starts_with 必须存在 ── audit-ok

#[test]
fn p2_6_1_zero_text_audit_kv_format_event_line() {
    // 零原文 5 个分类枚举值都有专道走产线 build_event_line
    for class in &["CJK", "ASCII", "Digit", "Punctuation", "Other"] {
        let kv: Vec<(&str, &str)> = vec![
            ("session_id", "sess-p26-1-test"),
            ("reply_len", "5"),
            ("reply_starts_with", class),
        ];
        let line = format_event_line(AuditLevel::Info, "exec_steps.ambiguous_reply", &kv);
        assert!(
            line.contains("INFO | exec_steps.ambiguous_reply"),
            "event header missing: {line}"
        );
        assert!(
            line.contains("session_id=sess-p26-1-test"),
            "session_id missing: {line}"
        );
        assert!(line.contains("reply_len=5"), "reply_len missing: {line}");
        assert!(
            line.contains(&format!("reply_starts_with={class}")),
            "reply_starts_with={class} missing: {line}"
        );
        // 零原文：reply_preview 必须不存在
        assert!(
            !line.contains("reply_preview"),
            "reply_preview 必须不存在（零原文设计）: {line}"
        );
    }
}

#[test]
fn open_log_append_creates_missing_parent_dir() {
    // 审计写入刻意不依赖 DB（库坏了也要留痕），所以不能假设数据目录
    // 已被 open_db 建好：目录不存在时必须自建，而不是静默丢一行
    // `[audit] write failed: No such file or directory`
    let dir = std::env::temp_dir().join(format!("wm-audit-mkdir-{}", uuid::Uuid::new_v4()));
    let log = dir.join("nested").join("bot.log");
    let f = open_log_append(&log).expect("缺父目录时应自建");
    drop(f);
    assert!(log.exists(), "bot.log 应已创建");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn p2_6_1_zero_text_audit_kv_writes_to_bot_log() {
    // 不依赖 app 启动：mock app + write_event 真写盘 + 读回验格式
    // 删除/读全量共享 bot.log，必须持测试串行锁（与 bot::config 同名用例互斥）
    let _serial = crate::db::lock::lock_or_recover(&BOT_LOG_TEST_LOCK, "audit::BOT_LOG_TEST_LOCK");
    use crate::audit::write_event;
    let app = tauri::test::mock_app();
    let bot_log = crate::db::data_dir(app.handle()).join("bot.log");
    // 清掉上一次运行残留，避免假阳
    let _ = std::fs::remove_file(&bot_log);
    write_event(
        app.handle(),
        AuditLevel::Info,
        "exec_steps.ambiguous_reply",
        &[
            ("session_id", "sess-p26-1-mock".to_string()),
            ("reply_len", "7".to_string()),
            ("reply_starts_with", "Other".to_string()),
        ],
    );
    let content =
        std::fs::read_to_string(&bot_log).expect("write_event 应在 mock app data_dir 写出 bot.log");
    let last_line = content
        .lines()
        .last()
        .expect("write_event 后 bot.log 至少有一行");
    assert!(
        last_line.contains("INFO | exec_steps.ambiguous_reply"),
        "事件名缺失: {last_line}"
    );
    assert!(
        last_line.contains("session_id=sess-p26-1-mock"),
        "session_id kv 缺失: {last_line}"
    );
    assert!(
        last_line.contains("reply_len=7"),
        "reply_len kv 缺失: {last_line}"
    );
    assert!(
        last_line.contains("reply_starts_with=Other"),
        "reply_starts_with kv 缺失: {last_line}"
    );
    assert!(
        !last_line.contains("reply_preview"),
        "零原文：reply_preview 必须不存在: {last_line}"
    );
}
