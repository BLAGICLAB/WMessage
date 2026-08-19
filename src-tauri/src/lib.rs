// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod api;
mod api_auth;
mod api_handlers;
mod api_server;
mod audit;
pub mod bot;
mod bot_chat;
mod bot_model_loop;
mod bot_py;
mod bot_scheduler;
pub mod bot_skills;
mod bot_slash;
mod bot_web;
mod db;
pub mod error;
pub mod intent_router;
pub mod middleware;
mod migration;
mod profile;
pub mod task_out;
pub mod tool_guard;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// 复制文件 + 任务标题到剪贴板：
/// - 文件类粘贴（Finder / 飞书 / 微信）得到真实文件
/// - 文本类粘贴得到任务标题
#[tauri::command]
fn copy_file_with_title(path: String, title: String) -> error::CommandResult<()> {
    #[cfg(target_os = "macos")]
    {
        // F2（Phase 6b）：平台 helper 仍为 Result<(), String>，经 From<String> → Internal 转换
        return copy_file_macos(&path, &title).map_err(error::CommandError::from);
    }
    #[cfg(windows)]
    {
        return copy_file_windows(&path, &title).map_err(error::CommandError::from);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (path, title);
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err(error::CommandError::Internal(
            "复制文件暂不支持当前平台".into(),
        ));
    }
}

/// 多文件复制 + 任务标题（2026-08-19 多文件绑定配套）：
/// - 文件类粘贴得到全部真实文件
/// - 文本类粘贴逐行得到 `{title}-{basename}`；单文件调用方仍走 copy_file_with_title（行为不变）
#[tauri::command]
fn copy_files_with_title(paths: Vec<String>, title: String) -> error::CommandResult<()> {
    if paths.is_empty() {
        return Err(error::CommandError::Internal(
            "复制文件列表为空".into(),
        ));
    }
    // 单文件直接复用旧路径，行为与既往完全一致
    if paths.len() == 1 {
        return copy_file_with_title(paths.into_iter().next().unwrap(), title);
    }
    let text = paths
        .iter()
        .map(|p| {
            let base = std::path::Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| p.clone());
            format!("{title}-{base}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    #[cfg(target_os = "macos")]
    {
        return copy_files_macos(&paths, &text).map_err(error::CommandError::from);
    }
    #[cfg(windows)]
    {
        return copy_files_windows(&paths, &text).map_err(error::CommandError::from);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (paths, text);
        return Err(error::CommandError::Internal(
            "复制文件暂不支持当前平台".into(),
        ));
    }
}

/// 唤起主窗口并强制置顶（单一真相）
///
/// 老板 2026-08-17 11:31/11:43 规则：所有唤起主窗口的路径（widget 双击标题、聊天区 📌、
/// 全局快捷键、托盘点击/菜单）都必须把主窗口推到桌面屏幕最顶层才能看见。
/// 仅 setFocus 在 Windows 上不一定推到 z-order 最顶层（其他窗口抢焦点时被遮住），
/// 需 alwaysOnTop 短暂闪烁 80ms 强制重排后再恢复（不长驻，避免干扰用户正常使用电脑）。
/// P2-26：80ms 等待挪到后台线程 —— 原先主线程 sleep(80ms)，全局快捷键/托盘点击
/// 处理全在主线程，UI 被冻结 80ms。
/// 泛型 Runtime（NEW-D-6 先例）：mock runtime 可直测不阻塞语义。
pub fn bring_main_to_front<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    let _ = window.set_always_on_top(true);
    let w = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(80));
        let _ = w.set_always_on_top(false);
    });
}

/// Tauri 命令包装：供前端 src/focus.ts 调用，与 4 个 Rust 内部调用点同逻辑
#[tauri::command]
fn focus_main_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        bring_main_to_front(&w);
    }
}

#[cfg(target_os = "macos")]
fn copy_file_macos(path: &str, title: &str) -> Result<(), String> {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    // 1) 现代文件类型（public.file-url）：Finder / 原生应用读这个
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let url_obj: objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>> =
        ProtocolObject::from_retained(url);
    let objs = NSArray::from_retained_slice(&[url_obj]);
    let _ok = pb.writeObjects(&objs);

    // 2) 老式文件列表类型（NSFilenamesPboardType）：Electron 系应用（飞书等）读这个
    let path_str = NSString::from_str(path);
    let paths = NSArray::from_retained_slice(&[path_str]);
    let _ok =
        unsafe { pb.setPropertyList_forType(&paths, &NSString::from_str("NSFilenamesPboardType")) };

    // 3) 标题文本：文本应用粘贴即标题
    let text = NSString::from_str(title);
    let _ok = pb.setString_forType(&text, unsafe { NSPasteboardTypeString });
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_files_macos(paths: &[String], text: &str) -> Result<(), String> {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();

    // 1) 现代文件类型（public.file-url）：一次写入全部文件 URL
    let urls: Vec<objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
        .iter()
        .map(|p| {
            let url = NSURL::fileURLWithPath(&NSString::from_str(p));
            ProtocolObject::from_retained(url)
        })
        .collect();
    let objs = NSArray::from_retained_slice(&urls);
    let _ok = pb.writeObjects(&objs);

    // 2) 老式文件列表类型（NSFilenamesPboardType）：Electron 系应用（飞书等）读这个
    let path_strs: Vec<objc2::rc::Retained<NSString>> =
        paths.iter().map(|p| NSString::from_str(p)).collect();
    let paths_arr = NSArray::from_retained_slice(&path_strs);
    let _ok = unsafe {
        pb.setPropertyList_forType(&paths_arr, &NSString::from_str("NSFilenamesPboardType"))
    };

    // 3) 文本：逐行 `{title}-{basename}`（调用方已拼好）
    let text = NSString::from_str(text);
    let _ok = pb.setString_forType(&text, unsafe { NSPasteboardTypeString });
    Ok(())
}

#[cfg(windows)]
fn copy_files_windows(paths: &[String], text: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
    use windows::Win32::UI::Shell::DROPFILES;

    unsafe {
        if OpenClipboard(None).is_err() {
            return Err("打开剪贴板失败".into());
        }
        let _ = EmptyClipboard();

        // 1) 文件列表（CF_HDROP）：多路径逐个 null 分隔，整体双 null 结尾
        let files_wide: Vec<u16> = paths
            .iter()
            .flat_map(|p| OsStr::new(p).encode_wide().chain(Some(0)))
            .chain(Some(0))
            .collect();
        let total = size_of::<DROPFILES>() + files_wide.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, total).map_err(|e| e.to_string())?;
        let base = GlobalLock(h) as *mut u8;
        if base.is_null() {
            let _ = GlobalFree(Some(h));
            let _ = CloseClipboard();
            return Err("锁定文件列表内存失败".into());
        }
        let drop: *mut DROPFILES = base as *mut DROPFILES;
        (*drop).pFiles = size_of::<DROPFILES>() as u32;
        (*drop).fWide = true.into();
        let dst = base.add(size_of::<DROPFILES>()) as *mut u16;
        ptr::copy_nonoverlapping(files_wide.as_ptr(), dst, files_wide.len());
        let _ = GlobalUnlock(h);
        if SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
            let _ = CloseClipboard();
            return Err("写入文件列表失败".into());
        }

        // 2) 标题文本（CF_UNICODETEXT）
        let title_wide: Vec<u16> = OsStr::new(text).encode_wide().chain(Some(0)).collect();
        let th = GlobalAlloc(GMEM_MOVEABLE, title_wide.len() * 2).map_err(|e| e.to_string())?;
        let tbase = GlobalLock(th) as *mut u16;
        if tbase.is_null() {
            let _ = GlobalFree(Some(th));
            let _ = CloseClipboard();
            return Err("锁定标题内存失败".into());
        }
        ptr::copy_nonoverlapping(title_wide.as_ptr(), tbase, title_wide.len());
        let _ = GlobalUnlock(th);
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(th.0))).is_err() {
            let _ = GlobalFree(Some(th));
        }

        let _ = CloseClipboard();
    }
    Ok(())
}

#[cfg(windows)]
fn copy_file_windows(path: &str, title: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
    use windows::Win32::UI::Shell::DROPFILES;

    unsafe {
        if OpenClipboard(None).is_err() {
            // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
            return Err("打开剪贴板失败".into());
        }
        let _ = EmptyClipboard();

        // 1) 文件列表（CF_HDROP）
        let file_wide: Vec<u16> = OsStr::new(path).encode_wide().chain(Some(0)).collect();
        let total = size_of::<DROPFILES>() + file_wide.len() * 2 + 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, total).map_err(|e| e.to_string())?;
        let base = GlobalLock(h) as *mut u8;
        if base.is_null() {
            let _ = GlobalFree(Some(h));
            let _ = CloseClipboard();
            // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
            return Err("锁定文件列表内存失败".into());
        }
        let drop: *mut DROPFILES = base as *mut DROPFILES;
        (*drop).pFiles = size_of::<DROPFILES>() as u32;
        (*drop).fWide = true.into();
        let dst = base.add(size_of::<DROPFILES>()) as *mut u16;
        ptr::copy_nonoverlapping(file_wide.as_ptr(), dst, file_wide.len());
        *dst.add(file_wide.len()) = 0; // 双 null 结尾
        let _ = GlobalUnlock(h);
        if SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0))).is_err() {
            let _ = GlobalFree(Some(h));
            let _ = CloseClipboard();
            // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
            return Err("写入文件列表失败".into());
        }

        // 2) 标题文本（CF_UNICODETEXT）
        let title_wide: Vec<u16> = OsStr::new(title).encode_wide().chain(Some(0)).collect();
        let th = GlobalAlloc(GMEM_MOVEABLE, title_wide.len() * 2).map_err(|e| e.to_string())?;
        let tbase = GlobalLock(th) as *mut u16;
        if tbase.is_null() {
            let _ = GlobalFree(Some(th));
            let _ = CloseClipboard();
            // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
            return Err("锁定标题内存失败".into());
        }
        ptr::copy_nonoverlapping(title_wide.as_ptr(), tbase, title_wide.len());
        let _ = GlobalUnlock(th);
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(th.0))).is_err() {
            let _ = GlobalFree(Some(th));
        }

        let _ = CloseClipboard();
    }
    Ok(())
}

/// P2-24：退出前统一清理（macOS Cmd+Q / Windows 托盘「退出」都走 RunEvent::ExitRequested）。
/// 原先只销毁主窗口：API 服务线程、SSE writer、活动 Skill、在途 Python 子进程全部
/// 随进程强退变孤儿。顺序：停 API（不再接新请求；G1 accept + SSE writer 全 join，
/// 保留 api-enabled.flag 供下次启动自动恢复）→ 终止活动 Skill → 按注册表杀在途
/// Python 整树 → 结构化审计。
/// 泛型 Runtime（与 NEW-D-6 同先例）：mock runtime 可直测全链路。
fn cleanup_on_exit<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    cleanup_on_exit_with(app, bot_py::kill_all_py_children);
}

/// 可测内核（P2-24）：kill_py 注入 —— 测试不传全局 kill_all（会把并行测试
/// 注册的在途子进程一起杀掉），生产固定接 bot_py::kill_all_py_children。
fn cleanup_on_exit_with<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    kill_py: impl FnOnce() -> usize,
) {
    let api_stopped = match app.try_state::<api_server::ApiState>() {
        Some(state) => api_handlers::api_stop_for_exit(app, &state).is_ok(),
        None => false,
    };
    bot_skills::skill_terminate_all(app, "应用退出");
    let py_killed = kill_py();
    audit::write_event(
        app,
        audit::AuditLevel::Info,
        "app_exit_cleanup",
        &[
            ("api_stopped", api_stopped.to_string()),
            ("py_killed", py_killed.to_string()),
        ],
    );
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 全局快捷键修饰键：macOS 用 Cmd+Ctrl（避开 Cmd+Shift+N 与 Finder 新建文件夹冲突），
    // Windows/Linux 用 Ctrl+Alt（避开 Ctrl+Shift+N 与浏览器隐身窗口冲突）
    let hotkey_mods = if cfg!(target_os = "macos") {
        Modifiers::SUPER | Modifiers::CONTROL
    } else {
        Modifiers::CONTROL | Modifiers::ALT
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let Some(main) = app.get_webview_window("main") else {
                        return;
                    };
                    match shortcut.key {
                        // 唤起/隐藏主窗口（macOS: Cmd+Ctrl+W；Win/Linux: Ctrl+Alt+W）
                        Code::KeyW => {
                            if main.is_visible().unwrap_or(false) {
                                let _ = main.hide();
                            } else {
                                bring_main_to_front(&main);
                            }
                        }
                        // 唤起主窗口 + 快速新建任务（macOS: Cmd+Ctrl+N；Win/Linux: Ctrl+Alt+N）
                        Code::KeyN => {
                            bring_main_to_front(&main);
                            let _ = app.emit("quick-add", ());
                        }
                        // 全局切换深/浅色主题（macOS: Cmd+Ctrl+T；Win/Linux: Ctrl+Alt+T）。
                        // 只发给主窗口，由它写 localStorage，挂件靠 storage 事件同步，避免双窗口互相触发。
                        Code::KeyT => {
                            let _ = app.emit_to("main", "toggle-theme", ());
                        }
                        _ => {}
                    }
                })
                .build(),
        )
        .setup(move |app| {
            // 本地 HTTP API 状态（默认关闭，设置页开关控制）
            app.manage(api_server::ApiState::default());
            // F-2 中间件注册表（Plugin/Extension 抽象层 P2）：注册 2 个内置中间件
            app.manage(middleware::build_default_registry());

            // 定时任务卡调度器：每 30s 扫一次到点任务并自动执行
            bot_scheduler::start_scheduler(app.handle().clone());

            // 开关持久化：上次退出前 API 开启过，则自动恢复（写 api-enabled.flag）
            {
                let handle = app.handle().clone();
                if api_auth::should_autostart(&handle) {
                    let state = app.state::<api_server::ApiState>();
                    if let Err(e) = api_handlers::api_start(handle, state) {
                        eprintln!("[api] auto-start failed: {e}");
                    }
                }
            }

            // 旧版本明文 key 迁移：bot-config.json 里的 apiKey → 系统凭据存储
            {
                let handle = app.handle().clone();
                if let Err(e) = bot::migrate_legacy_key(&handle) {
                    eprintln!("[bot] legacy key migration failed: {e}");
                }
            }

            // 桌面清理：后台轮询线程（每 10 分钟检测到期归档任务并执行规则迁移）
            {
                let handle = app.handle().clone();
                migration::spawn_polling(handle);
            }

            // P2-9：清扫残留的 py-runs 临时目录（spawn 失败/崩溃遗留，超 1 小时即删）
            bot_py::sweep_stale_py_runs(app.handle());

            // M4：系统侧边磁吸挂件窗口（贴边收起为触发条，悬停滑出）
            tauri::WebviewWindowBuilder::new(
                app,
                "widget",
                tauri::WebviewUrl::App("index.html#/widget".into()),
            )
            .title("WMessage 挂件")
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .inner_size(44.0, 220.0)
            .build()?;

            // M5 全局快捷键：唤起/隐藏主窗口 + 快速新建任务（键位见 run() 顶部注释）。
            // 容错：快捷键被其他应用占用时只记日志，绝不让 App 启动失败（审计 P2）
            let shortcut = app.global_shortcut();
            for (i, key) in [Code::KeyW, Code::KeyN, Code::KeyT].iter().enumerate() {
                if let Err(e) = shortcut.register(Shortcut::new(Some(hotkey_mods), *key)) {
                    eprintln!(
                        "[shortcut] 注册快捷键 {} 失败（可能被其他应用占用）：{e}",
                        ["W（唤起主窗口）", "N（快速新建）", "T（切换主题）"][i]
                    );
                }
            }

            // 主窗口「关闭」改为隐藏（挂件随时能唤起它，否则关闭后挂件无法打开主窗口）。
            // 真正退出：macOS 走 Cmd+Q（见下方 RunEvent::ExitRequested）；
            // Windows 走托盘右键「退出」（见下方托盘菜单）。
            if let Some(main) = app.get_webview_window("main") {
                let main2 = main.clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = main2.hide();
                    }
                });
            }

            // 系统托盘（P2-31 三平台：原仅 cfg(windows)，Linux/macOS 无托盘；tauri 2
            // tray-icon feature 已三平台支持）：#3 图标，右键菜单「打开主窗口 / 退出」，
            // 左键单击/双击恢复主窗口。Linux 需 libappindicator，缺失时初始化失败 ——
            // 降级为无托盘仅记日志，绝不让 App 启动失败（与快捷键注册容错同策略，审计 P2）。
            {
                use tauri::image::Image;
                use tauri::menu::{MenuBuilder, MenuItemBuilder};
                use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};

                let build_tray = || -> tauri::Result<()> {
                    let open = MenuItemBuilder::with_id("tray_open", "打开主窗口").build(&*app)?;
                    let quit = MenuItemBuilder::with_id("tray_quit", "退出").build(&*app)?;
                    let menu = MenuBuilder::new(&*app).items(&[&open, &quit]).build()?;

                    let icon = Image::from_bytes(include_bytes!("../icons/tray-wm-32.png"))?;

                    TrayIconBuilder::with_id("main-tray")
                        .icon(icon)
                        .tooltip("WMessage")
                        .menu(&menu)
                        .show_menu_on_left_click(false)
                        .on_menu_event(|app, event| match event.id().as_ref() {
                            "tray_open" => {
                                if let Some(w) = app.get_webview_window("main") {
                                    bring_main_to_front(&w);
                                }
                            }
                            "tray_quit" => {
                                app.exit(0);
                            }
                            _ => {}
                        })
                        .on_tray_icon_event(|tray, event| match event {
                            TrayIconEvent::Click {
                                button: MouseButton::Left,
                                ..
                            }
                            | TrayIconEvent::DoubleClick {
                                button: MouseButton::Left,
                                ..
                            } => {
                                if let Some(w) = tray.app_handle().get_webview_window("main") {
                                    bring_main_to_front(&w);
                                }
                            }
                            _ => {}
                        })
                        .build(&*app)?;
                    Ok(())
                };
                if let Err(e) = build_tray() {
                    eprintln!("[tray] 系统托盘初始化失败（Linux 可能缺 libappindicator）：{e}");
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            copy_file_with_title,
            copy_files_with_title,
            focus_main_window,
            bot_skills::open_file_path,
            bot_skills::pick_files_dialog,
            bot_skills::delete_bound_file,
            db::bind_files,
            db::bind_file,
            db::db_load,
            db::db_upsert,
            db::db_delete,
            db::db_merge,
            db::tasks_export,
            db::tasks_import,
            db::workspace_load,
            db::workspace_upsert,
            db::workspace_delete,
            db::bot_history_load,
            db::bot_history_save,
            db::bot_history_clear,
            db::bot_sessions_load,
            db::bot_session_create,
            db::bot_session_delete,
            db::bot_session_rename,
            api_handlers::api_start,
            api_handlers::api_stop,
            api_handlers::api_status,
            api_handlers::api_rotate_token,
            bot_slash::bot_get_enabled,
            bot_slash::bot_set_enabled,
            bot::bot_get_config,
            bot::bot_set_config,
            bot::bot_clear_api_key,
            bot_chat::bot_chat,
            bot_chat::bot_execute_task,
            bot_slash::bot_stop,
            bot_chat::bot_compact,
            bot_slash::bot_confirm_response,
            bot::bot_log_read,
            bot_py::py_get_enabled,
            bot_py::py_set_enabled,
            bot_py::py_env_check,
            bot_skills::skills_list,
            bot_skills::skills_import,
            bot_skills::skills_delete,
            bot_skills::skills_open_dir,
            profile::profile_get,
            profile::profile_set_name,
            profile::profile_set_avatar,
            profile::profile_remove_avatar,
            migration::migration_rules_load,
            migration::migration_rules_import,
            migration::migration_rules_template_save,
            migration::migration_log_read,
            migration::migration_run,
            migration::migration_status
        ])
        .build(tauri::generate_context!())
        // 启动期 panic 可接受（进程起不来就退）：Tauri builder 编译失败 = 环境/配置损坏，
        // 此时进程本来就该退出，让 OS 接管并提示用户。无业务热路径，无替代转换面。
        .expect("error while building tauri application")
        .run(|app, event| {
            // 真退出入口：macOS Cmd+Q / Windows 托盘右键「退出」（app.exit(0)）都会走到这里。
            // 主窗口 CloseRequested 被上面 prevent（改成隐藏），这里直接销毁主窗口，
            // 让退出流程正常走完。
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // P2-24：先清理资源（API server / 活动 Skill / 在途 Python 子进程），
                // 再销毁主窗口 —— 原先只 destroy，子进程与服务线程全部变孤儿
                cleanup_on_exit(app);
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.destroy();
                }
            }
        });
}

#[cfg(test)]
mod f2_copy_file_tests {
    /// F2（Phase 6b）：copy_file_with_title 的平台 helper 仍产 String 错误，
    /// 经 From<String> → CommandError::Internal 转换后 code 稳定、message 不丢。
    /// （macOS 分支直写 NSPasteboard，单测不碰真实剪贴板，只覆盖转换层。）
    #[test]
    fn string_error_converts_to_internal_with_message_preserved() {
        let helper: Result<(), String> = Err("打开剪贴板失败".into());
        let err = helper.map_err(crate::error::CommandError::from).unwrap_err();
        assert_eq!(err.code(), "INTERNAL");
        assert!(err.message().contains("打开剪贴板失败"));
    }
}

#[cfg(test)]
mod p2_30_capability_tests {
    /// P2-30：opener:allow-open-path 不得裸 "**" 通配（bot prompt 注入可诱导
    /// 打开任意路径）。收敛为 $APPDATA/** + $HOME/**：数据目录（exports/skills）
    /// 与用户主目录内文件放行，/etc/passwd 等系统路径默认拒绝。
    /// 本测试锁死 capabilities/default.json 防回退。
    #[test]
    fn opener_open_path_scope_is_not_bare_wildcard() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/capabilities/default.json"
        );
        let text = std::fs::read_to_string(path).expect("capabilities/default.json 必须可读");
        let json: serde_json::Value = serde_json::from_str(&text).expect("必须是合法 JSON");
        let perms = json["permissions"].as_array().expect("permissions 必须是数组");
        // 找到 opener:allow-open-path 对象项
        let open_path = perms
            .iter()
            .find_map(|p| {
                (p["identifier"] == "opener:allow-open-path").then_some(p)
            })
            .expect("必须配置 opener:allow-open-path");
        let allow = open_path["allow"]
            .as_array()
            .expect("allow 必须是数组")
            .iter()
            .filter_map(|e| e["path"].as_str())
            .collect::<Vec<_>>();
        assert!(!allow.is_empty(), "allow 列表不得为空");
        assert!(
            !allow.iter().any(|p| *p == "**" || *p == "/" || *p == "/*"),
            "禁止裸通配/根目录全量放行: {allow:?}"
        );
        // 每个 allow 根必须落在受控变量内（数据目录 / 用户主目录）
        for p in &allow {
            assert!(
                p.starts_with("$APPDATA/") || p.starts_with("$HOME/"),
                "allow 根必须是 $APPDATA 或 $HOME: {p}"
            );
        }
        // 验收：${dataDir}/exports/foo.pdf 允许（$APPDATA/** 覆盖数据目录）；
        // /etc/passwd 拒绝（不在任何 allow 根之下 → 默认 deny）
        assert!(
            allow.contains(&"$APPDATA/**"),
            "必须放行数据目录（含 exports/skills）: {allow:?}"
        );
        let passwd_covered = allow
            .iter()
            .any(|p| "/etc/passwd".starts_with(p.trim_end_matches("**").trim_end_matches('/')));
        assert!(!passwd_covered, "/etc/passwd 不得被任何 allow 覆盖");
    }
}

#[cfg(test)]
mod p2_31_tray_tests {
    /// P2-31：托盘初始化不得限定 cfg(windows) —— tauri 2 tray-icon 三平台支持，
    /// Linux/macOS 走同一初始化（Linux 缺 libappindicator 时降级 eprintln，不启动失败）。
    /// 本测试锁死 lib.rs 防回退；编译通过即证明 macOS 平台托盘代码路径有效。
    #[test]
    fn tray_init_is_not_windows_gated() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("lib.rs 必须可读");
        let pos = text
            .find("TrayIconBuilder::with_id")
            .expect("必须有托盘初始化");
        // 托盘初始化语句前 300 字符内不得有 cfg 平台门
        let before = &text[..pos];
        let tail = &before[before.len().saturating_sub(300)..];
        assert!(
            !tail.contains("#[cfg("),
            "托盘初始化不得被 cfg 平台门限制（P2-31 三平台）: {tail:?}"
        );
        // 初始化失败必须降级（不得用 ? 让 setup 失败）
        let after = &text[pos..];
        let scope = &after[..after.len().min(2500)];
        assert!(
            scope.contains("eprintln!"),
            "托盘初始化失败必须降级记日志，不得中断启动: {scope:?}"
        );
    }
}

#[cfg(test)]
mod p2_29_version_tests {
    /// P2-29：版本号单一真相源 = src-tauri/Cargo.toml（原先 4 处手动维护漂移：
    /// conf 1.0.0 / Cargo 0.1.0 / package.json 0.1.0 / 便携包 1.0.1）。
    /// tauri.conf.json 不得再写 version（tauri 2 构建期回退 CARGO_PKG_VERSION，
    /// 见 tauri-codegen context.rs）；package.json 由 pnpm prebuild 的
    /// scripts/sync-version.mjs 同步，必须与 Cargo.toml 一致。
    #[test]
    fn version_single_source_is_cargo_toml() {
        let conf_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(conf_path).expect("tauri.conf.json 必须可读"),
        )
        .expect("必须是合法 JSON");
        assert!(
            conf.get("version").is_none(),
            "tauri.conf.json 不得写 version（单一真相源是 Cargo.toml，构建期自动回退）"
        );
        let pkg_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../package.json");
        let pkg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(pkg_path).expect("package.json 必须可读"),
        )
        .expect("必须是合法 JSON");
        assert_eq!(
            pkg["version"].as_str().expect("package.json 必须有 version"),
            env!("CARGO_PKG_VERSION"),
            "package.json version 与 Cargo.toml 漂移：跑 pnpm run sync-version 同步"
        );
    }
}

#[cfg(test)]
mod p2_26_bring_front_tests {
    use tauri::Manager;

    /// P2-26：bring_main_to_front 不得阻塞调用线程 —— 80ms 置顶闪烁的等待
    /// 在后台线程，调用方（全局快捷键/托盘事件处理，全跑主线程）立即返回。
    #[test]
    fn bring_main_to_front_does_not_block_caller() {
        let app = tauri::test::mock_app();
        let w = tauri::WebviewWindowBuilder::new(
            app.handle(),
            "main",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .build()
        .expect("mock 窗口应创建成功");
        let start = std::time::Instant::now();
        super::bring_main_to_front(&w);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(80),
            "调用线程被阻塞了 {elapsed:?}（80ms sleep 应在后台线程）"
        );
        // 后台线程收尾（mock 窗口的 set_always_on_top 为 no-op，最多等 80ms+ 宽限）
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

#[cfg(test)]
mod p2_24_exit_cleanup_tests {
    use tauri::Manager;

    /// P2-24：ExitRequested 清理 —— mock 一个 Running 态 Skill + 真实启动 API server，
    /// cleanup_on_exit 后：API 状态释放且端口关闭、Skill 终止、api-enabled.flag 保留
    ///（退出 ≠ 用户关开关，下次启动应自动恢复）、app_exit_cleanup 审计落行。
    #[test]
    fn cleanup_on_exit_releases_api_and_skill() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        app.manage(crate::api_server::ApiState::default());
        // 真实启动 API server（固定生产端口；MemStore 免 Wry 绑定的 TauriStore，
        // server/线程/端口与 api_start 同为 start_api 真路径）
        {
            let store: std::sync::Arc<dyn crate::api::TaskStore> =
                std::sync::Arc::new(crate::api::MemStore {
                    tasks: std::sync::Mutex::new(Vec::new()),
                    hub: crate::api_server::EventHub::new(),
                });
            let running = crate::api_server::start_api(
                crate::api::API_PORT,
                "test-token".into(),
                store,
                None,
                None,
                None,
            )
            .expect("API 应启动成功");
            let state = app.state::<crate::api_server::ApiState>();
            *state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(running);
        }
        // mock 一个活动 Skill（Paused：terminate_all 覆盖 Running+Paused；
        // 不用 Running 是避免污染并行测试的 is_skill_active 全局断言）
        crate::bot_skills::test_insert_skill_run(
            "p2-24-skill",
            crate::bot_skills::SkillState::Paused,
        );
        // 模拟「API 开启中退出」：enabled flag 存在（api_start 成功后会写）
        let dir = crate::db::data_dir(&handle);
        std::fs::create_dir_all(&dir).unwrap();
        let flag = dir.join("api-enabled.flag");
        std::fs::write(&flag, b"1").unwrap();

        // kill fn 注入 spy：全局 kill_all 会误杀并行测试注册的在途子进程，
        // 真杀路径由 bot_py::kill_py_children 单测覆盖
        let kill_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let kc = kill_called.clone();
        super::cleanup_on_exit_with(&handle, move || {
            kc.store(true, std::sync::atomic::Ordering::SeqCst);
            0
        });
        assert!(
            kill_called.load(std::sync::atomic::Ordering::SeqCst),
            "退出清理必须调用 Python 子进程清理"
        );

        // API 已停：state 清空 + 端口拒绝连接
        {
            let state = app.state::<crate::api_server::ApiState>();
            assert!(
                state.0.lock().unwrap_or_else(|e| e.into_inner()).is_none(),
                "API 状态应已释放"
            );
        }
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", crate::api::API_PORT)).is_err(),
            "API 端口应已关闭"
        );
        // Skill 已终止
        assert_eq!(
            crate::bot_skills::test_skill_run_state("p2-24-skill"),
            Some(crate::bot_skills::SkillState::Terminated),
            "活动 Skill 应被终止"
        );
        // enabled flag 保留：退出路径区别于用户主动 api_stop（下次启动自动恢复）
        assert!(flag.exists(), "退出路径不得清 api-enabled.flag");
        // 审计落行
        let log = std::fs::read_to_string(dir.join("bot.log")).expect("bot.log 应存在");
        assert!(
            log.contains("app_exit_cleanup"),
            "缺 app_exit_cleanup 审计行"
        );

        // 收尾：清掉本测试在数据目录产生的文件与 Skill run，不污染其他测试
        let _ = std::fs::remove_file(&flag);
        crate::bot_skills::test_remove_skill_run("p2-24-skill");
    }
}
