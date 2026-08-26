//! 本地文件只读工具（2026-08-19 Phase 1：read_text_file / grep_files / list_files）。
//!
//! 安全设计（2026-08-26 授权模式改造，Kimi CLI 风格「执行前授权」）：
//! - 白名单（= 静默放行区）：内置默认（~/Desktop ~/Downloads ~/Documents + 任务卡绑定文件夹）
//!   ∪ bot-config.json `allowedDirs`（追加语义，不再整体替换默认）
//! - 白名单外按 perm_mode 分流：strict=硬拒（旧行为）/ ask=弹授权窗
//!   （允许一次/始终允许该目录/拒绝，始终允许自动写进 allowedDirs）/ yolo=直接放行
//! - resolve_with_perm：展开 ~ → canonicalize（不存在即拒绝）→ 白名单或授权放行；
//!   `..` 与软链逃逸在 canonicalize 后无处遁形；命中/授权/拒绝都记审计
//! - 只读：不提供写/删；二进制与图片拒绝（图片走既有视觉通道）
//! - 输出截断在工具层做；递归走目录有深度/条目上限，跳过隐藏目录与 node_modules/target 等

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::bot_chat::TaskRef;

const READ_MAX_BYTES: usize = 100 * 1024;
const READ_DEFAULT_LINES: usize = 500;
const READ_MAX_LINES: usize = 2000;
const GREP_MAX_HITS: usize = 50;
const GREP_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const LIST_MAX_ENTRIES: usize = 200;
/// 目录递归上限（防止深层目录树把上下文/耗时打爆）
const WALK_MAX_DEPTH: usize = 5;
/// 遍历时跳过的大而杂目录（另跳过所有 . 开头隐藏目录）
const SKIP_DIRS: [&str; 4] = ["node_modules", "target", "dist", "build"];

/// 跨平台用户主目录：优先 HOME；Windows GUI 程序（资源管理器双击启动）常无
/// HOME 环境变量，回退 USERPROFILE，再退 HOMEDRIVE+HOMEPATH。
///（2026-08-20 修复：Windows 绿色版 HOME 缺失 → 默认白名单为空 →
///  聊天附件 docx 被 extract_document 拒绝要求"再绑一遍"、且不再创建任务卡）
pub(crate) fn home_dir() -> Option<PathBuf> {
    home_dir_from(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        std::env::var_os("HOMEDRIVE"),
        std::env::var_os("HOMEPATH"),
    )
}

/// 可测内核：按 HOME → USERPROFILE → HOMEDRIVE+HOMEPATH 优先级取第一个非空值
fn home_dir_from(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
    drive: Option<std::ffi::OsString>,
    homepath: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    for v in [home, userprofile].into_iter().flatten() {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    match (drive, homepath) {
        (Some(d), Some(p)) if !d.is_empty() && !p.is_empty() => {
            let mut s = d;
            s.push(p);
            Some(PathBuf::from(s))
        }
        _ => None,
    }
}

/// 展开路径开头的 ~ / ~/ / ~\（LLM 给的路径常带波浪号；Windows 上可能是反斜杠）
fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" || p.starts_with("~/") || p.starts_with("~\\") {
        if let Some(home) = home_dir() {
            return PathBuf::from(home).join(p.trim_start_matches(['~', '/', '\\']));
        }
    }
    PathBuf::from(p)
}

/// Windows `canonicalize` 返回 `\\?\` 扩展长度前缀路径；剥掉再返回，
/// 避免该前缀泄漏给模型/前端（复读回 path 参数虽仍能解析，但易读性差且易困惑模型）。
/// 仅剥盘符形式（\\?\C:\...）；UNC 形式（\\?\UNC\...）保守起见原样保留。
/// 非 Windows 平台路径不会有此前缀，天然 no-op。
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let stripped = p
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .filter(|rest| !rest.starts_with(r"UNC\"))
        .map(str::to_string);
    match stripped {
        Some(s) => PathBuf::from(s),
        None => p,
    }
}

/// 白名单原始路径合并（2026-08-26 追加语义）：内置默认（桌面/下载/文档）+ 任务卡绑定
/// 文件夹 + 用户 allowedDirs 三者并集（不再「用户列表整体替换默认」——否则授权弹窗
/// 「始终允许」写入一个目录后，内置默认反而失效）。抽纯函数便于单测。
fn merge_raw_dirs(
    cfg_dirs: &[String],
    home: Option<&Path>,
    task_dirs: Vec<String>,
) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if let Some(home) = home {
        for d in ["Desktop", "Downloads", "Documents"] {
            raw.push(home.join(d).to_string_lossy().to_string());
        }
    }
    raw.extend(task_dirs);
    raw.extend(cfg_dirs.iter().cloned());
    raw
}

/// 白名单目录集合（canonical 化，只保留真实存在的目录）
async fn allowed_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let cfg = crate::bot::load_config(app);
    // 任务卡绑定的文件夹（用户显式绑过 = 显式授权过）
    let mut task_dirs: Vec<String> = Vec::new();
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            for f in t.effective_files() {
                if f.is_dir {
                    task_dirs.push(f.path);
                }
            }
        }
    }
    let raw = merge_raw_dirs(&cfg.allowed_dirs, home_dir().as_deref(), task_dirs);
    let mut out: Vec<PathBuf> = Vec::new();
    for r in raw {
        let p = expand_tilde(r.trim());
        if let Ok(c) = std::fs::canonicalize(&p) {
            if c.is_dir() && !out.contains(&c) {
                out.push(c);
            }
        }
    }
    out
}

/// 路径守卫（2026-08-26 授权模式改造，Kimi CLI 风格「执行前授权」）：
/// ~ 展开 → canonicalize（不存在即拒）→ 白名单命中直接放行；
/// 白名单外按 perm_mode 分流：strict 硬拒（旧行为）/ ask 弹授权窗（允许一次 /
/// 始终允许该目录自动写入 allowedDirs / 拒绝）/ yolo 直接放行。全程记审计。
/// 成功返回 canonical 路径（Windows 剥 \\?\ 前缀）。
pub async fn resolve_with_perm(app: &AppHandle, tool: &str, path: &str) -> Result<PathBuf, String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("路径不能为空".into());
    }
    let expanded = expand_tilde(p);
    let canonical = std::fs::canonicalize(&expanded)
        .map_err(|_| format!("路径不存在或不可访问：{p}"))?;
    let dirs = allowed_dirs(app).await;
    if dirs.iter().any(|d| canonical.starts_with(d)) {
        return Ok(strip_verbatim(canonical));
    }
    match crate::bot::perm_mode(app) {
        crate::bot::PermMode::Yolo => {
            crate::bot::audit_log(
                app,
                &format!("bot_fs.yolo_allow | tool: {tool} | path: {}", crate::bot::truncate_for_log(p, 200)),
            );
            Ok(strip_verbatim(canonical))
        }
        crate::bot::PermMode::Ask => {
            match crate::bot_slash::ask_path_confirm(app, tool, &format!("访问白名单外路径：{p}"))
                .await
            {
                crate::bot_slash::ConfirmChoice::Once => {
                    crate::bot::audit_log(
                        app,
                        &format!("bot_fs.ask_allow_once | tool: {tool} | path: {}", crate::bot::truncate_for_log(p, 200)),
                    );
                    Ok(strip_verbatim(canonical))
                }
                crate::bot_slash::ConfirmChoice::Always => {
                    // 文件取父目录，目录取自身（授权粒度 = 目录，与判定逻辑一致）
                    let dir = if canonical.is_dir() {
                        canonical.clone()
                    } else {
                        canonical.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| canonical.clone())
                    };
                    let dir_str = strip_verbatim(dir).to_string_lossy().to_string();
                    if let Err(e) = crate::bot::add_allowed_dir(app, &dir_str) {
                        crate::bot::audit_log(app, &format!("bot_fs.always_persist_fail | {e}"));
                    }
                    crate::bot::audit_log(
                        app,
                        &format!("bot_fs.ask_allow_always | tool: {tool} | dir: {}", crate::bot::truncate_for_log(&dir_str, 200)),
                    );
                    Ok(strip_verbatim(canonical))
                }
                crate::bot_slash::ConfirmChoice::Deny => {
                    crate::bot::audit_log(
                        app,
                        &format!("bot_fs.ask_denied | tool: {tool} | path: {}", crate::bot::truncate_for_log(p, 200)),
                    );
                    Err(format!(
                        "用户未授权访问该路径：{p}（可在授权弹窗点「始终允许该目录」，或在设置页把目录加入白名单）"
                    ))
                }
            }
        }
        crate::bot::PermMode::Strict => {
            crate::bot::audit_log(
                app,
                &format!("bot_fs.denied | path: {} | 不在白名单目录内", crate::bot::truncate_for_log(p, 200)),
            );
            Err(format!(
                "路径不在白名单目录内：{p}（白名单：设置页 allowedDirs，默认 桌面/下载/文档 + 任务卡绑定文件夹）"
            ))
        }
    }
}

/// 简化 glob 匹配（只支持 * 任意串、? 单字符；大小写敏感；匹配文件/目录名）
fn glob_match(pattern: &str, name: &str) -> bool {
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
fn walk(dir: &Path, mut visit: impl FnMut(&Path, bool) -> bool) {
    let mut stack: Vec<(PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        if depth > WALK_MAX_DEPTH {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
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
fn is_binary_file(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return true };
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

// ───────────────────────── 工具实现（bot 分发签名：(String, Vec<TaskRef>)） ─────────────────────────

/// read_text_file：读白名单内 UTF-8 文本，offset/limit 行切片（1 起），超 100KB 截断
pub async fn tool_read_text_file(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str() else {
        return ("read_text_file 缺少 path".into(), Vec::new());
    };
    let canonical = match resolve_with_perm(app, "read_text_file", path).await {
        Ok(p) => p,
        Err(e) => return (e, Vec::new()),
    };
    if canonical.is_dir() {
        return (format!("{} 是目录，列文件请用 list_files", path.trim()), Vec::new());
    }
    if is_binary_file(&canonical) {
        return (
            format!("{} 是二进制/非文本文件；Office/PDF 文档请用 extract_document", path.trim()),
            Vec::new(),
        );
    }
    let offset = v["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = (v["limit"].as_u64().unwrap_or(READ_DEFAULT_LINES as u64) as usize)
        .min(READ_MAX_LINES);
    let raw = match std::fs::read(&canonical) {
        Ok(b) => b,
        Err(e) => return (format!("读取失败：{e}"), Vec::new()),
    };
    let byte_truncated = raw.len() > READ_MAX_BYTES;
    let text = String::from_utf8_lossy(&raw[..raw.len().min(READ_MAX_BYTES)]).to_string();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if offset > total {
        return (format!("文件共 {total} 行，offset {offset} 超出范围"), Vec::new());
    }
    let slice: Vec<String> = lines
        .iter()
        .skip(offset - 1)
        .take(limit)
        .enumerate()
        .map(|(i, l)| format!("{}: {}", offset + i, l))
        .collect();
    let end = offset + slice.len() - 1;
    let mut out = format!("{}（第 {offset}-{end} 行 / 共 {total} 行）\n{}", path.trim(), slice.join("\n"));
    if end < total || byte_truncated {
        out.push_str(&format!("\n…（截断，继续读请用 offset={}", end + 1));
        if byte_truncated {
            out.push_str(&format!("；文件超 {}KB 只读了前部", READ_MAX_BYTES / 1024));
        }
        out.push(')');
    }
    crate::bot::audit_log(app, &format!("bot_fs.read | {} | lines {offset}-{end}/{total}", canonical.display()));
    (out, Vec::new())
}

/// grep_files：白名单目录内正则搜文件内容，输出 path:line:内容（ripgrep 风格）
pub async fn tool_grep_files(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = crate::bot::parse_args(args);
    let Some(pattern) = v["pattern"].as_str() else {
        return ("grep_files 缺少 pattern".into(), Vec::new());
    };
    // 非法正则降级为字面量搜索（regex::escape），不让一个坏 pattern 炸掉整轮
    let re = regex::Regex::new(pattern)
        .or_else(|_| regex::Regex::new(&regex::escape(pattern)));
    let Ok(re) = re else {
        return (format!("无效的正则表达式：{pattern}"), Vec::new());
    };
    let glob = v["glob"].as_str().unwrap_or("").trim().to_string();
    let max = (v["max"].as_u64().unwrap_or(GREP_MAX_HITS as u64) as usize).min(GREP_MAX_HITS);
    // dir 可选：缺省搜第一个白名单目录
    let dir = match v["dir"].as_str() {
        Some(d) if !d.trim().is_empty() => match resolve_with_perm(app, "grep_files", d).await {
            Ok(p) => p,
            Err(e) => return (e, Vec::new()),
        },
        _ => match allowed_dirs(app).await.first() {
            Some(d) => d.clone(),
            None => return ("没有可用的白名单目录".into(), Vec::new()),
        },
    };
    if !dir.is_dir() {
        return (format!("{} 不是目录", dir.display()), Vec::new());
    }
    let mut hits: Vec<String> = Vec::new();
    walk(&dir, &mut |path: &Path, is_dir: bool| {
        if hits.len() >= max {
            return false;
        }
        if is_dir {
            return true;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !glob.is_empty() && !glob_match(&glob, &name) {
            return true;
        }
        if path.metadata().map(|m| m.len() > GREP_MAX_FILE_BYTES).unwrap_or(true) {
            return true; // 超大文件跳过
        }
        if is_binary_file(path) {
            return true;
        }
        if let Ok(text) = std::fs::read_to_string(path) {
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{}:{}: {}", path.display(), i + 1, line.chars().take(200).collect::<String>()));
                    if hits.len() >= max {
                        return false;
                    }
                }
            }
        }
        true
    });
    crate::bot::audit_log(app, &format!("bot_fs.grep | dir: {} | pattern: {} | hits: {}", dir.display(), crate::bot::truncate_for_log(pattern, 100), hits.len()));
    if hits.is_empty() {
        return (format!("{} 内没有匹配「{pattern}」的内容", dir.display()), Vec::new());
    }
    let mut out = hits.join("\n");
    if hits.len() >= max {
        out.push_str(&format!("\n…（已达 {max} 条上限，缩小范围或加 glob 过滤）"));
    }
    (out, Vec::new())
}

/// list_files：列白名单目录内文件（可选 glob 过滤文件名），深度 ≤5，上限 200 条
pub async fn tool_list_files(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = crate::bot::parse_args(args);
    let Some(dir) = v["dir"].as_str() else {
        return ("list_files 缺少 dir".into(), Vec::new());
    };
    let canonical = match resolve_with_perm(app, "list_files", dir).await {
        Ok(p) => p,
        Err(e) => return (e, Vec::new()),
    };
    if !canonical.is_dir() {
        return (format!("{} 不是目录；读文件请用 read_text_file", dir.trim()), Vec::new());
    }
    let pattern = v["pattern"].as_str().unwrap_or("").trim().to_string();
    let mut entries: Vec<String> = Vec::new();
    walk(&canonical, &mut |path: &Path, is_dir: bool| {
        if entries.len() >= LIST_MAX_ENTRIES {
            return false;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !pattern.is_empty() && !glob_match(&pattern, &name) {
            return true;
        }
        let rel = path.strip_prefix(&canonical).unwrap_or(path);
        entries.push(format!("{}{}", rel.display(), if is_dir { "/" } else { "" }));
        true
    });
    crate::bot::audit_log(app, &format!("bot_fs.list | dir: {} | entries: {}", canonical.display(), entries.len()));
    if entries.is_empty() {
        return (format!("{} 内没有匹配的文件", canonical.display()), Vec::new());
    }
    let mut out = format!("{}（{} 条）：\n{}", canonical.display(), entries.len(), entries.join("\n"));
    if entries.len() >= LIST_MAX_ENTRIES {
        out.push_str(&format!("\n…（已达 {LIST_MAX_ENTRIES} 条上限，用 pattern 过滤缩小范围）"));
    }
    (out, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_match_basics() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "a.b.rs"));
        assert!(!glob_match("*.rs", "main.ts"));
        assert!(glob_match("todo*", "todo_list"));
        assert!(glob_match("????.txt", "2024.txt"));
        assert!(!glob_match("????.txt", "20245.txt"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
        assert!(glob_match("a*c", "abc"));
        assert!(!glob_match("a*c", "abd"));
    }

    #[test]
    fn expand_tilde_home() {
        // 走 home_dir() 跨平台解析（Windows 上 HOME 常缺失，不能直接 unwrap HOME）
        let home = home_dir().expect("dev/CI 机器应有可解析的用户主目录");
        assert_eq!(expand_tilde("~/x"), PathBuf::from(&home).join("x"));
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        // Windows 风格波浪号也展开（LLM 可能给 ~\Desktop）
        assert_eq!(expand_tilde("~\\x"), PathBuf::from(&home).join("x"));
    }

    /// Windows 绿色版 bug 根因（2026-08-20）：HOME 缺失时须回退 USERPROFILE
    #[test]
    fn home_dir_fallbacks() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        // HOME 优先
        assert_eq!(
            home_dir_from(os("/h"), os(r"C:\Users\u"), None, None),
            Some(PathBuf::from("/h"))
        );
        // HOME 缺失（或为空）→ USERPROFILE
        assert_eq!(
            home_dir_from(None, os(r"C:\Users\u"), None, None),
            Some(PathBuf::from(r"C:\Users\u"))
        );
        assert_eq!(
            home_dir_from(os(""), os(r"C:\Users\u"), None, None),
            Some(PathBuf::from(r"C:\Users\u"))
        );
        // 再退 HOMEDRIVE+HOMEPATH
        assert_eq!(
            home_dir_from(None, None, os("C:"), os(r"\Users\u")),
            Some(PathBuf::from(r"C:\Users\u"))
        );
        // 全空 → None
        assert_eq!(home_dir_from(None, None, None, None), None);
    }

    /// Windows canonicalize 的 \\?\ 前缀剥离：盘符形式剥掉，UNC 保留，普通路径不动
    #[test]
    fn strip_verbatim_prefix() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Users\u\Desktop\x.docx")),
            PathBuf::from(r"C:\Users\u\Desktop\x.docx")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\x.docx")),
            PathBuf::from(r"\\?\UNC\server\share\x.docx")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/Users/u/Desktop/x.docx")),
            PathBuf::from("/Users/u/Desktop/x.docx")
        );
    }

    /// 白名单判定核心：starts_with 是按路径分量比较，
    /// 前缀相似的不同目录（/tmp/ab vs /tmp/abc）不会误判命中
    #[test]
    fn prefix_similar_dirs_not_confused() {
        let allowed = Path::new("/tmp/ab");
        assert!(Path::new("/tmp/ab/c").starts_with(allowed));
        assert!(Path::new("/tmp/ab").starts_with(allowed));
        assert!(!Path::new("/tmp/abc").starts_with(allowed));
        assert!(!Path::new("/tmp/ab2/x").starts_with(allowed));
    }

    /// 2026-08-26 追加语义：allowedDirs 非空时内置默认（桌面/下载/文档）仍生效，
    /// 任务卡绑定文件夹也在并集里（旧行为是用户列表整体替换默认，会导致授权弹窗
    /// 「始终允许」写入一个目录后内置默认失效）
    #[test]
    fn merge_raw_dirs_appends_to_defaults() {
        let home = PathBuf::from("/home/u");
        let cfg = vec!["/data/work".to_string()];
        let task = vec!["/mnt/bound".to_string()];
        let raw = merge_raw_dirs(&cfg, Some(&home), task);
        assert_eq!(
            raw,
            vec![
                "/home/u/Desktop",
                "/home/u/Downloads",
                "/home/u/Documents",
                "/mnt/bound",
                "/data/work"
            ]
        );
    }

    /// home 缺失（Windows 绿色版 HOME/USERPROFILE 全空兜底场景）时不产默认目录，
    /// 但任务卡绑定与用户配置仍在
    #[test]
    fn merge_raw_dirs_without_home() {
        let raw = merge_raw_dirs(&["/data/x".to_string()], None, vec!["/mnt/b".to_string()]);
        assert_eq!(raw, vec!["/mnt/b", "/data/x"]);
    }
}
