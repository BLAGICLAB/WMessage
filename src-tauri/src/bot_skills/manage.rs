use super::parse::{
    parse_meta, parse_skill_steps, set_frontmatter_enabled, strip_frontmatter, unknown_tool_names,
    SkillMeta, SKILL_NAME_CHARS_OK,
};
use crate::error::{CommandError, CommandResult};
use serde::Serialize;
use tauri::{AppHandle, Manager};

// create_skill 相关（准入核 / 落盘 / 工具口）单独成模块。
// 公开面经这里逐项转出，bot_skills/mod.rs 的 `pub use manage::*;` 才能
// 继续把 crate::bot_skills::tool_create_skill 这类路径接住。
mod create;
pub use create::{tool_create_skill, validate_created_meta};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// frontmatter enabled（清单注入需要按它过滤）
    pub enabled: bool,
    /// frontmatter scope: session（模型 create_skill 建的会话档）。设置页照常
    /// 展示可管理；全局技能广告面与全局路由表都不收（与 intent_rules_from_dirs
    /// 同口径）——会话档只经 add_session_route 在本会话生效，进了全局清单
    /// 等于跨会话可见，别的会话模型会照着清单点名 use_skill。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub session_scoped: bool,
    /// last run outcome (SettingsPage badge)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<crate::db::PersistedSkillOutcome>,
    /// 可选版本号（frontmatter version）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 步骤/回滚引用了未内置工具的清单（设置页标红）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_tools: Vec<String>,
}

/// 用户安装的 skill 落盘目录。
///
/// **不走 `data_dir()`**：debug build 下 `data_dir()` 走 probe_log_dir，会探到
/// exe 同目录（target/debug/）——而 dev_skills_dir 又指向同一个 target/debug/skills，
/// `skill_search_paths` dedup 后只剩一个目录，用户从设置页拖进去的 skill 被
/// `cargo clean` 一起冲掉（因为它们实际写在 target/debug/skills 里）。
///
/// 改走 `app.path().app_data_dir()` 拿到稳定的系统应用数据目录（如 macOS 上的
/// `~/Library/Application Support/com.renshi.wmessage`）。这个目录与日志 / db
/// 所在的 `data_dir()` 解耦：skill 走稳定的应用数据目录，日志 / db 仍按便携策略
/// 跟随 exe。release build 下 `app_data_dir()` 与 probe 后的 `data_dir()` 也可能不同，
/// 但用户安装的 skill 不再被便携策略劫持到 exe 同目录。
///
/// 失败（罕见，仅限 sandbox 完全拒绝对系统应用数据目录的访问）回退 `data_dir()`，
/// 保证 skills_import / skills_open_dir / load_skill_meta 永不报「路径不存在」。
pub(crate) fn skills_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    data_dir_subdir(
        app.path().app_data_dir().ok(),
        || crate::db::data_dir(app),
        "skills",
    )
}

/// app_data 子目录定位内核（可测，不依赖 AppHandle / mock 语义）：
/// - 正常（`app_data_dir()` 可得）→ `app_data/<subdir>`（用户 skills 的 opener
///   scope `$APPDATA/**` 覆盖；builtin_skills 同父不同名）
/// - 失败回退 → `fallback_data_dir()/<subdir>`（便携场景可能不在 `$APPDATA` 下 ——
///   见 `docs/OCR-FIX-PLAN-2026-09-21.md` 的 C2a fallback 边缘 case）
///
/// `fallback` 用闭包懒求值：正常模式绝不触碰 `data_dir()`（其探针/建目录副作用）。
pub(super) fn data_dir_subdir(
    app_data: Option<std::path::PathBuf>,
    fallback_data_dir: impl FnOnce() -> std::path::PathBuf,
    subdir: &str,
) -> std::path::PathBuf {
    match app_data {
        Some(p) => p.join(subdir),
        None => fallback_data_dir().join(subdir),
    }
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
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Skill 搜索路径列表：用户 skills 必选 + 内置 builtin_skills + dev 模式追加
/// target/debug/skills。顺序即优先级（scan_skill_dirs 去重取先扫到的同名技能）：
/// 用户导入/修改的版本压过内置版，内置版压过 dev mock。
/// `#[cfg_attr(not(debug_assertions), allow(dead_code))]` —— release 模式 dev_skills_dir
/// 不存在，避免 dead_code 警告。
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn skill_search_paths<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Vec<std::path::PathBuf> {
    #[allow(unused_mut)] // release 模式：dev_skills_dir 分支被排除，paths 不需要 mut
    let mut paths = vec![skills_dir(app), super::builtin::builtin_skills_dir(app)];
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

/// 生效副本的 SKILL.md 路径：按 `skill_search_paths` 优先级取第一份存在的
/// `<dir>/<name>/SKILL.md`（与 `state::load_skill_meta` 的 fallback 同口径）。
///
/// 写入侧（启停开关）必须用它定位「改的是哪一份」——同名的用户副本压过内置
/// 物化副本、后者压过 dev mock；改到别的副本会出现「关的是 A、生效的是 B」
/// （用户点了开关却毫无反应，或关不掉真正生效的那份）。
fn effective_skill_md(dirs: &[std::path::PathBuf], name: &str) -> Option<std::path::PathBuf> {
    dirs.iter()
        .map(|d| d.join(name).join("SKILL.md"))
        .find(|p| p.is_file())
}

/// 扫描数据目录 + dev 模式 target/debug/skills 下所有 SKILL.md，返回 (目录名, 名称, 描述) 列表。
/// 数据目录优先（用户已导入 / 修改的 Skill 不被 dev mock 覆盖）。
/// 内部委托给纯函数 `scan_skill_dirs`，后者不依赖 AppHandle，单测可独立覆盖。
pub fn scan_skills<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<SkillInfo> {
    let mut out = scan_skill_dirs(&skill_search_paths(app));
    if let Ok(conn) = crate::db::open_db(app) {
        if let Ok(outcomes) = crate::db::load_all_skill_outcomes(&conn) {
            for s in &mut out {
                s.last_outcome = outcomes.get(&s.name).cloned();
            }
        }
    }
    out
}

/// 技能名校验：仅允许 ASCII 字母/数字 + `-` / `_`，非空。
/// 与 parse.rs::SKILL_NAME_CHARS_OK 同语义（不外暴露该常量以避免跨模块耦合）。
/// 抽公共函数：skills_import / skills_delete / scan_skill_dirs 三处复用。
pub(crate) fn validate_skill_name(name: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err(format!("技能名「{name}」无效（仅允许字母/数字/-/_）"));
    }
    Ok(())
}

/// 多目录扫描公共内核：顺序扫 `dirs` 列表中的每个目录，每个子目录视为一个 Skill
/// 候选（含 SKILL.md 才解析成 meta）。前面目录优先（用 HashSet seen 去重同名 Skill）。
/// `extract` 闭包决定如何从 meta 产出 `T`——返回 None 则跳过（用于 `scan_skill_dirs`
/// 过滤非法名、`intent_rules_from_dirs` 过滤 disabled/无 intents 的 skill）。
///
/// 错误处理：目录读不到 / SKILL.md 读不到 / 解析失败 → 静默跳过（不破坏同目录其他 skill）。
/// 单测场景：传临时目录数组验证去重逻辑，不依赖 AppHandle / db::data_dir。
fn visit_skill_dirs<F, T>(dirs: &[std::path::PathBuf], extract: F) -> Vec<T>
where
    F: Fn(&SkillMeta, &str, &str) -> Option<T>,
{
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<T> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if !seen.insert(dir_name.clone()) {
                continue;
            }
            let skill_md = path.join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&skill_md) else {
                continue;
            };
            let meta = parse_meta(&text, &dir_name);
            if let Some(t) = extract(&meta, &dir_name, &text) {
                out.push(t);
            }
        }
    }
    out
}

/// 多目录扫描核心：
/// 顺序扫 `dirs` 列表中的每个目录，每个目录的子目录视为一个 Skill（含 SKILL.md 即有效）。
/// **前面目录优先**（用 HashSet seen 去重）：同名 Skill 只保留先扫到的版本。
/// 非法名静默跳过——同时校验目录名 + frontmatter name，
/// 两者都必须走 SKILL_NAME_CHARS_OK。允许其一不合法会导致：
///   1. 路径含非法字符 → skills_delete 拒绝（装得上用不了删不掉）
///   2. 路由跳号 / 列表里出现无法 load 的项
/// 仅校验 frontmatter 不够——例如目录名 `中文 skill!`（非法）+ frontmatter `name: anything`
/// （合法）时，扫描器原本只查 name、会错误保留该项。
/// 排序按 name 字典序，输出稳定。
pub fn scan_skill_dirs(dirs: &[std::path::PathBuf]) -> Vec<SkillInfo> {
    let mut out: Vec<SkillInfo> = visit_skill_dirs(dirs, |meta, dir_name, text| {
        // 双重校验：frontmatter name 与目录名都要通过 SKILL_NAME_CHARS_OK。
        // 两者最终都会成为路径的一部分（删 / 加载都用），一个非法就 reject。
        if validate_skill_name(&meta.name).is_err() {
            return None;
        }
        if validate_skill_name(dir_name).is_err() {
            return None;
        }
        // 步骤/回滚引用的工具名 vs 内置注册表（MCP 挂载工具运行期可调，
        // 这里按内置名单校验，未知项交设置页标红提示）
        let unknown = parse_skill_steps(strip_frontmatter(text))
            .map(|(steps, rb)| unknown_tool_names(&steps, &rb, &registry_known_tools()))
            .unwrap_or_default();
        Some(SkillInfo {
            name: meta.name.clone(),
            description: meta.description.clone(),
            enabled: meta.enabled,
            session_scoped: meta.session_scoped,
            version: meta.version.clone(),
            unknown_tools: unknown,
            last_outcome: None,
        })
    });
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 内置注册表工具名集合（兼容校验真相源）
pub(crate) fn registry_known_tools() -> std::collections::HashSet<String> {
    crate::bot::registry::tools_index()
        .keys()
        .map(|s| s.to_string())
        .collect()
}

/// 全量技能路由规则扫描（未安装的技能不得有路由）：
/// 遍历搜索路径读每个 SKILL.md 的 frontmatter `intents`；
/// `enabled: false` → 该技能不产生路由。同名去重、前面目录优先（与 scan_skill_dirs 一致）。
///
/// 两类规则，点名在前、intent 在后：
/// - 点名规则：正文里出现技能名即命中（技能没声明 intents 也有这条）
/// - intent 规则：frontmatter `intents` 声明的正则
///
/// 点名必须整体排在 intent 前面：两条规则都命中时取先到的，
/// 「用 X 技能做个 PPT」要落到 X，而不是被另一个技能的 "PPT" 正则截胡。
pub fn intent_rules_from_dirs(
    dirs: &[std::path::PathBuf],
) -> Vec<crate::intent_router::IntentRule> {
    // 会话档技能（scope: session，模型 create_skill 建的）不进全局路由表：
    // 它们的路由只经 add_session_route 进会话档（本会话生效、跨会话不劫持）。
    // 文件在场但全局不可见——用户改掉 frontmatter 的 scope 才会提升为全局。
    let mut rules = visit_skill_dirs(dirs, |meta, _dir_name, _text| {
        if !meta.enabled || meta.session_scoped {
            return None;
        }
        crate::intent_router::explicit_name_pattern(&meta.name).map(|pattern| {
            crate::intent_router::IntentRule {
                patterns: vec![pattern],
                skill_name: meta.name.clone(),
                session: None,
            }
        })
    });
    rules.extend(visit_skill_dirs(dirs, |meta, _dir_name, _text| {
        if !meta.enabled || meta.session_scoped || meta.intents.is_empty() {
            return None;
        }
        Some(crate::intent_router::IntentRule {
            patterns: meta.intents.clone(),
            skill_name: meta.name.clone(),
            session: None,
        })
    }));
    rules
}

/// 重建进程级技能路由表：app 启动调一次；skills_import / skills_delete 成功后再调。
pub fn rebuild_intent_routes(app: &AppHandle) {
    crate::intent_router::rebuild_routes(intent_rules_from_dirs(&skill_search_paths(app)));
}

/// 技能能否被广告给 LLM（系统提示技能清单）：启用中且非会话档。
/// 禁用的不广告（否则模型调 use_skill 才被 preflight 拒绝，与路由表口径不一致）；
/// 会话档不广告（跨会话/重启不生效，进清单等于教别的会话点名一个用不了的技能）。
fn llm_advertisable(s: &SkillInfo) -> bool {
    s.enabled && !s.session_scoped
}

/// 技能清单块：注入系统提示词尾部（progressive disclosure 第一层）。
/// 过滤 enabled=false——禁用技能不再被广告给 LLM
/// （否则模型调 use_skill 才被 preflight 拒绝，与路由表口径不一致）。
pub fn build_skill_block<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let all = scan_skills(app);
    let skills: Vec<SkillInfo> = all.into_iter().filter(llm_advertisable).collect();
    render_skill_block(skills, false)
}

/// 语义化变体——按 query↔描述嵌入余弦排序，截前 5 条展开描述，
/// 其余合并为一行名称提及（progressive disclosure 纪律：清单不宜过长）。
/// query 为空 / embed 引擎不可用 → 降级为原样全清单（与同步版行为一致）。
/// embed_text 同步阻塞 ONNX → 本函数为 async，重活包 spawn_blocking。
pub async fn build_skill_block_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    query: Option<&str>,
) -> String {
    let all = scan_skills(app);
    let enabled: Vec<SkillInfo> = all.into_iter().filter(llm_advertisable).collect();
    if enabled.is_empty() {
        return "已安装技能：无".into();
    }
    let q = query
        .map(|q| q.trim().to_string())
        .filter(|q| !q.is_empty());
    let Some(q) = q else {
        return render_skill_block(enabled, false);
    };
    let enabled_for_rank = enabled.clone();
    let ranked = crate::py::document::spawn_blocking_map(move || {
        Ok(super::recommend::rank_skills(&q, enabled_for_rank))
    })
    .await
    .unwrap_or_else(|_| {
        // join 失败兜底：按名称序全清单（不丢技能可见性）
        let mut v = enabled;
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    });
    render_skill_block(ranked, true)
}

/// 清单渲染（拆分）：recommended=true 时前 5 条展开描述、其余合并一行提及
fn render_skill_block(skills: Vec<SkillInfo>, recommended: bool) -> String {
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
    if recommended && skills.len() > 5 {
        let head = &skills[..5];
        let rest: Vec<String> = skills[5..].iter().map(|s| s.name.clone()).collect();
        out.push_str("（按与本任务的相关度排序，前 5 个最相关）\n");
        for s in head {
            let desc = if s.description.is_empty() {
                "(无描述)".to_string()
            } else {
                s.description.chars().take(120).collect::<String>()
            };
            out.push_str(&format!("- {}: {}\n", s.name, desc));
        }
        out.push_str(&format!(
            "另有 {} 个技能：{}（需要时可用 use_skill 读取）\n",
            rest.len(),
            rest.join("、")
        ));
        return out;
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

// tauri 命令（设置页技能管理）

/// 已安装技能列表（设置页展示）
#[tauri::command]
pub fn skills_list(app: AppHandle) -> Vec<SkillInfo> {
    scan_skills(&app)
}

/// 打开技能目录（设置页「打开目录」按钮用；目录不存在则先创建）。
/// Rust 侧 opener 直接打开——前端 openPath 受 capability 白名单（$APPDATA/**）
/// 约束，便携模式下技能目录锚定在 exe 旁、不在 $APPDATA 内会被拒；
/// 与 open_file_path 同方案。
#[tauri::command]
pub fn skills_open_dir(app: AppHandle) -> CommandResult<()> {
    let dir = skills_dir(&app);
    std::fs::create_dir_all(&dir)?;
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| CommandError::IoError(format!("打开技能目录失败：{e}")))
}

/// 技能启用开关（设置页拨杆）：把 `enabled` 写进技能自身 SKILL.md 的 frontmatter。
///
/// **单一事实源 = frontmatter 的 `enabled` 键**（思路同 VS Code 把技能开关做成
/// frontmatter 属性）：清单过滤（`build_skill_block`）、意图路由
/// （`intent_rules_from_dirs`）、启动预审（`runtime::preflight`）三道既有闸
/// 都以 `meta.enabled` 为准——改一处即全链路生效，不另开状态存储
/// （两份事实源必然漂移，且用户手改 SKILL.md 时不知道该改哪一份）。
///
/// 落盘目标是**生效副本**（`effective_skill_md`）：与读取侧同一份，否则
/// 「关的是 A、生效的是 B」。
///
/// 原子写：同目录 tmp + rename——中途断电不留半份 SKILL.md
/// （技能文件被截断 = 整个技能失效）。
#[tauri::command]
pub fn skills_set_enabled(app: AppHandle, name: String, enabled: bool) -> CommandResult<()> {
    if let Err(reason) = validate_skill_name(&name) {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason,
        });
    }
    let Some(md) = effective_skill_md(&skill_search_paths(&app), &name) else {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("技能「{name}」不存在（检查设置页技能列表）"),
        });
    };
    write_enabled_to(&md, &name, enabled)?;
    // 意图路由是进程级缓存：关掉的技能必须立刻从路由表里消失，
    // 否则用户消息仍会被路由进一个已被禁用的技能。
    rebuild_intent_routes(&app);
    crate::audit_event!(
        &app,
        crate::audit::AuditLevel::Info,
        "skill.enabled_changed",
        "name" => name.clone(),
        "enabled" => enabled
    );
    Ok(())
}

/// 把 `enabled` 写进指定 SKILL.md（原子写）。抽成独立函数是为了脱离 AppHandle 单测。
fn write_enabled_to(md: &std::path::Path, name: &str, enabled: bool) -> Result<(), CommandError> {
    let text = std::fs::read_to_string(md)
        .map_err(|e| CommandError::IoError(format!("读取 {} 失败：{e}", md.display())))?;
    // 改写后必须能被 parse_meta 读回目标值，读不回就不写
    // （写一份「开关点了没反应」的技能比报错更糟）。
    let Some(rewritten) = set_frontmatter_enabled(&text, enabled) else {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("技能「{name}」的 frontmatter 改写失败（写回后解析器读不回目标值）"),
        });
    };
    crate::db::paths::atomic_write(md, &rewritten).map_err(CommandError::IoError)
}

/// 导入源 SKILL.md 字节上限：导入口无上限 read_to_string 会把超大文件整读进内存。
/// 256KB = 创建侧 `CREATE_MAX_SKILL_CHARS`（64K 字符）的 UTF-8 最坏字节数（4 B/char），
/// 与既有创建侧策略对齐的宽松上界。
pub(crate) const IMPORT_MAX_SKILL_MD_BYTES: u64 = 256 * 1024;

/// 读入导入源的 SKILL.md：先过字节大小闸再读（纯路径参数，单测直打）。
fn read_import_skill_md(src: &std::path::Path) -> Result<String, CommandError> {
    let skill_md = src.join("SKILL.md");
    let oversize = std::fs::metadata(&skill_md)
        .map(|m| m.len() > IMPORT_MAX_SKILL_MD_BYTES)
        .unwrap_or(false); // metadata 失败交给下方 read 的既有错误路径
    if oversize {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("SKILL.md 超过 {IMPORT_MAX_SKILL_MD_BYTES} 字节上限，拒绝导入"),
        });
    }
    std::fs::read_to_string(&skill_md).map_err(|_| "该文件夹没有 SKILL.md，不是有效技能".into())
}

/// 导入技能文件夹：校验含 SKILL.md，拷贝到数据目录 skills/<name>（重名拒绝，需先删）。
#[tauri::command]
pub fn skills_import(app: AppHandle, path: String) -> CommandResult<String> {
    let src = std::path::PathBuf::from(&path);
    if !src.is_dir() {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: "请选择技能文件夹".to_string(),
        });
    }
    let text = read_import_skill_md(&src)?;
    let dir_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = parse_meta(&text, &dir_name);
    let name = meta.name.clone();
    // 校验最终技能名（frontmatter 名非法时 parse 回退目录名，
    // 目录名本身也可能非法）——非法名装上后 load/delete 都拒绝，装得上用不了删不掉
    if let Err(reason) = validate_skill_name(&name) {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason,
        });
    }
    // 准入核与 create_skill / install_skill 同一口径（description 必填 / 危险词 /
    // intents 宽度闸）：导入是第三方内容的入口，不能绕过闸门落盘
    if let Err(reason) = validate_created_meta(&meta, &text) {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("拒绝导入：{reason}"),
        });
    }
    let dir = skills_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&name);
    // 原子排他占位（替代 exists 检查）：并发双导入（双击/两窗口）同抢一个 dest
    // 时只有一个 create_dir 能成功，败者在此被拒——双双闯过 exists 检查会拷出
    // 互相覆盖的半份目录。dest 是文件（非目录）时同样在此被拒。
    std::fs::create_dir(&dest).map_err(|_| CommandError::SkillLoadFailed {
        name: name.clone(),
        reason: "同名技能已存在：请先在设置页删除".into(),
    })?;
    // 递归拷贝（技能可能带 scripts/ 等资源）；拷贝失败移除占位目录，
    // 不留半拷贝残骸堵死重试
    if let Err(e) = copy_dir_all(&src, &dest) {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(e.into());
    }
    // 安装成功即按 frontmatter intents 生成该技能的路由条目
    rebuild_intent_routes(&app);
    Ok(name)
}

// ── create_skill：机器人自建技能（工具口） ──

/// 删除技能（整目录）
///
/// 删除范围：用户装的 skill 落在 `skills_dir()` (data dir)；debug build 下
/// dev mock (`target/debug/skills/`) 也会贡献 skill 列表。两者都要清，
/// 否则点删除 UI 里只跟了目录二一处的 skill 仍会显示。
///
/// 两者可能在 debug build 下指向同一路径（之前 dev-mode 设计 bug
/// 修复前），以 `already_removed` set 去重避免双删。
/// release build 下 `dev_skills_dir()` 整个函数不存在，第二分支自动不编译。
#[tauri::command]
pub fn skills_delete(app: AppHandle, name: String) -> CommandResult<()> {
    if let Err(reason) = validate_skill_name(&name) {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason,
        });
    }
    let mut already_removed: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut try_remove = |path: std::path::PathBuf| -> Result<(), CommandError> {
        if !path.exists() {
            return Ok(());
        }
        if !already_removed.insert(path.clone()) {
            return Ok(()); // 已删过，同一路径 skip
        }
        std::fs::remove_dir_all(&path).map_err(|e| CommandError::IoError(e.to_string()))?;
        Ok(())
    };
    // 数据目录（用户装的 skill）
    try_remove(skills_dir(&app).join(&name))?;
    // 内置技能物化副本（删除后运行期内不复活；应用更新带新版本号时重新物化）
    try_remove(super::builtin::builtin_skills_dir(&app).join(&name))?;
    // dev mock 目录（debug build 下 dev_skills_dir 贡献列表）
    #[cfg(debug_assertions)]
    {
        if let Some(dev_dir) = dev_skills_dir() {
            try_remove(dev_dir.join(&name))?;
        }
    }
    // 卸载即移除该技能的路由条目
    rebuild_intent_routes(&app);
    Ok(())
}

/// 递归深度上限：skills_import 是 IPC 入口、拖入目录属不可信输入，
/// 无界递归会在异常构造的深嵌套上耗尽栈
const COPY_DIR_MAX_DEPTH: usize = 16;

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    copy_dir_all_at(src, dst, 0)
}

fn copy_dir_all_at(
    src: &std::path::Path,
    dst: &std::path::Path,
    depth: usize,
) -> Result<(), String> {
    if depth >= COPY_DIR_MAX_DEPTH {
        return Err(format!(
            "目录嵌套超过 {COPY_DIR_MAX_DEPTH} 层上限，拒绝拷贝：{}",
            src.display()
        ));
    }
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
            copy_dir_all_at(&s, &d, depth + 1)?;
        } else if ty.is_file() {
            std::fs::copy(&s, &d).map_err(|e| e.to_string())?;
        } else {
            // 只放行普通文件：FIFO/socket/device 落 fs::copy 会以读端打开，
            // FIFO 无写端时 read 永久挂起 IPC 线程——fail-closed 拒绝
            return Err(format!("目录含不支持的特殊文件，拒绝拷贝：{}", s.display()));
        }
    }
    Ok(())
}

// 只被本模块的测试用；非测试构建里没人引用，转出会被 rustc 判 unused。
// 放在测试模块旁边而不是文件头——源码文本守卫按「首个 #[cfg(test)] 之前」
// 切生产段，测试专用的 cfg(test) 项放前面会把生产代码整段切掉。
#[cfg(test)]
pub(crate) use create::create_skill_at;

#[cfg(test)]
mod tests;
