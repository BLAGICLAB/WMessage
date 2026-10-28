//! 任务执行面：批量执行、ExecGuard、run_task_in_chat 全链路。

use super::*;

/// 批量执行的失败策略。显式化一卡失败后是「继续下一张」还是「中止」。
///
/// 默认 `ContinueOnError`（现状不变）；`StopOnFirstError` 可调用方选启用。
#[derive(Default)]
pub enum BatchPolicy {
    /// 一卡失败继续下一张（现有默认行为）。汇总报告里仍会列失败清单。
    #[default]
    ContinueOnError,
    /// 一卡失败即中止后续，报告「已执行 i 张，剩余 N-i-1 张未执行」。
    StopOnFirstError,
}

/// 聊天模式批量执行：每张卡调一次 run_task_in_chat（每卡独立新会话，
/// EXECUTE_SYSTEM_PROMPT + 50 轮工具循环），单卡失败不污染其他卡的执行记录。
/// 顺序执行（避免文件写冲突）；一卡失败继续（任一卡失败不阻断后续）。
/// /stop 语义：外层 stop 只在**卡片之间**判定——每卡内部用自己新建的
/// StopGuard（新会话 id，/stop 按会话匹配不到它），故 /stop 只阻止下一张卡
/// 启动，不中断正在执行的卡。
/// 汇总报告：每张卡的开头 + 执行结果 + 总数 + 失败清单；task_refs 跨卡去重（merge_task_refs_dedup）。
///
/// 接受 `policy` 参数控制失败是否继续。默认 `BatchPolicy::default()`（ContinueOnError）。
pub async fn chat_execute_tasks(
    app: &AppHandle,
    task_ids: Vec<(String, String)>,
    stop: StopGuard,
    policy: BatchPolicy,
) -> CommandResult<BotChatResult> {
    let total = task_ids.len();
    let mut all_text = String::new();
    let mut all_refs: Vec<TaskRef> = Vec::new();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    all_text.push_str(&format!("📋 批量执行 {} 张任务卡\n", total));

    for (idx, (task_id, title)) in task_ids.iter().enumerate() {
        if stop.stopped() {
            all_text.push_str(&format!("\n⏹ 已停止（剩余 {} 张未执行）", total - idx));
            break;
        }
        let label = if title.is_empty() {
            task_id.as_str()
        } else {
            title.as_str()
        };
        all_text.push_str(&format!("\n── [{}/{}] {} ──\n", idx + 1, total, label));
        match run_task_in_chat(app, task_id, TaskExecOrigin::Batch).await {
            Ok(r) => {
                if !r.result.text.is_empty() {
                    all_text.push_str(&r.result.text);
                    all_text.push('\n');
                }
                ok += 1;
                all_refs.extend(r.result.task_refs);
            }
            Err(e) => {
                let err_str = e.to_string();
                all_text.push_str(&format!("❌ 失败：{}\n", err_str));
                failed += 1;
                errors.push(format!("{} ({})", label, err_str));
                // StopOnFirstError —— 第一卡失败即中止后续，附“已完成 i 张”报告
                if matches!(policy, BatchPolicy::StopOnFirstError) {
                    let done = idx + 1; // 当前卡已计入 idx（0-based），+1 = 已处理数
                    let remaining = total.saturating_sub(done);
                    all_text.push_str(&format!(
                        "\n⛔ 中止：第 {} 张卡失败，后续 {} 张未执行。",
                        done, remaining
                    ));
                    break;
                }
            }
        }
    }
    all_text.push_str(&format!(
        "\n── 汇总 ──\n✅ 完成 {} / ❌ 失败 {} / 📊 共 {}",
        ok, failed, total
    ));
    if !errors.is_empty() {
        all_text.push_str("\n失败清单：\n");
        for e in &errors {
            all_text.push_str(&format!("- {}\n", e));
        }
    }
    Ok(BotChatResult {
        text: all_text,
        task_refs: merge_task_refs_dedup(all_refs),
        busy: false,
        trace_id: None,
    })
}

impl ExecGuard {
    pub(crate) fn acquire<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        task_id: &str,
    ) -> Option<Self> {
        let running = exec_running(app);
        {
            let mut set = crate::db::lock::lock_or_recover(&running, "app_state::exec_running");
            if set.contains(task_id) {
                return None;
            }
            set.insert(task_id.to_string());
        }
        Some(Self {
            task_id: task_id.to_string(),
            running,
        })
    }
}

impl Drop for ExecGuard {
    fn drop(&mut self) {
        crate::db::lock::lock_or_recover(&self.running, "bot_chat::DB_WRITE_LOCK")
            .remove(&self.task_id);
    }
}

/// 任务执行聊天化（一次性执行 = 一个新会话，不再 headless 黑箱）：
///
/// 评审已确认的决策：
/// 1. 一次执行 = 一个新会话（执行质量优先，不带杂历史）
/// 2. busy 时执行不排队（新会话立即开跑），只有"自动跳转查看"排队
/// 3. 会话膨胀靠手动删除（不做自动清理；删除不影响已沉淀记忆）
/// 4. 不做"查看执行对话"入口和 last_session_id 关联——直接在聊天窗口按前缀找
/// 5. 批量执行：每张卡一个独立新会话（单卡失败不污染）
///
/// 统一入口 run_task_in_chat 供 🤖 按钮 / ⏰ 定时 / 📦 批量三路径
/// 共用（取代原 run_task_in_chat 的 headless 模式——定时任务不再是黑箱，全程流式可见、
/// 可按会话 /stop、永久落库可回看）。
///
/// 执行来源（会话标题前缀 + chat-open-session 事件 origin 字段）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskExecOrigin {
    /// 🤖 按钮 / 选卡说「完成它」
    Manual,
    /// ⏰ 定时调度触发
    Scheduled,
    /// 📦 聊天批量执行（每卡一个独立会话）
    Batch,
}

impl TaskExecOrigin {
    fn title_prefix(self) -> &'static str {
        match self {
            TaskExecOrigin::Manual => "📋 任务：",
            TaskExecOrigin::Scheduled => "⏰ 定时：",
            TaskExecOrigin::Batch => "📦 批量：",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TaskExecOrigin::Manual => "manual",
            TaskExecOrigin::Scheduled => "scheduled",
            TaskExecOrigin::Batch => "batch",
        }
    }
}

/// run_task_in_chat 的返回：新会话 id + 执行结果（前端靠 chat-open-session 事件，
/// 不依赖返回值；调度器用 result.text 写 ⏰ 兜底摘要）
pub struct TaskChatRun {
    pub session_id: String,
    pub result: BotChatResult,
}

/// 创建执行会话（标题 = 来源前缀 + 任务标题）并把任务块作为 user 消息落库，
/// 广播 chat-open-session 给前端（busy 时排队提示，见设计第 5 节）。
pub(super) async fn create_exec_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    task: &crate::db::Task,
    origin: TaskExecOrigin,
) -> CommandResult<String> {
    let title = format!(
        "{}{}",
        origin.title_prefix(),
        crate::bot::truncate_for_log(task.title.trim(), 30)
    );
    let block = build_task_block(task);
    let app_for_audit = app.clone();
    let session = crate::db::lock::with_write_conn(app, move |conn| {
        // 建会话与任务块落库包同一事务：任务块写失败时会话不该留下空壳
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let s = crate::db::bot_session_create_inner(&tx, Some(title))?;
        crate::db::bot_history_save_audited(
            &app_for_audit,
            &tx,
            &s.id,
            &[crate::db::BotMsgRow {
                role: "user".into(),
                content: block,
                refs_json: None,
                thinking: None,
                tools_json: None,
                turn_status: None,
            }],
        )?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(s)
    })
    .await
    .map_err(CommandError::DbError)?;
    // per-onion 配置绑定（两层配置的 DB 层）：见 bot_onions 模块头
    crate::bot_onions::bind_session(app, &session.id);
    let _ = app.emit_to(
        "main",
        "chat-open-session",
        serde_json::json!({
            "sessionId": session.id,
            "taskId": task.id,
            "title": session.title,
            "origin": origin.as_str(),
        }),
    );
    Ok(session.id)
}

/// 执行会话的 assistant 回复落库（user 任务块 + 回复整体覆盖写，bot_history_save_inner
/// 语义即全量覆盖）。持久化失败只记审计——执行结果已经产生，记录缺失不阻断返回。
pub(super) async fn persist_exec_reply<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    task: &crate::db::Task,
    sid: &str,
    reply: &CommandResult<BotChatResult>,
) {
    let (content, refs) = match reply {
        Ok(r) => (r.text.clone(), serde_json::to_string(&r.task_refs).ok()),
        Err(e) => (format!("⚠️ 执行失败：{}", e.message()), None),
    };
    let block = build_task_block(task);
    let sid = sid.to_string();
    let app_for_audit = app.clone();
    let r = crate::db::lock::with_write_conn(app, move |conn| {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        crate::db::bot_history_save_audited(
            &app_for_audit,
            &tx,
            &sid,
            &[
                crate::db::BotMsgRow {
                    role: "user".into(),
                    content: block,
                    refs_json: None,
                    thinking: None,
                    tools_json: None,
                    turn_status: None,
                },
                crate::db::BotMsgRow {
                    role: "assistant".into(),
                    content,
                    refs_json: refs,
                    thinking: None,
                    tools_json: None,
                    turn_status: None,
                },
            ],
        )?;
        tx.commit().map_err(|e| e.to_string())
    })
    .await;
    match r {
        Ok(()) => {}
        Err(e) => {
            crate::audit_event!(app, crate::audit::AuditLevel::Warn, "task_exec.persist_failed",
                "error" => e);
        }
    }
}

/// 任务执行统一入口（生产薄壳：run_model_loop 真实循环；Wry 命令/调度路径用。
/// 测试请用 run_task_in_chat_with（泛型 Runtime + 注入模型循环））
pub async fn run_task_in_chat(
    app: &AppHandle,
    task_id: &str,
    origin: TaskExecOrigin,
) -> CommandResult<TaskChatRun> {
    run_task_in_chat_impl(app, task_id, origin).await
}

pub(super) async fn run_task_in_chat_impl(
    app: &AppHandle,
    task_id: &str,
    origin: TaskExecOrigin,
) -> CommandResult<TaskChatRun> {
    // 执行 trace 挂钩——壳造槽，闭包填 LoopTrace 统计，
    // 内核（run_task_in_chat_with）填归属并收尾。
    // 轮数走配置解析（config maxRounds，钳 5..=200；默认 50）
    let cfg_for_rounds = crate::bot::load_config(app);
    let hook: std::sync::Arc<std::sync::Mutex<crate::trace_sink::TraceCapture>> =
        Default::default();
    let hook_for_run = hook.clone();
    // 失败轨迹的执行明细回传槽：Err 时 run_model_loop 把已跑出的统计写这里，
    // 熔断/API 失败的 trace 才不会记成 turns=0
    let failed_trace = std::sync::Arc::new(std::sync::Mutex::new(None));
    let failed_for_run = failed_trace.clone();
    run_task_in_chat_with(
        app,
        task_id,
        origin,
        Some(hook),
        crate::bot::params::resolve_ask_budget_cfg(&cfg_for_rounds),
        |app2, msgs, stop| async move {
            // 任务执行链路同样带墙钟（档位全局墙钟的 60% 份额）：与主聊天
            // 同一口径，任务卡执行也不该能无限拖。
            let main_wall_secs = crate::bot::params::resolve_main_wall_secs(&cfg_for_rounds);
            let wall_budget = crate::bot_model_loop::WallBudget {
                deadline: std::time::Instant::now()
                    + std::time::Duration::from_secs(main_wall_secs),
                reserve: std::time::Duration::from_secs(
                    crate::bot::params::subagent_wall_reserve_secs(main_wall_secs),
                ),
                max_wall_seconds: main_wall_secs,
            };
            let outcome = crate::bot_model_loop::run_model_loop(
                app2,
                msgs,
                crate::bot::params::resolve_max_rounds_cfg(&cfg_for_rounds) as usize,
                &stop,
                None,
                // 任务执行链路不挂单次覆盖，按 bot-config.json 全局默认（RE-1）
                None,
                None, // per-onion 温度覆盖：任务执行链路不挂
                Some(&failed_for_run),
                Some(wall_budget),
            )
            .await;
            // LoopTrace 统计进 trace 汇总槽（轮数/工具数/tokens → exec_traces 行）
            let captured = match &outcome {
                Ok((_, _, trace)) => Some(trace.clone()),
                Err(_) => failed_for_run.lock().ok().and_then(|g| g.clone()),
            };
            if let (Some(trace), Ok(mut c)) = (captured, hook_for_run.lock()) {
                c.stats = Some(trace);
            }
            outcome.map(|(text, refs, _)| (text, refs))
        },
    )
    .await
}

/// run_task_in_chat 内核（模型循环注入——测试接 mock LLM server 全链路驱动，
/// 与 run_model_loop_core / truncate_with_summary_core 同先例）。
/// 流程：开关/防重入/任务校验 → 建新会话 + 任务块落库 + chat-open-session
/// → ChatGuard（执行期同会话 bot_chat 插话被拒）→ 记忆注入 → 模型循环
/// → assistant 回复落库（失败也落 ⚠️ 行）→ 失败沉淀 lesson。
pub async fn run_task_in_chat_with<R: tauri::Runtime, Run, Fut>(
    app: &tauri::AppHandle<R>,
    task_id: &str,
    origin: TaskExecOrigin,
    // 执行 trace 挂钩（None = 不采集，测试注入缝用）。
    // 内核负责建 trace（begin）与收尾（end）——成对执行，中途无早退分支。
    trace_hook: Option<std::sync::Arc<std::sync::Mutex<crate::trace_sink::TraceCapture>>>,
    // ask_user 提问预算（Wry 壳从 bot 参数解析后传入；内核是泛型 Runtime，
    // 读不了 Wry 绑定的 bot-config。测试传任意值即可）
    ask_budget: u8,
    run: Run,
) -> CommandResult<TaskChatRun>
where
    Run: FnOnce(tauri::AppHandle<R>, Vec<serde_json::Value>, StopGuard) -> Fut,
    Fut: std::future::Future<Output = CommandResult<(String, Vec<TaskRef>)>>,
{
    // 开关关闭时明确拒绝
    if !crate::bot_slash::bot_enabled(app) {
        return Err(CommandError::BotDisabled);
    }
    // 防重入：同一任务卡已有执行实例在跑 → 直接拒绝（RAII 守卫随函数返回/panic 自动释放）
    let Some(_exec_guard) = ExecGuard::acquire(app, task_id) else {
        // 拒绝也留痕：否则无法区分「用户在前次执行未结束时重复触发」与「守卫泄漏」
        crate::bot::audit_log(
            app,
            &format!(
                "execute_task_rejected | id: {} | 已有执行实例在跑（防重入拦截）",
                crate::bot::truncate_for_log(task_id, 60)
            ),
        );
        return Err(CommandError::TaskInvalidState {
            reason: "该任务卡正在执行中，请等待完成后再触发".into(),
        });
    };
    // 读库失败要上抛：吞成空列表等于把 DB 故障说成「任务卡不存在或已在回收站」，
    // 用户看到的是一句根本不成立的结论，真正的故障原因被埋掉
    let task = crate::db::db_load_for(app)
        .await?
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
        .ok_or("任务卡不存在或已在回收站")?;
    if task.column == crate::db::TaskStatus::Done {
        return Err(CommandError::TaskInvalidState {
            reason: "这张卡已标记完成；如需重新执行，先在卡片上取消完成".into(),
        });
    }
    if task.archived == Some(true) {
        return Err(CommandError::TaskInvalidState {
            reason: "任务已归档，不能执行；请先恢复".into(),
        });
    }
    crate::bot::audit_log(
        app,
        &format!(
            "execute_task | id: {} | title: {} | origin: {}",
            task.id,
            crate::bot::truncate_for_log(&task.title, 60),
            origin.as_str()
        ),
    );
    // 1. 新会话 + 任务块 user 消息落库 + chat-open-session 广播
    let sid = create_exec_session(app, &task, origin).await?;
    // 2. ChatGuard（设计 3.4：bot_execute_task 纳入会话锁——执行期间同会话的
    // bot_chat 插话会被拒「稍候再发」，防流式/历史交错）。新会话正常不会冲突，
    // 冲突说明守卫串号，按内部错误处理。
    let _chat_guard = match ChatGuard::acquire(app, Some(&sid)) {
        Ok(g) => g,
        Err(()) => {
            return Err(CommandError::Internal(format!(
                "执行会话 {sid} 已被占用（会话锁串号）"
            )));
        }
    };
    // 登记 session_id → TaskExecOrigin 映射，tool_link_file_to_task 内部查询。
    // 末尾无论成败都要 unregister_exec_session 清理。守卫成功后再登记——
    // acquire 失败的早退分支不应泄漏登记（无人 unregister）。
    crate::tool_guard::register_exec_session(&sid, origin);
    // 执行提问授权（ask_user 按 session_id 查表）：
    // - 手动执行任务卡：默认开启（预算 + 假设兜底 + 可停止的等待）——
    //   交互式技能的确认点（题材/书名等）在手动执行里也要有通道；
    // - 预算 = bot 参数 tools.ask_budget（Wry 壳解析后传入）；
    //   定时/批量保持无人值守自治（不注册，调用即回落假设）
    let ask_registered = if origin == TaskExecOrigin::Manual {
        crate::ask_engine::register_ask_context(app, &sid, &task.id, &task.title, ask_budget);
        true
    } else {
        false
    };
    // 建执行 trace + 注册 session→trace 映射（span/file_change 采集开关）。
    // 位置保证：此后到函数尾无早退分支，end_trace 必然成对执行。观测面失败降级 None。
    if let Some(hook) = &trace_hook {
        let trace_id = crate::trace_sink::begin_trace(
            app,
            &sid,
            Some(task.id.clone()),
            origin.as_str(),
            Some(task.title.clone()),
        )
        .await;
        if let Ok(mut c) = hook.lock() {
            c.trace_id = trace_id;
            c.session_id = Some(sid.clone());
        }
    }
    let stop = StopGuard::new(app, true, Some(sid.clone()));
    // 收尾判定停止用：stop 本体 move 进 run，先留只读令牌
    let stop_token = stop.token();
    let block = build_task_block(&task);
    let mut msgs = vec![
        serde_json::json!({"role": "system", "content": format!("{}\n\n{}\n\n{}", EXECUTE_SYSTEM_PROMPT, gen_dir_rule(app), build_skill_block_for(app, Some(&task.title)).await)}),
        serde_json::json!({"role": "user", "content": block}),
    ];
    // 任务卡执行/定时调度也注入记忆块——助手执行任务时知道用户
    // 偏好；查询 = 任务标题+备注前 200 字；失败静默降级为无记忆块（injection_block 内部兜底）。
    let mem_query: String = format!("{} {}", task.title, task.note.as_deref().unwrap_or(""))
        .chars()
        .take(200)
        .collect();
    if let Some(mem_block) = crate::memory::injection_block(app, &mem_query).await {
        msgs.insert(
            1,
            serde_json::json!({"role": "system", "content": mem_block}),
        );
    }
    // 交给机器人：卡片切机器人头像（前端 tasks-changed 广播后实时更新）
    set_bot_assigned(app, &task.id, true).await;
    let outcome = run(app.clone(), msgs, stop).await;
    // 执行结束（无论成败）：清除标记，恢复用户头像
    set_bot_assigned(app, &task.id, false).await;
    let outcome = outcome.map(|(text, refs)| BotChatResult {
        text,
        task_refs: refs,
        busy: false,
        trace_id: None,
    });
    // trace 收尾（无论成败；与 begin 成对）+ 注销 session→trace 映射
    if let Some(hook) = &trace_hook {
        crate::trace_sink::end_trace(
            app,
            hook,
            outcome.is_ok(),
            // 用户停止也返回 Ok（「⏹ 已停止」），按令牌判定收尾为 stopped
            stop_token.stopped(),
            outcome.as_ref().err().map(|e| e.to_string()),
        )
        .await;
    }
    // assistant 回复落库（失败也落 ⚠️ 行——会话即执行记录，留证可回看）
    persist_exec_reply(app, &task, &sid, &outcome).await;
    //  收尾：解除 session 注册（无论成败），按 TaskExecOrigin 分流落绑定通知。
    crate::tool_guard::unregister_exec_session(&sid);
    // 提问上下文成对注销（同执行会话生命周期）
    if ask_registered {
        crate::ask_engine::unregister_ask_context(app, &sid);
    }
    let task_column = crate::db::db_load_for(app).await.ok().and_then(|tasks| {
        tasks
            .into_iter()
            .find(|t| t.id == task_id)
            .map(|t| t.column)
    });
    if let Some(artifacts) =
        crate::bot_artifacts::should_emit(app, task_id, origin, task_column.map(|s| s.as_str()))
            .await
    {
        // 通知中心落一条持久化消息（原  挂件 artifact-batch-ready 弹窗已下线：
        // 消息互不覆盖、重启不丢）。失败不阻断执行收尾，stderr 留痕。
        let paths: Vec<String> = artifacts.iter().map(|a| a.path.clone()).collect();
        let origin_label = match origin {
            TaskExecOrigin::Manual => "手动执行",
            TaskExecOrigin::Scheduled => "定时执行",
            TaskExecOrigin::Batch => "批量执行",
        };
        let payload = serde_json::json!({
            "taskId": task_id,
            "taskTitle": task.title,
            "origin": origin_label,
            "paths": paths,
        });
        let title = format!(
            "任务「{}」完成，{} 个产物待绑定",
            task.title,
            payload["paths"].as_array().map(|a| a.len()).unwrap_or(0)
        );
        let body = format!("{origin_label} · 勾选要绑定到任务卡的文件");
        let insert = crate::db::open_db(app).and_then(|conn| {
            crate::notifications::notif_insert(
                &conn,
                &format!("artifact:{task_id}:{sid}"),
                crate::notifications::KIND_ARTIFACT,
                &title,
                &body,
                &payload,
            )
        });
        match insert {
            Ok(true) => crate::notifications::emit_changed(app),
            Ok(false) => {}
            Err(e) => eprintln!("[notifications] 产物绑定通知落库失败：{e}"),
        }
    }
    match outcome {
        Ok(result) => Ok(TaskChatRun {
            session_id: sid,
            result,
        }),
        Err(e) => {
            // 任务执行失败自动沉淀一条 lesson（source=system，
            // 语义去重合并同类失败）；写失败只记审计，不影响原错误返回
            crate::memory::auto_lesson_on_task_failure(app, &task.title, &e.message()).await;
            Err(e)
        }
    }
}
