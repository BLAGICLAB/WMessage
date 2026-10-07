//! 电脑辅助 Tier1 工具（N4）：reveal_path / open_url / clipboard_write / screenshot。
//!
//! 定位：只「看」与「打开」，不做鼠标键盘/UI 自动化（分层决策见
//! docs/batches/N4-DESKTOP-TIER1.spec.md 出界说明——原生 Computer Use 需要产品
//! 拍板 + VM 隔离，官方指引明确不建议在主机裸跑）。
//!
//! 安全边界（对齐官方 best practice）：
//! - reveal_path 走 `bot_fs::resolve_with_perm`，与读文件同一白名单闸；
//! - open_url 复用 `bot_web::check_public_url` 公网闸（拒绝本机/内网/保留地址，
//!   仅 http/https）；截图落 AI_Gen_Files（产物目录纪律，模型可见可 ocr）；
//! - clipboard_write 限长（防把整份文档灌进剪贴板被别的应用粘贴出去）；
//! - 截图依赖系统屏幕录制权限（macOS 未授权时出壁纸/黑图，工具描述与 README 明示）；
//! - 同步平台调用全部 spawn_blocking，不占 async worker。

use tauri::AppHandle;

use crate::bot::registry::ToolResult;

/// clipboard_write 文本长度上限（字符）
const CLIPBOARD_MAX_CHARS: usize = 100_000;

// ───────────────────────── reveal_path ─────────────────────────

/// 在访达/资源管理器中定位文件（不打开文件本身）。白名单闸与读文件一致。
pub async fn tool_reveal_path(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(path) = v["path"].as_str().map(|s| s.trim().to_string()) else {
        // 「reveal_path 缺少 path」首字「r」非 error/warn 前缀 → ok
        return ToolResult::ok("reveal_path 缺少 path 参数".to_string(), Vec::new());
    };
    if path.is_empty() {
        return ToolResult::ok("reveal_path 的 path 不能为空".to_string(), Vec::new());
    }
    let canonical =
        match crate::bot_fs::resolve_with_perm(app, "reveal_path", &path, interactive, session_id)
            .await
        {
            Ok(p) => p,
            // resolve_with_perm Err 返 String，首字不定 → ok
            Err(e) => return ToolResult::ok(e, Vec::new()),
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
            // 「已在访达显示」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(format!("已在系统文件管理器中定位：{shown}"), Vec::new())
        }
        Err(e) => {
            // 「定位失败：」首字「定」非 error/warn 前缀 → ok
            ToolResult::ok(format!("定位失败：{e}"), Vec::new())
        }
    }
}

// ───────────────────────── open_url ─────────────────────────

/// 用系统默认浏览器打开网页。URL 准入门与 fetch_text 同一公网闸
/// （仅 http/https；DNS 解析后拒绝本机/内网/保留地址）。
pub async fn tool_open_url(app: &AppHandle, args: &str) -> ToolResult {
    let v = crate::bot::parse_args(args);
    let Some(u) = v["url"].as_str().map(|s| s.trim().to_string()) else {
        // 「open_url 缺少 url」首字「o」非 error/warn 前缀 → ok
        return ToolResult::ok("open_url 缺少 url 参数".to_string(), Vec::new());
    };
    if u.is_empty() {
        return ToolResult::ok("open_url 的 url 不能为空".to_string(), Vec::new());
    }
    if let Err(e) = crate::bot::check_len(&u, crate::bot::MAX_KEYWORD, "网址") {
        return ToolResult::ok(e.to_string(), Vec::new());
    }
    let url = match crate::bot_web::ensure_public_http_url(&u).await {
        Ok(url) => url,
        // 错误文案首字不定（含「无效/仅支持/拒绝」等）→ ok
        Err(e) => return ToolResult::ok(e, Vec::new()),
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
            // 「已在浏览器打开」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(format!("已在默认浏览器打开：{shown}"), Vec::new())
        }
        // 「打开失败：」首字「打」非 error/warn 前缀 → ok
        Err(e) => ToolResult::ok(format!("打开失败：{e}"), Vec::new()),
    }
}

// ───────────────────────── clipboard_write ─────────────────────────

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
        // 「clipboard_write 缺少 text」首字「c」非 error/warn 前缀 → ok
        return ToolResult::ok("clipboard_write 缺少 text 参数".to_string(), Vec::new());
    };
    let text = match validate_clip_text(text) {
        Ok(t) => t,
        // 错误文案首字「剪/内」非 error/warn 前缀 → ok
        Err(e) => return ToolResult::ok(e, Vec::new()),
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
            // 「已复制」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok(
                format!("已复制到剪贴板（{n} 字符），可直接粘贴"),
                Vec::new(),
            )
        }
        // 「写入剪贴板失败：」首字「写」非 error/warn 前缀 → ok
        Ok(Err(e)) => ToolResult::ok(format!("写入剪贴板失败：{e}"), Vec::new()),
        // 「失败：剪贴板线程异常」以「失败」开头 → error（同截图线程异常口径）
        Err(e) => ToolResult::error(format!("失败：剪贴板线程异常：{e}"), Vec::new()),
    }
}

// ───────────────────────── screenshot ─────────────────────────

/// 截图文件名（纯函数，可测）：wm-screen-YYYYMMDD-HHMMSS.png
fn screenshot_filename(ts_ms: i64) -> String {
    use chrono::TimeZone;
    let dt = chrono::Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .unwrap_or_else(chrono::Local::now);
    format!("wm-screen-{}.png", dt.format("%Y%m%d-%H%M%S"))
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
    let status = std::process::Command::new("powershell")
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

/// 截取主显示器画面 → AI_Gen_Files/wm-screen-<时间戳>.png，返回路径。
/// 模型拿到路径后可接着 ocr_image 读屏上文字（读屏上内容不需要视觉模型）。
pub async fn tool_screenshot(app: &AppHandle, _args: &str) -> ToolResult {
    let ts = chrono::Utc::now().timestamp_millis();
    let name = screenshot_filename(ts);
    let out = match crate::db::gen_dir(app) {
        Ok(d) => d.join(&name),
        Err(e) => {
            // 「产物目录不可用：」首字「产」非 error/warn 前缀 → ok
            return ToolResult::ok(format!("产物目录不可用：{e}"), Vec::new());
        }
    };
    let out_for_task = out.clone();
    let r = tauri::async_runtime::spawn_blocking(move || platform_capture(&out_for_task)).await;
    match r {
        Ok(Ok(())) => {
            let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                // 「截图失败：产物为空」首字「截」非 error/warn 前缀 → ok
                return ToolResult::ok(
                    "截图失败：产物文件为空（请检查屏幕录制权限）".to_string(),
                    Vec::new(),
                );
            }
            crate::bot::audit_log(
                app,
                &format!("desktop.screenshot | {} | {size} bytes", name),
            );
            // N5：图随 ToolResult 直接进对话（模型视觉读取），不再引导 OCR 中转。
            // 「已截屏」首字「已」非 error/warn 前缀 → ok
            ToolResult::ok_with_images(
                format!(
                    "已截屏：{}\n主显示器全屏 PNG（{size} 字节）。截图已作为图片附在本消息之后，请直接用视觉能力读取内容。",
                    out.display()
                ),
                Vec::new(),
                vec![out.display().to_string()],
            )
        }
        Ok(Err(e)) => ToolResult::ok(format!("截图失败：{e}"), Vec::new()),
        // 「失败：截图线程异常」以「失败」开头 → error
        Err(e) => ToolResult::error(format!("失败：截图线程异常：{e}"), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 固定时间戳 → wm-screen-YYYYMMDD-HHMMSS.png
        let name = screenshot_filename(1_759_000_000_000);
        let re = regex::Regex::new(r"^wm-screen-\d{8}-\d{6}\.png$").unwrap();
        assert!(re.is_match(&name), "{name}");
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
}
