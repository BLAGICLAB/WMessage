//! 工具调度核心（execute_tool dispatch + early_return_events）。
//!
//! 原 bot.rs 行 1195–1392 全部调度代码 +
//! 行 3140–3193 (early_return_events_tests) 测试段搬入。
//!
//! 调度主表（execute_tool_impl via TOOLS_TABLE lookup）分发 29 个 tool_* 函数到 bot::tools。
//! TOOLS_TABLE / tools_json() / mutating_tools() 都在 bot::registry（阶段 2 单源真相）。

use tauri::AppHandle;

use crate::bot::registry::{tools_index, ToolCtx, TOOLS_TABLE};
use crate::bot::tools::{broadcast_after_mutation, files_audit_kv};

// ───────────────────────── 工具调度核心（execute_tool dispatch） ─────────────────────────

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

/// 早退路径（pre_execute 拦截 / skill_on_step 熔断）的审计事件序列。
/// `tool.call` 已在入口发出，这里按写入顺序补齐后续事件并以 `tool.return` 配平，
/// 否则统计面板出现「悬挂调用」（call > return）。
/// 抽成纯函数：事件名 + kv + 顺序可单测（execute_tool 走泛型 Runtime + 注入（见 skill_e2e.rs 的 mock executor），
/// 已可 mock；但具体工具执行仍需真实 FS / subprocess / 网络）；调用点只负责逐条 emit。
/// - 拦截路径（err=None）：pre_execute.deny + tool.return(reason=denied)
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
    }
    events.push((
        crate::audit::AuditLevel::Warn,
        "tool.return",
        with_ctx(vec![
            ("tool", name.to_string()),
            ("reason", reason.to_string()),
            ("exit_code", "none".to_string()),
            ("duration_ms", dur_ms.to_string()),
        ]),
    ));
    events
}

/// 进程内执行工具，返回 (给模型的文本结果, 涉及的任务引用)
/// `pub` 让 `bot_skills::run_skill_scheduler`（DSL 调度器）可调用，
/// 不暴露给前端 — 通过 `is_atomic_tool` 黑名单 + pre-execute 校验保护。
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

#[allow(clippy::too_many_arguments)]
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
    // 1. 后置拦截：原子黑名单（老板拍板）
    //    仅作为 Skill 内部子步骤、不允许裸调的底层原子 Function → 硬锁阻断
    //    只有 Skill 在 Running 状态时才放行；其他时候直接返回错误 + 提示走对应 Skill
    // 抽象层：execute_tool 通过 middleware::run_pre_execute 调 pre-execute
    // 任务卡执行流程（StopGuard.allow_atomic）视同 Skill 上下文放行——
    // EXECUTE_SYSTEM_PROMPT 把 link_file_to_task 列为收尾动作，该流程没有 SkillRun，
    // 不放行则 prompt 要求的核心动作必被自家网关否决。
    // （create_word_revisions 不在原子黑名单，聊天/执行均可直调）
    let active = crate::tool_guard::is_skill_active(app, session_id)
        || stop.is_some_and(|s| s.allow_atomic());
    if let crate::middleware::ExecutionDecision::Deny { reason } =
        crate::middleware::run_pre_execute(app, name, active)
    {
        // tool.call 已发出，早退前必须配平 tool.return（reason=denied），
        // 否则统计面板出现「悬挂调用」（call > return）
        for (level, event, kv) in early_return_events(
            name,
            "denied",
            start.elapsed().as_millis() as u64,
            None,
            session_id,
            trace,
        ) {
            crate::audit::write_event(app, level, event, &kv);
        }
        // B1：denied 路径返回 Warn，与原 classify_text + tool_call_failed（⚠️ 检测）结果一致。
        return crate::bot::registry::ToolResult::warn(reason, Vec::new());
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
            // B1：skill_step_failed 路径返回 Warn，与原 classify_text + tool_call_failed 结果一致。
            return crate::bot::registry::ToolResult::warn(e.to_string(), Vec::new());
        }
    }
    let ctx = ToolCtx {
        app,
        stop,
        interactive,
        session_id,
    };
    // B1：工具仍返 (String, Vec<TaskRef>) 元组；From 过渡层默认 Ok。
    // B2 cleanup 会删除 From impl，工具届时改为显式 ok/warn/error。
    let result: crate::bot::registry::ToolResult = match tools_index().get(name).copied() {
        // T7：O(n) 线性扫描 → O(1) HashMap 查找。
        Some(t) => (t.call)(&ctx, args).await.into(),
        None => crate::bot::registry::ToolResult::error(format!("未知工具：{name}"), Vec::new()),
    };

    // 3. post-execute 洋葱管线「出」钩子
    //    - 结构化审计事件（工具名/耗时/返回引用数/结果预览）写到 bot.log
    //    - 级别从 result.status 派生（代替原 classify_text 字符串匹配）
    //    - 镜像调用 skill_on_step_post：技能步骤结果/失败检测
    let dur_ms = start.elapsed().as_millis() as u64;
    let level = crate::audit::AuditLevel::from_tool_status(result.status);
    // T6：保守 token 预算——仅 audit，不截断。超阈值的工具输出走 `tool.output.over_budget`
    // 事件供事后分析，默认阈值 8192，read_text_file/fetch_url/list_files 等长输出工具 65536。
    if let Some(t) = TOOLS_TABLE.iter().find(|t| t.name == name) {
        let output_len = result.text.chars().count();
        if output_len > t.max_output_chars {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "tool.output.over_budget",
                &[
                    ("tool", name.to_string()),
                    ("output_len", output_len.to_string()),
                    ("max", t.max_output_chars.to_string()),
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
    if name != "use_skill" {
        crate::bot_skills::skill_on_step_post(app, name, &result.text, dur_ms, level, session_id);
    }

    result
}

pub(crate) fn parse_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or(serde_json::Value::Null)
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
    fn deny_path_emits_deny_then_balanced_tool_return() {
        // 拦截路径：pre_execute.deny → tool.return（顺序敏感），含 reason=denied / exit_code=none / duration_ms
        let trace = ToolCallTrace {
            turn: Some(2),
            tool_call_id: Some("call_abc".into()),
        };
        let evs = early_return_events(
            "create_word_revisions",
            "denied",
            3,
            None,
            Some("sess-1"),
            &trace,
        );
        assert_eq!(event_names(&evs), ["pre_execute.deny", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "tool"), Some("create_word_revisions"));
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "tool"), Some("create_word_revisions"));
        assert_eq!(kv_get(ret, "reason"), Some("denied"));
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert_eq!(kv_get(ret, "duration_ms"), Some("3"));
        // 两条事件都带 trace，可整轮回放
        for ev in &evs {
            assert_eq!(kv_get(&ev.2, "session_id"), Some("sess-1"));
            assert_eq!(kv_get(&ev.2, "turn"), Some("2"));
            assert_eq!(kv_get(&ev.2, "tool_call_id"), Some("call_abc"));
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


// ───────────────────────── 工具层 commit template ─────────────────────────

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
/// **设计要点**:失败也走 `ToolResult::ok` 而非 `err`,让 LLM pipeline severity
/// classifier 不把"完成任务失败:DB error"误判为 fatal。
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
        Err(e) => crate::bot::registry::ToolResult::ok(
            format!("{fail_prefix}：{e}"),
            Vec::new(),
        ),
    }
}
