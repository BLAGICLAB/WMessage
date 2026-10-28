//! 子 agent 派发：入参校验 / 预算钳制 / 建行 / 并发闸。
//!
//! 派发是「建行 + 放行」的组合：建行只写 subagents 表（不建子卡），
//! 并发闸在放行前占位（per-session 2 / global 3），闸满则排队等位。
//! 闸与建行放一处的原因：排队语义只在派发路径上出现。

use crate::bot_orchestrator::{
    run_subagent, safe_rel_path, SpawnAck, SpawnRequest, SubagentStage, MACHINE_PREFIXES,
};
use crate::db::{SubagentBudget, SubagentProfile, SubagentRow, SubagentStatus};
use crate::error::{CommandError, CommandResult};
use std::collections::HashMap;
use tauri::AppHandle;
use tauri::Emitter;

/// 预算钳制：None → 默认值；其余交给 SubagentBudget::clamped（硬顶 50 / 下限 1）。
pub fn clamp_budget(budget: Option<SubagentBudget>) -> SubagentBudget {
    budget.unwrap_or_default().clamped()
}
/// 配置档的 `max_turn` 只收紧：派发给的轮数更宽时不许被配置档顶开。
///
/// 取 min 而非覆盖——配置档是权限上限，不是「优先于调用方的指令」。
pub(crate) fn apply_onion_turn_cap(budget: &mut SubagentBudget, cap: Option<u32>) {
    if let Some(cap) = cap {
        if cap < budget.max_turns {
            budget.max_turns = cap;
        }
    }
}
/// 派发 profile 的解析顺序：显式 `profile` > 配置档的 `profile` > `stage` 派生。
///
/// **不是「配置档说了算」**：让 yaml 的 profile 覆盖运行时 profile 就是放宽
/// （模型给 research、配置写 general → 白名单变大），与设计 §3.4「只有收紧、
/// 没有第二张 allow 表」冲突。显式 profile 优先＝把档位的决定权交回运行时，
/// yaml 的 `profile:` 只在调用方没表态时兜底。
///
/// 三者都给不出时返回 None：那是「没说清派个什么档位」，必须报错而不是猜。
pub(crate) fn resolve_spawn_profile(
    explicit: Option<SubagentProfile>,
    config_profile: Option<SubagentProfile>,
    stage: Option<SubagentStage>,
) -> Option<SubagentProfile> {
    explicit
        .or(config_profile)
        .or_else(|| stage.map(|s| s.default_profile()))
}
fn validate_request(req: &SpawnRequest) -> CommandResult<()> {
    if req.objective.trim().is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "objective".into(),
            value: req.objective.clone(),
            reason: "目标不能为空。正确写法：objective=\"调研 X，产出 Y，含来源\"".into(),
        });
    }
    if req.acceptance_criteria.is_empty()
        || req.acceptance_criteria.iter().any(|c| c.trim().is_empty())
    {
        return Err(CommandError::InvalidArgument {
            field: "acceptance_criteria".into(),
            value: format!("{:?}", req.acceptance_criteria),
            reason: "验收标准必填且每条非空".into(),
        });
    }
    // 机器可判定条目的**形状**校验（派发期即验，不等跑完才发现）。
    for c in &req.acceptance_criteria {
        if let Some(reason) = machine_criterion_shape_error(c) {
            return Err(CommandError::InvalidArgument {
                field: "acceptance_criteria".into(),
                value: c.clone(),
                reason,
            });
        }
    }
    Ok(())
}
/// 机器可判定验收标准的**形状**校验（纯函数，单测锚点）。
///
/// 判据：条目前缀是 `file_exists:` / `file_nonempty:` 时，冒号后面必须是一个
/// **干净的相对路径**——不带空格、不带说明文字、不逃出产物目录。
///
/// 为什么要在派发期就拒：此前这类条目要等子 agent 跑完十几分钟，服务端才拿
/// 「产物不存在」判它没过——模型写的是 `file_exists:调研.md 已完成`，路径变成
/// 「调研.md 已完成」，文件当然不存在。假阴性白烧一整轮预算，父 agent 还会
/// 以为产物没产出。
///
/// 报错文案里带正例：模型连错几次，光说「格式不对」它不知道对的长什么样。
fn machine_criterion_shape_error(criterion: &str) -> Option<String> {
    let t = criterion.trim();
    let (prefix, rest) = MACHINE_PREFIXES
        .iter()
        .find_map(|(prefix, _)| t.strip_prefix(prefix).map(|r| (*prefix, r)))?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(format!(
            "{prefix} 后面必须给相对路径。正确写法：file_exists:report.md"
        ));
    }
    if rest.chars().any(char::is_whitespace) {
        return Some(format!(
            "{prefix} 后面只能跟相对路径，不能带空格或说明文字（收到「{rest}」）。\
             正确写法：file_exists:report.md；说明另起一条自然语言验收标准"
        ));
    }
    if safe_rel_path(rest).is_none() {
        return Some(format!(
            "{prefix} 后面必须是不逃出产物目录的相对路径（收到「{rest}」）。\
             正确写法：file_exists:report.md 或 file_exists:output/数据.xlsx"
        ));
    }
    None
}
/// spawn 纯 DB 核心：校验 → 缺 parent 卡 TaskNotFound → 幂等查表 → 插 queued 行。
/// 子 agent 不再建任务卡——执行契约全在行上（objective / acceptance_json /
/// context_summary / budget_json），前端经 `list_subagents` 读行看进度。
pub(crate) fn spawn_subagent_locked(
    conn: &mut rusqlite::Connection,
    req: &SpawnRequest,
    now: i64,
) -> CommandResult<SpawnAck> {
    validate_request(req)?;
    if let Some(parent) = &req.parent_task_id {
        let exists = crate::db::task_exists(conn, parent).map_err(CommandError::DbError)?;
        if !exists {
            return Err(CommandError::TaskNotFound(parent.clone()));
        }
    }
    let budget = clamp_budget(req.budget.clone());
    let objective = req.objective.trim().to_string();
    let acceptance_json = serde_json::to_string(&req.acceptance_criteria)
        .map_err(|e| CommandError::Internal(e.to_string()))?;
    // 幂等键（服务端派生，不指望模型给）：同一父会话里，档位 / 目标 / 验收标准 /
    // 指定模型全同的**活着的**子 agent 只应有一条。模型重复派发（同一轮里
    // 发两次，或隔几轮忘了自己已经派过）不该再插一行——两条活行各拿一份完整
    // 预算，同一份活干两遍。
    //
    // 只在有父会话时去重：跨会话的同目标派发是不同用户意图，不该互相吞掉。
    // 已终态的不参与去重——「再跑一次」是重试语义。
    if let Some(parent_sid) = req.parent_session_id.as_deref() {
        let live = crate::db::find_live_subagent_by_request(
            conn,
            parent_sid,
            req.profile.as_str(),
            &objective,
            &acceptance_json,
            req.model_profile.as_deref(),
        )
        .map_err(CommandError::DbError)?;
        if let Some(existing) = live {
            // 回显**已在跑**那一条的生效预算，不是本次请求的——本次请求
            // 根本没建行，它给的预算没有生效。
            let existing_budget = budget_of_row(&existing);
            return Ok(SpawnAck {
                subagent_id: existing.id,
                task_id: existing.task_id,
                trace_id: existing.trace_id,
                status: existing.status,
                deduped: true,
                budget: existing_budget,
            });
        }
    }
    let subagent_id = format!("sa_{}", uuid::Uuid::new_v4().simple());
    let trace_id = format!("trc_{}", uuid::Uuid::new_v4().simple());
    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    let row = SubagentRow {
        id: subagent_id.clone(),
        status: SubagentStatus::Queued,
        profile: req.profile,
        model: req.model_profile.clone(),
        parent_session_id: req.parent_session_id.clone(),
        session_id: None,
        task_id: None,
        objective,
        trace_id: trace_id.clone(),
        acceptance_json: Some(acceptance_json),
        context_summary: req
            .context_summary
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        budget_json: Some(
            serde_json::to_string(&budget).map_err(|e| CommandError::Internal(e.to_string()))?,
        ),
        result_json: None,
        error: None,
        created_at: now,
        started_at: None,
        finished_at: None,
        prompt_tokens: None,
        completion_tokens: None,
        cache_read_tokens: None,
    };
    crate::db::insert_subagent(&tx, &row).map_err(CommandError::DbError)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;
    Ok(SpawnAck {
        subagent_id,
        task_id: None,
        trace_id,
        status: SubagentStatus::Queued,
        deduped: false,
        budget,
    })
}
/// 行上落盘的生效预算（读预算的唯一读法）：解析失败回落默认值。
///
/// 幂等命中时要用**已在跑**那条的预算回显——那才是真正生效的值。
pub(crate) fn budget_of_row(row: &SubagentRow) -> SubagentBudget {
    row.budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
/// 按 subagent_id / task_id / 子会话 id 解析行（设计 §5 双键 +  会话键：
/// 任务卡停止按钮与子会话停止键只持有会话 id）。
/// 前缀约定：subagent_id 恒以 `sa_` 开头；task_id 与 session_id 均为裸 uuid——
/// 先 task_id 后 session_id 双探测（ 批勘误）。
pub(crate) fn resolve_row(conn: &rusqlite::Connection, key: &str) -> CommandResult<SubagentRow> {
    let row = if key.starts_with("sa_") {
        crate::db::load_subagent(conn, key).map_err(CommandError::DbError)?
    } else {
        match crate::db::find_subagent_by_task(conn, key).map_err(CommandError::DbError)? {
            Some(r) => Some(r),
            None => {
                crate::db::find_subagent_by_session(conn, key).map_err(CommandError::DbError)?
            }
        }
    };
    row.ok_or_else(|| CommandError::InvalidArgument {
        field: "subagent_id".into(),
        value: key.to_string(),
        reason: "subagent 不存在（subagent_id / task_id / 会话 id 均未命中）".into(),
    })
}
/// spawn 异步包装：持锁执行核心 → 审计 subagent_spawned → 发 subagent-spawned
/// 事件（行建好即通知前端重拉，收尾前就可见可取消）→ 派发 runner。
pub async fn spawn_subagent(app: &AppHandle, req: SpawnRequest) -> CommandResult<SpawnAck> {
    // LLM 未显式给预算时用配置默认（config subagentMax* 三项，resolve 内钳制）；
    // 显式给了的仍走 clamp_budget 硬顶。Gate 并发数（max_running）保持常量不在本路径。
    let mut req = req;
    if req.budget.is_none() {
        let cfg = crate::bot::load_config(app);
        req.budget = Some(crate::bot::params::resolve_subagent_budget(&cfg));
    }
    // 配置档 max_turn 只收紧（规则见 apply_onion_turn_cap）
    if let Some(b) = req.budget.as_mut() {
        apply_onion_turn_cap(b, req.onion.as_ref().and_then(|o| o.max_turn));
    }
    // 审计字段先拷出（闭包要 move req 进 spawn_blocking）；校验只走 locked 核心
    //（wrapper 侧重复调用已删——）
    let audit_parent_session = req.parent_session_id.clone();
    let audit_profile = req.profile;
    let audit_model = req.model_profile.clone();
    // stage 走 in-memory 闭包——spawn 触发 runner 是同步启动，闭包 move 进 runner；
    // 不落库（详见 SubagentStage 注释）。SubagentStage 是 Copy，所以这里直接搬走。
    let runner_stage = req.stage;
    // 配置档同理走内存闭包（理由见 SpawnOnion 注释）
    let runner_onion = req.onion.clone();
    let app2 = app.clone();
    let ack = tauri::async_runtime::spawn_blocking(move || -> CommandResult<SpawnAck> {
        let _g = crate::db::lock_db_write();
        let mut conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        let now = crate::db::now_ms();
        spawn_subagent_locked(&mut conn, &req, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent spawn 线程 join 失败：{e}")))??;
    // 命中幂等键：这一份活已经在跑了——不建行（本来就没建）、不发事件（前端
    // 已经见过这一行）、**不再起第二个 runner**（起第二个就是同一份活跑两遍、
    // 各烧一份预算）。
    if ack.deduped {
        crate::bot::audit_log(
            app,
            &format!(
                "subagent_spawn_deduped | id: {} | parent_session: {} | profile: {}",
                ack.subagent_id,
                audit_parent_session.as_deref().unwrap_or("-"),
                audit_profile.as_str(),
            ),
        );
        return Ok(ack);
    }
    // 审计：结构化 + 文本行（设计 §3 事件字段）。task_id 恒空（新行无卡），
    // 字段仍留——老审计日志按该键反查的查询不受影响
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "subagent_spawned",
        &[
            ("subagent_id", ack.subagent_id.clone()),
            ("task_id", ack.task_id.clone().unwrap_or_default()),
            (
                "parent_session_id",
                audit_parent_session.clone().unwrap_or_default(),
            ),
            ("profile", audit_profile.as_str().to_string()),
            ("model", audit_model.unwrap_or_default()),
            ("trace_id", ack.trace_id.clone()),
        ],
    );
    crate::bot::audit_log(
        app,
        &format!(
            "subagent_spawned | id: {} | parent_session: {} | profile: {} | trace: {}",
            ack.subagent_id,
            audit_parent_session.as_deref().unwrap_or("-"),
            audit_profile.as_str(),
            ack.trace_id,
        ),
    );
    // 非阻塞派发 runner（设计 §5：spawn 立即返回；执行在后台推进状态机）。
    // spawned 事件在派发前发：不与 runner 的收尾事件竞争先后，行建好即告知
    emit_subagent_spawned(app, &ack.subagent_id, audit_parent_session.as_deref());
    let runner_app = app.clone();
    let runner_sid = ack.subagent_id.clone();
    tauri::async_runtime::spawn(async move {
        run_subagent(runner_app, runner_sid, runner_stage, runner_onion).await
    });
    Ok(ack)
}
/// 并发上限（设计 §6 已）：全局 3、每个派发主会话 2。超限排队不失败
///（status=queued），有槽位唤醒。
pub const MAX_GLOBAL_RUNNING: usize = 3;
pub const MAX_PER_SESSION_RUNNING: usize = 2;
pub(crate) fn gate() -> &'static SubagentGate {
    static GATE: std::sync::OnceLock<SubagentGate> = std::sync::OnceLock::new();
    GATE.get_or_init(|| SubagentGate {
        state: std::sync::Mutex::new(GateState {
            global_running: 0,
            per_session: HashMap::new(),
            waiting: HashMap::new(),
        }),
        notify: tokio::sync::Notify::new(),
    })
}
/// 进程级并发闸。FIFO 口径勘误 采纳后的简化）：
/// 持久队列 + notified().await 在 future 被 drop 时会泄漏队列项（取消不安全），
/// 改为「RAII 等待计数 + 轮询/唤醒重试」——同会话内先到先得（严格），
/// 跨会话近似公平（3 槽争用面极小，A 期接受）。
pub(crate) struct SubagentGate {
    pub(crate) state: std::sync::Mutex<GateState>,
    pub(crate) notify: tokio::sync::Notify,
}
impl SubagentGate {
    /// 可取消排队——每轮 50ms 退避后调 `should_abort`，true 即退队返回
    /// None（waiting 计数由 WaitingGuard RAII 递减）。取消判定由调用方给
    ///（runner 查 DB 行终态），闸门本身不依赖存储。
    pub(crate) async fn wait_slot_cancellable(
        parent_session: Option<String>,
        mut should_abort: impl FnMut() -> bool,
    ) -> Option<GateTicket> {
        // 登记等待席位（RAII：无论从哪条路径退出，计数都会被递减）
        {
            let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
            if let Some(k) = &parent_session {
                *st.waiting.entry(k.clone()).or_insert(0) += 1;
            }
        }
        let _waiting = WaitingGuard {
            parent_session: parent_session.clone(),
        };
        loop {
            if should_abort() {
                return None;
            }
            let outcome = {
                let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
                let global_ok = st.global_running < MAX_GLOBAL_RUNNING;
                let sess_running = parent_session
                    .as_deref()
                    .and_then(|k| st.per_session.get(k).copied())
                    .unwrap_or(0);
                let sess_ok = sess_running < MAX_PER_SESSION_RUNNING;
                // 不做「等待序数」公平判定（waiting<=1 的判定
                // 在两个并发 waiter 同见 waiting=2 时会互相让行 → 集体饿死）。
                // 改为纯「试占」：有空槽即取，50ms 重试保证无饥饿。
                // FIFO 降级为 best-effort（waiting 计数仅供 check 展示排队位），
                // 勘误登记于批 spec。
                if global_ok && sess_ok {
                    st.global_running += 1;
                    if let Some(k) = &parent_session {
                        *st.per_session.entry(k.to_string()).or_insert(0) += 1;
                    }
                    Some(true)
                } else {
                    None
                }
            };
            if outcome == Some(true) {
                return Some(GateTicket { parent_session });
            }
            // 没轮到：等唤醒或 50ms 退避重试（无持久队列项，取消安全）
            gate().notify.notify_one();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// 排队位次（check progress 用）：同会话前面还有几个等待者；
    /// None = 不在等待（运行中或无关会话）。
    pub(crate) fn queue_position(parent_session: Option<&str>) -> Option<usize> {
        let st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
        let n = parent_session
            .and_then(|k| st.waiting.get(k).copied())
            .unwrap_or(0);
        (n > 0).then(|| n - 1)
    }
}
pub(crate) struct GateState {
    pub(crate) global_running: usize,
    /// running 计数：parent_session → 数
    pub(crate) per_session: HashMap<String, usize>,
    /// 等待计数：parent_session → 数（WaitingGuard Drop 递减，取消安全）
    pub(crate) waiting: HashMap<String, usize>,
}
/// 排队票据：Drop 释放槽位 + 递减等待计数（全部 RAII，无泄漏面）。
pub(crate) struct GateTicket {
    pub(crate) parent_session: Option<String>,
}
impl Drop for GateTicket {
    fn drop(&mut self) {
        let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
        st.global_running = st.global_running.saturating_sub(1);
        if let Some(k) = &self.parent_session {
            if let Some(c) = st.per_session.get_mut(k) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    st.per_session.remove(k);
                }
            }
        }
        drop(st);
        gate().notify.notify_one();
    }
}
/// 等待席位：Drop 递减 waiting 计数（waiter future 被 drop 也安全）。
struct WaitingGuard {
    parent_session: Option<String>,
}
impl Drop for WaitingGuard {
    fn drop(&mut self) {
        if let Some(k) = &self.parent_session {
            let mut st = crate::db::lock::lock_or_recover(&gate().state, "subagent gate");
            if let Some(c) = st.waiting.get_mut(k) {
                *c = c.saturating_sub(1);
                if *c == 0 {
                    st.waiting.remove(k);
                }
            }
        }
    }
}
/// 行刚建好（queued）就发一条：子 agent 列表只靠事件驱动重拉，没有这条，
/// 新行要等收尾才可见——长跑任务期间无处取消
fn emit_subagent_spawned(app: &AppHandle, subagent_id: &str, parent_session_id: Option<&str>) {
    let _ = app.emit_to(
        "main",
        "subagent-spawned",
        serde_json::json!({
            "subagentId": subagent_id,
            "parentSessionId": parent_session_id,
        }),
    );
}
