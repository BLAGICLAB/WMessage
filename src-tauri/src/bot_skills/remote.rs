//! 远程技能源：从 GitHub 仓库 / 腾讯 SkillHub / 直链 zip 取技能包，装进技能目录。
//!
//! 装的是**外部内容**：下载地址、解包条目名、技能正文都不可信。所以每一道闸
//! 都在落盘前问一句——下载走公网闸（逐跳校验重定向目标）、解包走归档工具那套
//! 限额闸（zip slip / 软链 / zip bomb）、技能名与危险词走 skills_import 同款校验。
//!
//! 装完与设置页导入落进同一个技能目录、同一套路由重建口径：装上即生效，用户
//! 说出技能的 intents 关键词即进入该技能流程。

use std::path::{Path, PathBuf};

use crate::bot::registry::ToolResult;

// ── 上限阀 ────────────────────────────────────────────────────────────────
//
// 下载体积上限：技能包本身是几十 KB 的文档 + 少量脚本；给到 64MB 是给「带
// 模型或数据集的仓库型技能包」留的余量，再大就不是技能包而是整个仓库。

/// 单次下载体积上限。
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// 装进技能目录的单文件上限。
const MAX_SKILL_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// 装进技能目录的文件数上限。
const MAX_SKILL_FILES: usize = 2000;
/// 装进技能目录的总字节上限。
const MAX_SKILL_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// 重定向跳数上限。
const MAX_REDIRECTS: usize = 5;
/// 技能包内的目录树扫描深度上限（够 cover `<repo>/skills/<skill>/`）。
const MAX_TREE_DEPTH: usize = 6;
/// 技能包内候选技能目录数上限（超了只取前 N 个用于报错提示）。
const MAX_CANDIDATES: usize = 200;

/// 下载用的 User-Agent：不少 CDN 对空 UA 直接 403。
const UA: &str = "wmessage-skill-installer/1.0";

/// SkillHub 公开 API 根（免鉴权；下载走 302 跳到对象存储）。
const SKILLHUB_API: &str = "https://api.skillhub.cn";

// ── 来源解析 ──────────────────────────────────────────────────────────────

/// 技能来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkillSource {
    /// 腾讯 SkillHub：slug 为公开 slug，owner 为命名空间前缀（可选）。
    SkillHub { slug: String, owner: Option<String> },
    /// GitHub 仓库；skill 为仓库内技能目录名（仓库含多个技能时用）。
    GitHub {
        owner: String,
        repo: String,
        skill: Option<String>,
    },
    /// 直链 zip。
    Zip { url: String },
}

/// 把用户/模型给的来源串解析成技能来源。
///
/// 接受的写法：
/// - `https://github.com/<owner>/<repo>` / `https://github.com/<owner>/<repo>/tree/<branch>/<path>`
/// - `https://skillhub.cn/skills/<slug>`（含 `/skills/<owner>/<slug>` 形态）
/// - `owner/repo`、`owner/repo#技能名`
/// - `skillhub:<slug>`、`@<owner>/<slug>`
/// - 任意 `*.zip` 直链
pub(crate) fn parse_source(input: &str) -> Result<SkillSource, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("来源为空：给 GitHub 仓库地址、SkillHub 技能页地址或 zip 直链".into());
    }
    if let Some(rest) = s.strip_prefix("skillhub:") {
        return parse_skillhub_slug(rest);
    }
    // `@owner/slug` 是 SkillHub 的规范写法（与 GitHub 的裸 owner/repo 区分）。
    if let Some(rest) = s.strip_prefix('@') {
        return parse_skillhub_slug(rest);
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return parse_url_source(s);
    }
    parse_github_bare(s)
}

fn parse_skillhub_slug(rest: &str) -> Result<SkillSource, String> {
    let rest = rest.trim().trim_matches('/');
    if rest.is_empty() {
        return Err("SkillHub 来源缺少 slug".into());
    }
    let (owner, slug) = match rest.split_once('/') {
        Some((o, s)) => (Some(o.to_string()), s.to_string()),
        None => (None, rest.to_string()),
    };
    if slug.is_empty() {
        return Err("SkillHub 来源缺少 slug".into());
    }
    Ok(SkillSource::SkillHub { slug, owner })
}

fn parse_url_source(raw: &str) -> Result<SkillSource, String> {
    let u = url::Url::parse(raw).map_err(|e| format!("网址无效：{e}"))?;
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    let segs: Vec<String> = u
        .path_segments()
        .map(|it| {
            it.filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    if is_skillhub_host(&host) {
        let idx = segs
            .iter()
            .position(|s| s == "skills")
            .ok_or_else(|| format!("SkillHub 网址缺少 /skills/<slug> 路径：{raw}"))?;
        let rest = &segs[idx + 1..];
        let slug = rest
            .last()
            .ok_or_else(|| format!("SkillHub 网址缺少技能名：{raw}"))?
            .clone();
        let owner = if rest.len() >= 2 {
            Some(rest[rest.len() - 2].clone())
        } else {
            None
        };
        return Ok(SkillSource::SkillHub { slug, owner });
    }

    if host == "github.com" || host == "www.github.com" {
        if segs.len() < 2 {
            return Err(format!("GitHub 网址缺少 owner/repo：{raw}"));
        }
        let owner = segs[0].clone();
        let repo = segs[1].trim_end_matches(".git").to_string();
        validate_github_seg(&owner)?;
        validate_github_seg(&repo)?;
        // /tree/<branch>/<path> 与 /blob/<branch>/<path>：path 末段是技能目录，
        // 指向具体文件时取它的父目录名。
        let skill = if segs.len() > 3 && (segs[2] == "tree" || segs[2] == "blob") {
            let path = segs[4..].join("/");
            let leaf = path.rsplit('/').next().unwrap_or_default().to_string();
            if leaf.eq_ignore_ascii_case("SKILL.md") {
                // 指向入口文件：取父目录的**目录名**（不是父目录整条路径——
                // 定位阶段按目录名匹配，给整条路径匹配不上）
                path.rsplit_once('/')
                    .map(|(parent, _)| parent.rsplit('/').next().unwrap_or_default())
                    .unwrap_or_default()
                    .to_string()
            } else {
                leaf
            }
        } else {
            String::new()
        };
        return Ok(SkillSource::GitHub {
            owner,
            repo,
            skill: if skill.is_empty() { None } else { Some(skill) },
        });
    }

    if segs
        .last()
        .map(|s| s.to_ascii_lowercase().ends_with(".zip"))
        .unwrap_or(false)
    {
        return Ok(SkillSource::Zip {
            url: raw.to_string(),
        });
    }

    Err(format!(
        "无法识别的技能来源（支持 GitHub 仓库 / SkillHub 技能页 / zip 直链）：{raw}"
    ))
}

fn parse_github_bare(s: &str) -> Result<SkillSource, String> {
    let (repo_part, skill) = match s.split_once('#') {
        Some((r, k)) => (r, Some(k.trim().to_string())),
        None => (s, None),
    };
    let (owner, repo) = repo_part.split_once('/').ok_or_else(|| {
        format!("无法识别的技能来源：{s}（要装 GitHub 技能请给 owner/repo 或完整网址）")
    })?;
    validate_github_seg(owner)?;
    validate_github_seg(repo)?;
    Ok(SkillSource::GitHub {
        owner: owner.to_string(),
        repo: repo.to_string(),
        skill: skill.filter(|k| !k.is_empty()),
    })
}

/// GitHub 路径段白名单：只允许字母/数字/点/横线/下划线，杜绝路径注入。
fn validate_github_seg(seg: &str) -> Result<(), String> {
    if seg.is_empty() {
        return Err("GitHub 仓库名不能为空".into());
    }
    if seg == "." || seg == ".." {
        return Err("GitHub 仓库名无效".into());
    }
    if !seg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(format!("GitHub 仓库名「{seg}」含非法字符"));
    }
    Ok(())
}

fn is_skillhub_host(host: &str) -> bool {
    host == "skillhub.cn"
        || host == "www.skillhub.cn"
        || host == "skillhub.tencent.com"
        || host == "www.skillhub.tencent.com"
        || host == "skillhub.cloud.tencent.com"
}

/// query 值转义：slug 里可能有 `/`（owner/slug 形态），要按 query 值转义。
fn encode_query_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── 下载 ──────────────────────────────────────────────────────────────────

/// 下载字节流：公网闸 + 逐跳校验重定向 + 体积闸。
///
/// 禁自动重定向（与 fetch_url 同口径）：公网 URL 302 到内网会绕过 SSRF 闸，
/// 所以每一跳都重新过一遍公网校验再决定跟不跟。
async fn fetch_bytes(url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let mut cur = url.to_string();
    for _ in 0..MAX_REDIRECTS {
        let u = url::Url::parse(&cur).map_err(|e| format!("网址无效：{cur}（{e}）"))?;
        let addrs = crate::bot_web::check_public_url(&u)
            .await
            .map_err(|e| e.to_string())?;
        let client = crate::bot_web::http_client_pinned(&u, &addrs).map_err(|e| e.to_string())?;
        let resp = client
            .get(u.clone())
            .header("User-Agent", UA)
            .send()
            .await
            .map_err(|e| format!("下载失败：{e}"))?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "重定向响应缺少 Location".to_string())?;
            cur = u
                .join(loc)
                .map_err(|e| format!("重定向地址无效：{loc}（{e}）"))?
                .to_string();
            continue;
        }
        if !status.is_success() {
            return Err(format!("下载失败：HTTP {}", status.as_u16()));
        }
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(format!("下载体积 {len} 字节超过上限 {max_bytes} 字节"));
            }
        }
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp;
        while let Some(chunk) = stream.chunk().await.map_err(|e| format!("下载中断：{e}"))? {
            if buf.len() as u64 + chunk.len() as u64 > max_bytes {
                return Err(format!("下载体积超过上限 {max_bytes} 字节"));
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok(buf);
    }
    Err(format!("重定向跳数超过 {MAX_REDIRECTS}"))
}

/// 解析下载地址候选（按序试，第一个成功即用）。
///
/// SkillHub 的 slug 有「裸 slug」与「owner/slug」两种可下载形态，两种都试。
async fn resolve_download_urls(source: &SkillSource) -> Result<Vec<String>, String> {
    match source {
        SkillSource::SkillHub { slug, owner } => {
            let mut urls = vec![skillhub_download_url(slug)];
            if let Some(owner) = owner {
                let qualified = format!("{owner}/{slug}");
                if qualified != *slug {
                    urls.push(skillhub_download_url(&qualified));
                }
            }
            Ok(urls)
        }
        SkillSource::GitHub { owner, repo, .. } => {
            let branch = github_default_branch(owner, repo).await?;
            Ok(vec![format!(
                "https://codeload.github.com/{owner}/{repo}/zip/refs/heads/{branch}"
            )])
        }
        SkillSource::Zip { url } => Ok(vec![url.clone()]),
    }
}

fn skillhub_download_url(slug: &str) -> String {
    format!(
        "{SKILLHUB_API}/api/v1/download?slug={}",
        encode_query_value(slug)
    )
}

/// 查仓库默认分支：codeload 的分支路径写错就是 404，先问准再下。
async fn github_default_branch(owner: &str, repo: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}");
    let body = fetch_bytes(&url, 512 * 1024)
        .await
        .map_err(|e| format!("读取仓库信息失败：{e}"))?;
    let text = String::from_utf8_lossy(&body).to_string();
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("仓库信息不是合法 JSON：{e}"))?;
    v.get("default_branch")
        .and_then(|b| b.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("仓库 {owner}/{repo} 没有默认分支（可能不存在或不是公开仓库）"))
}

// ── 技能包定位 ────────────────────────────────────────────────────────────

/// 在解包后的树里找到技能目录（含 SKILL.md 的那一层）。
///
/// 两种包型：SkillHub 的包把 SKILL.md 放在**根**（无顶层目录）；GitHub 的仓库包
/// 带 `<repo>-<branch>/` 顶层目录，技能在 `skills/<name>/` 下。
fn locate_skill(root: &Path, hint: Option<&str>) -> Result<PathBuf, String> {
    if has_skill_md(root) {
        return Ok(root.to_path_buf());
    }
    let mut hits: Vec<PathBuf> = Vec::new();
    collect_skill_dirs(root, 0, &mut hits);
    if hits.is_empty() {
        return Err("包里没有 SKILL.md，不是有效的技能包".into());
    }
    if hits.len() == 1 {
        return Ok(hits.remove(0));
    }
    if let Some(h) = hint {
        let h = h.trim().trim_matches('/');
        if !h.is_empty() {
            if let Some(m) = hits
                .iter()
                .find(|d| d.file_name().map(|n| n == h).unwrap_or(false))
            {
                return Ok(m.clone());
            }
            if let Some(m) = hits.iter().find(|d| {
                d.to_string_lossy()
                    .replace('\\', "/")
                    .ends_with(&format!("/{h}"))
            }) {
                return Ok(m.clone());
            }
        }
    }
    let names: Vec<String> = hits
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    Err(format!(
        "包里包含多个技能，请指定要装哪一个（owner/repo#技能名）：{}",
        names
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("、")
    ))
}

/// 目录里有没有 SKILL.md（文件名大小写不敏感——不同来源写法不一）。
fn has_skill_md(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("SKILL.md")
    })
}

fn collect_skill_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_TREE_DEPTH || out.len() >= MAX_CANDIDATES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        if name == "__MACOSX" || name == ".git" || name == "node_modules" {
            continue;
        }
        if has_skill_md(&p) {
            out.push(p.clone());
        }
        collect_skill_dirs(&p, depth + 1, out);
    }
}

// ── 落盘 ──────────────────────────────────────────────────────────────────

/// 把技能目录装进技能根目录：排他占位 + 失败回滚。
fn install_skill_dir(src: &Path, dest_root: &Path, name: &str) -> Result<(usize, u64), String> {
    let dest = dest_root.join(name);
    if dest.exists() {
        return Err(format!(
            "技能「{name}」已存在（如需覆盖请先删除同名技能再装）"
        ));
    }
    std::fs::create_dir_all(dest_root).map_err(|e| format!("创建技能目录失败：{e}"))?;
    std::fs::create_dir(&dest)
        .map_err(|_| format!("技能「{name}」已存在（如需覆盖请先删除同名技能再装）"))?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    if let Err(e) = copy_skill_tree(src, &dest, &mut files, &mut bytes) {
        // 回滚：留下半份技能目录 = 装得上用不了删不掉
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e);
    }
    Ok((files, bytes))
}

/// 复制技能目录内容（软链不落盘；SKILL.md 统一改名成大写形态）。
fn copy_skill_tree(
    src: &Path,
    dest: &Path,
    files: &mut usize,
    bytes: &mut u64,
) -> Result<(), String> {
    let mut stack: Vec<(PathBuf, PathBuf)> = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((sdir, ddir)) = stack.pop() {
        let entries = std::fs::read_dir(&sdir).map_err(|e| format!("读取技能目录失败：{e}"))?;
        for e in entries.flatten() {
            let p = e.path();
            let raw_name = e.file_name().to_string_lossy().to_string();
            if raw_name == ".DS_Store" || raw_name == "__MACOSX" {
                continue;
            }
            let ft = e
                .file_type()
                .map_err(|e| format!("读取文件类型失败：{e}"))?;
            if ft.is_symlink() {
                continue;
            }
            // 技能入口文件名统一成 SKILL.md：不同来源的大小写写法不一，
            // 而加载器只认这个确切名字。
            let name = if raw_name.eq_ignore_ascii_case("skill.md") {
                "SKILL.md".to_string()
            } else {
                raw_name.clone()
            };
            let d = ddir.join(&name);
            if ft.is_dir() {
                std::fs::create_dir_all(&d).map_err(|e| format!("创建目录失败：{e}"))?;
                stack.push((p, d));
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if len > MAX_SKILL_FILE_BYTES {
                return Err(format!(
                    "技能文件「{raw_name}」超过单文件上限 {}MB",
                    MAX_SKILL_FILE_BYTES / 1024 / 1024
                ));
            }
            if *files + 1 > MAX_SKILL_FILES {
                return Err(format!("技能文件数超过 {MAX_SKILL_FILES} 上限"));
            }
            if *bytes + len > MAX_SKILL_TOTAL_BYTES {
                return Err(format!(
                    "技能包总大小超过 {}MB 上限",
                    MAX_SKILL_TOTAL_BYTES / 1024 / 1024
                ));
            }
            std::fs::copy(&p, &d).map_err(|e| format!("复制技能文件失败：{e}"))?;
            *files += 1;
            *bytes += len;
        }
    }
    Ok(())
}

/// 技能名兜底：frontmatter 的 name 不合法（中文/符号）时用来源里的 slug。
/// 技能名会进路径，所以只留 ASCII 字母数字 + `-` / `_`。
fn sanitize_skill_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skill".to_string()
    } else {
        trimmed
    }
}

/// 定名：frontmatter 的 name 合法就用它，否则退回来源里的 slug。
///
/// 两处同名是硬要求：清单与路由用 frontmatter 名，磁盘目录用技能名——失配会产出
/// 「路由指 A、目录叫 B」的僵尸技能（清单里点得到、use_skill 却读不到）。
fn resolve_skill_name(frontmatter_name: &str, fallback: &str) -> String {
    if crate::bot_skills::validate_skill_name(frontmatter_name).is_ok() {
        frontmatter_name.to_string()
    } else {
        sanitize_skill_name(fallback)
    }
}

// ── 工具层 ────────────────────────────────────────────────────────────────

/// install_skill 工具：装远程技能（GitHub / SkillHub / zip 直链）。
pub async fn tool_install_skill(
    app: &tauri::AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(source_raw) = v["source"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::ok(
            "install_skill 缺少 source（GitHub 仓库地址 / SkillHub 技能页地址 / zip 直链）"
                .to_string(),
            Vec::new(),
        );
    };
    let hint = v["skill"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let source = match parse_source(source_raw) {
        Ok(s) => s,
        Err(e) => return ToolResult::ok(format!("install_skill：{e}"), Vec::new()),
    };

    let urls = match resolve_download_urls(&source).await {
        Ok(u) => u,
        Err(e) => return ToolResult::ok(format!("install_skill：{e}"), Vec::new()),
    };

    // 临时目录：解包与落盘之间的一次性中转，结束即清。
    let tmp = match TempWorkspace::new() {
        Ok(t) => t,
        Err(e) => return ToolResult::ok(format!("install_skill：{e}"), Vec::new()),
    };

    let mut zip_bytes: Option<Vec<u8>> = None;
    let mut last_err = String::new();
    for u in &urls {
        match fetch_bytes(u, MAX_DOWNLOAD_BYTES).await {
            Ok(b) => {
                zip_bytes = Some(b);
                break;
            }
            Err(e) => last_err = e,
        }
    }
    let Some(zip_bytes) = zip_bytes else {
        return ToolResult::ok(format!("install_skill：下载失败——{last_err}"), Vec::new());
    };

    let zip_path = tmp.path().join("skill.zip");
    if let Err(e) = std::fs::write(&zip_path, &zip_bytes) {
        return ToolResult::ok(format!("install_skill：写入临时文件失败——{e}"), Vec::new());
    }
    let unpack_dir = tmp.path().join("unpacked");
    if let Err(e) = std::fs::create_dir_all(&unpack_dir) {
        return ToolResult::ok(format!("install_skill：创建解包目录失败——{e}"), Vec::new());
    }
    let limits = crate::bot_archive::Limits::unpack();
    let unpack = crate::bot_archive::unpack(&zip_path, &unpack_dir, &limits, None);
    let outcome = match unpack {
        Ok(o) => o,
        Err(e) => {
            return ToolResult::ok(format!("install_skill：解包失败——{e}"), Vec::new());
        }
    };

    let hint_ref = hint.as_deref();
    let skill_dir = match locate_skill(&unpack_dir, hint_ref) {
        Ok(d) => d,
        Err(e) => return ToolResult::ok(format!("install_skill：{e}"), Vec::new()),
    };

    // 技能正文：读入口文件定名与校验
    let skill_md = skill_dir.join("SKILL.md");
    let text_path = if skill_md.is_file() {
        skill_md
    } else {
        match find_skill_md_file(&skill_dir) {
            Some(p) => p,
            None => {
                return ToolResult::ok(
                    "install_skill：包里没有 SKILL.md，不是有效的技能包".to_string(),
                    Vec::new(),
                )
            }
        }
    };
    let meta_text = match std::fs::read(&text_path) {
        Ok(b) => b,
        Err(e) => {
            return ToolResult::ok(
                format!("install_skill：读取 SKILL.md 失败——{e}"),
                Vec::new(),
            )
        }
    };
    if meta_text.len() as u64 > crate::bot_skills::IMPORT_MAX_SKILL_MD_BYTES {
        return ToolResult::ok(
            format!(
                "install_skill：SKILL.md 超过 {} 字节上限，拒绝安装",
                crate::bot_skills::IMPORT_MAX_SKILL_MD_BYTES
            ),
            Vec::new(),
        );
    }
    let text = String::from_utf8_lossy(&meta_text).to_string();

    // 定名：frontmatter 的 name 合法就用它（清单/路由/磁盘三处同名），
    // 否则退回来源里的 slug 并转成合法字符。
    let fallback = fallback_name(&source, &skill_dir, &unpack_dir);
    let meta = crate::bot_skills::parse_meta(&text, &fallback);
    let name = resolve_skill_name(&meta.name, &fallback);
    if let Err(reason) = crate::bot_skills::validate_created_meta(&meta) {
        return ToolResult::ok(format!("install_skill：拒绝安装——{reason}"), Vec::new());
    }

    // 安装前确认：装的是第三方内容，落盘后即进入技能目录、会被后续对话读取。
    let confirm_text = format!(
        "安装技能「{name}」（来源：{}）到技能目录，共 {} 个文件。",
        describe_source(&source),
        outcome.written.len()
    );
    let approved = crate::bot_slash::ask_user_confirm(
        app,
        "install_skill",
        &confirm_text,
        interactive,
        session_id,
    )
    .await;
    if !approved {
        return ToolResult::ok("用户拒绝了安装，技能未安装".to_string(), Vec::new());
    }

    let dest_root = crate::bot_skills::skills_dir(app);
    let (files, bytes) = match crate::py::document::spawn_blocking_map({
        let src = skill_dir.clone();
        let root = dest_root.clone();
        let name = name.clone();
        move || install_skill_dir(&src, &root, &name)
    })
    .await
    {
        Ok(r) => r,
        Err(e) => return ToolResult::ok(format!("install_skill：{e}"), Vec::new()),
    };

    crate::bot_skills::rebuild_intent_routes(app);
    crate::bot::audit_log(
        app,
        &format!(
            "skill_install | {} | source: {} | files: {files} | bytes: {bytes}",
            crate::bot::escape_for_log(&name, 100),
            crate::bot::escape_for_log(&describe_source(&source), 120)
        ),
    );

    let skipped = outcome.skipped.len();
    let mut out = format!(
        "技能「{name}」已安装并生效：{files} 个文件（{bytes} 字节），目录 {}。路由已重建：用户说出该技能 intents 关键词即会进入该技能流程。",
        dest_root.join(&name).display()
    );
    if skipped > 0 {
        out.push_str(&format!(
            "\n注意：包里有 {skipped} 个条目被跳过（路径不安全 / 软链 / 超限），未装入。"
        ));
    }
    if meta.description.trim().is_empty() {
        out.push_str("\n注意：该技能 frontmatter 没有 description，清单里不会展示它的用途。");
    }
    ToolResult::ok(out, Vec::new())
}

/// 技能目录里大小写不敏感地找 SKILL.md。
fn find_skill_md_file(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.eq_ignore_ascii_case("skill.md") {
            return Some(e.path());
        }
    }
    None
}

/// 名字兜底来源：SkillHub 用 slug，GitHub 用仓库内的技能目录名。
fn fallback_name(source: &SkillSource, skill_dir: &Path, unpack_root: &Path) -> String {
    match source {
        SkillSource::SkillHub { slug, .. } => slug.clone(),
        _ => {
            if skill_dir == unpack_root {
                "skill".to_string()
            } else {
                skill_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "skill".to_string())
            }
        }
    }
}

fn describe_source(source: &SkillSource) -> String {
    match source {
        SkillSource::SkillHub { slug, owner } => match owner {
            Some(o) => format!("SkillHub {o}/{slug}"),
            None => format!("SkillHub {slug}"),
        },
        SkillSource::GitHub { owner, repo, skill } => match skill {
            Some(s) => format!("GitHub {owner}/{repo}#{s}"),
            None => format!("GitHub {owner}/{repo}"),
        },
        SkillSource::Zip { url } => format!("zip {url}"),
    }
}

// ── 搜索 ──────────────────────────────────────────────────────────────────

/// search_skills 工具：在腾讯 SkillHub 上搜技能（公开 API，免鉴权）。
pub async fn tool_search_skills(app: &tauri::AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(keyword) = v["keyword"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::ok(
            "search_skills 缺少 keyword（搜索关键词）".to_string(),
            Vec::new(),
        );
    };
    let limit = v["limit"]
        .as_u64()
        .map(|n| n.clamp(1, 20) as usize)
        .unwrap_or(8);

    let url = format!(
        "{SKILLHUB_API}/api/skills?keyword={}&sortBy=score&pageSize={limit}",
        encode_query_value(keyword)
    );
    let body = match fetch_bytes(&url, 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return ToolResult::ok(format!("search_skills：搜索失败——{e}"), Vec::new()),
    };
    let text = String::from_utf8_lossy(&body).to_string();
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return ToolResult::ok(format!("search_skills：响应不是合法 JSON——{e}"), Vec::new())
        }
    };
    let skills = v
        .get("data")
        .and_then(|d| d.get("skills"))
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let total = v
        .get("data")
        .and_then(|d| d.get("total"))
        .and_then(|t| t.as_u64())
        .unwrap_or(skills.len() as u64);

    if skills.is_empty() {
        return ToolResult::ok(
            format!("SkillHub 上没搜到「{keyword}」相关技能（共 {total} 条命中，按相关度排序取前 {limit} 条）——换同义词或更具体的关键词再搜一次。"),
            Vec::new(),
        );
    }

    let mut out = format!(
        "SkillHub 上「{keyword}」相关技能共 {total} 条，按相关度取前 {} 条：",
        skills.len()
    );
    for (i, s) in skills.iter().enumerate() {
        let name = s.get("name").and_then(|x| x.as_str()).unwrap_or("(未命名)");
        let slug = s.get("slug").and_then(|x| x.as_str()).unwrap_or("");
        let desc = s
            .get("description_zh")
            .and_then(|x| x.as_str())
            .filter(|d| !d.trim().is_empty())
            .or_else(|| s.get("description").and_then(|x| x.as_str()))
            .unwrap_or("");
        let desc = truncate_chars(desc.trim(), 90);
        let category = s.get("category").and_then(|x| x.as_str()).unwrap_or("");
        let downloads = s.get("downloads").and_then(|x| x.as_u64()).unwrap_or(0);
        let installs = s.get("installs").and_then(|x| x.as_u64()).unwrap_or(0);
        out.push_str(&format!(
            "\n{i}. {name} —— {desc}\n   slug: {slug} | 分类: {category} | 下载: {downloads} | 安装: {installs}\n   安装来源：https://skillhub.cn/skills/{slug}"
        ));
    }
    out.push_str(
        "\n\n先把这些候选列给用户挑（说明各自做什么），用户选定后用 install_skill 的 source 传对应地址安装。",
    );
    crate::bot::audit_log(
        app,
        &format!(
            "skill_search | {} | hits: {}",
            crate::bot::escape_for_log(keyword, 60),
            skills.len()
        ),
    );
    ToolResult::ok(out, Vec::new())
}

/// 截断到指定字符数（中文按字符算，尾部补省略号）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── 临时目录 ──────────────────────────────────────────────────────────────

/// 一次性临时目录：退出即清（含失败路径）。
struct TempWorkspace {
    dir: PathBuf,
}

impl TempWorkspace {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "wmessage-skill-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
        Ok(Self { dir })
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ── 单测 ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 来源解析 ──

    #[test]
    fn parse_source_accepts_github_urls() {
        assert_eq!(
            parse_source("https://github.com/anthropics/skills").unwrap(),
            SkillSource::GitHub {
                owner: "anthropics".into(),
                repo: "skills".into(),
                skill: None
            }
        );
        assert_eq!(
            parse_source("https://github.com/owner/repo.git").unwrap(),
            SkillSource::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
                skill: None
            }
        );
        // tree 路径的末段是技能目录名
        assert_eq!(
            parse_source("https://github.com/o/r/tree/main/skills/my-skill").unwrap(),
            SkillSource::GitHub {
                owner: "o".into(),
                repo: "r".into(),
                skill: Some("my-skill".into())
            }
        );
        // blob 指向 SKILL.md 时取父目录名
        assert_eq!(
            parse_source("https://github.com/o/r/blob/main/skills/my-skill/SKILL.md").unwrap(),
            SkillSource::GitHub {
                owner: "o".into(),
                repo: "r".into(),
                skill: Some("my-skill".into())
            }
        );
    }

    #[test]
    fn parse_source_accepts_bare_github_with_skill() {
        assert_eq!(
            parse_source("owner/repo#my-skill").unwrap(),
            SkillSource::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
                skill: Some("my-skill".into())
            }
        );
        assert_eq!(
            parse_source("owner/repo").unwrap(),
            SkillSource::GitHub {
                owner: "owner".into(),
                repo: "repo".into(),
                skill: None
            }
        );
    }

    #[test]
    fn parse_source_accepts_skillhub_urls_and_slugs() {
        assert_eq!(
            parse_source("https://skillhub.cn/skills/weekly-report-pro").unwrap(),
            SkillSource::SkillHub {
                slug: "weekly-report-pro".into(),
                owner: None
            }
        );
        // 带命名空间的两段式：owner 取倒数第二段，slug 取最后一段
        assert_eq!(
            parse_source("https://skillhub.cn/skills/user_abc123/web-access").unwrap(),
            SkillSource::SkillHub {
                slug: "web-access".into(),
                owner: Some("user_abc123".into())
            }
        );
        assert_eq!(
            parse_source("https://skillhub.cloud.tencent.com/skills/foo").unwrap(),
            SkillSource::SkillHub {
                slug: "foo".into(),
                owner: None
            }
        );
        assert_eq!(
            parse_source("skillhub:my-skill").unwrap(),
            SkillSource::SkillHub {
                slug: "my-skill".into(),
                owner: None
            }
        );
        assert_eq!(
            parse_source("@user_abc123/my-skill").unwrap(),
            SkillSource::SkillHub {
                slug: "my-skill".into(),
                owner: Some("user_abc123".into())
            }
        );
    }

    #[test]
    fn parse_source_accepts_zip_direct_link() {
        assert_eq!(
            parse_source("https://example.com/packs/my-skill.zip").unwrap(),
            SkillSource::Zip {
                url: "https://example.com/packs/my-skill.zip".into()
            }
        );
    }

    #[test]
    fn parse_source_rejects_junk_and_path_injection() {
        assert!(parse_source("").is_err());
        assert!(parse_source("   ").is_err());
        assert!(parse_source("https://example.com/skills/foo").is_err());
        // owner/repo 段的路径注入
        assert!(parse_source("owner/../../etc").is_err());
        assert!(parse_source("own er/repo").is_err());
        assert!(parse_source("owner/repo#../evil").is_ok()); // # 后的技能名不进路径，定位阶段只作提示
    }

    // ── query 转义 ──

    #[test]
    fn encode_query_value_escapes_slash() {
        assert_eq!(encode_query_value("a/b"), "a%2Fb");
        assert_eq!(encode_query_value("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode_query_value("中文"), "%E4%B8%AD%E6%96%87");
    }

    #[test]
    fn skillhub_download_url_carries_slug() {
        assert_eq!(
            skillhub_download_url("my-skill"),
            "https://api.skillhub.cn/api/v1/download?slug=my-skill"
        );
        assert!(skillhub_download_url("owner/my-skill").contains("slug=owner%2Fmy-skill"));
    }

    // ── 定名 ──

    #[test]
    fn resolve_skill_name_prefers_valid_frontmatter_name() {
        assert_eq!(resolve_skill_name("my-skill", "fallback"), "my-skill");
    }

    /// SkillHub 上大量技能 frontmatter name 是中文——直接用会撞技能名白名单，
    /// 必须退回 slug，否则「装上了但清单里/路由里都是另一个名字」。
    #[test]
    fn resolve_skill_name_falls_back_when_frontmatter_name_is_chinese() {
        assert_eq!(
            resolve_skill_name("周报", "weekly-report-pro"),
            "weekly-report-pro"
        );
        assert_eq!(
            resolve_skill_name("周报/日报【自动匹配】", "weekly"),
            "weekly"
        );
    }

    #[test]
    fn sanitize_skill_name_keeps_ascii_only() {
        assert_eq!(sanitize_skill_name("my-skill_1"), "my-skill_1");
        assert_eq!(sanitize_skill_name("周报"), "skill");
        assert_eq!(sanitize_skill_name("a.b"), "a-b");
        assert_eq!(sanitize_skill_name(""), "skill");
    }

    // ── 技能包定位 ──

    fn write_skill_md(dir: &std::path::Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: d\n---\nbody\n"),
        )
        .unwrap();
    }

    #[test]
    fn locate_skill_finds_root_level_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill_md(tmp.path(), "root-skill");
        let got = locate_skill(tmp.path(), None).unwrap();
        assert_eq!(got, tmp.path());
    }

    #[test]
    fn locate_skill_finds_nested_skill_and_matches_hint() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill_md(&tmp.path().join("repo-main/skills/alpha"), "alpha");
        write_skill_md(&tmp.path().join("repo-main/skills/beta"), "beta");
        let got = locate_skill(tmp.path(), Some("beta")).unwrap();
        assert!(got.ends_with("skills/beta"), "got: {got:?}");
        // 无提示词时判为歧义并列出候选
        let err = locate_skill(tmp.path(), None).unwrap_err();
        assert!(err.contains("多个技能"), "{err}");
        assert!(err.contains("alpha") && err.contains("beta"), "{err}");
    }

    #[test]
    fn locate_skill_rejects_pack_without_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("readme.md"), "x").unwrap();
        let err = locate_skill(tmp.path(), None).unwrap_err();
        assert!(err.contains("没有 SKILL.md"), "{err}");
    }

    #[test]
    fn locate_skill_accepts_lowercase_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("pack");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("skill.md"), "---\nname: x\n---\n").unwrap();
        let got = locate_skill(&d, None).unwrap();
        assert_eq!(got, d);
    }

    // ── 落盘 ──

    #[test]
    fn install_skill_dir_copies_tree_and_renames_entry_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("references")).unwrap();
        std::fs::write(src.join("skill.md"), "---\nname: x\n---\n").unwrap();
        std::fs::write(src.join("references/a.md"), "a").unwrap();
        std::fs::write(src.join(".DS_Store"), "junk").unwrap();
        let root = tmp.path().join("skills");
        let (files, bytes) = install_skill_dir(&src, &root, "my-skill").unwrap();
        assert_eq!(files, 2, ".DS_Store 不该装进来");
        assert!(bytes > 0);
        let dest = root.join("my-skill");
        assert!(
            dest.join("SKILL.md").is_file(),
            "入口文件名要统一成 SKILL.md"
        );
        assert!(dest.join("references/a.md").is_file());
        assert!(!dest.join(".DS_Store").exists());
    }

    #[test]
    fn install_skill_dir_rejects_duplicate_name() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        write_skill_md(&src, "x");
        let root = tmp.path().join("skills");
        install_skill_dir(&src, &root, "dup").unwrap();
        let err = install_skill_dir(&src, &root, "dup").unwrap_err();
        assert!(err.contains("已存在"), "{err}");
    }

    /// 失败不留半份目录：留下半份 = 清单里能看到、use_skill 读不到的死技能。
    ///
    /// 造一个「复制到一半才失败」的场景：源里除入口文件外还有一个超单文件上限的
    /// 大文件——入口文件已经落盘，随后大文件被闸拦下。
    #[test]
    fn install_skill_dir_rolls_back_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        std::fs::write(src.join("huge.bin"), vec![0u8; 17 * 1024 * 1024]).unwrap();
        let root = tmp.path().join("skills");
        let err = install_skill_dir(&src, &root, "too-big").unwrap_err();
        assert!(err.contains("单文件上限"), "{err}");
        assert!(!root.join("too-big").exists(), "失败后不得留下半份目录");
    }

    // ── 描述截断 ──

    #[test]
    fn truncate_chars_counts_cjk_by_char() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("中文中文", 3), "中文…");
    }

    // ── 端到端（真实网络） ──

    /// 走一遍「解析来源 → 下载 → 解包 → 定位 → 落盘」，返回装好的文件数。
    async fn install_e2e(
        source_raw: &str,
        hint: Option<&str>,
        dest_root: &Path,
    ) -> Result<usize, String> {
        let source = parse_source(source_raw)?;
        let urls = resolve_download_urls(&source).await?;
        let tmp = TempWorkspace::new()?;
        let mut last_err = String::new();
        for u in &urls {
            let bytes = match fetch_bytes(u, MAX_DOWNLOAD_BYTES).await {
                Ok(b) => b,
                Err(e) => {
                    last_err = e;
                    continue;
                }
            };
            let zip = tmp.path().join("skill.zip");
            std::fs::write(&zip, &bytes).map_err(|e| e.to_string())?;
            let out = tmp.path().join("unpacked");
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let limits = crate::bot_archive::Limits::unpack();
            crate::bot_archive::unpack(&zip, &out, &limits, None)
                .map_err(|e| format!("解包失败：{e}"))?;
            let dir = locate_skill(&out, hint)?;
            let (files, _) = install_skill_dir(&dir, dest_root, "e2e-skill")?;
            return Ok(files);
        }
        Err(last_err)
    }

    /// SkillHub 端到端：公开 API → 302 → 对象存储 → zip → 装成技能。
    /// 网络不可达时只打印不失败（与 bot_web 的网络测试同口径）。
    #[test]
    fn e2e_install_from_skillhub() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        match tauri::async_runtime::block_on(install_e2e(
            "https://skillhub.cn/skills/kunlun-zhoubao",
            None,
            &root,
        )) {
            Ok(files) => {
                assert!(files > 0, "装出来的技能应有文件");
                let dest = root.join("e2e-skill");
                assert!(dest.join("SKILL.md").is_file(), "入口文件应就位");
            }
            Err(e) => eprintln!("skillhub e2e 未跑通（环境相关）: {e}"),
        }
    }

    /// GitHub 端到端：仓库 → 默认分支 → codeload zip → 定位技能 → 装成技能。
    /// 该仓库含多个技能，用 hint 指定其中之一。
    /// GitHub 在国内链路慢（十几秒），故默认忽略，按需手动跑：
    /// `cargo test --lib e2e_install_from_github -- --ignored`
    #[test]
    #[ignore]
    fn e2e_install_from_github() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("skills");
        match tauri::async_runtime::block_on(install_e2e(
            "https://github.com/Songhonglei/build-better-skills",
            Some("skill-hub-query"),
            &root,
        )) {
            Ok(files) => {
                assert!(files > 0, "装出来的技能应有文件");
                let dest = root.join("e2e-skill");
                assert!(dest.join("SKILL.md").is_file(), "入口文件应就位");
            }
            Err(e) => eprintln!("github e2e 未跑通（环境相关）: {e}"),
        }
    }
}
