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

/// 单次读取上限（字节）。`pub(crate)`：溢出指针文案要据此说明「可达范围」
/// （指针承诺了怎么读，就得说清读得到哪一段）。
pub(crate) const READ_MAX_BYTES: usize = crate::bot::constants::READ_MAX_BYTES;
const READ_DEFAULT_LINES: usize = crate::bot::constants::READ_DEFAULT_LINES;
const READ_MAX_LINES: usize = crate::bot::constants::READ_MAX_LINES;
const READ_MAX_LINE_CHARS: usize = crate::bot::constants::READ_MAX_LINE_CHARS;
const GREP_MAX_HITS: usize = crate::bot::constants::GREP_MAX_HITS;
const GREP_MAX_FILE_BYTES: u64 = crate::bot::constants::GREP_MAX_FILE_BYTES;
const LIST_MAX_ENTRIES: usize = crate::bot::constants::LIST_MAX_ENTRIES;
/// 目录递归上限（防止深层目录树把上下文/耗时打爆）
const WALK_MAX_DEPTH: usize = crate::bot::constants::WALK_MAX_DEPTH;
/// 遍历时跳过的大而杂目录（另跳过所有 . 开头隐藏目录）
const SKIP_DIRS: [&str; 4] = ["node_modules", "target", "dist", "build"];
/// edit_file 可编辑文件上限（对齐 read 的可处理量级，超出提示拆分）
const EDIT_MAX_FILE_BYTES: usize = crate::bot::constants::EDIT_MAX_FILE_BYTES;
/// write_file 内容上限（对齐 GREP_MAX_FILE_BYTES 量级）
const WRITE_MAX_BYTES: usize = crate::bot::constants::WRITE_MAX_BYTES;

// per-tool 权限规则（Agent 透明化设计 §9.2-1）

/// 规则评估结果（纯函数可测；评估序 deny > ask > allow，同工具多规则**首中即停**）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRuleAction {
    Deny,
    Ask,
    Allow,
}

/// 查工具规则表：返回该 tool 的首条命中规则；None = 无命中（走全局 perm_mode 兜底）。
/// 非法 action 的规则在落盘清洗时已剔除（commands::sanitize_tool_rules），
/// 这里对脏数据防御性跳过（不 Err——观测/授权面不因配置脏值崩）。
pub(crate) fn lookup_tool_rule(
    rules: Option<&[crate::bot::config::types::ToolRule]>,
    tool: &str,
) -> Option<ToolRuleAction> {
    let rules = rules?;
    for r in rules {
        if r.tool != tool {
            continue;
        }
        return match r.action.as_str() {
            "deny" => Some(ToolRuleAction::Deny),
            "ask" => Some(ToolRuleAction::Ask),
            "allow" => Some(ToolRuleAction::Allow),
            _ => None, // 同工具首条非法 → 视为无命中（清洗层已防，双保险）
        };
    }
    None
}

/// 读配置并查当前工具的规则（含审计 KV 用；无规则返回 None）
pub(crate) fn tool_rule_for(app: &AppHandle, tool: &str) -> Option<ToolRuleAction> {
    let cfg = crate::bot::load_config(app);
    lookup_tool_rule(cfg.tool_rules.as_deref(), tool)
}

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
            return home.join(p.trim_start_matches(['~', '/', '\\']));
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

/// Windows 长路径失败时的提示后缀：剥掉 `\\?\` 前缀的路径超过默认 260 上限、
/// 系统又没开启长路径支持时，fs 调用报的是普通 IO 错误（找不到路径/文件名
/// 过长），读不出真因——文案把这层点破。非 Windows 或路径不长时返回空串。
pub(crate) fn win_long_path_hint(p: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let n = p.as_os_str().to_string_lossy().chars().count();
        if n > 240 {
            return format!(
                "（路径 {} 字符，接近或超过 Windows 默认 260 上限：系统未开启长路径支持时这类路径会操作失败，可缩短路径，或在组策略/注册表启用 LongPathsEnabled）",
                n
            );
        }
    }
    #[cfg(not(windows))]
    let _ = p;
    String::new()
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
/// session 对应会话有活动技能时，该技能目录追加进读白名单
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
    // 上会阻塞全部 Tauri command / event（ performance）。
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
    // 活动技能目录 + 溢出目录的追加不依赖配置/DB 加载，独立成泛型函数：
    // allowed_dirs 其余部分绑死 Wry AppHandle，这段抽出来后 MockRuntime
    // 单测可以直驱，钉住「溢出根目录恒在读白名单且路径精确」。
    append_session_read_dirs(app, session, &mut out);
    out
}

/// 会话级附加读白名单：活动技能目录 + 溢出目录（都只加读侧，写闸门不受影响）。
fn append_session_read_dirs<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session: Option<&str>,
    out: &mut Vec<PathBuf>,
) {
    // 活动技能目录（会话级，只加读白名单；无活动技能/无目录则无追加）
    if let Some(run) = crate::bot_skills::active_skill_run_for(app, session) {
        if let Some(dir) = &run.dir {
            if let Ok(c) = std::fs::canonicalize(dir) {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    // 溢出目录（工具大结果的转存位置）：恒在读白名单里。
    // 漏了它，回灌给模型的指针就是一张读不回的废纸——模型只能看到节选、拿不到正文，
    // 等于比直接截断还糟。目录还不存在（还没发生过溢出）时 canonicalize 失败即跳过，
    // 而白名单是每次 read 调用重算的，等模型真去读时目录已经建好。
    // 只加读侧：writable_dirs 不动，模型不能往里写。
    if let Ok(c) = std::fs::canonicalize(crate::overflow::root(app)) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    // py 沙箱运行目录（temp/wmessage-py-runs）：脚本跑失败时目录会留下来，
    // 模型要读回「刚才到底跑了什么」就得能读它——读不到就只能把整段脚本重写一遍
    // （每次重写都重付一遍全文的输出 token）。
    // 目录还不存在（还没跑过 python）时 canonicalize 失败即跳过；白名单每次读调用
    // 重算，等模型真去读时目录已经建好。
    // 只加读侧：模型不能往沙箱里写（可写的只有 AI_Gen_Files 等既有根）。
    if let Ok(c) = std::fs::canonicalize(crate::py::runtime::py_runs_root()) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
}

/// 路径守卫（Kimi CLI 风格「执行前授权」）：
/// ~ 展开 → canonicalize（不存在即拒）→ 白名单命中直接放行；
/// 白名单外按 perm_mode 分流：strict 硬拒 / ask 弹授权窗（允许一次 /
/// 始终允许该目录自动写入 allowedDirs / 拒绝）/ yolo 直接放行。全程记审计。
/// interactive/session_id（会话隔离）：后台执行（interactive=false）
/// 不弹授权窗直接拒；弹窗事件带 sessionId 供前端按会话过滤。
/// 成功返回 canonical 路径（Windows 剥 \\?\ 前缀）。
/// 包 spawn_blocking 同步 syscall：避免阻塞 Tauri async runtime（ performance）。
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

/// async 版 is_dir：单次 metadata syscall 同样移出 runtime 线程。
///
/// 三态返回：**Some(true)** 是目录 / **Some(false)** 不是目录 /
/// **None** 探测失败（阻塞线程 join 失败或 metadata 报错）。
///
/// 为什么不是 bool：调用方必须能区分「不是目录」与「没探到」。授权粒度那处
/// 分不清就会朝**过度授权**方向失败：探测失败被判成文件 → 授权落到父目录，
/// 比用户选中的那个目录宽一层。
async fn is_dir_async(path: &Path) -> Option<bool> {
    let p = path.to_path_buf();
    // metadata 的 io::Error（不存在 / 无权限）与线程 join 失败都归到 None（探不到）。
    // spawn_blocking_io 已把 io::Error 压成 Err(String)，故一层 ok() 即三态。
    spawn_blocking_io(move || std::fs::metadata(&p).map(|m| m.is_dir()))
        .await
        .ok()
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
    // 会把 /etc/ 写进白名单（ symlink escape）。
    let expanded = expand_tilde(p);
    // canonicalize 包 spawn_blocking：同步 syscall 在 async runtime 会阻塞全部
    // Tauri command / event （ performance critical）。
    // 把 owned clone 先拿出来再 move 进闭包：闭包要求 'static，原 expanded 留给 symlink 修复用。
    let expanded_for_canonical = expanded.clone();
    let canonical =
        spawn_blocking_io(move || std::fs::canonicalize(expanded_for_canonical)).await?;
    let dirs = allowed_dirs(app, session_id).await;
    if is_within_allowlist(&canonical, &dirs) {
        return Ok(strip_verbatim(canonical));
    }
    // allow 规则 = 单工具 yolo（白名单外放行+审计；穿越/软链在 canonicalize
    // 后同样落白名单外——与全局 yolo 档同语义，由规则显式授权兜底）
    if crate::bot_fs::tool_rule_for(app, tool) == Some(crate::bot_fs::ToolRuleAction::Allow) {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "bot_fs.rule_allow",
            "tool" => tool,
            "path" => p,
        );
        return Ok(strip_verbatim(canonical));
    }
    match crate::bot::perm_mode(app) {
        crate::bot::PermMode::Yolo => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "bot_fs.yolo_allow",
                "tool" => tool,
                "path" => p,
            );
            Ok(strip_verbatim(canonical))
        }
        //  auto 档：白名单外降级 ask（弹窗/无人值守拒）——与 Ask 同分支
        crate::bot::PermMode::Ask | crate::bot::PermMode::Auto => {
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
                    crate::audit_event!(
                        app,
                        crate::audit::AuditLevel::Info,
                        "bot_fs.ask_allow_once",
                        "tool" => tool,
                        "path" => p,
                    );
                    Ok(strip_verbatim(canonical))
                }
                crate::bot_slash::ConfirmChoice::Always => {
                    // 【 symlink 修复】授权粒度 = 用户输入原始路径的父目录
                    // （文件 → parent、目录 → self），与判定逻辑保持一致。
                    // 绝不能用 canonical(parent())：那个是 symlink 解析后的路径，
                    // 可被攻击者控制指向 allowlist 外。
                    let dir = match is_dir_async(&expanded).await {
                        Some(true) => expanded.clone(),
                        Some(false) => expanded
                            .parent()
                            .map(|d| d.to_path_buf())
                            .unwrap_or_else(|| expanded.clone()),
                        // 探测失败：不落盘授权。落盘就得先定粒度，而两种粒度
                        // 此刻都只是猜——猜成文件会把父目录（比用户选的宽一层）
                        // 写进白名单，安全闸门朝过度授权方向失败。宁可不授权。
                        None => {
                            crate::audit_event!(
                                app,
                                crate::audit::AuditLevel::Warn,
                                "bot_fs.always_persist_skip",
                                "tool" => tool,
                                "path" => expanded.to_string_lossy(),
                                "reason" => "路径类型探测失败",
                            );
                            return Err(format!(
                                "无法确认路径类型，本次授权未保存（探测失败，请重试）：{p}"
                            ));
                        }
                    };
                    let dir_str = strip_verbatim(dir).to_string_lossy().to_string();
                    if let Err(e) = crate::bot::add_allowed_dir(app, &dir_str) {
                        crate::audit_event!(
                            app,
                            crate::audit::AuditLevel::Warn,
                            "bot_fs.always_persist_fail",
                            "err" => &e,
                        );
                    }
                    crate::audit_event!(
                        app,
                        crate::audit::AuditLevel::Info,
                        "bot_fs.ask_allow_always",
                        "tool" => tool,
                        "dir" => &dir_str,
                    );
                    Ok(strip_verbatim(canonical))
                }
                crate::bot_slash::ConfirmChoice::Deny => {
                    crate::audit_event!(
                        app,
                        crate::audit::AuditLevel::Warn,
                        "bot_fs.ask_denied",
                        "tool" => tool,
                        "path" => p,
                    );
                    Err(format!(
                        "用户未授权访问该路径：{p}（可在授权弹窗点「始终允许该目录」，或在设置页把目录加入白名单）"
                    ))
                }
            }
        }
        crate::bot::PermMode::Strict => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "bot_fs.denied",
                "path" => p,
                "reason" => "不在白名单目录内",
            );
            Err(format!(
                "路径不在白名单目录内：{p}（白名单：设置页 allowedDirs，默认 桌面/下载/文档 + 任务卡绑定文件夹）"
            ))
        }
    }
}

/// 读白名单内文本文件的全部内容（有上限），供「按路径取内容」的工具用。
///
/// 与 read_text_file 走同一条授权链（resolve_with_perm：canonicalize + 白名单
/// 判定 + 白名单外弹授权窗）+ 同一个有界读，不另开「只判路径不判内容」的旁路。
/// 超上限按截断处理并在返回值里说明——静默截断会让模型以为脚本全文如此。
pub async fn read_whitelisted_text(
    app: &AppHandle,
    tool: &str,
    path: &str,
    interactive: bool,
    session_id: Option<&str>,
    max_bytes: usize,
) -> Result<String, String> {
    let canonical = resolve_with_perm(app, tool, path, interactive, session_id).await?;
    let (raw, truncated) =
        spawn_blocking_io(move || read::read_capped_file_sync(&canonical, max_bytes)).await?;
    let text = String::from_utf8_lossy(&raw).to_string();
    if truncated {
        return Err(format!(
            "文件超过 {}KB 上限，只读到前部——请拆分后再试",
            max_bytes / 1024
        ));
    }
    Ok(text)
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

mod edit;
mod read;
mod receipt;

// 拆出后的子模块转出：保持 `crate::bot_fs::X` 旧路径与旧可见性不变
// （pub 的转成 pub、pub(crate) 的转成 pub(crate)、模块私有的仍是私有）
pub(crate) use edit::resolve_writable;
pub use edit::{tool_edit_file, tool_write_file};
pub use read::{tool_grep_files, tool_list_files, tool_read_text_file};
pub(crate) use receipt::build_binary_file_receipt;
pub use receipt::FileChangeReceipt;
pub(crate) use receipt::{build_unified_diff, sha256_hex};

// 跨模块共用项：门面转出后由 bot_fs/ 各子模块经 `use super::*` 取用
use read::read_capped_file_sync;
use receipt::{build_file_change_receipt, write_before_snapshot};

// 仅测试用到的内部项：转出后由 bot_fs/*_tests.rs 经 `use super::*` 取用
#[cfg(test)]
use edit::{
    build_writable_raw, edit_file_sync, try_apply_edit, validate_write_content, EditErrorKind,
    EditMode,
};
#[cfg(test)]
use read::{
    format_skip_note, glob_match, is_generated_name, read_with_ino_check, render_context_hits,
    render_read_slice, walk, InodeCheckOutcome,
};

#[cfg(test)]
mod file_change_tests;
#[cfg(test)]
mod read_path_single_open_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_rule_tests;
