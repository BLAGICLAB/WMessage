//! 子 Agent 编排器（orchestrator）—— 方案 B（docs/SUBAGENT-ORCHESTRATION-DESIGN-2026-09-27.md）。
//!
//! 与 bot_execute_task 的边界（设计 §2）：bot_execute_task 是「单发子 agent」（用户点执行，
//! 无编排语义）；本模块是「受管子 agent」（主 agent 派发，orchestrator 管生命周期/预算/
//! 审计）。两者复用 exec 运行基建，但入口、存储、事件各自独立。
//!
//! 分批（设计 §11）：
//! - `subagents` 表 + 生命周期状态机 + spawn/check/cancel 内部实现 +
//!   parent_task_id 关联（行上存派发时的主卡 id，不建子卡）+ 预算字段落表 +
//!   审计 subagent_spawned。不暴露 LLM 工具、不注册 tauri 命令。
//! - 三工具进 registry + 递归双保险 + 任务包装渲染 + 提示词 + runner + 收尾 JSON 解析。
//! - 预算强制（turns/tool_calls/wall）+ 并发排队（per-session 2 / global 3）+ 前端。
//!
//! 结构约定（同 task_patch / task_patch_locked 先例）：纯 DB 核心 `*_locked` 可在
//! in-memory conn 上单测；异步包装负责 spawn_blocking + DB_WRITE_LOCK + 审计 + 广播。

use crate::db::{now_ms, SubagentBudget, SubagentProfile, SubagentRow, SubagentStatus};
use crate::error::{CommandError, CommandResult};
use futures_util::FutureExt;
use tauri::AppHandle;

/// 预算默认值与硬顶（设计 §6 已）——常量与钳制实现在 db::SubagentBudget（写口
/// 强制：spawn 与 task_patch 都过 clamped()），此处 re-export 保持原路径可用。
pub use crate::db::{
    DEFAULT_MAX_TOOL_CALLS, DEFAULT_MAX_TURNS, DEFAULT_MAX_WALL_SECONDS, MAX_TURNS_HARD_CAP,
};
/// check_subagent 的 wait_ms 上限（设计 §5；放宽理由见
/// docs/SUBAGENT-CHECK-BACKOFF-DESIGN-2026-10-18.md）。
///
/// 上限放到 10 分钟：每返回一次 check_subagent，主 agent 就要多走一整个
/// LLM 轮次（重发请求 + 重算上下文 + 把轮询结果再塞进历史），所以「早知道」
/// 的边际收益远小于「少醒几次」。短上限逼着模型反复短轮询——实测一个 3 分钟
/// 的调研子任务被拆成 4 次 30s 等待，每次都是一轮模型开销，最后还因为等不到
/// 而把「仍在进行中」当成了整轮答复。一次给足预算让它整个睡到终态，轮次降到 1。
pub const CHECK_WAIT_MS_CAP: u64 = 600_000;
/// wait 兜底轮询步长阶梯（毫秒）。首档 1500ms：终态另有事件唤醒
/// （见 `finish_notify`），这条阶梯只是漏通知时的安全网，所以起手就不必密——
/// 越密只是越频繁地开库跑全量 DDL。末档后恒定封顶：兜底延迟要有上界，
/// 否则退化成看不见的卡住。
const CHECK_POLL_LADDER_MS: [u64; 10] = [
    1_500, 3_000, 6_000, 12_000, 24_000, 30_000, 60_000, 120_000, 300_000, 600_000,
];

/// 三段式派发阶段（设计 §8.5）——主 agent 在 recon/executor/verifier 三段闭环中
/// 选定当前阶段的角色语义。**不破坏 SubagentProfile 枚举**——profile 仍是工具白名单
/// 档（Research/Coder/General），stage 在 prompt 装配层额外拼一段角色 prompt：
/// - Recon → 拼 PROFILE_RECON（只读 + 联网调研）
/// - Executor → 拼 PROFILE_EXECUTOR（写文件/跑脚本/生成文档）
/// - Verifier → 拼 PROFILE_VERIFIER（只读 + 逐条 acceptance_criteria 验收）
/// 阶段不落库——spawn 触发 runner 的 in-memory 闭包 move 进 runner，崩溃重启的
/// queued 行降级到基线 PROFILE_*（profile 仍存表），属可控降级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStage {
    Recon,
    Executor,
    Verifier,
}

impl SubagentStage {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentStage::Recon => "recon",
            SubagentStage::Executor => "executor",
            SubagentStage::Verifier => "verifier",
        }
    }

    // 有意不实现 std::str::FromStr：解析失败返回 Option 而非 Err
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "recon" => SubagentStage::Recon,
            "executor" => SubagentStage::Executor,
            "verifier" => SubagentStage::Verifier,
            _ => return None,
        })
    }

    /// stage → 默认 profile 档（设计 §8.5 映射）：
    /// - recon/verifier → research（只读工具足够）
    /// - executor → coder（写类工具必需）
    /// 调用方未显式给 profile 时按此派生；显式给了的 profile 仍优先（角色与白名单解耦）。
    pub fn default_profile(self) -> SubagentProfile {
        match self {
            SubagentStage::Recon | SubagentStage::Verifier => SubagentProfile::Research,
            SubagentStage::Executor => SubagentProfile::Coder,
        }
    }
}

/// 派发时已解析校验过的 per-onion 配置档。
///
/// **随内存闭包进 runner，不落 subagents 行**：这两项是「这次派发做了什么决定」，
/// 不是子 agent 的固有属性——重启后不会有第二个派发方重跑同一次 spawn
/// （行已终态）。真正需要长期留存的是 `bot_sessions` 的两列，runner 建会话时写。
#[derive(Debug, Clone)]
pub struct SpawnOnion {
    /// 配置档名，同时是 `<数据目录>/onions/<config>.yaml` 的文件名
    pub config: String,
    /// 配置档 `max_turn`：轮数上限，只收紧不覆盖
    pub max_turn: Option<u32>,
}

/// spawn 入参（设计 §5 工具签名的内部形态）。
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub objective: String,
    pub profile: SubagentProfile,
    /// 三段式派发阶段（设计 §8.5）：None = 走基线 PROFILE_*（单段派发兼容路径）。
    /// 拼装阶段：runner 启动时随闭包 in-memory 传过去（详见 `spawn_subagent`）。
    pub stage: Option<SubagentStage>,
    /// **spawn 必填**，每条可检验；落行上 acceptance_json，runner 渲染包装时现读
    pub acceptance_criteria: Vec<String>,
    /// 上下文摘要（设计 §8.5「上下文摘要」段）：只给必要背景，不倒主对话全文。
    /// 落行上 context_summary——runner 渲染包装时从行上现读。
    pub context_summary: Option<String>,
    pub budget: Option<SubagentBudget>,
    /// A 期参数透传：None/无效值回退当前 active model（设计 §5）
    pub model_profile: Option<String>,
    /// per-onion 配置档（设计 §3.7 三段流水线的权限载体）。
    /// None = 不套配置档，工具集就是 profile 白名单原样。
    ///
    /// 名字与上限都在派发时**已解析校验**：加载失败必须在建行之前就报错，
    /// 否则留下一条「派发成功但永远跑不起来」的幽灵行。
    pub onion: Option<SpawnOnion>,
    /// 主卡（编排计划卡）id；缺卡 → TaskNotFound
    pub parent_task_id: Option<String>,
    /// 派发者主会话 id（串链审计）
    pub parent_session_id: Option<String>,
}

/// spawn 应答：非阻塞立即返回（设计 §5）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnAck {
    pub subagent_id: String,
    /// 历史子任务卡 id：子 agent 不再建卡，新应答恒为 None（字段保留兼容旧消费方）
    pub task_id: Option<String>,
    pub trace_id: String,
    pub status: SubagentStatus,
    /// 命中幂等键：返回的是**已在跑**的同一个子 agent，本次没有新建行、
    /// 也没有再起一个 runner。消费方据此区分「派发成功」与「重复派发未新建」。
    pub deduped: bool,
    /// **生效**预算（钳制后的值，不是调用方给的值）。
    ///
    /// 回显给调用方是必须的：调用方给的墙钟可能被钳到 30..=3600，模型不知道
    /// 自己的子 agent 最多能跑多久，就会按「我给了一小时」安排后续动作。
    pub budget: SubagentBudget,
}

/// check 应答（设计 §5/§7：status/progress/result?/error?）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckState {
    pub subagent_id: String,
    /// 历史子任务卡 id：无卡行恒为 None（字段保留兼容旧消费方）
    pub task_id: Option<String>,
    /// 子 agent 执行会话（围观串链键；runner 建会话前为 None）
    pub session_id: Option<String>,
    pub status: SubagentStatus,
    /// 进度小计：验收标准逐条达成数 + 预算消耗（A3 起带 turns/tool_calls）
    pub progress: serde_json::Value,
    /// 收尾结构化结果（§7），未收尾为 None
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// cancel 应答：`already_terminal` = 目标已是终态（幂等 no-op，不报错）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelAck {
    pub ok: bool,
    pub subagent_id: String,
    pub status: SubagentStatus,
    pub already_terminal: bool,
}

/// 预算钳制：None → 默认值；其余交给 SubagentBudget::clamped（硬顶 50 / 下限 1）。
pub fn clamp_budget(budget: Option<SubagentBudget>) -> SubagentBudget {
    budget.unwrap_or_default().clamped()
}

/// 配置档的 `max_turn` 只收紧：派发给的轮数更宽时不许被配置档顶开。
///
/// 取 min 而非覆盖——配置档是权限上限，不是「优先于调用方的指令」。
fn apply_onion_turn_cap(budget: &mut SubagentBudget, cap: Option<u32>) {
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
fn budget_of_row(row: &SubagentRow) -> SubagentBudget {
    row.budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// 按 subagent_id / task_id / 子会话 id 解析行（设计 §5 双键 +  会话键：
/// 任务卡停止按钮与子会话停止键只持有会话 id）。
/// 前缀约定：subagent_id 恒以 `sa_` 开头；task_id 与 session_id 均为裸 uuid——
/// 先 task_id 后 session_id 双探测（ 批勘误）。
fn resolve_row(conn: &rusqlite::Connection, key: &str) -> CommandResult<SubagentRow> {
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

/// check 纯 DB 核心：幂等读 + 进度小计（验收标准逐条达成比）。
/// 进度口径：total = 行上 acceptance_json 条数（派发时写死，全生命周期不变）；
/// done = 收尾 JSON 里 met=true 的条数——普通行读 `acceptance_check`
/// （RESULT_SCHEMA_HINT 要求的逐条验收），verifier 行读 `checks`
/// （verifier 独有 schema，与 classify 的结构识别同口径）。
/// 未收尾时 done 恒为 0。键名沿用 subtasksDone/subtasksTotal，前端契约不变。
pub(crate) fn check_subagent_locked(
    conn: &rusqlite::Connection,
    key: &str,
) -> CommandResult<CheckState> {
    let row = resolve_row(conn, key)?;
    let declared = acceptance_criteria_of(&row);
    let subtasks_total: usize = declared.len();
    let result: Option<serde_json::Value> = row
        .result_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let subtasks_done: usize = result
        .as_ref()
        .and_then(|v| {
            v.get("acceptance_check")
                .and_then(|c| c.as_array())
                .or_else(|| v.get("checks").and_then(|c| c.as_array()))
        })
        .map(|a| count_met_against_declared(&declared, a))
        // 交叉核对后仍封顶：重复条目已被去重，这里再兜一层防御
        // （子 agent 重复报同一条时 done 不得超过 total）
        .unwrap_or(0)
        .min(subtasks_total);
    // 排队位次：queued 状态时给出 FIFO 前面还有几个
    let queue_position = if row.status == SubagentStatus::Queued {
        SubagentGate::queue_position(row.parent_session_id.as_deref())
    } else {
        None
    };
    let mut progress = serde_json::json!({
        "subtasksDone": subtasks_done,
        "subtasksTotal": subtasks_total,
    });
    // 轮数读数：子 agent 在跑时 ctx 里有活的计数器，终态后守卫注销读不到。
    // 预算从行上 budget_json 取——轮数与预算放一起看才有意义（10/20 与 10/200
    // 是完全不同的紧迫度）。readOnly 消费，不参与任何强制。
    if let Some(budget) = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<SubagentBudget>(s).ok())
    {
        let used = crate::tool_guard::subagent_ctx(row.session_id.as_deref())
            .map(|c| c.used_rounds.load(std::sync::atomic::Ordering::Relaxed));
        if let Some(used) = used {
            progress["roundsUsed"] = serde_json::json!(used);
            progress["roundsBudget"] = serde_json::json!(budget.max_turns);
        }
    }
    if let Some(pos) = queue_position {
        progress["queuePosition"] = serde_json::json!(pos);
    }
    Ok(CheckState {
        subagent_id: row.id,
        task_id: row.task_id,
        session_id: row.session_id,
        status: row.status,
        progress,
        result,
        error: row.error,
    })
}

/// cancel 纯 DB 核心：queued/running → cancelled（error=原因）；终态 no-op。
/// 子 agent 无卡，取消只动行——前端经 subagent-finished 事件看状态。
pub(crate) fn cancel_subagent_locked(
    conn: &rusqlite::Connection,
    key: &str,
    reason: &str,
    now: i64,
) -> CommandResult<CancelAck> {
    let row = resolve_row(conn, key)?;
    if row.status.is_terminal() {
        return Ok(CancelAck {
            ok: true,
            subagent_id: row.id,
            status: row.status,
            already_terminal: true,
        });
    }
    crate::db::update_subagent_status(conn, &row.id, SubagentStatus::Cancelled, now, Some(reason))
        .map_err(CommandError::DbError)?;
    Ok(CancelAck {
        ok: true,
        subagent_id: row.id,
        status: SubagentStatus::Cancelled,
        already_terminal: false,
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
        let now = chrono::Utc::now().timestamp_millis();
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

/// 等待退出原因（审计事件 `subagent_check_wait` 的 exit 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitExit {
    /// 等到了终态
    Terminal,
    /// 预算耗尽仍未终态（wait_ms 到点）
    Timeout,
    /// 用户 /stop
    Stopped,
}

impl WaitExit {
    fn as_str(self) -> &'static str {
        match self {
            WaitExit::Terminal => "terminal",
            WaitExit::Timeout => "timeout",
            WaitExit::Stopped => "stopped",
        }
    }
}

/// 终态唤醒信号：任何子 agent 转入终态都经 `emit_subagent_finished` 这一个出口，
/// 在那里 notify_waiters 即可覆盖排队期取消 / 启动中止 / ghost 路径，不漏。
/// 用进程内 `Notify` 而非 Tauri 事件——等待方就在同一进程里，没必要过事件总线。
fn finish_notify() -> &'static tokio::sync::Notify {
    static N: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
    N.get_or_init(tokio::sync::Notify::new)
}

/// 兜底轮询步长：阶梯内递增，出阶梯后恒定封顶。
fn next_poll_step(step: usize) -> u64 {
    CHECK_POLL_LADDER_MS[step.min(CHECK_POLL_LADDER_MS.len() - 1)]
}

/// 等待核心：对「读一次状态」的取数函数泛型，因此可在没有 AppHandle 的单测里
/// 直接驱动（生产传入 spawn_blocking + open_db 的闭包）。
///
/// 唤醒策略 = 终态事件 **或** 兜底步长，先到先醒。正常路径下子 agent 一转终态
/// 就立刻返回，阶梯走不完；阶梯只兜「通知漏了」的情况。
async fn wait_until_terminal<F, Fut>(
    read: F,
    wait_ms: u64,
    stop: Option<&crate::bot_slash::StopToken>,
) -> Result<(CheckState, WaitExit), CommandError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = CommandResult<CheckState>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms);
    let mut step = 0usize;
    loop {
        // 先把唤醒监听挂上、再读状态：顺序反了会出现「读完状态到开始等待之间
        // 恰好转入终态」的窗口，而 notify_waiters 不留 permit，那次通知就丢了
        //（丢一次不致命，兜底阶梯接得住，只是白等一个步长）。
        let notified = finish_notify().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let state = read().await?;
        if state.status.is_terminal() {
            return Ok((state, WaitExit::Terminal));
        }
        // 停止先于超时判定：用户按了 /stop 就该按「被叫停」记账，
        // 而不是记成「等够了没等到」。
        if stop.is_some_and(|t| t.stopped()) {
            return Ok((state, WaitExit::Stopped));
        }
        if std::time::Instant::now() >= deadline {
            return Ok((state, WaitExit::Timeout));
        }
        let step_ms = next_poll_step(step);
        step += 1;
        let _ = tokio::time::timeout(std::time::Duration::from_millis(step_ms), notified).await;
    }
}

/// check 异步包装：wait_ms=0 单次读；>0 等待至终态、预算耗尽或用户停止
/// （幂等可轮询，设计 §5；等待策略见 docs/SUBAGENT-CHECK-BACKOFF-DESIGN-2026-10-18.md）。
pub async fn check_subagent(
    app: &AppHandle,
    key: &str,
    wait_ms: u64,
    stop: Option<&crate::bot_slash::StopToken>,
    session_id: Option<&str>,
) -> CommandResult<CheckState> {
    let wait = wait_ms.min(CHECK_WAIT_MS_CAP);
    let started = std::time::Instant::now();
    let app2 = app.clone();
    let key2 = key.to_string();
    let (state, exit) = wait_until_terminal(
        || {
            let app = app2.clone();
            let key = key2.clone();
            async move {
                tauri::async_runtime::spawn_blocking(move || -> CommandResult<CheckState> {
                    let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
                    check_subagent_locked(&conn, &key)
                })
                .await
                .map_err(|e| CommandError::from(format!("subagent check 线程 join 失败：{e}")))?
            }
        },
        wait,
        stop,
    )
    .await?;
    // 问到终态 = 父上下文已拿到这份结果：记进去重表，digest 不再重念，
    // 自动汇报轮也不会因为「没人提过」而重复讲同一件事。
    if state.status.is_terminal() {
        mark_digested(session_id, &state.subagent_id);
    }
    // 等待口径留痕：wait_ms=0 的单次读不记（否则审计被即时查询刷屏）。
    // 有等待才记——本次优化唯一的验证手段，「轮次少了」不能靠感觉，
    // 得能看实际等待时长与退出原因的分布。
    if wait > 0 {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "subagent_check_wait",
            &[
                ("wait_ms", wait.to_string()),
                ("waited_ms", started.elapsed().as_millis().to_string()),
                ("exit", exit.as_str().to_string()),
            ],
        );
    }
    Ok(state)
}

/// cancel 异步包装：置 cancelled + 审计 + 远程停 runner。终态事件由 runner
/// 补发（排队闸/槽后复核/收尾路径各自 emit）——本函数不发，避免与 runner
/// 对同一行收尾双发、前端重复弹「子任务已取消」。
pub async fn cancel_subagent_async(
    app: &AppHandle,
    key: &str,
    reason: &str,
) -> CommandResult<CancelAck> {
    let app2 = app.clone();
    let key = key.to_string();
    let reason = reason.to_string();
    let audit_reason = reason.clone();
    let ack = tauri::async_runtime::spawn_blocking(move || -> CommandResult<CancelAck> {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app2).map_err(CommandError::DbError)?;
        let now = chrono::Utc::now().timestamp_millis();
        cancel_subagent_locked(&conn, &key, &reason, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent cancel 线程 join 失败：{e}")))??;
    if !ack.already_terminal {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Info,
            "subagent_cancelled",
            &[
                ("subagent_id", ack.subagent_id.clone()),
                ("reason", audit_reason.clone()),
            ],
        );
        crate::bot::audit_log(
            app,
            &format!(
                "subagent_cancelled | id: {} | reason: {}",
                ack.subagent_id,
                crate::bot::truncate_for_log(&audit_reason, 120)
            ),
        );
        // 远程停止 runner 工具循环（令牌在 runner 注册；watcher 2s 兜底轮询）
        let token = crate::db::lock::lock_or_recover(
            &crate::app_state::subagent_stops(app),
            "app_state::subagent_stops",
        )
        .get(&ack.subagent_id)
        .cloned();
        if let Some(token) = token {
            token.stop();
        }
    }
    Ok(ack)
}

// 工具实现 + runner + 收尾解析

use crate::bot_chat::ChatGuard;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::Emitter;

/// 持 DB_WRITE_LOCK 的阻塞 DB 操作辅助（runner 链路各步共用）。
async fn db_locked<R, F, T>(app: &AppHandle<R>, f: F) -> CommandResult<T>
where
    R: tauri::Runtime,
    F: FnOnce(&mut rusqlite::Connection) -> CommandResult<T> + Send + 'static,
    T: Send + 'static,
{
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let mut conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        f(&mut conn)
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent db 线程 join 失败：{e}")))?
}

/// 从文本提取**最后一个**顶层 JSON 对象（设计 §7 解析策略）。
/// 字符串感知的花括号扫描：收集深度 1 的 {...} span，从末往前试 serde 解析，
/// 首个成功且为 object 的返回。找不到/全失败 → None。
/// 文本里所有**花括号配平**的顶层对象片段（按出现顺序）。
///
/// 收尾解析从末往前逐个试这些片段（见 [parse_final_json]）：最后一个对象坏了
/// 就试前一个——与「取最后一个能直接解析的对象」的老语义一致。
fn json_object_spans(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *c == '\\' {
                escaped = true;
            } else if *c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    spans.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    spans
        .into_iter()
        .map(|(a, b)| chars[a..b].iter().collect())
        .collect()
}

/// 被截断的对象片段：最后一个 `{` 到文末。
///
/// 括号配平的对象找不着时才会走到这里——收尾 JSON 写到一半被 max_tokens
/// 截断，就是这种形状。
fn truncated_object_tail(text: &str) -> Option<String> {
    let start = text.rfind('{')?;
    let tail = text[start..].trim();
    if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    }
}

/// 收尾 JSON 的**本地修复**（结构化输出三段修复的第 2 段）。
///
/// 业界口径：干净解析 → 本地修复 → 带修正提示重试 LLM。本地修复零 token、
/// 零延迟，必须排在补答之前——补答要再走一次 LLM 轮次。
///
/// 只做两种**不会改坏内容**的修复：
/// - 尾逗号（`{"a":1,}` → `{"a":1}`）
/// - 被截断的闭合（`{"a":1` → `{"a":1}`）
///
/// **不做** `None`/`True` → `null`/`true` 这类字面量替换——字符串内容里出现
/// 这些词是合法的，替换会把模型写的正文字段改坏，比解析失败更糟。
fn repair_json_candidate(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 1) 尾逗号
    let no_commas = strip_trailing_commas(trimmed);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&no_commas) {
        if v.is_object() {
            return Some(no_commas);
        }
    }
    // 2) 截断闭合（在尾逗号修复之后再做一次——两者可以叠加）
    let closed = close_truncated_json(&no_commas)?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&closed) {
        if v.is_object() {
            return Some(closed);
        }
    }
    None
}

/// 去掉结构性的尾逗号（string-aware）：`,` 之后只跟空白再接 `}` / `]` 时删它。
fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut drop = vec![false; chars.len()];
    let mut in_string = false;
    let mut escaped = false;
    for i in 0..chars.len() {
        let c = chars[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            ',' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                    drop[i] = true;
                }
            }
            _ => {}
        }
    }
    if !drop.iter().any(|d| *d) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, c) in chars.iter().enumerate() {
        if !drop[i] {
            out.push(*c);
        }
    }
    out
}

/// 补上被截断的闭合括号（string-aware）。
///
/// 字符串没闭合时**补一个引号再补括号**：模型写到一半被 max_tokens 截断，
/// 常见形态就是截在字符串中间（`{"status":"succeeded","summary":"调研完成`）。
/// 不补就整份报废——status 这类关键字段明明已经写出来了，却跟着一起丢。
/// 补出来的字符串是**残句**，但结构完整、关键字段在，比什么都拿不到强。
///
/// 只在片段末尾才会出现没闭合的字符串（片段本身就是尾巴），所以补引号不会
/// 把后面正常的内容吞进字符串。
fn close_truncated_json(s: &str) -> Option<String> {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in s.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = String::with_capacity(s.len() + 8);
    out.push_str(s);
    if in_string {
        out.push('"');
    }
    for c in stack.iter().rev() {
        out.push(*c);
    }
    Some(out)
}

/// 收尾 JSON 解析（干净解析 + 本地修复）。
///
/// 逐个候选对象从后往前试：最后一个对象坏了就试前一个——与「取最后一个能
/// 直接解析的对象」的老语义一致。最后再试一次**被截断的对象**（最后一个
/// `{` 到文末）：括号配平不上时，截断的 JSON 正是靠这条救。
pub(crate) fn parse_final_json(text: &str) -> Option<serde_json::Value> {
    for candidate in json_object_spans(text).into_iter().rev() {
        if let Some(v) = parse_json_candidate(&candidate, false) {
            return Some(v);
        }
    }
    // 截断的尾段：只在修出来的对象**认得出收尾语义**时才认（有 status 或
    // checks）。半截 JSON 往往连 status 都还没写就断了，修出来是个空壳——
    // 而判定里「没有 status」默认按 succeeded 算，把「没说完」说成「说完了」。
    if let Some(tail) = truncated_object_tail(text) {
        if let Some(v) = parse_json_candidate(&tail, true) {
            return Some(v);
        }
    }
    None
}

/// 单个候选片段的解析（干净 → 本地修复）。
///
/// `require_semantic`：额外要求对象里有 `status`（收尾语义）或 `checks`
/// （verifier 的逐条验收）。只对截断尾段启用——见 [parse_final_json]。
fn parse_json_candidate(candidate: &str, require_semantic: bool) -> Option<serde_json::Value> {
    let v = serde_json::from_str::<serde_json::Value>(candidate)
        .ok()
        .or_else(|| {
            repair_json_candidate(candidate)
                .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
        })?;
    if !v.is_object() {
        return None;
    }
    if require_semantic && v.get("status").is_none() && v.get("checks").is_none() {
        return None;
    }
    Some(v)
}

/// 工具预算触顶后的强制改判条件（收尾窗语义，纯函数）：
/// 触顶 + 终态还不是 budget_exceeded + 模型没交出可解析的收尾 JSON，
/// 三者同时成立才把终态改判 budget_exceeded(max_tool_calls)。
/// 交了卷的触顶是真实完成——dispatch 收尾窗放行过写产物，改判只会把
/// 父 agent 的「带残留重试」逻辑错误触发。
fn budget_force_needed(
    tool_calls_used: usize,
    max_tool_calls: u32,
    status: SubagentStatus,
    wrapped_up_cleanly: bool,
) -> bool {
    tool_calls_used >= max_tool_calls as usize
        && status != SubagentStatus::BudgetExceeded
        && !wrapped_up_cleanly
}

/// 收尾结果判定（纯函数，设计 §6/§7）：
/// 墙钟熔断 → budget_exceeded(max_wall_seconds) + 正文留产；
/// 轮数熔断 → budget_exceeded(max_turns) + 正文留产；
/// 其他循环错误 → failed；正文解析成功 → 模型自报 succeeded/failed（自报
/// cancelled/budget_exceeded 视为 failed + error 说明）；解析失败 →
/// failed + result_json_unparseable + 原文存 summary（部分结果不丢弃）。
///
/// 这里只看得到子 agent 自己说的话，判不了产物；机器验收在调用方之后才跑，
/// 它的结论由 `reconcile_machine_override` 在落库前回头修正终态——两步分开
/// 是为了让「模型说了什么」与「服务端验到了什么」各自独立可测。
pub(crate) fn classify_outcome(
    run_err: Option<&str>,
    wall_exceeded: bool,
    final_text: Option<&str>,
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    let text = final_text.unwrap_or_default();
    if wall_exceeded {
        // 外层墙钟兜底（future 被 timeout 掐掉）：正文随 future 一起没了，
        // 没有可留的产出。常态路径是轮边界的墙钟熔断（下一条）——那边带正文。
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_wall_seconds".into()),
            None,
        );
    }
    if let Some(err) = run_err {
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(err, 300)),
            None,
        );
    }
    // 墙钟熔断（模型循环在轮边界退出，正文带着）：与轮数熔断同款留产。
    //
    // 必须判在 FUSE_MARKER 之前——墙钟标记是「⏹ 已熔断（墙钟上限）」，
    // 而通用熔断标记是「⏹ 已熔断」，前者**包含**后者；判反了会被认成
    // 工具调用熔断（max_tool_calls），真实原因「墙钟触顶」又被丢掉。
    if text.contains(crate::bot_model_loop::WALL_FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_wall_seconds".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "墙钟预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    // 轮数熔断：熔断出口返回 Ok，正文带 ROUNDS_FUSE_MARKER（见 bot_model_loop）。
    // 终态仍是 budget_exceeded，但正文不是丢弃而是存进 result_json.summary——
    // 跑完的那十几轮里查到的东西对父 agent 仍有价值（判断该不该原目标重试、
    // 还是换 profile / 放宽验收），写 NULL 等于逼重试从零再来。
    if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_turns".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "轮数预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    // 工具熔断：正文带 FUSE_MARKER。轮数标记是它的前缀，所以必须判在轮数之后。
    //
    // 此前这条不认 → 落进 JSON 解析分支：模型照办输出 {"status":"succeeded"}
    // 就报成功（父 agent 因此不重试，产出残缺而无人知晓）；没有收尾 JSON 则
    // 报 failed + result_json_unparseable——真实原因「预算触顶」被丢掉。
    if text.contains(crate::bot_model_loop::FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_tool_calls".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "工具调用预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    // 干净解析 + 本地修复（尾逗号 / 截断闭合）——结构化输出三段修复的第 1、2 段。
    // 本地修复零 token 零延迟，排在补答（要再走一次 LLM）之前。
    let parsed = parse_final_json(text);
    match parsed {
        Some(v) => {
            // verifier 子 agent 输出独有 schema：含 checks 数组的 verified.json
            // ——只看 status 会漏掉「逐条 acceptance_criteria 验收」的真实语义。
            // 此处按结构识别走独立分支判定，不依赖 stage 字段（verifier 输出已经
            // 结构化，从结构出发比从 stage 出发的耦合面小）。
            if let Some(checks) = v.get("checks").and_then(|c| c.as_array()) {
                return classify_verifier_outcome(&v, checks);
            }
            let claimed = v
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("succeeded");
            match claimed {
                "succeeded" => (SubagentStatus::Succeeded, None, Some(v)),
                "failed" => (
                    SubagentStatus::Failed,
                    Some("model_reported_failure".into()),
                    Some(v),
                ),
                "cancelled" => (
                    SubagentStatus::Failed,
                    Some("model_reported_cancelled".into()),
                    Some(v),
                ),
                "budget_exceeded" => (
                    SubagentStatus::Failed,
                    Some("model_reported_budget_exceeded".into()),
                    Some(v),
                ),
                _ => (SubagentStatus::Succeeded, None, Some(v)),
            }
        }
        None => (
            SubagentStatus::Failed,
            Some(UNPARSEABLE_MARKER.into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "收尾 JSON 解析失败，原文截断存此防丢（设计 §7）"
            })),
        ),
    }
}

/// 收尾 JSON 解析失败时的补答（结构化输出三段修复的第 3 段）。
///
/// 业界口径：干净解析 → 本地修复 → 带修正提示重试 LLM。补答只做一次，
/// 且**消耗同一份墙钟预算**（重试不重置时钟）。
///
/// 只在「文本非空 + 不含熔断标记 + 本地修复后仍解析不出」时补答：
/// - 预算熔断（轮数 / 工具 / 墙钟）不是格式问题，补答只是白烧一次 token；
/// - 本地修复能救的已经在 `parse_final_json` 里救掉了，不必再问模型。
///
/// 返回补答后的正文（可解析）；补答失败或仍不可解析 → None（走原失败路径）。
async fn repair_final_json_once(
    app: &AppHandle,
    stop: &crate::bot_slash::StopGuard,
    text: &str,
    wall_budget: Option<crate::bot_model_loop::WallBudget>,
) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    for marker in [
        crate::bot_model_loop::ROUNDS_FUSE_MARKER,
        crate::bot_model_loop::WALL_FUSE_MARKER,
        crate::bot_model_loop::FUSE_MARKER,
    ] {
        if text.contains(marker) {
            return None;
        }
    }
    // 本地修复已经救回来的不算「解析失败」——别为一份能用的结果再烧一次 LLM
    if parse_final_json(text).is_some() {
        return None;
    }
    let prompt = format!(
        "你上一步的收尾输出无法被解析为收尾 JSON。\n\
         请把已经完成的工作整理成下面这个结构。**只输出 JSON 本身**，不要解释、不要调用任何工具：\n\
         {}\n\n\
         你上一步的原文（仅供参考，可能不完整或格式有误）：\n{}",
        crate::prompts::RESULT_SCHEMA_HINT,
        clip(text, 1500),
    );
    let msgs = vec![
        serde_json::json!({ "role": "system", "content": crate::prompts::SUBAGENT_BASE }),
        serde_json::json!({ "role": "user", "content": prompt }),
    ];
    // 一轮就够：补答只需要「把结果整理成 JSON」，不需要再探索。
    let out = crate::bot_model_loop::run_model_loop(
        app.clone(),
        msgs,
        1,
        stop,
        None,
        None,
        None,
        None,
        None,
        wall_budget,
    )
    .await
    .ok()?;
    let (reply, _refs, _trace) = out;
    if parse_final_json(&reply).is_some() {
        crate::bot::audit_log(
            app,
            &format!(
                "subagent_final_json_repaired | 收尾 JSON 补答成功（{} 字符）",
                reply.chars().count()
            ),
        );
        Some(reply)
    } else {
        crate::bot::audit_log(app, "subagent_final_json_repair_failed | 补答后仍不可解析");
        None
    }
}

/// verifier 路径判定：按 `checks[*].met` 综合决定子 agent 终态。
/// - checks 空：summary 含明确否决词（未通过/fail/reject/failed/否决）→ Failed；
///   否则视为 N/A 走 Succeeded（verifier 没排到检查项极少，fail-safe 默认放行）
/// - 全部 met=true → Succeeded
/// - 全部 met=false → Failed + 错误 reason 拼未通过条目的 criterion 列表（≤3 项）
///   给主 agent 看到「到底哪里没通过」的最小信号
/// - 部分 met=true → Succeeded（主 agent 看 result_json 自己决策下一步）
fn classify_verifier_outcome(
    v: &serde_json::Value,
    checks: &[serde_json::Value],
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    let total = checks.len();
    let passed = checks
        .iter()
        .filter(|c| c.get("met").and_then(|m| m.as_bool()) == Some(true))
        .count();
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
    if total == 0 {
        // checks 为空：按 summary 显式否决词判定
        let reject_keywords = ["未通过", "fail", "failed", "reject", "否决", "驳回"];
        let explicit_reject = reject_keywords
            .iter()
            .any(|kw| summary.to_lowercase().contains(&kw.to_lowercase()));
        if explicit_reject {
            return (
                SubagentStatus::Failed,
                Some(format!(
                    "verifier_summary_rejected: {}",
                    crate::bot::truncate_for_log(summary, 80)
                )),
                Some(v.clone()),
            );
        }
        return (SubagentStatus::Succeeded, None, Some(v.clone()));
    }
    if passed == total {
        return (SubagentStatus::Succeeded, None, Some(v.clone()));
    }
    if passed == 0 {
        // 全部未通过：把失败 criterion 列表（前 3 项）拼进 error reason 给主 agent 看
        let failed_list: Vec<String> = checks
            .iter()
            .take(3)
            .filter_map(|c| {
                let criterion = c.get("criterion").and_then(|s| s.as_str())?;
                Some(format!("{criterion}=false"))
            })
            .collect();
        let reason = if failed_list.is_empty() {
            "verifier_all_checks_failed".to_string()
        } else {
            format!("verifier_all_checks_failed: {}", failed_list.join("; "))
        };
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(&reason, 280)),
            Some(v.clone()),
        );
    }
    // 部分通过：判定 Succeeded（主 agent 看 result_json.checks 自行决策下一步）；
    // 这里保持纯函数不写审计——非关键信号，让 caller 在 run_subagent 收尾路径里
    // 决定是否打点（partical-pass 不是错误，重复审计反而拖累日志）。
    (SubagentStatus::Succeeded, None, Some(v.clone()))
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
        "\n预算:\nmax_turns={}, max_tool_calls={}, max_wall_seconds={}\n",
        budget.max_turns, budget.max_tool_calls, budget.max_wall_seconds
    ));
    w.push_str(&format!("\n产物目录:\n{}\n", artifact_dir.display()));
    w.push_str("\n完成后必须输出以下 JSON（status 与 summary 必填；acceptance_check / artifacts / blockers 没有就省掉）：\n");
    w.push_str(crate::prompts::RESULT_SCHEMA_HINT);
    w
}

/// 行上已声明的验收标准（渲染包装 / 机器判定 / 进度统计三处共用一个读法）
pub(crate) fn acceptance_criteria_of(row: &SubagentRow) -> Vec<String> {
    row.acceptance_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// 提示词用的软截断（按字符，不转义控制字符）。
///
/// **不复用 `truncate_for_log`**：那个会把换行/制表转义成 `\n` 形态，那是日志
/// 专用形态；回报段是要给模型读的，多行摘要转义成一坨反而更难读。
fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max).collect();
    out.push('…');
    out
}

/// 回报段的渲染上限：一次注入最多这几条 / 这么多字。
///
/// 上限不是洁癖：父会话的历史预算是共享的，一次注入十条长摘要就能把本轮
/// 对话挤掉。宁可少注入几条（下一轮还会补），也不要撑爆当轮上下文。
const DIGEST_MAX_ITEMS: usize = 8;
const DIGEST_MAX_SUMMARY_CHARS: usize = 600;
const DIGEST_MAX_ARTIFACTS: usize = 5;
/// 失败/超预算行的残留产出渲染上限（字）。
///
/// 比成功行的摘要短：残留产出的作用是让父 agent 判断「值不值得带残留重试」，
/// 不是让它读完——读太长会把当轮上下文挤爆。
const DIGEST_MAX_RESIDUAL_CHARS: usize = 300;

/// 机器验收未过的条目（只认服务端核验过的：`machine: true` 且 `met: false`）。
///
/// 不认模型自报的 `met: false`：那是子 agent 的自我评价，服务端没核验过，
/// 摆出来会让父 agent 分不清「服务端验了没过」与「模型说自己没过」。
fn machine_unmet_criteria(result: &serde_json::Value) -> Vec<String> {
    let arr = result
        .get("acceptance_check")
        .or_else(|| result.get("checks"))
        .and_then(|v| v.as_array());
    let Some(arr) = arr else { return Vec::new() };
    arr.iter()
        .filter(|it| {
            it.get("machine").and_then(|m| m.as_bool()) == Some(true)
                && it.get("met").and_then(|m| m.as_bool()) == Some(false)
        })
        .filter_map(|it| {
            it.get("criterion")
                .and_then(|c| c.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| clip(s, 60))
        })
        .collect()
}

/// 一段渲染好的子任务回报 + 本次注入后应记为「父上下文已吸收」的子 agent id。
pub(crate) struct SubagentDigest {
    pub text: String,
    pub absorb: Vec<String>,
}

/// 父侧装配（设计 §3.7「下一轮 assemble 检查」的落点）。
///
/// 口径三条，都是为了「齐了/没齐」在模型侧都看得见：
/// 1. **只注 `result_json`，绝不注子会话历史**——子 agent 的完整对话留在它自己
///    的会话行里，父上下文只吸收结构化收尾。这是 §3.7 要点 3 的硬要求。
/// 2. **终态即吸收**：终态行注入后记入 `absorb`，下一轮不再重复念同一份结果。
/// 3. **非终态不吸收**：还在跑的行下轮还要看它有没有完成，所以只报一句计数，
///    不逐条铺开。
///
/// 返回 None = 没有新东西可注入（该父会话的子任务都已汇报过，或一个都没有），
/// 调用方据此不加这段 system 消息。
pub(crate) fn build_subagent_digest(
    rows: &[SubagentRow],
    absorbed: &std::collections::HashSet<String>,
) -> Option<SubagentDigest> {
    let mut items = String::new();
    let mut absorb = Vec::new();
    let mut running = 0usize;
    let mut listed = 0usize;

    for row in rows {
        if absorbed.contains(&row.id) {
            continue;
        }
        let objective = clip(&row.objective, 40);
        if !row.status.is_terminal() {
            // 在途不逐条列：铺开只会挤占上下文，模型只需要知道「还有几个没回」
            running += 1;
            continue;
        }
        if listed >= DIGEST_MAX_ITEMS {
            continue;
        }
        listed += 1;
        absorb.push(row.id.clone());
        match row.status {
            SubagentStatus::Succeeded => {
                items.push_str(&format!("{listed}. {objective} → 成功\n"));
                if let Some(r) = row.result_json.as_deref().and_then(parse_result_json) {
                    if let Some(summary) = r
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        items.push_str(&format!(
                            "   摘要：{}\n",
                            clip(summary, DIGEST_MAX_SUMMARY_CHARS)
                        ));
                    }
                    if let Some(list) = r.get("artifacts").and_then(|v| v.as_array()) {
                        let shown: Vec<String> = list
                            .iter()
                            .take(DIGEST_MAX_ARTIFACTS)
                            .filter_map(|a| {
                                let path = a.get("path").and_then(|v| v.as_str())?;
                                let desc = a
                                    .get("description")
                                    .and_then(|v| v.as_str())
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty());
                                Some(match desc {
                                    Some(d) => format!("{}（{}）", clip(path, 80), clip(d, 60)),
                                    None => clip(path, 80),
                                })
                            })
                            .collect();
                        if !shown.is_empty() {
                            items.push_str(&format!("   产物：{}\n", shown.join("、")));
                        }
                    }
                    // 机检未过项：「成功」与「机检没过」两件事同时成立时，父 agent
                    // 必须看见后者——否则它只会以为这活干成了，把没过的项一起收下。
                    let unmet = machine_unmet_criteria(&r);
                    if !unmet.is_empty() {
                        items.push_str(&format!(
                            "   ⚠️ 机检未过 {} 项：{}\n",
                            unmet.len(),
                            unmet.join("、")
                        ));
                    }
                }
            }
            other => {
                // 失败/取消/超预算也必须回报：父 agent 否则会一直以为子任务还在跑，
                // 按「等齐」的预期一直等下去。
                let reason = row
                    .error
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("：{}", clip(s, 200)))
                    .unwrap_or_default();
                items.push_str(&format!(
                    "{listed}. {objective} → {}{reason}\n",
                    match other {
                        SubagentStatus::Failed => "失败",
                        SubagentStatus::Cancelled => "已取消",
                        SubagentStatus::BudgetExceeded => "超预算",
                        _ => unreachable!("非终态已在上分支返回"),
                    }
                ));
                // 残留产出：失败/超预算 ≠ 产出为零。跑了几十分钟查到的东西对父
                // agent 判断「该不该带残留重试」有决定性价值——不摆出来，父 agent
                // 只能从零重烧一遍。
                if let Some(r) = row.result_json.as_deref().and_then(parse_result_json) {
                    if let Some(summary) = r
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        items.push_str(&format!(
                            "   残留产出：{}\n",
                            clip(summary, DIGEST_MAX_RESIDUAL_CHARS)
                        ));
                    }
                }
            }
        }
    }

    if listed == 0 && running == 0 {
        return None;
    }
    let mut text = String::from("[子任务回报] 以下是你此前派出的子任务结果，可直接据此继续：\n");
    if listed > 0 {
        text.push_str(&items);
        text.push_str("（以上已汇报，不必再用 check_subagent 重复查。）\n");
    }
    if running > 0 {
        text.push_str(&format!(
            "另有 {running} 个子任务仍在排队或执行中，完成后会在下一轮自动汇总；\
             现在可以先基于以上结果推进，不必空等。\n"
        ));
    }
    Some(SubagentDigest { text, absorb })
}

/// 解析收尾 JSON：解析失败不当成「没有结果」，而是当没有——宁可少报一条，
/// 也不要把半截 JSON 念进提示词（`finalize` 侧已保证 result_json 结构完整，
/// 这里只是不假设它一定完整）。
fn parse_result_json(raw: &str) -> Option<serde_json::Value> {
    serde_json::from_str(raw).ok()
}

/// 父会话 → 已经汇报过的子 agent id。
///
/// **进程内、不落库**：这是一次「父上下文是否已吸收某结果」的去重标记，不是
/// 业务事实——DB 里的终态才是事实。落库要多一列 + 迁移，换来的只是重启后
/// 不重复念一遍；而重启后重复念的后果是提示词里多几段旧结果（啰嗦，不是
/// 错误），重启一次也只发生一次。这个取舍是刻意的，重启后重复汇报的口子
/// 记在设计文档 §十。
static DIGESTED: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>,
> = std::sync::OnceLock::new();

fn digested(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, std::collections::HashSet<String>>>
{
    DIGESTED.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 锁中毒按 into_inner 取数据（全仓 mutex 口径）：去重表坏了的后果只是
/// 「可能多汇报一次」，不该让整个对话发不出请求。
fn lock_digested() -> std::sync::MutexGuard<
    'static,
    std::collections::HashMap<String, std::collections::HashSet<String>>,
> {
    crate::db::lock::lock_or_recover(digested(), "bot_orchestrator::DIGESTED")
}

/// 本次会话要注入的子任务回报段（没有新回报时 None）。
///
/// 只读自己的父会话行，查库失败/无会话一律 None——父上下文缺一段汇报照样能
/// 发请求，用户也还能用 `check_subagent` 手动查。
///
/// **不在这里记已吸收**：本函数只把「要注入什么」和「该记哪些 id」交出去，记
/// 动作由调用方在模型循环成功之后做（`mark_digested`）。在这里就记，等于把
/// 「这段回报已经说过了」提前宣布——而模型循环失败时它压根没送进模型，汇报轮
/// 也会因此以为已经说过、从此不再重提，子任务结果在本次进程内永久消失。
pub(crate) async fn subagent_digest_block(
    app: &AppHandle,
    session_id: Option<&str>,
) -> Option<SubagentDigest> {
    let sid = session_id?;
    let app2 = app.clone();
    let sid_owned = sid.to_string();
    let rows = tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app2).ok()?;
        crate::db::load_subagents_by_parent(&conn, &sid_owned, DIGEST_SCAN_LIMIT as u32).ok()
    })
    .await
    .ok()
    .flatten()?;

    let built = {
        let mut guard = lock_digested();
        let set = guard.entry(sid.to_string()).or_default();
        build_subagent_digest(&rows, set)?
    };
    crate::bot::audit_log(
        app,
        &format!(
            "subagent_digest | session={sid} absorbed={}",
            built.absorb.len()
        ),
    );
    Some(built)
}

/// 标记「父会话已吸收某子 agent 的结果」。
///
/// 父 agent 直接用 `check_subagent` 问到终态时同样算吸收：结果已经在父上下文
/// 里，digest 再念一遍只是同一结论塞两遍；不记的话，自动汇报轮还会以为
/// 「还没人说过」而把同一件事再讲一次。
pub(crate) fn mark_digested(session_id: Option<&str>, subagent_id: &str) {
    let Some(sid) = session_id else { return };
    lock_digested()
        .entry(sid.to_string())
        .or_default()
        .insert(subagent_id.to_string());
}

fn is_digested(session_id: &str, subagent_id: &str) -> bool {
    lock_digested()
        .get(session_id)
        .is_some_and(|s| s.contains(subagent_id))
}

/// 每次会话扫多少个子 agent 行（最新在前的 LIMIT）。
///
/// 与 `DIGEST_MAX_ITEMS`（一次渲染几条）不同：前者是「看多新」，后者是
/// 「说多少」。前者要够大，免得长期挂着的排队行把新完成的结果挤出候选。
const DIGEST_SCAN_LIMIT: usize = 30;

/// 机器可判定的验收谓词（相对子 agent 产物目录解析）：
/// - `file_exists:<相对路径>`：该文件存在
/// - `file_nonempty:<相对路径>`：该文件存在且非空
///
/// 为什么要有这一层：verifier 收尾里的 `met` 是**自述**，服务端无从核实——
/// 模型写 `met:true` 就显示通过。而「产物文件到底在不在」这类标准恰好能机械
/// 判定。不带前缀的条目维持原样由 verifier 自评（很多标准本来就得看内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MachineCheck {
    FileExists(String),
    FileNonEmpty(String),
}

/// 谓词前缀 → 判定种类
const MACHINE_PREFIXES: &[(&str, u8)] = &[("file_exists:", 0), ("file_nonempty:", 1)];

/// 相对路径闸：绝对路径、盘符、`..`/`.` 分量一律不解析——判定路径若能逃出
/// 产物目录，「验收」就成了任意文件存在性探测器。
pub(crate) fn safe_rel_path(raw: &str) -> Option<&str> {
    let p = raw.trim();
    if p.is_empty() || p.starts_with('/') || p.starts_with('\\') {
        return None;
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        return None; // C:\ / C:/ 盘符
    }
    if p.split(['/', '\\'])
        .any(|c| c == ".." || c == "." || c.is_empty())
    {
        return None;
    }
    Some(p)
}

/// 解析一条验收标准里的机器可判定谓词；None = 自由文本条目（交给 verifier 自评）
pub(crate) fn parse_machine_criterion(c: &str) -> Option<MachineCheck> {
    let t = c.trim();
    for (prefix, kind) in MACHINE_PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            let rel = safe_rel_path(rest)?;
            return Some(match kind {
                0 => MachineCheck::FileExists(rel.to_string()),
                _ => MachineCheck::FileNonEmpty(rel.to_string()),
            });
        }
    }
    None
}

/// 单条机器判定结论
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineVerdict {
    /// 原样保留已声明的 criteria 文本（回填/比对用）
    pub criterion: String,
    pub met: bool,
    pub reason: String,
}

/// 逐条执行机器可判定谓词（纯 FS 读，不写）。
/// 路径不合法（写了谓词但路径逃逸）判 false 并说明——fail-closed：
/// 宁可记未通过，也不能把一条不可核验的标准当通过。
pub(crate) fn eval_machine_criteria(
    criteria: &[String],
    dir: &std::path::Path,
) -> Vec<MachineVerdict> {
    let mut out = Vec::new();
    for c in criteria {
        let t = c.trim();
        let Some(check) = parse_machine_criterion(t) else {
            continue;
        };
        let (rel, want_nonempty) = match &check {
            MachineCheck::FileExists(r) => (r.as_str(), false),
            MachineCheck::FileNonEmpty(r) => (r.as_str(), true),
        };
        let path = dir.join(rel);
        let (met, reason) = match std::fs::metadata(&path) {
            Err(e) => (false, format!("产物不存在：{}", e.kind())),
            Ok(md) => {
                if md.is_dir() {
                    (false, "路径是目录，不是产物文件".to_string())
                } else if want_nonempty && md.len() == 0 {
                    (false, "产物存在但为空文件".to_string())
                } else {
                    (
                        true,
                        format!("服务端核验：{} 存在（{} 字节）", rel, md.len()),
                    )
                }
            }
        };
        out.push(MachineVerdict {
            criterion: t.to_string(),
            met,
            reason,
        });
    }
    out
}

/// 把机器判定并入收尾 JSON：以服务端结论覆盖 verifier 的自评，
/// 找不到对应条目就补一条（不依赖模型主动上报这些条目）。
pub(crate) fn merge_machine_verdicts(result: &mut serde_json::Value, verdicts: &[MachineVerdict]) {
    if verdicts.is_empty() {
        return;
    }
    let key = if result.get("checks").is_some() {
        "checks"
    } else {
        "acceptance_check"
    };
    let arr = result
        .as_object_mut()
        .map(|o| o.entry(key).or_insert_with(|| serde_json::json!([])));
    let Some(arr) = arr else { return };
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let arr = arr.as_array_mut().expect("上一步已归一成数组");
    for v in verdicts {
        let mut found = false;
        for item in arr.iter_mut() {
            let same = item
                .get("criterion")
                .and_then(|c| c.as_str())
                .map(|s| s.trim())
                == Some(v.criterion.as_str());
            if same {
                item["met"] = serde_json::Value::Bool(v.met);
                item["machine"] = serde_json::Value::Bool(true);
                item["machine_reason"] = serde_json::Value::String(v.reason.clone());
                found = true;
                break;
            }
        }
        if !found {
            arr.push(serde_json::json!({
                "criterion": v.criterion,
                "met": v.met,
                "machine": true,
                "machine_reason": v.reason,
            }));
        }
    }
}

/// 进度统计的交叉核对：只认**已声明过**的验收标准，且每条最多计一次。
/// 少了这层，模型报一条没派发过的 `met:true` 就能把 done 灌满
/// （min(total) 只是封顶，不挡虚报）。
fn count_met_against_declared(declared: &[String], reported: &[serde_json::Value]) -> usize {
    let mut hit: Vec<&str> = Vec::new();
    for item in reported {
        if item.get("met").and_then(|m| m.as_bool()) != Some(true) {
            continue;
        }
        let Some(c) = item
            .get("criterion")
            .and_then(|c| c.as_str())
            .map(str::trim)
        else {
            continue;
        };
        if declared.iter().any(|d| d.trim() == c) && !hit.contains(&c) {
            hit.push(c);
        }
    }
    hit.len()
}

/// 机器判定并入收尾产物（纯函数，单测锚点）：改写收尾 JSON 的验收结论，
/// 并在最终正文末尾点名未通过项——正文是模型与用户唯一同时看得到的地方，
/// 只写进 result_json 等于把结论藏起来。
pub(crate) fn apply_machine_verdicts(
    result: Option<serde_json::Value>,
    final_text: String,
    verdicts: &[MachineVerdict],
) -> (Option<serde_json::Value>, String) {
    if verdicts.is_empty() {
        return (result, final_text);
    }
    let mut result = result;
    if let Some(v) = result.as_mut() {
        merge_machine_verdicts(v, verdicts);
    }
    let failed: Vec<String> = verdicts
        .iter()
        .filter(|v| !v.met)
        .map(|v| format!("{}（{}）", v.criterion, v.reason))
        .collect();
    if failed.is_empty() {
        return (result, final_text);
    }
    let mut t = final_text;
    t.push_str(&format!(
        "\n\n⚠️ 服务端机器验收未通过 {} 项：{}",
        failed.len(),
        failed.join("；")
    ));
    (result, t)
}

/// 机器验收推翻「收尾 JSON 解析失败」造成的假失败（纯函数，单测锚点）。
///
/// 起因：终态在 `classify_outcome` 里只凭子 agent 最后一条消息的收尾 JSON 判，
/// 正文通篇散文就落进 `result_json_unparseable` → failed。可服务端手里明明有
/// 更硬的证据——产物落盘了、机器可判定标准逐条验过——却排在终态之后算，
/// 没有任何一步回头修正。于是「产物齐、验收全过、状态失败」能同时成立。
///
/// 只推翻 `result_json_unparseable` 一种：模型自报 failed（`model_reported_*`）
/// 说的可能是服务端验不了的自由文本条目，机器全过不等于它同意。
/// 也要求至少有一条机器可判定结论——一条都没有时服务端零证据，推翻就是放行。
/// 不带前缀的自由文本条目本就没有服务端结论，按读者视角处理，原始正文仍
/// 完整留在 `result_json.summary` 里。
///
/// 返回 (终态, error, 是否被推翻)：error 保留解析失败标记并补上推翻理由，
/// 让「判成功」这件事在行上与审计里都留得下痕迹，而不是被悄悄抹平。
pub(crate) fn reconcile_machine_override(
    status: SubagentStatus,
    error: Option<String>,
    verdicts: &[MachineVerdict],
) -> (SubagentStatus, Option<String>, bool) {
    let parse_failed =
        matches!(status, SubagentStatus::Failed) && error.as_deref() == Some(UNPARSEABLE_MARKER);
    if !parse_failed || verdicts.is_empty() || !verdicts.iter().all(|v| v.met) {
        return (status, error, false);
    }
    (
        SubagentStatus::Succeeded,
        Some(format!(
            "{UNPARSEABLE_MARKER}:机器验收 {} 项全通过，按产物判成功",
            verdicts.len()
        )),
        true,
    )
}

/// 收尾 JSON 解析失败的 error 标记：判定与推翻两侧共用同一个字面量，
/// 改名时不会只改一处导致推翻静默失效。
const UNPARSEABLE_MARKER: &str = "result_json_unparseable";

/// 产物目录（设计 §6 隔离）：gen_dir/subagents/{subagent_id}/。
pub(crate) fn artifact_dir_of(gen_root: &std::path::Path, subagent_id: &str) -> std::path::PathBuf {
    gen_root.join("subagents").join(subagent_id)
}

// 并发闸per-session 2 / global 3 FIFO）

/// 并发上限（设计 §6 已）：全局 3、每个派发主会话 2。超限排队不失败
///（status=queued），有槽位唤醒。
pub const MAX_GLOBAL_RUNNING: usize = 3;
pub const MAX_PER_SESSION_RUNNING: usize = 2;

fn gate() -> &'static SubagentGate {
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
    state: std::sync::Mutex<GateState>,
    notify: tokio::sync::Notify,
}

struct GateState {
    global_running: usize,
    /// running 计数：parent_session → 数
    per_session: HashMap<String, usize>,
    /// 等待计数：parent_session → 数（WaitingGuard Drop 递减，取消安全）
    waiting: HashMap<String, usize>,
}

/// 排队票据：Drop 释放槽位 + 递减等待计数（全部 RAII，无泄漏面）。
pub(crate) struct GateTicket {
    parent_session: Option<String>,
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

// runner（执行引擎）

/// 取消令牌的 RAII 反注册守卫。
///
/// `subagent_stops` 里每登一条就要负责抹掉：漏一条 = cancel_subagent 之后
/// 还能停一个早该消失的子 agent，那张表也只增不减。手工 remove 漏掉的是
/// panic 路径（runner panic 后整条收尾被跳过），故用 Drop 兜住。
struct StopTokenGuard {
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
fn panic_to_command_error(msg: &str) -> CommandError {
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
async fn run_subagent(
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
    // 排队时间不计墙钟：max_wall_seconds 的 timeout 从 run_model_loop 起算（下方），
    // 天然满足设计 §6。
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
    // 收尾窗标志：dispatch 预算闸置位后只放行写产物类工具——预算触顶不再等于
    // 「什么都不能做」，把已有发现写成产物文件才是触顶后该做的事。
    let wrap_up_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // 墙钟绝对期限：从**进入 running** 起算（排队时间不计，与现行口径一致）。
    // 闸在 tool_guard 里按绝对期限算剩余——每层各算一份会漂。
    let wall_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(budget.max_wall_seconds.max(1));
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
            wrap_up: wrap_up_flag.clone(),
            wall_deadline,
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

    // 墙钟预算（软墙钟 + 墙钟熔断留产）：与上面 ctx 里的期限同一个绝对时刻
    // ——两处各算一份，起点漂移就等于没有统一口径。
    let wall_budget = crate::bot_model_loop::WallBudget {
        deadline: wall_deadline,
        reserve: std::time::Duration::from_secs(crate::bot::params::subagent_wall_reserve_secs(
            budget.max_wall_seconds,
        )),
        max_wall_seconds: budget.max_wall_seconds,
    };

    // 工具循环：轮数预算 + 墙钟预算（超时硬停→budget_exceeded；排队不计——running 起算）
    //
    // 收尾 JSON 解析失败时**补答一次**（结构化输出三段修复的第 3 段）：产出
    // 完整、机检全过，只因最后一段不是 JSON 就判 failed，是完成率最大的单项
    // 黑洞。补答走同一份墙钟预算（重试不重置时钟），仍在 timeout 之内。
    let run = async {
        let first = crate::bot_model_loop::run_model_loop(
            app.clone(),
            msgs,
            budget.max_turns as usize,
            &stop,
            None,
            None,
            // 子 agent 模型走自身预算/派发通道（model_profile），不挂每卡覆盖
            None,
            // 配置档的采样温度（f32 → f64 无损）。这是 yaml `temperature:` 字段的
            // 唯一消费者；None 时用模型条目的 temperature。
            bound_onion
                .as_ref()
                .and_then(|c| c.temperature)
                .map(f64::from),
            // 子 agent 不落 exec_traces，无需失败明细回传槽（轮数另走 tool_guard 仪表）
            None,
            Some(wall_budget),
        )
        .await?;
        let (text, refs, trace) = first;
        if let Some(fixed) = repair_final_json_once(&app, &stop, &text, Some(wall_budget)).await {
            return Ok((fixed, refs, trace));
        }
        Ok((text, refs, trace))
    };
    // runner panic 隔离（bot_scheduler 的既有范式）：panic 会跳过下面所有收尾
    // ——取消令牌反注册、状态落库、subagent-finished 事件——前端那一行永远转圈，
    // 行状态也停在 running（只能靠重启清扫僵尸行）。故把 panic 捕获成普通失败，
    // 让收尾路径照常走完：行落 failed，事件发出去，前端不再转圈。
    let wall = match tokio::time::timeout(
        std::time::Duration::from_secs(budget.max_wall_seconds.max(1)),
        std::panic::AssertUnwindSafe(run).catch_unwind(),
    )
    .await
    {
        // 墙钟触达
        Err(elapsed) => Err(elapsed),
        // 正常跑完
        Ok(Ok(Ok(v))) => Ok(Ok(v)),
        // runner 自己返错
        Ok(Ok(Err(e))) => Ok(Err(e)),
        // runner panic → 归一成普通失败（与返错同一层，收尾路径不变）
        Ok(Err(panic)) => {
            let msg = crate::audit::panic_payload_msg(panic.as_ref());
            crate::bot::audit_log(
                &app,
                &format!(
                    "subagent_runner_panic | id: {} | {} | 已归一为执行失败",
                    row.id,
                    crate::bot::truncate_for_log(&msg, 200)
                ),
            );
            Ok(Err(panic_to_command_error(&msg)))
        }
    };

    // 墙钟触达先 stop 后收尾——run future 被 timeout 掐掉时，脱离 runtime
    // 的在飞 Python/MCP（spawn_blocking 不随 future 取消）靠 StopToken 自行退出；
    // 短 grace 给它们的退出清理/Drop 留时间，不留孤儿进程
    if wall.is_err() {
        stop.force_stop();
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }

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
    let (mut status, mut error, mut result) = match &wall {
        Err(_elapsed) => classify_outcome(None, true, None),
        Ok(Err(e)) => classify_outcome(Some(&e.message()), false, None),
        Ok(Ok((text, _refs, _loop_trace))) => classify_outcome(None, false, Some(text)),
    };
    // 工具预算触达 → 强制 budget_exceeded(max_tool_calls)——只针对「没交答卷」的。
    //
    // dispatch 预算闸到顶后进入收尾窗：读类被拒、写产物放行。模型若照「只许写」
    // 的指引交出合法收尾 JSON（含产物），那是真实完成，不得改判——改判会把
    // 父 agent 的「带残留重试」逻辑错误触发，半成品被当失败重烧。没交答卷的
    // （熔断标记路径已自带 budget_exceeded；这里管「停了工具但只输出散文」的
    // 剩余路径）才改判，真实原因（预算触顶）不能丢。
    let tool_calls_used = used_tool_calls.load(std::sync::atomic::Ordering::SeqCst);
    let wrapped_up_cleanly = matches!(&wall, Ok(Ok((text, _, _))) if parse_final_json(text).is_some());
    if budget_force_needed(tool_calls_used, budget.max_tool_calls, status, wrapped_up_cleanly) {
        status = SubagentStatus::BudgetExceeded;
        error = Some("max_tool_calls".into());
        // 部分结果不丢弃：熔断前跑出来的正文对父 agent 判断「该不该原目标
        // 重试、还是放宽预算」仍有价值
        if let Ok(Ok((text, _, _))) = &wall {
            result = Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "工具调用预算触达，未产出收尾 JSON；以上为触达前保留的最后一段正文"
            }));
        }
    }
    // 轮数熔断现在也走 Ok 出口（正文带 ROUNDS_FUSE_MARKER），机器验收必须把它
    // 排除掉：熔断行没有收尾 JSON，等于没交答卷，拿产物文件判它「未通过」只会
    // 在正文里凭空多出一段子 agent 没说过的话。墙钟熔断同理。
    let fused_on_rounds = matches!(&wall, Ok(Ok((text, _, _)))
        if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER));
    let fused_on_wall = matches!(&wall, Ok(Ok((text, _, _)))
        if text.contains(crate::bot_model_loop::WALL_FUSE_MARKER));
    let final_text: String = match &wall {
        Ok(Ok((text, _, _))) => text.clone(),
        Ok(Err(e)) => format!("⚠️ 执行失败：{}", e.message()),
        Err(_) => "⏹ 墙钟预算触达，执行被强制停止（部分产物已保留）".into(),
    };
    // 机器可判定验收：verifier 收尾里的 met 是自述，服务端对「产物文件在不在」
    // 这类标准能自己判——判了才覆盖自评，并在正文里点名未通过项。
    // 只在跑完（有收尾 JSON）时判：预算触顶/异常退出的行没有验收可言。
    let machine_verdicts = if matches!(wall, Ok(Ok(_))) && !fused_on_rounds && !fused_on_wall {
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

/// 汇报轮等空档的上限：与 `check_subagent` 的等待预算同档。主会话那一轮跑了
/// 十分钟还没结束，用户早该知道子任务结果了，再等更久没有意义。
const REPORT_WAIT_MS: u64 = CHECK_WAIT_MS_CAP;
/// 汇报轮轮询主会话是否空闲的间隔。
const REPORT_RETRY_MS: u64 = 2000;

/// 派发方会话是否还在（读失败按「不在」处理：宁可少报一次，也不写孤儿消息）
///
/// bot_messages 没有外键约束，会话删了之后往里插一条谁也读不到的消息就是
/// 纯死数据，且会话已删时汇报轮本就没有对象。
fn bot_session_exists_locked(conn: &rusqlite::Connection, session_id: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM bot_sessions WHERE id = ?1",
        [session_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

async fn bot_session_exists(app: &AppHandle, session_id: &str) -> bool {
    let sid = session_id.to_string();
    db_locked(app, move |conn| Ok(bot_session_exists_locked(conn, &sid)))
        .await
        .unwrap_or(false)
}

/// 子 agent 收尾后要不要给父会话起汇报轮。
///
/// 用户自己取消的不汇报：用户刚点过停止，不需要模型再解释一遍
/// （设计文档「用户取消不汇报」）。判据必须用**收尾时的真实终态**——
/// 收尾前那行的 status 恒为 Queued（行是派发时建的），拿行快照判取消
/// 永远判不成立，用户取消后照样起汇报轮。
fn should_report_to_parent(status: SubagentStatus) -> bool {
    status != SubagentStatus::Cancelled
}

/// 子 agent 收尾后让主 agent 主动向用户转述（fire-and-forget）。
///
/// 为什么要这条：主 agent 原本只在「下一轮组装消息时注入 digest」这一个时机
/// 看到子任务结果。可父会话那一轮往往已经结束了——实测里主 agent 用 4 次 30s
/// 等待都没等到，把「仍在调研中」当成整轮答复收尾，子 agent 49 秒后才跑完，
/// 此后用户没再说话，这个结果就永远躺在库里没人提。
///
/// 门控是「主会话空闲」而不是「子 agent 一收尾」：主会话还在跑的时候，它自己
/// 会再 check 一次拿到终态并当轮转述（那条路径已记进去重表），这时再起一轮
/// 就是同一件事说两遍。
fn spawn_parent_report(app: &AppHandle, row: &SubagentRow, status: SubagentStatus) {
    let Some(parent) = row.parent_session_id.clone() else {
        return;
    };
    // 用户自己取消的：用户刚点过停止，不需要模型再解释一遍
    if !should_report_to_parent(status) {
        return;
    }
    let app = app.clone();
    let sub_id = row.id.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(REPORT_WAIT_MS);
        loop {
            if is_digested(&parent, &sub_id) {
                return;
            }
            // 会话可能在这段等待里被用户删了：bot_messages 没有外键约束，
            // 硬跑一轮只会给一个不存在的会话写一条没人读得到的孤儿消息
            if !bot_session_exists(&app, &parent).await {
                crate::bot::audit_log(
                    &app,
                    &format!("subagent_report_skipped | id: {sub_id} | 派发方会话已不存在"),
                );
                return;
            }
            match crate::bot_chat::report_subagent_result(&app, &parent).await {
                crate::bot_chat::ReportOutcome::Ran => return,
                // 失败也重试：失败的那轮没把子任务结果说出去，直接放弃等于这条
                // 结果永远没人汇报（digest 也不会记成已吸收，下一轮还有机会）
                crate::bot_chat::ReportOutcome::Failed | crate::bot_chat::ReportOutcome::Busy => {}
            }
            if std::time::Instant::now() >= deadline {
                crate::bot::audit_log(
                    &app,
                    &format!(
                        "subagent_report_gave_up | id: {sub_id} | 父会话一直没空下来，或汇报一直失败"
                    ),
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(REPORT_RETRY_MS)).await;
        }
    });
}

/// 启动清扫：进程重启后没有任何 runner 在跑，库里 running/queued 的行是
/// 上一世的僵尸——不收尾就在子 agent 面板里永远转圈。逐行落终态 + 审计，
/// 谁被清的、可追溯；取消原因写进行上的 error，`check` 回传给父 agent 与面板。
///
/// 排队中的行**不恢复**，是刻意的：恢复需要连 stage（角色语义层）与 onion
/// （per-agent 配置）一起还原，而这两项没落在行上（行只有 profile / model）。
/// 只按 profile 重投会跑出一个「用默认角色和默认配置完成」的成功结果，比取消
/// 更难发现。真要恢复，先把 stage / onion 持久化，再谈重投。
///
/// 前置前提：应用是单实例（tauri-plugin-single-instance），所以本进程启动时
/// 不可能有另一个进程正跑着同一份库的子 agent——否则这里会误杀活跑的。
pub fn sweep_zombie_subagents(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let sweep = {
            let conn = match crate::db::open_db(&app) {
                Ok(c) => c,
                Err(e) => {
                    crate::bot::audit_log(
                        &app,
                        &format!("subagent_zombie_sweep_db_fail | err: {e}"),
                    );
                    return;
                }
            };
            let _g = crate::db::lock_db_write();
            let mut cleaned = 0usize;
            // 终态按「事实」选：排队中的从未开跑，是被重启丢弃（cancelled）；
            // 已开跑的中途断掉才算失败（failed）。状态机也只放行这两条边
            for (st, to) in [
                (SubagentStatus::Queued, SubagentStatus::Cancelled),
                (SubagentStatus::Running, SubagentStatus::Failed),
            ] {
                let rows = match crate::db::load_subagents_by_status(&conn, st) {
                    Ok(r) => r,
                    Err(e) => {
                        crate::bot::audit_log(
                            &app,
                            &format!(
                                "subagent_zombie_sweep_load_fail | status: {} | err: {e}",
                                st.as_str()
                            ),
                        );
                        continue;
                    }
                };
                for row in rows {
                    let id = row.id.clone();
                    let res = crate::db::update_subagent_status(
                        &conn,
                        &id,
                        to,
                        now_ms(),
                        Some("应用重启，执行现场已不存在（僵尸行清理）"),
                    );
                    match res {
                        Ok(_) => {
                            cleaned += 1;
                            crate::bot::audit_log(
                                &app,
                                &format!(
                                    "subagent_zombie_cleared | id: {id} | was: {}",
                                    st.as_str()
                                ),
                            );
                        }
                        Err(e) => crate::bot::audit_log(
                            &app,
                            &format!("subagent_zombie_clear_fail | id: {id} | err: {e}"),
                        ),
                    }
                }
            }
            cleaned
        };
        if sweep > 0 {
            crate::bot::audit_log(
                &app,
                &format!("subagent_zombie_sweep_done | count: {sweep}"),
            );
        }
    });
}

/// 上报用终态：行已写入终态时以行为准（cancel 抢先 → cancelled），
/// 收尾闭包自身失败拿不到行时才回落 classify 的观感值
fn effective_status(row: Option<SubagentStatus>, classified: SubagentStatus) -> SubagentStatus {
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

// final_text 由上面的 move 闭包直接捕获（历史落库后不再外用）

/// 子 agent 系统提示的产物目录规则段（gen_dir 硬约束，同 EXECUTE 提示口径）。
fn gen_dir_rule_for(artifact_dir: &std::path::Path) -> String {
    format!(
        "## 产物目录（硬性约束）\n你的全部文件写入必须落在：{}\n目录外写入会被拒绝；报告/数据文件写进该目录并在收尾 JSON 的 artifacts 里登记路径。",
        artifact_dir.display()
    )
}

// LLM 工具实现（registry 调用）

fn parse_tool_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or(serde_json::Value::Null)
}

/// spawn_subagent 工具（递归双保险②：子 agent 会话调用 → 服务端身份校验拒绝）。
pub async fn tool_spawn_subagent(
    app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if crate::tool_guard::is_subagent_session(session_id.as_deref()) {
        return ToolResult::error(
            "子 agent 不得派发子 agent（递归禁用）。请专注完成自己的任务包装目标。",
            Vec::new(),
        );
    }
    let v = parse_tool_args(args);
    let objective = v["objective"].as_str().unwrap_or_default().to_string();
    // stage 可选：recon/executor/verifier 三段式派发阶段（设计 §8.5）。
    // profile 可选：不给时按 配置档 → stage 派生（见 resolve_spawn_profile）。
    let stage = v["stage"].as_str().and_then(SubagentStage::from_str);
    // 配置档：名字非法 / 文件不在 / YAML 语法错 / 字段不合法都在这里报错。
    // 放在建行之前——加载失败的配置档若留到 runner 才炸，就是一条永远跑不起来的行。
    let onion_cfg = match v["onion"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => match crate::bot_onions::load(app, id) {
            Ok(cfg) => Some(cfg),
            Err(e) => return ToolResult::error(format!("spawn 失败：{e}"), Vec::new()),
        },
        None => None,
    };
    let profile = match resolve_spawn_profile(
        v["profile"].as_str().and_then(SubagentProfile::from_str),
        onion_cfg.as_ref().and_then(|c| c.profile_enum()),
        stage,
    ) {
        Some(p) => p,
        None => {
            return ToolResult::error(
                "spawn 失败：profile 必须是 research / coder / general 之一；或给 onion=配置档名；或给 stage=recon/executor/verifier 让系统按默认派生",
                Vec::new(),
            );
        }
    };
    let criteria: Vec<String> = v["acceptanceCriteria"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if criteria.is_empty() {
        return ToolResult::error(
            "spawn 失败：acceptanceCriteria 必填且每条可检验",
            Vec::new(),
        );
    }
    let budget = match &v["budget"] {
        serde_json::Value::Object(_) => Some(
            SubagentBudget {
                max_turns: v["budget"]["maxTurns"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_TURNS as u64) as u32,
                max_tool_calls: v["budget"]["maxToolCalls"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_TOOL_CALLS as u64)
                    as u32,
                max_wall_seconds: v["budget"]["maxWallSeconds"]
                    .as_u64()
                    .unwrap_or(DEFAULT_MAX_WALL_SECONDS),
            }
            .clamped(),
        ),
        _ => None,
    };
    let req = SpawnRequest {
        objective,
        profile,
        stage,
        acceptance_criteria: criteria,
        context_summary: v["contextSummary"].as_str().map(str::to_string),
        budget,
        model_profile: v["modelProfile"].as_str().map(str::to_string),
        onion: onion_cfg.as_ref().map(|c| SpawnOnion {
            config: c.onion_id.clone(),
            max_turn: c.max_turn,
        }),
        parent_task_id: v["parentTaskId"].as_str().map(str::to_string),
        parent_session_id: session_id,
    };
    match spawn_subagent(app, req).await {
        Ok(ack) => ToolResult::ok(
            spawn_receipt_text(&ack),
            // 无卡：不挂 TaskRef（指向已不存在的卡，前端会渲染成死链）
            Vec::new(),
        ),
        Err(e) => ToolResult::error(format!("spawn 失败：{}", e.message()), Vec::new()),
    }
}

/// spawn 的工具回执文案。命中幂等键时要说清「没新建、还是那个」——
/// 模型读到「已派发」会以为又多了一个子 agent，转述给用户时就会说错。
///
/// 回执里回显**生效**预算：调用方给的墙钟会被钳到 30..=3600，模型不知道它
/// 的子 agent 最多能跑多久，就会照着自己给的数安排后续动作。
fn spawn_receipt_text(ack: &SpawnAck) -> String {
    let budget_note = format!(
        "生效预算：墙钟 {}s / 轮数 {} / 工具调用 {}。",
        ack.budget.max_wall_seconds, ack.budget.max_turns, ack.budget.max_tool_calls
    );
    if ack.deduped {
        format!(
            "同一目标已有在跑的子 agent，未重复派发：subagentId={} status={}。{budget_note}继续用 check_subagent 轮询它。",
            ack.subagent_id,
            ack.status.as_str()
        )
    } else {
        format!(
            "已派发子 agent（非阻塞）：subagentId={} status={}。{budget_note}用 check_subagent 轮询结果，先继续回应用户。",
            ack.subagent_id,
            ack.status.as_str()
        )
    }
}

/// check_subagent 工具（幂等可轮询；waitMs 上限 600000，等待期可被 /stop 打断）。
pub async fn tool_check_subagent(
    app: &AppHandle,
    args: &str,
    stop: Option<crate::bot_slash::StopToken>,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let v = parse_tool_args(args);
    let key = v["subagentId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string);
    let Some(key) = key else {
        return ToolResult::error("check 失败：subagentId 与 taskId 至少提供一个", Vec::new());
    };
    let wait_ms = v["waitMs"].as_u64().unwrap_or(0).min(CHECK_WAIT_MS_CAP);
    match check_subagent(app, &key, wait_ms, stop.as_ref(), session_id.as_deref()).await {
        Ok(state) => ToolResult::ok(
            serde_json::to_string(&state).unwrap_or_else(|_| state.status.as_str().to_string()),
            Vec::new(),
        ),
        Err(e) => ToolResult::error(format!("check 失败：{}", e.message()), Vec::new()),
    }
}

/// cancel_subagent 工具（与任务卡停止按钮同 API；子 agent 会话禁调）。
pub async fn tool_cancel_subagent(
    app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    if crate::tool_guard::is_subagent_session(session_id.as_deref()) {
        return ToolResult::error("子 agent 不得取消其他子 agent。", Vec::new());
    }
    let v = parse_tool_args(args);
    let key = v["subagentId"]
        .as_str()
        .or_else(|| v["taskId"].as_str())
        .map(str::to_string);
    let Some(key) = key else {
        return ToolResult::error("cancel 失败：subagentId 与 taskId 至少提供一个", Vec::new());
    };
    let reason = v["reason"].as_str().unwrap_or("主 agent 取消").to_string();
    match cancel_subagent_async(app, &key, &reason).await {
        Ok(ack) => ToolResult::ok(
            if ack.already_terminal {
                format!(
                    "子 agent {} 已是终态（{}），无需取消",
                    ack.subagent_id,
                    ack.status.as_str()
                )
            } else {
                format!("已取消子 agent {}", ack.subagent_id)
            },
            Vec::new(),
        ),
        Err(e) => ToolResult::error(format!("cancel 失败：{}", e.message()), Vec::new()),
    }
}

/// write_artifact_file 工具：仅限子 agent 会话 + 产物目录内（设计 §6 产物目录隔离）。
pub async fn tool_write_artifact_file(
    _app: &AppHandle,
    args: &str,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("write_artifact_file 仅子 agent 执行会话可用。", Vec::new());
    };
    let v = parse_tool_args(args);
    let filename = v["filename"].as_str().unwrap_or_default().trim();
    let content = v["content"].as_str().unwrap_or_default();
    // content 上限（ 采纳）：失控子 agent 循环写大文件可填满磁盘——
    // 8MB 与 registry 长输出工具软上限同量级，报告/数据文件足够
    const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
    if content.len() > MAX_ARTIFACT_BYTES {
        return ToolResult::error(
            format!(
                "写产物失败：content {} 字节超上限 {}（8MB）",
                content.len(),
                MAX_ARTIFACT_BYTES
            ),
            Vec::new(),
        );
    }
    // 纯文件名校验：拒绝路径分隔符、NUL（POSIX 截断/Windows 路径组件）
    // 与 Windows 保留设备名；.. 按组件判定而非子串——
    // 不误伤 foo..bar / ...txt 这类合法名
    const WIN_RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem_upper = filename
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if filename.is_empty()
        || filename == "."
        || filename == ".."
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains('\0')
        || filename.chars().any(|c| c.is_control())
        || WIN_RESERVED.contains(&stem_upper.as_str())
    {
        return ToolResult::error(
            "写产物失败：filename 必须是纯文件名（不含路径分隔符、`.`/`..`、控制字符；Windows 保留设备名亦拒绝）",
            Vec::new(),
        );
    }
    // 产物目录 canonicalize（写路径钉死规范化目录，防 symlink 语义漂移）
    let mk_dir = ctx.artifact_dir.clone();
    let canonical = tauri::async_runtime::spawn_blocking(move || -> Result<PathBuf, String> {
        std::fs::create_dir_all(&mk_dir).map_err(|e| e.to_string())?;
        std::fs::canonicalize(&mk_dir).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let dir = match canonical {
        Ok(p) => p,
        Err(e) => {
            return ToolResult::error(format!("写产物失败：产物目录不可用（{e}）"), Vec::new());
        }
    };
    let filename = filename.to_string();
    let content_len = content.len();
    let content = content.to_string();
    // FS 操作不持 DB 锁；同步小文件写放 spawn_blocking 防阻塞执行器
    let write_res = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut target = dir.join(&filename);
        let stem = std::path::Path::new(&filename)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let ext = std::path::Path::new(&filename)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let mut n = 1;
        while target.exists() {
            if n >= 1000 {
                return Err("同名产物过多（≥1000），停止追加写".into());
            }
            target = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::write(&target, &content).map_err(|e| e.to_string())?;
        Ok(target.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string());
    match write_res {
        Ok(Ok(path)) => ToolResult::ok(
            format!(
                "已写入产物 {path}（{content_len} 字节）。收尾 JSON 的 artifacts 里登记该路径。"
            ),
            Vec::new(),
        ),
        Ok(Err(e)) => ToolResult::error(format!("写产物失败：{e}"), Vec::new()),
        Err(e) => ToolResult::error(format!("写产物失败：{e}"), Vec::new()),
    }
}

/// cancel_subagent tauri 命令任务卡停止按钮 / 子会话停止键的同 API 入口，
/// 设计 §4.3；薄壳转内部 cancel_subagent_async）。
#[tauri::command]
pub async fn cancel_subagent(
    app: AppHandle,
    key: String,
    reason: Option<String>,
) -> CommandResult<CancelAck> {
    cancel_subagent_async(&app, &key, reason.as_deref().unwrap_or("用户取消")).await
}

/// list_subagents 的行视图 = DB 行 + 内存态实时轮数。
///
/// 轮数只活在 tool_guard 注册表（runner 开跑注册、收尾注销），不落 DB：落库就得
/// 每轮写一次，与「轮数仪表共用同一个 Arc」的设计正好抵消。守卫注销后读不到就
/// 报 None——凭空造数比不报更坏（与 check_subagent 同口径）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentRunView {
    #[serde(flatten)]
    pub row: crate::db::SubagentRow,
    /// 已跑轮数；非在途（守卫已注销）为 None
    pub rounds_used: Option<u32>,
    /// 轮数硬顶；与轮数同源同报，10/20 与 10/200 的紧迫度完全不同
    pub rounds_budget: Option<u32>,
}

/// 列某主会话派出的子 agent（新→旧）。聊天内嵌子 agent 区域与右侧栏
/// 「子 Agent」工作面的数据源——子 agent 不再建独立会话后，用户没有别的地方
/// 能看见自己派出去的那些还活着/刚跑完。
#[tauri::command]
pub async fn list_subagents(
    app: AppHandle,
    session_id: String,
    limit: Option<u32>,
) -> CommandResult<Vec<SubagentRunView>> {
    let limit = limit.unwrap_or(20).clamp(1, 100);
    tauri::async_runtime::spawn_blocking(move || {
        let _g = crate::db::lock_db_write();
        let conn = crate::db::open_db(&app).map_err(CommandError::DbError)?;
        let rows = crate::db::load_subagents_by_parent(&conn, &session_id, limit)
            .map_err(CommandError::DbError)?;
        Ok(rows.into_iter().map(attach_rounds).collect())
    })
    .await
    .map_err(|e| CommandError::from(format!("subagent list 线程 join 失败：{e}")))?
}

/// 把内存态轮数挂到 DB 行上。在途行取 tool_guard 的计数器，终态行留空。
fn attach_rounds(row: crate::db::SubagentRow) -> SubagentRunView {
    let ctx = crate::tool_guard::subagent_ctx(row.session_id.as_deref());
    SubagentRunView {
        rounds_used: ctx
            .as_ref()
            .map(|c| c.used_rounds.load(std::sync::atomic::Ordering::Relaxed) as u32),
        rounds_budget: ctx.map(|c| c.budget.max_turns),
        row,
    }
}

/// read_own_card 工具：只读自己的执行契约。子 agent 不再建卡——契约在
/// subagents 行上（objective / acceptance_json / context_summary / budget_json），
/// 行不存在只可能是 runner 上下文串了会话，属真错误要响亮报出。
pub async fn tool_read_own_card(
    app: &AppHandle,
    session_id: Option<String>,
) -> crate::bot::registry::ToolResult {
    use crate::bot::registry::ToolResult;
    let Some(ctx) = crate::tool_guard::subagent_ctx(session_id.as_deref()) else {
        return ToolResult::error("read_own_card 仅子 agent 执行会话可用。", Vec::new());
    };
    let subagent_id = ctx.subagent_id.clone();
    let loaded = db_locked(app, move |conn| {
        crate::db::load_subagent(conn, &subagent_id).map_err(CommandError::DbError)
    })
    .await;
    let row = match loaded {
        Ok(Some(r)) => r,
        Ok(None) => {
            return ToolResult::error("读契约失败：会话对应的 subagent 不在库中", Vec::new())
        }
        Err(e) => return ToolResult::error(format!("读契约失败：{}", e.message()), Vec::new()),
    };
    let criteria: Vec<String> = row
        .acceptance_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let budget: Option<SubagentBudget> = row
        .budget_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let cancelled = row.status == SubagentStatus::Cancelled;
    ToolResult::ok(
        own_card_payload(
            &row,
            criteria,
            budget,
            ctx.used_tool_calls.load(std::sync::atomic::Ordering::SeqCst),
            ctx.used_rounds.load(std::sync::atomic::Ordering::SeqCst),
            ctx.wall_deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_secs(),
        )
        .to_string(),
        Vec::new(),
    )
}

/// read_own_card 返回体的纯构造：行上契约 + ctx 实时仪表合成单一 JSON。
/// 抽离是因为 db 路径在 MockRuntime 测试下不可直驱，而「budgetUsage 必含
/// 实时余量、静态 budget 键不再单独出现」是模型看得见的行为契约。
fn own_card_payload(
    row: &crate::db::subagents::SubagentRow,
    criteria: Vec<String>,
    budget: Option<SubagentBudget>,
    tool_calls_used: usize,
    rounds_used: usize,
    wall_remaining_secs: u64,
) -> serde_json::Value {
    let cancelled = row.status == SubagentStatus::Cancelled;
    serde_json::json!({
        "subagent_id": row.id,
        "objective": row.objective,
        "acceptanceCriteria": criteria,
        "contextSummary": row.context_summary,
        "budgetUsage": {
            "toolCallsUsed": tool_calls_used,
            "toolCallsMax": budget.as_ref().map(|b| b.max_tool_calls),
            "roundsUsed": rounds_used,
            "roundsMax": budget.as_ref().map(|b| b.max_turns),
            "wallRemainingSecs": wall_remaining_secs,
        },
        "status": row.status.as_str(),
        "hint": if cancelled {
            "派发已被取消：立即停止新探索，输出收尾 JSON（status=cancelled）"
        } else {
            "上面的验收标准与上下文摘要是本任务的全部契约；budgetUsage 是实时余量，逐条对照完成后收尾"
        }
    })
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;
    // 仅 parent_card 夹具用：子 agent 自己不再建卡，但派发仍可挂编排计划卡
    use crate::db::Task;

    /// runner panic 必须归一成普通失败，收尾路径照常走。
    ///
    /// panic 会跳过整条收尾（取消令牌反注册、状态落库、subagent-finished
    /// 事件）——前端那一行永远转圈，行状态停在 running。
    #[test]
    fn runner_panic_becomes_ordinary_failure() {
        let e = panic_to_command_error("index out of bounds");
        let msg = e.message();
        assert!(
            msg.contains("panic") && msg.contains("index out of bounds"),
            "panic 信息要如实带进失败原因：{msg}"
        );
        // 超长信息截断（审计日志与失败原因都不该被一段长 payload 撑爆）
        let long = "x".repeat(5000);
        let e2 = panic_to_command_error(&long);
        assert!(
            e2.message().chars().count() < 400,
            "panic 信息应截断到 200 量级：{}",
            e2.message().chars().count()
        );
    }

    /// 接线守卫：runner future 必须包 catch_unwind，且取消令牌登记要有
    /// RAII 反注册——两者都是「panic 路径不漏」的前提。
    #[test]
    fn runner_is_panic_isolated_and_stop_token_has_raii_cleanup() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bot_orchestrator.rs"
        ))
        .expect("应能读到自己");
        let start = src
            .find("async fn run_subagent(")
            .expect("应能找到 run_subagent");
        let rest = &src[start..];
        // 函数体到顶格 "}" 为止（嵌套块的 "}" 都带缩进）
        let end = rest[1..].find("\n}\n").map(|i| i + 1).unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(
            body.contains("catch_unwind"),
            "runner future 必须包 catch_unwind（panic 要归一成普通失败走收尾）：{body}"
        );
        assert!(
            body.contains("StopTokenGuard"),
            "取消令牌登记必须配 RAII 反注册（panic 路径不能把条目留在 subagent_stops）：{body}"
        );
    }

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // tasks 表内联最小 schema（列序与 open_db 一致，含  新增三列）；
        // subagents 表与生产同源 DDL（单源真相）
        conn.execute_batch(crate::db::tasks::TASKS_DDL).unwrap();
        conn.execute_batch(crate::db::SUBAGENTS_DDL).unwrap();
        conn
    }

    fn parent_card(conn: &rusqlite::Connection) -> String {
        let card = Task {
            plan_start: None,
            plan_end: None,
            parent_id: None,
            id: "parent-1".into(),
            title: "主编排卡".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            column: crate::db::TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: Some(false),
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: Some(1_000),
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            depends_on: None,
            model: None,
            owner_id: None,
            created_at: None,
            expected_updated_at: None,
        };
        crate::db::upsert_tasks(conn, std::slice::from_ref(&card)).unwrap();
        "parent-1".into()
    }

    // check 等待策略：阶梯递进 + 终态事件唤醒 + /stop 打断

    fn fake_state(status: SubagentStatus) -> CheckState {
        CheckState {
            subagent_id: "sa_wait_test".into(),
            task_id: None,
            session_id: None,
            status,
            progress: serde_json::json!({ "subtasksDone": 0, "subtasksTotal": 1 }),
            result: None,
            error: None,
        }
    }

    #[test]
    fn poll_step_ladder_is_monotonic_and_capped() {
        // 起手就稀疏（终态走事件唤醒，阶梯只兜漏通知）：每次读都要开一次库
        // 跑全量 DDL，越密只是越频繁地开库
        let steps: Vec<u64> = (0..12).map(next_poll_step).collect();
        assert_eq!(steps[0], 1_500, "首档 1500ms");
        assert_eq!(steps[9], 600_000, "阶梯末档 10 分钟");
        assert_eq!(steps[11], 600_000, "出阶梯后封顶不增长");
        for w in steps.windows(2) {
            assert!(w[0] <= w[1], "步长不递减：{steps:?}");
        }
        // 上限与末档同档：预算给满时兜底阶梯不会越过预算本身
        assert_eq!(
            next_poll_step(CHECK_POLL_LADDER_MS.len() - 1),
            CHECK_WAIT_MS_CAP
        );
    }

    #[tokio::test]
    async fn wait_returns_at_once_when_already_terminal() {
        let (state, exit) = wait_until_terminal(
            || async { Ok::<_, CommandError>(fake_state(SubagentStatus::Succeeded)) },
            CHECK_WAIT_MS_CAP,
            None,
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Terminal);
        assert_eq!(state.status, SubagentStatus::Succeeded);
    }

    #[tokio::test]
    async fn wait_wakes_on_finish_event_instead_of_burning_budget() {
        // 终态事件必须立刻唤醒等待方：预算给满 30s，若退化成纯兜底轮询，
        // 这个用例会白等一个封顶步长（3s）才返回。
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done2 = done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            done2.store(true, std::sync::atomic::Ordering::SeqCst);
            finish_notify().notify_waiters();
        });
        let started = std::time::Instant::now();
        let (state, exit) = wait_until_terminal(
            || async {
                let terminal = done.load(std::sync::atomic::Ordering::SeqCst);
                Ok::<_, CommandError>(fake_state(if terminal {
                    SubagentStatus::Succeeded
                } else {
                    SubagentStatus::Running
                }))
            },
            CHECK_WAIT_MS_CAP,
            None,
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Terminal);
        assert_eq!(state.status, SubagentStatus::Succeeded);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "没被终态事件唤醒，等了 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn wait_returns_early_when_user_stops() {
        // 预算上限已到 30s，而 /stop 只在轮次边界检查——等待循环不自己查令牌，
        // 停止键就会锁死整个等待时长。
        let stop = crate::bot_slash::StopToken::new_for_test();
        stop.stop();
        let started = std::time::Instant::now();
        let (_, exit) = wait_until_terminal(
            || async { Ok::<_, CommandError>(fake_state(SubagentStatus::Running)) },
            CHECK_WAIT_MS_CAP,
            Some(&stop),
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Stopped);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "/stop 没能打断等待，等了 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn long_budget_does_not_reopen_db_on_every_step() {
        // 阶梯的意义就在这：每次读都要开一次库跑全量 DDL。旧的固定 150ms
        // 步长下 1.5s 预算要读 11 次，递进后只读 2 次（起手就 1500ms）。
        //
        // 预算取 1500ms（正好一个首档）而不是更大：阶梯末档已是分钟级，
        // 预算再大这个用例就得跑好几分钟，而 2 次 vs 11 次已足够判别。
        // 合并了原先「超时也带回当前状态」那条：断的仍是同一行预算判定。
        let reads = std::cell::Cell::new(0usize);
        let started = std::time::Instant::now();
        let (state, exit) = wait_until_terminal(
            || {
                reads.set(reads.get() + 1);
                async { Ok::<_, CommandError>(fake_state(SubagentStatus::Running)) }
            },
            1_500,
            None,
        )
        .await
        .unwrap();
        assert_eq!(exit, WaitExit::Timeout);
        assert_eq!(
            state.status,
            SubagentStatus::Running,
            "超时也要带回当前状态"
        );
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(1_500),
            "预算没走满就返回了：{:?}",
            started.elapsed()
        );
        // 上界留 3 而不是 2：第二次读若因计时抖动落在 deadline 前几百毫秒，会
        // 多走一档 3000ms——那是无害的，判别力仍远强于固定步长的 11 次。
        // finish_notify 是进程级信号，别的子 agent 收尾也会唤醒本等待方。
        assert!(
            reads.get() <= 3,
            "1.5s 预算读了 {} 次，阶梯没生效",
            reads.get()
        );
    }

    // 并发闸（FIFO/上限/释放）

    /// 并发闸状态机（合并为单测：static GATE 进程级共享，cargo 并行跑两个测试
    /// 会互相踩计数；手动构造票据，不调 wait_slot——它会与其他并行测试竞争
    /// 全局槽导致排队挂死）。
    #[test]
    fn gate_state_machine_acquire_release_and_queue_position() {
        let before_running = gate().state.lock().unwrap().global_running;
        // ① 拿槽 → 计数 +1、per_session 登记、未排队
        {
            let t = Some(GateTicket {
                parent_session: Some("ps_drop".into()),
            });
            // 手动占槽（wait_slot 锁内路径的等价操作）
            {
                let mut st = gate().state.lock().unwrap();
                st.global_running += 1;
                *st.per_session.entry("ps_drop".into()).or_insert(0) += 1;
            }
            {
                let st = gate().state.lock().unwrap();
                assert_eq!(st.global_running, before_running + 1, "拿槽 +1");
                assert_eq!(st.per_session.get("ps_drop"), Some(&1));
            }
            // queue_position 也拿 gate 锁（非递归 mutex），必须在持锁块外调用，
            // 否则同线程二次加锁自死锁
            assert_eq!(SubagentGate::queue_position(Some("ps_drop")), None);
            // ② Drop → 释放：计数回基线、归零清键
            drop(t);
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running, "Drop 必须释放槽位");
            assert!(!st.per_session.contains_key("ps_drop"), "计数归零须清键");
        }
        // ③ 多持多放对称：两张票据只放一张 → 残 1；全放 → 清键
        {
            let t1 = Some(GateTicket {
                parent_session: Some("ps_rel".into()),
            });
            let t2 = GateTicket {
                parent_session: Some("ps_rel".into()),
            };
            {
                let mut st = gate().state.lock().unwrap();
                st.global_running += 2;
                *st.per_session.entry("ps_rel".into()).or_insert(0) += 2;
            }
            drop(t2);
            assert_eq!(
                gate().state.lock().unwrap().per_session.get("ps_rel"),
                Some(&1),
                "放一张残 1"
            );
            drop(t1);
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running, "全放回基线");
            assert!(!st.per_session.contains_key("ps_rel"), "归零清键");
        }
        // ④ queue_position：同会话等待计数语义——前面还有 waiting-1 个
        {
            let mut st = gate().state.lock().unwrap();
            *st.waiting.entry("ps_q1".into()).or_insert(0) += 2;
            *st.waiting.entry("ps_q2".into()).or_insert(0) += 1;
        }
        assert_eq!(SubagentGate::queue_position(Some("ps_q1")), Some(1));
        assert_eq!(SubagentGate::queue_position(Some("ps_q2")), Some(0));
        assert_eq!(SubagentGate::queue_position(Some("ps_not_waiting")), None);
        {
            let mut st = gate().state.lock().unwrap();
            st.waiting.clear();
        }
        assert_eq!(SubagentGate::queue_position(Some("ps_q1")), None, "清场");
    }

    /// U20D 批 5：子 agent 会话的身份由 bot_sessions.is_subagent 结构化字段
    /// 承载，路由契约从「标题 🧩 前缀」升级为该字段。此处锁定：
    /// ① runner setup 必须经 subagent 变体建会话（漂移即测试红）；
    /// ② 标题已去 emoji 前缀（路由不再依赖标题）。
    ///
    /// 这一列用于「会话列表不列子 agent 会话」与库内判定；活会话的判定走
    /// tool_guard 注册表，前端拿到的 Session.isSubagent 恒为 false。
    #[test]
    fn subagent_sessions_flagged_structurally_not_by_title() {
        // concat! 拼接防本测试自匹配（dead_command_tests 同款手法）
        let wiring = concat!(
            "bot_session_create_",
            "subagent_inner(&tx, Some(title.clone()))"
        );
        assert!(
            include_str!("bot_orchestrator.rs").contains(wiring),
            "runner 建会话必须走 subagent 变体（is_subagent=1 结构化标记）"
        );
        assert!(
            !include_str!("bot_orchestrator.rs").contains(concat!("🧩 子", "任务：")),
            "标题已去 emoji 前缀（路由不依赖标题；子 agent 身份由 is_subagent 字段承载）"
        );
    }

    // ── 父侧装配（设计 §3.7 L2 落点）──

    fn digest_row(id: &str, status: SubagentStatus, result: Option<&str>) -> SubagentRow {
        SubagentRow {
            id: id.into(),
            status,
            profile: SubagentProfile::Coder,
            model: None,
            parent_session_id: Some("main-sess".into()),
            session_id: Some(format!("sub-{id}")),
            task_id: None,
            objective: format!("目标-{id}"),
            trace_id: format!("trc-{id}"),
            acceptance_json: None,
            context_summary: None,
            budget_json: None,
            result_json: result.map(str::to_string),
            error: (status == SubagentStatus::Failed).then(|| "LLM 5xx".to_string()),
            created_at: 1_000,
            started_at: Some(1_001),
            finished_at: Some(1_002),
        }
    }

    const OK_RESULT: &str = r#"{"status":"succeeded","summary":"覆盖 5 个竞品，含官网 URL","artifacts":[{"path":"report.md","description":"竞品对比"},{"path":"data.csv"}]}"#;

    #[test]
    fn digest_reports_terminal_rows_and_only_absorbs_those() {
        let rows = vec![
            digest_row("sa_ok", SubagentStatus::Succeeded, Some(OK_RESULT)),
            digest_row("sa_running", SubagentStatus::Running, None),
            digest_row("sa_fail", SubagentStatus::Failed, None),
        ];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new())
            .expect("有未汇报的子任务就该出段");
        assert!(d.text.contains("覆盖 5 个竞品"), "摘要要进段：{}", d.text);
        assert!(
            d.text.contains("report.md（竞品对比）"),
            "产物路径与说明都要进段：{}",
            d.text
        );
        assert!(
            d.text.contains("→ 失败：LLM 5xx"),
            "失败必须回报（否则父 agent 会一直等）：{}",
            d.text
        );
        assert!(
            d.text.contains("1 个子任务仍在排队或执行中"),
            "在途要报计数：{}",
            d.text
        );
        assert!(
            !d.text.contains("目标-sa_running"),
            "在途不逐条铺开：{}",
            d.text
        );
        // 只有终态被吸收：在途行下轮还要看它完没完
        assert_eq!(d.absorb, vec!["sa_ok".to_string(), "sa_fail".to_string()]);
    }

    #[test]
    fn digest_is_idempotent_across_turns() {
        let rows = vec![digest_row(
            "sa_ok",
            SubagentStatus::Succeeded,
            Some(OK_RESULT),
        )];
        let first = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        let mut set = std::collections::HashSet::new();
        for id in &first.absorb {
            set.insert(id.clone());
        }
        assert!(
            build_subagent_digest(&rows, &set).is_none(),
            "同一份结果第二轮不得再念一遍"
        );
        assert!(
            build_subagent_digest(&[], &set).is_none(),
            "没有子任务时不该出段"
        );
    }

    #[test]
    fn digest_never_injects_child_conversation_history() {
        // §3.7 要点 3：父 payload 只注 result_json，不注子会话历史。
        // 段里出现「system」「assistant」这类角色标记或子会话 id 即为破线。
        let rows = vec![digest_row(
            "sa_ok",
            SubagentStatus::Succeeded,
            Some(OK_RESULT),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert!(
            !d.text.contains("sub-sa_ok"),
            "子会话 id 不该出现：{}",
            d.text
        );
        assert!(
            !d.text.contains("\"role\""),
            "不得把子会话消息行当 payload 注入：{}",
            d.text
        );
    }

    #[test]
    fn digest_caps_items_and_summary_length() {
        // 一次注入不能把本轮对话挤掉：条数与摘要长度都要封顶。
        let long = "长".repeat(DIGEST_MAX_SUMMARY_CHARS * 3);
        let result = format!(
            r#"{{"summary":"{long}","artifacts":[{{"path":"a.md"}},{{"path":"b.md"}},{{"path":"c.md"}},{{"path":"d.md"}},{{"path":"e.md"}},{{"path":"f.md"}}]}}"#
        );
        let rows: Vec<SubagentRow> = (0..DIGEST_MAX_ITEMS + 5)
            .map(|i| {
                digest_row(
                    &format!("sa_{i}"),
                    SubagentStatus::Succeeded,
                    Some(result.as_str()),
                )
            })
            .collect();
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert_eq!(
            d.absorb.len(),
            DIGEST_MAX_ITEMS,
            "超出上限的行不注入（也不吸收，否则它永远进不了下一轮）"
        );
        let summary_line = d
            .text
            .lines()
            .find(|l| l.trim_start().starts_with("摘要："))
            .expect("有摘要行");
        assert!(
            summary_line.chars().count() <= DIGEST_MAX_SUMMARY_CHARS + 8,
            "摘要必须被截断：{} 字",
            summary_line.chars().count()
        );
        let artifacts_line = d
            .text
            .lines()
            .find(|l| l.trim_start().starts_with("产物："))
            .expect("有产物行");
        assert!(
            !artifacts_line.contains("f.md"),
            "产物条数封顶：{artifacts_line}"
        );
    }

    /// 失败/超预算行必须摆出**残留产出**。
    ///
    /// 父 agent 此前只看到「失败」二字，判断不了「该不该带残留重试」——
    /// 于是从零重烧一遍。跑了几十分钟查到的东西就在 result_json.summary 里，
    /// 不摆出来等于没有。
    #[test]
    fn digest_shows_residual_output_for_failed_rows() {
        let rows = vec![digest_row(
            "sa_failed",
            SubagentStatus::BudgetExceeded,
            Some(r#"{"summary":"已核到 3 家供应商报价单，还差比价","note":"墙钟熔断"}"#),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert!(
            d.text.contains("残留产出："),
            "失败行必须带残留产出：{}",
            d.text
        );
        assert!(
            d.text.contains("3 家供应商"),
            "残留产出的内容要能看到：{}",
            d.text
        );
    }

    /// 成功行的**机检未过项**必须摆出来。
    ///
    /// 「成功」与「机检没过」两件事同时成立时，父 agent 只看到「成功」就会把
    /// 没过的项一起收下。只认服务端核验过的（machine=true 且 met=false）——
    /// 模型自报的 met:false 是自我评价，混进来会让父 agent 分不清。
    #[test]
    fn digest_shows_machine_unmet_criteria() {
        let result = r#"{"status":"succeeded","acceptance_check":[
            {"criterion":"file_exists:调研.md","met":true,"machine":true},
            {"criterion":"file_nonempty:数据.xlsx","met":false,"machine":true},
            {"criterion":"内容符合用户口径","met":false}
        ]}"#;
        let rows = vec![digest_row(
            "sa_machine",
            SubagentStatus::Succeeded,
            Some(result),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
        assert!(
            d.text.contains("机检未过"),
            "成功行必须带机检未过项：{}",
            d.text
        );
        assert!(
            d.text.contains("file_nonempty:数据.xlsx"),
            "未过项要指名道姓：{}",
            d.text
        );
        assert!(
            !d.text.contains("内容符合用户口径"),
            "模型自报的未过项不摆（服务端没核验过）：{}",
            d.text
        );
    }

    #[test]
    fn digest_survives_malformed_result_json() {
        // 收尾 JSON 解析失败当「没有结果」：宁可少报一条，也不把半截 JSON
        // 念进提示词。行仍要被吸收——否则它每轮都重来一次。
        let rows = vec![digest_row(
            "sa_bad",
            SubagentStatus::Succeeded,
            Some("{半截"),
        )];
        let d = build_subagent_digest(&rows, &std::collections::HashSet::new())
            .expect("行本身仍要出段");
        assert!(!d.text.contains("半截"), "坏 JSON 不得进段：{}", d.text);
        assert!(d.text.contains("→ 成功"));
        assert_eq!(d.absorb, vec!["sa_bad".to_string()]);
    }

    #[test]
    fn malformed_result_json_is_not_wrapped_and_carried_on() {
        // 直接钉函数契约：解析失败返回 None。
        // 只在渲染输出上断言「文本里没有坏 JSON」是不够的——把失败包成
        // `{"raw": ...}` 继续往下走的写法同样能让输出里搜不到坏串，
        // 那样原始文本就还在数据流里（且总有一天会换个字段被渲染出来）。
        assert!(
            parse_result_json("{半截").is_none(),
            "解析失败必须返回 None，不得包装后继续"
        );
        assert!(
            parse_result_json(OK_RESULT).is_some(),
            "合法收尾 JSON 要能解析出来"
        );
        assert!(
            parse_result_json("[]").is_some(),
            "合法但结构不是对象也算解析成功（取字段时自然落空，不该当语法错）"
        );
    }

    /// 接线守卫：回报段的注入点必须在消息组装的**末尾**（历史之后）。
    ///
    /// 放在摘要/记忆块那段之前，模型看到的是「先回报后用户问题」，权重结构
    /// 反了；放在更早还会被后续逻辑改写。匹配串 `concat!` 拆分防自匹配。
    #[test]
    fn digest_is_injected_at_the_end_of_message_assembly() {
        let chat = include_str!("bot_chat.rs");
        let at = chat
            .find(concat!(
                "crate::bot_orchestrator::",
                "subagent_digest_block(&app, session_id.as_deref())"
            ))
            .expect("组装末尾必须调 subagent_digest_block");
        let history_end = chat
            .find("msgs.push(serde_json::json!({\"role\": role, \"content\": m.content}))")
            .expect("历史消息循环的收尾 push");
        // 模型循环锚点必须在注入点**之后**找：bot_chat.rs 前面还有若干处提到
        // run_model_loop 的注释/别名，全文件搜第一个会命中它们。
        let runs_loop = chat[at..]
            .find("run_model_loop(")
            .map(|i| i + at)
            .expect("注入点之后应有模型循环调用");
        assert!(
            history_end < at && at < runs_loop,
            "回报段必须夹在历史消息与模型循环之间（现 history_end={history_end} \
             digest={at} loop={runs_loop}）"
        );
        assert!(
            chat[at..].contains("\"role\": \"system\""),
            "回报段作为 system 消息追加"
        );
    }

    /// `temperature` 是 yaml 里长期没有消费者的字段（已解析、已落盘，就是没人用）。
    /// 这条把它钉成「真的进了模型循环」：优先级是配置档 > 模型条目。
    #[test]
    fn onion_temperature_reaches_the_model_loop() {
        let body = include_str!("bot_orchestrator.rs");
        let runner = body
            .split_once("let first = crate::bot_model_loop::run_model_loop(")
            .expect("子 agent 调模型循环")
            .1
            .split(";")
            .next()
            .expect("调用应在此处收尾");
        // 两个 token 分开断言而不是拼成 `bound_onion.as_ref()`：rustfmt 会按行宽
        // 把链式调用拆行，拼出来的串匹配不到——守卫被格式器牵着走。
        assert!(
            runner.contains("bound_onion"),
            "子 agent 必须把配置档的温度传进模型循环：{runner}"
        );
        assert!(
            runner.contains("c.temperature"),
            "传的是配置档的 temperature 字段：{runner}"
        );
    }

    /// tool_calls 预算计数器（ctx 侧）——dispatch 累计语义的单测锚点
    #[test]
    fn tool_call_counter_accumulates() {
        let ctx = crate::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_c1".into(),
            task_id: None,
            profile: SubagentProfile::Coder,
            tools: std::sync::Arc::new(
                crate::bot::registry::profile_whitelist(SubagentProfile::Coder).to_vec(),
            ),
            budget: SubagentBudget {
                max_turns: 5,
                max_tool_calls: 2,
                max_wall_seconds: 60,
            },
            artifact_dir: std::path::PathBuf::from("/tmp/art_c1"),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
        };
        assert_eq!(
            ctx.used_tool_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            ctx.used_tool_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            1
        );
        // used(2) >= max(2) → dispatch 层下一次调用将拒绝
        assert!(2 >= ctx.budget.max_tool_calls as usize);
    }

    // 收尾解析 / 包装渲染

    #[test]
    fn extract_last_json_takes_the_last_object() {
        // 纯 JSON
        assert_eq!(
            json_object_spans(r#"{"a":1}"#),
            vec![r#"{"a":1}"#.to_string()],
            "纯 JSON 应切出一个顶层对象"
        );
        // 散文夹 JSON + 多对象 → 取最后一个
        let t = r#"结论如下 {"status":"succeeded","summary":"第一步"} 中间 {"status":"succeeded","summary":"最终"}"#;
        let got = parse_final_json(t).expect("多对象时取最后一个");
        assert_eq!(got["summary"], "最终");
        // 字符串内花括号不破坏扫描
        let t2 = r#"{"summary":"带 } 花括号与 \" 引号"}"#;
        assert!(parse_final_json(t2).is_some());
        // 无 JSON
        assert!(parse_final_json("没有任何大括号的普通回复").is_none());
    }

    #[test]
    fn classify_outcome_maps_all_paths() {
        use SubagentStatus::*;
        // 墙钟 → budget_exceeded
        let (st, err, _) = classify_outcome(None, true, None);
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_wall_seconds"));
        // 轮数熔断 → budget_exceeded（标记在正文里，出口见 bot_model_loop 熔断）
        let fused = crate::bot_model_loop::rounds_fuse_message("上轮正文", "", 20);
        let (st, err, _) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_turns"));
        // 其他错误 → failed
        let (st, _, _) = classify_outcome(Some("HTTP 500"), false, None);
        assert_eq!(st, Failed);
        // 解析成功 + 模型自报 succeeded
        let (st, err, res) = classify_outcome(
            None,
            false,
            Some(r#"结论 {"status":"succeeded","summary":"ok"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        assert_eq!(res.unwrap()["summary"], "ok");
        // 模型自报 failed → failed + 说明
        let (st, err, _) =
            classify_outcome(None, false, Some(r#"{"status":"failed","summary":"x"}"#));
        assert_eq!(st, Failed);
        assert_eq!(err.as_deref(), Some("model_reported_failure"));
        // 解析失败 → failed + result_json_unparseable + 原文存 summary（部分结果不丢）
        let (st, err, res) = classify_outcome(None, false, Some("纯文本收尾，没有 JSON"));
        assert_eq!(st, Failed);
        assert_eq!(err.as_deref(), Some("result_json_unparseable"));
        let res = res.unwrap();
        assert!(res["summary"].as_str().unwrap().contains("纯文本收尾"));
    }

    /// 触顶改判的四象限：交了卷的触顶是真实完成（收尾窗放行过写产物），
    /// 只有「触顶 + 没交卷」才强制 budget_exceeded。
    #[test]
    fn budget_force_needed_only_when_not_wrapped_up() {
        use SubagentStatus::*;
        // 触顶 + 交卷（succeeded / 自报 failed 都是交卷）→ 不改判
        assert!(!budget_force_needed(100, 100, Succeeded, true));
        assert!(!budget_force_needed(100, 100, Failed, true));
        // 触顶 + 没交卷 → 改判（真实原因是预算）
        assert!(budget_force_needed(100, 100, Succeeded, false));
        assert!(budget_force_needed(100, 100, Failed, false));
        // 未触顶 / 已是 budget_exceeded（熔断标记路径）→ 不改判
        assert!(!budget_force_needed(99, 100, Failed, false));
        assert!(!budget_force_needed(100, 100, BudgetExceeded, false));
    }

    /// read_own_card 的返回体：budgetUsage 一块含全部实时余量，静态 budget
    /// 键不再单独出现（两份会让模型不知道看哪个）。
    #[test]
    fn own_card_payload_reports_live_budget_usage() {
        let row = crate::db::subagents::SubagentRow {
            id: "sa_x".into(),
            status: SubagentStatus::Running,
            profile: SubagentProfile::Research,
            model: None,
            parent_session_id: None,
            session_id: None,
            task_id: None,
            objective: "调研 X".into(),
            trace_id: "trc_x".into(),
            acceptance_json: Some(r#"["覆盖 X"]"#.into()),
            context_summary: None,
            budget_json: None,
            result_json: None,
            error: None,
            created_at: 0,
            started_at: None,
            finished_at: None,
        };
        let v = own_card_payload(
            &row,
            vec!["覆盖 X".into()],
            Some(SubagentBudget {
                max_turns: 50,
                max_tool_calls: 100,
                max_wall_seconds: 3600,
            }),
            47,
            12,
            1800,
        );
        assert_eq!(v["budgetUsage"]["toolCallsUsed"], 47);
        assert_eq!(v["budgetUsage"]["toolCallsMax"], 100);
        assert_eq!(v["budgetUsage"]["roundsUsed"], 12);
        assert_eq!(v["budgetUsage"]["roundsMax"], 50);
        assert_eq!(v["budgetUsage"]["wallRemainingSecs"], 1800);
        assert!(
            v.get("budget").is_none(),
            "静态 budget 键已并入 budgetUsage，不得再单独出现"
        );
        // 预算缺失（老行）时 max 为 null，used 仍要报——余量读数不依赖预算存在
        let v2 = own_card_payload(&row, vec![], None, 3, 1, 60);
        assert_eq!(v2["budgetUsage"]["toolCallsUsed"], 3);
        assert!(v2["budgetUsage"]["toolCallsMax"].is_null());
    }

    /// 轮数熔断必须保住已跑完的那十几轮产出。
    ///
    /// 回归背景：recon 子任务 20 轮用尽、28 次工具调用查完后熔断，终态
    /// budget_exceeded/max_turns 而 result_json 是 NULL——产出整段蒸发，父 agent
    /// 回灌拿不到 summary，重试只能从零再来（设计 §6「部分结果不丢弃」）。
    #[test]
    fn classify_outcome_keeps_partial_output_on_rounds_fuse() {
        use SubagentStatus::*;
        let partial = "查到 pp-ocr-v6/ 下有 det.onnx / cls.onnx / rec.onnx + keys.txt";
        let fused = crate::bot_model_loop::rounds_fuse_message(partial, "", 20);
        let (st, err, res) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_turns"));
        let res = res.expect("轮数熔断不得写 NULL result_json");
        let summary = res["summary"].as_str().expect("熔断行必须带 summary");
        assert!(
            summary.contains("det.onnx"),
            "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
        );
    }

    /// 工具熔断必须落 budget_exceeded(max_tool_calls)。
    ///
    /// 此前 classify_outcome 只认轮数熔断标记，工具熔断（另一个常量）落不进
    /// 预算分支：模型照办输出 {"status":"succeeded"} 就报成功，父 agent 因此
    /// 不重试——预算烧满、产出残缺，却记成一次干净的成功。
    #[test]
    fn classify_outcome_treats_tool_fuse_as_budget_exceeded() {
        use SubagentStatus::*;
        let partial = "查到 3 个候选包，还差最后一步没跑";
        let fused = crate::bot_model_loop::fuse_message(partial, "", 100);
        let (st, err, res) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded, "工具熔断必须落预算触达");
        assert_eq!(err.as_deref(), Some("max_tool_calls"));
        let res = res.expect("工具熔断不得写 NULL result_json");
        let summary = res["summary"].as_str().expect("熔断行必须带 summary");
        assert!(
            summary.contains("3 个候选包"),
            "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
        );

        // 轮数熔断仍归 max_turns（轮数标记是工具标记的前缀，判据不能反）
        let rounds = crate::bot_model_loop::rounds_fuse_message(partial, "", 20);
        let (st, err, _) = classify_outcome(None, false, Some(&rounds));
        assert_eq!(st, BudgetExceeded);
        assert_eq!(err.as_deref(), Some("max_turns"));
    }

    /// 墙钟熔断必须落 budget_exceeded(max_wall_seconds) **并保留正文**。
    ///
    /// 两个坑：
    /// 1. 墙钟标记「⏹ 已熔断（墙钟上限）」**包含**通用熔断标记「⏹ 已熔断」——
    ///    判据顺序反了会被认成工具调用熔断，真实原因「墙钟触顶」又被丢掉。
    /// 2. 与轮数熔断同款留产：跑了几十分钟的部分产出不能归零。
    #[test]
    fn classify_outcome_treats_wall_fuse_as_budget_exceeded_and_keeps_output() {
        use SubagentStatus::*;
        let partial = "已核到 3 家供应商的报价单，还差最后一步比价";
        let fused = crate::bot_model_loop::wall_fuse_message(partial, "", 600);
        let (st, err, res) = classify_outcome(None, false, Some(&fused));
        assert_eq!(st, BudgetExceeded, "墙钟熔断必须落预算触达");
        assert_eq!(
            err.as_deref(),
            Some("max_wall_seconds"),
            "墙钟熔断不得被认成工具调用熔断（墙钟标记包含通用标记）"
        );
        let res = res.expect("墙钟熔断不得写 NULL result_json");
        let summary = res["summary"].as_str().expect("熔断行必须带 summary");
        assert!(
            summary.contains("3 家供应商"),
            "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
        );
    }

    /// 收尾 JSON 的本地修复（结构化输出三段修复的第 2 段）：零 token、零延迟，
    /// 必须排在补答之前——补答要再走一次 LLM 轮次。
    #[test]
    fn parse_final_json_repairs_trailing_comma_and_truncation() {
        // 尾逗号
        let v = parse_final_json(r#"{"status":"succeeded","summary":"好了",}"#)
            .expect("尾逗号应被本地修复救回");
        assert_eq!(v["status"], "succeeded");
        // 被截断（写到一半没了）
        let v = parse_final_json(r#"{"status":"succeeded","summary":"调研完成，报告"#)
            .expect("截断的 JSON 应被补闭合救回");
        assert_eq!(v["status"], "succeeded");
        // markdown 围栏（既有能力，顺带锁住）
        let v = parse_final_json("```json\n{\"status\":\"failed\"}\n```").expect("围栏应能取到");
        assert_eq!(v["status"], "failed");
    }

    /// 本地修复不得改坏字符串内容：模型写的正文里出现逗号/花括号是合法的。
    #[test]
    fn parse_final_json_does_not_corrupt_string_content() {
        let raw = r#"{"status":"succeeded","summary":"结论：a, b, c"}"#;
        let v = parse_final_json(raw).expect("正常 JSON 必须能解析");
        assert_eq!(v["summary"], "结论：a, b, c");
        // 字符串里的尾逗号形态不能被当成尾逗号删掉
        let raw = r#"{"status":"succeeded","summary":"结尾带逗号,"}"#;
        let v = parse_final_json(raw).expect("字符串里的逗号不得被改");
        assert_eq!(v["summary"], "结尾带逗号,");
    }

    /// 散文收尾仍然解析不出来——修复不是万能的，解析失败要如实落失败。
    #[test]
    fn parse_final_json_leaves_prose_unparseable() {
        assert!(parse_final_json("纯文本收尾，没有 JSON").is_none());
        assert!(parse_final_json("").is_none());
    }

    /// 截断的 JSON 修出来若**认不出收尾语义**（没有 status / checks），
    /// 不得认成收尾结果——判定里「没有 status」默认按 succeeded 算，
    /// 把「没说完」说成「说完了」。
    #[test]
    fn parse_final_json_rejects_truncated_fragment_without_status() {
        assert!(
            parse_final_json(r#"{"summary":"写到一半断了"#).is_none(),
            "半截 JSON 没有 status，不得被认成收尾结果"
        );
        // 有 status 的截断件认得出来（模型已经表明了结论）
        assert!(
            parse_final_json(r#"{"status":"succeeded","summary":"写到一半断了"#).is_some(),
            "已有 status 的截断件应被救回"
        );
    }

    #[test]
    fn machine_pass_overrides_unparseable_finish() {
        let verdicts = vec![
            MachineVerdict {
                criterion: "file_exists:调研.md".into(),
                met: true,
                reason: "服务端核验：存在（13780 字节）".into(),
            },
            MachineVerdict {
                criterion: "file_nonempty:调研.md".into(),
                met: true,
                reason: "服务端核验：存在（13780 字节）".into(),
            },
        ];
        let (st, err, res) = classify_outcome(None, false, Some("散文收尾，没写 JSON"));
        assert_eq!(st, SubagentStatus::Failed, "起点是解析失败");
        assert!(res
            .expect("原文必须留在 result_json")
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap()
            .contains("散文收尾"));

        let (st2, err2, recovered) = reconcile_machine_override(st, err, &verdicts);
        assert_eq!(st2, SubagentStatus::Succeeded);
        assert!(recovered);
        // 推翻这件事本身要留痕：error 上保留解析失败标记并补出理由
        let e = err2.expect("推翻后仍须留标记");
        assert!(e.starts_with(UNPARSEABLE_MARKER), "{e}");
        assert!(e.contains("2 项"), "{e}");
    }

    /// 边界：机器全过只推翻「解析失败」这一种。模型自报失败说的可能是服务端
    /// 验不了的自由文本条目；一条机器结论都没有时服务端零证据；验了没过更是
    /// 实打实的不合格——这三种都必须维持失败。
    #[test]
    fn machine_pass_overrides_nothing_else() {
        let all_met = vec![MachineVerdict {
            criterion: "file_exists:a.md".into(),
            met: true,
            reason: "存在".into(),
        }];
        for (st, err) in [
            (
                SubagentStatus::Failed,
                Some("model_reported_failure".to_string()),
            ),
            (SubagentStatus::Failed, Some("HTTP 500".to_string())),
            (
                SubagentStatus::BudgetExceeded,
                Some("max_turns".to_string()),
            ),
        ] {
            let (s2, e2, recovered) = reconcile_machine_override(st, err.clone(), &all_met);
            assert_eq!(s2, st);
            assert_eq!(e2, err);
            assert!(!recovered);
        }
        // 零证据（一条机器标准都没声明）→ 维持失败
        let (s2, _, r2) = reconcile_machine_override(
            SubagentStatus::Failed,
            Some(UNPARSEABLE_MARKER.into()),
            &[],
        );
        assert_eq!(s2, SubagentStatus::Failed);
        assert!(!r2);
        // 验了没过 → 维持失败
        let unmet = vec![MachineVerdict {
            criterion: "file_exists:a.md".into(),
            met: false,
            reason: "产物不存在".into(),
        }];
        let (s3, _, r3) = reconcile_machine_override(
            SubagentStatus::Failed,
            Some(UNPARSEABLE_MARKER.into()),
            &unmet,
        );
        assert_eq!(s3, SubagentStatus::Failed);
        assert!(!r3);
    }

    /// 接线守卫：机器验收的修正必须落在终态快照之前。判定顺序反了的话，
    /// 「机器验收通过」与「行状态 failed」就会永远同时成立——那正是本次修的 bug。
    #[test]
    fn machine_override_runs_before_status_snapshot() {
        let src = include_str!("bot_orchestrator.rs");
        let fix = src
            .find("reconcile_machine_override(status, error,")
            .expect("finalize 必须调用 reconcile_machine_override");
        let snapshot = src.find("let status_for_db = status;").expect("终态快照点");
        assert!(fix < snapshot, "修正必须早于落库快照");
    }

    /// 「问过即已吸收」：`check_subagent` 拿到终态后结果已在父上下文里，
    /// digest 与自动汇报轮都不该再讲一遍。去重表按会话隔离，别把 A 会话的
    /// 结果算到 B 会话头上。
    #[test]
    fn digest_mark_is_per_session() {
        let sid = format!("dm-{}", uuid::Uuid::new_v4());
        assert!(!is_digested(&sid, "sa_a"));
        mark_digested(Some(&sid), "sa_a");
        assert!(is_digested(&sid, "sa_a"));
        assert!(
            !is_digested(&format!("{sid}-other"), "sa_a"),
            "去重表按会话隔离"
        );
        mark_digested(None, "sa_b");
        assert!(!is_digested(&sid, "sa_b"), "无会话 id 不得污染任何会话");

        // 回报段的吸收是两步：先构造（不碰已吸收集合），模型循环成功后才提交。
        // 构造即提交是「digest 失败即丢」的根因——循环失败时这段回报压根没送进
        // 模型，却已被记成已吸收，汇报轮从此不再重提这条子任务结果。
        let sid2 = format!("dmc-{}", uuid::Uuid::new_v4());
        let rows = vec![digest_row(
            "sa_x",
            SubagentStatus::Succeeded,
            Some(OK_RESULT),
        )];
        let built = build_subagent_digest(&rows, &std::collections::HashSet::new())
            .expect("有未汇报的终态子任务就该出段");
        assert_eq!(built.absorb, vec!["sa_x".to_string()]);
        assert!(
            !is_digested(&sid2, "sa_x"),
            "构造回报段不构成吸收——模型循环还没成功"
        );
        for id in &built.absorb {
            mark_digested(Some(&sid2), id);
        }
        assert!(is_digested(&sid2, "sa_x"), "提交后才算已吸收");
    }

    /// 接线守卫：汇报轮失败不能直接放弃——那等于这条子任务结果永远没人汇报。
    ///
    /// 失败与 Busy 同款处理：deadline 内 sleep 后再试（digest 此时也没记成已
    /// 吸收，下一轮还有机会把结果说出去）。
    #[test]
    fn report_failure_is_retried_instead_of_given_up() {
        let src = include_str!("bot_orchestrator.rs");
        // 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）
        assert!(
            !src.contains(concat!(
                "ReportOutcome::Ran | crate::bot_chat::",
                "ReportOutcome::Failed"
            )),
            "汇报失败不能和成功同款直接 return"
        );
        assert!(
            src.contains(concat!(
                "ReportOutcome::Failed | crate::bot_chat::",
                "ReportOutcome::Busy"
            )),
            "汇报失败要与 Busy 同款：deadline 内 sleep 后重试"
        );
    }

    /// 接线守卫：兜底写终态失败必须留痕。
    ///
    /// 兜底写的是「行停在 queued/running」的唯一补救；它自己失败时若不记，
    /// 那行就永远停在那里，面板上一直转圈，事后也查不到是哪一步失败的。
    /// 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）。
    #[test]
    fn terminal_state_fallback_write_leaves_a_trace_on_failure() {
        let src = include_str!("bot_orchestrator.rs");
        assert!(
            !src.contains(concat!("let _ = db_", "locked(")),
            "兜底写终态不得吞掉失败——行会永远停在 queued/running"
        );
        let write_needle = concat!("let write = db_", "locked(");
        let trace_needle = concat!("subagent_terminal_state_write_", "failed");
        // 按「兜底写」切段：每段到下一个兜底写为止，每段都要有自己的失败留痕
        let segments: Vec<&str> = src.split(write_needle).skip(1).collect();
        assert!(
            segments.len() >= 4,
            "兜底写终态有四处（加载失败 / 产物目录不可用 / 会话建立失败 / 收尾失败），\
             现 {len} 处",
            len = segments.len()
        );
        for seg in &segments {
            assert!(
                seg.contains(trace_needle),
                "兜底写终态失败必须留痕——行永远停在 queued/running 时只有这条线索"
            );
        }
    }

    /// 接线守卫：`subagent_digest_block` 只造段，不得顺手记「已吸收」。
    ///
    /// 它被模型循环**之前**调用——在这里记，等于把「这段回报已经说过了」提前
    /// 宣布；循环失败时它压根没送进模型，汇报轮也会因此以为说过、从此不再
    /// 重提，子任务结果在本次进程内永久消失。
    #[test]
    fn digest_block_builds_without_absorbing() {
        let src = include_str!("bot_orchestrator.rs");
        let start = src
            .find("pub(crate) async fn subagent_digest_block(")
            .expect("要有 subagent_digest_block");
        let rest = &src[start..];
        let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(
            !body.contains("set.insert"),
            "回报段的构造函数里不得记已吸收——记动作在模型循环成功之后"
        );
        assert!(
            body.contains("build_subagent_digest("),
            "构造函数仍要调 build_subagent_digest 出段"
        );
    }

    /// 接线守卫：「已吸收」的标记不在 `bot_chat_inner` 里做。
    ///
    /// 这一层既不知道送达结果、也不该替调用方决定：交互聊天（前端驱动）在
    /// 命令返回前记（模型确实看到了那段回报），汇报轮（后端驱动）要等落库
    /// 成功才记（它的历史下一轮从库里读，没落库的回复等于没说出去）。在这里
    /// 无条件记，等于把「已经说过了」提前宣布——落库失败时那段回报从此不再
    /// 进 digest，子任务结果在本次进程内永久消失。
    #[test]
    fn digest_absorb_mark_is_left_to_the_caller() {
        let chat = include_str!("bot_chat.rs");
        let start = chat
            .find("async fn bot_chat_inner(")
            .expect("bot_chat_inner 函数体起点");
        let body = &chat[start..];
        let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
        let inner = &body[..end];
        assert!(
            !inner.contains("mark_digested("),
            "bot_chat_inner 里不得记「已吸收」——标记由知道送达结果的调用方做"
        );
    }

    /// 派发方会话被删后不该再汇报：bot_messages 没有外键约束，硬跑一轮只会给
    /// 一个不存在的会话写一条永远读不到的孤儿消息。
    #[test]
    fn missing_parent_session_blocks_the_report() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE bot_sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                is_subagent INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        assert!(
            !bot_session_exists_locked(&conn, "s-gone"),
            "不存在的会话判为不在"
        );
        conn.execute(
            "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
            [],
        )
        .unwrap();
        assert!(bot_session_exists_locked(&conn, "s1"));
    }

    /// 接线守卫：汇报轮的轮次说明要排在 digest 注入之前。digest 放末尾是
    /// 刻意的（模型对末尾权重最高），说明插在它前面才不抢这份权重。
    #[test]
    fn report_turn_prompt_precedes_digest_injection() {
        let src = include_str!("bot_chat.rs");
        let prompt = src
            .find("REPORT_TURN_PROMPT}))")
            .expect("汇报轮必须注入轮次说明");
        let digest = src
            .find("subagent_digest_block(&app, session_id.as_deref())")
            .expect("digest 注入点");
        assert!(prompt < digest, "说明应在 digest 之前，末尾留给回报本身");
    }

    /// verifier 输出 verified.json schema 的判定分支覆盖。
    /// checks 数组结构而非 status 字段是 verifier 路径的真正信号源。
    #[test]
    fn classify_outcome_verifier_paths() {
        use SubagentStatus::*;
        // 全部 met=true → Succeeded
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"checks":[{"criterion":"c1","met":true,"evidence":"x"}],"summary":"all ok"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        // 全部 met=false → Failed + reason 列前 3 项 criterion
        let (st, err, res) = classify_outcome(
            None,
            false,
            Some(
                r#"{"checks":[{"criterion":"导出 PDF","met":false,"evidence":"缺依赖"},
                              {"criterion":"样式对齐","met":false,"evidence":"字体不一致"}],
                   "summary":"全部失败"}"#,
            ),
        );
        assert_eq!(st, Failed);
        let err_str = err.unwrap();
        assert!(
            err_str.starts_with("verifier_all_checks_failed:"),
            "actual: {err_str}"
        );
        assert!(err_str.contains("导出 PDF=false"), "actual: {err_str}");
        assert!(err_str.contains("样式对齐=false"), "actual: {err_str}");
        assert_eq!(res.unwrap()["summary"], "全部失败");
        // 部分 met=true → Succeeded（主 agent 看 checks 自己决策）
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(
                r#"{"checks":[{"criterion":"c1","met":true,"evidence":"x"},
                              {"criterion":"c2","met":false,"evidence":"y"}],
                   "summary":"部分通过"}"#,
            ),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        // checks 空 + summary 含「未通过」 → Failed
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"checks":[],"summary":"未通过：缺关键证据"}"#),
        );
        assert_eq!(st, Failed);
        let err_str = err.unwrap();
        assert!(
            err_str.starts_with("verifier_summary_rejected:"),
            "actual: {err_str}"
        );
        assert!(err_str.contains("缺关键证据"), "actual: {err_str}");
        // checks 空 + summary 不含否决词 → Succeeded（fail-safe 放行）
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"checks":[],"summary":"没有可检查项，跳过验证"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
        // status=succeeded 的普通收尾无 checks → 走现有路径不受影响
        let (st, err, _) = classify_outcome(
            None,
            false,
            Some(r#"{"status":"succeeded","summary":"ok"}"#),
        );
        assert_eq!(st, Succeeded);
        assert_eq!(err, None);
    }

    #[test]
    fn task_wrapper_contains_all_sections() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.context_summary = Some("上游已确认五个竞品清单".into());
        let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        let budget = crate::db::SubagentBudget::default();
        let w = render_task_wrapper(&row, &budget, std::path::Path::new("/tmp/art"));
        assert!(w.contains("[子任务派发]"));
        assert!(w.contains(&format!("subagent_id: {}", row.id)));
        assert!(w.contains("objective: 调研五个竞品并输出报告"));
        assert!(w.contains("- 覆盖至少 5 个产品"));
        // 上下文摘要取自行上 context_summary（不再从子卡 note 文本解析）
        assert!(w.contains("上下文摘要:\n上游已确认五个竞品清单"));
        assert!(w.contains("max_turns=30, max_tool_calls=100, max_wall_seconds=600"));
        assert!(w.contains("产物目录:"));
        assert!(w.contains("/tmp/art"));
        assert!(w.contains("完成后必须输出以下 JSON"));
        assert!(w.contains("\"succeeded|failed|cancelled|budget_exceeded\""));
    }

    /// 无上下文摘要时包装不带该段（不留空标题噪声）
    #[test]
    fn task_wrapper_omits_absent_context_summary() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        let w = render_task_wrapper(&row, &SubagentBudget::default(), std::path::Path::new("/a"));
        assert!(!w.contains("上下文摘要"));
    }

    fn req(parent: Option<String>) -> SpawnRequest {
        SpawnRequest {
            objective: "调研五个竞品并输出报告".into(),
            profile: SubagentProfile::Research,
            stage: None,
            acceptance_criteria: vec!["覆盖至少 5 个产品".into(), "每条含官网 URL".into()],
            context_summary: None,
            budget: None,
            model_profile: None,
            onion: None,
            parent_task_id: parent,
            parent_session_id: Some("main-sess".into()),
        }
    }

    #[test]
    fn spawn_creates_row_only_and_no_subcard() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let parent = parent_card(&conn);
        let ack = spawn_subagent_locked(&mut conn, &req(Some(parent)), 5_000).unwrap();
        assert!(ack.subagent_id.starts_with("sa_"));
        assert_eq!(ack.status, SubagentStatus::Queued);
        // 不再建卡：行上 task_id 为 NULL，tasks 表除 parent 外无新增
        assert_eq!(ack.task_id, None);
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.task_id, None);
        assert_eq!(row.parent_session_id.as_deref(), Some("main-sess"));
        let cards = crate::db::load_all(&conn).unwrap();
        assert_eq!(cards.len(), 1, "只应有 parent 卡，子 agent 不建卡");
        assert_eq!(cards[0].id, "parent-1");
        // 验收标准与上下文摘要落在行上（机器侧，不再双写 note）
        let criteria: Vec<String> =
            serde_json::from_str(row.acceptance_json.as_deref().unwrap()).unwrap();
        assert_eq!(criteria.len(), 2);
        assert!(row.context_summary.is_none());
        // 预算上表可见（默认值）
        let budget: SubagentBudget =
            serde_json::from_str(row.budget_json.as_deref().unwrap()).unwrap();
        assert_eq!(budget.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(budget.max_tool_calls, DEFAULT_MAX_TOOL_CALLS);
        assert_eq!(budget.max_wall_seconds, DEFAULT_MAX_WALL_SECONDS);
    }

    /// 空白上下文摘要不落库（否则包装会渲染一个空标题段）
    #[test]
    fn spawn_trims_blank_context_summary_to_none() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.context_summary = Some("   \n  ".into());
        let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.context_summary, None);
    }

    #[test]
    fn spawn_without_parent_card_is_allowed() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        assert!(ack.task_id.is_none());
        assert!(!ack.trace_id.is_empty());
    }

    /// 重复派发同一份请求不建第二条活行（幂等键：父会话 + 档位 + 目标 +
    /// 验收标准 + 指定模型）。
    ///
    /// 模型重复调用 spawn_subagent（同一轮发两次、或隔几轮忘了自己派过）原先
    /// 会建两条活行，各拿一份完整预算——同一份活干两遍。
    #[test]
    fn repeated_spawn_of_the_same_request_reuses_the_live_row() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        assert!(!first.deduped, "首次派发必须是新建");
        let second = spawn_subagent_locked(&mut conn, &req(None), 6_000).unwrap();
        assert!(second.deduped, "重复派发必须命中幂等键");
        assert_eq!(
            second.subagent_id, first.subagent_id,
            "重复派发必须返回同一个子 agent id"
        );
        let rows = crate::db::load_subagents_by_parent(&conn, "main-sess", 100).unwrap();
        assert_eq!(rows.len(), 1, "重复派发不得再建一条活行");
    }

    /// 已终态的不参与去重：「再跑一次」是重试语义，不能被幂等键吞掉。
    #[test]
    fn spawn_after_the_previous_one_finished_creates_a_new_row() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        crate::db::update_subagent_status(
            &conn,
            &first.subagent_id,
            SubagentStatus::Cancelled,
            6_000,
            None,
        )
        .unwrap();
        let second = spawn_subagent_locked(&mut conn, &req(None), 7_000).unwrap();
        assert!(!second.deduped, "已终态的旧行不该再命中幂等键");
        assert_ne!(second.subagent_id, first.subagent_id);
        let rows = crate::db::load_subagents_by_parent(&conn, "main-sess", 100).unwrap();
        assert_eq!(rows.len(), 2, "重试应另起一行");
    }

    /// 验收标准不同 = 不同的活，不去重（模型改需求后重派是正当的）。
    #[test]
    fn spawn_with_different_acceptance_criteria_is_not_deduped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let mut other = req(None);
        other.acceptance_criteria = vec!["覆盖至少 8 个产品".into()];
        let second = spawn_subagent_locked(&mut conn, &other, 6_000).unwrap();
        assert!(!second.deduped, "验收标准不同不该命中幂等键");
        assert_ne!(second.subagent_id, first.subagent_id);
    }

    /// 不同父会话的同目标派发是不同意图，不互相吞掉。
    #[test]
    fn spawn_from_a_different_parent_session_is_not_deduped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let mut other = req(None);
        other.parent_session_id = Some("other-sess".into());
        let second = spawn_subagent_locked(&mut conn, &other, 6_000).unwrap();
        assert!(!second.deduped, "跨会话不命中幂等键");
        assert_ne!(second.subagent_id, first.subagent_id);
    }

    /// 没有父会话（拿不到会话 id 的调用路径）时不去重：宁可多建一行，
    /// 也不要把别人的活吞掉。
    #[test]
    fn spawn_without_parent_session_is_not_deduped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.parent_session_id = None;
        let first = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let second = spawn_subagent_locked(&mut conn, &r, 6_000).unwrap();
        assert!(!first.deduped && !second.deduped);
        assert_ne!(first.subagent_id, second.subagent_id);
    }

    /// 幂等键挡住的是「建第二条活行」，挡不住「再起一个 runner」——后者照样
    /// 把同一份活跑两遍、各烧一份预算。deduped 的早退必须排在 runner 派发前。
    #[test]
    fn deduped_spawn_returns_before_the_runner_is_dispatched() {
        let src = include_str!("bot_orchestrator.rs");
        let start = src
            .find("pub async fn spawn_subagent(")
            .expect("bot_orchestrator.rs 应有 spawn_subagent");
        let dedup = start
            + src[start..]
                .find("ack.deduped")
                .expect("spawn_subagent 应有 deduped 早退分支");
        let runner = start
            + src[start..]
                .find("run_subagent(")
                .expect("spawn_subagent 应派发 runner");
        assert!(
            dedup < runner,
            "命中幂等键必须早于 runner 派发返回（否则同一份活跑两遍）"
        );
        assert!(
            src[dedup..runner].contains("return Ok(ack);"),
            "deduped 分支必须直接返回，不得落到 runner 派发"
        );
    }

    /// 回执文案两种形态：新建说「已派发」，命中幂等键要说清「没新建、
    /// 还是那个」——模型照抄「已派发」会把一个子 agent 说成两个。
    #[test]
    fn spawn_receipt_text_distinguishes_new_from_deduped() {
        let ack = SpawnAck {
            subagent_id: "sa_x".into(),
            task_id: None,
            trace_id: "trc_x".into(),
            status: SubagentStatus::Queued,
            deduped: false,
            budget: SubagentBudget::default(),
        };
        let fresh = spawn_receipt_text(&ack);
        assert!(fresh.contains("已派发子 agent"), "{fresh}");
        assert!(fresh.contains("sa_x"), "{fresh}");
        // 回执必须带生效预算——模型不知道墙钟被钳过，就会按自己给的数安排动作
        assert!(fresh.contains("生效预算"), "{fresh}");

        let deduped = SpawnAck {
            deduped: true,
            ..ack
        };
        let dup = spawn_receipt_text(&deduped);
        assert!(dup.contains("未重复派发"), "{dup}");
        assert!(dup.contains("sa_x"), "{dup}");
        assert!(
            !dup.contains("已派发子 agent"),
            "命中幂等键的回执不得说成新建：{dup}"
        );
    }

    #[test]
    fn spawn_with_missing_parent_card_is_task_not_found() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        // Result 的 OK 侧 SpawnAck 无 Debug——用 match 拆错误，不 unwrap_err
        let err = match spawn_subagent_locked(&mut conn, &req(Some("ghost-card".into())), 5_000) {
            Err(e) => e,
            Ok(_) => panic!("缺卡 spawn 必须失败"),
        };
        match err {
            CommandError::TaskNotFound(id) => assert_eq!(id, "ghost-card"),
            other => panic!("应 TaskNotFound，实际 {other:?}"),
        }
    }

    /// 机器可判定条目带杂质要在**派发期**就拒。
    ///
    /// 模型写 `file_exists:调研.md 已完成`，服务端把路径当成「调研.md 已完成」，
    /// 文件当然不存在——假阴性。此前要等子 agent 跑完十几分钟才暴露，白烧一整轮。
    #[test]
    fn spawn_rejects_machine_criterion_with_trailing_prose() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.acceptance_criteria = vec!["file_exists:调研.md 已完成".into()];
        let err = match spawn_subagent_locked(&mut conn, &r, 5_000) {
            Ok(_) => panic!("带杂质的机器可判定条目必须当场拒"),
            Err(e) => e,
        };
        let msg = err.message();
        assert!(
            msg.contains("file_exists:report.md"),
            "报错要带正例（模型不知道对的长什么样）：{msg}"
        );
        // 干净的写法照常放行
        let mut ok = req(None);
        ok.acceptance_criteria = vec![
            "file_exists:调研.md".into(),
            "file_nonempty:output/数据.xlsx".into(),
            "内容符合用户口径".into(),
        ];
        assert!(
            spawn_subagent_locked(&mut conn, &ok, 5_000).is_ok(),
            "干净的机器可判定条目不得被拒"
        );
    }

    #[test]
    fn spawn_rejects_blank_objective_and_empty_acceptance() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.objective = "  ".into();
        assert!(spawn_subagent_locked(&mut conn, &r, 5_000).is_err());
        let mut r2 = req(None);
        r2.acceptance_criteria = Vec::new();
        assert!(spawn_subagent_locked(&mut conn, &r2, 5_000).is_err());
        let mut r3 = req(None);
        r3.acceptance_criteria = vec!["  ".into()];
        assert!(spawn_subagent_locked(&mut conn, &r3, 5_000).is_err());
    }

    #[test]
    fn budget_clamps_turns_to_hard_cap() {
        let b = clamp_budget(Some(SubagentBudget {
            max_turns: 9_999,
            max_tool_calls: 0,
            max_wall_seconds: 0,
        }));
        assert_eq!(b.max_turns, MAX_TURNS_HARD_CAP);
        assert_eq!(b.max_tool_calls, 1);
        // 墙钟下界与配置键同源（30s）——「1 秒墙钟」等于白派一个子 agent
        assert_eq!(b.max_wall_seconds, 30);
        assert_eq!(
            clamp_budget(None),
            SubagentBudget {
                max_turns: 30,
                max_tool_calls: 100,
                max_wall_seconds: 600
            }
        );
    }

    /// 墙钟的硬顶必须与配置键走同一组常量——两条路两个闸时，LLM 显式给的
    /// `maxWallSeconds=999999` 会原样生效，墙钟实际由调用方说了算。
    #[test]
    fn budget_wall_clock_is_capped_at_the_same_bound_as_config() {
        let too_big = clamp_budget(Some(SubagentBudget {
            max_turns: 30,
            max_tool_calls: 100,
            max_wall_seconds: 999_999,
        }));
        assert_eq!(
            too_big.max_wall_seconds,
            crate::bot::params::SUBAGENT_MAX_WALL_SECS_MAX
        );
        // 配置键路径给同一个数，钳完必须一模一样
        let via_config = crate::bot::params::clamp_subagent_max_wall_secs(999_999);
        assert_eq!(too_big.max_wall_seconds, via_config);
    }

    #[test]
    fn cancel_sets_cancelled_and_terminal_noop() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let c = cancel_subagent_locked(&conn, &ack.subagent_id, "用户取消", 6_000).unwrap();
        assert!(c.ok);
        assert!(!c.already_terminal);
        assert_eq!(c.status, SubagentStatus::Cancelled);
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, SubagentStatus::Cancelled);
        assert_eq!(row.error.as_deref(), Some("用户取消"));
        assert_eq!(row.finished_at, Some(6_000));
        // 取消不动 tasks 表（无卡可退）
        assert!(crate::db::load_all(&conn).unwrap().is_empty());
        // 终态再取消 = no-op 幂等
        let again = cancel_subagent_locked(&conn, &ack.subagent_id, "再取消", 7_000).unwrap();
        assert!(again.already_terminal);
        assert_eq!(again.status, SubagentStatus::Cancelled);
        assert_eq!(
            crate::db::load_subagent(&conn, &ack.subagent_id)
                .unwrap()
                .unwrap()
                .finished_at,
            Some(6_000),
            "幂等重入不得改写首收尾时刻"
        );
    }

    /// 有卡的历史行仍能按 task_id 解析（子会话键之外的第二把键，降级路径不炸）
    #[test]
    fn cancel_resolves_by_task_id_on_legacy_row() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        // 模拟老库遗留：给行补一个 task_id
        conn.execute(
            "UPDATE subagents SET task_id = 'legacy-card' WHERE id = ?1",
            [&ack.subagent_id],
        )
        .unwrap();
        let c = cancel_subagent_locked(&conn, "legacy-card", "卡片停止按钮", 6_000).unwrap();
        assert_eq!(c.subagent_id, ack.subagent_id);
        assert_eq!(c.status, SubagentStatus::Cancelled);
    }

    /// 新行 task_id 为 NULL，按卡键解析落到会话键探测后仍报「不存在」而非 panic
    #[test]
    fn cancel_by_task_key_on_cardless_row_is_invalid_argument() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let err = match cancel_subagent_locked(&conn, "no-such-card", "x", 6_000) {
            Err(e) => e,
            Ok(_) => panic!("无卡行的假卡键不得解析成功"),
        };
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
        assert_eq!(ack.task_id, None);
    }

    #[test]
    fn cancel_unknown_id_is_invalid_argument() {
        let conn = test_conn();
        // OK 侧 CancelAck 无 Debug——match 拆错误
        let err = match cancel_subagent_locked(&conn, "sa_ghost", "x", 1) {
            Err(e) => e,
            Ok(_) => panic!("未知 id 必须失败"),
        };
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
    }

    #[test]
    fn cancel_beats_classified_failure_when_reporting_terminal_state() {
        // 用户取消 → StopToken 打断 → classify 只会给 Failed；上报必须用行上的
        // cancelled，否则每次取消都记一条 subagent_failed + 空原因教训
        assert_eq!(
            effective_status(Some(SubagentStatus::Cancelled), SubagentStatus::Failed),
            SubagentStatus::Cancelled
        );
        // 行没拿到（收尾闭包整体失败）才回落到观感值
        assert_eq!(
            effective_status(None, SubagentStatus::Failed),
            SubagentStatus::Failed
        );
    }

    /// 用户取消的子 agent 不给父会话起汇报轮：用户刚点过停止，
    /// 不需要模型再解释一遍。
    #[test]
    fn cancelled_subagent_does_not_report_to_parent() {
        assert!(!should_report_to_parent(SubagentStatus::Cancelled));
        for st in [
            SubagentStatus::Succeeded,
            SubagentStatus::Failed,
            SubagentStatus::BudgetExceeded,
            SubagentStatus::Running,
            SubagentStatus::Queued,
        ] {
            assert!(should_report_to_parent(st), "{st:?} 应汇报");
        }
    }

    /// 接线守卫：汇报轮的门控必须用收尾时的真实终态，不能用行快照。
    ///
    /// 行是派发时建的，收尾前 status 恒为 Queued——门控拿 `row.status` 判取消
    /// 永远判不成立，用户取消后照样起汇报轮（违反设计「用户取消不汇报」）。
    /// 单测验不到调用点传的是哪个值，故这里扫源码钉住接线。
    #[test]
    fn parent_report_gate_is_wired_to_the_effective_status() {
        let src = include_str!("bot_orchestrator.rs");
        let production = src
            .split_once("#[cfg(test)]")
            .expect("bot_orchestrator.rs 应有测试模块")
            .0;
        assert!(
            production.contains(concat!(
                "spawn_parent_report(&app, &row, effective_",
                "status)"
            )),
            "汇报轮必须传收尾时的真实终态（effective_status）"
        );
        // 只扫汇报轮函数体：别处读行快照是合法的（如 read_own_card 读的是
        // 库里当前行，不是收尾前的快照）
        let start = production
            .find("fn spawn_parent_report")
            .expect("应能找到 spawn_parent_report");
        let rest = &production[start..];
        let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
        let func = &rest[..end];
        assert!(
            func.contains("should_report_to_parent("),
            "汇报轮门控必须走 should_report_to_parent"
        );
        assert!(
            !func.contains("row.status"),
            "汇报轮门控不得读行快照判取消（快照恒为 Queued）"
        );
    }

    /// 在跑的子 agent 必须报得出「已跑 N / 预算 M 轮」。
    ///
    /// 回归背景：轮数此前完全不落任何地方，主 agent 靠 `check_subagent` 轮询
    /// 只看得到验收进度——一个跑满预算、在原地打转的子任务和一个刚启动的子任务
    /// 在 UI 上长得一模一样，只能干等 budget_exceeded 才知道它在烧钱。
    #[test]
    fn check_reports_rounds_while_subagent_runs() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let sid = format!("sa-test-rounds-{}", ack.subagent_id);
        // 注册子会话（行上 session_id 与之对应），模拟在跑的状态
        crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

        // 守卫注销时（终态）读不到计数器 → 不报轮数，字段整体不出现
        let queued = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert!(
            queued.progress.get("roundsUsed").is_none(),
            "无在跑计数器时不得报轮数（凭空造数比不报更坏）"
        );

        let _g2 = crate::tool_guard::SubagentSessionGuard::new(
            &sid,
            crate::tool_guard::SubagentSessionCtx {
                subagent_id: ack.subagent_id.clone(),
                task_id: None,
                profile: SubagentProfile::Research,
                tools: std::sync::Arc::new(vec![]),
                budget: SubagentBudget::default(),
                artifact_dir: std::path::PathBuf::from("/tmp/art_rounds"),
                used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(14)),
                wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
            },
        );
        let running = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(running.progress["roundsUsed"], 14);
        assert_eq!(
            running.progress["roundsBudget"],
            SubagentBudget::default().max_turns,
            "预算必须与轮数同报：10/20 与 10/200 紧迫度完全不同"
        );
    }

    /// list_subagents 的行视图与 check_subagent 同口径：在途报轮数与预算，
    /// 读不到仪表（终态/未开跑）就整段留空——两条链路口径必须一致，否则
    /// 「侧栏说跑了 14 轮、check 说没有」会让人怀疑数据。
    #[test]
    fn list_view_attaches_rounds_while_subagent_runs() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let sid = format!("sa-test-view-{}", ack.subagent_id);
        crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("spawn 后应有行");
        let queued = attach_rounds(row);
        assert_eq!(
            (queued.rounds_used, queued.rounds_budget),
            (None, None),
            "无在跑仪表时不得报轮数（凭空造数比不报更坏）"
        );

        let _g2 = crate::tool_guard::SubagentSessionGuard::new(
            &sid,
            crate::tool_guard::SubagentSessionCtx {
                subagent_id: ack.subagent_id.clone(),
                task_id: None,
                profile: SubagentProfile::Research,
                tools: std::sync::Arc::new(vec![]),
                budget: SubagentBudget::default(),
                artifact_dir: std::path::PathBuf::from("/tmp/art_view"),
                used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(14)),
                wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
            },
        );
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("行仍在");
        let running = attach_rounds(row);
        assert_eq!(running.rounds_used, Some(14));
        assert_eq!(
            running.rounds_budget,
            Some(SubagentBudget::default().max_turns),
            "预算必须与轮数同报：10/20 与 10/200 紧迫度完全不同"
        );
    }

    /// 守卫注销（子 agent 收尾）后轮数归空：列表要显示的是「这轮跑了多久」，
    /// 不是死后残留的最后一帧计数。终态行拿 started/finished 算时长即可。
    #[test]
    fn list_view_drops_rounds_after_guard_dropped() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        let sid = format!("sa-test-drop-{}", ack.subagent_id);
        crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

        {
            let _guard = crate::tool_guard::SubagentSessionGuard::new(
                &sid,
                crate::tool_guard::SubagentSessionCtx {
                    subagent_id: ack.subagent_id.clone(),
                    task_id: None,
                    profile: SubagentProfile::Research,
                    tools: std::sync::Arc::new(vec![]),
                    budget: SubagentBudget::default(),
                    artifact_dir: std::path::PathBuf::from("/tmp/art_drop"),
                    used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                    used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(7)),
                    wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
                },
            );
            let row = crate::db::load_subagent(&conn, &ack.subagent_id)
                .unwrap()
                .expect("行仍在");
            assert_eq!(attach_rounds(row).rounds_used, Some(7));
        }

        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("行仍在");
        let after = attach_rounds(row);
        assert_eq!(
            (after.rounds_used, after.rounds_budget),
            (None, None),
            "守卫注销后不得继续报轮数"
        );
    }

    /// 子 agent 预算默认值单源：db 侧常量必须引用 bot::params，改一处不漏两处。
    #[test]
    fn subagent_budget_defaults_have_single_source() {
        assert_eq!(
            crate::db::subagents::DEFAULT_MAX_TURNS,
            crate::bot::params::DEFAULT_SUBAGENT_MAX_TURNS
        );
        assert_eq!(
            crate::db::subagents::DEFAULT_MAX_TOOL_CALLS,
            crate::bot::params::DEFAULT_SUBAGENT_MAX_TOOL_CALLS
        );
        assert_eq!(
            crate::db::subagents::DEFAULT_MAX_WALL_SECONDS,
            crate::bot::params::DEFAULT_SUBAGENT_MAX_WALL_SECS
        );
    }

    #[test]
    fn check_counts_progress_from_acceptance_and_result() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        // 未收尾：total = 验收标准条数，done = 0
        let by_id = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(by_id.subagent_id, ack.subagent_id);
        assert_eq!(by_id.status, SubagentStatus::Queued);
        assert_eq!(by_id.progress["subtasksDone"], 0);
        assert_eq!(by_id.progress["subtasksTotal"], 2);
        assert!(by_id.result.is_none());
        assert_eq!(by_id.task_id, None);
        // 收尾后：done = result_json.acceptance_check 里 met=true 的条数
        crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"acceptance_check":[{"criterion":"覆盖至少 5 个产品","met":true},{"criterion":"每条含官网 URL","met":false}]}"#,
        )
        .unwrap();
        let done = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(done.progress["subtasksDone"], 1);
        assert_eq!(done.progress["subtasksTotal"], 2);
        // 幂等：两次 check 结果一致
        assert_eq!(
            check_subagent_locked(&conn, &ack.subagent_id)
                .unwrap()
                .progress,
            done.progress
        );
    }

    /// 机器可判定谓词：只有带前缀的条目进机器核验，自由文本原样留给 verifier
    #[test]
    fn machine_criterion_parsed_only_for_known_prefixes() {
        assert_eq!(
            parse_machine_criterion("file_exists:report.md"),
            Some(MachineCheck::FileExists("report.md".into()))
        );
        assert_eq!(
            parse_machine_criterion("  file_nonempty: out/数据.xlsx "),
            Some(MachineCheck::FileNonEmpty("out/数据.xlsx".into()))
        );
        assert_eq!(parse_machine_criterion("覆盖至少 5 个产品"), None);
        assert_eq!(parse_machine_criterion("每条含官网 URL"), None);
        assert_eq!(parse_machine_criterion(""), None);
    }

    /// 路径逃逸一律不解析：判定路径能跑出产物目录，验收就成了任意文件探测器
    #[test]
    fn machine_criterion_rejects_paths_escaping_artifact_dir() {
        for bad in [
            "file_exists:../../etc/passwd",
            "file_exists:/etc/passwd",
            "file_exists:C:\\Windows\\win.ini",
            "file_exists:./report.md",
            "file_exists:",
            "file_nonempty:a//b.md",
        ] {
            assert_eq!(
                parse_machine_criterion(bad),
                None,
                "逃逸路径必须不解析：{bad}"
            );
        }
        assert_eq!(safe_rel_path("a/b/c.md"), Some("a/b/c.md"));
    }

    /// 机器判定真去 FS 看：存在/不存在/空文件三种形态
    #[test]
    fn eval_machine_criteria_reads_real_files() {
        let dir = std::env::temp_dir().join(format!("wm-mc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("report.md"), "# 报告").unwrap();
        std::fs::write(dir.join("empty.md"), "").unwrap();

        let out = eval_machine_criteria(
            &[
                "file_exists:report.md".into(),
                "file_nonempty:report.md".into(),
                "file_nonempty:empty.md".into(),
                "file_exists:missing.md".into(),
                "自由文本标准".into(),
            ],
            &dir,
        );
        assert_eq!(out.len(), 4, "自由文本条目不进机器判定");
        assert!(out[0].met && out[1].met);
        assert!(!out[2].met, "空文件不满足 nonempty");
        assert!(!out[3].met, "缺失文件不通过");
        assert_eq!(out[3].criterion, "file_exists:missing.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 机器结论覆盖模型自评：模型说 met:true 而文件不存在 → 以服务端为准
    #[test]
    fn merge_machine_verdicts_overrides_self_report_and_fills_missing() {
        let mut result = serde_json::json!({
            "acceptance_check": [
                {"criterion": "file_exists:report.md", "met": true, "evidence": "我写好了"},
                {"criterion": "内容覆盖 5 个产品", "met": false}
            ]
        });
        merge_machine_verdicts(
            &mut result,
            &[MachineVerdict {
                criterion: "file_exists:report.md".into(),
                met: false,
                reason: "产物不存在：NotFound".into(),
            }],
        );
        let arr = result["acceptance_check"].as_array().unwrap();
        assert_eq!(arr[0]["met"], false, "自评必须被服务端结论覆盖");
        assert_eq!(arr[0]["machine"], true);
        assert!(arr[0]["machine_reason"]
            .as_str()
            .unwrap()
            .contains("产物不存在"));
        assert_eq!(arr[1]["met"], false, "非机器条目不受影响");

        // 模型压根没报这条 → 服务端补一条（不依赖模型主动上报）
        let mut only_free = serde_json::json!({"acceptance_check": []});
        merge_machine_verdicts(
            &mut only_free,
            &[MachineVerdict {
                criterion: "file_nonempty:out.xlsx".into(),
                met: true,
                reason: "服务端核验：out.xlsx 存在（12 字节）".into(),
            }],
        );
        let arr = only_free["acceptance_check"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["met"], true);

        // verifier 行用 checks 键，同样处理
        let mut verifier = serde_json::json!({"checks": []});
        merge_machine_verdicts(
            &mut verifier,
            &[MachineVerdict {
                criterion: "file_exists:v.md".into(),
                met: true,
                reason: "ok".into(),
            }],
        );
        assert_eq!(verifier["checks"].as_array().unwrap().len(), 1);
    }

    /// 机器判定落到用户可见的正文：未通过项必须点名，不能只躺在 result_json
    #[test]
    fn machine_failures_are_named_in_final_text() {
        let verdict = |c: &str, met: bool| MachineVerdict {
            criterion: c.into(),
            met,
            reason: "产物不存在：NotFound".into(),
        };
        // 全通过 → 正文不动
        let (r, text) = apply_machine_verdicts(
            Some(serde_json::json!({"acceptance_check": []})),
            "干完了".into(),
            &[verdict("file_exists:a.md", true)],
        );
        assert_eq!(text, "干完了");
        assert_eq!(r.unwrap()["acceptance_check"].as_array().unwrap().len(), 1);

        // 有未通过 → 正文点名 + 原因
        let (r, text) = apply_machine_verdicts(
            Some(serde_json::json!({"acceptance_check": []})),
            "干完了".into(),
            &[
                verdict("file_exists:a.md", true),
                verdict("file_exists:b.md", false),
            ],
        );
        assert!(text.contains("机器验收未通过 1 项"), "{text}");
        assert!(text.contains("file_exists:b.md"), "{text}");
        assert!(text.contains("产物不存在"), "{text}");
        assert!(text.starts_with("干完了"), "原正文必须保留：{text}");
        assert_eq!(r.unwrap()["acceptance_check"].as_array().unwrap().len(), 2);

        // 无机器条目 → 原样返回（不改 JSON、不动正文）
        let (r, text) = apply_machine_verdicts(
            Some(serde_json::json!({"acceptance_check": [{"criterion":"x","met":true}]})),
            "干完了".into(),
            &[],
        );
        assert_eq!(text, "干完了");
        assert_eq!(r.unwrap()["acceptance_check"][0]["met"], true);
        // 异常退出无收尾 JSON：不得凭空造一个
        let (r, _) = apply_machine_verdicts(
            None,
            "执行失败".into(),
            &[verdict("file_exists:a.md", false)],
        );
        assert!(r.is_none(), "没有收尾 JSON 时不应伪造");
    }

    /// 交叉核对：没派发过的标准、重复上报，都不得灌满进度
    #[test]
    fn progress_ignores_undeclared_and_duplicate_criteria() {
        let declared = vec![
            "覆盖至少 5 个产品".to_string(),
            "每条含官网 URL".to_string(),
        ];
        // 虚报一条从未派发的标准 + 重复报同一条 → 只应计 1
        let reported = vec![
            serde_json::json!({"criterion": "覆盖至少 5 个产品", "met": true}),
            serde_json::json!({"criterion": "覆盖至少 5 个产品", "met": true}),
            serde_json::json!({"criterion": "我其实做了 10 个产品", "met": true}),
        ];
        assert_eq!(count_met_against_declared(&declared, &reported), 1);
        // met 非 bool / 缺 criterion 的条目不计
        assert_eq!(
            count_met_against_declared(
                &declared,
                &[serde_json::json!({"criterion": "每条含官网 URL", "met": "yes"})]
            ),
            0
        );
        assert_eq!(
            count_met_against_declared(&declared, &[serde_json::json!({"met": true})]),
            0
        );
        // 白空格差异不算漏报（模型可能顺手加了空格）
        assert_eq!(
            count_met_against_declared(
                &declared,
                &[serde_json::json!({"criterion": " 每条含官网 URL ", "met": true})]
            ),
            1
        );
    }

    #[test]
    fn verifier_row_progress_counts_checks_array_not_zero() {
        // verifier 行的收尾 JSON 是 checks 数组（verifier 独有 schema）：
        // done 若只认 acceptance_check，验收全过的 verifier 行也显示 0/N，
        // 主 agent 会误判「没做成」而重派
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"checks":[{"criterion":"覆盖至少 5 个产品","met":true},{"criterion":"每条含官网 URL","met":true}]}"#,
        )
        .unwrap();
        let state = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(state.progress["subtasksDone"], 2);
        assert_eq!(state.progress["subtasksTotal"], 2);
    }

    #[test]
    fn check_unknown_key_is_invalid_argument() {
        let conn = test_conn();
        let err = check_subagent_locked(&conn, "sa_ghost").unwrap_err();
        assert!(matches!(err, CommandError::InvalidArgument { .. }));
    }

    #[test]
    fn check_returns_parsed_result_json() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
        crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"summary":"结论","confidence":0.9}"#,
        )
        .unwrap();
        crate::db::update_subagent_status(
            &conn,
            &ack.subagent_id,
            SubagentStatus::Running,
            5_100,
            None,
        )
        .unwrap();
        crate::db::update_subagent_status(
            &conn,
            &ack.subagent_id,
            SubagentStatus::Succeeded,
            5_200,
            None,
        )
        .unwrap();
        let state = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
        assert_eq!(state.status, SubagentStatus::Succeeded);
        assert_eq!(state.result.unwrap()["summary"], "结论");
    }

    // stage enum 字符串映射（设计 §8.5；调用方传入 stage=recon/executor/verifier）
    #[test]
    fn subagent_stage_roundtrips_string() {
        for (s, expected) in [
            (SubagentStage::Recon, "recon"),
            (SubagentStage::Executor, "executor"),
            (SubagentStage::Verifier, "verifier"),
        ] {
            assert_eq!(s.as_str(), expected);
            assert_eq!(SubagentStage::from_str(expected), Some(s));
        }
        assert_eq!(SubagentStage::from_str("unknown"), None);
    }

    // stage → 默认 profile 派生（设计 §8.5 映射）
    #[test]
    fn stage_default_profile_is_research_or_coder() {
        // recon/verifier 只读足够 → research
        assert_eq!(
            SubagentStage::Recon.default_profile(),
            SubagentProfile::Research
        );
        assert_eq!(
            SubagentStage::Verifier.default_profile(),
            SubagentProfile::Research
        );
        // executor 写类必需 → coder
        assert_eq!(
            SubagentStage::Executor.default_profile(),
            SubagentProfile::Coder
        );
    }

    // spawn 行上 profile 仍按显式值落库（stage 不进 DB 表）
    #[test]
    fn stage_is_not_persisted_and_profile_field_unaffected() {
        let mut conn = test_conn();
        let _g = crate::db::lock_db_write();
        let mut r = req(None);
        r.profile = SubagentProfile::Coder;
        r.stage = Some(SubagentStage::Executor);
        let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap();
        // 行 profile 是显式给的 Coder（stage 在内存里走闭包，不落表）
        assert_eq!(row.profile, SubagentProfile::Coder);
    }

    // ── per-onion 配置档接线（期 6）──

    /// profile 解析优先级：显式 > 配置档 > stage 派生，都没给则 None。
    ///
    /// 「配置档 > stage」这一档是配置档 profile 唯一的生效路径——它排在
    /// 显式之后，才不会让 yaml 覆盖运行时档位（那等于放宽）。
    #[test]
    fn spawn_profile_priority_is_explicit_then_config_then_stage() {
        let e = Some(SubagentProfile::General);
        let c = Some(SubagentProfile::Coder);
        // ① 显式压过配置档与 stage（防「配置覆盖运行时 profile = 放宽」）
        assert_eq!(
            resolve_spawn_profile(e, c, Some(SubagentStage::Executor)),
            Some(SubagentProfile::General)
        );
        // ② 没给显式 → 配置档兜底，压过 stage
        assert_eq!(
            resolve_spawn_profile(None, c, Some(SubagentStage::Executor)),
            Some(SubagentProfile::Coder)
        );
        // ③ 只有 stage → 按 stage 派生
        assert_eq!(
            resolve_spawn_profile(None, None, Some(SubagentStage::Verifier)),
            Some(SubagentProfile::Research)
        );
        // ④ 三者皆无 → None，调用方据此报错（不许猜一个默认档）
        assert_eq!(resolve_spawn_profile(None, None, None), None);
    }

    #[test]
    fn onion_turn_cap_only_tightens() {
        let mut b = SubagentBudget {
            max_turns: 30,
            max_tool_calls: 100,
            max_wall_seconds: 600,
        };
        // 配置档更紧 → 压到配置档的值
        apply_onion_turn_cap(&mut b, Some(4));
        assert_eq!(b.max_turns, 4);
        // 配置档更松 → 不许被它顶开（配置档是上限，不是覆盖）
        apply_onion_turn_cap(&mut b, Some(50));
        assert_eq!(b.max_turns, 4, "配置档比派发值宽时不应放宽轮数");
        // 没给上限 → 不动
        apply_onion_turn_cap(&mut b, None);
        assert_eq!(b.max_turns, 4);
    }

    /// 接线守卫：配置档从派发到执行的四段链路，一段都不能靠单测推断。
    ///
    /// 四段分别是「spawn 解析 → 内存闭包进 runner → 建会话写两列 →
    /// bind 出的交集进守卫白名单」。缺任一段的症状都是同一个：模型以为
    /// 派了个受限子 agent，实际跑的是全档位——纯函数单测全绿也照样漏。
    ///
    /// **所有匹配串都用 `concat!` 拆开**：本测试就住在被扫描的同一个文件里，
    /// 裸写字面量会被测试自己的源码匹配上，删掉真接线照样绿（本条守卫第一版
    /// 就是这么假绿的）。与 `tool_guard::dead_command_bind_file_not_in_dispatcher` 同款。
    #[test]
    fn onion_config_is_wired_from_spawn_to_guard_whitelist() {
        let body = include_str!("bot_orchestrator.rs");

        // ① 派发层解析出配置档并装进 SpawnRequest
        let parse_arm = body
            .split_once(concat!("crate::bot_onions::", "load(app, id)"))
            .expect("派发层必须先加载配置档再建行")
            .1
            .split("parent_session_id: session_id,")
            .next()
            .expect("SpawnRequest 构造应在 session_id 之前");
        assert!(
            parse_arm.contains(concat!("onion: ", "onion_cfg")),
            "SpawnRequest 必须带上已解析的配置档：{parse_arm}"
        );

        // ② 配置档随内存闭包进 runner（不落 subagents 行）
        assert!(
            body.contains(concat!("let runner_onion = ", "req.onion.clone();")),
            "配置档必须从 req 摘出来交给 runner，否则 runner 拿不到配置名"
        );
        assert!(
            body.contains(concat!(
                "run_subagent(runner_app, runner_sid, ",
                "runner_stage, runner_onion)"
            )),
            "run_subagent 必须收到配置档参数"
        );

        // ③ 建会话时把配置写进 bot_sessions 两列——不写则 bind_session 无从读起
        let setup = body
            .split_once(concat!("bot_session_", "set_onion("))
            .expect("子会话建会话时必须写配置档两列")
            .1;
        assert!(
            setup.contains(concat!("Some(&", "o.config)")),
            "要写配置档名（否则 bind 读到空指针，配置静默失效）"
        );

        // ④ 守卫白名单取交集，且 bind 发生在守卫注册之前
        let bind_at = body
            .find(concat!(
                "crate::bot_onions::",
                "bind_session(&app, &session_id)"
            ))
            .expect("runner 必须绑定配置");
        let guard_at = body
            .find("SubagentSessionGuard::new(")
            .expect("runner 必须注册守卫");
        assert!(
            bind_at < guard_at,
            "bind_session 必须在守卫注册之前：守卫的白名单取自绑定后的交集，\
             顺序反了等于拿 profile 白名单当交集"
        );
        let guard_arm = body.split_at(guard_at).1;
        let guard_arm = &guard_arm[..guard_arm.find(");").unwrap_or(guard_arm.len())];
        assert!(
            guard_arm.contains(concat!("tools: std::sync::Arc::", "new(subagent_tools)")),
            "守卫必须带交集后的工具集，闸才是按收窄判的：{guard_arm}"
        );
    }

    /// 接线守卫：配置档的角色与实际可用工具要进子 agent 的 system 提示词。
    ///
    /// 少了这段，配置档对模型就是「工具莫名少了一半」——它会反复调被收掉的
    /// 工具、每次吃一次白名单拒绝，白烧轮数。匹配串同样 `concat!` 拆开。
    #[test]
    fn onion_prompt_note_is_injected_into_subagent_system() {
        let body = include_str!("bot_orchestrator.rs");
        assert!(
            body.contains(concat!(
                "crate::bot_onions::",
                "render_prompt_note(cfg, row.profile)"
            )),
            "system 拼装必须注入配置档提示词段"
        );
        let system_at = body
            .find("let system = format!(")
            .expect("子 agent system 拼装点");
        let arm = &body[system_at..];
        let arm = &arm[..arm.find("let msgs = vec![").unwrap_or(arm.len())];
        assert!(
            arm.contains("onion_note"),
            "system 格式串必须带上配置档段，无配置档时为空串占位：{arm}"
        );
    }
}
