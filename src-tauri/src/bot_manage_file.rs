//! manage_file：文件复制 / 移动 / 删除（删除进回收站 + 危险操作确认）。
//!
//! 权限口径与既有工具对齐，不另开一套：
//! - **源**路径走读闸（`bot_fs::resolve_with_perm`）：读白名单 = Desktop /
//!   Downloads / Documents + AI_Gen_Files + 任务卡绑定目录 + 用户配置的 allowedDirs；
//! - **目标**路径走写闸（`bot_fs::resolve_writable`）：可写根 = AI_Gen_Files +
//!   任务卡绑定目录 + 用户配置的 allowedDirs。写闸的白名单外分支弹授权窗。
//! - **delete** 额外走危险操作确认（`bot_slash::ask_user_confirm`，60s 无响应默认拒绝）。
//!
//! 删除进回收站（`trash` crate：macOS 废纸篓 / Windows 回收站 / Linux gio trash）
//! 而不是物理删除——误删可从回收站找回。move 跨卷时源文件同样走回收站，
//! 半途失败（复制成功但源入回收站失败）时源文件仍在原处，不会凭空消失。
//!
//! 目标已存在一律拒绝（不覆盖）：覆盖是破坏性动作，与 write_file 的「覆盖需确认」
//! 是两套语义，这里选更严的一档——让模型自己换个名字，避免它以为覆盖了。

use std::path::Path;

use tauri::AppHandle;

use crate::bot::registry::ToolResult;

/// 递归复制的条目数上限（单棵树）：目录自环时的兜底闸
const MAX_TREE_ENTRIES: usize = 5_000;
/// 递归复制的总字节上限（单棵树）
const MAX_TREE_BYTES: u64 = 1024 * 1024 * 1024;

/// manage_file：文件复制 / 移动 / 删除。
///
/// action:
/// - `copy`：复制 path → dest（dest 必填）
/// - `move`：移动 path → dest（dest 必填）
/// - `delete`：删除 path（进回收站，弹窗确认）
pub async fn tool_manage_file(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let action = v["action"].as_str().unwrap_or("").trim().to_string();
    if action.is_empty() {
        return ToolResult::denied("manage_file 缺少 action 参数（可用 copy / move / delete）");
    }
    let path = v["path"].as_str().unwrap_or("").trim().to_string();
    if path.is_empty() {
        return ToolResult::denied("manage_file 缺少 path 参数");
    }
    match action.as_str() {
        "copy" | "move" => {
            let dest = v["dest"].as_str().unwrap_or("").trim().to_string();
            if dest.is_empty() {
                return ToolResult::denied(format!(
                    "manage_file 的 {action} 需要 dest 参数（目标路径）"
                ));
            }
            copy_or_move(app, &action, &path, &dest, interactive, session_id).await
        }
        "delete" => delete_to_trash(app, &path, interactive, session_id).await,
        other => ToolResult::denied(format!(
            "manage_file 不认识的 action：{other}（可用 copy / move / delete）"
        )),
    }
}

// ── copy / move ────────────────────────────────────────────────────────────

async fn copy_or_move(
    app: &AppHandle,
    action: &str,
    path: &str,
    dest: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    // 源走读闸：源必须在读白名单内且真实存在（canonicalize 顺带解析软链）
    let src =
        match crate::bot_fs::resolve_with_perm(app, "manage_file", path, interactive, session_id)
            .await
        {
            Ok(p) => p,
            Err(e) => return ToolResult::denied(e),
        };
    // 目标走写闸：目标可不存在（新建），校验的是父目录
    let dst =
        match crate::bot_fs::resolve_writable(app, "manage_file", dest, interactive, session_id)
            .await
        {
            Ok(p) => p,
            Err(e) => return ToolResult::denied(e),
        };
    if dst.exists() {
        return ToolResult::denied(format!(
            "目标已存在，未覆盖：{}（换个目标名重试）",
            dst.display()
        ));
    }
    // 目标落在源目录内部 → 拒绝（把目录复制进自己会无限递归）
    if dest_inside_src(&src, &dst) {
        return ToolResult::denied(format!(
            "目标 {} 在源 {} 内部，拒绝（不能把目录复制/移动进它自己）",
            dst.display(),
            src.display()
        ));
    }
    let src_s = src.display().to_string();
    let dst_s = dst.display().to_string();
    let is_move = action == "move";
    // 文件 IO 走 spawn_blocking：同步 syscall 压在 async worker 上会卡住整条工具链
    let (src2, dst2) = (src.clone(), dst.clone());
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        if is_move {
            move_impl(&src2, &dst2)
        } else {
            copy_tree(&src2, &dst2)
                .map(|(entries, bytes)| format!("已复制 {entries} 项 / {bytes} 字节"))
        }
    })
    .await
    .unwrap_or_else(|e| Err(format!("文件操作线程失败：{e}")));

    match outcome {
        Ok(summary) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "manage_file.{action} | {} | {} | {summary}",
                    crate::bot::truncate_for_log(&src_s, 200),
                    crate::bot::truncate_for_log(&dst_s, 200)
                ),
            );
            let verb = if is_move { "移动" } else { "复制" };
            ToolResult::ok(
                format!("已{verb}：{src_s} → {dst_s}（{summary}）。目标路径：{dst_s}"),
                Vec::new(),
            )
            .with_file_change(crate::bot_fs::build_binary_file_receipt(&dst_s))
        }
        Err(e) => ToolResult::error(format!("{action} 失败：{e}"), Vec::new()),
    }
}

/// 移动：同卷 rename（原子）；跨卷复制后把源移入回收站。
///
/// 返回摘要文本（条目数 / 字节数）。跨卷时源进回收站而非物理删除——
/// 回收站失败则报半成状态（源仍在原处），不吞错。
fn move_impl(src: &Path, dst: &Path) -> Result<String, String> {
    match std::fs::rename(src, dst) {
        Ok(()) => {
            let md = std::fs::symlink_metadata(dst).ok();
            let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
            Ok(format!("已移动（同一卷，直接重命名），大小 {size} 字节"))
        }
        Err(e) if is_cross_device(&e) => {
            let (entries, bytes) = copy_tree(src, dst)?;
            // 源入回收站：失败则报半成状态——目标已在，源还在，不算数据丢失
            if let Err(te) = trash::delete(src) {
                return Err(format!(
                    "跨卷移动：已复制到 {}（{entries} 项 / {bytes} 字节），但源文件移入回收站失败：{te}（源文件仍在原处，请手动清理）",
                    dst.display()
                ));
            }
            Ok(format!(
                "已移动（跨卷：复制后源入回收站），{entries} 项 / {bytes} 字节"
            ))
        }
        Err(e) => Err(format!("移动失败：{e}")),
    }
}

/// 递归复制一棵树（文件 + 目录）。符号链接既不跟随也不复制（跳过）。
///
/// 返回 (条目数, 总字节数)。目录自环由调用方的 dest_inside_src 拦掉，
/// MAX_TREE_ENTRIES 是第二道兜底。
fn copy_tree(src: &Path, dst: &Path) -> Result<(usize, u64), String> {
    let mut entries = 0usize;
    let mut bytes = 0u64;
    copy_tree_inner(src, dst, &mut entries, &mut bytes)?;
    Ok((entries, bytes))
}

fn copy_tree_inner(
    src: &Path,
    dst: &Path,
    entries: &mut usize,
    bytes: &mut u64,
) -> Result<(), String> {
    if *entries >= MAX_TREE_ENTRIES {
        return Err(format!("复制条目数超过上限 {MAX_TREE_ENTRIES}，已中止"));
    }
    // symlink_metadata：不跟随软链。软链跳过（不复制、不解引用）。
    let md = std::fs::symlink_metadata(src).map_err(|e| format!("读取源失败：{e}"))?;
    if md.file_type().is_symlink() {
        return Ok(());
    }
    if md.is_dir() {
        std::fs::create_dir_all(dst).map_err(|e| format!("创建目录失败：{e}"))?;
        let rd = std::fs::read_dir(src).map_err(|e| format!("读取目录失败：{e}"))?;
        for entry in rd {
            let entry = entry.map_err(|e| format!("读取目录项失败：{e}"))?;
            copy_tree_inner(&entry.path(), &dst.join(entry.file_name()), entries, bytes)?;
            if *bytes > MAX_TREE_BYTES {
                return Err(format!("复制总字节超过上限 {MAX_TREE_BYTES}，已中止"));
            }
        }
        // 目录权限随源走（create_dir_all 给的是 umask 默认值）
        let _ = std::fs::set_permissions(dst, md.permissions());
        *entries += 1;
    } else {
        let n = std::fs::copy(src, dst).map_err(|e| format!("复制失败：{e}"))?;
        *bytes += n;
        *entries += 1;
    }
    Ok(())
}

// ── delete ─────────────────────────────────────────────────────────────────

async fn delete_to_trash(
    app: &AppHandle,
    path: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    // 删除是写操作：走写闸（目标可写根 + 白名单外弹授权窗）
    let target =
        match crate::bot_fs::resolve_writable(app, "manage_file", path, interactive, session_id)
            .await
        {
            Ok(p) => p,
            Err(e) => return ToolResult::denied(e),
        };
    let shown = target.display().to_string();
    // 破坏性动作：弹窗确认后才执行（60s 无响应默认拒绝）
    let approved = crate::bot_slash::ask_user_confirm(
        app,
        "manage_file",
        &format!("删除：{shown}"),
        interactive,
        session_id,
    )
    .await;
    if !approved {
        return ToolResult::denied("用户拒绝了删除，文件未删除");
    }
    let target2 = target.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        trash::delete(&target2).map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|e| Err(format!("删除线程失败：{e}")));
    match res {
        Ok(()) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "manage_file.delete | {}",
                    crate::bot::truncate_for_log(&shown, 200)
                ),
            );
            // macOS 叫「废纸篓」，其余平台叫「回收站」——平台差异由 trash crate
            // 内部处理，本分支只管文案
            let where_ = if cfg!(target_os = "macos") {
                "废纸篓"
            } else {
                "回收站"
            };
            ToolResult::ok(
                format!("已删除：{shown}（已移入{where_}，可恢复）"),
                Vec::new(),
            )
        }
        Err(e) => ToolResult::error(format!("删除失败：{e}"), Vec::new()),
    }
}

// ── 纯函数内核（可单测）────────────────────────────────────────────────────

/// 跨卷错误判定：Unix EXDEV(18) / Windows ERROR_NOT_SAME_DEVICE(17)。
/// `ErrorKind::CrossesDevices` 覆盖不到老 toolchain 的原始码，故两条都认。
fn is_cross_device(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::CrossesDevices {
        return true;
    }
    match err.raw_os_error() {
        Some(18) => cfg!(unix),
        Some(17) => cfg!(windows),
        _ => false,
    }
}

/// 目标是否落在源目录内部（防目录复制进自己 → 无限递归）。
///
/// 比的是**目标的父目录**：目标 /a/b 的父是 /a，源是 /a 时判内部。
fn dest_inside_src(src: &Path, dst: &Path) -> bool {
    let dst_parent = dst.parent().filter(|p| !p.as_os_str().is_empty());
    match dst_parent {
        Some(p) => {
            // 父目录 canonicalize 后再比：dst 可能是软链路径，源是 canonical
            let canonical_parent = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            canonical_parent.starts_with(src) || p.starts_with(src)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wmessage-mf-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn copy_tree_copies_files_and_dirs() {
        let root = tmpdir("copy");
        let src = root.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), "A").unwrap();
        std::fs::write(src.join("sub/b.txt"), "B").unwrap();
        let dst = root.join("dst");
        let (entries, bytes) = copy_tree(&src, &dst).unwrap();
        assert_eq!(entries, 4, "src 目录 + sub 目录 + 两个文件");
        assert_eq!(bytes, 2);
        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(dst.join("sub/b.txt")).unwrap(), "B");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn copy_tree_skips_symlinks() {
        let root = tmpdir("symlink");
        let src = root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let outside = root.join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        std::fs::write(src.join("real.txt"), "R").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, src.join("link.txt")).unwrap();
        let dst = root.join("dst");
        let (entries, _) = copy_tree(&src, &dst).unwrap();
        assert!(!dst.join("link.txt").exists(), "软链不得被复制");
        assert!(dst.join("real.txt").exists());
        assert!(entries <= 2, "软链不计入条目");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dest_inside_src_detects_self_nesting() {
        let root = tmpdir("nest");
        let src = root.join("a");
        std::fs::create_dir_all(&src).unwrap();
        assert!(
            dest_inside_src(&src, &src.join("b")),
            "拷进自己内部应判内部"
        );
        assert!(!dest_inside_src(&src, &root.join("b")), "同级不算内部");
        assert!(!dest_inside_src(&src, &root.join("x/y")), "别处不算内部");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn is_cross_device_recognises_exdev() {
        // 构造 EXDEV：跨设备 rename 才能真触发，这里只验判定函数本身
        let err = std::io::Error::from_raw_os_error(18);
        assert!(is_cross_device(&err) == cfg!(unix));
        let other = std::io::Error::from_raw_os_error(2);
        assert!(!is_cross_device(&other));
    }

    /// 接线守卫：manage_file 必须真的接进工具表（改了工具名/接线而忘了登记，
    /// 这里会红）。
    #[test]
    fn manage_file_is_wired_into_registry() {
        let src = include_str!("bot/registry/schemas.rs");
        assert!(
            src.contains("name: \"manage_file\""),
            "manage_file 必须登记进 TOOLS_TABLE"
        );
        assert!(
            src.contains("SCHEMA_MANAGE_FILE"),
            "manage_file 必须有 schema 常量"
        );
    }
}
