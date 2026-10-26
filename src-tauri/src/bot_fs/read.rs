//! 读取面：glob 与目录遍历 / 二进制判定 / inode 复核 + 三个读工具。

use super::*;

/// 简化 glob 匹配（只支持 * 任意串、? 单字符；大小写敏感；匹配文件/目录名）
pub(super) fn glob_match(pattern: &str, name: &str) -> bool {
    fn inner(p: &[u8], n: &[u8]) -> bool {
        if p.is_empty() {
            return n.is_empty();
        }
        match p[0] {
            b'*' => (0..=n.len()).any(|i| inner(&p[1..], &n[i..])),
            b'?' => !n.is_empty() && inner(&p[1..], &n[1..]),
            c => !n.is_empty() && n[0] == c && inner(&p[1..], &n[1..]),
        }
    }
    inner(pattern.as_bytes(), name.as_bytes())
}

/// 目录遍历（迭代栈，深度上限 WALK_MAX_DEPTH，跳过隐藏目录与 SKIP_DIRS），
/// 回调返回 false 可提前终止（条目上限）
pub(super) fn walk(dir: &Path, mut visit: impl FnMut(&Path, bool) -> bool) {
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        if depth > WALK_MAX_DEPTH {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else {
            eprintln!("[bot_fs] walk 跳过不可读目录：{}", d.display());
            continue;
        };
        for entry in rd {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    eprintln!("[bot_fs] walk 跳过不可读条目（{}）：{err}", d.display());
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let ft = entry.file_type().ok();
            // 跳过符号链接（file_type 是 lstat 语义）——白名单目录内的
            // 软链指向外部时，grep 的 File::open/read_to_string 跟随软链会把外部文件内容带进
            // 模型上下文（一行 ln -s 即可逃逸）；类型读不到也按软链处理（fail-closed）
            if ft.as_ref().map(|t| t.is_symlink()).unwrap_or(true) {
                continue;
            }
            let is_dir = ft.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && (name.starts_with('.') || SKIP_DIRS.contains(&name.as_str())) {
                continue;
            }
            if !visit(&path, is_dir) {
                return;
            }
            if is_dir {
                stack.push((path, depth + 1));
            }
        }
    }
}

/// 是否二进制/非 UTF-8 文本（读前 8KB 含 NUL 即判二进制）
fn is_binary_file_sync(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return true;
    };
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

/// 有界读文件：只读前 max+1 字节判定截断——整读入内存后才截断的话，
/// 白名单内超大文件（GB 级日志）会把进程内存打爆。
/// 返回 (内容, 是否截断)。
pub(super) fn read_capped_file_sync(path: &Path, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    f.take(max as u64 + 1).read_to_end(&mut buf)?;
    let truncated = buf.len() > max;
    if truncated {
        buf.truncate(max);
    }
    Ok((buf, truncated))
}

/// 【 TOCTOU】inode re-check 结果。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum InodeCheckOutcome {
    /// inode 不变（或 platform 不支持 / 文件已不存在）→ 放行
    Ok,
    /// inode 变了 → 拒绝
    Replaced,
}

/// 读 canonical 路径当前 inode。三种情况（按 Phase 0.6 流程债 L402 精确三情况约束）：
///
/// 1. **Windows（`#[cfg(not(unix))]`）**：直接 `None`。
///    设计选择：Windows 端**无 TOCTOU 保护**（非缺陷——`read_with_ino_check`
///    整个 inode 比对逻辑都被 `#[cfg(unix)]` gate）。见 follow-up：
///    `Phase 6: Windows 端 TOCTOU 策略`（先决定做不做，再决定怎么做）。
///    **fail-open 是当前语义**——tool_read_text_file 在 Windows 上仍能读文件，
///    只是没有 inode re-check 这一道闸。
///
/// 2. **Unix 上 `metadata()` 成功**：`Some(ino)`，进入 re-check 保护范围（见下文
///    `read_with_ino_check` 的注释）。
///
/// 3. **Unix 上 `metadata()` 失败**（文件瞬间被删、权限拒、IO 错误等）：
///    `None`。**fail-open 是当前语义**——不放行也不拒绝（不报 Replaced），让
///    后续 `read_with_ino_check` 的 `File::open` 自然失败冒泡到 `tool_read_text_file`
///    返回「读取失败：...」。这是 C1b 阶段**有意选择**的不阻断读路径：
///    fail-closed 会让 race window 内的瞬态失败也拒绝，Windows + race 加起来
///    会让 read_text_file 在很多边界条件下不可用。
///
///    但 fail-open 也意味着：metadata 失败时**不参与 inode 比对**，等于这段
///    没保护。需要产品决策是否切 fail-closed（见 follow-up：
///    `Phase 6: capture_pre_ino None 时 fail-open vs fail-closed`），
///    决策点：fail-closed 会让 Windows 端 read_text_file 整体不可用。
#[cfg(unix)]
async fn capture_pre_ino(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let path = path.to_path_buf();
    spawn_blocking_io(move || std::fs::metadata(&path).map(|m| m.ino()))
        .await
        .ok()
}

#[cfg(not(unix))]
async fn capture_pre_ino(_path: &Path) -> Option<u64> {
    None
}

/// 读 + inode 复核 + 二进制判定，**全部在同一个 fd 上做完**。
///
/// 返回 (inode 判定, 是否二进制, 内容, 是否按上限截断)。
///
/// **为什么必须合并成一个 fd**：原实现分两步——`is_binary_file_with_ino_check`
/// 打开文件、stat fd 比对 inode、读 8KB 判二进制，**fd 在闭包内 drop**；
/// 随后 `read_capped_file` 按**路径重新 open** 读内容。真正的读取不在 inode
/// 比对窗口内：两步之间文件被换掉（删 + 同名新建），读回来的是新文件的内容
/// （TOCTOU）。本函数一次 open，比对、判二进制、读内容都在这个 fd 上完成，
/// 窗口关闭。
///
/// inode 比对本身的口径不变（unix only，Windows 无 TOCTOU 保护——设计选择）。
#[cfg(unix)]
pub(super) async fn read_with_ino_check(
    path: &Path,
    pre_ino: Option<u64>,
    max: usize,
) -> std::io::Result<(InodeCheckOutcome, bool, Vec<u8>, bool)> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let path = path.to_path_buf();
    crate::py::document::spawn_blocking_map(
        move || -> Result<(InodeCheckOutcome, bool, Vec<u8>, bool), String> {
            let f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
            let fd_ino = f.metadata().map_err(|e| e.to_string())?.ino();
            if let Some(pre) = pre_ino {
                if fd_ino != pre {
                    // inode 变了：内容一概不返回（读了也是被换掉的那个文件）
                    return Ok((InodeCheckOutcome::Replaced, false, Vec::new(), false));
                }
            }
            // 同一个 fd 上读：前 8KB 判二进制，其余接着读（fd 位置连续）
            let mut buf = Vec::new();
            f.take(max as u64 + 1)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            let truncated = buf.len() > max;
            if truncated {
                buf.truncate(max);
            }
            let is_binary = buf[..buf.len().min(8192)].contains(&0);
            Ok((InodeCheckOutcome::Ok, is_binary, buf, truncated))
        },
    )
    .await
    .map_err(std::io::Error::other)
}

/// Windows 端无 inode 比对（设计选择：非 unix 的 inode 语义不同），
/// 但**读与判二进制仍在同一个 fd 上**。
#[cfg(not(unix))]
pub(super) async fn read_with_ino_check(
    path: &Path,
    _pre_ino: Option<u64>,
    max: usize,
) -> std::io::Result<(InodeCheckOutcome, bool, Vec<u8>, bool)> {
    use std::io::Read;
    let path = path.to_path_buf();
    let res = spawn_blocking_io(move || -> Result<(bool, Vec<u8>, bool), std::io::Error> {
        let mut f = std::fs::File::open(&path)?;
        let mut buf = Vec::new();
        std::io::Read::by_ref(&mut f)
            .take(max as u64 + 1)
            .read_to_end(&mut buf)?;
        let truncated = buf.len() > max;
        if truncated {
            buf.truncate(max);
        }
        let is_binary = buf[..buf.len().min(8192)].contains(&0);
        Ok((is_binary, buf, truncated))
    })
    .await;
    match res {
        Ok((b, buf, t)) => Ok((InodeCheckOutcome::Ok, b, buf, t)),
        // IO 错误按二进制处理（fail-closed）
        Err(_) => Ok((InodeCheckOutcome::Ok, true, Vec::new(), false)),
    }
}

// 工具实现（bot 分发签名：(String, Vec<TaskRef>)）

/// read_text_file：读白名单内 UTF-8 文本，offset/limit 行切片（1 起），超 100KB 截断
pub async fn tool_read_text_file(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str() else {
        return ToolResult::denied("read_text_file 缺少 path");
    };
    let canonical =
        match resolve_with_perm(app, "read_text_file", path, interactive, session_id).await {
            Ok(p) => p,
            Err(e) => return ToolResult::denied(e),
        };
    match is_dir_async(&canonical).await {
        Some(true) => {
            return ToolResult::denied(format!("{} 是目录，列文件请用 list_files", path.trim()));
        }
        Some(false) => {}
        // 探测失败：说清是探测失败（不是「不是目录」）——后者会让用户以为
        // 路径类型没问题，转而去改别的
        None => {
            return ToolResult::error(
                format!("无法确认 {} 的路径类型（探测失败），请重试", path.trim()),
                Vec::new(),
            );
        }
    }
    // 【 TOCTOU】保护范围（精确）：
    //   canonicalize → capture_pre_ino → read_with_ino_check
    //   （**一次 open**：同一个 fd 上 stat 比对 inode → 读内容 → 判二进制）
    //   ——inode swap 与「读到的内容」之间不再有窗口：比对与读取在同一个 fd 上。
    //
    // 不在保护范围（明确写明）：
    //   - symlink target 替换（canonical 路径不变但内容指向新文件）。
    //     见 follow-up: "Phase 6: symlink target 替换防护收紧"
    //   - Windows 平台（`#[cfg(not(unix))]`）：capture_pre_ino 直接 None，整个
    //     inode re-check 不执行。设计选择，非缺陷——见 capture_pre_ino 注释
    //     + follow-up: "Phase 6: Windows 端 TOCTOU 策略"。
    let pre_ino = capture_pre_ino(&canonical).await;
    let (inode_check, is_binary, raw, byte_truncated) =
        match read_with_ino_check(&canonical, pre_ino, READ_MAX_BYTES).await {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("读取失败：{e}"), Vec::new()),
        };
    if inode_check == InodeCheckOutcome::Replaced {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "bot_fs.read_inode_mismatch",
            "path" => canonical.display().to_string(),
            "reason" => "canonicalize 与 stat 之间 inode 变了",
        );
        return ToolResult::denied(format!(
            "{} 在校验后被替换，拒绝读取（TOCTOU 防护）",
            path.trim()
        ));
    }
    if is_binary {
        return ToolResult::denied(format!(
            "{} 是二进制/非文本文件；Office/PDF 文档请用 extract_document",
            path.trim()
        ));
    }
    let offset = v["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit =
        (v["limit"].as_u64().unwrap_or(READ_DEFAULT_LINES as u64) as usize).min(READ_MAX_LINES);
    // 内容已在 read_with_ino_check 里从**核对过的那同一个 fd** 读出（不再按
    // 路径二次 open——二次 open 就是 TOCTOU 窗口）。
    let text = String::from_utf8_lossy(&raw).to_string();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if offset > total {
        return ToolResult::denied(format!("文件共 {total} 行，offset {offset} 超出范围"));
    }
    let slice: Vec<String> = lines
        .iter()
        .skip(offset - 1)
        .take(limit)
        .enumerate()
        .map(|(i, l)| format!("{}: {}", offset + i, l))
        .collect();
    let end = offset + slice.len() - 1;
    let mut out = format!(
        "{}（第 {offset}-{end} 行 / 共 {total} 行）\n{}",
        path.trim(),
        slice.join("\n")
    );
    if end < total || byte_truncated {
        out.push_str(&format!("\n…（截断，继续读请用 offset={}", end + 1));
        if byte_truncated {
            out.push_str(&format!("；文件超 {}KB 只读了前部", READ_MAX_BYTES / 1024));
        }
        out.push(')');
    }
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "bot_fs.read",
        "path" => canonical.display().to_string(),
        "offset" => offset,
        "end" => end,
        "total" => total,
    );
    ToolResult::ok(out, Vec::new())
}

/// grep_files：白名单目录内正则搜文件内容，输出 path:line:内容（ripgrep 风格）
/// N3-6：grep 上下文渲染（grep -C 风格，纯函数可测）。
/// 命中行 `path:行号: 内容`、上下文行 `path-行号- 内容`；重叠/相邻窗口合并为一块，
/// 块间 `--` 分隔；行宽截 200 字符。context=0 时退化为每命中一行的旧行为。
pub(super) fn render_context_hits(
    path: &str,
    lines: &[&str],
    hits: &[usize],
    context: usize,
) -> Vec<String> {
    let trunc = |s: &str| s.chars().take(200).collect::<String>();
    // 1) 命中行窗口（0-based，含端）合并：重叠或仅隔 1 行的相邻窗口并成一块
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for &h in hits {
        let s = h.saturating_sub(context);
        let e = (h + context).min(lines.len().saturating_sub(1));
        match windows.last_mut() {
            Some((_, pe)) if s <= *pe + 1 => *pe = (*pe).max(e),
            _ => windows.push((s, e)),
        }
    }
    // 2) 输出
    let mut out: Vec<String> = Vec::new();
    for (wi, (s, e)) in windows.iter().enumerate() {
        if wi > 0 {
            out.push("--".to_string());
        }
        for (i, line) in lines.iter().enumerate().take(*e + 1).skip(*s) {
            let mark = if hits.contains(&i) { ':' } else { '-' };
            out.push(format!("{path}{mark}{}{mark} {}", i + 1, trunc(line)));
        }
    }
    out
}

/// 生成物文件名排除：minified 产物与常见锁文件是 grep 噪声主体——命中行
/// 又长又无信息量，还挤占命中配额（GitHub 代码搜索同款口径）。只影响
/// grep_files；read_text_file 仍可定点读取这些文件。
pub(super) fn is_generated_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.ends_with(".min.js") || n.ends_with(".min.mjs") || n.ends_with(".min.css") {
        return true;
    }
    if n.ends_with(".map") {
        return true; // source map 产物
    }
    matches!(
        n.as_str(),
        "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "composer.lock" | "cargo.lock"
    )
}

/// 大文件跳过的可见化提示：静默跳过会让模型误信「搜了、没有」；
/// 附前几个文件名，模型可改用 read_text_file 定点查看。total == 0 → None。
pub(super) fn format_skip_note(skipped: &[String], total: usize) -> Option<String> {
    if total == 0 {
        return None;
    }
    let names = skipped.join("、");
    let more = if total > skipped.len() {
        format!(" 等共 {total} 个")
    } else {
        String::new()
    };
    Some(format!(
        "\n另有 {total} 个大文件（超过 2MB）未搜索{more}：{names}；如需查看请用 read_text_file 定点读取"
    ))
}

/// 单文件匹配（目录 walk 与单文件路径共用同一条判定）。
///
/// glob 过滤 / 生成名跳过 / 超大文件跳过 / 二进制跳过 / 逐行匹配 / 上下文渲染
/// 全在这里——单文件路径与目录 walk 走同一份逻辑，不复制第二套。
///
/// 返回 true = 可以继续（还有预算），false = 停（匹配数达上限）。
#[allow(clippy::too_many_arguments)]
pub(super) fn grep_file_into(
    path: &Path,
    re: &regex::Regex,
    glob: &str,
    max: usize,
    context: usize,
    hits: &mut Vec<String>,
    found: &mut usize,
    skipped: &mut Vec<String>,
    skipped_total: &mut usize,
) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !glob.is_empty() && !glob_match(glob, &name) {
        return true;
    }
    if is_generated_name(&name) {
        return true;
    }
    if path
        .metadata()
        .map(|m| m.len() > GREP_MAX_FILE_BYTES)
        .unwrap_or(true)
    {
        // 超大文件跳过必须可见（见 format_skip_note）
        *skipped_total += 1;
        if skipped.len() < 5 {
            skipped.push(name);
        }
        return true;
    }
    if is_binary_file_sync(path) {
        return true;
    }
    if let Ok(text) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = text.lines().collect();
        let hit_idx: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| re.is_match(line))
            .map(|(i, _)| i)
            .collect();
        if hit_idx.is_empty() {
            return true;
        }
        // max 按匹配数计（渲染行数随 context 放大）；预算内才渲染
        let budget = max.saturating_sub(*found);
        let taken = budget.min(hit_idx.len());
        hits.extend(render_context_hits(
            &path.display().to_string(),
            &lines,
            &hit_idx[..taken],
            context,
        ));
        *found += taken;
        if *found >= max {
            return false;
        }
    }
    true
}

/// grep 的目标形态：目录（递归 walk 其下文件）或单个文件（只搜它）。
#[derive(Debug)]
pub(super) enum GrepTarget {
    /// 目录：递归搜其下文件
    Dir,
    /// 单个文件：只搜这一个
    File(PathBuf),
}

/// 按路径类型定搜法（纯函数，单测入口）。
///
/// 探测失败（None）= 拿不准路径类型 → 调用方报错让模型重试，不猜。
pub(super) fn grep_target(is_dir: Option<bool>, path: &Path) -> Option<GrepTarget> {
    match is_dir {
        Some(true) => Some(GrepTarget::Dir),
        Some(false) => Some(GrepTarget::File(path.to_path_buf())),
        None => None,
    }
}

/// grep_files：按正则搜索本地文件内容（dir 可传目录（递归）或单个文件（只搜它））
pub async fn tool_grep_files(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(pattern) = v["pattern"].as_str() else {
        return ToolResult::denied("grep_files 缺少 pattern");
    };
    // 非法正则降级为字面量搜索（regex::escape），不让一个坏 pattern 炸掉整轮
    let re = regex::Regex::new(pattern).or_else(|_| regex::Regex::new(&regex::escape(pattern)));
    let Ok(re) = re else {
        return ToolResult::denied(format!("无效的正则表达式：{pattern}"));
    };
    let glob = v["glob"].as_str().unwrap_or("").trim().to_string();
    let max = (v["max"].as_u64().unwrap_or(GREP_MAX_HITS as u64) as usize).min(GREP_MAX_HITS);
    // N3-6：上下文行数（0..=5，默认 0 = 旧行为）
    let context = (v["context"].as_u64().unwrap_or(0) as usize).min(5);
    // dir 可选：缺省搜第一个白名单目录
    let dir = match v["dir"].as_str() {
        Some(d) if !d.trim().is_empty() => {
            match resolve_with_perm(app, "grep_files", d, interactive, session_id).await {
                Ok(p) => p,
                Err(e) => return ToolResult::denied(e),
            }
        }
        _ => match allowed_dirs(app, session_id).await.first() {
            Some(d) => d.clone(),
            None => return ToolResult::denied("没有可用的白名单目录"),
        },
    };
    // 路径类型决定搜法：目录 → 递归 walk；文件 → 只搜这一个文件。
    //
    // 「在某个文件里找关键词」只有这一条路：模型经常拿着一个**文件**路径来 grep
    // （溢出回灌指针、工具回执里的路径、技能正文路径），目录型实现只会回一句
    // 「不是目录」——而那句拒绝在技能里等于把技能判死（实测：模型为找技能正文的
    // 章节标题去 grep 一个溢出指针文件 → 参数闸拒绝 → 技能判死 → 整个回合结束）。
    let target = match grep_target(is_dir_async(&dir).await, &dir) {
        Some(t) => t,
        None => {
            return ToolResult::error(
                format!("无法确认 {} 的路径类型（探测失败），请重试", dir.display()),
                Vec::new(),
            );
        }
    };
    // walk 整体包 spawn_blocking：内部 read_dir / metadata / read_to_string
    // 都是同步 syscall，全部移到阻塞线程跑，不再阻塞 Tauri async runtime
    // （ performance critical）。
    // closure 内 is_binary_file_sync / read_to_string 仍 sync，但已在 spawn_blocking
    // 线程内 OK。流式优化留 follow-up。
    // 返回（渲染行, 匹配数）二元组：max 按匹配数计、行数随 context 放大，
    // 上限提示要看匹配数；spawn_blocking 'static 闭包只能带值回传。
    let dir_log = dir.display().to_string();
    let is_single_file = matches!(target, GrepTarget::File(_));
    let (hits, found, skipped, skipped_total) = crate::py::document::spawn_blocking_map(
        move || -> Result<(Vec<String>, usize, Vec<String>, usize), String> {
            let mut hits: Vec<String> = Vec::new();
            let mut found = 0usize;
            let mut skipped: Vec<String> = Vec::new();
            let mut skipped_total = 0usize;
            match &target {
                // 单文件：走与目录 walk 同一条判定（glob / 生成名 / 超大 / 二进制 / 匹配）
                GrepTarget::File(file) => {
                    grep_file_into(
                        file,
                        &re,
                        &glob,
                        max,
                        context,
                        &mut hits,
                        &mut found,
                        &mut skipped,
                        &mut skipped_total,
                    );
                }
                GrepTarget::Dir => {
                    walk(&dir, &mut |path: &Path, is_dir: bool| {
                        if found >= max {
                            return false;
                        }
                        if is_dir {
                            return true;
                        }
                        grep_file_into(
                            path,
                            &re,
                            &glob,
                            max,
                            context,
                            &mut hits,
                            &mut found,
                            &mut skipped,
                            &mut skipped_total,
                        )
                    });
                }
            }
            Ok((hits, found, skipped, skipped_total))
        },
    )
    .await
    .unwrap_or_default();
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "bot_fs.grep",
        "dir" => &dir_log,
        "pattern" => pattern,
        "hits" => found,
    );
    if hits.is_empty() {
        // 跳过的大文件随提示可见
        let skip_note = format_skip_note(&skipped, skipped_total).unwrap_or_default();
        let scope = if is_single_file {
            format!("文件 {dir_log}")
        } else {
            format!("{dir_log} 内")
        };
        return ToolResult::ok(
            format!("{scope}没有匹配「{pattern}」的内容{skip_note}"),
            Vec::new(),
        );
    }
    let mut out = hits.join("\n");
    if found >= max {
        out.push_str(&format!(
            "\n…（已达 {max} 条匹配上限，缩小范围或加 glob 过滤）"
        ));
    }
    if let Some(note) = format_skip_note(&skipped, skipped_total) {
        out.push_str(&note);
    }
    ToolResult::ok(out, Vec::new())
}

/// list_files：列白名单目录内文件（可选 glob 过滤文件名），深度 ≤5，上限 200 条
pub async fn tool_list_files(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(dir) = v["dir"].as_str() else {
        return ToolResult::denied("list_files 缺少 dir");
    };
    let canonical = match resolve_with_perm(app, "list_files", dir, interactive, session_id).await {
        Ok(p) => p,
        Err(e) => return ToolResult::denied(e),
    };
    match is_dir_async(&canonical).await {
        Some(true) => {}
        Some(false) => {
            return ToolResult::denied(format!(
                "{} 不是目录；读文件请用 read_text_file",
                dir.trim()
            ));
        }
        None => {
            return ToolResult::error(
                format!("无法确认 {} 的路径类型（探测失败），请重试", dir.trim()),
                Vec::new(),
            );
        }
    }
    let pattern = v["pattern"].as_str().unwrap_or("").trim().to_string();
    // walk 整体包 spawn_blocking（ performance）：同 grep_files 原因。
    let canonical_log = canonical.display().to_string();
    let entries: Vec<String> =
        crate::py::document::spawn_blocking_map(move || -> Result<Vec<String>, String> {
            let mut entries: Vec<String> = Vec::new();
            walk(&canonical, &mut |path: &Path, is_dir: bool| {
                if entries.len() >= LIST_MAX_ENTRIES {
                    return false;
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !pattern.is_empty() && !glob_match(&pattern, &name) {
                    return true;
                }
                let rel = path.strip_prefix(&canonical).unwrap_or(path);
                entries.push(format!(
                    "{}{}",
                    rel.display(),
                    if is_dir { "/" } else { "" }
                ));
                true
            });
            Ok(entries)
        })
        .await
        .unwrap_or_default();
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "bot_fs.list",
        "dir" => &canonical_log,
        "entries" => entries.len(),
    );
    if entries.is_empty() {
        return ToolResult::ok(format!("{} 内没有匹配的文件", canonical_log), Vec::new());
    }
    let mut out = format!(
        "{}（{} 条）：\n{}",
        canonical_log,
        entries.len(),
        entries.join("\n")
    );
    if entries.len() >= LIST_MAX_ENTRIES {
        out.push_str(&format!(
            "\n…（已达 {LIST_MAX_ENTRIES} 条上限，用 pattern 过滤缩小范围）"
        ));
    }
    ToolResult::ok(out, Vec::new())
}
