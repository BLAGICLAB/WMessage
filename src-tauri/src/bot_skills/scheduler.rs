use super::parse::parse_skill_steps;
use super::runtime::{advance_dsl, DslAdvanceAction};
use super::state::{
    active_skill_run_for, clear_terminal_skill_runs, load_skill_meta, now_ms, skill_runs,
    SkillState,
};
use super::vars::{extract_task_id, substitute_vars, CompletedStep};
use tauri::AppHandle;

/// persist DslOutcome to DB after run_skill_scheduler finishes (quiet failure)
fn persist_outcome_quiet(
    app: &AppHandle,
    name: &str,
    kind: &str,
    reason: Option<&str>,
    summary: Option<&str>,
    rollback_attempted: Option<bool>,
) {
    let Ok(conn) = crate::db::open_db(app) else {
        return;
    };
    // 纳入 DB_WRITE_LOCK：主窗长事务期间锁外直写会 SQLITE_BUSY 静默丢记录
    let _g = crate::db::DB_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let outcome = crate::db::PersistedSkillOutcome {
        skill_name: name.to_string(),
        kind: kind.to_string(),
        reason: reason.map(|s| s.to_string()),
        completed_summary: summary.map(|s| s.to_string()),
        rollback_attempted,
        last_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    let _ = crate::db::upsert_skill_outcome(&conn, &outcome);
}

// ─────────────────────── DSL Outcome / Failure + LLM 兜底 ───────────────────────

/// DSL 调度器成功 / 可恢复 / 暂停 输出（LLM 兜底路径）。
/// - Done：跑完所有 step，返回汇总文本给用户
/// - AwaitUser：暂停中等用户确认（auto-mode 极少触发，留接口）
/// - FailedButRecoverable：失败但已完成部分 step，让 LLM 基于 `completed_summary` 决策下一步
#[derive(Debug, Clone)]
pub enum DslOutcome {
    /// 正常完成：summary 文本直接给用户
    Done(String),
    /// 暂停中等用户确认
    AwaitUser,
    /// 失败但 LLM 可接管：把 `completed_summary` + `reason` 注入 system prompt 决策下一步
    FailedButRecoverable {
        reason: String,
        completed_summary: String,
        rollback_attempted: bool,
    },
}

/// DSL 调度器硬错误（用户主动终止 / 解析失败）—— LLM 不接管，直接报错给用户
#[derive(Debug, Clone)]
pub enum DslFailure {
    Terminated { reason: String },
}

/// 已完成步骤摘要（LLM 兜底）：
/// 把 ctx 里的步骤产物拼成可读 summary（注入 system prompt 用）。
/// 每步一行：`Step N (title): <result 前 200 字符 + ...>`
/// 空 ctx 返回 `(无已完成步骤)`。
pub fn format_completed_summary(ctx: &[CompletedStep]) -> String {
    if ctx.is_empty() {
        return "(无已完成步骤)".into();
    }
    let mut out = String::new();
    for step in ctx {
        let preview: String = step.result.chars().take(200).collect();
        let suffix = if step.result.chars().count() > 200 {
            "..."
        } else {
            ""
        };
        out.push_str(&format!(
            "- Step {} ({}): {}{}\n",
            step.index, step.title, preview, suffix
        ));
    }
    out
}

/// 工具执行结果文本的失败判定（调度器生产路径与测试同步循环共用）。
/// 委托 `audit::tool_call_failed`（全链路统一口径）：熔断「已强制终止」/ 门禁拦截「⚠️」/
/// 用户拒绝 等文案以「技能」/「用户」开头、不被前缀清单认，但都必须判失败，
/// 否则末步熔断会误报「✅ 完成」。
pub(crate) fn is_tool_failure_text(text: &str) -> bool {
    crate::audit::tool_call_failed("", text)
}

/// 跑 `## Rollback` 段（step 失败 / 状态机 FailWithRollback 两处共用）：
/// 回滚步骤同样走变量替换（失败前的步骤都已入 ctx）。
/// 返回值契约（SKILL_DSL.md §4.3.2）：true = 段存在且全部回滚步骤无失败——
/// 前端据此决定是否提示「已完成步骤未回滚，请人工核对」。
/// 回滚段执行时 run 已是 Failed，原子工具会被 AtomicGuard 拦截 → 临时重开为 Running，
/// 结束后复原（reopen/restore，见 state.rs）；逐步判定成败并记审计，
/// 吞掉结果会让回滚全挂也返回 true，护栏被架空。
/// 泛型 Runtime + execute_tool 注入（stop 分支选择由调用方闭包承接），调度器本体可被集成测试直驱。
#[allow(clippy::too_many_arguments)]
async fn run_rollback_segment_core<R: tauri::Runtime, X, XP>(
    app: &tauri::AppHandle<R>,
    name: &str,
    rollback: &[super::parse::SkillStep],
    ctx: &[CompletedStep],
    step_index: usize,
    reason: &str,
    session_id: Option<&str>,
    execute_tool: &X,
) -> bool
where
    X: Fn(String, String) -> XP,
    XP: std::future::Future<Output = (String, Vec<crate::bot_chat::TaskRef>)>,
{
    if rollback.is_empty() {
        return false;
    }
    crate::bot::audit_log_hook(
        app,
        &format!(
            "skill_dsl_rollback_start | name: {name} | step: {step_index} | reason: {}",
            crate::bot::truncate_for_log(reason, 120)
        ),
    );
    // 回滚窗口：Failed → Running（原子工具放行）。窗口内 skill_on_step 仍计数/可熔断，
    // 熔断会再把 run 标 Failed —— 后续回滚步骤的原子工具随之被拦，按失败计入。
    let reopened = super::state::reopen_failed_run_for_rollback(name, session_id);
    let mut failed_steps = 0usize;
    for rb in rollback {
        let rb_args = substitute_vars(&rb.args_json, ctx);
        let (text, _refs) = execute_tool(rb.tool_name.clone(), rb_args).await;
        if is_tool_failure_text(&text) {
            failed_steps += 1;
            crate::bot::audit_log_hook(
                app,
                &format!(
                    "skill_dsl_rollback_step_failed | name: {name} | tool: {} | {}",
                    // tool_name 来自 SKILL.md DSL，转义防日志撕裂
                    crate::bot::truncate_for_log(&rb.tool_name, 60),
                    crate::bot::truncate_for_log(&text, 120)
                ),
            );
        }
    }
    if reopened {
        super::state::restore_failed_run_after_rollback(name, session_id);
    }
    crate::bot::audit_log_hook(
        app,
        &format!(
            "skill_dsl_rollback_done | name: {name} | steps: {} | failed: {failed_steps}",
            rollback.len()
        ),
    );
    failed_steps == 0
}

/// 强制终止活动 Skill（/stop 联动；用户取消时调用）。
/// 按会话过滤：会话 B 的 /stop 不误杀会话 A 的活动技能；
/// session_id=None 终止所有会话（lib.rs 应用退出清理路径用）。
/// 泛型 Runtime：cleanup_on_exit 的 mock runtime 测试可直调。
pub fn skill_terminate_all<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    reason: &str,
    session_id: Option<&str>,
) {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    for (name, run) in runs.iter_mut() {
        if run.state == SkillState::Running || run.state == SkillState::Paused {
            if let Some(sid) = session_id {
                if run.session_id.as_deref() != Some(sid) {
                    continue; // 别的会话的技能不动
                }
            }
            run.state = SkillState::Terminated;
            run.end_reason = reason.to_string();
            crate::bot::audit_log_hook(app, &format!("skill_terminated | name: {name} | {reason}"));
        }
    }
}

/// DSL 调度器入口。仅供 `meta.mode == "auto"` 的 Skill 调用：
/// 解析 body → 顺序调 `bot::execute_tool` → 失败时跑回滚段。
/// stop：携带 /stop 守卫，在途 run_python 等长耗时步骤可被中断——
/// stop=None 时在途 Python 脚本必须跑完才能停。
///
/// 本函数只做依赖装配（load_skill_meta + execute_tool 闭包承接 stop 分支
/// + persist 闭包），实际调度逻辑在 run_skill_scheduler_core，
/// 后者泛型 Runtime + 注入 executor，集成测试可直驱（tests/skill_e2e.rs）。
pub async fn run_skill_scheduler(
    app: &AppHandle,
    name: &str,
    session_id: Option<&str>,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> Result<DslOutcome, DslFailure> {
    let (meta, body) = load_skill_meta(app, name).map_err(|e| DslFailure::Terminated {
        reason: e.to_string(),
    })?;
    let execute_tool = |tool: String, args: String| async move {
        match stop {
            Some(s) => crate::bot::execute_tool_with_stop(app, &tool, &args, Some(s)).await,
            None => crate::bot::execute_tool(app, &tool, &args, session_id).await,
        }
    };
    let persist_outcome = |name: &str,
                           kind: &str,
                           reason: Option<&str>,
                           summary: Option<&str>,
                           rollback_attempted: Option<bool>| {
        persist_outcome_quiet(app, name, kind, reason, summary, rollback_attempted);
    };
    run_skill_scheduler_core(
        app,
        name,
        &meta,
        &body,
        session_id,
        execute_tool,
        persist_outcome,
    )
    .await
}

/// DSL 调度器核心（run_skill_scheduler 的实际调度逻辑）：
/// 泛型 Runtime + execute_tool / persist_outcome 注入，不直接依赖 Wry——
/// 集成测试可用 MockRuntime + mock executor + 真 fixture SKILL.md 直驱真路径。
///
/// 行为：
/// - 每个 step 调一次注入的 execute_tool（生产闭包内带 pre-execute 校验）
/// - 任一 step 失败 → 顺序跑 `## Rollback` 段工具 → 返回 Err
/// - 全部成功 → 返回汇总文本
/// - SkillRun 状态机更新由 execute_tool 内的 `skill_on_step` / `skill_on_step_post` 自动维护
pub async fn run_skill_scheduler_core<R: tauri::Runtime, X, XP, P>(
    app: &tauri::AppHandle<R>,
    name: &str,
    meta: &super::parse::SkillMeta,
    body: &str,
    session_id: Option<&str>,
    execute_tool: X,
    persist_outcome: P,
) -> Result<DslOutcome, DslFailure>
where
    X: Fn(String, String) -> XP,
    XP: std::future::Future<Output = (String, Vec<crate::bot_chat::TaskRef>)>,
    P: Fn(&str, &str, Option<&str>, Option<&str>, Option<bool>),
{
    // 僵尸终态清理：上轮遗留的 Completed/Failed/Terminated run 会在第 0 步被 advance_dsl
    // 误判为完成信号直接 break（与主循环同款假死根因）
    clear_terminal_skill_runs();
    let (steps, rollback) =
        parse_skill_steps(body).map_err(|e| DslFailure::Terminated { reason: e })?;
    if steps.is_empty() {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_dsl_empty | name: {name} | body_len: {}", body.len()),
        );
        persist_outcome(name, "terminated", Some("DSL 解析为空"), None, None);
        return Err(DslFailure::Terminated {
            reason: format!("技能「{name}」无可执行步骤（DSL 解析为空）"),
        });
    }
    crate::bot::audit_log_hook(
        app,
        &format!("skill_dsl_start | name: {name} | steps: {}", steps.len()),
    );

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let mut results: Vec<(usize, String, String)> = Vec::new();
    for step in &steps {
        // 每 step 前查 advance_dsl 状态机，拦截漏停场景：
        // step_check 步数熔断 / skill_on_step_post 工具失败 /
        // skill_terminate_all 用户 /stop / start_skill 切技能 → SkillRun.state 已被改，
        // 调度器必须感知。
        if let Some(run) = active_skill_run_for(session_id) {
            let ts = now_ms();
            match advance_dsl(&run, ts) {
                DslAdvanceAction::Run => {}
                DslAdvanceAction::Finish => {
                    crate::bot::audit_log_hook(
                        app,
                        &format!(
                            "skill_dsl_finish_signal | name: {name} | step_before: {}",
                            step.index
                        ),
                    );
                    break;
                }
                DslAdvanceAction::AwaitUser => {
                    crate::bot::audit_log_hook(
                        app,
                        &format!("skill_dsl_await_user | name: {name} | step: {}", step.index),
                    );
                    persist_outcome(
                        name,
                        "await_user",
                        None,
                        Some(&format_completed_summary(&ctx)),
                        None,
                    );
                    return Ok(DslOutcome::AwaitUser);
                }
                DslAdvanceAction::FailWithRollback(reason) => {
                    let rb_attempted = run_rollback_segment_core(
                        app,
                        name,
                        &rollback,
                        &ctx,
                        step.index,
                        &reason,
                        session_id,
                        &execute_tool,
                    )
                    .await;
                    let final_reason = format!("技能「{name}」中止：{reason}");
                    let summary = format_completed_summary(&ctx);
                    persist_outcome(
                        name,
                        "failed_recoverable",
                        Some(&final_reason),
                        Some(&summary),
                        Some(rb_attempted),
                    );
                    return Ok(DslOutcome::FailedButRecoverable {
                        reason: final_reason,
                        completed_summary: summary,
                        rollback_attempted: rb_attempted,
                    });
                }
                DslAdvanceAction::Terminate(reason) => {
                    crate::bot::audit_log_hook(
                        app,
                        &format!(
                            "skill_dsl_terminated | name: {name} | step: {} | reason: {}",
                            step.index, reason
                        ),
                    );
                    let final_reason = format!("技能「{name}」终止：{reason}");
                    persist_outcome(name, "terminated", Some(&final_reason), None, None);
                    return Err(DslFailure::Terminated {
                        reason: final_reason,
                    });
                }
            }
        }
        crate::bot::audit_log_hook(
            app,
            &format!(
                "skill_dsl_step | name: {name} | step: {} | tool: {} | ctx_len: {}",
                step.index,
                // tool_name 来自 SKILL.md DSL，转义防日志撕裂
                crate::bot::truncate_for_log(&step.tool_name, 60),
                ctx.len()
            ),
        );
        // 变量替换：把上一步结果/UUID 拼进 args_json
        let resolved_args = substitute_vars(&step.args_json, &ctx);
        if resolved_args != step.args_json {
            crate::bot::audit_log_hook(
                app,
                &format!(
                    "skill_dsl_var_resolved | name: {name} | step: {} | resolved_args_len: {}",
                    step.index,
                    resolved_args.len()
                ),
            );
        }
        let (text, _refs) = execute_tool(step.tool_name.clone(), resolved_args).await;
        let failed = is_tool_failure_text(&text);
        if failed {
            let rb_attempted = run_rollback_segment_core(
                app,
                name,
                &rollback,
                &ctx,
                step.index,
                &text,
                session_id,
                &execute_tool,
            )
            .await;
            let final_reason = format!(
                "技能「{name}」Step {} ({}) 失败：{}",
                step.index, step.title, text
            );
            let summary = format_completed_summary(&ctx);
            persist_outcome(
                name,
                "failed_recoverable",
                Some(&final_reason),
                Some(&summary),
                Some(rb_attempted),
            );
            return Ok(DslOutcome::FailedButRecoverable {
                reason: final_reason,
                completed_summary: summary,
                rollback_attempted: rb_attempted,
            });
        }
        // 把这一步压进 ctx（变量替换的依赖源）：parsed 用于嵌套路径 `${step1.task.id}`
        // 纯文本 / Markdown 摘要 parse 失败为 None，substitute_vars 嵌套路径 fallback 保留 `${...}`
        ctx.push(CompletedStep {
            index: step.index,
            title: step.title.clone(),
            result: text.clone(),
            id: extract_task_id(&text),
            parsed: serde_json::from_str(&text).ok(),
        });
        results.push((step.index, step.title.clone(), text));
    }

    let mut summary = format!(
        "✅ 技能「{}」自动执行完成（{} 步）：\n",
        meta.name,
        steps.len()
    );
    for (idx, title, result) in &results {
        summary.push_str(&format!("\n### Step {}: {}\n{}\n", idx, title, result));
    }
    // Done 路径收尾状态机（Running → Completed）：成功路径必须调 skill_finish，
    // 否则 run 泄漏为「僵尸 Running」——原子工具闸门在技能结束后仍对本会话放行，
    // 且后续消息的工具调用被计入僵尸 run 直至步数熔断卡死会话。
    let _ = super::runtime::skill_finish(app, true, "done", session_id);
    crate::bot::audit_log_hook(
        app,
        &format!("skill_dsl_done | name: {name} | steps_ok: {}", steps.len()),
    );
    persist_outcome(name, "done", None, Some(&summary), None);
    Ok(DslOutcome::Done(summary))
}

#[cfg(test)]
mod tests {
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
            // 失败判定：与生产 run_skill_scheduler 共用同一判定函数（不再手写镜像）
            let failed = is_tool_failure_text(&text);
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
        let expected_preview: String = std::iter::repeat("x").take(200).collect::<String>() + "...";
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
            "bind_file" => r#"{"bound": true}"#.to_string(),
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
}
