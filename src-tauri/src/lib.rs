// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
mod db;
use tauri::Manager;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

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
        return Err("复制文件暂不支持当前平台".into());
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
    let _ok = unsafe {
        pb.setPropertyList_forType(&paths, &NSString::from_str("NSFilenamesPboardType"))
    };

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
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
    use windows::Win32::UI::Shell::DROPFILES;

    unsafe {
        if OpenClipboard(None).is_err() {
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
            return Err("写入文件列表失败".into());
        }

        // 2) 标题文本（CF_UNICODETEXT）
        let title_wide: Vec<u16> = OsStr::new(title).encode_wide().chain(Some(0)).collect();
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
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

            // 主窗口「关闭」改为隐藏：挂件随时能唤起它（否则关闭后挂件无法打开主窗口）。
            // 真正退出走 Cmd+Q（见下方 RunEvent::ExitRequested）。
            if let Some(main) = app.get_webview_window("main") {
                let main2 = main.clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = main2.hide();
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            copy_file_with_title,
            db::db_load,
            db::db_upsert,
            db::db_delete
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Cmd+Q 退出：主窗口 CloseRequested 被上面 prevent（改成隐藏），
            // 这里直接销毁主窗口，让退出流程正常走完
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.destroy();
                }
            }
        });
}
