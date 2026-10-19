//! 工具大结果溢出存储：超预算的输出转存成文件，回灌文本里换成可分页读的指针。
//!
//! 为什么与 trace 侧的钳制分开：两者管的不是一件事。
//! - `db::SPAN_TEXT_MAX` 管**存**——一条 span 的 args/result 落多少进 SQLite。
//! - 本模块管**喂给模型多少**——回灌进 messages 的那段文本有多大。
//!   一次 `run_python` 打出 66KB 就是约 1.6 万 token，撑爆的是上下文，不是数据库。
//!
//! 纪律：
//! - 路径只由程序生成（UTC 日期目录 + uuid 文件名），不接受任何外部输入拼路径；
//! - 转存失败退回无文件的纯头尾截断——宁可少给预览，也不能让结果凭空消失；
//! - 目录恒在 `bot_fs` 读白名单里，否则指针给到了模型却读不回来（见 bot_fs::allowed_dirs）。
//!
//! 为什么不放系统 temp：temp 由 OS 择机清理，指针会在模型还没来得及读时失效。

use std::path::PathBuf;

use tauri::AppHandle;

/// 溢出文件保留期（天）。与执行痕迹保留期同量级——两者都是「事后可能要看一眼」
/// 的证据，不是长期资产。
pub const OVERFLOW_RETENTION_DAYS: i64 = 30;

/// 清扫间隔（秒）：启动即跑一次，此后每天一次（与 trace 保留期清扫同节奏）。
const SWEEP_INTERVAL_SECS: u64 = 24 * 3600;

/// 溢出根目录：`data_dir/overflow/<YYYYMMDD>/<uuid>.txt`。
/// 按日期分目录是为了清扫能整目录地按 mtime 判龄，也免得单目录堆几十万文件。
pub fn root<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::db::paths::data_dir(app).join("overflow")
}

/// 转存完整工具输出，返回落盘路径。
///
/// 目录按当天创建（解析即建，模型随后读回时不至于扑空）。
pub fn spill<R: tauri::Runtime>(app: &AppHandle<R>, text: &str) -> Result<PathBuf, String> {
    let day = chrono::Utc::now().format("%Y%m%d").to_string();
    let dir = root(app).join(day);
    std::fs::create_dir_all(&dir).map_err(|e| format!("溢出目录创建失败：{e}"))?;
    let path = dir.join(format!("{}.txt", uuid::Uuid::new_v4().simple()));
    crate::db::paths::atomic_write(&path, text)?;
    Ok(path)
}

/// 给模型的指针文案：说清「完整内容在哪、怎么读」，别让它以为执行失败或结果为空。
pub fn pointer_text(tool: &str, path: &std::path::Path, text: &str) -> String {
    format!(
        "⚠️ 工具「{tool}」输出超过单次回灌预算，已**完整**转存到本地文件（下方是节选，不是全部）：\n\
         - 完整内容：{}\n\
         - 规模：{} 行 / {} 字节\n\
         - 读法：read_text_file 配 offset（行号，从 1 起）+ limit（每页行数）分页读\n\n\
         需要哪一段读哪一段，不要一次读完；上面这段是节选，中间部分以文件为准。",
        path.display(),
        text.lines().count(),
        text.len(),
    )
}

/// 保留期清扫：启动即跑一次，此后每天一次（与 `db::trace::spawn_retention_sweep` 同构）。
/// 失败只记审计，不打断启动、不终止循环。
pub fn spawn_sweep(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(e) = sweep(&handle, None) {
                crate::audit::write_event(
                    &handle,
                    crate::audit::AuditLevel::Warn,
                    "overflow.sweep_fail",
                    &[("err", e.to_string())],
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
        }
    });
}

/// 清扫保留期外的溢出文件，返回删除数。
///
/// 目录不存在 = 还没有溢出文件发生过，返回 0 而不是报错。日期目录删空后一并移除；
/// 删不掉（仍非空）忽略即可，下轮还会来。
pub fn sweep<R: tauri::Runtime>(app: &AppHandle<R>, days: Option<i64>) -> Result<usize, String> {
    let days = days.unwrap_or(OVERFLOW_RETENTION_DAYS).max(0);
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(days as u64 * 86_400))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let root = root(app);
    let Ok(days_dir) = std::fs::read_dir(&root) else {
        return Ok(0);
    };
    let mut removed = 0usize;
    for day in days_dir.flatten() {
        let day_path = day.path();
        if !day_path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        for f in files.flatten() {
            let Ok(meta) = f.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Ok(mtime) = meta.modified() else { continue };
            if mtime < cutoff && std::fs::remove_file(f.path()).is_ok() {
                removed += 1;
            }
        }
        let _ = std::fs::remove_dir(&day_path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_missing_dir_is_zero_not_error() {
        // data_dir 在测试环境解析不到 → root 不存在；不该把「没溢出过」报成失败
        let app = tauri::test::mock_app();
        assert_eq!(sweep(app.handle(), None).unwrap_or_default(), 0);
    }

    #[test]
    fn pointer_text_states_path_and_scale() {
        let text = "a\nb\nc";
        let p = pointer_text("run_python", std::path::Path::new("/x/o.txt"), text);
        assert!(p.contains("/x/o.txt"), "指针必须给出路径：{p}");
        assert!(p.contains("3 行"), "指针必须给出行数：{p}");
        assert!(p.contains("5 字节"), "指针必须给出字节数：{p}");
        assert!(p.contains("read_text_file"), "指针必须说清读法：{p}");
    }
}
