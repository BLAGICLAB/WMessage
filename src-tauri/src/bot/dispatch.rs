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

/// 子 agent 动作流（右侧栏实时可见性）：dispatch 在 call / return 两个相位
/// 各旁路一条 UI 事件，前端按 toolCallId 合并成一行。事件即焚不落库——
/// 历史回看走详情面板的结果与 digest，不另存流。
/// preview 与审计同源同截断（80 字），不另开脱敏面。
fn emit_subagent_activity(
    app: &AppHandle,
    ctx: &crate::tool_guard::SubagentSessionCtx,
    name: &str,
    trace: &ToolCallTrace,
    phase: &str,
    ok: Option<bool>,
    ms: Option<u64>,
    preview: &str,
) {
    let _ = app.emit_to(
        "main",
        "subagent-activity",
        serde_json::json!({
            "subagentId": ctx.subagent_id,
            "turn": trace.turn,
            "toolCallId": trace.tool_call_id,
            "tool": name,
            "phase": phase,
            "ok": ok,
            "ms": ms,
            "preview": preview.chars().take(80).collect::<String>(),
            "usedToolCalls": ctx.used_tool_calls.load(std::sync::atomic::Ordering::SeqCst),
        }),
    );
}

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

/// allowed-tools 闸判定（纯查询，mock handle 可测）：返回 Some((技能名, 声明集合))
/// = 该会话的活动技能声明了 allowed-tools 且本工具不在集合内。
/// use_skill 豁免由调用方把守（切技能是元动作，不属技能自身的工具面）。
fn allowed_tools_violation<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
    session_id: Option<&str>,
) -> Option<(String, Vec<String>)> {
    let run = crate::bot_skills::active_skill_run_for(app, session_id)?;
    if run.allowed_tools.is_empty() || run.allowed_tools.iter().any(|t| t == name) {
        return None;
    }
    Some((run.name.clone(), run.allowed_tools.clone()))
}

/// 集合外调用的拒绝文案（纯函数，可测）：⚠️ 开头进 GateDenied 分类
/// （classify_tool_failure 的门禁臂），点名技能、声明集合与被拒工具，
/// 模型才知道这是技能作者的自限、该改走集合内工具或请用户放宽。
fn allowed_tools_denial_text(skill: &str, declared: &[String], name: &str) -> String {
    format!(
        "⚠️ 技能「{skill}」的 allowed-tools 未包含工具 {name}（声明集合：{declared}），已拒绝执行",
        declared = declared.join(", ")
    )
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
    // 0.55 会话内活动技能的 allowed-tools 前置声明：作者自限的工具面。
    // DSL 调度器步骤路径在 scheduler 里有同名校验，这里补齐 use_skill
    // 交互路径——模型驱动的工具调用同样只放行声明集合内的工具，
    // use_skill 豁免（切技能是元动作，与规则表闸同口径）。
    if name != "use_skill" {
        if let Some((skill, declared)) = allowed_tools_violation(app, name, session_id) {
            let dur_ms = start.elapsed().as_millis() as u64;
            let mut cause_kv: Vec<(&str, String)> = vec![
                ("skill", skill.clone()),
                ("tool", name.to_string()),
                ("declared", declared.join(", ")),
            ];
            cause_kv.extend(trace_kv(trace, session_id));
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "skill.allowed_tools_violation",
                &cause_kv,
            );
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "tool.return",
                &tool_return_kv(name, "allowed_tools_violation", dur_ms, session_id, trace),
            );
            return crate::bot::registry::ToolResult::denied(allowed_tools_denial_text(
                &skill, &declared, name,
            ));
        }
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
        // 动作流 call 相位：白名单通过即上屏（后续无论放行还是被确认/收尾窗拒，
        // 前端都能按 toolCallId 合并成一行带结局的动作）
        emit_subagent_activity(
            app,
            &ctx,
            name,
            trace,
            "call",
            None,
            None,
            &args.chars().take(80).collect::<String>(),
        );
        // 工具调用的阶梯（软告警 / 收尾窗 / 硬停）在模型循环层统一判定
        // （fuse::gate_call），这里不设预算闸、不设墙钟。
        // 执行计数仪表：read_own_card 的实时余量与收尾时的预算触达判定读它。
        // 强制点在模型循环层的阶梯，这里只记数。
        ctx.used_tool_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        // 动作流 return 相位（子 agent 会话）：call 已上屏，坏入参的拒绝也要有结局，
        // 否则前端那行永远挂着「进行中」
        if let Some(ctx) = crate::tool_guard::subagent_ctx(session_id) {
            emit_subagent_activity(
                app,
                &ctx,
                name,
                trace,
                "return",
                Some(false),
                Some(start.elapsed().as_millis() as u64),
                &msg,
            );
        }
        return crate::bot::registry::ToolResult::denied(msg);
    }
    // 工具仍返 (String, Vec<TaskRef>) 元组；From 过渡层默认 Ok。
    //  cleanup 会删除 From impl，工具届时改为显式 ok/warn/error。
    // O(n) 线性扫描 → O(1) HashMap 查找；查表结果决定走内置工具还是 MCP 兜底路由
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
    // 「工具输出超预算」的审计事件**不在这里发**——这里只知道 per-tool 预算，
    // 而真实截断用 min(per-tool 预算, 全局 cap)（见 bot_model_loop 的
    // effective_output_cap）。全局 cap 更紧时（默认 30000 vs 长输出工具的
    // 65536）文本被截了而审计不报；反过来审计报了也可能根本没截——两边各说
    // 各话。事件移到截断发生的地方发（bot_model_loop 的 execute_tool 闭包），
    // 审计阈值与真实 cap 是同一个数。
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
    // 动作流 return 相位（子 agent 会话）：带上耗时/成败/结果预览——前端把
    // 同 toolCallId 的 call 行收口成一条带结局的动作
    if let Some(ctx) = crate::tool_guard::subagent_ctx(session_id) {
        emit_subagent_activity(
            app,
            &ctx,
            name,
            trace,
            "return",
            Some(result.status.is_success()),
            Some(dur_ms),
            &result.text,
        );
    }
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
            created_at: crate::db::now_ms(),
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
mod commit_and_report_tests;

#[cfg(test)]
mod early_return_events_tests;

#[cfg(test)]
mod pre_tool_hook_tests;

#[cfg(test)]
mod allowed_tools_tests;

#[cfg(test)]
mod invalid_args_tests;
