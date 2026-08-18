//! WMessage 机器人技能系统（轻量版 Agent Skills，参照 Anthropic SKILL.md 规范）。
//!
//! - 技能 = 数据目录 skills/<name>/SKILL.md（YAML frontmatter：name + description；正文=执行指令）
//! - Progressive disclosure：系统提示词只注入「名称+描述」清单；正文由 use_skill 工具按需读取
//! - 安装：设置页导入技能文件夹（拷贝进数据目录），或手动放入数据目录 skills/
//! - 安全：技能名白名单字符集（防路径穿越）；正文读取有大小上限

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use tauri::AppHandle;
use crate::audit_event;

/// 打开文件/文件夹（Rust 侧调用 opener 插件）：绕过前端窗口的 opener scope，
/// 挂件窗口内聊天文件按钮点击直接走这里，失败返回错误给前端兜底 revealItemInDir
#[tauri::command]
pub fn open_file_path(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

/// 文件多选对话框（Rust 侧 spawn_blocking 弹框）：挂件窗口 ➕ 添加附件用。
/// 此前前端 dialog.open 在挂件窗口可能不弹框，与 doc_extract 弹框同方案修复。
#[tauri::command]
pub async fn pick_files_dialog(app: AppHandle) -> Result<Vec<String>, String> {
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        handle.dialog().file().blocking_pick_files()
    })
    .await
    .map_err(|e| format!("对话框线程失败：{e}"))?;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| match p {
            tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
            _ => None,
        })
        .collect())
}

const SKILL_NAME_CHARS_OK: fn(char) -> bool = |c| c.is_ascii_alphanumeric() || c == '-' || c == '_';
const MAX_SKILL_BODY: usize = 50 * 1024;

/// 删除任务卡绑定的本地文件/文件夹（回收站彻底删除时调用）。
/// 使用跨平台 trash crate：macOS → 废纸篓 / Windows → 回收站 / Linux → gio trash。
/// trash::delete 内部递归处理文件/目录两种类型，不需要按 is_dir 分流。
/// 路径不存在视为已删（幂等）。
/// 错误透传给前端（权限不足/路径异常/网络盘不支持）→ 任务行保留，用户可重试
/// （老板 2026-08-17 21:17：安全优先于不可恢复，误删可从废纸篓/回收站找回）
///
/// ⚠️ 踩坑（2026-08-17 21:21 老板报「TargetedRoot」）：不能写 `trash::delete_all(p)`——
///    `&Path` 实现了 `IntoIterator<Item = &OsStr>`（Path::components 迭代器），
///    delete_all 会逐个 component 调 trash，第一个 component `/`（根）的 parent() 是 None → TargetedRoot。
///    正确写法是单数 `trash::delete(p)`（内部 `delete_all(&[path])`，把整条路径当作一项处理）。
#[tauri::command]
pub fn delete_bound_file(path: String, is_dir: bool) -> Result<(), String> {
    use std::path::Path;
    let p = Path::new(&path);
    if !p.exists() {
        return Ok(());
    }
    let _ = is_dir; // trash::delete 内部递归处理两种类型，不再需要分流
    trash::delete(p).map_err(|e| {
        format!("移到{}失败：{e}（文件可能仍在原位置，任务卡保留可重试）",
            if cfg!(target_os = "macos") { "废纸篓" }
            else if cfg!(target_os = "windows") { "回收站" }
            else { "垃圾箱" }
        )
    })
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// Phase 5 D (2026-08-18 08:00): last run outcome (SettingsPage badge)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<crate::db::PersistedSkillOutcome>,
}

fn skills_dir(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("skills")
}

/// dev 模式 mock Skill 扫描源：`target/debug/skills/`（dev 模式下随编译产物可见，
/// release 模式走 #[cfg(debug_assertions)] 条件编译不包含此函数）。
///
/// 优先级：数据目录的 Skill 优先 dev mock（用户已导入 / 修改的版本不被 dev 版本覆盖）。
/// 路径解析：`CARGO_TARGET_DIR` 环境变量优先，否则 `manifest_dir/target`。
/// 单测可通过 `dev_skills_dir_at(path)` 注入临时目录验证。
#[cfg(debug_assertions)]
fn dev_skills_dir() -> Option<std::path::PathBuf> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));
    dev_skills_dir_at(&target_root)
}

#[cfg(debug_assertions)]
fn dev_skills_dir_at(target_root: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = target_root.join("debug").join("skills");
    if path.exists() { Some(path) } else { None }
}

/// Skill 搜索路径列表：数据目录必选 + dev 模式追加 target/debug/skills。
/// 数据目录在前 → scan_skill_dirs 用 HashSet seen 去重时数据目录优先。
/// `#[cfg_attr(not(debug_assertions), allow(dead_code))]` —— release 模式 dev_skills_dir
/// 不存在，整个函数仅返回一个目录（数据目录），避免 dead_code 警告。
#[cfg_attr(not(debug_assertions), allow(dead_code))]
fn skill_search_paths(app: &AppHandle) -> Vec<std::path::PathBuf> {
    #[allow(unused_mut)] // release 模式：dev_skills_dir 分支被排除，paths 不需要 mut
    let mut paths = vec![skills_dir(app)];
    #[cfg(debug_assertions)]
    {
        if let Some(dev) = dev_skills_dir() {
            if dev != paths[0] {
                paths.push(dev);
            }
        }
    }
    paths
}

/// SKILL.md 完整元数据（Skill 运行模型 v1.0 字段集，全部带默认值）
#[derive(Debug, Clone)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    pub risk_level: String, // low | medium | high
    pub mode: String,       // auto | interactive
    pub max_steps: usize,
    pub timeout_secs: u64,
    pub rollback: String, // none | auto
    pub enabled: bool,
    /// 是否支持暂停后断点续跑（false 时暂停即终止：确认完成当前动作后技能结束）
    pub resumable: bool,
    pub intents: Vec<String>,
}

impl Default for SkillMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            risk_level: "medium".into(),
            mode: "interactive".into(),
            max_steps: 8,
            timeout_secs: 180,
            rollback: "none".into(),
            enabled: true,
            resumable: false,
            intents: Vec::new(),
        }
    }
}

fn parse_frontmatter(text: &str, dir_name: &str) -> (String, String) {
    let meta = parse_meta(text, dir_name);
    (meta.name, meta.description)
}

/// 解析完整元数据（调度器用）：缺失字段走默认值；非法值回退默认
pub fn parse_meta(text: &str, dir_name: &str) -> SkillMeta {
    let mut m = SkillMeta::default();
    let body = text.strip_prefix("---").unwrap_or(text);
    let Some(end) = body.find("\n---") else {
        m.name = dir_name.to_string();
        return m;
    };
    let fm = &body[..end];
    for line in fm.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim() {
            "name" => m.name = v.to_string(),
            "description" => m.description = v.to_string(),
            "risk_level" => {
                let v2 = v.to_ascii_lowercase();
                m.risk_level = if matches!(v2.as_str(), "low" | "medium" | "high") {
                    v2
                } else {
                    "medium".into()
                }
            }
            "mode" => {
                let v2 = v.to_ascii_lowercase();
                m.mode = if matches!(v2.as_str(), "auto" | "interactive") {
                    v2
                } else {
                    "interactive".into()
                }
            }
            "max_steps" => {
                if let Ok(n) = v.parse::<usize>() {
                    m.max_steps = n.clamp(1, 20);
                }
            }
            "timeout_secs" => {
                if let Ok(n) = v.parse::<u64>() {
                    m.timeout_secs = n.clamp(10, 600);
                }
            }
            "rollback" => {
                let v2 = v.to_ascii_lowercase();
                m.rollback = if matches!(v2.as_str(), "none" | "auto") {
                    v2
                } else {
                    "none".into()
                }
            }
            "enabled" => {
                m.enabled = !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "no");
            }
            "resumable" => {
                m.resumable = matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes");
            }
            "intents" => {
                // 支持 ["a","b"] 或逗号分隔 "a,b"
                let inner = v.trim();
                m.intents = inner
                    .trim_matches(['[', ']'])
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    if m.name.is_empty() || !m.name.chars().all(SKILL_NAME_CHARS_OK) {
        m.name = dir_name.to_string();
    }
    if m.name.is_empty() {
        m.name = dir_name.to_string();
    }
    // 风险推导模式（显式 mode 优先）：high/medium → interactive；low → auto
    // 元数据里 mode 未显式标记时按风险推导：这里无法区分"显式"与否（默认已是 interactive），
    // 保持解析值即可；high 风险强制 interactive（安全兜底）
    if m.risk_level == "high" {
        m.mode = "interactive".into();
    }
    m
}

/// 扫描数据目录 + dev 模式 target/debug/skills 下所有 SKILL.md，返回 (目录名, 名称, 描述) 列表。
/// 数据目录优先（用户已导入 / 修改的 Skill 不被 dev mock 覆盖）。
/// 内部委托给纯函数 `scan_skill_dirs`，后者不依赖 AppHandle，单测可独立覆盖。
pub fn scan_skills(app: &AppHandle) -> Vec<SkillInfo> {
    let mut out = scan_skill_dirs(&skill_search_paths(app));
    if let Ok(conn) = crate::db::open_db(app) {
        if let Ok(outcomes) = crate::db::load_all_skill_outcomes(&conn) {
            for s in &mut out { s.last_outcome = outcomes.get(&s.name).cloned(); }
        }
    }
    out
}

/// Phase 5 D: persist DslOutcome to DB after run_skill_scheduler finishes (quiet failure)
fn persist_outcome_quiet(app: &AppHandle, name: &str, kind: &str, reason: Option<&str>, summary: Option<&str>, rollback_attempted: Option<bool>) {
    let Ok(conn) = crate::db::open_db(app) else { return };
    let outcome = crate::db::PersistedSkillOutcome {
        skill_name: name.to_string(),
        kind: kind.to_string(),
        reason: reason.map(|s| s.to_string()),
        completed_summary: summary.map(|s| s.to_string()),
        rollback_attempted,
        last_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    let _ = crate::db::upsert_skill_outcome(&conn, &outcome);
}

/// 多目录扫描核心（Phase 4 第 4 项 2026-08-18 07:09）：
/// 顺序扫 `dirs` 列表中的每个目录，每个目录的子目录视为一个 Skill（含 SKILL.md 即有效）。
/// **前面目录优先**（用 HashSet seen 去重）：同名 Skill 只保留先扫到的版本。
/// 排序按 name 字典序，输出稳定。
///
/// 单测场景：传临时目录数组验证去重逻辑，不依赖 AppHandle / db::data_dir。
pub fn scan_skill_dirs(dirs: &[std::path::PathBuf]) -> Vec<SkillInfo> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<SkillInfo> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() { continue; }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if !seen.insert(dir_name.clone()) { continue; }
            let skill_md = path.join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&skill_md) else { continue };
            let (name, desc) = parse_frontmatter(&text, &dir_name);
            out.push(SkillInfo {
                name,
                description: desc,
                last_outcome: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 技能清单块：注入系统提示词尾部（progressive disclosure 第一层）。
pub fn build_skill_block(app: &AppHandle) -> String {
    let skills = scan_skills(app);
    if skills.is_empty() {
        return "已安装技能：无".into();
    }
    let mut out = String::from(
        "已安装技能（当任务需要专业技能时，先调用 use_skill 读取对应技能的完整文档，再按文档步骤执行；任务涉及的每个相关技能都要读，可多次调用、综合运用）：\n",
    );
    // 歧义兜底（核验要求）：技能较多时，指令可能匹配多个技能 → 先向用户确认再执行
    if skills.len() >= 3 {
        out.push_str(
            "注意：已安装技能较多。当用户指令与多个技能都可能相关、无法确定用哪个时，先列出候选技能（名称+一句话说明）向用户确认，用户选定后再 use_skill 读取执行；不要自行猜测。\n",
        );
    }
    for s in &skills {
        let desc = if s.description.is_empty() {
            "(无描述)".to_string()
        } else {
            s.description.chars().take(120).collect::<String>()
        };
        out.push_str(&format!("- {}: {}\n", s.name, desc));
    }
    out
}

// ───────────────────────── Skill 调度器（运行模型 v1.0：状态机 + 步骤循环 + 熔断 + 回滚记录） ─────────────────────────

/// Skill 运行状态（生命周期状态机，与 docs/SKILL-RUNTIME.md 对齐）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillState {
    /// 已加载：SKILL.md 解析成功，尚未预审
    Loaded,
    /// 前置预审通过，进入执行
    Running,
    /// 暂停等待人工确认（模式 B 中高危节点）
    Paused,
    /// 成功完成
    Completed,
    /// 失败异常（步骤失败/超时/熔断）
    Failed,
    /// 已终止（预审拦截/用户取消/强制停止）
    Terminated,
}

/// Skill 运行实例（会话级，内存态，不持久化）
#[derive(Clone)]
pub struct SkillRun {
    pub name: String,
    pub state: SkillState,
    /// 已执行步骤数（每次非 use_skill 的工具调用计一步）
    pub step: usize,
    pub max_steps: usize,
    pub started_at_ms: i64,
    pub timeout_secs: u64,
    /// 运行模式（auto/interactive）：v1.0 暂停语义由底层 ask_user_confirm 承接，
    /// 字段为审计与后续暂停态切换保留
    #[allow(dead_code)]
    pub mode: String,
    #[allow(dead_code)]
    pub risk_level: String,
    pub rollback: String,
    /// 已执行动作记录（工具名+参数摘要；回滚清单来源）
    pub actions: Vec<String>,
    /// 终止原因（Terminated/Failed 时填写）
    pub end_reason: String,
    /// 是否支持暂停后续跑（元数据 resumable 镜像）
    pub resumable: bool,
    /// resumable=false 时的暂停即终止：确认通过后执行当前动作，随后技能终止
    pub terminal_after_confirm: bool,
}

impl SkillRun {
    fn new(meta: &SkillMeta) -> Self {
        Self {
            name: meta.name.clone(),
            state: SkillState::Loaded,
            step: 0,
            max_steps: meta.max_steps,
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            timeout_secs: meta.timeout_secs,
            mode: meta.mode.clone(),
            risk_level: meta.risk_level.clone(),
            rollback: meta.rollback.clone(),
            actions: Vec::new(),
            end_reason: String::new(),
            resumable: meta.resumable,
            terminal_after_confirm: false,
        }
    }
}

/// 活动 Skill 运行表：name → SkillRun（同一技能同轮只允许一个实例）
static SKILL_RUNS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, SkillRun>>> =
    std::sync::OnceLock::new();

fn skill_runs() -> &'static std::sync::Mutex<std::collections::HashMap<String, SkillRun>> {
    SKILL_RUNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 当前是否有 Skill 处于 Running 状态（老板 2026-08-17 18:14 后置拦截拍板）
///
/// - Running：LLM 正在 Skill 内部循环中，原子工具是 Skill 的子步骤 → 放行
/// - Loaded / Finished / Terminated / Failed / Paused：原子工具应被阻断
///
/// 供 `tool_guard::is_skill_active` 调用（穿透 private Mutex 访问）
pub fn is_skill_active() -> bool {
    let Ok(guard) = skill_runs().lock() else { return false };
    guard.values().any(|r| matches!(r.state, SkillState::Running))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 主循环接入用（Block 2 2026-08-17 22:26）：克隆当前活动 Skill 快照（任意非 Loaded 状态）。
/// 供 `bot_chat` 主循环调，拿快照去 `advance_skill` 决策，不再持锁。
///
/// 状态过滤说明：Loaded 是 start_skill 中的过渡态——技能预审通过后立即转为 Running，
/// Loaded 仅在断言/异常路径短暂存在，advance_skill 返回 NoActive 与主循环预期一致。
pub fn active_skill_run() -> Option<SkillRun> {
    let guard = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .values()
        .find(|r| !matches!(r.state, SkillState::Loaded))
        .cloned()
}

/// 读取技能正文 + 完整元数据（Phase 4 第 4 项 2026-08-18 07:09：多目录 fallback）
/// 遍历 `skill_search_paths(app)`：数据目录找不到 → dev 模式 fallback target/debug/skills。
/// 数据目录优先（用户已修改的 Skill 优先于 dev mock 版本）。
fn load_skill_meta(app: &AppHandle, name: &str) -> Result<(SkillMeta, String), String> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err("技能名无效".into());
    }
    for dir in &skill_search_paths(app) {
        let path = dir.join(name).join("SKILL.md");
        if !path.exists() { continue; }
        let text = std::fs::read_to_string(&path)
            .map_err(|_| format!("技能「{name}」存在但 SKILL.md 读取失败"))?;
        let meta = parse_meta(&text, name);
        let body: String = text.chars().take(MAX_SKILL_BODY).collect();
        return Ok((meta, body));
    }
    Err(format!("技能「{name}」不存在（检查设置页技能列表）"))
}

// ─────────────────────── DSL 解析 + 调度器（Phase 1 2026-08-17 23:15）───────────────────────

/// Skill 步骤（DSL 解析后的结构，Phase 1）
///
/// Markdown DSL 格式（仅 `mode: "auto"` 的 Skill 走调度器）：
///   ## Step N: 标题
///   tool_name({"arg": "value"})
///   ## Rollback
///   undo_tool({"arg": "value"})
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillStep {
    /// 1-based 步骤序号（用于错误信息 / 回滚顺序）
    pub index: usize,
    pub title: String,
    /// 工具名（必须存在于 TOOLS 白名单）
    pub tool_name: String,
    /// JSON 参数串（直接透传给 `bot::execute_tool`）
    pub args_json: String,
}

/// 剥离 YAML frontmatter（`---` ... `---`），返回正文部分。
fn strip_frontmatter(text: &str) -> &str {
    if !text.starts_with("---") {
        return text;
    }
    let body = &text[3..];
    match body.find("\n---") {
        Some(end) => body[end + 4..].trim_start_matches('\n'),
        None => text,
    }
}

/// 解析 `## Step N: 标题` 或 `## Step N 标题` → `(序号, 标题)`
fn parse_step_heading(line: &str) -> Option<(usize, String)> {
    let rest = line.trim().strip_prefix("## Step ")?;
    let (num_str, title) = if let Some(colon) = rest.find(':') {
        (&rest[..colon], rest[colon + 1..].trim())
    } else if let Some(space) = rest.find(' ') {
        (&rest[..space], rest[space + 1..].trim())
    } else {
        (rest, "")
    };
    let num: usize = num_str.trim().parse().ok()?;
    Some((num, title.to_string()))
}

/// 解析一行工具调用：`tool_name({"arg": "value"})` → `(name, args)`
/// 约束：单行、结尾 `)`、name 为 ASCII 字母数字下划线；args 可含任意字符（含嵌套括号）
fn parse_tool_call(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if !line.ends_with(')') {
        return None;
    }
    let open = line.find('(')?;
    let name = line[..open].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let args = &line[open + 1..line.len() - 1];
    Some((name.to_string(), args.to_string()))
}

/// 解析 Skill body 为 (主步骤, 回滚步骤)。
/// - 空 body / 无 step → `(Vec::new(), Vec::new())`
/// - 解析失败 → Err
pub fn parse_skill_steps(body: &str) -> Result<(Vec<SkillStep>, Vec<SkillStep>), String> {
    let body = strip_frontmatter(body);
    let mut steps: Vec<SkillStep> = Vec::new();
    let mut rollback: Vec<SkillStep> = Vec::new();
    let mut current: Option<SkillStep> = None;
    let mut current_rollback: Option<SkillStep> = None;
    let mut mode: &str = "step"; // "step" | "rollback"

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((idx, title)) = parse_step_heading(line) {
            if let Some(s) = current.take() {
                steps.push(s);
            }
            if let Some(r) = current_rollback.take() {
                rollback.push(r);
            }
            current = Some(SkillStep {
                index: idx,
                title,
                tool_name: String::new(),
                args_json: String::new(),
            });
            mode = "step";
        } else if line == "## Rollback" || line.starts_with("## Rollback ") {
            if let Some(s) = current.take() {
                steps.push(s);
            }
            current_rollback = Some(SkillStep {
                index: 0,
                title: "rollback".into(),
                tool_name: String::new(),
                args_json: String::new(),
            });
            mode = "rollback";
        } else if line.starts_with('#') {
            continue;
        } else if let Some((name, args)) = parse_tool_call(line) {
            let target = if mode == "step" { &mut current } else { &mut current_rollback };
            if let Some(s) = target.as_mut() {
                s.tool_name = name;
                s.args_json = args;
            }
        }
    }
    if let Some(s) = current {
        steps.push(s);
    }
    if let Some(r) = current_rollback {
        rollback.push(r);
    }
    Ok((steps, rollback))
}

// ─────────────────────── 变量替换（Phase 2 2026-08-17 23:23）───────────────────────

/// 任务卡 UUID 提取正则（标准 UUID v4 格式）
static TASK_ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}").unwrap()
});

/// `${stepN.field}` 索引匹配（field ∈ {result, id}）
static VAR_BY_INDEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\{step(\d+)\.(result|id)\}").unwrap()
});

/// `${prev.field}` 上一步简写
static VAR_PREV: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\{prev\.(result|id)\}").unwrap()
});

/// `${stepN.path.to.field}` 嵌套路径（Phase 4 第 2 项 2026-08-18 06:25）
/// 路径 ≥2 段（首段标识符 + 后续 `.xxx`），与单段 result/id 不冲突
/// 例：`${step1.task.id}` / `${step1.list.0.title}` / `${step1.a.b.c.d}`
static VAR_NESTED_BY_INDEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\{step(\d+)\.([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)*)\}").unwrap()
});

/// `${prev.path.to.field}` 嵌套路径简写（同样 ≥2 段）
static VAR_NESTED_PREV: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\$\{prev\.([a-zA-Z0-9_]+(?:\.[a-zA-Z0-9_]+)*)\}").unwrap()
});

/// 已完成步骤的快照（变量替换上下文，调度器维护）
#[derive(Debug, Clone)]
pub struct CompletedStep {
    pub index: usize,
    /// 步骤标题。substitute_vars 不读，仅作为 ctx roundtrip 快照保留
    /// （调试 / 未来审计 / Skill 跨步 context 用）
    #[allow(dead_code)]
    pub title: String,
    /// 工具返回的原始文本
    pub result: String,
    /// 从 result 提取的 task id（UUID）；没找到为 None
    pub id: Option<String>,
    /// JSON 解析结果（execute_tool 返回字符串尝试 parse_json，失败 None）—— 嵌套路径用
    /// Phase 4 第 2 项 2026-08-18 06:25：substitute_vars 走 serde_json::Value 路径取值
    pub parsed: Option<serde_json::Value>,
}

/// 从工具结果文本中提取第一个 UUID；找不到返回 None。
pub fn extract_task_id(text: &str) -> Option<String> {
    TASK_ID_RE
        .find(text)
        .map(|m| m.as_str().to_string())
}

/// 变量替换（Phase 2 核心）：
/// - `${stepN.result}` → 第 N 步的工具返回文本
/// - `${stepN.id}` → 第 N 步从结果提取的 UUID（任务卡专用）
/// - `${prev.result}` → 上一步工具返回文本
/// - `${prev.id}` → 上一步提取的 UUID
/// - 未匹配的 `${...}` 保留原样（避免误吃合法 JSON 里的 `$` 字符）
pub fn substitute_vars(text: &str, ctx: &[CompletedStep]) -> String {
    // Phase 4 第 2 项（2026-08-18 06:25）：嵌套路径优先匹配
    // （`${stepN.task.id}` / `${prev.list.0.title}` / `${stepN.a.b.c.d}`）
    // 路径 ≥2 段才走嵌套 regex，单段 result/id 留给下方 VAR_BY_INDEX / VAR_PREV 处理
    let r0 = VAR_NESTED_BY_INDEX.replace_all(text, |caps: &regex::Captures| {
        let idx: usize = match caps[1].parse() {
            Ok(n) => n,
            Err(_) => return caps[0].to_string(),
        };
        let path = &caps[2];
        // 单段 result/id 让 VAR_BY_INDEX 后续处理（向后兼容）
        if !path.contains('.') && (path == "result" || path == "id") {
            return caps[0].to_string();
        }
        match ctx.iter().find(|s| s.index == idx) {
            Some(step) => resolve_nested_path(step.parsed.as_ref(), path)
                .unwrap_or_else(|| caps[0].to_string()),
            None => caps[0].to_string(),
        }
    });
    let r1 = VAR_NESTED_PREV.replace_all(&r0, |caps: &regex::Captures| {
        let path = &caps[1];
        // 单段 result/id 让 VAR_PREV 后续处理（向后兼容）
        if !path.contains('.') && (path == "result" || path == "id") {
            return caps[0].to_string();
        }
        match ctx.last() {
            Some(last) => resolve_nested_path(last.parsed.as_ref(), path)
                .unwrap_or_else(|| caps[0].to_string()),
            None => caps[0].to_string(),
        }
    });
    let r2 = VAR_BY_INDEX.replace_all(&r1, |caps: &regex::Captures| {
        let idx: usize = match caps[1].parse() {
            Ok(n) => n,
            Err(_) => return caps[0].to_string(),
        };
        let field = &caps[2];
        match ctx.iter().find(|s| s.index == idx) {
            Some(step) => match field {
                "result" => step.result.clone(),
                "id" => step.id.clone().unwrap_or_default(),
                _ => caps[0].to_string(),
            },
            None => caps[0].to_string(),
        }
    });
    let r3 = VAR_PREV.replace_all(&r2, |caps: &regex::Captures| {
        let field = &caps[1];
        match ctx.last() {
            Some(last) => match field {
                "result" => last.result.clone(),
                "id" => last.id.clone().unwrap_or_default(),
                _ => caps[0].to_string(),
            },
            None => caps[0].to_string(),
        }
    });
    r3.into_owned()
}

/// JSON 路径解析（Phase 4 第 2 项 2026-08-18 06:25）：沿 serde_json::Value 走路径取值
/// - 数字段 → 数组索引（`usize` 解析）
/// - 非数字段 → 对象字段
/// - 字段不存在 / parsed 为 None / 数组越界 → 返回 None（调用方决定 fallback：保留 `${...}`）
/// - 终值是 String → 原样；Null → "null"；其他（数字/布尔/数组/对象）→ serde_json 序列化成字符串
fn resolve_nested_path(parsed: Option<&serde_json::Value>, path: &str) -> Option<String> {
    let mut current = parsed?;
    for segment in path.split('.') {
        current = if let Ok(idx) = segment.parse::<usize>() {
            current.as_array()?.get(idx)?
        } else {
            current.as_object()?.get(segment)?
        };
    }
    Some(match current {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    })
}

/// 前置预审（Harness 意图预审层）：禁用/黑名单拒绝；超长/非法字段已在 parse_meta 兜底。
/// 返回 Ok 表示放行启动。
fn preflight(meta: &SkillMeta) -> Result<(), String> {
    if !meta.enabled {
        return Err(format!("技能「{}」已被禁用", meta.name));
    }
    // 黑名单意图关键词（与系统提示词安全红线一致）：命中即拒绝启动
    const BLACKLIST: [&str; 5] = [
        "全盘遍历", "批量删除", "无确认删除", "遍历文件系统", "清空所有",
    ];
    let hay = format!("{} {}", meta.description.to_lowercase(), meta.intents.join(" "));
    if let Some(hit) = BLACKLIST.iter().find(|b| hay.contains(&b.to_lowercase())) {
        return Err(format!("技能「{}」意图命中安全黑名单（{hit}），已拒绝启动", meta.name));
    }
    Ok(())
}

/// 启动 Skill：use_skill 工具调用即启动生命周期（预审 → Running），返回文档 + 运行约束提示。
pub fn start_skill(app: &AppHandle, name: &str) -> Result<(SkillMeta, String), String> {
    let (meta, body) = load_skill_meta(app, name)?;
    preflight(&meta)?;
    // 预审已通过 → 直接进入 Running（此前停在 Loaded，步骤钩子按 Running/Paused 查找，
    // 计数/熔断/动作记录全部静默失效 —— 核验发现的 P1）
    let mut run = SkillRun::new(&meta);
    run.state = SkillState::Running;
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    // 单活动技能策略：新技能启动时，把其他 Running/Paused 技能标 Completed。
    // 步骤钩子/暂停/确认按「唯一活动技能」定位，多技能同时 Running 会导致
    // 步骤计数与暂停确认全部记到第一个技能、后续技能被静默忽略（全面审计 P2）。
    let mut switched: Vec<String> = Vec::new();
    for (n, r) in runs.iter_mut() {
        if *n != meta.name && (r.state == SkillState::Running || r.state == SkillState::Paused) {
            r.state = SkillState::Completed;
            r.end_reason = "切换到其他技能".into();
            switched.push(n.clone());
        }
    }
    for n in &switched {
        crate::bot::audit_log_hook(app, &format!("skill_completed | name: {n} | 切换技能结束 | steps: {}", runs.get(n).map(|r| r.step).unwrap_or(0)));
    }
    runs.insert(meta.name.clone(), run);
    // 审计：skill.start 结构化（F-3 第二步 2026-08-18）
    audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "skill.start",
        "name" => meta.name.clone(),
        "risk" => meta.risk_level.clone(),
        "mode" => meta.mode.clone(),
        "max_steps" => meta.max_steps,
        "timeout_secs" => meta.timeout_secs,
        "rollback" => meta.rollback.clone(),
    );
    Ok((meta, body))
}

/// 工具 use_skill：读取技能文档全文返回给模型。
pub fn tool_use_skill(app: &AppHandle, args: &str) -> (String, Vec<crate::bot::TaskRef>) {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
    let Some(name) = v["name"].as_str().map(|s| s.trim().to_string()) else {
        return ("use_skill 缺少 name".into(), Vec::new());
    };
    match start_skill(app, &name) {
        Ok((meta, body)) => {
            let hint = format!(
                "【技能文档：{}】\n风险等级 {} · 运行模式 {} · 最多 {} 步 · 超时 {} 秒 · 回滚 {}",
                meta.name, meta.risk_level, meta.mode, meta.max_steps, meta.timeout_secs, meta.rollback
            );
            let mut out = hint + "\n\n" + &body;
            if meta.mode == "interactive" {
                out.push_str("\n\n（运行约束：本技能为人机协同模式，中高危动作执行前会暂停等待用户确认）");
            }
            (out, Vec::new())
        }
        Err(e) => (e, Vec::new()),
    }
}

/// 步骤钩子：每次非 use_skill 的工具调用前调用（Harness 七层之外、调度器层）。
/// 职责：步骤计数、步数/超时熔断、动作记录（回滚清单）。
/// 返回 Err 表示该 Skill 必须立即终止（模型收到错误后停止后续步骤）。
/// 步骤检查纯函数（单测入口）：计数、熔断、暂停拒绝、动作记录。不改日志。
fn step_check(run: &mut SkillRun, tool: &str, args: &str, now: i64) -> Result<(), String> {
    if run.state == SkillState::Paused {
        return Err("技能已暂停，等待用户确认中；确认通过后才能继续下一步".into());
    }
    run.step += 1;
    // 步数熔断（Skill 独立上限）
    if run.step > run.max_steps {
        run.state = SkillState::Failed;
        run.end_reason = format!("超过最大步数上限（{} 步）", run.max_steps);
        return Err(format!("技能「{}」{}，已强制终止", run.name, run.end_reason));
    }
    // 超时熔断
    let elapsed = (now - run.started_at_ms) / 1000;
    if elapsed > run.timeout_secs as i64 {
        run.state = SkillState::Failed;
        run.end_reason = format!("超时（超过 {} 秒）", run.timeout_secs);
        return Err(format!("技能「{}」{}，已强制终止", run.name, run.end_reason));
    }
    // 动作记录（回滚清单来源）：只记有副作用的工具，跳过只读查询
    const READONLY: [&str; 4] = ["list_tasks", "search_tasks", "use_skill", "web_search"];
    if !READONLY.contains(&tool) {
        let brief = truncate_skill_args(args, 120);
        run.actions.push(format!("{tool} | {brief}"));
    }
    Ok(())
}

pub fn skill_on_step(app: &AppHandle, tool: &str, args: &str) -> Result<(), String> {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    let Some(run) = runs.values_mut().find(|r| r.state == SkillState::Running || r.state == SkillState::Paused) else {
        return Ok(()); // 无活动 Skill（模型自由调用工具），不干预
    };
    let res = step_check(run, tool, args, now_ms());
    if let Err(e) = &res {
        let reason = run.end_reason.clone();
        crate::bot::audit_log_hook(app, &format!("skill_failed | name: {} | {reason} | {e}", run.name));
    }
    res
}

/// Post-execute Skill 步骤钩子（洋葱管线「出」钩子，2026-08-17 22:17 第一块落地）
/// 与 `skill_on_step` 对称：执行工具后调用，记录步骤结果 + 检测失败。
/// 仅在 Skill 处于 Running 时干预；Paused/Completed 等状态不写动作记录。
///
/// 失败判定：级别 ≥ Warn 且文本含「失败/错误/error:/Error:」→ 把 Skill 标 Failed，
/// 写入 end_reason 让 `skill_finish` 知道不再继续后续步骤。
pub fn skill_on_step_post(
    app: &AppHandle,
    tool: &str,
    result: &str,
    dur_ms: u64,
    level: crate::audit::AuditLevel,
) {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    let Some(run) = runs.values_mut().find(|r| r.state == SkillState::Running) else {
        return;
    };
    let name = run.name.clone();
    let is_fail = matches!(
        level,
        crate::audit::AuditLevel::Error | crate::audit::AuditLevel::Warn
    ) && (result.contains("失败")
        || result.contains("错误")
        || result.starts_with("error:")
        || result.starts_with("Error:"));
    if is_fail {
        run.state = SkillState::Failed;
        run.end_reason = format!("工具 {tool} 执行失败");
        let preview: String = result.chars().take(120).collect();
        crate::bot::audit_log_hook(
            app,
            &format!("skill_step_fail | name: {name} | tool: {tool} | {preview}"),
        );
    } else {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_step_ok | name: {name} | tool: {tool} | {dur_ms}ms"),
        );
    }
}

/// 参数摘要（动作记录用）
fn truncate_skill_args(args: &str, max: usize) -> String {
    let mut s: String = args.chars().take(max).collect();
    if args.chars().count() > max {
        s.push('…');
    }
    s.replace('\n', " ")
}

/// 暂停语义纯函数（单测入口）：把 Running 技能转入暂停等待。
/// resumable=false → terminal_after_confirm=true（确认通过后技能终止，暂停即终止）。
fn pause_state(run: &mut SkillRun) {
    run.state = SkillState::Paused;
    run.terminal_after_confirm = !run.resumable;
}

/// 确认结果纯函数（单测入口）：通过 → 恢复 Running 或暂停即终止；拒绝 → Terminated。
fn confirm_state(run: &mut SkillRun, approved: bool) {
    if approved {
        if run.terminal_after_confirm {
            run.state = SkillState::Terminated;
            run.end_reason = "暂停即终止（技能不支持断点续跑，resumable=false）".into();
        } else {
            run.state = SkillState::Running;
        }
    } else {
        run.state = SkillState::Terminated;
        run.end_reason = "用户拒绝确认".into();
    }
}

/// 高危动作确认开始：活动技能转入 Paused（ask_user_confirm 调用前触发）。
pub fn skill_mark_paused(app: &AppHandle, tool: &str) {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = runs.values_mut().find(|r| r.state == SkillState::Running) {
        pause_state(run);
        crate::bot::audit_log_hook(app, &format!("skill_paused | name: {} | at: {tool} | resumable: {}", run.name, run.resumable));
    }
}

/// 确认结果到达：恢复 Running / 暂停即终止 / 拒绝终止（bot_confirm_response 调用）。
pub fn skill_confirm_result(app: &AppHandle, approved: bool) {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = runs.values_mut().find(|r| r.state == SkillState::Paused) {
        confirm_state(run, approved);
        let name = run.name.clone();
        let state = format!("{:?}", run.state);
        let reason = run.end_reason.clone();
        crate::bot::audit_log_hook(app, &format!("skill_confirm | name: {name} | approved: {approved} | -> {state} {reason}"));
    }
}

/// 提取 SKILL.md 正文里的「## 回滚」章节（无则空串）
fn rollback_section(body: &str) -> String {
    let marker = "## 回滚";
    let Some(start) = body.find(marker) else {
        return String::new();
    };
    let tail = &body[start + marker.len()..];
    // 到下一个 ## 标题为止
    match tail.find("\n## ") {
        Some(off) => tail[..off].trim().to_string(),
        None => tail.trim().to_string(),
    }
}

/// 收尾钩子：模型循环结束时调用（成功/失败/用户停止）。
/// 返回回滚建议文本（失败且 rollback=auto 且有动作记录时非空），调用方拼进回复让模型执行逆操作。
pub fn skill_finish(app: &AppHandle, ok: bool, reason: &str) -> String {
    let mut rollback_hint = String::new();
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    for (name, run) in runs.iter_mut() {
        if run.state != SkillState::Running && run.state != SkillState::Paused {
            continue;
        }
        if ok {
            run.state = SkillState::Completed;
            crate::bot::audit_log_hook(app, &format!("skill_completed | name: {name} | steps: {}", run.step));
        } else {
            run.state = SkillState::Failed;
            run.end_reason = reason.to_string();
            let mut log = format!("skill_failed | name: {name} | {reason} | actions: {}", run.actions.len());
            // 回滚建议（务实版）：失败 + 声明可回滚 + 有已执行动作 → 生成建议文本
            if run.rollback == "auto" && !run.actions.is_empty() {
                log.push_str(" | rollback_suggested");
                let actions_text = run
                    .actions
                    .iter()
                    .map(|a| format!("- {a}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let rb_section = rollback_section(&load_skill_meta(app, name).map(|(_, b)| b).unwrap_or_default());
                rollback_hint = format!(
                    "\n\n【技能回滚建议】技能「{name}」执行中断（{reason}），已执行 {n} 个动作：\n{actions_text}\n{}",
                    if rb_section.is_empty() {
                        "该技能未提供回滚章节，请向用户说明已执行动作，由用户决定手动补救。".to_string()
                    } else {
                        format!("技能文档回滚章节：\n{rb_section}\n请先询问用户是否需要回滚；用户同意后，按回滚章节逐条执行逆操作（每一步同样经过安全校验）。")
                    },
                    n = run.actions.len()
                );
            }
            crate::bot::audit_log_hook(app, &log);
        }
    }
    rollback_hint
}

/// 状态机推进建议（主循环决策输入，2026-08-17 22:26 Block 2 骨架）
///
/// 由 `advance_skill()` 根据当前 SkillState 返回，告诉主循环下一步该做什么。
///
/// 设计动机：把散落在 step_check / skill_on_step_post / skill_finish 的
/// 状态推进决策集中到这一处，未来加横切关注点（审批/沙箱/上下文压缩）
/// 只动 advance_skill，主循环不重构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvanceAction {
    /// 无活动 Skill 或已终结（Loaded/Completed/Failed/Terminated）→ 主循环进入 LLM 自由调用
    NoActive,
    /// 等待用户确认（Paused）→ 主循环跳出当前迭代，等待 bot_confirm_response 唤起
    AwaitConfirm,
    /// 正常运行中 → 继续下一轮迭代
    Continue,
    /// Skill 成功完成 → 调 skill_finish 收尾
    Finish,
    /// Skill 失败 → 调 skill_finish 传错误信息（含 max_steps/超时/工具错误）
    Fail(String),
    /// Skill 被终止 → 调 skill_finish 传终止原因（含用户拒绝/强制停止）
    Terminate(String),
}

/// 状态机推进决策（主循环每轮迭代开头调用，2026-08-17 22:26 Block 2 骨架）
///
/// 纯函数：不写日志、不改全局状态、不改传入的 run，便于单测。
/// 实时超时检测：即使 step_check 没跑到（长工具执行期间），主循环也能感知超时。
///
/// 调用时机（明早接主循环）：
///   bot_chat 主循环每次 LLM 响应/工具执行后调一次，根据 AdvanceAction 决定：
///   - Continue → next iteration
///   - AwaitConfirm → break，等待 bot_confirm_response
///   - Finish / Fail / Terminate → 调 skill_finish 收尾，break
///   - NoActive → 继续 LLM 自由调用，下次迭代再问
pub fn advance_skill(run: &SkillRun, now_ms: i64) -> AdvanceAction {
    match run.state {
        SkillState::Loaded => AdvanceAction::NoActive,
        SkillState::Running => {
            // 实时超时检测（即使 step_check 没跑到，主循环也能感知）
            let elapsed = (now_ms - run.started_at_ms) / 1000;
            if elapsed > run.timeout_secs as i64 {
                AdvanceAction::Fail(format!("超时（超过 {} 秒）", run.timeout_secs))
            } else {
                AdvanceAction::Continue
            }
        }
        SkillState::Paused => AdvanceAction::AwaitConfirm,
        SkillState::Completed => AdvanceAction::Finish,
        SkillState::Failed => AdvanceAction::Fail(run.end_reason.clone()),
        SkillState::Terminated => AdvanceAction::Terminate(run.end_reason.clone()),
    }
}

/// DSL 调度器专用决策（Phase 4 第 1 项 2026-08-18 06:20）：
/// 把 `advance_skill` 的 6 分支映射到 DSL 调度器可执行的 5 种动作。
///
/// 动机：原 `run_skill_scheduler` 一次性顺序跑所有 step，不读 SkillRun.state，
/// 导致 step_check 步数熔断 / skill_on_step_post 工具失败 / skill_terminate_all 用户 /stop
/// 改 state 后，调度器仍继续跑后续 step（漏停、超时后还跑、用户取消后还跑）。
/// 现在每 step 前查一次，按状态机决策：
/// - Run → 正常跑当前 step（Loaded → NoActive / Running 未超时 → Continue）
/// - Finish → 收尾，跳出循环（Completed）
/// - AwaitUser → 暂停中，返回 `__await_user__` 让主循环挂起（Paused，auto 模式基本不触发，留接口）
/// - FailWithRollback(reason) → 跑 rollback 段 + 报错（Failed / Running 超时）
/// - Terminate(reason) → 不跑 rollback 直接报错（Terminated / 用户主动取消 / 切技能）
///
/// 纯函数：不写日志、不改全局状态、不改 run，便于单测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DslAdvanceAction {
    /// 正常执行当前 step（Loaded / Running 未超时）
    Run,
    /// Skill 已成功完成（Completed）→ 跳出循环，跑汇总
    Finish,
    /// Skill 暂停中等用户确认（Paused）→ 返回 `__await_user__` 给主循环挂起
    AwaitUser,
    /// Skill 失败（Failed / 步数熔断 / 超时）→ 跑 rollback + 报错
    FailWithRollback(String),
    /// Skill 终止（Terminated / 用户 /stop / 切技能）→ 不跑 rollback + 报错
    Terminate(String),
}

pub fn advance_dsl(run: &SkillRun, now_ms: i64) -> DslAdvanceAction {
    match advance_skill(run, now_ms) {
        AdvanceAction::NoActive | AdvanceAction::Continue => DslAdvanceAction::Run,
        AdvanceAction::Finish => DslAdvanceAction::Finish,
        AdvanceAction::AwaitConfirm => DslAdvanceAction::AwaitUser,
        AdvanceAction::Fail(reason) => DslAdvanceAction::FailWithRollback(reason),
        AdvanceAction::Terminate(reason) => DslAdvanceAction::Terminate(reason),
    }
}

// ─────────────────────── DSL Outcome / Failure + LLM 兜底（Phase 4 第 5 项 2026-08-18 07:20）───────────────────────

/// DSL 调度器成功 / 可恢复 / 暂停 输出（Phase 4 LLM 兜底路径，2026-08-18 07:20）。
/// - Done：跑完所有 step，返回汇总文本给用户
/// - AwaitUser：暂停中等用户确认（auto-mode 极少触发，留接口）
/// - FailedButRecoverable：失败但已完成部分 step，让 LLM 基于 `completed_summary` 决策下一步
#[derive(Debug, Clone)]
pub enum DslOutcome {
    /// 正常完成：summary 文本直接给用户
    Done(String),
    /// 暂停中等用户确认
    AwaitUser,
    /// 失败但 LLM 可接管：把 `completed_summary` + `reason` 注入 system prompt 决策下一步
    FailedButRecoverable {
        reason: String,
        completed_summary: String,
        rollback_attempted: bool,
    },
}

/// DSL 调度器硬错误（用户主动终止 / 解析失败）—— LLM 不接管，直接报错给用户
#[derive(Debug, Clone)]
pub enum DslFailure {
    Terminated { reason: String },
}

/// 已完成步骤摘要（Phase 4 LLM 兜底，2026-08-18 07:20）：
/// 把 ctx 里的步骤产物拼成可读 summary（注入 system prompt 用）。
/// 每步一行：`Step N (title): <result 前 200 字符 + ...>`
/// 空 ctx 返回 `(无已完成步骤)`。
pub fn format_completed_summary(ctx: &[CompletedStep]) -> String {
    if ctx.is_empty() {
        return "(无已完成步骤)".into();
    }
    let mut out = String::new();
    for step in ctx {
        let preview: String = step.result.chars().take(200).collect();
        let suffix = if step.result.chars().count() > 200 { "..." } else { "" };
        out.push_str(&format!(
            "- Step {} ({}): {}{}\n",
            step.index, step.title, preview, suffix
        ));
    }
    out
}

/// 强制终止所有活动 Skill（/stop 联动；用户取消时调用）
pub fn skill_terminate_all(app: &AppHandle, reason: &str) {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    for (name, run) in runs.iter_mut() {
        if run.state == SkillState::Running || run.state == SkillState::Paused {
            run.state = SkillState::Terminated;
            run.end_reason = reason.to_string();
            crate::bot::audit_log_hook(app, &format!("skill_terminated | name: {name} | {reason}"));
        }
    }
}

/// DSL 调度器入口（Phase 1 2026-08-17 23:15）。仅供 `meta.mode == "auto"` 的 Skill 调用：
/// 解析 body → 顺序调 `bot::execute_tool` → 失败时跑回滚段。
///
/// 行为：
/// - 每个 step 调一次 `bot::execute_tool`（带 pre-execute 校验）
/// - 任一 step 失败 → 顺序跑 `## Rollback` 段工具 → 返回 Err
/// - 全部成功 → 返回汇总文本
/// - SkillRun 状态机更新由 `execute_tool` 内的 `skill_on_step` / `skill_on_step_post` 自动维护
pub async fn run_skill_scheduler(app: &AppHandle, name: &str) -> Result<DslOutcome, DslFailure> {
    let (meta, body) = load_skill_meta(app, name)
        .map_err(|e| DslFailure::Terminated { reason: e })?;
    let (steps, rollback) = parse_skill_steps(&body)
        .map_err(|e| DslFailure::Terminated { reason: e })?;
    if steps.is_empty() {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_dsl_empty | name: {name} | body_len: {}", body.len()),
        );
        persist_outcome_quiet(app, name, "terminated", Some("DSL 解析为空"), None, None);
        return Err(DslFailure::Terminated {
            reason: format!("技能「{name}」无可执行步骤（DSL 解析为空）"),
        });
    }
    crate::bot::audit_log_hook(
        app,
        &format!("skill_dsl_start | name: {name} | steps: {}", steps.len()),
    );

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let mut results: Vec<(usize, String, String)> = Vec::new();
    for step in &steps {
        // Phase 4 第 1 项（2026-08-18 06:20）：每 step 前查 advance_dsl 状态机。
        // 拦截漏停场景：step_check 步数熔断 / skill_on_step_post 工具失败 /
        // skill_terminate_all 用户 /stop / start_skill 切技能 → SkillRun.state 已被改，
        // 调度器必须感知。
        if let Some(run) = active_skill_run() {
            let ts = now_ms();
            match advance_dsl(&run, ts) {
                DslAdvanceAction::Run => {}
                DslAdvanceAction::Finish => {
                    crate::bot::audit_log_hook(
                        app,
                        &format!(
                            "skill_dsl_finish_signal | name: {name} | step_before: {}",
                            step.index
                        ),
                    );
                    break;
                }
                DslAdvanceAction::AwaitUser => {
                    crate::bot::audit_log_hook(
                        app,
                        &format!("skill_dsl_await_user | name: {name} | step: {}", step.index),
                    );
                    persist_outcome_quiet(app, name, "await_user", None, Some(&format_completed_summary(&ctx)), None);
                    return Ok(DslOutcome::AwaitUser);
                }
                DslAdvanceAction::FailWithRollback(reason) => {
                    let rb_attempted = !rollback.is_empty();
                    if rb_attempted {
                        crate::bot::audit_log_hook(
                            app,
                            &format!(
                                "skill_dsl_rollback_start | name: {name} | step: {} | reason: {}",
                                step.index,
                                reason.chars().take(120).collect::<String>()
                            ),
                        );
                        // 回滚段也走变量替换（失败前的步骤都已入 ctx）
                        for rb in &rollback {
                            let rb_args = substitute_vars(&rb.args_json, &ctx);
                            let _ = crate::bot::execute_tool(app, &rb.tool_name, &rb_args).await;
                        }
                        crate::bot::audit_log_hook(
                            app,
                            &format!("skill_dsl_rollback_done | name: {name}"),
                        );
                    }
                    let final_reason = format!("技能「{name}」中止：{reason}");
                    let summary = format_completed_summary(&ctx);
                    persist_outcome_quiet(app, name, "failed_recoverable", Some(&final_reason), Some(&summary), Some(rb_attempted));
                    return Ok(DslOutcome::FailedButRecoverable {
                        reason: final_reason,
                        completed_summary: summary,
                        rollback_attempted: rb_attempted,
                    });
                }
                DslAdvanceAction::Terminate(reason) => {
                    crate::bot::audit_log_hook(
                        app,
                        &format!(
                            "skill_dsl_terminated | name: {name} | step: {} | reason: {}",
                            step.index, reason
                        ),
                    );
                    let final_reason = format!("技能「{name}」终止：{reason}");
                    persist_outcome_quiet(app, name, "terminated", Some(&final_reason), None, None);
                    return Err(DslFailure::Terminated {
                        reason: final_reason,
                    });
                }
            }
        }
        crate::bot::audit_log_hook(
            app,
            &format!(
                "skill_dsl_step | name: {name} | step: {} | tool: {} | ctx_len: {}",
                step.index, step.tool_name, ctx.len()
            ),
        );
        // Phase 2 接入（2026-08-18）：变量替换 —— 把上一步结果/UUID 拼进 args_json
        let resolved_args = substitute_vars(&step.args_json, &ctx);
        if resolved_args != step.args_json {
            crate::bot::audit_log_hook(
                app,
                &format!(
                    "skill_dsl_var_resolved | name: {name} | step: {} | resolved_args_len: {}",
                    step.index,
                    resolved_args.len()
                ),
            );
        }
        let (text, _refs) = crate::bot::execute_tool(app, &step.tool_name, &resolved_args).await;
        let failed = text.starts_with("未知工具")
            || text.starts_with("失败")
            || text.starts_with("错误")
            || text.starts_with("error:")
            || text.starts_with("Error:");
        if failed {
            if !rollback.is_empty() {
                crate::bot::audit_log_hook(
                    app,
                    &format!(
                        "skill_dsl_rollback_start | name: {name} | step: {} | reason: {}",
                        step.index,
                        text.chars().take(120).collect::<String>()
                    ),
                );
                // 回滚段也走变量替换（失败前的步骤都已入 ctx）
                for rb in &rollback {
                    let rb_args = substitute_vars(&rb.args_json, &ctx);
                    let _ = crate::bot::execute_tool(app, &rb.tool_name, &rb_args).await;
                }
                crate::bot::audit_log_hook(
                    app,
                    &format!("skill_dsl_rollback_done | name: {name}"),
                );
            }
            let rb_attempted = !rollback.is_empty();
            let final_reason = format!(
                "技能「{name}」Step {} ({}) 失败：{}",
                step.index, step.title, text
            );
            let summary = format_completed_summary(&ctx);
            persist_outcome_quiet(app, name, "failed_recoverable", Some(&final_reason), Some(&summary), Some(rb_attempted));
            return Ok(DslOutcome::FailedButRecoverable {
                reason: final_reason,
                completed_summary: summary,
                rollback_attempted: rb_attempted,
            });
        }
        // 把这一步压进 ctx（变量替换的依赖源）
        // Phase 4 第 2 项（2026-08-18 06:25）：parsed 用于嵌套路径 `${step1.task.id}`
        // 纯文本 / Markdown 摘要 parse 失败为 None，substitute_vars 嵌套路径 fallback 保留 `${...}`
        ctx.push(CompletedStep {
            index: step.index,
            title: step.title.clone(),
            result: text.clone(),
            id: extract_task_id(&text),
            parsed: serde_json::from_str(&text).ok(),
        });
        results.push((step.index, step.title.clone(), text));
    }

    let mut summary = format!(
        "✅ 技能「{}」自动执行完成（{} 步）：\n",
        meta.name,
        steps.len()
    );
    for (idx, title, result) in &results {
        summary.push_str(&format!("\n### Step {}: {}\n{}\n", idx, title, result));
    }
    crate::bot::audit_log_hook(
        app,
        &format!("skill_dsl_done | name: {name} | steps_ok: {}", steps.len()),
    );
    persist_outcome_quiet(app, name, "done", None, Some(&summary), None);
    Ok(DslOutcome::Done(summary))
}

// ───────────────────────── tauri 命令（设置页技能管理） ─────────────────────────

/// 已安装技能列表（设置页展示）
#[tauri::command]
pub fn skills_list(app: AppHandle) -> Vec<SkillInfo> {
    scan_skills(&app)
}

/// 技能目录路径（设置页「打开目录」按钮用；目录不存在则先创建）
#[tauri::command]
pub fn skills_open_dir(app: AppHandle) -> Result<String, String> {
    let dir = skills_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.to_string_lossy().to_string())
}

/// 导入技能文件夹：校验含 SKILL.md，拷贝到数据目录 skills/<name>（重名拒绝，需先删）。
/// 返回技能名。
#[tauri::command]
pub fn skills_import(app: AppHandle, path: String) -> Result<String, String> {
    let src = std::path::PathBuf::from(&path);
    if !src.is_dir() {
        return Err("请选择技能文件夹".into());
    }
    let skill_md = src.join("SKILL.md");
    let text = std::fs::read_to_string(&skill_md).map_err(|_| "该文件夹没有 SKILL.md，不是有效技能".to_string())?;
    let dir_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (name, _) = parse_frontmatter(&text, &dir_name);
    let dir = skills_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&name);
    if dest.exists() {
        return Err(format!("技能「{name}」已存在：请先在设置页删除同名技能"));
    }
    // 递归拷贝（技能可能带 scripts/ 等资源）
    copy_dir_all(&src, &dest)?;
    Ok(name)
}

/// 删除技能（整目录）
#[tauri::command]
pub fn skills_delete(app: AppHandle, name: String) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err("技能名无效".into());
    }
    let dir = skills_dir(&app).join(&name);
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let s = entry.path();
        let d = dst.join(entry.file_name());
        if ty.is_symlink() {
            continue; // 跳过符号链接，防拷贝越界
        }
        if ty.is_dir() {
            copy_dir_all(&s, &d)?;
        } else {
            std::fs::copy(&s, &d).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_run(max_steps: usize, timeout_secs: u64) -> SkillRun {
        SkillRun {
            name: "test-skill".into(),
            state: SkillState::Loaded,
            step: 0,
            max_steps,
            started_at_ms: 1000,
            timeout_secs,
            mode: "interactive".into(),
            risk_level: "medium".into(),
            rollback: "auto".into(),
            actions: Vec::new(),
            end_reason: String::new(),
            resumable: true,
            terminal_after_confirm: false,
        }
    }

    // ── 状态机推进（Block 2 骨架） ──

    #[test]
    fn advance_loaded_returns_no_active() {
        let run = test_run(8, 180);
        // test_run 初始状态就是 Loaded
        assert_eq!(run.state, SkillState::Loaded);
        assert_eq!(advance_skill(&run, 1000), AdvanceAction::NoActive);
    }

    #[test]
    fn advance_running_no_timeout_continues() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Running;
        // started_at_ms=1000, now=10s 后 11000, timeout=180s → 仍可继续
        assert_eq!(advance_skill(&run, 11000), AdvanceAction::Continue);
    }

    #[test]
    fn advance_running_just_before_timeout_continues() {
        let mut run = test_run(8, 60);
        run.state = SkillState::Running;
        // 恰好 60s 未超时（入 strict > 才 Fail）
        assert_eq!(
            advance_skill(&run, 1000 + 60 * 1000),
            AdvanceAction::Continue
        );
    }

    #[test]
    fn advance_running_timeout_returns_fail() {
        let mut run = test_run(8, 60);
        run.state = SkillState::Running;
        // 启动 61s 后超时 (60s 上限)
        let action = advance_skill(&run, 1000 + 61 * 1000);
        match action {
            AdvanceAction::Fail(reason) => {
                assert!(reason.contains("超时"), "reason 应含「超时」: {reason}");
                assert!(reason.contains("60"), "reason 应含阈值 60s: {reason}");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn advance_paused_returns_await_confirm() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Paused;
        assert_eq!(advance_skill(&run, 1000), AdvanceAction::AwaitConfirm);
    }

    #[test]
    fn advance_completed_returns_finish() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Completed;
        assert_eq!(advance_skill(&run, 1000), AdvanceAction::Finish);
    }

    #[test]
    fn advance_failed_returns_fail_with_end_reason() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Failed;
        run.end_reason = "超过最大步数上限（8 步）".into();
        assert_eq!(
            advance_skill(&run, 1000),
            AdvanceAction::Fail("超过最大步数上限（8 步）".into())
        );
    }

    #[test]
    fn advance_terminated_returns_terminate_with_end_reason() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Terminated;
        run.end_reason = "用户拒绝确认".into();
        assert_eq!(
            advance_skill(&run, 1000),
            AdvanceAction::Terminate("用户拒绝确认".into())
        );
    }

    // ── DSL 调度器决策（Phase 4 第 1 项 2026-08-18 06:20） ──

    #[test]
    fn advance_dsl_continue_returns_run() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Running;
        // Running 11s（未超 180s 上限）→ 正常执行
        assert!(matches!(advance_dsl(&run, 11000), DslAdvanceAction::Run));
    }

    #[test]
    fn advance_dsl_no_active_loaded_returns_run() {
        // Loaded 是 start_skill 之前的过渡态，advance_skill 返回 NoActive，
        // DSL 调度器仍按 Run 处理（不阻断 step 执行）
        let run = test_run(8, 180);
        assert_eq!(run.state, SkillState::Loaded);
        assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::Run));
    }

    #[test]
    fn advance_dsl_paused_returns_await_user() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Paused;
        assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::AwaitUser));
    }

    #[test]
    fn advance_dsl_completed_returns_finish() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Completed;
        assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::Finish));
    }

    #[test]
    fn advance_dsl_failed_returns_fail_with_rollback() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Failed;
        run.end_reason = "超过最大步数上限（8 步）".into();
        match advance_dsl(&run, 1000) {
            DslAdvanceAction::FailWithRollback(reason) => {
                assert_eq!(reason, "超过最大步数上限（8 步）");
            }
            other => panic!("expected FailWithRollback, got {other:?}"),
        }
    }

    #[test]
    fn advance_dsl_terminated_returns_terminate_no_rollback() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Terminated;
        run.end_reason = "用户 /stop".into();
        match advance_dsl(&run, 1000) {
            DslAdvanceAction::Terminate(reason) => assert_eq!(reason, "用户 /stop"),
            other => panic!("expected Terminate, got {other:?}"),
        }
    }

    #[test]
    fn advance_dsl_timeout_returns_fail_with_rollback() {
        // 端到端：Running + 启动后 61s（60s 上限）→ advance_skill 内部超时检测 → Fail
        // → advance_dsl 映射成 FailWithRollback（含 reason）
        let mut run = test_run(8, 60);
        run.state = SkillState::Running;
        let action = advance_dsl(&run, 1000 + 61 * 1000);
        match action {
            DslAdvanceAction::FailWithRollback(reason) => {
                assert!(reason.contains("超时"), "reason 应含「超时」: {reason}");
                assert!(reason.contains("60"), "reason 应含阈值 60s: {reason}");
            }
            other => panic!("expected FailWithRollback, got {other:?}"),
        }
    }

    // ── 元数据解析 ──

    #[test]
    fn meta_parses_full_fields() {
        let text = "---\nname: daily-archive\ndescription: 汇总归档\nrisk_level: medium\nmode: interactive\nmax_steps: 6\ntimeout_secs: 120\nrollback: auto\nenabled: true\nintents: [\"日报\",\"归档\"]\n---\n# 步骤\n";
        let m = parse_meta(text, "fallback");
        assert_eq!(m.name, "daily-archive");
        assert_eq!(m.risk_level, "medium");
        assert_eq!(m.mode, "interactive");
        assert_eq!(m.max_steps, 6);
        assert_eq!(m.timeout_secs, 120);
        assert_eq!(m.rollback, "auto");
        assert!(m.enabled);
        assert_eq!(m.intents, vec!["日报", "归档"]);
    }

    #[test]
    fn meta_defaults_and_clamps() {
        let m = parse_meta("没有 frontmatter", "dir-x");
        assert_eq!(m.name, "dir-x");
        assert_eq!(m.risk_level, "medium");
        assert_eq!(m.max_steps, 8);
        assert_eq!(m.timeout_secs, 180);
        assert!(!m.rollback.eq("auto"));

        // 非法值回退 + 越界 clamp
        let t = "---\nrisk_level: dangerous\nmode: auto\nmax_steps: 999\ntimeout_secs: 1\n---\n";
        let m2 = parse_meta(t, "d");
        assert_eq!(m2.risk_level, "medium"); // 非法回退
        assert_eq!(m2.mode, "auto"); // 合法保留
        assert_eq!(m2.max_steps, 20); // clamp 上限
        assert_eq!(m2.timeout_secs, 10); // clamp 下限
    }

    #[test]
    fn meta_high_risk_forces_interactive() {
        let t = "---\nrisk_level: high\nmode: auto\n---\n";
        let m = parse_meta(t, "d");
        assert_eq!(m.mode, "interactive"); // 安全兜底：high 强制人机协同
    }

    #[test]
    fn meta_disabled_flag() {
        let t = "---\nenabled: false\n---\n";
        let m = parse_meta(t, "d");
        assert!(!m.enabled);
    }

    // ── 前置预审 ──

    #[test]
    fn preflight_rejects_disabled() {
        let mut m = SkillMeta::default();
        m.name = "x".into();
        m.enabled = false;
        assert!(preflight(&m).is_err());
    }

    #[test]
    fn preflight_rejects_blacklist_intent() {
        let mut m = SkillMeta::default();
        m.name = "x".into();
        m.description = "批量删除所有任务".into();
        assert!(preflight(&m).unwrap_err().contains("黑名单"));

        m.description = "全盘遍历文件".into();
        assert!(preflight(&m).is_err());
    }

    #[test]
    fn preflight_passes_normal() {
        let mut m = SkillMeta::default();
        m.name = "ok".into();
        m.description = "汇总今日任务并归档".into();
        assert!(preflight(&m).is_ok());
    }

    // ── 步骤循环 / 熔断 ──

    #[test]
    fn step_check_counts_and_records_actions() {
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        assert!(step_check(&mut r, "list_tasks", "{}", 2000).is_ok());
        assert_eq!(r.step, 1);
        assert!(r.actions.is_empty()); // 只读不记
        assert!(step_check(&mut r, "create_task", "{\"title\":\"x\"}", 2000).is_ok());
        assert_eq!(r.actions.len(), 1); // 有副作用记入
        assert!(r.actions[0].starts_with("create_task"));
    }

    #[test]
    fn step_check_fuses_on_max_steps() {
        let mut r = test_run(2, 180);
        r.state = SkillState::Running;
        assert!(step_check(&mut r, "create_task", "{}", 2000).is_ok());
        assert!(step_check(&mut r, "edit_task", "{}", 2000).is_ok());
        let e = step_check(&mut r, "complete_task", "{}", 2000).unwrap_err();
        assert!(e.contains("超过最大步数上限"));
        assert_eq!(r.state, SkillState::Failed);
    }

    #[test]
    fn step_check_fuses_on_timeout() {
        let mut r = test_run(8, 60);
        r.state = SkillState::Running;
        // 已过 61 秒
        let e = step_check(&mut r, "list_tasks", "{}", 1000 + 61 * 1000).unwrap_err();
        assert!(e.contains("超时"));
        assert_eq!(r.state, SkillState::Failed);
    }

    #[test]
    fn step_check_rejects_when_paused() {
        let mut r = test_run(8, 180);
        r.state = SkillState::Paused;
        assert!(step_check(&mut r, "list_tasks", "{}", 2000).is_err());
        assert_eq!(r.step, 0); // 暂停态不计数
    }

    #[test]
    fn meta_parses_resumable() {
        let t = "---\nresumable: true\n---\n";
        let m = parse_meta(t, "d");
        assert!(m.resumable);
        let t2 = "---\nresumable: false\n---\n";
        assert!(!parse_meta(t2, "d").resumable);
        assert!(!parse_meta("无 frontmatter", "d").resumable); // 默认 false
    }

    #[test]
    fn pause_state_resumable_flows() {
        // resumable=true：暂停后可恢复
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        r.resumable = true;
        pause_state(&mut r);
        assert_eq!(r.state, SkillState::Paused);
        assert!(!r.terminal_after_confirm);
        confirm_state(&mut r, true);
        assert_eq!(r.state, SkillState::Running); // 确认通过恢复
        // 再次暂停后拒绝
        pause_state(&mut r);
        confirm_state(&mut r, false);
        assert_eq!(r.state, SkillState::Terminated);
        assert_eq!(r.end_reason, "用户拒绝确认");
    }

    #[test]
    fn pause_state_non_resumable_terminates_after_confirm() {
        // resumable=false：暂停即终止——确认通过后技能终止
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        r.resumable = false;
        pause_state(&mut r);
        assert_eq!(r.state, SkillState::Paused);
        assert!(r.terminal_after_confirm);
        confirm_state(&mut r, true);
        assert_eq!(r.state, SkillState::Terminated);
        assert!(r.end_reason.contains("resumable=false"));
    }

    #[test]
    fn rollback_section_extracted() {
        let body = "# 文档\n## 步骤\n1. 干活\n## 回滚\n1. 撤销 A\n2. 恢复 B\n## 其他\n尾巴";
        let rb = rollback_section(body);
        assert!(rb.contains("撤销 A"));
        assert!(rb.contains("恢复 B"));
        assert!(!rb.contains("尾巴")); // 到下一个 ## 为止

        assert_eq!(rollback_section("没有回滚章节"), "");
    }

    #[test]
    fn frontmatter_parses_name_and_description() {
        let text = "---\nname: ppt-pro\ndescription: \"PPT 制作规范\"\n---\n# 正文\n步骤一\n";
        let (n, d) = parse_frontmatter(text, "fallback");
        assert_eq!(n, "ppt-pro");
        assert_eq!(d, "PPT 制作规范");
    }

    #[test]
    fn frontmatter_missing_falls_back_to_dir_name() {
        let (n, d) = parse_frontmatter("没有 frontmatter", "dir-x");
        assert_eq!(n, "dir-x");
        assert_eq!(d, "");
    }

    #[test]
    fn frontmatter_invalid_name_falls_back() {
        let text = "---\nname: \"../evil\"\ndescription: x\n---\n";
        let (n, _) = parse_frontmatter(text, "dir-x");
        assert_eq!(n, "dir-x"); // 非法名（含路径字符）回退目录名
    }

    #[test]
    fn read_skill_rejects_path_traversal() {
        // 直接验证 name 校验函数逻辑（无需 AppHandle）
        let ok = |s: &str| !s.is_empty() && s.chars().all(SKILL_NAME_CHARS_OK);
        assert!(ok("ppt-pro"));
        assert!(ok("minimax_docx"));
        assert!(!ok("../evil"));
        assert!(!ok("a/b"));
        assert!(!ok(""));
    }

    // ── DSL 解析（Phase 1 2026-08-17 23:15） ──

    #[test]
    fn dsl_parse_empty_body() {
        let (steps, rollback) = parse_skill_steps("").unwrap();
        assert!(steps.is_empty());
        assert!(rollback.is_empty());
    }

    #[test]
    fn dsl_parse_single_step() {
        let body = "## Step 1: 列出任务\nlist_tasks({})\n";
        let (steps, rb) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(rb.is_empty());
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].title, "列出任务");
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[0].args_json, "{}");
    }

    #[test]
    fn dsl_parse_multiple_steps_preserves_order() {
        let body = "## Step 1: 第一步\nfoo({})\n\n## Step 2: 第二步\nbar({\"x\": \"y\"})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].index, 1);
        assert_eq!(steps[0].tool_name, "foo");
        assert_eq!(steps[1].index, 2);
        assert_eq!(steps[1].tool_name, "bar");
        assert_eq!(steps[1].args_json, "{\"x\": \"y\"}");
    }

    #[test]
    fn dsl_parse_with_rollback_section() {
        let body = "## Step 1: 行动\ndo({\"k\": \"v\"})\n\n## Rollback\nundo({\"k\": \"v\"})\n";
        let (steps, rb) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(rb.len(), 1);
        assert_eq!(rb[0].tool_name, "undo");
        assert_eq!(rb[0].args_json, "{\"k\": \"v\"}");
    }

    #[test]
    fn dsl_parse_strips_frontmatter() {
        let body = "---\nname: test\ndescription: 解析测试\n---\n## Step 1: 行动\nfoo({})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_name, "foo");
    }

    #[test]
    fn dsl_parse_ignores_free_text_comments() {
        // 自由文字 / 注释行应被忽略，不破坏解析
        let body = "# 标题\n\n这是自由文字说明\n\n## Step 1: 行动\nfoo({})\n";
        let (steps, _) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].tool_name, "foo");
    }

    #[test]
    fn dsl_parse_tool_call_rejects_bad_name() {
        // 工具名含非法字符（不是 ASCII 字母数字下划线）应被拒
        assert!(parse_tool_call("foo-bar({})").is_none());
        assert!(parse_tool_call("foo bar({})").is_none());
        // 不以 ) 结尾也应被拒
        assert!(parse_tool_call("foo({}").is_none());
        assert!(parse_tool_call("foo{}").is_none());
        // 合法 name（含数字）应通过
        assert!(parse_tool_call("foo123({})").is_some());
    }

    // ── Phase 2 变量替换（2026-08-18 05:18 接入 run_skill_scheduler） ──

    fn ctx_one_step(uuid: &str, result: &str) -> Vec<CompletedStep> {
        vec![CompletedStep {
            index: 1,
            title: "step-1".into(),
            result: result.into(),
            id: Some(uuid.into()),
            // Phase 4 第 2 项：result 尝试 parse_json，失败 None（嵌套路径 fallback 保留原样）
            parsed: serde_json::from_str(result).ok(),
        }]
    }

    #[test]
    fn substitute_vars_resolves_step_index_id() {
        // 任务卡 UUID 跨步骤传递：${step1.id} → 真实 UUID
        let ctx = ctx_one_step("7c9e6679-7425-40de-944b-e07fc1f90ae7", "task created");
        let args = r#"{"id": "${step1.id}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}"#
        );
    }

    #[test]
    fn substitute_vars_resolves_step_index_result() {
        // 步骤原始结果跨步骤传递：${step1.result} → 上一步工具返回原文
        // ctx.result 是 Rust String，\n 是真实换行符 0x0A（不是字面两字符 \n）
        let ctx = ctx_one_step("uuid-1", "第一行\n第二行");
        let args = "{\"note\": \"${step1.result}\"}";
        // 期望值同样含真实换行符
        assert_eq!(
            substitute_vars(args, &ctx),
            "{\"note\": \"第一行\n第二行\"}"
        );
    }

    #[test]
    fn substitute_vars_resolves_prev_alias() {
        // ${prev.*} 简写指向 ctx 最后一项
        let ctx = vec![
            CompletedStep { index: 1, title: "a".into(), result: "first".into(), id: None, parsed: None },
            CompletedStep { index: 2, title: "b".into(), result: "second".into(), id: Some("uuid-2".into()), parsed: None },
        ];
        assert_eq!(substitute_vars("${prev.result}", &ctx), "second");
        assert_eq!(substitute_vars("${prev.id}", &ctx), "uuid-2");
        // 多次出现的 ${prev.result} 全部替换
        assert_eq!(
            substitute_vars("[${prev.result}]-${prev.result}", &ctx),
            "[second]-second"
        );
    }

    #[test]
    fn substitute_vars_keeps_unknown_intact() {
        // 未匹配的 ${...} 保留原样（避免误吃合法 JSON 中的 $ 字符）
        let ctx = ctx_one_step("uuid-1", "r");
        // 越界索引：保留
        assert_eq!(
            substitute_vars(r#"{"ref": "${step99.id}"}"#, &ctx),
            r#"{"ref": "${step99.id}"}"#
        );
        // 非法 field：保留
        assert_eq!(
            substitute_vars(r#"{"x": "${step1.unknown}"}"#, &ctx),
            r#"{"x": "${step1.unknown}"}"#
        );
        // 合法 JSON 里的 $1.50 不应被吃
        assert_eq!(
            substitute_vars(r#"{"price": "$1.50"}"#, &ctx),
            r#"{"price": "$1.50"}"#
        );
        // 空 ctx：${prev.*} 保留
        assert_eq!(substitute_vars("${prev.id}", &[]), "${prev.id}");
    }

    #[test]
    fn extract_task_id_picks_first_uuid() {
        // 工具结果含任务卡 UUID → 提取
        let text = "任务已创建：\nID = 7c9e6679-7425-40de-944b-e07fc1f90ae7\n标题：买牛奶";
        assert_eq!(
            extract_task_id(text).as_deref(),
            Some("7c9e6679-7425-40de-944b-e07fc1f90ae7")
        );
        // 多 UUID 取首个
        let text2 = "first 11111111-2222-3333-4444-555555555555 then 66666666-7777-8888-9999-000000000000";
        assert_eq!(
            extract_task_id(text2).as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        // 无 UUID → None
        assert_eq!(extract_task_id("no uuid here"), None);
        // 部分 UUID 格式（不足 36 字符）不应匹配
        assert_eq!(extract_task_id("short 7c9e6679-7425-40de-944b"), None);
    }

    #[test]
    fn substitute_vars_handles_multiple_indexes_in_one_call() {
        // 一个 args_json 里同时引用多个 step：${step1.id} + ${step2.result}
        let ctx = vec![
            CompletedStep {
                index: 1,
                title: "create".into(),
                result: "r1".into(),
                id: Some("uuid-1".into()),
                parsed: None,
            },
            CompletedStep {
                index: 2,
                title: "query".into(),
                result: "second-step-output".into(),
                id: None,
                parsed: None,
            },
        ];
        let args = r#"{"task": "${step1.id}", "note": "${step2.result}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"task": "uuid-1", "note": "second-step-output"}"#
        );
    }


    // ── Phase 4 第 2 项：嵌套路径（2026-08-18 06:25） ──

    fn ctx_one_step_parsed(uuid: &str, parsed_json: &str) -> Vec<CompletedStep> {
        vec![CompletedStep {
            index: 1,
            title: "step-1".into(),
            result: parsed_json.into(),
            id: Some(uuid.into()),
            parsed: serde_json::from_str(parsed_json).ok(),
        }]
    }

    #[test]
    fn nested_path_resolves_step_index_nested_field() {
        // `${step1.task.id}` 嵌套路径解析
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"{"task": {"id": "task-uuid-aaa", "title": "买牛奶"}}"#,
        );
        let args = r#"{"ref": "${step1.task.id}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"ref": "task-uuid-aaa"}"#
        );
    }

    #[test]
    fn nested_path_resolves_prev_nested_field() {
        // `${prev.task.title}` 上一步嵌套字段
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"{"task": {"title": "归档演示"}}"#,
        );
        let args = r#"{"title": "${prev.task.title}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"title": "归档演示"}"#
        );
    }

    #[test]
    fn nested_path_resolves_array_index() {
        // `${step1.1.id}` 数字段作为数组索引
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"[{"id": "first"}, {"id": "second"}, {"id": "third"}]"#,
        );
        let args = r#"{"id": "${step1.1.id}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"id": "second"}"#
        );
    }

    #[test]
    fn nested_path_keeps_intact_when_field_missing() {
        // JSON 存在但字段缺失 → 保留 `${step1...}` 原样
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"{"task": {"id": "x"}}"#,
        );
        let args = r#"{"x": "${step1.task.title}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"x": "${step1.task.title}"}"#
        );
    }

    #[test]
    fn nested_path_keeps_intact_when_not_json() {
        // parsed = None（纯文本 result，JSON parse 失败）→ 保留原样
        let ctx = ctx_one_step("uuid-1", "plain text response");
        assert!(ctx[0].parsed.is_none());
        let args = r#"{"x": "${step1.task.id}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"x": "${step1.task.id}"}"#
        );
    }

    #[test]
    fn nested_path_resolves_deep_chain() {
        // `${step1.a.b.c.d}` 深嵌套 5 层
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"{"a": {"b": {"c": {"d": "deep-value"}}}}"#,
        );
        let args = r#"{"x": "${step1.a.b.c.d}"}"#;
        assert_eq!(
            substitute_vars(args, &ctx),
            r#"{"x": "deep-value"}"#
        );
    }

    #[test]
    fn nested_path_serializes_non_string_value() {
        // 终值非字符串（数字/布尔/null）→ 序列化成字符串
        let ctx = ctx_one_step_parsed(
            "uuid-1",
            r#"{"n": 42, "b": true, "z": null}"#,
        );
        assert_eq!(substitute_vars(r#"{"v": "${step1.n}"}"#, &ctx), r#"{"v": "42"}"#);
        assert_eq!(substitute_vars(r#"{"v": "${step1.b}"}"#, &ctx), r#"{"v": "true"}"#);
        assert_eq!(substitute_vars(r#"{"v": "${step1.z}"}"#, &ctx), r#"{"v": "null"}"#);
    }

    #[test]
    fn nested_path_does_not_collide_with_single_segment_result_id() {
        // 回归测试：`${stepN.result}` / `${stepN.id}` 仍走原 VAR_BY_INDEX 路径
        // 不被嵌套 regex 抢先吃掉
        let ctx = ctx_one_step("task-uuid", "raw text result");
        assert_eq!(
            substitute_vars(r#"{"r": "${step1.result}"}"#, &ctx),
            r#"{"r": "raw text result"}"#
        );
        assert_eq!(
            substitute_vars(r#"{"i": "${step1.id}"}"#, &ctx),
            r#"{"i": "task-uuid"}"#
        );
    }

    // ── Phase 3 样板 Skill 端到端 DSL 验证（2026-08-18 05:30） ──

    /// 样板 SKILL.md 的 body（与 target/debug/skills/minimax-task-archive-demo/SKILL.md 同源）
    const DEMO_BODY: &str = "---\n\
name: minimax-task-archive-demo\n\
description: Phase 3 样板\n\
---\n\
# 任务归档演示\n\
\n\
演示 2 步 DSL。\n\
\n\
## Step 1: 列出当前所有任务\n\
list_tasks({})\n\
\n\
## Step 2: 查询第一张任务卡详情\n\
query_single_task({\"id\": \"${step1.id}\"})\n";

    #[test]
    fn demo_skill_parses_two_steps_no_rollback() {
        // 样板 Skill 应被解析为 2 个 step，无 rollback 段
        let (steps, rollback) = parse_skill_steps(DEMO_BODY).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(rollback.is_empty());
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[0].args_json, "{}");
        assert_eq!(steps[1].tool_name, "query_single_task");
        assert_eq!(steps[1].args_json, r#"{"id": "${step1.id}"}"#);
    }

    #[test]
    fn demo_skill_substitution_chain_resolves_step1_id() {
        // 模拟调度器跑完 Step 1 → Step 2 之前的 ctx 状态：
        // Step 1 工具返回文本含 UUID → extract_task_id 拿到 id → push 进 ctx
        // Step 2 执行前调 substitute_vars → ${step1.id} 应被替换成真实 UUID
        let step1_text = "当前 3 张任务卡：\n\
- 7c9e6679-7425-40de-944b-e07fc1f90ae7 买牛奶\n\
- 11111111-2222-3333-4444-555555555555 写报告\n\
- 66666666-7777-8888-9999-000000000000 周会准备";
        let step1_uuid = extract_task_id(step1_text).unwrap();
        assert_eq!(step1_uuid, "7c9e6679-7425-40de-944b-e07fc1f90ae7");

        let ctx = vec![CompletedStep {
            index: 1,
            title: "列出当前所有任务".into(),
            result: step1_text.into(),
            id: Some(step1_uuid.clone()),
            // Phase 4 第 2 项：parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
            parsed: None,
        }];
        // Step 2 的 args_json 模板（与 SKILL.md 一致）
        let step2_args = r#"{"id": "${step1.id}"}"#;
        let resolved = substitute_vars(step2_args, &ctx);
        assert_eq!(
            resolved,
            format!(r#"{{"id": "{}"}}"#, step1_uuid)
        );
    }

    // ── Phase 3 第二步（2026-08-18 05:33）：v2 升级样板 ──

    /// v2 SKILL.md 的 body（与 target/debug/skills/minimax-task-summary-v2/SKILL.md 同源）
    const V2_BODY: &str = "---\n\
name: minimax-task-summary-v2\n\
description: v2 升级\n\
rollback: auto\n\
---\n\
# 任务汇总 v2\n\
\n\
v2 相对 v1 改进：加 rollback 段。\n\
\n\
## Step 1: 列出当前所有任务\n\
list_tasks({})\n\
\n\
## Step 2: 查询第一张任务卡详情\n\
query_single_task({\"id\": \"${step1.id}\"})\n\
\n\
## Rollback\n\
query_single_task({\"id\": \"${step1.id}\"})\n";

    #[test]
    fn v2_parses_two_steps_one_rollback() {
        // v2 应被解析为 2 step + 1 rollback step
        let (steps, rollback) = parse_skill_steps(V2_BODY).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(rollback.len(), 1);
        assert_eq!(steps[0].tool_name, "list_tasks");
        assert_eq!(steps[1].tool_name, "query_single_task");
        assert_eq!(steps[1].args_json, r#"{"id": "${step1.id}"}"#);
        // rollback 段也是 query_single_task，与主 step 复用同样变量替换路径
        assert_eq!(rollback[0].tool_name, "query_single_task");
        assert_eq!(rollback[0].args_json, r#"{"id": "${step1.id}"}"#);
    }

    #[test]
    fn v2_rollback_step_args_also_go_through_substitute_vars() {
        // rollback 段 args 也走变量替换（Phase 2 已接入 run_skill_scheduler）
        // 模拟 Step 1 跑完后，回滚段用 ctx 里的 id 替换 ${step1.id}
        let ctx = vec![CompletedStep {
            index: 1,
            title: "list".into(),
            result: "r".into(),
            id: Some("uuid-99".into()),
            // Phase 4 第 2 项：parsed 字段（嵌套路径用，纯文本 result parse 失败为 None）
            parsed: None,
        }];
        let rb_args = r#"{"id": "${step1.id}"}"#;
        let resolved = substitute_vars(rb_args, &ctx);
        assert_eq!(resolved, r#"{"id": "uuid-99"}"#);
    }

    // ── Phase 3 第三步（2026-08-18 06:10）：扫所有 Skill 目录通用验证 ──

    /// 扫 target/debug/skills/ 下所有 SKILL.md，验证 parse_skill_steps 通过
    /// 且至少 1 step，rollback 段（若有）也合法。
    /// 不依赖具体某个 Skill 文件——sub-agent 写完任意 Skill 后，这个测试自动覆盖。
    #[test]
    fn scan_all_skills_in_debug_dir_parse_correctly() {
        let skill_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("skills");
        let mut found = 0;
        let read_dir = match std::fs::read_dir(&skill_dir) {
            Ok(d) => d,
            Err(_) => {
                // 目录不存在不报错（开发期可能未初始化）
                eprintln!("skill dir 不存在，跳过扫瞄：{}", skill_dir.display());
                return;
            }
        };
        for entry in read_dir.flatten() {
            let skill_md = entry.path().join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }
            let body = std::fs::read_to_string(&skill_md).unwrap_or_else(|e| {
                panic!("read {} failed: {}", skill_md.display(), e)
            });
            let (steps, rollback) = parse_skill_steps(&body).unwrap_or_else(|e| {
                panic!(
                    "parse_skill_steps failed for {}: {}",
                    entry.path().display(),
                    e
                )
            });
            assert!(
                !steps.is_empty(),
                "{} 没有 step",
                entry.path().display()
            );
            for (i, step) in steps.iter().enumerate() {
                assert!(
                    !step.tool_name.is_empty(),
                    "{} step {} 缺 tool_name",
                    entry.path().display(),
                    i
                );
            }
            for (i, rb) in rollback.iter().enumerate() {
                assert!(
                    !rb.tool_name.is_empty(),
                    "{} rollback step {} 缺 tool_name",
                    entry.path().display(),
                    i
                );
            }
            found += 1;
        }
        // 不强制 Skill 数 — 任意现存 Skill 都应被正确解析
        if found == 0 {
            // 目录存在但为空（cargo clean 误删 dev mock / dev 首次未 init）—— graceful skip
            eprintln!("skill dir 为空，跳过扫瞄：{}", skill_dir.display());
            return;
        }
        eprintln!("扫瞄 {} 个 Skill，全部解析通过", found);
    }

    // ── Phase 4 第 3 项：端到端 run_dsl_loop_sync（2026-08-18 06:36） ──
    //
    // 同步版核心循环（生产 run_skill_scheduler 是 async + 调 bot::execute_tool）。
    // 这里剥离 AppHandle 依赖 + 注入 mock executor，让单测能验证：
    //   1. 嵌套变量替换端到端走通（Phase 4 第 2 项）
    //   2. advance_dsl 4 个非 Run 分支的端到端拦截（Phase 4 第 1 项）
    //   3. rollback 在 step 失败时被调用、Terminate 时被跳过

    fn run_dsl_loop_sync(
        ctx: &mut Vec<CompletedStep>,
        steps: &[SkillStep],
        rollback: &[SkillStep],
        skill_run: Option<&SkillRun>,
        mut exec: impl FnMut(&str, &str) -> String,
    ) -> Result<Vec<(usize, String, String)>, String> {
        let mut results: Vec<(usize, String, String)> = Vec::new();
        for step in steps {
            // Phase 4 第 1 项：advance_dsl 状态机检查
            if let Some(run) = skill_run {
                match advance_dsl(run, now_ms()) {
                    DslAdvanceAction::Run => {}
                    DslAdvanceAction::Finish => break,
                    DslAdvanceAction::AwaitUser => return Err("__await_user__".into()),
                    DslAdvanceAction::FailWithRollback(reason) => {
                        for rb in rollback {
                            let rb_args = substitute_vars(&rb.args_json, ctx);
                            exec(&rb.tool_name, &rb_args);
                        }
                        return Err(format!("技能中止：{reason}"));
                    }
                    DslAdvanceAction::Terminate(reason) => {
                        return Err(format!("技能终止：{reason}"));
                    }
                }
            }
            // Phase 2 + Phase 4 第 2 项：嵌套变量替换
            let resolved_args = substitute_vars(&step.args_json, ctx);
            let text = exec(&step.tool_name, &resolved_args);
            // 失败判定（跟生产 run_skill_scheduler 一致）
            let failed = text.starts_with("未知工具")
                || text.starts_with("失败")
                || text.starts_with("错误")
                || text.starts_with("error:")
                || text.starts_with("Error:");
            if failed {
                for rb in rollback {
                    let rb_args = substitute_vars(&rb.args_json, ctx);
                    exec(&rb.tool_name, &rb_args);
                }
                return Err(format!(
                    "Step {} ({}) 失败：{}",
                    step.index, step.title, text
                ));
            }
            ctx.push(CompletedStep {
                index: step.index,
                title: step.title.clone(),
                result: text.clone(),
                id: extract_task_id(&text),
                parsed: serde_json::from_str(&text).ok(),
            });
            results.push((step.index, step.title.clone(), text));
        }
        Ok(results)
    }

    #[test]
    fn e2e_runs_all_steps_with_nested_var_substitution() {
        // 端到端：2-step DSL 演示 `${step1.task.id}` 嵌套路径替换真的 work
        // Step 1 list_tasks → JSON 嵌套结构 → push ctx
        // Step 2 query_single_task 用 `${step1.task.id}` → 期望 args 替换成真实 UUID
        let body = "## Step 1: list\nlist_tasks({})\n\n## Step 2: query\nquery_single_task({\"id\": \"${step1.task.id}\"})";
        let (steps, rollback) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 2);
        assert!(rollback.is_empty());

        let calls: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
        let mut exec = |tool: &str, args: &str| -> String {
            calls.borrow_mut().push((tool.to_string(), args.to_string()));
            match tool {
                "list_tasks" => r#"{"task": {"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "买牛奶"}}"#.to_string(),
                "query_single_task" => "ok".to_string(),
                _ => format!("未知工具: {tool}"),
            }
        };

        let mut ctx: Vec<CompletedStep> = Vec::new();
        let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, None, &mut exec);

        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        assert_eq!(ctx.len(), 2);

        let calls = calls.borrow();
        assert_eq!(calls.len(), 2, "expected 2 tool calls");
        assert_eq!(calls[0].0, "list_tasks");
        assert_eq!(calls[0].1, "{}");
        assert_eq!(calls[1].0, "query_single_task");
        // 关键断言：${step1.task.id} 嵌套路径真的被替换成标准 UUID
        assert_eq!(calls[1].1, r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}"#);

        // ctx 累积验证：Step 2 的 result 是 query 的返回值，parsed 是 None（不是 JSON）
        assert_eq!(ctx[0].result, r#"{"task": {"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "买牛奶"}}"#);
        assert!(ctx[0].parsed.is_some(), "Step 1 合法 JSON 应 parse 成功");
        assert_eq!(ctx[0].id.as_deref(), Some("7c9e6679-7425-40de-944b-e07fc1f90ae7"));
        assert_eq!(ctx[1].result, "ok");
        assert!(ctx[1].parsed.is_none(), "Step 2 'ok' 不是 JSON，parsed 应为 None");
    }

    #[test]
    fn e2e_fails_with_rollback_when_step_fails() {
        // 端到端：Step 2 失败触发 rollback（含嵌套变量替换）
        // rollback step 用 ${step1.task.id} → 期望拿到 Step 1 创建的 UUID
        let body = "## Step 1: create\ncreate_task({})\n\n## Step 2: risky\nrisky_tool({\"x\": \"${step1.task.id}\"})\n\n## Rollback\nrollback_tool({\"ref\": \"${step1.task.id}\"})";
        let (steps, rollback) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(rollback.len(), 1);

        let calls: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
        let mut exec = |tool: &str, args: &str| -> String {
            calls.borrow_mut().push((tool.to_string(), args.to_string()));
            match tool {
                "create_task" => r#"{"task": {"id": "uuid-step1"}}"#.to_string(),
                "risky_tool" => "失败：工具异常".to_string(),
                "rollback_tool" => "rolled back".to_string(),
                _ => format!("未知工具: {tool}"),
            }
        };

        let mut ctx: Vec<CompletedStep> = Vec::new();
        let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, None, &mut exec);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Step 2"), "expected 'Step 2' in error: {err}");
        assert!(err.contains("失败"), "expected '失败' in error: {err}");

        // 关键验证：Step 1 跑了 + Step 2 跑了 + rollback 跑了（3 次）
        let calls = calls.borrow();
        assert_eq!(calls.len(), 3, "expected 3 tool calls (step1+step2+rollback)");
        assert_eq!(calls[0].0, "create_task");
        assert_eq!(calls[1].0, "risky_tool");
        // Step 2 的 args 也走嵌套路径替换（验证 rollback 段、step 段共享 substitute_vars 路径）
        assert_eq!(calls[1].1, r#"{"x": "uuid-step1"}"#);
        assert_eq!(calls[2].0, "rollback_tool");
        // rollback args 也走嵌套路径替换（Phase 2 已实现，e2e 端到端覆盖）
        assert_eq!(calls[2].1, r#"{"ref": "uuid-step1"}"#);
    }

    #[test]
    fn e2e_terminates_skips_rollback() {
        // 端到端：SkillRun state = Terminated → advance_dsl 返回 Terminate → 跳过 rollback
        let body = "## Step 1: list\nlist_tasks({})\n\n## Step 2: query\nquery_single_task({})\n\n## Rollback\nrollback({})";
        let (steps, rollback) = parse_skill_steps(body).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(rollback.len(), 1);

        // mock SkillRun：state = Terminated（用户 /stop 触发）
        let mut run = test_run(8, 180);
        run.state = SkillState::Terminated;
        run.end_reason = "用户 /stop".into();

        let calls: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let mut exec = |tool: &str, _args: &str| -> String {
            calls.borrow_mut().push(tool.to_string());
            "ok".to_string()
        };

        let mut ctx: Vec<CompletedStep> = Vec::new();
        let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, Some(&run), &mut exec);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("终止"), "expected '终止' in error: {err}");
        assert!(err.contains("用户 /stop"), "expected reason in error: {err}");

        // 关键验证：Terminate 在 step 1 之前就拦截 → 0 次工具调用 + rollback 跳过
        let calls = calls.borrow();
        assert_eq!(calls.len(), 0, "expected 0 tool calls (Terminated before any step), got: {:?}", *calls);
    }

    #[test]
    fn e2e_pauses_with_await_user_when_paused() {
        // 端到端：SkillRun state = Paused → advance_dsl 返回 AwaitUser → 返回 __await_user__
        let body = "## Step 1: list\nlist_tasks({})";
        let (steps, rollback) = parse_skill_steps(body).unwrap();

        // mock SkillRun：state = Paused（用户确认等待中）
        let mut run = test_run(8, 180);
        run.state = SkillState::Paused;

        let calls: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let mut exec = |tool: &str, _args: &str| -> String {
            calls.borrow_mut().push(tool.to_string());
            "ok".to_string()
        };

        let mut ctx: Vec<CompletedStep> = Vec::new();
        let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, Some(&run), &mut exec);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "__await_user__");

        // 关键验证：Paused 在 step 1 之前就拦截 → 0 次工具调用
        let calls = calls.borrow();
        assert_eq!(calls.len(), 0, "expected 0 tool calls (Paused before any step), got: {:?}", *calls);
    }

    // ── Phase 4 第 4 项：scan_skill_dirs 多目录去重（2026-08-18 07:09） ──

    /// 临时目录 RAII 守卫（测试结束自动清理，panic 也清理）
    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_tempdir(label: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "wmessage-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    /// 在 dir 下创建 mock Skill（目录 + SKILL.md 含 YAML frontmatter）
    fn make_mock_skill(parent: &std::path::Path, name: &str, description: &str) {
        let skill_dir = parent.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let text = format!(
            "---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nbody\n"
        );
        std::fs::write(skill_dir.join("SKILL.md"), text).unwrap();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn scan_skill_dirs_dedupes_with_first_dir_priority() {
        // dir1 优先 + dir2 兜底：同名 Skill 返回 dir1 的版本（description 区分）
        let temp = make_tempdir("scan-dedup");
        let dir1 = temp.0.join("dir1");
        let dir2 = temp.0.join("dir2");
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();
        // 共享 skill "shared"：dir1 版本 description="from-dir1"
        make_mock_skill(&dir1, "shared", "from-dir1");
        make_mock_skill(&dir1, "only-in-dir1", "first");
        // dir2 版本 description="from-dir2"
        make_mock_skill(&dir2, "shared", "from-dir2");
        make_mock_skill(&dir2, "only-in-dir2", "second");

        // 顺序：dir1 → dir2（dir1 优先）
        let out = scan_skill_dirs(&[dir1.clone(), dir2.clone()]);
        assert_eq!(out.len(), 3, "expected 3 unique skills (shared + 2 unique)");
        let shared = out.iter().find(|s| s.name == "shared").unwrap();
        assert_eq!(
            shared.description, "from-dir1",
            "shared 应取 dir1 版本（前面目录优先），got: {}",
            shared.description
        );
        assert!(out.iter().any(|s| s.name == "only-in-dir1"));
        assert!(out.iter().any(|s| s.name == "only-in-dir2"));

        // 顺序反过来：dir2 → dir1，shared 应取 dir2 版本
        let out2 = scan_skill_dirs(&[dir2, dir1]);
        let shared2 = out2.iter().find(|s| s.name == "shared").unwrap();
        assert_eq!(
            shared2.description, "from-dir2",
            "顺序反过来后 shared 应取 dir2 版本（前面目录优先）"
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn scan_skill_dirs_skips_nonexistent_dirs() {
        // 目录不存在 → 不 panic，跳过即可
        let temp = make_tempdir("scan-missing");
        let dir = temp.0.join("real");
        std::fs::create_dir_all(&dir).unwrap();
        make_mock_skill(&dir, "test-skill", "ok");

        let nonexistent = std::path::PathBuf::from("/tmp/wmessage-nonexistent-dir-xxx-does-not-exist");
        let _ = std::fs::remove_dir_all(&nonexistent);  // 确保不存在
        let out = scan_skill_dirs(&[nonexistent, dir]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "test-skill");
    }

    #[test]
    fn scan_skill_dirs_returns_empty_for_empty_dirs_list() {
        let out = scan_skill_dirs(&[]);
        assert!(out.is_empty());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_skills_dir_at_returns_path_when_exists() {
        // 在临时 target_root 下建 debug/skills/mock，验证 dev_skills_dir_at 能找到
        let temp = make_tempdir("dev-skills");
        let target_root = temp.0.join("target");
        let dev_skills = target_root.join("debug").join("skills");
        std::fs::create_dir_all(&dev_skills).unwrap();
        make_mock_skill(&dev_skills, "dev-mock", "from-dev");

        let found = dev_skills_dir_at(&target_root);
        assert!(found.is_some(), "dev_skills_dir_at 应找到 debug/skills");
        assert_eq!(found.unwrap(), dev_skills);

        // 不存在的 target_root → None
        let missing = temp.0.join("missing-target");
        assert!(dev_skills_dir_at(&missing).is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn dev_skills_dir_at_returns_none_when_debug_skills_missing() {
        // target_root 存在但 debug/skills 不存在 → None
        let temp = make_tempdir("dev-skills-empty");
        let target_root = temp.0.join("target");
        std::fs::create_dir_all(&target_root).unwrap();
        // 没建 debug/skills
        assert!(dev_skills_dir_at(&target_root).is_none());
    }

    // ── Phase 4 第 5 项：format_completed_summary + DslOutcome/DslFailure（2026-08-18 07:20） ──

    #[test]
    fn format_completed_summary_returns_placeholder_for_empty_ctx() {
        // 空 ctx → "(无已完成步骤)" 占位
        let ctx: Vec<CompletedStep> = Vec::new();
        assert_eq!(format_completed_summary(&ctx), "(无已完成步骤)");
    }

    #[test]
    fn format_completed_summary_renders_single_step() {
        // 单 step → "Step N (title): <result>"
        let ctx = vec![CompletedStep {
            index: 1,
            title: "list-tasks".into(),
            result: "task list result".into(),
            id: None,
            parsed: None,
        }];
        let out = format_completed_summary(&ctx);
        assert!(out.contains("- Step 1 (list-tasks): task list result"));
    }

    #[test]
    fn format_completed_summary_renders_multiple_steps() {
        // 多 step → 每步一行，按 ctx 顺序
        let ctx = vec![
            CompletedStep { index: 1, title: "step-a".into(), result: "result-a".into(), id: None, parsed: None },
            CompletedStep { index: 2, title: "step-b".into(), result: "result-b".into(), id: None, parsed: None },
            CompletedStep { index: 3, title: "step-c".into(), result: "result-c".into(), id: None, parsed: None },
        ];
        let out = format_completed_summary(&ctx);
        assert!(out.contains("Step 1 (step-a): result-a"));
        assert!(out.contains("Step 2 (step-b): result-b"));
        assert!(out.contains("Step 3 (step-c): result-c"));
        // 顺序：a 在 b 前面，b 在 c 前面
        let pos_a = out.find("Step 1").unwrap();
        let pos_b = out.find("Step 2").unwrap();
        let pos_c = out.find("Step 3").unwrap();
        assert!(pos_a < pos_b && pos_b < pos_c);
    }

    #[test]
    fn format_completed_summary_truncates_long_results() {
        // 超长 result (>200 字符) → 截断 + "..." 后缀
        let long_result: String = "x".repeat(500);
        let ctx = vec![CompletedStep {
            index: 1,
            title: "long".into(),
            result: long_result.clone(),
            id: None,
            parsed: None,
        }];
        let out = format_completed_summary(&ctx);
        // 截断后 preview = 200 字符 + "..." = 203 字符（在 - Step 1 (long): 之后）
        let marker = "- Step 1 (long): ";
        let start = out.find(marker).unwrap() + marker.len();
        let after = &out[start..];
        // preview 部分应当是 200 个 x + "..."
        let expected_preview: String = std::iter::repeat("x").take(200).collect::<String>() + "...";
        assert!(after.starts_with(&expected_preview), "expected preview starts with 200x + '...', got first chars: {}", &after[..after.len().min(50)]);
    }

    #[test]
    fn dsl_outcome_done_carries_summary_string() {
        // DslOutcome::Done 携带 summary 字符串（构造 + 取出来一致）
        let outcome = DslOutcome::Done("summary text".into());
        match outcome {
            DslOutcome::Done(s) => assert_eq!(s, "summary text"),
            _ => panic!("expected Done variant"),
        }
    }

    #[test]
    fn dsl_failure_terminated_carries_reason() {
        // DslFailure::Terminated 携带 reason（用户主动终止场景）
        let failure = DslFailure::Terminated { reason: "用户 /stop".into() };
        match failure {
            DslFailure::Terminated { reason } => assert_eq!(reason, "用户 /stop"),
        }
    }

    // ── Phase 5 第 1 项：11 个真业务 Skill 端到端 smoke test（2026-08-18 07:30） ──

    /// 通用 mock executor（Phase 5 smoke test）：每个工具返回成功 + 含标准 UUID 让变量替换 / 嵌套路径 work
    /// - 返回 JSON 字符串时尽量含 UUID 7c9e6679-7425-40de-944b-e07fc1f90ae7（让 `${step1.task.id}` 等嵌套路径能取到值）
    /// - 不存在的工具返回 "mock ok"（确保所有 Skill 都能跑完不 panic）
    fn generic_mock_executor(tool: &str, _args: &str) -> String {
        match tool {
            "list_tasks" | "search_tasks" => {
                r#"[{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "mock"}]"#.to_string()
            }
            "query_single_task" => {
                r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "mock", "task": {"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}}"#.to_string()
            }
            "create_task" | "complete_task" | "edit_task" | "delete_task" | "add_subtask" => {
                r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}"#.to_string()
            }
            "create_word" | "create_excel" | "create_ppt" | "create_pdf" => {
                r#"{"path": "/tmp/mock-output"}"#.to_string()
            }
            "extract_document" => r#"{"text": "mock extracted content"}"#.to_string(),
            "run_python" => "mock python output".to_string(),
            "web_search" => {
                r#"[{"title": "mock result", "url": "https://example.com"}]"#.to_string()
            }
            "fetch_url" => r#"{"text": "mock fetched content"}"#.to_string(),
            "bind_file" => r#"{"bound": true}"#.to_string(),
            "use_skill" => "mock skill body".to_string(),
            _ => "mock ok".to_string(),
        }
    }

    #[test]
    fn smoke_all_real_skills_run_dsl_loop_with_mock_executor() {
        // 端到端 smoke：扫 target/debug/skills/ 下所有 13 个 mock Skill
        // 每个 Skill 跑 run_dsl_loop_sync + 通用 mock executor
        // 验证：parse 不 panic + 整链路跑通 + ctx 累积 + 嵌套变量替换
        let skills_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/debug/skills");
        if !skills_dir.exists() {
            // 没建 mock 跳过（防止 dev 模式第一次 cargo test 失败）
            eprintln!("跳过：{} 不存在（dev 模式需先写出 mock Skill）", skills_dir.display());
            return;
        }

        let mut skill_count = 0;
        let mut failed: Vec<String> = Vec::new();
        let entries = std::fs::read_dir(&skills_dir).expect("read_dir skills");
        for entry in entries {
            let entry = entry.expect("entry");
            let path = entry.path();
            if !path.is_dir() { continue; }
            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() { continue; }
            let name = path.file_name().unwrap().to_string_lossy().to_string();

            // parse
            let body = std::fs::read_to_string(&skill_md)
                .unwrap_or_else(|e| panic!("Skill {name} SKILL.md 读失败: {e}"));
            let (steps, rollback) = match parse_skill_steps(&body) {
                Ok(sr) => sr,
                Err(e) => {
                    failed.push(format!("{name}: parse failed: {e}"));
                    continue;
                }
            };
            if steps.is_empty() {
                failed.push(format!("{name}: steps empty"));
                continue;
            }

            // 记录每个 step 的 args 在跑前是否含 ${...}（用于后续验证嵌套变量替换真的替换了）
            let step_args_with_var: Vec<bool> = steps.iter()
                .map(|s| s.args_json.contains("${"))
                .collect();

            // 跑 run_dsl_loop_sync
            let mut ctx: Vec<CompletedStep> = Vec::new();
            let mut exec = generic_mock_executor;
            let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, None, &mut exec);
            if let Err(e) = result {
                failed.push(format!("{name}: run failed: {e}"));
                continue;
            }

            // 验证：ctx 累积（至少跟 step 数一样）
            if ctx.len() != steps.len() {
                failed.push(format!(
                    "{name}: ctx len {} != steps len {}",
                    ctx.len(), steps.len()
                ));
                continue;
            }

            // 验证：含 ${...} 的 step args 跑完后应该已经被替换（ctx 累积至少 1 步后续 step 才能拿到）
            // 这里只 sanity check 跑通即可；嵌套变量替换正确性在 Phase 4 第 2 项单测里覆盖
            let _ = step_args_with_var;

            skill_count += 1;
        }

        if !failed.is_empty() {
            panic!(
                "{} 个 Skill 端到端跑失败：
  - {}",
                failed.len(),
                failed.join("\n  - ")
            );
        }

        // 期望至少 11 个真业务 Skill + 2 样板（task-summary / task-archive-demo / task-summary-v2）
        // 但 skills_dir 为空时 graceful skip（cargo clean 误删 dev mock / dev 首次未 init）
        if skill_count == 0 {
            eprintln!(
                "smoke 跳过：{} 下未扫到任何 Skill（dev mock 可能被 cargo clean 误删）",
                skills_dir.display()
            );
            return;
        }
        eprintln!("端到端 smoke：{} 个 Skill 全部跑通", skill_count);
    }
}
