use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

/// 打开文件/文件夹（Rust 侧调用 opener 插件）：绕过前端窗口的 opener scope，
/// 挂件窗口内聊天文件按钮点击直接走这里，失败返回错误给前端兜底 revealItemInDir
#[tauri::command]
pub fn open_file_path(app: AppHandle, path: String) -> CommandResult<()> {
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
            _ => None,
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
pub fn delete_bound_file(path: String, is_dir: bool) -> CommandResult<()> {
    use std::path::Path;
    let p = Path::new(&path);
    if !p.exists() {
        return Ok(());
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

