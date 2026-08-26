//! 任务卡逐步执行模式（2026-08-19 老板拍板）：
//! 多子任务卡手动「交给机器人」（🤖 / bot_execute_task）→ 子任务一个一个做；
//! 每个做完把结果发回聊天，等用户确认：
//! - 「继续」→ 勾选该子任务（系统直接落库，不经 LLM），做下一个
//! - 「重做」+ 修改意见（或任意其它内容）→ 带着意见重做当前子任务
//! - 「停」→ 结束执行，已勾选的保持现状
//! 全部做完只汇报、不勾任务卡完成状态（由用户最终确认）。
//!
//! 边界：聊天批量执行（pre-step 路由 RouteAction::ExecuteTasks，经 ChatExecuteMiddleware 命中）
//! 与定时调度（interactive=false）不走本模式，整卡连续做完（多卡/无人在场场景不适合逐步确认）。
//!
//! 状态只存内存（task_id + 当前待确认子任务 id），进程退出即丢；
//! 每步上下文从 DB 重读重建，不保留 LLM 历史（省 token、子任务勾选状态永远新鲜）。

use std::sync::{Mutex, OnceLock};

use serde_json::json;
use tauri::AppHandle;

use crate::bot_chat::{BotChatResult, ExecGuard};
use crate::bot_slash::StopGuard;
use crate::error::{CommandError, CommandResult};

/// 挂起的逐步执行：等用户确认当前子任务（全局最多一个，新执行覆盖旧的）
/// 2026-08-26 会话隔离：记录归属会话 id——别的会话的消息不会被当成
/// 逐步执行的应答截胡（has_pending 按会话匹配）
struct PendingExec {
    task_id: String,
    subtask_id: String,
    /// 触发会话 id（2026-08-26 会话隔离）
    session_id: Option<String>,
}

static PENDING: OnceLock<Mutex<Option<PendingExec>>> = OnceLock::new();

fn pending_slot() -> &'static Mutex<Option<PendingExec>> {
    PENDING.get_or_init(|| Mutex::new(None))
}

/// 当前会话是否有挂起的逐步执行（2026-08-26 起按会话匹配：
/// 会话 A 挂起时，会话 B 的消息走正常聊天路由，不被 resume 截胡）
pub fn has_pending_for(session_id: Option<&str>) -> bool {
    pending_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|p| p.session_id.as_deref() == session_id)
}

fn park(p: PendingExec) {
    *pending_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

fn take_pending() -> Option<PendingExec> {
    pending_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// 结束/清空挂起（正常结束、用户喊停、被新执行覆盖）；恢复任务卡用户头像
pub async fn clear(app: &AppHandle, reason: &str) {
    if let Some(p) = take_pending() {
        crate::bot::audit_log(app, &format!("exec_steps.clear | task: {} | {reason}", p.task_id));
        crate::bot_chat::set_bot_assigned(app, &p.task_id, false).await;
    }
}

/// 用户应答分类（纯函数便于单测）。
/// 判定顺序：停 → 重做 → 继续 → 默认按「修改意见」重做（用户在确认语境下的自由文本
/// 大概率是对本步结果的反馈）。
#[derive(Debug, PartialEq, Eq)]
pub enum StepReply {
    Continue,
    Redo(String),
    Stop,
}

pub fn classify_reply(text: &str) -> StepReply {
    let t = text.trim();
    const STOPS: [&str; 6] = ["停", "别做", "不做了", "算了", "结束", "取消执行"];
    if STOPS.iter().any(|k| t.starts_with(k))
        || t.trim_start_matches('/').eq_ignore_ascii_case("stop")
    {
        return StepReply::Stop;
    }
    const REDOS: [&str; 5] = ["重做", "重来", "重新做", "重新来", "再做"];
    for k in REDOS {
        if let Some(rest) = t.strip_prefix(k) {
            let fb = rest.trim_matches(|c: char| matches!(c, '，' | ',' | '：' | ':' | ' '));
            return StepReply::Redo(fb.to_string());
        }
    }
    const CONTINUES: [&str; 7] = ["继续", "好了", "可以", "行", "下一", "没问题", "嗯"];
    if CONTINUES.iter().any(|k| t.starts_with(k))
        || t.eq_ignore_ascii_case("ok")
        || t.eq_ignore_ascii_case("continue")
        || t.eq_ignore_ascii_case("go")
    {
        return StepReply::Continue;
    }
    StepReply::Redo(t.to_string())
}

/// 从 DB 重读任务卡（每步重读：用户可能在确认期间手动改过卡）
async fn load_task(app: &AppHandle, task_id: &str) -> CommandResult<crate::db::Task> {
    crate::db::db_load(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
        .ok_or_else(|| CommandError::TaskInvalidState {
            reason: "任务卡不存在或已删除".into(),
        })
}

/// 勾选当前子任务：系统直接落库（确定性动作，不经 LLM），广播 tasks-updated 同步三端
async fn mark_subtask_done(app: &AppHandle, task_id: &str, subtask_id: &str) {
    let Ok(all) = crate::db::db_load(app.clone()).await else {
        return;
    };
    let Some(mut t) = all
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
    else {
        return;
    };
    let done_now = t
        .subtasks
        .as_mut()
        .and_then(|subs| subs.iter_mut().find(|s| s.id == subtask_id && !s.done))
        .map(|s| {
            s.done = true;
            s.text.clone()
        });
    let Some(text) = done_now else { return };
    t.updated_at = Some(chrono::Utc::now().timestamp_millis());
    if crate::db::db_upsert(app.clone(), vec![t.clone()]).await.is_ok() {
        crate::bot::audit_log(
            app,
            &format!("exec_steps.subtask_done | task: {task_id} | 已勾选「{}」", crate::bot::truncate_for_log(&text, 60)),
        );
        crate::bot::broadcast_after_mutation(app, vec![t], vec![]);
    }
}

/// 跑一个子任务：组上下文 → 模型循环 → 挂起等确认
async fn run_step(
    app: &AppHandle,
    task_id: &str,
    subtask_id: &str,
    feedback: Option<&str>,
    stop: &StopGuard,
) -> CommandResult<BotChatResult> {
    let task = load_task(app, task_id).await?;
    let subs = task.subtasks.clone().unwrap_or_default();
    let total = subs.len();
    let done_count = subs.iter().filter(|s| s.done).count();
    // 目标子任务已被用户手动勾掉/删除 → 顺移到下一个未勾
    let Some(sub) = subs.iter().find(|s| s.id == subtask_id && !s.done) else {
        return Box::pin(advance_or_finish(app, task_id, stop)).await;
    };
    let sys = format!(
        "{}\n{}\n\n{}",
        crate::bot_chat::EXECUTE_SYSTEM_PROMPT,
        crate::bot_chat::STEPWISE_ADDENDUM,
        crate::bot_skills::build_skill_block(app)
    );
    let mut block = crate::bot_chat::build_task_block(&task);
    block.push_str(&format!(
        "\n\n【逐步执行】本轮只做子任务「{}」（进度 {}/{}）。做完后用一两句话汇报你做了什么、结果/产物在哪。",
        sub.text,
        done_count + 1,
        total
    ));
    if let Some(fb) = feedback.filter(|f| !f.trim().is_empty()) {
        block.push_str(&format!("\n【用户对这一步的修改意见】{fb}"));
    }
    let msgs = vec![
        json!({"role": "system", "content": sys}),
        json!({"role": "user", "content": block}),
    ];
    let (text, refs) = crate::bot_model_loop::run_model_loop(
        app.clone(),
        msgs,
        crate::bot_model_loop::DEFAULT_MAX_ROUNDS,
        stop,
    )
    .await?;
    park(PendingExec {
        task_id: task_id.to_string(),
        subtask_id: sub.id.clone(),
        session_id: stop.session_id().map(|s| s.to_string()),
    });
    Ok(BotChatResult {
        text: format!(
            "{text}\n\n———\n✅ 子任务「{}」做完了（{}/{}）。回复：\n• 「继续」→ 勾选它，做下一个\n• 「重做」+ 修改意见 → 重做这一步\n• 「停」→ 结束执行",
            sub.text,
            done_count + 1,
            total
        ),
        task_refs: refs,
    })
}

/// 继续：找下一个未勾子任务；没有了就收尾汇报（不勾任务卡完成）
async fn advance_or_finish(
    app: &AppHandle,
    task_id: &str,
    stop: &StopGuard,
) -> CommandResult<BotChatResult> {
    let task = load_task(app, task_id).await?;
    let subs = task.subtasks.clone().unwrap_or_default();
    if let Some(next) = subs.iter().find(|s| !s.done) {
        // Box::pin：run_step ↔ advance_or_finish 互调是异步递归，Rust 要求显式装箱
        return Box::pin(run_step(app, task_id, &next.id.clone(), None, stop)).await;
    }
    clear(app, "全部子任务完成").await;
    Ok(BotChatResult {
        text: "🎉 所有子任务都已完成并勾选。任务卡本身我没有标记完成——你确认没问题后自己勾完成，或跟我说「完成它」。".into(),
        task_refs: vec![],
    })
}

/// 开始逐步执行（bot_execute_task 在 ≥2 个未勾子任务时分流到这里）
pub async fn start(app: &AppHandle, task: &crate::db::Task, session_id: Option<&str>) -> CommandResult<BotChatResult> {
    // 防重入：与 execute_task_core 同一守卫（同一卡不能同时两个执行实例）
    let Some(_guard) = ExecGuard::acquire(&task.id) else {
        crate::bot::audit_log(
            app,
            &format!("execute_task_rejected | id: {} | 已有执行实例在跑（防重入拦截）", task.id),
        );
        return Err(CommandError::TaskInvalidState {
            reason: "该任务卡正在执行中，请等待完成后再触发".into(),
        });
    };
    if has_pending_for(session_id) {
        clear(app, "新任务卡逐步执行覆盖旧挂起").await;
    }
    crate::bot::audit_log(
        app,
        &format!(
            "exec_steps.start | id: {} | title: {}",
            task.id,
            crate::bot::truncate_for_log(&task.title, 60)
        ),
    );
    crate::bot_chat::set_bot_assigned(app, &task.id, true).await;
    let first = task
        .subtasks
        .as_deref()
        .and_then(|s| s.iter().find(|x| !x.done))
        .map(|x| x.id.clone())
        .ok_or_else(|| CommandError::TaskInvalidState {
            reason: "没有未完成的子任务".into(),
        })?;
    let stop = StopGuard::new(true, session_id.map(|s| s.to_string()));
    let r = run_step(app, &task.id, &first, None, &stop).await;
    if r.is_err() {
        clear(app, "逐步执行起步失败").await;
    }
    r
}

/// 聊天入口发现挂起时调用：按用户应答继续/重做/停
pub async fn resume(app: &AppHandle, reply: &str, session_id: Option<&str>) -> CommandResult<BotChatResult> {
    let Some(p) = take_pending() else {
        return Ok(BotChatResult {
            text: "（当前没有待确认的子任务执行）".into(),
            task_refs: vec![],
        });
    };
    // 任何分支都必须重新 park 或 clear，不能丢状态
    match classify_reply(reply) {
        StepReply::Stop => {
            park(p); // clear 内部 take，先放回去保证头像复位
            clear(app, "用户停止逐步执行").await;
            Ok(BotChatResult {
                text: "⏹ 已结束逐步执行。已确认勾选的子任务保持现状，其余未动。".into(),
                task_refs: vec![],
            })
        }
        StepReply::Continue => {
            crate::bot::audit_log(
                app,
                &format!("exec_steps.confirm | task: {} | 用户确认，勾选并继续", p.task_id),
            );
            mark_subtask_done(app, &p.task_id, &p.subtask_id).await;
            let stop = StopGuard::new(true, session_id.map(|s| s.to_string()));
            advance_or_finish(app, &p.task_id, &stop).await
        }
        StepReply::Redo(feedback) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "exec_steps.redo | task: {} | 意见: {}",
                    p.task_id,
                    crate::bot::truncate_for_log(&feedback, 100)
                ),
            );
            let stop = StopGuard::new(true, session_id.map(|s| s.to_string()));
            run_step(app, &p.task_id, &p.subtask_id, Some(&feedback), &stop).await
        }
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn continue_keywords() {
        for t in ["继续", "继续吧", "好了", "可以", "行", "下一个", "没问题", "嗯", "ok", "OK", " go ", "continue"] {
            assert_eq!(classify_reply(t), StepReply::Continue, "{t} 应判为继续");
        }
    }

    #[test]
    fn stop_keywords() {
        for t in ["停", "停止", "别做了", "不做了", "算了", "结束吧", "/stop", "STOP"] {
            assert_eq!(classify_reply(t), StepReply::Stop, "{t} 应判为停止");
        }
    }

    #[test]
    fn redo_with_feedback() {
        assert_eq!(classify_reply("重做"), StepReply::Redo("".into()));
        assert_eq!(classify_reply("重做，配色太深了"), StepReply::Redo("配色太深了".into()));
        assert_eq!(classify_reply("重新做：换个角度"), StepReply::Redo("换个角度".into()));
    }

    #[test]
    fn free_text_is_redo_feedback() {
        // 确认语境下的自由文本 = 对本步结果的修改意见
        assert_eq!(classify_reply("配色太深了，换浅色"), StepReply::Redo("配色太深了，换浅色".into()));
    }
}
