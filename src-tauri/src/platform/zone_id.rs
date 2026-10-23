//! 「来自互联网」标记（Zone.Identifier 备用数据流）的清理
//!
//! Windows 给从网络下载、从压缩包解压出来的文件打上 Zone.Identifier 备用数据流
//! （MOTW, Mark-of-the-Web）。**复制文件走 Win32 `CopyFileEx`，备用数据流会跟着
//! 一起复制**——所以 app 自己产出的文档也会带着这个标记：
//!
//! - Word 打开带标记的 docx 会走受保护视图，把文件复制进 Office 文档缓存
//!   （`%LOCALAPPDATA%\Microsoft\Windows\INetCache\Content.Word\~WRDxxxx.docx`）
//! - 杀软再把这次「写文档」按风险程序连坐拦掉
//!   （360 主动防御：「[自动阻止] 修改 文档或图片文件」）
//!
//! 修法是让 app 交付出去的副本不带这个标记。剥流用
//! `CreateFileW` + `SetFileInformationByHandle(FileDispositionInfo)`——只删
//! Zone.Identifier 这一条流，主数据流与其余流不动。
//!
//! 非 Windows 平台没有这个概念，整层是 no-op。

use std::path::Path;

/// Windows 上「来自互联网」标记的备用数据流名。
#[cfg(windows)]
const ZONE_STREAM: &str = "Zone.Identifier";

/// ERROR_FILE_NOT_FOUND：流不存在（文件也没标记）。
#[cfg(windows)]
const ERROR_FILE_NOT_FOUND: u32 = 2;
/// ERROR_PATH_NOT_FOUND：路径整体不存在。
#[cfg(windows)]
const ERROR_PATH_NOT_FOUND: u32 = 3;

/// 「来自互联网」标记的流路径：`<文件路径>:Zone.Identifier`（UTF-16, NUL 结尾）。
#[cfg(windows)]
fn zone_stream_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(u16::from(b':'));
    wide.extend(ZONE_STREAM.encode_utf16());
    wide.push(0);
    wide
}

/// 剥掉一个文件的「来自互联网」标记（Zone.Identifier 流）。
///
/// - `Ok(true)`  标记流存在且已被删除
/// - `Ok(false)` 文件本来就没有标记
/// - `Err(why)` 剥不掉（权限不足 / 非 NTFS 卷 / 被独占打开……）
///
/// 剥不掉**不**构成生成失败：标记只让 Office 多绕一圈受保护视图，文件本身是好的。
/// 错误由返回值带出，调用方决定记日志继续还是中断，这里不吞。
#[cfg(windows)]
pub fn strip_zone_identifier(path: &Path) -> Result<bool, String> {
    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, SetFileInformationByHandle, DELETE,
        FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide = zone_stream_path(path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            DELETE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            // 流不存在 = 文件本来就没有标记（文件整体不存在也落这里）
            let code = e.code();
            if code == HRESULT::from_win32(ERROR_FILE_NOT_FOUND)
                || code == HRESULT::from_win32(ERROR_PATH_NOT_FOUND)
            {
                return Ok(false);
            }
            return Err(format!("打开标记流失败：{e}"));
        }
    };

    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let deleted = unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    // 句柄必须关：流只是被「标记删除」，不关则删除不落盘
    let _ = unsafe { CloseHandle(handle) };
    deleted.map_err(|e| format!("删除标记流失败：{e}"))?;
    Ok(true)
}

/// 剥掉一个文件的「来自互联网」标记（Zone.Identifier 流）。
///
/// - `Ok(true)`  标记流存在且已被删除
/// - `Ok(false)` 文件本来就没有标记（Windows 以外没有这个概念，一律视为无标记）
/// - `Err(why)` 剥不掉（权限不足 / 非 NTFS 卷 / 被独占打开……）
#[cfg(not(windows))]
pub fn strip_zone_identifier(_path: &Path) -> Result<bool, String> {
    Ok(false)
}

/// 递归剥掉路径下所有文件的「来自互联网」标记，返回剥掉的个数。
///
/// 目录自身的标记不动：Office 看的是**文件**的标记，目录带不带不影响打开。
/// 符号链接不跟随——链接目标在目录外，改它的标记是越权。
/// 剥不掉的项通过 `audit` 留痕，不中断遍历（一个剥不掉不影响其余文件）。
pub fn strip_zone_identifier_rec(path: &Path, audit: &mut dyn FnMut(&str)) -> usize {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        return entries
            .flatten()
            .map(|e| strip_zone_identifier_rec(&e.path(), audit))
            .sum();
    }
    match strip_zone_identifier(path) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            audit(&format!(
                "剥「来自互联网」标记失败 | {} | {e}",
                path.display()
            ));
            0
        }
    }
}

#[cfg(all(test, windows))]
/// 给文件写一个 Zone.Identifier 流（模拟「从网上下载」）。
pub(crate) fn test_mark_file(path: &Path) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, WriteFile, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let wide = zone_stream_path(path);
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            (GENERIC_WRITE.0) | (GENERIC_READ.0),
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .expect("造 Zone.Identifier 流");
    let payload = b"[ZoneTransfer]\r\nZoneId=3\r\n";
    unsafe {
        WriteFile(h, Some(payload), None, None).expect("写标记流内容");
    }
    let _ = unsafe { CloseHandle(h) };
}

#[cfg(all(test, windows))]
/// 文件当前是否带「来自互联网」标记。
pub(crate) fn test_has_mark(path: &Path) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide = zone_stream_path(path);
    let opened = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };
    match opened {
        Ok(h) => {
            let _ = unsafe { CloseHandle(h) };
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非 Windows 平台是 no-op：任何路径都算「本来就没有标记」，且不报错。
    ///
    /// 锁的是契约而不是实现——产物落盘路径上一律会调它，
    /// 不能因为「当前平台用不上」就变成会失败的路径。
    #[test]
    #[cfg(not(windows))]
    fn strip_is_noop_off_windows() {
        assert_eq!(
            strip_zone_identifier(Path::new("/nonexistent/x.docx")),
            Ok(false)
        );
        assert_eq!(strip_zone_identifier(Path::new("")), Ok(false));
    }

    /// 递归版对不存在的路径返回 0，不报错、不 panic。
    #[test]
    fn rec_on_missing_path_is_zero() {
        let mut audit = |_m: &str| panic!("不存在的路径不该产生审计");
        assert_eq!(
            strip_zone_identifier_rec(Path::new("/nonexistent/dir"), &mut audit),
            0
        );
    }

    /// 递归版扫普通目录：无标记可剥时计数 0，且不产生失败审计。
    #[test]
    fn rec_on_plain_dir_reports_no_failure() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("a.txt"), b"x").expect("写文件");
        std::fs::create_dir_all(dir.path().join("sub")).expect("建子目录");
        std::fs::write(dir.path().join("sub").join("b.txt"), b"y").expect("写文件");
        let mut msgs: Vec<String> = Vec::new();
        let n = strip_zone_identifier_rec(dir.path(), &mut |m| msgs.push(m.to_string()));
        assert_eq!(n, 0, "普通文件没有标记可剥");
        assert!(msgs.is_empty(), "不该有失败审计：{msgs:?}");
    }

    /// Windows 真逻辑：带标记的文件剥完标记消失，主数据流一个字节没动。
    ///
    /// 这是本次修复的复现测试——修之前标记留在产物上，
    /// Word 打开它会走受保护视图写 Office 文档缓存，杀软随即拦掉那次写入。
    #[cfg(windows)]
    mod windows_only {
        use super::*;

        #[test]
        fn strip_removes_mark_and_keeps_content() {
            let dir = tempfile::tempdir().expect("临时目录");
            let f = dir.path().join("doc.docx");
            std::fs::write(&f, b"PK\x03\x04 payload").expect("写主数据流");
            test_mark_file(&f);
            assert!(test_has_mark(&f), "造标记失败：流都没写进去");

            assert_eq!(strip_zone_identifier(&f), Ok(true));
            assert!(!test_has_mark(&f), "标记流还在");
            assert_eq!(
                std::fs::read(&f).unwrap(),
                b"PK\x03\x04 payload",
                "剥标记不该动主数据流"
            );
        }

        #[test]
        fn strip_on_unmarked_file_reports_no_mark() {
            let dir = tempfile::tempdir().expect("临时目录");
            let f = dir.path().join("plain.docx");
            std::fs::write(&f, b"x").expect("写文件");
            assert_eq!(strip_zone_identifier(&f), Ok(false));
            assert!(!test_has_mark(&f));
        }
    }
}
