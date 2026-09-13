//! 工具调度核心（execute_tool dispatch + early_return_events）。
//!
//! 原 bot.rs 行 1195–1392 全部调度代码 +
//! 行 3140–3193 (early_return_events_tests) 测试段搬入。
//!
//! 调度主表（execute_tool_impl via TOOLS_TABLE lookup）分发 29 个 tool_* 函数到 bot::tools。
//! TOOLS_TABLE / tools_json() / mutating_tools() 都在 bot::registry（阶段 2 单源真相）。

use tauri::AppHandle;

use crate::bot::registry::{ToolCtx, TOOLS_TABLE};
use crate::bot::tools::files_audit_kv;

// ───────────────────────── 工具调度核心（execute_tool dispatch） ─────────────────────────

/// 早退路径（pre_execute 拦截 / skill_on_step 熔断）的审计事件序列。
/// `tool.call` 已在入口发出，这里按写入顺序补齐后续事件并以 `tool.return` 配平，
/// 否则统计面板出现「悬挂调用」（call > return）。
/// 抽成纯函数：事件名 + kv + 顺序可单测（execute_tool 是 Wry 签名，无法 mock runtime 直调，
/// 与 skill_e2e.rs 注释记录的「泛型化重构暂缓」一致）；调用点只负责逐条 emit。
/// - 拦截路径（err=None）：pre_execute.deny + tool.return(reason=denied)
/// - 熔断路径（err=Some）：skill_on_step_error（补 Warn 可见性）+ tool.return(reason=skill_step_failed)
fn early_return_events(
    name: &str,
    reason: &str,
    dur_ms: u64,
    err: Option<&str>,
) -> Vec<(
    crate::audit::AuditLevel,
    &'static str,
    Vec<(&'static str, String)>,
)> {
    let mut events = Vec::with_capacity(2);
    match err {
        Some(e) => events.push((
            crate::audit::AuditLevel::Warn,
            "skill_on_step_error",
            vec![("tool", name.to_string()), ("err", e.to_string())],
        )),
        None => events.push((
            crate::audit::AuditLevel::Warn,
            "pre_execute.deny",
            vec![("tool", name.to_string())],
        )),
    }
    events.push((
        crate::audit::AuditLevel::Warn,
        "tool.return",
        vec![
            ("tool", name.to_string()),
            ("reason", reason.to_string()),
            ("exit_code", "none".to_string()),
            ("duration_ms", dur_ms.to_string()),
        ],
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
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    execute_tool_impl(app, name, args, None, true, session_id).await
}

/// execute_tool 的可停止版本：携带 /stop 守卫，run_python 等长耗时工具
/// 在执行中即可被中断；Skill 调度器等无守卫调用方走 execute_tool（stop=None）。
pub async fn execute_tool_with_stop(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    // 会话隔离：交互属性与会话归属从 StopGuard 取（无守卫 = 后台调度器路径
    // 不会出现——调度器走 run_task_in_chat 也持 StopGuard；None 仅 DSL 调度器遗留路径）
    let interactive = stop.map(|s| s.is_interactive()).unwrap_or(true);
    let session_id = stop.and_then(|s| s.session_id());
    execute_tool_impl(app, name, args, stop, interactive, session_id).await
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_impl(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let start = std::time::Instant::now();
    // 0. tool.call 结构化
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "tool.call",
        "tool" => name,
        "args_preview" => args.chars().take(80).collect::<String>(),
    );
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
    if let Some(msg) = crate::middleware::run_pre_execute(app, name, active) {
        // tool.call 已发出，早退前必须配平 tool.return（reason=denied），
        // 否则统计面板出现「悬挂调用」（call > return）
        for (level, event, kv) in
            early_return_events(name, "denied", start.elapsed().as_millis() as u64, None)
        {
            crate::audit::write_event(app, level, event, &kv);
        }
        return (msg, Vec::new());
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
            ) {
                crate::audit::write_event(app, level, event, &kv);
            }
            return (e.into(), Vec::new());
        }
    }
    let ctx = ToolCtx {
        app,
        stop,
        interactive,
        session_id,
    };
    let (text, refs): (String, Vec<crate::bot_chat::TaskRef>) =
        match TOOLS_TABLE.iter().find(|t| t.name == name) {
            Some(t) => (t.call)(&ctx, args).await,
            None => (format!("未知工具：{name}"), Vec::new()),
        };

    // 3. post-execute 洋葱管线「出」钩子
    //    - 结构化审计事件（工具名/耗时/返回引用数/结果预览）写到 bot.log
    //    - 失败分类：未知工具→Error；含「失败/错误/error:」→Warn；其他→Info
    //    - 镜像调用 skill_on_step_post：技能步骤结果/失败检测
    let dur_ms = start.elapsed().as_millis() as u64;
    let level = crate::audit::classify_text(name, &text);
    // create_task/edit_task 带 files 参数时补 files_count/truncated kv（多文件绑定）
    let mut kv: Vec<(&str, String)> = vec![
        ("tool", name.to_string()),
        ("ms", dur_ms.to_string()),
        ("refs", refs.len().to_string()),
        ("preview", text.chars().take(80).collect::<String>()),
    ];
    if matches!(name, "create_task" | "edit_task") {
        if let Some((cnt, truncated)) = files_audit_kv(args) {
            kv.push(("files_count", cnt.to_string()));
            kv.push(("truncated", truncated.to_string()));
        }
    }
    crate::audit::write_event(app, level, "tool.return", &kv);
    if name != "use_skill" {
        crate::bot_skills::skill_on_step_post(app, name, &text, dur_ms, level, session_id);
    }

    (text, refs)
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
        let evs = early_return_events("create_word_revisions", "denied", 3, None);
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
    }

    #[test]
    fn skill_step_error_path_warns_then_balanced_tool_return() {
        // 熔断路径：skill_on_step_error（补 Warn 可见性）→ tool.return(reason=skill_step_failed)
        let evs = early_return_events(
            "run_python",
            "skill_step_failed",
            5,
            Some("超过最大步数上限（8 步）"),
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
}
