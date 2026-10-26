//! 文档面：提取文档、Word/Excel/PPT/PDF 生成、联网搜索、抓页、自由 Python。

use super::*;

// 文档 / Python 工具（bot_py 桥接）

/// 提取文档文本：path 给定则直读（任务卡绑定文件），否则弹框选文件；返回路径 + 文本供模型阅读/润色
/// extract_document path 白名单：任务卡绑定文件 / AI_Gen_Files 目录内文件静默放行。
/// 规范化路径比较，防 ../ 绕过。无 path 时走弹框（用户亲手选，不受此限）。
/// 授权分流：其余路径不硬拒，走 bot_fs::resolve_with_perm 分流
///（strict 硬拒 / ask 弹授权窗 / yolo 放行），拒绝文案透传给模型。
/// interactive/session_id 透传给授权弹窗：后台执行（interactive=false）不弹窗直接拒。
/// 路径校验的同步内核（**非 async**）：存在性 + 真实落点 + gen 目录真实落点。
///
/// 单独成函数有两个理由：
/// · 同步 syscall 跑在 async runtime 线程上会阻塞全部 Tauri command /
///   event（ performance）——调用方整体包 spawn_blocking_map；
/// · 源码守卫据此断言两个 async 工具函数体内没有裸的同步 syscall。
pub(super) struct PathProbe {
    pub(super) exists: bool,
    pub(super) canon: Option<std::path::PathBuf>,
    pub(super) gen: Option<std::path::PathBuf>,
}

pub(super) fn probe_paths_sync(path: &str, app: &AppHandle) -> PathProbe {
    PathProbe {
        exists: std::path::Path::new(path).exists(),
        canon: std::fs::canonicalize(path).ok(),
        gen: crate::db::gen_dir(app)
            .ok()
            .and_then(|d| std::fs::canonicalize(d).ok()),
    }
}

/// 任务卡绑定文件里有没有与 canon 同一落点的（同步内核，非 async）。
///
/// 单独成函数同上：调用方整体包 spawn_blocking_map——任务多、绑定文件多时
/// 是几十上百次 syscall，跑在 runtime 线程上会卡住整个 app 的事件循环。
fn any_bound_path_matches(bound: &[String], canon: &std::path::Path) -> bool {
    bound
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .any(|fc| fc == canon)
}

async fn extract_path_check(
    app: &AppHandle,
    path: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> Result<(), String> {
    // 同步 syscall（canonicalize / gen_dir）整体包 spawn_blocking：跑在 async
    // runtime 线程上会阻塞全部 Tauri command / event（同文件
    // sanitize_task_files_arg 的既有范式， performance）。
    // 线程异常与「拿不到真实路径」同口径：都拒（fail-closed）。
    let app_for_probe = app.clone();
    let path_for_probe = path.to_string();
    let checked = crate::py::document::spawn_blocking_map(move || {
        Ok::<_, String>(probe_paths_sync(&path_for_probe, &app_for_probe))
    })
    .await;
    let probe = match checked {
        Ok(p) => p,
        Err(e) => return Err(format!("路径不存在或不可访问：{path}（{e}）")),
    };
    let Some(canon) = probe.canon else {
        return Err(format!("路径不存在或不可访问：{path}"));
    };
    // 1) AI_Gen_Files 目录内
    if let Some(gen) = &probe.gen {
        if canon.starts_with(gen) {
            return Ok(());
        }
    }
    // 2) 任务卡绑定文件（多文件绑定：files 列表 + 旧字段兜底走 effective_files）
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        let bound: Vec<String> = tasks
            .iter()
            .flat_map(|t| t.effective_files())
            .map(|f| f.path)
            .collect();
        let canon_for_cmp = canon.clone();
        let hit = crate::py::document::spawn_blocking_map(move || {
            Ok::<_, String>(any_bound_path_matches(&bound, &canon_for_cmp))
        })
        .await
        .unwrap_or(false);
        if hit {
            return Ok(());
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
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let path_opt = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 无 path 时 bot_py::doc_extract 会弹系统选择框，
    // 后台定时执行弹框 = 永久阻塞卡死，必须直接拒绝并引导模型传 path
    if path_opt.is_none() && !interactive {
        return ToolResult::error(
            "失败：后台执行不能弹窗选文件，请提供 path 参数指定文档路径".to_string(),
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
            return ToolResult::denied(e);
        }
    }
    match crate::bot_py::doc_extract(app.clone(), path_opt).await {
        Ok(res) => {
            // N3-2：扫描版 PDF 兜底——文本层近空（pypdf 提不出内容）→ PyMuPDF 转图
            // → 本地 OCR（macOS Vision / PP-，字节全本地）。失败/不适用保持原输出。
            let is_pdf = res.path.to_ascii_lowercase().ends_with(".pdf");
            if is_pdf && pdf_text_looks_empty(&res.text) {
                match scan_pdf_ocr_text(app, &res.path).await {
                    Ok(Some(text)) => {
                        return ToolResult::ok(
                            format_extract_output(&res.path, &text, offset, limit),
                            Vec::new(),
                        );
                    }
                    Err(hint) => {
                        // 缺 pymupdf：原文输出 + 安装指引（原文可能确实只有页码等零星内容）
                        let merged = format!("{}\n\n{hint}", res.text);
                        return ToolResult::ok(
                            format_extract_output(&res.path, &merged, offset, limit),
                            Vec::new(),
                        );
                    }
                    Ok(None) => {} // 渲染/OCR 失败：保持 pypdf 原输出
                }
            }
            ToolResult::ok(
                format_extract_output(&res.path, &res.text, offset, limit),
                Vec::new(),
            )
        }
        Err(e) => ToolResult::error(format!("提取失败：{e}"), Vec::new()),
    }
}

/// N3-2 纯函数：PDF 提取文本是否「近空」——去掉 `=== 第N页 ===` 页标记后
/// 全为空白视为无文本层（扫描件/图片型 PDF）
pub(super) fn pdf_text_looks_empty(text: &str) -> bool {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("=== 第"))
        .all(|l| l.trim().is_empty())
}

/// N3-2：扫描件兜底编排。Ok(Some)=兜底成功带页标记与头部说明）；
/// Ok(None)=渲染或  失败（保持原提取输出）；Err(hint)=缺 pymupdf（hint 拼给模型）。
/// 隐私红线不变：页面 PNG 只落系统临时目录并在用后删除，识别全程本地。
async fn scan_pdf_ocr_text(app: &AppHandle, path: &str) -> Result<Option<String>, String> {
    const MAX_SCAN_PAGES: usize = 20;
    let tmp = std::env::temp_dir().join(format!("wm-scan-{}", uuid::Uuid::new_v4().simple()));
    if std::fs::create_dir_all(&tmp).is_err() {
        return Ok(None);
    }
    let render = crate::bot_py::pdf_render_pages(
        app.clone(),
        path.to_string(),
        tmp.display().to_string(),
        MAX_SCAN_PAGES,
    )
    .await;
    let pages = match render {
        Ok(n) => n,
        Err(e) if e.starts_with("NEED_PYMUPDF") => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "（检测到扫描版/图片型 PDF：文本层为空，已尝试转图本地 OCR 兜底，但本机缺 PyMuPDF——请运行 python3 -m pip install pymupdf 后重试；单次最多识别前 {MAX_SCAN_PAGES} 页）"
            ));
        }
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "scan_pdf.render_fail | {}",
                    crate::bot::truncate_for_log(&e, 200)
                ),
            );
            let _ = std::fs::remove_dir_all(&tmp);
            return Ok(None);
        }
    };
    // 逐页 OCR（spawn_blocking：Vision/ONNX 均同步阻塞推理）；单页失败截断并注明
    let tmp_for_ocr = tmp.clone();
    let ocr = crate::py::document::spawn_blocking_map(
        move || -> Result<(Vec<String>, Option<String>), String> {
            let mut parts = Vec::new();
            let mut first_err = None;
            for i in 1..=pages {
                let p = tmp_for_ocr.join(format!("page-{i:03}.png"));
                let bytes = match std::fs::read(&p) {
                    Ok(b) => b,
                    Err(e) => {
                        first_err = Some(format!("读取渲染页失败：{e}"));
                        break;
                    }
                };
                match crate::ocr::recognize_bytes(&bytes) {
                    Ok(t) => parts.push(format!("=== 第{i}页 ===\n{t}")),
                    Err(e) => {
                        first_err = Some(format!("第 {i} 页识别失败：{e}"));
                        break;
                    }
                }
            }
            Ok((parts, first_err))
        },
    )
    .await
    .unwrap_or_else(|_| (Vec::new(), Some("OCR 线程异常".into())));
    let _ = std::fs::remove_dir_all(&tmp);
    let (parts, first_err) = ocr;
    if parts.is_empty() {
        if let Some(e) = &first_err {
            crate::bot::audit_log(
                app,
                &format!(
                    "scan_pdf.ocr_fail | {}",
                    crate::bot::truncate_for_log(e, 200)
                ),
            );
        }
        return Ok(None);
    }
    crate::bot::audit_log(app, &format!("scan_pdf.ocr | pages: {}", parts.len()));
    let mut text = format!(
        "〔扫描版 PDF：文本层为空，已转图走本地 OCR 兜底（前 {} 页）〕\n",
        parts.len()
    );
    text.push_str(&parts.join("\n"));
    if let Some(e) = first_err {
        text.push_str(&format!("\n（后续页中断：{e}）"));
    } else if pages == MAX_SCAN_PAGES {
        text.push_str(&format!(
            "\n（超过 {MAX_SCAN_PAGES} 页的部分未识别；可拆分后分次提取）"
        ));
    }
    Ok(Some(text))
}

/// extract_document 输出格式化：字符级分页（可续读，不硬截断）。
/// 头部 [位置] 行让模型知道总量与续读点；还有更多时尾部给 offset 续读提示。
/// offset 按字符计（非字节）；limit 默认 30000、硬钳 60000（防爆上下文）。
pub(super) const EXTRACT_DEFAULT_LIMIT: usize = 30000;
const EXTRACT_MAX_LIMIT: usize = 60000;

pub(super) fn format_extract_output(path: &str, text: &str, offset: usize, limit: usize) -> String {
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
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ToolResult::denied("create_word 缺少 paragraphs");
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ToolResult::denied("paragraphs 不能为空");
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    let tables = v.get("tables").cloned();
    // N3-4：images 仅放行 AI_Gen_Files 目录内的已存在图片（防模型借参数探测/读任意路径），
    // 被拒条目审计并计入提示
    let (images, dropped_imgs) = sanitize_image_paths_arg(app, &v).await;
    // 模板解析：显式名优先，否则设置页默认模板；都无 → 内置空白口径
    let template = crate::bot_py::word_template_resolve(app, v["template"].as_str());
    match crate::bot_py::doc_make_word(
        app.clone(),
        title,
        paragraphs,
        opt_filename(&v),
        tables,
        images,
        template,
    )
    .await
    {
        Ok(out) => {
            let warn = if dropped_imgs > 0 {
                format!(
                    "（{dropped_imgs} 个图片路径被忽略：仅支持 AI_Gen_Files 目录内的已存在图片）"
                )
            } else {
                String::new()
            };
            // 真校验后的产物元信息：让模型必须把完整路径转告用户（防「说了生成却找不到文件」）
            let size_note = std::fs::metadata(&out)
                .map(|m| format!("，{} KB", m.len() / 1024))
                .unwrap_or_default();
            ToolResult::ok(
                format!(
                    "已生成 Word 文档：{out}{size_note}{warn}。向用户报告时必须附上完整路径；用户找不到文件时可用 reveal_path 打开所在文件夹"
                ),
                Vec::new(),
            )
            .with_file_change(crate::bot_fs::build_binary_file_receipt(&out))
        }
        Err(e) => ToolResult::error(format!("生成失败：{e}"), Vec::new()),
    }
}

/// N3-4：create_word 的 images 参数清洗（纯内核 filter_images_in 的异步壳）：
/// canonicalize 后必须落在 gen_dir 内且文件存在；整体包 spawn_blocking（同步 syscall
/// 不占 async worker，与 sanitize_task_files_arg 同模式）。
async fn sanitize_image_paths_arg(app: &AppHandle, v: &serde_json::Value) -> (Vec<String>, usize) {
    let Some(arr) = v["images"].as_array() else {
        return (Vec::new(), 0);
    };
    let paths: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    if paths.is_empty() {
        return (Vec::new(), 0);
    }
    let app2 = app.clone();
    let (out, dropped) =
        crate::py::document::spawn_blocking_map(move || -> Result<(Vec<String>, usize), String> {
            let gen_canon = crate::db::gen_dir(&app2)
                .ok()
                .and_then(|d| std::fs::canonicalize(d).ok());
            Ok(filter_images_in(gen_canon.as_deref(), paths))
        })
        .await
        .unwrap_or((Vec::new(), 0));
    if dropped > 0 {
        crate::bot::audit_log(
            app,
            &format!(
                "doc_word_images_sanitized | dropped: {dropped} | 仅放行 AI_Gen_Files 内已存在图片"
            ),
        );
    }
    (out, dropped)
}

/// images 清洗纯内核（单测注入 gen 目录）：存在且 canonicalize 后在 gen 内才放行
pub(super) fn filter_images_in(
    gen_canon: Option<&std::path::Path>,
    paths: Vec<String>,
) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut dropped = 0usize;
    for p in paths {
        let ok = gen_canon.is_some_and(|g| {
            std::fs::canonicalize(&p)
                .map(|c| c.starts_with(g))
                .unwrap_or(false)
        });
        if ok {
            out.push(p);
        } else {
            dropped += 1;
        }
    }
    (out, dropped)
}

/// 修订模式 Word：回读原文 + 修订段落 diff，产出带 track changes 标记的文档
pub(crate) async fn tool_create_word_revisions(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    // originalPath 由模型转述，同样过授权校验（防回读任意文件，走分流）
    if let Some(op) = v["originalPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(e) = extract_path_check(app, op, interactive, session_id).await {
            return ToolResult::denied(e);
        }
    }
    let Some(arr) = v["revised"].as_array() else {
        return ToolResult::denied("create_word_revisions 缺少 revised（润色后的段落列表）");
    };
    let revised: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if revised.is_empty() {
        return ToolResult::denied("revised 不能为空");
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
        return ToolResult::denied("create_word_revisions 缺少原文：请传 originalPath（来自 extract_document 的 [文档路径]）或 original 行列表");
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_word_revisions(app.clone(), title, path, original, revised, opt_filename(&v)).await
    {
        Ok((out, engine)) => ToolResult::ok(
            format!(
                "已生成修订版 Word（修订模式：删除线=删、红色下划线=增，可在 Word「审阅」里逐条接受/拒绝；引擎：{}）：{out}",
                if engine == "dotnet" { ".NET OpenXML" } else { "Python 兜底（.NET 不可用或执行失败）" }
            ),
            Vec::new(),
        )
        .with_file_change(crate::bot_fs::build_binary_file_receipt(&out)),
        Err(e) => ToolResult::error(format!("生成失败：{e}"), Vec::new()),
    }
}
pub(crate) async fn tool_create_excel(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(sheets) = v["sheets"].as_array() else {
        return ToolResult::denied("create_excel 缺少 sheets");
    };
    if sheets.is_empty() {
        return ToolResult::denied("sheets 不能为空");
    }
    match crate::bot_py::doc_make_excel(app.clone(), sheets.clone(), opt_filename(&v)).await {
        Ok(out) => {
            // 真校验后的产物元信息：让模型必须把完整路径转告用户（防「说了生成却找不到文件」）
            let size_note = std::fs::metadata(&out)
                .map(|m| format!("，{} KB", m.len() / 1024))
                .unwrap_or_default();
            ToolResult::ok(
                format!(
                    "已生成 Excel 文档：{out}{size_note}。向用户报告时必须附上完整路径；用户找不到文件时可用 reveal_path 打开所在文件夹"
                ),
                Vec::new(),
            )
            .with_file_change(crate::bot_fs::build_binary_file_receipt(&out))
        }
        Err(e) => ToolResult::error(format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PPT：slides 结构 [{type, title, bullets/rows, notes}]，版式随模板（缺省内置母版）
pub(crate) async fn tool_create_ppt(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(slides) = v["slides"].as_array() else {
        return ToolResult::denied("create_ppt 缺少 slides");
    };
    if slides.is_empty() {
        return ToolResult::denied("slides 不能为空");
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    // template：显式 .pptx 模板名（设置页导入的模板库）；缺省走 _default 标记，再回退内置母版
    let template = v["template"].as_str().map(|s| s.to_string());
    match crate::bot_py::doc_make_ppt(
        app.clone(),
        title,
        slides.clone(),
        opt_filename(&v),
        template,
    )
    .await
    {
        Ok(out) => {
            // 真校验后的产物元信息：让模型必须把完整路径转告用户（防「说了生成却找不到文件」）
            let size_note = std::fs::metadata(&out)
                .map(|m| format!("，{} KB", m.len() / 1024))
                .unwrap_or_default();
            ToolResult::ok(
                format!(
                    "已生成 PPT 演示文稿：{out}{size_note}。向用户报告时必须附上完整路径；用户找不到文件时可用 reveal_path 打开所在文件夹"
                ),
                Vec::new(),
            )
            .with_file_change(crate::bot_fs::build_binary_file_receipt(&out))
        }
        Err(e) => ToolResult::error(format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PDF：title + 段落列表 + 可选表格（N3-3，与 create_word 同形状）
pub(crate) async fn tool_create_pdf(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ToolResult::denied("create_pdf 缺少 paragraphs");
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ToolResult::denied("paragraphs 不能为空");
    }
    let tables = v.get("tables").cloned();
    let title = v["title"].as_str().unwrap_or("").to_string();
    // N3-4：images 与 create_word 同闸——仅放行 AI_Gen_Files 目录内的已存在图片
    let (images, dropped_imgs) = sanitize_image_paths_arg(app, &v).await;
    match crate::bot_py::doc_make_pdf(
        app.clone(),
        title,
        paragraphs,
        tables,
        images,
        opt_filename(&v),
    )
    .await
    {
        Ok(out) => {
            let warn = if dropped_imgs > 0 {
                format!(
                    "（{dropped_imgs} 个图片路径被忽略：仅支持 AI_Gen_Files 目录内的已存在图片）"
                )
            } else {
                String::new()
            };
            // 真校验后的产物元信息：让模型必须把完整路径转告用户（防「说了生成却找不到文件」）
            let size_note = std::fs::metadata(&out)
                .map(|m| format!("，{} KB", m.len() / 1024))
                .unwrap_or_default();
            ToolResult::ok(
                format!(
                    "已生成 PDF 文档：{out}{size_note}{warn}。向用户报告时必须附上完整路径；用户找不到文件时可用 reveal_path 打开所在文件夹"
                ),
                Vec::new(),
            )
            .with_file_change(crate::bot_fs::build_binary_file_receipt(&out))
        }
        Err(e) => ToolResult::error(format!("生成失败：{e}"), Vec::new()),
    }
}

/// 模型给的 `count` 解析：钳制边界单源在 `params`，工具入参口不自己写一份
/// `clamp(1, N)`（与配置落盘侧、读侧 resolver 同一函数）。
///
/// 超 u32::MAX 的值按 u32::MAX 钳，而不是先截断：`5_000_000_000 as u32` = 705032704，
/// 截断后再钳会放行一个荒诞值而不是收口到硬顶。
pub(super) fn parse_search_count(v: &serde_json::Value) -> Option<u32> {
    v.as_u64()
        .map(|n| crate::bot::params::clamp_search_max_results(u32::try_from(n).unwrap_or(u32::MAX)))
}

/// 联网搜索：本机执行（引擎路由 Tavily/Brave/双引擎抓取，见 bot_web::resolve_search_route），
/// 结果回传给模型
pub(crate) async fn tool_web_search(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(query) = v["query"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("web_search 缺少 query");
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return ToolResult::denied(e);
    }
    audit_log(
        app,
        &format!("web_search | query: {}", escape_for_log(&query, 100)),
    );
    // 过滤参数（count/timeRange/site），缺省 = 旧行为；
    // count 缺省值走配置解析（config searchMaxResults，默认 8）；
    // 模型显式给的 count 与配置缺省值共用 params 的同一条钳制边界
    let cfg_search = crate::bot::load_config(app);
    let opts = crate::bot_web::SearchOpts {
        count: parse_search_count(&v["count"])
            .unwrap_or(crate::bot::params::resolve_search_max_results(&cfg_search)),
        time_range: crate::bot_web::TimeRange::parse(v["timeRange"].as_str()),
        site: v["site"].as_str().and_then(crate::bot_web::sanitize_site),
    };
    match crate::bot_web::web_search_with_config(app, &query, &opts).await {
        Ok(results) => ToolResult::ok(results, Vec::new()),
        Err(e) => ToolResult::error(format!("搜索失败：{e}"), Vec::new()),
    }
}

/// fetch_url 输出切片（N3-1 纯函数）：offset 字符级续读（与 extract_document 同模式）。
/// 无 offset 且内容不超限时输出与旧版零差异（不加头部行，省 token）；
/// 超限截断带续读提示；offset 越界给出总量提示。
const FETCH_BODY_CAP: usize = 30000;

pub(super) fn slice_fetch_output(text: &str, offset: usize) -> String {
    let total = text.chars().count();
    if offset >= total && total > 0 {
        return format!("offset {offset} 已超出网页总长 {total} 字符，没有更多内容");
    }
    let slice: String = text.chars().skip(offset).take(FETCH_BODY_CAP).collect();
    let end = offset + slice.chars().count();
    let mut out = String::new();
    if offset > 0 {
        out.push_str(&format!("[位置] {offset}–{end} / 共 {total} 字符\n"));
    }
    out.push_str(&slice);
    if end < total {
        out.push_str(&format!(
            "\n\n（已截断：还有 {} 字符未读，用 offset={end} 参数续读）",
            total - end
        ));
    }
    out
}

/// 抓取网页正文：http/https 公网地址，转纯文本回传（30K 字分页，可 offset 续读）。
/// 每次调用重新抓取整页再切片——无缓存失效问题，网页内容也不会因续读而过期。
pub(crate) async fn tool_fetch_url(
    app: &AppHandle,
    args: &str,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let Some(u) = v["url"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ToolResult::denied("fetch_url 缺少 url");
    };
    if let Err(e) = check_len(&u, MAX_KEYWORD, "网址") {
        return ToolResult::denied(e);
    }
    let offset = v["offset"].as_u64().unwrap_or(0) as usize;
    audit_log(
        app,
        &format!("fetch_url | url: {}", escape_for_log(&u, 100)),
    );
    match crate::bot_web::fetch_text(&u).await {
        Ok(text) => ToolResult::ok(slice_fetch_output(&text, offset), Vec::new()),
        Err(e) => ToolResult::error(format!("抓取失败：{e}"), Vec::new()),
    }
}

/// run_python 算不算失败（纯函数，单测入口）。
///
/// 退出码非 0 = 脚本没跑成；没有退出码 = 异常终止（被信号杀死），同样不是成功。
///
/// 如实报失败的意义：报成功等于告诉模型「跑通了」，它就不会去改脚本；而
/// 「别整段重写、只改出错那几行」的失败提示只在失败时才注入——这条链断了，
/// 模型就会把整段脚本重写一遍（每次重写重付一遍全文的输出 token）。
pub(super) fn run_failed(exit_code: Option<i32>) -> bool {
    !matches!(exit_code, Some(0))
}

/// run_python 的代码来源：内联代码，还是已落盘的脚本文件。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ScriptSource {
    /// 内联代码（`code` 参数）
    Inline(String),
    /// 已落盘的脚本文件路径（`scriptPath` 参数）
    File(String),
    /// 两个都没给
    Missing,
}

/// 从工具参数里判定代码来源（纯函数，单测入口）。
///
/// scriptPath 优先于 code：模型改了脚本却仍传旧 code 时，执行旧 code 会让
/// 「改了文件没生效」变成看不见的坑——文件是新的、跑的是旧的。
pub(super) fn script_source(v: &serde_json::Value) -> ScriptSource {
    let p = v["scriptPath"].as_str().unwrap_or("").trim();
    if !p.is_empty() {
        return ScriptSource::File(p.to_string());
    }
    match v["code"].as_str() {
        Some(c) => ScriptSource::Inline(c.to_string()),
        None => ScriptSource::Missing,
    }
}

/// 自由 Python 编程：开关开启才放行（超时 60s、独立临时目录、输出截断）
/// py_exec_sync 是 sync 阻塞（最长 300s），必须经 async 包装挪到 blocking
/// 线程池，不得占住 async runtime worker（与 doc_* 同模式）。
/// 透传 /stop 令牌，在途 Python 可被中断（StopGuard → owned StopToken）。
///
/// 代码从两处来：`code`（内联）或 `scriptPath`（已落盘的脚本文件）。
/// 落盘那条是长脚本的正解：出错时只改出错的那几行重跑，不必把整段代码重写
/// 一遍（内联代码每改一次就要重付一遍全文的输出 token，且改错无从对照）。
pub(crate) async fn tool_run_python(
    app: &AppHandle,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
    interactive: bool,
    session_id: Option<&str>,
) -> crate::bot::registry::ToolResult {
    let v = parse_args(args);
    let code = match script_source(&v) {
        ScriptSource::Inline(c) => c,
        ScriptSource::File(p) => {
            match crate::bot_fs::read_whitelisted_text(
                app,
                "run_python",
                &p,
                interactive,
                session_id,
                crate::bot_fs::READ_MAX_BYTES,
            )
            .await
            {
                Ok(text) => text,
                Err(e) => return ToolResult::denied(format!("读取脚本失败：{e}")),
            }
        }
        ScriptSource::Missing => {
            return ToolResult::denied(
                "run_python 缺少 code（内联代码）或 scriptPath（已落盘脚本）",
            )
        }
    };
    // 超时优先级：工具参数 timeoutSecs > 设置页配置 python_timeout_secs > 内置 60s
    //（pandas 大计算 60s 偏紧；硬钳在 py::runtime::resolve_timeout）
    let timeout_secs = v["timeoutSecs"]
        .as_u64()
        .or_else(|| load_config(app).python_timeout_secs);
    match crate::bot_py::py_exec_sync_async(
        app.clone(),
        code,
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
            // 退出码非 0 = 脚本没跑成，**如实报失败**：报成功等于告诉模型
            // 「跑通了」，它就不会去改脚本；而失败提示（别整段重写、只改出错
            // 那几行）也只在失败时才注入。
            if run_failed(r.exit_code) {
                let code_text = match r.exit_code {
                    Some(c) => c.to_string(),
                    None => "无退出码（异常终止）".to_string(),
                };
                let note = r
                    .retained_dir
                    .as_deref()
                    .map(|d| {
                        format!(
                            "\n（脚本已保留在 {d}/{}，可 read_text_file 读回后只改出错的那几行，再用 run_python 的 scriptPath 重跑；报错行号与该文件一致）",
                            crate::bot_py::SCRIPT_FILE
                        )
                    })
                    .unwrap_or_default();
                ToolResult::error(
                    format!("脚本退出码 {code_text}（未跑成功）\n{out}{note}"),
                    Vec::new(),
                )
            } else {
                ToolResult::ok(out, Vec::new())
            }
        }
        Err(e) => ToolResult::error(format!("执行失败：{e}"), Vec::new()),
    }
}
