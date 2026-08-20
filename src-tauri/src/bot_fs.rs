//! 本地文件只读工具（2026-08-19 Phase 1：read_text_file / grep_files / list_files）。
//!
//! 安全设计（对齐 Harness 网关 + 安全红线「不遍历全盘」的白名单化落地）：
//! - 只允许访问白名单目录内路径：bot-config.json `allowedDirs` 非空则用用户列表，
//!   空则用内置默认（~/Desktop ~/Downloads ~/Documents + 任务卡绑定文件夹）
//! - resolve_allowed：展开 ~ → canonicalize（不存在即拒绝）→ 必须落在白名单内；
//!   `..` 与软链逃逸在 canonicalize 后无处遁形；每次拒绝记审计
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

/// 展开路径开头的 ~ / ~/（LLM 给的路径常带波浪号）
fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" || p.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(p.trim_start_matches("~/").trim_start_matches('~'));
        }
    }
    PathBuf::from(p)
}

/// 白名单目录集合（canonical 化，只保留真实存在的目录）
async fn allowed_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let cfg = crate::bot::load_config(app);
    let mut raw: Vec<String> = cfg.allowed_dirs.clone();
    if raw.is_empty() {
        // 内置默认：桌面/下载/文档
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            for d in ["Desktop", "Downloads", "Documents"] {
                raw.push(home.join(d).to_string_lossy().to_string());
            }
        }
        // + 任务卡绑定的文件夹（用户显式绑过 = 显式授权过）
        if let Ok(tasks) = crate::db::db_load(app.clone()).await {
            for t in tasks {
                for f in t.effective_files() {
                    if f.is_dir {
                        raw.push(f.path);
                    }
                }
            }
        }
    }
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

/// 路径守卫：~ 展开 → canonicalize（不存在即拒）→ 必须落在某个白名单目录内。
/// 成功返回 canonical 路径；拒绝记审计（白名单命中/拒绝都留痕）。
pub async fn resolve_allowed(app: &AppHandle, path: &str) -> Result<PathBuf, String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("路径不能为空".into());
    }
    let expanded = expand_tilde(p);
    let canonical = std::fs::canonicalize(&expanded)
        .map_err(|_| format!("路径不存在或不可访问：{p}"))?;
    let dirs = allowed_dirs(app).await;
    if dirs.iter().any(|d| canonical.starts_with(d)) {
        Ok(canonical)
    } else {
        crate::bot::audit_log(
            app,
            &format!("bot_fs.denied | path: {} | 不在白名单目录内", crate::bot::truncate_for_log(p, 200)),
        );
        Err(format!(
            "路径不在白名单目录内：{p}（白名单：设置页 allowedDirs，默认 桌面/下载/文档 + 任务卡绑定文件夹）"
        ))
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
    let canonical = match resolve_allowed(app, path).await {
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
        Some(d) if !d.trim().is_empty() => match resolve_allowed(app, d).await {
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
    let canonical = match resolve_allowed(app, dir).await {
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
        let home = std::env::var_os("HOME").unwrap();
        assert_eq!(expand_tilde("~/x"), PathBuf::from(&home).join("x"));
        assert_eq!(expand_tilde("~"), PathBuf::from(&home));
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
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
}
