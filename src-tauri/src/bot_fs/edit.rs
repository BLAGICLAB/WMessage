//! 文件编辑面：写闸 + 三级匹配内核 + tool_edit_file + tool_write_file。

use super::*;

// 文件编辑（edit_file / write_file）
//
// 写白名单 ≠ 读白名单（设计决策，spec ）：可写根 = AI_Gen_Files + 任务卡绑定文件夹
// + 设置页 allowedDirs；桌面/下载/文档默认项**只读不可写**——用户没显式授权改家目录
// 文件。白名单外走 perm_mode 三分支：strict 拒 / ask 弹 danger 确认（每次，不持久化）/
// yolo 放行+审计。

/// 可写根的原始集合（纯内核，可测）：gen + 任务卡绑定文件夹 + cfg.allowedDirs。
/// 刻意不含 merge_raw_dirs 里的 home/Desktop/Downloads/Documents 默认读项。
pub(super) fn build_writable_raw(
    gen: Option<String>,
    task_dirs: Vec<String>,
    cfg_dirs: &[String],
) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    raw.extend(gen);
    raw.extend(task_dirs);
    raw.extend(cfg_dirs.iter().cloned());
    raw
}

/// 可写目录集合（canonical 化，只保留真实存在的目录）。
/// 与 allowed_dirs 同样的 canonicalize 包 spawn_blocking 防 runtime 阻塞。
async fn writable_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let cfg = crate::bot::load_config(app);
    let mut task_dirs: Vec<String> = Vec::new();
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            // 回收站任务的绑定目录不进写白名单（防「删卡不解权」残留授权）
            if t.deleted_at.is_some() {
                continue;
            }
            for f in t.effective_files() {
                if f.is_dir {
                    task_dirs.push(f.path);
                }
            }
        }
    }
    let gen = crate::db::gen_dir(app)
        .ok()
        .map(|p| p.to_string_lossy().to_string());
    let raw = build_writable_raw(gen, task_dirs, &cfg.allowed_dirs);
    spawn_blocking_io(move || {
        let mut out: Vec<PathBuf> = Vec::new();
        for r in raw {
            let p = expand_tilde(r.trim());
            if let Ok(c) = std::fs::canonicalize(&p) {
                if c.is_dir() && !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        Ok(out)
    })
    .await
    .unwrap_or_default()
}

/// 写路径守卫：resolve_with_perm 的写语义变体。
/// 差异：目标文件可不存在（write_file 新建），改为校验**父目录**存在且落在
/// 可写根内；白名单外 ask 分支用 ask_user_confirm（danger，每次确认，不持久化
/// ——「始终允许」语义对写太宽）。目标末段是符号链接时直接拒绝（防 edit 的读
/// 穿过软链绕过读闸、防 rename 把链接整体替换）。成功返回展开后的目标路径
/// （文件可能尚不存在，不能 canonicalize；父目录已 canonical 校验，TOCTOU 窗口
/// 与读闸门同级留档）。
///
/// 第二个调用方：归档工具（bot_archive）解包 / 打包的落点也要走写闸。
pub(crate) async fn resolve_writable(
    app: &AppHandle,
    tool: &str,
    path: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> Result<PathBuf, String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("路径不能为空".into());
    }
    let expanded = expand_tilde(p);
    // 目标末段若是符号链接则拒绝：edit_file 的读取会穿过软链绕过读白名单闸，
    // atomic_write 的 rename 会把软链整体替换成普通文件（链外的真实文件不动但链接被毁）。
    // 父目录软链不受影响（父目录已 canonicalize 校验）；检查与写入之间的竞态残余窗口
    // 与读闸门同级留档。
    let link_probe = expanded.clone();
    let is_symlink = spawn_blocking_io(move || {
        Ok(std::fs::symlink_metadata(&link_probe)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false))
    })
    .await
    .unwrap_or(false);
    if is_symlink {
        return Err(format!(
            "目标路径是符号链接，拒绝写入：{p}（请直接对真实文件操作，或删除软链后重试）"
        ));
    }
    let expanded_for_parent = expanded.clone();
    let parent_canon = spawn_blocking_io(move || {
        let parent = expanded_for_parent
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "路径没有父目录")
            })?;
        std::fs::canonicalize(parent)
    })
    .await
    .map_err(|e| {
        format!(
            "父目录不存在或无法解析：{}（{e}）",
            expanded
                .parent()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "?".into())
        )
    })?;
    let dirs = writable_dirs(app).await;
    if is_within_allowlist(&parent_canon, &dirs) {
        return Ok(strip_verbatim(expanded));
    }
    // allow 规则 = 单工具 yolo（写白名单外放行+审计，与读侧同语义）
    if crate::bot_fs::tool_rule_for(app, tool) == Some(crate::bot_fs::ToolRuleAction::Allow) {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "bot_fs.rule_allow_write",
            "tool" => tool,
            "path" => p,
        );
        return Ok(strip_verbatim(expanded));
    }
    match crate::bot::perm_mode(app) {
        crate::bot::PermMode::Yolo => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "bot_fs.yolo_allow_write",
                "tool" => tool,
                "path" => p,
            );
            Ok(strip_verbatim(expanded))
        }
        //  auto 档：白名单外降级 ask（写确认每次不持久化；无人值守拒）——与 Ask 同分支
        crate::bot::PermMode::Ask | crate::bot::PermMode::Auto => {
            let approved = crate::bot_slash::ask_user_confirm(
                app,
                tool,
                &format!("写入白名单外路径：{p}"),
                interactive,
                session_id,
            )
            .await;
            if approved {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Info,
                    "bot_fs.ask_allow_write",
                    "tool" => tool,
                    "path" => p,
                );
                Ok(strip_verbatim(expanded))
            } else {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "bot_fs.ask_denied_write",
                    "tool" => tool,
                    "path" => p,
                );
                Err(format!(
                    "用户未授权写入该路径：{p}（可在设置页把目录加入白名单；后台执行时确认窗不可用，一律拒绝）"
                ))
            }
        }
        crate::bot::PermMode::Strict => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "bot_fs.denied_write",
                "path" => p,
                "reason" => "不在可写目录内",
            );
            Err(format!(
                "路径不在可写目录内：{p}（可写目录：AI_Gen_Files + 任务卡绑定文件夹 + 设置页 allowedDirs）"
            ))
        }
    }
}

/// edit 匹配级别（审计与回执用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditMode {
    Exact,
    /// 空白容错（逐行 trim_end 比较）命中
    WhitespaceTolerant,
    /// regex 模式（oldString 作为 Rust regex 模式串、replace_all；$1/$2 捕获组）
    /// `matches` = find_iter 计数（也是实际 replace_all 次数，0 表示试过但 0 处匹配
    /// ——对应 caller 早退到 NotFound；非 0 表示已替换次数，audit/receipt 据此把
    /// 「1 处替换」与「1 万处替换」区分开）
    Regex {
        matches: usize,
    },
}

/// edit 失败类型（进审计的结构化字段，供失败统计与反思采集用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditErrorKind {
    /// 精确与空白容错都未命中
    NotFound,
    /// 命中多处，无法唯一定位
    MultiHit,
}

/// edit 三级匹配内核（纯函数，无 IO 可测）：
/// 1. 精确唯一 → 应用
/// 2. 精确多处 → MultiHit
/// 3. 精确 0 处 → 空白容错（逐行 trim_end 比较，CRLF 经 lines() 天然免疫）：
///    唯一 → 应用（重建时保留文件原本的主导换行符）；0 处 → NotFound（带
///    reflection 提示：先 read_text_file、注意缩进）；多处 → MultiHit
/// 返回 (替换后全文, 级别, 行数变化 new-old)。
/// 不做 Levenshtein 模糊匹配（误替换风险 > 收益，spec  留档）。
pub(crate) fn try_apply_edit(
    content: &str,
    old: &str,
    new: &str,
    use_regex: bool,
) -> Result<(String, EditMode, i64, usize), (EditErrorKind, String)> {
    // regex 模式：oldString 当作 Rust regex 模式串、replace_all；$1/$2 由 regex crate 解析。
    // fail-closed：非法 pattern 报错而非 panic；空替换会清空文件也拒（防呆）。
    if use_regex {
        use regex::Regex;
        let re = match Regex::new(old) {
            Ok(r) => r,
            Err(e) => {
                return Err((EditErrorKind::NotFound, format!("regex 模式串非法：{e}")));
            }
        };
        let count = re.find_iter(content).count();
        let applied = re.replace_all(content, new).into_owned();
        // 输出体积护栏：与读侧 EDIT_MAX_FILE_BYTES 同一量级；1MB 输入若替换扩张
        // 出数倍会塞爆 write 路径 + 后续 checkpoint 快照
        if applied.len() > EDIT_MAX_FILE_BYTES {
            return Err((
                EditErrorKind::NotFound,
                format!(
                    "regex 替换后体积 {} 超过 {} 字节上限（匹配 {count} 处）。拆细替换或改用 write_file。",
                    applied.len(),
                    EDIT_MAX_FILE_BYTES
                ),
            ));
        }
        if count == 0 {
            let head: Vec<&str> = content.lines().take(3).collect();
            let head_show = if head.is_empty() {
                "（空文件）".to_string()
            } else {
                head.join("\n")
            };
            return Err((
                EditErrorKind::NotFound,
                format!("regex 模式在文件中 0 处匹配。文件开头：\n{head_show}"),
            ));
        }
        let applied = re.replace_all(content, new).into_owned();
        if applied.is_empty() && !content.is_empty() {
            return Err((
                EditErrorKind::NotFound,
                "regex 替换后文件会被清空，已拒绝（清空文件请用 write_file 显式写空内容）"
                    .to_string(),
            ));
        }
        let delta = applied.lines().count() as i64 - content.lines().count() as i64;
        return Ok((applied, EditMode::Regex { matches: count }, delta, count));
    }
    let exact_hits = content.matches(old).count();
    if exact_hits == 1 {
        let applied = content.replacen(old, new, 1);
        let delta = new.lines().count() as i64 - old.lines().count() as i64;
        return Ok((applied, EditMode::Exact, delta, 1));
    }
    if exact_hits > 1 {
        return Err((
            EditErrorKind::MultiHit,
            format!(
                "oldString 在文件中命中 {exact_hits} 处，无法唯一定位。请加入更多上下文（前后各 1-2 行）让匹配唯一。"
            ),
        ));
    }
    // 空白容错：逐行 trim_end 比较（尾部空白/CRLF 不敏感；缩进仍要求一致）
    let old_lines: Vec<&str> = old.lines().map(|l| l.trim_end()).collect();
    if old_lines.is_empty() {
        return Err((
            EditErrorKind::NotFound,
            "oldString 为空白。请先 read_text_file 确认原文，再提供要替换的具体内容".into(),
        ));
    }
    let content_lines: Vec<&str> = content.lines().map(|l| l.trim_end()).collect();
    let n = old_lines.len();
    let mut windows: Vec<usize> = Vec::new();
    if content_lines.len() >= n {
        for start in 0..=(content_lines.len() - n) {
            if content_lines[start..start + n] == old_lines[..] {
                windows.push(start);
            }
        }
    }
    match windows.len() {
        1 => {
            let start = windows[0];
            // 按**字节范围**替换，不把全文按行重建。重建有两个副作用：
            // · 文件原有的混合换行（部分 CRLF 部分 LF）被整体归一成一档——
            //   那是一次与本次编辑无关的全文件改动（diff 全红），而精确
            //   匹配路径的 replacen 不会有；
            // · newString 末尾的换行被吃掉（按行取 lines() 不含尾换行），
            //   替换区在文件末尾时，模型明确要求的尾换行直接消失。
            let ranges = line_ranges(content);
            let byte_start = ranges[start].0;
            let byte_end = ranges[start + n - 1].1;
            // 新内容按文件的主导换行符归一（模型给的 newString 多是 LF），
            // 尾换行保留——与精确匹配路径（replacen 原样插入）语义对齐。
            let eol = dominant_eol(content);
            let replacement = normalize_eol(new, eol);
            let mut applied = String::with_capacity(content.len() + replacement.len());
            applied.push_str(&content[..byte_start]);
            applied.push_str(&replacement);
            applied.push_str(&content[byte_end..]);
            let delta = new.lines().count() as i64 - n as i64;
            Ok((applied, EditMode::WhitespaceTolerant, delta, 1))
        }
        0 => {
            let head: Vec<&str> = content.lines().take(3).collect();
            let head_show = if head.is_empty() {
                "（空文件）".to_string()
            } else {
                head.join("\n")
            };
            Err((
                EditErrorKind::NotFound,
                format!(
                    "oldString 未在文件中找到（含空白容错）。建议：先 read_text_file 确认原文再复制精确内容；注意缩进与空白（容错只放宽行尾空白与 CRLF，不放宽缩进）。文件开头：\n{head_show}"
                ),
            ))
        }
        _ => Err((
            EditErrorKind::MultiHit,
            format!(
                "oldString 在空白容错下仍命中 {} 处。请加入更多上下文（前后各 1-2 行）让匹配唯一。",
                windows.len()
            ),
        )),
    }
}

/// content 逐行的字节范围（行首偏移, 行内容末尾偏移；不含行尾的 \r 与 \n）。
///
/// 下标与 `content.lines()` 一一对应（行数相同、顺序相同），供空白容错
/// 把「命中的那几行」换算成字节区间——按区间拼接才不会动到区间之外的
/// 换行符。
fn line_ranges(content: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(content.lines().count());
    let mut start = 0usize;
    for chunk in content.split_inclusive('\n') {
        let trimmed = chunk.trim_end_matches(['\r', '\n']);
        out.push((start, start + trimmed.len()));
        start += chunk.len();
    }
    out
}

/// 文件的主导换行符：出现过 CRLF 就按 CRLF，否则 LF。
fn dominant_eol(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// 把文本的换行统一到 eol（**尾换行保留**）。
fn normalize_eol(text: &str, eol: &str) -> String {
    let lf = text.replace("\r\n", "\n");
    if eol == "\n" {
        lf
    } else {
        lf.replace('\n', "\r\n")
    }
}

/// write 内容校验（纯函数，可测）：NUL 拒（文本工具不写二进制）、2MB 上限
pub(crate) fn validate_write_content(content: &str) -> Result<(), String> {
    if content.contains('\0') {
        return Err("内容包含 NUL 字节：write_file 只写文本文件".into());
    }
    if content.len() > WRITE_MAX_BYTES {
        return Err(format!(
            "内容 {} 字节超过 write_file 上限 {WRITE_MAX_BYTES}，大文件请拆分或用其他方式",
            content.len()
        ));
    }
    Ok(())
}
/// edit 同步内核（spawn_blocking 调用；独立函数便于单测）：
/// 读文件（≤1MB、UTF-8）→ 三级匹配 → 返回替换结果。
/// 同时带回**修改前全文**（diff 与回滚快照的证据源，避免再读一次盘）。
pub(super) fn edit_file_sync(
    canonical: PathBuf,
    old: String,
    new: String,
    use_regex: bool,
) -> Result<(String, String, EditMode, i64, usize), (EditErrorKind, String)> {
    // 先按上限读、**再**判体积：整读入内存后才判上限，白名单内一个 GB 级
    // 日志就能把进程内存打爆（read_capped_file_sync 存在的理由就是这个）。
    let (buf, truncated) = read_capped_file_sync(&canonical, EDIT_MAX_FILE_BYTES).map_err(|e| {
        (
            EditErrorKind::NotFound,
            format!("读取失败（非 UTF-8 文本或不可读）：{e}"),
        )
    })?;
    if truncated {
        // 体积是这类文件的真因：整读会先撞上非 UTF-8 报错，用户看不到
        // 「文件太大、请拆分」这条可操作的指引
        let size = std::fs::metadata(&canonical).map(|m| m.len()).unwrap_or(0);
        return Err((
            EditErrorKind::NotFound,
            format!("文件 {size} 字节超过 edit_file 上限 {EDIT_MAX_FILE_BYTES}"),
        ));
    }
    let content = String::from_utf8(buf).map_err(|_| {
        (
            EditErrorKind::NotFound,
            "读取失败（非 UTF-8 文本或不可读）".to_string(),
        )
    })?;
    let (applied, mode, delta, replacements) = try_apply_edit(&content, &old, &new, use_regex)?;
    Ok((content, applied, mode, delta, replacements))
}

/// edit_file：对文本文件做精确字符串替换（三级匹配内核 + 写闸门 + 原子写回）
pub async fn tool_edit_file(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::denied("edit_file 缺少 path 参数");
    };
    let Some(old) = v["oldString"].as_str().map(|s| s.to_string()) else {
        return ToolResult::denied("edit_file 缺少 oldString 参数");
    };
    let Some(new) = v["newString"].as_str().map(|s| s.to_string()) else {
        return ToolResult::denied("edit_file 缺少 newString 参数");
    };
    if path.is_empty() {
        return ToolResult::denied("edit_file 的 path 不能为空");
    }
    if old.trim().is_empty() {
        return ToolResult::denied("oldString 不能为空（请提供要替换的原文）");
    }
    if old == new {
        return ToolResult::denied("oldString 与 newString 相同，没有可应用的变更");
    }
    // mode 解析：默认 literal；显式传 "regex" 启用正则 replace_all
    let mode_str = v["mode"].as_str().unwrap_or("literal");
    let use_regex: bool = match mode_str {
        "literal" => false,
        "regex" => true,
        other => {
            return ToolResult::denied(format!(
                "edit_file 的 mode 非法：{other}（仅支持 literal|regex）"
            ));
        }
    };
    let canonical = match resolve_writable(app, "edit_file", &path, interactive, session_id).await {
        Ok(p) => p,
        Err(e) => return ToolResult::denied(e),
    };
    let shown = canonical.display().to_string();
    let write_target = canonical.clone();
    let out = crate::py::document::spawn_blocking_map(move || {
        Ok(edit_file_sync(canonical, old, new, use_regex))
    })
    .await
    .unwrap_or_else(|e| Err((EditErrorKind::NotFound, format!("编辑线程异常：{e}"))));
    match out {
        Ok((before, applied, mode, delta, replacements)) => {
            if let Err(e) = crate::db::atomic_write(&write_target, &applied) {
                return ToolResult::error(format!("写回失败：{e}"), Vec::new());
            }
            let mode_note = match mode {
                EditMode::Exact => "",
                EditMode::WhitespaceTolerant => "（经空白容错匹配：行尾空白/CRLF 有差异）",
                EditMode::Regex { .. } => "（regex 模式全部匹配替换）",
            };
            let delta_note = if delta > 0 {
                format!("+{delta} 行")
            } else if delta < 0 {
                format!("{delta} 行")
            } else {
                "行数不变".to_string()
            };
            let mode_tag = match mode {
                EditMode::Exact => "exact",
                EditMode::WhitespaceTolerant => "ws",
                EditMode::Regex { .. } => "regex",
            };
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "bot_fs.edit_file",
                "path" => &shown,
                "mode" => mode_tag,
                "delta_lines" => delta,
                "replacements" => replacements,
            );
            //  变更证据（Agent 透明化设计 §4.2）：unified diff + before 快照。
            // 快照失败降级 before_ref=None（checkpoint.write_fail 审计），不翻转业务结果。
            let mut receipt =
                build_file_change_receipt(&shown, "modify", Some(&before), &applied, replacements);
            receipt.before_ref = write_before_snapshot(app, &before);
            ToolResult::ok(
                format!("已修改 {shown}（{delta_note}）{mode_note}"),
                Vec::new(),
            )
            .with_file_change(receipt)
        }
        Err((kind, hint)) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "bot_fs.edit_file.fail",
                "path" => &shown,
                "kind" => format!("{kind:?}"),
                "hint" => &hint,
            );
            ToolResult::error(hint, Vec::new())
        }
    }
}

/// write_file：创建新文件 / 整体写入（覆盖已有文件需用户确认；非交互自动拒）
pub async fn tool_write_file(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::denied("write_file 缺少 path 参数");
    };
    let Some(content) = v["content"].as_str().map(|s| s.to_string()) else {
        return ToolResult::denied("write_file 缺少 content 参数");
    };
    if path.is_empty() {
        return ToolResult::denied("write_file 的 path 不能为空");
    }
    if let Err(e) = validate_write_content(&content) {
        return ToolResult::denied(e);
    }
    let canonical = match resolve_writable(app, "write_file", &path, interactive, session_id).await
    {
        Ok(p) => p,
        Err(e) => return ToolResult::denied(e),
    };
    // 错误文案用：canonical 马上要 move 进阻塞闭包，提示后缀提前留一份
    let canonical_for_err = canonical.clone();
    // existed 只认文件：目录目标不弹覆盖确认，直接写盘自然 IsADirectory 失败
    let c = canonical.clone();
    let existed = spawn_blocking_io(move || Ok(c.is_file()))
        .await
        .unwrap_or(false);
    if existed {
        // auto 档白名单内的覆盖写自动接受（resolve_writable 已过白名单闸；
        // acceptEdits 核心语义——白名单内的「文件编辑类操作」不再人工确认）。
        // yolo 档维持既有弹窗行为（默认行为零变更约束）；白名单外写入在
        // resolve_writable 已按 ask 弹过窗，此处不重复确认。
        let skip_confirm = crate::bot::perm_mode(app) == crate::bot::PermMode::Auto;
        let approved = skip_confirm
            || crate::bot_slash::ask_user_confirm(
                app,
                "write_file",
                &format!("覆盖已存在文件：{}", canonical.display()),
                interactive,
                session_id,
            )
            .await;
        if !approved {
            // 非交互（子 agent）确认窗不可用自动拒——覆盖走 edit_file 精确修改
            return ToolResult::denied("覆盖未获确认，文件未修改。修改已有文件请用 edit_file（精确替换，无需确认）；新建文件请换一个文件名");
        }
    }
    //  变更证据：modify 需在写盘前取旧文（读不出/超上限/二进制 → 无 diff 降级）。
    // 这是观测面读取而非安全判定，不做 inode re-check（TOCTOU 与读闸门同级留档）。
    let before_content: Option<String> = if existed {
        let c = canonical.clone();
        spawn_blocking_io(move || read_capped_file_sync(&c, EDIT_MAX_FILE_BYTES))
            .await
            .ok()
            .filter(|(_, truncated)| !truncated)
            .and_then(|(buf, _)| String::from_utf8(buf).ok())
    } else {
        None
    };
    let lines = content.lines().count() as i64;
    let content_after = content.clone();
    let out = crate::py::document::spawn_blocking_map(move || {
        crate::db::atomic_write(&canonical, &content).map(|_| canonical)
    })
    .await;
    match out {
        Ok(written) => {
            let action = if existed { "已覆盖" } else { "已创建" };
            //  变更证据：create = 全 + 行 diff；modify = before/after diff + 回滚快照。
            // 快照失败降级 before_ref=None（checkpoint.write_fail 审计），不翻转业务结果。
            let kind: &'static str = if existed { "modify" } else { "create" };
            let mut receipt = build_file_change_receipt(
                &written.display().to_string(),
                kind,
                before_content.as_deref(),
                &content_after,
                // write_file 是全量覆写/新建：replacements=0（kind="create"/"modify"
                // 已隐含语义；edit 路径走 replacements=N 区分 1 处 vs 1 万处）
                0,
            );
            if let Some(b) = before_content.as_deref() {
                receipt.before_ref = write_before_snapshot(app, b);
            }
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "bot_fs.write_file",
                "path" => written.display().to_string(),
                "lines" => lines,
                "action" => action,
            );
            ToolResult::ok(
                format!("已写入 {}（{action}，{lines} 行）", written.display()),
                Vec::new(),
            )
            .with_file_change(receipt)
        }
        Err(e) => ToolResult::error(
            format!(
                "写入失败：{e}{}",
                crate::bot_fs::win_long_path_hint(&canonical_for_err)
            ),
            Vec::new(),
        ),
    }
}
