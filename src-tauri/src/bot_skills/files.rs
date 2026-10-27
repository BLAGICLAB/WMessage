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
/// `open_file_path` 已放开（聊天里的文件/链接点开不再要求
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
/// 聊天文件按钮点击直接走这里，失败返回可读错误、前端弹错提示（不静默）。
///
/// 聊天里的文件/链接点开**不再要求绑定**——
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

/// 交付物路径探测（聊天交付物区用）：批量判定路径是否存在、是不是目录、多大。
///
/// 为什么需要独立命令而不是让前端自己判：存在性判定是**同步 syscall**
/// （metadata），跑在 async runtime 线程上会阻塞全部 Tauri command 与
/// event——一批路径几十次 metadata，足够让界面卡一下。整体包 blocking 池。
///
/// 不存在的路径**不是错误**：交付物区要据此把它从清单里剔掉（挂一个打不开
/// 的条目正是要治的毛病），所以结果是「每条路径一份判定」，命令本身只在
/// 参数越界时失败。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathProbe {
    /// 原样回显（前端按这个去打开）
    pub path: String,
    /// 磁盘上是否存在
    pub exists: bool,
    /// 是目录（不是文件）
    pub is_dir: bool,
    /// 文件字节数（目录为 0）
    pub size: u64,
}

/// 单次探测的路径条数上限：防把成千上万条路径塞进一次 IPC
const PROBE_PATHS_MAX: usize = 500;

/// 打开产物目录（AI_Gen_Files）并在系统文件管理器里显示它。
///
/// 返回解析后的目录路径：前端要拿它做 tooltip（让用户知道产物落在哪），
/// 不能让前端自己拼——便携模式下数据目录跟着 exe 走，前端拼不出来。
///
/// 目录解析即建（`db::gen_dir` 的既有语义）：产物目录是机器人写盘的落点，
/// 启动时就预建过；这里再解析一次只是拿路径。
#[tauri::command]
pub async fn bot_open_gen_dir(app: AppHandle) -> CommandResult<String> {
    let dir = crate::db::gen_dir(&app)
        .map_err(|e| CommandError::IoError(format!("定位产物目录失败：{e}")))?;
    let path = dir.to_string_lossy().into_owned();
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.clone(), None::<&str>)
        .map_err(|e| CommandError::IoError(format!("打开产物目录失败：{e}")))?;
    Ok(path)
}

#[tauri::command]
pub async fn bot_probe_paths(paths: Vec<String>) -> CommandResult<Vec<PathProbe>> {
    if paths.len() > PROBE_PATHS_MAX {
        return Err(CommandError::InvalidArgument {
            field: "paths".to_string(),
            value: paths.len().to_string(),
            reason: format!("一次最多探测 {PROBE_PATHS_MAX} 条路径"),
        });
    }
    tauri::async_runtime::spawn_blocking(move || {
        paths.into_iter().map(probe_one).collect::<Vec<PathProbe>>()
    })
    .await
    .map_err(|e| CommandError::Internal(format!("路径探测线程 join 失败：{e}")))
}

/// 单条路径判定（同步内核，跑在 blocking 线程里）。
///
/// metadata 跟随符号链接：软链指向的文件算「存在且是文件」——用户点开软链
/// 要的是它指向的东西，不是链接本身。软链断了就当不存在。
fn probe_one(path: String) -> PathProbe {
    let meta = std::fs::metadata(std::path::Path::new(&path)).ok();
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
    PathProbe {
        exists: meta.is_some(),
        is_dir,
        size: meta
            .as_ref()
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .unwrap_or(0),
        path,
    }
}

/// 文本预览结果（右侧栏「文件」工作面点开文件后就地渲染）。
///
/// **分两态而不是「成功 / 失败」**：二进制、文件夹这类目标**读得到、但渲染不了**
/// ——命令本身是成功的，只是「不能作为文本渲染」。塞进错误通道的话前端只能
/// 靠错误文案认它（「消息里有没有『二进制』三个字」），文案一改就失效；
/// 而错误消息本身又是给人看的（「参数校验失败：path=…（…）」），不适合当机器信号。
/// 独立成态后前端拿到的是稳定的 kind 标签 + 一句干净的原因。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TextPreviewResult {
    /// 可作为文本渲染
    Text(TextPreview),
    /// 读到了但不能作为文本渲染（二进制 / 文件夹）：可交给系统默认程序打开
    Unsupported { path: String, reason: String },
}

/// 文本预览载荷（右侧栏「文件」工作面点开文件后就地渲染）。
///
/// **为什么需要它**：以前文件行点不开，只能跳到系统默认程序——在聊天里
/// 改完一批文件想确认内容，得一个个切出去又切回来，对话上下文整个断掉。
/// 预览留在侧栏里，对话不动。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPreview {
    /// 回显路径（前端按它判 markdown / 纯文本）
    pub path: String,
    /// 文本内容（已按上限截断，按 UTF-8 字符边界切）
    pub content: String,
    /// 是否因超限被截断（前端显式提示，不让人以为这就是全文）
    pub truncated: bool,
    /// 原文件字节数（截断提示里说清「只显示了前 N / 共 M 字节」）
    pub total_bytes: u64,
    /// 内容里的行数（按 \n 计，截断后的）
    pub lines: usize,
}

/// 预览读取上限：够读文档与 markdown，又不至于把 webview 撑住。
/// 超出只截断不报错——用户要的是「看一眼」，不是全文导出。
pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

/// 二进制探测：前 8KB 里出现 NUL 即判二进制。
///
/// **按内容判，不按扩展名判**：`.txt` 可能是二进制、`.bin` 可能是文本，
/// 扩展名白名单会两头都判错。UTF-16/UTF-32 文本恰好都含 NUL，会被误判成
/// 二进制——那是可接受的保守方向（回一句「这是二进制文件」并给出外部打开
/// 入口，好过吐一屏乱码；系统默认程序读 UTF-16 文本本来就没问题）。
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 文本预览内核（纯函数，可单测）：读文件、按上限截断、判二进制。
///
/// **读得到但渲染不了**（二进制 / 文件夹）回 `Unsupported` 而不是错误：
/// 命令成功了，只是没有可渲染的文本。真正的读不到（路径不存在、权限）
/// 才回 Err，且只回可读原因，前端弹错不静默——「点了没反应」和
/// 「显示为空」都会让人以为文件是空的。
pub fn read_text_preview(path: &str) -> CommandResult<TextPreviewResult> {
    let p = std::path::Path::new(path);
    let meta = std::fs::metadata(p)
        .map_err(|e| CommandError::IoError(format!("读取文件信息失败：{e}")))?;
    if meta.is_dir() {
        return Ok(TextPreviewResult::Unsupported {
            path: path.to_string(),
            reason: "这是文件夹，无法在栏内预览内容".into(),
        });
    }
    // 多读 1 字节判溢出，避免为了知道「超没超限」把整个大文件读进内存
    let mut buf = Vec::with_capacity(PREVIEW_MAX_BYTES.min(meta.len() as usize + 1));
    let mut f =
        std::fs::File::open(p).map_err(|e| CommandError::IoError(format!("打开文件失败：{e}")))?;
    {
        use std::io::Read;
        // take(上限+1)：超限只多读一个字节即可判定
        let mut limited = f.by_ref().take(PREVIEW_MAX_BYTES as u64 + 1);
        limited
            .read_to_end(&mut buf)
            .map_err(|e| CommandError::IoError(format!("读取文件失败：{e}")))?;
    }
    let truncated = buf.len() > PREVIEW_MAX_BYTES;
    if truncated {
        buf.truncate(PREVIEW_MAX_BYTES);
    }
    if buf.iter().take(BINARY_SNIFF_BYTES).any(|&b| b == 0u8) {
        return Ok(TextPreviewResult::Unsupported {
            path: path.to_string(),
            reason: "这是二进制文件（图片、PDF、Office 文档等），无法在栏内预览内容".into(),
        });
    }
    // 非法 UTF-8（截断处切在多字节字符中间也会这样）→ 丢尾部残字节，
    // 宁可少显示一个字符，也别让整个预览渲染失败
    let content = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            // 截在多字节字符中间时 valid_up_to 可能正好是原长（后续全是残字节）
            let upto = valid.min(e.as_bytes().len());
            String::from_utf8_lossy(&e.into_bytes()[..upto]).into_owned()
        }
    };
    Ok(TextPreviewResult::Text(TextPreview {
        path: path.to_string(),
        lines: content.lines().count(),
        content,
        truncated,
        total_bytes: meta.len(),
    }))
}

/// 文本预览命令（右侧栏文件工作面）：同步 IO，挪进 blocking 线程池，
/// 不占 async runtime worker（与 doc_* 同模式）。
#[tauri::command]
pub async fn bot_preview_text(path: String) -> CommandResult<TextPreviewResult> {
    tauri::async_runtime::spawn_blocking(move || read_text_preview(&path))
        .await
        .map_err(|e| CommandError::from(format!("预览线程 join 失败：{e}")))?
}

/// 文件多选对话框（Rust 侧 spawn_blocking 弹框）：聊天 ➕ 添加附件用。
/// 此前前端 dialog.open 在 webview 里可能不弹框，与 doc_extract 弹框同方案修复。
#[tauri::command]
/// 任务卡绑定的**统一**选文件通道（Rust 原生对话框）：卡片 / 详情面板 /
/// 聊天上下文栏三处绑定曾各走各的（webview dialog.open vs 这里），现只此
/// 一条——与合并/提示/落库段同侧，不隔 IPC。
/// `directory=true` 单选文件夹（每卡一个文件夹槽），否则多选文件。
pub async fn pick_files_dialog(
    app: AppHandle,
    directory: Option<bool>,
) -> CommandResult<Vec<String>> {
    let directory = directory.unwrap_or(false);
    let handle = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let builder = handle.dialog().file();
        if directory {
            // 单选文件夹归一成 vec，与多选分支同一返回形状
            builder.blocking_pick_folder().map(|p| vec![p])
        } else {
            builder.blocking_pick_files()
        }
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
/// **删除范围 = 绑集**（ high）：与已放开的 open 不同，
/// 删除是破坏性操作，仍只允许删任务卡绑定文件/文件夹——**不**把 AI_Gen_Files
/// 纳入删除范围（历史上曾误传 gen_dir 静默扩大删除范围，与注释不符，已修）。
///
/// **修  #4 medium（is_dir 死参数）**：`is_dir` 已在 IPC 表面与前端同步删除
/// （`TodoCard.tsx` 原传 `isDir`）；`delete_bound_file` 不接 isDir。
/// 归属闸：`task_id` 传了就判「这条路径是不是本卡的、还有没有别人绑着」
/// （回收站三选项删除按卡传 id）；不传则退化为纯绑集判定。
///
/// **修  H1（high，注释与实现不符 + 缺防御）**：`trash::delete` 不可逆，
/// 必须在**紧邻调用前**做二次 `recheck_canonical`（fail-closed），并用其返回的
/// canonical 路径去删——“删的就是刚校验的那条”。原注释声称有此二次校验但代码没有，
/// 本次补上真实实现（不再是有断言无代码）。
/// 这条路径除了本卡，还被谁绑着——返回人类可读的持有者名单（卡片标题 /
/// 工作区条目名）。空 = 只此一卡绑定，可安全删。
/// 删除是破坏性的：同一条路径可以同时被两张卡（或一张卡 + 一个工作区链接）
/// 引用，绑集是全局的所以单看白名单无法区分「谁拥有它」，删 A 卡的文件会把
/// B 卡的引用一起变成死链。
async fn other_holders(
    app: &AppHandle,
    path: &str,
    owner_task_id: &str,
) -> Result<Vec<String>, CommandError> {
    let canon = canonical_string(std::path::Path::new(path)).ok_or_else(|| {
        CommandError::InvalidArgument {
            field: "path".into(),
            value: path.to_string(),
            reason: "路径不存在或无法规范化".into(),
        }
    })?;
    // 读不到卡表 = 判不出归属，必须 fail-closed：放行等于把「无法确认独占」
    // 当成「确认独占」，正好毁掉这道闸的意义
    let tasks = crate::db::db_load(app.clone())
        .await
        .map_err(|e| CommandError::Internal(format!("检查文件占用失败：{e}")))?;
    let mut holders: Vec<String> = Vec::new();
    for t in tasks {
        if t.id == owner_task_id || t.deleted_at.is_some() {
            continue;
        }
        if t.effective_files()
            .iter()
            .any(|f| canonical_string(std::path::Path::new(&f.path)).as_deref() == Some(&canon))
        {
            holders.push(format!("任务「{}」", t.title));
        }
    }
    let handle = app.clone();
    let canon_for_ws = canon.clone();
    let ws: Vec<String> = crate::py::document::spawn_blocking_map(move || {
        // 同样 fail-closed：读不到工作区就判不出「有没有人在用这个路径」
        let items =
            crate::db::open_db(&handle).and_then(|conn| crate::db::load_workspace(&conn))?;
        Ok(items
            .into_iter()
            .filter(|it| {
                it.links.iter().any(|l| {
                    link_kind_contributes_path(&l.kind)
                        && canonical_string(std::path::Path::new(&l.target_uri)).as_deref()
                            == Some(canon_for_ws.as_str())
                })
            })
            .map(|it| format!("工作区「{}」", it.title))
            .collect::<Vec<_>>())
    })
    .await
    .map_err(|e| CommandError::Internal(format!("检查文件占用失败：{e}")))?;
    holders.extend(ws);
    Ok(holders)
}

/// **归属闸**：前端「永久删除并删文件」按卡逐个调用本命令，带 `taskId` 才判得
/// 出归属——路径仍被别的卡/工作区引用就拒绝并指名持有者（绑集是全局的，只看
/// 白名单会把别人的引用一起变成死链）。不传 `taskId` 时退化为旧口径（绑集
/// 命中即放行）。
#[tauri::command]
pub async fn delete_bound_file(
    app: AppHandle,
    path: String,
    task_id: Option<String>,
) -> CommandResult<()> {
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
    if let Some(tid) = task_id.as_deref().filter(|s| !s.is_empty()) {
        let canon = canonical_string(std::path::Path::new(&path)).ok_or_else(|| {
            CommandError::InvalidArgument {
                field: "path".into(),
                value: path.clone(),
                reason: "路径不存在或无法规范化".into(),
            }
        })?;
        let holders = other_holders(&app, &path, tid).await?;
        if !holders.is_empty() {
            crate::bot::audit_log(
                &app,
                &format!(
                    "delete_bound_file shared_refused | task: {tid} | held_by: {}",
                    crate::audit::escape_for_log(&holders.join(", "), 200)
                ),
            );
            return Err(CommandError::InvalidArgument {
                field: "path".into(),
                value: canon,
                reason: format!(
                    "该文件仍被 {} 引用，请先在那些位置解除绑定",
                    holders.join("、")
                ),
            });
        }
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
/// （破坏性操作）；open 已放开（任意存在的路径直接打开）。
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

    ///  high 的口径延续：删除范围只含绑集。
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

#[cfg(test)]
mod preview_tests {
    use super::{read_text_preview, TextPreviewResult, PREVIEW_MAX_BYTES};

    /// 测试临时目录：唯一 tmp 子目录，跑完即删（不留垃圾在系统 temp 里）
    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wmessage-preview-test-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建测试临时目录");
        d
    }

    fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, bytes).expect("写测试文件");
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn reads_text_and_counts_lines() {
        let dir = tmp_dir("text");
        let path = write(&dir, "a.txt", b"line1\nline2\nline3");
        let p = read_text_preview(&path).expect("文本文件应可预览");
        let super::TextPreviewResult::Text(p) = p else {
            panic!("文本文件应走 Text 分支");
        };
        assert_eq!(p.content, "line1\nline2\nline3");
        assert_eq!(p.lines, 3);
        assert_eq!(p.total_bytes, 17);
        assert!(!p.truncated, "小文件不该报截断");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_goes_through_unchanged() {
        // markdown 不做转换：转的是渲染层的事，这里只保证字节不丢
        let dir = tmp_dir("md");
        let md = b"# Title\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let path = write(&dir, "a.md", md);
        let p = read_text_preview(&path).expect("md 应可预览");
        let super::TextPreviewResult::Text(p) = p else {
            panic!("md 应走 Text 分支");
        };
        assert_eq!(p.content.as_bytes(), md);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_reports_unsupported_with_readable_reason() {
        // 按内容判：NUL 出现即二进制。回「不可预览」而不是空内容——
        // 空内容会被当成「文件是空的」，用户白点一次；
        // 也不当成错误：读到了、只是渲染不了，命令本身是成功的
        let dir = tmp_dir("bin");
        let path = write(&dir, "a.txt", b"PK\x03\x04\x00\x01\x02binary");
        let r = read_text_preview(&path).expect("二进制应给出结果而非报错");
        let TextPreviewResult::Unsupported { reason, .. } = r else {
            panic!("二进制应走 Unsupported 分支");
        };
        assert!(reason.contains("二进制"), "原因要说清是二进制：{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_path_reports_error_and_directory_reports_unsupported() {
        let dir = tmp_dir("missing");
        let err =
            read_text_preview(&dir.join("nope.txt").to_string_lossy()).expect_err("不存在应报错");
        assert!(
            err.message().contains("读取文件信息失败"),
            "{}",
            err.message()
        );
        // 目录不报「读失败」而要说清是文件夹：用户点文件夹只想看内容
        let r = read_text_preview(&dir.to_string_lossy()).expect("目录应给出结果而非报错");
        let TextPreviewResult::Unsupported { reason, .. } = r else {
            panic!("目录应走 Unsupported 分支");
        };
        assert!(reason.contains("文件夹"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn result_serializes_with_kind_tag() {
        // 前端靠 kind 分流：text 走渲染，unsupported 走「用系统默认程序打开」。
        // 标签名是线协议的一部分，改名等于让前端两边对不上
        let dir = tmp_dir("serde");
        let text = write(&dir, "a.txt", b"hello");
        let json = serde_json::to_value(read_text_preview(&text).expect("文本应可预览"))
            .expect("应能序列化");
        assert_eq!(json["kind"], "text");
        assert_eq!(json["content"], "hello");
        assert_eq!(json["truncated"], false);
        let bin = write(&dir, "b.bin", b"PK\x03\x04\x00\x01");
        let json = serde_json::to_value(read_text_preview(&bin).expect("二进制应给出结果"))
            .expect("应能序列化");
        assert_eq!(json["kind"], "unsupported");
        assert!(
            json["reason"].as_str().is_some_and(|s| !s.is_empty()),
            "unsupported 必须带原因：{json}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversize_file_is_truncated_not_rejected() {
        // 超出上限只截断：用户要的是「看一眼」，报错等于不给看。
        // 断言截断后仍是合法 UTF-8——按字节切会切在多字节字符中间
        let dir = tmp_dir("big");
        let mut body = vec![b'a'; PREVIEW_MAX_BYTES + 4096];
        // 尾部塞一个三字节中文字符，保证截断边界会切到它
        body.extend_from_slice("报告".as_bytes());
        let path = write(&dir, "big.txt", &body);
        let p = read_text_preview(&path).expect("超大文件应截断而非报错");
        let super::TextPreviewResult::Text(p) = p else {
            panic!("超大文本应走 Text 分支");
        };
        assert!(p.truncated, "超限必须标截断");
        assert_eq!(p.content.len(), PREVIEW_MAX_BYTES, "截断后应正好等于上限");
        assert!(
            p.total_bytes > PREVIEW_MAX_BYTES as u64,
            "原文件大小要如实报出，供前端提示「只显示了前 N」"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod probe_tests {
    use super::probe_one;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("wmessage-probe-test-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建测试临时目录");
        d
    }

    #[test]
    fn probe_reports_existing_file_with_size() {
        let dir = tmp_dir("file");
        let p = dir.join("a.txt");
        std::fs::write(&p, b"12345").expect("写测试文件");
        let r = probe_one(p.to_string_lossy().into_owned());
        assert!(r.exists, "存在的文件应报 exists");
        assert!(!r.is_dir, "文件不是目录");
        assert_eq!(r.size, 5, "文件字节数要如实报出");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_reports_directory() {
        let dir = tmp_dir("dir");
        let r = probe_one(dir.to_string_lossy().into_owned());
        assert!(r.exists);
        assert!(r.is_dir, "目录要认出来");
        assert_eq!(r.size, 0, "目录没有文件字节数");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_reports_missing_path_without_erroring() {
        let r = probe_one("/definitely/not/here/x.docx".to_string());
        assert!(!r.exists, "不存在的路径不是错误，是一条判定结果");
        assert!(!r.is_dir);
        assert_eq!(r.size, 0);
    }
}
