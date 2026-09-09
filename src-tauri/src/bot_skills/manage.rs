use super::parse::{parse_frontmatter, parse_meta, SKILL_NAME_CHARS_OK};
use crate::error::{CommandError, CommandResult};
use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    /// frontmatter enabled（2026-08-27 审计 P2：清单注入需要按它过滤）
    pub enabled: bool,
    /// Phase 5 D (2026-08-18 08:00): last run outcome (SettingsPage badge)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<crate::db::PersistedSkillOutcome>,
}

fn skills_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
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
pub(crate) fn skill_search_paths<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<std::path::PathBuf> {
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
            out.push(SkillInfo {
                name: meta.name,
                description: meta.description,
                enabled: meta.enabled,
                last_outcome: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 全量技能路由规则扫描（2026-08-19 动态路由，老板拍板：未安装的技能不得有路由）：
/// 遍历搜索路径读每个 SKILL.md 的 frontmatter `intents`；
/// `enabled: false` / intents 为空 → 该技能不产生路由。同名去重、前面目录优先（与 scan_skill_dirs 一致）。
/// 单测可传临时目录数组，不依赖 AppHandle。
pub fn intent_rules_from_dirs(dirs: &[std::path::PathBuf]) -> Vec<crate::intent_router::IntentRule> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
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
            let Ok(text) = std::fs::read_to_string(path.join("SKILL.md")) else {
                continue;
            };
            let meta = parse_meta(&text, &dir_name);
            if !meta.enabled || meta.intents.is_empty() {
                continue;
            }
            out.push(crate::intent_router::IntentRule {
                patterns: meta.intents,
                skill_name: meta.name,
            });
        }
    }
    out
}

/// 重建进程级技能路由表：app 启动调一次；skills_import / skills_delete 成功后再调。
pub fn rebuild_intent_routes(app: &AppHandle) {
    crate::intent_router::rebuild_routes(intent_rules_from_dirs(&skill_search_paths(app)));
}

/// 技能清单块：注入系统提示词尾部（progressive disclosure 第一层）。
/// 2026-08-27 审计 P2：过滤 enabled=false——禁用技能不再被广告给 LLM
/// （原先清单照样列出，模型调 use_skill 才被 preflight 拒绝，与路由表口径不一致）。
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
/// 返回技能名。
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
    // 2026-08-27 审计 P2：校验最终技能名（frontmatter 名非法时 parse 回退目录名，
    // 目录名本身也可能非法）——非法名装上后 load/delete 都拒绝，装得上用不了删不掉
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason: "技能名无效（frontmatter name 或目录名仅允许字母/数字/-/_）".into(),
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
    // 动态路由（2026-08-19）：安装成功即按 frontmatter intents 生成该技能的路由条目
    rebuild_intent_routes(&app);
    Ok(name)
}

/// 删除技能（整目录）
#[tauri::command]
pub fn skills_delete(app: AppHandle, name: String) -> CommandResult<()> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason: "技能名无效（仅允许字母/数字/-/_）".into(),
        });
    }
    let dir = skills_dir(&app).join(&name);
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    // 动态路由（2026-08-19）：卸载即移除该技能的路由条目
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
        let text =
            format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nbody\n");
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

    // ── 2026-08-19 动态路由：intent_rules_from_dirs（未安装不得有路由） ──

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

}
