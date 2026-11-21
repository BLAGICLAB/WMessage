//! 子 agent 执行体：run_subagent 及其生命周期配套。
//!
//! runner 负责「跑完一个子 agent」的全过程：排队取闸位 → 建会话 → 跑工具循环
//! → 收尾落库 → 回报父会话。取消 watcher 与停滞判停也在这条路径上。

use crate::bot_chat::ChatGuard;
use crate::bot_orchestrator::{
    acceptance_criteria_of, apply_machine_verdicts, artifact_dir_of, classify_outcome, db_locked,
    eval_machine_criteria, gen_dir_rule_for, reconcile_machine_override, repair_final_json_once,
    spawn_parent_report, SpawnOnion, SubagentStage,
};
use crate::db::{now_ms, SubagentProfile, SubagentRow, SubagentStatus};
use crate::error::{CommandError, CommandResult};
use futures_util::FutureExt;
use std::path::PathBuf;
use tauri::AppHandle;

/// 取消令牌的 RAII 反注册守卫。
///
/// `subagent_stops` 里每登一条就要负责抹掉：漏一条 = cancel_subagent 之后
/// 还能停一个早该消失的子 agent，那张表也只增不减。手工 remove 漏掉的是
/// panic 路径（runner panic 后整条收尾被跳过），故用 Drop 兜住。
pub(crate) struct StopTokenGuard {
    app: AppHandle,
    id: String,
}
impl Drop for StopTokenGuard {
    fn drop(&mut self) {
        crate::db::lock::lock_or_recover(
            &crate::app_state::subagent_stops(&self.app),
            "app_state::subagent_stops",
        )
        .remove(&self.id);
    }
}
/// 终态落库：改走 Effects 队列（subagent_finalize kind）。进程内指数重试
/// 不持久化——崩溃即丢，行永久停在 queued/running；队列把「终态未落」变成
/// 可恢复待办（重试幂等：同一终态重复写无害，已终态直接成功），超限审计兜底。
async fn enqueue_terminal_write(
    app: &AppHandle,
    id: &str,
    status: SubagentStatus,
    finished_at: i64,
    err_msg: &str,
) -> Result<(), CommandError> {
    let payload = serde_json::json!({
        "subagentId": id,
        "status": status.as_str(),
        "finishedAt": finished_at,
        "error": err_msg,
    })
    .to_string();
    let key = format!("subagent_finalize:{id}");
    let (key2, payload2) = (key.clone(), payload);
    crate::db::lock::with_write_conn(app, move |conn| {
        crate::db::effects::enqueue(
            conn,
            crate::bot::effects::KIND_SUBAGENT_FINALIZE,
            &key2,
            &payload2,
            crate::db::now_ms(),
        )
    })
    .await
    .map_err(CommandError::DbError)?;
    crate::bot::effects::kick();
    Ok(())
}

/// runner panic → 普通失败（与 runner 返错同一层，收尾路径不变）。
///
/// 单独成纯函数：panic 的三种 payload（&str / String / 其它）由
/// audit::panic_payload_msg 归一，这里只负责截断与包装。
pub(crate) fn panic_to_command_error(msg: &str) -> CommandError {
    CommandError::Internal(format!(
        "执行器异常退出（panic）：{}",
        crate::bot::truncate_for_log(msg, 200)
    ))
}
/// 子 agent 执行引擎：独立会话 + 工具循环 + 收尾落库。spawn 异步包装末尾派发
/// （非阻塞）。执行基建复用 exec 家族（ChatGuard/ExecGuard/StopGuard/tool_guard），
/// 但入口/存储/事件与 bot_execute_task 各自独立（设计 §2）。
/// 具体化 `tauri::AppHandle`（= Wry）：run_model_loop 生产壳即该签名
/// （mock 链路走 run_model_loop_core，同 llm_integration 分层）。
#[allow(clippy::too_many_lines)]
pub(crate) async fn run_subagent(
    app: AppHandle,
    subagent_id: String,
    stage: Option<SubagentStage>,
    onion: Option<SpawnOnion>,
) {
    let sid_for_load = subagent_id.clone();
    let row = match db_locked(&app, move |conn| {
        crate::db::load_subagent(conn, &sid_for_load)
            .map_err(CommandError::DbError)?
            .ok_or(CommandError::TaskNotFound(sid_for_load))
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // 加载失败也要落终态，防 ghost queued 行永挂；parent 会话未知（行
            // 没读到），发不带归属的收尾事件——前端只重拉不弹提示
            abort_and_finish(&app, &subagent_id, None, &e.message(), &e.message()).await;
            return;
        }
    };
    // 派发后、开跑前已被取消/终态 → 不启动。行已终态但事件没人发过（cancel
    // 包装不 emit），这里补上，前端才不会把该行停在 queued
    if row.status != SubagentStatus::Queued {
        emit_subagent_finished(
            &app,
            &row.id,
            row.parent_session_id.as_deref(),
            row.status,
            "",
        );
        return;
    }
    // 轮数与主会话共用同一套安全网（常量）；配置档 `max_turn` 只收紧——
    // 配置档是权限上限，不是「优先于调用方的指令」，取 min 而非覆盖。
    let max_rounds: usize = {
        let shared = crate::bot::constants::MAX_ROUNDS as usize;
        match onion.as_ref().and_then(|o| o.max_turn) {
            Some(cap) => shared.min(cap as usize).max(1),
            None => shared,
        }
    };

    // 排队闸 + 槽后复核 + Running 落库（见 acquire_gate_slot）
    let Some(_gate_ticket) = acquire_gate_slot(&app, &row).await else {
        return;
    };
    let started = std::time::Instant::now();

    let Some(artifact_dir) = prepare_artifact_dir(&app, &row).await else {
        return;
    };
    let Some((session_id, wrapper)) =
        setup_subagent_session(&app, &row, &artifact_dir, &onion).await
    else {
        return;
    };

    // per-onion 配置绑定（两层配置的 DB 层）：读会话的 onion_config 元列 + 落盘
    // yaml，解析校验后进进程内缓存。**必须在守卫注册之前**——守卫要拿
    // 「profile ∩ allowed」的交集作为白名单判据，配置没绑就没交集可算。
    crate::bot_onions::bind_session(&app, &session_id);
    // 白名单全集：配置档在就取交集，不在就是 profile 白名单原样（语义不变）
    let subagent_tools: Vec<&'static str> =
        match crate::bot_onions::cached_for_session(Some(session_id.as_str())) {
            Some(cfg) => crate::bot_onions::effective_tools(row.profile, &cfg.allowed),
            None => crate::bot::registry::profile_whitelist(row.profile).to_vec(),
        };
    // 提示词里的工具行要用同一份交集（守卫白名单与提示词清单同源）
    let prompt_tools = subagent_tools.join("、");

    // 工具守卫注册（白名单闸 + spawn 递归身份校验 + read_own_card/write_artifact 定位）。
    // RAII 守卫：Drop 反注册——runner 中途 panic 也不泄漏条目（ 采纳）
    // 轮数仪表句柄：循环每轮自报时写进来，供 check_subagent 的 progress 读。
    // 与 ctx 里的 used_rounds 是同一个 Arc（守卫按值克隆 ctx，Arc 保证同源）。
    let rounds_gauge = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // 工具调用计数：与 ctx 里的 used_tool_calls 同一个 Arc（守卫按值克隆 ctx）。
    // 消费方是 read_own_card 的实时余量与动作流「已用工具数」——收尾时的预算
    // 触达判定**不读它**：靠正文的 FUSE_MARKER / ROUNDS_FUSE_MARKER 标记
    // （subagent_repair），熔断路径必带标记，计数器兜底是想象的机制。
    // 触达必须落 budget_exceeded 不能落 succeeded（父 agent 提示词教它「failed /
    // budget_exceeded 都要读 error 决定重试」，报成 succeeded 就不重试）。
    let used_tool_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // 进展心跳：从进入 running 起算，模型循环薄壳的流式事件出口每次刷新
    let last_progress_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_ms() as u64));
    // 工具在跑标记：看门狗据此在工具执行期挂起停滞判定（长工具执行期无心跳，
    // 与 600s 阈值零余量）。bot-tool-running/done 事件翻转，见 shell 的 emit 出口。
    let tool_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _session_guard = crate::tool_guard::SubagentSessionGuard::new(
        &session_id,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: row.id.clone(),
            task_id: row.task_id.clone(),
            profile: row.profile,
            tools: std::sync::Arc::new(subagent_tools),
            artifact_dir: artifact_dir.clone(),
            acceptance_criteria_count: self_judged_criteria_count(&row),
            used_tool_calls: used_tool_calls.clone(),
            used_rounds: rounds_gauge.clone(),
            last_progress_ms: last_progress_ms.clone(),
            tool_in_flight: tool_in_flight.clone(),
        },
    );

    // 会话锁（用户向围观会话发消息 → 友好拒绝）+ 停止守卫（interactive=围观流式）
    let chat_guard = ChatGuard::acquire(&app, Some(session_id.as_str()));
    if chat_guard.is_err() {
        // 此处行已是 Running——中止必须落终态，不能悬挂
        let sid_failed = row.id.clone();
        if let Err(e) = db_locked(&app, move |conn| {
            crate::db::update_subagent_status(
                conn,
                &sid_failed,
                SubagentStatus::Failed,
                now_ms(),
                Some("session_busy"),
            )
            .map_err(CommandError::DbError)
        })
        .await
        {
            // 回滚留痕：行已 Cancelled 时 update_subagent_status 会拒绝置 Failed，
            // 终态由 watcher 兜底——失败可见非静默（围观/审计设计保留会话行）
            eprintln!(
                "[subagent] session_busy 中止回滚失败（subagent: {}）：{e}",
                row.id
            );
        }
        crate::bot::audit_log(
            &app,
            &format!("subagent_runner_abort | id: {} | err: session_busy", row.id),
        );
        emit_subagent_finished(
            &app,
            &row.id,
            row.parent_session_id.as_deref(),
            SubagentStatus::Failed,
            "",
        );
        return;
    }
    let stop = crate::bot_slash::StopGuard::new(&app, true, Some(session_id.clone()));
    // 取消令牌登记：cancel_subagent 持句柄远程停止。已知窗口（可接受：
    // ChatGuard 成功到令牌登记之间收到的 cancel 落不到令牌——由 2s watcher 兜底
    // （行已 cancelled → watcher 置 stop）。
    crate::db::lock::lock_or_recover(
        &crate::app_state::subagent_stops(&app),
        "app_state::subagent_stops",
    )
    .insert(row.id.clone(), stop.token());
    // 取消令牌的 RAII 反注册：任何提前返回 / panic 路径都不会把条目留在
    // subagent_stops 里（残留令牌让 cancel_subagent 停一个早该消失的子
    // agent，这张表也只增不减）。下面还有一次显式 remove（早于收尾落库），
    // 守卫那次是幂等的兜底。
    let _stop_token_guard = StopTokenGuard {
        app: app.clone(),
        id: row.id.clone(),
    };

    // 取消 watcher（设计 §4.3 硬停）：2s 轮询行状态，进终态即停工具循环；
    // 同一轮询顺带判停滞（子 agent 没有墙钟，这是它唯一的时间兜底）
    let stalled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = spawn_cancel_watcher(
        &app,
        &stop,
        &row.id,
        &last_progress_ms,
        &tool_in_flight,
        &stalled,
    );

    // 消息装配 + 配置档绑定 + 跨进程续跑装载（见 assemble_messages）
    let (msgs, max_rounds, bound_onion) = assemble_messages(
        &app,
        &row,
        stage,
        &session_id,
        &artifact_dir,
        &prompt_tools,
        &wrapper,
        max_rounds,
    );

    // 工具循环：轮数预算（熔断→budget_exceeded；排队不计——running 起算）。
    // 没有墙钟：时间兜底在工具级超时与停滞判定，不在整条 future 上掐。
    //
    // 收尾 JSON 解析失败时**补答一次**（结构化输出三段修复的第 3 段）：产出
    // 完整、机检全过，只因最后一段不是 JSON 就判 failed，是完成率最大的单项
    // 黑洞。
    // 盘上交付件的 mtime 下限：本轮开跑前落盘的 verif*.json 一律不算（跨会话
    // 旧报告冒充防线，与模型循环薄壳里 B 的探测同一个口径、各自取各自的开跑时刻）
    let started_at = std::time::SystemTime::now();
    let run = execute_with_recovery(
        &app,
        &stop,
        &row.id,
        &artifact_dir,
        msgs,
        max_rounds,
        // 配置档的采样温度（f32 → f64 无损）。这是 yaml `temperature:` 字段的
        // 唯一消费者；None 时用模型条目的 temperature。
        bound_onion
            .as_ref()
            .and_then(|c| c.temperature)
            .map(f64::from),
        self_judged_criteria_count(&row),
        started_at,
    );
    // runner panic 隔离（bot_scheduler 的既有范式）：panic 会跳过下面所有收尾
    // ——取消令牌反注册、状态落库、subagent-finished 事件——前端那一行永远转圈，
    // 行状态也停在 running（只能靠重启清扫僵尸行）。故把 panic 捕获成普通失败，
    // 让收尾路径照常走完：行落 failed，事件发出去，前端不再转圈。
    let outcome = match std::panic::AssertUnwindSafe(run).catch_unwind().await {
        // 正常跑完
        Ok(Ok(v)) => Ok(v),
        // runner 自己返错
        Ok(Err(e)) => Err(e),
        // runner panic → 归一成普通失败（与返错同一层，收尾路径不变）
        Err(panic) => {
            let msg = crate::audit::panic_payload_msg(panic.as_ref());
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_panic | id: {} | {} | 已归一为执行失败",
                    row.id,
                    crate::bot::truncate_for_log(&msg, 200)
                ),
            );
            Err(panic_to_command_error(&msg))
        }
    };

    // 收尾清理：watcher 由 AbortOnDrop Drop 保证中止（含 panic/unwind 路径——
    // StopGuard::drop 不置位停止标志，单靠手动 abort 会泄漏）
    let _watcher = watcher;
    // tool_guard 注册走 RAII 守卫（_session_guard Drop 反注册）
    crate::db::lock::lock_or_recover(
        &crate::app_state::subagent_stops(&app),
        "app_state::subagent_stops",
    )
    .remove(&row.id);
    drop(chat_guard);

    // 收尾判定（设计 §6/§7）：classify/停滞改判/机器验收旁证与改判
    let (status, error, result, final_text) = judge_outcome(
        &app,
        &row,
        &outcome,
        stalled.load(std::sync::atomic::Ordering::SeqCst),
        &artifact_dir,
    );
    // usage 记账：模型循环累计的 token 三档随终态同事务落库（返错 / panic
    // 路径拿不到 trace，保持 NULL——对账时按「未记」读，不编 0）
    let usage_for_db = match &outcome {
        Ok((_, _, trace)) => Some((
            trace.prompt_tokens,
            trace.completion_tokens,
            trace.cache_read_tokens,
        )),
        _ => None,
    };
    // 返回 fresh row——下游审计/事件直接用，免二次持锁查询
    let finalized = persist_final_state(
        &app,
        &row,
        &session_id,
        &wrapper,
        status,
        error,
        result,
        &final_text,
        usage_for_db,
    )
    .await;

    // 审计 + 广播 + 前端事件（设计 §3 事件全链按 subagent_id 串链）
    let elapsed = started.elapsed().as_millis() as u64;
    report_and_emit(&app, &row, finalized, status, elapsed).await;
}

/// 审计 + 广播 + 前端事件（设计 §3 事件全链按 subagent_id 串链）。
/// 行的真实终态优先于 classify_outcome：用户取消走到这里时 classify 只会给出
/// Failed（执行被 StopToken 打断），照它上报 = 每次取消都记一条 subagent_failed
/// 并沉淀「失败原因：」的空教训。收尾闭包整体失败拿不到行时回读一次真实终态
///（cancel 抢先后的收尾失败等场景），读不到才回落 classify 的观感值。
async fn report_and_emit(
    app: &AppHandle,
    row: &SubagentRow,
    finalized: Option<SubagentRow>,
    status: SubagentStatus,
    elapsed: u64,
) {
    let mut row_status = finalized.as_ref().map(|r| r.status);
    if row_status.is_none() {
        row_status = load_subagent_status(app, &row.id).await;
    }
    let effective_status = effective_status(row_status, status);
    let event = match effective_status {
        SubagentStatus::Succeeded => "subagent_done",
        SubagentStatus::BudgetExceeded => "subagent_budget_exceeded",
        SubagentStatus::Cancelled => "subagent_cancelled",
        _ => "subagent_failed",
    };
    // fresh 直接取自收尾闭包返回值（不再二次持锁查询）
    let summary = finalized
        .as_ref()
        .and_then(|r| r.result_json.as_deref())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("summary")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });
    let row_error = finalized
        .as_ref()
        .and_then(|r| r.error.clone())
        .unwrap_or_default();
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        event,
        &[
            ("subagent_id", row.id.clone()),
            ("task_id", row.task_id.clone().unwrap_or_default()),
            (
                "parent_session_id",
                row.parent_session_id.clone().unwrap_or_default(),
            ),
            ("profile", row.profile.as_str().to_string()),
            ("trace_id", row.trace_id.clone()),
            ("elapsed_ms", elapsed.to_string()),
            ("error", row_error.clone()),
        ],
    );
    crate::bot::audit_log(
        app,
        &format!(
            "{} | id: {} | status: {} | elapsed_ms: {} | error: {}",
            event,
            row.id,
            effective_status.as_str(),
            elapsed,
            crate::bot::truncate_for_log(&row_error, 120)
        ),
    );
    // Failed / BudgetExceeded 自动沉淀 lesson：与 run_task_in_chat 的
    // auto_lesson_on_task_failure 同口径（source=system, scenario=task_exec,
    // tags=[lesson, task_exec]）——store 语义去重自动合并同类失败教训。
    // 走 fire-and-forget，不阻塞 finalize 返回路径。
    // Cancelled 是用户选择不沉淀，Succeeded 无失败不沉淀。
    if matches!(
        effective_status,
        SubagentStatus::Failed | SubagentStatus::BudgetExceeded
    ) {
        let app_for_lesson = app.clone();
        let lesson_title = row.objective.clone();
        let lesson_error = row_error.clone();
        tauri::async_runtime::spawn(async move {
            crate::memory::auto_lesson_on_task_failure(
                &app_for_lesson,
                &lesson_title,
                &lesson_error,
            )
            .await;
        });
    }
    emit_subagent_finished(
        app,
        &row.id,
        row.parent_session_id.as_deref(),
        effective_status,
        summary.as_deref().unwrap_or_default(),
    );
    spawn_parent_report(app, row, effective_status);
}

/// 加载/产物目录/会话 setup 失败的统一中止路径：终态入队（兜 DB 锁瞬态，防
/// 行永挂 queued/running）+ 审计 + 收尾事件。调用方 await 后直接 return。
async fn abort_and_finish(
    app: &AppHandle,
    id: &str,
    parent_session_id: Option<&str>,
    audit_reason: &str,
    msg: &str,
) {
    let _ = enqueue_terminal_write(app, id, SubagentStatus::Failed, now_ms(), msg).await;
    crate::bot::audit_log(
        app,
        &format!("subagent_runner_abort | id: {id} | err: {audit_reason}"),
    );
    emit_subagent_finished(app, id, parent_session_id, SubagentStatus::Failed, "");
}

/// 排队闸（global 3 / per-session 2，超限等待不失败）。排队可取消——每轮退避
/// 同时查行终态，取消即退队；Running 改在**拿到槽位并复核后**才写，排队中的
/// 行不再假挂运行中。返回 None = 应中止（已审计/已发事件），调用方直接 return。
async fn acquire_gate_slot(
    app: &AppHandle,
    row: &SubagentRow,
) -> Option<crate::bot::subagent_spawn::GateTicket> {
    // 评审 M 采纳 + 挪线修正：取消检测的 DB 读不该压在 async worker 上——
    // open_db 含建目录 + PRAGMA + 全量 DDL 迁移，500ms 一次就是反复整套迁移。
    // 闸门闭包是同步 FnMut（放不进 await），改用排队期专用轻连接：直接 open
    //（不跑 DDL）+ busy_timeout，单行 SELECT 只花微秒级；拿到槽位后的复核
    // 仍走 open_db 全量路径。
    let gate_conn: Option<std::sync::Mutex<rusqlite::Connection>> = {
        let path = crate::db::data_dir(app).join("wmessage.db");
        rusqlite::Connection::open(&path)
            .map(|conn| {
                let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
                std::sync::Mutex::new(conn)
            })
            .ok() // 开不出连接 = 取消检测降级为 no-op（槽后复核兜底）
    };
    let sid_gate_check = row.id.clone();
    let gate_tick = std::cell::Cell::new(0u32);
    let mut cancelled_while_queued = move || {
        let n = gate_tick.get();
        gate_tick.set(n.wrapping_add(1));
        if !n.is_multiple_of(10) {
            return false;
        }
        let Some(conn) = &gate_conn else {
            return false; // DB 读失败不误判取消（拿到槽位后的复核兜底）
        };
        let Ok(conn) = conn.lock() else {
            return false;
        };
        crate::db::load_subagent(&conn, &sid_gate_check)
            .ok()
            .flatten()
            .map(|r| r.status.is_terminal())
            .unwrap_or(false)
    };
    let ticket = crate::bot_orchestrator::SubagentGate::wait_slot_cancellable(
        row.parent_session_id.clone(),
        &mut cancelled_while_queued,
    )
    .await;
    let ticket = match ticket {
        Some(t) => t,
        // 排队中被取消：行已被 cancel 写成终态，此处仅退出（守卫 RAII 清理）
        None => {
            crate::bot::audit_log(
                app,
                &format!(
                    "subagent_runner_abort | id: {} | err: cancelled_while_queued",
                    row.id
                ),
            );
            emit_subagent_finished(
                app,
                &row.id,
                row.parent_session_id.as_deref(),
                SubagentStatus::Cancelled,
                "",
            );
            return None;
        }
    };
    {
        // 拿到槽位后复核（cancel-vs-runner 竞态闸）：行已终态 = cancel 先赢 →
        // 中止；DB 读失败保守中止（执行已取消的任务比不跑更糟
        let sid_still_queued = row.id.clone();
        let state_now = db_locked(app, move |conn| {
            Ok(crate::db::load_subagent(conn, &sid_still_queued)
                .ok()
                .flatten()
                .map(|r| r.status))
        })
        .await
        .unwrap_or(None);
        match state_now {
            Some(s) if !s.is_terminal() => {}
            other => {
                crate::bot::audit_log(
                    app,
                    &format!(
                        "subagent_runner_abort | id: {} | err: aborted_while_queued | state: {}",
                        row.id,
                        other
                            .map(|s| s.as_str().to_string())
                            .unwrap_or_else(|| "db_read_failed".into()),
                    ),
                );
                // 拿到槽位后的复核撞上终态：行已由 cancel 写成 cancelled；
                // 读不到行（DB 失败/行已没）时不要谎报——只留审计，等启动清扫
                if let Some(s) = other {
                    emit_subagent_finished(app, &row.id, row.parent_session_id.as_deref(), s, "");
                }
                return None;
            }
        }
        // Running 写库挪到槽位拿到之后（排队窗口内行保持 queued 语义）。
        // 写失败必须中止（评审 HIGH 采纳）：recheck 与本写之间 cancel 可抢先写
        // Cancelled，update_subagent_status 因非法状态转移报错——吞错等于在
        // 已取消任务上继续跑
        let sid_running = row.id.clone();
        let run_write = db_locked(app, move |conn| {
            crate::db::update_subagent_status(
                conn,
                &sid_running,
                SubagentStatus::Running,
                now_ms(),
                None,
            )
            .map_err(CommandError::DbError)
        })
        .await;
        if let Err(e) = run_write {
            crate::bot::audit_log(
                app,
                &format!(
                    "subagent_runner_abort | id: {} | err: running_write_failed | {e}",
                    row.id
                ),
            );
            // 写 Running 被拒基本只有种成因：cancel 已把行写成终态。回读
            // 真实终态上报（直接写 Cancelled 会在「DB 抖动」这类非 cancel
            // 失败上撒谎）；读不到行就只留审计，交给启动清扫收尸
            let st = load_subagent_status(app, &row.id).await;
            if let Some(st) = st {
                emit_subagent_finished(app, &row.id, row.parent_session_id.as_deref(), st, "");
            }
            return None;
        }
    }
    Some(ticket)
}

/// 消息装配：system = 子 agent 通用提示 + profile 段 + 产物目录规则；user = 任务包装。
/// stage 在场：拼 PROFILE_RECON/EXECUTOR/VERIFIER 替代基线 PROFILE_*（设计 §8.5）。
/// profile 仍决定工具白名单档——stage 只换角色语义层。两层解耦，允许调用方：
/// 1) 仅给 stage（profile 自动按 default_profile 派生）；或
/// 2) 同时给 stage + profile（白名单与角色互不干扰，如 verifier=research 的只读够用）。
/// 附：配置档绑定读取 + 跨进程续跑装载（sidecar 存在且属本行 → 全量历史代替
/// 新任务包装，轮数按 RESUME_ROUNDS 收紧）。返回 (msgs, 生效轮数, 绑定的配置档)。
fn assemble_messages(
    app: &AppHandle,
    row: &SubagentRow,
    stage: Option<SubagentStage>,
    session_id: &str,
    artifact_dir: &std::path::Path,
    prompt_tools: &str,
    wrapper: &str,
    max_rounds: usize,
) -> (
    Vec<serde_json::Value>,
    usize,
    Option<crate::bot_onions::OnionConfig>,
) {
    let profile_prompt = match stage {
        Some(SubagentStage::Recon) => crate::prompts::PROFILE_RECON.to_string(),
        Some(SubagentStage::Executor) => {
            // 工具行按实际生效集（profile ∩ 配置档 allowed）——subagent_tools
            // 就是那个交集：写死某一档的话，显式给了别的配置档时清单与白名单
            // 脱节，模型照着调用必撞 dispatch 白名单闸
            crate::prompts::profile_executor(prompt_tools)
        }
        Some(SubagentStage::Verifier) => crate::prompts::PROFILE_VERIFIER.to_string(),
        None => match row.profile {
            SubagentProfile::Research => crate::prompts::PROFILE_RESEARCH.to_string(),
            SubagentProfile::Coder => crate::prompts::PROFILE_CODER.to_string(),
            SubagentProfile::General => crate::prompts::PROFILE_GENERAL.to_string(),
        },
    };
    // wrapper 已由 setup 渲染返回（同一文本：LLM 输入 = 历史落库 = 收尾覆盖源）
    // 配置档段：角色 + 交集后的实际可用工具（§3.4「配置看着生效、实际落空」
    // 要让模型也看得见）；无配置档时是空串，不占位也不改既有提示词。
    let bound_onion = crate::bot_onions::cached_for_session(Some(session_id));
    let onion_note = bound_onion
        .as_ref()
        .map(|cfg| {
            format!(
                "\n\n{}",
                crate::bot_onions::render_prompt_note(cfg, row.profile)
            )
        })
        .unwrap_or_default();
    let system = format!(
        "{}\n\n{}\n\n{}{}",
        crate::prompts::SUBAGENT_BASE,
        profile_prompt,
        gen_dir_rule_for(artifact_dir),
        onion_note,
    );
    let mut msgs = vec![
        serde_json::json!({ "role": "system", "content": system }),
        serde_json::json!({ "role": "user", "content": wrapper }),
    ];
    // 跨进程续跑（二期）：sidecar 存在且属本行 → 装载全量历史代替新任务包装。
    // 新行的 id 全局唯一、永远不会带 sidecar，「存在且 id 匹配」即续跑判定；
    // 不匹配/解析失败按正常启动（审计留痕，不静默）。system 消息沿用 sidecar
    // 里那份（角色提示词/工具清单与上次跑一致），onion 绑定由 bind_session
    // 从会话元数据重挂，白名单闸与上次同源。
    let resume_sidecar = artifact_dir.join("session_history.json");
    let resume_loaded = crate::bot::subagent_tools::load_resume_history(&resume_sidecar, &row.id);
    let mut max_rounds = max_rounds;
    if let Some(history) = resume_loaded {
        msgs = history;
        msgs.push(serde_json::json!({
            "role": "user",
            "content": format!(
                "【系统提示】这是跨进程续跑：你此前一次执行因预算触顶被中断，未交出收尾。\
                 现追加 {} 轮预算完成任务：基于上面已有进展继续，尽快收尾并经 submit_result 提交；\
                 不要重新开始已完成的步骤，也不要展开与收尾无关的新探索。",
                crate::bot::subagent_repair::RESUME_ROUNDS
            ),
        }));
        max_rounds = crate::bot::subagent_repair::RESUME_ROUNDS.min(max_rounds);
        crate::bot::audit_log(
            app,
            &format!(
                "subagent_resumed_cross_process | id: {} | 历史消息 {} 条",
                row.id,
                msgs.len()
            ),
        );
    }
    (msgs, max_rounds, bound_onion)
}

/// 工具循环 + 熔断出口恢复链 + sidecar 生命周期 + 收尾 JSON 补答一次。
/// 轮数预算（熔断→budget_exceeded；排队不计——running 起算）；没有墙钟：
/// 时间兜底在工具级超时与停滞判定，不在整条 future 上掐。
/// 熔断出口恢复链（剥标记 → 盘上 verdict → 强制交卷轮 → 原会话续跑一次）：
/// 恢复成功就把收尾正文换回给下游 classify 照常判定（交出收尾 JSON 判真实
/// 完成，收尾窗同款口径）；恢复失败维持熔断正文走 budget_exceeded 留产。
/// 恢复轮次的 refs / trace 不并回——refs 是主聊天的引用面板数据，恢复轮
/// 不新增用户可见引用；恢复轮与补答轮的轮数与 token 不进 usage 记账，
/// 恢复出口只有 subagent_fuse_recovered（带 via）一条审计。
async fn execute_with_recovery(
    app: &AppHandle,
    stop: &crate::bot_slash::StopGuard,
    row_id: &str,
    artifact_dir: &std::path::Path,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    temperature: Option<f64>,
    required_checks: usize,
    started_at: std::time::SystemTime,
) -> CommandResult<(
    String,
    Vec<crate::bot::TaskRef>,
    crate::bot_model_loop::LoopTrace,
)> {
    let first = crate::bot_model_loop::run_model_loop(
        app.clone(),
        msgs,
        max_rounds,
        stop,
        None,
        None,
        temperature,
        // 子 agent 不落 exec_traces，无需失败明细回传槽（轮数另走 tool_guard 仪表）
        None,
    )
    .await?;
    // 第三元是循环出口的最终消息栈：循环里累积的 assistant/tool 消息只有这份
    // 是全的——恢复链（交卷轮/续跑轮）与 sidecar 都以它为原料。
    let (text, refs, final_msgs, trace) = first;
    if text.contains(crate::bot_model_loop::FUSE_MARKER)
        || text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER)
    {
        let gen_dir_opt = crate::db::gen_dir(app).ok();
        let ctx = crate::bot::subagent_repair::FusedRecoveryCtx {
            required_checks,
            artifact_dir,
            gen_dir: gen_dir_opt.as_deref(),
            started: started_at,
        };
        // 轮执行注入点：恢复轮共用同一 StopGuard（取消即时生效）与 LLM 装配。
        // app 取引用克隆（move 闭包搬走 AppHandle 会断掉后续收尾路径的 app 使用）
        let app_ref = app;
        let stop_ref = stop;
        let run_rounds = move |history: Vec<serde_json::Value>, rounds: usize| {
            let app = app_ref.clone();
            async move {
                crate::bot_model_loop::run_model_loop(
                    app, history, rounds, stop_ref, None, None, None, None,
                )
                .await
                .ok()
                .map(|(t, _, _, _)| t)
            }
        };
        let mut recovery_msgs = final_msgs;
        let outcome = crate::bot::subagent_repair::recover_fused_exit(
            stop,
            &mut recovery_msgs,
            &text,
            &ctx,
            run_rounds,
        )
        .await;
        match outcome {
            crate::bot::subagent_repair::FusedRecovery::Recovered { text, via } => {
                crate::bot::audit_log(
                    app,
                    &format!("subagent_fuse_recovered | id: {} | via: {via}", row_id),
                );
                if let Some(fixed) = repair_final_json_once(app, stop, &text).await {
                    return Ok((fixed, refs, trace));
                }
                return Ok((text, refs, trace));
            }
            crate::bot::subagent_repair::FusedRecovery::NotRecovered => {
                // 续跑-1：全量对话历史落 sidecar——熔断残留只有 800 字，这里
                // 是跨进程续跑的原料与事后诊断的完整底账。
                let sidecar = artifact_dir.join("session_history.json");
                let payload = serde_json::json!({
                    "subagent_id": row_id,
                    "fused": true,
                    "messages": recovery_msgs,
                });
                if let Err(e) = std::fs::create_dir_all(artifact_dir)
                    .and_then(|_| std::fs::write(&sidecar, payload.to_string()))
                {
                    crate::bot::audit_log(
                        app,
                        &format!(
                            "subagent_history_sidecar_failed | id: {} | {} | {e}",
                            row_id,
                            sidecar.display()
                        ),
                    );
                } else {
                    crate::bot::audit_log(
                        app,
                        &format!(
                            "subagent_history_sidecar | id: {} | {}",
                            row_id,
                            sidecar.display()
                        ),
                    );
                }
            }
        }
    } else {
        // 非熔断收尾（交卷/补答成功/失败带 JSON）：sidecar 已无续跑价值
        // （只有 budget_exceeded 行可续跑），清掉——「存在且 id 匹配」就是
        // 续跑判定，残留旧文件会让下一次同名误判失真。
        let sidecar = artifact_dir.join("session_history.json");
        if sidecar.exists() {
            match std::fs::remove_file(&sidecar) {
                Ok(()) => crate::bot::audit_log(
                    app,
                    &format!(
                        "subagent_history_sidecar_cleared | id: {} | {}",
                        row_id,
                        sidecar.display()
                    ),
                ),
                Err(e) => crate::bot::audit_log(
                    app,
                    &format!(
                        "subagent_history_sidecar_clear_failed | id: {} | {e}",
                        row_id
                    ),
                ),
            }
        }
    }
    if let Some(fixed) = repair_final_json_once(app, stop, &text).await {
        return Ok((fixed, refs, trace));
    }
    Ok((text, refs, trace))
}

/// 收尾判定（设计 §6/§7）：classify_outcome → 停滞改判 → 机器验收旁证与改判。
/// 停滞超时强制停止：循环按「用户停止」出口带正文返回，判定看不出是假死，
/// 这里按真实原因改判——父 agent 读 error 决定重试，「stalled」比
/// 「result_json_unparseable」如实得多。部分正文照常保留。
/// 机器可判定验收：verifier 收尾里的 met 是自述，服务端对「产物文件在不在」
/// 这类标准能自己判——判了才覆盖自评，并在正文里点名未通过项。
/// 只在跑完（有收尾 JSON 或恢复链 verdict）时判：熔断/停滞行没有验收可言。
/// 机器验收是对「模型自报」的独立旁证，判定顺序不能让它落在终态之后：
/// 不在这里回头修正，「机器验收通过」与「行状态失败」就会永远同时成立。
/// 返回 (status, error, result, final_text)。
fn judge_outcome(
    app: &AppHandle,
    row: &SubagentRow,
    outcome: &CommandResult<(
        String,
        Vec<crate::bot::TaskRef>,
        crate::bot_model_loop::LoopTrace,
    )>,
    stalled: bool,
    artifact_dir: &std::path::Path,
) -> (
    SubagentStatus,
    Option<String>,
    Option<serde_json::Value>,
    String,
) {
    let (mut status, mut error, mut result) = match outcome {
        Err(e) => classify_outcome(Some(&e.message()), None),
        Ok((text, _refs, _loop_trace)) => classify_outcome(None, Some(text)),
    };
    if stalled {
        status = SubagentStatus::Failed;
        error = Some("stalled".into());
        if let Ok((text, _, _)) = outcome {
            result = Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": format!(
                    "停滞超时（{}s 无 LLM 增量与工具进展），已强制停止；以上为停止前保留的最后一段正文",
                    crate::bot::constants::SUBAGENT_STALL_TIMEOUT_SECS
                )
            }));
        }
    }
    // 预算熔断（轮数 / 工具共用「⏹ 已熔断」前缀，查短标记即覆盖两者）与停滞
    // 强停都走 Ok 出口但没有收尾 JSON，等于没交答卷——机器验收必须让开：拿
    // 产物文件判熔断行「未通过」只会在正文里凭空多出一段子 agent 没说过的话。
    // 恢复链交出的收尾正文不带标记，照常判定。
    let outcome_text = match outcome {
        Ok((text, _, _)) => Some(text.as_str()),
        Err(_) => None,
    };
    let final_text: String = match outcome {
        Ok((text, _, _)) => text.clone(),
        Err(e) => format!("⚠️ 执行失败：{}", e.message()),
    };
    let machine_verdicts = if machine_verdicts_allowed(outcome_text, stalled) {
        eval_machine_criteria(&acceptance_criteria_of(row), artifact_dir)
    } else {
        Vec::new()
    };
    let failed_machine = machine_verdicts.iter().filter(|v| !v.met).count();
    if failed_machine > 0 {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "subagent.machine_check_failed",
            "subagent_id" => row.id.clone(),
            "failed" => failed_machine.to_string(),
            "total" => machine_verdicts.len().to_string(),
        );
    } else if !machine_verdicts.is_empty() {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "subagent.machine_check_passed",
            "subagent_id" => row.id.clone(),
            "total" => machine_verdicts.len().to_string(),
        );
    }
    // 收尾 JSON 与最终正文都带上机器结论——正文这行是模型和用户唯一同时
    // 看得到的地方，不能只躺在 result_json 里等人去查
    let (result, final_text) = apply_machine_verdicts(result, final_text, &machine_verdicts);
    let (status, error, recovered_by_machine) =
        reconcile_machine_override(status, error, &machine_verdicts);
    let final_text = if recovered_by_machine {
        format!(
            "{final_text}\n\n✅ 服务端机器验收 {} 项全部通过——子 agent 未按 schema 写收尾 JSON，按产物判成功",
            machine_verdicts.len()
        )
    } else {
        final_text
    };
    if recovered_by_machine {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "subagent.parse_failure_recovered",
            "subagent_id" => row.id.clone(),
            "criteria" => machine_verdicts.len().to_string(),
        );
    }
    (status, error, result, final_text)
}

/// finalize 落库（多写包同一事务 + cancel-vs-runner 竞态闸——行已终态（cancel
/// 先赢）时不做任何状态/结果副作用，只落历史）：状态/结果/usage 更新 + 任务包装
/// 与收尾文本历史落库 + 重建广播。失败 best-effort 落 Failed（收尾失败行会永挂
/// Running）。返回 fresh row——下游审计/事件直接用，免二次持锁查询；None =
/// 收尾闭包本身失败（已审计）。
async fn persist_final_state(
    app: &AppHandle,
    row: &SubagentRow,
    session_id: &str,
    wrapper: &str,
    status: SubagentStatus,
    error: Option<String>,
    result: Option<serde_json::Value>,
    final_text: &str,
    usage_for_db: Option<(u64, u64, u64)>,
) -> Option<SubagentRow> {
    let app_for_audit = app.clone();
    let row_for_db = row.clone();
    let session_id_for_db = session_id.to_string();
    let status_for_db = status;
    let result_for_db = result.clone();
    let error_for_db = error.clone();
    let wrapper_for_db = wrapper.to_string();
    let final_for_db = final_text.to_string();
    let finalized: Option<SubagentRow> =
        db_locked(app, move |conn| -> CommandResult<Option<SubagentRow>> {
            // 多写包同一事务 + cancel-vs-runner 竞态闸——
            // 行已终态（cancel 先赢）时不做任何状态/结果副作用，只落历史
            let current = crate::db::load_subagent(conn, &row_for_db.id)
                .map_err(CommandError::DbError)?
                .ok_or_else(|| CommandError::TaskNotFound(row_for_db.id.clone()))?;
            let already_terminal = current.status.is_terminal();
            let tx = conn
                .transaction()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            // 历史落库（任务包装 + 收尾文本）无论终态与否都执行（围观/取证可见），
            // 且与状态更新同处一个事务：历史是「DELETE + 逐条 INSERT」两步，
            // 不在事务里就是半截写——中途失败会留下「历史被清空但新行没写进去」
            let fresh = if already_terminal {
                // 终态已被 cancel 抢先写入：不做任何状态/结果副作用，但把行本身
                // 带回去——下游事件与教训要用行的真实终态（cancelled），不能用
                // classify_outcome 的「执行被打断 → Failed」观感
                current
            } else {
                crate::db::update_subagent_status(
                    &tx,
                    &row_for_db.id,
                    status_for_db,
                    now_ms(),
                    error_for_db.as_deref(),
                )
                .map_err(CommandError::DbError)?;
                if let Some(v) = &result_for_db {
                    crate::db::set_subagent_result(&tx, &row_for_db.id, &v.to_string())
                        .map_err(CommandError::DbError)?;
                }
                if let Some((p, c, cr)) = usage_for_db {
                    crate::db::update_subagent_usage(&tx, &row_for_db.id, p, c, cr)
                        .map_err(CommandError::DbError)?;
                }
                crate::db::load_subagent(&tx, &row_for_db.id)
                    .map_err(CommandError::DbError)?
                    .ok_or_else(|| CommandError::TaskNotFound(row_for_db.id.clone()))?
            };
            // 身份预生成（T1 服务端生成）：事件载荷与库行同 id
            let history_rows = vec![
                crate::db::BotMsgRow {
                    msg_id: Some(crate::db::bot_history::new_bot_msg_id()),
                    file_changes_json: None,
                    trace_id: None,
                    role: "user".into(),
                    content: wrapper_for_db,
                    refs_json: None,
                    thinking: None,
                    tools_json: None,
                    turn_status: None,
                },
                crate::db::BotMsgRow {
                    msg_id: Some(crate::db::bot_history::new_bot_msg_id()),
                    file_changes_json: None,
                    trace_id: None,
                    role: "assistant".into(),
                    content: final_for_db,
                    refs_json: None,
                    thinking: None,
                    tools_json: None,
                    turn_status: None,
                },
            ];
            crate::db::bot_history_save_audited(
                &app_for_audit,
                &tx,
                &session_id_for_db,
                &history_rows,
            )
            .map_err(CommandError::DbError)?;
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            // 重建广播（真源化批 5）：子会话整段换血后投影靠事件对账——
            // 新 id 在投影里必然陌生，前端收到即整段 reload（不变式 2 兜底）
            for r in &history_rows {
                crate::bot_chat::emit_row_upserted(&app_for_audit, Some(&session_id_for_db), r);
            }
            Ok(Some(fresh))
        })
        .await
        .unwrap_or_else(|e| {
            let msg = crate::bot::truncate_for_log(&e.message(), 200);
            // 收尾失败行会永挂 Running——best-effort 落 Failed
            //（终态判定在收尾闭包内已做；此处只兜住闭包本身失败的情形）。
            // 后台任务持 owned handle（spawn 要求 'static，不能借用 app）。
            let app_for_fail = app.clone();
            let sid_failed = row.id.clone();
            tauri::async_runtime::spawn(async move {
                let _ = enqueue_terminal_write(
                    &app_for_fail,
                    &sid_failed,
                    SubagentStatus::Failed,
                    now_ms(),
                    &msg,
                )
                .await;
            });
            crate::bot::audit_log(
                app,
                &format!(
                    "subagent_finalize_error | id: {} | err: {}",
                    row.id,
                    crate::bot::truncate_for_log(&e.message(), 200)
                ),
            );
            None
        });
    finalized
}

/// watcher 句柄 RAII：Drop 即 abort（收尾/panic/unwind 路径都不会泄漏循环任务；
/// StopGuard::drop 不置位停止标志，单靠手动 abort 会泄漏）。
struct AbortOnDrop(tauri::async_runtime::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// 取消 watcher（设计 §4.3 硬停）：2s 轮询行状态，进终态即停工具循环。
/// 子 agent 无卡，不再有「软删卡 = 取消派发」这条信号——cancel 置行终态，
/// 这是唯一的外部取消入口。同一轮询顺带判停滞：心跳超过停滞超时没刷新 →
/// 置位 stalled 并停循环。
fn spawn_cancel_watcher(
    app: &AppHandle,
    stop: &crate::bot_slash::StopGuard,
    subagent_id: &str,
    last_progress_ms: &std::sync::Arc<std::sync::atomic::AtomicU64>,
    tool_in_flight: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    stalled: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> AbortOnDrop {
    let watcher_app = app.clone();
    let watcher_stop = stop.token();
    let watcher_sid = subagent_id.to_string();
    let watcher_progress = last_progress_ms.clone();
    let watcher_tool_in_flight = tool_in_flight.clone();
    let watcher_stalled = stalled.clone();
    let watcher = tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if watcher_stop.stopped() {
                return;
            }
            let idle_ms = (now_ms() as u64)
                .saturating_sub(watcher_progress.load(std::sync::atomic::Ordering::Relaxed));
            // 工具在跑时不判停滞：run_python/MCP 单调用上限同为 600s，与阈值
            // 零余量，长工具执行期 emit 不发心跳，会被这一刀误杀。工具各有
            // 自己的超时上限兜底，标记不会永久占住。
            let tool_running = watcher_tool_in_flight.load(std::sync::atomic::Ordering::Relaxed);
            if !tool_running && idle_ms >= crate::bot::constants::SUBAGENT_STALL_TIMEOUT_SECS * 1000
            {
                watcher_stalled.store(true, std::sync::atomic::Ordering::SeqCst);
                crate::bot::audit_log(
                    &watcher_app,
                    &format!(
                        "subagent_stalled | id: {} | idle_ms: {idle_ms} | 停滞超时，已强制停止",
                        watcher_sid
                    ),
                );
                watcher_stop.stop();
                return;
            }
            let sid = watcher_sid.clone();
            // 只读轮询：直接开连接，不持 DB_WRITE_LOCK（防多子 agent 写锁互饿）
            let watcher_app2 = watcher_app.clone();
            // 查询期瞬时错误（DB 忙/锁竞争）按「未取消」处理——误停健康 run
            // 比晚两秒发现真取消伤得多；真取消由下一 tick 复查兜住，与排队
            // 路径「DB 读失败不误判取消」同一容错方向。
            let aborted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
                let conn = crate::db::open_db(&watcher_app2)?;
                Ok(crate::db::load_subagent(&conn, &sid)
                    .ok()
                    .flatten()
                    .map(|r| r.status.is_terminal())
                    .unwrap_or(false))
            })
            .await
            .unwrap_or(Ok(false))
            .unwrap_or(false);
            if aborted {
                watcher_stop.stop();
                return;
            }
        }
    });
    AbortOnDrop(watcher)
}

/// 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/。
/// gen_dir 解析失败必须 fail-closed：此前退到 "__gen_dir__" 相对路径兜底，
/// 会把产物目录钉到进程 CWD（打包应用 CWD 可能是 /），产物越过配置的隔离根。
/// create + canonicalize（写路径钉死规范化目录）失败仅审计：write_artifact_file
/// 每次调用会再校验，目录坏了只影响产物不影响状态机。
/// None = 已中止，调用方直接 return。
async fn prepare_artifact_dir(app: &AppHandle, row: &SubagentRow) -> Option<PathBuf> {
    #[allow(unused_mut)]
    let mut artifact_dir = match crate::db::gen_dir(app) {
        Ok(g) => artifact_dir_of(&g, &row.id),
        Err(e) => {
            let msg = format!("产物目录不可用（{e}），子任务中止");
            abort_and_finish(
                app,
                &row.id,
                row.parent_session_id.as_deref(),
                &format!("gen_dir_failed | {e}"),
                &msg,
            )
            .await;
            return None;
        }
    };
    let mk_dir = artifact_dir.clone();
    let mk_res = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        std::fs::create_dir_all(&mk_dir).map_err(|e| e.to_string())?;
        std::fs::canonicalize(&mk_dir).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    match mk_res {
        Ok(c) => {
            // 覆盖为规范化路径（registry ctx 与包装渲染都用它）
            artifact_dir = c;
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "subagent_artifact_dir_warn | id: {} | 产物目录创建/规范化失败（{e}），产物写将被拒绝",
                    row.id
                ),
            );
        }
    }
    Some(artifact_dir)
}

/// 开跑：建会话 + 包装落库 + session/assignee 回填（单次锁内完成）。
/// 标题不带 emoji 前缀：子 agent 身份由 bot_sessions.is_subagent 结构化
/// 字段承载（U20D 批 5），会话列表据此把它们挡在对话列表外。
/// None = 已中止，调用方直接 return。
async fn setup_subagent_session(
    app: &AppHandle,
    row: &SubagentRow,
    artifact_dir: &std::path::Path,
    onion: &Option<SpawnOnion>,
) -> Option<(String, String)> {
    let title = format!(
        "子任务：{}",
        crate::bot::truncate_for_log(row.objective.trim(), 30)
    );
    let setup = {
        let artifact_dir = artifact_dir.to_path_buf();
        let row = row.clone();
        let onion = onion.clone();
        db_locked(app, move |conn| -> CommandResult<(String, String)> {
            // 建会话与回填 session_id 包同一事务，防出现「建了会话但行没记」的孤儿会话
            let tx = conn
                .transaction()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            let session = crate::db::bot_session_create_subagent_inner(&tx, Some(title.clone()))
                .map_err(CommandError::DbError)?;
            // 包装文本仅返回（渲染一次，交 runner 供 LLM 与 finalize 复用）——
            // 历史落库统一在 finalize 全量覆盖写（setup 的
            // bot_history_save_inner 是半截写，会被 finalize DELETE+INSERT 覆盖，
            // 中途崩溃还留「只有任务块没有结论」的误导历史）
            let wrapper = render_task_wrapper(&row, &artifact_dir);
            crate::db::set_subagent_session(&tx, &row.id, &session.id)
                .map_err(CommandError::DbError)?;
            // 配置档元列随会话一起建：配置正文在 `<数据目录>/onions/`，这里只存指针。
            // 不写的话 bind_session 读不到，子 agent 会拿到未收窄的整套 profile 工具。
            if let Some(o) = &onion {
                crate::db::bot_session_set_onion(
                    &tx,
                    &session.id,
                    Some(&o.config),
                    o.max_turn.map(|t| t as i64),
                )
                .map_err(CommandError::DbError)?;
            }
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok((session.id, wrapper))
        })
    }
    .await;
    match setup {
        Ok(v) => Some(v),
        Err(e) => {
            let msg = e.message();
            abort_and_finish(
                app,
                &row.id,
                row.parent_session_id.as_deref(),
                &format!("session_setup_failed | {msg}"),
                &msg,
            )
            .await;
            None
        }
    }
}

mod reporting;

pub(crate) use reporting::{
    effective_status, find_fresh_deliverable_report, render_task_wrapper,
    verifier_deliverable_nudge,
};
use reporting::{
    emit_subagent_finished, load_subagent_status, machine_verdicts_allowed,
    self_judged_criteria_count,
};
