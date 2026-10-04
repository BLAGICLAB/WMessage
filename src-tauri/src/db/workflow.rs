//! 工作流（W1-CANVAS，设计 docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md §3.2/§7）：
//! 元数据 CRUD + 「指纹 diff 保存」——节点卡本身存 tasks 表（origin='workflow'），
//! 本模块只管 workflows 行与保存时的 id 保留/新建/删除裁决。
//!
//! 指纹（fingerprint）：递归内容签名 `(title|note|tags|sorted fp(上游))`。
//! 两张内容与拓扑都相同的卡无论真实 id 是否相同，指纹一致 → 保存时保留原任务 id
//! （连带 column/result/budget/子任务全部执行痕迹），实现「编辑后断点续跑仍成立」。
//!
//! 画布契约：每个草稿节点带唯一 localId（画布内部拓扑键），dependsOn 引用 localId；
//! taskId 仅是画布侧的上次保存绑定提示，**服务端不信任它**——保留与否由指纹裁决。

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::async_runtime;
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

use super::tasks::{
    delete_tasks, load_tasks_by_workflow, upsert_tasks, CanvasPos, Task, TaskStatus,
    TASK_ORIGIN_WORKFLOW,
};

/// 节点数硬顶（设计 §4：拆解/加卡/保存三入口同拦）
pub const MAX_WORKFLOW_NODES: usize = 30;
pub const MAX_WORKFLOW_NAME: usize = 80;
pub const MAX_WORKFLOW_GOAL: usize = 500;
pub const MAX_NODE_TITLE: usize = 80;
pub const MAX_NODE_NOTE: usize = 500;

/// 单源 DDL：open_db 与测试建表共用，防两处 schema 漂移
pub const WORKFLOWS_DDL: &str = "CREATE TABLE IF NOT EXISTS workflows (
   id         TEXT PRIMARY KEY,
   name       TEXT NOT NULL,
   goal       TEXT NOT NULL,
   created_at INTEGER,
   updated_at INTEGER
 );";

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
}

/// workflow_save 的单个节点草稿。taskId = 已保存卡的绑定提示（服务端不信任，
/// 保留与否由指纹裁决）；dependsOn 引用本列表内其他节点的 localId。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNodeDraft {
    pub local_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub pos: Option<CanvasPos>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSaveInput {
    /// None = 新建工作流；Some = 覆盖保存既有工作流
    #[serde(default)]
    pub workflow_id: Option<String>,
    pub name: String,
    pub goal: String,
    pub nodes: Vec<WorkflowNodeDraft>,
}

/// 保存结果：画布节点本地 id → 真实任务 id 绑定（前端据此重建连线与卡绑定）
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSaveBinding {
    pub local_id: String,
    pub task_id: String,
    pub created: bool,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSaveResult {
    pub workflow_id: String,
    pub bindings: Vec<WorkflowSaveBinding>,
    pub kept: usize,
    pub created: usize,
    pub deleted: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDetail {
    #[serde(flatten)]
    pub workflow: Workflow,
    /// 该工作流的全部节点卡（未软删；画布以此为准重建）
    pub tasks: Vec<Task>,
}

// ────────────── workflows 行 CRUD ──────────────

fn workflow_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Workflow> {
    Ok(Workflow {
        id: r.get(0)?,
        name: r.get(1)?,
        goal: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
    })
}

const WORKFLOW_COLS: &str = "SELECT id, name, goal, created_at, updated_at FROM workflows";

pub fn load_workflows(conn: &rusqlite::Connection) -> Result<Vec<Workflow>, String> {
    let mut stmt = conn
        .prepare(&format!("{WORKFLOW_COLS} ORDER BY updated_at DESC"))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], workflow_from_row)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

pub fn load_workflow(conn: &rusqlite::Connection, id: &str) -> Result<Option<Workflow>, String> {
    conn.query_row(
        &format!("{WORKFLOW_COLS} WHERE id = ?1"),
        [id],
        workflow_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn upsert_workflow(conn: &rusqlite::Connection, w: &Workflow) -> Result<(), String> {
    conn.execute(
        "INSERT INTO workflows (id, name, goal, created_at, updated_at) VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, goal=excluded.goal, updated_at=excluded.updated_at",
        rusqlite::params![w.id, w.name, w.goal, w.created_at, w.updated_at],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn delete_workflow_row(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM workflows WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ────────────── 校验 ──────────────

fn check_len(v: &str, max: usize, label: &str, field: &str) -> CommandResult<String> {
    let t = v.trim().to_string();
    if t.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: field.into(),
            value: v.to_string(),
            reason: format!("{label}不能为空"),
        });
    }
    if t.chars().count() > max {
        return Err(CommandError::InvalidArgument {
            field: field.into(),
            value: v.to_string(),
            reason: format!("{label}超过 {max} 字上限"),
        });
    }
    Ok(t)
}

/// 节点级校验：数量上限、标题/备注长度、依赖引用非空 + localId 唯一。
/// 环/悬空引用在指纹阶段判（draft_fingerprints）。
fn validate_nodes(nodes: &[WorkflowNodeDraft]) -> CommandResult<()> {
    if nodes.len() > MAX_WORKFLOW_NODES {
        return Err(CommandError::InvalidArgument {
            field: "nodes".into(),
            value: nodes.len().to_string(),
            reason: format!("节点数超过 {MAX_WORKFLOW_NODES} 上限"),
        });
    }
    for (i, n) in nodes.iter().enumerate() {
        if n.local_id.trim().is_empty() {
            return Err(CommandError::InvalidArgument {
                field: "nodes".into(),
                value: n.local_id.clone(),
                reason: format!("第 {} 个节点缺少 localId", i + 1),
            });
        }
        check_len(&n.title, MAX_NODE_TITLE, "节点标题", "nodes")?;
        if let Some(note) = &n.note {
            if note.chars().count() > MAX_NODE_NOTE {
                return Err(CommandError::InvalidArgument {
                    field: "nodes".into(),
                    value: note.to_string(),
                    reason: format!("第 {} 个节点备注超过 {MAX_NODE_NOTE} 字上限", i + 1),
                });
            }
        }
        if n.depends_on.iter().any(|d| d.trim().is_empty()) {
            return Err(CommandError::InvalidArgument {
                field: "nodes".into(),
                value: serde_json::to_string(&n.depends_on).unwrap_or_default(),
                reason: format!("第 {} 个节点存在空依赖引用", i + 1),
            });
        }
    }
    let mut seen = std::collections::HashSet::new();
    for n in nodes {
        if !seen.insert(n.local_id.as_str()) {
            return Err(CommandError::InvalidArgument {
                field: "nodes".into(),
                value: n.local_id.clone(),
                reason: format!("localId 重复：{}", n.local_id),
            });
        }
    }
    Ok(())
}

// ────────────── 递归内容指纹 ──────────────

const FP_SEP: char = '\u{1}';

fn fingerprint_of(
    title: &str,
    note: &Option<String>,
    tags: &Option<Vec<String>>,
    upstream: &[String],
) -> String {
    let mut tags_sorted: Vec<&str> = tags.iter().flatten().map(|s| s.as_str()).collect();
    tags_sorted.sort();
    format!(
        "t:{t}{FP_SEP}n:{n}{FP_SEP}g:{g}{FP_SEP}d:[{d}]",
        t = title.trim(),
        n = note.as_deref().map(str::trim).unwrap_or(""),
        g = tags_sorted.join(","),
        d = upstream.join(","),
    )
}

/// 画布草稿的递归内容指纹。memo 按节点下标缓存；环 → Err（闭环无法定义拓扑指纹）；
/// 依赖引用不在画布内 → Err（悬空引用拒绝保存）。
fn draft_fingerprints(
    nodes: &[WorkflowNodeDraft],
    index: &std::collections::HashMap<&str, usize>,
) -> CommandResult<Vec<String>> {
    let mut memo: Vec<Option<String>> = vec![None; nodes.len()];
    let mut visiting: Vec<bool> = vec![false; nodes.len()];
    let mut out = Vec::with_capacity(nodes.len());
    for i in 0..nodes.len() {
        out.push(draft_fp_at(i, nodes, index, &mut memo, &mut visiting)?);
    }
    Ok(out)
}

fn draft_fp_at(
    i: usize,
    nodes: &[WorkflowNodeDraft],
    index: &std::collections::HashMap<&str, usize>,
    memo: &mut Vec<Option<String>>,
    visiting: &mut Vec<bool>,
) -> CommandResult<String> {
    if let Some(fp) = &memo[i] {
        return Ok(fp.clone());
    }
    if visiting[i] {
        return Err(CommandError::InvalidArgument {
            field: "nodes".into(),
            value: nodes[i].title.clone(),
            reason: format!("节点「{}」存在循环依赖", nodes[i].title),
        });
    }
    visiting[i] = true;
    let mut upstream: Vec<String> = Vec::with_capacity(nodes[i].depends_on.len());
    for dep in &nodes[i].depends_on {
        let j = index
            .get(dep.as_str())
            .ok_or_else(|| CommandError::InvalidArgument {
                field: "nodes".into(),
                value: dep.clone(),
                reason: format!("节点「{}」的依赖「{dep}」不在本画布内", nodes[i].title),
            })?;
        upstream.push(draft_fp_at(*j, nodes, index, memo, visiting)?);
    }
    visiting[i] = false;
    upstream.sort();
    let fp = fingerprint_of(&nodes[i].title, &nodes[i].note, &nodes[i].tags, &upstream);
    memo[i] = Some(fp.clone());
    Ok(fp)
}

/// 已存任务卡的递归内容指纹（dependsOn 是真实任务 id）。
/// 上游悬空（不在本工作流集合内，含已删行）或成环 → 唯一不可匹配指纹（保存时按消失删除）。
fn task_fingerprints(tasks: &[Task]) -> Vec<String> {
    let by_id: std::collections::HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.id.as_str(), i))
        .collect();
    let mut memo: Vec<Option<String>> = vec![None; tasks.len()];
    let mut visiting: Vec<bool> = vec![false; tasks.len()];
    let mut out = Vec::with_capacity(tasks.len());
    for i in 0..tasks.len() {
        out.push(task_fp_at(i, tasks, &by_id, &mut memo, &mut visiting));
    }
    out
}

fn task_fp_at(
    i: usize,
    tasks: &[Task],
    by_id: &std::collections::HashMap<&str, usize>,
    memo: &mut Vec<Option<String>>,
    visiting: &mut Vec<bool>,
) -> String {
    if let Some(fp) = &memo[i] {
        return fp.clone();
    }
    if visiting[i] {
        return format!("__cycle__:{}", tasks[i].id);
    }
    visiting[i] = true;
    let mut upstream: Vec<String> = Vec::new();
    for dep in tasks[i].depends_on.iter().flatten() {
        match by_id.get(dep.as_str()) {
            Some(&j) => upstream.push(task_fp_at(j, tasks, by_id, memo, visiting)),
            // 悬空依赖：指向本工作流之外 → 唯一指纹，永不匹配 → 保存时删除重建
            None => upstream.push(format!("__dangling__:{dep}")),
        }
    }
    visiting[i] = false;
    upstream.sort();
    let fp = fingerprint_of(&tasks[i].title, &tasks[i].note, &tasks[i].tags, &upstream);
    memo[i] = Some(fp.clone());
    fp
}

// ────────────── 指纹 diff 保存（锁内事务，设计 §7） ──────────────

/// 保存的完整产出：result 给前端重建绑定；upserts/deleted_ids 由**锁内**写定，
/// 广播直接携带（OCR r1 high：锁外重读 DB 会与其他写者交错，快照可能与本次保存不一致）
pub(crate) struct WorkflowSaveOutcome {
    pub result: WorkflowSaveResult,
    pub upserts: Vec<Task>,
    pub deleted_ids: Vec<String>,
}

pub(crate) fn workflow_save_locked(
    conn: &mut rusqlite::Connection,
    input: WorkflowSaveInput,
    now: i64,
) -> CommandResult<WorkflowSaveOutcome> {
    debug_assert!(
        super::holding_db_write(),
        "workflow_save_locked 必须在持有 DB_WRITE_LOCK（lock_db_write()）时调用"
    );
    let name = check_len(&input.name, MAX_WORKFLOW_NAME, "工作流名称", "name")?;
    let goal = check_len(&input.goal, MAX_WORKFLOW_GOAL, "工作流目标", "goal")?;
    validate_nodes(&input.nodes)?;
    let index: std::collections::HashMap<&str, usize> = input
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.local_id.as_str(), i))
        .collect();
    // ① 指纹（内部完成环检测 + 悬空引用检测）
    let draft_fps = draft_fingerprints(&input.nodes, &index)?;

    // ② 工作流行 upsert（新建行 created_at=now；既有行保留 created_at）——
    // 一次 load 同时完成存在性校验与 created_at 读取（OCR r1 low：勿查两遍）
    let (wf_id, prev_row) = match &input.workflow_id {
        Some(id) => {
            let prev = load_workflow(conn, id)?.ok_or(CommandError::TaskNotFound(id.clone()))?;
            (id.clone(), Some(prev))
        }
        None => (uuid::Uuid::new_v4().simple().to_string(), None),
    };
    let (created_at, _) = match prev_row {
        Some(p) => (p.created_at, p.name),
        None => (Some(now), name.clone()),
    };
    upsert_workflow(
        conn,
        &Workflow {
            id: wf_id.clone(),
            name,
            goal,
            created_at,
            updated_at: Some(now),
        },
    )
    .map_err(CommandError::from)?;

    // ③ 现有节点卡 + 指纹
    let existing = load_tasks_by_workflow(conn, &wf_id).map_err(CommandError::from)?;
    let existing_fps = task_fingerprints(&existing);

    // ④ 匹配：fp 相同按出现顺序配对 → 保留原卡；未配对草稿 → 新建；未配对现有 → 删除
    let mut free_by_fp: std::collections::HashMap<&str, std::collections::VecDeque<usize>> =
        std::collections::HashMap::new();
    for (i, fp) in existing_fps.iter().enumerate() {
        free_by_fp.entry(fp.as_str()).or_default().push_back(i);
    }
    // 真实 id 预分配：保留卡 = 原任务 id，新卡 = 新 uuid（上游依赖据此映射）
    let mut real_id: Vec<String> = vec![String::new(); input.nodes.len()];
    let mut kept_flags: Vec<bool> = vec![false; input.nodes.len()];
    for (i, fp) in draft_fps.iter().enumerate() {
        if let Some(j) = free_by_fp.get_mut(fp.as_str()).and_then(|q| q.pop_front()) {
            real_id[i] = existing[j].id.clone();
            kept_flags[i] = true;
        } else {
            real_id[i] = uuid::Uuid::new_v4().simple().to_string();
        }
    }

    let tx = conn
        .transaction()
        .map_err(|e| CommandError::DbError(e.to_string()))?;

    let mut upserts: Vec<Task> = Vec::new();
    for (i, node) in input.nodes.iter().enumerate() {
        if kept_flags[i] {
            // 保留卡：仅坐标变更（RMW 基线 = 锁内现读的 updated_at；
            // column/result/budget/子任务等执行痕迹原样保留）
            let mut t = existing
                .iter()
                .find(|t| t.id == real_id[i])
                .expect("kept 节点必在 existing 中")
                .clone();
            t.canvas_pos = node.pos.clone();
            t.expected_updated_at = t.updated_at;
            t.updated_at = Some(now);
            upserts.push(t);
        } else {
            let deps: Vec<String> = node
                .depends_on
                .iter()
                .filter_map(|d| index.get(d.as_str()).map(|&j| real_id[j].clone()))
                .collect();
            upserts.push(Task {
                id: real_id[i].clone(),
                title: node.title.trim().to_string(),
                due: None,
                note: node
                    .note
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                tags: node.tags.clone(),
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
                updated_at: Some(now),
                schedule: None,
                sched_last: None,
                bot_assigned: None,
                assignee: None,
                budget: None,
                result: None,
                origin: Some(TASK_ORIGIN_WORKFLOW.to_string()),
                workflow_id: Some(wf_id.clone()),
                depends_on: Some(deps),
                canvas_pos: node.pos.clone(),
                expected_updated_at: None,
            });
        }
    }
    upsert_tasks(&tx, &upserts).map_err(CommandError::from)?;

    // 消失卡：未被任何草稿节点保留的现有卡 → 删除（改字=新卡契约的另一半）
    let kept_task_ids: std::collections::HashSet<&str> = input
        .nodes
        .iter()
        .zip(&kept_flags)
        .filter(|(_, kept)| **kept)
        .map(|(n, _)| {
            let i = index.get(n.local_id.as_str()).expect("localId 已校验");
            real_id[*i].as_str()
        })
        .collect();
    let deleted_ids: Vec<String> = existing
        .iter()
        .filter(|t| !kept_task_ids.contains(t.id.as_str()))
        .map(|t| t.id.clone())
        .collect();
    let deleted_n = deleted_ids.len();
    delete_tasks(&tx, &deleted_ids).map_err(CommandError::from)?;
    tx.commit()
        .map_err(|e| CommandError::DbError(e.to_string()))?;

    let created_n = input.nodes.len() - kept_flags.iter().filter(|k| **k).count();
    Ok(WorkflowSaveOutcome {
        result: WorkflowSaveResult {
            workflow_id: wf_id,
            bindings: input
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| WorkflowSaveBinding {
                    local_id: n.local_id.clone(),
                    task_id: real_id[i].clone(),
                    created: !kept_flags[i],
                })
                .collect(),
            kept: input.nodes.len() - created_n,
            created: created_n,
            deleted: deleted_n,
        },
        upserts,
        deleted_ids,
    })
}

// ────────────── Tauri commands ──────────────

fn audit_event(app: &AppHandle, event: &str, workflow_id: &str, extra: &[(&str, String)]) {
    let mut kvs: Vec<(&str, String)> = vec![("workflowId", workflow_id.to_string())];
    kvs.extend_from_slice(extra);
    crate::audit::write_event(app, crate::audit::AuditLevel::Info, event, &kvs);
}

#[tauri::command]
pub async fn workflow_save(
    app: AppHandle,
    input: WorkflowSaveInput,
) -> CommandResult<WorkflowSaveResult> {
    let app_emit = app.clone();
    let outcome = async_runtime::spawn_blocking(move || {
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        let now = chrono::Utc::now().timestamp_millis();
        workflow_save_locked(&mut conn, input, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流保存线程 join 失败：{e}")))??;
    let r = &outcome.result;
    audit_event(
        &app_emit,
        "workflow_save",
        &r.workflow_id,
        &[
            ("kept", r.kept.to_string()),
            ("created", r.created.to_string()),
            ("deleted", r.deleted.to_string()),
        ],
    );
    // 广播：挂件/主窗收敛（与 task_patch 同款协议）。载荷 = 锁内写定的行，
    // 不锁外重读（OCR r1 high TOCTOU）
    {
        use tauri::Emitter;
        let _ = app_emit.emit("tasks-changed", ());
        let _ = app_emit.emit_to(
            "main",
            "tasks-updated",
            serde_json::json!({
                "source": crate::mutation::MutationOrigin::Main.as_str(),
                "upserts": outcome.upserts,
                "deletes": outcome.deleted_ids
            }),
        );
    }
    Ok(outcome.result)
}

#[tauri::command]
pub async fn workflow_list(app: AppHandle) -> CommandResult<Vec<Workflow>> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        load_workflows(&conn).map_err(CommandError::from)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流列表线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workflow_load(app: AppHandle, id: String) -> CommandResult<WorkflowDetail> {
    async_runtime::spawn_blocking(move || {
        let conn = super::open_db(&app)?;
        let workflow = load_workflow(&conn, &id)?.ok_or(CommandError::TaskNotFound(id.clone()))?;
        let tasks = load_tasks_by_workflow(&conn, &id).map_err(CommandError::from)?;
        Ok(WorkflowDetail { workflow, tasks })
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流读取线程 join 失败：{e}")))?
}

#[tauri::command]
pub async fn workflow_rename(app: AppHandle, id: String, name: String) -> CommandResult<Workflow> {
    let app_emit = app.clone();
    let row = async_runtime::spawn_blocking(move || -> CommandResult<Workflow> {
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        let mut wf = load_workflow(&conn, &id)?.ok_or(CommandError::TaskNotFound(id.clone()))?;
        let name = check_len(&name, MAX_WORKFLOW_NAME, "工作流名称", "name")?;
        wf.name = name;
        wf.updated_at = Some(chrono::Utc::now().timestamp_millis());
        upsert_workflow(&conn, &wf).map_err(CommandError::from)?;
        Ok(wf)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流重命名线程 join 失败：{e}")))??;
    audit_event(&app_emit, "workflow_rename", &row.id, &[]);
    Ok(row)
}

/// 删除工作流：级联硬删全部节点卡（工作流卡不进回收站——软删会使
/// 「workflow_id 外联 + 指纹重建」语义含混；确认弹窗在画布侧）
#[tauri::command]
pub async fn workflow_delete(app: AppHandle, id: String) -> CommandResult<usize> {
    let app_emit = app.clone();
    let id_for_audit = id.clone();
    let (result, deleted_ids) =
        async_runtime::spawn_blocking(move || -> CommandResult<(usize, Vec<String>)> {
            let _g = super::lock_db_write();
            let mut conn = super::open_db(&app)?;
            if load_workflow(&conn, &id)?.is_none() {
                return Err(CommandError::TaskNotFound(id.clone()));
            }
            let tx = conn
                .transaction()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            let ids: Vec<String> = load_tasks_by_workflow(&tx, &id)
                .map_err(CommandError::from)?
                .iter()
                .map(|t| t.id.clone())
                .collect();
            delete_tasks(&tx, &ids).map_err(CommandError::from)?;
            delete_workflow_row(&tx, &id).map_err(CommandError::from)?;
            tx.commit()
                .map_err(|e| CommandError::DbError(e.to_string()))?;
            Ok((ids.len(), ids))
        })
        .await
        .map_err(|e| CommandError::from(format!("工作流删除线程 join 失败：{e}")))??;
    audit_event(
        &app_emit,
        "workflow_delete",
        &id_for_audit,
        &[("deleted", result.to_string())],
    );
    {
        use tauri::Emitter;
        let _ = app_emit.emit("tasks-changed", ());
        let _ = app_emit.emit_to(
            "main",
            "tasks-updated",
            serde_json::json!({
                "source": crate::mutation::MutationOrigin::Main.as_str(),
                "upserts": [],
                "deletes": deleted_ids
            }),
        );
    }
    Ok(result)
}

// ────────────── 单测 ──────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::tasks::TASKS_DDL).unwrap();
        conn.execute_batch(WORKFLOWS_DDL).unwrap();
        conn
    }

    fn node(local_id: &str, title: &str, deps: &[&str]) -> WorkflowNodeDraft {
        WorkflowNodeDraft {
            local_id: local_id.to_string(),
            task_id: None,
            title: title.to_string(),
            note: None,
            tags: None,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            pos: Some(CanvasPos { x: 10.0, y: 20.0 }),
        }
    }

    fn save(conn: &mut rusqlite::Connection, nodes: Vec<WorkflowNodeDraft>) -> WorkflowSaveResult {
        save_into(conn, None, nodes)
    }

    fn save_into(
        conn: &mut rusqlite::Connection,
        workflow_id: Option<String>,
        nodes: Vec<WorkflowNodeDraft>,
    ) -> WorkflowSaveResult {
        workflow_save_locked(
            conn,
            WorkflowSaveInput {
                workflow_id,
                name: "测试工作流".into(),
                goal: "目标".into(),
                nodes,
            },
            1_000,
        )
        .unwrap()
        .result
    }

    #[test]
    fn save_new_creates_workflow_and_task_rows() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r = save(
            &mut conn,
            vec![node("n1", "收集", &[]), node("n2", "汇总", &["n1"])],
        );
        assert_eq!(r.created, 2);
        assert_eq!(r.kept, 0);
        assert_eq!(r.deleted, 0);
        assert_eq!(r.bindings.len(), 2);
        let tasks = load_tasks_by_workflow(&conn, &r.workflow_id).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks
            .iter()
            .all(|t| t.origin.as_deref() == Some("workflow")));
        let child = tasks.iter().find(|t| t.title == "汇总").unwrap();
        let parent_id = tasks.iter().find(|t| t.title == "收集").unwrap().id.clone();
        assert_eq!(child.depends_on.as_ref().unwrap(), &vec![parent_id]);
        // workflows 行存在
        assert!(load_workflow(&conn, &r.workflow_id).unwrap().is_some());
    }

    #[test]
    fn save_identical_content_keeps_task_ids() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r1 = save(
            &mut conn,
            vec![node("n1", "A", &[]), node("n2", "B", &["n1"])],
        );
        let ids1: Vec<String> = r1.bindings.iter().map(|b| b.task_id.clone()).collect();
        // 第二次保存：同样内容（画布重新生成过、localId/taskId 全新）→ 指纹命中，全部保留
        let r2 = save_into(
            &mut conn,
            Some(r1.workflow_id.clone()),
            vec![node("x1", "A", &[]), node("x2", "B", &["x1"])],
        );
        assert_eq!(r2.kept, 2);
        assert_eq!(r2.created, 0);
        assert_eq!(r2.deleted, 0);
        let ids2: Vec<String> = r2.bindings.iter().map(|b| b.task_id.clone()).collect();
        assert_eq!(ids1, ids2, "内容相同的两次保存必须保留原任务 id");
    }

    #[test]
    fn save_same_content_different_positions_updates_pos_only() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r1 = save(&mut conn, vec![node("n1", "A", &[])]);
        let id1 = r1.bindings[0].task_id.clone();
        let r2 = save_into(
            &mut conn,
            Some(r1.workflow_id.clone()),
            vec![WorkflowNodeDraft {
                pos: Some(CanvasPos { x: 999.0, y: -5.0 }),
                ..node("x1", "A", &[])
            }],
        );
        assert_eq!(r2.kept, 1);
        let t = load_tasks_by_workflow(&conn, &r2.workflow_id).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].id, id1);
        assert_eq!(t[0].canvas_pos.as_ref().unwrap().x, 999.0);
        assert_eq!(t[0].canvas_pos.as_ref().unwrap().y, -5.0);
    }

    #[test]
    fn save_title_change_replaces_task() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r1 = save(&mut conn, vec![node("n1", "A", &[])]);
        let old_id = r1.bindings[0].task_id.clone();
        let r2 = save_into(
            &mut conn,
            Some(r1.workflow_id.clone()),
            vec![node("x1", "A改", &[])],
        );
        assert_eq!(r2.created, 1);
        assert_eq!(r2.deleted, 1);
        let tasks = load_tasks_by_workflow(&conn, &r2.workflow_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_ne!(
            tasks[0].id, old_id,
            "内容变了必须换新卡（v1 契约：改字=新卡）"
        );
    }

    #[test]
    fn save_removed_node_deletes_task() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        save(&mut conn, vec![node("n1", "A", &[]), node("n2", "B", &[])]);
        let r1 = load_workflows(&conn).unwrap().remove(0);
        let r2 = save_into(&mut conn, Some(r1.id.clone()), vec![node("x1", "A", &[])]);
        assert_eq!(r2.deleted, 1);
        assert_eq!(
            load_tasks_by_workflow(&conn, &r2.workflow_id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn save_rejects_cycle_self_dep_and_dangling_ref() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let mut run = |nodes: Vec<WorkflowNodeDraft>| {
            workflow_save_locked(
                &mut conn,
                WorkflowSaveInput {
                    workflow_id: None,
                    name: "x".into(),
                    goal: "g".into(),
                    nodes,
                },
                1_000,
            )
        };
        // 自环
        let err = run(vec![node("n1", "A", &["n1"])])
            .map(|o| o.result)
            .unwrap_err();
        assert!(err.to_string().contains("循环依赖"));
        // 间接环 A→B→A
        let err = run(vec![node("n1", "A", &["n2"]), node("n2", "B", &["n1"])])
            .map(|o| o.result)
            .unwrap_err();
        assert!(err.to_string().contains("循环依赖"));
        // 悬空引用
        let err = run(vec![node("n1", "A", &["不存在"])])
            .map(|o| o.result)
            .unwrap_err();
        assert!(err.to_string().contains("不在本画布内"));
        // localId 重复
        let err = run(vec![node("n1", "A", &[]), node("n1", "B", &[])])
            .map(|o| o.result)
            .unwrap_err();
        assert!(err.to_string().contains("重复"));
    }

    #[test]
    fn save_rejects_over_limits() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let mut run = |nodes: Vec<WorkflowNodeDraft>| {
            workflow_save_locked(
                &mut conn,
                WorkflowSaveInput {
                    workflow_id: None,
                    name: "x".into(),
                    goal: "g".into(),
                    nodes,
                },
                1_000,
            )
        };
        // 节点数上限
        let nodes: Vec<WorkflowNodeDraft> = (0..MAX_WORKFLOW_NODES + 1)
            .map(|i| node(&format!("n{i}"), &format!("N{i}"), &[]))
            .collect();
        let err = run(nodes).map(|o| o.result).unwrap_err();
        assert!(err.to_string().contains("上限"));
        // 空标题
        let err = run(vec![node("n1", "   ", &[])])
            .map(|o| o.result)
            .unwrap_err();
        assert!(err.to_string().contains("不能为空"));
        // 空名称
        let err = workflow_save_locked(
            &mut conn,
            WorkflowSaveInput {
                workflow_id: None,
                name: "  ".into(),
                goal: "g".into(),
                nodes: vec![],
            },
            1_000,
        )
        .map(|o| o.result)
        .unwrap_err();
        assert!(err.to_string().contains("不能为空"));
    }

    #[test]
    fn save_empty_nodes_clears_all() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r1 = save(&mut conn, vec![node("n1", "A", &[])]);
        let r2 = workflow_save_locked(
            &mut conn,
            WorkflowSaveInput {
                workflow_id: Some(r1.workflow_id.clone()),
                name: "测试工作流".into(),
                goal: "目标".into(),
                nodes: vec![],
            },
            2_000,
        )
        .unwrap();
        assert_eq!(r2.result.deleted, 1);
        assert!(load_tasks_by_workflow(&conn, &r2.result.workflow_id)
            .unwrap()
            .is_empty());
        // 工作流行仍在
        assert!(load_workflow(&conn, &r2.result.workflow_id)
            .unwrap()
            .is_some());
    }

    #[test]
    fn delete_workflow_cascades_tasks() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r = save(
            &mut conn,
            vec![node("n1", "A", &[]), node("n2", "B", &["n1"])],
        );
        let tx = conn.transaction().unwrap();
        let ids: Vec<String> = load_tasks_by_workflow(&tx, &r.workflow_id)
            .unwrap()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        delete_tasks(&tx, &ids).unwrap();
        delete_workflow_row(&tx, &r.workflow_id).unwrap();
        tx.commit().unwrap();
        assert_eq!(ids.len(), 2);
        assert!(load_workflow(&conn, &r.workflow_id).unwrap().is_none());
        assert!(load_tasks_by_workflow(&conn, &r.workflow_id)
            .unwrap()
            .is_empty());
    }
}
