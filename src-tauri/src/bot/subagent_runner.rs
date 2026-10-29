//! 子 agent 执行体：run_subagent 及其生命周期配套。
//!
//! runner 负责「跑完一个子 agent」的全过程：排队取闸位 → 建会话 → 跑工具循环
//! → 收尾落库 → 回报父会话。取消 watcher 与墙钟硬停也在这条路径上。

use crate::bot_chat::ChatGuard;
use crate::bot_orchestrator::{
    acceptance_criteria_of, apply_machine_verdicts, artifact_dir_of, budget_force_needed,
    budget_of_row, classify_outcome, db_locked, eval_machine_criteria, finish_notify,
    gen_dir_rule_for, parse_final_json, reconcile_machine_override, repair_final_json_once,
    spawn_parent_report, SpawnOnion, SubagentStage,
};
use crate::db::{now_ms, SubagentBudget, SubagentProfile, SubagentRow, SubagentStatus};
use crate::error::{CommandError, CommandResult};
use futures_util::FutureExt;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri::Emitter;

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
            // 加载失败也要落终态，防 ghost queued 行永挂
            let ghost_sid = subagent_id.clone();
            let ghost_err = e.message();
            let write = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &ghost_sid,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&ghost_err),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            if let Err(write_err) = write {
                // 终态写不进去 = 行永远停在 queued/running，面板上一直转圈。
                // 这层兜底本身失败不留痕就等于静默丢单，必须记
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "subagent_terminal_state_write_failed | id: {} | err: {}",
                        subagent_id,
                        write_err.message()
                    ),
                );
            }
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: {}",
                    subagent_id,
                    e.message()
                ),
            );
            // parent 会话未知（行没读到），发不带归属的收尾事件——前端只重拉不弹提示
            emit_subagent_finished(&app, &subagent_id, None, SubagentStatus::Failed, "");
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
    let budget: SubagentBudget = budget_of_row(&row);

    // 并发闸global 3 / per-session 2，超限等待不失败）。
    // 排队可取消——每轮退避同时查行终态，取消即退队；Running 改在**拿到槽位并复核后**
    // 才写，排队中的行不再假挂运行中。
    let sid_gate_check = row.id.clone();
    let app_gate_check = app.clone();
    // 评审 M 采纳：节流 DB 读——open_db 含建目录+pragmas 文件 IO，不该 50ms
    // 一次压在 async worker 上；取消检测 500ms 粒度足够（槽后复核兜底）
    let gate_tick = std::cell::Cell::new(0u32);
    let mut cancelled_while_queued = move || {
        let n = gate_tick.get();
        gate_tick.set(n.wrapping_add(1));
        if !n.is_multiple_of(10) {
            return false;
        }
        crate::db::open_db(&app_gate_check)
            .ok()
            .and_then(|conn| {
                crate::db::load_subagent(&conn, &sid_gate_check)
                    .ok()
                    .flatten()
            })
            .map(|r| r.status.is_terminal())
            .unwrap_or(false) // DB 读失败不误判取消（拿到槽位后的复核兜底）
    };
    let ticket = crate::bot_orchestrator::SubagentGate::wait_slot_cancellable(
        row.parent_session_id.clone(),
        &mut cancelled_while_queued,
    )
    .await;
    let _ticket = match ticket {
        Some(t) => t,
        // 排队中被取消：行已被 cancel 写成终态，此处仅退出（守卫 RAII 清理）
        None => {
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: cancelled_while_queued",
                    row.id
                ),
            );
            emit_subagent_finished(
                &app,
                &row.id,
                row.parent_session_id.as_deref(),
                SubagentStatus::Cancelled,
                "",
            );
            return;
        }
    };
    {
        // 拿到槽位后复核（cancel-vs-runner 竞态闸）：行已终态 = cancel 先赢 →
        // 中止；DB 读失败保守中止（执行已取消的任务比不跑更糟
        let sid_still_queued = row.id.clone();
        let state_now = db_locked(&app, move |conn| {
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
                    &app,
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
                    emit_subagent_finished(&app, &row.id, row.parent_session_id.as_deref(), s, "");
                }
                return;
            }
        }
        // Running 写库挪到槽位拿到之后（排队窗口内行保持 queued 语义）。
        // 写失败必须中止（评审 HIGH 采纳）：recheck 与本写之间 cancel 可抢先写
        // Cancelled，update_subagent_status 因非法状态转移报错——吞错等于在
        // 已取消任务上继续跑
        let sid_running = row.id.clone();
        let run_write = db_locked(&app, move |conn| {
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
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: running_write_failed | {e}",
                    row.id
                ),
            );
            // 写 Running 被拒基本只有一种成因：cancel 已把行写成终态。回读
            // 真实终态上报（直接写 Cancelled 会在「DB 抖动」这类非 cancel
            // 失败上撒谎）；读不到行就只留审计，交给启动清扫收尸
            let st = load_subagent_status(&app, &row.id).await;
            if let Some(st) = st {
                emit_subagent_finished(&app, &row.id, row.parent_session_id.as_deref(), st, "");
            }
            return;
        }
    }
    let started = std::time::Instant::now();

    // 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/
    // gen_dir 解析失败必须 fail-closed：此前退到 "__gen_dir__" 相对路径兜底，
    // 会把产物目录钉到进程 CWD（打包应用 CWD 可能是 /），产物越过配置的隔离根
    #[allow(unused_mut)]
    let mut artifact_dir = match crate::db::gen_dir(&app) {
        Ok(g) => artifact_dir_of(&g, &row.id),
        Err(e) => {
            let msg = format!("产物目录不可用（{e}），子任务中止");
            let sid_failed = row.id.clone();
            let write = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &sid_failed,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&msg),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            if let Err(write_err) = write {
                // 终态写不进去 = 行永远停在 queued/running，面板上一直转圈。
                // 这层兜底本身失败不留痕就等于静默丢单，必须记
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "subagent_terminal_state_write_failed | id: {} | err: {}",
                        row.id,
                        write_err.message()
                    ),
                );
            }
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: gen_dir_failed | {e}",
                    row.id
                ),
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
    };
    // create + canonicalize（写路径钉死规范化目录）。失败仅审计：
    // write_artifact_file 每次调用会再校验，目录坏了只影响产物不影响状态机
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
                &app,
                &format!(
                    "subagent_artifact_dir_warn | id: {} | 产物目录创建/规范化失败（{e}），产物写将被拒绝",
                    row.id
                ),
            );
        }
    }

    // 开跑：建会话 + 包装落库 + session/assignee 回填（单次锁内完成）。
    // 标题不带 emoji 前缀：子 agent 身份由 bot_sessions.is_subagent 结构化
    // 字段承载（U20D 批 5），会话列表据此把它们挡在对话列表外
    let title = format!(
        "子任务：{}",
        crate::bot::truncate_for_log(row.objective.trim(), 30)
    );
    let setup = {
        let artifact_dir = artifact_dir.clone();
        let row = row.clone();
        let budget = budget.clone();
        let onion = onion.clone();
        db_locked(&app, move |conn| -> CommandResult<(String, String)> {
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
            let wrapper = render_task_wrapper(&row, &budget, &artifact_dir);
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
    let (session_id, wrapper) = match setup {
        Ok(v) => v,
        Err(e) => {
            let msg = e.message();
            let msg_for_db = msg.clone();
            let sid = row.id.clone();
            let write = db_locked(&app, move |conn| {
                crate::db::update_subagent_status(
                    conn,
                    &sid,
                    SubagentStatus::Failed,
                    now_ms(),
                    Some(&msg_for_db),
                )
                .map_err(CommandError::DbError)
            })
            .await;
            if let Err(write_err) = write {
                // 终态写不进去 = 行永远停在 queued/running，面板上一直转圈。
                // 这层兜底本身失败不留痕就等于静默丢单，必须记
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "subagent_terminal_state_write_failed | id: {} | err: {}",
                        row.id,
                        write_err.message()
                    ),
                );
            }
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_abort | id: {} | err: session_setup_failed | {msg}",
                    row.id
                ),
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

    // 工具守卫注册（白名单闸 + spawn 递归身份校验 + read_own_card/write_artifact 定位）。
    // RAII 守卫：Drop 反注册——runner 中途 panic 也不泄漏条目（ 采纳）
    // 轮数仪表句柄：循环每轮自报时写进来，供 check_subagent 的 progress 读。
    // 与 ctx 里的 used_rounds 是同一个 Arc（守卫按值克隆 ctx，Arc 保证同源）。
    let rounds_gauge = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // 工具调用计数：与 ctx 里的 used_tool_calls 同一个 Arc（守卫按值克隆 ctx）。
    // 收尾时读一次，判断预算是否触达——触达必须落 budget_exceeded，不能落
    // succeeded（父 agent 提示词教它「failed / budget_exceeded 都要读 error
    // 决定重试」，报成 succeeded 就不重试，产出残缺而无人知晓）。
    let used_tool_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _session_guard = crate::tool_guard::SubagentSessionGuard::new(
        &session_id,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: row.id.clone(),
            task_id: row.task_id.clone(),
            profile: row.profile,
            tools: std::sync::Arc::new(subagent_tools),
            budget: budget.clone(),
            artifact_dir: artifact_dir.clone(),
            used_tool_calls: used_tool_calls.clone(),
            used_rounds: rounds_gauge.clone(),
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

    // 取消 watcher（设计 §4.3 硬停）：2s 轮询行状态，进终态即停工具循环。
    // 子 agent 无卡，不再有「软删卡 = 取消派发」这条信号——cancel 置行终态，
    // 这是唯一的外部取消入口
    let watcher_app = app.clone();
    let watcher_stop = stop.token();
    let watcher_sid = row.id.clone();
    struct AbortOnDrop(tauri::async_runtime::JoinHandle<()>);
    impl Drop for AbortOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let watcher = tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if watcher_stop.stopped() {
                return;
            }
            let sid = watcher_sid.clone();
            // 只读轮询：直接开连接，不持 DB_WRITE_LOCK（防多子 agent 写锁互饿）
            let watcher_app2 = watcher_app.clone();
            let aborted = tauri::async_runtime::spawn_blocking(move || -> Result<bool, String> {
                let conn = crate::db::open_db(&watcher_app2)?;
                Ok(crate::db::load_subagent(&conn, &sid)
                    .ok()
                    .flatten()
                    .map(|r| r.status.is_terminal())
                    .unwrap_or(true))
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

    // 消息装配：system = 子 agent 通用提示 + profile 段 + 产物目录规则；user = 任务包装
    // stage 在场：拼 PROFILE_RECON/EXECUTOR/VERIFIER 替代基线 PROFILE_*（设计 §8.5）。
    // profile 仍决定工具白名单档——stage 只换角色语义层。两层解耦，允许调用方：
    // 1) 仅给 stage（profile 自动按 default_profile 派生）；或
    // 2) 同时给 stage + profile（白名单与角色互不干扰，如 verifier=research 的只读够用）。
    let profile_prompt = match stage {
        Some(SubagentStage::Recon) => crate::prompts::PROFILE_RECON,
        Some(SubagentStage::Executor) => crate::prompts::PROFILE_EXECUTOR,
        Some(SubagentStage::Verifier) => crate::prompts::PROFILE_VERIFIER,
        None => match row.profile {
            SubagentProfile::Research => crate::prompts::PROFILE_RESEARCH,
            SubagentProfile::Coder => crate::prompts::PROFILE_CODER,
            SubagentProfile::General => crate::prompts::PROFILE_GENERAL,
        },
    };
    // wrapper 已由 setup 渲染返回（同一文本：LLM 输入 = 历史落库 = 收尾覆盖源）
    // 配置档段：角色 + 交集后的实际可用工具（§3.4「配置看着生效、实际落空」
    // 要让模型也看得见）；无配置档时是空串，不占位也不改既有提示词。
    let bound_onion = crate::bot_onions::cached_for_session(Some(session_id.as_str()));
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
        gen_dir_rule_for(&artifact_dir),
        onion_note,
    );
    let msgs = vec![
        serde_json::json!({ "role": "system", "content": system }),
        serde_json::json!({ "role": "user", "content": wrapper }),
    ];

    // 工具循环：轮数预算（熔断→budget_exceeded；排队不计——running 起算）。
    // 没有墙钟：时间兜底在工具级超时与停滞判定，不在整条 future 上掐。
    //
    // 收尾 JSON 解析失败时**补答一次**（结构化输出三段修复的第 3 段）：产出
    // 完整、机检全过，只因最后一段不是 JSON 就判 failed，是完成率最大的单项
    // 黑洞。
    let run = async {
        let first = crate::bot_model_loop::run_model_loop(
            app.clone(),
            msgs,
            budget.max_turns as usize,
            &stop,
            None,
            None,
            // 配置档的采样温度（f32 → f64 无损）。这是 yaml `temperature:` 字段的
            // 唯一消费者；None 时用模型条目的 temperature。
            bound_onion
                .as_ref()
                .and_then(|c| c.temperature)
                .map(f64::from),
            // 子 agent 不落 exec_traces，无需失败明细回传槽（轮数另走 tool_guard 仪表）
            None,
        )
        .await?;
        let (text, refs, trace) = first;
        if let Some(fixed) = repair_final_json_once(&app, &stop, &text).await {
            return Ok((fixed, refs, trace));
        }
        Ok((text, refs, trace))
    };
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
    let _watcher = AbortOnDrop(watcher);
    // tool_guard 注册走 RAII 守卫（_session_guard Drop 反注册）
    crate::db::lock::lock_or_recover(
        &crate::app_state::subagent_stops(&app),
        "app_state::subagent_stops",
    )
    .remove(&row.id);
    drop(chat_guard);

    // 收尾判定（设计 §6/§7）
    let (mut status, mut error, mut result) = match &outcome {
        Err(e) => classify_outcome(Some(&e.message()), None),
        Ok((text, _refs, _loop_trace)) => classify_outcome(None, Some(text)),
    };
    // 工具预算触达 → 强制 budget_exceeded(max_tool_calls)——只针对「没交答卷」的。
    //
    // 预算触顶后进入收尾窗：读类被拒、写产物放行。模型若照「只许写」
    // 的指引交出合法收尾 JSON（含产物），那是真实完成，不得改判——改判会把
    // 父 agent 的「带残留重试」逻辑错误触发，半成品被当失败重烧。没交答卷的
    // （熔断标记路径已自带 budget_exceeded；这里管「停了工具但只输出散文」的
    // 剩余路径）才改判，真实原因（预算触顶）不能丢。
    let tool_calls_used = used_tool_calls.load(std::sync::atomic::Ordering::SeqCst);
    let wrapped_up_cleanly =
        matches!(&outcome, Ok((text, _, _)) if parse_final_json(text).is_some());
    if budget_force_needed(
        tool_calls_used,
        budget.max_tool_calls,
        status,
        wrapped_up_cleanly,
    ) {
        status = SubagentStatus::BudgetExceeded;
        error = Some("max_tool_calls".into());
        // 部分结果不丢弃：熔断前跑出来的正文对父 agent 判断「该不该原目标
        // 重试、还是放宽预算」仍有价值
        if let Ok((text, _, _)) = &outcome {
            result = Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "工具调用预算触达，未产出收尾 JSON；以上为触达前保留的最后一段正文"
            }));
        }
    }
    // 轮数熔断现在也走 Ok 出口（正文带 ROUNDS_FUSE_MARKER），机器验收必须把它
    // 排除掉：熔断行没有收尾 JSON，等于没交答卷，拿产物文件判它「未通过」只会
    // 在正文里凭空多出一段子 agent 没说过的话。
    let fused_on_rounds = matches!(&outcome, Ok((text, _, _))
        if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER));
    let final_text: String = match &outcome {
        Ok((text, _, _)) => text.clone(),
        Err(e) => format!("⚠️ 执行失败：{}", e.message()),
    };
    // 机器可判定验收：verifier 收尾里的 met 是自述，服务端对「产物文件在不在」
    // 这类标准能自己判——判了才覆盖自评，并在正文里点名未通过项。
    // 只在跑完（有收尾 JSON）时判：预算触顶/异常退出的行没有验收可言。
    let machine_verdicts = if outcome.is_ok() && !fused_on_rounds {
        eval_machine_criteria(&acceptance_criteria_of(&row), &artifact_dir)
    } else {
        Vec::new()
    };
    let failed_machine = machine_verdicts.iter().filter(|v| !v.met).count();
    if failed_machine > 0 {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Warn,
            "subagent.machine_check_failed",
            "subagent_id" => row.id.clone(),
            "failed" => failed_machine.to_string(),
            "total" => machine_verdicts.len().to_string(),
        );
    } else if !machine_verdicts.is_empty() {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "subagent.machine_check_passed",
            "subagent_id" => row.id.clone(),
            "total" => machine_verdicts.len().to_string(),
        );
    }
    // 收尾 JSON 与最终正文都带上机器结论——正文这行是模型和用户唯一同时
    // 看得到的地方，不能只躺在 result_json 里等人去查
    let (result, final_text) = apply_machine_verdicts(result, final_text, &machine_verdicts);
    // 机器验收是对「模型自报」的独立旁证，判定顺序不能让它落在终态之后：
    // 不在这里回头修正，「机器验收通过」与「行状态失败」就会永远同时成立。
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
            &app,
            crate::audit::AuditLevel::Warn,
            "subagent.parse_failure_recovered",
            "subagent_id" => row.id.clone(),
            "criteria" => machine_verdicts.len().to_string(),
        );
    }
    let wrapper_for_db = wrapper.clone();
    let row_for_db = row.clone();
    let session_id_for_db = session_id.clone();
    let status_for_db = status;
    let result_for_db = result.clone();
    let error_for_db = error.clone();
    let final_for_db = final_text.clone();
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
    let app_for_audit = app.clone();
    let finalized: Option<SubagentRow> =
        db_locked(&app, move |conn| -> CommandResult<Option<SubagentRow>> {
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
            crate::db::bot_history_save_audited(
                &app_for_audit,
                &tx,
                &session_id_for_db,
                &[
                    crate::db::BotMsgRow {
                        role: "user".into(),
                        content: wrapper_for_db,
                        refs_json: None,
                        thinking: None,
                        tools_json: None,
                        turn_status: None,
                    },
                    crate::db::BotMsgRow {
                        role: "assistant".into(),
                        content: final_for_db,
                        refs_json: None,
                        thinking: None,
                        tools_json: None,
                        turn_status: None,
                    },
                ],
            )
            .map_err(CommandError::DbError)?;
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok(Some(fresh))
        })
        .await
        .unwrap_or_else(|e| {
            let msg = crate::bot::truncate_for_log(&e.message(), 200);
            // 收尾失败行会永挂 Running——best-effort 落 Failed
            //（终态判定在收尾闭包内已做；此处只兜住闭包本身失败的情形）。
            // 后台任务持 owned handle（spawn 要求 'static，不能借用 app）。
            let sid_failed = row.id.clone();
            let sid_for_log = row.id.clone();
            let app_for_fail = app.clone();
            tauri::async_runtime::spawn(async move {
                let write = db_locked(&app_for_fail, move |conn| {
                    crate::db::update_subagent_status(
                        conn,
                        &sid_failed,
                        SubagentStatus::Failed,
                        now_ms(),
                        Some(&msg),
                    )
                    .map_err(CommandError::DbError)
                })
                .await;
                if let Err(write_err) = write {
                    // 终态写不进去 = 行永远停在 running，面板上一直转圈。
                    // 这层兜底本身失败不留痕就等于静默丢单，必须记
                    crate::bot::audit_log(
                        &app_for_fail,
                        &format!(
                            "subagent_terminal_state_write_failed | id: {} | err: {}",
                            sid_for_log,
                            write_err.message()
                        ),
                    );
                }
            });
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_finalize_error | id: {} | err: {}",
                    row.id,
                    crate::bot::truncate_for_log(&e.message(), 200)
                ),
            );
            None
        });

    // 审计 + 广播 + 前端事件（设计 §3 事件全链按 subagent_id 串链）
    let elapsed = started.elapsed().as_millis() as u64;
    // 行的真实终态优先于 classify_outcome：用户取消走到这里时 classify 只会给出
    // Failed（执行被 StopToken 打断），照它上报 = 每次取消都记一条 subagent_failed
    // 并沉淀「失败原因：」的空教训。收尾闭包整体失败（finalized=None）才回落到观感值
    let effective_status = effective_status(finalized.as_ref().map(|r| r.status), status);
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
        &app,
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
        &app,
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
        &app,
        &row.id,
        row.parent_session_id.as_deref(),
        effective_status,
        summary.as_deref().unwrap_or_default(),
    );
    spawn_parent_report(&app, &row, effective_status);
}
/// 任务包装（设计 §8.5）：orchestrator 从 subagents 行渲染，不接受 LLM 自由拼接。
/// 目标 / 验收标准 / 上下文摘要 / 预算全部来自行上字段。
pub(crate) fn render_task_wrapper(
    row: &SubagentRow,
    budget: &SubagentBudget,
    artifact_dir: &std::path::Path,
) -> String {
    let criteria: Vec<String> = acceptance_criteria_of(row);
    let mut w = String::new();
    w.push_str("[子任务派发]\n");
    w.push_str(&format!("subagent_id: {}\n", row.id));
    w.push_str(&format!("objective: {}\n", row.objective));
    w.push_str("\nacceptance_criteria:\n");
    for c in &criteria {
        w.push_str(&format!("- {c}\n"));
    }
    if let Some(ctx) = row
        .context_summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        w.push_str("\n上下文摘要:\n");
        w.push_str(ctx);
        w.push('\n');
    }
    w.push_str(&format!(
        "\n预算:\nmax_turns={}, max_tool_calls={}\n",
        budget.max_turns, budget.max_tool_calls
    ));
    w.push_str(&format!("\n产物目录:\n{}\n", artifact_dir.display()));
    w.push_str("\n完成后必须输出以下 JSON（status 与 summary 必填；acceptance_check / artifacts / blockers 没有就省掉）：\n");
    w.push_str(crate::prompts::RESULT_SCHEMA_HINT);
    w
}
/// 上报用终态：行已写入终态时以行为准（cancel 抢先 → cancelled），
/// 收尾闭包自身失败拿不到行时才回落 classify 的观感值
pub(crate) fn effective_status(
    row: Option<SubagentStatus>,
    classified: SubagentStatus,
) -> SubagentStatus {
    row.unwrap_or(classified)
}
/// 回读一行子 agent 的状态：读失败或行已不存在都返 None（调用方据此决定
/// 「按真实状态上报」还是「只留审计」）
async fn load_subagent_status(app: &AppHandle, id: &str) -> Option<SubagentStatus> {
    let id = id.to_string();
    db_locked(app, move |conn| {
        Ok(crate::db::load_subagent(conn, &id)
            .ok()
            .flatten()
            .map(|r| r.status))
    })
    .await
    .unwrap_or(None)
}
/// `subagent-finished` 唯一收尾出口：主会话视图据此刷新子 agent 列表 + 打完成
/// 提示。所有把行写成终态的路径都必须走到这里（含排队期取消、启动中止、
/// 行加载失败的 ghost 路径）——前端只认事件，不发就永远停在 queued
fn emit_subagent_finished(
    app: &AppHandle,
    subagent_id: &str,
    parent_session_id: Option<&str>,
    status: SubagentStatus,
    summary: &str,
) {
    let _ = app.emit_to(
        "main",
        "subagent-finished",
        serde_json::json!({
            "subagentId": subagent_id,
            "parentSessionId": parent_session_id,
            "status": status.as_str(),
            "summary": summary,
        }),
    );
    // 叫醒 check 等待方：本函数是所有转终态路径的唯一出口，通知挂在这里
    // 就不会漏（含排队期取消 / 启动中止 / ghost 路径）。
    finish_notify().notify_waiters();
}
