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

/// 同进程内触碰溢出根目录的测试共用串行锁：sweep 会删整个根下的过期文件、
/// 兜底用例会把根临时占成文件——并行互踩会互相制造假失败（nextest 每测试
/// 一进程本无此问题，cargo test --lib 单进程并行时有）。
#[cfg(test)]
pub(crate) static OVERFLOW_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// spill 的异步壳：落盘是同步 syscall，放 blocking 池跑，不占 runtime 线程。
pub async fn spill_async<R: tauri::Runtime>(
    app: &AppHandle<R>,
    text: &str,
) -> Result<PathBuf, String> {
    let app = app.clone();
    let text = text.to_string();
    tauri::async_runtime::spawn_blocking(move || spill(&app, &text))
        .await
        .map_err(|e| format!("溢出转存线程失败：{e}"))?
}

/// 给模型的指针文案：说清「完整内容在哪、怎么读」，别让它以为执行失败或结果为空。
///
/// **可达范围必须一起说清**：`read_text_file` 只读文件前 100KB（offset 是在
/// 这 100KB 的行上取的）。指针原先承诺「完整内容已转存，用 offset + limit
/// 分页读」，但超过 100KB 的文件分页读只能读到前部，后半段会撞上
/// 「offset 超出范围」——模型读不到尾部会以为文件被截断了。
pub fn pointer_text(tool: &str, path: &std::path::Path, text: &str) -> String {
    let read_cap = crate::bot::READ_MAX_BYTES;
    let reach = if text.len() > read_cap {
        format!(
            "（单次读取上限 {}KB：分页读只能读到文件前部，后半段用 grep_files 按关键词定位）",
            read_cap / 1024
        )
    } else {
        String::new()
    };
    format!(
        "⚠️ 工具「{tool}」输出超过单次回灌预算，已**完整**转存到本地文件（下方是节选，不是全部）：\n\
         - 完整内容：{}\n\
         - 规模：{} 行 / {} 字节\n\
         - 读法：read_text_file 配 offset（行号，从 1 起）+ limit（每页行数）分页读{}\n\n\
         需要哪一段读哪一段，不要一次读完；上面这段是节选，中间部分以文件为准。",
        path.display(),
        text.lines().count(),
        text.len(),
        reach,
    )
}

/// 保留期清扫：启动即跑一次，此后每天一次（与 `db::trace::spawn_retention_sweep` 同构）。
/// 扫描是同步目录遍历，放 blocking 池；单轮 panic 以 JoinError 形态收口——
/// 清扫停摆 = 溢出文件无限堆积且无人察觉，循环绝不因单轮异常退出。
/// 失败只记审计，不打断启动、不终止循环。
pub fn spawn_sweep(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let h = handle.clone();
            let outcome = tauri::async_runtime::spawn_blocking(move || sweep(&h, None)).await;
            match outcome {
                Err(e) => crate::audit::write_event(
                    &handle,
                    crate::audit::AuditLevel::Warn,
                    "overflow.sweep_fail",
                    &[("err", format!("清扫任务异常（本轮跳过，循环继续）：{e}"))],
                ),
                Ok(Err(e)) => crate::audit::write_event(
                    &handle,
                    crate::audit::AuditLevel::Warn,
                    "overflow.sweep_fail",
                    &[("err", e)],
                ),
                Ok(Ok(_)) => {}
            }
            tokio::time::sleep(std::time::Duration::from_secs(SWEEP_INTERVAL_SECS)).await;
        }
    });
}

/// 清扫保留期外的溢出文件，返回删除数。
///
/// 目录不存在 = 还没有溢出文件发生过，返回 0 而不是报错。判龄锚点是**日期
/// 目录名**（spill 落盘那天的 UTC 日期），不是文件 mtime——mtime 随任何触碰
/// 变化，时钟回拨会把 mtime 推到未来（文件在时钟追上来之前永远清不掉），
/// 时钟前跳则把保留期悄悄缩短；目录名只在创建时写一次，稳定可测。
/// 按日粒度整目录删除；非日期目录（误放）不碰，删不掉的下轮再来。
pub fn sweep<R: tauri::Runtime>(app: &AppHandle<R>, days: Option<i64>) -> Result<usize, String> {
    // 上限 100 年：Duration::days 对异常大的输入会 panic，钳住才谈得上防御
    let days = days.unwrap_or(OVERFLOW_RETENTION_DAYS).clamp(0, 36_500);
    let today = chrono::Utc::now().date_naive();
    let cutoff = today - chrono::Duration::days(days);
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
        let Ok(day_date) =
            chrono::NaiveDate::parse_from_str(&day.file_name().to_string_lossy(), "%Y%m%d")
        else {
            continue;
        };
        // 只清创建日期早于（含当天恰好到期）cutoff 的整目录
        if day_date > cutoff {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&day_path) else {
            continue;
        };
        let n = files.flatten().filter(|f| f.path().is_file()).count();
        if std::fs::remove_dir_all(&day_path).is_ok() {
            removed += n;
        }
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

    /// 清扫方向：过期的删、保留期内的留。判定方向反过来时两条都会翻——
    /// days=0 那轮删不掉任何文件；days=3650 那轮把该保留的删了。
    #[test]
    fn mutant_m8_sweep_removes_expired_keeps_fresh() {
        let _g = OVERFLOW_TEST_LOCK.blocking_lock();
        let app = tauri::test::mock_app();
        let root = root(app.handle());
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join(chrono::Utc::now().format("%Y%m%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();

        // days=0：cutoff=此刻，此前创建的文件必过期 → 删
        let removed = sweep(app.handle(), Some(0)).unwrap();
        assert_eq!(removed, 1, "days=0 时现存文件应判定过期删除");
        assert!(!file.exists(), "过期文件应已删除");

        // days=3650：cutoff 在十年前 → 新文件必须保留
        //（sweep 会把删空的日期目录一并移除，重建后再放新文件）
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "x").unwrap();
        let kept = sweep(app.handle(), Some(3650)).unwrap();
        assert_eq!(kept, 0, "保留期内的文件不得删除");
        assert!(file.exists(), "保留期内的文件必须还在");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 负数天数必须钳到 0：`days as u64` 对 -1 翻成 u64::MAX，再乘 86400
    /// 在 debug 构建直接 overflow panic；钳 0 后语义与 days=0 一致（全过期）。
    #[test]
    fn mutant_m9_sweep_negative_days_clamped_to_zero() {
        let _g = OVERFLOW_TEST_LOCK.blocking_lock();
        let app = tauri::test::mock_app();
        let root = root(app.handle());
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join(chrono::Utc::now().format("%Y%m%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let removed = sweep(app.handle(), Some(-1)).unwrap();
        assert_eq!(removed, 1, "负数天数按 0 处理：现存文件全部过期删除");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pointer_text_states_path_and_scale() {
        let text = "a\nb\nc";
        let p = pointer_text("run_python", std::path::Path::new("/x/o.txt"), text);
        assert!(p.contains("/x/o.txt"), "指针必须给出路径：{p}");
        assert!(p.contains("3 行"), "指针必须给出行数：{p}");
        assert!(p.contains("5 字节"), "指针必须给出字节数：{p}");
        assert!(p.contains("read_text_file"), "指针必须说清读法：{p}");
        assert!(
            !p.contains("单次读取上限"),
            "文件没超单次读取上限时不必提上限：{p}"
        );
    }

    /// 判龄锚点是目录名而非 mtime 的两个边界：
    /// - 时钟回拨残留的「未来日期」目录不得被清（mtime 判龄时这类文件在
    ///   时钟追上来之前永远清不掉，目录名判龄则直接跳过）；
    /// - 非日期目录（用户误放进根目录的东西）原样不动。
    #[test]
    fn sweep_respects_future_dated_and_non_date_dirs() {
        let _g = OVERFLOW_TEST_LOCK.blocking_lock();
        let app = tauri::test::mock_app();
        let root = root(app.handle());
        let _ = std::fs::remove_dir_all(&root);
        // 时钟回拨残留：目录名标着 10 天后的日期
        let future = root.join(
            (chrono::Utc::now().date_naive() + chrono::Duration::days(10))
                .format("%Y%m%d")
                .to_string(),
        );
        std::fs::create_dir_all(&future).unwrap();
        std::fs::write(future.join("a.txt"), "x").unwrap();
        // 非日期目录
        let stray = root.join("not-a-date");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("b.txt"), "x").unwrap();

        let removed = sweep(app.handle(), Some(0)).unwrap();
        assert_eq!(removed, 0, "未来日期与非日期目录都不得被 days=0 清扫");
        assert!(future.join("a.txt").exists(), "未来日期目录必须原样保留");
        assert!(stray.join("b.txt").exists(), "非日期目录必须原样保留");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 超过单次读取上限的文件：指针必须说清**可达范围**。
    ///
    /// `read_text_file` 只读前 100KB，offset 是在这 100KB 的行上取的——
    /// 指针原先承诺「分页读」，但 100KB 之后的行压根到不了（offset 超出范围）。
    #[test]
    fn pointer_text_states_the_reachable_range_for_big_files() {
        let big = "x".repeat(crate::bot::READ_MAX_BYTES + 1);
        let p = pointer_text("run_python", std::path::Path::new("/x/o.txt"), &big);
        assert!(
            p.contains("单次读取上限 100KB"),
            "超限文件必须说清读得到哪一段：{p}"
        );
        assert!(
            p.contains("grep_files"),
            "要给一条真的能读到后半段的路径：{p}"
        );
    }
}
