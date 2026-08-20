//! F-6 端到端测试（2026-08-18）
//!
//! 设计要点（务实方案）：
//! - **不依赖 Tauri runtime**（macOS EventLoop 主线程限制 + mock_runtime state lookup 失效）
//! - **不污染 target/debug/skills/**（避免触发 lib smoke_all_real_skills 回归）
//! - 真 Skill fixture 走 `tests/fixtures/` + `scan_skill_dirs` 直接加载
//! - middleware 测试走 `MiddlewareRegistry` 直接 API（不走 Tauri state 间接层）
//!
//! 覆盖范围：
//! 1. pre-step 路由（middleware::MiddlewareRegistry）
//! 2. pre-execute 黑名单阻断 + 放行（同上）
//! 3. 真 Skill fixture 解析（scan_skill_dirs + parse_meta）
//! 4. 黑名单/白名单原子分类（tool_guard 纯函数）
//! 5. 意图路由动态规则（intent_router 纯函数 + fixture 扫描，2026-08-19 起路由来自已安装技能 intents）
//!
//! 后续 F-6 step 2 计划：补 mock LLM HTTP server + 完整 run_skill_scheduler 端到端 +
//! interactive mode Skill LLM 调工具的端到端（需要 cascading generic refactor 把 execute_tool /
//! tool_* / db_* 都泛型化以接受 MockRuntime AppHandle）。

use std::path::PathBuf;
use wmessage_lib::bot_skills::{self, scan_skill_dirs};
use wmessage_lib::intent_router::{self, RouteAction};
use wmessage_lib::{middleware, tool_guard};

// ────────────────────────────────────────────────────────────────────
// helpers
// ────────────────────────────────────────────────────────────────────

fn ppt_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("minimax-ppt")
}

/// scan_skill_dirs 期望传**父目录**（含 Skill 子目录），返回 dir 下的 Skill 列表
fn fixtures_parent_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// 中间件 API 的 AppHandle 首参占位（P2-13/14 签名扩展后测试适配，2026-08-19）。
/// 这些用例只命中路由/闸门判定，不触发 audit 落盘路径；App 泄漏给测试进程，退出即回收。
fn mock_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    Box::leak(Box::new(tauri::test::mock_app())).handle().clone()
}

// ────────────────────────────────────────────────────────────────────
// 1. pre-step 路由：关键词 → Skill 命中
// ────────────────────────────────────────────────────────────────────

#[test]
fn pre_step_routes_installed_skill_intent_to_skill() {
    // 2026-08-19 动态路由：路由表 = 已安装技能的 intents 声明。
    // 先按 fixture（已安装技能的替身）重建全局表，再走 middleware 全链路验证命中。
    intent_router::rebuild_routes(bot_skills::intent_rules_from_dirs(&[fixtures_parent_path()]));
    let registry = middleware::build_default_registry();
    let route = registry.run_pre_step(&mock_handle(), "帮我做一份 XX 主题的 PPT");
    // 还原空表，避免污染同进程其他测试
    intent_router::rebuild_routes(vec![]);
    match route {
        Some(RouteAction::Skill(s)) => {
            assert_eq!(s, "minimax-ppt", "期望命中 fixture Skill（intents 含 PPT）");
        }
        other => panic!("期望命中 Skill, got {other:?}"),
    }
}

#[test]
fn pre_step_pass_through_for_normal_query() {
    let registry = middleware::build_default_registry();
    let pass = registry.run_pre_step(&mock_handle(), "你好世界");
    assert!(
        matches!(pass, Some(RouteAction::PassThrough)),
        "普通消息应 PassThrough；got {pass:?}"
    );
}

// ────────────────────────────────────────────────────────────────────
// 2. pre-execute 黑名单阻断 + 放行
// ────────────────────────────────────────────────────────────────────

#[test]
fn pre_execute_blocks_atomic_tool_when_no_skill() {
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), "create_word_revisions", false);
    let msg = blocked.expect("create_word_revisions + 非 Skill 状态应被阻断");
    assert!(msg.contains("Skill"), "阻断消息应引导走 Skill；实际：{msg}");
}

#[test]
fn pre_execute_allows_atomic_tool_when_skill_active() {
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), "create_word_revisions", true);
    assert!(
        blocked.is_none(),
        "create_word_revisions + Skill Running 状态应放行；实际：{blocked:?}"
    );
}

#[test]
fn pre_execute_allows_whitelist_tool() {
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), "run_python", false);
    assert!(
        blocked.is_none(),
        "白名单 run_python 不应被阻断；实际：{blocked:?}"
    );
}

// ────────────────────────────────────────────────────────────────────
// 3. 黑名单/白名单原子分类（纯函数）
// ────────────────────────────────────────────────────────────────────

#[test]
fn atomic_guard_blacklist_classification() {
    // 黑名单必须识别
    assert!(tool_guard::is_atomic_tool("create_word_revisions"));
    assert!(tool_guard::is_atomic_tool("link_file_to_task"));

    // 已知白名单必须不被误判为黑名单
    for name in [
        "list_tasks",
        "query_single_task",
        "create_task",
        "complete_task",
        "delete_task",
        "edit_task",
        "run_python",
        "web_search",
        "fetch_url",
        "use_skill",
    ] {
        assert!(
            !tool_guard::is_atomic_tool(name),
            "{name} 是白名单工具，不应被判为原子黑名单"
        );
    }
}

// ────────────────────────────────────────────────────────────────────
// 4. 意图路由：动态规则（2026-08-19 重构：路由 = 已安装技能的 intents 声明，
//    未安装的技能不得有路由；静态 INTENT_RULES 表已删除）
// ────────────────────────────────────────────────────────────────────

#[test]
fn intent_router_routes_come_from_installed_skill_intents() {
    // fixture minimax-ppt 声明了 intents: ["PPT", "F-6 测试"] → 扫描即得路由规则
    let rules = bot_skills::intent_rules_from_dirs(&[fixtures_parent_path()]);
    assert!(
        rules.iter().any(|r| r.skill_name == "minimax-ppt"),
        "fixture 的 intents 应产生路由规则: {rules:?}"
    );
    // 扫描出的规则直接驱动匹配
    assert_eq!(
        intent_router::route_with_rules("做个 PPT 模板", &rules),
        RouteAction::Skill("minimax-ppt".to_string())
    );
    assert_eq!(
        intent_router::route_with_rules("F-6 测试 跑一下", &rules),
        RouteAction::Skill("minimax-ppt".to_string())
    );
    // 规则集之外（= 未安装的技能）→ 无路由，放行进 LLM
    assert_eq!(
        intent_router::route_with_rules("用修订模式润色 Word", &rules),
        RouteAction::PassThrough
    );
    assert_eq!(
        intent_router::route_with_rules("做个 PPT 模板", &[]),
        RouteAction::PassThrough,
        "空规则集（什么都没装）→ 恒 PassThrough"
    );
}

// ────────────────────────────────────────────────────────────────────
// 5. 真 Skill fixture 加载（不污染 target/debug/skills/）
// ────────────────────────────────────────────────────────────────────

#[test]
fn real_skill_fixture_loads_via_scan_skill_dirs() {
    let ppt_path = ppt_fixture_path();
    assert!(
        ppt_path.join("SKILL.md").exists(),
        "fixture SKILL.md 必须存在：{}",
        ppt_path.display()
    );

    // SkillInfo 只有 name/description/last_outcome 字段，body 直接读文件
    let body = std::fs::read_to_string(ppt_path.join("SKILL.md")).expect("read fixture SKILL.md");

    let skills = scan_skill_dirs(&[fixtures_parent_path()]);
    assert_eq!(skills.len(), 1, "fixture 目录只放 1 个 Skill");

    let skill = &skills[0];
    assert_eq!(skill.name, "minimax-ppt");
    assert!(
        skill.description.contains("F-6 端到端"),
        "description 应包含 F-6 标记；got：{}",
        skill.description
    );
    assert!(skill.last_outcome.is_none(), "新 fixture 无运行历史");

    // body 验证
    assert!(body.contains("## Step 1"), "body 应包含 DSL Step 1 段");
    assert!(body.contains("## Step 2"), "body 应包含 DSL Step 2 段");
    assert!(body.contains("list_tasks({})"), "Step 1 应调 list_tasks");
    assert!(body.contains("create_task("), "Step 2 应调 create_task");

    // 验证 metadata 解析（frontmatter 字段 → SkillMeta）
    let meta = bot_skills::parse_meta(&body, &skill.name);
    assert_eq!(meta.name, "minimax-ppt");
    assert_eq!(meta.mode, "auto", "fixture mode 应为 auto");
    assert_eq!(meta.risk_level, "low");
    assert_eq!(meta.max_steps, 5);
    assert_eq!(meta.timeout_secs, 60);
}
