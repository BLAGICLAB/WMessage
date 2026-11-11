//! 工具转发层：65 个 `call_*` 一行转发（`ToolCtx` + args → 工具实现）。

use super::{ToolCtx, ToolFuture};

use crate::bot::tools::{
    tool_add_subtask, tool_complete_task, tool_create_excel, tool_create_pdf, tool_create_ppt,
    tool_create_task, tool_create_word, tool_create_word_revisions, tool_delete_task,
    tool_edit_task, tool_extract_document, tool_fetch_url, tool_get_current_time,
    tool_link_file_to_task, tool_list_services, tool_query_single_task, tool_query_tasks,
    tool_remove_subtask, tool_run_python, tool_start_service, tool_stop_service,
    tool_toggle_subtask, tool_web_search,
};
use crate::bot_skills::{tool_create_skill, tool_use_skill};

// 适配器（统一签名，按需拆 ctx 字段）
// link_file_to_task 是 async fn，必须 .await——拆出来后已修正。
// list_tasks/search_tasks 合并为 query_tasks：收 args。
pub(super) fn call_query_tasks<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_query_tasks(ctx.app, args).await })
}

pub(super) fn call_query_single_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_query_single_task(ctx.app, args).await })
}

pub(super) fn call_create_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_task(ctx.app, args).await })
}

pub(super) fn call_complete_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_complete_task(ctx.app, args).await })
}

pub(super) fn call_delete_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move { tool_delete_task(ctx.app, args, interactive, session_id).await })
}

pub(super) fn call_edit_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_edit_task(ctx.app, args).await })
}

pub(super) fn call_add_subtask<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_add_subtask(ctx.app, args).await })
}

pub(super) fn call_toggle_subtask<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_toggle_subtask(ctx.app, args).await })
}

pub(super) fn call_remove_subtask<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_remove_subtask(ctx.app, args).await })
}

pub(super) fn call_read_text_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move {
        crate::bot_fs::tool_read_text_file(ctx.app, args, interactive, session_id).await
    })
}

pub(super) fn call_ocr_image<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::ocr::tool_ocr_image(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_grep_files<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_grep_files(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_list_files<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_list_files(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_link_file_to_task<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { tool_link_file_to_task(ctx.app, args, session_id).await })
}

pub(super) fn call_extract_document<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move { tool_extract_document(ctx.app, args, interactive, session_id).await })
}

pub(super) fn call_create_word<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_word(ctx.app, args).await })
}

pub(super) fn call_create_word_revisions<'a>(
    ctx: &'a ToolCtx<'a>,
    args: &'a str,
) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { tool_create_word_revisions(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_create_excel<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_excel(ctx.app, args).await })
}

pub(super) fn call_create_ppt<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_ppt(ctx.app, args).await })
}

pub(super) fn call_create_pdf<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_pdf(ctx.app, args).await })
}

pub(super) fn call_run_python<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let stop = ctx.stop;
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move { tool_run_python(ctx.app, args, stop, interactive, session_id).await })
}

pub(super) fn call_start_service<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    let app = ctx.app;
    // 权限档与路径闸住在 Wry 层（bot-config 读写、bot_fs 授权弹窗都是 Wry 专属），
    // 这里算好传进去：工具本体因此不绑 Runtime，能在 mock runtime 下被真调用
    // （与调度器 core 注入 executor 同款思路）。
    let yolo = crate::bot::perm_mode(app) == crate::bot::PermMode::Yolo;
    // 闭包自带所有权：注入的 future 必须是 'static（工具本体不绑 Runtime）
    let app_for_cwd = app.clone();
    let sid_for_cwd = session_id.map(|s| s.to_string());
    Box::pin(async move {
        tool_start_service(app, args, session_id, yolo, move |p| {
            let app = app_for_cwd.clone();
            let sid = sid_for_cwd.clone();
            Box::pin(async move {
                crate::bot_fs::resolve_with_perm(
                    &app,
                    "start_service",
                    &p,
                    interactive,
                    sid.as_deref(),
                )
                .await
            })
        })
        .await
    })
}

pub(super) fn call_stop_service<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_stop_service(ctx.app, args).await })
}

pub(super) fn call_list_services<'a>(ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_list_services(ctx.app) })
}

pub(super) fn call_ast_inspect<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    let stop = ctx.stop;
    Box::pin(async move {
        crate::py::ast::tool_ast_inspect(ctx.app, args, interactive, session_id, stop).await
    })
}

pub(super) fn call_unpack_archive<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let interactive = ctx.interactive;
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_archive::tool_unpack_archive(&app, &args, interactive, session_id.as_deref())
            .await
    })
}

pub(super) fn call_pack_archive<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let interactive = ctx.interactive;
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_archive::tool_pack_archive(&app, &args, interactive, session_id.as_deref()).await
    })
}

pub(super) fn call_manage_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let interactive = ctx.interactive;
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_manage_file::tool_manage_file(&app, &args, interactive, session_id.as_deref())
            .await
    })
}

pub(super) fn call_web_search<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_web_search(ctx.app, args).await })
}

pub(super) fn call_fetch_url<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_fetch_url(ctx.app, args).await })
}

pub(super) fn call_http_request<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move {
        crate::bot::tools::tool_http_request(ctx.app, args, ctx.interactive, ctx.session_id).await
    })
}

pub(super) fn call_lookup_receipt<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { crate::bot::tools::tool_lookup_receipt(ctx.app, args, session_id).await })
}

pub(super) fn call_get_current_time<'a>(_ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_get_current_time() })
}

pub(super) fn call_remember_fact<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::memory::tool_remember_fact(ctx.app, args).await })
}

pub(super) fn call_recall_facts<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::memory::tool_recall_facts(ctx.app, args).await })
}

pub(super) fn call_record_lesson<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::memory::tool_record_lesson(ctx.app, args).await })
}

/// 任务执行提问——引擎在 ask_engine（oneshot 等待 + 假设兜底），
/// 超时口径 ASK_TIMEOUT_SECS（默认 24h），测试可传短超时直打引擎。
/// ctx.stop 透传/stop 必须能打断 24h 等待，不能挂死模型循环）。
pub(super) fn call_ask_user<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app;
    let session_id = ctx.session_id;
    let stop = ctx.stop;
    Box::pin(async move {
        crate::ask_engine::engine_ask_user(
            app,
            session_id,
            args,
            std::time::Duration::from_secs(crate::ask_engine::ASK_TIMEOUT_SECS),
            stop,
        )
        .await
    })
}

pub(super) fn call_create_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { tool_create_skill(ctx.app, args, ctx.interactive, ctx.session_id).await })
}

pub(super) fn call_install_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move {
        crate::bot_skills::tool_install_skill(ctx.app, args, ctx.interactive, ctx.session_id).await
    })
}

pub(super) fn call_search_skills<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_skills::tool_search_skills(ctx.app, args).await })
}

pub(super) fn call_apps_list<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::apps::manage::tool_apps_list(ctx.app, args).await })
}

pub(super) fn call_apps_create<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::apps::manage::tool_apps_create(ctx.app, args).await })
}

pub(super) fn call_apps_update_files<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::apps::manage::tool_apps_update_files(ctx.app, args).await })
}

pub(super) fn call_use_skill<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { tool_use_skill(ctx.app, args, session_id) })
}

// 适配器：编排三工具 + 子 agent 白名单工具
pub(super) fn call_spawn_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(
        async move { crate::bot_orchestrator::tool_spawn_subagent(&app, &args, session_id).await },
    )
}

pub(super) fn call_check_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    // 等待期要能被 /stop 打断（waitMs 上限已到 10 分钟，不接就是锁死停止键）：
    // StopGuard 借用活不过闭包，取 owned 令牌下传。
    let stop = ctx.stop.map(|g| g.token());
    // 会话 id 跟着走：问到终态要记进「父会话已吸收」去重表，否则 digest 与
    // 自动汇报轮会把同一份结果再念一遍。
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_orchestrator::tool_check_subagent(&app, &args, stop, session_id).await
    })
}

pub(super) fn call_cancel_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(
        async move { crate::bot_orchestrator::tool_cancel_subagent(&app, &args, session_id).await },
    )
}

pub(super) fn call_resume_subagent<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(
        async move { crate::bot_orchestrator::tool_resume_subagent(&app, &args, session_id).await },
    )
}

pub(super) fn call_write_artifact_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let args = args.to_string();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move {
        crate::bot_orchestrator::tool_write_artifact_file(&app, &args, session_id).await
    })
}

pub(super) fn call_read_own_card<'a>(ctx: &'a ToolCtx<'a>, _args: &'a str) -> ToolFuture<'a> {
    let app = ctx.app.clone();
    let session_id = ctx.session_id.map(|s| s.to_string());
    Box::pin(async move { crate::bot_orchestrator::tool_read_own_card(&app, session_id).await })
}

// 电脑辅助 Tier1 适配器
pub(super) fn call_reveal_path<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move {
        crate::bot_desktop::tool_reveal_path(ctx.app, args, interactive, session_id).await
    })
}

pub(super) fn call_open_url<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_open_url(ctx.app, args, ctx.interactive).await })
}

pub(super) fn call_clipboard_write<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_clipboard_write(ctx.app, args).await })
}

pub(super) fn call_copy_file_to_clipboard<'a>(
    ctx: &'a ToolCtx<'a>,
    args: &'a str,
) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(async move {
        crate::bot_desktop::tool_copy_file_to_clipboard(ctx.app, args, interactive, session_id)
            .await
    })
}

pub(super) fn call_schedule_reminder<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_reminders::tool_schedule_reminder(ctx.app, args).await })
}

pub(super) fn call_list_reminders<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_reminders::tool_list_reminders(ctx.app, args).await })
}

pub(super) fn call_cancel_reminder<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_reminders::tool_cancel_reminder(ctx.app, args).await })
}

pub(super) fn call_list_windows<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_list_windows(ctx.app, args).await })
}

pub(super) fn call_activate_window<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_activate_window(ctx.app, args).await })
}

pub(super) fn call_ui_inspect<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_ui_inspect(ctx.app, args).await })
}

pub(super) fn call_ui_click<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_ui::tool_ui_click(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_ui_type<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_ui::tool_ui_type(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_wait_for_element<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_ui::tool_wait_for_element(ctx.app, args).await })
}

pub(super) fn call_screenshot<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    Box::pin(async move { crate::bot_desktop::tool_screenshot(ctx.app, args).await })
}

// 文件编辑适配器
pub(super) fn call_edit_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_edit_file(ctx.app, args, interactive, session_id).await },
    )
}

pub(super) fn call_write_file<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let interactive = ctx.interactive;
    let session_id = ctx.session_id;
    Box::pin(
        async move { crate::bot_fs::tool_write_file(ctx.app, args, interactive, session_id).await },
    )
}

// 计划工具适配器
pub(super) fn call_update_plan<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { crate::bot::tools::tool_update_plan(ctx.app, args, session_id).await })
}

// 组挂载元工具适配器（差距收编方案 D-3）：纯内存 store 写入，无 IO
pub(super) fn call_activate_tools<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    Box::pin(async move { crate::bot::registry::groups::tool_activate_tools(session_id, args) })
}

/// 终局提交（二期乙）：校验参数后把收尾 JSON 原文作为结果返回。模型循环
/// 见「成功 + 本工具」即以该 JSON 结束回合——此处做会话闸（仅子 agent 执行
/// 会话可交卷，与 write_artifact_file / read_own_card 同一面）+ 参数准入校验
/// （对象形态 / status 枚举 / summary 非空），其余键（acceptance_check /
/// checks / artifacts / blockers 及任何额外键）原样透传，verifier 的 checks
/// 键由收尾分类器按结构识别。
pub(super) fn call_submit_result<'a>(ctx: &'a ToolCtx<'a>, args: &'a str) -> ToolFuture<'a> {
    let session_id = ctx.session_id;
    // 本体无 await：闸与校验都是同步短路径，收敛到不绑 Runtime 的入口
    //（测试可直驱，与「工具本体不绑 Runtime」同口径）
    Box::pin(async move { submit_result_checked(session_id, args) })
}

/// submit_result 本体（不绑 Runtime）：会话闸 + 参数准入校验。
fn submit_result_checked(session_id: Option<&str>, args: &str) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if !crate::tool_guard::is_subagent_session(session_id) {
        return ToolResult::error("submit_result 仅子 agent 执行会话可用。", Vec::new());
    }
    match validate_submit_args(args) {
        Ok(canonical) => ToolResult::ok(canonical, Vec::new()),
        Err(e) => ToolResult::error(e, Vec::new()),
    }
}

/// 终局提交参数校验（纯函数，单测不打 ToolCtx）：
/// 对象形态 / status 枚举 / summary 非空三道，其余键原样透传。
fn validate_submit_args(args: &str) -> Result<String, String> {
    const ALLOWED_STATUS: [&str; 4] = ["succeeded", "failed", "cancelled", "budget_exceeded"];
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("收尾参数不是合法 JSON：{e}"))?;
    if !v.is_object() {
        return Err("收尾参数必须是 JSON 对象（{\"status\":…,\"summary\":…}）".into());
    }
    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
    if !ALLOWED_STATUS.contains(&status) {
        return Err(format!(
            "status 必须只取 succeeded/failed/cancelled/budget_exceeded，收到「{status}」"
        ));
    }
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
    if summary.trim().is_empty() {
        return Err("summary 必填且不能为空（结论摘要，不倒原文）".into());
    }
    // 规范化序列化（键序归一、空白收敛）后原样交回——模型循环拿它当
    // 回合最终正文，收尾分类器按结构识别 status / checks。
    Ok(v.to_string())
}

#[cfg(test)]
mod submit_result_tests {
    use super::validate_submit_args;

    #[test]
    fn valid_args_pass_through_canonical() {
        let out = validate_submit_args(
            r#"{"status":"succeeded" , "summary":"ok","checks":[{"criterion":"C1","met":false}]}"#,
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["status"], "succeeded");
        assert_eq!(v["checks"][0]["met"], false, "checks 原样透传");
    }

    /// 会话闸：submit_result 只认子 agent 执行会话。主会话模型硬猜工具名调
    /// 过来必须在工具体被拒——否则收尾 JSON 会以 Submitted 终结主回合。
    #[test]
    fn submit_result_denied_outside_subagent_sessions() {
        let args = r#"{"status":"succeeded","summary":"done"}"#;
        let out = super::submit_result_checked(Some("sess-submit-gate-main"), args);
        assert!(!out.status.is_success(), "主会话调用必须被拒");
        assert!(out.text.contains("仅子 agent 执行会话可用"), "{}", out.text);

        // 已注册的子 agent 执行会话：闸放行，参数校验照常通过
        let sid = "sess-submit-gate-sub";
        crate::tool_guard::register_subagent_session(sid, submit_gate_ctx());
        let out2 = super::submit_result_checked(Some(sid), args);
        assert!(
            out2.status.is_success(),
            "子 agent 会话应放行：{}",
            out2.text
        );
        crate::tool_guard::unregister_subagent_session(sid);
    }

    /// 参数不合法时即便会话闸放行也照旧报错（闸不替代参数校验）
    #[test]
    fn submit_result_keeps_param_validation_inside_subagent_sessions() {
        let sid = "sess-submit-gate-params";
        crate::tool_guard::register_subagent_session(sid, submit_gate_ctx());
        let out = super::submit_result_checked(Some(sid), r#"{"status":"done","summary":"x"}"#);
        assert!(!out.status.is_success());
        assert!(out.text.contains("status 必须"), "{}", out.text);
        crate::tool_guard::unregister_subagent_session(sid);
    }

    fn submit_gate_ctx() -> crate::tool_guard::SubagentSessionCtx {
        use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
        use std::sync::Arc;
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_submit_gate".into(),
            task_id: None,
            profile: crate::db::SubagentProfile::Research,
            tools: Arc::new(Vec::new()),
            artifact_dir: std::path::PathBuf::from("/tmp/sa_submit_gate"),
            acceptance_criteria_count: 0,
            used_tool_calls: Arc::new(AtomicUsize::new(0)),
            used_rounds: Arc::new(AtomicUsize::new(0)),
            last_progress_ms: Arc::new(AtomicU64::new(0)),
            tool_in_flight: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn invalid_args_are_named_errors() {
        // 非法 JSON
        let e = validate_submit_args("{oops").unwrap_err();
        assert!(e.contains("不是合法 JSON"));
        // 非 object
        let e = validate_submit_args("[1,2]").unwrap_err();
        assert!(e.contains("JSON 对象"));
        // status 越枚举
        let e = validate_submit_args(r#"{"status":"done","summary":"x"}"#).unwrap_err();
        assert!(e.contains("status 必须"));
        // summary 空
        let e = validate_submit_args(r#"{"status":"succeeded","summary":"  "}"#).unwrap_err();
        assert!(e.contains("summary 必填"));
    }
}
