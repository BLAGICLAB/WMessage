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
    Box::pin(async move { crate::bot_desktop::tool_open_url(ctx.app, args).await })
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
