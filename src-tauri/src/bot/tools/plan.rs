//! 计划工具：`update_plan` —— 整表替换写会话内计划状态，并广播 `bot-plan` 事件。
//!
//! 为什么是工具而不是解析模型汇报：自然语言没有稳定结构，模型会漏报、
//! 重复报、描述与实际不符；漏报一次就等于面板上留一条永久错位的状态。
//! 专用工具让「进度」有唯一来源，也让状态机（至多一条 in_progress）
//! 能在后端硬校验，而不是靠模型自觉。
//!
//! 返回给模型的文本只留一行摘要：完整清单模型自己刚写出来过，
//! 再回灌一遍是白烧上下文。

use crate::bot::registry::ToolResult;

pub(crate) async fn tool_update_plan<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    args: &str,
    session_id: Option<&str>,
) -> ToolResult {
    let Some(session_id) = session_id.filter(|s| !s.is_empty()) else {
        return ToolResult::denied(
            "update_plan 需要会话上下文（计划挂在会话上）；本轮没有会话 id，无法记录计划。"
                .to_string(),
        );
    };
    let (steps, explanation) = match crate::bot::plan_state::parse_update_plan(args) {
        Ok(parsed) => parsed,
        Err(e) => return ToolResult::error(format!("update_plan 入参不合法：{e}"), Vec::new()),
    };
    let snapshot = crate::bot::plan_state::PlanSnapshot {
        steps,
        explanation,
        updated_at: crate::db::now_ms(),
    };
    crate::bot::plan_state::set_plan(session_id, snapshot);
    let view = crate::bot::plan_state::plan_view(session_id)
        .expect("刚写入的计划必然能读回：set_plan 之后同一会话表内取");
    // 走统一出口：分配序号 + 写会话缓冲 + 广播，sessionId 由前端按会话过滤。
    crate::bot::stream_log::emit_recorded(
        app,
        Some(session_id),
        "bot-plan",
        crate::bot::plan_state::plan_payload(&view),
    );
    ToolResult::ok(render_ack(&view), Vec::new())
}

/// 回给模型的一行摘要：状态分布 + 下一步提示。
fn render_ack(view: &crate::bot::plan_state::PlanView) -> String {
    use crate::bot::plan_state::PlanStepStatus;
    let mut done = 0usize;
    let mut doing = 0usize;
    for s in &view.steps {
        match s.status {
            PlanStepStatus::Completed => done += 1,
            PlanStepStatus::InProgress => doing += 1,
            PlanStepStatus::Pending => {}
        }
    }
    let mut out = format!(
        "计划已更新：共 {} 步（已完成 {done} / 进行中 {doing} / 未开始 {}）。",
        view.steps.len(),
        view.steps.len() - done - doing
    );
    if let Some(expl) = &view.explanation {
        out.push_str(&format!("变更说明：{expl}"));
    }
    out.push_str("完成一步就立刻再调一次更新状态，不要攒到最后一起补。");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::plan_state::PlanStepStatus;
    use crate::bot::registry::ToolStatus;

    /// 工具跑通整条路：写会话表 → 记 bot-plan 事件 → 回一行摘要给模型。
    #[tokio::test]
    async fn writes_snapshot_records_event_and_acks_model() {
        let app = tauri::test::mock_app();
        let sid = "plan-tool-e2e-session";
        crate::bot::plan_state::clear_plan(sid);
        // 缓冲只记在途轮次的事件（回合外不记）——工具总在回合内被调，先登记在途
        crate::bot::stream_log::begin(sid);
        let r = tool_update_plan(
            app.handle(),
            r#"{"plan":[{"step":"读文件","status":"completed"},{"step":"改代码","status":"in_progress"}]}"#,
            Some(sid),
        )
        .await;
        assert_eq!(r.status, ToolStatus::Ok);
        assert!(r.text.contains("2 步"), "摘要要带步数：{}", r.text);

        // 状态落会话表：面板用 bot_plan_state 取到的就是这份
        let view = crate::bot::plan_state::plan_view(sid).expect("计划应已写入");
        assert_eq!(view.steps.len(), 2);
        assert_eq!(view.steps[1].status, PlanStepStatus::InProgress);

        // 事件进会话缓冲：前端按 seq 补齐时拿回同一份
        let replay = crate::bot::stream_log::replay(sid, 0);
        let ev = replay
            .events
            .iter()
            .find(|e| e.event == "bot-plan")
            .expect("应记录 bot-plan 事件");
        // 信封必须带会话号：前端按会话过滤这条事件，缺了就静默丢掉
        // （面板于是不实时刷新，只在重新挂载拉快照时才对）
        assert_eq!(
            ev.payload["sessionId"],
            serde_json::json!(sid),
            "bot-plan 载荷必须带 sessionId"
        );
        assert_eq!(
            ev.payload["steps"][0]["status"],
            serde_json::json!("completed")
        );
        assert_eq!(
            ev.payload["steps"][1]["status"],
            serde_json::json!("in_progress")
        );
        crate::bot::stream_log::finish(sid);
        crate::bot::plan_state::clear_plan(sid);
    }

    /// 入参不合法 → 报错且不留下半份状态（面板不能显示一份看不懂的清单）
    #[tokio::test]
    async fn rejects_bad_args_without_leaving_state() {
        let app = tauri::test::mock_app();
        let sid = "plan-tool-bad-args";
        crate::bot::plan_state::clear_plan(sid);
        let r = tool_update_plan(
            app.handle(),
            r#"{"plan":[{"step":"a","status":"in_progress"},{"step":"b","status":"in_progress"}]}"#,
            Some(sid),
        )
        .await;
        assert_eq!(r.status, ToolStatus::Error);
        assert!(
            crate::bot::plan_state::get_plan(sid).is_none(),
            "入参不合法时不得留下半份状态"
        );
    }

    /// 没有会话 id → 门禁拒绝（计划挂在会话上，无处可挂）
    #[tokio::test]
    async fn without_session_is_denied() {
        let app = tauri::test::mock_app();
        let r = tool_update_plan(
            app.handle(),
            r#"{"plan":[{"step":"a","status":"pending"}]}"#,
            None,
        )
        .await;
        assert_eq!(r.status, ToolStatus::Denied);
    }
}
