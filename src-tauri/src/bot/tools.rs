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

use crate::bot::dispatch::parse_args;
use crate::bot::{
    audit_log, check_len, escape_for_log, load_config, MAX_DUE, MAX_KEYWORD, MAX_NOTE,
    MAX_SUBTASK_TEXT, MAX_TAGS, MAX_TAG_LEN, MAX_TITLE,
};
use crate::error::CommandError;

// ───────────────────────── 时间 + 长期记忆工具 ─────────────────────────

/// get_current_time：返回本地日期时间+星期（模型做「今天/明天/周几」判断的锚点，禁止猜日期）
pub(crate) fn tool_get_current_time() -> (String, Vec<crate::bot_chat::TaskRef>) {
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
    (
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
fn sanitize_task_files_arg(
    app: &AppHandle,
    v: &serde_json::Value,
) -> Option<(Vec<crate::db::TaskFile>, bool)> {
    let (files, truncated) = parse_task_files_arg(v)?;
    // gen_dir 拿不到（创建失败）则按 None 处理——白名单校验「拿不到则拒」
    let gen_canon = crate::db::gen_dir(app)
        .ok()
        .and_then(|d| std::fs::canonicalize(d).ok());
    let (out, dropped) = sanitize_task_files_in(gen_canon.as_deref(), files);
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

async fn active_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    crate::db::db_load(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.deleted_at.is_none() && t.archived != Some(true) && t.column != "done")
        .collect()
}

// ───────────────────────── 任务管理工具实现（20+ functions） ─────────────────────────

pub(crate) async fn tool_list_tasks(app: &AppHandle) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let tasks = active_tasks(app).await;
    if tasks.is_empty() {
        return ("当前没有未完成的任务".into(), Vec::new());
    }
    let mut lines: Vec<String> = Vec::new();
    for t in &tasks {
        let col = match t.column.as_str() {
            "doing" => "进行中",
            "done" => "已完成",
            _ => "待办",
        };
        let due = t
            .due
            .as_deref()
            .map(|d| format!("，截止 {d}"))
            .unwrap_or_default();
        lines.push(format!("- [{}] {}{}（id={}）", col, t.title, due, t.id));
    }
    let refs: Vec<crate::bot_chat::TaskRef> = tasks
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

/// 单卡查询（白名单单点工具）：
/// 按 id 取单张任务卡的完整详情（区别于 list_tasks 的批量清单 + search_tasks 的关键词检索）。
/// - 必填参数：id（任务卡 UUID）
/// - 输出：标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除/机器人执行状态指示
/// - 单点白名单工具（非原子黑名单），LLM 可裸调
/// - 返回的 TaskRef 供后续 taskId 操作（complete_task / edit_task / bind_file 等）跟随引用
pub(crate) async fn tool_query_single_task(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(id) = v["id"].as_str().map(|s| s.trim().to_string()) else {
        return ("query_single_task 缺少 id 参数".into(), Vec::new());
    };
    if id.is_empty() {
        return ("query_single_task id 不能为空".into(), Vec::new());
    }
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
        return ("查询失败：数据库读取错误".into(), Vec::new());
    };
    let Some(t) = tasks.into_iter().find(|t| t.id == id) else {
        return (format!("未找到 id={id} 的任务卡"), Vec::new());
    };
    let col = match t.column.as_str() {
        "doing" => "进行中",
        "done" => "已完成",
        _ => "待办",
    };
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
    (
        lines.join("\n"),
        vec![crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        }],
    )
}

/// 搜索任务：搜所有任务卡（待办/进行中/已完成/已归档；不含回收站软删）。
/// 关键词匹配标题/备注/标签/子任务（大小写不敏感 contains）；结果带 id 供后续 taskId 操作
pub(crate) async fn tool_search_tasks(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("search_tasks 缺少 query".into(), Vec::new());
    };
    if query.is_empty() {
        return ("搜索关键词不能为空".into(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e.into(), Vec::new());
    }
    let tasks = crate::db::db_load(app.clone()).await.unwrap_or_default();
    let mut hits: Vec<crate::db::Task> = tasks
        .into_iter()
        .filter(|t| {
            t.deleted_at.is_none() && {
                let title_hit = t.title.to_lowercase().contains(&query);
                let note_hit = t
                    .note
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&query))
                    .unwrap_or(false);
                let tag_hit = t
                    .tags
                    .as_deref()
                    .map(|tags| tags.iter().any(|tg| tg.to_lowercase().contains(&query)))
                    .unwrap_or(false);
                let sub_hit = t
                    .subtasks
                    .as_deref()
                    .map(|subs| subs.iter().any(|s| s.text.to_lowercase().contains(&query)))
                    .unwrap_or(false);
                title_hit || note_hit || tag_hit || sub_hit
            }
        })
        .collect();
    if hits.is_empty() {
        return (
            format!(
                "没有找到匹配「{}」的任务",
                v["query"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    }
    hits.sort_by(|a, b| {
        a.order
            .unwrap_or(f64::MAX)
            .partial_cmp(&b.order.unwrap_or(f64::MAX))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut lines: Vec<String> = Vec::new();
    for t in &hits {
        let col = match t.column.as_str() {
            "doing" => "进行中",
            "done" => "已完成",
            _ => "待办",
        };
        let arch = if t.archived == Some(true) {
            "（已归档）"
        } else {
            ""
        };
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
        lines.push(format!(
            "- [{}] {}{}{}{}（id={}）",
            col, t.title, arch, due, tags, t.id
        ));
    }
    let refs: Vec<crate::bot_chat::TaskRef> = hits
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

pub(crate) async fn tool_create_task(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(title) = v["title"].as_str() else {
        return ("create_task 缺少 title".into(), Vec::new());
    };
    let title = title.trim();
    if title.is_empty() {
        return ("任务标题不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(title, MAX_TITLE, "任务标题") {
        return (e.into(), Vec::new());
    }
    if let Some(n) = v["note"].as_str() {
        if let Err(e) = check_len(n, MAX_NOTE, "备注") {
            return (e.into(), Vec::new());
        }
    }
    if let Some(d) = v["due"].as_str() {
        if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
            return (e.into(), Vec::new());
        }
    }
    let mut task = crate::db::Task {
        id: uuid::Uuid::new_v4().simple().to_string(),
        title: title.to_string(),
        due: v["due"].as_str().map(|s| s.to_string()),
        note: v["note"].as_str().map(|s| s.to_string()),
        tags: None,
        files: None,
        file_path: None,
        file_is_dir: None,
        column: match v["column"].as_str() {
            Some("doing") => "doing".into(),
            _ => "todo".into(),
        },
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(chrono::Utc::now().timestamp_millis()),
        schedule: None,
        sched_last: None,
        bot_assigned: None,
        expected_updated_at: None, // 新建任务：无读快照基线
    };
    // 多文件绑定：files 参数 [{path,isDir}]，超 10 截断 + 警告
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件）
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v) {
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
    match crate::db::db_upsert(app.clone(), vec![task.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![task.clone()], vec![]);
            (
                format!("已新建任务「{}」{files_warn}", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("新建任务失败：{e}"), Vec::new()),
    }
}

pub(crate) async fn tool_complete_task(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    // 走 resolve_task（taskId 精确匹配优先、title 关键词兜底 +
    // taskId/title 交叉校验），与其它任务操作工具对齐——只读 title 会让
    // schema 声明的「taskId 优先」被完全忽略，任务卡执行路径只传 taskId 时确定性失败。
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let mut next = task.clone();
    next.column = "done".into();
    next.completed_at = Some(chrono::Utc::now().timestamp_millis());
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at
    next.updated_at = next.completed_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已完成任务「{}」", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("完成任务失败：{e}"), Vec::new()),
    }
}

/// 删除任务到回收站：**弹窗确认后才执行**（危险操作护栏；60s 无响应默认拒绝）
pub(crate) async fn tool_delete_task(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
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
        return ("用户拒绝了删除，任务未删除".into(), Vec::new());
    }
    let mut next = task.clone();
    next.deleted_at = Some(chrono::Utc::now().timestamp_millis());
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at
    next.updated_at = next.deleted_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已删除任务「{}」（进回收站）", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("删除任务失败：{e}"), Vec::new()),
    }
}

/// 按标题关键词找第一条未完成任务（大小写不敏感）
async fn find_task_by_keyword(app: &AppHandle, kw: &str) -> Option<crate::db::Task> {
    active_tasks(app)
        .await
        .into_iter()
        .find(|t| t.title.to_lowercase().contains(kw))
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
            if let Some(t) = active_tasks(app).await.into_iter().find(|t| t.id == id) {
                // 交叉校验：同窗格连续操作不同任务卡时，模型会沿用上一张卡的
                // taskId 张冠李戴。taskId 与 title 关键词同时给出且对不上 → 不信 id，
                // 改用 title 重新定位（定位不到就报错，让模型/用户确认）
                if let Some(kw) = v["title"].as_str().map(|s| s.trim().to_lowercase()) {
                    if !kw.is_empty() && !t.title.to_lowercase().contains(&kw) {
                        if let Some(t2) = find_task_by_keyword(app, &kw).await {
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
            if let Some(t) = find_task_by_keyword(app, &kw).await {
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

pub(crate) async fn tool_edit_task(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let mut next = task.clone();
    let mut changed: Vec<&str> = Vec::new();
    if let Some(nt) = v["newTitle"].as_str() {
        let nt = nt.trim();
        if !nt.is_empty() {
            if let Err(e) = check_len(nt, MAX_TITLE, "新标题") {
                return (e.into(), Vec::new());
            }
            next.title = nt.to_string();
            changed.push("标题");
        }
    }
    if let Some(n) = v["note"].as_str() {
        if !n.trim().is_empty() {
            if let Err(e) = check_len(n, MAX_NOTE, "备注") {
                return (e.into(), Vec::new());
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
                return (e.into(), Vec::new());
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
            return (format!("标签数量超上限（最多 {MAX_TAGS} 个）"), Vec::new());
        }
        for t in tags {
            if let Some(ts) = t.as_str() {
                if let Err(e) = check_len(ts, MAX_TAG_LEN, "标签") {
                    return (e.into(), Vec::new());
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
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v) {
        if truncated {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
        apply_files_to_task(&mut next, files);
        changed.push("绑定文件");
    }
    if let Some(c) = v["column"].as_str() {
        let c = c.trim();
        if matches!(c, "todo" | "doing" | "done") && c != next.column {
            next.column = c.to_string();
            // 列变更补完成语义（与主窗口一致）
            if c == "done" {
                next.completed_at = Some(chrono::Utc::now().timestamp_millis());
                next.archived = Some(false);
            } else {
                next.completed_at = None;
                next.archived = None;
            }
            changed.push("状态列");
        }
    }
    if changed.is_empty() {
        return ("没有可修改的字段".into(), Vec::new());
    }
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!(
                    "已更新任务「{}」（{}）{files_warn}",
                    next.title,
                    changed.join("、")
                ),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("编辑任务失败：{e}"), Vec::new()),
    }
}

pub(crate) async fn tool_add_subtask(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(text) = v["text"].as_str().map(|s| s.trim()) else {
        return ("add_subtask 缺少 text".into(), Vec::new());
    };
    if text.is_empty() {
        return ("子任务内容不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(text, MAX_SUBTASK_TEXT, "子任务内容") {
        return (e.into(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let mut next = task.clone();
    let mut subs = next.subtasks.unwrap_or_default();
    subs.push(crate::db::Subtask {
        id: uuid::Uuid::new_v4().simple().to_string(),
        text: text.to_string(),
        done: false,
    });
    next.subtasks = Some(subs);
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已给任务「{}」添加子任务「{}」", next.title, text),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("添加子任务失败：{e}"), Vec::new()),
    }
}

pub(crate) async fn tool_toggle_subtask(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(skw) = v["text"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("toggle_subtask 缺少 text".into(), Vec::new());
    };
    if let Err(e) = check_len(&skw, MAX_SUBTASK_TEXT, "子任务关键词") {
        return (e.into(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let Some(idx) = subs
        .iter()
        .position(|s| s.text.to_lowercase().contains(&skw))
    else {
        return (
            format!(
                "任务「{}」没有匹配「{}」的子任务",
                task.title,
                v["text"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    };
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2[idx].done = !subs2[idx].done;
    next.subtasks = Some(subs2);
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
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
            (
                format!(
                    "子任务「{st_text}」已{}",
                    if done_mark {
                        "勾选 ✓"
                    } else {
                        "取消勾选"
                    }
                ),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("切换子任务状态失败：{e}"), Vec::new()),
    }
}

pub(crate) async fn tool_remove_subtask(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(skw) = v["text"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("remove_subtask 缺少 text".into(), Vec::new());
    };
    if skw.is_empty() {
        return ("子任务关键词不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(&skw, MAX_SUBTASK_TEXT, "子任务关键词") {
        return (e.into(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let Some(idx) = subs
        .iter()
        .position(|s| s.text.to_lowercase().contains(&skw))
    else {
        return (
            format!(
                "任务「{}」没有匹配「{}」的子任务",
                task.title,
                v["text"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    };
    let removed_text = subs[idx].text.clone();
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2.remove(idx);
    next.subtasks = Some(subs2);
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已删除任务「{}」的子任务「{}」", next.title, removed_text),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("删除子任务失败：{e}"), Vec::new()),
    }
}

/// 登记产物到任务卡执行流程的产物清单（不立即绑）
///
/// 设计：bot 流程内调用是「登记」语义——记到内存登记表 `bot_artifacts::REGISTRY`，
/// 流程结束按 `TaskExecOrigin` 分流（D4d）弹汇总窗口让用户勾选绑定。
/// 普通 chat 场景（无 TaskExecOrigin 上下文）直接拒，避免登记表被反复污染。
///
/// 路径白名单：仅接受 AI_Gen_Files 目录内的文件（产物必经此目录生成），
/// 防止 LLM 借 bind_files 间接读 ~/.ssh/id_rsa 等敏感文件。
pub(crate) async fn tool_link_file_to_task(
    app: &AppHandle,
    args: &str,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    if !crate::tool_guard::is_task_execution_flow(session_id) {
        return (
            "link_file_to_task 仅在任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内有效；普通对话场景调用无效果（不报错也不绑），不要反复尝试".into(),
            Vec::new(),
        );
    }
    let v = parse_args(args);
    let Some(path) = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("link_file_to_task 缺少 path".into(), Vec::new());
    };
    if !std::path::Path::new(&path).exists() {
        return (format!("路径不存在，拒绝登记：{path}"), Vec::new());
    }
    let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| std::path::PathBuf::from(&path));
    let in_gen = crate::db::gen_dir(app)
        .ok()
        .and_then(|d| std::fs::canonicalize(d).ok())
        .is_some_and(|gen| canon.starts_with(&gen));
    if !in_gen {
        return (
            "已拒绝登记该路径：link_file_to_task 只能登记 AI_Gen_Files 目录内的产物；其他文件请在任务卡上手动「绑定文件」".into(),
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
        return ("link_file_to_task 缺少 taskId / title".into(), Vec::new());
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
    (
        format!(
            "已登记{kind_label}「{path}」。流程结束、任务完成时会弹汇总窗口让你勾选绑定；不要在此刻绑定——任务未完成或中断不绑定。"
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
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let path_opt = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 无 path 时 bot_py::doc_extract 会弹系统选择框，
    // 后台定时执行弹框 = 永久阻塞卡死，必须直接拒绝并引导模型传 path
    if path_opt.is_none() && !interactive {
        return (
            "失败：后台执行不能弹窗选文件，请提供 path 参数指定文档路径".into(),
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
            return (e.into(), Vec::new());
        }
    }
    match crate::bot_py::doc_extract(app.clone(), path_opt).await {
        Ok(res) => (
            format_extract_output(&res.path, &res.text, offset, limit),
            Vec::new(),
        ),
        Err(e) => (format!("提取失败：{e}"), Vec::new()),
    }
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
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ("create_word 缺少 paragraphs".into(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ("paragraphs 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    let tables = v.get("tables").cloned();
    match crate::bot_py::doc_make_word(app.clone(), title, paragraphs, opt_filename(&v), tables)
        .await
    {
        Ok(out) => (format!("已生成 Word 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 修订模式 Word：回读原文 + 修订段落 diff，产出带 track changes 标记的文档
pub(crate) async fn tool_create_word_revisions(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    // originalPath 由模型转述，同样过授权校验（防回读任意文件，走分流）
    if let Some(op) = v["originalPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(e) = extract_path_check(app, op, interactive, session_id).await {
            return (e.into(), Vec::new());
        }
    }
    let Some(arr) = v["revised"].as_array() else {
        return (
            "create_word_revisions 缺少 revised（润色后的段落列表）".into(),
            Vec::new(),
        );
    };
    let revised: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if revised.is_empty() {
        return ("revised 不能为空".into(), Vec::new());
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
        return (
            "create_word_revisions 缺少原文：请传 originalPath（来自 extract_document 的 [文档路径]）或 original 行列表"
                .into(),
            Vec::new(),
        );
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_word_revisions(app.clone(), title, path, original, revised, opt_filename(&v)).await
    {
        Ok((out, engine)) => (
            format!(
                "已生成修订版 Word（修订模式：删除线=删、红色下划线=增，可在 Word「审阅」里逐条接受/拒绝；引擎：{}）：{out}",
                if engine == "dotnet" { ".NET OpenXML" } else { "Python 兜底（.NET 不可用或执行失败）" }
            ),
            Vec::new(),
        ),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}
pub(crate) async fn tool_create_excel(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(sheets) = v["sheets"].as_array() else {
        return ("create_excel 缺少 sheets".into(), Vec::new());
    };
    if sheets.is_empty() {
        return ("sheets 不能为空".into(), Vec::new());
    }
    match crate::bot_py::doc_make_excel(app.clone(), sheets.clone(), opt_filename(&v)).await {
        Ok(out) => (format!("已生成 Excel 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PPT：slides 结构 [{title, bullets: [..]}]
pub(crate) async fn tool_create_ppt(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(slides) = v["slides"].as_array() else {
        return ("create_ppt 缺少 slides".into(), Vec::new());
    };
    if slides.is_empty() {
        return ("slides 不能为空".into(), Vec::new());
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
        Ok(out) => (format!("已生成 PPT 演示文稿：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PDF：title + 段落列表
pub(crate) async fn tool_create_pdf(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ("create_pdf 缺少 paragraphs".into(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ("paragraphs 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_pdf(app.clone(), title, paragraphs, opt_filename(&v)).await {
        Ok(out) => (format!("已生成 PDF 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 联网搜索：本机执行（引擎路由 Tavily/Brave/双引擎抓取，见 bot_web::resolve_search_route），
/// 结果回传给模型
pub(crate) async fn tool_web_search(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("web_search 缺少 query".into(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e.into(), Vec::new());
    }
    audit_log(
        app,
        &format!("web_search | query: {}", escape_for_log(&query, 100)),
    );
    match crate::bot_web::web_search_with_config(app, &query).await {
        Ok(results) => (results, Vec::new()),
        Err(e) => (format!("搜索失败：{e}"), Vec::new()),
    }
}

/// 抓取网页正文：http/https 公网地址，转纯文本回传（截 30000 字）
pub(crate) async fn tool_fetch_url(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(u) = v["url"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("fetch_url 缺少 url".into(), Vec::new());
    };
    if let Err(e) = check_len(&u, MAX_KEYWORD, "网址") {
        return (e.into(), Vec::new());
    }
    audit_log(
        app,
        &format!("fetch_url | url: {}", escape_for_log(&u, 100)),
    );
    match crate::bot_web::fetch_text(&u).await {
        Ok(text) => {
            let mut out: String = text.chars().take(30000).collect();
            if out.chars().count() >= 30000 {
                out.push_str("\n\n（内容过长已截断）");
            }
            (out, Vec::new())
        }
        Err(e) => (format!("抓取失败：{e}"), Vec::new()),
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
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(code) = v["code"].as_str() else {
        return ("run_python 缺少 code".into(), Vec::new());
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
            (out, Vec::new())
        }
        Err(e) => (format!("执行失败：{e}"), Vec::new()),
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
/// execute_tool 是 Wry AppHandle 签名，无法 mock runtime 直调（见 tests/skill_e2e.rs 注释），
/// 故事件序列抽为纯函数 early_return_events，这里验证事件名/顺序/reason kv。
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
            id: "t".into(),
            title: "x".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: Some("/old.txt".into()),
            file_is_dir: Some(false),
            column: "todo".into(),
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
            expected_updated_at: None,
        };
        apply_files_to_task(
            &mut t,
            vec![
                crate::db::TaskFile {
                    path: "/n/1.pdf".into(),
                    is_dir: false,
                },
                crate::db::TaskFile {
                    path: "/n/dir".into(),
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
        let (text, refs) = tool_get_current_time();
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
