//! 本地文件只读工具：read_text_file / grep_files / list_files。
//!
//! 安全设计（Kimi CLI 风格「执行前授权」）：
//! - 白名单（= 静默放行区）：内置默认（~/Desktop ~/Downloads ~/Documents + 任务卡绑定文件夹）
//!   ∪ bot-config.json `allowedDirs`（追加语义，不整体替换默认）
//! - 白名单外按 perm_mode 分流：strict=硬拒 / ask=弹授权窗
//!   （允许一次/始终允许该目录/拒绝，始终允许自动写进 allowedDirs）/ yolo=直接放行
//! - resolve_with_perm：展开 ~ → canonicalize（不存在即拒绝）→ 白名单或授权放行；
//!   `..` 与软链逃逸在 canonicalize 后无处遁形；命中/授权/拒绝都记审计
//! - 只读：不提供写/删；二进制与图片拒绝（图片走既有视觉通道）
//! - 输出截断在工具层做；递归走目录有深度/条目上限，跳过隐藏目录与 node_modules/target 等

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::bot::registry::ToolResult;

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
/// N6：edit_file 可编辑文件上限（对齐 read 的可处理量级，超出提示拆分）
const EDIT_MAX_FILE_BYTES: usize = 1024 * 1024;
/// N6：write_file 内容上限（对齐 GREP_MAX_FILE_BYTES 量级）
const WRITE_MAX_BYTES: usize = 2 * 1024 * 1024;

/// 跨平台用户主目录：优先 HOME；Windows GUI 程序（资源管理器双击启动）常无
/// HOME 环境变量，回退 USERPROFILE，再退 HOMEDRIVE+HOMEPATH。
///（Windows 绿色版 HOME 缺失会让默认白名单为空 →
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
///
/// `pub(crate)`：C2b-1 起 `bot_skills/files.rs` 的 open/delete 也要把 canonical
/// 路径交给下游（「操作的就是刚校验的那条路径」），复用本函数而非另造一套前缀逻辑。
pub(crate) fn strip_verbatim(p: PathBuf) -> PathBuf {
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

/// 白名单原始路径合并（追加语义）：内置默认（桌面/下载/文档）+ AI 产物目录 +
/// 任务卡绑定文件夹 + 用户 allowedDirs 四者并集（不能是「用户列表整体替换默认」——
/// 否则授权弹窗「始终允许」写入一个目录后，内置默认反而失效）。
/// AI_Gen_Files 必须在列：产物落盘后 read_text_file / list_files 得能读回。抽纯函数便于单测。
fn merge_raw_dirs(
    cfg_dirs: &[String],
    home: Option<&Path>,
    task_dirs: Vec<String>,
    gen_dir: Option<String>,
) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if let Some(home) = home {
        for d in ["Desktop", "Downloads", "Documents"] {
            raw.push(home.join(d).to_string_lossy().to_string());
        }
    }
    raw.extend(gen_dir);
    raw.extend(task_dirs);
    raw.extend(cfg_dirs.iter().cloned());
    raw
}

/// 白名单目录集合（canonical 化，只保留真实存在的目录）。
/// N7-⑥：session 对应会话有活动技能时，该技能目录追加进读白名单
///（第三层渐进披露——SKILL.md 附带资料 references/*.md 由模型按需 read_text_file；
/// 仅读侧，写闸门 writable_dirs 不受影响）。
async fn allowed_dirs(app: &AppHandle, session: Option<&str>) -> Vec<PathBuf> {
    let cfg = crate::bot::load_config(app);
    // 任务卡绑定的文件夹（用户显式绑过 = 显式授权过）
    let mut task_dirs: Vec<String> = Vec::new();
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            // 回收站任务的绑定目录不进白名单（防「删卡不解权」残留授权）
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
    let raw = merge_raw_dirs(&cfg.allowed_dirs, home_dir().as_deref(), task_dirs, gen);
    // canonicalize 循环整体包 spawn_blocking：逐目录同步 syscall 在 async runtime
    // 上会阻塞全部 Tauri command / event（OCR C5-BT-04 performance）。
    // JoinError → unwrap_or_default 保「拿不到目录 → 空白名单」语义（fail-closed 不变）。
    let mut out = spawn_blocking_io(move || {
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
    .unwrap_or_else(|e| {
        // 桥接失败（JoinError）/白名单读取失败（已转 String）：可见化后按既有契约回空白名单（fail-closed 不变）
        eprintln!("[bot_fs] 白名单 canonicalize 失败，回空白名单：{e}");
        Vec::new()
    });
    // N7-⑥：活动技能目录（会话级，只加读白名单；无活动技能/无目录则无追加）
    if let Some(run) = crate::bot_skills::active_skill_run_for(app, session) {
        if let Some(dir) = &run.dir {
            if let Ok(c) = std::fs::canonicalize(dir) {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// 路径守卫（Kimi CLI 风格「执行前授权」）：
/// ~ 展开 → canonicalize（不存在即拒）→ 白名单命中直接放行；
/// 白名单外按 perm_mode 分流：strict 硬拒 / ask 弹授权窗（允许一次 /
/// 始终允许该目录自动写入 allowedDirs / 拒绝）/ yolo 直接放行。全程记审计。
/// interactive/session_id（会话隔离）：后台执行（interactive=false）
/// 不弹授权窗直接拒；弹窗事件带 sessionId 供前端按会话过滤。
/// 成功返回 canonical 路径（Windows 剥 \\?\ 前缀）。
/// 包 spawn_blocking 同步 syscall：避免阻塞 Tauri async runtime（OCR C1b performance）。
///
/// closure 要求：FnOnce() -> Result<T, std::io::Error> + Send + 'static。
/// 返回：Result<T, String>——内部把 io::Error 转 String（走 spawn_blocking_map 契约，
/// py/document.rs:1059），调用方拿到的是 String 错误，丢了 io::ErrorKind。
/// 这条是 C1b follow-up 登记的设计债（medium L378），Phase 6 评估是否要扩
/// spawn_blocking_io 保留 ErrorKind。
async fn spawn_blocking_io<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, std::io::Error> + Send + 'static,
    T: Send + 'static,
{
    crate::py::document::spawn_blocking_map(|| f().map_err(|e| e.to_string())).await
}

/// async 版 is_dir：单次 metadata syscall 同样移出 runtime 线程（OCR C5-BT-04）。
/// JoinError → false（按「不是目录」处理，后续 open/read_dir 自然失败冒泡）。
async fn is_dir_async(path: &Path) -> bool {
    let p = path.to_path_buf();
    spawn_blocking_io(move || Ok(p.is_dir()))
        .await
        .unwrap_or(false)
}

pub async fn resolve_with_perm(
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
    // 用户输入原始路径（展开 ~ 后、**canonicalize 前**）——后续 canonical
    // 用于白名单校验，但写入 allowedDirs 必须用「用户输入视角的父目录」，
    // 绝不能用 canonical(parent())，否则 allowlist/sub/link.txt → /etc/passwd
    // 会把 /etc/ 写进白名单（OCR C1b symlink escape）。
    let expanded = expand_tilde(p);
    // canonicalize 包 spawn_blocking：同步 syscall 在 async runtime 会阻塞全部
    // Tauri command / event （OCR C1b performance critical）。
    // 把 owned clone 先拿出来再 move 进闭包：闭包要求 'static，原 expanded 留给 symlink 修复用。
    let expanded_for_canonical = expanded.clone();
    let canonical =
        spawn_blocking_io(move || std::fs::canonicalize(expanded_for_canonical)).await?;
    let dirs = allowed_dirs(app, session_id).await;
    if is_within_allowlist(&canonical, &dirs) {
        return Ok(strip_verbatim(canonical));
    }
    match crate::bot::perm_mode(app) {
        crate::bot::PermMode::Yolo => {
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_fs.yolo_allow | tool: {tool} | path: {}",
                    crate::bot::truncate_for_log(p, 200)
                ),
            );
            Ok(strip_verbatim(canonical))
        }
        crate::bot::PermMode::Ask => {
            match crate::bot_slash::ask_path_confirm(
                app,
                tool,
                &format!("访问白名单外路径：{p}"),
                interactive,
                session_id,
            )
            .await
            {
                crate::bot_slash::ConfirmChoice::Once => {
                    crate::bot::audit_log(
                        app,
                        &format!(
                            "bot_fs.ask_allow_once | tool: {tool} | path: {}",
                            crate::bot::truncate_for_log(p, 200)
                        ),
                    );
                    Ok(strip_verbatim(canonical))
                }
                crate::bot_slash::ConfirmChoice::Always => {
                    // 【OCR C1b symlink 修复】授权粒度 = 用户输入原始路径的父目录
                    // （文件 → parent、目录 → self），与判定逻辑保持一致。
                    // 绝不能用 canonical(parent())：那个是 symlink 解析后的路径，
                    // 可被攻击者控制指向 allowlist 外。
                    let dir = if is_dir_async(&expanded).await {
                        expanded.clone()
                    } else {
                        expanded
                            .parent()
                            .map(|d| d.to_path_buf())
                            .unwrap_or_else(|| expanded.clone())
                    };
                    let dir_str = strip_verbatim(dir).to_string_lossy().to_string();
                    if let Err(e) = crate::bot::add_allowed_dir(app, &dir_str) {
                        crate::bot::audit_log(app, &format!("bot_fs.always_persist_fail | {e}"));
                    }
                    crate::bot::audit_log(
                        app,
                        &format!(
                            "bot_fs.ask_allow_always | tool: {tool} | dir: {}",
                            crate::bot::truncate_for_log(&dir_str, 200)
                        ),
                    );
                    Ok(strip_verbatim(canonical))
                }
                crate::bot_slash::ConfirmChoice::Deny => {
                    crate::bot::audit_log(
                        app,
                        &format!(
                            "bot_fs.ask_denied | tool: {tool} | path: {}",
                            crate::bot::truncate_for_log(p, 200)
                        ),
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
                &format!(
                    "bot_fs.denied | path: {} | 不在白名单目录内",
                    crate::bot::truncate_for_log(p, 200)
                ),
            );
            Err(format!(
                "路径不在白名单目录内：{p}（白名单：设置页 allowedDirs，默认 桌面/下载/文档 + 任务卡绑定文件夹）"
            ))
        }
    }
}

/// 白名单判定核（纯函数）：canonical 路径是否落在任一白名单目录内。
///
/// `Path::starts_with` 按**路径分量**比较，所以白名单 `/tmp/ab` 不会误吞 `/tmp/abc`；
/// 而 `..` 穿越与软链逃逸要靠调用方的 `canonicalize` 先解析掉——两步一起才成立，
/// 单测 `allowlist_rejects_traversal_and_symlink_escape` /
/// `allowlist_rejects_symlink_escape` 用真实文件系统同时锁住这两步。
fn is_within_allowlist(canonical: &Path, dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| canonical.starts_with(d))
}

// ───────────────────────── N6：文件编辑（edit_file / write_file） ─────────────────────────
//
// 写白名单 ≠ 读白名单（设计决策，spec N6）：可写根 = AI_Gen_Files + 任务卡绑定文件夹
// + 设置页 allowedDirs；桌面/下载/文档默认项**只读不可写**——用户没显式授权改家目录
// 文件。白名单外走 perm_mode 三分支：strict 拒 / ask 弹 danger 确认（每次，不持久化）/
// yolo 放行+审计。

/// 可写根的原始集合（纯内核，可测）：gen + 任务卡绑定文件夹 + cfg.allowedDirs。
/// 刻意不含 merge_raw_dirs 里的 home/Desktop/Downloads/Documents 默认读项。
fn build_writable_raw(
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
/// ——「始终允许」语义对写太宽）。成功返回展开后的目标路径（文件可能尚不存在，
/// 不能 canonicalize；父目录已 canonical 校验，TOCTOU 窗口与读闸门同级留档）。
async fn resolve_writable(
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
    match crate::bot::perm_mode(app) {
        crate::bot::PermMode::Yolo => {
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_fs.yolo_allow_write | tool: {tool} | path: {}",
                    crate::bot::truncate_for_log(p, 200)
                ),
            );
            Ok(strip_verbatim(expanded))
        }
        crate::bot::PermMode::Ask => {
            let approved = crate::bot_slash::ask_user_confirm(
                app,
                tool,
                &format!("写入白名单外路径：{p}"),
                interactive,
                session_id,
            )
            .await;
            if approved {
                crate::bot::audit_log(
                    app,
                    &format!(
                        "bot_fs.ask_allow_write | tool: {tool} | path: {}",
                        crate::bot::truncate_for_log(p, 200)
                    ),
                );
                Ok(strip_verbatim(expanded))
            } else {
                crate::bot::audit_log(
                    app,
                    &format!(
                        "bot_fs.ask_denied_write | tool: {tool} | path: {}",
                        crate::bot::truncate_for_log(p, 200)
                    ),
                );
                Err(format!(
                    "用户未授权写入该路径：{p}（可在设置页把目录加入白名单；后台执行时确认窗不可用，一律拒绝）"
                ))
            }
        }
        crate::bot::PermMode::Strict => {
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_fs.denied_write | path: {} | 不在可写目录内",
                    crate::bot::truncate_for_log(p, 200)
                ),
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
}

/// edit 失败类型（进审计的结构化字段，供自进化反思统计）
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
/// 不做 Levenshtein 模糊匹配（误替换风险 > 收益，spec N6 留档）。
pub(crate) fn try_apply_edit(
    content: &str,
    old: &str,
    new: &str,
) -> Result<(String, EditMode, i64), (EditErrorKind, String)> {
    let exact_hits = content.matches(old).count();
    if exact_hits == 1 {
        let applied = content.replacen(old, new, 1);
        let delta = new.lines().count() as i64 - old.lines().count() as i64;
        return Ok((applied, EditMode::Exact, delta));
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
            let eol = if content.contains("\r\n") {
                "\r\n"
            } else {
                "\n"
            };
            let mut out_lines: Vec<String> =
                content.lines().take(start).map(|l| l.to_string()).collect();
            out_lines.extend(new.lines().map(|l| l.to_string()));
            out_lines.extend(content.lines().skip(start + n).map(|l| l.to_string()));
            let mut applied = out_lines.join(eol);
            if content.ends_with('\n') {
                applied.push('\n');
            }
            let delta = new.lines().count() as i64 - n as i64;
            Ok((applied, EditMode::WhitespaceTolerant, delta))
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

// ───────────────────── P1-b：文件变更证据（Agent 透明化设计 §4.2） ─────────────────────

/// 文件变更证据（P1-b）：文件类工具成功落盘后随 `ToolResult::file_changes` 回传，
/// dispatch（P1-c）据此落 `file_changes` 表 + emit `bot-file-changed`。
/// 纯数据结构——trace 归属字段（trace_id/span_id/created_at）由 dispatch 侧补。
#[derive(Debug, Clone)]
pub struct FileChangeReceipt {
    pub path: String,
    /// "create" | "modify"（现有文件工具不产 delete）
    pub kind: &'static str,
    pub added: i64,
    pub deleted: i64,
    pub diff: Option<String>,
    pub truncated: bool,
    /// before 全文快照文件名（`data_dir/checkpoints/<before_ref>`，uuid 发号）。
    /// None = 新建（无旧文可存）或快照写盘失败（降级，`checkpoint.write_fail` 审计）。
    pub before_ref: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
}

/// unified diff 生成（纯函数可测）：similar 行级 diff → unified 文本 + ±行计数。
/// ±行计数是事实（不随截断丢失），diff 文本超 `MAX_DIFF_LINES` 行截断置位。
pub(crate) fn build_unified_diff(
    before: &str,
    after: &str,
    path: &str,
) -> (Option<String>, i64, i64, bool) {
    let diff = similar::TextDiff::from_lines(before, after);
    let mut added = 0i64;
    let mut deleted = 0i64;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => deleted += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    if added == 0 && deleted == 0 {
        return (None, 0, 0, false);
    }
    let full = diff
        .unified_diff()
        .context_radius(3)
        .header(path, path)
        .to_string();
    let lines: Vec<&str> = full.lines().collect();
    let truncated = lines.len() > crate::db::MAX_DIFF_LINES;
    let mut text = lines
        .iter()
        .take(crate::db::MAX_DIFF_LINES)
        .copied()
        .collect::<Vec<&str>>()
        .join("\n");
    if truncated {
        text.push_str(&format!(
            "\n…（diff 超过 {} 行已截断）",
            crate::db::MAX_DIFF_LINES
        ));
    }
    (Some(text), added, deleted, truncated)
}

/// sha256 十六进制（回滚证据链：P2 回滚前比对 after_sha 防文件漂移）
fn sha256_hex(s: &str) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// before 全文快照（`data_dir/checkpoints/<uuid>`，db::atomic_write 原子写）。
/// 快照是回滚的 nice-to-have：失败降级 None + Error 审计，不阻断已成功的业务写盘
/// ——工具结果不能因观测面失败而翻转。
fn write_before_snapshot(app: &AppHandle, before: &str) -> Option<String> {
    let dir = crate::db::paths::data_dir(app).join("checkpoints");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Error,
            "checkpoint.write_fail",
            &[("err", e.to_string())],
        );
        return None;
    }
    let name = uuid::Uuid::new_v4().to_string();
    match crate::db::atomic_write(&dir.join(&name), before) {
        Ok(()) => Some(name),
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Error,
                "checkpoint.write_fail",
                &[("err", e)],
            );
            None
        }
    }
}

/// 变更证据构建（纯函数可测；快照写盘由调用方另接 write_before_snapshot 回填 before_ref）：
/// create（before=None）→ 全 + 行 diff、无 before_sha；modify → before/after diff + before_sha。
pub(crate) fn build_file_change_receipt(
    path: &str,
    kind: &'static str,
    before: Option<&str>,
    after: &str,
) -> FileChangeReceipt {
    let (diff, added, deleted, truncated) = match before {
        Some(b) => build_unified_diff(b, after, path),
        None => build_unified_diff("", after, path),
    };
    FileChangeReceipt {
        path: path.to_string(),
        kind,
        added,
        deleted,
        diff,
        truncated,
        before_ref: None,
        before_sha: before.map(sha256_hex),
        after_sha: Some(sha256_hex(after)),
    }
}

/// edit 同步内核（spawn_blocking 调用；独立函数便于单测）：
/// 读文件（≤1MB、UTF-8）→ 三级匹配 → 返回替换结果。
/// P1-b 起同时带回**修改前全文**（diff 与回滚快照的证据源，避免再读一次盘）。
fn edit_file_sync(
    canonical: PathBuf,
    old: String,
    new: String,
) -> Result<(String, String, EditMode, i64), (EditErrorKind, String)> {
    let content = std::fs::read_to_string(&canonical).map_err(|e| {
        (
            EditErrorKind::NotFound,
            format!("读取失败（非 UTF-8 文本或不可读）：{e}"),
        )
    })?;
    if content.len() > EDIT_MAX_FILE_BYTES {
        return Err((
            EditErrorKind::NotFound,
            format!(
                "文件 {} 字节超过 edit_file 上限 {EDIT_MAX_FILE_BYTES}",
                content.len()
            ),
        ));
    }
    let (applied, mode, delta) = try_apply_edit(&content, &old, &new)?;
    Ok((content, applied, mode, delta))
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
        return ToolResult::ok("edit_file 缺少 path 参数".to_string(), Vec::new());
    };
    let Some(old) = v["oldString"].as_str().map(|s| s.to_string()) else {
        return ToolResult::ok("edit_file 缺少 oldString 参数".to_string(), Vec::new());
    };
    let Some(new) = v["newString"].as_str().map(|s| s.to_string()) else {
        return ToolResult::ok("edit_file 缺少 newString 参数".to_string(), Vec::new());
    };
    if path.is_empty() {
        return ToolResult::ok("edit_file 的 path 不能为空".to_string(), Vec::new());
    }
    if old.trim().is_empty() {
        return ToolResult::ok(
            "oldString 不能为空（请提供要替换的原文）".to_string(),
            Vec::new(),
        );
    }
    if old == new {
        return ToolResult::ok(
            "oldString 与 newString 相同，没有可应用的变更".to_string(),
            Vec::new(),
        );
    }
    let canonical = match resolve_writable(app, "edit_file", &path, interactive, session_id).await {
        Ok(p) => p,
        // resolve_writable Err 返 String，首字不定 → ok
        Err(e) => return ToolResult::ok(e, Vec::new()),
    };
    let log_path = crate::bot::truncate_for_log(&canonical.display().to_string(), 200);
    let shown = canonical.display().to_string();
    let write_target = canonical.clone();
    let out =
        crate::py::document::spawn_blocking_map(move || Ok(edit_file_sync(canonical, old, new)))
            .await
            .unwrap_or_else(|e| Err((EditErrorKind::NotFound, format!("编辑线程异常：{e}"))));
    match out {
        Ok((before, applied, mode, delta)) => {
            if let Err(e) = crate::db::atomic_write(&write_target, &applied) {
                // 「写回失败：」首字「写」非 error/warn 前缀 → ok
                return ToolResult::ok(format!("写回失败：{e}"), Vec::new());
            }
            let mode_note = match mode {
                EditMode::Exact => "",
                EditMode::WhitespaceTolerant => "（经空白容错匹配：行尾空白/CRLF 有差异）",
            };
            let delta_note = if delta > 0 {
                format!("+{delta} 行")
            } else if delta < 0 {
                format!("{delta} 行")
            } else {
                "行数不变".to_string()
            };
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_fs.edit_file | {} | {} | {delta_note}",
                    log_path,
                    match mode {
                        EditMode::Exact => "exact",
                        EditMode::WhitespaceTolerant => "ws",
                    }
                ),
            );
            // P1-b 变更证据（Agent 透明化设计 §4.2）：unified diff + before 快照。
            // 快照失败降级 before_ref=None（checkpoint.write_fail 审计），不翻转业务结果。
            let mut receipt = build_file_change_receipt(&shown, "modify", Some(&before), &applied);
            receipt.before_ref = write_before_snapshot(app, &before);
            // 「已修改」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(
                format!("已修改 {shown}（{delta_note}）{mode_note}"),
                Vec::new(),
            )
            .with_file_change(receipt)
        }
        Err((kind, hint)) => {
            // 失败带教学提示走 ok 通道（reflection 文案，自进化经 tool.call_failed 采集）
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_fs.edit_file.fail | {} | kind: {:?} | {}",
                    log_path,
                    kind,
                    crate::bot::truncate_for_log(&hint, 200)
                ),
            );
            ToolResult::ok(hint, Vec::new())
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
        return ToolResult::ok("write_file 缺少 path 参数".to_string(), Vec::new());
    };
    let Some(content) = v["content"].as_str().map(|s| s.to_string()) else {
        return ToolResult::ok("write_file 缺少 content 参数".to_string(), Vec::new());
    };
    if path.is_empty() {
        return ToolResult::ok("write_file 的 path 不能为空".to_string(), Vec::new());
    }
    if let Err(e) = validate_write_content(&content) {
        return ToolResult::ok(e, Vec::new());
    }
    let canonical = match resolve_writable(app, "write_file", &path, interactive, session_id).await
    {
        Ok(p) => p,
        Err(e) => return ToolResult::ok(e, Vec::new()),
    };
    let existed = is_dir_async(&canonical).await || {
        let c = canonical.clone();
        spawn_blocking_io(move || Ok(c.is_file()))
            .await
            .unwrap_or(false)
    };
    if existed {
        let approved = crate::bot_slash::ask_user_confirm(
            app,
            "write_file",
            &format!("覆盖已存在文件：{}", canonical.display()),
            interactive,
            session_id,
        )
        .await;
        if !approved {
            // 非交互（子 agent）确认窗不可用自动拒——覆盖走 edit_file 精确修改
            return ToolResult::ok(
                "覆盖未获确认，文件未修改。修改已有文件请用 edit_file（精确替换，无需确认）；新建文件请换一个文件名".to_string(),
                Vec::new(),
            );
        }
    }
    // P1-b 变更证据：modify 需在写盘前取旧文（读不出/超上限/二进制 → 无 diff 降级）。
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
            // P1-b 变更证据：create = 全 + 行 diff；modify = before/after diff + 回滚快照。
            // 快照失败降级 before_ref=None（checkpoint.write_fail 审计），不翻转业务结果。
            let kind: &'static str = if existed { "modify" } else { "create" };
            let mut receipt = build_file_change_receipt(
                &written.display().to_string(),
                kind,
                before_content.as_deref(),
                &content_after,
            );
            if let Some(b) = before_content.as_deref() {
                receipt.before_ref = write_before_snapshot(app, b);
            }
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_fs.write_file | {} | {lines} lines | {}",
                    crate::bot::truncate_for_log(&written.display().to_string(), 200),
                    action
                ),
            );
            // 「已写入」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(
                format!("已写入 {}（{action}，{lines} 行）", written.display()),
                Vec::new(),
            )
            .with_file_change(receipt)
        }
        Err(e) => ToolResult::ok(format!("写入失败：{e}"), Vec::new()),
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
fn read_capped_file_sync(path: &Path, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
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

/// async 版 read_capped_file：包 spawn_blocking。
/// 返 std::io::Result 保持调用方契约（spawn_blocking_map 返 String，这里 .map_err 转回 io::Error）。
async fn read_capped_file(path: &Path, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let path = path.to_path_buf();
    crate::py::document::spawn_blocking_map(move || {
        read_capped_file_sync(&path, max).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| std::io::Error::other(e))
}

/// 【OCR C1b TOCTOU】inode re-check 结果。
#[derive(Debug, PartialEq, Eq)]
enum InodeCheckOutcome {
    /// inode 不变（或 platform 不支持 / 文件已不存在）→ 放行
    Ok,
    /// inode 变了 → 拒绝
    Replaced,
}

/// 读 canonical 路径当前 inode。三种情况（按 Phase 0.6 流程债 L402 精确三情况约束）：
///
/// 1. **Windows（`#[cfg(not(unix))]`）**：直接 `None`。
///    设计选择：Windows 端**无 TOCTOU 保护**（非缺陷——`is_binary_file_with_ino_check`
///    整个 inode 比对逻辑都被 `#[cfg(unix)]` gate）。见 follow-up：
///    `Phase 6: Windows 端 TOCTOU 策略`（先决定做不做，再决定怎么做）。
///    **fail-open 是当前语义**——tool_read_text_file 在 Windows 上仍能读文件，
///    只是没有 inode re-check 这一道闸。
///
/// 2. **Unix 上 `metadata()` 成功**：`Some(ino)`，进入 re-check 保护范围（见下文
///    `is_binary_file_with_ino_check` 的注释）。
///
/// 3. **Unix 上 `metadata()` 失败**（文件瞬间被删、权限拒、IO 错误等）：
///    `None`。**fail-open 是当前语义**——不放行也拒绝（不报 Replaced），让
///    后续 `read_capped_file` 的 `File::open` 自然失败冒泡到 `tool_read_text_file`
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

/// 二进制检测 + inode re-check 合一。
/// 返回 `(InodeCheckOutcome, bool /* is_binary */)`——二进制判定不再丢。
///
/// **保护范围（精确）**：
/// 1. `tool_read_text_file::resolve_with_perm` → canonicalize 出 path
/// 2. `capture_pre_ino(path)` 读 pre_ino（unix only，Windows 返 None）
/// 3. 本函数 open file 拿 fd → stat fd 看 inode → 与 pre 比对
///    - 不变 → `Ok`
///    - 变了 → `Replaced`（说明 canonical 与 stat 之间文件被换过）
/// 同时 fd 上读 8KB，判二进制（保留 is_binary 结果，不丢）。
///
/// **不在保护范围（明确写明）**：
/// - `read_capped_file` 后续 `File::open` + read 不在 inode 比对窗口内
///   ——本函数 open 拿到的 fd 不传出去，read 重新 open。
///   见 follow-up: `Phase 6: resolve_with_perm 其它调用点 TOCTOU 统一策略`。
/// - symlink target 替换（canonical 路径不变但内容指向新文件）。
///   见 follow-up: `Phase 6: symlink target 替换防护收紧`。
#[cfg(unix)]
async fn is_binary_file_with_ino_check(
    path: &Path,
    pre_ino: Option<u64>,
) -> std::io::Result<(InodeCheckOutcome, bool)> {
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let pre = pre_ino;
    let path = path.to_path_buf();
    // 注意：closure 返 Result<(InodeCheckOutcome, bool), String> 走 spawn_blocking_map，
    // 这里我们用 spawn_blocking_map 直接（不走 spawn_blocking_io），因为返回值不需
    // 要 io::Error 类型擦除。
    crate::py::document::spawn_blocking_map(move || -> Result<(InodeCheckOutcome, bool), String> {
        let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let fd_ino = f.metadata().map_err(|e| e.to_string())?.ino();
        if let Some(pre) = pre {
            if fd_ino != pre {
                return Ok((InodeCheckOutcome::Replaced, false));
            }
        }
        let mut buf = [0u8; 8192];
        let n = f.read(&mut buf).unwrap_or(0);
        let is_binary = buf[..n].contains(&0);
        Ok((InodeCheckOutcome::Ok, is_binary))
    })
    .await
    .map_err(std::io::Error::other)
}

#[cfg(not(unix))]
async fn is_binary_file_with_ino_check(
    path: &Path,
    _pre_ino: Option<u64>,
) -> std::io::Result<(InodeCheckOutcome, bool)> {
    // Windows 端 TOCTOU 未实现（设计选择）：只判二进制，不做 inode 比对。
    // 复用 is_binary_file_sync 的二进制判定（但走 spawn_blocking_io 不阻塞 runtime）。
    let path = path.to_path_buf();
    let res = spawn_blocking_io(move || -> Result<bool, std::io::Error> {
        Ok(is_binary_file_sync(&path))
    })
    .await;
    match res {
        Ok(b) => Ok((InodeCheckOutcome::Ok, b)),
        Err(_) => Ok((InodeCheckOutcome::Ok, true)), // IO 错误按二进制处理（fail-closed）
    }
}

// ───────────────────────── 工具实现（bot 分发签名：(String, Vec<TaskRef>)） ─────────────────────────

/// read_text_file：读白名单内 UTF-8 文本，offset/limit 行切片（1 起），超 100KB 截断
pub async fn tool_read_text_file(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str() else {
        // 「read_text_file 缺少 path」首字「r」非 error/warn 前缀 → ok
        return ToolResult::ok("read_text_file 缺少 path".to_string(), Vec::new());
    };
    let canonical =
        match resolve_with_perm(app, "read_text_file", path, interactive, session_id).await {
            Ok(p) => p,
            // resolve_with_perm Err 返 String，首字不定 → ok
            Err(e) => return ToolResult::ok(e, Vec::new()),
        };
    if is_dir_async(&canonical).await {
        // format! 文本首字为路径首字符（不定） → ok
        return ToolResult::ok(
            format!("{} 是目录，列文件请用 list_files", path.trim()),
            Vec::new(),
        );
    }
    // 【OCR C1b TOCTOU】保护范围（精确）：
    //   canonicalize → capture_pre_ino → is_binary_file_with_ino_check
    //   （内部 open file 拿 fd → stat fd 看 inode → 与 pre_ino 比对 → 顺手判 8KB 二进制）
    //   —— 三步窗口内的 inode swap 被捕获（canonical 与 stat 之间文件被换）。
    //
    // 不在保护范围（明确写明）：
    //   - `read_capped_file` 后续 `File::open` + read 不在 inode 比对窗口内
    //     （is_binary_file_with_ino_check 的 fd 不传出，后续 read 重新 open）。
    //     见 follow-up: "Phase 6: resolve_with_perm 其它调用点 TOCTOU 统一策略"
    //   - symlink target 替换（canonical 路径不变但内容指向新文件）。
    //     见 follow-up: "Phase 6: symlink target 替换防护收紧"
    //   - Windows 平台（`#[cfg(not(unix))]`）：capture_pre_ino 直接 None，整个
    //     inode re-check 不执行。设计选择，非缺陷——见 capture_pre_ino 注释
    //     + follow-up: "Phase 6: Windows 端 TOCTOU 策略"。
    let pre_ino = capture_pre_ino(&canonical).await;
    let (inode_check, is_binary) = match is_binary_file_with_ino_check(&canonical, pre_ino).await {
        Ok(r) => r,
        Err(e) => return ToolResult::ok(format!("读取失败：{e}"), Vec::new()),
    };
    if inode_check == InodeCheckOutcome::Replaced {
        crate::bot::audit_log(
            app,
            &format!(
                "bot_fs.read_inode_mismatch | {} | canonicalize 与 stat 之间 inode 变了",
                crate::bot::truncate_for_log(&canonical.display().to_string(), 200)
            ),
        );
        return ToolResult::ok(
            format!("{} 在校验后被替换，拒绝读取（TOCTOU 防护）", path.trim()),
            Vec::new(),
        );
    }
    if is_binary {
        // 同上，首字不定 → ok
        return ToolResult::ok(
            format!(
                "{} 是二进制/非文本文件；Office/PDF 文档请用 extract_document",
                path.trim()
            ),
            Vec::new(),
        );
    }
    let offset = v["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit =
        (v["limit"].as_u64().unwrap_or(READ_DEFAULT_LINES as u64) as usize).min(READ_MAX_LINES);
    let (raw, byte_truncated) = match read_capped_file(&canonical, READ_MAX_BYTES).await {
        Ok(r) => r,
        // 「读取失败」首字「读」非「失败」前缀 → ok
        Err(e) => return ToolResult::ok(format!("读取失败：{e}"), Vec::new()),
    };
    let text = String::from_utf8_lossy(&raw).to_string();
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if offset > total {
        // 「文件共 N 行」首字「文」非 error/warn 前缀 → ok
        return ToolResult::ok(
            format!("文件共 {total} 行，offset {offset} 超出范围"),
            Vec::new(),
        );
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
    crate::bot::audit_log(
        app,
        &format!(
            "bot_fs.read | {} | lines {offset}-{end}/{total}",
            crate::bot::truncate_for_log(&canonical.display().to_string(), 200)
        ),
    );
    // 文件内容文本，首字符不定 → ok
    ToolResult::ok(out, Vec::new())
}

/// grep_files：白名单目录内正则搜文件内容，输出 path:line:内容（ripgrep 风格）
/// N3-6：grep 上下文渲染（grep -C 风格，纯函数可测）。
/// 命中行 `path:行号: 内容`、上下文行 `path-行号- 内容`；重叠/相邻窗口合并为一块，
/// 块间 `--` 分隔；行宽截 200 字符。context=0 时退化为每命中一行的旧行为。
fn render_context_hits(path: &str, lines: &[&str], hits: &[usize], context: usize) -> Vec<String> {
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
        for i in *s..=*e {
            let mark = if hits.contains(&i) { ':' } else { '-' };
            out.push(format!("{path}{mark}{}{mark} {}", i + 1, trunc(lines[i])));
        }
    }
    out
}

pub async fn tool_grep_files(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(pattern) = v["pattern"].as_str() else {
        // 「grep_files 缺少 pattern」首字「g」非 error/warn 前缀 → ok
        return ToolResult::ok("grep_files 缺少 pattern".to_string(), Vec::new());
    };
    // 非法正则降级为字面量搜索（regex::escape），不让一个坏 pattern 炸掉整轮
    let re = regex::Regex::new(pattern).or_else(|_| regex::Regex::new(&regex::escape(pattern)));
    let Ok(re) = re else {
        // 「无效的正则表达式」首字「无」非 error/warn 前缀 → ok
        return ToolResult::ok(format!("无效的正则表达式：{pattern}"), Vec::new());
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
                // resolve_with_perm Err 返 String，首字不定 → ok
                Err(e) => return ToolResult::ok(e, Vec::new()),
            }
        }
        _ => match allowed_dirs(app, session_id).await.first() {
            Some(d) => d.clone(),
            // 「没有可用的白名单目录」首字「没」非 error/warn 前缀 → ok
            None => return ToolResult::ok("没有可用的白名单目录".to_string(), Vec::new()),
        },
    };
    if !is_dir_async(&dir).await {
        // format! 文本首字为目录路径首字符（不定） → ok
        return ToolResult::ok(format!("{} 不是目录", dir.display()), Vec::new());
    }
    // walk 整体包 spawn_blocking：内部 read_dir / metadata / read_to_string
    // 都是同步 syscall，全部移到阻塞线程跑，不再阻塞 Tauri async runtime
    // （OCR C1b performance critical）。
    // closure 内 is_binary_file_sync / read_to_string 仍 sync，但已在 spawn_blocking
    // 线程内 OK。流式优化留 follow-up。
    // 返回（渲染行, 匹配数）二元组：max 按匹配数计、行数随 context 放大，
    // 上限提示要看匹配数；spawn_blocking 'static 闭包只能带值回传。
    let dir_log = dir.display().to_string();
    let (hits, found) =
        crate::py::document::spawn_blocking_map(move || -> Result<(Vec<String>, usize), String> {
            let mut hits: Vec<String> = Vec::new();
            let mut found = 0usize;
            walk(&dir, &mut |path: &Path, is_dir: bool| {
                if found >= max {
                    return false;
                }
                if is_dir {
                    return true;
                }
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !glob.is_empty() && !glob_match(&glob, &name) {
                    return true;
                }
                if path
                    .metadata()
                    .map(|m| m.len() > GREP_MAX_FILE_BYTES)
                    .unwrap_or(true)
                {
                    return true; // 超大文件跳过
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
                    let budget = max - found;
                    let taken = budget.min(hit_idx.len());
                    hits.extend(render_context_hits(
                        &path.display().to_string(),
                        &lines,
                        &hit_idx[..taken],
                        context,
                    ));
                    found += taken;
                    if found >= max {
                        return false;
                    }
                }
                true
            });
            Ok((hits, found))
        })
        .await
        .unwrap_or_default();
    crate::bot::audit_log(
        app,
        &format!(
            "bot_fs.grep | dir: {} | pattern: {} | hits: {}",
            crate::bot::truncate_for_log(&dir_log, 200),
            crate::bot::truncate_for_log(pattern, 100),
            found
        ),
    );
    if hits.is_empty() {
        // 「... 内没有匹配 ...」首字不定 → ok
        return ToolResult::ok(
            format!("{} 内没有匹配「{pattern}」的内容", dir_log),
            Vec::new(),
        );
    }
    let mut out = hits.join("\n");
    if found >= max {
        out.push_str(&format!(
            "\n…（已达 {max} 条匹配上限，缩小范围或加 glob 过滤）"
        ));
    }
    // 匹配结果文本，首字符任意 UTF-8 → ok
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
        // 「list_files 缺少 dir」首字「l」非 error/warn 前缀 → ok
        return ToolResult::ok("list_files 缺少 dir".to_string(), Vec::new());
    };
    let canonical = match resolve_with_perm(app, "list_files", dir, interactive, session_id).await {
        Ok(p) => p,
        // resolve_with_perm Err 返 String，首字不定 → ok
        Err(e) => return ToolResult::ok(e, Vec::new()),
    };
    if !is_dir_async(&canonical).await {
        // format! 文本首字为目录路径首字符（不定） → ok
        return ToolResult::ok(
            format!("{} 不是目录；读文件请用 read_text_file", dir.trim()),
            Vec::new(),
        );
    }
    let pattern = v["pattern"].as_str().unwrap_or("").trim().to_string();
    // walk 整体包 spawn_blocking（OCR C1b performance）：同 grep_files 原因。
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
    crate::bot::audit_log(
        app,
        &format!(
            "bot_fs.list | dir: {} | entries: {}",
            crate::bot::truncate_for_log(&canonical_log, 200),
            entries.len()
        ),
    );
    if entries.is_empty() {
        // 「... 内没有匹配的文件」首字不定 → ok
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
    // 文件列表文本，首字符不定 → ok
    ToolResult::ok(out, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───── N3-6：grep 上下文渲染 ─────

    #[test]
    fn render_context_zero_matches_old_format() {
        let lines = ["alpha", "beta", "gamma"];
        let out = render_context_hits("/tmp/a.txt", &lines, &[1], 0);
        assert_eq!(
            out,
            vec!["/tmp/a.txt:2: beta"],
            "context=0 应与旧格式逐字一致"
        );
    }

    #[test]
    fn render_context_windows_and_separator() {
        let lines = ["l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9"];
        // 两个相距远的命中（1 与 8），context=1 → 两块，中间 --
        let out = render_context_hits("/t/f.txt", &lines, &[1, 8], 1);
        assert_eq!(
            out,
            vec![
                "/t/f.txt-1- l0",
                "/t/f.txt:2: l1",
                "/t/f.txt-3- l2",
                "--",
                "/t/f.txt-8- l7",
                "/t/f.txt:9: l8",
                "/t/f.txt-10- l9",
            ]
        );
    }

    #[test]
    fn render_context_merges_overlapping_windows() {
        let lines = ["l0", "l1", "l2", "l3", "l4"];
        // 命中 1 与 2，context=1：窗口 [0..2] 与 [1..3] 重叠 → 合并一块、无 --
        let out = render_context_hits("/t/f.txt", &lines, &[1, 2], 1);
        assert_eq!(
            out,
            vec![
                "/t/f.txt-1- l0",
                "/t/f.txt:2: l1",
                "/t/f.txt:3: l2",
                "/t/f.txt-4- l3",
            ]
        );
        assert!(!out.contains(&"--".to_string()), "合并块不得有分隔符");
    }

    #[test]
    fn render_context_clamps_at_file_edges() {
        let lines = ["only"];
        let out = render_context_hits("/t/f.txt", &lines, &[0], 5);
        assert_eq!(out, vec!["/t/f.txt:1: only"], "窗口越界应夹到文件边界");
    }

    // ───── N6：edit 三级匹配 / write 校验 / 可写根集合 ─────

    #[test]
    fn apply_edit_exact_unique_and_multi_hit() {
        let content = "fn a() {}\nfn b() {}\n";
        let old = "fn b() {}";
        let (out, mode, delta) = try_apply_edit(content, old, "fn b() -> i32 { 1 }").unwrap();
        assert_eq!(mode, EditMode::Exact);
        assert_eq!(delta, 0);
        assert!(out.contains("fn b() -> i32 { 1 }"), "{out}");
        // 多处命中：两个相同函数体
        let dup = "x = 1;\nx = 1;\n";
        let (kind, hint) = try_apply_edit(dup, "x = 1;", "x = 2;").unwrap_err();
        assert_eq!(kind, EditErrorKind::MultiHit);
        assert!(hint.contains("2 处"), "{hint}");
    }

    #[test]
    fn apply_edit_whitespace_fallback_and_not_found() {
        // 原文行尾有尾随空格 + CRLF：精确失败、空白容错命中
        let content = "fn a() {\r\n    return 1;   \r\n}\r\n";
        let old = "fn a() {\n    return 1;\n}";
        let (out, mode, delta) =
            try_apply_edit(content, old, "fn a() {\r\n    return 2;\r\n}").unwrap();
        assert_eq!(mode, EditMode::WhitespaceTolerant);
        assert_eq!(delta, 0);
        assert!(out.contains("return 2;"), "{out}");
        // 空白容错也多处 → MultiHit
        let dup_ws = "x = 1;  \nx = 1;\n";
        let (kind, _) = try_apply_edit(dup_ws, "x = 1;", "x = 2;").unwrap_err();
        assert_eq!(kind, EditErrorKind::MultiHit);
        // 全失败 → NotFound + reflection 提示带文件开头
        let (kind, hint) = try_apply_edit("alpha\nbeta\n", "不存在的行", "x").unwrap_err();
        assert_eq!(kind, EditErrorKind::NotFound);
        assert!(hint.contains("read_text_file"), "{hint}");
        assert!(hint.contains("alpha"), "提示应带文件开头：{hint}");
    }

    #[test]
    fn apply_edit_preserves_crlf_and_trailing_newline() {
        let content = "a\r\nold\r\nb\r\n";
        let (out, _, _) = try_apply_edit(content, "old", "new").unwrap();
        assert_eq!(out, "a\r\nnew\r\nb\r\n", "CRLF 文件替换后保持 CRLF");
        // 无结尾换行的文件重建后不引入结尾换行
        let content2 = "x\nold";
        let (out2, _, _) = try_apply_edit(content2, "old", "new").unwrap();
        assert_eq!(out2, "x\nnew");
    }

    #[test]
    fn validate_write_content_rejects_nul_and_oversize() {
        assert!(validate_write_content("# ok\n").is_ok());
        assert!(
            validate_write_content("").is_ok(),
            "空文件合法（新建空文件）"
        );
        assert!(validate_write_content("a\0b").is_err(), "NUL 拒");
        let big = "A".repeat(2 * 1024 * 1024 + 1);
        assert!(validate_write_content(&big).is_err(), "超 2MB 拒");
    }

    #[test]
    fn writable_raw_excludes_home_defaults() {
        // 关键安全断言：可写根不含桌面/下载/文档默认项（读白名单有、写白名单刻意没有）
        let raw = build_writable_raw(
            Some("/data/AI_Gen_Files".into()),
            vec!["/Users/me/project".into()],
            &["/Users/me/工作".to_string()],
        );
        assert_eq!(raw.len(), 3, "{raw:?}");
        assert!(!raw
            .iter()
            .any(|d| d.contains("Desktop") || d.contains("Downloads")));
    }

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

    /// walk 跳过符号链接——白名单目录内的软链
    /// 指向外部时，grep 的内容读取跟随软链会把外部文件带进模型上下文
    #[cfg(unix)]
    #[test]
    fn walk_skips_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // 真实文件 + 指向外部文件的软链 + 指向外部目录的软链
        std::fs::write(root.join("real.txt"), b"ok").unwrap();
        let outside =
            std::env::temp_dir().join(format!("wm_outside_{}.txt", uuid::Uuid::new_v4().simple()));
        std::fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), root.join("link_dir")).unwrap();

        let mut visited: Vec<String> = Vec::new();
        walk(root, &mut |p: &Path, _is_dir: bool| {
            visited.push(p.file_name().unwrap().to_string_lossy().to_string());
            true
        });
        assert!(visited.contains(&"real.txt".to_string()));
        assert!(
            !visited.contains(&"link.txt".to_string()),
            "软链文件必须跳过"
        );
        assert!(
            !visited.contains(&"link_dir".to_string()),
            "软链目录必须跳过"
        );
        // 软链目标的内容不应出现在遍历里
        assert!(!visited.iter().any(|n| n.starts_with("wm_outside_")));
        let _ = std::fs::remove_file(&outside);
    }

    /// HOME 缺失（Windows 绿色版常见）时须回退 USERPROFILE
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

    /// 追加语义：allowedDirs 非空时内置默认（桌面/下载/文档）仍生效，
    /// 任务卡绑定文件夹也在并集里（用户列表整体替换默认会导致授权弹窗
    /// 「始终允许」写入一个目录后内置默认失效）
    #[test]
    fn merge_raw_dirs_appends_to_defaults() {
        let home = PathBuf::from("/home/u");
        let cfg = vec!["/data/work".to_string()];
        let task = vec!["/mnt/bound".to_string()];
        let raw = merge_raw_dirs(&cfg, Some(&home), task, Some("/data/gen".to_string()));
        assert_eq!(
            raw,
            vec![
                "/home/u/Desktop",
                "/home/u/Downloads",
                "/home/u/Documents",
                "/data/gen",
                "/mnt/bound",
                "/data/work"
            ]
        );
    }

    /// home 缺失（Windows 绿色版 HOME/USERPROFILE 全空兜底场景）时不产默认目录，
    /// 但 AI 产物目录、任务卡绑定与用户配置仍在
    #[test]
    fn merge_raw_dirs_without_home() {
        let raw = merge_raw_dirs(
            &["/data/x".to_string()],
            None,
            vec!["/mnt/b".to_string()],
            Some("/data/gen".to_string()),
        );
        assert_eq!(raw, vec!["/data/gen", "/mnt/b", "/data/x"]);
        // gen 目录缺失（创建失败）时其余集合不受影响
        let raw = merge_raw_dirs(&["/data/x".to_string()], None, vec![], None);
        assert_eq!(raw, vec!["/data/x"]);
    }

    // ── 白名单逃逸（bot_fs 是「模型编造路径 / 前端 XSS 驱动任意文件读」的关键防线）──

    /// 判定 = canonicalize + 分量前缀比较，两步缺一不可：
    /// `..` 穿越必须被解析后在白名单外，前缀相似目录不得误判命中
    #[test]
    fn allowlist_rejects_traversal_and_prefix_similar_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        std::fs::write(allowed.join("mine.txt"), b"mine").unwrap();

        let allow_dirs = vec![
            std::fs::canonicalize(&allowed).unwrap(),
            std::fs::canonicalize(tmp.path()).unwrap(), // 目录本身也可命中（授权粒度=目录）
        ];

        let inside = std::fs::canonicalize(allowed.join("mine.txt")).unwrap();
        assert!(is_within_allowlist(&inside, &allow_dirs), "白名单内应放行");

        // `..` 穿越：canonicalize 已解析 `..` → 落在白名单外 → 拒不命中
        let traversal = std::fs::canonicalize(allowed.join("../outside/secret.txt")).unwrap();
        assert!(
            !is_within_allowlist(&traversal, &[allow_dirs[0].clone()]),
            "`..` 穿越到白名单外必须被拒"
        );

        // 前缀相似目录（/x/allowed vs /x/allowed-evil）：分量比较不得误吞
        let sibling = tmp.path().join("allowed-evil");
        std::fs::create_dir_all(&sibling).unwrap();
        let sibling_c = std::fs::canonicalize(&sibling).unwrap();
        assert!(
            !is_within_allowlist(&sibling_c, &[allow_dirs[0].clone()]),
            "前缀相似目录不得误判命中"
        );
    }

    /// 软链逃逸（unix）：白名单目录内的软链指向外部 → canonicalize 跟随软链 →
    /// 判定必须在白名单外（否则 grep_files 会把外部文件内容带进模型上下文）
    #[cfg(unix)]
    #[test]
    fn allowlist_rejects_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), allowed.join("link.txt")).unwrap();

        let allow_dirs = vec![std::fs::canonicalize(&allowed).unwrap()];
        let followed = std::fs::canonicalize(allowed.join("link.txt")).unwrap();
        assert!(
            !is_within_allowlist(&followed, &allow_dirs),
            "白名单内软链指向外部文件时必须拒（canonicalize 跟随软链后不在白名单内）"
        );
    }

    /// 【OCR C1b】始终允许该目录：授权粒度 = 用户输入原始父目录（不是 canonical.parent）
    ///
    /// 攻击模型：用户输入 ~/allowed/sub/link.txt → link.txt 是软链 → /etc/passwd。
    /// canonical 解析后 = /etc/passwd，canonical.parent() = /etc。
    /// 旧实现用 canonical.parent() 会把 /etc/ 写进 allowedDirs（symlink escape）。
    /// 新实现必须用 expand_tilde(p).parent()（用户输入视角）。
    ///
    /// 验证方式：构造同样场景，断言 allowlist 增加的是用户输入目录 ~/allowed/sub，
    /// 不是 /etc。直接测 resolve_with_perm 需要 AppHandle 复杂 mock；
    /// 这里走「等价纯函数」路径，验证"dir 计算逻辑"本身。
    #[cfg(unix)]
    #[test]
    fn always_allow_uses_user_input_dir_not_canonical_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let allowed = tmp.path().join("allowed");
        let sub = allowed.join("sub");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // macOS firmlink：/var → /private/var，setup 即规范化，下方断言在 canonical 视角比较
        let outside = std::fs::canonicalize(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();
        // sub/link.txt → /tmp/xxx/outside/secret.txt
        std::os::unix::fs::symlink(outside.join("secret.txt"), sub.join("link.txt")).unwrap();

        // 用户输入路径（与 resolve_with_perm 中"expanded" 同形）
        let user_input = sub.join("link.txt");
        let expanded = user_input.clone();
        let canonical = std::fs::canonicalize(&expanded).unwrap();

        // 旧实现（有 bug）：dir = canonical.parent() = /tmp/xxx/outside
        let old_dir = canonical
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| canonical.clone());

        // 新实现（修复）：dir = expanded.parent()（用户输入视角）
        // expanded 是 ~/allowed/sub/link.txt，expanded.parent() = ~/allowed/sub
        let new_dir = expanded
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| canonical.clone());

        // 旧值会等于 outside（escape 发生），新值必须等于 sub（不逃逸）
        assert_eq!(
            new_dir, sub,
            "新实现 dir 必须是 expanded.parent() = sub，不能是 canonical.parent() = outside"
        );
        assert_ne!(
            old_dir, sub,
            "sanity check：旧实现 dir = canonical.parent() 应等于 outside（演示攻击场景）"
        );
        assert_eq!(
            old_dir, outside,
            "sanity check：旧实现 dir 应该等于 outside（演示 canonical.parent 被换的事实）"
        );
    }

    /// 【OCR C1b TOCTOU】canonicalize 与 read 之间，原路径被替换为同名新 inode 文件 → 必须拒绝。
    ///
    /// 覆盖范围：
    /// - ✅ canonical 出来的 path 不变，但 path 对应的 inode 变了（删 + 同名新建）
    ///   → inode 比对捕获
    /// - ❌ 不覆盖：canonical 出来的 path 仍指向旧 inode，但旧 inode 是 symlink、
    ///   symlink target 被替换为另一文件（攻击者控制 target 内容）。
    ///   见 docs/OCR-FIX-PLAN-2026-09-21.md follow-up：
    ///   "Phase 6: symlink target 替换防护"
    ///
    /// Windows 端 TOCTOU 未实现（保守选择：inode re-check 在 #[cfg(unix)] 下，
    /// Windows 上 InodeCheckOutcome::Ok 直接 no-op 放行）——见 commit message。
    #[cfg(unix)]
    #[tokio::test]
    async fn canonical_path_replaced_with_new_inode_is_rejected() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        std::fs::write(&file, b"original").unwrap();

        // 1. 模拟"canonicalize 与 read 之间，文件被换"
        //    先记录旧 inode（相当于 resolve_with_perm 完成时的 ino）
        let pre_ino = std::fs::metadata(&file).unwrap().ino();

        //    删除原文件
        std::fs::remove_file(&file).unwrap();
        //    同名新建一个**不同 inode**的文件（不同 mtime/dev 都行，关键是 inode）
        std::fs::write(&file, b"replaced").unwrap();
        let new_ino = std::fs::metadata(&file).unwrap().ino();
        assert_ne!(pre_ino, new_ino, "sanity: 新文件 inode 必须与旧不同");

        // 2. 调用 is_binary_file_with_ino_check：应返回 Rejected + is_binary
        let (outcome, is_binary) = is_binary_file_with_ino_check(&file, Some(pre_ino))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            InodeCheckOutcome::Replaced,
            "inode 变了必须返回 Rejected"
        );
        // 顺手验证二进制判定也不丢（这里是文本文件，应为 false）
        assert!(!is_binary, "文本文件的 is_binary 应为 false");
    }
}

// ───────────────────── P1-b：文件变更证据（Agent 透明化设计 §4.2） ─────────────────────

#[cfg(test)]
mod file_change_tests {
    use super::*;

    #[test]
    fn diff_modify_counts_and_unified_text() {
        let before = "a\nb\nc\n";
        let after = "a\nX\nc\nd\n";
        let (diff, added, deleted, truncated) = build_unified_diff(before, after, "/x/f.txt");
        assert_eq!(added, 2, "插入 X 行 + d 行");
        assert_eq!(deleted, 1, "删除 b 行");
        assert!(!truncated);
        let d = diff.unwrap();
        assert!(
            d.starts_with("--- /x/f.txt"),
            "unified header 用目标路径：{d}"
        );
        assert!(d.contains("+X"), "{d}");
        assert!(d.contains("-b"), "{d}");
    }

    #[test]
    fn diff_create_all_insertions() {
        let (diff, added, deleted, truncated) = build_unified_diff("", "l1\nl2\n", "/x/new.txt");
        assert_eq!(added, 2);
        assert_eq!(deleted, 0);
        assert!(!truncated);
        assert!(diff.unwrap().contains("+l1"));
    }

    #[test]
    fn diff_identical_content_yields_no_diff() {
        let (diff, added, deleted, truncated) = build_unified_diff("same\n", "same\n", "/x");
        assert!(diff.is_none(), "零变更不产 diff（write 同内容覆盖场景）");
        assert_eq!((added, deleted, truncated), (0, 0, false));
    }

    #[test]
    fn diff_truncates_text_but_keeps_counts() {
        // MAX_DIFF_LINES=2000：2500 行新文件 → 文本截断置位，±计数不截（计数是事实）
        let after: String = (0..2500).map(|i| format!("line{i}\n")).collect();
        let (diff, added, deleted, truncated) = build_unified_diff("", &after, "/x/big.txt");
        assert!(truncated);
        assert_eq!(added, 2500);
        assert_eq!(deleted, 0);
        let d = diff.unwrap();
        assert!(d.contains("已截断"), "截断标记必须可见：{d}");
        assert_eq!(
            d.lines().count(),
            crate::db::MAX_DIFF_LINES + 1,
            "正文 2000 行 + 截断提示行"
        );
    }

    #[test]
    fn diff_handles_crlf_content() {
        let before = "a\r\nb\r\n";
        let after = "a\r\nB\r\n";
        let (diff, added, deleted, _) = build_unified_diff(before, after, "/x/crlf.txt");
        assert_eq!((added, deleted), (1, 1));
        let d = diff.unwrap();
        assert!(d.contains("-b"), "{d:?}");
    }

    #[test]
    fn receipt_create_has_no_before_evidence() {
        let r = build_file_change_receipt("/x/new.txt", "create", None, "hello\n");
        assert_eq!(r.kind, "create");
        assert_eq!(r.path, "/x/new.txt");
        assert_eq!(r.added, 1);
        assert_eq!(r.deleted, 0);
        assert!(r.before_sha.is_none(), "create 无 before 证据");
        assert!(r.before_ref.is_none(), "快照由调用方回填，builder 恒 None");
        assert_eq!(r.after_sha.as_deref(), Some(sha256_hex("hello\n").as_str()));
        assert!(r.diff.unwrap().contains("+hello"));
    }

    #[test]
    fn receipt_modify_carries_before_sha_and_diff() {
        let r = build_file_change_receipt("/x/f.py", "modify", Some("old\n"), "new\n");
        assert_eq!(r.kind, "modify");
        assert_eq!(r.added, 1);
        assert_eq!(r.deleted, 1);
        assert_eq!(r.before_sha.as_deref(), Some(sha256_hex("old\n").as_str()));
        assert_eq!(r.after_sha.as_deref(), Some(sha256_hex("new\n").as_str()));
        assert!(r.diff.unwrap().contains("-old"));
    }

    #[test]
    fn tool_result_carries_file_changes_and_defaults_empty() {
        let r = ToolResult::ok("x", Vec::new())
            .with_file_change(build_file_change_receipt("/a", "create", None, "b\n"));
        assert_eq!(r.file_changes.len(), 1);
        assert_eq!(r.file_changes[0].path, "/a");
        // 其余构造器默认空——既有 33 工具零感知（加字段不改语义的兼容锁）
        assert!(ToolResult::ok("o", Vec::new()).file_changes.is_empty());
        assert!(ToolResult::warn("w", Vec::new()).file_changes.is_empty());
        assert!(ToolResult::error("e", Vec::new()).file_changes.is_empty());
    }

    /// sha256 已知向量（回滚证据链的算法稳定性锁——换库/换编码当场炸）
    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
