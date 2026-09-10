use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

/// 收集「允许直接打开/删除」的路径集合（2026-08-27 安全审计 SEC-P1-3）：
/// 任务卡绑定文件/文件夹（含回收站卡——彻底删除场景需要）+ 工作区链接目标 +
/// AI_Gen_Files 目录内文件。open_file_path / delete_bound_file 共用——这两个命令
/// 前端直达，原先零校验，LLM 输出里的代码块路径 / 前端 XSS 都可驱动其打开或删除任意文件。
async fn collect_openable_paths(app: &AppHandle) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            for f in t.effective_files() {
                set.insert(f.path);
            }
        }
    }
    if let Ok(conn) = crate::db::open_db(app) {
        if let Ok(items) = crate::db::load_workspace(&conn) {
            for it in items {
                for l in it.links {
                    if l.kind != "url" {
                        set.insert(l.target_uri);
                    }
                }
            }
        }
    }
    set
}

/// path 是否可打开：绑定集合精确命中，或 AI_Gen_Files 目录内（canonical 双向比较）
fn path_openable(app: &AppHandle, path: &str, set: &std::collections::HashSet<String>) -> bool {
    if set.contains(path) {
        return true;
    }
    let gen = crate::db::data_dir(app).join("AI_Gen_Files");
    if let (Ok(c), Ok(g)) = (std::fs::canonicalize(path), std::fs::canonicalize(&gen)) {
        return c.starts_with(&g);
    }
    false
}

/// 打开文件/文件夹（Rust 侧调用 opener 插件）：绕过前端窗口的 opener scope，
/// 挂件窗口内聊天文件按钮点击直接走这里，失败返回可读错误、前端弹错提示（不静默）。
/// 2026-08-27 SEC-P1-3：限定任务卡绑定集合 / 工作区链接 / AI_Gen_Files——
/// 原先任意路径可打开（.app/.command 即代码执行），是前端 XSS → RCE 的一跳。
#[tauri::command]
pub async fn open_file_path(app: AppHandle, path: String) -> CommandResult<()> {
    let set = collect_openable_paths(&app).await;
    if !path_openable(&app, &path, &set) {
        crate::bot::audit_log(
            &app,
            &format!("open_file_path denied | {}", crate::audit::escape_for_log(&path, 200)),
        );
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "仅允许打开任务卡绑定文件/工作区链接/AI_Gen_Files 内的文件".into(),
        });
    }
    // 2026-09-05：存在性前置检查——原先直接进 opener，文件不存在时回的是 OS 英文
    // 报错（且前端 silent 吞掉，点了没反应）；现在给可读原因，前端弹错不静默
    if !std::path::Path::new(&path).exists() {
        crate::bot::audit_log(
            &app,
            &format!("open_file_path missing | {}", crate::audit::escape_for_log(&path, 200)),
        );
        return Err(CommandError::IoError(format!(
            "路径不存在（可能已被移动或删除）：{path}"
        )));
    }
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| CommandError::IoError(format!("打开路径失败：{e}")))
}

/// 文件多选对话框（Rust 侧 spawn_blocking 弹框）：挂件窗口 ➕ 添加附件用。
/// 此前前端 dialog.open 在挂件窗口可能不弹框，与 doc_extract 弹框同方案修复。
#[tauri::command]
pub async fn pick_files_dialog(app: AppHandle) -> CommandResult<Vec<String>> {
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        handle.dialog().file().blocking_pick_files()
    })
    .await
    .map_err(|e| CommandError::Internal(format!("对话框线程失败：{e}")))?;
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| match p {
            tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
            // Windows WebView2 弹框可能回 Url 变体：一并收下，否则附件被静默吞掉
            //（对齐 bot.rs / bot_py.rs 的 file_path_to_string 双变体实现）
            tauri_plugin_dialog::FilePath::Url(u) => Some(u.to_string()),
        })
        .collect())
}

/// 删除任务卡绑定的本地文件/文件夹（回收站彻底删除时调用）。
/// 使用跨平台 trash crate：macOS → 废纸篓 / Windows → 回收站 / Linux → gio trash。
/// trash::delete 内部递归处理文件/目录两种类型，不需要按 is_dir 分流。
/// 路径不存在视为已删（幂等）。
/// 错误透传给前端（权限不足/路径异常/网络盘不支持）→ 任务行保留，用户可重试
/// （老板 2026-08-17 21:17：安全优先于不可恢复，误删可从废纸篓/回收站找回）
///
/// ⚠️ 踩坑（2026-08-17 21:21 老板报「TargetedRoot」）：不能写 `trash::delete_all(p)`——
///    `&Path` 实现了 `IntoIterator<Item = &OsStr>`（Path::components 迭代器），
///    delete_all 会逐个 component 调 trash，第一个 component `/`（根）的 parent() 是 None → TargetedRoot。
///    正确写法是单数 `trash::delete(p)`（内部 `delete_all(&[path])`，把整条路径当作一项处理）。
#[tauri::command]
pub async fn delete_bound_file(app: AppHandle, path: String, is_dir: bool) -> CommandResult<()> {
    use std::path::Path;
    let p = Path::new(&path);
    if !p.exists() {
        return Ok(());
    }
    // 2026-08-27 SEC-P1-3：只能删「任务卡绑定文件/文件夹」集合内的路径——
    // 原先任意路径可进废纸篓，前端 XSS 可批量删除用户文件
    let set = collect_openable_paths(&app).await;
    if !set.contains(&path) {
        crate::bot::audit_log(
            &app,
            &format!("delete_bound_file denied | {}", crate::audit::escape_for_log(&path, 200)),
        );
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "仅允许删除任务卡绑定的文件/文件夹".into(),
        });
    }
    let _ = is_dir; // trash::delete 内部递归处理两种类型，不再需要分流
    trash::delete(p).map_err(|e| {
        CommandError::IoError(format!(
            "移到{}失败：{e}（文件可能仍在原位置，任务卡保留可重试）",
            if cfg!(target_os = "macos") {
                "废纸篓"
            } else if cfg!(target_os = "windows") {
                "回收站"
            } else {
                "垃圾箱"
            }
        ))
    })
}

