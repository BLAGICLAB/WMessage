//! 电脑辅助 Tier1 工具：reveal_path / open_url / clipboard_write /
//! copy_file_to_clipboard / screenshot。
//!
//! 定位：只「看」与「打开」+ 把东西递到用户手上——**不做鼠标键盘注入**。
//! 鼠标键盘 / UI 树读取是另一套能力，放在 `bot_ui.rs` + `ui/`（独立总开关，
//! 默认关闭，见 ui::ui_enabled）；本模块只保留无副作用的看/开/递。
//!
//! 安全边界（对齐官方 best practice）：
//! - reveal_path 走 `bot_fs::resolve_with_perm`，与读文件同一白名单闸；
//! - open_url 复用 `bot_web::check_public_url` 公网闸（拒绝本机/内网/保留地址，
//!   仅 http/https）；截图落 AI_Gen_Files（产物目录纪律，模型可见可 ocr）；
//! - clipboard_write 限长（防把整份文档灌进剪贴板被别的应用粘贴出去）；
//! - copy_file_to_clipboard 同样走读闸（剪贴板内容的敏感度不高于读文件本身），
//!   平台实现复用 `platform::copy_file`（前端「复制文件 + 任务标题」同一套代码）；
//! - 截图依赖系统屏幕录制权限（macOS 未授权时出壁纸/黑图，工具描述与 README 明示）；
//! - 同步平台调用全部 spawn_blocking，不占 async worker。

use tauri::AppHandle;

use crate::bot::registry::ToolResult;

/// clipboard_write 文本长度上限（字符）
const CLIPBOARD_MAX_CHARS: usize = 100_000;

// reveal_path

/// 在访达/资源管理器中定位文件（不打开文件本身）。白名单闸与读文件一致。
pub async fn tool_reveal_path(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::denied("reveal_path 缺少 path 参数");
    };
    if path.is_empty() {
        return ToolResult::denied("reveal_path 的 path 不能为空");
    }
    let canonical =
        match crate::bot_fs::resolve_with_perm(app, "reveal_path", &path, interactive, session_id)
            .await
        {
            Ok(p) => p,
            Err(e) => return ToolResult::denied(e),
        };
    use tauri_plugin_opener::OpenerExt;
    let shown = canonical.display().to_string();
    match app.opener().reveal_item_in_dir(&canonical) {
        Ok(()) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "desktop.reveal | {}",
                    crate::bot::truncate_for_log(&shown, 200)
                ),
            );
            ToolResult::ok(format!("已在系统文件管理器中定位：{shown}"), Vec::new())
        }
        Err(e) => ToolResult::error(format!("定位失败：{e}"), Vec::new()),
    }
}

// open_url

/// open_url 准入门失败怎么落地成工具结果（纯函数，供测试钉住）。
///
/// 策略拒绝 = 换个地址也没用（Denied → 审计 WARN，预期内控制流）；
/// DNS 解析失败 = 网络故障（Error → 审计 ERROR）。两者混成一个 denied 会把
/// 真实故障降级成「换个 url 重试」，模型会在同一条死路上反复试。
fn open_url_gate_result(err: &crate::bot_web::UrlGateError) -> ToolResult {
    match err {
        crate::bot_web::UrlGateError::Denied { reason } => ToolResult::denied(reason.clone()),
        crate::bot_web::UrlGateError::Unresolved { reason } => {
            ToolResult::error(format!("打开失败：{reason}"), Vec::new())
        }
    }
}

/// 用系统默认浏览器打开网页。URL 准入门与 fetch_text 同一公网闸
/// （仅 http/https；DNS 解析后拒绝本机/内网/保留地址）。
/// 后台执行（无人值守）一律拒：窗口弹出来没人看，链接查询串还可能把数据
/// 带出去且不可见——「给用户看」的动作没有用户在场就没有意义。
pub async fn tool_open_url(app: &AppHandle, args: &str, interactive: bool) -> ToolResult {
    if !interactive {
        return ToolResult::denied(
            "后台执行不打开浏览器（无人值守时窗口没人看，链接查询串也可能带出数据）；请在用户在场时使用",
        );
    }
    let v = crate::bot::parse_args(args);
    let Some(u) = v["url"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::denied("open_url 缺少 url 参数");
    };
    if u.is_empty() {
        return ToolResult::denied("open_url 的 url 不能为空");
    }
    if let Err(e) = crate::bot::check_len(&u, crate::bot::MAX_KEYWORD, "网址") {
        return ToolResult::denied(e);
    }
    let url = match crate::bot_web::ensure_public_http_url(&u).await {
        Ok(url) => url,
        Err(e) => return open_url_gate_result(&e),
    };
    use tauri_plugin_opener::OpenerExt;
    let shown = url.as_str().to_string();
    match app.opener().open_url(&shown, None::<&str>) {
        Ok(()) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "desktop.open_url | {}",
                    crate::bot::escape_for_log(&shown, 200)
                ),
            );
            ToolResult::ok(format!("已在默认浏览器打开：{shown}"), Vec::new())
        }
        Err(e) => ToolResult::error(format!("打开失败：{e}"), Vec::new()),
    }
}

// clipboard_write

/// 剪贴板文本校验（纯函数，可测）：空串拒绝、超限拒绝（截断会静默丢内容，
/// 不如让模型自己分段复制）
fn validate_clip_text(text: &str) -> Result<String, String> {
    let t = text.trim_end_matches('\n');
    if t.trim().is_empty() {
        return Err("剪贴板内容不能为空".into());
    }
    let n = t.chars().count();
    if n > CLIPBOARD_MAX_CHARS {
        return Err(format!(
            "内容 {n} 字符超过剪贴板上限 {CLIPBOARD_MAX_CHARS}，请分段复制"
        ));
    }
    Ok(t.to_string())
}

/// 把文本写入系统剪贴板（官方 clipboard-manager 插件；写入会覆盖剪贴板原内容）
pub async fn tool_clipboard_write(app: &AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(text) = v["text"].as_str() else {
        return ToolResult::denied("clipboard_write 缺少 text 参数");
    };
    let text = match validate_clip_text(text) {
        Ok(t) => t,
        Err(e) => return ToolResult::denied(e),
    };
    let n = text.chars().count();
    // 模块约定「同步平台调用全部 spawn_blocking」：剪贴板写入触系统 pasteboard
    // （macOS 走 IPC），粘贴板慢/卡时不占 async worker——与截图路径同策略
    let app2 = app.clone();
    let written = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        app2.clipboard().write_text(text)
    })
    .await;
    match written {
        Ok(Ok(())) => {
            crate::bot::audit_log(app, &format!("desktop.clipboard_write | {} chars", n));
            ToolResult::ok(
                format!("已复制到剪贴板（{n} 字符），可直接粘贴"),
                Vec::new(),
            )
        }
        Ok(Err(e)) => ToolResult::error(format!("写入剪贴板失败：{e}"), Vec::new()),
        Err(e) => ToolResult::error(format!("失败：剪贴板线程异常：{e}"), Vec::new()),
    }
}

// copy_file_to_clipboard

/// 一次放进剪贴板的文件数上限：再多用户自己都点不清
const MAX_CLIP_FILES: usize = 20;

/// 从工具参数里取出待复制的路径（`path` 单个 / `paths` 多个，二者可只给其一）。
///
/// 纯函数，可测：去重保序、剔空串。返回空 Vec 表示参数没给路径。
fn collect_clip_paths(v: &serde_json::Value) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    if let Some(arr) = v["paths"].as_array() {
        for it in arr {
            if let Some(s) = it.as_str() {
                let t = s.trim().to_string();
                if !t.is_empty() {
                    raw.push(t);
                }
            }
        }
    }
    if let Some(s) = v["path"].as_str() {
        let t = s.trim().to_string();
        if !t.is_empty() {
            raw.push(t);
        }
    }
    // 去重保序：同一文件列两次只放一份（剪贴板里出现两份同一个文件没意义）
    let mut uniq: Vec<String> = Vec::new();
    for p in raw {
        if !uniq.contains(&p) {
            uniq.push(p);
        }
    }
    uniq
}

/// 成功审计行（纯函数，单测锚点）：路径拼进 free-form 审计行前过 escape_for_log
/// ——路径来自文件系统，换行/管道符不得伪造或撕裂审计行（与 open_url /
/// reveal_path 的成功审计行同惯例）
fn clip_audit_line(n: usize, shown: &str) -> String {
    format!(
        "desktop.copy_file_to_clipboard | {n} 个文件 | {}",
        crate::bot::escape_for_log(shown, 200)
    )
}

/// 把本地文件放进系统剪贴板：用户在任意应用里按 Cmd/Ctrl+V 就能把这个文件粘出去
/// （发微信 / 飞书附件的最短路径——机器人放好，用户自己粘）。
///
/// 路径走读闸（`bot_fs::resolve_with_perm`）：剪贴板内容的敏感度不高于读文件本身，
/// 故与读文件同一白名单；白名单外弹授权窗。平台实现复用 `platform::copy_file`
/// （前端「复制文件 + 任务标题」同一套 NSPasteboard / CF_HDROP 代码）。
pub async fn tool_copy_file_to_clipboard(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let raw = collect_clip_paths(&v);
    if raw.is_empty() {
        return ToolResult::denied(
            "copy_file_to_clipboard 缺少 path 参数（要复制多个文件用 paths 数组）",
        );
    }
    if raw.len() > MAX_CLIP_FILES {
        return ToolResult::denied(format!(
            "一次最多复制 {MAX_CLIP_FILES} 个文件（收到 {} 个），请分批复制",
            raw.len()
        ));
    }
    // 逐个过读闸：读白名单 = Desktop / Downloads / Documents + AI_Gen_Files +
    // 任务卡绑定目录 + 用户配置的 allowedDirs；白名单外按权限模式处理
    let mut resolved: Vec<String> = Vec::new();
    for p in &raw {
        match crate::bot_fs::resolve_with_perm(
            app,
            "copy_file_to_clipboard",
            p,
            interactive,
            session_id,
        )
        .await
        {
            Ok(c) => resolved.push(c.display().to_string()),
            Err(e) => return ToolResult::denied(e),
        }
    }
    // 文本类粘贴的载荷：文件名（多个用、分隔）
    let title = resolved
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.clone())
        })
        .collect::<Vec<_>>()
        .join("、");
    let shown = resolved.join("、");
    let n = resolved.len();
    // 模块约定「同步平台调用全部 spawn_blocking」：剪贴板写入走系统 IPC
    // （macOS pasteboard / Windows 全局内存），慢时不占 async worker
    let moved = resolved;
    let r = tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = moved.iter().map(|s| s.as_str()).collect();
        platform_copy_files(&refs, &title)
    })
    .await;
    match r {
        Ok(Ok(())) => {
            crate::bot::audit_log(app, &clip_audit_line(n, &shown));
            ToolResult::ok(
                format!(
                    "已放入系统剪贴板（{n} 个文件）：{shown}。可在微信 / 飞书等应用里直接粘贴出这些文件。"
                ),
                Vec::new(),
            )
        }
        Ok(Err(e)) => ToolResult::error(format!("放入剪贴板失败：{e}"), Vec::new()),
        Err(e) => ToolResult::error(format!("失败：剪贴板线程异常：{e}"), Vec::new()),
    }
}

/// 平台分发：把文件列表 + 文本载荷写进系统剪贴板。
#[cfg(target_os = "macos")]
fn platform_copy_files(paths: &[&str], title: &str) -> Result<(), String> {
    crate::platform::copy_file::copy_files_macos(paths, title)
}

#[cfg(target_os = "windows")]
fn platform_copy_files(paths: &[&str], title: &str) -> Result<(), String> {
    crate::platform::copy_file::copy_files_windows(paths, title).map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_copy_files(_paths: &[&str], _title: &str) -> Result<(), String> {
    Err("当前平台不支持把文件放进剪贴板".into())
}

// screenshot

/// 截图文件名（纯函数，可测）：wm-screen-YYYYMMDD-HHMMSS-mmm-nnnnnn.png。
/// 毫秒后缀防并行 tool_call 同秒互相覆盖；毫秒内纳秒偏置防同毫秒内撞名
fn screenshot_filename(ts_ms: i64, sub_ms_ns: u32) -> String {
    use chrono::TimeZone;
    let dt = chrono::Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .unwrap_or_else(chrono::Local::now);
    format!(
        "wm-screen-{}-{:03}-{:06}.png",
        dt.format("%Y%m%d-%H%M%S"),
        ts_ms.rem_euclid(1000),
        sub_ms_ns.rem_euclid(1_000_000)
    )
}

#[cfg(target_os = "macos")]
fn platform_capture(out: &std::path::Path) -> Result<(), String> {
    // 官方 CLI：-x 静音；无窗口参数 = 主显示器全屏。
    // 屏幕录制权限未授予时进程仍成功但产物是壁纸/黑图——调用方以文件存在 + 非零校验，
    // 权限问题靠工具描述与 README 提示用户授权。
    let status = std::process::Command::new("screencapture")
        .arg("-x")
        .arg(out)
        .status()
        .map_err(|e| format!("启动 screencapture 失败：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("screencapture 退出码 {status}"))
    }
}

#[cfg(target_os = "windows")]
fn platform_capture(out: &std::path::Path) -> Result<(), String> {
    // .NET 内置（零依赖）：VirtualScreen 全虚拟屏 → CopyFromScreen → PNG。
    // 路径插进单引号字符串：PowerShell 转义单引号靠双写（''），不能靠删除——
    // 删除会破坏含引号的合法路径，其他元字符也依旧裸奔。
    let out_s = out.display().to_string().replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms,System.Drawing; \
         $b=[System.Windows.Forms.SystemInformation]::VirtualScreen; \
         $bmp=New-Object System.Drawing.Bitmap $b.Width,$b.Height; \
         $g=[System.Drawing.Graphics]::FromImage($bmp); \
         $g.CopyFromScreen($b.Left,$b.Top,0,0,$bmp.Size); \
         $bmp.Save('{}',[System.Drawing.Imaging.ImageFormat]::Png)",
        out_s
    );
    // 静默拉起：GUI 宿主拉控制台程序默认会给它**新建一个控制台窗口**——
    // 用户桌面就会闪一个黑框。截屏在 UI 自动化里是「每个动作前后各一张」，
    // 不抑制窗口就是每次操作闪两次，还抢焦点。
    let status = crate::bot::procs::silent_cmd("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|e| format!("启动 PowerShell 失败：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("PowerShell 截屏退出码 {status}"))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_capture(_out: &std::path::Path) -> Result<(), String> {
    Err("当前平台不支持 screenshot".into())
}

/// 截屏到指定路径（供 UI 动作留痕复用）。返回产物是否非空。
///
/// 与 tool_screenshot 的区别：不写「模型可见」的文案，只落盘——UI 动作留痕要的是
/// 「动作前后各一张」，路径由调用方写进工具结果。
pub(crate) fn capture_to(out: &std::path::Path) -> bool {
    match platform_capture(out) {
        Ok(()) => std::fs::metadata(out).map(|m| m.len() > 0).unwrap_or(false),
        Err(_) => false,
    }
}

/// 截图成功的工具结果：文本 + 图片 + 落盘凭证。
///
/// 凭证（`file_changes`）不能少：PNG 是真落在 AI_Gen_Files 的文件产物，模型
/// 如实说「已生成截图 /path/x.png」时，落盘凭证闸要认得这个路径是工具产物——
/// 认不出就把真截出来的图判成凭空声称，反过来要求模型重截。
/// PNG 是二进制，只记路径与内容哈希（无行级 diff），与文档生成器同档。
fn screenshot_ok_result(out: &std::path::Path, size: u64) -> ToolResult {
    let path = out.display().to_string();
    ToolResult::ok_with_images(
        format!(
            "已截屏：{path}\n主显示器全屏 PNG（{size} 字节）。截图已作为图片附在本消息之后，请直接用视觉能力读取内容。"
        ),
        Vec::new(),
        vec![path.clone()],
    )
    .with_file_change(crate::bot_fs::build_binary_file_receipt(&path))
}

/// 截取主显示器画面 → AI_Gen_Files/wm-screen-<时间戳>.png，返回路径。
/// 模型拿到路径后可接着 ocr_image 读屏上文字（读屏上内容不需要视觉模型）。
pub async fn tool_screenshot(app: &AppHandle, _args: &str) -> ToolResult {
    // 纳秒时间戳防同毫秒并发同名覆盖（多核/线程池瞬间可撞同毫秒）
    let ts_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let ts_ms = if ts_ns != 0 {
        ts_ns / 1_000_000
    } else {
        crate::db::now_ms()
    };
    // 毫秒内纳秒偏置（0–999999）后缀在文件名尾，并发同名 = 唯一
    let sub_ms = if ts_ns != 0 {
        (ts_ns.rem_euclid(1_000_000)) as u32
    } else {
        0
    };
    let name = screenshot_filename(ts_ms, sub_ms);
    let out = match crate::db::gen_dir(app) {
        Ok(d) => d.join(&name),
        Err(e) => {
            return ToolResult::error(format!("产物目录不可用：{e}"), Vec::new());
        }
    };
    let out_for_task = out.clone();
    let r = tauri::async_runtime::spawn_blocking(move || platform_capture(&out_for_task)).await;
    match r {
        Ok(Ok(())) => {
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                return ToolResult::error(
                    "截图失败：产物文件为空（请检查屏幕录制权限）".to_string(),
                    Vec::new(),
                );
            }
            crate::bot::audit_log(
                app,
                &format!("desktop.screenshot | {} | {size} bytes", name),
            );
            // 图随 ToolResult 直接进对话（模型视觉读取），不再引导。
            screenshot_ok_result(&out, size)
        }
        Ok(Err(e)) => ToolResult::error(format!("截图失败：{e}"), Vec::new()),
        Err(e) => ToolResult::error(format!("失败：截图线程异常：{e}"), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本文件生产代码切片（测试模块之前）——源码文本守卫用。
    fn desktop_src_before_tests() -> &'static str {
        include_str!("bot_desktop.rs")
            .split_once("#[cfg(test)]")
            .expect("bot_desktop.rs 应有测试模块")
            .0
    }

    /// Windows 截屏不许直接 `Command::new` 拉 powershell。
    ///
    /// GUI 宿主拉起控制台程序，Windows 默认给它**新建一个控制台窗口**——用户桌面
    /// 闪一个黑框，还抢焦点。截屏在 UI 自动化里是「每个动作前后各一张」，
    /// 不抑制窗口就是**一次操作闪两次**（真实漏过一次，见附录 D-8）。
    #[test]
    fn windows_capture_must_not_spawn_a_visible_console() {
        let src = desktop_src_before_tests();
        assert!(
            !src.contains(r#"Command::new("powershell")"#),
            "截屏不许直接 Command::new(\"powershell\")：会弹控制台窗口（改走 bot::procs::silent_cmd）"
        );
        assert!(
            src.contains("silent_cmd("),
            "Windows 截屏应通过 bot::procs::silent_cmd 拉起（带 CREATE_NO_WINDOW）"
        );
    }

    /// open_url 的准入门失败分两级：**策略拒绝**（WARN）与 **DNS 解析失败**（ERROR）。
    ///
    /// 混成一个 denied 会把真实网络故障降级成「换个 url 重试」——模型看不到
    /// 「这是故障」，只会换个地址再来一轮。
    #[test]
    fn open_url_gate_failures_are_not_all_denied() {
        use crate::audit::AuditLevel;
        use crate::bot::registry::ToolStatus;
        use crate::bot_web::UrlGateError;

        let denied = open_url_gate_result(&UrlGateError::Denied {
            reason: "[web] 已拒绝访问本机/内网地址".to_string(),
        });
        assert_eq!(
            denied.status,
            ToolStatus::Denied,
            "策略拒绝是「工具没执行」，必须是 Denied"
        );
        assert_eq!(
            AuditLevel::from_tool_status(denied.status),
            AuditLevel::Warn,
            "策略拒绝是预期内控制流，审计记 WARN"
        );

        let unresolved = open_url_gate_result(&UrlGateError::Unresolved {
            reason: "[web] 域名解析失败：os error 11001".to_string(),
        });
        assert_eq!(
            unresolved.status,
            ToolStatus::Error,
            "DNS 解析失败是执行故障，必须是 Error（不能降级成 Denied）"
        );
        assert_eq!(
            AuditLevel::from_tool_status(unresolved.status),
            AuditLevel::Error,
            "DNS 解析失败必须记 ERROR——与同因的 fetch_url 同级"
        );
        assert!(
            unresolved.text.contains("域名解析失败"),
            "模型要看到真实原因：{}",
            unresolved.text
        );
    }

    #[test]
    fn clip_text_rejects_blank_and_over_limit() {
        assert!(validate_clip_text("   \n ").is_err(), "纯空白拒绝");
        assert_eq!(
            validate_clip_text("你好\n").unwrap(),
            "你好",
            "只去结尾换行"
        );
        let big = "A".repeat(CLIPBOARD_MAX_CHARS + 1);
        let err = validate_clip_text(&big).unwrap_err();
        assert!(err.contains("超过剪贴板上限"), "{err}");
        // 恰好等于上限放行
        let ok = "B".repeat(CLIPBOARD_MAX_CHARS);
        assert!(validate_clip_text(&ok).is_ok());
    }

    #[test]
    fn screenshot_filename_has_timestamp_shape() {
        // 固定时间戳 → wm-screen-YYYYMMDD-HHMMSS-mmm-nnnnnn.png
        let name = screenshot_filename(1_759_000_000_000, 0);
        let re = regex::Regex::new(r"^wm-screen-\d{8}-\d{6}-\d{3}-\d{6}\.png$").unwrap();
        assert!(re.is_match(&name), "{name}");
        // 同毫秒两截屏文件名必须不同（毫秒内纳秒偏置防并行覆盖）
        let next = screenshot_filename(1_759_000_000_000, 1);
        assert_ne!(name, next, "{name}");
    }

    /// 真实截屏冒烟（macOS，需屏幕录制授权；Windows 手动跑 PowerShell 分支）。
    /// 手动跑：cargo test --lib desktop:: -- --ignored
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "真实截屏冒烟（依赖屏幕录制授权，手动跑）"]
    fn macos_capture_smoke() {
        let out = std::env::temp_dir().join("wm-screen-smoke.png");
        platform_capture(&out).expect("screencapture 应成功");
        let size = std::fs::metadata(&out).expect("产物应存在").len();
        assert!(size > 0, "产物非空");
        std::fs::remove_file(&out).ok();
    }

    /// 路径收集：path 单个 / paths 多个，二者都给时合并；去重保序、剔空串。
    #[test]
    fn collect_clip_paths_merges_dedupes_and_trims() {
        let v = serde_json::json!({"path": " /a/b.txt ", "paths": ["/c/d.txt", "/a/b.txt", "  "]});
        assert_eq!(
            collect_clip_paths(&v),
            vec!["/c/d.txt".to_string(), "/a/b.txt".to_string()],
            "paths 在前、path 在后；重复项只留一份；空白项剔除"
        );
        // 只有 paths
        let only = serde_json::json!({"paths": ["/x", "/y"]});
        assert_eq!(collect_clip_paths(&only).len(), 2);
        // 一个都没给
        assert!(collect_clip_paths(&serde_json::json!({})).is_empty());
        // 全是空白
        assert!(collect_clip_paths(&serde_json::json!({"path": "  "})).is_empty());
    }

    /// 成功审计行：路径过 escape_for_log——路径里的换行/管道符不得撕裂审计行，
    /// 超长路径按 200 字符截断（与 open_url / reveal_path 同惯例）。
    #[test]
    fn clip_audit_line_escapes_and_truncates_path() {
        let line = clip_audit_line(2, "/tmp/a\nb | c.txt");
        assert!(!line.contains('\n'), "路径换行必须被转义: {line:?}");
        assert!(line.contains("\\nb"), "换行落成 \\n 短形式: {line:?}");
        assert!(!line.contains(" | c.txt"), "管道符必须被倍增转义: {line:?}");
        assert!(
            line.starts_with("desktop.copy_file_to_clipboard | 2 个文件 | "),
            "{line:?}"
        );
        // 超长路径：转义后按 200 字符截断加省略号，不整条塞进日志
        let long = "/x".repeat(300);
        let truncated = clip_audit_line(1, &long);
        let prefix = "desktop.copy_file_to_clipboard | 1 个文件 | ";
        assert!(
            truncated.chars().count() <= prefix.chars().count() + 201,
            "超长路径必须截断: {}",
            truncated.chars().count()
        );
        assert!(truncated.ends_with('…'), "截断带省略号: {truncated:?}");
    }

    /// 截图落盘凭证：成功结果必须带一条 file_changes（路径 + 内容哈希）。
    /// 缺了它，模型如实说「已生成截图 /path/x.png」会被落盘凭证闸判成凭空声称，
    /// 反过来要求模型重截——真截出来的图被当成编造。
    #[test]
    fn screenshot_result_carries_a_file_change_receipt() {
        let dir = std::env::temp_dir().join(format!("wm-shot-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("wm-screen-test.png");
        std::fs::write(&out, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let r = screenshot_ok_result(&out, 8);
        assert_eq!(r.status, crate::bot::registry::ToolStatus::Ok);
        assert_eq!(r.images.len(), 1, "截图要作为图片附在结果后");
        assert_eq!(r.file_changes.len(), 1, "截图是文件产物，必须带落盘凭证");
        let fc = &r.file_changes[0];
        assert_eq!(fc.path, out.display().to_string());
        assert_eq!(fc.kind, "create");
        assert!(fc.after_sha.is_some(), "凭证要带内容哈希");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 接线守卫：copy_file_to_clipboard 必须真的接进工具表（改了工具名/接线而
    /// 忘了登记，这里会红）。
    #[test]
    fn copy_file_to_clipboard_is_wired_into_registry() {
        let src = include_str!("bot/registry/schemas.rs");
        assert!(
            src.contains("name: \"copy_file_to_clipboard\""),
            "copy_file_to_clipboard 必须登记进 TOOLS_TABLE"
        );
        assert!(
            src.contains("SCHEMA_COPY_FILE_TO_CLIPBOARD"),
            "copy_file_to_clipboard 必须有 schema 常量"
        );
    }

    /// 真实剪贴板冒烟（macOS）：两个临时文件写进系统剪贴板，再读回校验——
    /// 文本载荷是标题，板上含 public.file-url（Finder / 微信认这个类型）。
    /// 会覆盖用户剪贴板，故默认忽略；手动跑：
    /// cargo test --lib desktop::macos_clipboard_file_smoke -- --ignored
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "真实剪贴板冒烟（会覆盖用户剪贴板，手动跑）"]
    fn macos_clipboard_file_smoke() {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

        let dir =
            std::env::temp_dir().join(format!("wm-clip-smoke-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();

        let paths = [
            a.to_str().unwrap().to_string(),
            b.to_str().unwrap().to_string(),
        ];
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        platform_copy_files(&refs, "a.txt、b.txt").expect("写入剪贴板应成功");

        let pb = NSPasteboard::generalPasteboard();
        // SAFETY: NSPasteboardTypeString 是 Foundation 公开常量，值稳定不释放、全局唯一无别名风险。
        let text = pb
            .stringForType(unsafe { NSPasteboardTypeString })
            .expect("板上应有文本载荷");
        assert_eq!(text.to_string(), "a.txt、b.txt");
        let types = pb.types().expect("板上应有类型列表");
        assert!(
            types.iter().any(|t| t.to_string() == "public.file-url"),
            "板上应含 public.file-url 类型（Finder / 微信靠它认出这是文件）"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
