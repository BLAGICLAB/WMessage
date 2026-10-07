//! 工作流执行引擎（W3-RUNNER，设计 docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md §8）：
//! 拓扑调度——**就绪即跑**（LLMCompiler 模型：某节点前驱全部完成即刻启动，非整层屏障），
//! 失败传播 = Airflow `all_success` 默认语义（上游失败 → 传递下游标 ⏭ 跳过，无关节点照跑），
//! 断点续跑 = 任务卡即节点的架构红利（done+success 的节点直接视为已完成）。
//!
//! 复用清单（设计红线「不另造」）：并发闸 = SubagentGate；执行 = run_task_in_chat
//! （ExecGuard 防重入 / 30min 超时 / 预算 / 会话即记录全部继承）；
//! 通知 = 调度器 notify 同款；⏭ 跳过写 note = 调度器 ⏰ 摘要前置同款 RMW 合并。
//!
//! 停止语义（设计 §8.1）：workflow_stop 取消**未启动**队列（含在 Gate 排队的
//! —— wait_slot_cancellable 感知取消）；运行中会话不中断，走既有 StopGuard /stop 通道。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

use crate::bot_chat::{run_task_in_chat_ctx, TaskExecCtx, TaskExecOrigin};
use crate::db::{Task, TaskStatus};
use crate::error::{CommandError, CommandResult};

// ────────────── 纯逻辑：建图 / 判定 / 跳过闭包（单测锚点） ──────────────

/// 拆好的执行图。indegree 只计**未完成**的上游（done+success 节点在启动时已解锁下游）。
pub(crate) struct Dag {
    /// 下游入度（不含已完成上游的边）
    pub indegree: HashMap<String, usize>,
    /// 上游 id → 直接下游 id 列表
    pub dependents: HashMap<String, Vec<String>>,
    /// **将执行**节点 id（done+success 已排除——它们不上报终态，
    /// 计入 total 会让控制器 recv 永久等待，W4 全量对照 critical 级教训）
    pub nodes: Vec<String>,
    /// 入度 0 的初始就绪集（稳定序：按 tasks 输入顺序）
    pub ready: Vec<String>,
}

/// 建图（纯逻辑，单测锚点）：环 → Err；悬空依赖（指向不存在/非本图节点）→ Err。
/// done+success 节点的边不计入入度——它们启动即视为已解析（断点续跑）。
pub(crate) fn build_dag(tasks: &[Task], is_success: &dyn Fn(&Task) -> bool) -> Result<Dag, String> {
    let ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    // 入度统计要按 id 取节点判成功态，先建索引避免每条依赖线性扫全表
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut indegree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    let mut ready: Vec<String> = Vec::new();
    for t in tasks {
        indegree.entry(t.id.clone()).or_insert(0);
    }
    for t in tasks {
        if is_success(t) {
            continue; // 断点续跑：完成节点无入度，直接就绪
        }
        let mut deps: HashSet<&str> = HashSet::new();
        for d in t.depends_on.iter().flatten() {
            if !ids.contains(d.as_str()) {
                return Err(format!(
                    "任务「{}」的依赖 {} 不在工作流内（悬空引用）",
                    t.title, d
                ));
            }
            if !deps.insert(d) {
                continue; // 重复边去重
            }
            dependents
                .entry(d.to_string())
                .or_default()
                .push(t.id.clone());
        }
        let deg = deps
            .iter()
            .filter(|d| by_id.get(*d).map(|up| !is_success(up)).unwrap_or(false))
            .count();
        indegree.insert(t.id.clone(), deg);
        if deg == 0 {
            ready.push(t.id.clone());
        }
    }
    // 防御性环检测：Kahn 拓扑计数——能被拓扑归约的活跃节点数 < 活跃总数 ⇒ 有环。
    // done 节点视为已解析，不参与计数（其入度条目恒 0 且不入队）
    let total_active = tasks.iter().filter(|t| !is_success(t)).count();
    let mut deg = indegree.clone();
    let mut queue: Vec<String> = ready.clone();
    let mut processed = queue.len();
    while let Some(n) = queue.pop() {
        let downstream = dependents.get(&n).cloned().unwrap_or_default();
        for d in downstream {
            if let Some(v) = deg.get_mut(&d) {
                if *v > 0 {
                    *v -= 1;
                    if *v == 0 {
                        processed += 1;
                        queue.push(d.clone());
                    }
                }
            }
        }
    }
    if processed < total_active {
        return Err("工作流存在循环依赖，无法执行".into());
    }
    Ok(Dag {
        indegree,
        dependents,
        nodes: tasks
            .iter()
            .filter(|t| !is_success(t))
            .map(|t| t.id.clone())
            .collect(),
        ready,
    })
}

/// 节点成功判定（单测锚点）：column=done 且 result.status ∈ {缺失, success}。
/// 与画布节点描边（TaskNode::nodeBorder）保持同一语义——done 无 result = 用户手动完成。
pub(crate) fn node_is_success(t: &Task) -> bool {
    if t.column != TaskStatus::Done {
        return false;
    }
    match t
        .result
        .as_ref()
        .and_then(|r| r.get("status"))
        .and_then(|s| s.as_str())
    {
        None | Some("success") => true,
        _ => false,
    }
}

/// 失败传播闭包（单测锚点）：failed 的全部传递下游（BFS，不含 failed 自身），
/// 调用方负责排除已解析节点。
pub(crate) fn skip_closure(failed: &str, dependents: &HashMap<String, Vec<String>>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack = vec![failed.to_string()];
    while let Some(cur) = stack.pop() {
        for d in dependents.get(&cur).into_iter().flatten() {
            if seen.insert(d.clone()) {
                out.push(d.clone());
                stack.push(d.clone());
            }
        }
    }
    out
}

// ────────────── W-QA：结构化交接 / 证据结果 / 重试决策（纯逻辑，单测锚点） ──────────────

/// 上游简报单卡上限（Anthropic 多 agent 实战教训：交接只传压缩摘要，不传全文）
const UPSTREAM_PER_CAP: usize = 600;
/// 上游简报总上限
const UPSTREAM_TOTAL_CAP: usize = 2400;

// ────────────── W10：节点级验收（设计 §4.1，纯逻辑单测锚点） ──────────────

/// 验收返工独立预算（不与失败重试 C1 混用；拍板 4：status 与 verdict 分离）
pub(crate) const ACCEPTANCE_REWORK_BUDGET: u32 = 2;

/// 验收裁决四值；unknown = 评审调用/解析失败降级（不阻断、不返工）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptanceVerdict {
    Pass,
    Partial,
    Fail,
    Unknown,
}

impl AcceptanceVerdict {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            AcceptanceVerdict::Pass => "pass",
            AcceptanceVerdict::Partial => "partial",
            AcceptanceVerdict::Fail => "fail",
            AcceptanceVerdict::Unknown => "unknown",
        }
    }
}

/// 验收输出解析（纯逻辑，单测锚点）：剥 fences → JSON verdict 归一；
/// 契约外字符串/坏 JSON → Unknown（照 parse_review_report 降级口径）。
pub(crate) fn parse_acceptance_verdict(raw: &str) -> AcceptanceVerdict {
    let text = crate::workflow_decompose::strip_fences(raw);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return AcceptanceVerdict::Unknown;
    };
    match v.get("verdict").and_then(serde_json::Value::as_str) {
        Some("pass") => AcceptanceVerdict::Pass,
        Some("partial") => AcceptanceVerdict::Partial,
        Some("fail") => AcceptanceVerdict::Fail,
        _ => AcceptanceVerdict::Unknown,
    }
}

/// 验收证据提取（纯逻辑）：evidence ≤100 字截断；缺失/非串 → 空串
pub(crate) fn parse_acceptance_evidence(raw: &str) -> String {
    let text = crate::workflow_decompose::strip_fences(raw);
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return String::new();
    };
    v.get("evidence")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .map(|s| s.chars().take(100).collect())
        .unwrap_or_default()
}

/// 返工决策（纯逻辑，单测锚点）：fail 且还有余量 → 返工；
/// fail 余量用尽 → 终态 failed（下游跳过）；partial/unknown/pass → 带结果继续（status 不变）。
pub(crate) fn acceptance_rework_decision(
    verdict: AcceptanceVerdict,
    rework_left: u32,
) -> AcceptanceAction {
    match verdict {
        AcceptanceVerdict::Fail if rework_left > 0 => AcceptanceAction::Rework,
        AcceptanceVerdict::Fail => AcceptanceAction::Fail,
        _ => AcceptanceAction::Accept,
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum AcceptanceAction {
    /// 验收通过/部分通过/降级未知：status 保持 success，带徽标继续
    Accept,
    /// 带 evidence 重跑本卡（attempt 递增，独立预算）
    Rework,
    /// 预算用尽仍 fail：终态改 failed（下游照现语义跳过）+ 审计 Warn
    Fail,
}

/// 验收评审提示词（契约硬约束在 system 侧）：宁缺毋滥——
/// partial 留给"有缺口但不影响下游使用"，明确不达标才 fail。
const ACCEPTANCE_SYSTEM_PROMPT: &str = "\
你是工作流节点验收员。对照该任务的验收标准，判断产出摘要与产物文件是否达标。\
只输出一个 JSON 对象，不要输出任何解释或 Markdown 围栏，形如：\
{\"verdict\":\"pass|partial|fail\",\"evidence\":\"≤100字依据\"}\
verdict：pass=达标；partial=有缺口但不影响下游使用；fail=明确不达标。\
判定从紧：只有产出与验收标准明确冲突时才 fail；无法判断时用 partial，不要臆测。";

/// 验收核查单发调用（无会话无工具；照 clarify/收尾评审同款样板）。
/// 返回 (verdict, evidence)；调用失败 → (Unknown, "")，不阻断。
async fn check_acceptance(
    app: &AppHandle,
    title: &str,
    acceptance: &str,
    summary: &str,
    artifacts: &[String],
) -> (AcceptanceVerdict, String) {
    let mut user = format!("任务：{title}\n验收标准：{acceptance}");
    if !summary.trim().is_empty() {
        user.push_str(&format!(
            "\n产出摘要：{}",
            summary.chars().take(300).collect::<String>()
        ));
    }
    if !artifacts.is_empty() {
        user.push_str(&format!("\n产物文件：{}", artifacts.join("；")));
    }
    match crate::bot_chat::summarize_messages(
        app,
        ACCEPTANCE_SYSTEM_PROMPT,
        &[crate::bot_chat::ChatMsg {
            role: "user".into(),
            content: user,
        }],
    )
    .await
    {
        Ok(raw) => (
            parse_acceptance_verdict(&raw),
            parse_acceptance_evidence(&raw),
        ),
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "workflow_acceptance",
                &[
                    ("outcome", "call_failed".into()),
                    ("error", crate::audit::escape_for_log(&e.message(), 120)),
                ],
            );
            (AcceptanceVerdict::Unknown, String::new())
        }
    }
}

// ────────────── W10：run 级审计落库（尽力而为，尽力而为——写失败不阻断执行） ──────────────

/// 审计表写入 helper（spawn_blocking + 失败 eprintln；bot.log 双写由调用方自行决定）
async fn wa_log(
    app: &AppHandle,
    workflow_id: &str,
    run_started_at: i64,
    node_task_id: Option<&str>,
    kind: &str,
    level: crate::audit::AuditLevel,
    payload: serde_json::Value,
) {
    let app2 = app.clone();
    let wid = workflow_id.to_string();
    let nid = node_task_id.map(|s| s.to_string());
    let kind = kind.to_string();
    let level_str = match level {
        crate::audit::AuditLevel::Info => "info",
        crate::audit::AuditLevel::Warn => "warn",
        crate::audit::AuditLevel::Error => "error",
    };
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
        crate::db::workflow_audit::wa_insert(
            &conn,
            &wid,
            run_started_at,
            nid.as_deref(),
            &kind,
            level_str,
            &payload,
        )
        .map(|_| ())
    })
    .await;
    match r {
        Ok(Ok(())) => {}
        Ok(Err(e)) => eprintln!("[workflow_audit] 写入失败（不阻断）：{e}"),
        Err(e) => eprintln!("[workflow_audit] 审计线程 join 失败（不阻断）：{e}"),
    }
}

/// 单个上游卡的简报行：标题/状态/验收标准/产出摘要/产物文件（typed-schema handoff）
fn upstream_line(t: &Task) -> String {
    let status = if node_is_success(t) {
        "✅ 完成"
    } else {
        "⚠️ 未成功"
    };
    let mut line = format!("- 「{}」{status}", t.title);
    if let Some(acc) = t.acceptance.as_deref().filter(|a| !a.trim().is_empty()) {
        line.push_str(&format!("\n  验收标准：{acc}"));
    }
    if let Some(r) = &t.result {
        if let Some(s) = r.get("summary").and_then(|v| v.as_str()) {
            let summary: String = s.trim().chars().take(UPSTREAM_PER_CAP).collect();
            if !summary.is_empty() {
                line.push_str(&format!("\n  产出摘要：{summary}"));
            }
        }
    }
    let bound = t.effective_files();
    if !bound.is_empty() {
        line.push_str(&format!(
            "\n  产物文件：{}",
            bound
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    line
}

/// 直接上游简报装配（纯逻辑，单测锚点）：总 ≤{UPSTREAM_TOTAL_CAP} 字，超出截断并标注
pub(crate) fn upstream_brief(upstreams: &[&Task]) -> Option<String> {
    if upstreams.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut used = 0usize;
    for t in upstreams {
        let remaining = UPSTREAM_TOTAL_CAP.saturating_sub(used);
        if remaining == 0 {
            out.push_str("\n- （上游过多，后续简报已截断）");
            break;
        }
        let clipped: String = upstream_line(t).chars().take(remaining).collect();
        used += clipped.chars().count();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&clipped);
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 节点收尾结构化结果（纯逻辑，单测锚点，W-QA A2）：完成判定从「模型自律」升级为
/// 「引擎写证据」。status：熔断→failed；循环错误→failed；卡片已标完成→success；
/// 循环正常返回但未标完成→incomplete（模型漏调 complete_task，可归因）。
pub(crate) fn build_node_result(
    column_done: bool,
    fused: bool,
    reply_text: Option<&str>,
    error: Option<&str>,
    artifacts: &[String],
    attempt: u32,
) -> serde_json::Value {
    let (status, err) = if fused {
        ("failed", Some("⏹ 熔断：调用工具达上限被停止".to_string()))
    } else if let Some(e) = error {
        ("failed", Some(crate::bot::truncate_for_log(e, 200)))
    } else if column_done {
        ("success", None)
    } else {
        (
            "incomplete",
            Some("循环正常结束但模型未调用 complete_task 标记完成".to_string()),
        )
    };
    let summary: String = reply_text.unwrap_or("").trim().chars().take(300).collect();
    let mut v = serde_json::json!({
        "status": status,
        "summary": summary,
        "attempt": attempt,
        "engine": "runner",
        "finishedAt": chrono::Utc::now().timestamp_millis(),
    });
    if let Some(e) = err {
        v["error"] = serde_json::Value::String(e);
    }
    if !artifacts.is_empty() {
        v["artifacts"] = serde_json::json!(artifacts);
    }
    v
}

/// 失败自动重试决策（纯逻辑，单测锚点，C1）：非取消、非熔断且还有余量才重试。
/// 熔断不自动重试——工具上限没调，重试必然再熔断，留给人工调参后续跑（W5 语义）。
pub(crate) fn should_retry(ok: bool, cancelled: bool, fused: bool, retries_left: u32) -> bool {
    !ok && !cancelled && !fused && retries_left > 0
}

/// 评审报告（W-QA C2，rubric 结构化裁决 / LLM-as-Judge）
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewReport {
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub overall: String,
    #[serde(default)]
    pub issues: Vec<ReviewIssue>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewIssue {
    #[serde(default)]
    pub node_title: String,
    #[serde(default)]
    pub problem: String,
    #[serde(default)]
    pub needs_rework: bool,
}

/// 评审输出解析（纯逻辑，单测锚点）：fences 剥离 → JSON；
/// 解析失败降级为纯文本报告（评审内容不丢，返工环自然不触发）。
pub(crate) fn parse_review_report(raw: &str) -> serde_json::Value {
    let text = crate::workflow_decompose::strip_fences(raw);
    match serde_json::from_str::<ReviewReport>(text) {
        Ok(r) => serde_json::to_value(&r)
            .unwrap_or_else(|_| serde_json::json!({"verdict": "unknown", "overall": raw})),
        Err(_) => serde_json::json!({ "verdict": "unknown", "overall": raw.trim(), "issues": [] }),
    }
}

/// 收尾评审提示词（W-QA C2）：对照总目标逐节点核查 + 整体一致性（子任务都对但
/// 拼起来不成立是重点检查项）。输出契约硬约束在 system 侧，用户改不着。
const REVIEW_SYSTEM_PROMPT: &str = "\
你是工作流质量审校员。对照工作流总目标，逐节点核查产出：验收标准是否达成、各节点产出是否一致连贯。\
重点检查「每个子任务单独看都对、拼在一起不成立」的整体性缺口。\
只输出一个 JSON 对象，不要输出任何解释或 Markdown 围栏，形如：\
{\"verdict\":\"pass|partial|fail\",\"overall\":\"两三句整体结论\",\"issues\":[{\"nodeTitle\":\"节点标题\",\"problem\":\"具体问题\",\"needsRework\":false}]}\
verdict：pass=全部达标且整体连贯；partial=有小缺口但不影响整体；fail=整体未达成。\
issues：只列有问题的节点，没有问题则为 []；needsRework=true 仅当该节点明确不达标需要重做（宁缺毋滥）。";

// ────────────── 运行注册表 / 取消 ──────────────

struct RunHandle {
    cancel: Arc<AtomicBool>,
    /// W10：run 分组键的一半（审计表按 (workflow_id, run_started_at) 聚合；
    /// stop 事件凭它定位自己属于哪次 run）
    run_started_at: i64,
}

/// 工作流 → 在跑实例（workflow_run 防重入 + workflow_stop 取消源）
fn runs() -> &'static Mutex<HashMap<String, Arc<RunHandle>>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Arc<RunHandle>>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registry_poisoned() -> CommandError {
    CommandError::Internal("运行注册表中毒".into())
}

pub(crate) fn runner_is_running(workflow_id: &str) -> bool {
    runs()
        .lock()
        .map(|m| m.contains_key(workflow_id))
        .unwrap_or(false)
}

// ────────────── 节点终态上报 ──────────────

struct NodeOutcome {
    id: String,
    /// true = 成功（解锁下游）；false = 失败（触发跳过传播）
    ok: bool,
    /// 取消（在 Gate 排队时被取消）：不计失败，控制器按「已停止」归档
    cancelled: bool,
    /// 熔断（Function 调用超上限）：失败的一种，但归因备注不同（W5-FUSE）
    fused: bool,
}

/// 节点任务的兜底上报守卫：任务体在发出终态前提前退栈（如 panic）时，
/// Drop 补发一条失败终态，防止控制器按「还有节点未终态」永久等待。
struct OutcomeFallback {
    tx: tokio::sync::mpsc::UnboundedSender<NodeOutcome>,
    id: String,
    /// 正常路径发完终态后置 true，Drop 即成 no-op
    sent: bool,
}

impl Drop for OutcomeFallback {
    fn drop(&mut self) {
        if !self.sent {
            let _ = self.tx.send(NodeOutcome {
                id: std::mem::take(&mut self.id),
                ok: false,
                cancelled: false,
                fused: false,
            });
        }
    }
}

// ────────────── 命令 ──────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunStart {
    pub total: usize,
    /// 断点续跑直接视为已完成的节点数
    pub already_done: usize,
    /// 本次将执行的节点数
    pub to_run: usize,
}

async fn load_workflow_tasks(app: &AppHandle, workflow_id: &str) -> CommandResult<Vec<Task>> {
    let app = app.clone();
    let wid = workflow_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app)?;
        crate::db::tasks::load_tasks_by_workflow(&conn, &wid).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流节点读取线程 join 失败：{e}")))?
}

/// 开始执行整张工作流（断点续跑语义；已在跑 → 拒绝）
/// 触发一次工作流执行（前端按钮 / 定时调度器；trigger 只进审计，不影响执行语义）
#[tauri::command]
pub async fn workflow_run(
    app: AppHandle,
    workflow_id: String,
    trigger: Option<String>,
) -> CommandResult<WorkflowRunStart> {
    let trigger = match trigger.as_deref() {
        Some("schedule") => "schedule",
        _ => "manual",
    };
    {
        let m = runs().lock().map_err(|_| registry_poisoned())?;
        if m.contains_key(&workflow_id) {
            return Err(CommandError::TaskInvalidState {
                reason: "该工作流已在执行中".into(),
            });
        }
    }
    let tasks = load_workflow_tasks(&app, &workflow_id).await?;
    if tasks.is_empty() {
        return Err(CommandError::TaskInvalidState {
            reason: "工作流没有任何节点卡".into(),
        });
    }
    let dag = build_dag(&tasks, &node_is_success).map_err(CommandError::Internal)?;
    let already_done = tasks.iter().filter(|t| node_is_success(t)).count();
    let to_run = tasks.len() - already_done;

    let cancel = Arc::new(AtomicBool::new(false));
    // W10：run 分组键（审计表 + RunHandle 共享同一时刻戳）
    let run_started_at = chrono::Utc::now().timestamp_millis();
    runs().lock().map_err(|_| registry_poisoned())?.insert(
        workflow_id.clone(),
        Arc::new(RunHandle {
            cancel: cancel.clone(),
            run_started_at,
        }),
    );
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "workflow_run",
        &[
            ("workflowId", workflow_id.clone()),
            ("total", tasks.len().to_string()),
            ("alreadyDone", already_done.to_string()),
            ("toRun", to_run.to_string()),
        ],
    );

    let name_by_id: HashMap<String, String> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.title.clone()))
        .collect();
    // 每卡模型覆盖（W6-MODEL）：task.model = 模型库条目 id
    let model_by_id: HashMap<String, Option<String>> = tasks
        .iter()
        .map(|t| (t.id.clone(), t.model.clone()))
        .collect();
    // 直接上游表（任务 id → 它自己的依赖列表）——spawn 前据此装配上游产出简报；
    // 注意方向不能反：查「本节点的上游」必须以本节点 id 为 key
    let mut upstream_of: HashMap<String, Vec<String>> = HashMap::new();
    for t in &tasks {
        if let Some(deps) = &t.depends_on {
            upstream_of
                .entry(t.id.clone())
                .or_default()
                .extend(deps.iter().cloned());
        }
    }
    // W-QA B1：工作流总目标（此前执行期根本不读，goal 只是画布元数据）
    let goal = load_workflow_goal(&app, &workflow_id).await;
    // W9-ASK：执行提问开关（clarify_meta.askMode，默认开）——run 开始时读一次
    let asks_enabled = load_asks_enabled(&app, &workflow_id).await;
    // W10：节点级验收开关（workflow_settings，默认开）——run 开始时读一次
    let acceptance_enabled = {
        let app2 = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            crate::db::open_db(&app2)
                .map(|conn| crate::db::workflow_settings::node_acceptance_enabled(&conn))
                .unwrap_or(crate::db::workflow_settings::DEFAULT_NODE_ACCEPTANCE)
        })
        .await
        .unwrap_or(crate::db::workflow_settings::DEFAULT_NODE_ACCEPTANCE)
    };
    // W10：run_start 审计行（goal 摘要 ≤80 字/节点数/trigger）
    wa_log(
        &app,
        &workflow_id,
        run_started_at,
        None,
        crate::db::workflow_audit::KIND_RUN_START,
        crate::audit::AuditLevel::Info,
        serde_json::json!({
            "trigger": trigger,
            "total": tasks.len(),
            "alreadyDone": already_done,
            "goal": goal.as_deref().map(|g| g.chars().take(80).collect::<String>()),

        }),
    )
    .await;
    let app2 = app.clone();
    let wf = workflow_id.clone();
    tauri::async_runtime::spawn(async move {
        run_controller(
            app2,
            wf,
            dag,
            name_by_id,
            model_by_id,
            upstream_of,
            goal,
            asks_enabled,
            acceptance_enabled,
            run_started_at,
            cancel,
        )
        .await;
    });
    Ok(WorkflowRunStart {
        total: tasks.len(),
        already_done,
        to_run,
    })
}

/// 停止工作流：取消未启动队列（在 Gate 排队的节点任务由 wait_slot_cancellable 感知）。
/// 运行中会话不中断——走既有 StopGuard /stop 通道（设计 §8.1）。
#[tauri::command]
pub async fn workflow_stop(app: AppHandle, workflow_id: String) -> CommandResult<bool> {
    let handle = runs()
        .lock()
        .map_err(|_| registry_poisoned())?
        .get(&workflow_id)
        .cloned();
    match handle {
        Some(h) => {
            h.cancel.store(true, Ordering::SeqCst);
            crate::audit::write_event(
                &app,
                crate::audit::AuditLevel::Info,
                "workflow_stop",
                &[("workflowId", workflow_id.clone())],
            );
            // W10：stop 审计行（凭 RunHandle.run_started_at 归组到本次 run）
            wa_log(
                &app,
                &workflow_id,
                h.run_started_at,
                None,
                crate::db::workflow_audit::KIND_STOP,
                crate::audit::AuditLevel::Info,
                serde_json::json!({ "by": "user" }),
            )
            .await;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// 前端恢复执行态用（导航回来后按钮状态取真）
#[tauri::command]
pub async fn workflow_is_running(workflow_id: String) -> bool {
    runner_is_running(&workflow_id)
}

// ────────────── 控制器 ──────────────

async fn run_controller(
    app: AppHandle,
    workflow_id: String,
    dag: Dag,
    name_by_id: HashMap<String, String>,
    model_by_id: HashMap<String, Option<String>>,
    upstream_of: HashMap<String, Vec<String>>,
    goal: Option<String>,
    // W9-ASK：执行提问开关（workflow_run 开始时按 clarify_meta.askMode 读出）
    asks_enabled: bool,
    // W10：节点级验收开关（workflow_settings，默认开）
    acceptance_enabled: bool,
    // W10：run 分组键（审计表）
    run_started_at: i64,
    cancel: Arc<AtomicBool>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<NodeOutcome>();
    let mut indegree = dag.indegree.clone();
    let mut resolved: HashSet<String> = HashSet::new();
    // 在跑集合：跨闭包共享（Arc<Mutex>），取消收尾时区分「在跑」与「从未启动」
    let running: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    // C1：每节点剩余自动重试次数（初始 1；熔断除外——should_retry 裁决）
    let mut retries_left: HashMap<String, u32> =
        dag.nodes.iter().map(|id| (id.clone(), 1)).collect();
    // A2/C1：attempt 计数（跨重试累计，写进 result 证据链）
    let attempts: Arc<Mutex<HashMap<String, u32>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut failed_names: Vec<String> = Vec::new();
    // C2：评审需要失败/跳过名单（终态归因，凭卡面 note 前缀判断不可靠）
    let mut failed_ids: HashSet<String> = HashSet::new();
    let mut skipped_ids: HashSet<String> = HashSet::new();
    let mut done_count = 0usize;
    let total = dag.nodes.len();

    let spawn_node = {
        let tx = tx.clone();
        let running = running.clone();
        let model_by_id = model_by_id.clone();
        let upstream_of = upstream_of.clone();
        let goal = goal.clone();
        let attempts = attempts.clone();
        let workflow_id = workflow_id.clone();
        move |app: &AppHandle, cancel: &Arc<AtomicBool>, id: String| {
            let app = app.clone();
            let tx = tx.clone();
            let cancel = cancel.clone();
            let running = running.clone();
            let model = model_by_id.get(&id).cloned().flatten();
            let ups = upstream_of.get(&id).cloned().unwrap_or_default();
            let goal = goal.clone();
            // W9-ASK：ask 授权与档案注入需要 workflow_id——async move 块按 move
            // 捕获会吞掉闭包捕获的原本体（Fn 退化 FnOnce），同 goal 先克隆一份
            let workflow_id = workflow_id.clone();
            let attempts = attempts.clone();
            if let Ok(mut r) = running.lock() {
                r.insert(id.clone());
            }
            tauri::async_runtime::spawn(async move {
                // 崩溃兜底：正常发完终态置 sent，panic 提前退栈时由 Drop 补发失败终态
                let mut fallback = OutcomeFallback {
                    tx: tx.clone(),
                    id: id.clone(),
                    sent: false,
                };
                // P1-d：节点进入执行即广播（Gate 排队视同 running；前端画布实时高亮）。
                // 5s 轮询保留为兜底，事件只做低延迟增量。
                let _ = app.emit(
                    "workflow-node-status",
                    serde_json::json!({ "taskId": id.clone(), "status": "running" }),
                );
                let ticket =
                    crate::bot_orchestrator::SubagentGate::wait_slot_cancellable(None, || {
                        cancel.load(Ordering::SeqCst)
                    })
                    .await;
                if ticket.is_none() {
                    // 在 Gate 排队时被取消：不计失败
                    let _ = tx.send(NodeOutcome {
                        id,
                        ok: false,
                        cancelled: true,
                        fused: false,
                    });
                    fallback.sent = true;
                    return;
                }
                let _ticket = ticket; // RAII 占槽：任务结束自动释放
                                      // W-QA B2：spawn 前装配上游产出简报——此时直接上游必已终态且成功
                                      //（失败分支已被跳过传播拦截，轮到本节点的上游全部 ok）
                let upstream_brief = load_upstream_brief(&app, &ups).await;
                // W9-ASK：双层档案注入（工作流决策摘要 + 本卡历史）——防跑偏，
                // 用户在其他卡的纠偏这里看得见；读库失败降级 None 不阻断执行
                let brief = {
                    let app2 = app.clone();
                    let wid = workflow_id.clone();
                    let tid = id.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        crate::db::open_db(&app2).ok().and_then(|conn| {
                            crate::db::brief::brief_for_injection(&conn, &wid, Some(&tid))
                                .ok()
                                .flatten()
                        })
                    })
                    .await
                    .ok()
                    .flatten()
                };
                let ctx = TaskExecCtx {
                    goal: goal.clone(),
                    upstream_brief,
                    brief,
                    // 执行提问授权（W9-ASK：clarify_meta.askMode，run 开始时读一次；
                    // 拍板 5 默认"关键决策才问"=开）
                    ask: Some(crate::bot_chat::AskExecContext {
                        workflow_id: workflow_id.clone(),
                        asks_enabled,
                        run_started_at,
                    }),
                    rework_evidence: None,
                };
                // W-QA C1：attempt 计数（证据链：重试/返工后 attempt 递增）
                let attempt = {
                    let mut m = attempts.lock().unwrap_or_else(|e| e.into_inner());
                    let n = m.entry(id.clone()).or_insert(0);
                    *n += 1;
                    *n
                };
                wa_log(
                    &app,
                    &workflow_id,
                    run_started_at,
                    Some(&id),
                    crate::db::workflow_audit::KIND_NODE_START,
                    crate::audit::AuditLevel::Info,
                    serde_json::json!({ "attempt": attempt }),
                )
                .await;
                let node_started_ms = chrono::Utc::now().timestamp_millis();
                let mut attempt_n = attempt;
                let mut result =
                    run_task_in_chat_ctx(&app, &id, TaskExecOrigin::Workflow, model.clone(), ctx)
                        .await;
                // W10：验收核查环（设计 §4.1）——首轮成功且 acceptance 非空且开关开
                // 且未被取消才进；fail 带证据返工（独立预算 ≤2），用尽仍 fail 终态 failed。
                // partial/unknown → status 保持 success 带徽标继续（拍板 4）。
                let mut acceptance_final: Option<(AcceptanceVerdict, String)> = None;
                let mut rework_used = 0u32;
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        break; // 用户停止：不做验收（豁免口径同收尾评审）
                    }
                    // 熔断识别（W5-FUSE）：循环优雅返回「⏹ 已熔断」消息且任务未完成
                    let fused_now = matches!(&result, Ok(r) if r.result.text.contains(crate::bot_model_loop::FUSE_MARKER));
                    let fresh = crate::db::db_load(app.clone())
                        .await
                        .ok()
                        .and_then(|ts| ts.into_iter().find(|t| t.id == id));
                    let Some(t) = &fresh else { break };
                    let column_done = t.column == TaskStatus::Done;
                    let ok_now = column_done && !fused_now && result.is_ok();
                    if !ok_now {
                        break; // 执行本身没成：不进验收（失败走 C1 重试链）
                    }
                    let Some(acc) = t
                        .acceptance
                        .as_deref()
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                    else {
                        break; // 没有验收标准：无从核查（W-QA 卡即契约缺失=豁免）
                    };
                    if !acceptance_enabled {
                        break;
                    }
                    let summary = t
                        .result
                        .as_ref()
                        .and_then(|r| r.get("summary"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let artifacts: Vec<String> =
                        t.effective_files().into_iter().map(|f| f.path).collect();
                    let (verdict, evidence) =
                        check_acceptance(&app, &t.title, acc, &summary, &artifacts).await;
                    match acceptance_rework_decision(
                        verdict,
                        ACCEPTANCE_REWORK_BUDGET - rework_used,
                    ) {
                        AcceptanceAction::Accept => {
                            acceptance_final = Some((verdict, evidence));
                            break;
                        }
                        AcceptanceAction::Fail => {
                            acceptance_final = Some((verdict, evidence));
                            break;
                        }
                        AcceptanceAction::Rework => {
                            rework_used += 1;
                            attempt_n += 1;
                            {
                                let mut m = attempts.lock().unwrap_or_else(|e| e.into_inner());
                                m.insert(id.clone(), attempt_n);
                            }
                            wa_log(
                                &app,
                                &workflow_id,
                                run_started_at,
                                Some(&id),
                                crate::db::workflow_audit::KIND_REWORK,
                                crate::audit::AuditLevel::Warn,
                                serde_json::json!({
                                    "reason": "acceptance_fail",
                                    "evidence": evidence,
                                    "reworkUsed": rework_used,
                                    "attempt": attempt_n,
                                }),
                            )
                            .await;
                            crate::audit::write_event(
                                &app,
                                crate::audit::AuditLevel::Warn,
                                "workflow_acceptance",
                                &[
                                    ("taskId", id.clone()),
                                    ("verdict", "fail".into()),
                                    ("rework", rework_used.to_string()),
                                ],
                            );
                            let ctx = TaskExecCtx {
                                goal: goal.clone(),
                                upstream_brief: load_upstream_brief(&app, &ups).await,
                                brief: None, // 首轮注入过；返工重在证据，档案层不重复灌
                                ask: Some(crate::bot_chat::AskExecContext {
                                    workflow_id: workflow_id.clone(),
                                    asks_enabled,
                                    run_started_at,
                                }),
                                rework_evidence: Some(format!(
                                    "上一轮产出未通过验收核查：{evidence}"
                                )),
                            };
                            result = run_task_in_chat_ctx(
                                &app,
                                &id,
                                TaskExecOrigin::Workflow,
                                model.clone(),
                                ctx,
                            )
                            .await;
                            // 继续循环：下一轮对新产出再验收
                        }
                    }
                }
                // 熔断识别（W5-FUSE）：循环优雅返回「⏹ 已熔断」消息且任务未完成
                let fused = matches!(&result, Ok(r) if r.result.text.contains(crate::bot_model_loop::FUSE_MARKER));
                // W-QA A2：引擎写结构化结果（先落证据，再判成败）——
                // 此前 result 只有子 agent 编排链在写，工作流卡基本恒空
                let fresh = crate::db::db_load(app.clone())
                    .await
                    .ok()
                    .and_then(|ts| ts.into_iter().find(|t| t.id == id));
                let (column_done, artifacts) = match &fresh {
                    Some(t) => (
                        t.column == TaskStatus::Done,
                        t.effective_files()
                            .into_iter()
                            .map(|f| f.path)
                            .collect::<Vec<_>>(),
                    ),
                    None => (false, Vec::new()),
                };
                let node_result = build_node_result(
                    column_done,
                    fused,
                    result.as_ref().ok().map(|r| r.result.text.as_str()),
                    result.as_ref().err().map(|e| e.message()).as_deref(),
                    &artifacts,
                    attempt_n,
                );
                let mut node_result = node_result;
                let mut acceptance_failed = false;
                if let Some((verdict, evidence)) = &acceptance_final {
                    node_result["acceptanceVerdict"] = serde_json::json!(verdict.as_str());
                    node_result["acceptanceEvidence"] = serde_json::json!(evidence);
                    if *verdict == AcceptanceVerdict::Fail {
                        acceptance_failed = true;
                        // 终态改 failed（拍板 4 下游照现语义跳过）：status/error 覆写
                        node_result["status"] = serde_json::json!("failed");
                        node_result["error"] = serde_json::json!(format!(
                            "验收未通过（返工 {rework_used} 次后仍不达标）：{evidence}"
                        ));
                    }
                }
                let elapsed_ms = (chrono::Utc::now().timestamp_millis() - node_started_ms).max(0);
                node_result["ms"] = serde_json::json!(elapsed_ms);
                write_node_result(&app, &id, node_result.clone()).await;
                wa_log(
                    &app,
                    &workflow_id,
                    run_started_at,
                    Some(&id),
                    crate::db::workflow_audit::KIND_NODE_RESULT,
                    if acceptance_failed {
                        crate::audit::AuditLevel::Warn
                    } else {
                        crate::audit::AuditLevel::Info
                    },
                    serde_json::json!({
                        "status": node_result["status"],
                        "attempt": attempt_n,
                        "ms": elapsed_ms,
                        "acceptanceVerdict": node_result.get("acceptanceVerdict"),
                    }),
                )
                .await;
                if let Some((verdict, evidence)) = &acceptance_final {
                    wa_log(
                        &app,
                        &workflow_id,
                        run_started_at,
                        Some(&id),
                        crate::db::workflow_audit::KIND_ACCEPTANCE_CHECK,
                        if acceptance_failed {
                            crate::audit::AuditLevel::Warn
                        } else {
                            crate::audit::AuditLevel::Info
                        },
                        serde_json::json!({
                            "verdict": verdict.as_str(),
                            "evidence": evidence,
                            "reworkUsed": rework_used,
                        }),
                    )
                    .await;
                    crate::audit::write_event(
                        &app,
                        if acceptance_failed {
                            crate::audit::AuditLevel::Warn
                        } else {
                            crate::audit::AuditLevel::Info
                        },
                        "workflow_acceptance",
                        &[
                            ("taskId", id.clone()),
                            ("verdict", verdict.as_str().into()),
                            ("reworked", rework_used.to_string()),
                        ],
                    );
                }
                let ok = column_done && !fused && result.is_ok() && !acceptance_failed;
                // P1-d：节点收尾状态实时广播（画布描边 + 失败原因/trace 入口的数据源）
                let _ = app.emit(
                    "workflow-node-status",
                    serde_json::json!({
                        "taskId": id.clone(),
                        "status": if ok { "done" } else { "failed" },
                        "sessionId": result.as_ref().ok().map(|r| r.session_id.clone()),
                    }),
                );
                let _ = tx.send(NodeOutcome {
                    id,
                    ok,
                    cancelled: false,
                    fused,
                });
                fallback.sent = true;
            });
        }
    };

    for id in &dag.ready {
        spawn_node(&app, &cancel, id.clone());
    }

    let mut cancelled_early = false;
    while resolved.len() < total {
        if cancel.load(Ordering::SeqCst) {
            cancelled_early = true;
            break;
        }
        let Some(outcome) = rx.recv().await else {
            break;
        };
        if outcome.cancelled {
            // 取消不计失败，但计 resolved（它不会再上报终态）
            resolved.insert(outcome.id.clone());
            if let Ok(mut r) = running.lock() {
                r.remove(&outcome.id);
            }
            continue;
        }
        // C1：失败自动重试一次（熔断除外）——不入 resolved，等重试后的新 outcome
        let retries = retries_left.get(&outcome.id).copied().unwrap_or(0);
        if should_retry(outcome.ok, false, outcome.fused, retries) {
            retries_left.insert(outcome.id.clone(), retries - 1);
            if let Ok(mut r) = running.lock() {
                r.remove(&outcome.id);
            }
            mark_note_prefix(&app, &outcome.id, "🔁 执行异常，自动重试").await;
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if cancel.load(Ordering::SeqCst) {
                // 退避期间被停止：按未启动归档，不再重试
                mark_note_prefix(&app, &outcome.id, "⏭ 已停止，未执行").await;
                resolved.insert(outcome.id.clone());
                continue;
            }
            spawn_node(&app, &cancel, outcome.id);
            continue;
        }
        resolved.insert(outcome.id.clone());
        if let Ok(mut r) = running.lock() {
            r.remove(&outcome.id);
        }
        if outcome.ok {
            done_count += 1;
            let downstream: Vec<String> = dag
                .dependents
                .get(&outcome.id)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            for d in downstream {
                let deg = indegree.get_mut(&d).map(|v| {
                    *v = v.saturating_sub(1);
                    *v
                });
                let d_free = !resolved.contains(&d)
                    && running.lock().map(|r| !r.contains(&d)).unwrap_or(false);
                if deg == Some(0) && d_free {
                    spawn_node(&app, &cancel, d);
                }
            }
        } else {
            failed_ids.insert(outcome.id.clone());
            if outcome.fused {
                // 熔断归因写卡（W5-FUSE）：卡片本身带 ⚠️ 说明，用户知道调上限后可续跑
                mark_note_prefix(
                    &app,
                    &outcome.id,
                    "⚠️ 已熔断：调用工具达上限被停止。可到 设置→工作流 调高「调用工具上限」，然后单卡 🤖 重跑本卡，或直接「继续执行」",
                )
                .await;
            }
            let failed_name = name_by_id
                .get(&outcome.id)
                .cloned()
                .unwrap_or_else(|| outcome.id.clone());
            // 失败传播：传递下游全标跳过（Airflow all_success）
            for skipped in skip_closure(&outcome.id, &dag.dependents) {
                let skipped_running = running
                    .lock()
                    .map(|r| r.contains(&skipped))
                    .unwrap_or(false);
                if resolved.contains(&skipped) || skipped_running {
                    continue;
                }
                resolved.insert(skipped.clone());
                skipped_ids.insert(skipped.clone());
                // P1-d：传递下游被跳过 → 实时广播（画布灰显）
                let _ = app.emit(
                    "workflow-node-status",
                    serde_json::json!({ "taskId": skipped.clone(), "status": "skipped" }),
                );
                mark_note_prefix(
                    &app,
                    &skipped,
                    &format!("⏭ 因上游「{}」失败未执行", failed_name),
                )
                .await;
            }
            failed_names.push(failed_name);
        }
    }
    // 取消收尾：未解析且**未在跑**的节点标 ⏭ 已停止；在跑的会话自然收尾
    // （它们的结果由执行链路自己写卡，此处不抢）
    let to_mark: Vec<String> = dag
        .nodes
        .iter()
        .filter(|id| {
            !resolved.contains(*id) && running.lock().map(|r| !r.contains(*id)).unwrap_or(true)
        })
        .cloned()
        .collect();
    for id in &to_mark {
        mark_note_prefix(&app, id, "⏭ 已停止，未执行").await;
    }

    // W-QA C2/C3：结算评审 + 有界返工环（用户主动停止时不做——半途结果不构成评审对象）
    if !cancelled_early && total > 0 {
        review_and_rework(
            &app,
            &workflow_id,
            &goal,
            &dag,
            &name_by_id,
            &failed_ids,
            &skipped_ids,
            &spawn_node,
            &cancel,
            &running,
            &mut rx,
            run_started_at,
        )
        .await;
    }

    // W9-ASK：run 收尾——本 run 的 pending 问题批量失效（半途结果不构成提问对象，
    // 重跑会重新注册提问上下文）；有失效才广播，省一次通知页刷新
    {
        let app2 = app.clone();
        let wid = workflow_id.clone();
        let invalidated = tauri::async_runtime::spawn_blocking(move || {
            crate::db::open_db(&app2).ok().and_then(|conn| {
                crate::workflow_questions::invalidate_workflow_questions(&conn, &wid).ok()
            })
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
        if invalidated > 0 {
            crate::notifications::emit_changed(&app);
        }
    }
    runs().lock().map(|mut m| m.remove(&workflow_id));
    let failed_n = failed_names.len();
    let skipped_n = total.saturating_sub(done_count + failed_n);
    crate::audit::write_event(
        &app,
        if failed_n > 0 {
            crate::audit::AuditLevel::Warn
        } else {
            crate::audit::AuditLevel::Info
        },
        "workflow_run_done",
        &[
            ("workflowId", workflow_id.clone()),
            ("total", total.to_string()),
            ("done", done_count.to_string()),
            ("failed", failed_n.to_string()),
            ("skipped", skipped_n.to_string()),
        ],
    );
    // W10：run_done 审计行 + 保留清理（设置项，默认最近 20 个 run）
    wa_log(
        &app,
        &workflow_id,
        run_started_at,
        None,
        crate::db::workflow_audit::KIND_RUN_DONE,
        if failed_n > 0 {
            crate::audit::AuditLevel::Warn
        } else {
            crate::audit::AuditLevel::Info
        },
        serde_json::json!({
            "total": total,
            "done": done_count,
            "failed": failed_n,
            "skipped": skipped_n,
        }),
    )
    .await;
    {
        let app2 = app.clone();
        let r = tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
            let conn = crate::db::open_db(&app2).map_err(|e| e.to_string())?;
            let keep = crate::db::workflow_settings::audit_retention_runs(&conn);
            crate::db::workflow_audit::wa_prune(&conn, keep)
        })
        .await;
        match r {
            Ok(Ok(n)) if n > 0 => {
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "workflow_audit_pruned",
                    &[("rows", n.to_string())],
                );
            }
            Ok(Err(e)) => eprintln!("[workflow_audit] 保留清理失败（不阻断）：{e}"),
            _ => {}
        }
    }
    notify_workflow_done(&app, total, done_count, failed_n);
}

// ────────────── W-QA：上下文装配 / 证据落卡 / 评审与返工 ──────────────

/// 读工作流总目标（B1：此前 goal 只是画布元数据，执行期根本不读）
async fn load_workflow_goal(app: &AppHandle, workflow_id: &str) -> Option<String> {
    let app = app.clone();
    let wid = workflow_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = crate::db::open_db(&app).ok()?;
        crate::db::workflow::load_workflow(&conn, &wid)
            .ok()
            .flatten()
            .map(|w| w.goal)
    })
    .await
    .ok()
    .flatten()
}

/// 执行提问开关（W9-ASK 拍板 10）：读 workflows.clarify_meta.askMode，
/// "never" = 从不提问；缺列/缺字段/解析失败一律默认开（关键决策才问）。
async fn load_asks_enabled(app: &AppHandle, workflow_id: &str) -> bool {
    let app = app.clone();
    let wid = workflow_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        crate::db::open_db(&app).ok().and_then(|conn| {
            crate::db::workflow::load_workflow(&conn, &wid)
                .ok()
                .flatten()
                .and_then(|w| w.clarify_meta)
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    v.get("askMode")
                        .and_then(serde_json::Value::as_str)
                        .map(String::from)
                })
                .map(|mode| mode != "never")
        })
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(true)
}

/// 装配直接上游简报（B2）：spawn 前重读上游终态卡（A2 已写结构化 result）
async fn load_upstream_brief(app: &AppHandle, upstream_ids: &[String]) -> Option<String> {
    if upstream_ids.is_empty() {
        return None;
    }
    let all = crate::db::db_load(app.clone()).await.ok()?;
    let ups: Vec<&Task> = upstream_ids
        .iter()
        .filter_map(|id| all.iter().find(|t| &t.id == id))
        .collect();
    upstream_brief(&ups)
}

/// 节点结构化结果落卡（RMW 合并，与 mark_note_prefix 同款；best-effort 不阻断收尾）
async fn write_node_result(app: &AppHandle, task_id: &str, result: serde_json::Value) {
    if let Ok(cur) = crate::db::db_load(app.clone()).await {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task_id) {
            fresh.result = Some(result);
            fresh.expected_updated_at = fresh.updated_at;
            fresh.updated_at = Some(chrono::Utc::now().timestamp_millis());
            if crate::db::db_upsert(app.clone(), vec![fresh.clone()])
                .await
                .is_ok()
            {
                crate::bot::broadcast_after_mutation(app, vec![fresh], vec![]);
            }
        }
    }
}

/// 返工前置位（C3）：Done 卡 run_task_in_chat 会拒绝执行，先重置回 Todo；
/// 上轮 result 保留（新执行会覆写，attempt 递增即证据链）
async fn reset_node_for_rework(app: &AppHandle, task_id: &str) {
    if let Ok(cur) = crate::db::db_load(app.clone()).await {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task_id) {
            if fresh.column != TaskStatus::Done {
                return;
            }
            fresh.column = TaskStatus::Todo;
            fresh.completed_at = None;
            fresh.expected_updated_at = fresh.updated_at;
            fresh.updated_at = Some(chrono::Utc::now().timestamp_millis());
            if crate::db::db_upsert(app.clone(), vec![fresh.clone()])
                .await
                .is_ok()
            {
                crate::bot::broadcast_after_mutation(app, vec![fresh], vec![]);
            }
        }
    }
}

/// 结算评审（C2，LLM-as-Judge）：goal + 各节点终态（含验收标准/产出摘要/失败归因）
/// → rubric 结构化裁决。调用失败降级为 unknown 报告（不阻断收尾）。
async fn run_review(
    app: &AppHandle,
    goal: &Option<String>,
    tasks: &[Task],
    failed_ids: &HashSet<String>,
    skipped_ids: &HashSet<String>,
) -> serde_json::Value {
    let mut user = String::from("工作流总目标：\n");
    user.push_str(
        goal.as_deref()
            .filter(|g| !g.trim().is_empty())
            .unwrap_or("（未提供）"),
    );
    user.push_str("\n\n各节点执行结果：");
    for t in tasks {
        let status = if skipped_ids.contains(&t.id) {
            "⏭ 跳过"
        } else if failed_ids.contains(&t.id) {
            "❌ 失败"
        } else if node_is_success(t) {
            "✅ 完成"
        } else {
            "⚠️ 未完成"
        };
        user.push_str(&format!("\n\n### {status}「{}」", t.title));
        if let Some(acc) = t.acceptance.as_deref().filter(|a| !a.trim().is_empty()) {
            user.push_str(&format!("\n验收标准：{acc}"));
        }
        if let Some(r) = &t.result {
            if let Some(s) = r.get("summary").and_then(|v| v.as_str()) {
                user.push_str(&format!("\n产出摘要：{}", s.trim()));
            }
            if let Some(e) = r.get("error").and_then(|v| v.as_str()) {
                user.push_str(&format!("\n归因：{e}"));
            }
        }
        let bound = t.effective_files();
        if !bound.is_empty() {
            user.push_str(&format!(
                "\n产物文件：{}",
                bound
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            ));
        }
    }
    user.push_str("\n\n请按系统指令输出评审 JSON。");
    match crate::bot_chat::summarize_messages(
        app,
        REVIEW_SYSTEM_PROMPT,
        &[crate::bot_chat::ChatMsg {
            role: "user".into(),
            content: user,
        }],
    )
    .await
    {
        Ok(raw) => parse_review_report(&raw),
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "workflow_review",
                &[
                    ("outcome", "failed".to_string()),
                    ("error", crate::audit::escape_for_log(&e.message(), 200)),
                ],
            );
            serde_json::json!({
                "verdict": "unknown",
                "overall": format!("评审调用失败：{}", e.message()),
                "issues": [],
            })
        }
    }
}

/// 报告落库 + 广播（best-effort：写失败只记审计，不影响执行收尾）
async fn persist_and_emit_report(app: &AppHandle, workflow_id: &str, report: &serde_json::Value) {
    let json = serde_json::to_string(report).unwrap_or_default();
    let at = chrono::Utc::now().timestamp_millis();
    let app2 = app.clone();
    let wid = workflow_id.to_string();
    let stored = json.clone();
    let r = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] workflow_runner::DB_WRITE_LOCK: {e:?}");
            e.into_inner()
        });
        let conn = crate::db::open_db(&app2)?;
        crate::db::workflow::workflow_set_report(&conn, &wid, &stored, at)
    })
    .await;
    match r {
        Ok(Ok(())) => {
            let _ = app.emit(
                "workflow-report",
                serde_json::json!({ "workflowId": workflow_id, "report": report }),
            );
        }
        Ok(Err(e)) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "workflow_review",
                &[
                    ("outcome", "persist_failed".to_string()),
                    ("error", crate::audit::escape_for_log(&e, 200)),
                ],
            );
        }
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "workflow_review",
                &[
                    ("outcome", "persist_failed".to_string()),
                    ("error", crate::audit::escape_for_log(&e.to_string(), 200)),
                ],
            );
        }
    }
}

/// 结算评审 + 有界返工环（C2/C3）：评审 → needsRework 节点（含传递下游）返工一轮
/// → 仅返工节点轻量终审更新报告。全程每节点至多返工 1 次、评审调用至多 2 次
///（Reflexion 环必须有界——无限返工既烧 token 又可能震荡）。
#[allow(clippy::too_many_arguments)]
async fn review_and_rework(
    app: &AppHandle,
    workflow_id: &str,
    goal: &Option<String>,
    dag: &Dag,
    name_by_id: &HashMap<String, String>,
    failed_ids: &HashSet<String>,
    skipped_ids: &HashSet<String>,
    spawn_node: &impl Fn(&AppHandle, &Arc<AtomicBool>, String),
    cancel: &Arc<AtomicBool>,
    running: &Arc<Mutex<HashSet<String>>>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<NodeOutcome>,
    // W10：run 分组键（review/rework_round 审计行归组）
    run_started_at: i64,
) {
    let tasks = load_workflow_tasks(app, workflow_id)
        .await
        .unwrap_or_default();
    if tasks.is_empty() {
        return;
    }
    // ① 全图评审
    let report = run_review(app, goal, &tasks, failed_ids, skipped_ids).await;
    // W10：review 审计行（verdict + issues 数）
    wa_log(
        app,
        workflow_id,
        run_started_at,
        None,
        crate::db::workflow_audit::KIND_REVIEW,
        crate::audit::AuditLevel::Info,
        serde_json::json!({
            "verdict": report.get("verdict"),
            "issues": report.get("issues").and_then(serde_json::Value::as_array).map(|a| a.len()).unwrap_or(0),
        }),
    )
    .await;
    persist_and_emit_report(app, workflow_id, &report).await;

    // ② needsRework → 返工根节点（failed 已有重试语义，不再返工；
    //    不在本次运行图内的节点——如 done+success 断点跳过——也可返工，卡即存在）
    let title_to_id: HashMap<&str, &str> = tasks
        .iter()
        .map(|t| (t.title.as_str(), t.id.as_str()))
        .collect();
    let mut roots: Vec<String> = Vec::new();
    let mut seen_root: HashSet<String> = HashSet::new();
    for issue in report
        .get("issues")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if !issue
            .get("needsRework")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let Some(title) = issue.get("nodeTitle").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(&id) = title_to_id.get(title) else {
            continue;
        };
        if failed_ids.contains(id) || !seen_root.insert(id.to_string()) {
            continue;
        }
        roots.push(id.to_string());
    }
    if roots.is_empty() {
        return;
    }
    // ③ 返工闭包 = 根 + 传递下游（下游建立在旧产出上，上游返工后必须联动重跑）
    let mut rework_set: Vec<String> = Vec::new();
    let mut in_set: HashSet<String> = HashSet::new();
    for root in &roots {
        for id in std::iter::once(root.clone()).chain(skip_closure(root, &dag.dependents)) {
            if in_set.insert(id.clone()) {
                rework_set.push(id);
            }
        }
    }
    for id in &rework_set {
        let is_root = roots.contains(id);
        let reason = if is_root {
            let problem = report
                .get("issues")
                .and_then(|v| v.as_array())
                .and_then(|arr| {
                    arr.iter().find(|i| {
                        i.get("nodeTitle").and_then(|t| t.as_str())
                            == name_by_id.get(id).map(|s| s.as_str())
                    })
                })
                .and_then(|i| i.get("problem"))
                .and_then(|p| p.as_str())
                .unwrap_or("产出未达验收标准");
            format!("🔍 审校返工：{problem}")
        } else {
            "🔍 审校返工（上游返工，联动重跑）".to_string()
        };
        mark_note_prefix(app, id, &reason).await;
        reset_node_for_rework(app, id).await;
    }
    // ④ 返工子图调度：入度只计返工集内的边（闭包保证祖先全在集内）
    let mut indeg: HashMap<String, usize> = HashMap::new();
    let mut dependents_in: HashMap<String, Vec<String>> = HashMap::new();
    for id in &rework_set {
        let deps: Vec<String> = tasks
            .iter()
            .find(|t| &t.id == id)
            .and_then(|t| t.depends_on.clone())
            .unwrap_or_default();
        indeg.insert(
            id.clone(),
            deps.iter().filter(|d| in_set.contains(*d)).count(),
        );
        for d in deps {
            if in_set.contains(&d) {
                dependents_in.entry(d).or_default().push(id.clone());
            }
        }
    }
    let mut pending = rework_set.len();
    for id in &rework_set {
        if indeg.get(id).copied().unwrap_or(0) == 0 {
            spawn_node(app, cancel, id.clone());
        }
    }
    let mut reworked_ids: Vec<String> = Vec::new();
    while pending > 0 {
        if cancel.load(Ordering::SeqCst) {
            return; // 停止：终审不做，报告保留首轮结论
        }
        let Some(outcome) = rx.recv().await else {
            return;
        };
        if !in_set.contains(&outcome.id) {
            continue; // 非返工节点的迟到消息（理论上主循环已结束）
        }
        pending -= 1;
        if let Ok(mut r) = running.lock() {
            r.remove(&outcome.id);
        }
        if outcome.cancelled {
            continue;
        }
        if outcome.ok {
            reworked_ids.push(outcome.id.clone());
            let downstream: Vec<String> = dependents_in
                .get(&outcome.id)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            for d in downstream {
                let deg = indeg.get_mut(&d).map(|v| {
                    *v = v.saturating_sub(1);
                    *v
                });
                let d_free = running.lock().map(|r| !r.contains(&d)).unwrap_or(true);
                if deg == Some(0) && d_free {
                    spawn_node(app, cancel, d);
                }
            }
        } else {
            // 返工后仍失败：集内下游不再重跑（上游产出依旧缺失），标注后消化其 outcome
            let failed_name = name_by_id
                .get(&outcome.id)
                .cloned()
                .unwrap_or_else(|| outcome.id.clone());
            for skipped in skip_closure(&outcome.id, &dependents_in) {
                if running
                    .lock()
                    .map(|r| r.contains(&skipped))
                    .unwrap_or(false)
                {
                    continue;
                }
                pending = pending.saturating_sub(1);
                mark_note_prefix(
                    app,
                    &skipped,
                    &format!("⏭ 上游「{failed_name}」返工后仍失败，未重跑"),
                )
                .await;
            }
        }
    }
    // ⑤ 轻量终审：只覆盖返工节点，更新报告
    if !reworked_ids.is_empty() {
        let fresh = load_workflow_tasks(app, workflow_id)
            .await
            .unwrap_or_default();
        let reworked_tasks: Vec<Task> = fresh
            .into_iter()
            .filter(|t| reworked_ids.contains(&t.id))
            .collect();
        if !reworked_tasks.is_empty() {
            let final_report =
                run_review(app, goal, &reworked_tasks, failed_ids, &HashSet::new()).await;
            let mut final_report = final_report;
            final_report["reworkRound"] = serde_json::json!(true);
            final_report["reworkedNodes"] = serde_json::json!(reworked_ids
                .iter()
                .filter_map(|id| name_by_id.get(id))
                .collect::<Vec<_>>());
            // W10：rework_round 审计行
            wa_log(
                app,
                workflow_id,
                run_started_at,
                None,
                crate::db::workflow_audit::KIND_REWORK_ROUND,
                crate::audit::AuditLevel::Info,
                serde_json::json!({
                    "reworked": reworked_ids.len(),
                    "verdict": final_report.get("verdict"),
                }),
            )
            .await;
            persist_and_emit_report(app, workflow_id, &final_report).await;
        }
    }
}

/// note 前置标记（RMW 合并，与调度器 ⏰ 摘要前置同款；基于执行后最新数据合并）。
/// 调用方：⏭ 跳过 / ⚠️ 熔断归因（W5-FUSE）
async fn mark_note_prefix(app: &AppHandle, task_id: &str, reason: &str) {
    if let Ok(cur) = crate::db::db_load(app.clone()).await {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task_id) {
            let note = match fresh.note.as_deref().filter(|n| !n.trim().is_empty()) {
                Some(n) => format!("{reason}\n{n}"),
                None => reason.to_string(),
            };
            fresh.note = Some(note);
            fresh.expected_updated_at = fresh.updated_at;
            fresh.updated_at = Some(chrono::Utc::now().timestamp_millis());
            if crate::db::db_upsert(app.clone(), vec![fresh.clone()])
                .await
                .is_ok()
            {
                crate::bot::broadcast_after_mutation(app, vec![fresh], vec![]);
            }
        }
    }
}

fn notify_workflow_done(app: &AppHandle, total: usize, done: usize, failed: usize) {
    let title = if failed > 0 {
        format!("🔀 工作流结束（{failed} 个失败）")
    } else {
        "🔀 工作流完成".to_string()
    };
    let body = format!("{done}/{total} 个节点完成");
    if let Err(e) = app
        .notification()
        .builder()
        .title(&title)
        .body(&body)
        .show()
    {
        eprintln!("[workflow] 系统通知发送失败（未授权？）：{e}");
    }
}

// ────────────── 单测 ──────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CanvasPos;

    fn task(id: &str, deps: &[&str]) -> Task {
        Task {
            id: id.into(),
            title: format!("任务{id}"),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column: TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: Some(1),
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            origin: Some(crate::db::TASK_ORIGIN_WORKFLOW.into()),
            workflow_id: Some("wf".into()),
            depends_on: Some(deps.iter().map(|s| s.to_string()).collect()),
            canvas_pos: Some(CanvasPos { x: 0.0, y: 0.0 }),
            model: None,
            acceptance: None,
            owner_id: None,
            created_at: None,
            enabled: None,
            expected_updated_at: None,
        }
    }

    fn done_success(mut t: Task) -> Task {
        t.column = TaskStatus::Done;
        t.result = Some(serde_json::json!({"status": "success"}));
        t
    }

    #[test]
    fn build_dag_ready_and_indegree() {
        let tasks = vec![task("a", &[]), task("b", &["a"]), task("c", &["a", "b"])];
        let dag = build_dag(&tasks, &node_is_success).unwrap();
        assert_eq!(dag.ready, vec!["a".to_string()]);
        assert_eq!(dag.indegree["b"], 1);
        assert_eq!(dag.indegree["c"], 2);
        assert_eq!(dag.dependents["a"], vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn build_dag_done_nodes_are_ready_immediately() {
        // 断点续跑：b 已完成（success）→ 唯一依赖 b 的 c 直接就绪
        let tasks = vec![
            task("a", &[]),
            done_success(task("b", &["a"])),
            task("c", &["b"]),
        ];
        let dag = build_dag(&tasks, &node_is_success).unwrap();
        assert!(dag.ready.contains(&"a".to_string()));
        assert!(dag.ready.contains(&"c".to_string()));
        assert_eq!(dag.indegree["c"], 0);
        // c 仍依赖未完成的 a 时不算就绪（只豁免已完成上游）
        let tasks2 = vec![
            task("a", &[]),
            done_success(task("b", &["a"])),
            task("c", &["a", "b"]),
        ];
        let dag2 = build_dag(&tasks2, &node_is_success).unwrap();
        assert!(!dag2.ready.contains(&"c".to_string()));
        assert_eq!(dag2.indegree["c"], 1);
    }

    #[test]
    fn dag_nodes_exclude_done_success() {
        // 全量对照 critical 回归锁：done+success 节点不得进 nodes——
        // 它们不上报终态，计入 total 会让控制器 recv 永久等待（断点续跑死锁）
        let tasks = vec![
            task("a", &[]),
            done_success(task("b", &["a"])),
            task("c", &["b"]),
            done_success(task("d", &[])),
        ];
        let dag = build_dag(&tasks, &node_is_success).unwrap();
        assert_eq!(dag.nodes, vec!["a".to_string(), "c".to_string()]);
        assert!(!dag.nodes.iter().any(|id| id == "b" || id == "d"));
    }

    #[test]
    fn build_dag_rejects_cycle_and_dangling() {
        let cycle = vec![task("a", &["b"]), task("b", &["a"])];
        assert!(build_dag(&cycle, &node_is_success).is_err());
        let dangling = vec![task("a", &["ghost"])];
        assert!(build_dag(&dangling, &node_is_success).is_err());
    }

    #[test]
    fn success_judgment_matches_node_border_semantics() {
        // done + 无 result = 手动完成 = 成功
        let mut manual = task("m", &[]);
        manual.column = TaskStatus::Done;
        assert!(node_is_success(&manual));
        // done + success
        assert!(node_is_success(&done_success(task("s", &[]))));
        // done + failed
        let mut failed = task("f", &[]);
        failed.column = TaskStatus::Done;
        failed.result = Some(serde_json::json!({"status": "failed"}));
        assert!(!node_is_success(&failed));
        // todo
        assert!(!node_is_success(&task("t", &[])));
    }

    #[test]
    fn skip_closure_is_transitive() {
        let tasks = vec![
            task("a", &[]),
            task("b", &["a"]),
            task("c", &["b"]),
            task("d", &[]), // 无关分支
            task("e", &["c"]),
        ];
        let dag = build_dag(&tasks, &node_is_success).unwrap();
        let mut closure = skip_closure("a", &dag.dependents);
        closure.sort();
        assert_eq!(closure, vec!["b", "c", "e"]);
        assert!(!closure.contains(&"d".to_string()));
    }

    #[test]
    fn parallel_branches_have_independent_indegree() {
        // a、b 无依赖并行；c 只等 a —— b 失败不影响 c 分支的就绪
        let tasks = vec![
            task("a", &[]),
            task("b", &[]),
            task("c", &["a"]),
            task("d", &["b"]),
        ];
        let dag = build_dag(&tasks, &node_is_success).unwrap();
        assert_eq!(dag.ready.len(), 2);
        assert_eq!(dag.indegree["c"], 1);
        assert_eq!(dag.indegree["d"], 1);
    }

    // ────────────── W-QA：证据结果 / 重试决策 / 上游简报 / 评审解析 ──────────────

    #[test]
    fn node_result_statuses() {
        // done + 正常 → success
        let ok = build_node_result(true, false, Some("完成"), None, &[], 1);
        assert_eq!(ok["status"], "success");
        assert_eq!(ok["attempt"], 1);
        assert_eq!(ok["engine"], "runner");
        // 循环正常返回但未标完成 → incomplete（可归因：漏调 complete_task）
        let inc = build_node_result(false, false, Some("做完了"), None, &[], 1);
        assert_eq!(inc["status"], "incomplete");
        // 熔断 → failed（即使卡片已标完成，熔断语义优先）
        let fused = build_node_result(true, true, Some("⏹ 已熔断"), None, &[], 1);
        assert_eq!(fused["status"], "failed");
        // 循环错误 → failed，attempt 留证据链
        let err = build_node_result(false, false, None, Some("对话轮数超限"), &[], 2);
        assert_eq!(err["status"], "failed");
        assert_eq!(err["attempt"], 2);
        // 摘要截断 300 字 + 产物入 JSON
        let long = "a".repeat(400);
        let truncated = build_node_result(true, false, Some(&long), None, &["x.md".into()], 1);
        assert_eq!(truncated["summary"].as_str().unwrap().chars().count(), 300);
        assert_eq!(truncated["artifacts"][0], "x.md");
    }

    #[test]
    fn retry_decision_excludes_fused_and_cancelled() {
        assert!(should_retry(false, false, false, 1));
        assert!(!should_retry(false, false, false, 0)); // 余量耗尽
        assert!(!should_retry(false, true, false, 1)); // 取消不重试
        assert!(!should_retry(false, false, true, 1)); // 熔断不自动重试（需人工调上限）
        assert!(!should_retry(true, false, false, 1)); // 成功不重试
    }

    // ────────────── W10：验收解析/返工决策（纯逻辑单测锚点） ──────────────

    #[test]
    fn acceptance_verdict_parses_and_degrades() {
        use AcceptanceVerdict::{Fail, Partial, Pass, Unknown};
        assert_eq!(
            parse_acceptance_verdict(
                "```json\n{\"verdict\":\"pass\",\"evidence\":\"产出齐全\"}\n```"
            ),
            Pass
        );
        assert_eq!(
            parse_acceptance_verdict(r#"{"verdict":"partial","evidence":"缺一节"}"#),
            Partial
        );
        assert_eq!(
            parse_acceptance_verdict(r#"{"verdict":"fail","evidence":"文件不存在"}"#),
            Fail
        );
        // 契约外字符串/坏 JSON/缺 verdict → Unknown 降级（不阻断不返工）
        assert_eq!(parse_acceptance_verdict("抱歉，我无法输出 JSON"), Unknown);
        assert_eq!(parse_acceptance_verdict(r#"{"foo":1}"#), Unknown);
        assert_eq!(
            parse_acceptance_verdict(r#"{"verdict":"unknown"}"#),
            Unknown
        );
        // evidence 非串 → 空
        let ev = parse_acceptance_evidence(r#"{"verdict":"fail","evidence":[{"x":1}]}"#);
        assert!(ev.is_empty());
        let long = parse_acceptance_evidence(&format!(
            r#"{{"verdict":"fail","evidence":"{}"}}"#,
            "长".repeat(150)
        ));
        assert_eq!(long.chars().count(), 100);
    }

    #[test]
    fn acceptance_rework_decision_matches_budget() {
        use AcceptanceAction::{Accept, Fail as AFail, Rework};
        use AcceptanceVerdict::{Fail, Partial, Pass, Unknown};
        // fail + 余量 → 返工；用尽 → 终态 failed
        assert_eq!(
            acceptance_rework_decision(Fail, ACCEPTANCE_REWORK_BUDGET),
            Rework
        );
        assert_eq!(acceptance_rework_decision(Fail, 1), Rework);
        assert_eq!(acceptance_rework_decision(Fail, 0), AFail);
        // partial/unknown/pass → 带结果继续（status 不变，拍板 4）
        assert_eq!(acceptance_rework_decision(Partial, 2), Accept);
        assert_eq!(acceptance_rework_decision(Unknown, 2), Accept);
        assert_eq!(acceptance_rework_decision(Pass, 0), Accept);
    }

    #[test]
    fn upstream_brief_caps_and_formats() {
        let mut a = task("a", &[]);
        a.column = TaskStatus::Done; // node_is_success 要求 done+success
        a.result = Some(serde_json::json!({"status": "success", "summary": "产出素材清单"}));
        a.acceptance = Some("产出素材清单.md".into());
        let b = task("b", &["a"]);
        let brief = upstream_brief(&[&a, &b]).unwrap();
        assert!(brief.contains("任务a"));
        assert!(brief.contains("产出素材清单")); // result.summary 进入简报
        assert!(brief.contains("产出素材清单.md")); // 验收标准进入简报
        assert!(brief.contains("✅ 完成"));
        // 空上游 → None（不注入空段）
        assert!(upstream_brief(&[]).is_none());
        // 单上游：per-cap 600 生效（2000 字摘要被裁到 600），总长受控
        let mut big = task("c", &[]);
        big.result = Some(serde_json::json!({"status": "success", "summary": "长".repeat(2000)}));
        let one = upstream_brief(&[&big]).unwrap();
        assert!(one.chars().count() < UPSTREAM_PER_CAP + 60);
        assert!(!one.contains("已截断"));
        // 多上游超总上限：截断标注出现
        let many: Vec<Task> = (0..6)
            .map(|i| {
                let mut t = task(&i.to_string(), &[]);
                t.result =
                    Some(serde_json::json!({"status": "success", "summary": "长".repeat(600)}));
                t
            })
            .collect();
        let refs: Vec<&Task> = many.iter().collect();
        let briefs = upstream_brief(&refs).unwrap();
        assert!(briefs.chars().count() <= UPSTREAM_TOTAL_CAP + 40);
        assert!(briefs.contains("已截断"));
    }

    #[test]
    fn review_parse_json_and_fallback() {
        let raw = "```json\n{\"verdict\":\"partial\",\"overall\":\"整体可用\",\"issues\":[{\"nodeTitle\":\"写初稿\",\"problem\":\"缺结论\",\"needsRework\":true}]}\n```";
        let v = parse_review_report(raw);
        assert_eq!(v["verdict"], "partial");
        assert_eq!(v["issues"][0]["nodeTitle"], "写初稿");
        assert_eq!(v["issues"][0]["needsRework"], true);
        // 非 JSON → 降级纯文本（内容不丢，返工环不触发）
        let v = parse_review_report("抱歉，我无法输出 JSON");
        assert_eq!(v["verdict"], "unknown");
        assert_eq!(v["issues"].as_array().unwrap().len(), 0);
        assert!(v["overall"].as_str().unwrap().contains("抱歉"));
    }
}
