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
/// 每卡验收标准上限（W-QA 卡即契约）：一行可验证的完成标准
pub const MAX_NODE_ACCEPTANCE: usize = 120;

/// 单源 DDL：open_db 与测试建表共用，防两处 schema 漂移
pub const WORKFLOWS_DDL: &str = "CREATE TABLE IF NOT EXISTS workflows (
   id          TEXT PRIMARY KEY,
   name        TEXT NOT NULL,
   goal        TEXT NOT NULL,
   created_at  INTEGER,
   updated_at  INTEGER,
   attachments TEXT,
   schedule    TEXT,
   sched_last  INTEGER,
   enabled     INTEGER,
   last_report TEXT,
   last_report_at INTEGER
 );";

/// workflows.attachments 幂等 ALTER（W8-ATTACH：老库的 workflows 表无此列）
pub fn ensure_workflows_attachments(conn: &rusqlite::Connection) -> Result<(), String> {
    let has: bool = conn
        .prepare("PRAGMA table_info(workflows)")
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
            Ok(rows.filter_map(|n| n.ok()).any(|n| n == "attachments"))
        })
        .map_err(|e| e.to_string())?;
    if !has {
        conn.execute("ALTER TABLE workflows ADD COLUMN attachments TEXT", [])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// workflows 定时三列幂等 ALTER（定时任务模块：到点自动执行整张工作流）。
/// enabled NULL 恒等于启用（与 tasks.enabled 同语义）。
pub fn ensure_workflows_schedule(conn: &rusqlite::Connection) -> Result<(), String> {
    for (col, ty) in [
        ("schedule", "TEXT"),
        ("sched_last", "INTEGER"),
        ("enabled", "INTEGER"),
    ] {
        let has: bool = conn
            .prepare("PRAGMA table_info(workflows)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == col))
            })
            .map_err(|e| e.to_string())?;
        if !has {
            conn.execute(&format!("ALTER TABLE workflows ADD COLUMN {col} {ty}"), [])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// workflows 收尾审校报告两列幂等 ALTER（W-QA：runner 结算后写入，
/// 画布 GoalNode 展示；upsert_workflow 的 ON CONFLICT 不更新此二列）
pub fn ensure_workflows_report(conn: &rusqlite::Connection) -> Result<(), String> {
    for (col, ty) in [("last_report", "TEXT"), ("last_report_at", "INTEGER")] {
        let has: bool = conn
            .prepare("PRAGMA table_info(workflows)")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
                Ok(rows.filter_map(|n| n.ok()).any(|n| n == col))
            })
            .map_err(|e| e.to_string())?;
        if !has {
            conn.execute(&format!("ALTER TABLE workflows ADD COLUMN {col} {ty}"), [])
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub goal: String,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    /// 拆解附件路径清单（W8-ATTACH，JSON 数组；仅本机语义，不进导出文件）
    pub attachments: Option<Vec<String>>,
    /// 定时执行规则（定时任务模块）：daily:HH:MM / weekly:D:HH:MM / monthly:DD:HH:MM /
    /// at:YYYY-MM-DDTHH:MM；None = 未定时。到点由 bot_scheduler 触发整张工作流。
    #[serde(default)]
    pub schedule: Option<String>,
    /// 上次定时触发时间（epoch ms）
    #[serde(default)]
    pub sched_last: Option<i64>,
    /// 定时启用开关：None/Some(true) = 启用，Some(false) = 暂停（保留配置）
    #[serde(default)]
    pub enabled: Option<bool>,
    /// 上轮执行的收尾审校报告（W-QA，JSON：verdict/overall/issues）；None = 从未评审
    #[serde(default)]
    pub last_report: Option<String>,
    /// 报告写入时间（epoch ms）
    #[serde(default)]
    pub last_report_at: Option<i64>,
}

/// workflow_save 的单个节点草稿。taskId = 已保存卡的绑定提示（服务端不信任，
/// 保留与否由指纹裁决）；dependsOn 引用本列表内其他节点的 localId。
#[derive(Deserialize, Debug)]
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
    /// 执行模型覆盖（W6-MODEL）：模型库条目 id；None = 跟随全局
    #[serde(default)]
    pub model: Option<String>,
    /// 子任务清单（W8-ATTACH）：仅新建卡构建（保留卡保护执行痕迹）
    #[serde(default)]
    pub subtasks: Option<Vec<String>>,
    /// 每卡验收标准（W-QA 卡即契约）：拆解生成的一行可验证完成标准
    #[serde(default)]
    pub acceptance: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSaveInput {
    /// None = 新建工作流；Some = 覆盖保存既有工作流
    #[serde(default)]
    pub workflow_id: Option<String>,
    pub name: String,
    pub goal: String,
    pub nodes: Vec<WorkflowNodeDraft>,
    /// 拆解附件路径清单（W8-ATTACH）：随保存落 workflows 行（重新生成可复用）
    #[serde(default)]
    pub attachments: Option<Vec<String>>,
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
    let attachments: Option<String> = r.get(5)?;
    Ok(Workflow {
        id: r.get(0)?,
        name: r.get(1)?,
        goal: r.get(2)?,
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
        attachments: attachments.and_then(|s| serde_json::from_str(&s).ok()),
        schedule: r.get(6)?,
        sched_last: r.get(7)?,
        enabled: r.get::<_, Option<i64>>(8)?.map(|v| v != 0),
        last_report: r.get(9)?,
        last_report_at: r.get(10)?,
    })
}

const WORKFLOW_COLS: &str =
    "SELECT id, name, goal, created_at, updated_at, attachments, schedule, sched_last, enabled, \
     last_report, last_report_at FROM workflows";

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
    let attachments = match w.attachments.as_ref() {
        Some(a) => Some(serde_json::to_string(a).map_err(|e| e.to_string())?),
        None => None,
    };
    conn.execute(
        "INSERT INTO workflows (id, name, goal, created_at, updated_at, attachments, schedule, sched_last, enabled) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, goal=excluded.goal, updated_at=excluded.updated_at, attachments=excluded.attachments",
        rusqlite::params![
            w.id, w.name, w.goal, w.created_at, w.updated_at, attachments,
            w.schedule, w.sched_last, w.enabled.map(|b| b as i64)
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn delete_workflow_row(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    conn.execute("DELETE FROM workflows WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 收尾审校报告定点写（W-QA）：upsert_workflow 不触碰此二列（画布保存不冲掉报告）
pub(crate) fn workflow_set_report(
    conn: &rusqlite::Connection,
    id: &str,
    report: &str,
    at: i64,
) -> Result<(), String> {
    conn.execute(
        "UPDATE workflows SET last_report = ?2, last_report_at = ?3 WHERE id = ?1",
        rusqlite::params![id, report, at],
    )
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
        // 验收标准校验（W-QA）：与拆解侧同规则 ≤120 字
        if let Some(acc) = &n.acceptance {
            if acc.chars().count() > MAX_NODE_ACCEPTANCE {
                return Err(CommandError::InvalidArgument {
                    field: "nodes".into(),
                    value: acc.to_string(),
                    reason: format!(
                        "第 {} 个节点验收标准超过 {MAX_NODE_ACCEPTANCE} 字上限",
                        i + 1
                    ),
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
        // 子任务清单校验（W8-ATTACH，与拆解侧同规则）：≤8 条 × ≤60 字
        if let Some(list) = &n.subtasks {
            if list.len() > 8 {
                return Err(CommandError::InvalidArgument {
                    field: "nodes".into(),
                    value: list.len().to_string(),
                    reason: format!("第 {} 个节点子任务超过 8 条上限", i + 1),
                });
            }
            for t in list {
                if t.trim().chars().count() > 60 {
                    return Err(CommandError::InvalidArgument {
                        field: "nodes".into(),
                        value: t.clone(),
                        reason: format!("第 {} 个节点的子任务超过 60 字上限", i + 1),
                    });
                }
            }
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
    acceptance: &Option<String>,
    upstream: &[String],
) -> String {
    let mut tags_sorted: Vec<&str> = tags.iter().flatten().map(|s| s.as_str()).collect();
    tags_sorted.sort();
    format!(
        "t:{t}{FP_SEP}n:{n}{FP_SEP}g:{g}{FP_SEP}a:{a}{FP_SEP}d:[{d}]",
        t = title.trim(),
        n = note.as_deref().map(str::trim).unwrap_or(""),
        g = tags_sorted.join(","),
        // W-QA：验收标准是卡内容的一部分——改验收 = 改字 = 删旧建新（与 note 同语义）
        a = acceptance.as_deref().map(str::trim).unwrap_or(""),
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
    let fp = fingerprint_of(
        &nodes[i].title,
        &nodes[i].note,
        &nodes[i].tags,
        &nodes[i].acceptance,
        &upstream,
    );
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
    let fp = fingerprint_of(
        &tasks[i].title,
        &tasks[i].note,
        &tasks[i].tags,
        &tasks[i].acceptance,
        &upstream,
    );
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
    // workflows 行与节点卡**同一事务**（全量对照 high：原先行在事务外先落，
    // 后续失败会留下无节点的孤儿工作流行，违背"锁内事务"承诺）
    upsert_workflow(
        &tx,
        &Workflow {
            id: wf_id.clone(),
            name,
            goal,
            created_at,
            updated_at: Some(now),
            attachments: input.attachments.clone(),
            schedule: None, // 定时配置不随画布保存重置（ON CONFLICT 不更新此字段）
            sched_last: None,
            enabled: None,
            last_report: None, // 报告由 runner 结算写入，画布保存不触碰
            last_report_at: None,
        },
    )
    .map_err(CommandError::from)?;

    let mut upserts: Vec<Task> = Vec::with_capacity(input.nodes.len());
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
            // model 同步以草稿为准（None = 跟随全局；保留卡也允许改模型——
            // W6 r1 high/critical：只改模型不改标题的场景指纹命中，不同步则静默丢弃）
            t.model = node.model.clone();
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
                // W8-ATTACH（OCR r1 critical）：草稿子任务清单 → 新卡 Subtask
                //（uuid + 未勾选）；保留卡不覆盖（执行痕迹保护）
                subtasks: node
                    .subtasks
                    .clone()
                    .filter(|list| !list.is_empty())
                    .map(|list| {
                        list.iter()
                            .filter(|t| !t.trim().is_empty())
                            .map(|t| super::tasks::Subtask {
                                id: uuid::Uuid::new_v4().simple().to_string(),
                                text: t.trim().to_string(),
                                done: false,
                            })
                            .collect()
                    }),
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
                model: node.model.clone(),
                acceptance: node
                    .acceptance
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                owner_id: None,        // 本机创建的工作流卡 = 本人
                created_at: Some(now), // 创建时间打戳（与 updated_at 同值；此后 UPDATE 不覆盖）
                enabled: None,         // 新建节点卡无定时配置
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
    let mut deleted_ids: Vec<String> = Vec::with_capacity(existing.len());
    deleted_ids.extend(
        existing
            .iter()
            .filter(|t| !kept_task_ids.contains(t.id.as_str()))
            .map(|t| t.id.clone()),
    );
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

// ────────────── 文件格式 v1（W4-TEMPLATE，设计 §4）──────────────

pub const WORKFLOW_FILE_VERSION: u32 = 1;

/// 模板文件读取上限（30 节点文件 ≈ 10KB，1MB 已是百倍余量；
/// bounded read 同 tasks_import 的 check-then-act 防御）
const MAX_IMPORT_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFileNode {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub pos: Option<CanvasPos>,
    /// 执行模型覆盖（W6-MODEL）
    #[serde(default)]
    pub model: Option<String>,
    /// 子任务文本清单（W8-ATTACH）
    #[serde(default)]
    pub subtasks: Option<Vec<String>>,
    /// 每卡验收标准（W-QA 卡即契约；旧模板缺省 = 无）
    #[serde(default)]
    pub acceptance: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowFile {
    pub version: u32,
    #[serde(default)]
    pub generator: Option<String>,
    #[serde(default)]
    pub exported_at: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// 缺省回退 name（导入宽松项；其余违规整包拒绝）
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub nodes: Vec<WorkflowFileNode>,
}

/// 拓扑导出序（纯逻辑，单测锚点）：多轮扫描就绪节点（依赖已全放置即就绪），
/// 同轮按输入序稳定输出；悬空/自环依赖不阻塞（导出端防御，导入端另有校验）；
/// 环内节点按输入序追加在后（不影响本地 id 映射的唯一性）。
/// 入参是 (id, deps) 视图——topo 只需要这两样，避免整卡深拷贝（OCR r1）。
pub(crate) fn topo_export_order(items: &[(String, Vec<String>)]) -> Vec<String> {
    let ids: std::collections::HashSet<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    loop {
        let mut progressed = false;
        for (id, deps) in items {
            if placed.contains(id) {
                continue;
            }
            let blocked = deps
                .iter()
                .any(|d| ids.contains(d.as_str()) && d != id && !placed.contains(d));
            if !blocked {
                placed.insert(id.clone());
                out.push(id.clone());
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    for (id, _) in items {
        if !placed.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// 工作流 + 节点卡 → 文件结构（纯逻辑，单测锚点）：按 (order,id) 稳定排序后
/// 拓扑排列，真实任务 id 映射为 n1..nN 本地 id，dependsOn 同步重映射；
/// 悬空依赖（指向图外/已删行）在映射时丢弃——导入端 save 链会对剩余引用做全量校验。
pub(crate) fn workflow_file_from(wf: &Workflow, tasks: &[Task]) -> WorkflowFile {
    let mut sorted: Vec<&Task> = tasks.iter().collect();
    sorted.sort_by(|a, b| {
        a.order
            .unwrap_or(0.0)
            .partial_cmp(&b.order.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.id.cmp(&b.id))
    });
    // (id, deps) 视图：topo 只需要这两样，避免整卡深拷贝（OCR r1 medium）
    let id_deps: Vec<(String, Vec<String>)> = sorted
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                t.depends_on.iter().flatten().cloned().collect(),
            )
        })
        .collect();
    let topo = topo_export_order(&id_deps);
    let id_map: std::collections::HashMap<String, String> = topo
        .iter()
        .enumerate()
        .map(|(i, id)| (id.clone(), format!("n{}", i + 1)))
        .collect();
    let by_id: std::collections::HashMap<&str, &Task> =
        sorted.iter().map(|t| (t.id.as_str(), *t)).collect();
    let nodes = topo
        .iter()
        .filter_map(|id| by_id.get(id.as_str()).map(|t| (id, *t)))
        .map(|(id, t)| WorkflowFileNode {
            id: id_map[id].clone(),
            title: t.title.clone(),
            note: t.note.clone(),
            tags: t.tags.clone(),
            depends_on: t
                .depends_on
                .iter()
                .flatten()
                .filter_map(|d| id_map.get(d).cloned())
                .collect(),
            pos: t.canvas_pos.clone(),
            model: t.model.clone(),
            subtasks: t
                .subtasks
                .as_ref()
                .map(|list| list.iter().map(|st| st.text.clone()).collect()),
            acceptance: t.acceptance.clone(),
        })
        .collect();
    WorkflowFile {
        version: WORKFLOW_FILE_VERSION,
        generator: Some(format!("wmessage {}", env!("CARGO_PKG_VERSION"))),
        exported_at: Some(chrono::Utc::now().to_rfc3339()),
        name: wf.name.clone(),
        description: None,
        goal: Some(wf.goal.clone()),
        nodes,
    }
}

/// 文件 → 保存草稿（纯逻辑，单测锚点）：文件级校验（version/name/goal/数量/id 唯一）
/// + dependsOn 去重保序；图规则（环/悬空/长度/重名）交给 workflow_save_locked 既有链。
pub(crate) fn parse_workflow_file(raw: &str) -> CommandResult<WorkflowSaveInput> {
    let file: WorkflowFile =
        serde_json::from_str(raw).map_err(|e| CommandError::InvalidArgument {
            field: "file".into(),
            value: raw.chars().take(120).collect(),
            reason: format!("不是有效的 .wflow.json：{e}"),
        })?;
    if file.version != WORKFLOW_FILE_VERSION {
        return Err(CommandError::InvalidArgument {
            field: "version".into(),
            value: file.version.to_string(),
            reason: format!("文件版本不支持（需要 {WORKFLOW_FILE_VERSION}）"),
        });
    }
    let name = check_len(&file.name, MAX_WORKFLOW_NAME, "工作流名称", "name")?;
    let goal = check_len(
        file.goal.as_deref().unwrap_or(&name),
        MAX_WORKFLOW_GOAL,
        "工作流目标",
        "goal",
    )?;
    if file.nodes.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "nodes".into(),
            value: "0".into(),
            reason: "至少需要 1 个节点".into(),
        });
    }
    if file.nodes.len() > MAX_WORKFLOW_NODES {
        return Err(CommandError::InvalidArgument {
            field: "nodes".into(),
            value: file.nodes.len().to_string(),
            reason: format!("节点数超过 {MAX_WORKFLOW_NODES} 上限"),
        });
    }
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for n in &file.nodes {
        if n.id.trim().is_empty() {
            return Err(CommandError::InvalidArgument {
                field: "nodes".into(),
                value: n.id.clone(),
                reason: "节点 id 不能为空".into(),
            });
        }
        if !seen.insert(n.id.as_str()) {
            return Err(CommandError::InvalidArgument {
                field: "nodes".into(),
                value: n.id.clone(),
                reason: format!("节点 id 重复：{}", n.id),
            });
        }
    }
    let nodes = file
        .nodes
        .into_iter()
        .map(|mut n| {
            let mut depends_on: Vec<String> = Vec::new();
            for d in n.depends_on {
                if !depends_on.contains(&d) {
                    depends_on.push(d);
                }
            }
            WorkflowNodeDraft {
                acceptance: n.acceptance.take(),
                local_id: n.id,
                task_id: None,
                title: n.title,
                note: n.note,
                tags: n.tags,
                depends_on,
                pos: n.pos,
                model: n.model.take().filter(|m| !m.trim().is_empty()),
                subtasks: n.subtasks,
            }
        })
        .collect();
    Ok(WorkflowSaveInput {
        workflow_id: None,
        name,
        goal,
        nodes,
        attachments: None,
    })
}

/// 导出 .wflow.json（实例化语义的另一半：库内真实 id → 文件本地 id）
#[tauri::command]
pub async fn workflow_export(
    app: AppHandle,
    workflow_id: String,
    path: String,
) -> CommandResult<usize> {
    crate::db::tasks::check_export_path(&path)?;
    let file = {
        let app = app.clone();
        let wid = workflow_id.clone();
        async_runtime::spawn_blocking(move || -> CommandResult<WorkflowFile> {
            let conn = super::open_db(&app)?;
            let wf = load_workflow(&conn, &wid)?
                .ok_or_else(|| CommandError::TaskNotFound(wid.clone()))?;
            let tasks = load_tasks_by_workflow(&conn, &wid).map_err(CommandError::from)?;
            Ok(workflow_file_from(&wf, &tasks))
        })
        .await
        .map_err(|e| CommandError::from(format!("工作流导出线程 join 失败：{e}")))??
    };
    let json =
        serde_json::to_string_pretty(&file).map_err(|e| CommandError::from(e.to_string()))?;
    {
        let path2 = path.clone();
        async_runtime::spawn_blocking(move || {
            super::paths::atomic_write(std::path::Path::new(&path2), &json)
                .map_err(|e| format!("写入文件失败：{e}"))
        })
        .await
        .map_err(|e| CommandError::from(format!("工作流导出写入线程 join 失败：{e}")))?
        .map_err(CommandError::from)?;
    }
    audit_event(
        &app,
        "workflow_export",
        &workflow_id,
        &[
            ("nodes", file.nodes.len().to_string()),
            ("path", crate::bot::truncate_for_log(&path, 120)),
        ],
    );
    Ok(file.nodes.len())
}

/// 导入 .wflow.json = **实例化**：全新 workflow 行 + 全新任务 id（设计 §4；
/// 与 tasks_import 的按 id 合并刻意分离）。图规则全部由 workflow_save_locked 承接。
#[tauri::command]
pub async fn workflow_import(app: AppHandle, path: String) -> CommandResult<WorkflowSaveResult> {
    crate::db::tasks::check_export_path(&path)?;
    let path_for_audit = path.clone(); // 审计用；本体 move 进读文件闭包
    let input = {
        async_runtime::spawn_blocking(move || -> CommandResult<WorkflowSaveInput> {
            let f = std::fs::File::open(&path)
                .map_err(|e| CommandError::from(format!("无法读取所选文件：{e}")))?;
            let mut limited = std::io::Read::take(f, MAX_IMPORT_FILE_BYTES + 1);
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut limited, &mut buf)
                .map_err(|e| CommandError::from(format!("无法读取所选文件：{e}")))?;
            if buf.len() as u64 > MAX_IMPORT_FILE_BYTES {
                return Err(CommandError::InvalidArgument {
                    field: "path".into(),
                    value: path,
                    reason: "模板文件超过 1MB 上限".into(),
                });
            }
            let raw = String::from_utf8(buf)
                .map_err(|e| CommandError::from(format!("不是有效的 UTF-8 文本：{e}")))?;
            parse_workflow_file(&raw)
        })
        .await
        .map_err(|e| CommandError::from(format!("工作流导入解析线程 join 失败：{e}")))??
    };
    let app_emit = app.clone();
    let outcome = async_runtime::spawn_blocking(move || {
        let _g = super::lock_db_write();
        let mut conn = super::open_db(&app)?;
        let now = chrono::Utc::now().timestamp_millis();
        workflow_save_locked(&mut conn, input, now)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流导入线程 join 失败：{e}")))??;
    let r = &outcome.result;
    audit_event(
        &app_emit,
        "workflow_import",
        &r.workflow_id,
        &[
            ("imported", r.created.to_string()),
            ("path", crate::bot::truncate_for_log(&path_for_audit, 120)),
        ],
    );
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

/// 设置/取消工作流定时（定时任务模块）。schedule=None = 取消；
/// 非法格式（bot_scheduler::validate_schedule 不认）响亮拒绝。
/// 只动 schedule 字段；sched_last 由调度器维护（与任务卡 task_patch 通道同语义）。
#[tauri::command]
pub async fn workflow_set_schedule(
    app: AppHandle,
    id: String,
    schedule: Option<String>,
) -> CommandResult<Workflow> {
    if let Some(s) = &schedule {
        let s = s.trim();
        if s.is_empty() {
            return Err(CommandError::InvalidArgument {
                field: "schedule".into(),
                value: s.to_string(),
                reason: "schedule 不能为空串（取消定时请传 null）".into(),
            });
        }
        crate::bot_scheduler::validate_schedule(s).map_err(|reason| {
            CommandError::InvalidArgument {
                field: "schedule".into(),
                value: s.to_string(),
                reason,
            }
        })?;
    }
    let app_emit = app.clone();
    let row = async_runtime::spawn_blocking(move || -> CommandResult<Workflow> {
        let _g = super::lock_db_write();
        let conn = super::open_db(&app)?;
        let mut wf = load_workflow(&conn, &id)?.ok_or(CommandError::TaskNotFound(id.clone()))?;
        wf.schedule = schedule.map(|s| s.trim().to_string());
        wf.updated_at = Some(chrono::Utc::now().timestamp_millis());
        upsert_workflow(&conn, &wf).map_err(CommandError::from)?;
        Ok(wf)
    })
    .await
    .map_err(|e| CommandError::from(format!("工作流定时设置线程 join 失败：{e}")))??;
    audit_event(
        &app_emit,
        "workflow_set_schedule",
        &row.id,
        &[(
            "schedule",
            row.schedule.clone().unwrap_or_else(|| "null".into()),
        )],
    );
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
            acceptance: None,
            local_id: local_id.to_string(),
            task_id: None,
            title: title.to_string(),
            note: None,
            tags: None,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            pos: Some(CanvasPos { x: 10.0, y: 20.0 }),
            model: None,
            subtasks: None,
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
                attachments: None,
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
                    attachments: None,
                },
                1_000,
            )
        };

        // 错误路径统一取 result（避免逐处 .map）
        let r =
            |run: &mut dyn FnMut(Vec<WorkflowNodeDraft>) -> CommandResult<WorkflowSaveOutcome>,
             nodes: Vec<WorkflowNodeDraft>|
             -> CommandResult<WorkflowSaveResult> { run(nodes).map(|o| o.result) };
        // 自环
        let err = r(&mut run, vec![node("n1", "A", &["n1"])]).unwrap_err();
        assert!(err.to_string().contains("循环依赖"));
        // 间接环 A→B→A
        let err = r(
            &mut run,
            vec![node("n1", "A", &["n2"]), node("n2", "B", &["n1"])],
        )
        .unwrap_err();
        assert!(err.to_string().contains("循环依赖"));
        // 悬空引用
        let err = r(&mut run, vec![node("n1", "A", &["不存在"])]).unwrap_err();
        assert!(err.to_string().contains("不在本画布内"));
        // localId 重复
        let err = r(&mut run, vec![node("n1", "A", &[]), node("n1", "B", &[])]).unwrap_err();
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
                    attachments: None,
                },
                1_000,
            )
        };

        // 错误路径统一取 result（避免逐处 .map）
        let r =
            |run: &mut dyn FnMut(Vec<WorkflowNodeDraft>) -> CommandResult<WorkflowSaveOutcome>,
             nodes: Vec<WorkflowNodeDraft>|
             -> CommandResult<WorkflowSaveResult> { run(nodes).map(|o| o.result) };
        // 节点数上限
        let nodes: Vec<WorkflowNodeDraft> = (0..MAX_WORKFLOW_NODES + 1)
            .map(|i| node(&format!("n{i}"), &format!("N{i}"), &[]))
            .collect();
        let err = r(&mut run, nodes).unwrap_err();
        assert!(err.to_string().contains("上限"));
        // 空标题
        let err = r(&mut run, vec![node("n1", "   ", &[])]).unwrap_err();
        assert!(err.to_string().contains("不能为空"));
        // 空名称
        let err = workflow_save_locked(
            &mut conn,
            WorkflowSaveInput {
                workflow_id: None,
                name: "  ".into(),
                goal: "g".into(),
                nodes: vec![],
                attachments: None,
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
                attachments: None,
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

    // ────────────── W4-TEMPLATE：文件格式 v1 ──────────────

    #[test]
    fn parse_valid_v1_maps_and_dedupes_deps() {
        let raw = r#"{
            "version": 1, "name": "周报", "goal": "每周出周报",
            "nodes": [
                {"id": "n1", "title": "收集", "note": "产出清单", "dependsOn": []},
                {"id": "n2", "title": "写稿", "dependsOn": ["n1", "n1"], "pos": [10, 20]}
            ]
        }"#;
        let input = parse_workflow_file(raw).unwrap();
        assert_eq!(input.name, "周报");
        assert_eq!(input.nodes.len(), 2);
        assert_eq!(input.nodes[0].local_id, "n1");
        assert_eq!(
            input.nodes[1].depends_on,
            vec!["n1".to_string()],
            "重复依赖去重"
        );
        assert_eq!(input.nodes[1].pos.as_ref().unwrap().x, 10.0);
        assert!(input.nodes[0].pos.is_none(), "缺 pos 走 dagre 布局");
    }

    #[test]
    fn parse_rejects_version_shapes_and_duplicates() {
        // 版本不支持
        let raw = r#"{"version": 2, "name": "x", "nodes": [{"id": "a", "title": "A"}]}"#;
        let err = parse_workflow_file(raw).unwrap_err();
        assert!(err.to_string().contains("版本不支持"));
        // 坏 JSON
        assert!(parse_workflow_file("不是 JSON").is_err());
        // 空节点
        let raw = r#"{"version": 1, "name": "x", "nodes": []}"#;
        let err = parse_workflow_file(raw).unwrap_err();
        assert!(err.to_string().contains("至少"));
        // 超上限
        let nodes: Vec<String> = (0..MAX_WORKFLOW_NODES + 1)
            .map(|i| format!(r#"{{"id": "n{i}", "title": "T{i}"}}"#))
            .collect();
        let raw = format!(
            r#"{{"version": 1, "name": "x", "nodes": [{}]}}"#,
            nodes.join(",")
        );
        let err = parse_workflow_file(&raw).unwrap_err();
        assert!(err.to_string().contains("上限"));
        // id 重复 / 空 id
        let raw = r#"{"version": 1, "name": "x", "nodes": [
            {"id": "a", "title": "A"}, {"id": "a", "title": "B"}]}"#;
        let err = parse_workflow_file(raw).unwrap_err();
        assert!(err.to_string().contains("重复"));
        let raw = r#"{"version": 1, "name": "x", "nodes": [{"id": "  ", "title": "A"}]}"#;
        let err = parse_workflow_file(raw).unwrap_err();
        assert!(err.to_string().contains("不能为空"));
    }

    #[test]
    fn parse_goal_falls_back_to_name() {
        let raw = r#"{"version": 1, "name": "我的流程", "nodes": [{"id": "a", "title": "A"}]}"#;
        let input = parse_workflow_file(raw).unwrap();
        assert_eq!(input.goal, "我的流程");
    }

    #[test]
    fn topo_export_orders_upstream_first() {
        let items = vec![
            ("c".to_string(), vec!["b".to_string()]),
            ("a".to_string(), vec![]),
            ("b".to_string(), vec!["a".to_string()]),
            ("p".to_string(), vec![]), // 独立分支
        ];
        let order = topo_export_order(&items);
        let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("b") < pos("c"));
    }

    #[test]
    fn export_roundtrip_preserves_graph() {
        let _g = super::super::lock_db_write();
        let mut conn = setup_conn();
        let r = save(
            &mut conn,
            vec![node("n1", "收集", &[]), node("n2", "汇总", &["n1"])],
        );
        let wf = load_workflow(&conn, &r.workflow_id).unwrap().unwrap();
        let tasks = load_tasks_by_workflow(&conn, &r.workflow_id).unwrap();
        let file = workflow_file_from(&wf, &tasks);
        assert_eq!(
            file.nodes[0].pos,
            Some(CanvasPos { x: 10.0, y: 20.0 }),
            "pos 随文件透传"
        );
        assert_eq!(file.version, WORKFLOW_FILE_VERSION);
        assert_eq!(file.nodes.len(), 2);
        assert!(file
            .generator
            .as_deref()
            .unwrap_or("")
            .starts_with("wmessage"));
        // 文件内本地 id：上游 n1 在前，下游 dependsOn = ["n1"]
        assert_eq!(file.nodes[0].id, "n1");
        assert_eq!(file.nodes[1].depends_on, vec!["n1".to_string()]);
        // 序列化 → 解析回草稿 → 再落库为全新实例（实例化语义）
        let json = serde_json::to_string_pretty(&file).unwrap();
        let input = parse_workflow_file(&json).unwrap();
        let r2 = workflow_save_locked(&mut conn, input, 2_000)
            .unwrap()
            .result;
        assert_ne!(r2.workflow_id, r.workflow_id, "导入必须生成全新工作流");
        assert_eq!(r2.created, 2);
        let tasks2 = load_tasks_by_workflow(&conn, &r2.workflow_id).unwrap();
        assert_eq!(tasks2.len(), 2);
        let child = tasks2.iter().find(|t| t.title == "汇总").unwrap();
        let parent = tasks2.iter().find(|t| t.title == "收集").unwrap();
        assert_eq!(
            child.depends_on.as_ref().unwrap(),
            &vec![parent.id.clone()],
            "实例化的依赖指向新实例内的任务"
        );
    }
}
