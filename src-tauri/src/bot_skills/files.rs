use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

/// 消费侧 kind 白名单（可测内核）：仅 {file, folder} 链接目标纳入可打开/删除集合
/// （url 不是路径；app/command/未来 kind 一律不纳入；**集合判断，不是 `!= "url"` 黑名单**）。
///
/// **单一来源（修 ）**：集合直接引用写入侧 `db::workspace::ALLOWED_LINK_KINDS`，
/// 不再硬编码字面集——写入侧增删 kind 时消费者自动跟随，消除「注释称同集但无强制」的漂移。
fn link_kind_contributes_path(kind: &str) -> bool {
    // url 在白名单内但不是路径（打开走浏览器）；其余白名单 kind 均贡献路径。
    kind != "url" && crate::db::workspace::ALLOWED_LINK_KINDS.contains(&kind)
}

/// 收集「允许直接删除」的**绑集**路径（canonical form）：
/// 任务卡绑定文件/文件夹（含回收站卡——彻底删除场景需要）+ 工作区链接目标。
/// **只服务 `delete_bound_file`**——删除是破坏性操作，仍限定绑集；
/// `open_file_path` 已按 2026-10-08 口径放开（聊天里的文件/链接点开不再要求
/// 绑定，任意存在的本地路径直接打开），不再查询本集合。
///
/// **canonical 化策略（修  #3 high 归一化不一致）**：
/// 入集时统一 `canonicalize`——绑集从此只存 canonical form，path_openable_in 后续比对
/// 也走 canonical 双侧，消除 raw 字符串 alias（`./`/`//`/macOS `/private/var` vs `/var`/
/// 8.3 短名/symlink 不同 alias）命中不一致问题。canonicalize 失败的路径不入集
/// （避免后续比对歧义）。
///
/// **健壮性（修 #5 medium 静默收缩白名单）**：任务卡 DB 与 workspace 读取失败各自独立
/// audit log，不静默丢错。
///
/// **性能（修  + C2c-v4 完整化）**：`open_db` / `load_workspace` /
/// `canonicalize` 全是同步 IO，统一 move 进**一个** blocking 任务（`spawn_blocking_map`），
/// 不再跑在 async runtime 上。
///
/// **失败语义（修 ）**：blocking 任务失败 → `Err`（audit + 可辨识内部错误），
/// **不再静默塌成空集** —— 空集会让之后所有合法 delete 被误判「未绑定」直到进程重启。
async fn collect_openable_paths(
    app: &AppHandle,
) -> Result<std::collections::HashSet<String>, CommandError> {
    // db_load 已是 async；先只收 raw 路径
    let mut raw: Vec<String> = Vec::new();

    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            for f in t.effective_files() {
                raw.push(f.path);
            }
        }
    } else {
        crate::bot::audit_log(
            app,
            "collect_openable_paths tasks load failed | collecting continue (degraded)",
        );
    }

    // C2c-v4：open_db / load_workspace / canonicalize 同属同步 IO，合并进同一个 blocking 任务。
    // C2c-v5：两处失败 audit 均带 `{e}`，审计可区分 DB 锁 / 迁移失败 / IO 错误。
    let handle = app.clone();
    let set = crate::py::document::spawn_blocking_map(move || {
        let mut raw = raw;
        match crate::db::open_db(&handle) {
            Ok(conn) => match crate::db::load_workspace(&conn) {
                Ok(items) => {
                    for it in items {
                        for l in it.links {
                            // 消费侧白名单（纵深防御， #2）：见 link_kind_contributes_path。
                            if link_kind_contributes_path(&l.kind) {
                                raw.push(l.target_uri);
                            }
                        }
                    }
                }
                Err(e) => crate::bot::audit_log(
                    &handle,
                    &format!(
                        "collect_openable_paths workspace load failed | {e} | collecting continue (degraded)"
                    ),
                ),
            },
            Err(e) => crate::bot::audit_log(
                &handle,
                &format!(
                    "collect_openable_paths workspace db open failed | {e} | collecting continue (degraded)"
                ),
            ),
        }
        Ok(raw
            .into_iter()
            .filter_map(|p| canonical_string(std::path::Path::new(&p)))
            .collect::<std::collections::HashSet<String>>())
    })
    .await
    .map_err(|e| {
        // C2c-v2（功能中断真修）：不再返回空集，改为 audit + 可辨识内部错误（fail-closed）。
        crate::bot::audit_log(
            app,
            &format!("collect_openable_paths blocking task failed | {e}"),
        );
        CommandError::Internal(format!("收集绑定路径失败，已拒绝本次操作：{e}"))
    })?;
    Ok(set)
}

/// canonicalize → 字符串（Windows 剥 `\\?\` 前缀，复用 `bot_fs::strip_verbatim`）。
/// 绑集构建 / 查找 / 返回 canonical 三处统一走它，保证规范化一致（ #3 + M2）。
/// 非 UTF-8 路径返 None（fail-closed）：lossy 替换会把不同路径撞成同一个
/// 集合键，绕过精确匹配白名单。
fn canonical_string(p: &std::path::Path) -> Option<String> {
    std::fs::canonicalize(p)
        .ok()
        .map(crate::bot_fs::strip_verbatim)
        .and_then(|pb| pb.into_os_string().into_string().ok())
}

/// 二次 race-window 校验（fail-closed）：紧邻副作用前重新 canonicalize + 确认仍在
/// allowlist 内，并返回 canonical 字符串——调用方用它去 delete，保证
/// 「操作的路径 = 刚校验的路径」H1 同用）。
/// 失败 → audit log + Err，不产生任何副作用。
fn recheck_canonical(
    app: &AppHandle,
    op: &str,
    path: &str,
    set: &std::collections::HashSet<String>,
) -> Result<String, CommandError> {
    match canonical_if_openable(path, set) {
        Some(c) => Ok(c),
        None => {
            crate::bot::audit_log(
                app,
                &format!(
                    "{op} race-detected | path-replaced-between-check-and-op | {}",
                    crate::audit::escape_for_log(path, 200)
                ),
            );
            Err(CommandError::InvalidArgument {
                field: "path".into(),
                value: path.to_string(),
                reason: "TOCTOU 检测：路径在验证与操作之间被替换，已拒绝".into(),
            })
        }
    }
}

/// 若 `path` 允许删除（canonical 后精确命中绑集）返回其 canonical 字符串
/// （Windows 剥 `\\?\`）；否则 None。
///
/// 返回 canonical 而非 bool：调用方拿它去做实际 delete，保证
/// 「操作的路径 = 刚校验的路径」（修 ）。
///
/// - **绑集**：canonical 精确命中（集合只存 canonical form）
/// - `..` 穿越与软链逃逸解析后再比较（「前端 XSS → 任意文件删除」的关键防线）
/// - 路径不存在 → None（拒）
///
/// **平台无关**：Unix/Windows 均走 canonicalize（无 inode re-check——与 C1b bot_fs
/// 限制一致：Windows 端 capture_pre_ino=None）。race-window 二次校验由调用方
/// `recheck_canonical` 紧邻副作用实现。
fn canonical_if_openable(path: &str, set: &std::collections::HashSet<String>) -> Option<String> {
    let path_str = canonical_string(std::path::Path::new(path))?;
    if set.contains(&path_str) {
        return Some(path_str);
    }
    None
}

/// bool 版（绑集精确命中）——delete 侧入口判定用。
/// 实现委托 `canonical_if_openable`，单一真源。
fn path_openable_in(path: &str, set: &std::collections::HashSet<String>) -> bool {
    canonical_if_openable(path, set).is_some()
}

/// 打开文件/文件夹（Rust 侧调用 opener 插件）：绕过前端窗口的 opener scope，
/// 挂件窗口内聊天文件按钮点击直接走这里，失败返回可读错误、前端弹错提示（不静默）。
///
/// **口径变更（2026-10-08 用户）**：聊天里的文件/链接点开**不再要求绑定**——
/// 原白名单（任务卡绑集 / 工作区链接 / AI_Gen_Files + TOCTOU 二次校验，
/// 系列）已整体移除，任意存在的本地路径直接打开。原防线承担的「前端 XSS → 打开
/// 任意文件」风险随口径一并由用户接受；**删除侧不受影响**——`delete_bound_file`
/// 仍限定绑集（破坏性操作另行把关），共用 helper 收窄为 delete 专用。
#[tauri::command]
pub async fn open_file_path(app: AppHandle, path: String) -> CommandResult<()> {
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

    // canonical 仅作归一（Windows 剥 \\?\ verbatim 前缀 + alias 解析），非安全闸；
    // 失败（含不存在竞态）回退用原路径直接打开
    let canonical = std::fs::canonicalize(std::path::Path::new(&path))
        .map(crate::bot_fs::strip_verbatim)
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| path.clone());

    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(canonical, None::<&str>)
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
/// trash::delete 内部递归处理文件/目录两种类型。
/// 路径不存在视为已删（幂等）。
/// 错误透传给前端（权限不足/路径异常/网络盘不支持）→ 任务行保留，用户可重试
/// （安全优先于不可恢复，误删可从废纸篓/回收站找回）
///
/// ⚠️ 踩坑：不能写 `trash::delete_all(p)`——
///    `&Path` 实现了 `IntoIterator<Item = &OsStr>`（Path::components 迭代器），
///    delete_all 会逐个 component 调 trash，第一个 component `/`（根）的 parent() 是 None → TargetedRoot。
///    正确写法是单数 `trash::delete(p)`（内部 `delete_all(&[path])`，把整条路径当作一项处理）。
///
/// **修  #6 medium（防御与 open 一致）**：不再 raw `set.contains(&path)`，
/// 与 open_file_path 同走 `path_openable_in`（canonical 双侧）。
///
/// **删除范围 = 绑集**（ high）：与已放开的 open 不同（2026-10-08 口径），
/// 删除是破坏性操作，仍只允许删任务卡绑定文件/文件夹——**不**把 AI_Gen_Files
/// 纳入删除范围（历史上曾误传 gen_dir 静默扩大删除范围，与注释不符，已修）。
///
/// **修  #4 medium（is_dir 死参数）**：`is_dir` 已在 IPC 表面与前端同步删除
/// （`TodoCard.tsx` 原传 `isDir`）；`delete_bound_file(path)` 只接 path。
///
/// **修  H1（high，注释与实现不符 + 缺防御）**：`trash::delete` 不可逆，
/// 必须在**紧邻调用前**做二次 `recheck_canonical`（fail-closed），并用其返回的
/// canonical 路径去删——“删的就是刚校验的那条”。原注释声称有此二次校验但代码没有，
/// 本次补上真实实现（不再是有断言无代码）。
#[tauri::command]
pub async fn delete_bound_file(app: AppHandle, path: String) -> CommandResult<()> {
    if !std::path::Path::new(&path).exists() {
        return Ok(());
    }
    // 只能删「任务卡绑定文件/文件夹」集合内的路径（前端 XSS 可批量删除用户文件）
    let set = collect_openable_paths(&app).await?;
    // 入口 check：fail-fast + audit
    if !path_openable_in(&path, &set) {
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
    // H1：不可逆操作 → 紧邻 trash::delete 的二次 re-check（fail-closed）
    let canonical = recheck_canonical(&app, "delete_bound_file", &path, &set)?;
    trash::delete(std::path::Path::new(&canonical)).map_err(|e| {
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

/// 这两个命令（open_file_path / delete_bound_file）中，delete 仍限定绑集
/// （破坏性操作）；open 已按 2026-10-08 口径放开（任意存在的路径直接打开）。
/// 判定核测试锁住「绑集精确命中」的放行与 `..` / 软链逃逸 / 不存在路径的拒绝行为。
#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> std::collections::HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn openable_requires_exact_binding_hit() {
        // 修  #3：绑集以 canonical form 入集，比对走双侧 canonicalize。
        // 需创建真实文件以获得 canonical 路径。
        let tmp = tempfile::tempdir().unwrap();
        let bound = tmp.path().join("a.docx");
        std::fs::write(&bound, b"x").unwrap();
        let bound_canon = std::fs::canonicalize(&bound)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let s = set(&[bound_canon.as_str()]);

        // 精确命中（canonical 后入集、canonical 后查）
        assert!(path_openable_in(bound_canon.as_str(), &s), "精确命中放行");
        // 近似路径不命中（集合是精确匹配，不是前缀匹配）
        let wrong = tmp.path().join("a.docx.bak");
        std::fs::write(&wrong, b"x").unwrap();
        assert!(
            !path_openable_in(wrong.to_str().unwrap(), &s),
            "近似路径不命中"
        );
        // 不存在路径 → 拒（canonicalize 失败）
        assert!(
            !path_openable_in("/nonexistent/path", &set(&[])),
            "不存在路径拒"
        );
        // 未命中集合 → 一律拒
        assert!(!path_openable_in("/anything", &set(&[])));
    }

    /// 修  #3 high（归一化不一致）：原先绑集分支用 raw `set.contains`，
    /// 路径 `./a.docx`、`a.docx/`、`a//b.docx`、macOS `/private/var` vs `/var`、
    /// symlink 不同 alias 命中不一致。现在绑集以 canonical form 入集、比对走
    /// 双侧 canonicalize——同一文件的不同 alias 全部 canonical 到同一形式、命中一致。
    #[cfg(unix)]
    #[test]
    fn openable_normalizes_aliases_via_canonicalize() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.docx");
        std::fs::write(&real, b"x").unwrap();
        let real_canon = std::fs::canonicalize(&real).unwrap();

        // 绑集以 canonical form 入集
        let s = set(&[real_canon.to_str().unwrap()]);

        // raw canonical 命中
        assert!(path_openable_in(real_canon.to_str().unwrap(), &s));

        // `./real.docx` alias：canonical 后等价于 real_canon → 命中
        let with_dot = format!("{}/./real.docx", tmp.path().to_string_lossy());
        assert!(
            path_openable_in(&with_dot, &s),
            "`./` alias 应与 canonical 一致命中"
        );

        // 软链 alias 指向同一文件 → canonical 解析后等价 → 命中
        let link = tmp.path().join("link.docx");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(
            path_openable_in(link.to_str().unwrap(), &s),
            "symlink alias 应与 canonical 一致命中"
        );

        // 不同文件（不是 alias） → 拒
        let other = tmp.path().join("other.docx");
        std::fs::write(&other, b"y").unwrap();
        assert!(
            !path_openable_in(other.to_str().unwrap(), &s),
            "不同文件不得命中"
        );
    }

    /// 修 ：`canonical_if_openable` 返回 canonical 字符串，调用方据此
    /// open/delete ——「操作的就是刚校验的那条路径」。
    #[test]
    fn canonical_if_openable_returns_canonical_form() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let canon = canonical_string(&f).unwrap();
        let set = set(&[canon.as_str()]);
        let got = canonical_if_openable(f.to_str().unwrap(), &set).expect("应放行");
        assert_eq!(
            got, canon,
            "返回的必须是 canonical 形式（供调用方直接 open/delete）"
        );
    }

    ///  high 的口径延续：删除范围只含绑集。open 已放开（2026-10-08），
    /// 但 delete 侧未命中绑集（即便落在 AI_Gen_Files 内）也不放行。
    #[test]
    fn delete_scope_requires_binding_hit() {
        let tmp = tempfile::tempdir().unwrap();
        let gen = tmp.path().join("AI_Gen_Files");
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::write(gen.join("out.docx"), b"x").unwrap();
        let p = gen.join("out.docx").to_str().unwrap().to_string();

        // delete 侧（绑集为空）：未命中 → 拒——即便落在 AI_Gen_Files 内也不放行
        assert!(
            !path_openable_in(&p, &set(&[])),
            "delete 侧不得放行未绑定文件"
        );
    }

    /// 修 ：`trash::delete` 前的二次 re-check 必须能拦住 raced symlink
    /// swap。纯内核模拟：先是合法绑定文件，被换成指向绑集外的 symlink（模拟 check
    /// 与副作用之间的 swap）后，`canonical_if_openable`（= recheck_canonical 的内核）
    /// 必须返回 None —— 即「紧邻删除前的二次校验」会拒并 fail-closed。
    #[cfg(unix)]
    #[test]
    fn recheck_rejects_raced_symlink_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let bound = tmp.path().join("bound.txt");
        std::fs::write(&bound, b"ok").unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"secret").unwrap();

        // 绑集 = bound 的 canonical form（模拟 collect_openable_paths）
        let bound_canon = canonical_string(&bound).unwrap();
        let set = set(&[bound_canon.as_str()]);

        // 首次校验：合法 → Some(canonical)
        assert!(
            canonical_if_openable(bound.to_str().unwrap(), &set).is_some(),
            "初始合法绑定应放行"
        );

        // race：把 bound 换成指向绑集外的 symlink
        std::fs::remove_file(&bound).unwrap();
        std::os::unix::fs::symlink(&outside, &bound).unwrap();

        // 二次 re-check：canonical 现落到 outside（不在绑集）→ None → fail-closed
        assert!(
            canonical_if_openable(bound.to_str().unwrap(), &set).is_none(),
            "raced symlink swap 必须被二次 re-check 拦下（fail-closed）"
        );
    }

    ///  #2 簇B：消费侧是**白名单集合判断**，不是 `!= "url"` 黑名单——
    /// 未来 kind（"future_foo"）必须被拒（黑名单实现会放行它）。
    #[test]
    fn consumer_link_kind_is_whitelist_not_blacklist() {
        assert!(link_kind_contributes_path("file"));
        assert!(link_kind_contributes_path("folder"));
        assert!(!link_kind_contributes_path("url"), "url 不是路径");
        assert!(!link_kind_contributes_path("app"));
        assert!(!link_kind_contributes_path("command"));
        // 关键：未来 kind 必须被拒（若实现是 `!= "url"` 黑名单，这里会是 true）
        assert!(
            !link_kind_contributes_path("future_foo"),
            "未来 kind 必须被白名单拒（不是 != url 黑名单）"
        );
    }
}
