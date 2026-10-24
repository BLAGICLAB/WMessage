//! 端到端测试
//! B5-6：`#![allow(clippy::await_holding_lock)]`——测试串行锁（SERIAL/MutexGuard）
//! **故意**持跨 await：#[tokio::test] 独立 current-thread runtime，guard 持有
//! 至测试结束正是串行化语义，无真实死锁面（std Mutex 阻塞的是本测试线程）。
#![allow(clippy::await_holding_lock)]

//!
//! 设计要点（务实方案）：
//! - **不依赖 Tauri runtime**（macOS EventLoop 主线程限制 + mock_runtime state lookup 失效）
//! - **不污染 target/debug/skills/**（避免触发 lib smoke_all_real_skills 回归）
//! - 真 Skill fixture 走 `tests/fixtures/` + `scan_skill_dirs` 直接加载
//!
//! 覆盖范围：
//! 1. pre-step 路由（intent_router 直调；原 MiddlewareRegistry 链已拆除为固定直调）
//! 2. 真 Skill fixture 解析（scan_skill_dirs + parse_meta）
//! 3. 任务卡执行流程识别（tool_guard 纯函数）
//! 4. 意图路由动态规则（intent_router 纯函数 + fixture 扫描，路由来自已安装技能 intents）
//!
//! run_skill_scheduler_core（调度器本体，重构后泛型 Runtime +
//! 注入 executor/persist）真路径 e2e 见本文件 scheduler_e2e_* 用例；run_model_loop_core
//! 对 mock LLM server 的真路径见 llm_integration.rs 的 core_* 用例。

use std::path::PathBuf;
use wmessage_lib::bot::registry::ToolResult;
use wmessage_lib::bot_skills::{self, scan_skill_dirs};
use wmessage_lib::intent_router::{self, RouteAction};
use wmessage_lib::tool_guard;

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

/// scheduler e2e 各节用的 AppState 注入句柄。App 泄漏给测试进程，退出即回收。
fn mock_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    Box::leak(Box::new(tauri::test::mock_app()))
        .handle()
        .clone()
}

// ────────────────────────────────────────────────────────────────────
// 1. pre-step 路由：关键词 → Skill 命中
// ────────────────────────────────────────────────────────────────────

/// 全局路由表 Drop 守卫：fixture 路由用毕还原空表。还原语句放在断言前只能保
/// happy path——assert panic 会跳过还原，全局表残留 fixture 路由污染同进程
/// 后续测试（cargo test 单进程跑全套时真实存在；nextest 每测试一进程不受影响）。
/// drop 兜底后两条路径都还原。
struct RoutesResetGuard;

impl Drop for RoutesResetGuard {
    fn drop(&mut self) {
        intent_router::rebuild_routes(vec![]);
    }
}

#[test]
fn pre_step_routes_installed_skill_intent_to_skill() {
    // 动态路由：路由表 = 已安装技能的 intents 声明。
    // 先按 fixture（已安装技能的替身）重建全局表，再走路由函数验证命中。
    // 重建的是进程级全局表 → 与其他同样重建全局表的用例串行。
    let _serial = SKILL_ROUTES_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    intent_router::rebuild_routes(bot_skills::intent_rules_from_dirs(
        &[fixtures_parent_path()],
    ));
    let _routes_guard = RoutesResetGuard;
    // 还原由 guard drop 兜底（panic 路径同样生效）
    let route = intent_router::route_user_input("帮我做一份 XX 主题的 PPT");
    match route {
        RouteAction::Skill(s) => {
            assert_eq!(s, "minimax-ppt", "期望命中 fixture Skill（intents 含 PPT）");
        }
        other => panic!("期望命中 Skill, got {other:?}"),
    }
}

/// 用户点名技能 → 优先于 intent 正则：
/// 「用 X 技能做个 PPT」必须落到 X，而不是被另一个技能的 "PPT" 正则截胡。
/// 被截胡的后果是模型只能中途 use_skill 切换——那条路径出过「刚切就断」的事故。
#[test]
fn pre_step_routes_explicitly_named_skill_before_intent_rule() {
    let _serial = SKILL_ROUTES_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!(
        "wmessage-skill-e2e-named-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    let alpha = dir.join("alpha-ppt");
    let beta = dir.join("beta-craft");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        alpha.join("SKILL.md"),
        "---\nname: alpha-ppt\ndescription: a\nintents: [\"PPT\"]\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(
        beta.join("SKILL.md"),
        "---\nname: beta-craft\ndescription: b\n---\nbody\n",
    )
    .unwrap();

    intent_router::rebuild_routes(bot_skills::intent_rules_from_dirs(std::slice::from_ref(
        &dir,
    )));
    let _routes_guard = RoutesResetGuard;
    let route = intent_router::route_user_input("用beta-craft技能做个PPT");
    match route {
        RouteAction::Skill(s) => assert_eq!(s, "beta-craft", "点名应压过 intent 正则"),
        other => panic!("期望命中 Skill, got {other:?}"),
    }
    // 正文大小写与技能名不一致也算点名（用户常写 "用 PPT-craft-master 技能做"）
    let route = intent_router::route_user_input("用BETA-CRAFT技能做个PPT");
    match route {
        RouteAction::Skill(s) => assert_eq!(s, "beta-craft", "点名匹配应大小写不敏感"),
        other => panic!("期望命中 Skill, got {other:?}"),
    }
    // 没点名 → 仍按 intent 正则走
    let route = intent_router::route_user_input("做个PPT");
    match route {
        RouteAction::Skill(s) => assert_eq!(s, "alpha-ppt", "没点名时按 intent 走"),
        other => panic!("期望命中 Skill, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pre_step_pass_through_for_normal_query() {
    assert!(intent_router::is_chat_execute_trigger("你好世界").is_none());
    assert!(matches!(
        intent_router::route_user_input("你好世界"),
        RouteAction::PassThrough
    ));
}

// ────────────────────────────────────────────────────────────────────
// 3. 任务卡执行流程识别（tool_guard 纯函数；原原子黑名单/中间件放行组已随框架拆除）
// ────────────────────────────────────────────────────────────────────

/// 配套契约：无 session 上下文 → 工具内部判为非任务执行流程（据此返回「无效果、不报错」）
#[test]
fn task_exec_flow_requires_session_context() {
    assert!(!tool_guard::is_task_execution_flow(None));
}

// ────────────────────────────────────────────────────────────────────
// 4. 意图路由：动态规则（路由 = 已安装技能的 intents 声明，
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
    assert!(body.contains("query_tasks({})"), "Step 1 应调 query_tasks");
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
// run_skill_scheduler_core 真路径 e2e
//
// 背景：P0-1（SKILL_RUNS 跨会话顶号 / Done 路径不收尾）/ P0-5（回滚窗口）
// 修复密集区原先无法直测——run_skill_scheduler 直连 Wry AppHandle +
// bot::execute_tool + db::open_db。重构后 core 泛型 Runtime + 注入
// execute_tool / persist_outcome，这里用 MockRuntime + mock executor +
// 真 fixture SKILL.md + 真 upsert_skill_outcome（临时库）直驱调度器本体。
//
// 并行隔离：本文件 4 个新用例都写 SKILL_RUNS，而集成测试是独立 crate——
// 既够不到 lib 内 cfg(test) 的测试辅助，也注不进 `pub(crate)` 的 `AppState`
// （阶段 3.3：表已随 AppState 走，集成测试统一落进程级兜底实例），
// 故在本二进制内自建一把串行锁（lib 侧对应的 `SKILL_RUNS_TEST_LOCK` 已随迁移撤掉）。
// ────────────────────────────────────────────────────────────────────

use std::sync::{Arc, Mutex};
use wmessage_lib::bot::{load_all_skill_outcomes, upsert_skill_outcome, PersistedSkillOutcome};
use wmessage_lib::bot_skills::{
    parse_meta, run_skill_scheduler_core, test_hook_insert_skill_run, test_hook_remove_skill_run,
    test_hook_skill_run_state, DslFailure, DslOutcome, SkillRun, SkillState,
};

/// 本文件里写进程级全局状态（SKILL_RUNS 表、全局路由表）的用例共用一把串行锁：
/// 集成测试是独立 crate，够不到 lib 内 cfg(test) 的测试辅助，也注不进 `pub(crate)`
/// 的 `AppState`，全局状态只能靠本进程内的锁互斥。
static SKILL_SCHED_TEST_LOCK: Mutex<()> = Mutex::new(());
/// 重建全局路由表的用例专用锁：路由表是进程级单例，两个用例同时重建会互相污染
/// （一个用例重建完，另一个用例断言时读到的已经是对方的表）。
static SKILL_ROUTES_TEST_LOCK: Mutex<()> = Mutex::new(());

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
        params: serde_json::Value::Null,
        dir: None,
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
/// 与生产写入路径一致：probe_log_dir 解析（根治后 cargo target
/// 路径下走 app_data_dir，不再命中 target/debug/deps 共享区——读路径必须用
/// 同一函数，否则读会读到旧进程残留行）。`db::data_dir` 是 probe_log_dir 的
/// 公开别名（pub 函数），测试包外用这一入口避免触及 `pub(crate)` 的 probe_log_dir。
fn bot_log_path() -> PathBuf {
    let app = tauri::test::mock_app();
    wmessage_lib::db::data_dir(app.handle()).join("bot.log")
}

/// 记录运行前 bot.log 长度，运行后只断言新增尾巴（别的测试/历史运行也会写该文件）
fn bot_log_tail_since(offset: u64) -> String {
    let data = std::fs::read(bot_log_path()).unwrap_or_default();
    String::from_utf8_lossy(&data[(offset as usize).min(data.len())..]).into_owned()
}

fn bot_log_len() -> u64 {
    std::fs::metadata(bot_log_path())
        .map(|m| m.len())
        .unwrap_or(0)
}

/// 持久化校验用临时库：schema 走单源 DDL（db::SKILL_OUTCOMES_DDL；集成测试够不到
/// 私有 db 模块的 open_db；upsert/load 本身走生产函数，DDL 已无漂移面）
fn open_temp_db(tag: &str) -> (rusqlite::Connection, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "wmessage-skill-e2e-{}-{tag}.db",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let conn = rusqlite::Connection::open(&path).expect("open temp db");
    conn.execute_batch(wmessage_lib::db::SKILL_OUTCOMES_DDL)
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
    let _serial = SKILL_SCHED_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(&app, make_run("minimax-ppt", SkillState::Running));
    let log_offset = bot_log_len();

    // mock executor：step1 返回 JSON（供 ${step1.result} 变量替换），step2 记录替换后 args
    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((tool.clone(), args));
            let text = match tool.as_str() {
                "query_tasks" => {
                    r#"[{"id":"7c9e6679-7425-40de-944b-e07fc1f90ae7","title":"买牛奶"}]"#
                        .to_string()
                }
                "create_task" => "已创建".to_string(),
                other => panic!("意外工具调用：{other}"),
            };
            ToolResult::ok(text, Vec::new())
        }
    };
    let (conn, db_path) = open_temp_db("done");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(
        &app,
        "minimax-ppt",
        &meta,
        &body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("fixture 两步全成功应 Done");

    // Done 收尾（P0-1 回归锁）：成功路径必须调真 skill_finish 把 Running → Completed
    assert_eq!(
        test_hook_skill_run_state(&app, "minimax-ppt"),
        Some(SkillState::Completed),
        "Done 路径应把 run 收尾为 Completed（P0-1：原先泄漏为僵尸 Running）"
    );
    // 工具编排 + 变量替换：两步都执行，step2 的 ${step1.result} 被真替换
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "应执行 2 步：{calls:?}");
    assert_eq!(calls[0].0, "query_tasks");
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
            assert!(
                summary.contains("✅") && summary.contains("Step 1") && summary.contains("Step 2")
            );
        }
        other => panic!("期望 Done，got {other:?}"),
    }
    // 审计写入真路径（audit_log_hook → bot.log）：start / step×2 / done。
    // 桥接后 free-form 行落盘为「INFO | 事件 | detail=...」，且 detail 内的
    // 「 | 」被强转义成「|  」（防伪造日志行）——断言按事件名与键值分别 contains，
    // 不再拼整行（跨转义边界）。
    let tail = bot_log_tail_since(log_offset);
    for needle in [
        "skill_dsl_start",
        "name: minimax-ppt",
        "steps: 2",
        "skill_dsl_step",
        "step: 1",
        "step: 2",
        "skill_completed",
        "skill_dsl_done",
        "steps_ok: 2",
    ] {
        assert!(
            tail.contains(needle),
            "bot.log 新增段缺「{needle}」：{tail}"
        );
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
        row.completed_summary
            .as_deref()
            .unwrap_or("")
            .contains("Step 1"),
        "落库 summary 应含步骤摘要：{row:?}"
    );

    test_hook_remove_skill_run(&app, "minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

/// 别的技能接管了本会话（用户 use_skill 切换）→ 调度器必须收手：
/// 一步都不跑、不记 done、不碰接管方的 run 状态。
/// 推进接管方的状态机 = 把别人的 run 当自己的用（技能一用就断的同源问题）。
#[tokio::test]
async fn scheduler_e2e_bails_out_when_another_skill_takes_over() {
    let _serial = SKILL_SCHED_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    // 同会话里另一个技能在跑（= 用户 use_skill 切换后的状态）
    let mut other = make_run("other-skill", SkillState::Running);
    other.session_id = Some("sess-takeover".into());
    test_hook_insert_skill_run(&app, other);
    let log_offset = bot_log_len();

    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((tool.clone(), args));
            ToolResult::ok("不应被调用".to_string(), Vec::new())
        }
    };
    let (conn, db_path) = open_temp_db("takeover");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(
        &app,
        "minimax-ppt",
        &meta,
        &body,
        Some("sess-takeover"),
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await;

    match outcome {
        Err(DslFailure::Terminated { reason }) => {
            assert!(reason.contains("接管"), "reason 应说明被接管：{reason}");
        }
        other => panic!("期望 Terminated（被接管），got {other:?}"),
    }
    assert!(calls.lock().unwrap().is_empty(), "被接管后不该再跑任何步骤");
    assert_eq!(
        test_hook_skill_run_state(&app, "other-skill"),
        Some(SkillState::Running),
        "接管方的 run 状态不该被本调度器动过"
    );
    let persisted = persist_calls.lock().unwrap().clone();
    assert_eq!(persisted.len(), 1, "只记一次终态：{persisted:?}");
    assert_eq!(
        persisted[0].1, "terminated",
        "被接管记 terminated，不是 done"
    );
    let tail = bot_log_tail_since(log_offset);
    for needle in [
        "skill_dsl_run_replaced",
        "name: minimax-ppt",
        "active: other-skill",
    ] {
        assert!(
            tail.contains(needle),
            "bot.log 应留接管痕迹（缺 {needle}）：{tail}"
        );
    }

    test_hook_remove_skill_run(&app, "other-skill");
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn scheduler_e2e_paused_run_returns_await_user() {
    // 确认窗口：run 处于 Paused（等用户确认）→ advance_dsl 在 step 1 前拦截，
    // 0 次工具调用，持久化 await_user，返回 DslOutcome::AwaitUser
    let _serial = SKILL_SCHED_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(&app, make_run("minimax-ppt", SkillState::Paused));
    let log_offset = bot_log_len();

    let exec = |tool: String, args: String| async move {
        panic!("确认窗口内不应执行任何工具：{tool} {args}");
        #[allow(unreachable_code)]
        ToolResult::ok(String::new(), Vec::new())
    };
    let (conn, db_path) = open_temp_db("await");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(
        &app,
        "minimax-ppt",
        &meta,
        &body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("Paused 应走 AwaitUser 而非硬错误");

    assert!(
        matches!(outcome, DslOutcome::AwaitUser),
        "期望 AwaitUser，got {outcome:?}"
    );
    assert_eq!(
        persist_calls.lock().unwrap().as_slice(),
        &[(
            "minimax-ppt".to_string(),
            "await_user".to_string(),
            None,
            None
        )]
    );
    let stored = load_all_skill_outcomes(&conn).expect("load outcomes");
    assert_eq!(stored["minimax-ppt"].kind, "await_user");
    let tail = bot_log_tail_since(log_offset);
    for needle in ["skill_dsl_await_user", "name: minimax-ppt", "step: 1"] {
        assert!(
            tail.contains(needle),
            "应记 await_user 审计（缺 {needle}）：{tail}"
        );
    }
    // 暂停中的 run 不被调度器改动（等 bot_confirm_response 唤起）
    assert_eq!(
        test_hook_skill_run_state(&app, "minimax-ppt"),
        Some(SkillState::Paused)
    );

    test_hook_remove_skill_run(&app, "minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn scheduler_e2e_failed_step_runs_rollback_window() {
    // P0-5 回滚窗口：step 2 失败（生产里 skill_on_step_post 会把 run 标 Failed，
    // 这里由 mock executor 同步模拟该标记）→ 回滚段执行时 Failed 临时重开为
    // Running（原子工具门禁放行），结束后复原 Failed。
    let _serial = SKILL_SCHED_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (mut meta, body) = fixture_meta_body();
    // 回滚段只在 `rollback: auto` 时才执行（作者声明 none = 不执行逆操作）。
    // 这个用例验的是「回滚窗口」机制（回滚执行时 run 被临时重开为 Running），
    // 所以显式声明 auto；夹具里的 none 留着当「不跑回滚」那侧的样本。
    meta.rollback = "auto".into();
    let app = mock_handle();
    test_hook_insert_skill_run(&app, make_run("minimax-ppt", SkillState::Running));
    let log_offset = bot_log_len();

    let rollback_states: Arc<Mutex<Vec<Option<SkillState>>>> = Arc::new(Mutex::new(Vec::new()));
    let app_for_exec = app.clone();
    let rb_states2 = rollback_states.clone();
    let exec = move |tool: String, _args: String| {
        let rb_states = rb_states2.clone();
        let app = app_for_exec.clone();
        async move {
            match tool.as_str() {
                "query_tasks" => ToolResult::ok(r#"[]"#, Vec::new()),
                "create_task" => {
                    // 模拟生产 skill_on_step_post：工具失败后 run 被标 Failed
                    test_hook_insert_skill_run(&app, make_run("minimax-ppt", SkillState::Failed));
                    // status=Error 而非 ok：调度器按 ToolStatus::is_success() 判步骤
                    // 成败（成功与否的唯一真相源），ok + 失败文案是生产里不存在的组合
                    ToolResult::error("失败：模拟工具异常", Vec::new())
                }
                "rollback_marker" => {
                    // 关键断言：回滚步骤执行时 run 必须已被重开为 Running（P0-5 窗口）
                    rb_states
                        .lock()
                        .unwrap()
                        .push(test_hook_skill_run_state(&app, "minimax-ppt"));
                    ToolResult::ok("rolled back", Vec::new())
                }
                other => panic!("意外工具调用：{other}"),
            }
        }
    };
    let (conn, db_path) = open_temp_db("rollback");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(
        &app,
        "minimax-ppt",
        &meta,
        &body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("step 失败应走 FailedButRecoverable 而非硬错误");

    match &outcome {
        DslOutcome::FailedButRecoverable {
            reason,
            rollback_attempted,
            ..
        } => {
            assert!(reason.contains("Step 2"), "reason 应带失败步骤：{reason}");
            assert!(
                *rollback_attempted,
                "有回滚段且全部成功 → rollback_attempted=true"
            );
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
        test_hook_skill_run_state(&app, "minimax-ppt"),
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
        "skill_dsl_rollback_start",
        "name: minimax-ppt",
        "step: 2",
        "skill_dsl_rollback_done",
        "steps: 1",
        "failed: 0",
    ] {
        assert!(
            tail.contains(needle),
            "bot.log 新增段缺「{needle}」：{tail}"
        );
    }

    test_hook_remove_skill_run(&app, "minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn scheduler_e2e_zombie_terminal_run_cleared_at_entry() {
    // agent 假死回归（事故根因）：上轮遗留的 Completed 终态 run
    // 若不清理，会在第 0 步被 advance_dsl 误判 Finish 直接 break（0 次工具调用）。
    // core 入口的 clear_terminal_skill_runs 必须清掉它，两步照常执行。
    let _serial = SKILL_SCHED_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (meta, body) = fixture_meta_body();
    let app = mock_handle();
    test_hook_insert_skill_run(&app, make_run("minimax-ppt", SkillState::Completed));

    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, _args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push(tool);
            ToolResult::ok("ok", Vec::new())
        }
    };
    let (conn, db_path) = open_temp_db("zombie");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(
        &app,
        "minimax-ppt",
        &meta,
        &body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("僵尸终态清理后应正常跑完");

    assert!(
        matches!(outcome, DslOutcome::Done(_)),
        "期望 Done（而非被僵尸 run 短路），got {outcome:?}"
    );
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &["query_tasks".to_string(), "create_task".to_string()],
        "两步都应执行（未被第 0 步短路）"
    );

    test_hook_remove_skill_run(&app, "minimax-ppt");
    let _ = std::fs::remove_file(&db_path);
}

// ────────────────────────────────────────────────────────────────────
// 受管服务：Skill DSL 全链路（真工具 + 真闸 + 真进程）
// ────────────────────────────────────────────────────────────────────

/// 受管服务走**真实 Skill DSL 链路**的端到端用例。
///
/// 单测验的是 `substitute_vars` 这个纯函数，验不到
/// 「调度器 → 变量替换 → 工具入参解析 → 授权闸 → 真进程 → 登记表 → 停止」这条链。
/// 这里把 executor 换成**真工具**（生产实现，非 mock）。
///
/// 覆盖与不覆盖（别误以为全链路都测到了）：
/// - 覆盖：调度器步骤推进、变量替换（`${step1.serviceId}`）、工具入参解析、
///   授权闸（设置页开关 + 启动器白名单）、真进程起停、登记表状态机。
/// - 不覆盖：**dispatch 层的三道闸**（pre_tool 钩子 / per-tool 规则表 /
///   子 agent 白名单）与 **bot_fs 路径授权闸**——前者 dispatch 整体是 Wry 专属，
///   后者要读权限档并弹窗，两者都在 Wry 层；本仓测试只有 mock runtime
///   （Wry 需要显示环境 + 主线程，见本文件头部说明）。
///   路径闸在测试里用「固定解析器」替身：它只把入参路径变成 canonical 路径，
///   不做授权判定。
#[tokio::test]
async fn scheduler_e2e_managed_service_start_then_stop_via_real_tool() {
    let _serial = SKILL_SCHED_TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let app = mock_handle();
    test_hook_insert_skill_run(&app, make_run("svc-dsl", SkillState::Running));

    // 闸 1（设置页开关）默认关 —— 走生产 API 打开，收尾再关掉
    wmessage_lib::bot::services::set_enabled(&app, true).expect("受管服务开关应可写");

    // 服务的工作目录：随便一个存在的目录（路径闸在测试里是替身）
    let cwd = std::env::temp_dir().join(format!("wmessage-svc-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).expect("建临时工作目录");

    // 真 Skill 文本：两步，起服务 → 用上一步的 serviceId 停服务
    let body = format!(
        "---\nname: svc-dsl\ndescription: 受管服务 DSL 全链路测试\nmode: auto\n\
         risk_level: low\nmax_steps: 5\ntimeout_secs: 60\nrollback: none\n\
         allowed-tools: start_service, stop_service\n---\n\n\
         # 受管服务 DSL 测试\n\n\
         ## Step 1: 启动长驻服务\n\
         start_service({{\"command\":\"python3\",\"args\":[\"-c\",\"import time\\nwhile True: time.sleep(1)\"],\"cwd\":{:?}}})\n\n\
         ## Step 2: 用上一步的 serviceId 停掉它\n\
         stop_service({{\"serviceId\":\"${{step1.serviceId}}\"}})\n",
        cwd.to_string_lossy()
    );
    let meta = parse_meta(&body, "svc-dsl");

    // executor 换成真工具
    let exec_app = app.clone();
    let exec = move |tool: String, args: String| {
        let app = exec_app.clone();
        async move {
            match tool.as_str() {
                "start_service" => {
                    wmessage_lib::bot::tools::test_hook_tool_start_service(
                        &app,
                        &args,
                        false,
                        |p: String| -> std::pin::Pin<
                            Box<
                                dyn std::future::Future<Output = Result<std::path::PathBuf, String>>
                                    + Send,
                            >,
                        > {
                            Box::pin(async move { Ok(std::path::PathBuf::from(p)) })
                        },
                    )
                    .await
                }
                "stop_service" => {
                    wmessage_lib::bot::tools::test_hook_tool_stop_service(&app, &args).await
                }
                other => panic!("意外工具调用：{other}"),
            }
        }
    };

    let (conn, db_path) = open_temp_db("svcdsl");
    let persist_calls: Arc<Mutex<Vec<(String, String, Option<String>, Option<bool>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let persist = make_persist(&conn, persist_calls.clone());

    let outcome = run_skill_scheduler_core(
        &app,
        "svc-dsl",
        &meta,
        &body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await;

    // 收尾：关开关、删临时目录
    let _ = wmessage_lib::bot::services::set_enabled(&app, false);
    let _ = std::fs::remove_dir_all(&cwd);

    match &outcome {
        Ok(DslOutcome::Done(summary)) => {
            assert!(summary.contains("✅"), "两步都该成功：{summary}");
        }
        other => panic!("期望 Done，got {other:?}"),
    }

    // 服务确实起过又被停掉：登记表里是终态 Exited
    let services = wmessage_lib::bot::services::list_all(&app);
    assert_eq!(services.len(), 1, "应恰好登记一个服务：{services:?}");
    assert_eq!(
        services[0].state.as_str(),
        "exited",
        "step2 必须把服务停掉（step1 的 serviceId 经变量替换传到了 step2）"
    );

    test_hook_remove_skill_run(&app, "svc-dsl");
    let _ = std::fs::remove_file(&db_path);
}
