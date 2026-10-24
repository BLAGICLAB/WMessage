//! 内置技能（随应用分发的种子技能）。
//!
//! 源文件在 `assets/skills/`，include_bytes! 烘进二进制；启动时物化到
//! `builtin_skills/`（与用户 skills 目录同父、不同名，导入/删除语义互不干扰）。
//! 物化幂等规则：目录缺失 → Seeded；磁盘版本 ≠ 内置版本 → Updated（覆盖）；
//! 相同 → Unchanged（版本不变时保留用户对物化副本的本地修改）。
//! 覆盖时仍保留用户在设置页关掉的开关（磁盘副本显式 `enabled: false`）——
//! 应用升级不该把用户手动禁用的技能重新打开。
//! 搜索优先级（`skill_search_paths`）：用户 skills > builtin_skills > dev mock——
//! 用户导入同名副本时自然压过内置版；删除内置副本后运行期内不复活，
//! 应用更新带新版本号时重新物化。
//!
//! **退役清理**：资产撤了、清单里那一条也撤了之后，旧版本物化出来的副本
//! 仍留在 `builtin_skills/`——而该目录在搜索路径里，副本会被扫到、出现在
//! 技能列表里，用户在设置页删不掉（删了，下次启动若资产还在会被物化回来；
//! 资产真没了就变成**永久僵尸技能**）。故启动时先清一次：物化目录里
//! 不在当前清单中的子目录全删（`builtin_skills/` 是应用自管目录，用户技能
//! 在同父的 `skills/`，不碰）。

use super::parse::{frontmatter_enabled_declared, parse_meta, set_frontmatter_enabled};
use tauri::Manager;

pub struct BuiltinSkill {
    pub dir_name: &'static str,
    pub content: &'static [u8],
}

/// 内置技能清单：**当前为空**——`pptx-design` 已于 2026-10-24 退役
/// （用户自装技能、不再随应用分发）。
///
/// 新增一条 = ① 放资产 `assets/skills/<dir>/SKILL.md`；② 在此登记
/// `BuiltinSkill { dir_name: "<dir>", content: include_bytes!("../../assets/skills/<dir>/SKILL.md") }`。
/// 资产目录与本清单的一致性由 `asset_dir_matches_registration_list` 测试守住
/// （只放资产不登记 = 那个技能永远不会被物化，是个哑资产）。
pub const BUILTIN_SKILLS: &[BuiltinSkill] = &[];

/// 内置技能物化目录：用户 skills 目录的兄弟目录（app_data_dir 不可得时回退
/// data_dir，与 skills_dir 同一回退语义，保证 scan/import/delete 的路径总能算出）
pub fn builtin_skills_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    super::manage::data_dir_subdir(
        app.path().app_data_dir().ok(),
        || crate::db::data_dir(app),
        "builtin_skills",
    )
}

/// 单个内置技能的物化处置结果
#[derive(Debug)]
pub enum SeedOutcome {
    /// 首次写入
    Seeded,
    /// 版本变化覆盖（含磁盘副本损坏的自愈重写）
    Updated,
    /// 版本相同，跳过
    Unchanged,
    /// 磁盘操作失败；逐项返回，不阻断其余技能与启动
    Failed(String),
}

/// 退役清理结果：删掉了哪些、哪些没删掉（附原因）
#[derive(Debug, Default)]
pub struct RetiredCleanup {
    /// 删掉的物化副本目录名
    pub removed: Vec<String>,
    /// 没删掉的（目录名 + 原因）
    pub failed: Vec<(String, String)>,
}

/// 物化全部内置技能（幂等，可重入）。返回顺序同 BUILTIN_SKILLS。
pub fn materialize_builtin_skills(dir: &std::path::Path) -> Vec<(&'static str, SeedOutcome)> {
    materialize_skills(dir, BUILTIN_SKILLS)
}

/// 物化给定技能清单（纯函数，可单测）。生产走 `materialize_builtin_skills`。
fn materialize_skills(
    dir: &std::path::Path,
    skills: &[BuiltinSkill],
) -> Vec<(&'static str, SeedOutcome)> {
    skills
        .iter()
        .map(|skill| (skill.dir_name, materialize_one(dir, skill)))
        .collect()
}

/// 清理已退役的内置技能物化副本：物化目录里不在当前清单中的子目录全删。
///
/// 只删**子目录**（物化出来的技能都是目录）；`builtin_skills/` 本身留着
/// （空目录无害，也免得每次启动都重建）。
pub fn purge_retired_builtin_skills(dir: &std::path::Path) -> RetiredCleanup {
    purge_retired_skills_in(dir, BUILTIN_SKILLS)
}

/// 清理内核（纯函数，可单测）：`skills` = 当前在册的内置技能清单。
fn purge_retired_skills_in(dir: &std::path::Path, skills: &[BuiltinSkill]) -> RetiredCleanup {
    let mut out = RetiredCleanup::default();
    let Ok(rd) = std::fs::read_dir(dir) else {
        // 目录还不存在 = 没有可清的
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if skills.iter().any(|s| s.dir_name == name) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => out.removed.push(name),
            Err(e) => out.failed.push((name, e.to_string())),
        }
    }
    out
}

fn materialize_one(dir: &std::path::Path, skill: &BuiltinSkill) -> SeedOutcome {
    let dest_dir = dir.join(skill.dir_name);
    let dest = dest_dir.join("SKILL.md");
    let existed = dest.exists();
    if existed && versions_match(disk_version(&dest, skill.dir_name), bundled_version(skill)) {
        return SeedOutcome::Unchanged;
    }
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        return SeedOutcome::Failed(format!("create_dir_all: {e}"));
    }
    // 兄弟临时文件 + rename 覆盖：裸 fs::write 原地 truncate，中断留截断 SKILL.md
    // （dest 要么完整旧版要么完整新版）。tmp 在技能目录内，扫描只认 SKILL.md，不碍事。
    let tmp = dest_dir.join(".SKILL.md.tmp");
    let content = materialize_content(&dest, existed, skill.content);
    let write_result = std::fs::write(&tmp, content).and_then(|()| std::fs::rename(&tmp, &dest));
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    match write_result {
        Ok(()) if existed => SeedOutcome::Updated,
        Ok(()) => SeedOutcome::Seeded,
        Err(e) => SeedOutcome::Failed(format!("write: {e}")),
    }
}

/// 物化内容 = 内置资产字节，或「保留用户显式关闭」后的字节。
///
/// 应用升级带来新版本时物化会覆盖磁盘副本——用户在设置页手动关掉的技能
/// 会「自己又开了」。磁盘副本里显式声明 `enabled: false` = 用户的手动决定，
/// 必须跨版本保留；未声明（None）= 用户没表态，跟随资产默认值。
///
/// 只保留 `false` 不保留 `true`：资产默认恒为启用（`builtin_assets_parse_clean`
/// 锁死），保留 `true` 与跟随资产等价，多写一份状态只会多一个漂移源。
fn materialize_content(dest: &std::path::Path, existed: bool, bundled: &[u8]) -> Vec<u8> {
    if !existed {
        return bundled.to_vec();
    }
    let Ok(old) = std::fs::read_to_string(dest) else {
        return bundled.to_vec();
    };
    if frontmatter_enabled_declared(&old) != Some(false) {
        return bundled.to_vec();
    }
    let Ok(text) = std::str::from_utf8(bundled) else {
        return bundled.to_vec();
    };
    match set_frontmatter_enabled(text, false) {
        Some(t) => t.into_bytes(),
        None => bundled.to_vec(),
    }
}

/// 版本一致判定（纯函数，单测直打）：任一侧 None 都视为不一致——磁盘读不出/
/// 解析不出 → 强制重写自愈；内置资产 version 解析不出 → 恒重写（资产修复前
/// 每次启动覆盖）。None == None 相等跳过会让「双坏」状态永不自愈。
fn versions_match(disk: Option<String>, bundled: Option<String>) -> bool {
    match (disk, bundled) {
        (Some(d), Some(b)) => d == b,
        _ => false,
    }
}

/// 磁盘副本的 frontmatter version；读不到/解析不出按 None（视为需覆盖自愈）
fn disk_version(dest: &std::path::Path, dir_name: &str) -> Option<String> {
    let text = std::fs::read_to_string(dest).ok()?;
    parse_meta(&text, dir_name).version
}

/// 内置资产的 frontmatter version；资产非 UTF-8 时按 None（每次启动覆盖自愈，
/// 正确性由 builtin_assets_parse_clean 测试锁死）
fn bundled_version(skill: &BuiltinSkill) -> Option<String> {
    let text = std::str::from_utf8(skill.content).unwrap_or("");
    parse_meta(text, skill.dir_name).version
}

/// 启动时物化 + 逐项审计。必须在 `rebuild_intent_routes` 之前调用——
/// 新物化的技能当次启动就要进路由。Unchanged 静默。
///
/// 先清退役副本再物化：清掉的副本不该在本次启动的技能列表里留影。
pub fn materialize_at_startup<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let dir = builtin_skills_dir(app);
    let cleanup = purge_retired_builtin_skills(&dir);
    for name in &cleanup.removed {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "builtin_skill_retired",
            "skill" => name.clone()
        );
    }
    for (name, err) in &cleanup.failed {
        // 清不掉不阻断启动：残留副本顶多在列表里多一项，用户仍可手动删
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "builtin_skill_purge_failed",
            "skill" => name.clone(),
            "error" => err.clone()
        );
    }
    for (name, outcome) in materialize_builtin_skills(&dir) {
        match outcome {
            SeedOutcome::Unchanged => {}
            SeedOutcome::Seeded => crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "builtin_skill_seeded",
                "skill" => name
            ),
            SeedOutcome::Updated => crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "builtin_skill_updated",
                "skill" => name
            ),
            SeedOutcome::Failed(e) => crate::audit_event!(
                app,
                crate::audit::AuditLevel::Error,
                "builtin_skill_seed_failed",
                "skill" => name,
                "error" => e
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse_skill_steps;
    use super::*;

    /// 测试用内置技能：机制测试不依赖真实资产（资产清单当前为空——
    /// `pptx-design` 已退役）。版本号硬编码在内容里，故 `bundled_version`
    /// 读出来是固定的 1.0.0。
    const FIXTURE_CONTENT: &str = "---\nname: test-builtin\ndescription: 测试用内置技能\nversion: 1.0.0\nmode: interactive\n---\n# 新版内容\n";
    const FIXTURE: BuiltinSkill = BuiltinSkill {
        dir_name: "test-builtin",
        content: FIXTURE_CONTENT.as_bytes(),
    };

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "wm-builtin-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn skill_md(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join(FIXTURE.dir_name).join("SKILL.md")
    }

    /// 物化测试用技能（等价于生产走清单，但与真实资产解耦）
    fn materialize_fixture(dir: &std::path::Path) -> Vec<(&'static str, SeedOutcome)> {
        materialize_skills(dir, &[FIXTURE])
    }

    #[test]
    fn seeds_when_missing_then_unchanged() {
        let root = temp_root("seed-missing");
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Seeded), "out: {out:?}");
        let dest = skill_md(&root);
        assert!(dest.exists());
        assert!(
            std::fs::metadata(&dest)
                .map(|m| m.len() > 0)
                .unwrap_or(false),
            "落盘文件不得为空"
        );
        let again = materialize_fixture(&root);
        assert!(matches!(again[0].1, SeedOutcome::Unchanged));
        std::fs::remove_dir_all(&root).ok();
    }

    /// 原子写语义：落盘内容与内置资产逐字节一致（无截断）、目录无 tmp 残渣，
    /// 且已存在同内容副本时幂等跳过。
    #[test]
    fn seeds_full_content_without_tmp_residue_and_is_idempotent() {
        let root = temp_root("seed-atomic");
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Seeded), "out: {out:?}");
        let dest = skill_md(&root);
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            FIXTURE.content,
            "落盘内容必须与内置资产逐字节一致"
        );
        assert!(
            !root.join(FIXTURE.dir_name).read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "原子写不得遗留 tmp 残渣"
        );
        // 已存在同内容文件：幂等跳过（不重写、不改内容）
        let again = materialize_fixture(&root);
        assert!(matches!(again[0].1, SeedOutcome::Unchanged));
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE.content);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn updates_when_disk_version_differs() {
        let root = temp_root("seed-update");
        let dest_dir = root.join(FIXTURE.dir_name);
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            dest_dir.join("SKILL.md"),
            "---\nname: test-builtin\ndescription: 旧版\nversion: 0.0.1\n---\n旧内容\n",
        )
        .unwrap();
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Updated), "out: {out:?}");
        let text = std::fs::read_to_string(skill_md(&root)).unwrap();
        assert!(text.contains("新版内容"), "应覆盖为内置新版");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skips_when_version_same_even_if_content_edited() {
        let root = temp_root("seed-same");
        materialize_fixture(&root);
        // 版本取自 fixture 资产（硬编码会随版本升级漂移，实测踩过）
        let version = bundled_version(&FIXTURE).unwrap_or_default();
        std::fs::write(
            skill_md(&root),
            format!(
                "---\nname: test-builtin\ndescription: 本地修改版\nversion: {version}\n---\n本地\n"
            ),
        )
        .unwrap();
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Unchanged), "out: {out:?}");
        assert!(
            std::fs::read_to_string(skill_md(&root))
                .unwrap()
                .contains("本地修改版"),
            "版本相同不得覆盖用户的本地修改"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 自愈语义：任一侧 None 都不得判「版本一致」——None == None 跳过会让
    /// 磁盘副本与资产双坏的状态永不重写。
    #[test]
    fn versions_match_treats_any_none_as_mismatch() {
        assert!(versions_match(Some("1.0.0".into()), Some("1.0.0".into())));
        assert!(!versions_match(Some("1.0.0".into()), Some("2.0.0".into())));
        assert!(
            !versions_match(None, Some("1.0.0".into())),
            "磁盘读不出 → 重写"
        );
        assert!(
            !versions_match(Some("1.0.0".into()), None),
            "资产无版本 → 恒重写"
        );
        assert!(!versions_match(None, None), "双坏不得相等跳过");
    }

    #[test]
    fn updates_when_disk_version_missing() {
        // 磁盘副本 frontmatter 丢 version（disk_version = None）→ 强制重写自愈
        let root = temp_root("seed-disk-none");
        let dest_dir = root.join(FIXTURE.dir_name);
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(dest_dir.join("SKILL.md"), "损坏内容，无 frontmatter\n").unwrap();
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Updated), "out: {out:?}");
        assert!(
            std::fs::read_to_string(skill_md(&root))
                .unwrap()
                .contains("新版内容"),
            "应覆盖为内置新版"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 应用升级（内置版本变化）不得把用户手动关掉的技能重新打开：
    /// 磁盘副本显式 `enabled: false` → 物化新版内容时保留关闭态。
    #[test]
    fn keeps_user_disabled_flag_across_version_bump() {
        let root = temp_root("seed-keep-disabled");
        materialize_fixture(&root);
        let dest = skill_md(&root);
        // 用户在设置页关掉内置技能（frontmatter 改写）
        let text = std::fs::read_to_string(&dest).unwrap();
        let off = super::super::parse::set_frontmatter_enabled(&text, false).unwrap();
        std::fs::write(&dest, off).unwrap();
        // 磁盘版本落后 → 触发覆盖式物化（版本号取自资产，勿硬编码）
        let version = bundled_version(&FIXTURE).unwrap_or_default();
        std::fs::write(
            &dest,
            std::fs::read_to_string(&dest)
                .unwrap()
                .replace(&format!("version: {version}"), "version: 0.0.1"),
        )
        .unwrap();
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Updated), "out: {out:?}");
        let after = std::fs::read_to_string(&dest).unwrap();
        assert!(after.contains("新版内容"), "应覆盖为内置新版内容");
        assert!(
            !parse_meta(&after, FIXTURE.dir_name).enabled,
            "用户关掉的技能不得被应用升级重新打开"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 磁盘副本没声明 enabled（用户没表态）→ 跟随资产默认值，不擅自写状态。
    #[test]
    fn follows_asset_default_when_user_never_toggled() {
        let root = temp_root("seed-follow-default");
        materialize_fixture(&root);
        let dest = skill_md(&root);
        // 去掉 enabled 键 + 改版本号，模拟「用户没动过开关的旧副本」
        let version = bundled_version(&FIXTURE).unwrap_or_default();
        let text = std::fs::read_to_string(&dest).unwrap();
        let stripped: String = text
            .lines()
            .filter(|l| !l.starts_with("enabled:"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            &dest,
            stripped.replace(&format!("version: {version}"), "version: 0.0.1"),
        )
        .unwrap();
        let out = materialize_fixture(&root);
        assert!(matches!(out[0].1, SeedOutcome::Updated), "out: {out:?}");
        let after = std::fs::read_to_string(&dest).unwrap();
        assert!(
            parse_meta(&after, FIXTURE.dir_name).enabled,
            "未表态时跟随资产默认（启用）"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 退役清理：资产撤了、清单里那条也撤了 → 残留的物化副本必须被删掉。
    /// 不删的话它留在搜索路径里 = 列表里的僵尸技能，用户删不掉。
    #[test]
    fn purge_removes_retired_materialized_copies() {
        let root = temp_root("purge-retired");
        // 模拟「旧版本物化过、现在退役了」：目录里有副本
        let retired = root.join("pptx-design");
        std::fs::create_dir_all(&retired).unwrap();
        std::fs::write(
            retired.join("SKILL.md"),
            "---\nname: pptx-design\ndescription: 退役技能\n---\n旧内容\n",
        )
        .unwrap();
        let cleanup = purge_retired_builtin_skills(&root);
        assert!(
            cleanup.removed.iter().any(|n| n == "pptx-design"),
            "退役副本必须被删：{:?}",
            cleanup
        );
        assert!(!retired.exists(), "退役副本目录不得残留");
        assert!(
            cleanup.failed.is_empty(),
            "不该有删失败的：{:?}",
            cleanup.failed
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 清理只认清单：在册的技能副本必须留下，不在册的（退役了）必须删掉。
    #[test]
    fn purge_keeps_registered_skills() {
        let root = temp_root("purge-keep");
        let registered = root.join(FIXTURE.dir_name);
        let retired = root.join("retired-one");
        for d in [&registered, &retired] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        }
        let cleanup = purge_retired_skills_in(&root, &[FIXTURE]);
        assert!(
            registered.exists(),
            "在册的内置技能副本不得被清理删掉：{:?}",
            cleanup
        );
        assert!(
            !retired.exists(),
            "不在清单里的副本必须删掉（退役清理）：{:?}",
            cleanup
        );
        assert!(
            cleanup.removed.iter().any(|n| n == "retired-one"),
            "删掉的名单须如实回报：{:?}",
            cleanup
        );
        assert!(
            cleanup.failed.is_empty(),
            "不该有删失败的：{:?}",
            cleanup.failed
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// 物化目录不存在时清理不得报错（首次启动即没有这个目录）。
    #[test]
    fn purge_on_missing_dir_is_noop() {
        let root = temp_root("purge-missing");
        let missing = root.join("not-there");
        let cleanup = purge_retired_builtin_skills(&missing);
        assert!(cleanup.removed.is_empty());
        assert!(cleanup.failed.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// 资产目录与登记清单必须一致：`assets/skills/` 下多一个目录而清单里没登记
    /// → 这个技能永远不会被物化（用户看不到它），是个静默的哑资产。
    #[test]
    fn asset_dir_matches_registration_list() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skills");
        if let Ok(rd) = std::fs::read_dir(&assets) {
            for entry in rd.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                assert!(
                    BUILTIN_SKILLS.iter().any(|s| s.dir_name == name),
                    "assets/skills/{name} 未在 BUILTIN_SKILLS 登记——它永远不会被物化"
                );
            }
        }
        // 登记了的必须有资产文件（include_bytes! 已在编译期兜住，这里再钉一遍语义）
        for s in BUILTIN_SKILLS {
            let p = assets.join(s.dir_name).join("SKILL.md");
            assert!(
                p.is_file(),
                "{} 登记了但资产文件缺失：{}",
                s.dir_name,
                p.display()
            );
        }
    }

    #[test]
    fn builtin_assets_parse_clean() {
        for skill in BUILTIN_SKILLS {
            let text = std::str::from_utf8(skill.content).unwrap_or_default();
            assert!(!text.is_empty(), "{} 资产非 UTF-8 或为空", skill.dir_name);
            let meta = parse_meta(text, skill.dir_name);
            assert_eq!(meta.name, skill.dir_name, "frontmatter name 须等于目录名");
            assert!(
                !meta.description.is_empty(),
                "description 必填（清单与路由依赖）"
            );
            assert!(meta.enabled, "内置技能须默认启用");
            assert!(!meta.intents.is_empty(), "无 intents 则路由失效");
            // 版本必须恒可解析（bundled_version 恒 Some）：versions_match 的
            // 自愈语义靠「资产侧 Some」区分正常比对与强制重写
            assert!(
                meta.version.is_some(),
                "{} frontmatter 必须带 version",
                skill.dir_name
            );
            // 参考文档型技能：interactive 模式（LLM 驱动读文档执行），正文不得含
            // `## Step` 标题。两个条件绑定锁死——mode 若改回 auto，调度器会因
            // 「DSL 解析为空」直接终止技能（skill_dsl_empty，实测踩过）
            assert_eq!(meta.mode, "interactive", "参考文档型技能必须 interactive");
            let (steps, rollback) = parse_skill_steps(text).unwrap_or_default();
            assert!(
                steps.is_empty() && rollback.is_empty(),
                "{} 正文混入了 Step 标题",
                skill.dir_name
            );
        }
    }
}
