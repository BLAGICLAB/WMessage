//! 跨平台「复制文件 + 任务标题到剪贴板」helper
//!
//! 行为契约：
//! - 文件类粘贴（Finder / 飞书 / 微信）得到真实文件
//! - 文本类粘贴得到任务标题
//!
//! 平台差异：
//! - **macOS**：NSPasteboard 写三类（`public.file-url` / `NSFilenamesPboardType` /
//!   `NSPasteboardTypeString`），返回值 `Result<(), String>`（macOS helper
//!   暂未走 `CommandError`，由 `copy_file_with_title` 经 `From<String>` 转换）
//! - **Windows**：CF_HDROP + CF_UNICODETEXT，返回 `Result<(), CommandError>`
//!   直接是 `CommandResult` 形态
//! - **其它平台**：`copy_file_with_title` 顶层兜底返回 `DomainRule` 错误

/// macOS 实现：NSPasteboard 写三类粘贴板
///
/// 2026-09-14 Sprint B2：从 `lib.rs:98-122` 搬过来，逻辑一字不动。
/// 保留 `Result<(), String>` 是为了不破坏 F2（Phase 6b）已有的 From 转换链
/// （`f2_copy_file_tests::string_error_converts_to_internal_with_message_preserved`）。
#[cfg(target_os = "macos")]
pub(crate) fn copy_file_macos(path: &str, title: &str) -> Result<(), String> {
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
    // SAFETY: &NSString 由 NSString::from_str 创建存于本栈帧，调用期间不释放；&paths 是 CFArray 借用视图（paths 已 validate 非空），调用方保证生命周期。
    let _ok =
        unsafe { pb.setPropertyList_forType(&paths, &NSString::from_str("NSFilenamesPboardType")) };

    // 3) 标题文本：文本应用粘贴即标题
    let text = NSString::from_str(title);
    // SAFETY: NSPasteboardTypeString 是 Foundation 公开常量，值稳定不释放、全局唯一无别名风险；unsafe 仅用于将 *const NSString 转为 &NSString。
    let _ok = pb.setString_forType(&text, unsafe { NSPasteboardTypeString });
    Ok(())
}

/// Windows 实现：CF_HDROP + CF_UNICODETEXT
///
/// 2026-09-14 Sprint B2：从 `lib.rs:126-185` 搬过来，逻辑一字不动。
#[cfg(windows)]
pub(crate) fn copy_file_windows(path: &str, title: &str) -> Result<(), crate::error::CommandError> {
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
            return Err(crate::error::CommandError::DomainRule {
                domain: "clipboard".to_string(),
                reason: "打开剪贴板失败".to_string(),
            });
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
            return Err(crate::error::CommandError::DomainRule {
                domain: "clipboard".to_string(),
                reason: "锁定文件列表内存失败".to_string(),
            });
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
            return Err(crate::error::CommandError::DomainRule {
                domain: "clipboard".to_string(),
                reason: "写入文件列表失败".to_string(),
            });
        }

        // 2) 标题文本（CF_UNICODETEXT）
        let title_wide: Vec<u16> = OsStr::new(title).encode_wide().chain(Some(0)).collect();
        let th = GlobalAlloc(GMEM_MOVEABLE, title_wide.len() * 2).map_err(|e| e.to_string())?;
        let tbase = GlobalLock(th) as *mut u16;
        if tbase.is_null() {
            let _ = GlobalFree(Some(th));
            let _ = CloseClipboard();
            return Err(crate::error::CommandError::DomainRule {
                domain: "clipboard".to_string(),
                reason: "锁定标题内存失败".to_string(),
            });
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
