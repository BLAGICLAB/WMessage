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
//! 2026-09-03 T1-2 已完成：run_skill_scheduler_core（调度器本体，重构后泛型 Runtime +
//! 注入 executor/persist）真路径 e2e 见本文件 scheduler_e2e_* 用例；run_model_loop_core
//! 对 mock LLM server 的真路径见 llm_integration.rs 的 core_* 用例。

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
    let blocked = registry.run_pre_execute(&mock_handle(), "link_file_to_task", false);
    let msg = blocked.expect("link_file_to_task + 非 Skill 状态应被阻断");
    assert!(msg.contains("Skill"), "阻断消息应引导走 Skill；实际：{msg}");
}

#[test]
fn pre_execute_allows_atomic_tool_when_skill_active() {
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), "link_file_to_task", true);
    assert!(
        blocked.is_none(),
        "link_file_to_task + Skill Running 状态应放行；实际：{blocked:?}"
    );
}

/// 2026-09-02 老板拍板：create_word_revisions 去 Skill 化（移出黑名单，聊天直调放行）
#[test]
fn pre_execute_allows_create_word_revisions_without_skill() {
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), "create_word_revisions", false);
    assert!(
        blocked.is_none(),
        "create_word_revisions 已移出黑名单，聊天直调应放行；实际：{blocked:?}"
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
    // 黑名单必须识别（2026-09-02：create_word_revisions 去 Skill 化移出，仅剩 link_file_to_task）
    assert!(!tool_guard::is_atomic_tool("create_word_revisions"));
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

// ────────────────────────────────────────────────────────────────────
// T1-2（2026-09-03，原审计 #2）：run_skill_scheduler_core 真路径 e2e
//
// 背景：P0-1（SKILL_RUNS 跨会话顶号 / Done 路径不收尾）/ P0-5（回滚窗口）
// 修复密集区原先无法直测——run_skill_scheduler 直连 Wry AppHandle +
// bot::execute_tool + db::open_db。重构后 core 泛型 Runtime + 注入
// execute_tool / persist_outcome，这里用 MockRuntime + mock executor +
// 真 fixture SKILL.md + 真 upsert_skill_outcome（临时库）直驱调度器本体。
//
// 并行隔离：本文件 4 个新用例都写全局 SKILL_RUNS，照批次8先例全程持
// 本地串行锁（lib 内 SKILL_RUNS_TEST_LOCK 是 cfg(test) 的，集成测试是
// 独立 crate 够不到，故在本二进制内自建一把）。
// ────────────────────────────────────────────────────────────────────

use std::sync::{Arc, Mutex};
use wmessage_lib::bot::{
    load_all_skill_outcomes, upsert_skill_outcome, PersistedSkillOutcome,
};
use wmessage_lib::bot_skills::{
    parse_meta, run_skill_scheduler_core, test_hook_insert_skill_run,
    test_hook_remove_skill_run, test_hook_skill_run_state, DslOutcome, SkillRun, SkillState,
};

static SKILL_SCHED_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 构造一个归属后台会话（session None）的 SkillRun（字段全 pub，无需 AppHandle）
fn make_run(name: &str, state: SkillState) -> SkillRun {
    SkillRun {
        name: name.into(),
        state,
        step: 0,
        max_steps: 5,
        started_at_ms: chrono::Utc::now().timestamp_millis(),
        timeout_secs: 60,
        rollback: "none".into(),
        actions: Vec::new(),
        end_reason: String::new(),
        resumable: true,
        terminal_after_confirm: false,
        session_id: None,
    }
}

/// 读真 fixture（minimax-ppt）的 meta + body，与生产 load_skill_meta 同解析路径
fn fixture_meta_body() -> (wmessage_lib::bot_skills::SkillMeta, String) {
    let body = std::fs::read_to_string(ppt_fixture_path().join("SKILL.md"))
        .expect("read fixture SKILL.md");
    (parse_meta(&body, "minimax-ppt"), body)
}

/// bot.log 路径（probe_log_dir 在 cargo test 下 = current_exe 父目录，
/// 见 audit.rs probe_log_dir_matches_exe_parent_in_cargo_test；middleware 测试同先例）
fn bot_log_path() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("bot.log")
}

/// 记录运行前 bot.log 长度，运行后只断言新增尾巴（别的测试/历史运行也会写该文件）
fn bot_log_tail_since(offset: u64) -> String {
    let data = std::fs::read(&bot_log_path()).unwrap_or_default();
    String::from_utf8_lossy(&data[(offset as usize).min(data.len())..]).into_owned()
}

fn bot_log_len() -> u64 {
    std::fs::metadata(&bot_log_path()).map(|m| m.len()).unwrap_or(0)
}

/// 持久化校验用临时库：schema 镜像 db.rs 的 skill_outcomes DDL（集成测试够不到
/// 私有 db 模块的 open_db；upsert/load 本身走生产函数，防漂移面只剩这段 DDL）
fn open_temp_db(tag: &str) -> (rusqlite::Connection, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "wmessage-skill-e2e-{}-{tag}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let conn = rusqlite::Connection::open(&path).expect("open temp db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS skill_outcomes (
           skill_name         TEXT PRIMARY KEY,
           kind               TEXT NOT NULL,
           reason             TEXT,
           completed_summary  TEXT,
           rollback_attempted INTEGER,
           last_at_ms         INTEGER NOT NULL
         );",
    )
    .expect("create skill_outcomes");
    (conn, path)
}

/// persist_outcome 注入闭包：记录调用参数 + 走生产 upsert_skill_outcome 真落库
fn make_persist(
    conn: &rusqlite::Connection,
    calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>>,
) -> impl Fn(&str, &str, Option<&str>, Option<&str>, Option<bool>) + '_ {
    move |name, kind, reason, summary, rb| {
        calls.lock().unwrap().push((
            name.to_string(),
            kind.to_string(),
            reason.map(|s| s.to_string()),
            rb,
        ));
        upsert_skill_outcome(
            conn,
            &PersistedSkillOutcome {
                skill_name: name.to_string(),
                kind: kind.to_string(),
                reason: reason.map(|s| s.to_string()),
                completed_summary: summary.map(|s| s.to_string()),
                rollback_attempted: rb,
                last_at_ms: chrono::Utc::now().timestamp_millis(),
            },
        )
        .expect("persist 落库应成功");
    }
}

#[tokio::test]
async fn scheduler_e2e_done_path_finishes_run_audits_and_persists() {
    let _serial = SKILL_SCHED_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(make_run("minimax-ppt", SkillState::Running));
    let log_offset = bot_log_len();

    // mock executor：step1 返回 JSON（供 ${step1.result} 变量替换），step2 记录替换后 args
    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((tool.clone(), args));
            let text = match tool.as_str() {
                "list_tasks" => r#"[{"id":"7c9e6679-7425-40de-944b-e07fc1f90ae7","title":"买牛奶"}]"#.to_string(),
                "create_task" => "已创建".to_string(),
                other => panic!("意外工具调用：{other}"),
            };
            (text, Vec::new())
        }
    };
    let (conn, db_path) = open_temp_db("done");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(&app, "minimax-ppt", &meta, &body, None, exec, persist)
        .await
        .expect("fixture 两步全成功应 Done");

    // Done 收尾（P0-1 回归锁）：成功路径必须调真 skill_finish 把 Running → Completed
    assert_eq!(
        test_hook_skill_run_state("minimax-ppt"),
        Some(SkillState::Completed),
        "Done 路径应把 run 收尾为 Completed（P0-1：原先泄漏为僵尸 Running）"
    );
    // 工具编排 + 变量替换：两步都执行，step2 的 ${step1.result} 被真替换
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "应执行 2 步：{calls:?}");
    assert_eq!(calls[0].0, "list_tasks");
    assert_eq!(calls[1].0, "create_task");
    assert!(
        !calls[1].1.contains("${") && calls[1].1.contains("7c9e6679"),
        "step2 args 应完成变量替换：{}",
        calls[1].1
    );
    drop(calls);
    // Done 汇总文本
    match &outcome {
        DslOutcome::Done(summary) => {
            assert!(summary.contains("✅") && summary.contains("Step 1") && summary.contains("Step 2"));
        }
        other => panic!("期望 Done，got {other:?}"),
    }
    // 审计写入真路径（audit_log_hook → bot.log）：start / step×2 / done
    let tail = bot_log_tail_since(log_offset);
    for needle in [
        "skill_dsl_start | name: minimax-ppt | steps: 2",
        "skill_dsl_step | name: minimax-ppt | step: 1",
        "skill_dsl_step | name: minimax-ppt | step: 2",
        "skill_completed | name: minimax-ppt",
        "skill_dsl_done | name: minimax-ppt | steps_ok: 2",
    ] {
        assert!(tail.contains(needle), "bot.log 新增段缺「{needle}」：{tail}");
    }
    // 持久化真路径：persist 载荷 + 真 upsert 落库可回读
    assert_eq!(
        persist_calls.lock().unwrap().as_slice(),
        &[("minimax-ppt".to_string(), "done".to_string(), None, None)]
    );
    let stored = load_all_skill_outcomes(&conn).expect("load outcomes");
    let row = stored.get("minimax-ppt").expect("应有 minimax-ppt 行");
    assert_eq!(row.kind, "done");
    assert!(
        row.completed_summary.as_deref().unwrap_or("").contains("Step 1"),
        "落库 summary 应含步骤摘要：{row:?}"
    );

    test_hook_remove_skill_run("minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn scheduler_e2e_paused_run_returns_await_user() {
    // 确认窗口：run 处于 Paused（等用户确认）→ advance_dsl 在 step 1 前拦截，
    // 0 次工具调用，持久化 await_user，返回 DslOutcome::AwaitUser
    let _serial = SKILL_SCHED_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(make_run("minimax-ppt", SkillState::Paused));
    let log_offset = bot_log_len();

    let exec = |tool: String, args: String| async move {
        panic!("确认窗口内不应执行任何工具：{tool} {args}");
        #[allow(unreachable_code)]
        (String::new(), Vec::new())
    };
    let (conn, db_path) = open_temp_db("await");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(&app, "minimax-ppt", &meta, &body, None, exec, persist)
        .await
        .expect("Paused 应走 AwaitUser 而非硬错误");

    assert!(
        matches!(outcome, DslOutcome::AwaitUser),
        "期望 AwaitUser，got {outcome:?}"
    );
    assert_eq!(
        persist_calls.lock().unwrap().as_slice(),
        &[("minimax-ppt".to_string(), "await_user".to_string(), None, None)]
    );
    let stored = load_all_skill_outcomes(&conn).expect("load outcomes");
    assert_eq!(stored["minimax-ppt"].kind, "await_user");
    let tail = bot_log_tail_since(log_offset);
    assert!(
        tail.contains("skill_dsl_await_user | name: minimax-ppt | step: 1"),
        "应记 await_user 审计：{tail}"
    );
    // 暂停中的 run 不被调度器改动（等 bot_confirm_response 唤起）
    assert_eq!(
        test_hook_skill_run_state("minimax-ppt"),
        Some(SkillState::Paused)
    );

    test_hook_remove_skill_run("minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn scheduler_e2e_failed_step_runs_rollback_window() {
    // P0-5 回滚窗口：step 2 失败（生产里 skill_on_step_post 会把 run 标 Failed，
    // 这里由 mock executor 同步模拟该标记）→ 回滚段执行时 Failed 临时重开为
    // Running（原子工具门禁放行），结束后复原 Failed。
    let _serial = SKILL_SCHED_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(make_run("minimax-ppt", SkillState::Running));
    let log_offset = bot_log_len();

    let rollback_states: Arc<Mutex<Vec<Option<SkillState>>>> = Arc::new(Mutex::new(Vec::new()));
    let rb_states2 = rollback_states.clone();
    let exec = move |tool: String, _args: String| {
        let rb_states = rb_states2.clone();
        async move {
            match tool.as_str() {
                "list_tasks" => (r#"[]"#.to_string(), Vec::new()),
                "create_task" => {
                    // 模拟生产 skill_on_step_post：工具失败后 run 被标 Failed
                    test_hook_insert_skill_run(make_run("minimax-ppt", SkillState::Failed));
                    ("失败：模拟工具异常".to_string(), Vec::new())
                }
                "rollback_marker" => {
                    // 关键断言：回滚步骤执行时 run 必须已被重开为 Running（P0-5 窗口）
                    rb_states
                        .lock()
                        .unwrap()
                        .push(test_hook_skill_run_state("minimax-ppt"));
                    ("rolled back".to_string(), Vec::new())
                }
                other => panic!("意外工具调用：{other}"),
            }
        }
    };
    let (conn, db_path) = open_temp_db("rollback");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(&app, "minimax-ppt", &meta, &body, None, exec, persist)
        .await
        .expect("step 失败应走 FailedButRecoverable 而非硬错误");

    match &outcome {
        DslOutcome::FailedButRecoverable {
            reason,
            rollback_attempted,
            ..
        } => {
            assert!(reason.contains("Step 2"), "reason 应带失败步骤：{reason}");
            assert!(*rollback_attempted, "有回滚段且全部成功 → rollback_attempted=true");
        }
        other => panic!("期望 FailedButRecoverable，got {other:?}"),
    }
    // 回滚窗口：执行回滚步骤的那一刻 run 必须是 Running（重开），结束后复原 Failed
    assert_eq!(
        rollback_states.lock().unwrap().as_slice(),
        &[Some(SkillState::Running)],
        "回滚窗口内 run 应被临时重开为 Running（P0-5）"
    );
    assert_eq!(
        test_hook_skill_run_state("minimax-ppt"),
        Some(SkillState::Failed),
        "回滚结束后应复原 Failed 终态"
    );
    // 持久化 + 审计
    let calls = persist_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, "failed_recoverable");
    assert_eq!(calls[0].3, Some(true));
    drop(calls);
    let tail = bot_log_tail_since(log_offset);
    for needle in [
        "skill_dsl_rollback_start | name: minimax-ppt | step: 2",
        "skill_dsl_rollback_done | name: minimax-ppt | steps: 1 | failed: 0",
    ] {
        assert!(tail.contains(needle), "bot.log 新增段缺「{needle}」：{tail}");
    }

    test_hook_remove_skill_run("minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn scheduler_e2e_zombie_terminal_run_cleared_at_entry() {
    // agent 假死回归（2026-08-18 事故根因）：上轮遗留的 Completed 终态 run
    // 若不清理，会在第 0 步被 advance_dsl 误判 Finish 直接 break（0 次工具调用）。
    // core 入口的 clear_terminal_skill_runs 必须清掉它，两步照常执行。
    let _serial = SKILL_SCHED_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(make_run("minimax-ppt", SkillState::Completed));

    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, _args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push(tool);
            ("ok".to_string(), Vec::new())
        }
    };
    let (conn, db_path) = open_temp_db("zombie");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(&app, "minimax-ppt", &meta, &body, None, exec, persist)
        .await
        .expect("僵尸终态清理后应正常跑完");

    assert!(
        matches!(outcome, DslOutcome::Done(_)),
        "期望 Done（而非被僵尸 run 短路），got {outcome:?}"
    );
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["list_tasks".to_string(), "create_task".to_string()],
        "两步都应执行（未被第 0 步短路）"
    );

    test_hook_remove_skill_run("minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}
