//! 工具调度核心（execute_tool dispatch + early_return_events）。
//!
//! 原 bot.rs 行 1195–1392 全部调度代码 +
//! 行 3140–3193 (early_return_events_tests) 测试段搬入。
//!
//! 调度主表（execute_tool_impl via TOOLS_TABLE lookup）分发内置工具（数目见 registry::BUILTIN_TOOL_COUNT）到 bot::tools。
//! TOOLS_TABLE / tools_json() / mutating_tools() 都在 bot::registry（阶段 2 单源真相）。

use tauri::{AppHandle, Emitter};

use crate::bot::hooks::{PreToolVerdict, ToolCallView};
use crate::bot::registry::{tools_index, ToolCtx};
use crate::bot::tools::{broadcast_after_mutation, files_audit_kv};

// 工具调度核心（execute_tool dispatch）

/// 工具调用审计上下文（模型循环 → dispatch）：一次调用的「可整轮回放」标识。
///
/// - `turn`：第几轮模型循环（0 起）——同一轮的多个 tool_call 归到一组，出问题整轮复现
/// - `tool_call_id`：LLM 签发的 tool_call id（回填 tool 消息用的那个）。服务端日志、
///   前端 `bot-tool` / `bot-tool-done` 事件同 id，按 id 即可对齐一条调用全链路
///
/// `session_id` 不放进本结构：dispatch 本来就从 stop / 形参拿到（见 `execute_tool_impl`），
/// 两处来源会漂移。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallTrace {
    pub turn: Option<usize>,
    pub tool_call_id: Option<String>,
}

/// 审计 kv：trace 里有值的字段才写。
/// scheduler 等无模型循环上下文的调用方不传 trace——不产生误导性的 `turn=0`；
/// `session_id` 恒写（没有就是 `-`），便于按会话 grep。
fn trace_kv(trace: &ToolCallTrace, session_id: Option<&str>) -> Vec<(&'static str, String)> {
    let mut kv: Vec<(&'static str, String)> = vec![(
        "session_id",
        session_id
            .filter(|s| !s.is_empty())
            .unwrap_or("-")
            .to_string(),
    )];
    if let Some(turn) = trace.turn {
        kv.push(("turn", turn.to_string()));
    }
    if let Some(id) = trace.tool_call_id.as_deref().filter(|s| !s.is_empty()) {
        kv.push(("tool_call_id", id.to_string()));
    }
    kv
}

/// 所有「已发出 `tool.call` 但工具没真跑」的早退分支共用的 `tool.return` kv。
///
/// 抽成单一构造器而不是三处各写一份字面量：`tool.call` / `tool.return` 是按
/// 会话整轮对齐的，键集漂一处就等于统计面板出现一条对不上的记录，而三份
/// 手抄的形状没有任何机制保证它们一致。键集一致性由此**由构造保证**，
/// 不再靠测试去比对三份副本。
fn tool_return_kv(
    name: &str,
    reason: &str,
    dur_ms: u64,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
) -> Vec<(&'static str, String)> {
    let mut kv: Vec<(&'static str, String)> = vec![
        ("tool", name.to_string()),
        ("reason", reason.to_string()),
        ("exit_code", "none".to_string()),
        ("duration_ms", dur_ms.to_string()),
    ];
    kv.extend(trace_kv(trace, session_id));
    kv
}

/// 早退路径（per-tool 规则闸拦截 / skill_on_step 熔断）的审计事件序列。
/// `tool.call` 已在入口发出，这里按写入顺序补齐后续事件并以 `tool.return` 配平，
/// 否则统计面板出现「悬挂调用」（call > return）。
/// 抽成纯函数：事件名 + kv + 顺序可单测；调用点只负责逐条 emit。
/// - 规则闸路径（err=None）：pre_execute.deny + tool.return(reason=denied_by_rule)
///   （主事件名沿用 pre-execute 时代的 pre_execute.deny，日志消费方口径不变）
/// - 熔断路径（err=Some）：skill_on_step_error（补 Warn 可见性）+ tool.return(reason=skill_step_failed)
/// 两条路径都带 trace（session_id / turn / tool_call_id），与正常路径同口径。
fn early_return_events(
    name: &str,
    reason: &str,
    dur_ms: u64,
    err: Option<&str>,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
) -> Vec<(
    crate::audit::AuditLevel,
    &'static str,
    Vec<(&'static str, String)>,
)> {
    let ctx = trace_kv(trace, session_id);
    let with_ctx = |mut kv: Vec<(&'static str, String)>| {
        kv.extend(ctx.iter().cloned());
        kv
    };
    let mut events = Vec::with_capacity(2);
    match err {
        Some(e) => events.push((
            crate::audit::AuditLevel::Warn,
            "skill_on_step_error",
            with_ctx(vec![("tool", name.to_string()), ("err", e.to_string())]),
        )),
        None => events.push((
            crate::audit::AuditLevel::Warn,
            "pre_execute.deny",
            with_ctx(vec![("tool", name.to_string())]),
        )),
    };
    events.push((
        crate::audit::AuditLevel::Warn,
        "tool.return",
        tool_return_kv(name, reason, dur_ms, session_id, trace),
    ));
    events
}

/// 进程内执行工具，返回 (给模型的文本结果, 涉及的任务引用)
/// `pub` 让 `bot_skills::run_skill_scheduler`（DSL 调度器）可调用，不暴露给前端。
/// 调用侧防护：per-tool 规则表闸 + subagent 白名单兜底 + skill 步骤钩子（本函数内）。
/// 会话隔离：session_id 随调用链透传（DSL 调度器从 bot_chat 带下来），
/// 无 StopGuard 时按交互执行处理（DSL 调度器只在聊天上下文里跑）。
pub async fn execute_tool(
    app: &AppHandle,
    name: &str,
    args: &str,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    execute_tool_impl(
        app,
        name,
        args,
        None,
        true,
        session_id,
        &ToolCallTrace::default(),
    )
    .await
}

/// execute_tool 的可停止版本：携带 /stop 守卫，run_python 等长耗时工具
/// 在执行中即可被中断；Skill 调度器等无守卫调用方走 execute_tool（stop=None）。
pub async fn execute_tool_with_stop(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> crate::bot::registry::ToolResult {
    traced_impl(app, name, args, stop, &ToolCallTrace::default()).await
}

/// 带模型循环上下文的版本：turn / tool_call_id 进 `tool.call` / `tool.return` 审计，
/// 出问题可按 (session_id, turn) 或 tool_call_id 整轮回放。
pub async fn execute_tool_traced(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
    trace: &ToolCallTrace,
) -> crate::bot::registry::ToolResult {
    traced_impl(app, name, args, stop, trace).await
}

/// execute_tool / execute_tool_with_stop / execute_tool_traced 的共同内核：
/// 交互属性与会话归属从 StopGuard 取（无守卫 = 后台调度器路径不会出现——
/// 调度器走 run_task_in_chat 也持 StopGuard；None 仅 DSL 调度器遗留路径）。
async fn traced_impl(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
    trace: &ToolCallTrace,
) -> crate::bot::registry::ToolResult {
    let interactive = stop.map(|s| s.is_interactive()).unwrap_or(true);
    let session_id = stop.and_then(|s| s.session_id());
    execute_tool_impl(app, name, args, stop, interactive, session_id, trace).await
}

/// 钩子 Interrupt 早退的审计事件序列（纯函数：事件名 + kv + 顺序可单测）。
/// 形状与 `early_return_events` 同构（真实原因事件 → `tool.return` 配平，
/// 后者带 `reason` / `exit_code=none` / `duration_ms` + trace），但原因事件名
/// 独立：日志消费方要能分清拒绝来自规则表还是来自钩子，混用 `pre_execute.deny`
/// 就把两种来源糊在一起了。
fn hook_interrupt_events(
    name: &str,
    reason: &str,
    dur_ms: u64,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
) -> Vec<(
    crate::audit::AuditLevel,
    &'static str,
    Vec<(&'static str, String)>,
)> {
    let mut cause_kv: Vec<(&'static str, String)> =
        vec![("tool", name.to_string()), ("reason", reason.to_string())];
    cause_kv.extend(trace_kv(trace, session_id));
    let ret_kv = tool_return_kv(name, "pre_tool_hook_interrupt", dur_ms, session_id, trace);
    vec![
        (
            crate::audit::AuditLevel::Warn,
            "pre_tool.hook_interrupt",
            cause_kv,
        ),
        (crate::audit::AuditLevel::Warn, "tool.return", ret_kv),
    ]
}

/// 钩子自身失败（返 Err / panic）的审计 kv（纯函数，形状可单测）。
/// `kind` 区分两条 fail-open 分支——「返错误」和「炸了」排查时不是一回事。
fn hook_failure_kv(
    hook: &str,
    kind: &str,
    err: &str,
    name: &str,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
) -> Vec<(&'static str, String)> {
    let mut kv: Vec<(&'static str, String)> = vec![
        ("hook", hook.to_string()),
        ("kind", kind.to_string()),
        ("err", err.to_string()),
        ("tool", name.to_string()),
    ];
    kv.extend(trace_kv(trace, session_id));
    kv
}

/// L4 pre_tool 钩子层（三道闸入口，调用点只有一个）：只读拦截——钩子只能放行或
/// 拒绝，禁止改写入参（改写能力在 L2/L3，见 hooks 模块头纪律 2）。
/// 两条容错口径：
/// - 钩子返 Err / panic → 记 `pre_tool.hook_failed` 后**放行**（fail-open），
///   钩子实现里的 bug 不能变成用户主对话中断；
/// - 钩子 Interrupt → 早退并用 `tool.return` 配平 `tool.call`，否则统计面板出现悬挂调用。
/// 层序由本函数在调用栈上的位置保证（钩子在规则闸 / skill 熔断 / 子 agent 白名单
/// / 入参 JSON 闸之前），注册表不提供任何配置层序的入口。
fn run_pre_tool_layer(
    app: &AppHandle,
    name: &str,
    args: &str,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
    start: &std::time::Instant,
) -> Option<crate::bot::registry::ToolResult> {
    let call = ToolCallView {
        tool: name,
        args,
        session_id,
        trace,
    };
    let run = crate::bot::hooks::registry().run_pre_tool(&call);
    for failure in &run.failures {
        let (kind, err) = match &failure.kind {
            crate::bot::hooks::HookFailureKind::Returned(e) => ("returned_err", e.as_str()),
            crate::bot::hooks::HookFailureKind::Panic(e) => ("panic", e.as_str()),
        };
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Warn,
            "pre_tool.hook_failed",
            &hook_failure_kv(failure.hook, kind, err, name, session_id, trace),
        );
    }
    match run.decision {
        PreToolVerdict::Continue => None,
        PreToolVerdict::Interrupt { reason } => {
            for (level, event, kv) in hook_interrupt_events(
                name,
                &reason,
                start.elapsed().as_millis() as u64,
                session_id,
                trace,
            ) {
                crate::audit::write_event(app, level, event, &kv);
            }
            // ⚠️ 前缀不是装饰：classify_tool_failure 的门禁臂按它判 gate_denied，
            // 让 error_class 能区分「被闸拦下」和「工具自己报错」两种失败。
            // 成败判定本身由 status=Denied 承担（见 ToolResult::denied）。
            Some(crate::bot::registry::ToolResult::denied(format!(
                "⚠️ {reason}"
            )))
        }
    }
}

///  per-tool 规则闸（execute_tool_impl 的 0.6 步，抽离保持入口薄壳——
/// 审计结构约束：skill_on_step 熔断检查紧跟规则闸之后、L4 钩子层之后）：
/// deny → 硬拒（配平 tool.return + tool_rule.hit 审计）；
/// ask → 强制确认（拒绝同样配平）；allow/无命中 → None 放行。
async fn tool_rule_gate(
    app: &AppHandle,
    name: &str,
    interactive: bool,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
    start: &std::time::Instant,
) -> Option<crate::bot::registry::ToolResult> {
    match crate::bot_fs::tool_rule_for(app, name) {
        Some(crate::bot_fs::ToolRuleAction::Deny) => {
            for (level, event, kv) in early_return_events(
                name,
                "denied_by_rule",
                start.elapsed().as_millis() as u64,
                None,
                session_id,
                trace,
            ) {
                crate::audit::write_event(app, level, event, &kv);
            }
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "tool_rule.hit",
                &[("tool", name.to_string()), ("action", "deny".to_string())],
            );
            // ⚠️ 前缀让 error_class 归到 gate_denied；成败由 status=Denied 承担。
            Some(crate::bot::registry::ToolResult::denied(format!(
                "⚠️ 工具「{name}」已被工具规则禁用（设置页授权卡可调整）。如任务确实需要，请向用户说明。"
            )))
        }
        Some(crate::bot_fs::ToolRuleAction::Ask) => {
            let approved = crate::bot_slash::ask_user_confirm(
                app,
                name,
                &format!("工具规则要求确认：{name}"),
                interactive,
                session_id,
            )
            .await;
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "tool_rule.hit",
                &[
                    ("tool", name.to_string()),
                    (
                        "action",
                        if approved {
                            "ask_approved"
                        } else {
                            "ask_denied"
                        }
                        .to_string(),
                    ),
                ],
            );
            if approved {
                None
            } else {
                for (level, event, kv) in early_return_events(
                    name,
                    "denied_by_rule",
                    start.elapsed().as_millis() as u64,
                    None,
                    session_id,
                    trace,
                ) {
                    crate::audit::write_event(app, level, event, &kv);
                }
                // 「用户拒绝」前缀让 error_class 归到 user_denied（比笼统的
                // gate_denied 信息量更大：这是人的决定，不是闸自动拦的）
                Some(crate::bot::registry::ToolResult::denied(format!(
                    "用户拒绝：工具「{name}」未获确认（后台无人应答）。"
                )))
            }
        }
        _ => None,
    }
}

/// 跑一个工具 future，panic 归一成普通失败。
///
/// 工具 panic → future unwind → 主聊天链路整轮崩掉：这一轮对话全丢
/// （无部分输出、无错误提示），前端停在「正在思考」。子 agent runner
/// 早有同样的隔离（`bot_orchestrator::run_subagent`），这里给主链路补齐——
/// **同一份工具代码，不该有两种健壮性**。hooks 扩展点三层都做了 fail-open，
/// 被扩展的本体反而没有。
///
/// panic 归一成 `Error` 而不是直接结束这一轮：模型能读到失败原因并换策略，
/// 这也正是幻觉守卫要的「成败凭证」——panic 不该被算成一次成功调用。
async fn run_tool_panic_isolated<R: tauri::Runtime, F>(
    app: &AppHandle<R>,
    name: &str,
    fut: F,
) -> crate::bot::registry::ToolResult
where
    F: std::future::Future<Output = crate::bot::registry::ToolResult>,
{
    use futures_util::FutureExt;
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(panic) => {
            let msg = crate::audit::panic_payload_msg(panic.as_ref());
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "tool.panic",
                &[
                    ("tool", name.to_string()),
                    ("panic", crate::bot::truncate_for_log(&msg, 300)),
                ],
            );
            crate::bot::registry::ToolResult::error(
                format!("工具执行异常，已中断该次调用：{msg}"),
                Vec::new(),
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
async fn execute_tool_impl(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
    interactive: bool,
    session_id: Option<&str>,
    trace: &ToolCallTrace,
) -> crate::bot::registry::ToolResult {
    let start = std::time::Instant::now();
    // 0. tool.call 结构化（含 session_id / turn / tool_call_id，供整轮回放）
    let mut call_kv: Vec<(&str, String)> = vec![
        ("tool", name.to_string()),
        ("args_preview", args.chars().take(80).collect::<String>()),
    ];
    call_kv.extend(trace_kv(trace, session_id));
    crate::audit::write_event(app, crate::audit::AuditLevel::Info, "tool.call", &call_kv);
    // 0.5  L4 pre_tool 钩子层（本期唯一接线点）：三道闸的入口——规则表、子 agent
    // 白名单、入参 JSON 闸都在它之后。只读拦截，钩子看不到可变入参；
    // 钩子出错 fail-open（只记审计），只有 Interrupt 才拒绝本次工具调用。
    if let Some(interrupted) = run_pre_tool_layer(app, name, args, session_id, trace, &start) {
        return interrupted;
    }
    // 0.6  per-tool 规则表（Agent 透明化设计 §9.2-1）：deny 硬拒 / ask 强制确认 /
    // allow 到文件工具侧生效（bot_fs 两个 resolve 内接线）。Some = 已早退（配平完成），
    // None = 放行走后续全局档。use_skill 豁免。函数体抽离保持 execute_tool_impl 入口薄壳。
    if name != "use_skill" {
        if let Some(denied) =
            tool_rule_gate(app, name, interactive, session_id, trace, &start).await
        {
            return denied;
        }
    }
    // 2. Skill 调度器步骤钩子：活动技能时计数/熔断/动作记录（use_skill 自身跳过）
    if name != "use_skill" {
        if let Err(e) = crate::bot_skills::skill_on_step(app, name, args, session_id) {
            // 补 Warn 可见性（skill_on_step_error）+ tool.return 配平（reason=skill_step_failed）
            for (level, event, kv) in early_return_events(
                name,
                "skill_step_failed",
                start.elapsed().as_millis() as u64,
                Some(&e.to_string()),
                session_id,
                trace,
            ) {
                crate::audit::write_event(app, level, event, &kv);
            }
            // 熔断/暂停 = 工具没被执行，是 Denied 不是 Warn。上游文案自带
            // 「技能已暂停」「已强制终止」，classify 能分别归到 skill_paused / fused。
            return crate::bot::registry::ToolResult::denied(e.to_string());
        }
    }
    // 1.6（置于 skill 钩子后）：子 agent 会话白名单闸（设计 §5.1/§10 服务端强制）。
    // schema 层已按 profile ∩ per-onion allowed 过滤（tools_json_for，递归双保险①），
    // 此处兜底：白名单外工具一律拒——含 spawn/check/cancel（递归禁用②的 dispatch 侧）与
    // 任务卡主状态写（主状态只读）。早退事件配平 tool.return。
    if let Some(ctx) = crate::tool_guard::subagent_ctx(session_id) {
        // 判据是 ctx.tools（交集后的全集）而非 profile 白名单：schema 层藏起来的
        // 工具模型仍可能硬猜名字调过来，只有闸也按交集拒，收窄才是真的。
        if !ctx.tools.contains(&name) {
            // 事件顺序：先 subagent_whitelist_deny（真实原因，带 trace），再
            // tool.return 配平（reason=subagent_whitelist_deny，与 call 可按会话整轮对齐）。
            // 不复用 early_return_events（那是 skill_on_step 熔断专用形态）。
            let mut deny_kv: Vec<(&str, String)> = vec![
                ("subagent_id", ctx.subagent_id.clone()),
                ("profile", ctx.profile.as_str().to_string()),
                ("allowed", ctx.tools.len().to_string()),
                ("tool", name.to_string()),
            ];
            deny_kv.extend(trace_kv(trace, session_id));
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "subagent_whitelist_deny",
                &deny_kv,
            );
            let ret_kv = tool_return_kv(
                name,
                "subagent_whitelist_deny",
                start.elapsed().as_millis() as u64,
                session_id,
                trace,
            );
            crate::audit::write_event(app, crate::audit::AuditLevel::Warn, "tool.return", &ret_kv);
            return crate::bot::registry::ToolResult::denied(format!(
                "⚠️ 子 agent 白名单外工具「{name}」已拒绝（profile={}，本次可用 {} 个）。只能使用白名单内工具；遇到阻塞在收尾 JSON 的 blockers 里如实记录。",
                ctx.profile.as_str(),
                ctx.tools.len()
            ));
        }
        //  预算强制（设计 §6 max_tool_calls）：白名单工具执行前核对——
        // 触顶拒绝新调用（模型收到后自然收尾；墙钟/轮数两顶在 runner 侧）。
        // 原子 check-and-increment（ 采纳）：load→比较→add 三步在并行
        // tool_call 下可同时通过——fetch_update 保证「仅 prev<max 时 +1」原子完成。
        let max = ctx.budget.max_tool_calls as usize;
        let acquired = ctx
            .used_tool_calls
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |prev| {
                    if prev < max {
                        Some(prev + 1)
                    } else {
                        None
                    }
                },
            )
            .is_ok();
        if !acquired {
            let used = ctx
                .used_tool_calls
                .load(std::sync::atomic::Ordering::SeqCst);
            let mut budget_kv: Vec<(&str, String)> = vec![
                ("subagent_id", ctx.subagent_id.clone()),
                ("used", used.to_string()),
                ("max", ctx.budget.max_tool_calls.to_string()),
            ];
            budget_kv.extend(trace_kv(trace, session_id));
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "subagent_budget_tool_calls_exceeded",
                &budget_kv,
            );
            let ret_kv = tool_return_kv(
                name,
                "budget_tool_calls_exceeded",
                start.elapsed().as_millis() as u64,
                session_id,
                trace,
            );
            crate::audit::write_event(app, crate::audit::AuditLevel::Warn, "tool.return", &ret_kv);
            return crate::bot::registry::ToolResult::denied(format!(
                "⚠️ 工具调用预算已触顶（{used} / {max}）：停止新探索，整理当前结果并输出收尾 JSON。"
            ));
        }
        let mut prog_kv: Vec<(&str, String)> = vec![
            ("subagent_id", ctx.subagent_id.clone()),
            ("task_id", ctx.task_id.clone().unwrap_or_default()),
        ];
        prog_kv.extend(trace_kv(trace, session_id));
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "subagent_progress",
            &prog_kv,
        );
    }
    let ctx = ToolCtx {
        app,
        stop,
        interactive,
        session_id,
    };
    // 入参 JSON 闸：坏 arguments 不进工具（模型拿回可自查的报错 + 审计留痕）。
    // 位置在全部闸之后——规则 deny、子 agent 白名单、预算的拒绝原因比
    // 「你的 JSON 坏了」更该先让模型看到，不因本次改动被抢答。
    if let Some(msg) = invalid_args_message(name, args) {
        let mut bad_kv: Vec<(&str, String)> = vec![
            ("tool", name.to_string()),
            ("args_preview", args.chars().take(80).collect::<String>()),
        ];
        bad_kv.extend(trace_kv(trace, session_id));
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Warn,
            "tool_args_invalid_json",
            &bad_kv,
        );
        let ret_kv = tool_return_kv(
            name,
            "invalid_args_json",
            start.elapsed().as_millis() as u64,
            session_id,
            trace,
        );
        crate::audit::write_event(app, crate::audit::AuditLevel::Warn, "tool.return", &ret_kv);
        return crate::bot::registry::ToolResult::denied(msg);
    }
    // 工具仍返 (String, Vec<TaskRef>) 元组；From 过渡层默认 Ok。
    //  cleanup 会删除 From impl，工具届时改为显式 ok/warn/error。
    // O(n) 线性扫描 → O(1) HashMap 查找；结果存 tool_def 供 post-execute 复用
    // （原先下方又走 TOOLS_TABLE.iter().find 线性扫，每次调用两次 name 查找）。
    let tool_def = tools_index().get(name).copied();
    let mut result: crate::bot::registry::ToolResult = match tool_def {
        Some(t) => run_tool_panic_isolated(app, name, (t.call)(&ctx, args)).await,
        // 阶段 4（MCP 增量挂载）：查表 miss 先走外部工具兜底路由——
        // 反查挂载表命中则调远端；编造的 mcp_* 名在路由内仍回「未知工具」。
        // 审计（tool.call/tool.return）已在函数出入口统一发出，无需重复。
        None => {
            run_tool_panic_isolated(
                app,
                name,
                crate::bot::mcp::mount::execute_mcp_tool(app, name, args),
            )
            .await
        }
    };

    // 3. L5 post_tool 钩子层：工具已跑完、结果落定但还没进审计与回灌之前。
    //    放在这里的理由：审计的 preview 与回灌给模型的文本都应是**钩子改写后**
    //    的那一份——先钩子后审计，反过来记的就是一份模型从没见过的内容。
    {
        let call = crate::bot::hooks::ToolCallView {
            tool: name,
            args,
            session_id,
            trace,
        };
        let mut text = std::mem::take(&mut result.text);
        let run = crate::bot::hooks::registry().run_post_tool(&call, &mut text);
        result.text = text;
        for f in &run.failures {
            let (kind, err) = match &f.kind {
                crate::bot::hooks::HookFailureKind::Returned(e) => ("returned_err", e.as_str()),
                crate::bot::hooks::HookFailureKind::Panic(e) => ("panic", e.as_str()),
            };
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "post_tool.hook_failed",
                &hook_failure_kv(f.hook, kind, err, name, session_id, trace),
            );
        }
    }

    // 4. post-execute 洋葱管线「出」钩子
    //    - 结构化审计事件（工具名/耗时/返回引用数/结果预览）写到 bot.log
    //    - 级别从 result.status 派生（代替原 classify_text 字符串匹配）
    //    - 镜像调用 skill_on_step_post：技能步骤结果/失败检测
    let dur_ms = start.elapsed().as_millis() as u64;
    let level = crate::audit::AuditLevel::from_tool_status(result.status);
    // 保守 token 预算——仅 audit，不截断。超阈值的工具输出走 `tool.output.over_budget`
    // 事件供事后分析，默认阈值 8192 字节，read_text_file/fetch_url/list_files 等长输出工具 65536 字节。
    // 口径必须与回灌漏斗一致（字节）：审计阈值和真实 cap 是同一个数，否则会出现
    // 「审计报了超预算、实际没截」或反之，两边各说各话。
    if let Some(t) = tool_def {
        let output_bytes = result.text.len();
        if output_bytes > t.max_output_bytes {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "tool.output.over_budget",
                &[
                    ("tool", name.to_string()),
                    ("output_bytes", output_bytes.to_string()),
                    ("max_bytes", t.max_output_bytes.to_string()),
                ],
            );
        }
    }
    // create_task/edit_task 带 files 参数时补 files_count/truncated kv（多文件绑定）
    let mut kv: Vec<(&str, String)> = vec![
        ("tool", name.to_string()),
        ("ms", dur_ms.to_string()),
        ("refs", result.refs.len().to_string()),
        ("preview", result.text.chars().take(80).collect::<String>()),
    ];
    if matches!(name, "create_task" | "edit_task") {
        if let Some((cnt, truncated)) = files_audit_kv(args) {
            kv.push(("files_count", cnt.to_string()));
            kv.push(("truncated", truncated.to_string()));
        }
    }
    // session_id / turn / tool_call_id：与 tool.call 同口径，按 (session_id, turn) 或
    // tool_call_id 可整轮回放一次模型循环里的工具序列
    kv.extend(trace_kv(trace, session_id));
    crate::audit::write_event(app, level, "tool.return", &kv);
    //  执行痕迹采集（Agent 透明化设计 §4.2）：会话在跑 trace 时逐调用落 span，
    // 文件类工具附带的变更证据落 file_changes + emit `bot-file-changed`（全窗口）。
    // fire-and-forget：sink 未初始化/满队列不阻断工具结果；registry 无映射（主聊天等）零开销跳过。
    if let Some(trace_id) = crate::app_state::trace_id_for_session(app, session_id) {
        let ok = result.status.is_success();
        crate::trace_sink::record_span(crate::trace_sink::SpanRecord {
            trace_id,
            turn: trace.turn.unwrap_or(0) as i64,
            tool_call_id: trace.tool_call_id.clone(),
            name: name.to_string(),
            args: crate::trace_sink::clamp_span_text(app, name, "args", args.to_string()),
            result: crate::trace_sink::clamp_span_text(app, name, "result", result.text.clone()),
            ok,
            // error_class 只做子类细分（timeout/permission/gate_denied…），成败
            // 由上面的 ok 决定；拿不到 status 的 MCP 工具靠这里兜住子类
            error_class: crate::audit::classify_error_class(&result.text).map(str::to_string),
            duration_ms: Some(dur_ms as i64),
            created_at: chrono::Utc::now().timestamp_millis(),
        });
        for rc in &result.file_changes {
            crate::trace_sink::record_file_change(crate::trace_sink::file_change_record(
                trace_id, rc,
            ));
            let _ = app.emit(
                "bot-file-changed",
                serde_json::json!({
                    "sessionId": session_id,
                    "traceId": trace_id,
                    "path": rc.path,
                    "kind": rc.kind,
                    "added": rc.added,
                    "deleted": rc.deleted,
                }),
            );
        }
    }
    if name != "use_skill" {
        crate::bot_skills::skill_on_step_post(
            app,
            name,
            &result.text,
            dur_ms,
            result.status,
            session_id,
        );
    }

    result
}

/// 工具入参 JSON 合法性闸（执行前，纯函数便于单测）：空/空白是「无参工具」
/// 常态，放行；非空但解析失败 = 调用方（模型 arguments 或 Skill DSL 变量替换）
/// 写坏了 JSON，回一条能让它自查的文案。
///
/// 为什么在 dispatch 拦而不是留给 `parse_args`：parse_args 落 Null 继续跑时，
/// 模型收到的回报是「缺参数」，看不出是自己的 arguments 坏了；更糟的是
/// 无参工具（query_tasks / list_files 等）会把 Null 当「不过滤、不指定」
/// 真跑一遍——参数错配被当成用户本意执行了。返回 None 表示放行。
///
/// 文案前缀 ⚠️ 让 `classify_error_class` 把这次归到 gate_denied；成败判定本身
/// 由 `ToolResult::denied` 的 status 承担。
fn invalid_args_message(name: &str, args: &str) -> Option<String> {
    if args.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(args) {
        Ok(_) => None,
        Err(e) => Some(format!(
            "⚠️ 工具「{name}」的 arguments 不是合法 JSON，已拒绝执行（未做任何改动）：{e}。\
             请重新发起一次调用：arguments 必须是单个 JSON 对象（如 {{\"path\":\"a.txt\"}}），\
             不得有多余引号、裸换行或尾逗号。"
        )),
    }
}

pub(crate) fn parse_args(args: &str) -> serde_json::Value {
    match serde_json::from_str(args) {
        Ok(v) => v,
        // 空/空白入参是「无参工具」常态，按 Null 静默放行；
        // 非空但坏 JSON 由 dispatch 的入参闸先拦下（工具根本不执行），
        // 走到这里说明绕过了闸——降级 Null 并记 bot.log 兜底。
        Err(_) if args.trim().is_empty() => serde_json::Value::Null,
        Err(e) => {
            log::warn!(
                target: "bot::dispatch",
                "工具参数 JSON 解析失败（按 Null 处理）：{e} | args: {}",
                crate::audit::escape_for_log(args, 200)
            );
            serde_json::Value::Null
        }
    }
}

// 工具层 commit template

/// 工具层写库收尾：调 `db::db_upsert_for` → `broadcast_after_mutation` → 返回
/// `ToolResult`。覆盖 7 处原 inline `match db_upsert(...) { Ok => broadcast+ok, Err => ok(fail) }`
/// 模板(bot/tools.rs: tool_create_task / tool_complete_task / tool_delete_task /
/// tool_edit_task / tool_add_subtask / tool_toggle_subtask / tool_remove_subtask)。
///
/// 调用方负责：
/// 1. RMW 接线(4 个标准 tool 用 `db::prepare_for_upsert(&mut task)`;
///    `tool_complete_task` / `tool_delete_task` 复用 `completed_at` / `deleted_at`
///    时间戳作为 `updated_at`,inline 自戳;`tool_create_task` 是新建,`expected_updated_at = None`)
/// 2. 构造成功消息(部分工具需要先 `let st_text = ...` 等中间变量,如 toggle_subtask)
/// 3. 构造 `refs`(一般 `[TaskRef { id, title }]`)
///
/// 失败消息模板:`{fail_prefix}：{e}`,与历史 7 处 inline 行为 1:1 等价。
/// **失败走 `ToolResult::error`**:写库失败就是这次调用没成功。此前返 `ok` 是为了
/// 让「LLM pipeline severity classifier」不把它当 fatal,但那条顾虑针对的是
/// **模型读到的文本**——`{fail_prefix}：{e}` 里 fail_prefix 必带「失败」二字,
/// 文本分类照样判失败,改 status 不影响模型侧。真正被 status=Ok 坑到的是本地
/// 消费者:幻觉守卫 `mutation_succeeded` 按 status 判,写库失败却被记成
/// 「动过手」,模型随后可以随意汇报任务已完成。
/// 显式的 `task.commit_failed` Error 审计保留(带 tool_prefix / task_id / err),
/// 它比 `tool.return` 的通用事件更好检索。
pub(crate) async fn commit_and_report<R: tauri::Runtime>(
    app: &AppHandle<R>,
    task: &crate::db::Task,
    // thunk(impl FnOnce)而非直接 String/Vec:成功路径才求值,
    // 失败路径(E)直接 drop 闭包,不分配 throw away。
    success_msg: impl FnOnce() -> String,
    refs: impl FnOnce() -> Vec<crate::bot_chat::TaskRef>,
    fail_prefix: &'static str,
) -> crate::bot::registry::ToolResult {
    match crate::db::db_upsert_for(app, vec![task.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![task.clone()], Vec::new());
            crate::bot::registry::ToolResult::ok(success_msg(), refs())
        }
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Error,
                "task.commit_failed",
                &[
                    ("tool_prefix", fail_prefix.to_string()),
                    ("task_id", task.id.clone()),
                    ("err", e.to_string()),
                ],
            );
            crate::bot::registry::ToolResult::error(format!("{fail_prefix}：{e}"), Vec::new())
        }
    }
}

#[cfg(test)]
mod commit_and_report_tests {
    /// 写库失败时 status 与文本必须给出同一个答案。
    ///
    /// status=Error 是新的成败口径；文本 `{fail_prefix}：{err}` 要能被
    /// `classify_tool_failure` 认出，两者才不打架（此前 status=Ok 而文本判失败，
    /// 同一个结果两种答案：审计说成功、调度器说失败）。
    /// 7 个任务卡工具的 fail_prefix 必须都含「失败」，否则 error_class 落空、
    /// 转换历史消息的 Anthropic 层不会设 is_error。
    #[test]
    fn every_commit_and_report_fail_prefix_is_classifiable() {
        // 逐行扫：进入 commit_and_report( 后，收集形参区里最后一个「独占一行的
        // 字符串字面量」——那就是 fail_prefix（成功/失败消息都是 format! 或
        // 多行闭包，只有 fail_prefix 是单行字面量）
        let mut prefixes: Vec<String> = Vec::new();
        let mut in_call = false;
        let mut pending: Option<String> = None;
        for line in include_str!("tools.rs").lines() {
            let t = line.trim();
            if t.starts_with("commit_and_report(") {
                in_call = true;
                pending = None;
                continue;
            }
            if in_call {
                if t.starts_with(".await") {
                    if let Some(p) = pending.take() {
                        prefixes.push(p);
                    }
                    in_call = false;
                    continue;
                }
                if let Some(rest) = t.strip_suffix(',') {
                    let s = rest.trim();
                    if s.len() > 1 && s.starts_with('"') && s.ends_with('"') {
                        pending = Some(s.trim_matches('"').to_string());
                    }
                }
            }
        }
        assert_eq!(
            prefixes.len(),
            7,
            "任务卡工具应恰好 7 处 commit_and_report，实参解析出 {} 个: {prefixes:?}",
            prefixes.len()
        );
        for p in &prefixes {
            assert!(
                p.contains("失败"),
                "fail_prefix 应含「失败」二字（当前 {p:?}）"
            );
            assert!(
                crate::audit::tool_call_failed("complete_task", &format!("{p}：磁盘只读")),
                "fail_prefix 必须能被 classify_tool_failure 认出，否则 status 与文本口径分裂：{p}"
            );
            assert!(
                crate::audit::classify_error_class(&format!("{p}：磁盘只读")).is_some(),
                "error_class 不应为空：{p}"
            );
        }
    }

    /// 写库失败走 Error 不是 Denied：工具跑过了，是它自己没做完。
    #[test]
    fn commit_failure_status_is_error_not_denied() {
        // 直接验映射：Error → ERROR 审计；Denied 是「没执行」，语义不同
        assert_eq!(
            crate::audit::AuditLevel::from_tool_status(crate::bot::registry::ToolStatus::Error),
            crate::audit::AuditLevel::Error
        );
        assert_eq!(
            crate::audit::AuditLevel::from_tool_status(crate::bot::registry::ToolStatus::Denied),
            crate::audit::AuditLevel::Warn
        );
    }
}

#[cfg(test)]
mod early_return_events_tests {
    use super::*;

    type AuditEvent = (
        crate::audit::AuditLevel,
        &'static str,
        Vec<(&'static str, String)>,
    );

    fn event_names(evs: &[AuditEvent]) -> Vec<&'static str> {
        evs.iter().map(|(_, e, _)| *e).collect()
    }

    fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn rule_gate_deny_path_emits_deny_then_balanced_tool_return() {
        // 规则闸拦截路径：pre_execute.deny → tool.return（顺序敏感），
        // reason=denied_by_rule / exit_code=none / duration_ms，事件带 trace 可整轮回放
        let trace = ToolCallTrace {
            turn: Some(2),
            tool_call_id: Some("call_abc".into()),
        };
        let evs = early_return_events(
            "create_word_revisions",
            "denied_by_rule",
            3,
            None,
            Some("sess-1"),
            &trace,
        );
        let names: Vec<&str> = evs.iter().map(|(_, e, _)| *e).collect();
        assert_eq!(names, ["pre_execute.deny", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(
            evs.iter()
                .find(|(_, e, _)| *e == "pre_execute.deny")
                .and_then(|(_, _, kv)| kv.iter().find(|(k, _)| *k == "tool"))
                .map(|(_, v)| v.as_str()),
            Some("create_word_revisions")
        );
        for ev in &evs {
            let kv = &ev.2;
            assert_eq!(
                kv.iter()
                    .find(|(k, _)| *k == "session_id")
                    .map(|(_, v)| v.as_str()),
                Some("sess-1")
            );
            assert_eq!(
                kv.iter()
                    .find(|(k, _)| *k == "turn")
                    .map(|(_, v)| v.as_str()),
                Some("2")
            );
        }
    }

    #[test]
    fn skill_step_error_path_warns_then_balanced_tool_return() {
        // 熔断路径：skill_on_step_error（补 Warn 可见性）→ tool.return(reason=skill_step_failed)
        let evs = early_return_events(
            "run_python",
            "skill_step_failed",
            5,
            Some("超过最大步数上限（8 步）"),
            None,
            &ToolCallTrace::default(),
        );
        assert_eq!(event_names(&evs), ["skill_on_step_error", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "err"), Some("超过最大步数上限（8 步）"));
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "reason"), Some("skill_step_failed"));
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert!(kv_get(ret, "duration_ms").is_some());
    }

    // ── trace_kv：有值才写，无会话上下文不产生误导性 turn=0 ──

    #[test]
    fn trace_kv_omits_absent_turn_and_tool_call_id() {
        let kv = trace_kv(&ToolCallTrace::default(), None);
        assert_eq!(kv_get(&kv, "session_id"), Some("-"));
        assert!(kv_get(&kv, "turn").is_none(), "无 turn 不应写 turn=0");
        assert!(kv_get(&kv, "tool_call_id").is_none());
    }

    #[test]
    fn trace_kv_writes_present_fields_and_blank_ids_fall_back_to_dash() {
        let kv = trace_kv(
            &ToolCallTrace {
                turn: Some(0),
                tool_call_id: Some(String::new()),
            },
            Some(""),
        );
        assert_eq!(kv_get(&kv, "session_id"), Some("-"), "空会话按 - 记");
        assert_eq!(kv_get(&kv, "turn"), Some("0"), "第 0 轮是有效值");
        assert!(
            kv_get(&kv, "tool_call_id").is_none(),
            "空 id 不写（合成 id 前的畸形流不该出现空值行）"
        );
    }
}

#[cfg(test)]
mod pre_tool_hook_tests {
    use super::*;

    /// L5 接线守卫：钩子层必须**真被 `execute_tool_impl` 调到**，且排在
    /// 「工具已执行」之后、「审计 preview 与结果回灌」之前。
    ///
    /// 变异实测把 dispatch 里那段调用整段删掉（工具结果再也过不了钩子）时，
    /// hooks 模块 17 条 + registry 与 bot_onions 全部 57 条测试照样全绿——
    /// 它们验的是注册表的行为，验不到 dispatch 有没有接线。所以这里钉住
    /// 「语句存在 + 三点相对位置」，钉的是接线而不是中间量。
    #[test]
    fn post_tool_layer_is_wired_between_execution_and_audit() {
        let body = include_str!("dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        let body = body
            .split_once("async fn execute_tool_impl(")
            .expect("找不到 execute_tool_impl")
            .1
            .split("\n}\n")
            .next()
            .expect("execute_tool_impl 应以顶格 } 收尾");
        let exec = body.find("(t.call)(&ctx, args)").expect("找不到工具执行点");
        let hook = body
            .find("run_post_tool(&call, &mut text)")
            .expect("execute_tool_impl 必须调用 post_tool 钩子层：结果回灌前必须过钩子");
        let audit = body
            .find("tool.output.over_budget")
            .expect("找不到输出预算审计");
        let refill = body
            .find("skill_on_step_post")
            .expect("找不到结果回灌/技能后置钩子");
        assert!(
            exec < hook,
            "钩子必须在工具执行之后（拿得到结果才谈得上改写）"
        );
        assert!(
            hook < audit,
            "钩子必须在审计 preview 之前：否则记的是一份模型从没见过的文本"
        );
        assert!(
            hook < refill,
            "钩子必须在结果回灌之前：否则改写不会到达模型"
        );
    }

    /// 主聊天路径的 panic 隔离守卫：工具 future 必须包 catch_unwind。
    ///
    /// 工具 panic → future unwind → 这一轮对话整轮丢失（无部分输出、无错误提示），
    /// 前端停在「正在思考」。子 agent runner 早有同样的隔离
    /// （`bot_orchestrator::run_subagent`），主链路必须有——**同一份工具代码
    /// 不该有两种健壮性**。hooks 扩展点三层都做了 fail-open，被扩展的本体
    /// 反而没有。
    #[test]
    fn tool_execution_is_panic_isolated() {
        let src = include_str!("dispatch.rs");
        let body = src
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        // 内置工具与 MCP 兜底路由都要走隔离层。
        // 判据按「调用点之前有没有出现过隔离层的名字」——不按整行文本：
        // rustfmt 一换行整行匹配就失效，而失效的守卫往往是被改断言绕过的。
        // （用 rfind 而不是定长字节窗口：窗口偏移可能落在多字节字符中间。）
        let call_pos = body.find("(t.call)(&ctx, args)").expect("找不到工具执行点");
        assert!(
            body[..call_pos].rfind("run_tool_panic_isolated").is_some(),
            "内置工具执行必须走 panic 隔离层"
        );
        let mcp_pos = body.find("execute_mcp_tool").expect("找不到 MCP 兜底路由");
        assert!(
            body[..mcp_pos].rfind("run_tool_panic_isolated").is_some(),
            "MCP 兜底路由也要走 panic 隔离层"
        );
        // 隔离层本身必须真的 catch，且把 panic 归一成失败（不是成功调用）
        // 匹配函数名不带括号：泛型化后名字后面是 `<R, F>`，带括号就匹配不到。
        let iso = body
            .split_once("fn run_tool_panic_isolated")
            .expect("找不到 panic 隔离层")
            .1;
        assert!(
            iso.contains("catch_unwind"),
            "隔离层必须真的 catch_unwind（panic 要归一成普通失败回灌给模型）"
        );
        assert!(
            iso.contains("ToolResult::error"),
            "panic 必须归一成失败——算成成功调用会污染幻觉守卫的成功凭证表"
        );
    }

    /// panic 的**行为**测试：工具炸了不许把这一轮带走。
    ///
    /// 归一成 `Error` 而不是「成功」：成功凭证表会被污染（幻觉守卫查不到
    /// 真实成功记录，反过来给模型背书）。
    #[tokio::test]
    async fn panic_in_tool_is_normalized_into_an_error_result() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let fut = async move {
            // black_box 挡住编译期折叠，编译器才不会把另一支判成不可达
            if std::hint::black_box(true) {
                panicking_tool()
            } else {
                crate::bot::registry::ToolResult::ok("没炸", Vec::new())
            }
        };
        let r = super::run_tool_panic_isolated(&handle, "boom_tool", fut).await;
        assert_eq!(
            r.status,
            crate::bot::registry::ToolStatus::Error,
            "panic 必须归一成失败，不能算成成功调用"
        );
        assert!(
            r.text.contains("工具执行异常"),
            "失败文案要说清是异常中断：{}",
            r.text
        );
    }

    /// 一个只会炸的工具（模拟工具内部 panic）。
    fn panicking_tool() -> crate::bot::registry::ToolResult {
        panic!("工具内部炸了");
    }

    fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    fn keys(kv: &[(&'static str, String)]) -> Vec<&'static str> {
        kv.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn hook_interrupt_balances_tool_call_with_same_shape_as_existing_early_returns() {
        // 配平形状必须与既有两处 early-return 分支同口径（统计面板按
        // tool.call / tool.return 配对，不配平 = 悬挂调用）。
        let trace = ToolCallTrace {
            turn: Some(4),
            tool_call_id: Some("call_hk".into()),
        };
        let evs = hook_interrupt_events("write_file", "本轮不允许写盘", 7, Some("sess-9"), &trace);
        let names: Vec<&str> = evs.iter().map(|(_, e, _)| *e).collect();
        assert_eq!(names, ["pre_tool.hook_interrupt", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "reason"), Some("本轮不允许写盘"));
        // 与 early_return_events 的 tool.return 同键集：tool / reason / exit_code /
        // duration_ms + trace 段。
        let baseline = early_return_events(
            "write_file",
            "denied_by_rule",
            7,
            None,
            Some("sess-9"),
            &trace,
        );
        let baseline_ret = &baseline[1].2;
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert_eq!(kv_get(ret, "duration_ms"), Some("7"));
        assert_eq!(
            kv_get(ret, "reason"),
            Some("pre_tool_hook_interrupt"),
            "reason 要能区分拒绝来源（规则表 vs 钩子）"
        );
        let mut a = keys(baseline_ret);
        let mut b = keys(ret);
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "tool.return 的 kv 键集必须与既有 early-return 一致");
        // 第三处早退（子 agent 白名单）原先是手写字面量，与另两处没有任何
        // 机制保证一致。现在三处都走 tool_return_kv 一致性由构造保证，
        // 本断言只钉住「没有哪一处绕开它自己手搓 kv」。
        let body = include_str!("dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        // 每处早退分支：从「事件名」到下一个 return 之间那段——手搓 kv 就在
        // 这段里（键集只由 tool_return_kv 决定）。
        for (event, label) in [
            ("subagent_whitelist_deny\",", "子 agent 白名单"),
            ("subagent_budget_tool_calls_exceeded\",", "工具调用预算触顶"),
            ("tool_args_invalid_json", "入参 JSON 非法"),
        ] {
            let branch = body
                .split_once(event)
                .unwrap_or_else(|| panic!("找不到{label}早退分支"))
                .1
                .split("return crate::bot::registry::ToolResult::denied(")
                .next()
                .expect("早退分支应在此处返回");
            assert!(
                branch.contains("tool_return_kv("),
                "{label}分支必须走 tool_return_kv 构造 tool.return：{branch}"
            );
            assert!(
                !branch.contains("exit_code"),
                "{label}分支不得自己手搓 tool.return 的 kv（键集只由 tool_return_kv 决定）：{branch}"
            );
        }
        assert_eq!(kv_get(ret, "turn"), Some("4"));
        assert_eq!(kv_get(ret, "tool_call_id"), Some("call_hk"));
        assert_eq!(kv_get(ret, "session_id"), Some("sess-9"));
    }

    #[test]
    fn subagent_whitelist_gate_judges_on_the_intersection() {
        // 接线守卫：闸的判据是 ctx.tools（profile ∩ per-onion allowed），
        // 不是 profile 白名单。只收窄 schema 不收窄闸等于没收窄——模型看不见
        // 的工具仍能硬猜名字调通配置档明令禁用的能力。
        let body = include_str!("dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        let gate = body
            .split_once("if let Some(ctx) = crate::tool_guard::subagent_ctx(session_id) {")
            .expect("找不到子 agent 白名单闸的入口")
            .1
            .split("subagent_whitelist_deny\",")
            .next()
            .expect("闸分支应一直到 deny 事件");
        assert!(
            gate.contains(concat!("ctx.tools.", "contains(&name)")),
            "闸的判据必须是 ctx.tools（交集后的全集）：{gate}"
        );
        assert!(
            !gate.contains(concat!("profile_whitelist(", "ctx.profile)")),
            "闸不得退回 profile 白名单（那样 allowed 收窄形同虚设）：{gate}"
        );
    }

    #[test]
    fn hook_interrupt_text_is_classified_as_tool_failure() {
        // ⚠️ 前缀缺失时 Skill DSL 调度器会把拒跑记成步骤成功（scheduler 只看
        // tool_call_failed 判定），拒跑必须被判失败。
        let msg = format!("⚠️ {}", "本轮不允许写盘");
        assert!(
            crate::audit::tool_call_failed("write_file", &msg),
            "钩子拒绝文案必须被判失败：{msg}"
        );
    }

    /// 门禁拒绝的文案必须同时满足两件事：status=Denied（成败判定）+
    /// 文本可被 classify_tool_failure 认出子类（error_class 细分 + 转换历史
    /// 消息的 Anthropic 层靠它设 is_error）。
    ///
    /// 这条测试盯的是实际发出去的文案。历史上规则 deny / 用户拒绝 / 白名单外 /
    /// 预算触顶这 4 条既不带「⚠️」也不带「失败：」，classify 全部落空 → 返回 None
    /// → Skill DSL 调度器把被拒步骤记成成功、trace span 记 ok=true。
    #[test]
    fn every_gate_denial_is_denied_status_and_classifiable_text() {
        use crate::bot::registry::{ToolResult, ToolStatus};
        // 与 execute_tool_impl 里 7 个门禁早退点一一对应（rendered 文案）
        let cases: Vec<(&str, ToolResult)> = vec![
            (
                "pre_tool 钩子拒绝",
                ToolResult::denied("⚠️ 本轮不允许写盘"),
            ),
            (
                "工具规则 deny",
                ToolResult::denied("⚠️ 工具「run_python」已被工具规则禁用（设置页授权卡可调整）。如任务确实需要，请向用户说明。"),
            ),
            (
                "确认被拒",
                ToolResult::denied("用户拒绝：工具「delete_task」未获确认（后台无人应答）。"),
            ),
            (
                "skill 熔断",
                ToolResult::denied("skill「日报」超过最大步数上限（8 步），已强制终止"),
            ),
            (
                "子 agent 白名单外",
                ToolResult::denied(
                    "⚠️ 子 agent 白名单外工具「edit_file」已拒绝（profile=Research，本次可用 8 个）。只能使用白名单内工具；遇到阻塞在收尾 JSON 的 blockers 里如实记录。",
                ),
            ),
            (
                "预算触顶",
                ToolResult::denied(
                    "⚠️ 工具调用预算已触顶（12 / 12）：停止新探索，整理当前结果并输出收尾 JSON。",
                ),
            ),
            (
                "arguments 非法 JSON",
                ToolResult::denied(
                    "⚠️ 工具「create_task」的 arguments 不是合法 JSON，已拒绝执行（未做任何改动）：expected value。",
                ),
            ),
        ];
        for (label, r) in &cases {
            assert_eq!(
                r.status,
                ToolStatus::Denied,
                "{label}：门禁早退必须是 Denied，用 warn 会被调度器/trace 记成成功"
            );
            assert!(
                !r.status.is_success(),
                "{label}：Denied 的 is_success 必须为 false"
            );
            assert!(
                crate::audit::tool_call_failed("t", &r.text),
                "{label}：文案必须能被 classify_tool_failure 认出，否则 error_class 与 Anthropic is_error 全丢：{}",
                r.text
            );
            assert!(
                crate::audit::classify_error_class(&r.text).is_some(),
                "{label}：error_class 不应为空"
            );
        }
        // 审计级别：被拒是预期内控制流，不该刷 ERROR
        for (label, r) in &cases {
            assert_eq!(
                crate::audit::AuditLevel::from_tool_status(r.status),
                crate::audit::AuditLevel::Warn,
                "{label}：Denied 映射 WARN（正常控制流），ERROR 只留给真执行失败"
            );
        }
    }

    /// 防止后来者把门禁早退又写成 ToolResult::warn —— 那正是本条修复的回归形态。
    ///
    /// 全文件口径：dispatch 的早退只有两类语义——闸/拒绝类一律 Denied，工具
    /// 本体自己跑完返回的结果一律由 tools.rs 构造。`ToolResult::warn` 在本文件
    /// 不该有生产用途（真要表达「做成但有告警」应改用 `ok` + 告警文本，或在
    /// 工具本体里显式 warn）。
    #[test]
    fn no_gate_early_return_uses_warn_status() {
        let production = include_str!("dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        assert!(
            !production.contains("ToolResult::warn("),
            "dispatch 的生产代码不得构造 ToolResult::warn（会把「没执行」记成成功）"
        );
        // 7 处：pre_tool 钩子拒绝 / 规则 deny / 确认被拒 / skill 熔断 /
        // 子 agent 白名单外 / 预算触顶 / arguments 非法 JSON。
        // 数量变了请同步本测试与 every_gate_denial_* 的用例表。
        assert_eq!(
            production.matches("ToolResult::denied(").count(),
            7,
            "门禁早退点应恰好 7 处"
        );
    }

    /// ToolStatus 四态的成败语义：Warn 算成功（做成了但有告警），Error/Denied 不算。
    #[test]
    fn tool_status_success_semantics() {
        use crate::bot::registry::ToolStatus;
        assert!(ToolStatus::Ok.is_success());
        assert!(ToolStatus::Warn.is_success());
        assert!(!ToolStatus::Error.is_success());
        assert!(!ToolStatus::Denied.is_success());
    }

    #[test]
    fn hook_failure_kv_names_hook_kind_and_error() {
        // fail-open 的审计必须能定位到「哪个钩子、哪种失败、什么错」，
        // 否则钩子里的 bug 在 bot.log 里无从排查。
        let trace = ToolCallTrace {
            turn: Some(1),
            tool_call_id: Some("call_x".into()),
        };
        let kv = hook_failure_kv(
            "builtin_observer",
            "panic",
            "boom",
            "read_text_file",
            Some("sess-2"),
            &trace,
        );
        assert_eq!(kv_get(&kv, "hook"), Some("builtin_observer"));
        assert_eq!(kv_get(&kv, "kind"), Some("panic"));
        assert_eq!(kv_get(&kv, "err"), Some("boom"));
        assert_eq!(kv_get(&kv, "tool"), Some("read_text_file"));
        assert_eq!(kv_get(&kv, "session_id"), Some("sess-2"));
        assert_eq!(kv_get(&kv, "turn"), Some("1"));
        assert_eq!(kv_get(&kv, "tool_call_id"), Some("call_x"));
    }

    #[test]
    fn pre_tool_layer_is_wired_at_the_entry_of_the_three_gates() {
        // 纯函数单测钉不住「钩子层是否真接在三道闸入口」：把 run_pre_tool_layer
        // 挪到闸后（或写成调用还在但不可达的形状），其余单测照样全绿——
        // 而这一期的全部价值就是「每个工具调用都过一次钩子」。
        // execute_tool_impl 需要 Wry AppHandle，单测够不到，故以源码文本钉住
        // 调用位置（与 invalid_args_gate_is_wired_into_execute_tool_impl 同款理由）。
        let src = include_str!("dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        let body = src
            .split_once("async fn execute_tool_impl(")
            .expect("找不到 execute_tool_impl")
            .1
            .split("\n}\n")
            .next()
            .expect("execute_tool_impl 应以顶格 } 收尾");
        let call = body.find("tool.call").expect("找不到 tool.call 审计");
        let hook = body
            // 钉 `if let Some(...) = ` 而不只是调用本身：变异实测把它改成
            // `let _ = run_pre_tool_layer(...)`（seam 彻底死亡、钩子永远不
            // 影响任何工具调用）时，四条顺序断言照样全绿——只搜裸调用串会
            // 给 B3 一份假保证。这里钉的是**返回值被消费**。
            .find("if let Some(interrupted) = run_pre_tool_layer(")
            .expect("execute_tool_impl 必须消费 pre_tool 钩子层的判定并早退");
        let hook_return = body
            .find("return interrupted;")
            .expect("钩子 Interrupt 必须早退（否则拒绝判定被丢弃）");
        let rule_gate = body.find("tool_rule_gate(app, name").expect("找不到规则闸");
        let subagent_gate = body
            .find("crate::tool_guard::subagent_ctx(session_id)")
            .expect("找不到子 agent 白名单闸");
        let args_gate = body
            .find("if let Some(msg) = invalid_args_message(name, args)")
            .expect("找不到入参 JSON 闸");
        assert!(call < hook, "钩子层在 tool.call 之后（否则审计不成对）");
        assert!(
            hook < hook_return,
            "早退必须紧跟钩子层（中间插别的东西说明判定被丢弃或改写）"
        );
        assert!(
            hook < rule_gate && hook < subagent_gate && hook < args_gate,
            "钩子层必须插在三道闸入口之前：规则闸 {rule_gate} / 子 agent 闸 {subagent_gate} / 入参闸 {args_gate}，钩子 {hook}"
        );
    }

    #[test]
    fn pre_tool_layer_passes_through_when_registry_has_no_denial() {
        // 无 AppHandle 的纯函数面：注册表默认全放行时判定为 Continue
        // （fail-open 与「无钩子拒绝」两条路径在 dispatch 侧都落到 None）。
        let trace = ToolCallTrace::default();
        let call = ToolCallView {
            tool: "read_text_file",
            args: "{}",
            session_id: None,
            trace: &trace,
        };
        let run = crate::bot::hooks::registry().run_pre_tool(&call);
        assert_eq!(run.decision, PreToolVerdict::Continue);
        assert!(
            run.failures.is_empty(),
            "内置钩子不得自报错：{:?}",
            run.failures
        );
    }
}

#[cfg(test)]
mod invalid_args_tests {
    use super::*;

    #[test]
    fn empty_args_pass_as_no_arg_tools() {
        // 无参工具常态：空串 / 纯空白 / 只有换行都放行
        assert!(invalid_args_message("query_tasks", "").is_none());
        assert!(invalid_args_message("query_tasks", "   ").is_none());
        assert!(invalid_args_message("get_current_time", "\n\t ").is_none());
    }

    #[test]
    fn parseable_args_pass_including_non_object_json() {
        // 只拦「解析不了」，不越权改判定：合法 JSON 一律放行
        // （部分网关/老模型会发 null / 数组形态，那是既有行为，不在本次收紧范围）
        assert!(invalid_args_message("create_task", r#"{"title":"x"}"#).is_none());
        assert!(invalid_args_message("create_task", "null").is_none());
        assert!(invalid_args_message("create_task", "{}").is_none());
    }

    #[test]
    fn malformed_args_return_actionable_message_for_model() {
        let msg = invalid_args_message("create_task", r#"{"title":"x""#)
            .expect("非空但坏 JSON 必须拒绝并回可解释错误");
        assert!(msg.contains("create_task"), "文案须指名工具：{msg}");
        assert!(msg.contains("不是合法 JSON"), "{msg}");
        assert!(
            msg.contains("已拒绝执行") && msg.contains("未做任何改动"),
            "须说明未执行，模型才会换参数重试而不是以为已生效：{msg}"
        );
        // serde 的真实错因带出来，模型才知道是截断还是引号坏
        assert!(
            msg.contains("EOF while parsing"),
            "须带 serde 原始错因：{msg}"
        );
    }

    #[test]
    fn malformed_args_message_is_classified_as_failure() {
        // 不带 ⚠️ 前缀就落不进 classify_tool_failure 的门禁臂：
        // Skill DSL 调度器会把拒跑记成步骤成功（scheduler 只看 tool_call_failed）
        let msg = invalid_args_message("create_task", "{坏}").expect("坏 JSON 必须拒");
        assert!(
            crate::audit::tool_call_failed("create_task", &msg),
            "拒跑文案必须被判失败：{msg}"
        );
    }

    #[test]
    fn invalid_args_gate_is_wired_into_execute_tool_impl() {
        // 纯函数单测钉不住「闸是否真接在执行路径上」：变异实测把闸改成不可达
        // （if true { None } else { 闸 }），1511 个 lib 测试照样全绿——而这个提交
        // 的全部价值就是「工具不再执行」。execute_tool_impl 需要 Wry AppHandle，
        // 集成测试的 MockRuntime 够不到，故以源码文本钉住「语句形状 + 位置」。
        let src = include_str!("dispatch.rs")
            .split_once("#[cfg(test)]")
            .expect("dispatch.rs 应有测试模块")
            .0;
        let body = src
            .split_once("async fn execute_tool_impl(")
            .expect("找不到 execute_tool_impl")
            .1
            .split("\n}\n")
            .next()
            .expect("execute_tool_impl 应以顶格 } 收尾");
        // 钉语句本身而非函数名：写成 if true { None } else { 闸(...) } 这类
        // 「调用还在但不可达」的形状也必须红
        let gate = body
            .find("if let Some(msg) = invalid_args_message(name, args)")
            .expect("execute_tool_impl 必须以闸的 if let 直接调用：坏 JSON 不得进工具");
        let hook = body
            .find("skill_on_step(app, name, args, session_id)")
            .expect("找不到 skill 步骤钩子");
        let lookup = body
            .find("tools_index().get(name)")
            .expect("找不到工具查表");
        assert!(
            hook < gate,
            "入参闸应在 skill 钩子之后（熔断原因比 JSON 错因更该先给模型）"
        );
        assert!(
            gate < lookup,
            "入参闸必须在工具查表与执行之前，否则等于没拦"
        );
    }

    #[test]
    fn skill_variable_breakage_is_rejected_not_silently_nulled() {
        // bot_skills::vars 的已知限制（见该文件同名单测）：占位符紧邻转义引号时
        // 原样插入，结果是非合法 JSON。这里必须拒绝（原先静默 Null 后按缺参跑工具）
        let broken = r#"{"a": "say \"hi\" he said "no""}"#;
        assert!(
            invalid_args_message("create_task", broken).is_some(),
            "DSL 变量替换破坏的 JSON 必须被闸挡住"
        );
    }
}
