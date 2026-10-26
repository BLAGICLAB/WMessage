use super::parse::parse_skill_steps;
use super::runtime::{advance_dsl, DslAdvanceAction};
use super::state::{
    active_skill_run_for, clear_terminal_skill_runs, load_skill_meta, skill_runs, SkillState,
};
use super::vars::{extract_task_id, substitute_vars, substitute_vars_with_params, CompletedStep};
use crate::db::now_ms;
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
    let _g = crate::db::lock::lock_or_recover(
        &crate::db::DB_WRITE_LOCK,
        "bot_skills::scheduler::skill_runs",
    );
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

// DSL Outcome / Failure + LLM 兜底

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

/// 跑 `## Rollback` 段（step 失败 / 状态机 FailWithRollback 两处共用）：
/// 回滚步骤同样走变量替换（失败前的步骤都已入 ctx）。
/// 返回值契约（SKILL_DSL.md §4.3.2）：true = 段存在且全部回滚步骤无失败——
/// 前端据此决定是否提示「已完成步骤未回滚，请人工核对」。
/// 回滚段执行时 run 已是 Failed，原子工具会被 AtomicGuard 拦截 → 临时重开为 Running，
/// 结束后复原（reopen/restore，见 state.rs）；逐步判定成败并记审计，
/// 吞掉结果会让回滚全挂也返回 true，护栏被架空。
/// 泛型 Runtime + execute_tool 注入（stop 分支选择由调用方闭包承接），调度器本体可被集成测试直驱。
/// 回滚复原守卫：回滚段 execute_tool panic / 提前 return 时 Drop 兜底复原
/// Failed 终态——否则 run 永久卡 Running，AtomicGuard 放行窗口被永久重开。
/// 正常路径仍走原位置手动 restore（审计行顺序不变），随后 disarm；
/// 双重 restore 安全（restore 对非 Running 是 no-op）。
struct RollbackRestoreGuard<'a, R: tauri::Runtime> {
    app: &'a tauri::AppHandle<R>,
    name: &'a str,
    session_id: Option<&'a str>,
    armed: bool,
}

impl<R: tauri::Runtime> Drop for RollbackRestoreGuard<'_, R> {
    fn drop(&mut self) {
        if self.armed {
            super::state::restore_failed_run_after_rollback(self.app, self.name, self.session_id);
        }
    }
}

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
    XP: std::future::Future<Output = crate::bot::registry::ToolResult>,
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
    let reopened = super::state::reopen_failed_run_for_rollback(app, name, session_id);
    let mut restore_guard = RollbackRestoreGuard {
        app,
        name,
        session_id,
        armed: reopened,
    };
    let mut failed_steps = 0usize;
    for rb in rollback {
        let rb_args = substitute_vars(&rb.args_json, ctx);
        let tool_outcome = execute_tool(rb.tool_name.clone(), rb_args).await;
        let text = tool_outcome.text;
        if !tool_outcome.status.is_success() {
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
        super::state::restore_failed_run_after_rollback(app, name, session_id);
    }
    restore_guard.armed = false;
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
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::scheduler::skill_runs");
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
    let (meta, body, _) = load_skill_meta(app, name).map_err(|e| DslFailure::Terminated {
        reason: e.to_string(),
    })?;
    // params 从本会话活动 run 取（use_skill 启动时写入；pre-step 路由路径无 run
    // → Null = 无参数替换源）。步骤工具白名单读的是 meta.allowed_tools
    //（SKILL.md 声明），在 run_skill_scheduler_core 内逐步骤校验。
    let params = active_skill_run_for(app, session_id)
        .filter(|r| r.name == name)
        .map(|r| r.params)
        .unwrap_or(serde_json::Value::Null);
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
        &params,
        execute_tool,
        persist_outcome,
    )
    .await
}

/// 兼容门已知工具全集（纯函数，单测直打）：内置注册表 + 当前挂载 MCP 工具函数名。
/// dispatch 对查表 miss 有 MCP 兜底路由（find_mounted 按 func_name 反查同名），
/// 把挂载名并入后，主步全为 MCP 工具的纯外部技能不会被误判「全部未知」而终止。
fn compat_known_tools(mounted_func_names: &[String]) -> std::collections::HashSet<String> {
    let mut known: std::collections::HashSet<String> = crate::bot::registry::tools_index()
        .keys()
        .map(|s| s.to_string())
        .collect();
    known.extend(mounted_func_names.iter().cloned());
    known
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
#[allow(clippy::too_many_lines)]
pub async fn run_skill_scheduler_core<R: tauri::Runtime, X, XP, P>(
    app: &tauri::AppHandle<R>,
    name: &str,
    meta: &super::parse::SkillMeta,
    body: &str,
    session_id: Option<&str>,
    params: &serde_json::Value,
    execute_tool: X,
    persist_outcome: P,
) -> Result<DslOutcome, DslFailure>
where
    X: Fn(String, String) -> XP,
    XP: std::future::Future<Output = crate::bot::registry::ToolResult>,
    P: Fn(&str, &str, Option<&str>, Option<&str>, Option<bool>),
{
    // 僵尸终态清理：上轮遗留的 Completed/Failed/Terminated run 会在第 0 步被 advance_dsl
    // 误判为完成信号直接 break（与主循环同款假死根因）；只清本会话（跨会话隔离）
    clear_terminal_skill_runs(app, session_id);
    let (steps, rollback) = match parse_skill_steps(body) {
        Ok(r) => r,
        Err(e) => {
            // 零步骤出口也必须收尾 run（见 mark_session_run_terminated），
            // 否则 run 泄漏为僵尸 Running，被下一轮的无关确认窗误杀
            super::state::mark_session_run_terminated(app, name, session_id, &e);
            return Err(DslFailure::Terminated { reason: e });
        }
    };
    // 回滚段**只在 `rollback: auto` 时才执行**。
    //
    // 作者显式声明「不执行回滚」时，正文里留着 `## Rollback` 段（复制模板时
    // 最常见）也不能跑——那会执行作者明确声明不要执行的逆操作（delete_task
    // 等），可能删掉作者想保留的数据。此前全文零处读 `meta.rollback`：
    // 回滚段存不存在只看正文有没有那节。
    let rollback_slice: &[super::parse::SkillStep] = if meta.rollback == "auto" {
        &rollback
    } else {
        &[]
    };
    if steps.is_empty() {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_dsl_empty | name: {name} | body_len: {}", body.len()),
        );
        persist_outcome(name, "terminated", Some("DSL 解析为空"), None, None);
        let reason = format!("技能「{name}」无可执行步骤（DSL 解析为空）");
        super::state::mark_session_run_terminated(app, name, session_id, &reason);
        return Err(DslFailure::Terminated { reason });
    }
    // 兼容审计——步骤/回滚引用了未内置的工具。全部未知 → 提前终止
    // （几乎必然是工具改名后未同步的旧技能，逐步失败只会白烧回滚）；部分未知 →
    // 警告继续（可能混用 MCP 工具，dispatch 兜底可执行）。
    let mounted: Vec<String> = crate::bot::mcp::mount::mcp_mounted_tools()
        .into_iter()
        .map(|t| t.func_name)
        .collect();
    let known = compat_known_tools(&mounted);
    let unknown = super::parse::unknown_tool_names(&steps, &rollback, &known);
    if !unknown.is_empty() {
        crate::bot::audit_log_hook(
            app,
            &format!(
                "skill.compat_warn | name: {name} | unknown: {} | steps: {}",
                unknown.join(","),
                steps.len()
            ),
        );
        // 终止判定按主步逐条计数：unknown 是 steps+rollback 去重后的工具名清单，
        // 与步数错比会漏终止（主步重复调用同一改名工具时 unknown.len() 恒小于步数）；
        // 仅 rollback 段引用未知工具不会触发（主步未知计数不含）
        if steps
            .iter()
            .filter(|s| !known.contains(&s.tool_name))
            .count()
            == steps.len()
        {
            let reason = format!(
                "技能「{name}」的所有步骤都引用了已不存在的工具（{}）——工具已更名或技能过旧，请更新 SKILL.md",
                unknown.join("、")
            );
            persist_outcome(name, "terminated", Some(&reason), None, None);
            super::state::mark_session_run_terminated(app, name, session_id, &reason);
            return Err(DslFailure::Terminated { reason });
        }
    }
    crate::bot::audit_log_hook(
        app,
        &format!("skill_dsl_start | name: {name} | steps: {}", steps.len()),
    );

    let mut ctx: Vec<CompletedStep> = Vec::new();
    let mut results: Vec<(usize, String, String)> = Vec::new();
    // 外部完成信号早退标记：Finish = 外部已置 run Completed，break 是承认外部终态非跑完
    let mut finish_signal = false;
    for step in &steps {
        // 每 step 前查 advance_dsl 状态机，拦截漏停场景：
        // step_check 步数熔断 / skill_on_step_post 工具失败 /
        // skill_terminate_all 用户 /stop / start_skill 切技能 → SkillRun.state 已被改，
        // 调度器必须感知。
        if let Some(run) = active_skill_run_for(app, session_id) {
            // 别的技能接管了本会话（用户 use_skill 切换）→ 本调度器收手：
            // 不推进接管方的状态、不跑剩余步骤、不记 done、不调 skill_finish
            //（那会把接管方标成完成，等于把新技能掐死）。
            if run.name != name {
                let reason = format!("技能「{name}」已被技能「{}」接管，剩余步骤未执行", run.name);
                crate::bot::audit_log_hook(
                    app,
                    &format!(
                        "skill_dsl_run_replaced | name: {name} | active: {}",
                        run.name
                    ),
                );
                persist_outcome(name, "terminated", Some(&reason), None, None);
                return Err(DslFailure::Terminated { reason });
            }
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
                    finish_signal = true;
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
                        rollback_slice,
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
        // 变量替换：把上一步结果/UUID 拼进 args_json（params 静态参数一并替换）
        let resolved_args = substitute_vars_with_params(&step.args_json, &ctx, params);
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
        // allowed-tools 双闸——技能声明了允许工具集合时，步骤工具必须 ∈ 集合
        //（作者主动限权；与系统兜底「tool ∈ 注册表」正交）。违规 = 步骤失败，
        // 走既有回滚 + FailedButRecoverable 管道（LLM 兜底接管）。
        if !meta.allowed_tools.is_empty() && !meta.allowed_tools.contains(&step.tool_name) {
            let text = format!(
                "失败：工具 {} 不在本技能 allowed-tools 声明内（允许：{}）。请修正 SKILL.md 或改由人工执行该步骤",
                step.tool_name,
                meta.allowed_tools.join(", ")
            );
            let rb_attempted = run_rollback_segment_core(
                app,
                name,
                rollback_slice,
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
            crate::bot::audit_log_hook(
                app,
                &format!(
                    "skill.allowed_tools_violation | name: {name} | step: {} | tool: {}",
                    step.index,
                    crate::bot::truncate_for_log(&step.tool_name, 60)
                ),
            );
            return Ok(DslOutcome::FailedButRecoverable {
                reason: final_reason,
                completed_summary: summary,
                rollback_attempted: rb_attempted,
            });
        }
        let tool_outcome = execute_tool(step.tool_name.clone(), resolved_args).await;
        let text = tool_outcome.text;
        let failed = !tool_outcome.status.is_success();
        if failed {
            let rb_attempted = run_rollback_segment_core(
                app,
                name,
                rollback_slice,
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
            // 失败教训直沉淀（有已执行动作才记，防空转噪音）
            if !ctx.is_empty() {
                super::runtime::sink_skill_failure_lesson(
                    app,
                    name,
                    &final_reason,
                    &ctx.iter()
                        .map(|c| format!("Step {}: {}", c.index, c.title))
                        .collect::<Vec<_>>(),
                );
            }
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
        results.len()
    );
    if finish_signal {
        summary.push_str(&format!(
            "\n（外部完成信号介入：计划共 {} 步，实际执行 {} 步，剩余步骤跳过）\n",
            steps.len(),
            results.len()
        ));
    }
    for (idx, title, result) in &results {
        summary.push_str(&format!("\n### Step {}: {}\n{}\n", idx, title, result));
    }
    // Done 路径收尾状态机（Running → Completed）：成功路径必须调 skill_finish，
    // 否则 run 泄漏为「僵尸 Running」——原子工具闸门在技能结束后仍对本会话放行，
    // 且后续消息的工具调用被计入僵尸 run 直至步数熔断卡死会话。
    // finish_signal 路径跳过：run 已被外部置 Completed（无僵尸泄漏），再调 = 误记 no_transition。
    if !finish_signal {
        let _ = super::runtime::skill_finish(app, true, "done", session_id);
    }
    let steps_ok = results.len();
    crate::bot::audit_log_hook(
        app,
        &format!("skill_dsl_done | name: {name} | steps_ok: {steps_ok}"),
    );
    persist_outcome(name, "done", None, Some(&summary), None);
    Ok(DslOutcome::Done(summary))
}

#[cfg(test)]
mod tests;
