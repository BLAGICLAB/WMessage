//! 工作流问答——应答端与生命周期设计 §3.2/§3.3）。
//!
//! ask 端（runner，下一片接入）：落 notifications（kind=workflow_question，id=wfq:{qid}）
//! + 注册 oneshot waiter（AppState.question_waiters）阻塞等待。
//! 本模块是应答端：
//! - `workflow_question_respond`：答 → resolve 通知 → 落档案 → 唤醒（存活才发）→ 审计广播
//! - `invalidate_workflow_questions`：run 收尾/停止/删除时批量失效本工作流的 pending 问题
//!
//! 红线：
//! - **忽略问题工作流也能走**：assume/dismiss 恒返回问题自带假设值；通知已消失（超时回收过）
//!   时静默成功（幂等），不报错不落档案
//! - run 已死后才送达的回答：档案仍落（重跑生效），唤醒静默跳过——不自动拉起旧 run

use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::error::{CommandError, CommandResult};

/// 问题通知 id 前缀（ask 端落队列用同一构造，两端靠它对上）
pub const QUESTION_NOTIF_PREFIX: &str = "wfq:";

pub fn question_notif_id(question_id: &str) -> String {
    format!("{QUESTION_NOTIF_PREFIX}{question_id}")
}

/// 应答动作
pub const ACTION_ANSWER: &str = "answer";
pub const ACTION_ASSUME: &str = "assume";
pub const ACTION_DISMISS: &str = "dismiss";

/// 应答核心（纯 DB 逻辑，与 AppHandle 解耦——单测锚点；command 包壳做唤醒/审计/广播）：
/// ①resolve 通知 ②落档案条目（answer 原文，或"未答按假设"）。
/// 返回要发给 waiter 的文本（answer 原文 / assumption）；通知不存在 → Ok(None)（幂等，什么都不做）。
/// 应答结果：给 waiter 的文本 + 审计上下文（answered 行归组用）
#[derive(Debug)]
pub(crate) struct RespondOutcome {
    pub text: String,
    pub workflow_id: String,
    pub task_id: Option<String>,
    pub run_started_at: i64,
    pub question: String,
    pub action: String,
}

pub(crate) fn respond_core(
    conn: &rusqlite::Connection,
    question_id: &str,
    action: &str,
    answer: Option<&str>,
) -> Result<Option<RespondOutcome>, String> {
    let notif_id = question_notif_id(question_id);
    let Some(view) = crate::notifications::notif_get(conn, &notif_id)? else {
        return Ok(None);
    };
    let payload = &view.payload;
    let assumption = payload
        .get("assumption")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let (resolved_status, brief_text) = match action {
        ACTION_ANSWER => {
            let a = answer.unwrap_or("").trim().to_string();
            if a.is_empty() {
                return Err("answer 动作必须携带回答文本".into());
            }
            (crate::notifications::STATUS_DONE, a)
        }
        // assume/dismiss：未答按假设继续（无假设兜底的问题在 ask 端就不允许落队列）
        ACTION_ASSUME | ACTION_DISMISS => {
            let text = if assumption.is_empty() {
                "（未回答，按继续执行处理）".to_string()
            } else {
                format!("（未回答，按 AI 假设继续：{assumption}）")
            };
            (
                if action == ACTION_DISMISS {
                    crate::notifications::STATUS_DISMISSED
                } else {
                    crate::notifications::STATUS_DONE
                },
                text,
            )
        }
        other => return Err(format!("非法应答动作：{other}")),
    };
    crate::notifications::notif_resolve(conn, &notif_id, resolved_status)?;
    // 档案：卡级（有 taskId 时）+ 全局层各一条——用户在这张卡上的回答，其他卡也要看得见
    let workflow_id = payload
        .get("workflowId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let task_id = payload
        .get("taskId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let question = payload
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !workflow_id.is_empty() {
        if let Some(tid) = task_id {
            crate::db::brief::brief_insert(
                conn,
                &workflow_id,
                Some(tid),
                crate::db::brief::KIND_QA,
                crate::db::brief::SOURCE_USER,
                &brief_text,
                Some(question),
            )?;
        }
        crate::db::brief::brief_insert(
            conn,
            &workflow_id,
            None,
            crate::db::brief::KIND_QA,
            crate::db::brief::SOURCE_USER,
            &brief_text,
            Some(question),
        )?;
    }
    Ok(Some(RespondOutcome {
        text: brief_text,
        workflow_id,
        task_id: task_id.map(|s| s.to_string()),
        run_started_at: payload
            .get("runStartedAt")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        question: question.to_string(),
        action: action.to_string(),
    }))
}

/// run 收尾/停止/工作流删除时：本工作流的 pending 问题批量失效（dismissed）。
/// 纯 DB；调用方（runner）负责 afterwards 的 notifications-changed 广播。
pub(crate) fn invalidate_workflow_questions(
    conn: &rusqlite::Connection,
    workflow_id: &str,
) -> Result<usize, String> {
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, payload FROM notifications
                 WHERE kind = ?1 AND status = 'pending'",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([crate::notifications::KIND_WORKFLOW_QUESTION], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(_, p)| {
                serde_json::from_str::<Value>(p)
                    .ok()
                    .and_then(|v| {
                        v.get("workflowId")
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .as_deref()
                    == Some(workflow_id)
            })
            .map(|(id, _)| id)
            .collect()
    };
    for id in &ids {
        crate::notifications::notif_resolve(conn, id, crate::notifications::STATUS_DISMISSED)?;
    }
    Ok(ids.len())
}

/// 应答一条工作流问题（通知页问题卡 invoke）：
/// action = answer（富回答）/ assume（按假设继续）/ dismiss（忽略——等同按假设）。
#[tauri::command]
pub async fn workflow_question_respond(
    app: AppHandle,
    question_id: String,
    action: String,
    answer: Option<String>,
) -> CommandResult<()> {
    if action != ACTION_ANSWER && action != ACTION_ASSUME && action != ACTION_DISMISS {
        return Err(CommandError::InvalidArgument {
            field: "action".into(),
            value: action,
            reason: format!("仅允许 {ACTION_ANSWER}/{ACTION_ASSUME}/{ACTION_DISMISS}"),
        });
    }
    if question_id.trim().is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "questionId".into(),
            value: question_id,
            reason: "questionId 不能为空".into(),
        });
    }
    let qid = question_id.clone();
    let action_clone = action.clone();
    let answer_clone = answer.clone();
    let app2 = app.clone();
    let outcome =
        tauri::async_runtime::spawn_blocking(move || -> Result<Option<RespondOutcome>, String> {
            let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
            respond_core(&conn, &qid, &action_clone, answer_clone.as_deref())
        })
        .await
        .map_err(|e| CommandError::from(format!("问答应答线程 join 失败：{e}")))?
        .map_err(CommandError::DbError)?;
    // 通知不存在 = 已被超时回收/重复应答——幂等成功，不重复落档案不重复唤醒
    let Some(outcome) = outcome else {
        return Ok(());
    };
    // 问答日志入审计表（尽力而为）；任务卡提问（无 workflowId）不落工作流审计
    if !outcome.workflow_id.is_empty() {
        let app2 = app.clone();
        let wf2 = outcome.workflow_id.clone();
        let tid2 = outcome.task_id.clone();
        let rsa = outcome.run_started_at;
        let q2 = outcome.question.clone();
        let a2 = outcome.text.clone();
        let act2 = outcome.action.clone();
        let r = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
            crate::db::workflow_audit::wa_insert(
                &conn,
                &wf2,
                rsa,
                tid2.as_deref(),
                crate::db::workflow_audit::KIND_QUESTION_ANSWERED,
                "info",
                &serde_json::json!({ "question": q2, "answer": a2, "action": act2 }),
            )
            .map(|_| ())
        })
        .await;
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("[workflow_audit] question_answered 写入失败（不阻断）：{e}"),
            Err(e) => eprintln!("[workflow_audit] question_answered 线程失败（不阻断）：{e}"),
        }
    }
    // 唤醒 waiter（run 存活才有；run 已死 → 档案已落，重跑生效，此处静默跳过）
    if let Some(tx) = crate::app_state::question_waiters(&app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] question_waiters: {e:?}");
            e.into_inner()
        })
        .remove(&question_id)
    {
        let _ = tx.send(outcome.text);
    }
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "workflow_question_answered",
        &[("action", action)],
    );
    crate::notifications::emit_changed(&app);
    Ok(())
}

// ask 端（ §3.2）：注册表 + ask_user 引擎

/// 提问等待上限
pub(crate) const ASK_TIMEOUT_SECS: u64 = 24 * 3600;

/// 提问通知标题主体：工作流问题挂工作流头衔，任务卡提问（manual）单列
fn question_title(workflow: Option<&str>, node_title: &str) -> String {
    match workflow {
        Some(_) => format!("工作流任务「{node_title}」提问"),
        None => format!("任务卡「{node_title}」提问"),
    }
}

/// 会话级提问上下文（bot_chat 会话建立时注册、收尾注销；ask_user 查表）。
/// 预算计数在条目上——同一把锁内检查+扣减，无竞态。
pub struct AskRegistration {
    /// 关联工作流；None = 任务卡手动执行（无工作流档案与审计归组）
    pub workflow_id: Option<String>,
    pub task_id: String,
    pub node_title: String,
    /// 提问模式开关
    pub asks_enabled: bool,
    /// run 分组键（审计行归组 + 问题 payload 透传给应答端）
    pub run_started_at: i64,
    pub asks_left: std::sync::atomic::AtomicU8,
    /// 注册时定死的预算上限（全局设置 ask_budget）——预算用尽提示用真实值
    pub asks_max: u8,
}

/// 注册提问上下文（bot_chat 在 register_exec_session 之后调用）。
/// budget = 全局提问预算（workflow_settings.ask_budget，工作流与任务卡共用）
pub(crate) fn register_ask_context<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
    ask: &crate::bot_chat::AskExecContext,
    task_id: &str,
    node_title: &str,
    budget: u8,
) {
    crate::app_state::ask_contexts(app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] ask_contexts: {e:?}");
            e.into_inner()
        })
        .insert(
            session_id.to_string(),
            AskRegistration {
                workflow_id: ask.workflow_id.clone(),
                task_id: task_id.to_string(),
                node_title: node_title.to_string(),
                asks_enabled: ask.asks_enabled,
                run_started_at: ask.run_started_at,
                asks_left: std::sync::atomic::AtomicU8::new(budget),
                asks_max: budget,
            },
        );
}

/// 注销（bot_chat 收尾与 unregister_exec_session 成对调用）
pub(crate) fn unregister_ask_context<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
) {
    crate::app_state::ask_contexts(app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] ask_contexts: {e:?}");
            e.into_inner()
        })
        .remove(session_id);
}

#[derive(Debug, PartialEq)]
pub(crate) struct AskArgs {
    pub question: String,
    pub why: Option<String>,
    pub options: Vec<String>,
    pub assumption: String,
}

/// ask_user 参数解析（纯逻辑，单测锚点）：question/assumption 必填（红线——
/// 没有假设的问题不许问），question ≤200 / why ≤100 / options ≤4×40 / assumption ≤200 截断。
pub(crate) fn parse_ask_args(args: &str) -> Result<AskArgs, String> {
    let v: serde_json::Value =
        serde_json::from_str(args).map_err(|e| format!("参数不是有效 JSON：{e}"))?;
    let question = v
        .get("question")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if question.is_empty() {
        return Err("缺少 question（要问用户的问题）".into());
    }
    let assumption = v
        .get("assumption")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if assumption.is_empty() {
        return Err(
            "缺少 assumption（你的推荐假设）——用户不回答时按它继续，给不出假设的问题不许问".into(),
        );
    }
    let why = v
        .get("why")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(100).collect::<String>());
    let options: Vec<String> = v
        .get("options")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .take(4)
                .map(|s| s.chars().take(40).collect::<String>())
                .collect()
        })
        .unwrap_or_default();
    Ok(AskArgs {
        question: question.chars().take(200).collect(),
        why,
        options,
        assumption: assumption.chars().take(200).collect(),
    })
}

/// ask_user 工具入口（registry TOOLS_TABLE 挂载；timeout 参数化供测试用短值）：
/// 未注册上下文（非工作流链路）→ warn 引导自行继续；模式关闭/超预算 → 不落队列直接回落假设；
/// 正常路径：落通知队列（wfq:{qid}）+ 系统通知门铃 → oneshot 等待 → 回答/假设作为工具结果。
/// stop：/stop 等取消源（ high——24h 等待必须可被用户停止打断，否则模型循环
/// 挂到超时；轮询 stopped() 每 500ms，命中即回落假设并清理 waiter）。
pub(crate) async fn engine_ask_user(
    app: &tauri::AppHandle,
    session_id: Option<&str>,
    args: &str,
    timeout: std::time::Duration,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let parsed = match parse_ask_args(args) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e, Vec::new()),
    };
    let Some(sid) = session_id else {
        return ToolResult::warn(
            "ask_user 仅任务执行中可用（工作流节点/手动执行的任务卡）；聊天中请直接文字提问。请按你的假设继续执行。",
            Vec::new(),
        );
    };
    let assumption_note = format!(
        "（未获回答，已按你声明的假设继续）假设：{}",
        parsed.assumption
    );
    // 锁内一次取全量 owned 数据（含模式检查与预算扣减）——引用不出锁作用域
    let (wf_id, task_id, node_title, run_started_at, budget_left, asks_max) = {
        let mut map = crate::app_state::ask_contexts(app)
            .lock()
            .unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] ask_contexts: {e:?}");
                e.into_inner()
            });
        let Some(reg) = map.get_mut(sid) else {
            return ToolResult::warn(
                "ask_user 仅任务执行中可用（工作流节点/手动执行的任务卡）；聊天中请直接文字提问。请按你的假设继续执行。",
                Vec::new(),
            );
        };
        if !reg.asks_enabled {
            return ToolResult::warn(format!("提问模式已关闭，{assumption_note}"), Vec::new());
        }
        let left = reg.asks_left.load(std::sync::atomic::Ordering::SeqCst);
        if left > 0 {
            reg.asks_left
                .store(left - 1, std::sync::atomic::Ordering::SeqCst);
        }
        (
            reg.workflow_id.clone(),
            reg.task_id.clone(),
            reg.node_title.clone(),
            reg.run_started_at,
            left,
            reg.asks_max,
        )
    };
    if budget_left == 0 {
        return ToolResult::warn(
            format!("提问预算（{asks_max} 次）已用尽，{assumption_note}"),
            Vec::new(),
        );
    }
    let qid = uuid::Uuid::new_v4().simple().to_string();
    let payload = serde_json::json!({
        "questionId": qid,
        "workflowId": wf_id.clone(),
        "taskId": task_id.clone(),
        "nodeTitle": node_title.clone(),
        "question": parsed.question,
        "why": parsed.why,
        "options": parsed.options,
        "assumption": parsed.assumption,
        // run 分组键透传——应答端写审计行凭它归到正确的 run
        "runStartedAt": run_started_at,
        "createdAt": chrono::Utc::now().timestamp_millis(),
    });
    let title = question_title(wf_id.as_deref(), &node_title);
    let body = parsed.question.clone();
    let qid_insert = qid.clone();
    let app2 = app.clone();
    let inserted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        crate::notifications::notif_insert(
            &conn,
            &question_notif_id(&qid_insert),
            crate::notifications::KIND_WORKFLOW_QUESTION,
            &title,
            &body,
            &payload,
        )
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match inserted {
        Ok(true) => {}
        // 同 qid 重复（uuid 冲突现实不可达）或写库失败：不阻塞节点，回落假设
        Ok(false) => {
            return ToolResult::warn(format!("提问队列写入冲突，{assumption_note}"), Vec::new());
        }
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "workflow_question_asked",
                &[
                    ("outcome", "insert_failed".into()),
                    ("error", crate::audit::escape_for_log(&e, 120)),
                ],
            );
            return ToolResult::warn(format!("提问队列暂不可用，{assumption_note}"), Vec::new());
        }
    }
    // 等待应答；超时/通道关闭/用户停止 → 回落假设（红线：忽略问题工作流也能走）。
    // waiter 必须先于 audit/emit_changed/OS 门铃注册emit/通知会把
    // 用户引到通知页应答，若 waiter 未就位，应答端找不到通道 → 档案落了回答而
    // 引擎按假设继续，两层记录不一致）。
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    crate::app_state::question_waiters(app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] question_waiters: {e:?}");
            e.into_inner()
        })
        .insert(qid.clone(), tx);
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "workflow_question_asked",
        &[
            ("taskId", task_id.clone()),
            ("hasOptions", (!parsed.options.is_empty()).to_string()),
        ],
    );
    // 问答日志入审计表（设计 §4.2 kinds 含 question_asked/answered）；
    // 任务卡提问（无工作流）不落工作流审计——归组键不存在，硬写只会出孤儿行
    if let Some(wf2) = wf_id.clone() {
        let app2 = app.clone();
        let tid2 = task_id.clone();
        let q2 = parsed.question.clone();
        let a2 = parsed.assumption.clone();
        let rsa = run_started_at;
        let r = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
            crate::db::workflow_audit::wa_insert(
                &conn,
                &wf2,
                rsa,
                Some(&tid2),
                crate::db::workflow_audit::KIND_QUESTION_ASKED,
                "info",
                &serde_json::json!({ "question": q2, "assumption": a2 }),
            )
            .map(|_| ())
        })
        .await;
        // 内外两层都要接：JoinError 与 DB 错误都打日志（ high——
        // 只 match 外层会把 open_db/写库失败静默吞掉）
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("[workflow_audit] question_asked 写入失败（不阻断）：{e}"),
            Err(e) => eprintln!("[workflow_audit] question_asked 线程失败（不阻断）：{e}"),
        }
    }
    crate::notifications::emit_changed(app);
    // 系统通知门铃（OS 通知放不下富回答，点击进应用通知页）
    let _ = app
        .notification()
        .builder()
        .title(format!(
            "❓ {}",
            question_title(wf_id.as_deref(), &node_title)
        ))
        .body(&parsed.question)
        .show();
    enum WaitOutcome {
        Answered(String),
        /// 超时或用户停止（同口径：回落假设）
        Expired,
    }
    let waited = tokio::select! {
        r = tokio::time::timeout(timeout, rx) => match r {
            Ok(Ok(answer)) => WaitOutcome::Answered(answer),
            _ => WaitOutcome::Expired,
        },
        // /stop 等取消源：轮询停止标志（StopGuard 无 watch 通道，500ms 粒度足够——
        // 提问本身以小时计），命中 = 用户主动放弃本轮工作流，同超时口径回落假设
        _ = async {
            loop {
                match stop {
                    Some(s) if s.stopped() => break,
                    _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
                }
            }
        } => WaitOutcome::Expired,
    };
    // 无论结果都摘除 waiter（应答端 remove 幂等；超时/停止路径靠这里防泄漏）
    crate::app_state::question_waiters(app)
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] question_waiters: {e:?}");
            e.into_inner()
        })
        .remove(&qid);
    match waited {
        WaitOutcome::Answered(answer) => {
            ToolResult::ok(format!("用户回答了你的提问：{answer}"), Vec::new())
        }
        WaitOutcome::Expired => ToolResult::warn(
            format!(
                "提问在等待期内未获回答，已按你声明的假设继续。假设：{}",
                parsed.assumption
            ),
            Vec::new(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::brief;

    fn mem_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::notifications::notif_insert(
            &conn,
            "seed-irrelevant",
            crate::notifications::KIND_MEMORY,
            "t",
            "",
            &serde_json::json!({}),
        )
        .unwrap();
        brief::ensure_brief_entries(&conn).unwrap();
        conn
    }

    fn seed_question(conn: &rusqlite::Connection, workflow_id: &str, task_id: &str) -> String {
        let qid = format!("q-{workflow_id}-{task_id}");
        let payload = serde_json::json!({
            "questionId": qid,
            "workflowId": workflow_id,
            "taskId": task_id,
            "nodeTitle": "写初稿",
            "question": "投放哪个平台？",
            "options": ["公众号", "知乎"],
            "assumption": "公众号",
        });
        crate::notifications::notif_insert(
            conn,
            &question_notif_id(&qid),
            crate::notifications::KIND_WORKFLOW_QUESTION,
            "工作流提问",
            "投放哪个平台？",
            &payload,
        )
        .unwrap();
        qid
    }

    #[test]
    fn answer_resolves_notif_writes_both_brief_layers() {
        let conn = mem_conn();
        let qid = seed_question(&conn, "wf1", "t1");
        let text = respond_core(&conn, &qid, ACTION_ANSWER, Some("  知乎，重点发长文  "))
            .unwrap()
            .unwrap()
            .text;
        assert_eq!(text, "知乎，重点发长文");
        // 通知已 resolve
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(&qid))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DONE);
        // 档案双层各一条：卡级 + 全局级，reason = 问题原文
        let card = brief::brief_list_card(&conn, "t1").unwrap();
        assert_eq!(card.len(), 1);
        assert_eq!(card[0].reason.as_deref(), Some("投放哪个平台？"));
        assert!(card[0].text.contains("知乎"));
        assert_eq!(brief::brief_list_global(&conn, "wf1").unwrap().len(), 1);
    }

    #[test]
    fn assume_and_dismiss_fall_back_to_assumption() {
        let conn = mem_conn();
        let qid = seed_question(&conn, "wf1", "t1");
        let text = respond_core(&conn, &qid, ACTION_ASSUME, None)
            .unwrap()
            .unwrap()
            .text;
        assert!(text.contains("公众号"), "assume 应返回问题自带假设：{text}");
        assert!(text.contains("未回答"));
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(&qid))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DONE);

        let qid2 = seed_question(&conn, "wf2", "t2");
        let text = respond_core(&conn, &qid2, ACTION_DISMISS, None)
            .unwrap()
            .unwrap()
            .text;
        assert!(text.contains("公众号"));
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(&qid2))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DISMISSED);
    }

    #[test]
    fn answer_requires_text_and_unknown_id_is_noop() {
        let conn = mem_conn();
        let qid = seed_question(&conn, "wf1", "t1");
        assert!(respond_core(&conn, &qid, ACTION_ANSWER, Some("  ")).is_err());
        assert!(respond_core(&conn, &qid, ACTION_ANSWER, None).is_err());
        assert!(respond_core(&conn, &qid, "bogus", None).is_err());
        // 未知 id：幂等 Ok(None)，不落档案
        assert!(respond_core(&conn, "q-gone", ACTION_ANSWER, Some("x"))
            .unwrap()
            .is_none());
        assert!(brief::brief_list_global(&conn, "wf1").unwrap().is_empty());
    }

    #[test]
    fn invalidate_only_touches_target_workflow() {
        let conn = mem_conn();
        seed_question(&conn, "wf1", "t1");
        seed_question(&conn, "wf2", "t2");
        assert_eq!(invalidate_workflow_questions(&conn, "wf1").unwrap(), 1);
        let still_pending = crate::notifications::notif_get(&conn, &question_notif_id("q-wf2-t2"))
            .unwrap()
            .unwrap();
        assert_eq!(still_pending.status, crate::notifications::STATUS_PENDING);
        let gone = crate::notifications::notif_get(&conn, &question_notif_id("q-wf1-t1"))
            .unwrap()
            .unwrap();
        assert_eq!(gone.status, crate::notifications::STATUS_DISMISSED);
        // 二次失效：幂等 0 条
        assert_eq!(invalidate_workflow_questions(&conn, "wf1").unwrap(), 0);
    }

    #[test]
    fn ask_args_require_question_and_assumption() {
        // 红线：没有假设的问题不许问
        let err = parse_ask_args(r#"{"question":"平台？"}"#).unwrap_err();
        assert!(err.contains("assumption"));
        assert!(parse_ask_args(r#"{"assumption":"公众号"}"#).is_err());
        assert!(parse_ask_args("不是 JSON").is_err());
        let ok = parse_ask_args(
            r#"{"question":"平台？","why":"规范不同","options":["公众号","知乎"],"assumption":"公众号"}"#,
        )
        .unwrap();
        assert_eq!(ok.question, "平台？");
        assert_eq!(ok.assumption, "公众号");
        assert_eq!(ok.options, vec!["公众号", "知乎"]);
        // 超限截断 + 空白项剔除
        let ok = parse_ask_args(
            r#"{"question":"问问问问问问","options":["  ","选项"],"assumption":"假"}"#,
        )
        .unwrap();
        assert_eq!(ok.options, vec!["选项"]);
    }

    #[test]
    fn manual_task_question_answers_without_workflow() {
        // 任务卡手动执行的提问：payload 无 workflowId——应答照常走通，
        // 不落工作流档案（brief 只属于工作流），outcome.workflow_id 为空
        let conn = mem_conn();
        let qid = "q-manual-t9";
        let payload = serde_json::json!({
            "questionId": qid,
            "taskId": "t9",
            "nodeTitle": "写一部10章左右武侠小说",
            "question": "书名用《问剑录》还是《断剑记》？",
            "assumption": "《问剑录》",
        });
        crate::notifications::notif_insert(
            &conn,
            &question_notif_id(qid),
            crate::notifications::KIND_WORKFLOW_QUESTION,
            &question_title(None, "写一部10章左右武侠小说"),
            "书名用《问剑录》还是《断剑记》？",
            &payload,
        )
        .unwrap();

        let outcome = respond_core(&conn, qid, ACTION_ANSWER, Some("用《问剑录》"))
            .unwrap()
            .unwrap();
        assert_eq!(outcome.workflow_id, "", "任务卡提问无工作流归组");
        assert_eq!(outcome.task_id.as_deref(), Some("t9"));
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(qid))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DONE);
    }

    #[test]
    fn question_title_splits_workflow_from_task_card() {
        // 标题口径：工作流问题与任务卡提问单列——通知中心里两种来源可辨
        assert_eq!(
            question_title(Some("wf1"), "写初稿"),
            "工作流任务「写初稿」提问"
        );
        assert_eq!(question_title(None, "写初稿"), "任务卡「写初稿」提问");
    }
}
