//! 子任务汇报轮：回传结果、门控、历史映射。

use super::*;

/// 汇报轮的结局：跑完 / 撞上别的回合 / 报错。
pub(crate) enum ReportOutcome {
    Ran,
    /// 主会话这一轮还没结束（`bot_chat` 的会话锁挡住了）
    Busy,
    Failed,
}

/// 子任务收尾汇报轮：主会话空闲时由后端主动起的一轮对话，没有用户消息。
///
/// 复用 `bot_chat` 的主链路而不是另写一条，是为了和主聊天共用同一套提示词装配、
/// 历史截断、记忆注入与 digest 注入——另起一份必然随主链路漂移。
/// 面向用户输入的步骤（规范化审计、挂起恢复、纠正检测、技能推荐、pre-step 路由、
/// 计划生成）在 `TurnOrigin::Report` 下由 `user_input` 统一空转，不需要各自分支。
pub(crate) async fn report_subagent_result(app: &AppHandle, session_id: &str) -> ReportOutcome {
    let history = report_turn_history(app, session_id).await;
    let out = match bot_chat_inner(
        app.clone(),
        history,
        Some(session_id.to_string()),
        None,
        TurnOrigin::Report,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "subagent_report_failed",
                "error" => crate::bot::truncate_for_log(&e.message(), 200),
            );
            return ReportOutcome::Failed;
        }
    };
    if out.result.busy {
        return ReportOutcome::Busy;
    }
    if out.result.text.trim().is_empty() {
        // 正文为空 = 什么都没说出来：不记已吸收，重试时那段回报还在 digest 里
        return ReportOutcome::Failed;
    }
    // 送达 = 落库成功。汇报轮没有前端快照，它的历史下一轮从库里读——回复没
    // 落库就等于没说出来，用户那边看不到，模型下一轮也读不到这段汇报。
    match persist_report_reply(app, session_id, &out.result).await {
        Ok(()) => {
            // 送达成功 → 现在才算已吸收（digest 不再重念这段回报）
            for id in &out.absorb {
                crate::bot_orchestrator::mark_digested(Some(session_id), id);
            }
            ReportOutcome::Ran
        }
        Err(_) => {
            // 没送达 → 不记已吸收：重试时那段回报还在 digest 里，模型能重说一遍。
            // 记了就是「已经汇报过了」，重试也只会空转一轮，子任务结果就此消失。
            ReportOutcome::Failed
        }
    }
}

/// 汇报轮带的历史：库里已落盘的消息行。
///
/// 为什么必须带历史：对话历史不在 `bot_chat` 里读库，而是前端整段传进来；汇报轮
/// 没有前端，就只能自己读。传空历史的后果不是「模型看不到上文」这么轻——请求里
/// 只剩 system 消息，转成 Anthropic 协议时 system 全被抽走，messages 归零，
/// 以「没有可发送的消息」硬失败，用户端表现是子任务跑完了却永远没人转述。
pub(super) fn report_turn_history_from_rows(rows: Vec<crate::db::BotMsgRow>) -> Vec<ChatMsg> {
    let mut msgs: Vec<ChatMsg> = rows
        .into_iter()
        .map(|r| ChatMsg {
            role: r.role,
            content: r.content,
        })
        .collect();
    if msgs.is_empty() {
        // 派发方会话刚落库、首条用户消息的保存还没落地时会走到这里（汇报轮在
        // 主会话锁释放后触发，而历史是前端在收到答复后才写的，两者没有时序保证）。
        msgs.push(ChatMsg {
            role: "user".into(),
            content: REPORT_TURN_STUB.into(),
        });
    }
    msgs
}

/// 汇报轮历史的取数薄壳：读库失败（会话被删、DB 不可用）不阻断汇报，退回占位消息。
pub(super) async fn report_turn_history(app: &AppHandle, session_id: &str) -> Vec<ChatMsg> {
    let rows = match crate::db::bot_history_load(app.clone(), session_id.to_string()).await {
        Ok(rows) => rows,
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "subagent_report.history_load_failed",
                "error" => crate::bot::truncate_for_log(&e.message(), 200),
            );
            Vec::new()
        }
    };
    report_turn_history_from_rows(rows)
}

/// 汇报轮回复的落地：追加落库 + 通知前端。
///
/// 不走前端的全量覆盖存历史：汇报轮没有前端快照，前端那份在下一条用户消息
/// 才覆写；这一轮必须自己落库，否则用户不发言就重启，汇报内容就凭空消失。
///
/// 返回 Err = 没送达：调用方据此不记「已吸收」并重试（这段汇报是子任务结果
/// 唯一的出口，丢了就是永久丢失）。
pub(super) async fn persist_report_reply(
    app: &AppHandle,
    session_id: &str,
    result: &BotChatResult,
) -> Result<(), String> {
    let sid = session_id.to_string();
    let content = result.text.clone();
    let refs = serde_json::to_string(&result.task_refs).ok();
    let r = crate::db::lock::with_write_conn(app, move |conn| {
        crate::db::bot_history_append_inner(
            conn,
            &sid,
            &crate::db::BotMsgRow {
                role: "assistant".into(),
                content,
                refs_json: refs,
                thinking: None,
                tools_json: None,
                turn_status: None,
            },
        )
    })
    .await;
    if let Err(e) = r {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "subagent_report.persist_failed",
            "error" => e,
        );
        return Err(e);
    }
    crate::bot::stream_log::emit_recorded(
        app,
        Some(session_id),
        "bot-chat-report",
        serde_json::json!({ "sessionId": session_id, "text": result.text }),
    );
    Ok(())
}
