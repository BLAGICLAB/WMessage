use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

/// 收集「允许直接打开/删除」的路径集合：
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
    let gen = crate::db::gen_dir(app).ok();
    path_openable_in(path, set, gen.as_deref())
}

/// 可测内核（注入产物目录，不碰 AppHandle）：
/// - 绑定集合**精确命中**才放行（集合语义是精确路径，不做前缀/近似匹配）
/// - 否则仅当路径 canonical 后落在 AI_Gen_Files 目录内；**两侧都 canonicalize**，
///   所以 `..` 穿越与软链逃逸都会被解析掉再比较——这是「前端 XSS → 任意文件
///   打开/删除」这一跳的关键防线，单测逐条钉住
/// - 路径不存在 / 产物目录不可用 → 一律拒
fn path_openable_in(
    path: &str,
    set: &std::collections::HashSet<String>,
    gen_dir: Option<&std::path::Path>,
) -> bool {
    if set.contains(path) {
        return true;
    }
    let Some(gen) = gen_dir.and_then(|d| std::fs::canonicalize(d).ok()) else {
        return false;
    };
    match std::fs::canonicalize(path) {
        Ok(c) => c.starts_with(&gen),
        Err(_) => false,
    }
}

/// 打开文件/文件夹（Rust 侧调用 opener 插件）：绕过前端窗口的 opener scope，
/// 挂件窗口内聊天文件按钮点击直接走这里，失败返回可读错误、前端弹错提示（不静默）。
/// 限定任务卡绑定集合 / 工作区链接 / AI_Gen_Files——
/// 原先任意路径可打开（.app/.command 即代码执行），是前端 XSS → RCE 的一跳。
#[tauri::command]
pub async fn open_file_path(app: AppHandle, path: String) -> CommandResult<()> {
    let set = collect_openable_paths(&app).await;
    if !path_openable(&app, &path, &set) {
        crate::bot::audit_log(
            &app,
            &format!(
                "open_file_path denied | {}",
                crate::audit::escape_for_log(&path, 200)
            ),
        );
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "仅允许打开任务卡绑定文件/工作区链接/AI_Gen_Files 内的文件".into(),
        });
    }
    // 存在性前置检查——原先直接进 opener，文件不存在时回的是 OS 英文
    // 报错（且前端 silent 吞掉，点了没反应）；现在给可读原因，前端弹错不静默
    if !std::path::Path::new(&path).exists() {
        crate::bot::audit_log(
            &app,
            &format!(
                "open_file_path missing | {}",
                crate::audit::escape_for_log(&path, 200)
            ),
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
/// （安全优先于不可恢复，误删可从废纸篓/回收站找回）
///
/// ⚠️ 踩坑：不能写 `trash::delete_all(p)`——
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
    // 只能删「任务卡绑定文件/文件夹」集合内的路径——
    // 原先任意路径可进废纸篓，前端 XSS 可批量删除用户文件
    let set = collect_openable_paths(&app).await;
    if !set.contains(&path) {
        crate::bot::audit_log(
            &app,
            &format!(
                "delete_bound_file denied | {}",
                crate::audit::escape_for_log(&path, 200)
            ),
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

/// 这两个命令（open_file_path / delete_bound_file）前端直达、原先零校验，
/// LLM 输出里的代码块路径或前端 XSS 可驱动其打开/删除任意文件 ——
/// 判定核必须逐条锁住「集合精确命中」与「产物目录内」两个放行口，
/// 以及 `..` / 软链逃逸与不存在路径的拒绝行为。
#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> std::collections::HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn openable_requires_exact_binding_hit() {
        let s = set(&["/tmp/a.docx"]);
        assert!(path_openable_in("/tmp/a.docx", &s, None), "精确命中放行");
        // 近似路径不命中（集合是精确匹配，不是前缀匹配）
        assert!(!path_openable_in("/tmp/a.docx.bak", &s, None));
        assert!(!path_openable_in("/tmp/", &s, None));
        // 产物目录不可用 + 未命中集合 → 一律拒
        assert!(!path_openable_in("/anything", &set(&[]), None));
    }

    #[test]
    fn openable_allows_only_files_inside_gen_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let gen = tmp.path().join("AI_Gen_Files");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(gen.join("out.docx"), b"x").unwrap();
        std::fs::write(outside.join("evil.txt"), b"x").unwrap();

        let inside = gen.join("out.docx");
        assert!(path_openable_in(
            inside.to_str().unwrap(),
            &set(&[]),
            Some(&gen)
        ));

        let outside_file = outside.join("evil.txt");
        assert!(
            !path_openable_in(outside_file.to_str().unwrap(), &set(&[]), Some(&gen)),
            "产物目录外的文件不得放行"
        );

        // `..` 穿越：canonical 后落在产物目录外 → 拒
        let traversal = gen.join("../outside/evil.txt");
        assert!(
            !path_openable_in(traversal.to_str().unwrap(), &set(&[]), Some(&gen)),
            "`..` 穿越到产物目录外必须被拒"
        );

        // 不存在的路径 → 拒（canonicalize 失败）
        let missing = gen.join("nope.docx");
        assert!(!path_openable_in(
            missing.to_str().unwrap(),
            &set(&[]),
            Some(&gen)
        ));

        // 前缀相似目录（AI_Gen_Files vs AI_Gen_Files-evil）：不得按字符串前缀误吞
        let sibling = tmp.path().join("AI_Gen_Files-evil");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("x.docx"), b"x").unwrap();
        let sibling_file = sibling.join("x.docx");
        assert!(
            !path_openable_in(sibling_file.to_str().unwrap(), &set(&[]), Some(&gen)),
            "前缀相似目录不得误判命中（分量比较，不是字符串前缀）"
        );
    }

    #[cfg(unix)]
    #[test]
    fn openable_rejects_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let gen = tmp.path().join("AI_Gen_Files");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), gen.join("link.txt")).unwrap();

        let link = gen.join("link.txt");
        assert!(
            !path_openable_in(link.to_str().unwrap(), &set(&[]), Some(&gen)),
            "产物目录内软链指向外部 → 必须拒"
        );
    }
}
