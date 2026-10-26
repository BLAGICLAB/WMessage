//! 文档生成（Word / Excel / PDF / PPT）+ py_exec_sync 入口
//!
//! 包含 6 个固定 Python 脚本常量 + 5 个 doc_* tauri 命令 + py_exec_sync 同步入口。
//! 引擎优先 .NET OpenXML（修订版 Word），不可用回退 Python 脚本。

use std::path::Path;

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::bot::StopToken;
use crate::error::{CommandError, CommandResult};
use crate::py::audit::py_audit;
use crate::py::env::run_dotnet_revisions;
use crate::py::runtime::{
    py_gate_acquire, run_python, run_python_ungated, PyRunResult, MAX_TIMEOUT_SECS,
};

// 固定文档脚本模板

pub const EXTRACT_SCRIPT: &str = include_str!("scripts/extract.py");

pub const MAKE_DOCX_SCRIPT: &str = include_str!("scripts/make_docx.py");

// HARVEST_PROTO_SCRIPT：模板文字排版参数提取（双层口径的提取端）。打开模板
// 正文收割格式原型，折算成参数 JSON 单行打到 stdout（Rust 侧落 <模板名>.json
// 旁车）。上传（word_template_import）、编辑面板（word_template_params_get /
// reextract）、生成自愈（doc_make_word 旁车缺失时）三处共用这一个启发式——
// MAKE_DOCX_SCRIPT 只按参数构造、绝不再收割，防止提取与生成两套口径漂移。
pub const HARVEST_PROTO_SCRIPT: &str = include_str!("scripts/harvest_proto.py");

// MAKE_DOCX_REVISIONS_SCRIPT：修订版 Word 的 Python 脚本——引擎优先 .NET，
// .NET 不可用或失败时回退走本脚本（run_doc_revisions 内的 fallback）。
pub const MAKE_DOCX_REVISIONS_SCRIPT: &str = include_str!("scripts/make_docx_revisions.py");

pub const MAKE_XLSX_SCRIPT: &str = include_str!("scripts/make_xlsx.py");

pub const MAKE_PDF_SCRIPT: &str = include_str!("scripts/make_pdf.py");

/// N3-2：扫描版 PDF 页渲染——PyMuPDF（fitz）官方推荐渲染管线，2x zoom ≈144 DPI
///（）。exit 2 = 缺 pymupdf（Rust 侧给 pip 指引，优雅降级）；
/// 页数钳在前 max_pages 页（防大文档渲染爆内存/超时）。
pub const PDF_RENDER_SCRIPT: &str = include_str!("scripts/pdf_render.py");

pub const MAKE_PPTX_SCRIPT: &str = include_str!("scripts/make_pptx.py");

// 对外命令

/// 执行同步核心（工具链在 async 上下文直接调用）
/// `stop`：/stop 令牌，在途执行可被中断；UI 直调传 None

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocExtract {
    pub path: String,
    pub text: String,
}

pub async fn doc_extract(app: AppHandle, path: Option<String>) -> CommandResult<DocExtract> {
    let path = match path {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            let handle = app.clone();
            let picked = tauri::async_runtime::spawn_blocking(move || {
                handle.dialog().file().blocking_pick_file()
            })
            .await
            .unwrap_or_else(|e| {
                // 对话框线程异常不能伪装成「用户取消」——记审计后按未选择处理
                py_audit(
                    &app,
                    &format!(
                        "doc_extract dialog join err | {}",
                        crate::audit::escape_for_log(&e.to_string(), 200)
                    ),
                );
                None
            });
            resolve_doc_path(picked.and_then(file_path_to_string))?
        }
    };
    py_audit(
        &app,
        &format!(
            "doc_extract | path: {}",
            crate::audit::escape_for_log(&path, 300)
        ),
    );
    let input = serde_json::json!({ "path": path }).to_string();
    let r = run_doc_script(&app, "doc_extract", EXTRACT_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_extract failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("提取失败", &r.stderr));
    }
    Ok(DocExtract {
        path,
        text: r.stdout,
    })
}

pub fn resolve_doc_path(picked: Option<String>) -> CommandResult<String> {
    picked.ok_or_else(|| CommandError::Internal("用户取消了选择".into()))
}

/// N3-2：渲染扫描版 PDF 的前 max_pages 页为 PNG（供本地 ）。
/// Err 前缀 `NEED_PYMUPDF` = 缺 pymupdf（调用方给 pip 指引，优雅降级）。
pub async fn pdf_render_pages(
    app: AppHandle,
    path: String,
    out_dir: String,
    max_pages: usize,
) -> Result<usize, String> {
    let input = serde_json::json!({
        "path": path,
        "out_dir": out_dir,
        "max_pages": max_pages
    })
    .to_string();
    let r = run_doc_script(&app, "pdf_render_pages", PDF_RENDER_SCRIPT, input).await?;
    if r.exit_code == Some(2) {
        return Err(
            "NEED_PYMUPDF: 需要 PyMuPDF 渲染扫描版 PDF：python3 -m pip install pymupdf".into(),
        );
    }
    if r.exit_code != Some(0) {
        return Err(format!("PDF 页面渲染失败：{}", r.stderr.trim()));
    }
    r.stdout
        .trim()
        .parse::<usize>()
        .map_err(|e| format!("PDF 渲染输出异常（{e}）"))
}

pub fn script_fail_err(what: &str, stderr: &str) -> CommandError {
    // 空细节不拼悬空冒号：明说脚本没给原因（排查方向转向 stdout / 外部信号）
    let msg = if stderr.trim().is_empty() {
        format!("{what}：（脚本未输出失败原因）")
    } else {
        format!("{what}：{}", stderr.trim())
    };
    CommandError::Internal(msg)
}

pub fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        // 桌面端对话框可能给 file:// Url：转回本地路径；非文件 URL 返回 None（fail-closed）
        tauri_plugin_dialog::FilePath::Url(u) => u
            .to_file_path()
            .ok()
            .and_then(|pb| pb.to_str().map(String::from)),
    }
}

/// 产物交付前的收口：剥掉「来自互联网」标记（Zone.Identifier 备用数据流）。
///
/// Windows 上复制文件会连带复制这个标记（`CopyFileEx` 复制全部数据流），
/// 于是 app 交出去的副本被 Office 当成「外来的文件」：打开走受保护视图，
/// 把文件复制进 Office 文档缓存（`INetCache\Content.Word\~WRDxxxx.docx`），
/// 杀软再把这次「写文档」按风险程序连坐拦掉。
///
/// 剥不掉只记日志不中断——标记只影响打开路径，文件本身是好的。
fn strip_download_mark(app: &AppHandle, path: &Path, tag: &str) {
    if let Err(e) = crate::platform::zone_id::strip_zone_identifier(path) {
        py_audit(
            app,
            &format!(
                "{tag} 剥「来自互联网」标记失败 | {} | {e}",
                crate::audit::escape_for_log(&path.to_string_lossy(), 200)
            ),
        );
    }
}

pub async fn doc_make_word(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
    tables: Option<serde_json::Value>,
    images: Vec<String>,
    template: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    // 参数层：旁车 <模板名>.json 优先；缺失（存量模板）时现场提取并落盘自愈，
    // 启发式单点在 HARVEST_PROTO_SCRIPT。提取失败 = 模板打不开/损坏（与生成
    // 脚本内打开模板同败因），直接上抛
    let tpl_name = template
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(Path::new)
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string());
    let (tpl_params, params_note) = match template.as_deref().filter(|t| !t.is_empty()) {
        Some(tp) => {
            let sidecar = Path::new(tp).with_extension("json");
            match std::fs::read_to_string(&sidecar)
                .ok()
                .and_then(|s| serde_json::from_str::<WordTemplateParams>(&s).ok())
            {
                Some(v) => (Some(v), "params: saved"),
                None => {
                    let v = harvest_template_params(&app, Path::new(tp)).await?;
                    if let Err(e) = word_template_params_write(&sidecar, &v) {
                        // 落盘失败不阻断生成（参数已到手），下次生成重提
                        py_audit(
                            &app,
                            &format!(
                                "doc_make_word 旁车参数落盘失败 | {}",
                                crate::audit::escape_for_log(&e, 120)
                            ),
                        );
                    }
                    (Some(v), "params: harvested")
                }
            }
        }
        None => (None, ""),
    };
    // 审计带模板名与参数来源：锚定是否生效在产物里肉眼难辨，排障先要知道
    // 这次用没用模板、参数是存量旁车还是现场收割
    let tpl_note = match &tpl_name {
        Some(n) => format!(" | tpl: {n} {params_note}"),
        None => " | tpl: -".to_string(),
    };
    let input = serde_json::json!({
        "title": title,
        "paragraphs": paragraphs,
        "tables": tables.unwrap_or(serde_json::json!([])),
        "images": images,
        "template": template.unwrap_or_default(),
        "tpl_params": tpl_params,
        "out": out,
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_word", MAKE_DOCX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_word failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 Word 失败", &r.stderr));
    }
    // 真校验：退出码 0 ≠ 文件落盘——脚本任何「静默跳过保存」的路径都会假成功，
    // 这里对产物本身做存在性 + 非空检查（杜绝对用户谎报生成成功）
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_word 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    strip_download_mark(&app, Path::new(&out), "doc_make_word");
    py_audit(&app, &format!("doc_make_word{tpl_note} | out: {out}"));
    Ok(out)
}

pub async fn doc_make_word_revisions(
    app: AppHandle,
    title: String,
    original_path: Option<String>,
    original: Vec<String>,
    revised: Vec<String>,
    filename: Option<String>,
) -> CommandResult<(String, &'static str)> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let src = original_path.clone().unwrap_or_default();
    let input = serde_json::json!({
        "title": title,
        "original_path": original_path.unwrap_or_default(),
        "original": original,
        "revised": revised,
        "out": out
    })
    .to_string();
    let (r, engine) = run_doc_revisions(
        &app,
        "doc_make_word_revisions",
        MAKE_DOCX_REVISIONS_SCRIPT,
        input,
    )
    .await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_word_revisions failed | engine: {engine} | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成修订版 Word 失败", &r.stderr));
    }
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_word_revisions 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    strip_download_mark(&app, Path::new(&out), "doc_make_word_revisions");
    py_audit(
        &app,
        &format!("doc_make_word_revisions | engine: {engine} | src: {src} | out: {out}"),
    );
    Ok((out, engine))
}

pub async fn doc_make_excel(
    app: AppHandle,
    sheets: Vec<serde_json::Value>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "xlsx")?;
    let input = serde_json::json!({ "sheets": sheets, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_excel", MAKE_XLSX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_excel failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 Excel 失败", &r.stderr));
    }
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_excel 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    strip_download_mark(&app, Path::new(&out), "doc_make_excel");
    py_audit(&app, &format!("doc_make_excel | out: {out}"));
    Ok(out)
}

pub async fn doc_make_pdf(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    tables: Option<serde_json::Value>,
    images: Vec<String>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pdf")?;
    let input = serde_json::json!({
        "title": title,
        "paragraphs": paragraphs,
        "tables": tables.unwrap_or(serde_json::json!([])),
        "images": images,
        "out": out
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_pdf", MAKE_PDF_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_pdf failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 PDF 失败", &r.stderr));
    }
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_pdf 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    strip_download_mark(&app, Path::new(&out), "doc_make_pdf");
    py_audit(&app, &format!("doc_make_pdf | out: {out}"));
    Ok(out)
}

pub async fn doc_make_ppt(
    app: AppHandle,
    title: String,
    slides: Vec<serde_json::Value>,
    filename: Option<String>,
    template: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pptx")?;
    // 版式随母版：显式模板 → _default 标记 → 内置母版，永不为空
    let user_requested = template.is_some();
    let template = match pptx_template_resolve(&app, template.as_deref()) {
        Some(p) => p,
        None => {
            // 显式给名但解析不到：落到内置母版 = 静默换版式，用户无可观；记审计
            if user_requested {
                py_audit(
                    &app,
                    &format!(
                        "doc_make_ppt template_fallback | requested={} -> builtin",
                        crate::audit::escape_for_log(template.as_deref().unwrap(), 200)
                    ),
                );
            }
            builtin_pptx_master(&app)?
        }
    };
    let input = serde_json::json!({
        "title": title,
        "slides": slides,
        "out": out,
        "template": template,
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_ppt", MAKE_PPTX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_ppt failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成 PPT 失败", &r.stderr));
    }
    // 真校验（同 doc_make_word）：退出码 0 ≠ 文件落盘
    let meta = std::fs::metadata(&out);
    if meta.as_ref().map(|m| m.len() == 0).unwrap_or(true) {
        py_audit(
            &app,
            &format!("doc_make_ppt 假成功拦截 | {out} 未落盘或为空"),
        );
        return Err(CommandError::IoError(format!(
            "脚本退出正常但文件未生成（{out}），已拦截本次假成功"
        )));
    }
    strip_download_mark(&app, Path::new(&out), "doc_make_ppt");
    py_audit(&app, &format!("doc_make_ppt | out: {out}"));
    Ok(out)
}

pub fn strip_known_ext(name: &str, ext: &str) -> String {
    let suffix = format!(".{}", ext.to_lowercase());
    if name.to_lowercase().ends_with(&suffix) {
        let keep = name.chars().count() - suffix.chars().count();
        name.chars().take(keep).collect()
    } else {
        name.to_string()
    }
}

pub fn gen_out_path(app: &AppHandle, filename: Option<&str>, ext: &str) -> CommandResult<String> {
    gen_out_path_in(&crate::db::gen_dir(app)?, filename, ext)
}

pub fn gen_out_path_in(dir: &Path, filename: Option<&str>, ext: &str) -> CommandResult<String> {
    std::fs::create_dir_all(dir)?;
    let base = filename
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| format!("文档-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let base = strip_known_ext(&base, ext);
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    Ok(candidate.to_string_lossy().to_string())
}

// ── Word 模板管理（设置页「Word 模板」面板 + create_word 模板锚定） ──
//
// 模板 = 数据目录 word_templates/<name>.docx，用户在 Word 里排好版式上传；
// 生成时打开模板并清空正文示例（保留 sectPr 的页面设置/页眉页脚/样式表），
// 内容按命名样式填充——「LLM 填内容、模板管排版」的确定性排版口径。

fn word_templates_dir(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("word_templates")
}

fn default_marker_path(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("_default")
}

/// 模板解析：显式名字优先（不存在 → None），否则默认模板（不存在 → None）。
/// None = 无模板可用，生成走内置空白默认。
pub fn word_template_resolve(app: &AppHandle, name: Option<&str>) -> Option<String> {
    word_template_resolve_in(&word_templates_dir(app), name)
}

fn word_template_resolve_in(dir: &std::path::Path, name: Option<&str>) -> Option<String> {
    let explicit = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .filter(|n| !n.contains('/') && !n.contains('\\') && !n.contains(".."))
        .map(|n| dir.join(format!("{n}.docx")));
    let candidate = match explicit {
        Some(p) => p,
        None => {
            let def = std::fs::read_to_string(default_marker_path(dir)).ok()?;
            dir.join(format!("{def}.docx"))
        }
    };
    candidate
        .exists()
        .then(|| candidate.to_string_lossy().to_string())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WordTemplateInfo {
    pub name: String,
    pub mtime_ms: i64,
    pub is_default: bool,
}

#[tauri::command]
pub fn word_template_list(app: AppHandle) -> CommandResult<Vec<WordTemplateInfo>> {
    let dir = word_templates_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let def = std::fs::read_to_string(default_marker_path(&dir)).unwrap_or_default();
    let mut out: Vec<WordTemplateInfo> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| CommandError::IoError(e.to_string()))? {
        let entry = entry.map_err(|e| CommandError::IoError(e.to_string()))?;
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("docx") {
            continue;
        }
        let Some(name) = p.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let mtime_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(WordTemplateInfo {
            name: name.to_string(),
            mtime_ms,
            is_default: def == name,
        });
    }
    out.sort_by_key(|t| std::cmp::Reverse(t.mtime_ms));
    Ok(out)
}

#[tauri::command]
pub async fn word_template_import(app: AppHandle, path: String) -> CommandResult<String> {
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "请选择一个 .docx 模板文件".into(),
        });
    }
    if src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| !e.eq_ignore_ascii_case("docx"))
        .unwrap_or(true)
    {
        return Err(CommandError::InvalidArgument {
            field: "path".into(),
            value: path,
            reason: "模板必须是 .docx 文件".into(),
        });
    }
    let dir = word_templates_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dest = dir.join(format!("{name}.docx"));
    if dest.exists() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name.clone(),
            reason: "同名模板已存在：请先删除再上传".into(),
        });
    }
    std::fs::copy(&src, &dest).map_err(|e| CommandError::IoError(e.to_string()))?;
    // 拷贝会连带复制「来自互联网」标记（CopyFileEx 复制全部备用数据流）——
    // 模板是用户从网上下载的常见来源，标记留着会让它进 Office 受保护视图
    strip_download_mark(&app, &dest, "word_template_import");
    // 参数层：上传即提取——模板打不开/损坏在这里 fail-closed，拷贝一并回滚；
    // 提取产物落旁车 json，生成与编辑面板都直接取参数
    let params = match harvest_template_params(&app, &dest).await {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&dest);
            return Err(e);
        }
    };
    if let Err(e) = word_template_params_save_in(&dir, &name, &params) {
        let _ = std::fs::remove_file(&dest);
        let _ = std::fs::remove_file(template_params_path_in(&dir, &name));
        return Err(CommandError::IoError(format!("模板参数写入失败：{e}")));
    }
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_import | {} | params: extracted",
            crate::bot::escape_for_log(&name, 100)
        ),
    );
    Ok(name)
}

#[tauri::command]
pub fn word_template_delete(app: AppHandle, name: String) -> CommandResult<()> {
    let dir = word_templates_dir(&app);
    let p = dir.join(format!("{name}.docx"));
    if p.exists() {
        std::fs::remove_file(&p).map_err(|e| CommandError::IoError(e.to_string()))?;
    }
    // 旁车参数一并删，不留孤儿 json
    let _ = std::fs::remove_file(template_params_path_in(&dir, &name));
    if std::fs::read_to_string(default_marker_path(&dir))
        .map(|d| d == name)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(default_marker_path(&dir));
    }
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_delete | {}",
            crate::bot::escape_for_log(&name, 100)
        ),
    );
    Ok(())
}

#[tauri::command]
pub fn word_template_set_default(app: AppHandle, name: String) -> CommandResult<()> {
    let dir = word_templates_dir(&app);
    let p = dir.join(format!("{name}.docx"));
    if !p.exists() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason: "模板不存在".into(),
        });
    }
    std::fs::write(default_marker_path(&dir), &name)
        .map_err(|e| CommandError::IoError(e.to_string()))?;
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_default | {}",
            crate::bot::escape_for_log(&name, 100)
        ),
    );
    Ok(())
}

// ── Word 模板参数层（双层口径：<name>.docx 管页面家具，<name>.json 管文字排版）──

/// 一组文字排版参数（title/body/h1/h2/h3 各一份）。字段名 snake_case：与
/// HARVEST_PROTO_SCRIPT 的 stdout、MAKE_DOCX_SCRIPT 入参同键，不做 camelCase
/// 改写；数值 = OOXML 原生单位，号数/磅/字符的人话换算在前端
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct WordTextStyle {
    pub font_east: String,
    pub font_ascii: String,
    /// 字号，半磅（三号 = 32）
    pub size_half: i32,
    pub bold: bool,
    /// left / center / right / both
    pub align: String,
    /// auto=倍数（line_line/240） / exact=固定值 / atLeast=最小值
    pub line_rule: String,
    /// 行距值：auto 时 240×倍数，exact/atLeast 时缇；0 = 不显式设置
    pub line_line: i32,
    /// 首行缩进字符数（0 = 不缩进）
    pub indent_chars: f64,
    /// 段前/段后，缇
    pub before: i32,
    pub after: i32,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct WordTemplateParams {
    pub version: i32,
    pub title: WordTextStyle,
    pub body: WordTextStyle,
    pub h1: WordTextStyle,
    pub h2: WordTextStyle,
    pub h3: WordTextStyle,
}

fn template_params_path_in(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    dir.join(format!("{name}.json"))
}

fn word_template_params_write(
    sidecar: &std::path::Path,
    params: &WordTemplateParams,
) -> Result<(), String> {
    let s = serde_json::to_string_pretty(params).map_err(|e| e.to_string())?;
    std::fs::write(sidecar, s).map_err(|e| e.to_string())
}

fn word_template_params_save_in(
    dir: &std::path::Path,
    name: &str,
    params: &WordTemplateParams,
) -> Result<(), String> {
    word_template_params_write(&template_params_path_in(dir, name), params)
}

fn word_template_params_load_in(dir: &std::path::Path, name: &str) -> Option<WordTemplateParams> {
    let s = std::fs::read_to_string(template_params_path_in(dir, name)).ok()?;
    serde_json::from_str(&s).ok()
}

/// 跑提取脚本收割模板文字排版参数——上传、编辑面板、生成自愈三处共用，
/// 启发式单点在 HARVEST_PROTO_SCRIPT
async fn harvest_template_params(
    app: &AppHandle,
    docx_path: &Path,
) -> CommandResult<WordTemplateParams> {
    let input = serde_json::json!({ "template": docx_path.to_string_lossy() }).to_string();
    let r = run_doc_script(app, "word_template_harvest", HARVEST_PROTO_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            app,
            &format!(
                "word_template_harvest failed | {}",
                crate::audit::escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("模板参数提取失败", &r.stderr));
    }
    serde_json::from_str(r.stdout.trim())
        .map_err(|e| CommandError::Internal(format!("模板参数解析失败：{e}")))
}

/// 读模板参数；旁车缺失（存量模板）时现场提取并落盘补齐——与生成路径自愈同口径
#[tauri::command]
pub async fn word_template_params_get(
    app: AppHandle,
    name: String,
) -> CommandResult<WordTemplateParams> {
    let dir = word_templates_dir(&app);
    if !dir.join(format!("{name}.docx")).exists() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason: "模板不存在".into(),
        });
    }
    if let Some(p) = word_template_params_load_in(&dir, &name) {
        return Ok(p);
    }
    let docx_path = dir.join(format!("{name}.docx"));
    let params = harvest_template_params(&app, &docx_path).await?;
    word_template_params_save_in(&dir, &name, &params).map_err(CommandError::IoError)?;
    Ok(params)
}

#[tauri::command]
pub fn word_template_params_update(
    app: AppHandle,
    old_name: String,
    new_name: String,
    params: WordTemplateParams,
) -> CommandResult<()> {
    let dir = word_templates_dir(&app);
    word_template_params_update_in(&dir, &old_name, &new_name, &params)
        .map_err(CommandError::IoError)?;
    crate::bot::audit_log(
        &app,
        &format!(
            "word_template_update | {} -> {}",
            crate::bot::escape_for_log(&old_name, 100),
            crate::bot::escape_for_log(&new_name, 100)
        ),
    );
    Ok(())
}

/// update 内核（_in 后缀供测试）：改名连带 docx/旁车 json/_default 标记三处；
/// 参数整体落盘（结构校验已由命令入参反序列化完成）
fn word_template_params_update_in(
    dir: &std::path::Path,
    old_name: &str,
    new_name: &str,
    params: &WordTemplateParams,
) -> Result<(), String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("模板名不能为空".to_string());
    }
    if new_name.contains('/') || new_name.contains('\\') || new_name.contains("..") {
        return Err("模板名含非法字符".to_string());
    }
    let old_docx = dir.join(format!("{old_name}.docx"));
    if !old_docx.exists() {
        return Err(format!("模板不存在：{old_name}"));
    }
    if new_name != old_name {
        let new_docx = dir.join(format!("{new_name}.docx"));
        if new_docx.exists() {
            return Err(format!("同名模板已存在：{new_name}"));
        }
        std::fs::rename(&old_docx, &new_docx).map_err(|e| format!("改名失败：{e}"))?;
        let marker_matches = std::fs::read_to_string(default_marker_path(dir))
            .map(|d| d == old_name)
            .unwrap_or(false);
        if marker_matches {
            let _ = std::fs::write(default_marker_path(dir), new_name);
        }
    }
    word_template_params_save_in(dir, new_name, params)?;
    if new_name != old_name {
        let _ = std::fs::remove_file(template_params_path_in(dir, old_name));
    }
    Ok(())
}

/// 重新提取：按当前 docx 收割参数覆盖旁车（提取启发式升级/模板被外部修改后用）
#[tauri::command]
pub async fn word_template_reextract(
    app: AppHandle,
    name: String,
) -> CommandResult<WordTemplateParams> {
    let dir = word_templates_dir(&app);
    let docx_path = dir.join(format!("{name}.docx"));
    if !docx_path.exists() {
        return Err(CommandError::InvalidArgument {
            field: "name".into(),
            value: name,
            reason: "模板不存在".into(),
        });
    }
    let params = harvest_template_params(&app, &docx_path).await?;
    word_template_params_save_in(&dir, &name, &params).map_err(CommandError::IoError)?;
    Ok(params)
}

// ── PPT 模板（create_ppt 版式随母版；无用户模板时回退内置母版）──

/// 内置母版（午夜商务主题，16:9，封面/章节版式深底白字）：python-pptx 默认模板
/// 改 theme 配色/字体（含 eastAsia 雅黑）+ 版式深底生成，随二进制分发
const BUILTIN_PPTX_MASTER: &[u8] = include_bytes!("../../assets/pptx-master/default.pptx");

fn pptx_templates_dir(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("pptx_templates")
}

/// 内置母版缓存路径（app 拥有的缓存：字节与内嵌资产不一致即覆盖，无删除语义；
/// 用户自传模板在 pptx_templates/，两套目录互不干扰）
fn builtin_pptx_master(app: &AppHandle) -> CommandResult<String> {
    let dir = crate::db::data_dir(app).join("builtin_templates");
    builtin_pptx_master_in(&dir)
}

fn builtin_pptx_master_in(dir: &std::path::Path) -> CommandResult<String> {
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let dest = dir.join("pptx-default.pptx");
    let stale = std::fs::read(&dest)
        .map(|bytes| bytes != BUILTIN_PPTX_MASTER)
        .unwrap_or(true);
    if stale {
        std::fs::write(&dest, BUILTIN_PPTX_MASTER)
            .map_err(|e| CommandError::IoError(e.to_string()))?;
    }
    Ok(dest.to_string_lossy().to_string())
}

/// 模板解析：显式名优先（不存在 → None），否则默认模板（不存在 → None）。
/// None = 无用户模板可用，调用方回退内置母版。
pub fn pptx_template_resolve(app: &AppHandle, name: Option<&str>) -> Option<String> {
    pptx_template_resolve_in(&pptx_templates_dir(app), name)
}

fn pptx_template_resolve_in(dir: &std::path::Path, name: Option<&str>) -> Option<String> {
    let explicit = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .filter(|n| !n.contains('/') && !n.contains('\\') && !n.contains(".."))
        .map(|n| dir.join(format!("{n}.pptx")));
    let candidate = match explicit {
        Some(p) => p,
        None => {
            let def = std::fs::read_to_string(default_marker_path(dir)).ok()?;
            dir.join(format!("{def}.pptx"))
        }
    };
    candidate
        .exists()
        .then(|| candidate.to_string_lossy().to_string())
}

// py_exec_sync / spawn_blocking_map / run_doc_*

pub async fn spawn_blocking_map<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("执行线程异常：{e}"))?
}

pub async fn run_doc_script(
    app: &AppHandle,
    name: &str,
    script: &'static str,
    input: String,
) -> Result<PyRunResult, String> {
    let handle = app.clone();
    match spawn_blocking_map(move || {
        run_python(&handle, script, Some(&input), &[], Some(120), None).map_err(|e| e.to_string())
    })
    .await
    {
        Ok(inner) => Ok(inner),
        Err(e) => {
            py_audit(
                app,
                &format!(
                    "{name} err | {}",
                    crate::audit::escape_for_log(&e.to_string(), 300)
                ),
            );
            Err(e.to_string())
        }
    }
}

pub async fn run_doc_revisions(
    app: &AppHandle,
    name: &str,
    script: &'static str,
    input: String,
) -> Result<(PyRunResult, &'static str), String> {
    let handle = app.clone();
    let name_in = name.to_string();
    match spawn_blocking_map(move || {
        // doc 生成流持一个许可跨 dotnet+python 兜底（退出态在此拒发）
        let _gate = py_gate_acquire().map_err(|e| e.to_string())?;
        if let Some(r) = run_dotnet_revisions(&handle, &input) {
            match r {
                Ok(res) if res.exit_code == Some(0) => return Ok((res, "dotnet")),
                Ok(res) => py_audit(
                    &handle,
                    &format!(
                        "{name_in} dotnet failed, fallback python | {}",
                        crate::audit::escape_for_log(&res.stderr, 200)
                    ),
                ),
                Err(e) => py_audit(
                    &handle,
                    &format!(
                        "{name_in} dotnet err, fallback python | {}",
                        crate::audit::escape_for_log(&e, 200)
                    ),
                ),
            }
        }
        run_python_ungated(&handle, script, Some(&input), &[], Some(120), None)
            .map(|res| (res, "python"))
            .map_err(|e| e.to_string())
    })
    .await
    {
        Ok(inner) => Ok(inner),
        Err(e) => {
            py_audit(
                app,
                &format!(
                    "{name} err | {}",
                    crate::audit::escape_for_log(&e.to_string(), 300)
                ),
            );
            Err(e.to_string())
        }
    }
}

pub fn py_exec_sync(
    app: &AppHandle,
    code: String,
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, String> {
    crate::py::commands::ensure_py_enabled(app)?;
    if let Some(t) = timeout_secs {
        if t > MAX_TIMEOUT_SECS {
            py_audit(
                app,
                &format!("py_exec err | kind=timeout_cap | requested={t} cap={MAX_TIMEOUT_SECS}"),
            );
            return Err(format!("timeout 超过 {MAX_TIMEOUT_SECS}s 上限"));
        }
    }
    py_audit(
        app,
        &format!(
            "py_exec | script: {}",
            crate::audit::escape_for_log(&code, 300)
        ),
    );
    let r = match run_python(app, &code, None, &[], timeout_secs, stop) {
        Ok(r) => r,
        Err(e) => {
            py_audit(
                app,
                &format!(
                    "py_exec err | {}",
                    crate::audit::escape_for_log(&e.to_string(), 300)
                ),
            );
            return Err(e.to_string());
        }
    };
    py_audit(
        app,
        &format!(
            "py_exec done | exit={:?} {}ms | out: {}",
            r.exit_code,
            r.duration_ms,
            crate::audit::escape_for_log(&r.stdout, 300)
        ),
    );
    Ok(r)
}

pub async fn py_exec_sync_async(
    app: AppHandle,
    code: String,
    timeout_secs: Option<u64>,
    stop: Option<StopToken>,
) -> Result<PyRunResult, String> {
    spawn_blocking_map(move || py_exec_sync(&app, code, timeout_secs, stop.as_ref())).await
}

#[cfg(test)]
mod download_mark_guard_tests;
#[cfg(test)]
mod extract_script_e2e_tests;
#[cfg(test)]
mod fake_success_guard_tests;
#[cfg(test)]
mod pptx_template_tests;
#[cfg(test)]
mod script_stderr_guard_tests;
#[cfg(test)]
mod word_template_params_tests;
#[cfg(test)]
mod word_template_tests;
