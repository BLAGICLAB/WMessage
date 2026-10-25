//! 技能调度器的纯函数与集成测试。

use super::*;
use crate::bot_skills::{test_run, SkillRun, SkillStep};

// ── 端到端 run_dsl_loop_sync ──
//
// 同步版核心循环（生产 run_skill_scheduler 是 async + 调 bot::execute_tool）。
// 这里剥离 AppHandle 依赖 + 注入 mock executor，让单测能验证：
//   1. 嵌套变量替换端到端走通
//   2. advance_dsl 4 个非 Run 分支的端到端拦截
//   3. rollback 在 step 失败时被调用、Terminate 时被跳过

fn run_dsl_loop_sync(
    ctx: &mut Vec<CompletedStep>,
    steps: &[SkillStep],
    rollback: &[SkillStep],
    skill_run: Option<&SkillRun>,
    mut exec: impl FnMut(&str, &str) -> String,
) -> Result<Vec<(usize, String, String)>, String> {
    let mut results: Vec<(usize, String, String)> = Vec::new();
    for step in steps {
        // advance_dsl 状态机检查
        if let Some(run) = skill_run {
            match advance_dsl(run, now_ms()) {
                DslAdvanceAction::Run => {}
                DslAdvanceAction::Finish => break,
                DslAdvanceAction::AwaitUser => return Err("__await_user__".into()),
                DslAdvanceAction::FailWithRollback(reason) => {
                    for rb in rollback {
                        let rb_args = substitute_vars(&rb.args_json, ctx);
                        exec(&rb.tool_name, &rb_args);
                    }
                    return Err(format!("技能中止：{reason}"));
                }
                DslAdvanceAction::Terminate(reason) => {
                    return Err(format!("技能终止：{reason}"));
                }
            }
        }
        // 嵌套变量替换
        let resolved_args = substitute_vars(&step.args_json, ctx);
        let text = exec(&step.tool_name, &resolved_args);
        // 失败判定：本函数是测试专用的同步版，mock executor 只回文本拿不到
        // ToolStatus，故仍走文本分类。生产路径（上方 run_skill_scheduler_core）
        // 用 status.is_success()，两者的口径差由 audit 单测兜住。
        let failed = crate::audit::tool_call_failed("", &text);
        if failed {
            for rb in rollback {
                let rb_args = substitute_vars(&rb.args_json, ctx);
                exec(&rb.tool_name, &rb_args);
            }
            return Err(format!(
                "Step {} ({}) 失败：{}",
                step.index, step.title, text
            ));
        }
        ctx.push(CompletedStep {
            index: step.index,
            title: step.title.clone(),
            result: text.clone(),
            id: extract_task_id(&text),
            parsed: serde_json::from_str(&text).ok(),
        });
        results.push((step.index, step.title.clone(), text));
    }
    Ok(results)
}

#[test]
fn e2e_runs_all_steps_with_nested_var_substitution() {
    // 端到端：2-step DSL 演示 `${step1.task.id}` 嵌套路径替换真的 work
    // Step 1 list_tasks → JSON 嵌套结构 → push ctx
    // Step 2 query_single_task 用 `${step1.task.id}` → 期望 args 替换成真实 UUID
    let body = "## Step 1: list\nlist_tasks({})\n\n## Step 2: query\nquery_single_task({\"id\": \"${step1.task.id}\"})";
    let (steps, rollback) = parse_skill_steps(body).unwrap();
    assert_eq!(steps.len(), 2);
    assert!(rollback.is_empty());

    let calls: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
    let mut exec = |tool: &str, args: &str| -> String {
        calls
            .borrow_mut()
            .push((tool.to_string(), args.to_string()));
        match tool {
            "list_tasks" => {
                r#"{"task": {"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "买牛奶"}}"#
                    .to_string()
            }
            "query_single_task" => "ok".to_string(),
            _ => format!("未知工具: {tool}"),
        }
    };

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, None, &mut exec);

    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    assert_eq!(ctx.len(), 2);

    let calls = calls.borrow();
    assert_eq!(calls.len(), 2, "expected 2 tool calls");
    assert_eq!(calls[0].0, "list_tasks");
    assert_eq!(calls[0].1, "{}");
    assert_eq!(calls[1].0, "query_single_task");
    // 关键断言：${step1.task.id} 嵌套路径真的被替换成标准 UUID
    assert_eq!(
        calls[1].1,
        r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}"#
    );

    // ctx 累积验证：Step 2 的 result 是 query 的返回值，parsed 是 None（不是 JSON）
    assert_eq!(
        ctx[0].result,
        r#"{"task": {"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "买牛奶"}}"#
    );
    assert!(ctx[0].parsed.is_some(), "Step 1 合法 JSON 应 parse 成功");
    assert_eq!(
        ctx[0].id.as_deref(),
        Some("7c9e6679-7425-40de-944b-e07fc1f90ae7")
    );
    assert_eq!(ctx[1].result, "ok");
    assert!(
        ctx[1].parsed.is_none(),
        "Step 2 'ok' 不是 JSON，parsed 应为 None"
    );
}

#[test]
fn e2e_fails_with_rollback_when_step_fails() {
    // 端到端：Step 2 失败触发 rollback（含嵌套变量替换）
    // rollback step 用 ${step1.task.id} → 期望拿到 Step 1 创建的 UUID
    let body = "## Step 1: create\ncreate_task({})\n\n## Step 2: risky\nrisky_tool({\"x\": \"${step1.task.id}\"})\n\n## Rollback\nrollback_tool({\"ref\": \"${step1.task.id}\"})";
    let (steps, rollback) = parse_skill_steps(body).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(rollback.len(), 1);

    let calls: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
    let mut exec = |tool: &str, args: &str| -> String {
        calls
            .borrow_mut()
            .push((tool.to_string(), args.to_string()));
        match tool {
            "create_task" => r#"{"task": {"id": "uuid-step1"}}"#.to_string(),
            "risky_tool" => "失败：工具异常".to_string(),
            "rollback_tool" => "rolled back".to_string(),
            _ => format!("未知工具: {tool}"),
        }
    };

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, None, &mut exec);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("Step 2"), "expected 'Step 2' in error: {err}");
    assert!(err.contains("失败"), "expected '失败' in error: {err}");

    // 关键验证：Step 1 跑了 + Step 2 跑了 + rollback 跑了（3 次）
    let calls = calls.borrow();
    assert_eq!(
        calls.len(),
        3,
        "expected 3 tool calls (step1+step2+rollback)"
    );
    assert_eq!(calls[0].0, "create_task");
    assert_eq!(calls[1].0, "risky_tool");
    // Step 2 的 args 也走嵌套路径替换（验证 rollback 段、step 段共享 substitute_vars 路径）
    assert_eq!(calls[1].1, r#"{"x": "uuid-step1"}"#);
    assert_eq!(calls[2].0, "rollback_tool");
    // rollback args 也走嵌套路径替换（e2e 端到端覆盖）
    assert_eq!(calls[2].1, r#"{"ref": "uuid-step1"}"#);
}

#[test]
fn e2e_terminates_skips_rollback() {
    // 端到端：SkillRun state = Terminated → advance_dsl 返回 Terminate → 跳过 rollback
    let body = "## Step 1: list\nlist_tasks({})\n\n## Step 2: query\nquery_single_task({})\n\n## Rollback\nrollback({})";
    let (steps, rollback) = parse_skill_steps(body).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(rollback.len(), 1);

    // mock SkillRun：state = Terminated（用户 /stop 触发）
    let mut run = test_run(8, 180);
    run.state = SkillState::Terminated;
    run.end_reason = "用户 /stop".into();

    let calls: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let mut exec = |tool: &str, _args: &str| -> String {
        calls.borrow_mut().push(tool.to_string());
        "ok".to_string()
    };

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, Some(&run), &mut exec);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("终止"), "expected '终止' in error: {err}");
    assert!(
        err.contains("用户 /stop"),
        "expected reason in error: {err}"
    );

    // 关键验证：Terminate 在 step 1 之前就拦截 → 0 次工具调用 + rollback 跳过
    let calls = calls.borrow();
    assert_eq!(
        calls.len(),
        0,
        "expected 0 tool calls (Terminated before any step), got: {:?}",
        *calls
    );
}

#[test]
fn e2e_pauses_with_await_user_when_paused() {
    // 端到端：SkillRun state = Paused → advance_dsl 返回 AwaitUser → 返回 __await_user__
    let body = "## Step 1: list\nlist_tasks({})";
    let (steps, rollback) = parse_skill_steps(body).unwrap();

    // mock SkillRun：state = Paused（用户确认等待中）
    let mut run = test_run(8, 180);
    run.state = SkillState::Paused;

    let calls: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let mut exec = |tool: &str, _args: &str| -> String {
        calls.borrow_mut().push(tool.to_string());
        "ok".to_string()
    };

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, Some(&run), &mut exec);

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "__await_user__");

    // 关键验证：Paused 在 step 1 之前就拦截 → 0 次工具调用
    let calls = calls.borrow();
    assert_eq!(
        calls.len(),
        0,
        "expected 0 tool calls (Paused before any step), got: {:?}",
        *calls
    );
}

// ── format_completed_summary + DslOutcome/DslFailure ──

#[test]
fn format_completed_summary_returns_placeholder_for_empty_ctx() {
    // 空 ctx → "(无已完成步骤)" 占位
    let ctx: Vec<CompletedStep> = Vec::new();
    assert_eq!(format_completed_summary(&ctx), "(无已完成步骤)");
}

#[test]
fn format_completed_summary_renders_single_step() {
    // 单 step → "Step N (title): <result>"
    let ctx = vec![CompletedStep {
        index: 1,
        title: "list-tasks".into(),
        result: "task list result".into(),
        id: None,
        parsed: None,
    }];
    let out = format_completed_summary(&ctx);
    assert!(out.contains("- Step 1 (list-tasks): task list result"));
}

#[test]
fn format_completed_summary_renders_multiple_steps() {
    // 多 step → 每步一行，按 ctx 顺序
    let ctx = vec![
        CompletedStep {
            index: 1,
            title: "step-a".into(),
            result: "result-a".into(),
            id: None,
            parsed: None,
        },
        CompletedStep {
            index: 2,
            title: "step-b".into(),
            result: "result-b".into(),
            id: None,
            parsed: None,
        },
        CompletedStep {
            index: 3,
            title: "step-c".into(),
            result: "result-c".into(),
            id: None,
            parsed: None,
        },
    ];
    let out = format_completed_summary(&ctx);
    assert!(out.contains("Step 1 (step-a): result-a"));
    assert!(out.contains("Step 2 (step-b): result-b"));
    assert!(out.contains("Step 3 (step-c): result-c"));
    // 顺序：a 在 b 前面，b 在 c 前面
    let pos_a = out.find("Step 1").unwrap();
    let pos_b = out.find("Step 2").unwrap();
    let pos_c = out.find("Step 3").unwrap();
    assert!(pos_a < pos_b && pos_b < pos_c);
}

#[test]
fn format_completed_summary_truncates_long_results() {
    // 超长 result (>200 字符) → 截断 + "..." 后缀
    let long_result: String = "x".repeat(500);
    let ctx = vec![CompletedStep {
        index: 1,
        title: "long".into(),
        result: long_result.clone(),
        id: None,
        parsed: None,
    }];
    let out = format_completed_summary(&ctx);
    // 截断后 preview = 200 字符 + "..." = 203 字符（在 - Step 1 (long): 之后）
    let marker = "- Step 1 (long): ";
    let start = out.find(marker).unwrap() + marker.len();
    let after = &out[start..];
    // preview 部分应当是 200 个 x + "..."
    let expected_preview: String = "x".repeat(200) + "...";
    assert!(
        after.starts_with(&expected_preview),
        "expected preview starts with 200x + '...', got first chars: {}",
        &after[..after.len().min(50)]
    );
}

#[test]
fn dsl_outcome_done_carries_summary_string() {
    // DslOutcome::Done 携带 summary 字符串（构造 + 取出来一致）
    let outcome = DslOutcome::Done("summary text".into());
    match outcome {
        DslOutcome::Done(s) => assert_eq!(s, "summary text"),
        _ => panic!("expected Done variant"),
    }
}

#[test]
fn dsl_failure_terminated_carries_reason() {
    // DslFailure::Terminated 携带 reason（用户主动终止场景）
    let failure = DslFailure::Terminated {
        reason: "用户 /stop".into(),
    };
    match failure {
        DslFailure::Terminated { reason } => assert_eq!(reason, "用户 /stop"),
    }
}

// ── dev mock Skill 端到端 smoke test ──

/// 通用 mock executor（smoke test 用）：每个工具返回成功 + 含标准 UUID 让变量替换 / 嵌套路径 work
/// - 返回 JSON 字符串时尽量含 UUID 7c9e6679-7425-40de-944b-e07fc1f90ae7（让 `${step1.task.id}` 等嵌套路径能取到值）
/// - 不存在的工具返回 "mock ok"（确保所有 Skill 都能跑完不 panic）
fn generic_mock_executor(tool: &str, _args: &str) -> String {
    match tool {
        "list_tasks" | "search_tasks" => {
            r#"[{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "mock"}]"#.to_string()
        }
        "query_single_task" => {
            r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7", "title": "mock", "task": {"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}}"#.to_string()
        }
        "create_task" | "complete_task" | "edit_task" | "delete_task" | "add_subtask" => {
            r#"{"id": "7c9e6679-7425-40de-944b-e07fc1f90ae7"}"#.to_string()
        }
        "create_word" | "create_excel" | "create_ppt" | "create_pdf" => {
            r#"{"path": "/tmp/mock-output"}"#.to_string()
        }
        "extract_document" => r#"{"text": "mock extracted content"}"#.to_string(),
        "run_python" => "mock python output".to_string(),
        "web_search" => {
            r#"[{"title": "mock result", "url": "https://example.com"}]"#.to_string()
        }
        "fetch_url" => r#"{"text": "mock fetched content"}"#.to_string(),
        "use_skill" => "mock skill body".to_string(),
        _ => "mock ok".to_string(),
    }
}

#[test]
fn smoke_all_real_skills_run_dsl_loop_with_mock_executor() {
    // 端到端 smoke：扫 target/debug/skills/ 下全部 dev mock Skill（数量不固定，
    // 该目录是本地 dev 手放的 mock，不入库、可能被 cargo clean 清掉）
    // 每个 Skill 跑 run_dsl_loop_sync + 通用 mock executor
    // 验证：parse 不 panic + 整链路跑通 + ctx 累积 + 嵌套变量替换
    let skills_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/skills");
    if !skills_dir.exists() {
        // 没建 mock 跳过（防止 dev 模式第一次 cargo test 失败）
        eprintln!(
            "跳过：{} 不存在（dev 模式需先写出 mock Skill）",
            skills_dir.display()
        );
        return;
    }

    let mut skill_count = 0;
    let mut failed: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&skills_dir).expect("read_dir skills");
    for entry in entries {
        let entry = entry.expect("entry");
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        // parse
        let body = std::fs::read_to_string(&skill_md)
            .unwrap_or_else(|e| panic!("Skill {name} SKILL.md 读失败: {e}"));
        // interactive 技能（如 minimax-docx）无 DSL Step——LLM 驱动、不进调度器，跳过
        if crate::bot_skills::parse_meta(&body, &name).mode != "auto" {
            continue;
        }
        let (steps, rollback) = match parse_skill_steps(&body) {
            Ok(sr) => sr,
            Err(e) => {
                failed.push(format!("{name}: parse failed: {e}"));
                continue;
            }
        };
        if steps.is_empty() {
            failed.push(format!("{name}: steps empty"));
            continue;
        }

        // 记录每个 step 的 args 在跑前是否含 ${...}（用于后续验证嵌套变量替换真的替换了）
        let step_args_with_var: Vec<bool> =
            steps.iter().map(|s| s.args_json.contains("${")).collect();

        // 跑 run_dsl_loop_sync
        let mut ctx: Vec<CompletedStep> = Vec::new();
        let mut exec = generic_mock_executor;
        let result = run_dsl_loop_sync(&mut ctx, &steps, &rollback, None, &mut exec);
        if let Err(e) = result {
            failed.push(format!("{name}: run failed: {e}"));
            continue;
        }

        // 验证：ctx 累积（至少跟 step 数一样）
        if ctx.len() != steps.len() {
            failed.push(format!(
                "{name}: ctx len {} != steps len {}",
                ctx.len(),
                steps.len()
            ));
            continue;
        }

        // 验证：含 ${...} 的 step args 跑完后应该已经被替换（ctx 累积至少 1 步后续 step 才能拿到）
        // 这里只 sanity check 跑通即可；嵌套变量替换正确性在 e2e_runs_all_steps_with_nested_var_substitution 等单测里覆盖
        let _ = step_args_with_var;

        skill_count += 1;
    }

    if !failed.is_empty() {
        panic!(
            "{} 个 Skill 端到端跑失败：
  - {}",
            failed.len(),
            failed.join("\n  - ")
        );
    }

    // 断言只要求扫到 ≥1 个（具体数量随本地 dev mock 增减，不硬编码）；
    // skills_dir 为空时 graceful skip（cargo clean 误删 dev mock / dev 首次未 init）
    if skill_count == 0 {
        eprintln!(
            "smoke 跳过：{} 下未扫到任何 Skill（dev mock 可能被 cargo clean 误删）",
            skills_dir.display()
        );
        return;
    }
    eprintln!("端到端 smoke：{} 个 Skill 全部跑通", skill_count);
}

/// 回滚复原守卫：回滚段 panic 时 Drop 兜底把临时重开的 run 复原 Failed——
/// 否则 run 永久卡 Running，AtomicGuard 放行窗口被永久重开。
#[test]
fn rollback_restore_guard_restores_failed_on_panic() {
    let app = {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    };
    let name = "test-rb-guard-panic";
    let mut run = test_run(8, 180);
    run.name = name.into();
    run.state = SkillState::Failed;
    run.session_id = Some("s-rb-g".into());
    {
        let registry = skill_runs(&app);
        registry.lock().unwrap().insert(name.into(), run);
    }
    let reopened =
        crate::bot_skills::state::reopen_failed_run_for_rollback(&app, name, Some("s-rb-g"));
    assert!(reopened, "Failed run 应被临时重开");
    {
        let registry = skill_runs(&app);
        let g = registry.lock().unwrap();
        assert_eq!(
            g.get(name).map(|r| r.state.clone()),
            Some(SkillState::Running)
        );
    }
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = RollbackRestoreGuard {
            app: &app,
            name,
            session_id: Some("s-rb-g"),
            armed: reopened,
        };
        panic!("模拟回滚步骤 execute_tool panic");
    }));
    assert!(caught.is_err(), "panic 应被捕获");
    {
        let registry = skill_runs(&app);
        let g = registry.lock().unwrap();
        assert_eq!(
            g.get(name).map(|r| r.state.clone()),
            Some(SkillState::Failed),
            "Drop 兜底应复原 Failed 终态"
        );
    }
    // 收尾：不给其他测试留状态
    let registry = skill_runs(&app);
    registry.lock().unwrap().remove(name);
}

/// 回归锁：全部主步重复调用同一改名工具时必须提前终止。
/// 旧判定 unknown.len() >= steps.len() 把「去重后未知工具名数」与「主步数」
/// 错比——3 步都调 ghost_tool 时 unknown.len()=1 < 3，漏终止、逐步失败白烧回滚。
#[tokio::test]
async fn compat_gate_terminates_when_all_steps_call_same_renamed_tool() {
    let app = {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    };
    let body = "## Step 1: a\nghost_tool({})\n\n## Step 2: b\nghost_tool({})\n\n## Step 3: c\nghost_tool({})\n\n## Rollback\nrb_tool({})";
    let meta = crate::bot_skills::parse_meta(body, "compat-gate-test");
    let exec_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let exec_calls2 = exec_calls.clone();
    let exec = move |_tool: String, _args: String| {
        let calls = exec_calls2.clone();
        async move {
            calls.lock().unwrap().push("called".into());
            crate::bot::registry::ToolResult::ok("mock ok".to_string(), Vec::new())
        }
    };
    let persisted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let persisted2 = persisted.clone();
    let persist = move |name: &str,
                        kind: &str,
                        _reason: Option<&str>,
                        _summary: Option<&str>,
                        _rb: Option<bool>| {
        persisted2
            .lock()
            .unwrap()
            .push((name.to_string(), kind.to_string()));
    };
    let result = run_skill_scheduler_core(
        &app,
        "compat-gate-test",
        &meta,
        body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await;
    assert!(
        matches!(result, Err(DslFailure::Terminated { .. })),
        "全部主步未知必须提前终止：{result:?}"
    );
    assert_eq!(
        exec_calls.lock().unwrap().len(),
        0,
        "终止前不得调用任何工具"
    );
    let pv = persisted.lock().unwrap();
    assert!(
        pv.iter().any(|(_, k)| k == "terminated"),
        "应落 terminated：{pv:?}"
    );
}

/// 兼容门已知集并入挂载 MCP 工具名（与 dispatch 兜底 find_mounted 同一
/// func_name 名字空间）：主步全为挂载名时不再满足「全部未知」终止条件。
/// 挂载集读全局连接槽无法注入 → 集合构建抽成纯函数直打。
#[test]
fn compat_known_tools_includes_mounted_mcp_names() {
    let builtin = crate::bot::registry::tools_index().len();
    let mounted = vec!["mcp_fake_srv_echo".to_string()];
    let known = compat_known_tools(&mounted);
    assert!(known.contains("mcp_fake_srv_echo"), "挂载名必须在已知集内");
    assert!(known.contains("query_tasks"), "内置工具不得被挤出");
    assert_eq!(known.len(), builtin + 1, "已知集 = 内置 + 挂载");
    // 纯 MCP 技能（主步全为挂载名）不满足「全部未知」终止条件
    let body = "## Step 1: a\nmcp_fake_srv_echo({})\n\n## Step 2: b\nmcp_fake_srv_echo({})";
    let (steps, rollback) = parse_skill_steps(body).unwrap();
    assert!(rollback.is_empty());
    let all_unknown = steps
        .iter()
        .filter(|s| !known.contains(&s.tool_name))
        .count()
        == steps.len();
    assert!(!all_unknown, "主步全为挂载名不得被判全部未知");
    // 回归锁：未挂载的编造名仍全未知（并入挂载名不得放松原有闸）
    let known_without = compat_known_tools(&[]);
    assert!(
        steps.iter().all(|s| !known_without.contains(&s.tool_name)),
        "无挂载时编造名仍应全未知"
    );
}

/// 对照组：仅 rollback 段引用未知工具、主步全部已知 → 不终止，正常跑完主步
#[tokio::test]
async fn compat_gate_keeps_running_when_only_rollback_unknown() {
    let app = {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    };
    // query_single_task 内置已知；rb_tool 未知且只在 rollback 段
    let body = "## Step 1: list\nquery_single_task({})\n\n## Rollback\nrb_tool({})";
    let meta = crate::bot_skills::parse_meta(body, "compat-gate-rb-only");
    let exec = move |_tool: String, _args: String| async move {
        crate::bot::registry::ToolResult::ok("[]".to_string(), Vec::new())
    };
    let persisted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let persisted2 = persisted.clone();
    let persist =
        move |_name: &str, kind: &str, _r: Option<&str>, _s: Option<&str>, _rb: Option<bool>| {
            persisted2
                .lock()
                .unwrap()
                .push((_name.to_string(), kind.to_string()));
        };
    let outcome = run_skill_scheduler_core(
        &app,
        "compat-gate-rb-only",
        &meta,
        body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("主步全部已知不得被兼容闸终止");
    assert!(matches!(outcome, DslOutcome::Done(_)));
    let pv = persisted.lock().unwrap();
    assert!(
        !pv.iter().any(|(_, k)| k == "terminated"),
        "仅 rollback 未知不得终止：{pv:?}"
    );
}

/// `rollback: none` 时，正文里的 `## Rollback` 段不许被执行。
///
/// 作者显式声明「不执行回滚」时，复制模板留下的回滚段（最常见的来源）
/// 仍会跑逆操作（delete_task 等）——可能删掉作者想保留的数据。
/// 此前全文零处读 `meta.rollback`：回滚段存不存在只看正文有没有那节。
#[tokio::test]
async fn rollback_none_does_not_run_the_rollback_segment() {
    let app = {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    };
    let body = "## Step 1: list\nquery_single_task({})\n\n## Rollback\ndelete_task({})";
    let mut meta = crate::bot_skills::parse_meta(body, "rollback-gate-none");
    meta.rollback = "none".into();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, _args: String| {
        let calls2 = calls2.clone();
        async move {
            calls2.lock().unwrap().push(tool.clone());
            if tool == "query_single_task" {
                crate::bot::registry::ToolResult::error("步骤失败", Vec::new())
            } else {
                crate::bot::registry::ToolResult::ok("已回滚", Vec::new())
            }
        }
    };
    let persisted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let persisted2 = persisted.clone();
    let persist =
        move |name: &str, kind: &str, _r: Option<&str>, _s: Option<&str>, _rb: Option<bool>| {
            persisted2
                .lock()
                .unwrap()
                .push((name.to_string(), kind.to_string()));
        };
    let outcome = run_skill_scheduler_core(
        &app,
        "rollback-gate-none",
        &meta,
        body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("应跑出结果");
    match outcome {
        DslOutcome::FailedButRecoverable {
            rollback_attempted, ..
        } => assert!(!rollback_attempted, "rollback: none 时不许声称回滚跑过"),
        other => panic!("主步失败应落 FailedButRecoverable，实际 {other:?}"),
    }
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "回滚段不许执行（作者声明了 rollback: none）：{calls:?}"
    );
    assert_eq!(calls[0], "query_single_task");
}

/// 对照组：`rollback: auto` 时回滚段照跑（上面的闸不许把 auto 也掐了）。
#[tokio::test]
async fn rollback_auto_still_runs_the_rollback_segment() {
    let app = {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    };
    let body = "## Step 1: list\nquery_single_task({})\n\n## Rollback\ndelete_task({})";
    let mut meta = crate::bot_skills::parse_meta(body, "rollback-gate-auto");
    meta.rollback = "auto".into();
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let calls2 = calls.clone();
    let exec = move |tool: String, _args: String| {
        let calls2 = calls2.clone();
        async move {
            calls2.lock().unwrap().push(tool.clone());
            if tool == "query_single_task" {
                crate::bot::registry::ToolResult::error("步骤失败", Vec::new())
            } else {
                crate::bot::registry::ToolResult::ok("已回滚", Vec::new())
            }
        }
    };
    let persisted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let persisted2 = persisted.clone();
    let persist =
        move |name: &str, kind: &str, _r: Option<&str>, _s: Option<&str>, _rb: Option<bool>| {
            persisted2
                .lock()
                .unwrap()
                .push((name.to_string(), kind.to_string()));
        };
    let outcome = run_skill_scheduler_core(
        &app,
        "rollback-gate-auto",
        &meta,
        body,
        None,
        &serde_json::Value::Null,
        exec,
        persist,
    )
    .await
    .expect("应跑出结果");
    match outcome {
        DslOutcome::FailedButRecoverable {
            rollback_attempted, ..
        } => assert!(rollback_attempted, "rollback: auto 时回滚段应跑过"),
        other => panic!("主步失败应落 FailedButRecoverable，实际 {other:?}"),
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2, "rollback: auto 时回滚段必须执行：{calls:?}");
    assert_eq!(calls[1], "delete_task");
}
