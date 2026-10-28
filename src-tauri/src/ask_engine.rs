//! 任务执行问答引擎（ask_user 工具 + 通知页应答）。
//! 原随工作流画布而生（workflow_questions），画布退役后中立化保留——
//! 手动执行任务卡、以及未来一切「有人值守」的执行链路共用。
//!
//! ask 端（ask_user 工具，bot/registry 挂载）：落 notifications
//! （kind=workflow_question，遗留名含库内历史行，id=wfq:{qid}）
//! + 注册 oneshot waiter（AppState.question_waiters）阻塞等待。
//! 应答端（ask_respond 命令，通知页问题卡 invoke）：
//! 答 → resolve 通知 → 唤醒（存活才发）→ 审计广播。
//!
//! 红线：
//! - **忽略问题执行也能走**：assume/dismiss 恒返回问题自带假设值；通知已消失（超时回收过）
//!   时静默成功（幂等），不报错
//! - 执行已结束后才送达的回答：唤醒静默跳过——不自动拉起旧执行

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

/// 应答核心（纯 DB 逻辑，与 AppHandle 解耦——单测锚点）：
/// resolve 通知，返回要发给 waiter 的文本（answer 原文 / 未答按假设）。
/// 通知不存在 → Ok(None)（幂等，什么都不做）。
pub(crate) fn respond_core(
    conn: &rusqlite::Connection,
    question_id: &str,
    action: &str,
    answer: Option<&str>,
) -> Result<Option<String>, String> {
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
    let (resolved_status, text) = match action {
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
    Ok(Some(text))
}

/// 应答一条任务执行提问（通知页问题卡 invoke）：
/// action = answer（富回答）/ assume（按假设继续）/ dismiss（忽略——等同按假设）。
#[tauri::command]
pub async fn ask_respond(
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
    let text = tauri::async_runtime::spawn_blocking(move || -> Result<Option<String>, String> {
        // 通知表写入与全仓写口同纪律：持 DB 写锁——主窗导入/大事务持锁超
        // busy_timeout 时，锁外直写会静默丢掉「用户点了选项」这条应答
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        respond_core(&conn, &qid, &action_clone, answer_clone.as_deref())
    })
    .await
    .map_err(|e| CommandError::from(format!("问答应答线程 join 失败：{e}")))?
    .map_err(CommandError::DbError)?;
    // 通知不存在 = 已被超时回收/重复应答——幂等成功，不重复唤醒
    let Some(text) = text else {
        return Ok(());
    };
    // 唤醒 waiter（执行存活才有；已结束 → 静默跳过，不自动拉起旧执行）
    if let Some(tx) = crate::db::lock::lock_or_recover(
        crate::app_state::question_waiters(&app),
        "question_waiters",
    )
    .remove(&question_id)
    {
        let _ = tx.send(text);
    }
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "ask_answered",
        &[("action", action)],
    );
    crate::notifications::emit_changed(&app);
    Ok(())
}

// ask 端：注册表 + ask_user 引擎

/// 提问等待上限
pub(crate) const ASK_TIMEOUT_SECS: u64 = 24 * 3600;

/// 提问通知标题主体
fn question_title(node_title: &str) -> String {
    format!("任务卡「{node_title}」提问")
}

/// 会话级提问上下文（bot_chat 会话建立时注册、收尾注销；ask_user 查表）。
/// 预算计数在条目上——同一把锁内检查+扣减，无竞态。
pub struct AskRegistration {
    pub task_id: String,
    pub node_title: String,
    /// 提问模式开关
    pub asks_enabled: bool,
    pub asks_left: std::sync::atomic::AtomicU8,
    /// 注册时定死的预算上限——预算用尽提示用真实值
    pub asks_max: u8,
}

/// 注册提问上下文（bot_chat 在 register_exec_session 之后调用，手动执行路径）。
/// budget = 全局提问预算（bot 参数 tools.ask_budget）
pub(crate) fn register_ask_context<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: &str,
    task_id: &str,
    node_title: &str,
    budget: u8,
) {
    crate::db::lock::lock_or_recover(crate::app_state::ask_contexts(app), "ask_contexts").insert(
        session_id.to_string(),
        AskRegistration {
            task_id: task_id.to_string(),
            node_title: node_title.to_string(),
            asks_enabled: true,
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
    crate::db::lock::lock_or_recover(crate::app_state::ask_contexts(app), "ask_contexts")
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

/// 摘除提问 waiter：insert 失败注销与等待结束防泄漏共用（应答端 remove 幂等，
/// 双方摘除只对先到者生效，晚到者拿到 None）
fn remove_question_waiter<R: tauri::Runtime>(app: &tauri::AppHandle<R>, qid: &str) {
    crate::db::lock::lock_or_recover(crate::app_state::question_waiters(app), "question_waiters")
        .remove(qid);
}

/// 提问队列写入失败时回冲预算：原子回冲到本调用扣减前值。fetch_max 只补不超发
///（若已有人回冲/值更高则不动）；同一注册的 ask 串行，此处纯属防御；映射缺行 =
/// 注册已随会话收尾注销，无需回冲。
fn restore_ask_budget<R: tauri::Runtime>(app: &tauri::AppHandle<R>, sid: &str, prev: u8) {
    let mut map =
        crate::db::lock::lock_or_recover(crate::app_state::ask_contexts(app), "ask_contexts");
    if let Some(reg) = map.get_mut(sid) {
        reg.asks_left
            .fetch_max(prev, std::sync::atomic::Ordering::SeqCst);
    }
}

/// ask_user 工具入口（registry TOOLS_TABLE 挂载；timeout 参数化供测试用短值）：
/// 未注册上下文（聊天等非执行链路）→ warn 引导自行继续；模式关闭/超预算 → 不落队列直接回落假设；
/// 正常路径：落通知队列（wfq:{qid}）+ 系统通知门铃 → oneshot 等待 → 回答/假设作为工具结果。
/// stop：/stop 等取消源（24h 等待必须可被用户停止打断，否则模型循环
/// 挂到超时；轮询 stopped() 每 500ms，命中即回落假设并清理 waiter）。
pub(crate) async fn engine_ask_user(
    app: &tauri::AppHandle,
    session_id: Option<&str>,
    args: &str,
    timeout: std::time::Duration,
    stop: Option<&crate::bot::StopGuard>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let parsed = match parse_ask_args(args) {
        Ok(p) => p,
        Err(e) => return ToolResult::error(e, Vec::new()),
    };
    let Some(sid) = session_id else {
        return ToolResult::warn(
            "ask_user 仅任务执行中可用（手动执行任务卡）；聊天中请直接文字提问。请按你的假设继续执行。",
            Vec::new(),
        );
    };
    let assumption_note = format!(
        "（未获回答，已按你声明的假设继续）假设：{}",
        parsed.assumption
    );
    // 锁内一次取全量 owned 数据（含模式检查与预算扣减）——引用不出锁作用域
    let (task_id, node_title, budget_left, asks_max) = {
        let mut map =
            crate::db::lock::lock_or_recover(crate::app_state::ask_contexts(app), "ask_contexts");
        let Some(reg) = map.get_mut(sid) else {
            return ToolResult::warn(
                "ask_user 仅任务执行中可用（手动执行任务卡）；聊天中请直接文字提问。请按你的假设继续执行。",
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
            reg.task_id.clone(),
            reg.node_title.clone(),
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
        "taskId": task_id.clone(),
        "nodeTitle": node_title.clone(),
        "question": parsed.question,
        "why": parsed.why,
        "options": parsed.options,
        "assumption": parsed.assumption,
        "createdAt": crate::db::now_ms(),
    });
    let title = question_title(&node_title);
    let body = parsed.question.clone();
    // waiter 先于 notif_insert 注册：通知落库后用户随时可从通知中心/应答入口进来，
    // 应答端写完才查 waiter——晚注册的窗口里回答会「档案落了回答、引擎按假设
    // 继续」两层不一致。qid 是本次新造的 uuid，落库前无人能引用，提前注册无重复
    // 消费面（应答端 remove 单次消费）；insert 失败路径负责注销防泄漏。
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    crate::db::lock::lock_or_recover(crate::app_state::question_waiters(app), "question_waiters")
        .insert(qid.clone(), tx);
    let qid_insert = qid.clone();
    let app2 = app.clone();
    let inserted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
        // 同 ask_answer：通知落库持 DB 写锁，不裸写
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        crate::notifications::notif_insert(
            &conn,
            &question_notif_id(&qid_insert),
            crate::notifications::KIND_TASK_QUESTION,
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
        // 同 qid 重复（uuid 冲突现实不可达）或写库失败：不阻塞执行，回落假设；
        // waiter 已提前注册，必须注销防泄漏；预算先扣后写，写失败原路回冲
        // （不回冲 = 队列故障白扣一次提问额度）
        Ok(false) => {
            remove_question_waiter(app, &qid);
            restore_ask_budget(app, sid, budget_left);
            return ToolResult::warn(format!("提问队列写入冲突，{assumption_note}"), Vec::new());
        }
        Err(e) => {
            remove_question_waiter(app, &qid);
            restore_ask_budget(app, sid, budget_left);
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "ask_user_queue_error",
                &[
                    ("outcome", "insert_failed".into()),
                    ("error", crate::audit::escape_for_log(&e, 120)),
                ],
            );
            return ToolResult::warn(format!("提问队列暂不可用，{assumption_note}"), Vec::new());
        }
    }
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "ask_user_asked",
        &[
            ("taskId", task_id.clone()),
            ("hasOptions", (!parsed.options.is_empty()).to_string()),
        ],
    );
    crate::notifications::emit_changed(app);
    // 系统通知门铃（OS 通知放不下富回答，点击进应用通知页）
    let _ = app
        .notification()
        .builder()
        .title(format!("❓ {}", question_title(&node_title)))
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
        // 提问本身以小时计），命中 = 用户主动放弃本轮执行，同超时口径回落假设
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
    remove_question_waiter(app, &qid);
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
        conn
    }

    fn seed_question(conn: &rusqlite::Connection, task_id: &str) -> String {
        let qid = format!("q-{task_id}");
        let payload = serde_json::json!({
            "questionId": qid,
            "taskId": task_id,
            "nodeTitle": "写初稿",
            "question": "投放哪个平台？",
            "options": ["公众号", "知乎"],
            "assumption": "公众号",
        });
        crate::notifications::notif_insert(
            conn,
            &question_notif_id(&qid),
            crate::notifications::KIND_TASK_QUESTION,
            "任务卡提问",
            "投放哪个平台？",
            &payload,
        )
        .unwrap();
        qid
    }

    #[test]
    fn answer_resolves_notif_and_returns_text() {
        let conn = mem_conn();
        let qid = seed_question(&conn, "t1");
        let text = respond_core(&conn, &qid, ACTION_ANSWER, Some("  知乎，重点发长文  "))
            .unwrap()
            .unwrap();
        assert_eq!(text, "知乎，重点发长文");
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(&qid))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DONE);
    }

    #[test]
    fn assume_and_dismiss_fall_back_to_assumption() {
        let conn = mem_conn();
        let qid = seed_question(&conn, "t1");
        let text = respond_core(&conn, &qid, ACTION_ASSUME, None)
            .unwrap()
            .unwrap();
        assert!(text.contains("公众号"), "assume 应返回问题自带假设：{text}");
        assert!(text.contains("未回答"));
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(&qid))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DONE);

        let qid2 = seed_question(&conn, "t2");
        let text = respond_core(&conn, &qid2, ACTION_DISMISS, None)
            .unwrap()
            .unwrap();
        assert!(text.contains("公众号"));
        let notif = crate::notifications::notif_get(&conn, &question_notif_id(&qid2))
            .unwrap()
            .unwrap();
        assert_eq!(notif.status, crate::notifications::STATUS_DISMISSED);
    }

    #[test]
    fn answer_requires_text_and_unknown_id_is_noop() {
        let conn = mem_conn();
        let qid = seed_question(&conn, "t1");
        assert!(respond_core(&conn, &qid, ACTION_ANSWER, Some("  ")).is_err());
        assert!(respond_core(&conn, &qid, ACTION_ANSWER, None).is_err());
        assert!(respond_core(&conn, &qid, "bogus", None).is_err());
        // 未知 id：幂等 Ok(None)
        assert!(respond_core(&conn, "q-gone", ACTION_ANSWER, Some("x"))
            .unwrap()
            .is_none());
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

    // ── 引擎窗口语义（mock AppState 注入，见 bot_slash::test_handle 先例）──

    fn test_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(crate::app_state::AppState::default());
        app.handle().clone()
    }

    fn seed_ask_registration(
        app: &tauri::AppHandle<tauri::test::MockRuntime>,
        sid: &str,
        budget: u8,
    ) {
        crate::app_state::ask_contexts(app).lock().unwrap().insert(
            sid.to_string(),
            AskRegistration {
                task_id: "t1".into(),
                node_title: "写初稿".into(),
                asks_enabled: true,
                asks_left: std::sync::atomic::AtomicU8::new(budget),
                asks_max: budget,
            },
        );
    }

    fn asks_left(app: &tauri::AppHandle<tauri::test::MockRuntime>, sid: &str) -> u8 {
        crate::app_state::ask_contexts(app)
            .lock()
            .unwrap()
            .get(sid)
            .unwrap()
            .asks_left
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn ask_budget_restore_refunds_exactly_and_never_inflates() {
        // 引擎先扣减后 insert；insert 失败必须把预算原路回冲——回冲后值等于
        // 扣减前值，重复回冲/未知会话都不得超发或误伤
        let app = test_handle();
        seed_ask_registration(&app, "s1", 3);
        // 引擎同款扣减（left=3 → store 2）
        {
            let mut map = crate::app_state::ask_contexts(&app).lock().unwrap();
            let reg = map.get_mut("s1").unwrap();
            let left = reg.asks_left.load(std::sync::atomic::Ordering::SeqCst);
            reg.asks_left
                .store(left - 1, std::sync::atomic::Ordering::SeqCst);
        }
        assert_eq!(asks_left(&app, "s1"), 2, "setup: 已扣减");
        restore_ask_budget(&app, "s1", 3);
        assert_eq!(asks_left(&app, "s1"), 3, "insert 失败预算须回冲到调用前值");
        restore_ask_budget(&app, "s1", 3);
        assert_eq!(asks_left(&app, "s1"), 3, "重复回冲不得超发");
        restore_ask_budget(&app, "s-gone", 9);
        assert_eq!(asks_left(&app, "s1"), 3, "未知会话回冲是无害 no-op");
        // 预算耗尽（0）时的失败回冲同样成立
        seed_ask_registration(&app, "s2", 1);
        {
            let mut map = crate::app_state::ask_contexts(&app).lock().unwrap();
            map.get_mut("s2")
                .unwrap()
                .asks_left
                .store(0, std::sync::atomic::Ordering::SeqCst);
        }
        restore_ask_budget(&app, "s2", 1);
        assert_eq!(asks_left(&app, "s2"), 1, "最后一次预算的失败回冲");
    }

    #[test]
    fn waiter_window_answerable_and_insert_failure_deregisters() {
        // 注释承诺的顺序契约：waiter 注册先于 notif_insert。窗口语义锁定两点——
        // ① 提前注册后应答端照常单次消费（remove + send），引擎侧 rx 收得到；
        // ② insert 失败路径注销后通道关闭、表内无泄漏条目。
        let app = test_handle();
        let qid = "q-window";
        let (tx, mut rx) = tokio::sync::oneshot::channel::<String>();
        crate::app_state::question_waiters(&app)
            .lock()
            .unwrap()
            .insert(qid.into(), tx);
        // 应答端同款消费（remove 单次）
        let got = crate::app_state::question_waiters(&app)
            .lock()
            .unwrap()
            .remove(qid);
        got.expect("提前注册的 waiter 应答端必须找得到")
            .send("知乎".into())
            .unwrap();
        assert_eq!(rx.try_recv().unwrap(), "知乎");
        assert!(
            crate::app_state::question_waiters(&app)
                .lock()
                .unwrap()
                .get(qid)
                .is_none(),
            "应答端 remove 后无残留"
        );
        // insert 失败路径：注销防泄漏
        let (tx2, mut rx2) = tokio::sync::oneshot::channel::<String>();
        crate::app_state::question_waiters(&app)
            .lock()
            .unwrap()
            .insert(qid.into(), tx2);
        remove_question_waiter(&app, qid);
        assert!(
            crate::app_state::question_waiters(&app)
                .lock()
                .unwrap()
                .get(qid)
                .is_none(),
            "insert 失败注销后表内不得残留 waiter"
        );
        assert!(rx2.try_recv().is_err(), "注销后通道应关闭（tx 已 drop）");
    }
}
