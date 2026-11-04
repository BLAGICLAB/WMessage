//! 数据目录解析（从 audit.rs 抽出，独立模块）
//!
//! 抽出的动机：原 audit.rs 同时承担两件事——
//! 1) **目录解析**（probe_log_dir / cached_probe_dir / probe_dir / 辅助函数 + 缓存）；
//! 2) **结构化审计日志**（write_event / write_error_audit / write_warn_audit_to / 行拼装）。
//! 现状 `db.rs / profile.rs / bot.rs / middleware.rs` 引用 audit 只是为了拿目录，
//! 属于 SRP 违例（audit 改了行格式，所有调用方都要重审）。
//!
//! 拆出 paths 后：
//! - paths 只负责「目录怎么定」，调用方拿 `PathBuf`；
//! - audit 只负责「目录定了之后日志怎么写」，不需要知道数据目录策略；
//! - 未来数据目录策略再变只动 paths。
//!
//! 内联 fallback WARN：app_data_dir 解析失败退系统 temp 时需要在数据
//! 目录里写一条 WARN 审计。审计模块（audit）写日志依赖 paths 的目录解析——
//! 抽到 paths 后 audit 不能反向调 paths → 在 paths 内联一份最小化的 WARN 写入，
//! 格式与 audit 完全一致（同样 `[ts] WARN | event | k=v`），行解析端无感。

use tauri::Manager;

/// 数据目录解析公共入口（db / profile / audit / bot 全靠它定版）。
///
/// 分支：
/// 0. exe 在 cargo target 的 **deps/** 子目录（= cargo test 测试进程）→
///    隔离到 `{temp}/wm-cargo-data-<pid>`（每进程一个，防并行测试抢同一个
///    SQLite 写锁）——既不落 target/（防打包装箱带数据），
///    也不落真实 app_data（mock runtime 的 app_data_dir 解析到真实用户数据根，
///    直接返回会把测试库写进用户数据目录旁，已实证泄漏）；
///    target/debug|release **本体**（cargo run / tauri dev / 手动运行）不隔离，
///    数据目录 = 用户真实 app_data——dev 的数据就是用户数据，不得被劫走；
/// 1. 其余情况数据目录恒为 `app_data_dir()`（安装版落点，程序目录与数据目录
///    彻底分离）；app_data 不可用时退 `std::env::temp_dir()`。
///
/// 测试进程的隔离数据目录：**每个进程一个**。
///
/// 为什么按进程而不是全测试共用一个：nextest 是进程级并行，共用一个
/// wmessage.db / bot-config.json 会互相抢 SQLite 写锁（busy_timeout 只有 2s），
/// 表现为随机失败的 flaky 用例（实测 `memory_conflict` 改口裁决挂过一次）。
///
/// 顺带清陈旧目录：按进程建目录意味着每轮测试会留下上千个 temp 目录
/// （每进程一个）。建目录时把 1 小时前的老目录删掉——单个测试进程活几秒、
/// 一轮全量 30 秒内，1 小时前的必然是上一轮遗留的垃圾。
fn test_data_dir() -> std::path::PathBuf {
    use std::time::{Duration, SystemTime};
    let root = std::env::temp_dir();
    let mine = format!("wm-cargo-data-{}", std::process::id());
    let dir = root.join(&mine);
    if let Ok(entries) = std::fs::read_dir(&root) {
        let stale_before = SystemTime::now().checked_sub(Duration::from_secs(3600));
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("wm-cargo-data-") || name == mine {
                continue;
            }
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .zip(stale_before)
                .map(|(m, cutoff)| m < cutoff)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    dir
}

/// 数据目录恒为 `app_data_dir()`（安装版落点）；exe 目录是否可写、exe 旁有无
/// 历史数据痕迹（wmessage.db / AI_Gen_Files）均不影响判定——数据目录与程序
/// 目录彻底分离，程序目录的卸载/升级不再牵连数据。仅两分支：
/// ① exe 在 cargo target 下：deps = 测试进程 → temp 每进程隔离；debug|release
///    本体 = cargo run / tauri dev → 真实 app_data（dev 的数据就是用户数据）。
/// ② 其余情况 → app_data_dir()，不可用时退系统 temp。
///
/// `pub(crate)`：bot 降级 key 路径（无 AppHandle）复用同一数据目录策略。
pub(crate) fn probe_dir(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    // Cargo 构建产物（exe 在 target/.../{deps,debug,release}）：旁路便携分支，
    // 防 dev 数据落 target/（打包装箱可能带数据）。两级区分：
    // - deps/ = cargo test 测试进程 → 数据隔离到 temp 固定子目录（mock runtime
    //   的 app_data 解析到真实用户数据根，测试库就写进用户数据目录旁了）；
    // - debug|release 本体 = cargo run / tauri dev / 手动运行 → 数据走真实
    //   app_data（dev 的数据就是用户数据，不得被隔离劫走）。
    if let Some(dir) = exe_dir {
        if is_cargo_target_dir(dir) {
            let is_test_process = dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "deps")
                .unwrap_or(false);
            if is_test_process {
                return test_data_dir();
            }
            return app_data.unwrap_or_else(std::env::temp_dir);
        }
    }
    app_data.unwrap_or_else(std::env::temp_dir)
}

/// 进程级定版入口：首次 `probe_log_dir` 调用把结果钉在 `PROBE_CACHE`，后续
/// 即使探测条件变化也不再翻转——杀软瞬时锁定/UAC 抖动不得让数据目录分裂。
/// app_data_dir 解析失败（数据目录退系统 temp）时记 WARN 审计。
///
/// 测试期已知共享面：nextest 每测试一进程，测试构建下数据目录解析到
/// `target/debug/deps/`，同 pid 的进程内并行测试会共享 bot.log/DB 等——
/// 曾实证打中 profile.json（已按 pid 隔离），其余文件多轮全绿未处理。
/// 若再现 flake：先定位具体调用点，再在「按测试隔离（治本）/窄串行锁（补丁）」
/// 中选档；三档方案与实证记录见 git log。
pub(crate) fn probe_log_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let app_data = app.path().app_data_dir().ok();
    // 首次探测判定（WARN 只在定版那次记）：生产读缓存是否已初始化；
    // 测试构建无缓存，每次现探且不为翻转记 WARN（测试不验证日志副作用）
    #[cfg(not(test))]
    let first_probe = PROBE_CACHE.get().is_none();
    #[cfg(test)]
    let first_probe = false;
    // app_data 可用性快照：下一行会把 app_data 移入 probe_dir_cached
    let app_data_ok = app_data.is_some();
    let resolved = probe_dir_cached(exe_dir.as_deref(), app_data);
    // 兜底判定：app_data_dir 解析失败（数据目录退系统 temp，易失）→ 记 WARN（只在首次探测记一次）。
    // 写日志挪到独立线程：调用方可能正持有 BOT_LOG_LOCK（audit_log → data_dir → here），
    // std Mutex 不可重入，直接写会死锁。
    if first_probe && !app_data_ok {
        let resolved_s = resolved.to_string_lossy().to_string();
        let dir = resolved.clone();
        std::thread::spawn(move || {
            write_fallback_warn(
                &dir,
                "data_dir_fallback",
                &[
                    ("resolved", resolved_s.as_str()),
                    (
                        "reason",
                        "app_data_dir 解析失败，数据目录退系统临时目录（易失，重启即丢）",
                    ),
                ],
            );
        });
    }
    resolved
}

/// 已定版的数据目录（降级 key 路径等无 AppHandle 调用方复用，
/// 保证与 probe_log_dir 同一份结果；未初始化 = 主流程还没探测过，返回 None）。
///
/// 测试构建无缓存（见 PROBE_CACHE 注释），恒 None → 调用方走原现探逻辑。
#[cfg(not(test))]
pub(crate) fn cached_probe_dir() -> Option<std::path::PathBuf> {
    PROBE_CACHE.get().cloned()
}

/// 测试构建桩：无进程级缓存，恒 None
#[cfg(test)]
pub(crate) fn cached_probe_dir() -> Option<std::path::PathBuf> {
    None
}

// 运行期文件目录（runtime/flags）

/// 运行期状态根目录名：`{data_dir}/runtime`。
pub const RUNTIME_SUBDIR: &str = "runtime";
/// 运行期开关/令牌子目录名：`{data_dir}/runtime/flags`。
pub const FLAGS_SUBDIR: &str = "flags";

/// 旧版本散落在数据目录根的运行期文件（迁移到 runtime/flags）。
pub const LEGACY_ROOT_RUNTIME_FILES: &[&str] = &[
    "api-token.txt",
    "api-enabled.flag",
    "py-enabled.flag",
    "bot-enabled.flag",
];

/// 运行期状态目录：`{data_dir}/runtime`。
/// flag / token 这类「机器生成、用户不看」的文件统一收口进来，
/// 数据目录根只留用户可见的配置与数据（wmessage.db / bot-config.json / AI_Gen_Files…）。
pub fn runtime_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    crate::db::data_dir(app).join(RUNTIME_SUBDIR)
}

/// 运行期 flag 目录：`{data_dir}/runtime/flags`（api-token.txt / *-enabled.flag）。
pub fn flags_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    runtime_dir(app).join(FLAGS_SUBDIR)
}

/// 启动时一次性迁移：数据目录根的老运行期文件 → runtime/flags。
/// 返回实际移动的文件名（调用方记审计）。幂等：
/// - 源不存在 → 跳过；
/// - 目标已存在（新版本已写过）→ 不覆盖，跳过（源文件留在原位不删）；
/// - 单文件失败 → 保留原位下次启动再试，不影响其余文件。
pub fn migrate_legacy_runtime_files<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Vec<&'static str> {
    migrate_legacy_runtime_files_in(&crate::db::data_dir(app), &flags_dir(app))
}

/// 可测内核：显式传入根目录/flags 目录（不依赖 AppHandle + 便携探测）。
fn migrate_legacy_runtime_files_in(
    root: &std::path::Path,
    flags: &std::path::Path,
) -> Vec<&'static str> {
    let mut moved = Vec::new();
    for name in LEGACY_ROOT_RUNTIME_FILES {
        let src = root.join(name);
        let dst = flags.join(name);
        if !src.exists() || dst.exists() {
            continue;
        }
        if std::fs::create_dir_all(flags).is_err() {
            break;
        }
        if std::fs::rename(&src, &dst).is_ok() {
            moved.push(*name);
        }
    }
    moved
}

// 内部：缓存 + 可测内核

/// 探测结果缓存（进程级 OnceLock）：首次 probe_log_dir 调用定版。
/// 测试构建不缓存——同进程多测试各自探测不同临时目录/模拟 exe 目录，
/// 全局缓存会让先跑的测试劫持后续所有结果；
/// 定版语义由可注入内核 probe_dir_cached_in 的单测覆盖。
#[cfg(not(test))]
static PROBE_CACHE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

#[cfg(not(test))]
fn probe_dir_cached(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    probe_dir_cached_in(&PROBE_CACHE, exe_dir, app_data)
}

/// 测试构建：不缓存，每次现探（测试隔离优先）
#[cfg(test)]
fn probe_dir_cached(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    probe_dir(exe_dir, app_data)
}

/// 可测内核：OnceLock 定版语义——首次探测结果钉死，后续调用条件变化也不再翻转
fn probe_dir_cached_in(
    cache: &std::sync::OnceLock<std::path::PathBuf>,
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    cache.get_or_init(|| probe_dir(exe_dir, app_data)).clone()
}

// 内部：判定辅助

/// Cargo 构建/测试环境判定：路径含 `target/<...>` 且末段是 deps/debug/release。
/// 命中此分支说明进程是 `cargo test` / `cargo run` / `tauri build` 后的可执行——
/// 「可写」是构建副产物，不是用户真数据目录。必须旁路（否则 cargo test 写的
/// wmessage.db 残留在 target/，打包工作流把 release 目录装箱就「带数据」：
/// 曾有用户解压新版本看到旧任务卡）。
fn is_cargo_target_dir(dir: &std::path::Path) -> bool {
    let s = dir.to_string_lossy();
    if !s.contains("target/") && !s.contains("target\\") {
        return false;
    }
    matches!(
        dir.file_name().and_then(|n| n.to_str()),
        Some("deps" | "debug" | "release")
    )
}

// 内部：fallback WARN 内联

/// app_data_dir 解析失败时内联写一条 WARN 审计（行格式与 audit 模块的 write_at 完全一致）。
///
/// 必须内联在 paths 里：audit 写日志依赖 paths 的目录解析，paths 反向调 audit
/// 会构成 `audit → paths → audit` 的循环依赖。重复拼装格式代价 10 行，
/// 但保住了模块边界 + 单向依赖（audit 只读 paths 的目录 API，不反过来）。
///
/// 行格式：`[ts] WARN | event | k=v | k=v` —— 与 audit::write_at 输出字符级一致，
/// 行解析端（log viewer / 抓 log 的 grep 工具）无感。
///
/// 落盘路径 = `dir/bot.log`；写失败走 stderr 提示，审计丢了至少可诊断，不 panic。
fn write_fallback_warn(dir: &std::path::Path, event: &str, kv: &[(&str, &str)]) {
    use std::io::Write;

    let p = dir.join("bot.log");
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    // 与 audit::build_event_line 同款拼装—— kv value 过 log_sanitize::escape_for_log
    // （强版：`|` 倍增 + 控制字符全集显式转义，防 value 撕裂/伪造日志行）
    let mut line = format!("[{}] WARN | {}", ts, event);
    for (k, v) in kv {
        // usize::MAX = 不截断：fallback 值本身短，截断语义留给调用方
        let esc = crate::bot::log_sanitize::escape_for_log(v, usize::MAX);
        line.push_str(&format!(" | {}={}", k, esc));
    }
    // 创建即 0600（与 audit::open_log_append 一致）；写失败 eprintln 不阻塞
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let open = opts.open(&p);
    #[cfg(unix)]
    if open.is_ok() {
        use std::os::unix::fs::PermissionsExt;
        // mode() 只在新建时生效——已存在文件补 chmod（幂等 best-effort）
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    match open {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                eprintln!("[paths] fallback_warn write fail: {e} path={}", p.display());
            }
        }
        Err(e) => {
            eprintln!("[paths] fallback_warn open fail: {e} path={}", p.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // ── probe_dir 三分支 ──

    #[test]
    fn probe_dir_cached_is_stable_across_calls() {
        // OnceLock 定版后，第二次调用即使探测条件
        // 变化（传入不同 exe_dir）也返回首次结果——运行期数据目录不再翻转。
        // 用独立 OnceLock 实例，不碰进程级全局缓存（防劫持其他测试）
        let cache = std::sync::OnceLock::new();
        let first = probe_dir_cached_in(
            &cache,
            None,
            Some(std::path::PathBuf::from("/tmp/wm-probe-cache-test")),
        );
        let second = probe_dir_cached_in(&cache, Some(Path::new("/nonexistent-exe-dir")), None);
        assert_eq!(first, second, "定版后探测条件变化不得改变数据目录");
    }

    #[test]
    fn probe_dir_returns_app_data_when_exe_dir_writable() {
        // 安装版口径：exe 目录可写也不锚定 exe 目录——数据目录恒 app_data，
        // 程序目录只放程序（卸载/升级/重装不牵连数据）。
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path());
    }

    #[test]
    fn probe_dir_exe_under_system_temp_falls_back_to_app_data() {
        // zip 内直接双击运行：exe 落系统 temp → 不在 temp 建数据，退化 app_data
        let exe_dir = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path());
        assert!(!exe_dir.path().join(".wm-write-probe").exists());
    }

    #[test]
    fn probe_dir_ignores_legacy_data_traces_beside_exe() {
        // exe 旁有老现场的数据痕迹（wmessage.db / AI_Gen_Files）也不再锚定 exe 目录——
        // 数据目录恒 app_data，程序目录与数据目录彻底分离。
        let exe_dir = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join("wmessage.db"), b"").unwrap();
        std::fs::create_dir_all(exe_dir.path().join("AI_Gen_Files")).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path(), "历史数据痕迹不再锚定数据目录");
    }

    #[test]
    fn probe_dir_no_exe_no_app_data_falls_back_to_temp() {
        // 分支 3：exe 目录不可写 且 app_data_dir 不可用 → 退系统临时目录
        let got = probe_dir(None, None);
        assert_eq!(got, std::env::temp_dir());
    }

    #[test]
    fn probe_log_dir_cargo_test_bypasses_portable_branch() {
        // cargo test 下的 exe_parent = target/debug/deps，沿用便携分支会把
        // wmessage.db 留在 target/（打包工作流可能带数据装箱），必须旁路。
        // 隔离口径：也不得退真实 app_data——mock runtime 的 app_data_dir 解析到
        // 真实 `~/Library/Application Support` 根（无 identifier），直接返回会把
        // 测试库写进用户数据目录旁（已实证：trace 测试会话写满 wmessage.db）。
        // 钉到 temp 固定子目录。
        let app = tauri::test::mock_app();
        let got = probe_log_dir(app.handle());
        let exe_parent = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_ne!(
            got, exe_parent,
            "cargo target 路径下不应走便携分支（污染 build artifact）"
        );
        assert_ne!(
            got,
            app.path().app_data_dir().unwrap_or_else(|_| got.clone()),
            "不得退真实 app_data（测试库会写进用户数据目录旁）"
        );
        assert_eq!(
            got,
            std::env::temp_dir().join(format!("wm-cargo-data-{}", std::process::id())),
            "必须钉在 temp 的每进程隔离子目录（并行测试不能共用一个库）"
        );
    }

    #[test]
    fn probe_dir_dev_binary_in_target_debug_is_not_isolated() {
        // 回归：cargo run / tauri dev 的 exe 在 target/debug（非 deps），
        // 数据目录必须是用户真实 app_data——隔离一刀切曾把 dev 数据目录
        // 劫到 temp，程序页上架找不到脚本（AppRunResult 空跑）。
        let debug_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let got = probe_dir(
            Some(&debug_dir),
            Some(std::path::PathBuf::from("/fake/app-data")),
        );
        assert_eq!(
            got,
            std::path::PathBuf::from("/fake/app-data"),
            "dev 本体不得被隔离劫走"
        );
    }

    // ── fallback WARN 内联：行格式字符级一致 audit::write_at ──

    #[test]
    fn write_fallback_warn_format_matches_audit() {
        // 内联版本必须与 audit::write_at 行格式字符级一致：
        // `[ts] WARN | event | k=v | k=v\n`
        // 验证三件事：(a) 行首 ts 格式 (b) WARN 级别 (c) kv 拼装含分隔符
        let tmp = tempfile::tempdir().unwrap();
        write_fallback_warn(
            tmp.path(),
            "data_dir_fallback",
            &[
                ("exe_dir", "/a/b/c"),
                ("resolved", "/x/y/z"),
                ("reason", "exe 目录写探针失败"),
            ],
        );
        let content = std::fs::read_to_string(tmp.path().join("bot.log")).unwrap();
        let line = content.lines().next().expect("必须写出至少一行");
        // (a) ts 形如 `[2026-09-13 12:34:56.789]`：分钟级别即视为合规
        assert!(line.starts_with('['), "行首应为 '[ts]'，got: {line:?}");
        let ts_end = line.find(']').unwrap();
        assert!(
            line[1..ts_end].len() >= "2026-09-13 12:34:56.789".len() - 4,
            "ts 长度异常: {:?}",
            &line[1..ts_end]
        );
        // (b) WARN 级别
        assert!(
            line[ts_end..].starts_with("] WARN | "),
            "级别必须 WARN: {line:?}"
        );
        // (c) event + kv 三段以 " | " 分隔
        assert!(
            line.contains(" | data_dir_fallback | "),
            "event 段缺失: {line:?}"
        );
        assert!(
            line.contains(" | exe_dir=/a/b/c"),
            "exe_dir 段缺失: {line:?}"
        );
        assert!(
            line.contains(" | resolved=/x/y/z"),
            "resolved 段缺失: {line:?}"
        );
        assert!(line.contains(" | reason="), "reason 段缺失: {line:?}");
        // (d) 末尾 \n
        assert!(
            line.ends_with('\n') || content.ends_with('\n'),
            "行末换行缺失"
        );
    }

    #[test]
    fn fallback_warn_kv_value_escapes_newline_and_pipe() {
        // fallback_warn 的 kv value 走 log_sanitize::escape_for_log（usize::MAX 不截断）：
        // \n → \\n，"| " → "||  "，与 bot.log 同一转义纪律
        let esc = |s: &str| crate::bot::log_sanitize::escape_for_log(s, usize::MAX);
        assert_eq!(esc("a\nb"), "a\\nb");
        assert_eq!(esc("a| b"), "a||  b");
        assert_eq!(esc("clean"), "clean");
    }

    // ── runtime/flags 收口 ──

    #[test]
    fn flags_dir_sits_under_data_dir_runtime_flags() {
        let app = tauri::test::mock_app();
        let flags = flags_dir(app.handle());
        assert!(
            flags.ends_with(std::path::Path::new("runtime").join("flags")),
            "flags 目录必须落在 runtime/flags 下，got {}",
            flags.display()
        );
        assert!(
            flags.starts_with(crate::db::data_dir(app.handle())),
            "flags 目录必须仍在数据目录内，got {}",
            flags.display()
        );
    }

    #[test]
    fn migrate_legacy_runtime_files_moves_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let flags = root.join("runtime").join("flags");
        std::fs::write(root.join("api-token.txt"), b"tok").unwrap();
        std::fs::write(root.join("bot-enabled.flag"), b"1").unwrap();
        // 目标已存在（新版本已写过）：不得覆盖，源文件保持原位
        std::fs::create_dir_all(&flags).unwrap();
        std::fs::write(flags.join("py-enabled.flag"), b"new").unwrap();
        std::fs::write(root.join("py-enabled.flag"), b"old").unwrap();

        let mut moved = migrate_legacy_runtime_files_in(root, &flags);
        moved.sort_unstable();
        assert_eq!(moved, vec!["api-token.txt", "bot-enabled.flag"]);
        assert_eq!(
            std::fs::read_to_string(flags.join("api-token.txt")).unwrap(),
            "tok"
        );
        assert!(!root.join("api-token.txt").exists(), "源文件应已移走");
        assert_eq!(
            std::fs::read_to_string(flags.join("py-enabled.flag")).unwrap(),
            "new",
            "目标已存在时不得被旧文件覆盖"
        );
        assert!(
            root.join("py-enabled.flag").exists(),
            "未迁移的源文件保持原位"
        );
        // 幂等：再跑一次无迁移
        assert!(migrate_legacy_runtime_files_in(root, &flags).is_empty());
    }
}
