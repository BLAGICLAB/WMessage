//! 公共杂项面：时间工具、files 参数解析与安全校验、审计补充 kv、改库广播。

use super::*;

// 时间 + 长期记忆工具

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
pub(super) fn parse_task_files_arg(
    v: &serde_json::Value,
) -> Option<(Vec<crate::db::TaskFile>, bool)> {
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
/// 白名单闸（对齐 link_file_to_task）：文件与文件夹一律要求 canonicalize 后落在
/// AI_Gen_Files 内；is_dir 以真实 metadata 校正（不信模型自报）。被拒条目记审计。
/// 用户亲手绑定走 bind_file 系统弹框，不在此限。
pub(super) async fn sanitize_task_files_arg(
    app: &AppHandle,
    v: &serde_json::Value,
) -> Option<(Vec<crate::db::TaskFile>, bool)> {
    let (files, truncated) = parse_task_files_arg(v)?;
    // gen_dir + canonicalize 逐文件校验整体包 spawn_blocking：同步 syscall 在
    // async runtime（tool_create_task/tool_edit_task）上会阻塞全部 Tauri
    // command / event（ performance）。JoinError → 全丢并记审计
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

/// sanitize 的纯内核（单测可注入 gen 目录）：文件与文件夹一律要求 canonicalize
/// 后落在 gen_canon 内，is_dir 以真实 metadata 校正（不信模型自报——防把文件标成
/// 目录绕过目录语义）；解析失败/stat 失败 → 丢。返回 (保留列表, 丢弃数)。
pub(super) fn sanitize_task_files_in(
    gen_canon: Option<&std::path::Path>,
    files: Vec<crate::db::TaskFile>,
) -> (Vec<crate::db::TaskFile>, usize) {
    let mut out: Vec<crate::db::TaskFile> = Vec::new();
    let mut dropped = 0usize;
    for f in files {
        let canon = std::fs::canonicalize(&f.path).ok();
        let in_gen = canon
            .as_deref()
            .zip(gen_canon)
            .is_some_and(|(c, g)| c.starts_with(g));
        let real_is_dir = canon
            .as_deref()
            .and_then(|c| std::fs::metadata(c).ok())
            .map(|m| m.is_dir());
        match (in_gen, real_is_dir) {
            (true, Some(is_dir)) => out.push(crate::db::TaskFile {
                path: f.path,
                is_dir,
            }),
            _ => dropped += 1,
        }
    }
    (out, dropped)
}

/// tool.return 审计补充 kv（create_task/edit_task 带 files 时）：(原始条数, 是否截断)
pub(crate) fn files_audit_kv(args: &str) -> Option<(usize, bool)> {
    let v = parse_args(args);
    let arr = v["files"].as_array()?;
    Some((arr.len(), arr.len() > crate::db::MAX_TASK_FILES))
}

/// 改库后广播：各页重读（tasks-changed）+ App 合并 UI 不回写（tasks-updated, source: Bot）
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

pub(super) async fn active_tasks(app: &AppHandle) -> Result<Vec<crate::db::Task>, String> {
    let tasks = crate::db::db_load(app.clone()).await?;
    Ok(tasks
        .into_iter()
        .filter(|t| {
            t.deleted_at.is_none() && t.archived != Some(true) && t.column != TaskStatus::Done
        })
        .collect())
}
