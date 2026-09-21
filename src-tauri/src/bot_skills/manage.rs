use super::parse::{parse_frontmatter, parse_meta, SkillMeta, SKILL_NAME_CHARS_OK};
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
fn skills_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    skills_dir_from(app.path().app_data_dir().ok(), || crate::db::data_dir(app))
}

/// 纯内核（可测，不依赖 AppHandle / mock 语义）：
/// - 正常（`app_data_dir()` 可得）→ `app_data/skills`（**opener scope `$APPDATA/**` 覆盖**）
/// - 失败回退 → `fallback_data_dir()/skills`（便携场景可能不在 `$APPDATA` 下 —— 见
///   `docs/OCR-FIX-PLAN-2026-09-21.md` 的 C2a fallback 边缘 case）
///
/// `fallback` 用闭包懒求值：正常模式绝不触碰 `data_dir()`（其探针/建目录副作用）。
fn skills_dir_from(
    app_data: Option<std::path::PathBuf>,
    fallback_data_dir: impl FnOnce() -> std::path::PathBuf,
) -> std::path::PathBuf {
    match app_data {
        Some(p) => p.join("skills"),
        None => fallback_data_dir().join("skills"),
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

/// Skill 搜索路径列表：数据目录必选 + dev 模式追加 target/debug/skills。
/// 数据目录在前 → scan_skill_dirs 用 HashSet seen 去重时数据目录优先。
/// `#[cfg_attr(not(debug_assertions), allow(dead_code))]` —— release 模式 dev_skills_dir
/// 不存在，整个函数仅返回一个目录（数据目录），避免 dead_code 警告。
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn skill_search_paths<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Vec<std::path::PathBuf> {
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
fn validate_skill_name(name: &str) -> Result<(), String> {
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
    F: Fn(&SkillMeta, &str) -> Option<T>,
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
            if let Some(t) = extract(&meta, &dir_name) {
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
    let mut out: Vec<SkillInfo> = visit_skill_dirs(dirs, |meta, dir_name| {
        // 双重校验：frontmatter name 与目录名都要通过 SKILL_NAME_CHARS_OK。
        // 两者最终都会成为路径的一部分（删 / 加载都用），一个非法就 reject。
        if validate_skill_name(&meta.name).is_err() {
            return None;
        }
        if validate_skill_name(dir_name).is_err() {
            return None;
        }
        Some(SkillInfo {
            name: meta.name.clone(),
            description: meta.description.clone(),
            enabled: meta.enabled,
            last_outcome: None,
        })
    });
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 全量技能路由规则扫描（未安装的技能不得有路由）：
/// 遍历搜索路径读每个 SKILL.md 的 frontmatter `intents`；
/// `enabled: false` / intents 为空 → 该技能不产生路由。同名去重、前面目录优先（与 scan_skill_dirs 一致）。
pub fn intent_rules_from_dirs(
    dirs: &[std::path::PathBuf],
) -> Vec<crate::intent_router::IntentRule> {
    visit_skill_dirs(dirs, |meta, _dir_name| {
        if !meta.enabled || meta.intents.is_empty() {
            return None;
        }
        Some(crate::intent_router::IntentRule {
            patterns: meta.intents.clone(),
            skill_name: meta.name.clone(),
        })
    })
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
    let skills: Vec<&SkillInfo> = all.iter().filter(|s| s.enabled).collect();
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

// ───────────────────────── tauri 命令（设置页技能管理） ─────────────────────────

/// 已安装技能列表（设置页展示）
#[tauri::command]
pub fn skills_list(app: AppHandle) -> Vec<SkillInfo> {
    scan_skills(&app)
}

/// 技能目录路径（设置页「打开目录」按钮用；目录不存在则先创建）
#[tauri::command]
pub fn skills_open_dir(app: AppHandle) -> CommandResult<String> {
    let dir = skills_dir(&app);
    std::fs::create_dir_all(&dir)?;
    Ok(dir.to_string_lossy().to_string())
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
    let skill_md = src.join("SKILL.md");
    let text = std::fs::read_to_string(&skill_md)
        .map_err(|_| "该文件夹没有 SKILL.md，不是有效技能".to_string())?;
    let dir_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (name, _) = parse_frontmatter(&text, &dir_name);
    // 校验最终技能名（frontmatter 名非法时 parse 回退目录名，
    // 目录名本身也可能非法）——非法名装上后 load/delete 都拒绝，装得上用不了删不掉
    if let Err(reason) = validate_skill_name(&name) {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason,
        }
        .into());
    }
    let dir = skills_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join(&name);
    if dest.exists() {
        return Err(CommandError::SkillLoadFailed {
            name: name.clone(),
            reason: "同名技能已存在：请先在设置页删除".into(),
        }
        .into());
    }
    // 递归拷贝（技能可能带 scripts/ 等资源）
    copy_dir_all(&src, &dest)?;
    // 安装成功即按 frontmatter intents 生成该技能的路由条目
    rebuild_intent_routes(&app);
    Ok(name)
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
        let got = skills_dir_from(
            Some(std::path::PathBuf::from("/x/AppData/com.renshi.wmessage")),
            || panic!("正常模式不得调用 fallback（会触碰 data_dir 副作用）"),
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
        let got = skills_dir_from(None, || std::path::PathBuf::from("/x/portable"));
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
    fn intent_rules_collects_only_enabled_skills_with_intents() {
        let temp = make_tempdir("intent-rules");
        // enabled + 多行 intents（含量词逗号，验证不被切碎）→ 产生路由
        let dir_a = temp.0.join("skill-a");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::write(
            dir_a.join("SKILL.md"),
            "---\nname: skill-a\ndescription: a\nintents:\n  - '(?i)(润色|修订).{0,15}(word|文档)'\n  - '(?is)\\.docx?'\n---\nbody\n",
        )
        .unwrap();
        // 无 intents 声明 → 不产生路由
        make_mock_skill(&temp.0, "skill-b", "no-intents");
        // enabled: false → 不产生路由
        let dir_c = temp.0.join("skill-c");
        std::fs::create_dir_all(&dir_c).unwrap();
        std::fs::write(
            dir_c.join("SKILL.md"),
            "---\nname: skill-c\nenabled: false\nintents: [\"zzz\"]\n---\nbody\n",
        )
        .unwrap();

        let rules = intent_rules_from_dirs(&[temp.0.clone()]);
        assert_eq!(rules.len(), 1, "只有 enabled 且声明 intents 的技能产生路由");
        assert_eq!(rules[0].skill_name, "skill-a");
        assert_eq!(rules[0].patterns.len(), 2);
        assert!(
            rules[0].patterns[0].contains("{0,15}"),
            "量词逗号应完整保留: {}",
            rules[0].patterns[0]
        );
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
}
