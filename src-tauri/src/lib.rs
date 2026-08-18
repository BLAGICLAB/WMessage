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
fn copy_file_with_title(path: String, title: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return copy_file_macos(&path, &title);
    }
    #[cfg(windows)]
    {
        return copy_file_windows(&path, &title);
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (path, title);
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("复制文件暂不支持当前平台".into());
    }
}

/// 唤起主窗口并强制置顶（单一真相）
///
/// 老板 2026-08-17 11:31/11:43 规则：所有唤起主窗口的路径（widget 双击标题、聊天区 📌、
/// 全局快捷键、托盘点击/菜单）都必须把主窗口推到桌面屏幕最顶层才能看见。
/// 仅 setFocus 在 Windows 上不一定推到 z-order 最顶层（其他窗口抢焦点时被遮住），
/// 需 alwaysOnTop 短暂闪烁 80ms 强制重排后再恢复（不长驻，避免干扰用户正常使用电脑）。
pub fn bring_main_to_front(window: &tauri::WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    let _ = window.set_always_on_top(true);
    std::thread::sleep(std::time::Duration::from_millis(80));
    let _ = window.set_always_on_top(false);
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

            // Windows 系统托盘：#3 图标，右键菜单「打开主窗口 / 退出」，
            // 左键单击/双击恢复主窗口。
            #[cfg(target_os = "windows")]
            {
                use tauri::image::Image;
                use tauri::menu::{MenuBuilder, MenuItemBuilder};
                use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};

                let open = MenuItemBuilder::with_id("tray_open", "打开主窗口").build(app)?;
                let quit = MenuItemBuilder::with_id("tray_quit", "退出").build(app)?;
                let menu = MenuBuilder::new(app).items(&[&open, &quit]).build()?;

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
                    .build(app)?;
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            copy_file_with_title,
            focus_main_window,
            bot_skills::open_file_path,
            bot_skills::pick_files_dialog,
            bot_skills::delete_bound_file,
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
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.destroy();
                }
            }
        });
}
