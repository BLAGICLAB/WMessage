use crate::error::{CommandError, CommandResult};
use tauri::AppHandle;

/// 消费侧 kind 白名单（可测内核）：仅 {file, folder} 链接目标纳入可打开/删除集合
/// （url 不是路径；app/command/未来 kind 一律不纳入；**集合判断，不是 `!= "url"` 黑名单**）。
///
/// **单一来源（修 OCR C2b2-1）**：集合直接引用写入侧 `db::workspace::ALLOWED_LINK_KINDS`，
/// 不再硬编码字面集——写入侧增删 kind 时消费者自动跟随，消除「注释称同集但无强制」的漂移。
fn link_kind_contributes_path(kind: &str) -> bool {
    // url 在白名单内但不是路径（打开走浏览器）；其余白名单 kind 均贡献路径。
    kind != "url" && crate::db::workspace::ALLOWED_LINK_KINDS.contains(&kind)
}

/// 收集「允许直接打开/删除」的**绑集**路径（canonical form）：
/// 任务卡绑定文件/文件夹（含回收站卡——彻底删除场景需要）+ 工作区链接目标。
/// **注意**：AI_Gen_Files 目录**不在**本集合内——由 `canonical_if_openable` 的 gen_dir
/// 分支单独放行（集合只存绑定/链接的精确路径）。
/// open_file_path / delete_bound_file 共用——这两个命令
/// 前端直达，原先零校验，LLM 输出里的代码块路径 / 前端 XSS 都可驱动其打开或删除任意文件。
///
/// **canonical 化策略（修 OCR C2b #3 high 归一化不一致）**：
/// 入集时统一 `canonicalize`——绑集从此只存 canonical form，path_openable_in 后续比对
/// 也走 canonical 双侧，消除 raw 字符串 alias（`./`/`//`/macOS `/private/var` vs `/var`/
/// 8.3 短名/symlink 不同 alias）命中不一致问题。canonicalize 失败的路径不入集
/// （避免后续比对歧义）。
///
/// **健壮性（修 #5 medium 静默收缩白名单）**：任务卡 DB 与 workspace 读取失败各自独立
/// audit log，不静默丢错。
///
/// **性能（修 OCR C2b M1 + C2c-v4 完整化）**：`open_db` / `load_workspace` /
/// `canonicalize` 全是同步 IO，统一 move 进**一个** blocking 任务（`spawn_blocking_map`），
/// 不再跑在 async runtime 上。
///
/// **失败语义（修 OCR C2c-v2）**：blocking 任务失败 → `Err`（audit + 可辨识内部错误），
/// **不再静默塌成空集** —— 空集会让之后所有合法 open/delete 被误判「未绑定」直到进程重启。
///
/// **返回**：`(绑集 canonical form, gen_dir canonical form)`。gen_dir 在**同一 blocking 任务内**
/// 算好一次（修 OCR C2b1-r3b：原先每次校验重 canonicalize，open/delete 各一次 IPC 算两遍）。
async fn collect_openable_paths(
    app: &AppHandle,
) -> Result<(std::collections::HashSet<String>, Option<String>), CommandError> {
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
                            // 消费侧白名单（纵深防御，OCR C2b #2）：见 link_kind_contributes_path。
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
        let gen_canon = crate::db::gen_dir(&handle)
            .ok()
            .and_then(|p| canonical_string(&p));
        Ok((
            raw.into_iter()
                .filter_map(|p| canonical_string(std::path::Path::new(&p)))
                .collect::<std::collections::HashSet<String>>(),
            gen_canon,
        ))
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
/// 绑集构建 / 查找 / 返回 canonical 三处统一走它，保证规范化一致（OCR C2b #3 + M2）。
fn canonical_string(p: &std::path::Path) -> Option<String> {
    std::fs::canonicalize(p)
        .ok()
        .map(crate::bot_fs::strip_verbatim)
        .map(|pb| pb.to_string_lossy().to_string())
}

/// 二次 race-window 校验（fail-closed）：紧邻副作用前重新 canonicalize + 确认仍在
/// allowlist 内，并返回 canonical 字符串——调用方用它去 open/delete，保证
/// 「操作的路径 = 刚校验的路径」（OCR C2b M2；H1 的 delete 侧同用）。
/// 失败 → audit log + Err，不产生任何副作用。
fn recheck_canonical(
    app: &AppHandle,
    op: &str,
    path: &str,
    set: &std::collections::HashSet<String>,
    gen_dir_canon: Option<&str>,
) -> Result<String, CommandError> {
    match canonical_if_openable(path, set, gen_dir_canon) {
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

/// 若 `path` 允许（canonical 后命中绑集，或落在 gen_dir 内）返回其 canonical 字符串
/// （Windows 剥 `\\?\`）；否则 None。
///
/// 返回 canonical 而非 bool：调用方拿它去做实际 open/delete，保证
/// 「操作的路径 = 刚校验的路径」（修 OCR C2b M2）。
///
/// - **绑集**：canonical 精确命中（集合只存 canonical form）
/// - **gen_dir**：canonical 必须落在 AI_Gen_Files 目录内
///   （`gen_dir_canon` 由调用方一次性算好——不再每次重 canonicalize，见下）
/// - `..` 穿越与软链逃逸解析后再比较（「前端 XSS → 任意文件打开/删除」的关键防线）
/// - 路径不存在 / 产物目录不可用 → None（拒）
///
/// **平台无关**：Unix/Windows 均走 canonicalize（无 inode re-check——与 C1b bot_fs
/// 限制一致：Windows 端 capture_pre_ino=None）。race-window 二次校验由调用方
/// `recheck_canonical` 紧邻副作用实现。
///
/// **`gen_dir_canon` 必须是已 canonicalize 的字符串**（OCR C2c-verify #3 / C2b1-r3b）：
/// 原先内部对 raw gen_dir 每次重算 canonicalize，open/delete 一条 IPC 会算两遍；
/// 现由 `collect_openable_paths` 在同一 blocking 任务里算好一次。
fn canonical_if_openable(
    path: &str,
    set: &std::collections::HashSet<String>,
    gen_dir_canon: Option<&str>,
) -> Option<String> {
    let path_str = canonical_string(std::path::Path::new(path))?;
    if set.contains(&path_str) {
        return Some(path_str);
    }
    let gen = gen_dir_canon?;
    // **分量边界而非字符串前缀**：Path::starts_with 按路径分量比较，
    // `AI_Gen_Files/x` 命中、`AI_Gen_Files-evil/x` 不命中（原 String::starts_with 是 bug，
    // 靠 macOS `/var` vs `/private/var` 偶然通过；本 commit 与测试同时修正）。
    if std::path::Path::new(&path_str).starts_with(gen) {
        return Some(path_str);
    }
    None
}

/// bool 版（绑集精确命中 or gen_dir 内）——保留给现有测试与只需判定不需 canonical 的调用点。
/// 实现委托 `canonical_if_openable`，单一真源。`gen_dir_canon` 必须已 canonical。
fn path_openable_in(
    path: &str,
    set: &std::collections::HashSet<String>,
    gen_dir_canon: Option<&str>,
) -> bool {
    canonical_if_openable(path, set, gen_dir_canon).is_some()
}

/// 打开文件/文件夹（Rust 侧调用 opener 插件）：绕过前端窗口的 opener scope，
/// 挂件窗口内聊天文件按钮点击直接走这里，失败返回可读错误、前端弹错提示（不静默）。
/// 限定任务卡绑定集合 / 工作区链接 / AI_Gen_Files——
///
/// **修 OCR C2b #1 critical TOCTOU**：验证序列 = 入口 check（fail-fast + audit）→
/// **紧邻 open 的二次 `recheck_canonical`**（race-window 内重新 canonicalize）。
///
/// **修 OCR C2b M2**：`opener.open_path` 传二次校验返回的 **canonical** 路径，
/// 不是原始用户串——“打开的就是刚校验的那条路径”；raw 路径里的 symlink 跳板
/// 不会被 open 时二次解析。**如实注明**：canonical **不**闭合“canonical 路径的
/// 分量在校验到 open 之间被换”这一内核层固有窗口（opener API 无 fd/O_NOFOLLOW）；
/// 本次把窗口从“raw 链任意 symlink”收窄到“canonical 路径分量”，是 best-effort 缓解。
///
/// **修 OCR C2b M3**：`gen_dir` 只算一次（原 path_openable 内一次 + 这里一次）。
///
/// **存在性检查提前**（修 original UX bug：OS 英文报错 + silent 吞错）。
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

    // M3 + C2b1-r3b：绑集与 gen_dir canonical 在 collect 内同一 blocking 任务一次算好
    let (set, gen) = collect_openable_paths(&app).await?;

    // 入口 check：fail-fast + audit（真正的安全控制是紧邻 open 的二次校验）
    if !path_openable_in(&path, &set, gen.as_deref()) {
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

    // TOCTOU（#1）：紧邻 open 的二次校验（fail-closed），取回 canonical 用于实际打开（M2）
    let canonical = recheck_canonical(&app, "open_file_path", &path, &set, gen.as_deref())?;

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
/// **修 OCR C2b #6 medium（防御与 open 一致）**：不再 raw `set.contains(&path)`，
/// 与 open_file_path 同走 `path_openable_in`（canonical 双侧）。
///
/// **删除范围 = 绑集，不含 AI_Gen_Files**（OCR C2b-1 r3 high）：与 open 不同，
/// delete 的 `gen_dir` 参数传 **None** —— 只允许删任务卡绑定文件/文件夹，
/// 不把 AI_Gen_Files 纳入删除范围（与原 docstring / denial reason 一致）。
///
/// **修 OCR C2b #4 medium（is_dir 死参数）**：`is_dir` 已在 IPC 表面与前端同步删除
/// （`TodoCard.tsx` 原传 `isDir`）；`delete_bound_file(path)` 只接 path。
///
/// **修 OCR C2b H1（high，注释与实现不符 + 缺防御）**：`trash::delete` 不可逆，
/// 必须在**紧邻调用前**做二次 `recheck_canonical`（fail-closed），并用其返回的
/// canonical 路径去删——“删的就是刚校验的那条”。原注释声称有此二次校验但代码没有，
/// 本次补上真实实现（不再是有断言无代码）。
#[tauri::command]
pub async fn delete_bound_file(app: AppHandle, path: String) -> CommandResult<()> {
    if !std::path::Path::new(&path).exists() {
        return Ok(());
    }
    // 只能删「任务卡绑定文件/文件夹」集合内的路径（前端 XSS 可批量删除用户文件）
    let (set, _gen) = collect_openable_paths(&app).await?;
    // 入口 check：fail-fast + audit。
    // gen_dir 传 None：delete 只针对绑定文件/文件夹，不把 AI_Gen_Files 纳入删除范围
    // （OCR C2b-1 r3 high：此前误传 gen 导致静默扩大删除范围到 AI_Gen_Files，与注释不符）。
    if !path_openable_in(&path, &set, None) {
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
    // H1：不可逆操作 → 紧邻 trash::delete 的二次 re-check（fail-closed，同样 gen_dir=None）
    let canonical = recheck_canonical(&app, "delete_bound_file", &path, &set, None)?;
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
        // 修 OCR C2b #3：绑集以 canonical form 入集，比对走双侧 canonicalize。
        // 需创建真实文件以获得 canonical 路径。
        let tmp = tempfile::tempdir().unwrap();
        let bound = tmp.path().join("a.docx");
        std::fs::write(&bound, b"x").unwrap();
        let bound_canon = std::fs::canonicalize(&bound).unwrap().to_string_lossy().to_string();
        let s = set(&[bound_canon.as_str()]);

        // 精确命中（canonical 后入集、canonical 后查）
        assert!(path_openable_in(bound_canon.as_str(), &s, None), "精确命中放行");
        // 近似路径不命中（集合是精确匹配，不是前缀匹配）
        let wrong = tmp.path().join("a.docx.bak");
        std::fs::write(&wrong, b"x").unwrap();
        assert!(
            !path_openable_in(wrong.to_str().unwrap(), &s, None),
            "近似路径不命中"
        );
        // 不存在路径 → 拒（canonicalize 失败）
        assert!(
            !path_openable_in("/nonexistent/path", &set(&[]), None),
            "不存在路径拒"
        );
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
        // 修 OCR C2b #3：内部 path_openable_in 两侧都 canonicalize 后比较。
        // 测试也必须用 canonical gen，否则 macOS `/var` vs `/private/var` 路径
        // 解析导致 starts_with 误判：旧测试靠 OS 偶然幸运通过，但 `#3` 修复后
        // 这个缺陷被暴露。
        let gen_canon = canonical_string(&gen).unwrap();

        let inside = gen.join("out.docx");
        assert!(path_openable_in(
            inside.to_str().unwrap(),
            &set(&[]),
            Some(gen_canon.as_str())
        ));

        let outside_file = outside.join("evil.txt");
        assert!(
            !path_openable_in(
                outside_file.to_str().unwrap(),
                &set(&[]),
                Some(gen_canon.as_str())
            ),
            "产物目录外的文件不得放行"
        );

        // `..` 穿越：canonical 后落在产物目录外 → 拒
        let traversal = gen.join("../outside/evil.txt");
        assert!(
            !path_openable_in(
                traversal.to_str().unwrap(),
                &set(&[]),
                Some(gen_canon.as_str())
            ),
            "`..` 穿越到产物目录外必须被拒"
        );

        // 不存在的路径 → 拒（canonicalize 失败）
        let missing = gen.join("nope.docx");
        assert!(!path_openable_in(
            missing.to_str().unwrap(),
            &set(&[]),
            Some(gen_canon.as_str())
        ));

        // 前缀相似目录（AI_Gen_Files vs AI_Gen_Files-evil）：不得按字符串前缀误吞。
        // 关键：必须以 canonical `gen_canon` 为前缀 vs canonical `sibling_canon`，
        // 两个都是同根 canonical 后才能靠「分量边界」拒，否则 macOS `/var` vs
        // `/private/var` 的 symlink 解析会把字符串前缀 false 误判为通过。
        let sibling = tmp.path().join("AI_Gen_Files-evil");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("x.docx"), b"x").unwrap();
        let sibling_file_canon = std::fs::canonicalize(&sibling.join("x.docx")).unwrap();
        assert!(
            !path_openable_in(
                sibling_file_canon.to_str().unwrap(),
                &set(&[]),
                Some(gen_canon.as_str())
            ),
            "前缀相似目录不得误判命中（分量比较，不是字符串前缀）"
        );
    }

    /// 修 OCR C2b #3 high（归一化不一致）：原先绑集分支用 raw `set.contains`，
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
        assert!(path_openable_in(real_canon.to_str().unwrap(), &s, None));

        // `./real.docx` alias：canonical 后等价于 real_canon → 命中
        let with_dot = format!("{}/./real.docx", tmp.path().to_string_lossy());
        assert!(
            path_openable_in(&with_dot, &s, None),
            "`./` alias 应与 canonical 一致命中"
        );

        // 软链 alias 指向同一文件 → canonical 解析后等价 → 命中
        let link = tmp.path().join("link.docx");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(
            path_openable_in(link.to_str().unwrap(), &s, None),
            "symlink alias 应与 canonical 一致命中"
        );

        // 不同文件（不是 alias） → 拒
        let other = tmp.path().join("other.docx");
        std::fs::write(&other, b"y").unwrap();
        assert!(
            !path_openable_in(other.to_str().unwrap(), &s, None),
            "不同文件不得命中"
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
        // gen 传 canonical（修 C2b1-L2 测试 footgun：raw gen 在 macOS /var vs
        // /private/var 下是假通过——旧测试靠 OS 偶然）
        let gen_canon = canonical_string(&gen).unwrap();
        assert!(
            !path_openable_in(link.to_str().unwrap(), &set(&[]), Some(gen_canon.as_str())),
            "产物目录内软链指向外部 → 必须拒"
        );
    }

    /// 修 OCR C2b H1：`trash::delete` / `open` 前的二次 re-check 必须能拦住 raced
    /// symlink swap。纯内核模拟：先是合法绑定文件，被换成指向 allowlist 外的 symlink
    /// （模拟 check 与副作用之间的 swap）后，`canonical_if_openable`（= recheck_canonical
    /// 的内核）必须返回 None —— 即「紧邻删除/打开前的二次校验」会拒并 fail-closed。
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
            canonical_if_openable(bound.to_str().unwrap(), &set, None).is_some(),
            "初始合法绑定应放行"
        );

        // race：把 bound 换成指向 allowlist 外的 symlink
        std::fs::remove_file(&bound).unwrap();
        std::os::unix::fs::symlink(&outside, &bound).unwrap();

        // 二次 re-check：canonical 现落到 outside（不在绑集）→ None → fail-closed
        assert!(
            canonical_if_openable(bound.to_str().unwrap(), &set, None).is_none(),
            "raced symlink swap 必须被二次 re-check 拦下（fail-closed）"
        );
    }

    /// 修 OCR C2b M2：`canonical_if_openable` 返回 canonical 字符串，调用方据此
    /// open/delete ——「操作的就是刚校验的那条路径」。
    #[test]
    fn canonical_if_openable_returns_canonical_form() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let canon = canonical_string(&f).unwrap();
        let set = set(&[canon.as_str()]);
        let got = canonical_if_openable(f.to_str().unwrap(), &set, None).expect("应放行");
        assert_eq!(
            got, canon,
            "返回的必须是 canonical 形式（供调用方直接 open/delete）"
        );
    }

    /// OCR C2b-1 r3 high：delete 的允许范围只含绑集，**不含** gen_dir。
    /// open 传 gen → AI_Gen_Files 内文件可打开；delete 传 None → 同一文件不可删（与
    /// docstring / denial reason 一致）。
    #[test]
    fn delete_scope_excludes_gen_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let gen = tmp.path().join("AI_Gen_Files");
        std::fs::create_dir_all(&gen).unwrap();
        std::fs::write(gen.join("out.docx"), b"x").unwrap();
        let file = gen.join("out.docx");
        let p = file.to_str().unwrap();

        // open 侧（传 canonical gen）：允许
        let gen_canon = canonical_string(&gen).unwrap();
        assert!(
            path_openable_in(p, &set(&[]), Some(gen_canon.as_str())),
            "open 侧应允许 gen_dir 内文件"
        );
        // delete 侧（gen_dir=None）：拒绝——即便落在 gen_dir 内也不放行
        assert!(
            !path_openable_in(p, &set(&[]), None),
            "delete 侧（gen_dir=None）不得放行 gen_dir 内文件"
        );
    }

    /// OCR C2b #2 簇B：消费侧是**白名单集合判断**，不是 `!= "url"` 黑名单——
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
