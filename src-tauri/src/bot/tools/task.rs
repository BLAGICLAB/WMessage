//! 任务面：查询族（view/过滤/渲染/query）与管理族（增删改/子任务/绑文件）。

use super::*;

// 任务查询工具（：list+search 合并）

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
pub(super) fn parse_view(v: Option<&str>, has_query: bool) -> TaskView {
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
pub(super) fn view_keep(view: TaskView, t: &crate::db::Task) -> bool {
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
pub(super) fn keyword_hit(kw: &str, t: &crate::db::Task) -> bool {
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
pub(super) fn tag_keep(tag: &str, t: &crate::db::Task) -> bool {
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
pub(super) fn parse_limit(v: &serde_json::Value) -> usize {
    v["limit"]
        .as_u64()
        .map(|n| n.clamp(1, 200) as usize)
        .unwrap_or(50)
}

/// 任务行渲染（query_tasks 统一行格式：列/标题/归档/截止/标签/id）
pub(super) fn render_task_line(t: &crate::db::Task) -> String {
    let col = status_label(t.column);
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
    format!("- [{col}] {}{arch}{due}{tags}（id={}）", t.title, t.id)
}

/// 查询任务（：原 list_tasks + search_tasks 合并）：
/// - 无 query：列清单（view 默认 active = 原 list_tasks）
/// - 有 query：关键词检索（view 默认 all 全库 = 原 search_tasks）
/// - view/tag/limit 正交过滤
pub(crate) async fn tool_query_tasks(app: &AppHandle, args: &str) -> ToolResult {
    let v = parse_args(args);
    let raw_query = v["query"].as_str().map(|s| s.trim().to_string());
    let has_query = raw_query.as_deref().is_some_and(|q| !q.is_empty());
    // 显式传了空串 query：提示语义（不传 = 清单），不当作全库检索
    if raw_query.as_deref() == Some("") {
        return ToolResult::denied("query 为空串：要列清单请不传 query 参数");
    }
    if let Some(q) = raw_query.as_deref() {
        if let Err(e) = check_len(q, MAX_KEYWORD, "搜索关键词") {
            return ToolResult::denied(e);
        }
    }
    let tag = v["tag"].as_str().unwrap_or("");
    if let Err(e) = check_len(tag, MAX_TAG_LEN, "标签") {
        return ToolResult::denied(e);
    }
    let limit = parse_limit(&v);
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
        return ToolResult::error("查询失败：数据库读取错误".to_string(), Vec::new());
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
    let mut lines: Vec<String> = Vec::new();
    for t in &hits {
        lines.push(render_task_line(t));
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

// 任务管理工具实现（20+ functions）

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
        return ToolResult::denied("query_single_task 缺少 id 参数");
    };
    if id.is_empty() {
        return ToolResult::denied("query_single_task id 不能为空");
    }
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
        return ToolResult::error("查询失败：数据库读取错误".to_string(), Vec::new());
    };
    let Some(t) = tasks.iter().find(|t| t.id == id).cloned() else {
        return ToolResult::denied(format!("未找到 id={id} 的任务卡"));
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
    // 只读行：创建时间 / 依赖（模型可感知字段，无写入口）
    if let Some(ca) = t.created_at {
        if let Some(dt) = chrono::DateTime::from_timestamp_millis(ca) {
            lines.push(format!(
                "  创建：{}",
                dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")
            ));
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

/// create/edit 共用的 due 参数闸：长度 + 格式一起判。格式判据与 task_patch /
/// HTTP 三写口共用（db::tasks::due_format_error）——不合法的截止落库后
/// due_notify 按「无截止」处理，通知静默不发。
///
/// 格式拒因带行动指引：底座提示词虽有「被拒不降级重试」，实测模型在紧邻的
/// 下一跳仍会丢参重建卡（用户要的截止时间悄悄消失）。回执是模型当下唯一
/// 盯着的信息，指引贴在案发现场比重复底座提示词有效。
pub(super) fn due_param_denied(d: &str) -> Option<ToolResult> {
    if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
        return Some(ToolResult::denied(e));
    }
    crate::db::tasks::due_format_error(d).map(|r| {
        ToolResult::denied(format!(
            "{r}；due 参数被拒不接受丢参或改填别的值重试——把上面的原因原样转告用户，请用户给出明确的截止时间"
        ))
    })
}

pub(crate) async fn tool_create_task(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(title) = v["title"].as_str() else {
        return ToolResult::denied("create_task 缺少 title");
    };
    let title = title.trim();
    if title.is_empty() {
        return ToolResult::denied("任务标题不能为空");
    }
    if let Err(e) = check_len(title, MAX_TITLE, "任务标题") {
        return ToolResult::denied(e);
    }
    if let Some(n) = v["note"].as_str() {
        if let Err(e) = check_len(n, MAX_NOTE, "备注") {
            return ToolResult::denied(e);
        }
    }
    if let Some(denied) = v["due"].as_str().and_then(due_param_denied) {
        return denied;
    }
    let now = crate::db::now_ms();
    let mut task = crate::db::Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: uuid::Uuid::new_v4().simple().to_string(),
        title: title.to_string(),
        due: v["due"].as_str().map(|s| s.to_string()),
        note: v["note"].as_str().map(|s| s.to_string()),
        tags: None,
        files: None,
        column: match v["column"].as_str() {
            None => TaskStatus::Todo,
            Some("todo") => TaskStatus::Todo,
            Some("doing") => TaskStatus::Doing,
            // 非法枚举响亮拒绝（schema enum 只有 todo/doing）：静默降级成 todo
            // 会让模型以为建成了它要的列
            Some(other) => {
                return ToolResult::denied(format!(
                    "column 只能是 todo 或 doing（收到 {other:?}）"
                ));
            }
        },
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(now),
        bot_assigned: None,
        assignee: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,            // 机器人建卡 = 本人（任务图谱设计 §1.1）
        created_at: Some(now),     // 创建时间打戳（与 updated_at 同值；此后 UPDATE 不覆盖）
        expected_updated_at: None, // 新建任务：无读快照基线
    };
    // 多文件绑定：files 参数 [{path,isDir}]，超 10 截断 + 警告
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件/文件夹）；新卡无历史绑定，
    // 统一内核追加即落位
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v).await {
        let (_, overflow) = crate::db::upsert_files_to_task(&mut task, files);
        if truncated || overflow > 0 {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
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
        Err(e) => return resolve_task_err(e),
    };
    let mut next = task.clone();
    // 完成三件套与 task_set_column / HTTP 同口径：Done + 完成时间 + 归档位
    // 复位 + 机器人接管标记清除（漏了会让完成卡继续显示机器人在办）
    next.column = TaskStatus::Done;
    next.completed_at = Some(crate::db::now_ms());
    next.archived = Some(false);
    next.bot_assigned = None;
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
        Err(e) => return resolve_task_err(e),
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
        return ToolResult::denied("用户拒绝了删除，任务未删除");
    }
    let mut next = task.clone();
    next.deleted_at = Some(crate::db::now_ms());
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

/// 定位失败的归类：读库失败 = 动了没成（error）；定位不到 / 参数缺失 /
/// taskId 与标题打架 = 工具没动手（denied）。
///
/// 按 `CommandError` 的变体分，不看文案——文案只给模型读，成败由 status 承担。
fn resolve_task_err(e: CommandError) -> ToolResult {
    let text = e.to_string();
    match e {
        CommandError::DbError(_) => ToolResult::error(text, Vec::new()),
        _ => ToolResult::denied(text),
    }
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

/// owner 参数解析：personId 精确 → 名字精确 → 「我/本人」→ 名字唯一包含。
/// 歧义/未命中返回候选文案（由调用方走 denied 通道回给模型，让模型向用户消歧）。
///
/// 成员表由调用方读进来：读表失败是执行失败（error），与这里的「歧义 / 未命中」
/// （工具没动手，denied）不是一回事，不混进同一个 Err。
fn resolve_owner(list: &[crate::db::PeopleEntry], input: &str) -> Result<String, String> {
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
        Err(e) => return resolve_task_err(e),
    };
    let mut next = task.clone();
    let mut changed: Vec<&str> = Vec::new();
    if let Some(nt) = v["newTitle"].as_str() {
        let nt = nt.trim();
        if !nt.is_empty() {
            if let Err(e) = check_len(nt, MAX_TITLE, "新标题") {
                return ToolResult::denied(e);
            }
            next.title = nt.to_string();
            changed.push("标题");
        }
    }
    if let Some(n) = v["note"].as_str() {
        if !n.trim().is_empty() {
            if let Err(e) = check_len(n, MAX_NOTE, "备注") {
                return ToolResult::denied(e);
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
            if let Some(denied) = due_param_denied(d) {
                return denied;
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
            return ToolResult::denied(format!("标签数量超上限（最多 {MAX_TAGS} 个）"));
        }
        for t in tags {
            if let Some(ts) = t.as_str() {
                if let Err(e) = check_len(ts, MAX_TAG_LEN, "标签") {
                    return ToolResult::denied(e);
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
    // 多文件绑定：files 参数 [{path,isDir}] 绑定一律加法——文件追加、文件夹槽替换
    // （不覆盖既有文件绑定）；空数组 = 无操作。清空/减少绑定是 UI 专属操作，模型不可达。
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件/文件夹）
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v).await {
        let (applied, overflow) = crate::db::upsert_files_to_task(&mut next, files);
        if truncated || overflow > 0 {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
        if applied > 0 {
            changed.push("绑定文件");
        }
    }
    if let Some(c) = v["column"].as_str() {
        // 非法枚举响亮拒绝而不是静默跳过：跳过后模型收到「没有可修改的字段」，
        // 错误归因会被带偏
        let c: TaskStatus = match c.trim().parse() {
            Ok(c) => c,
            Err(e) => return ToolResult::denied(format!("column 不合法：{e}")),
        };
        if c != next.column {
            next.column = c;
            // 完成三件套与 task_set_column / HTTP 同口径（含清 bot_assigned）
            if c == TaskStatus::Done {
                next.completed_at = Some(crate::db::now_ms());
                next.archived = Some(false);
                next.bot_assigned = None;
            } else {
                next.completed_at = None;
                next.archived = None;
            }
            changed.push("状态列");
        }
    }
    // 每卡执行模型（W6 model 列的模型写入口；空串=清除恢复跟随全局）
    if let Some(m) = v["model"].as_str() {
        let m = m.trim();
        if !m.is_empty() {
            if let Err(e) = check_len(m, MAX_MODEL_NAME, "模型名") {
                return ToolResult::denied(e);
            }
            next.model = Some(m.to_string());
        } else {
            next.model = None;
        }
        changed.push("执行模型");
    }
    // 归属人（任务图谱 owner 列；空串=归属本人；名字歧义报候选）
    if let Some(o) = v["owner"].as_str() {
        let o = o.trim();
        if o.is_empty() {
            next.owner_id = None;
            changed.push("归属人");
        } else {
            // people_list 是同步 command（people.rs 无 async），直接调
            let list = match crate::db::people_list(app.clone()) {
                Ok(l) => l,
                Err(e) => return ToolResult::error(format!("读取成员失败：{e}"), Vec::new()),
            };
            match resolve_owner(&list, o) {
                Ok(pid) => {
                    next.owner_id = Some(pid);
                    changed.push("归属人");
                }
                Err(msg) => return ToolResult::denied(msg),
            }
        }
    }
    if changed.is_empty() {
        return ToolResult::denied("没有可修改的字段");
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
        return ToolResult::denied("add_subtask 缺少 text");
    };
    if text.is_empty() {
        return ToolResult::denied("子任务内容不能为空");
    }
    if let Err(e) = check_len(text, MAX_SUBTASK_TEXT, "子任务内容") {
        return ToolResult::denied(e);
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return resolve_task_err(e),
    };
    let mut next = task.clone();
    let mut subs = next.subtasks.unwrap_or_default();
    subs.push(crate::db::Subtask {
        id: uuid::Uuid::new_v4().simple().to_string(),
        text: text.to_string(),
        done: false,
        task_id: None,
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

/// 子任务定位：subtaskId 精确优先，回落 text 关键词 contains。
/// 返回 index；参数缺失/未命中返回错误文案（调用方走 denied 通道）。
pub(super) fn locate_subtask(
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
                return ToolResult::denied(e);
            }
        }
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return resolve_task_err(e),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let idx = match locate_subtask(&subs, sub_id, text_kw) {
        Ok(i) => i,
        Err(msg) => return ToolResult::denied(format!("任务「{}」{msg}", task.title)),
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
                return ToolResult::denied(e);
            }
        }
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return resolve_task_err(e),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let idx = match locate_subtask(&subs, sub_id, text_kw) {
        Ok(i) => i,
        Err(msg) => return ToolResult::denied(format!("任务「{}」{msg}", task.title)),
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
/// 流程结束按 `TaskExecOrigin` 分流落「通知中心」待绑定消息让用户勾选。
/// 普通 chat 场景（无 TaskExecOrigin 上下文）直接拒，避免登记表被反复污染。
///
/// 路径白名单：仅接受 AI_Gen_Files 目录内的文件（产物必经此目录生成），
/// 防止 LLM 借 bind_files 间接读 ~/.ssh/id_rsa 等敏感文件。
/// 绑定目标解析（纯函数供单测）：taskId 直查；"title:X" 精确匹配标题。
/// 必须命中恰一张未删除卡——0 张（不存在/回收站）→ Err（link_file_to_task
/// 不能新建卡）；多张同名 → Err（改用 taskId 消歧）。
pub(crate) fn resolve_task_key<'a>(
    tasks: &'a [crate::db::Task],
    key: &str,
) -> Result<&'a crate::db::Task, String> {
    if let Some(title) = key.strip_prefix("title:") {
        let matched: Vec<&crate::db::Task> = tasks
            .iter()
            .filter(|t| t.deleted_at.is_none() && t.title == title)
            .collect();
        match matched.len() {
            0 => Err(format!(
                "没有找到名为「{title}」的任务卡。link_file_to_task 不能新建卡，请先建卡或改用 taskId"
            )),
            1 => Ok(matched[0]),
            n => Err(format!(
                "有 {n} 张同名任务卡「{title}」，请改用 taskId 指定"
            )),
        }
    } else {
        tasks
            .iter()
            .find(|t| t.id == key && t.deleted_at.is_none())
            .ok_or_else(|| format!("任务卡不存在（或已在回收站）：{key}"))
    }
}

pub(crate) async fn tool_link_file_to_task(
    app: &AppHandle,
    args: &str,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(path) = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("link_file_to_task 缺少 path");
    };
    // canonicalize 失败不回退原路径（fail-closed）：白名单比对必须基于
    // 规范化后的真实落点，拿原始路径比 lexical starts_with 会给软链/竞态窗口留绕过面
    //
    // 同步 syscall（exists / canonicalize / gen_dir）整体包 spawn_blocking：跑在
    // async runtime 线程上会阻塞全部 Tauri command / event（ performance）。
    let app_for_probe = app.clone();
    let path_for_probe = path.clone();
    let checked = crate::py::document::spawn_blocking_map(move || {
        Ok::<_, String>(probe_paths_sync(&path_for_probe, &app_for_probe))
    })
    .await;
    let probe = match checked {
        Ok(p) => p,
        Err(e) => {
            return ToolResult::denied(format!("路径校验失败，拒绝绑定：{path}（{e}）"));
        }
    };
    if !probe.exists {
        return ToolResult::denied(format!("路径不存在，拒绝绑定：{path}"));
    }
    let Some(canon) = probe.canon else {
        return ToolResult::denied(format!(
            "路径校验失败，拒绝绑定：{path}（无法解析真实路径，请确认文件仍然存在后重试）"
        ));
    };
    let in_gen = probe.gen.is_some_and(|gen| canon.starts_with(&gen));
    if !in_gen {
        return ToolResult::denied("已拒绝绑定该路径：link_file_to_task 只能绑定 AI_Gen_Files 目录内的文件/文件夹；其他文件请在任务卡上手动「绑定文件」");
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
        return ToolResult::denied("link_file_to_task 缺少 taskId / title");
    };
    let kind = match v["kind"].as_str().unwrap_or("final") {
        "intermediate" => crate::bot_artifacts::ArtifactKind::Intermediate,
        _ => crate::bot_artifacts::ArtifactKind::Final,
    };
    // 双分支分流：任务执行流程走「登记 → 完成弹窗勾选 → 落库」；
    // 普通对话没有收尾弹窗时机，直接落库绑定（同样只增不减）
    if crate::tool_guard::is_task_execution_flow(session_id) {
        crate::bot_artifacts::register(app, &key, path.clone(), kind).await;
        let kind_label = match kind {
            crate::bot_artifacts::ArtifactKind::Final => "最终产物",
            crate::bot_artifacts::ArtifactKind::Intermediate => "中间产物",
        };
        return ToolResult::ok(
            format!(
                "已登记{kind_label}「{path}」。流程结束、任务完成时会在通知中心生成待绑定消息让用户勾选；不要在此刻绑定——任务未完成或中断不绑定。"
            ),
            Vec::new(),
        );
    }
    // 普通对话分支：白名单内 + 必须命中恰一张未删除卡（不能无中生有建卡）
    let tasks = match crate::db::db_load(app.clone()).await {
        Ok(t) => t,
        Err(e) => {
            return ToolResult::error(format!("绑定失败：读取任务卡出错（{e}）"), Vec::new());
        }
    };
    let target = match resolve_task_key(&tasks, &key) {
        Ok(t) => t,
        Err(reason) => return ToolResult::denied(reason),
    };
    let is_dir = std::fs::metadata(&canon)
        .map(|m| m.is_dir())
        .unwrap_or(false);
    let mut next = target.clone();
    let type_label = if is_dir { "文件夹" } else { "文件" };
    let (applied, overflow) = crate::db::upsert_files_to_task(
        &mut next,
        vec![crate::db::TaskFile {
            path: path.clone(),
            is_dir,
        }],
    );
    if applied == 0 {
        return ToolResult::denied(format!(
            "该路径已绑定在任务「{}」上，无需重复绑定",
            next.title
        ));
    }
    next.expected_updated_at = next.updated_at;
    next.updated_at = Some(crate::db::now_ms());
    if let Err(e) = crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        return ToolResult::error(format!("绑定落库失败：{e}"), Vec::new());
    }
    crate::bot::broadcast_after_mutation(app, vec![next.clone()], vec![]);
    ToolResult::ok(
        format!(
            "已绑定{type_label}「{path}」到任务「{}」{}",
            next.title,
            if overflow > 0 {
                "（卡上绑定已满，超出上限的部分未生效）"
            } else {
                ""
            }
        ),
        Vec::new(),
    )
}
