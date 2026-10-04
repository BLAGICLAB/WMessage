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

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::bot_chat::{run_task_in_chat, TaskExecOrigin};
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
            .filter(|d| {
                tasks
                    .iter()
                    .find(|x| x.id == **d)
                    .map(|up| !is_success(up))
                    .unwrap_or(false)
            })
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

// ────────────── 运行注册表 / 取消 ──────────────

struct RunHandle {
    cancel: Arc<AtomicBool>,
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
#[tauri::command]
pub async fn workflow_run(app: AppHandle, workflow_id: String) -> CommandResult<WorkflowRunStart> {
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
    runs().lock().map_err(|_| registry_poisoned())?.insert(
        workflow_id.clone(),
        Arc::new(RunHandle {
            cancel: cancel.clone(),
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
    let app2 = app.clone();
    let wf = workflow_id.clone();
    tauri::async_runtime::spawn(async move {
        run_controller(app2, wf, dag, name_by_id, model_by_id, cancel).await;
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
                &[("workflowId", workflow_id)],
            );
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
    cancel: Arc<AtomicBool>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<NodeOutcome>();
    let mut indegree = dag.indegree.clone();
    let mut resolved: HashSet<String> = HashSet::new();
    // 在跑集合：跨闭包共享（Arc<Mutex>），取消收尾时区分「在跑」与「从未启动」
    let running: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut failed_names: Vec<String> = Vec::new();
    let mut done_count = 0usize;
    let total = dag.nodes.len();

    let spawn_node = {
        let tx = tx.clone();
        let running = running.clone();
        let model_by_id = model_by_id.clone();
        move |app: &AppHandle, cancel: &Arc<AtomicBool>, id: String| {
            let app = app.clone();
            let tx = tx.clone();
            let cancel = cancel.clone();
            let running = running.clone();
            let model = model_by_id.get(&id).cloned().flatten();
            if let Ok(mut r) = running.lock() {
                r.insert(id.clone());
            }
            tauri::async_runtime::spawn(async move {
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
                    return;
                }
                let _ticket = ticket; // RAII 占槽：任务结束自动释放
                let result = run_task_in_chat(&app, &id, TaskExecOrigin::Workflow, model).await;
                // 熔断识别（W5-FUSE）：循环优雅返回「⏹ 已熔断」消息且任务未完成
                let fused = matches!(&result, Ok(r) if r.result.text.contains(crate::bot_model_loop::FUSE_MARKER));
                let ok = match &result {
                    Ok(_) => crate::db::db_load(app.clone())
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .find(|t| t.id == id)
                        .map(|t| node_is_success(&t))
                        .unwrap_or(false),
                    Err(_) => false,
                };
                let _ = tx.send(NodeOutcome {
                    id,
                    ok,
                    cancelled: false,
                    fused,
                });
            });
        }
    };

    for id in &dag.ready {
        spawn_node(&app, &cancel, id.clone());
    }

    while resolved.len() < total {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some(outcome) = rx.recv().await else {
            break;
        };
        resolved.insert(outcome.id.clone());
        if let Ok(mut r) = running.lock() {
            r.remove(&outcome.id);
        }
        if outcome.cancelled {
            continue;
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
            ("workflowId", workflow_id),
            ("total", total.to_string()),
            ("done", done_count.to_string()),
            ("failed", failed_n.to_string()),
            ("skipped", skipped_n.to_string()),
        ],
    );
    notify_workflow_done(&app, total, done_count, failed_n);
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
            owner_id: None,
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
}
