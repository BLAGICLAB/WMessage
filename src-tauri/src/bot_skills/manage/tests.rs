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
    let body = manage_src();
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

/// create_skill 必须把技能钉成会话档：落盘前强制 frontmatter `scope: session`
/// （模型 content 里写什么 scope 都会被覆盖）+ 落盘后经 add_session_route
/// 注册本会话路由。「模型建技能跨会话劫持路由」的入口就在这两处，少一处都回流。
#[test]
fn tool_create_skill_pins_session_scope() {
    let body = create_skill_src();
    let body = body
        .split_once("pub async fn tool_create_skill(")
        .expect("找不到 tool_create_skill")
        .1
        .split("\n}\n")
        .next()
        .expect("tool_create_skill 应以顶格 } 收尾");
    let inject = body
        .find("force_frontmatter_session_scope(&content)")
        .expect("落盘前必须强制 scope: session——模型写的 scope 值不可信");
    let write = body
        .find("create_skill_at(")
        .expect("tool_create_skill 必须走 create_skill_at 落盘");
    assert!(inject < write, "scope 注入必须在落盘之前");
    assert!(
        body.contains("add_session_route("),
        "落盘后必须注册会话档路由（本会话立即生效）"
    );
}

/// 会话档技能不得进全局路由表：intent_rules_from_dirs 的点名与 intents
/// 两个收集闭包都必须过滤 session_scoped——文件在场不等于全局可路由，
/// 否则重启/下次重建时模型建的技能就回流成全局劫持。
#[test]
fn intent_rules_from_dirs_skips_session_scoped_skills() {
    let body = manage_src();
    let body = body
        .split_once("pub fn intent_rules_from_dirs(")
        .expect("找不到 intent_rules_from_dirs")
        .1
        .split("\n}\n")
        .next()
        .expect("intent_rules_from_dirs 应以顶格 } 收尾");
    assert_eq!(
        body.matches("meta.session_scoped").count(),
        2,
        "两个收集闭包都必须过滤会话档：{body}"
    );
}

/// create_skill 落盘前必须有确认窗，且确认窗要展示 intents 与 mode——
/// 这两项决定「哪些话会被路由进来」和「命中后是否绕开 LLM 直接执行」。
#[test]
fn tool_create_skill_confirms_before_writing_and_shows_intents_and_mode() {
    let body = create_skill_src();
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
    let body = create_skill_src();
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

/// create_skill 的生产代码切片——源码文本守卫用。
///
/// 实现已搬到 manage/create.rs（准入核 / 落盘 / 工具口都在那边）。
/// 该文件没有内联 `#[cfg(test)]` 块，整份都是生产代码，直接用
/// `include_str!` 即可。
fn create_skill_src() -> &'static str {
    include_str!("create.rs")
}

/// 本文件（manage.rs）的生产代码切片——源码文本守卫用。
///
/// 剔除内联 `#[cfg(test)]` 项，而不是按「首个 #[cfg(test)] 之前」截断：
/// 后者会被文件靠前的 `#[cfg(test)]` 项（如测试专用的 re-export）
/// 提前截断点，把后面大段生产代码整段漏掉。
fn manage_src() -> &'static str {
    let full = include_str!("../manage.rs");
    let cut = full.find("#[cfg(test)]").expect("manage.rs 应有测试模块");
    &full[..cut]
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
    let text = format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nbody\n");
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

    let nonexistent = std::path::PathBuf::from("/tmp/wmessage-nonexistent-dir-xxx-does-not-exist");
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

// ── 启用开关落盘 ──

/// 开关写进 SKILL.md 的 frontmatter，且解析器读得回来（关闭 → 不进清单/路由）。
#[test]
fn write_enabled_to_flips_frontmatter_flag() {
    let tmp = make_tempdir("set-enabled");
    let md = tmp.0.join("SKILL.md");
    std::fs::write(
        &md,
        "---\nname: s1\ndescription: d\nversion: 1.0.0\n---\n# 正文\n",
    )
    .unwrap();
    write_enabled_to(&md, "s1", false).unwrap();
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(!parse_meta(&text, "s1").enabled, "关闭后解析器须读回 false");
    assert!(text.contains("version: 1.0.0"), "其余 frontmatter 键不得丢");
    write_enabled_to(&md, "s1", true).unwrap();
    let text = std::fs::read_to_string(&md).unwrap();
    assert!(parse_meta(&text, "s1").enabled, "开启后解析器须读回 true");
}

/// 写盘后不得留 tmp 残渣（原子写语义）。
#[test]
fn write_enabled_to_leaves_no_tmp_residue() {
    let tmp = make_tempdir("set-enabled-tmp");
    let md = tmp.0.join("SKILL.md");
    std::fs::write(&md, "---\nname: s1\ndescription: d\n---\nbody").unwrap();
    write_enabled_to(&md, "s1", false).unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(&tmp.0)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "不得遗留 tmp 残渣");
}

/// 生效副本 = 优先级最高的那份（用户副本压过内置副本）——
/// 写错副本就是「关的是 A、生效的是 B」。
#[test]
fn effective_skill_md_prefers_earlier_dir() {
    let tmp = make_tempdir("effective-md");
    let user = tmp.0.join("user");
    let builtin = tmp.0.join("builtin");
    for d in [&user, &builtin] {
        std::fs::create_dir_all(d.join("s1")).unwrap();
        std::fs::write(d.join("s1").join("SKILL.md"), "---\nname: s1\n---\n").unwrap();
    }
    let picked = effective_skill_md(&[user.clone(), builtin.clone()], "s1").unwrap();
    assert_eq!(picked, user.join("s1").join("SKILL.md"));
    // 用户目录没有该技能 → 落到内置副本
    let picked = effective_skill_md(&[user.clone(), builtin.clone()], "s2");
    assert!(picked.is_none(), "两边都没有时返回 None");
}

/// 关闭的技能不产出意图路由条目（开关必须真的拦得住路由）。
#[test]
fn disabled_skill_produces_no_intent_rules() {
    let tmp = make_tempdir("disabled-routes");
    let dir = tmp.0.join("skills");
    std::fs::create_dir_all(dir.join("s1")).unwrap();
    let md = dir.join("s1").join("SKILL.md");
    let body = "---\nname: s1\ndescription: d\nintents:\n  - 做PPT\n---\nbody";
    std::fs::write(&md, body).unwrap();
    assert!(
        !intent_rules_from_dirs(std::slice::from_ref(&dir)).is_empty(),
        "开启时应有路由条目"
    );
    write_enabled_to(&md, "s1", false).unwrap();
    assert!(
        intent_rules_from_dirs(std::slice::from_ref(&dir)).is_empty(),
        "关闭后不得产出任何路由条目"
    );
    // 清单同样过滤（禁用技能不再被广告给模型）
    let listed = scan_skill_dirs(&[dir]);
    assert!(
        listed.iter().all(|s| !s.enabled),
        "关闭后清单须标为 disabled"
    );
}
