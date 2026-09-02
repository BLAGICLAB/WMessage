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

/// 挂起的逐步执行：等用户确认当前子任务。
/// 2026-08-27 审计 P1-12：全局单槽 → **按会话分槽**（HashMap<会话 key, 挂起>）——
/// 原先会话 B 触发逐步执行会静默覆盖会话 A 的挂起（A 之后回「继续」落入普通聊天被当新指令）。
struct PendingExec {
    task_id: String,
    subtask_id: String,
    /// 触发会话 id（2026-08-26 会话隔离）
    session_id: Option<String>,
    /// 批次5审计 P2：执行防重入守卫随挂起存活——原先 start() 返回即 Drop 释放，
    /// 确认挂起期间调度器能对同一卡并发起执行（SchedGuard/ExecGuard 互不知晓）。
    /// RAII：挂起被 take/clear 后随 PendingExec 一起 Drop，自动释放。
    #[allow(dead_code)] // 纯存活性持有（靠 Drop 释放防重入），从不读取
    exec_guard: ExecGuard,
}

static PENDING: OnceLock<Mutex<std::collections::HashMap<String, PendingExec>>> = OnceLock::new();

fn pending_map() -> &'static Mutex<std::collections::HashMap<String, PendingExec>> {
    PENDING.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 会话 key：None（后台）归到空串槽位
fn session_key(session_id: Option<&str>) -> String {
    session_id.unwrap_or("").to_string()
}

/// 当前会话是否有挂起的逐步执行（2026-08-26 起按会话匹配：
/// 会话 A 挂起时，会话 B 的消息走正常聊天路由，不被 resume 截胡）
pub fn has_pending_for(session_id: Option<&str>) -> bool {
    pending_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&session_key(session_id))
}

fn park(p: PendingExec) {
    let key = session_key(p.session_id.as_deref());
    pending_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, p);
}

fn take_pending_for(session_id: Option<&str>) -> Option<PendingExec> {
    pending_map()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&session_key(session_id))
}

/// 结束/清空**本会话**的挂起（正常结束、用户喊停、被同会话新执行覆盖）；恢复任务卡用户头像
pub async fn clear_for(app: &AppHandle, session_id: Option<&str>, reason: &str) {
    if let Some(p) = take_pending_for(session_id) {
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
    t.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
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
    exec_guard: ExecGuard,
) -> CommandResult<BotChatResult> {
    let task = load_task(app, task_id).await?;
    let subs = task.subtasks.clone().unwrap_or_default();
    let total = subs.len();
    let done_count = subs.iter().filter(|s| s.done).count();
    // 目标子任务已被用户手动勾掉/删除 → 顺移到下一个未勾
    let Some(sub) = subs.iter().find(|s| s.id == subtask_id && !s.done) else {
        return Box::pin(advance_or_finish(app, task_id, stop, exec_guard)).await;
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
        None,
    )
    .await?;
    park(PendingExec {
        task_id: task_id.to_string(),
        subtask_id: sub.id.clone(),
        session_id: stop.session_id().map(|s| s.to_string()),
        exec_guard,
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
    exec_guard: ExecGuard,
) -> CommandResult<BotChatResult> {
    let task = load_task(app, task_id).await?;
    let subs = task.subtasks.clone().unwrap_or_default();
    if let Some(next) = subs.iter().find(|s| !s.done) {
        // Box::pin：run_step ↔ advance_or_finish 互调是异步递归，Rust 要求显式装箱
        return Box::pin(run_step(app, task_id, &next.id.clone(), None, stop, exec_guard)).await;
    }
    clear_for(app, stop.session_id(), "全部子任务完成").await;
    Ok(BotChatResult {
        text: "🎉 所有子任务都已完成并勾选。任务卡本身我没有标记完成——你确认没问题后自己勾完成，或跟我说「完成它」。".into(),
        task_refs: vec![],
    })
}

/// 开始逐步执行（bot_execute_task 在 ≥2 个未勾子任务时分流到这里）
pub async fn start(app: &AppHandle, task: &crate::db::Task, session_id: Option<&str>) -> CommandResult<BotChatResult> {
    // 防重入：与 execute_task_core 同一守卫（同一卡不能同时两个执行实例）
    let Some(exec_guard) = ExecGuard::acquire(&task.id) else {
        crate::bot::audit_log(
            app,
            &format!("execute_task_rejected | id: {} | 已有执行实例在跑（防重入拦截）", task.id),
        );
        return Err(CommandError::TaskInvalidState {
            reason: "该任务卡正在执行中，请等待完成后再触发".into(),
        });
    };
    if has_pending_for(session_id) {
        clear_for(app, session_id, "新任务卡逐步执行覆盖旧挂起").await;
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
    let first = match task
        .subtasks
        .as_deref()
        .and_then(|s| s.iter().find(|x| !x.done))
        .map(|x| x.id.clone())
    {
        Some(id) => id,
        None => {
            // 批次5审计 F11：早退也要复位机器人头像（原先 ? 直接返回，卡片永远顶头像）
            crate::bot_chat::set_bot_assigned(app, &task.id, false).await;
            return Err(CommandError::TaskInvalidState {
                reason: "没有未完成的子任务".into(),
            });
        }
    };
    let stop = StopGuard::new_task_exec(true, session_id.map(|s| s.to_string()));
    // 批次5审计 P2：ExecGuard 随 run_step 传入并 park 进挂起态，确认等待期仍持防重入
    let r = run_step(app, &task.id, &first, None, &stop, exec_guard).await;
    if r.is_err() {
        clear_for(app, session_id, "逐步执行起步失败").await;
        // 批次5审计 F11：起步失败从未 park，上面的 clear_for 是 no-op，必须显式复位头像
        crate::bot_chat::set_bot_assigned(app, &task.id, false).await;
    }
    r
}

/// 聊天入口发现挂起时调用：按用户应答继续/重做/停
pub async fn resume(app: &AppHandle, reply: &str, session_id: Option<&str>) -> CommandResult<BotChatResult> {
    let Some(p) = take_pending_for(session_id) else {
        return Ok(BotChatResult {
            text: "（当前没有待确认的子任务执行）".into(),
            task_refs: vec![],
        });
    };
    // P1-12（2026-08-27 审计）：续跑失败必须显式收尾——原先 pending 已 take、
    // LLM 失败后任务卡永远顶着机器人头像且零审计（与 start() 的错误清理不对称）。
    // 失败语义按「结束本次逐步执行」处理（已勾选的保持现状），不静默挂起。
    let cleanup_on_err = |app: &AppHandle, task_id: &str, r: &CommandResult<BotChatResult>| {
        if let Err(e) = r {
            crate::bot::audit_log(
                app,
                &format!("exec_steps.resume_failed | task: {task_id} | {}", crate::bot::truncate_for_log(&e.to_string(), 200)),
            );
        }
        r.is_err()
    };
    // 批次5审计 P2：守卫随挂起取回——续跑分支再 park / 停止分支随解构 Drop 释放
    let PendingExec {
        task_id,
        subtask_id,
        exec_guard,
        ..
    } = p;
    // 任何分支都必须重新 park 或清理，不能丢状态
    match classify_reply(reply) {
        StepReply::Stop => {
            // 直接清理（pending 已 take）：审计 + 恢复任务卡用户头像
            crate::bot::audit_log(
                app,
                &format!("exec_steps.clear | task: {} | 用户停止逐步执行", task_id),
            );
            crate::bot_chat::set_bot_assigned(app, &task_id, false).await;
            Ok(BotChatResult {
                text: "⏹ 已结束逐步执行。已确认勾选的子任务保持现状，其余未动。".into(),
                task_refs: vec![],
            })
        }
        StepReply::Continue => {
            crate::bot::audit_log(
                app,
                &format!("exec_steps.confirm | task: {} | 用户确认，勾选并继续", task_id),
            );
            mark_subtask_done(app, &task_id, &subtask_id).await;
            let stop = StopGuard::new_task_exec(true, session_id.map(|s| s.to_string()));
            let r = advance_or_finish(app, &task_id, &stop, exec_guard).await;
            if cleanup_on_err(app, &task_id, &r) {
                crate::bot_chat::set_bot_assigned(app, &task_id, false).await;
            }
            r
        }
        StepReply::Redo(feedback) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "exec_steps.redo | task: {} | 意见: {}",
                    task_id,
                    crate::bot::truncate_for_log(&feedback, 100)
                ),
            );
            let stop = StopGuard::new_task_exec(true, session_id.map(|s| s.to_string()));
            let r = run_step(app, &task_id, &subtask_id, Some(&feedback), &stop, exec_guard).await;
            if cleanup_on_err(app, &task_id, &r) {
                crate::bot_chat::set_bot_assigned(app, &task_id, false).await;
            }
            r
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

    // ── P1-12（2026-08-27 审计）：挂起按会话分槽 ──

    #[test]
    fn pending_slots_are_per_session() {
        // 会话 A 挂起不影响会话 B；覆盖只发生在同会话内
        park(PendingExec { task_id: "tA".into(), subtask_id: "s1".into(), session_id: Some("sess-a-p12".into()), exec_guard: crate::bot_chat::ExecGuard::acquire("tA").expect("tA 守卫") });
        park(PendingExec { task_id: "tB".into(), subtask_id: "s2".into(), session_id: Some("sess-b-p12".into()), exec_guard: crate::bot_chat::ExecGuard::acquire("tB").expect("tB 守卫") });
        assert!(has_pending_for(Some("sess-a-p12")));
        assert!(has_pending_for(Some("sess-b-p12")));
        assert!(!has_pending_for(Some("sess-c-p12")));
        // 取 B 不动 A
        let b = take_pending_for(Some("sess-b-p12")).expect("B 应有挂起");
        assert_eq!(b.task_id, "tB");
        assert!(has_pending_for(Some("sess-a-p12")));
        assert!(!has_pending_for(Some("sess-b-p12")));
        // 收尾：不给其它测试留状态
        let _ = take_pending_for(Some("sess-a-p12"));
    }
}

#[cfg(test)]
mod batch5_guard_tests {
    /// 批次5审计 P2：ExecGuard RAII 语义——持有期间同卡不得再获取，Drop 后释放
    #[test]
    fn exec_guard_blocks_second_acquire_until_drop() {
        let g = crate::bot_chat::ExecGuard::acquire("batch5-test-task").expect("首次获取应成功");
        assert!(
            crate::bot_chat::ExecGuard::acquire("batch5-test-task").is_none(),
            "持有期间不得重复获取"
        );
        drop(g);
        assert!(
            crate::bot_chat::ExecGuard::acquire("batch5-test-task").is_some(),
            "Drop 后应可再获取"
        );
    }

    /// 批次5审计 P2 回归锁：挂起态必须持有 ExecGuard
    ///（否则确认等待期调度器可对同一卡并发起执行）
    #[test]
    fn pending_exec_holds_exec_guard() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/exec_steps.rs"
        ))
        .unwrap();
        let pos = text.find("struct PendingExec").expect("PendingExec 必须存在");
        let scope: String = text[pos..].chars().take(800).collect();
        assert!(
            scope.contains("exec_guard: ExecGuard"),
            "PendingExec 必须持有 ExecGuard: {scope:?}"
        );
    }
}
