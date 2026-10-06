//! 工具实现层（28 个 tool_* 函数 + 4 个测试段）。
//!
//! 原 bot.rs 行 1393–2661 全部工具代码 +
//! 行 3070–3138 (tool_extract_document_tests) /
//! 行 3195–3344 (task_files_arg_tests) /
//! 行 3346–3370 (phase4_facts_tests) /
//! 行 3372–3390 (background_dialog_tests) 测试段搬入。
//!
//! 全部 tool_* / files_audit_kv 均为 pub(crate) fn（跨子模块可见，dispatch.rs 经 use 调）；
//! 外部入口为 bot::dispatch::execute_tool（pub）。

use tauri::{AppHandle, Emitter};

use crate::bot::dispatch::{commit_and_report, parse_args};
use crate::bot::format::status_label;
use crate::bot::registry::ToolResult;
use crate::bot::{
    audit_log, check_len, escape_for_log, load_config, MAX_DUE, MAX_KEYWORD, MAX_NOTE,
    MAX_SUBTASK_TEXT, MAX_TAGS, MAX_TAG_LEN, MAX_TITLE,
};
use crate::db::{prepare_for_upsert, TaskStatus};
use crate::error::CommandError;

// ───────────────────────── 时间 + 长期记忆工具 ─────────────────────────

/// get_current_time：返回本地日期时间+星期（模型做「今天/明天/周几」判断的锚点，禁止猜日期）
pub(crate) fn tool_get_current_time() -> crate::bot::registry::ToolResult {
    let now = chrono::Local::now();
    let week = [
        "星期一",
        "星期二",
        "星期三",
        "星期四",
        "星期五",
        "星期六",
        "星期日",
    ][chrono::Datelike::weekday(&now).num_days_from_monday() as usize];
    crate::bot::registry::ToolResult::ok(
        format!("现在：{} {week}", now.format("%Y-%m-%d %H:%M:%S")),
        Vec::new(),
    )
}

/// 解析 create_task/edit_task 的 files 参数（[{path, isDir}]）：
/// 去重保序、空路径丢弃、超 MAX_TASK_FILES 截断（Rust 侧硬上限）。
/// 返回 Some((files, truncated))；无 files 字段返回 None（不改绑定）。
fn parse_task_files_arg(v: &serde_json::Value) -> Option<(Vec<crate::db::TaskFile>, bool)> {
    let arr = v["files"].as_array()?;
    let truncated = arr.len() > crate::db::MAX_TASK_FILES;
    let mut out: Vec<crate::db::TaskFile> = Vec::new();
    for item in arr {
        let Some(path) = item["path"].as_str().map(|s| s.trim().to_string()) else {
            continue;
        };
        if path.is_empty() || out.iter().any(|f| f.path == path) {
            continue;
        }
        out.push(crate::db::TaskFile {
            path,
            is_dir: item["isDir"].as_bool().unwrap_or(false),
        });
        if out.len() >= crate::db::MAX_TASK_FILES {
            break;
        }
    }
    Some((out, truncated))
}

/// 模型来源 files 的安全校验：create_task/edit_task 的
/// files 参数直接来自模型，不加校验模型可把任意目录标 isDir=true 绑进任务卡，
/// `allowed_dirs` 会把它并入文件白名单（且先于 permMode 分流），strict 模式也被架空。
/// 收窄（对齐 link_file_to_task）：仅放行 AI_Gen_Files 目录内的已存在文件，强制 isDir=false；
/// 被拒条目记审计。用户亲手绑定走 bind_file 系统弹框，不在此限。
async fn sanitize_task_files_arg(
    app: &AppHandle,
    v: &serde_json::Value,
) -> Option<(Vec<crate::db::TaskFile>, bool)> {
    let (files, truncated) = parse_task_files_arg(v)?;
    // gen_dir + canonicalize 逐文件校验整体包 spawn_blocking：同步 syscall 在
    // async runtime（tool_create_task/tool_edit_task）上会阻塞全部 Tauri
    // command / event（OCR C5-BT-04 performance）。JoinError → 全丢并记审计
    // （dropped = 全部），与 sanitize「拿不到则拒」fail-closed 语义一致。
    let app_for_gen = app.clone();
    let n_files = files.len();
    let (out, dropped) = crate::py::document::spawn_blocking_map(move || {
        // gen_dir 拿不到（创建失败）则按 None 处理——白名单校验「拿不到则拒」
        let gen_canon = crate::db::gen_dir(&app_for_gen)
            .ok()
            .and_then(|d| std::fs::canonicalize(d).ok());
        Ok::<_, String>(sanitize_task_files_in(gen_canon.as_deref(), files))
    })
    .await
    .unwrap_or_else(|e| {
        audit_log(
            app,
            &format!("task_files_sanitized | join_error: {e} | 绑定文件全部丢弃"),
        );
        (Vec::new(), n_files)
    });
    if dropped > 0 {
        audit_log(
            app,
            &format!("task_files_sanitized | dropped: {dropped} | 模型来源 files 仅放行 AI_Gen_Files 内已存在文件"),
        );
    }
    Some((out, truncated))
}

/// sanitize 的纯内核（单测可注入 gen 目录）：仅放行 gen_canon 目录内的已存在文件，
/// 强制 isDir=false（目录绑定一律丢——目录绑定的授权只能来自用户手选）。
/// 返回 (保留列表, 丢弃数)。
fn sanitize_task_files_in(
    gen_canon: Option<&std::path::Path>,
    files: Vec<crate::db::TaskFile>,
) -> (Vec<crate::db::TaskFile>, usize) {
    let mut out: Vec<crate::db::TaskFile> = Vec::new();
    let mut dropped = 0usize;
    for f in files {
        let ok = !f.is_dir
            && gen_canon.is_some_and(|g| {
                std::fs::canonicalize(&f.path)
                    .map(|c| c.starts_with(g))
                    .unwrap_or(false)
            });
        if ok {
            out.push(f);
        } else {
            dropped += 1;
        }
    }
    (out, dropped)
}

/// files 写回任务时的双写：新 files 列 + 旧 file_path/file_is_dir 首条（过渡期旧版本可读）
pub fn apply_files_to_task(t: &mut crate::db::Task, files: Vec<crate::db::TaskFile>) {
    t.file_path = files.first().map(|f| f.path.clone());
    t.file_is_dir = files.first().map(|f| f.is_dir);
    t.files = if files.is_empty() { None } else { Some(files) };
}

/// tool.return 审计补充 kv（create_task/edit_task 带 files 时）：(原始条数, 是否截断)
pub(crate) fn files_audit_kv(args: &str) -> Option<(usize, bool)> {
    let v = parse_args(args);
    let arr = v["files"].as_array()?;
    Some((arr.len(), arr.len() > crate::db::MAX_TASK_FILES))
}

/// 改库后广播：挂件重读（tasks-changed）+ 主窗口合并 UI 不回写（tasks-updated, source: Bot）
pub fn broadcast_after_mutation<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    upserts: Vec<crate::db::Task>,
    deletes: Vec<String>,
) {
    if !upserts.is_empty() || !deletes.is_empty() {
        let _ = app.emit("tasks-changed", ());
        let _ = app.emit(
            "tasks-updated",
            serde_json::json!({ "source": crate::mutation::MutationOrigin::Bot.as_str(), "upserts": upserts, "deletes": deletes }),
        );
    }
}

async fn active_tasks(app: &AppHandle) -> Result<Vec<crate::db::Task>, String> {
    let tasks = crate::db::db_load(app.clone()).await?;
    Ok(tasks
        .into_iter()
        .filter(|t| {
            t.deleted_at.is_none() && t.archived != Some(true) && t.column != TaskStatus::Done
        })
        .collect())
}

// ───────────────────────── 任务查询工具（T1-QUERYTASKS：list+search 合并） ─────────────────────────

/// query_tasks 视图范围（list_tasks + search_tasks 合并后的唯一查询口径）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TaskView {
    /// 未完成未归档未删（原 list_tasks 口径）
    Active,
    /// 已完成未归档未删
    Done,
    /// 已归档未删
    Archived,
    /// 回收站（软删）
    Trash,
    /// 除回收站外全部（原 search_tasks 口径）
    All,
}

/// 解析 view 参数。None = 自动档：有 query → All（检索全库），无 query → Active（清单）。
/// 非法值回退 Active（fail-soft，不报错打断对话）。
fn parse_view(v: Option<&str>, has_query: bool) -> TaskView {
    match v {
        Some("done") => TaskView::Done,
        Some("archived") => TaskView::Archived,
        Some("trash") => TaskView::Trash,
        Some("all") => TaskView::All,
        Some(_) => TaskView::Active,
        None if has_query => TaskView::All,
        None => TaskView::Active,
    }
}

/// 视图过滤纯函数（单测锚点）
fn view_keep(view: TaskView, t: &crate::db::Task) -> bool {
    let not_deleted = t.deleted_at.is_none();
    let not_archived = t.archived != Some(true);
    match view {
        TaskView::Active => not_deleted && not_archived && t.column != TaskStatus::Done,
        TaskView::Done => not_deleted && not_archived && t.column == TaskStatus::Done,
        TaskView::Archived => not_deleted && !not_archived,
        TaskView::Trash => !not_deleted,
        TaskView::All => not_deleted,
    }
}

/// 关键词命中纯函数（原 tool_search_tasks 匹配口径原样搬移）：标题/备注/标签/子任务 contains；
/// 空关键词恒命中（清单模式）
fn keyword_hit(kw: &str, t: &crate::db::Task) -> bool {
    let kw = kw.trim().to_lowercase();
    if kw.is_empty() {
        return true;
    }
    let title_hit = t.title.to_lowercase().contains(&kw);
    let note_hit = t
        .note
        .as_deref()
        .map(|n| n.to_lowercase().contains(&kw))
        .unwrap_or(false);
    let tag_hit = t
        .tags
        .as_deref()
        .map(|tags| tags.iter().any(|tg| tg.to_lowercase().contains(&kw)))
        .unwrap_or(false);
    let sub_hit = t
        .subtasks
        .as_deref()
        .map(|subs| subs.iter().any(|s| s.text.to_lowercase().contains(&kw)))
        .unwrap_or(false);
    title_hit || note_hit || tag_hit || sub_hit
}

/// 标签过滤纯函数：大小写不敏感精确匹配；空标签恒命中
fn tag_keep(tag: &str, t: &crate::db::Task) -> bool {
    let tag = tag.trim().to_lowercase();
    if tag.is_empty() {
        return true;
    }
    t.tags
        .as_deref()
        .map(|ts| ts.iter().any(|x| x.to_lowercase() == tag))
        .unwrap_or(false)
}

/// limit 参数：默认 50，夹到 1..=200
fn parse_limit(v: &serde_json::Value) -> usize {
    v["limit"]
        .as_u64()
        .map(|n| n.clamp(1, 200) as usize)
        .unwrap_or(50)
}

/// 工作流 id→名称映射（fail-soft：读失败 = 空映射，行标记省略）
async fn workflow_name_map(app: &AppHandle) -> std::collections::HashMap<String, String> {
    match crate::db::workflow_list(app.clone()).await {
        Ok(list) => list.into_iter().map(|w| (w.id, w.name)).collect(),
        Err(_) => std::collections::HashMap::new(),
    }
}

/// schedule 原始串 → 人性化展示（只读展示；不做编辑入口——定时将来在工作流侧做）。
/// 格式单源见 bot_scheduler::occurrence_after（daily/weekly/monthly/at），未知格式原样返回。
fn humanize_schedule(s: Option<&str>) -> Option<String> {
    let s = s?;
    if let Some(t) = s.strip_prefix("daily:") {
        return Some(format!("每天 {t}"));
    }
    if let Some(t) = s.strip_prefix("weekly:") {
        let (d, hm) = t.split_once(':')?;
        let idx = d.parse::<usize>().ok()?.checked_sub(1)?;
        let name = ["一", "二", "三", "四", "五", "六", "日"].get(idx)?;
        return Some(format!("每周{name} {hm}"));
    }
    if let Some(t) = s.strip_prefix("monthly:") {
        let (dd, hm) = t.split_once(':')?;
        return Some(format!("每月{dd}日 {hm}"));
    }
    if let Some(t) = s.strip_prefix("at:") {
        return Some(format!("一次性 {}", t.replace('T', " ")));
    }
    Some(s.to_string())
}

/// 任务行渲染（query_tasks 统一行格式：列/标题/归档/工作流/截止/标签/id）
fn render_task_line(
    t: &crate::db::Task,
    wf_names: &std::collections::HashMap<String, String>,
) -> String {
    let col = status_label(t.column);
    let arch = if t.archived == Some(true) {
        "（已归档）"
    } else {
        ""
    };
    let wf = t
        .workflow_id
        .as_deref()
        .and_then(|id| wf_names.get(id))
        .map(|n| format!("（工作流：{n}）"))
        .unwrap_or_default();
    let due = t
        .due
        .as_deref()
        .map(|d| format!("，截止 {d}"))
        .unwrap_or_default();
    let tags = t
        .tags
        .as_deref()
        .map(|ts| format!("，标签：{}", ts.join("/")))
        .unwrap_or_default();
    format!("- [{col}] {}{arch}{wf}{due}{tags}（id={}）", t.title, t.id)
}

/// 查询任务（T1-QUERYTASKS：原 list_tasks + search_tasks 合并）：
/// - 无 query：列清单（view 默认 active = 原 list_tasks）
/// - 有 query：关键词检索（view 默认 all 全库 = 原 search_tasks）
/// - view/tag/limit 正交过滤；输出行带（工作流：名称）标记，模型可按标记汇总工作流问答
pub(crate) async fn tool_query_tasks(app: &AppHandle, args: &str) -> ToolResult {
    let v = parse_args(args);
    let raw_query = v["query"].as_str().map(|s| s.trim().to_string());
    let has_query = raw_query.as_deref().is_some_and(|q| !q.is_empty());
    // 显式传了空串 query：提示语义（不传 = 清单），不当作全库检索
    if raw_query.as_deref() == Some("") {
        return ToolResult::ok(
            "query 为空串：要列清单请不传 query 参数".to_string(),
            Vec::new(),
        );
    }
    if let Some(q) = raw_query.as_deref() {
        if let Err(e) = check_len(q, MAX_KEYWORD, "搜索关键词") {
            return ToolResult::ok(e.to_string(), Vec::new());
        }
    }
    let tag = v["tag"].as_str().unwrap_or("");
    if let Err(e) = check_len(tag, MAX_TAG_LEN, "标签") {
        return ToolResult::ok(e.to_string(), Vec::new());
    }
    let limit = parse_limit(&v);
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
        // 「查询失败」首字是「查」，非 error/warn 前缀 → ok（与 query_single_task 同口径）
        return ToolResult::ok("查询失败：数据库读取错误".to_string(), Vec::new());
    };
    let view = parse_view(v["view"].as_str(), has_query);
    let kw = raw_query.as_deref().unwrap_or("");
    let mut hits: Vec<&crate::db::Task> = tasks
        .iter()
        .filter(|t| view_keep(view, t) && tag_keep(tag, t) && keyword_hit(kw, t))
        .collect();
    hits.sort_by(|a, b| {
        a.order
            .unwrap_or(f64::MAX)
            .partial_cmp(&b.order.unwrap_or(f64::MAX))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = hits.len();
    hits.truncate(limit);
    if hits.is_empty() {
        let msg = match view {
            TaskView::Active if kw.is_empty() => "当前没有未完成的任务".to_string(),
            TaskView::Done if kw.is_empty() => "暂无已完成（未归档）任务".to_string(),
            TaskView::Archived if kw.is_empty() => "暂无已归档任务".to_string(),
            TaskView::Trash if kw.is_empty() => "回收站为空".to_string(),
            TaskView::All if kw.is_empty() => "还没有任何任务".to_string(),
            _ => format!("没有找到匹配「{kw}」的任务"),
        };
        return ToolResult::ok(msg, Vec::new());
    }
    let wf_names = workflow_name_map(app).await;
    let mut lines: Vec<String> = Vec::new();
    for t in &hits {
        lines.push(render_task_line(t, &wf_names));
    }
    if total > hits.len() {
        lines.push(format!(
            "（共 {total} 条，仅显示前 {} 条；需要更多可加大 limit）",
            hits.len()
        ));
    }
    let refs: Vec<crate::bot_chat::TaskRef> = hits
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    ToolResult::ok(lines.join("\n"), refs)
}

// ───────────────────────── 任务管理工具实现（20+ functions） ─────────────────────────

/// 单卡查询（白名单单点工具）：
/// 按 id 取单张任务卡的完整详情（区别于 list_tasks 的批量清单 + search_tasks 的关键词检索）。
/// - 必填参数：id（任务卡 UUID）
/// - 输出：标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除/机器人执行状态指示
/// - 单点白名单工具（非原子黑名单），LLM 可裸调
/// - 返回的 TaskRef 供后续 taskId 操作（complete_task / edit_task / bind_file 等）跟随引用
pub(crate) async fn tool_query_single_task(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(id) = v["id"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::ok("query_single_task 缺少 id 参数".to_string(), Vec::new());
    };
    if id.is_empty() {
        return ToolResult::ok("query_single_task id 不能为空".to_string(), Vec::new());
    }
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
        // 「查询失败」首字是「查」，不是以「失败」开头；brief 要求机械首字符判定 → ok
        return ToolResult::ok("查询失败：数据库读取错误".to_string(), Vec::new());
    };
    let Some(t) = tasks.iter().find(|t| t.id == id).cloned() else {
        return ToolResult::ok(format!("未找到 id={id} 的任务卡"), Vec::new());
    };
    let col = status_label(t.column);
    let mut lines: Vec<String> = vec![format!("- [{}] {}（id={}）", col, t.title, t.id)];
    if let Some(note) = &t.note {
        if !note.is_empty() {
            lines.push(format!("  备注：{note}"));
        }
    }
    if let Some(due) = &t.due {
        if !due.is_empty() {
            lines.push(format!("  截止：{due}"));
        }
    }
    if let Some(subtasks) = &t.subtasks {
        if !subtasks.is_empty() {
            lines.push(format!("  子任务（{}）：", subtasks.len()));
            for st in subtasks {
                let mark = if st.done { "✓" } else { "·" };
                lines.push(format!("    [{mark}] {}（id={}）", st.text, st.id));
            }
        }
    }
    if let Some(tags) = &t.tags {
        if !tags.is_empty() {
            lines.push(format!("  标签：{}", tags.join(", ")));
        }
    }
    // T1 只读行：创建时间 / 定时 / 所属工作流 / 依赖（模型可感知新字段，但无写入口——
    // schedule 编辑将来在工作流侧做，dependsOn 由工作流自动生成、普通卡手动维护）
    if let Some(ca) = t.created_at {
        if let Some(dt) = chrono::DateTime::from_timestamp_millis(ca) {
            lines.push(format!(
                "  创建：{}",
                dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")
            ));
        }
    }
    if let Some(s) = humanize_schedule(t.schedule.as_deref()) {
        lines.push(format!("  定时：{s}"));
    }
    if let Some(wid) = t.workflow_id.as_deref() {
        let name = workflow_name_map(app).await.get(wid).cloned();
        match name {
            Some(n) => lines.push(format!("  所属工作流：{n}")),
            None => lines.push(format!("  所属工作流：（id={wid}）")),
        }
    }
    if let Some(deps) = &t.depends_on {
        if !deps.is_empty() {
            let titles: Vec<String> = deps
                .iter()
                .map(|d| {
                    tasks
                        .iter()
                        .find(|x| &x.id == d)
                        .map(|x| x.title.clone())
                        .unwrap_or_else(|| d.clone())
                })
                .collect();
            lines.push(format!("  依赖：{}", titles.join("、")));
        }
    }
    let bound_files = t.effective_files();
    if !bound_files.is_empty() {
        lines.push("  绑定文件：".to_string());
        for f in &bound_files {
            let kind = if f.is_dir { "目录" } else { "文件" };
            lines.push(format!("    - [{kind}] {}", f.path));
        }
    }
    let mut status: Vec<&str> = Vec::new();
    if t.archived == Some(true) {
        status.push("已归档");
    }
    if t.deleted_at.is_some() {
        status.push("已删除（回收站）");
    }
    if t.bot_assigned == Some(true) {
        status.push("机器人执行中");
    }
    if !status.is_empty() {
        lines.push(format!("  状态：{}", status.join(" / ")));
    }
    ToolResult::ok(
        lines.join("\n"),
        vec![crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        }],
    )
}

pub(crate) async fn tool_create_task(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(title) = v["title"].as_str() else {
        return ToolResult::ok("create_task 缺少 title".to_string(), Vec::new());
    };
    let title = title.trim();
    if title.is_empty() {
        return ToolResult::ok("任务标题不能为空".to_string(), Vec::new());
    }
    if let Err(e) = check_len(title, MAX_TITLE, "任务标题") {
        // check_len 返回 String，首字「任」非 error/warn 前缀 → ok
        return ToolResult::ok(e.to_string(), Vec::new());
    }
    if let Some(n) = v["note"].as_str() {
        if let Err(e) = check_len(n, MAX_NOTE, "备注") {
            return ToolResult::ok(e.to_string(), Vec::new());
        }
    }
    if let Some(d) = v["due"].as_str() {
        if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
            return ToolResult::ok(e.to_string(), Vec::new());
        }
    }
    let now = chrono::Utc::now().timestamp_millis();
    let mut task = crate::db::Task {
        acceptance: None,
        id: uuid::Uuid::new_v4().simple().to_string(),
        title: title.to_string(),
        due: v["due"].as_str().map(|s| s.to_string()),
        note: v["note"].as_str().map(|s| s.to_string()),
        tags: None,
        files: None,
        file_path: None,
        file_is_dir: None,
        column: match v["column"].as_str() {
            Some("doing") => TaskStatus::Doing,
            _ => TaskStatus::Todo,
        },
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
        origin: None,
        workflow_id: None,
        depends_on: None,
        canvas_pos: None,
        model: None,
        owner_id: None,            // 机器人建卡 = 本人（任务图谱设计 §1.1）
        created_at: Some(now),     // 创建时间打戳（与 updated_at 同值；此后 UPDATE 不覆盖）
        enabled: None,             // 机器人建卡无定时配置
        expected_updated_at: None, // 新建任务：无读快照基线
    };
    // 多文件绑定：files 参数 [{path,isDir}]，超 10 截断 + 警告
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件）
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v).await {
        if truncated {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
        apply_files_to_task(&mut task, files);
    }
    // 插到列表顶部：取当前最小 order 减 1
    if let Ok(all) = crate::db::db_load(app.clone()).await {
        let min = all
            .iter()
            .filter_map(|t| t.order)
            .fold(f64::INFINITY, f64::min);
        task.order = Some(if min.is_finite() { min - 1.0 } else { 0.0 });
    }
    commit_and_report(
        app,
        &task,
        || format!("已新建任务「{}」{files_warn}", task.title),
        || {
            vec![crate::bot_chat::TaskRef {
                id: task.id.clone(),
                title: task.title.clone(),
            }]
        },
        "新建任务失败",
    )
    .await
}

pub(crate) async fn tool_complete_task(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    // 走 resolve_task（taskId 精确匹配优先、title 关键词兜底 +
    // taskId/title 交叉校验），与其它任务操作工具对齐——只读 title 会让
    // schema 声明的「taskId 优先」被完全忽略，任务卡执行路径只传 taskId 时确定性失败。
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        // CommandError.to_string() 首字以中文描述开头，非 error/warn 前缀 → ok
        Err(e) => return ToolResult::ok(e.to_string(), Vec::new()),
    };
    let mut next = task.clone();
    next.column = TaskStatus::Done;
    next.completed_at = Some(chrono::Utc::now().timestamp_millis());
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at
    next.updated_at = next.completed_at;
    commit_and_report(
        app,
        &next,
        || format!("已完成任务「{}」", task.title),
        || {
            vec![crate::bot_chat::TaskRef {
                id: task.id.clone(),
                title: task.title.clone(),
            }]
        },
        "完成任务失败",
    )
    .await
}

/// 删除任务到回收站：**弹窗确认后才执行**（危险操作护栏；60s 无响应默认拒绝）
pub(crate) async fn tool_delete_task(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return ToolResult::ok(e.to_string(), Vec::new()),
    };
    let approved = crate::bot_slash::ask_user_confirm(
        app,
        "delete_task",
        &task.title,
        interactive,
        session_id,
    )
    .await;
    if !approved {
        // 「用户拒绝了删除」首字「用」非 error/warn 前缀 → ok
        return ToolResult::ok("用户拒绝了删除，任务未删除".to_string(), Vec::new());
    }
    let mut next = task.clone();
    next.deleted_at = Some(chrono::Utc::now().timestamp_millis());
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at
    next.updated_at = next.deleted_at;
    commit_and_report(
        app,
        &next,
        || format!("已删除任务「{}」（进回收站）", task.title),
        || {
            vec![crate::bot_chat::TaskRef {
                id: task.id.clone(),
                title: task.title.clone(),
            }]
        },
        "删除任务失败",
    )
    .await
}

/// 按标题关键词找第一条未完成任务（大小写不敏感）
async fn find_task_by_keyword(
    app: &AppHandle,
    kw: &str,
) -> Result<Option<crate::db::Task>, String> {
    let tasks = active_tasks(app).await?;
    Ok(tasks
        .into_iter()
        .find(|t| t.title.to_lowercase().contains(kw)))
}

/// 定位任务：优先 taskId 精确匹配，其次标题关键词模糊匹配。
/// 返回 (task, 定位说明)；找不到返回错误文案。
async fn resolve_task(
    app: &AppHandle,
    v: &serde_json::Value,
) -> Result<crate::db::Task, CommandError> {
    if let Some(id) = v["taskId"].as_str() {
        let id = id.trim();
        if !id.is_empty() {
            let tasks = active_tasks(app)
                .await
                .map_err(|e| CommandError::DbError(format!("读取未完成任务失败：{e}")))?;
            if let Some(t) = tasks.into_iter().find(|t| t.id == id) {
                // 交叉校验：同窗格连续操作不同任务卡时，模型会沿用上一张卡的
                // taskId 张冠李戴。taskId 与 title 关键词同时给出且对不上 → 不信 id，
                // 改用 title 重新定位（定位不到就报错，让模型/用户确认）
                if let Some(kw) = v["title"].as_str().map(|s| s.trim().to_lowercase()) {
                    if !kw.is_empty() && !t.title.to_lowercase().contains(&kw) {
                        if let Some(t2) = find_task_by_keyword(app, &kw)
                            .await
                            .map_err(|e| CommandError::DbError(format!("按关键词查找失败：{e}")))?
                        {
                            return Ok(t2);
                        }
                        return Err(CommandError::DomainRule {
                            domain: "argument".to_string(),
                            reason: format!(
                                "taskId 命中的任务「{}」与标题关键词「{}」不符，且按标题未找到未完成任务，请确认操作对象",
                                t.title,
                                v["title"].as_str().unwrap_or("")
                            ),
                        });
                    }
                }
                return Ok(t);
            }
            return Err(CommandError::DomainRule {
                domain: "task".to_string(),
                reason: format!("未找到 id={id} 的未完成任务（可能已完成或已删除）"),
            });
        }
    }
    if let Some(kw) = v["title"].as_str() {
        let kw = kw.trim().to_lowercase();
        if !kw.is_empty() {
            if let Some(t) = find_task_by_keyword(app, &kw)
                .await
                .map_err(|e| CommandError::DbError(format!("按关键词查找失败：{e}")))?
            {
                return Ok(t);
            }
            return Err(CommandError::DomainRule {
                domain: "task".to_string(),
                reason: format!(
                    "未找到匹配「{}」的未完成任务",
                    v["title"].as_str().unwrap_or("")
                ),
            });
        }
    }
    Err(CommandError::DomainRule {
        domain: "argument".to_string(),
        reason: "缺少 taskId 或 title 参数".to_string(),
    })
}

/// 模型名长度上限（edit_task 的 model 参数；模型库名最长约 80，放宽到 200 防御性截断误伤）
const MAX_MODEL_NAME: usize = 200;

/// owner 参数解析（T1）：personId 精确 → 名字精确 → 「我/本人」→ 名字唯一包含。
/// 歧义/未命中返回候选文案（由调用方走 ok 通道回给模型，让模型向用户消歧）。
async fn resolve_owner(app: &AppHandle, input: &str) -> Result<String, String> {
    // people_list 是同步 command（people.rs 无 async），直接调
    let list = crate::db::people_list(app.clone()).map_err(|e| format!("读取成员失败：{e}"))?;
    if list.iter().any(|p| p.id == input) {
        return Ok(input.to_string());
    }
    let exact: Vec<&crate::db::PeopleEntry> = list.iter().filter(|p| p.name == input).collect();
    if exact.len() > 1 {
        return Err(format!(
            "「{input}」匹配到 {} 位同名成员，请用 personId 或说明是哪位",
            exact.len()
        ));
    }
    if let [p] = exact[..] {
        return Ok(p.id.clone());
    }
    if matches!(input, "我" | "本人" | "我自己") {
        if let Some(p) = list.iter().find(|p| p.is_self) {
            return Ok(p.id.clone());
        }
    }
    let kw = input.to_lowercase();
    let contains: Vec<&crate::db::PeopleEntry> = list
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&kw))
        .collect();
    match contains.as_slice() {
        [p] => Ok(p.id.clone()),
        [] => Err(format!("没有找到成员「{input}」，请确认名字后重试")),
        many => {
            let names: Vec<String> = many.iter().take(5).map(|p| p.name.clone()).collect();
            Err(format!(
                "「{input}」匹配到多个成员：{}，请说清是哪位",
                names.join("、")
            ))
        }
    }
}

pub(crate) async fn tool_edit_task(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return ToolResult::ok(e.to_string(), Vec::new()),
    };
    let mut next = task.clone();
    let mut changed: Vec<&str> = Vec::new();
    if let Some(nt) = v["newTitle"].as_str() {
        let nt = nt.trim();
        if !nt.is_empty() {
            if let Err(e) = check_len(nt, MAX_TITLE, "新标题") {
                return ToolResult::ok(e.to_string(), Vec::new());
            }
            next.title = nt.to_string();
            changed.push("标题");
        }
    }
    if let Some(n) = v["note"].as_str() {
        if !n.trim().is_empty() {
            if let Err(e) = check_len(n, MAX_NOTE, "备注") {
                return ToolResult::ok(e.to_string(), Vec::new());
            }
        }
        next.note = if n.trim().is_empty() {
            None
        } else {
            Some(n.to_string())
        };
        changed.push("备注");
    }
    if let Some(d) = v["due"].as_str() {
        if !d.trim().is_empty() {
            if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
                return ToolResult::ok(e.to_string(), Vec::new());
            }
        }
        next.due = if d.trim().is_empty() {
            None
        } else {
            Some(d.to_string())
        };
        changed.push("截止时间");
    }
    if let Some(tags) = v["tags"].as_array() {
        if tags.len() > MAX_TAGS {
            // 「标签数量超上限」首字「标」非 error/warn 前缀 → ok
            return ToolResult::ok(format!("标签数量超上限（最多 {MAX_TAGS} 个）"), Vec::new());
        }
        for t in tags {
            if let Some(ts) = t.as_str() {
                if let Err(e) = check_len(ts, MAX_TAG_LEN, "标签") {
                    return ToolResult::ok(e.to_string(), Vec::new());
                }
            }
        }
        let list: Vec<String> = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        next.tags = if list.is_empty() { None } else { Some(list) };
        changed.push("标签");
    }
    // 多文件绑定：files 参数 [{path,isDir}] 整体替换列表；空数组清除；超 10 截断 + 警告
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件）
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v).await {
        if truncated {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
        apply_files_to_task(&mut next, files);
        changed.push("绑定文件");
    }
    if let Some(c) = v["column"]
        .as_str()
        .and_then(|s| s.trim().parse::<TaskStatus>().ok())
    {
        if c != next.column {
            next.column = c;
            // 列变更补完成语义（与主窗口一致）
            if c == TaskStatus::Done {
                next.completed_at = Some(chrono::Utc::now().timestamp_millis());
                next.archived = Some(false);
            } else {
                next.completed_at = None;
                next.archived = None;
            }
            changed.push("状态列");
        }
    }
    // T1：每卡执行模型（W6 model 列的模型写入口；空串=清除恢复跟随全局）
    if let Some(m) = v["model"].as_str() {
        let m = m.trim();
        if !m.is_empty() {
            if let Err(e) = check_len(m, MAX_MODEL_NAME, "模型名") {
                return ToolResult::ok(e.to_string(), Vec::new());
            }
            next.model = Some(m.to_string());
        } else {
            next.model = None;
        }
        changed.push("执行模型");
    }
    // T1：归属人（任务图谱 owner 列；空串=归属本人；名字歧义报候选）
    if let Some(o) = v["owner"].as_str() {
        let o = o.trim();
        if o.is_empty() {
            next.owner_id = None;
            changed.push("归属人");
        } else {
            match resolve_owner(app, o).await {
                Ok(pid) => {
                    next.owner_id = Some(pid);
                    changed.push("归属人");
                }
                // 歧义/未命中文案首字是「没」/「「」等中文，非 error/warn 前缀 → ok
                Err(msg) => return ToolResult::ok(msg, Vec::new()),
            }
        }
    }
    if changed.is_empty() {
        // 「没有可修改的字段」首字「没」非 error/warn 前缀 → ok
        return ToolResult::ok("没有可修改的字段".to_string(), Vec::new());
    }
    prepare_for_upsert(&mut next);
    commit_and_report(
        app,
        &next,
        || {
            format!(
                "已更新任务「{}」（{}）{files_warn}",
                next.title,
                changed.join("、")
            )
        },
        || {
            vec![crate::bot_chat::TaskRef {
                id: next.id.clone(),
                title: next.title.clone(),
            }]
        },
        "编辑任务失败",
    )
    .await
}

pub(crate) async fn tool_add_subtask(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(text) = v["text"].as_str().map(|s| s.trim()) else {
        // 「add_subtask 缺少 text」首字「a」非 error/warn 前缀 → ok
        return ToolResult::ok("add_subtask 缺少 text".to_string(), Vec::new());
    };
    if text.is_empty() {
        // 「子任务内容不能为空」首字「子」非 error/warn 前缀 → ok
        return ToolResult::ok("子任务内容不能为空".to_string(), Vec::new());
    }
    if let Err(e) = check_len(text, MAX_SUBTASK_TEXT, "子任务内容") {
        return ToolResult::ok(e.to_string(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return ToolResult::ok(e.to_string(), Vec::new()),
    };
    let mut next = task.clone();
    let mut subs = next.subtasks.unwrap_or_default();
    subs.push(crate::db::Subtask {
        id: uuid::Uuid::new_v4().simple().to_string(),
        text: text.to_string(),
        done: false,
    });
    next.subtasks = Some(subs);
    prepare_for_upsert(&mut next);
    commit_and_report(
        app,
        &next,
        || format!("已给任务「{}」添加子任务「{}」", next.title, text),
        || {
            vec![crate::bot_chat::TaskRef {
                id: next.id.clone(),
                title: next.title.clone(),
            }]
        },
        "添加子任务失败",
    )
    .await
}

/// 子任务定位（T1）：subtaskId 精确优先，回落 text 关键词 contains。
/// 返回 index；参数缺失/未命中返回错误文案（调用方走 ok 通道）。
fn locate_subtask(
    subs: &[crate::db::Subtask],
    subtask_id: Option<&str>,
    text_kw: Option<&str>,
) -> Result<usize, String> {
    if let Some(sid) = subtask_id.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return subs
            .iter()
            .position(|s| s.id == sid)
            .ok_or_else(|| format!("没有找到 id={sid} 的子任务"));
    }
    let Some(kw) = text_kw.map(|s| s.trim()) else {
        return Err("缺少 subtaskId 或 text 参数（至少提供一个）".to_string());
    };
    if kw.is_empty() {
        return Err("子任务关键词不能为空".to_string());
    }
    let kw_lower = kw.to_lowercase();
    subs.iter()
        .position(|s| s.text.to_lowercase().contains(&kw_lower))
        .ok_or_else(|| format!("没有匹配「{kw}」的子任务"))
}

pub(crate) async fn tool_toggle_subtask(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let sub_id = v["subtaskId"].as_str();
    let text_kw = v["text"].as_str();
    if sub_id.is_none() {
        if let Some(kw) = text_kw {
            if let Err(e) = check_len(kw.trim(), MAX_SUBTASK_TEXT, "子任务关键词") {
                return ToolResult::ok(e.to_string(), Vec::new());
            }
        }
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return ToolResult::ok(e.to_string(), Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let idx = match locate_subtask(&subs, sub_id, text_kw) {
        Ok(i) => i,
        // 错误文案首字「没/缺/子」非 error/warn 前缀 → ok
        Err(msg) => return ToolResult::ok(format!("任务「{}」{msg}", task.title), Vec::new()),
    };
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2[idx].done = !subs2[idx].done;
    next.subtasks = Some(subs2);
    prepare_for_upsert(&mut next);
    let st_text = next
        .subtasks
        .as_ref()
        .and_then(|s| s.get(idx))
        .map(|s| s.text.clone())
        .unwrap_or_default();
    let done_mark = next
        .subtasks
        .as_ref()
        .and_then(|s| s.get(idx))
        .map(|s| s.done)
        .unwrap_or(false);
    commit_and_report(
        app,
        &next,
        || {
            format!(
                "子任务「{st_text}」已{}",
                if done_mark {
                    "勾选 ✓"
                } else {
                    "取消勾选"
                }
            )
        },
        || {
            vec![crate::bot_chat::TaskRef {
                id: next.id.clone(),
                title: next.title.clone(),
            }]
        },
        "切换子任务状态失败",
    )
    .await
}

pub(crate) async fn tool_remove_subtask(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let sub_id = v["subtaskId"].as_str();
    let text_kw = v["text"].as_str();
    if sub_id.is_none() {
        if let Some(kw) = text_kw {
            if let Err(e) = check_len(kw.trim(), MAX_SUBTASK_TEXT, "子任务关键词") {
                return ToolResult::ok(e.to_string(), Vec::new());
            }
        }
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return ToolResult::ok(e.to_string(), Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let idx = match locate_subtask(&subs, sub_id, text_kw) {
        Ok(i) => i,
        Err(msg) => return ToolResult::ok(format!("任务「{}」{msg}", task.title), Vec::new()),
    };
    let removed_text = subs[idx].text.clone();
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2.remove(idx);
    next.subtasks = Some(subs2);
    prepare_for_upsert(&mut next);
    commit_and_report(
        app,
        &next,
        || format!("已删除任务「{}」的子任务「{}」", next.title, removed_text),
        || {
            vec![crate::bot_chat::TaskRef {
                id: next.id.clone(),
                title: next.title.clone(),
            }]
        },
        "删除子任务失败",
    )
    .await
}

/// 登记产物到任务卡执行流程的产物清单（不立即绑）
///
/// 设计：bot 流程内调用是「登记」语义——记到内存登记表 `bot_artifacts::REGISTRY`，
/// 流程结束按 `TaskExecOrigin` 分流（D4d）落「通知中心」待绑定消息让用户勾选。
/// 普通 chat 场景（无 TaskExecOrigin 上下文）直接拒，避免登记表被反复污染。
///
/// 路径白名单：仅接受 AI_Gen_Files 目录内的文件（产物必经此目录生成），
/// 防止 LLM 借 bind_files 间接读 ~/.ssh/id_rsa 等敏感文件。
pub(crate) async fn tool_link_file_to_task(
    app: &AppHandle,
    args: &str,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    if !crate::tool_guard::is_task_execution_flow(session_id) {
        // 「link_file_to_task 仅在...」首字「l」非 error/warn 前缀 → ok
        return ToolResult::ok(
            "link_file_to_task 仅在任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内有效；普通对话场景调用无效果（不报错也不绑），不要反复尝试".to_string(),
            Vec::new(),
        );
    }
    let v = parse_args(args);
    let Some(path) = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        // 「link_file_to_task 缺少 path」首字「l」非 error/warn 前缀 → ok
        return ToolResult::ok("link_file_to_task 缺少 path".to_string(), Vec::new());
    };
    if !std::path::Path::new(&path).exists() {
        // 「路径不存在」首字「路」非 error/warn 前缀 → ok
        return ToolResult::ok(format!("路径不存在，拒绝登记：{path}"), Vec::new());
    }
    // canonicalize 失败不回退原路径（fail-closed）：白名单比对必须基于
    // 规范化后的真实落点，拿原始路径比 lexical starts_with 会给软链/竞态窗口留绕过面
    let canon = match std::fs::canonicalize(&path) {
        Ok(c) => c,
        Err(_) => {
            // 「路径校验失败」首字「路」非 error/warn 前缀 → ok
            return ToolResult::ok(
                format!(
                    "路径校验失败，拒绝登记：{path}（无法解析真实路径，请确认文件仍然存在后重试）"
                ),
                Vec::new(),
            );
        }
    };
    let in_gen = crate::db::gen_dir(app)
        .ok()
        .and_then(|d| std::fs::canonicalize(d).ok())
        .is_some_and(|gen| canon.starts_with(&gen));
    if !in_gen {
        // 「已拒绝登记」首字「已」非 error/warn 前缀 → ok（语义上其实是拒绝，机械规则降级）
        return ToolResult::ok(
            "已拒绝登记该路径：link_file_to_task 只能登记 AI_Gen_Files 目录内的产物；其他文件请在任务卡上手动「绑定文件」".to_string(),
            Vec::new(),
        );
    }
    // taskId / title 任一必传（绑定目标）
    let task_key = v["taskId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            v["title"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| format!("title:{s}"))
        });
    let Some(key) = task_key else {
        // 「link_file_to_task 缺少 taskId / title」首字「l」非 error/warn 前缀 → ok
        return ToolResult::ok(
            "link_file_to_task 缺少 taskId / title".to_string(),
            Vec::new(),
        );
    };
    let kind = match v["kind"].as_str().unwrap_or("final") {
        "intermediate" => crate::bot_artifacts::ArtifactKind::Intermediate,
        _ => crate::bot_artifacts::ArtifactKind::Final,
    };
    crate::bot_artifacts::register(app, &key, path.clone(), kind).await;
    let kind_label = match kind {
        crate::bot_artifacts::ArtifactKind::Final => "最终产物",
        crate::bot_artifacts::ArtifactKind::Intermediate => "中间产物",
    };
    // 「已登记{kind_label}」首字「已」非 error/warn 前缀 → ok
    ToolResult::ok(
        format!(
            "已登记{kind_label}「{path}」。流程结束、任务完成时会在通知中心生成待绑定消息让用户勾选；不要在此刻绑定——任务未完成或中断不绑定。"
        ),
        Vec::new(),
    )
}

// ───────────────────────── 文档 / Python 工具（bot_py 桥接） ─────────────────────────

/// 提取文档文本：path 给定则直读（任务卡绑定文件），否则弹框选文件；返回路径 + 文本供模型阅读/润色
/// extract_document path 白名单：任务卡绑定文件 / AI_Gen_Files 目录内文件静默放行。
/// 规范化路径比较，防 ../ 绕过。无 path 时走弹框（用户亲手选，不受此限）。
/// 授权分流：其余路径不硬拒，走 bot_fs::resolve_with_perm 分流
///（strict 硬拒 / ask 弹授权窗 / yolo 放行），拒绝文案透传给模型。
/// interactive/session_id 透传给授权弹窗：后台执行（interactive=false）不弹窗直接拒。
async fn extract_path_check(
    app: &AppHandle,
    path: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> Result<(), String> {
    let Ok(canon) = std::fs::canonicalize(path) else {
        return Err(format!("路径不存在或不可访问：{path}"));
    };
    // 1) AI_Gen_Files 目录内
    if let Ok(gen) = crate::db::gen_dir(app).and_then(|d| std::fs::canonicalize(d)) {
        if canon.starts_with(&gen) {
            return Ok(());
        }
    }
    // 2) 任务卡绑定文件（多文件绑定：files 列表 + 旧字段兜底走 effective_files）
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            for f in t.effective_files() {
                if let Ok(fc) = std::fs::canonicalize(&f.path) {
                    if fc == canon {
                        return Ok(());
                    }
                }
            }
        }
    }
    // 3) 授权分流（与 bot_fs 同一口径：白名单静默放行 / strict 拒 /
    //    ask 弹窗 / yolo 放）
    crate::bot_fs::resolve_with_perm(app, "extract_document", path, interactive, session_id)
        .await
        .map(|_| ())
}

pub(crate) async fn tool_extract_document(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let path_opt = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 无 path 时 bot_py::doc_extract 会弹系统选择框，
    // 后台定时执行弹框 = 永久阻塞卡死，必须直接拒绝并引导模型传 path
    if path_opt.is_none() && !interactive {
        // 「失败：后台执行...」以「失败」开头 → error
        return ToolResult::error(
            "失败：后台执行不能弹窗选文件，请提供 path 参数指定文档路径".to_string(),
            Vec::new(),
        );
    }
    // 分页参数：offset 字符偏移续读；limit 默认 30000、硬钳 60000
    let offset = v["offset"].as_u64().unwrap_or(0) as usize;
    let limit = v["limit"]
        .as_u64()
        .map(|n| (n as usize).min(EXTRACT_MAX_LIMIT))
        .filter(|&n| n > 0)
        .unwrap_or(EXTRACT_DEFAULT_LIMIT);
    // 模型直传 path 时授权校验（无 path 走弹框，用户亲手选不受限）
    if let Some(p) = path_opt.as_deref() {
        if let Err(e) = extract_path_check(app, p, interactive, session_id).await {
            return ToolResult::ok(e.to_string(), Vec::new());
        }
    }
    match crate::bot_py::doc_extract(app.clone(), path_opt).await {
        Ok(res) => {
            // N3-2：扫描版 PDF 兜底——文本层近空（pypdf 提不出内容）→ PyMuPDF 转图
            // → 本地 OCR（macOS Vision / PP-OCRv6，字节全本地）。失败/不适用保持原输出。
            let is_pdf = res.path.to_ascii_lowercase().ends_with(".pdf");
            if is_pdf && pdf_text_looks_empty(&res.text) {
                match scan_pdf_ocr_text(app, &res.path).await {
                    Ok(Some(text)) => {
                        return ToolResult::ok(
                            format_extract_output(&res.path, &text, offset, limit),
                            Vec::new(),
                        );
                    }
                    Err(hint) => {
                        // 缺 pymupdf：原文输出 + 安装指引（原文可能确实只有页码等零星内容）
                        let merged = format!("{}\n\n{hint}", res.text);
                        return ToolResult::ok(
                            format_extract_output(&res.path, &merged, offset, limit),
                            Vec::new(),
                        );
                    }
                    Ok(None) => {} // 渲染/OCR 失败：保持 pypdf 原输出
                }
            }
            ToolResult::ok(
                format_extract_output(&res.path, &res.text, offset, limit),
                Vec::new(),
            )
        }
        // 「提取失败」首字「提」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("提取失败：{e}"), Vec::new()),
    }
}

/// N3-2 纯函数：PDF 提取文本是否「近空」——去掉 `=== 第N页 ===` 页标记后
/// 全为空白视为无文本层（扫描件/图片型 PDF）
fn pdf_text_looks_empty(text: &str) -> bool {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("=== 第"))
        .all(|l| l.trim().is_empty())
}

/// N3-2：扫描件兜底编排。Ok(Some)=兜底成功（OCR 全文，带页标记与头部说明）；
/// Ok(None)=渲染或 OCR 失败（保持原提取输出）；Err(hint)=缺 pymupdf（hint 拼给模型）。
/// 隐私红线不变：页面 PNG 只落系统临时目录并在用后删除，识别全程本地。
async fn scan_pdf_ocr_text(app: &AppHandle, path: &str) -> Result<Option<String>, String> {
    const MAX_SCAN_PAGES: usize = 20;
    let tmp = std::env::temp_dir().join(format!("wm-scan-{}", uuid::Uuid::new_v4().simple()));
    if std::fs::create_dir_all(&tmp).is_err() {
        return Ok(None);
    }
    let render = crate::bot_py::pdf_render_pages(
        app.clone(),
        path.to_string(),
        tmp.display().to_string(),
        MAX_SCAN_PAGES,
    )
    .await;
    let pages = match render {
        Ok(n) => n,
        Err(e) if e.starts_with("NEED_PYMUPDF") => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "（检测到扫描版/图片型 PDF：文本层为空，已尝试转图本地 OCR 兜底，但本机缺 PyMuPDF——请运行 python3 -m pip install pymupdf 后重试；单次最多识别前 {MAX_SCAN_PAGES} 页）"
            ));
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "scan_pdf.render_fail | {}",
                    crate::bot::truncate_for_log(&e, 200)
                ),
            );
            let _ = std::fs::remove_dir_all(&tmp);
            return Ok(None);
        }
    };
    // 逐页 OCR（spawn_blocking：Vision/ONNX 均同步阻塞推理）；单页失败截断并注明
    let tmp_for_ocr = tmp.clone();
    let ocr = crate::py::document::spawn_blocking_map(
        move || -> Result<(Vec<String>, Option<String>), String> {
            let mut parts = Vec::new();
            let mut first_err = None;
            for i in 1..=pages {
                let p = tmp_for_ocr.join(format!("page-{i:03}.png"));
                let bytes = match std::fs::read(&p) {
                    Ok(b) => b,
                    Err(e) => {
                        first_err = Some(format!("读取渲染页失败：{e}"));
                        break;
                    }
                };
                match crate::ocr::recognize_bytes(&bytes) {
                    Ok(t) => parts.push(format!("=== 第{i}页 ===\n{t}")),
                    Err(e) => {
                        first_err = Some(format!("第 {i} 页识别失败：{e}"));
                        break;
                    }
                }
            }
            Ok((parts, first_err))
        },
    )
    .await
    .unwrap_or_else(|_| (Vec::new(), Some("OCR 线程异常".into())));
    let _ = std::fs::remove_dir_all(&tmp);
    let (parts, first_err) = ocr;
    if parts.is_empty() {
        if let Some(e) = &first_err {
            crate::bot::audit_log(
                app,
                &format!(
                    "scan_pdf.ocr_fail | {}",
                    crate::bot::truncate_for_log(e, 200)
                ),
            );
        }
        return Ok(None);
    }
    crate::bot::audit_log(app, &format!("scan_pdf.ocr | pages: {}", parts.len()));
    let mut text = format!(
        "〔扫描版 PDF：文本层为空，已转图走本地 OCR 兜底（前 {} 页）〕\n",
        parts.len()
    );
    text.push_str(&parts.join("\n"));
    if let Some(e) = first_err {
        text.push_str(&format!("\n（后续页中断：{e}）"));
    } else if pages == MAX_SCAN_PAGES {
        text.push_str(&format!(
            "\n（超过 {MAX_SCAN_PAGES} 页的部分未识别；可拆分后分次提取）"
        ));
    }
    Ok(Some(text))
}

/// extract_document 输出格式化：字符级分页（可续读，不硬截断）。
/// 头部 [位置] 行让模型知道总量与续读点；还有更多时尾部给 offset 续读提示。
/// offset 按字符计（非字节）；limit 默认 30000、硬钳 60000（防爆上下文）。
const EXTRACT_DEFAULT_LIMIT: usize = 30000;
const EXTRACT_MAX_LIMIT: usize = 60000;

fn format_extract_output(path: &str, text: &str, offset: usize, limit: usize) -> String {
    let total = text.chars().count();
    if offset >= total && total > 0 {
        return format!(
            "[文档路径] {path}\noffset {offset} 已超出文档总长 {total} 字符，没有更多内容"
        );
    }
    let slice: String = text.chars().skip(offset).take(limit).collect();
    let end = offset + slice.chars().count();
    let mut out =
        format!("[文档路径] {path}\n[位置] {offset}–{end} / 共 {total} 字符\n[文档内容]\n{slice}");
    if end < total {
        out.push_str(&format!(
            "\n\n（已截断：还有 {} 字符未读，用 offset={end} 参数续读；修订模式请把 original 参数填上你实际收到的原文行列表）",
            total - end
        ));
    }
    out
}

/// 解析文档工具共用的 filename 参数
fn opt_filename(v: &serde_json::Value) -> Option<String> {
    v["filename"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 生成 Word：润色后的段落写新文档（只产出、不覆盖，落 AI_Gen_Files）
pub(crate) async fn tool_create_word(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        // 「create_word 缺少 paragraphs」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok("create_word 缺少 paragraphs".to_string(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        // 「paragraphs 不能为空」首字「p」非 error/warn 前缀 → ok
        return ToolResult::ok("paragraphs 不能为空".to_string(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    let tables = v.get("tables").cloned();
    // N3-4：images 仅放行 AI_Gen_Files 目录内的已存在图片（防模型借参数探测/读任意路径），
    // 被拒条目审计并计入提示
    let (images, dropped_imgs) = sanitize_image_paths_arg(app, &v).await;
    match crate::bot_py::doc_make_word(
        app.clone(),
        title,
        paragraphs,
        opt_filename(&v),
        tables,
        images,
    )
    .await
    {
        Ok(out) => {
            let warn = if dropped_imgs > 0 {
                format!(
                    "（{dropped_imgs} 个图片路径被忽略：仅支持 AI_Gen_Files 目录内的已存在图片）"
                )
            } else {
                String::new()
            };
            // 「已生成 Word 文档」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(format!("已生成 Word 文档：{out}{warn}"), Vec::new())
        }
        // 「生成失败」首字「生」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("生成失败：{e}"), Vec::new()),
    }
}

/// N3-4：create_word 的 images 参数清洗（纯内核 filter_images_in 的异步壳）：
/// canonicalize 后必须落在 gen_dir 内且文件存在；整体包 spawn_blocking（同步 syscall
/// 不占 async worker，与 sanitize_task_files_arg 同模式）。
async fn sanitize_image_paths_arg(app: &AppHandle, v: &serde_json::Value) -> (Vec<String>, usize) {
    let Some(arr) = v["images"].as_array() else {
        return (Vec::new(), 0);
    };
    let paths: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    if paths.is_empty() {
        return (Vec::new(), 0);
    }
    let app2 = app.clone();
    let (out, dropped) =
        crate::py::document::spawn_blocking_map(move || -> Result<(Vec<String>, usize), String> {
            let gen_canon = crate::db::gen_dir(&app2)
                .ok()
                .and_then(|d| std::fs::canonicalize(d).ok());
            Ok(filter_images_in(gen_canon.as_deref(), paths))
        })
        .await
        .unwrap_or((Vec::new(), 0));
    if dropped > 0 {
        crate::bot::audit_log(
            app,
            &format!(
                "doc_word_images_sanitized | dropped: {dropped} | 仅放行 AI_Gen_Files 内已存在图片"
            ),
        );
    }
    (out, dropped)
}

/// images 清洗纯内核（单测注入 gen 目录）：存在且 canonicalize 后在 gen 内才放行
fn filter_images_in(
    gen_canon: Option<&std::path::Path>,
    paths: Vec<String>,
) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for p in paths {
        let ok = gen_canon.is_some_and(|g| {
            std::fs::canonicalize(&p)
                .map(|c| c.starts_with(g))
                .unwrap_or(false)
        });
        if ok {
            out.push(p);
        } else {
            dropped += 1;
        }
    }
    (out, dropped)
}

/// 修订模式 Word：回读原文 + 修订段落 diff，产出带 track changes 标记的文档
pub(crate) async fn tool_create_word_revisions(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    // originalPath 由模型转述，同样过授权校验（防回读任意文件，走分流）
    if let Some(op) = v["originalPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(e) = extract_path_check(app, op, interactive, session_id).await {
            return ToolResult::ok(e.to_string(), Vec::new());
        }
    }
    let Some(arr) = v["revised"].as_array() else {
        // 「create_word_revisions 缺少 revised」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok(
            "create_word_revisions 缺少 revised（润色后的段落列表）".to_string(),
            Vec::new(),
        );
    };
    let revised: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if revised.is_empty() {
        // 「revised 不能为空」首字「r」非 error/warn 前缀 → ok
        return ToolResult::ok("revised 不能为空".to_string(), Vec::new());
    }
    let path = v["originalPath"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let original: Vec<String> = v["original"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if path.is_none() && original.is_empty() {
        // 「create_word_revisions 缺少原文」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok(
            "create_word_revisions 缺少原文：请传 originalPath（来自 extract_document 的 [文档路径]）或 original 行列表"
                .to_string(),
            Vec::new(),
        );
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_word_revisions(app.clone(), title, path, original, revised, opt_filename(&v)).await
    {
        Ok((out, engine)) => ToolResult::ok(
            format!(
                "已生成修订版 Word（修订模式：删除线=删、红色下划线=增，可在 Word「审阅」里逐条接受/拒绝；引擎：{}）：{out}",
                if engine == "dotnet" { ".NET OpenXML" } else { "Python 兜底（.NET 不可用或执行失败）" }
            ),
            Vec::new(),
        ),
        // 「生成失败」首字「生」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("生成失败：{e}"), Vec::new()),
    }
}
pub(crate) async fn tool_create_excel(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(sheets) = v["sheets"].as_array() else {
        // 「create_excel 缺少 sheets」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok("create_excel 缺少 sheets".to_string(), Vec::new());
    };
    if sheets.is_empty() {
        // 「sheets 不能为空」首字「s」非 error/warn 前缀 → ok
        return ToolResult::ok("sheets 不能为空".to_string(), Vec::new());
    }
    match crate::bot_py::doc_make_excel(app.clone(), sheets.clone(), opt_filename(&v)).await {
        Ok(out) => {
            // 「已生成 Excel 文档」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(format!("已生成 Excel 文档：{out}"), Vec::new())
        }
        // 「生成失败」首字「生」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PPT：slides 结构 [{title, bullets: [..]}]
pub(crate) async fn tool_create_ppt(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(slides) = v["slides"].as_array() else {
        // 「create_ppt 缺少 slides」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok("create_ppt 缺少 slides".to_string(), Vec::new());
    };
    if slides.is_empty() {
        // 「slides 不能为空」首字「s」非 error/warn 前缀 → ok
        return ToolResult::ok("slides 不能为空".to_string(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    // theme：blue/navy/teal/forest/wine/sky/plum/coral/dark/green 十套；模型自选，非法回退 blue
    let theme = v["theme"].as_str().map(|s| s.to_string());
    // customColors：可选配色覆盖（脚本侧校验 hex，非法忽略）
    let custom_colors = v.get("customColors").cloned();
    match crate::bot_py::doc_make_ppt(
        app.clone(),
        title,
        slides.clone(),
        opt_filename(&v),
        theme,
        custom_colors,
    )
    .await
    {
        Ok(out) => {
            // 「已生成 PPT 演示文稿」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(format!("已生成 PPT 演示文稿：{out}"), Vec::new())
        }
        // 「生成失败」首字「生」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PDF：title + 段落列表 + 可选表格（N3-3，与 create_word 同形状）
pub(crate) async fn tool_create_pdf(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        // 「create_pdf 缺少 paragraphs」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok("create_pdf 缺少 paragraphs".to_string(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        // 「paragraphs 不能为空」首字「p」非 error/warn 前缀 → ok
        return ToolResult::ok("paragraphs 不能为空".to_string(), Vec::new());
    }
    let tables = v.get("tables").cloned();
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_pdf(app.clone(), title, paragraphs, tables, opt_filename(&v))
        .await
    {
        Ok(out) => {
            // 「已生成 PDF 文档」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(format!("已生成 PDF 文档：{out}"), Vec::new())
        }
        // 「生成失败」首字「生」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("生成失败：{e}"), Vec::new()),
    }
}

/// 联网搜索：本机执行（引擎路由 Tavily/Brave/双引擎抓取，见 bot_web::resolve_search_route），
/// 结果回传给模型
pub(crate) async fn tool_web_search(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(query) = v["query"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        // 「web_search 缺少 query」首字「w」非 error/warn 前缀 → ok
        return ToolResult::ok("web_search 缺少 query".to_string(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return ToolResult::ok(e.to_string(), Vec::new());
    }
    audit_log(
        app,
        &format!("web_search | query: {}", escape_for_log(&query, 100)),
    );
    // N3-5：过滤参数（count/timeRange/site），缺省 = 旧行为；
    // P3-a：count 缺省值走配置解析（config searchMaxResults，钳 1..=10；默认 8）
    let cfg_search = crate::bot::load_config(app);
    let opts = crate::bot_web::SearchOpts {
        count: v["count"]
            .as_u64()
            .map(|n| n.clamp(1, 10) as u32)
            .unwrap_or(crate::bot::params::resolve_search_max_results(&cfg_search)),
        time_range: crate::bot_web::TimeRange::parse(v["timeRange"].as_str()),
        site: v["site"].as_str().and_then(crate::bot_web::sanitize_site),
    };
    match crate::bot_web::web_search_with_config(app, &query, &opts).await {
        Ok(results) => ToolResult::ok(results, Vec::new()),
        // 「搜索失败」首字「搜」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("搜索失败：{e}"), Vec::new()),
    }
}

/// fetch_url 输出切片（N3-1 纯函数）：offset 字符级续读（与 extract_document 同模式）。
/// 无 offset 且内容不超限时输出与旧版零差异（不加头部行，省 token）；
/// 超限截断带续读提示；offset 越界给出总量提示。
const FETCH_BODY_CAP: usize = 30000;

fn slice_fetch_output(text: &str, offset: usize) -> String {
    let total = text.chars().count();
    if offset >= total && total > 0 {
        return format!("offset {offset} 已超出网页总长 {total} 字符，没有更多内容");
    }
    let slice: String = text.chars().skip(offset).take(FETCH_BODY_CAP).collect();
    let end = offset + slice.chars().count();
    let mut out = String::new();
    if offset > 0 {
        out.push_str(&format!("[位置] {offset}–{end} / 共 {total} 字符\n"));
    }
    out.push_str(&slice);
    if end < total {
        out.push_str(&format!(
            "\n\n（已截断：还有 {} 字符未读，用 offset={end} 参数续读）",
            total - end
        ));
    }
    out
}

/// 抓取网页正文：http/https 公网地址，转纯文本回传（30K 字分页，可 offset 续读）。
/// 每次调用重新抓取整页再切片——无缓存失效问题，网页内容也不会因续读而过期。
pub(crate) async fn tool_fetch_url(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(u) = v["url"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        // 「fetch_url 缺少 url」首字「f」非 error/warn 前缀 → ok
        return ToolResult::ok("fetch_url 缺少 url".to_string(), Vec::new());
    };
    if let Err(e) = check_len(&u, MAX_KEYWORD, "网址") {
        return ToolResult::ok(e.to_string(), Vec::new());
    }
    let offset = v["offset"].as_u64().unwrap_or(0) as usize;
    audit_log(
        app,
        &format!("fetch_url | url: {}", escape_for_log(&u, 100)),
    );
    match crate::bot_web::fetch_text(&u).await {
        Ok(text) => {
            // 抓取成功返回正文，首字符可能是任意 UTF-8 → ok
            ToolResult::ok(slice_fetch_output(&text, offset), Vec::new())
        }
        // 「抓取失败」首字「抓」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("抓取失败：{e}"), Vec::new()),
    }
}

/// 自由 Python 编程：开关开启才放行（超时 60s、独立临时目录、输出截断）
/// py_exec_sync 是 sync 阻塞（最长 300s），必须经 async 包装挪到 blocking
/// 线程池，不得占住 async runtime worker（与 doc_* 同模式）。
/// 透传 /stop 令牌，在途 Python 可被中断（StopGuard → owned StopToken）。
pub(crate) async fn tool_run_python(
    app: &AppHandle,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(code) = v["code"].as_str() else {
        // 「run_python 缺少 code」首字「r」非 error/warn 前缀 → ok
        return ToolResult::ok("run_python 缺少 code".to_string(), Vec::new());
    };
    // 超时优先级：工具参数 timeoutSecs > 设置页配置 python_timeout_secs > 内置 60s
    //（pandas 大计算 60s 偏紧；硬钳 300s 在 bot_py::resolve_timeout）
    let timeout_secs = v["timeoutSecs"]
        .as_u64()
        .or_else(|| load_config(app).python_timeout_secs);
    match crate::bot_py::py_exec_sync_async(
        app.clone(),
        code.to_string(),
        timeout_secs,
        stop.map(|s| s.token()),
    )
    .await
    {
        Ok(r) => {
            let mut out = String::new();
            if !r.stdout.trim().is_empty() {
                out.push_str(&r.stdout);
            }
            if !r.stderr.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[stderr] {}", r.stderr.trim()));
            }
            if out.is_empty() {
                out = "执行完成（无输出）".into();
            }
            // Python 执行输出，首字符可能是任意 UTF-8 → ok
            ToolResult::ok(out, Vec::new())
        }
        // 「执行失败」首字「执」非「失败」前缀 → ok
        Err(e) => ToolResult::ok(format!("执行失败：{e}"), Vec::new()),
    }
}

// ────────────────────────────────────────────────────────────────────
// 测试：BotConfig 序列化 + extract_document 输出格式化
// ────────────────────────────────────────────────────────────────────

/// BotConfig 序列化与默认值单测
#[cfg(test)]
mod tool_extract_document_tests {
    use super::*;

    #[test]
    fn format_extract_output_short_text_returns_full_text_no_truncation_suffix() {
        let text = "短文本".repeat(100); // 100 个汉字 = 300 chars，远低于默认 limit
        let out = format_extract_output("/tmp/sample.md", &text, 0, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains(&text),
            "短文本应原样保留：\n--out--\n{out}\n--text--\n{text}"
        );
        assert!(
            !out.contains("已截断"),
            "短文本不应出现截断提示，实际输出：\n{out}"
        );
        assert!(
            out.starts_with("[文档路径] /tmp/sample.md\n[位置] 0–300 / 共 300 字符\n[文档内容]\n"),
            "头部应带位置行，实际：\n{out}"
        );
    }

    #[test]
    fn format_extract_output_long_text_truncates_with_suffix() {
        // 35000 个 'A'，超默认 30000
        let text = "A".repeat(35000);
        let out = format_extract_output("/tmp/big.md", &text, 0, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains("已截断"),
            "长文本必须出现截断提示，实际输出末尾：\n{}",
            &out[out.len().saturating_sub(200)..]
        );
        // 输出含有的 'A' 数量应 == 30000（截断后）
        let a_count = out.matches('A').count();
        assert_eq!(
            a_count, 30000,
            "长文本截断后应剩 30000 个 'A'，实际 {a_count}"
        );
        // 续读提示给出下一页起点
        assert!(
            out.contains("offset=30000"),
            "截断提示应给续读 offset：\n{out}"
        );
    }

    #[test]
    fn format_extract_output_offset_reads_next_page() {
        let text = "A".repeat(35000);
        let out = format_extract_output("/tmp/big.md", &text, 30000, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains("[位置] 30000–35000 / 共 35000 字符"),
            "第二页位置行：\n{out}"
        );
        assert_eq!(out.matches('A').count(), 5000, "第二页应只有剩余 5000 字符");
        assert!(!out.contains("已截断"), "读完最后一页不应再有截断提示");
    }

    #[test]
    fn format_extract_output_offset_beyond_total() {
        let text = "短";
        let out = format_extract_output("/tmp/a.md", text, 100, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains("超出文档总长"),
            "offset 越界应明确提示：\n{out}"
        );
    }
}
/// 早退路径审计事件序列单测（tool.call 配平 tool.return）。
// 此处覆盖 parse_task_files_arg 单测；
// execute_tool 的 mock 路径见 skill_e2e.rs。
#[cfg(test)]
mod task_files_arg_tests {
    use super::*;

    /// files 参数解析：去重保序、空白丢弃、isDir 读取、超 10 截断标记
    #[test]
    fn parse_task_files_arg_dedup_cap() {
        // 正常解析 + isDir
        let v = serde_json::json!({"files": [
            {"path": "/a/1.pdf", "isDir": false},
            {"path": "/a/dir", "isDir": true},
        ]});
        let (files, truncated) = parse_task_files_arg(&v).unwrap();
        assert_eq!(files.len(), 2);
        assert!(!files[0].is_dir && files[1].is_dir);
        assert!(!truncated);

        // 去重保序 + 空路径丢弃
        let v = serde_json::json!({"files": [
            {"path": "/a/1.pdf"}, {"path": "/a/1.pdf"}, {"path": "  "}, {"path": "/a/2.pdf"},
        ]});
        let (files, _) = parse_task_files_arg(&v).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "/a/1.pdf");
        assert_eq!(files[1].path, "/a/2.pdf");

        // 超上限：12 → 10 且 truncated=true
        let many: Vec<serde_json::Value> = (0..12)
            .map(|i| serde_json::json!({"path": format!("/f/{i}.txt"), "isDir": false}))
            .collect();
        let v = serde_json::json!({"files": many});
        let (files, truncated) = parse_task_files_arg(&v).unwrap();
        assert_eq!(files.len(), crate::db::MAX_TASK_FILES, "超 10 截断");
        assert_eq!(files[9].path, "/f/9.txt", "保序截前 10");
        assert!(truncated, "超上限必须标记 truncated");

        // 无 files 字段 → None（不动绑定）；空数组 → Some(空)（清除绑定）
        assert!(parse_task_files_arg(&serde_json::json!({"title": "x"})).is_none());
        let (files, truncated) = parse_task_files_arg(&serde_json::json!({"files": []})).unwrap();
        assert!(files.is_empty() && !truncated);
    }

    /// 模型来源 files 仅放行 AI_Gen_Files 内已存在文件，
    /// 目录绑定一律丢（目录授权只能来自用户手选 bind_file）
    #[test]
    fn sanitize_task_files_drops_dirs_and_outside_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let gen = tmp.path().join("AI_Gen_Files");
        std::fs::create_dir_all(&gen).unwrap();
        let inside = gen.join("report.docx");
        std::fs::write(&inside, b"x").unwrap();
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, b"s").unwrap();
        let gen_canon = std::fs::canonicalize(&gen).unwrap();

        let files = vec![
            crate::db::TaskFile {
                path: inside.to_string_lossy().to_string(),
                is_dir: false,
            },
            crate::db::TaskFile {
                path: outside.to_string_lossy().to_string(),
                is_dir: false,
            },
            // 目录绑定（哪怕是 gen 目录本身）一律丢
            crate::db::TaskFile {
                path: gen.to_string_lossy().to_string(),
                is_dir: true,
            },
            // 不存在的路径也丢
            crate::db::TaskFile {
                path: gen.join("nope.txt").to_string_lossy().to_string(),
                is_dir: false,
            },
        ];
        let (kept, dropped) = sanitize_task_files_in(Some(&gen_canon), files);
        assert_eq!(kept.len(), 1, "只有 AI_Gen_Files 内已存在文件保留");
        assert!(kept[0].path.ends_with("report.docx"));
        assert_eq!(dropped, 3);

        // gen 目录不可用（None）→ 全丢（fail-closed）
        let (kept, dropped) = sanitize_task_files_in(
            None,
            vec![crate::db::TaskFile {
                path: inside.to_string_lossy().to_string(),
                is_dir: false,
            }],
        );
        assert!(kept.is_empty() && dropped == 1);
    }

    /// 双写：files 首条同步进旧 file_path/file_is_dir；空列表清三字段
    #[test]
    fn apply_files_to_task_dual_writes_legacy_fields() {
        let mut t = crate::db::Task {
            acceptance: None,
            id: "t".to_string(),
            title: "x".to_string(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: Some("/old.txt".into()),
            file_is_dir: Some(false),
            column: TaskStatus::Todo,
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: None,
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            origin: None,
            workflow_id: None,
            depends_on: None,
            canvas_pos: None,
            model: None,
            owner_id: None,
            created_at: None,
            enabled: None,
            expected_updated_at: None,
        };
        apply_files_to_task(
            &mut t,
            vec![
                crate::db::TaskFile {
                    path: "/n/1.pdf".to_string(),
                    is_dir: false,
                },
                crate::db::TaskFile {
                    path: "/n/dir".to_string(),
                    is_dir: true,
                },
            ],
        );
        assert_eq!(t.file_path.as_deref(), Some("/n/1.pdf"), "旧字段=首条");
        assert_eq!(t.file_is_dir, Some(false));
        assert_eq!(t.files.as_deref().unwrap().len(), 2);

        apply_files_to_task(&mut t, vec![]);
        assert!(t.files.is_none() && t.file_path.is_none() && t.file_is_dir.is_none());
    }

    /// tool.return 审计 kv：files_count=原始条数（截断前）、truncated 标记；无 files 不补 kv
    #[test]
    fn files_audit_kv_counts_raw_and_flags_truncation() {
        let args = r#"{"files":[{"path":"/a"},{"path":"/b"}]}"#;
        assert_eq!(files_audit_kv(args), Some((2, false)));
        let many: Vec<String> = (0..11)
            .map(|i| format!("{{\"path\":\"/f/{i}\"}}"))
            .collect();
        let args = format!("{{\"files\":[{}]}}", many.join(","));
        assert_eq!(files_audit_kv(&args), Some((11, true)), "超 10 → truncated");
        assert_eq!(files_audit_kv(r#"{"title":"x"}"#), None);
    }
}

#[cfg(test)]
mod phase4_facts_tests {
    use super::*;

    #[test]
    fn get_current_time_format_has_date_and_weekday() {
        let result = tool_get_current_time();
        let refs = &result.refs;
        let text = &result.text;
        assert!(refs.is_empty());
        assert!(text.starts_with("现在："), "实际：{text}");
        assert!(
            [
                "星期一",
                "星期二",
                "星期三",
                "星期四",
                "星期五",
                "星期六",
                "星期日"
            ]
            .iter()
            .any(|w| text.contains(w)),
            "应含中文星期，实际：{text}"
        );
    }
}

#[cfg(test)]
mod background_dialog_tests {
    /// 回归锁：后台执行（interactive=false）不得弹系统文件选择框——
    /// tool_bind_file 已下线（bind_files 复数形是前端走的），现仅 extract_document
    /// 无 path 后台必须拒绝。源码锁防回退。
    #[test]
    fn background_execution_never_pops_file_dialog() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot/tools.rs"))
                .unwrap();
        assert!(
            !text.contains(concat!("tool_bind", "_file(")),
            "tool_bind_file 死函数不得回退（前端走 bind_files 复数形）"
        );
        assert!(
            text.contains("后台执行不能弹窗选文件，请提供 path 参数"),
            "extract_document 无 path 后台必须拒绝"
        );
    }
}

// ───────────────────────── T1-QUERYTASKS：query 工具族纯函数测试 ─────────────────────────

#[cfg(test)]
mod query_tasks_tests {
    use super::*;

    fn task_with(
        column: TaskStatus,
        archived: Option<bool>,
        deleted_at: Option<i64>,
        title: &str,
    ) -> crate::db::Task {
        crate::db::Task {
            acceptance: None,
            id: format!("id-{title}"),
            title: title.to_string(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: None,
            file_is_dir: None,
            column,
            subtasks: None,
            completed_at: None,
            created_at: None,
            archived,
            deleted_at,
            collapsed: None,
            order: None,
            updated_at: None,
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            assignee: None,
            budget: None,
            result: None,
            origin: None,
            workflow_id: None,
            depends_on: None,
            canvas_pos: None,
            model: None,
            owner_id: None,
            enabled: None,
            expected_updated_at: None,
        }
    }

    #[test]
    fn parse_view_defaults_follow_query_presence() {
        // 自动档：无 query=Active 清单；有 query=All 全库检索（对齐原 list/search 口径）
        assert_eq!(parse_view(None, false), TaskView::Active);
        assert_eq!(parse_view(None, true), TaskView::All);
        assert_eq!(parse_view(Some("active"), false), TaskView::Active);
        assert_eq!(parse_view(Some("done"), true), TaskView::Done);
        assert_eq!(parse_view(Some("archived"), false), TaskView::Archived);
        assert_eq!(parse_view(Some("trash"), true), TaskView::Trash);
        assert_eq!(parse_view(Some("all"), false), TaskView::All);
        // 非法值 fail-soft 回退 Active
        assert_eq!(parse_view(Some("bogus"), true), TaskView::Active);
    }

    #[test]
    fn view_keep_partitions_four_card_states() {
        let active = task_with(TaskStatus::Todo, None, None, "a");
        let done = task_with(TaskStatus::Done, Some(false), None, "b");
        let archived = task_with(TaskStatus::Done, Some(true), None, "c");
        let trashed = task_with(TaskStatus::Todo, None, Some(1), "d");
        // Active：未完成未归档未删（原 active_tasks 口径）
        assert!(view_keep(TaskView::Active, &active));
        assert!(!view_keep(TaskView::Active, &done));
        assert!(!view_keep(TaskView::Active, &archived));
        assert!(!view_keep(TaskView::Active, &trashed));
        // Done：已完成未归档未删
        assert!(view_keep(TaskView::Done, &done));
        assert!(!view_keep(TaskView::Done, &archived));
        // Archived：已归档未删（跨状态列）
        assert!(view_keep(TaskView::Archived, &archived));
        assert!(!view_keep(TaskView::Archived, &done));
        // Trash：仅软删
        assert!(view_keep(TaskView::Trash, &trashed));
        assert!(!view_keep(TaskView::Trash, &active));
        // All：除回收站外全部
        assert!(view_keep(TaskView::All, &active));
        assert!(view_keep(TaskView::All, &done));
        assert!(view_keep(TaskView::All, &archived));
        assert!(!view_keep(TaskView::All, &trashed));
    }

    #[test]
    fn keyword_hit_matches_title_note_tag_subtask() {
        let mut t = task_with(TaskStatus::Todo, None, None, "季度汇报 PPT");
        assert!(keyword_hit("汇报", &t));
        assert!(!keyword_hit("不存在词xyz", &t));
        t.note = Some("引用了 Q3 数据".to_string());
        assert!(keyword_hit("q3", &t));
        t.tags = Some(vec!["工作".to_string(), "周报".to_string()]);
        assert!(keyword_hit("周报", &t));
        t.subtasks = Some(vec![crate::db::Subtask {
            id: "s1".into(),
            text: "核对配色变量".into(),
            done: false,
        }]);
        assert!(keyword_hit("配色", &t));
        // 空关键词恒命中（清单模式）
        assert!(keyword_hit("", &t));
    }

    #[test]
    fn tag_keep_is_exact_case_insensitive() {
        let mut t = task_with(TaskStatus::Todo, None, None, "a");
        t.tags = Some(vec!["Work".to_string()]);
        assert!(tag_keep("work", &t));
        assert!(!tag_keep("wor", &t), "标签过滤是精确匹配，不是 contains");
        assert!(tag_keep("", &t), "空标签恒命中");
    }

    #[test]
    fn parse_limit_defaults_and_clamps() {
        assert_eq!(parse_limit(&serde_json::json!({})), 50);
        assert_eq!(parse_limit(&serde_json::json!({"limit": 10})), 10);
        assert_eq!(parse_limit(&serde_json::json!({"limit": 0})), 1);
        assert_eq!(parse_limit(&serde_json::json!({"limit": 9999})), 200);
    }

    #[test]
    fn humanize_schedule_covers_all_four_formats() {
        assert_eq!(
            humanize_schedule(Some("daily:09:30")),
            Some("每天 09:30".to_string())
        );
        assert_eq!(
            humanize_schedule(Some("weekly:3:08:00")),
            Some("每周三 08:00".to_string())
        );
        assert_eq!(
            humanize_schedule(Some("monthly:1:10:00")),
            Some("每月1日 10:00".to_string())
        );
        assert_eq!(
            humanize_schedule(Some("at:2026-10-08T14:00")),
            Some("一次性 2026-10-08 14:00".to_string())
        );
        // 未知格式原样展示；None 不产出行
        assert_eq!(humanize_schedule(Some("weird")), Some("weird".to_string()));
        assert_eq!(humanize_schedule(None), None);
    }

    #[test]
    fn locate_subtask_prefers_id_falls_back_to_text() {
        let subs = vec![
            crate::db::Subtask {
                id: "sub-a".into(),
                text: "买牛奶".into(),
                done: false,
            },
            crate::db::Subtask {
                id: "sub-b".into(),
                text: "买面包".into(),
                done: true,
            },
        ];
        // id 精确优先（即使 text 关键词也能命中别的条）
        assert_eq!(locate_subtask(&subs, Some("sub-b"), Some("牛奶")), Ok(1));
        // 无 id 走文本 contains
        assert_eq!(locate_subtask(&subs, None, Some("奶")), Ok(0));
        // id 未命中：报 id 不存在（不静默回落文本，防误伤别的条）
        assert!(locate_subtask(&subs, Some("sub-x"), Some("奶")).is_err());
        // 两者都缺 / 空串
        assert!(locate_subtask(&subs, None, None).is_err());
        assert!(locate_subtask(&subs, Some("  "), None).is_err());
        assert!(locate_subtask(&subs, None, Some("  ")).is_err());
    }

    #[test]
    fn render_task_line_carries_workflow_tag() {
        let mut t = task_with(TaskStatus::Doing, Some(true), None, "写周报");
        t.workflow_id = Some("wf-1".into());
        let mut names = std::collections::HashMap::new();
        names.insert("wf-1".to_string(), "周报流水线".to_string());
        let line = render_task_line(&t, &names);
        assert!(line.contains("[进行中]"), "{line}");
        assert!(line.contains("（已归档）"), "{line}");
        assert!(line.contains("（工作流：周报流水线）"), "{line}");
        assert!(line.contains("（id=id-写周报）"), "{line}");
        // 孤儿 workflowId（workflows 表无此行）→ 标记省略，不 panic
        let line2 = render_task_line(&t, &std::collections::HashMap::new());
        assert!(!line2.contains("工作流"), "{line2}");
    }
}

// ───────────────────────── N3-TOOLPOLISH：六项升级纯函数测试 ─────────────────────────

#[cfg(test)]
mod n3_toolpolish_tests {
    use super::*;

    // ── N3-1：fetch_url offset 切片 ──

    #[test]
    fn fetch_slice_short_text_unchanged_no_header() {
        // 短文本 + 无 offset：与旧版输出零差异（不加 [位置] 行）
        let out = slice_fetch_output("你好世界", 0);
        assert_eq!(out, "你好世界");
    }

    #[test]
    fn fetch_slice_truncates_with_offset_hint() {
        let text = "A".repeat(35_000);
        let out = slice_fetch_output(&text, 0);
        assert!(out.contains("已截断"), "{out}");
        assert!(
            out.contains("offset=30000"),
            "应给续读点：{}",
            &out[out.len() - 120..]
        );
        assert_eq!(out.matches('A').count(), 30000, "正文截 30000");
    }

    #[test]
    fn fetch_slice_offset_reads_next_page_with_header() {
        let text: String = (0..35_000)
            .map(|i| char::from_u32(0x4e00 + (i % 500) as u32).unwrap())
            .collect();
        let out = slice_fetch_output(&text, 30000);
        assert!(out.starts_with("[位置] 30000–35000 / 共 35000 字符\n"));
        // 第二页读完 → 不再带截断提示
        assert!(!out.contains("已截断"), "35K 总量第二页应读完");
    }

    #[test]
    fn fetch_slice_offset_beyond_total_gives_hint() {
        let out = slice_fetch_output("短文本", 100);
        assert_eq!(out, "offset 100 已超出网页总长 3 字符，没有更多内容");
    }

    // ── N3-2：扫描 PDF 判定 ──

    #[test]
    fn pdf_empty_detection_ignores_page_markers() {
        assert!(pdf_text_looks_empty("=== 第1页 ===\n\n=== 第2页 ===\n  \n"));
        assert!(pdf_text_looks_empty(""), "空输出本身就是无文本层");
        assert!(!pdf_text_looks_empty("=== 第1页 ===\n合同正文"));
        // 标记只在行首才算（行中出现的相似文本是正文）
        assert!(!pdf_text_looks_empty("正文里引用了 === 第3页 === 字样"));
    }

    // ── N3-4：create_word images 清洗 ──

    #[test]
    fn filter_images_in_keeps_only_gen_dir_files() {
        let dir = std::env::temp_dir().join(format!("wm-img-t-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let inside = dir.join("pic.png");
        std::fs::write(&inside, b"png").unwrap();
        let outside = std::env::temp_dir().join("outside.png");
        let _ = std::fs::remove_file(&outside);
        std::fs::write(&outside, b"png").unwrap();
        let gen_canon = std::fs::canonicalize(&dir).ok();
        let (out, dropped) = filter_images_in(
            gen_canon.as_deref(),
            vec![
                inside.display().to_string(),
                outside.display().to_string(),
                "/nope/x.png".into(),
            ],
        );
        assert_eq!(out.len(), 1, "仅 gen 内图片放行：{out:?}");
        assert_eq!(dropped, 2);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_file(&outside).ok();
    }
}
