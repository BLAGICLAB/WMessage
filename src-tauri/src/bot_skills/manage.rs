use super::parse::{
    parse_meta, parse_skill_steps, strip_frontmatter, unknown_tool_names, SkillMeta,
    SKILL_NAME_CHARS_OK,
};
use crate::error::{CommandError, CommandResult};
use serde::Serialize;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// frontmatter enabled（清单注入需要按它过滤）
    pub enabled: bool,
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
    let mut rules = visit_skill_dirs(dirs, |meta, _dir_name, _text| {
        if !meta.enabled {
            return None;
        }
        crate::intent_router::explicit_name_pattern(&meta.name).map(|pattern| {
            crate::intent_router::IntentRule {
                patterns: vec![pattern],
                skill_name: meta.name.clone(),
            }
        })
    });
    rules.extend(visit_skill_dirs(dirs, |meta, _dir_name, _text| {
        if !meta.enabled || meta.intents.is_empty() {
            return None;
        }
        Some(crate::intent_router::IntentRule {
            patterns: meta.intents.clone(),
            skill_name: meta.name.clone(),
        })
    }));
    rules
}

/// 重建进程级技能路由表：app 启动调一次；skills_import / skills_delete 成功后再调。
pub fn rebuild_intent_routes(app: &AppHandle) {
    crate::intent_router::rebuild_routes(intent_rules_from_dirs(&skill_search_paths(app)));
}

/// 技能清单块：注入系统提示词尾部（progressive disclosure 第一层）。
/// 过滤 enabled=false——禁用技能不再被广告给 LLM
/// （否则模型调 use_skill 才被 preflight 拒绝，与路由表口径不一致）。
pub fn build_skill_block<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let all = scan_skills(app);
    let skills: Vec<SkillInfo> = all.into_iter().filter(|s| s.enabled).collect();
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
    let enabled: Vec<SkillInfo> = all.into_iter().filter(|s| s.enabled).collect();
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
    if let Err(reason) = validate_created_meta(&meta) {
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

/// SKILL.md 正文字符上限（对齐业界 SKILL.md ≤500 行/≈5K tokens 规范的宽松裕度）
const CREATE_MAX_SKILL_CHARS: usize = 64 * 1024;
/// 单附件上限；附件个数上限；附件总字节上限
const CREATE_MAX_FILE_BYTES: usize = 200 * 1024;
const CREATE_MAX_FILES: usize = 10;
const CREATE_MAX_TOTAL_BYTES: usize = 1024 * 1024;

/// 创建校验的元数据部分（纯逻辑可测）：description 必填（技能清单与路由展示必需）
/// + intents/description 危险关键词预检（与运行时 preflight 同一黑名单——把拒绝
/// 提前到落盘前，而非装上后每次启动才炸）+ intents 宽度闸（装上即进进程级路由表，
/// 过宽模式会把此后每条用户消息都劫持进该技能）。
pub(crate) fn validate_created_meta(meta: &SkillMeta) -> Result<(), String> {
    if meta.description.trim().is_empty() {
        return Err("frontmatter 缺少 description（技能清单与路由展示必需）".into());
    }
    let hay = format!(
        "{} {}",
        meta.description.to_lowercase(),
        meta.intents.join(" ")
    );
    if let Some(hit) = super::runtime::INTENT_BLACKLIST
        .iter()
        .find(|b| hay.contains(&b.to_lowercase()))
    {
        return Err(format!(
            "intents/description 命中危险关键词「{hit}」，拒绝创建"
        ));
    }
    for p in &meta.intents {
        crate::intent_router::check_intent_pattern(p)
            .map_err(|reason| format!("intents「{p}」{reason}"))?;
    }
    Ok(())
}

/// 待创建技能的元数据核（纯逻辑、可测）：解析 frontmatter + 名称一致性 +
/// 准入核。名称一致性：frontmatter name 必须与技能名一致——路由/清单用
/// meta.name，磁盘目录用参数名，失配会产出「路由指 frontmatter 名、目录叫参数名」
/// 的僵尸技能（load/delete 都按目录名找）。
fn prepare_created_meta(name: &str, content: &str) -> Result<SkillMeta, String> {
    let meta = parse_meta(content, name);
    if meta.name != name {
        return Err(format!(
            "SKILL.md frontmatter name「{}」与技能名「{name}」不一致——frontmatter name 必须与技能名完全一致",
            meta.name
        ));
    }
    validate_created_meta(&meta)?;
    Ok(meta)
}

/// 落盘核（同步、可测）：name 须已过 validate_skill_name；content 为完整 SKILL.md
/// 文本；files 为相对技能目录的附件（path, content）。排他占位 + 失败清理，
/// 与 skills_import 同一模式。返回写入的附件个数。
fn create_skill_at(
    dir: &std::path::Path,
    name: &str,
    content: &str,
    files: &[(String, String)],
) -> Result<usize, CommandError> {
    if content.chars().count() > CREATE_MAX_SKILL_CHARS {
        return Err(CommandError::InvalidArgument {
            field: "content".into(),
            value: String::new(),
            reason: format!("SKILL.md 超过 {CREATE_MAX_SKILL_CHARS} 字符上限"),
        });
    }
    // frontmatter name 必须与目录名一致 + 准入核（description 必填 / 危险词 /
    // intents 宽度闸）：解析与校验口径单源，工具层与落盘核共用
    prepare_created_meta(name, content).map_err(|reason| CommandError::InvalidArgument {
        field: "content".into(),
        value: String::new(),
        reason,
    })?;
    if files.len() > CREATE_MAX_FILES {
        return Err(CommandError::InvalidArgument {
            field: "files".into(),
            value: files.len().to_string(),
            reason: format!("附件超过 {CREATE_MAX_FILES} 个上限"),
        });
    }
    let mut total = content.len();
    let mut cleaned: Vec<(std::path::PathBuf, &str)> = Vec::new();
    for (p, c) in files {
        let pb = std::path::Path::new(p);
        if pb.is_absolute() || p.contains("..") {
            return Err(CommandError::InvalidArgument {
                field: "files.path".into(),
                value: p.clone(),
                reason: "附件路径必须为技能目录内的相对路径".into(),
            });
        }
        // 逐组件字符集校验（字母/数字/点/-/_，禁空组件）：穿越与怪名都在这里挡下
        for comp in pb.components() {
            let s = match comp {
                std::path::Component::Normal(s) => s.to_string_lossy().to_string(),
                _ => {
                    return Err(CommandError::InvalidArgument {
                        field: "files.path".into(),
                        value: p.clone(),
                        reason: "附件路径含非法组件（绝对路径/./.. 等）".into(),
                    })
                }
            };
            if s.is_empty()
                || !s
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
                || s == "."
                || s == ".."
            {
                return Err(CommandError::InvalidArgument {
                    field: "files.path".into(),
                    value: p.clone(),
                    reason: format!("附件路径组件「{s}」含不允许的字符"),
                });
            }
        }
        if pb
            .file_name()
            .map(|f| f.to_string_lossy().eq_ignore_ascii_case("skill.md"))
            .unwrap_or(false)
        {
            return Err(CommandError::InvalidArgument {
                field: "files.path".into(),
                value: p.clone(),
                reason: "SKILL.md 由 content 参数提供，附件不得覆盖".into(),
            });
        }
        if c.len() > CREATE_MAX_FILE_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "files.content".into(),
                value: p.clone(),
                reason: format!("单附件超过 {} 字节上限", CREATE_MAX_FILE_BYTES),
            });
        }
        total += c.len();
        if total > CREATE_MAX_TOTAL_BYTES {
            return Err(CommandError::InvalidArgument {
                field: "files".into(),
                value: p.clone(),
                reason: format!("附件总大小超过 {} 字节上限", CREATE_MAX_TOTAL_BYTES),
            });
        }
        cleaned.push((pb.to_path_buf(), c.as_str()));
    }

    // 排他占位（同 skills_import：并发双建只有一个成功）
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let dest = dir.join(name);
    std::fs::create_dir(&dest).map_err(|_| CommandError::SkillLoadFailed {
        name: name.to_string(),
        reason: "同名技能已存在：如需覆盖请先删除".into(),
    })?;
    let write_all = || -> Result<(), CommandError> {
        std::fs::write(dest.join("SKILL.md"), content)
            .map_err(|e| CommandError::IoError(e.to_string()))?;
        for (p, c) in &cleaned {
            let target = dest.join(p);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CommandError::IoError(e.to_string()))?;
            }
            std::fs::write(target, c).map_err(|e| CommandError::IoError(e.to_string()))?;
        }
        Ok(())
    };
    if let Err(e) = write_all() {
        // 创建已失败，回滚（清理半拷贝）再失败时两层信息都要让调用方看到——
        // 静默吞掉回滚失败会留下堵死重试的残骸且无迹可查
        if let Err(rb) = std::fs::remove_dir_all(&dest) {
            return Err(CommandError::IoError(format!(
                "{e}（回滚清理目录 {} 也失败：{rb}，请手动删除后重试）",
                dest.display()
            )));
        }
        return Err(e);
    }
    Ok(cleaned.len())
}

/// 机器人自建技能：把对话中沉淀的可复用流程固化为 SKILL.md（可带脚本/模板附件），
/// 创建即生效（路由重建）。与 skills_import 同一校验核/排他落盘/路由重建口径；
/// intents 危险词在创建时预检（同运行时 preflight 黑名单），并过 intents 宽度闸
/// （装上即进程级路由，过宽模式不得进）。
///
/// 落盘前有确认窗：技能装上即进**进程级**路由表，此后命中的用户消息会被直接
/// 送进该技能（正文进系统提示；mode=auto 时整段 DSL 无 LLM 参与地执行）。
/// 与 install_skill 同一姿态——「从公网下载要确认，本地写一个不要」不是设计。
pub async fn tool_create_skill(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let v = crate::bot::parse_args(args);
    let Some(name) = v["name"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("create_skill 缺少 name（技能名：字母/数字/-/_）".to_string());
    };
    if let Err(reason) = validate_skill_name(&name) {
        return ToolResult::denied(format!("create_skill：{reason}"));
    }
    let Some(content) = v["content"].as_str().map(|s| s.to_string()) else {
        return ToolResult::denied(
            "create_skill 缺少 content（完整 SKILL.md 文本，frontmatter 必含 description）"
                .to_string(),
        );
    };
    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(arr) = v["files"].as_array() {
        for f in arr {
            match (f["path"].as_str(), f["content"].as_str()) {
                (Some(p), Some(c)) => files.push((p.to_string(), c.to_string())),
                _ => {
                    return ToolResult::denied("files 条目必须同时包含 path 与 content".to_string())
                }
            }
        }
    }

    // 准入核先行（解析 + 名称一致 + 危险词 + intents 宽度闸）：不合格的技能
    // 不该占用用户一次确认
    //
    // 口径：这些分支**工具根本没执行**（没落盘、没进路由表）——与工具层其余
    // 分支一致（缺 title、空 query、开关未开都是 denied），也与 install_skill
    // 对同一准入核的口径一致（那边是 denied）。走 error 会让「被闸拦下」记成
    // ERROR 进 error.log——被拒是预期内控制流，不是故障。
    let meta = match prepare_created_meta(&name, &content) {
        Ok(m) => m,
        Err(reason) => return ToolResult::denied(format!("create_skill：{reason}")),
    };
    // 确认窗展示 intents 与 mode：这两项决定「建成后哪些话会被路由进来」和
    // 「命中后是否绕开 LLM 直接执行」——不给用户看就等于让用户盲批
    let confirm_text = format!(
        "创建技能「{name}」并立即生效（重启后仍有效）。触发词 intents：{}；执行模式 mode：{}（auto = 命中后不经 LLM 复核直接执行技能步骤）。",
        if meta.intents.is_empty() {
            "未声明（不会自动触发）".to_string()
        } else {
            meta.intents.join("、")
        },
        meta.mode
    );
    let approved = crate::bot_slash::ask_user_confirm(
        app,
        "create_skill",
        &confirm_text,
        interactive,
        session_id,
    )
    .await;
    if !approved {
        return ToolResult::denied("用户拒绝了创建，技能未创建".to_string());
    }

    let dir = skills_dir(app);
    let dir_log = dir.display().to_string();
    crate::bot::audit_log(
        app,
        &format!(
            "skill_create | {} | files: {}",
            crate::bot::escape_for_log(&name, 100),
            files.len()
        ),
    );
    let name_for_task = name.clone();
    let created = crate::py::document::spawn_blocking_map(move || {
        create_skill_at(&dir, &name_for_task, &content, &files).map_err(|e| e.to_string())
    })
    .await;
    match created {
        Ok(file_count) => {
            rebuild_intent_routes(app);
            ToolResult::ok(
                format!(
                    "技能「{name}」已创建并生效（附件 {file_count} 个），技能目录：{dir_log}。路由已重建：用户说出该技能 intents 关键词即会进入该技能流程。"
                ),
                Vec::new(),
            )
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "skill_create_failed | {} | {}",
                    crate::bot::escape_for_log(&name, 100),
                    crate::bot::escape_for_log(&e, 200)
                ),
            );
            ToolResult::error(format!("创建失败：{e}"), Vec::new())
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    // ── create_skill_at：机器人自建技能落盘核 ──

    #[test]
    fn create_skill_writes_doc_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let content =
            "---\nname: my-skill\ndescription: 测试技能\nintents:\n  - 测试流程\n---\n# 步骤\n做事";
        let files = vec![("scripts/run.py".to_string(), "print('hi')".to_string())];
        let n = create_skill_at(tmp.path(), "my-skill", content, &files).unwrap();
        assert_eq!(n, 1);
        assert!(tmp.path().join("my-skill/SKILL.md").is_file());
        assert!(tmp.path().join("my-skill/scripts/run.py").is_file());
    }

    #[test]
    fn create_skill_rejects_duplicate_name() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("dup")).unwrap();
        let err = create_skill_at(
            tmp.path(),
            "dup",
            "---\nname: dup\ndescription: d\n---\nx",
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::SkillLoadFailed { .. }), "{err}");
    }

    /// 复现：frontmatter `name:` 与目录名失配时，旧实现照单全收——产出
    /// 「路由/清单用 frontmatter 名、磁盘目录用参数名」的僵尸技能
    ///（load/delete 都按目录名找，frontmatter 名指到的技能不存在）。
    /// 修复后 create_skill_at 必须拒绝。
    #[test]
    fn create_skill_rejects_frontmatter_name_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let err = create_skill_at(
            tmp.path(),
            "dir-name",
            "---\nname: frontmatter-name\ndescription: d\n---\nx",
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument { .. }), "{err}");
        assert!(
            err.to_string().contains("frontmatter"),
            "错误信息须说清 frontmatter name 失配，got: {err}"
        );
        assert!(
            !tmp.path().join("dir-name").exists(),
            "拒绝时不得留下半份目录"
        );
    }

    #[test]
    fn create_skill_rejects_blacklisted_intents() {
        let tmp = tempfile::tempdir().unwrap();
        let err = create_skill_at(
            tmp.path(),
            "bad",
            "---\nname: bad\ndescription: 一键清空所有数据\n---\nx",
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("危险关键词"), "{err}");
    }

    #[test]
    fn create_skill_requires_description() {
        let tmp = tempfile::tempdir().unwrap();
        let err = create_skill_at(tmp.path(), "x", "---\nname: x\n---\n正文", &[]).unwrap_err();
        assert!(err.to_string().contains("description"), "{err}");
    }

    #[test]
    fn create_skill_rejects_traversal_and_skill_md_override() {
        let tmp = tempfile::tempdir().unwrap();
        let traversal = vec![("../evil.txt".to_string(), "x".to_string())];
        let err = create_skill_at(
            tmp.path(),
            "x",
            "---\nname: x\ndescription: d\n---\nb",
            &traversal,
        )
        .unwrap_err();
        assert!(err.to_string().contains("相对路径"), "{err}");

        let override_md = vec![("SKILL.md".to_string(), "x".to_string())];
        let err = create_skill_at(
            tmp.path(),
            "x",
            "---\nname: x\ndescription: d\n---\nb",
            &override_md,
        )
        .unwrap_err();
        assert!(err.to_string().contains("不得覆盖"), "{err}");
    }

    #[test]
    fn create_skill_enforces_file_count_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<(String, String)> = (0..11)
            .map(|i| (format!("f{i}.txt"), "x".to_string()))
            .collect();
        let err = create_skill_at(
            tmp.path(),
            "x",
            "---\nname: x\ndescription: d\n---\nb",
            &files,
        )
        .unwrap_err();
        assert!(err.to_string().contains("上限"), "{err}");
    }

    // ── 准入核：intents 宽度闸（装上即进程级路由）──

    /// 复现：intents 写成 `.*` 时旧实现照单全收——技能装上即进进程级路由表，
    /// 此后每条用户消息都被劫持进该技能（正文进系统提示，mode=auto 时无 LLM
    /// 复核直接执行）。修复后落盘前必须拒。
    #[test]
    fn create_skill_rejects_overly_broad_intents() {
        let tmp = tempfile::tempdir().unwrap();
        let err = create_skill_at(
            tmp.path(),
            "wide",
            "---\nname: wide\ndescription: d\nintents:\n  - .*\n---\nx",
            &[],
        )
        .unwrap_err();
        assert!(err.to_string().contains("过宽"), "{err}");
        assert!(!tmp.path().join("wide").exists(), "拒绝时不得留下半份目录");
    }

    /// 宽度闸不得误杀真实意图模式（量词 / 字符类 / 分支 / 转义混用）。
    #[test]
    fn create_skill_accepts_realistic_composite_intents() {
        let tmp = tempfile::tempdir().unwrap();
        let content = "---\nname: docx-fix\ndescription: 文档润色\nintents:\n  - (?i)(润色|修订).{0,15}(word|文档)\n  - (?is)(润色|修订)[\\s\\S]*\\.docx?\n---\nx";
        assert!(
            create_skill_at(tmp.path(), "docx-fix", content, &[]).is_ok(),
            "合法复合 intents 模式不该被宽度闸拒掉"
        );
    }

    /// 导入入口必须走准入核：它是第三方内容的入口，绕过闸门等于装上哑弹技能
    /// 或劫持路由（skills_import 需要 AppHandle，纯函数单测够不到，故以源码
    /// 文本钉住「调用存在」）。
    #[test]
    fn skills_import_runs_the_admission_check() {
        let body = manage_src_before_tests();
        let body = body
            .split_once("pub fn skills_import(")
            .expect("找不到 skills_import")
            .1
            .split("\n}\n")
            .next()
            .expect("skills_import 应以顶格 } 收尾");
        assert!(
            body.contains("validate_created_meta(&meta)"),
            "skills_import 必须调 validate_created_meta：导入是第三方内容入口，不能绕过准入核"
        );
    }

    /// create_skill 落盘前必须有确认窗，且确认窗要展示 intents 与 mode——
    /// 这两项决定「哪些话会被路由进来」和「命中后是否绕开 LLM 直接执行」。
    #[test]
    fn tool_create_skill_confirms_before_writing_and_shows_intents_and_mode() {
        let body = manage_src_before_tests();
        let body = body
            .split_once("pub async fn tool_create_skill(")
            .expect("找不到 tool_create_skill")
            .1
            .split("\n}\n")
            .next()
            .expect("tool_create_skill 应以顶格 } 收尾");
        let confirm = body
            .find("ask_user_confirm(")
            .expect("tool_create_skill 必须有确认窗");
        let write = body
            .find("create_skill_at(")
            .expect("tool_create_skill 必须走 create_skill_at 落盘");
        assert!(confirm < write, "确认窗必须在落盘之前");
        assert!(
            body.contains("meta.intents.join"),
            "确认窗必须展示 intents：不给用户看触发词就等于让用户盲批"
        );
        assert!(
            body.contains("meta.mode"),
            "确认窗必须展示 mode：auto 模式命中后不经 LLM 复核直接执行"
        );
    }

    /// create_skill 的前置拒绝一律 denied（工具根本没执行），只有落盘失败才是 error。
    ///
    /// 口径三处一致：① 工具层其余分支（缺 title、空 query、开关未开）都是 denied；
    /// ② install_skill 对**同一个**准入核也是 denied；③ 审计层
    /// `ToolStatus::Denied → WARN`（「被拒是预期内的控制流，不是故障」）。
    /// 走 error 会把「被闸拦下」记成 ERROR 进 error.log——同因不同级正是
    /// 实机复验里 D-2 那条修复要根治的形态。
    #[test]
    fn tool_create_skill_pre_execution_rejections_are_denied() {
        let body = manage_src_before_tests();
        let body = body
            .split_once("pub async fn tool_create_skill(")
            .expect("找不到 tool_create_skill")
            .1
            .split("\n}\n")
            .next()
            .expect("tool_create_skill 应以顶格 } 收尾");
        let admission = body
            .split_once("prepare_created_meta(&name, &content)")
            .expect("tool_create_skill 必须走准入核")
            .1
            .split(';')
            .next()
            .expect("准入核分支应有分号收尾");
        assert!(
            admission.contains("ToolResult::denied"),
            "准入核拒绝（宽度闸 / 危险词）必须走 denied：{admission}"
        );
        assert_eq!(
            body.matches("ToolResult::error(").count(),
            1,
            "create_skill 里除落盘执行失败外不该再有 error——前置拒绝都是 denied：{body}"
        );
    }

    /// 本文件生产代码切片（测试模块之前）——源码文本守卫用。
    fn manage_src_before_tests() -> &'static str {
        include_str!("manage.rs")
            .split_once("#[cfg(test)]")
            .expect("manage.rs 应有测试模块")
            .0
    }

    // ── scan_skill_dirs 多目录去重 ──

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
        let text =
            format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nbody\n");
        std::fs::write(skill_dir.join("SKILL.md"), text).unwrap();
    }

    /// 正常模式（`app_data_dir()` 可得）：`skills_dir` 必须返回 `app_data_dir()/skills`。
    /// 这正是 `SkillsPanel` 的 `openPath` 目标，也是 opener scope `$APPDATA/**` 覆盖的路径。
    /// 锁此契约：若哪天误改成总是 fallback（便携 `data_dir()`），正常模式下 SkillsPanel
    /// 的 openPath 会被 scope 拒绝（C2a 登记的 fallback 边缘 case 被扩大到常态）。
    /// 不依赖 mock app 语义（纯函数内核）。
    #[test]
    fn skills_dir_normal_mode_uses_app_data_dir() {
        let got = data_dir_subdir(
            Some(std::path::PathBuf::from("/x/AppData/com.renshi.wmessage")),
            || panic!("正常模式不得调用 fallback（会触碰 data_dir 副作用）"),
            "skills",
        );
        assert_eq!(
            got,
            std::path::PathBuf::from("/x/AppData/com.renshi.wmessage/skills")
        );
    }

    /// 失败回退：`app_data_dir()` 失败 → `data_dir()/skills`（便携场景可能不在 `$APPDATA` 下）。
    /// 仅锁「回退目标 = data_dir/skills」这一事实；是否该扩大 scope 是 Phase 6 的产品决策。
    #[test]
    fn skills_dir_fallback_mode_uses_data_dir() {
        let got = data_dir_subdir(None, || std::path::PathBuf::from("/x/portable"), "skills");
        assert_eq!(got, std::path::PathBuf::from("/x/portable/skills"));
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

        let nonexistent =
            std::path::PathBuf::from("/tmp/wmessage-nonexistent-dir-xxx-does-not-exist");
        let _ = std::fs::remove_dir_all(&nonexistent); // 确保不存在
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

    // ── intent_rules_from_dirs（未安装不得有路由） ──

    #[test]
    fn intent_rules_cover_enabled_skills_with_name_rule_before_intent_rule() {
        let temp = make_tempdir("intent-rules");
        // enabled + 多行 intents（含量词逗号，验证不被切碎）→ 点名规则 + intent 规则
        let dir_a = temp.0.join("skill-a");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::write(
            dir_a.join("SKILL.md"),
            "---\nname: skill-a\ndescription: a\nintents:\n  - '(?i)(润色|修订).{0,15}(word|文档)'\n  - '(?is)\\.docx?'\n---\nbody\n",
        )
        .unwrap();
        // enabled 但无 intents → 仍有点名规则（点名不依赖 intents）
        make_mock_skill(&temp.0, "skill-b", "no-intents");
        // enabled: false → 不产生路由
        let dir_c = temp.0.join("skill-c");
        std::fs::create_dir_all(&dir_c).unwrap();
        std::fs::write(
            dir_c.join("SKILL.md"),
            "---\nname: skill-c\nenabled: false\nintents: [\"zzz\"]\n---\nbody\n",
        )
        .unwrap();

        let rules = intent_rules_from_dirs(std::slice::from_ref(&temp.0));
        let names: Vec<&str> = rules.iter().map(|r| r.skill_name.as_str()).collect();
        assert_eq!(
            rules.len(),
            3,
            "skill-a（点名+intent）+ skill-b（只有点名）：{names:?}"
        );
        assert!(
            !names.contains(&"skill-c"),
            "disabled 技能不产生路由：{names:?}"
        );
        // 点名规则整体排在 intent 规则之前
        let a_rules: Vec<&crate::intent_router::IntentRule> =
            rules.iter().filter(|r| r.skill_name == "skill-a").collect();
        assert_eq!(a_rules.len(), 2);
        assert_eq!(a_rules[0].patterns.len(), 1, "点名规则在 intent 规则之前");
        assert_eq!(a_rules[1].patterns.len(), 2, "intent 规则保留全部声明");
        assert!(
            a_rules[1].patterns[0].contains("{0,15}"),
            "量词逗号应完整保留: {}",
            a_rules[1].patterns[0]
        );
        let b_rules: Vec<&crate::intent_router::IntentRule> =
            rules.iter().filter(|r| r.skill_name == "skill-b").collect();
        assert_eq!(b_rules.len(), 1, "无 intents 的技能只有点名规则");
        assert_eq!(b_rules[0].patterns.len(), 1);
    }

    #[test]
    fn intent_rules_empty_dirs_give_empty_table() {
        // 什么都没装 → 空规则集（route 层恒 PassThrough）
        assert!(intent_rules_from_dirs(&[]).is_empty());
    }

    // ── validate_skill_name 抽公共校验 ──

    #[test]
    fn validate_skill_name_accepts_valid_names() {
        assert!(validate_skill_name("gorden-ppt-skill").is_ok());
        assert!(validate_skill_name("minimax_docx").is_ok());
        assert!(validate_skill_name("a").is_ok());
        assert!(validate_skill_name("Skill123").is_ok());
    }

    #[test]
    fn validate_skill_name_rejects_invalid_names() {
        // 空串
        assert!(validate_skill_name("").is_err());
        // 路径分隔符
        assert!(validate_skill_name("a/b").is_err());
        assert!(validate_skill_name("../evil").is_err());
        // 非 ASCII 字母 / 数字 / `-` / `_`
        assert!(validate_skill_name("中文 skill").is_err());
        assert!(validate_skill_name("skill!").is_err());
        assert!(validate_skill_name("skill with space").is_err());
        assert!(validate_skill_name("skill.toml").is_err());
    }

    // ── scan_skill_dirs 非法名跳过 ──

    #[test]
    fn scan_skill_dirs_skips_invalid_name_skill() {
        // 场景：用户拖入一个含非法名的 skill（中文 / 符号 / 路径字符），
        // scan_skill_dirs 静默跳过不进入清单——否则 load_skill_meta 会找到它
        // 但 load/delete 都拒绝，装得上用不了删不掉（留下垃圾目录）。
        let temp = make_tempdir("scan-invalid-name");
        let dir = temp.0.join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        // 合法 skill：应保留
        make_mock_skill(&dir, "valid-skill", "ok");
        // 非法名（中文 + 感叹号）：应跳过
        make_mock_skill(&dir, "中文！skill", "中文带符号，应跳");
        // 非法名（含路径分隔符 / frontmatter 回退到目录名）：应跳过
        make_mock_skill(&dir, "a/b", "路径分隔符");

        let out = scan_skill_dirs(&[dir]);
        assert_eq!(out.len(), 1, "非法名必须被静默跳过");
        assert_eq!(out[0].name, "valid-skill");
    }

    #[test]
    fn scan_skill_dirs_skips_when_only_dir_name_invalid() {
        // 场景：目录名非法但 frontmatter name 合法——
        // 之前只校验 meta.name 时这条会错误地进入清单。修复后双重校验，
        // 目录名 `中文 skill!` + frontmatter `name: anything` 应被跳过。
        // （目录名最终会用于 skills_delete 路径构造，不合法 → 装得上用不了删不掉。）
        let temp = make_tempdir("scan-bad-dirname");
        let dir = temp.0.join("skills");
        std::fs::create_dir_all(&dir).unwrap();
        // 目录名含中文 + 感叹号；frontmatter 用合法 name "anything"
        let skill_dir = dir.join("中文 skill!");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: anything\ndescription: 目录名非法但 frontmatter 合法\n---\n# body\n",
        )
        .unwrap();
        // 合法 skill：应保留
        make_mock_skill(&dir, "valid-skill", "ok");

        let out = scan_skill_dirs(&[dir]);
        assert_eq!(
            out.len(),
            1,
            "非法目录名必须被静默跳过（即使 frontmatter name 合法）"
        );
        assert_eq!(out[0].name, "valid-skill");
    }

    // ── copy_dir_all：不可信拖入目录的拷贝防线 ──

    #[test]
    fn copy_dir_all_copies_nested_files() {
        let tmp = make_tempdir("copy-ok");
        let src = tmp.0.join("src");
        std::fs::create_dir_all(src.join("scripts/sub")).unwrap();
        std::fs::write(src.join("SKILL.md"), b"m").unwrap();
        std::fs::write(src.join("scripts/run.py"), b"p").unwrap();
        std::fs::write(src.join("scripts/sub/a.txt"), b"a").unwrap();
        let dst = tmp.0.join("dst");
        copy_dir_all(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("SKILL.md")).unwrap(), b"m");
        assert_eq!(std::fs::read(dst.join("scripts/run.py")).unwrap(), b"p");
        assert_eq!(std::fs::read(dst.join("scripts/sub/a.txt")).unwrap(), b"a");
    }

    #[test]
    fn copy_dir_all_rejects_deep_nesting() {
        let tmp = make_tempdir("copy-deep");
        let src = tmp.0.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mut cursor = src.clone();
        for i in 0..20 {
            cursor = cursor.join(format!("d{i}"));
            std::fs::create_dir_all(&cursor).unwrap();
        }
        let dst = tmp.0.join("dst");
        let err = copy_dir_all(&src, &dst).unwrap_err();
        assert!(err.contains("层"), "错误须说明深度上限，got: {err}");
    }

    /// FIFO 落 fs::copy 会以读端打开并无写端永久阻塞 IPC 线程：修复后必须
    /// 快速报错（本测试能结束即证不挂起）且 FIFO 不出现在目标。
    #[cfg(unix)]
    #[test]
    fn copy_dir_all_rejects_fifo_without_hanging() {
        let tmp = make_tempdir("copy-fifo");
        let src = tmp.0.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), b"m").unwrap();
        // std 的 mkfifo 尚未稳定，走 libc 造具名管道
        let cpath = std::ffi::CString::new(src.join("pipe").to_string_lossy().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) },
            0,
            "mkfifo 失败"
        );
        let dst = tmp.0.join("dst");
        let r = copy_dir_all(&src, &dst);
        assert!(r.is_err(), "含 FIFO 的目录必须被拒");
        assert!(!dst.join("pipe").exists(), "FIFO 不得出现在目标");
    }

    // ── skills_import SKILL.md 大小闸 ──

    #[test]
    fn import_rejects_oversized_skill_md() {
        let tmp = make_tempdir("import-oversize");
        let src = tmp.0.join("big");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SKILL.md"), vec![b'x'; 256 * 1024 + 1]).unwrap();
        let err = read_import_skill_md(&src).unwrap_err();
        assert!(matches!(err, CommandError::DomainRule { .. }), "{err}");
        assert!(err.to_string().contains("上限"), "got: {err}");
        // 边界内照常读入
        let ok = tmp.0.join("ok");
        std::fs::create_dir_all(&ok).unwrap();
        std::fs::write(ok.join("SKILL.md"), "---\nname: ok\n---\nbody").unwrap();
        assert!(read_import_skill_md(&ok).is_ok());
        // 缺 SKILL.md 走既有错误形态
        let missing = tmp.0.join("missing");
        std::fs::create_dir_all(&missing).unwrap();
        let err = read_import_skill_md(&missing).unwrap_err();
        assert!(err.to_string().contains("SKILL.md"), "got: {err}");
    }
}
