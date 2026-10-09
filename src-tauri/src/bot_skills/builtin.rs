//! 内置技能（随应用分发的种子技能）。
//!
//! 源文件在 `assets/skills/`，include_bytes! 烘进二进制；启动时物化到
//! `builtin_skills/`（与用户 skills 目录同父、不同名，导入/删除语义互不干扰）。
//! 物化幂等规则：目录缺失 → Seeded；磁盘版本 ≠ 内置版本 → Updated（覆盖）；
//! 相同 → Unchanged（版本不变时保留用户对物化副本的本地修改）。
//! 搜索优先级（`skill_search_paths`）：用户 skills > builtin_skills > dev mock——
//! 用户导入同名副本时自然压过内置版；删除内置副本后运行期内不复活，
//! 应用更新带新版本号时重新物化。

use super::parse::parse_meta;
use tauri::Manager;

pub struct BuiltinSkill {
    pub dir_name: &'static str,
    pub content: &'static [u8],
}

/// 内置技能清单：新增 = assets/skills/<dir>/SKILL.md + 此处登记一条
pub const BUILTIN_SKILLS: &[BuiltinSkill] = &[BuiltinSkill {
    dir_name: "pptx-design",
    content: include_bytes!("../../assets/skills/pptx-design/SKILL.md"),
}];

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

/// 物化全部内置技能（幂等，可重入）。返回顺序同 BUILTIN_SKILLS。
pub fn materialize_builtin_skills(dir: &std::path::Path) -> Vec<(&'static str, SeedOutcome)> {
    BUILTIN_SKILLS
        .iter()
        .map(|skill| (skill.dir_name, materialize_one(dir, skill)))
        .collect()
}

fn materialize_one(dir: &std::path::Path, skill: &BuiltinSkill) -> SeedOutcome {
    let dest_dir = dir.join(skill.dir_name);
    let dest = dest_dir.join("SKILL.md");
    let existed = dest.exists();
    if existed && disk_version(&dest, skill.dir_name) == bundled_version(skill) {
        return SeedOutcome::Unchanged;
    }
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        return SeedOutcome::Failed(format!("create_dir_all: {e}"));
    }
    match std::fs::write(&dest, skill.content) {
        Ok(()) if existed => SeedOutcome::Updated,
        Ok(()) => SeedOutcome::Seeded,
        Err(e) => SeedOutcome::Failed(format!("write: {e}")),
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
pub fn materialize_at_startup<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let dir = builtin_skills_dir(app);
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
        dir.join("pptx-design").join("SKILL.md")
    }

    #[test]
    fn seeds_when_missing_then_unchanged() {
        let root = temp_root("seed-missing");
        let out = materialize_builtin_skills(&root);
        assert!(matches!(out[0].1, SeedOutcome::Seeded), "out: {out:?}");
        let dest = skill_md(&root);
        assert!(dest.exists());
        assert!(
            std::fs::metadata(&dest)
                .map(|m| m.len() > 0)
                .unwrap_or(false),
            "落盘文件不得为空"
        );
        let again = materialize_builtin_skills(&root);
        assert!(matches!(again[0].1, SeedOutcome::Unchanged));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn updates_when_disk_version_differs() {
        let root = temp_root("seed-update");
        let dest_dir = root.join("pptx-design");
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(
            dest_dir.join("SKILL.md"),
            "---\nname: pptx-design\ndescription: 旧版\nversion: 0.0.1\n---\n旧内容\n",
        )
        .unwrap();
        let out = materialize_builtin_skills(&root);
        assert!(matches!(out[0].1, SeedOutcome::Updated), "out: {out:?}");
        let text = std::fs::read_to_string(skill_md(&root)).unwrap();
        assert!(text.contains("PPT 设计规范"), "应覆盖为内置新版");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skips_when_version_same_even_if_content_edited() {
        let root = temp_root("seed-same");
        materialize_builtin_skills(&root);
        std::fs::write(
            skill_md(&root),
            "---\nname: pptx-design\ndescription: 本地修改版\nversion: 1.0.0\n---\n本地\n",
        )
        .unwrap();
        let out = materialize_builtin_skills(&root);
        assert!(matches!(out[0].1, SeedOutcome::Unchanged), "out: {out:?}");
        assert!(
            std::fs::read_to_string(skill_md(&root))
                .unwrap()
                .contains("本地修改版"),
            "版本相同不得覆盖用户的本地修改"
        );
        std::fs::remove_dir_all(&root).ok();
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
            // 自由文档式 skill：正文不得含 `## Step` 标题，否则误入 DSL 步骤模式
            let (steps, rollback) = parse_skill_steps(text).unwrap_or_default();
            assert!(
                steps.is_empty() && rollback.is_empty(),
                "{} 正文混入了 Step 标题",
                skill.dir_name
            );
        }
    }
}
