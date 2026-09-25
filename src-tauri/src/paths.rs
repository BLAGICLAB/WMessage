//! 数据目录解析与便携模式判定（从 audit.rs 抽出，独立模块）
//!
//! 抽出的动机：原 audit.rs 同时承担两件事——
//! 1) **目录解析**（probe_log_dir / cached_probe_dir / probe_dir / 辅助函数 + 缓存）；
//! 2) **结构化审计日志**（write_event / write_error_audit / write_warn_audit_to / 行拼装）。
//! 现状 `db.rs / profile.rs / bot.rs / middleware.rs` 引用 audit 只是为了拿目录，
//! 属于 SRP 违例（audit 改了行格式，所有调用方都要重审）。
//!
//! 拆出 paths 后：
//! - paths 只负责「目录怎么定」，调用方拿 `PathBuf`；
//! - audit 只负责「目录定了之后日志怎么写」，不需要知道便携策略；
//! - 未来加第二条便携策略（如 per-user config）只动 paths。
//!
//! 内联 fallback WARN：探测到 exe 目录写探针失败 / 落在系统 temp 时需要在数据
//! 目录里写一条 WARN 审计。审计模块（audit）写日志依赖 paths 的目录解析——
//! 抽到 paths 后 audit 不能反向调 paths → 在 paths 内联一份最小化的 WARN 写入，
//! 格式与 audit 完全一致（同样 `[ts] WARN | event | k=v`），行解析端无感。

use tauri::Manager;

/// 数据目录解析公共入口（db / profile / audit / bot 全靠它定版）。
///
/// 三分支：
/// 1. exe 目录可写 + 无数据痕迹 → 便携模式（数据随 exe 走，U盘/绿色包）；
/// 2. exe 目录不可写（如 Program Files） → 退 app_data_dir（系统应用数据目录）；
/// 3. 全不可用 → 退 `std::env::temp_dir()`。
///
/// 前置规则：
/// 1. exe 旁已有 `wmessage.db` / `AI_Gen_Files` → 强制便携锚定 exe 目录；
///    写探针瞬时失败（杀软锁定/UAC 抖动）不得翻转已有数据目录。
/// 2. exe 落在系统临时目录（压缩包内直接双击运行）→ 不在 temp 建数据目录，
///    跳过便携分支退化 app_data。
/// 3. macOS `.app` 包内目录（`*.app/Contents/MacOS`）即使可写也不算便携目录——
///    拖到 `~/Applications` 后该目录可写，DB / 日志 / AI_Gen_Files 全写进 app 包
///    会破坏签名、删 app = 删全部用户数据。
///
/// `pub(crate)`：bot.rs 降级 key 路径（无 AppHandle）复用同一便携策略。
pub(crate) fn probe_dir(
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(dir) = exe_dir {
        if !is_macos_app_bundle_dir(dir) {
            if dir.join("wmessage.db").exists() || dir.join("AI_Gen_Files").exists() {
                return dir.to_path_buf();
            }
            if !is_under_system_temp(dir) {
                let probe = dir.join(".wm-write-probe");
                if std::fs::File::create(&probe).is_ok() {
                    let _ = std::fs::remove_file(&probe);
                    return dir.to_path_buf();
                }
            }
        }
    }
    app_data.unwrap_or_else(std::env::temp_dir)
}

/// 进程级定版入口：首次 `probe_log_dir` 调用把结果钉在 `PROBE_CACHE`，后续
/// 即使探测条件变化也不再翻转——杀软瞬时锁定/UAC 抖动不得把当次数据目录翻到
/// `~/Library/Application Support/...`，AI_Gen_Files、数据库、日志分裂。
///
/// 兜底分支发生时记一条 WARN 审计（写清翻到哪、为什么），可诊断。
///
/// # 测试期的跨进程共享
///
/// 留档观察，未下沉到依赖容器。pid 隔离在 profile.json 上有效，
/// 其余文件暂无实证。详见下方 `probe_log_dir` 整段 doc（三档方案 + 实证 + 代价）。
///
/// `cargo nextest run` 是**每测试一进程**，而本函数在测试构建下解析到
/// `current_exe().parent()` = `target/debug/deps/` —— 于是同一份 `bot.log`（审计追加）、
/// `wmessage.db`、`api-enabled.flag` / `bot-enabled.flag`、降级 key 文件会被多个测试进程共享。
///
/// 现状与证据：
/// - `profile.json` 已按 pid 隔离（`profile.rs::test_isolated_dir`），这是唯一**被实证打中**的
///   共享文件（`cargo nextest run` 曾 6 次复现 profile 用例随机挂，修后 5×nextest 全绿）；
/// - 上列其余文件**5 次 nextest 未出现失败**，故暂不处理（不扩大范围，等实锤）。
///
/// 追加实证（**进程内**并行，与 nextest 不同层）：
/// `cargo test --lib`（同进程多线程）下 `exit_cleanup_tests::cleanup_on_exit_releases_api_and_skill`
/// 失败 **1 次**：`lib.rs:813`「退出路径不得清 api-enabled.flag」。
/// - 该用例单跑 3/3 通过；其后连跑 16 轮全套 lib（6 + 10）**全绿**；
/// - **机制未定位**，已排除：exit 路径自身（`api_stop_for_exit` → `clear_enabled=false`）、
///   测试直调 `api_stop`/`api_status`（全仓无调用点）、`migration` 测试（各自 temp 目录）、
///   `profile.rs::fresh_app`（只删 `profile*`）、`paths.rs` 测试（各自 uuid 子目录）；
///   剩余嫌疑是「某并行用例对共享 data 目录的写/删」，但未找到具体调用点。
/// - **关键结论**：这类失败发生在**同进程并行**，B1/B2/B3（按 **pid** 隔离）**覆盖不到它**
///   ——同 pid 的测试本就共享一个目录。要治它需要**按测试**隔离（当前架构没有该能力，
///   例如给这些文件加测试专用 override hook），或对相关用例加一把窄串行锁（打补丁）。
///   nextest 是每测试一进程，因此天然避开这一类（这解释了 5×nextest 全绿）。
///
/// 若将来要彻底隔离，候选方案（本次评估的价格）：
/// - **B1**：本函数在 `cfg(test)` 下返回 `.../deps/<pid>/`。代价：
///   `probe_log_dir_matches_exe_parent_in_cargo_test` 的 `assert_eq!` 须改弱为
///   「前缀 + pid 后缀」；`profile.rs::test_isolated_dir` 变冗余应删（消掉「profile 数据目录
///   ≠ 审计目录」这处不一致）；`target/debug/deps` 下目录/文件累积从「十位数/次」升到
///   「百位数/次」（nextest 每进程一套）。
/// - **B2**：B1 + 保守清理（首次调用删 mtime > 1h 的同名前缀目录；只删"肯定已死"的，
///   避免删到在跑进程的目录反而制造更凶的 flake）。
/// - **B3**：连集成测试一起管（本函数读 `WMESSAGE_TEST_DATA_DIR` env 覆盖 + nextest 配置注入）。
///   注意：集成测试（`tests/*.rs`）的 lib 按**发布语义**编译，看不到 `cfg(test)` 分支，
///   所以 B1/B2 只修 lib 单测那一半；且 nextest 配置文件位置依赖 cwd（仓库根 vs `src-tauri/`），
///   较脆弱。
///
/// 触发条件：出现**实证**失败（哪个用例、哪条断言、哪个共享文件）→ 再按上表选档。
///
/// 决策：只留档、不盲改 —— 上面那条进程内实证尚未定位到具体
/// 调用点，无靶点的修改等于猜；下次复现时先定位「哪个调用点删/写了哪个文件」，再按
/// 进程间走 B1/B2/B3、进程内走「按测试隔离（治本）」或「窄串行锁（打补丁）」选档。
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
    let resolved = probe_dir_cached(exe_dir.as_deref(), app_data);
    // 兜底判定：exe 目录存在但结果不是它 → 发生了翻转，记 WARN（只在首次探测记一次）。
    // 写日志挪到独立线程：调用方可能正持有 BOT_LOG_LOCK（audit_log → data_dir → here），
    // std Mutex 不可重入，直接写会死锁。
    if first_probe && exe_dir.as_deref().is_some_and(|d| d != resolved.as_path()) {
        let exe_dir_s = exe_dir
            .as_deref()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        let resolved_s = resolved.to_string_lossy().to_string();
        // zip 直跑（exe 在系统 temp 下）与写探针失败给不同 WARN 原因，便于诊断
        let reason = if exe_dir.as_deref().is_some_and(is_under_system_temp) {
            "exe 位于系统临时目录（疑似压缩包内直接双击运行），不在 temp 建数据目录，已退化 app_data——请解压后再运行"
        } else {
            "exe 目录写探针失败（杀软锁定/权限不足/压缩包内运行），数据目录按便携策略兜底"
        };
        let dir = resolved.clone();
        std::thread::spawn(move || {
            write_fallback_warn(
                &dir,
                "data_dir_fallback",
                &[
                    ("exe_dir", exe_dir_s.as_str()),
                    ("resolved", resolved_s.as_str()),
                    ("reason", reason),
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

// ───────────────────────── 运行期文件目录（runtime/flags） ─────────────────────────

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

// ───────────────────────── 内部：缓存 + 可测内核 ─────────────────────────

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
#[cfg_attr(not(test), allow(dead_code))] // 生产只经 probe_dir_cached 间接调用
fn probe_dir_cached_in(
    cache: &std::sync::OnceLock<std::path::PathBuf>,
    exe_dir: Option<&std::path::Path>,
    app_data: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    cache.get_or_init(|| probe_dir(exe_dir, app_data)).clone()
}

// ───────────────────────── 内部：判定辅助 ─────────────────────────

/// exe 目录是否在系统临时目录下（zip 直跑场景）：canonicalize 后比较，
/// macOS /var ↔ /private/var 软链由 canonicalize 归一
fn is_under_system_temp(dir: &std::path::Path) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = std::fs::canonicalize(&tmp).unwrap_or(tmp);
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    dir.starts_with(&tmp)
}

/// macOS .app 包内 MacOS 目录判定：…/Xxx.app/Contents/MacOS
fn is_macos_app_bundle_dir(dir: &std::path::Path) -> bool {
    let mut comps = dir.components().rev();
    matches!(comps.next(), Some(c) if c.as_os_str() == "MacOS")
        && matches!(comps.next(), Some(c) if c.as_os_str() == "Contents")
        && comps
            .next()
            .is_some_and(|c| c.as_os_str().to_string_lossy().ends_with(".app"))
}

// ───────────────────────── 内部：fallback WARN 内联 ─────────────────────────

/// 探测翻转时内联写一条 WARN 审计（行格式与 audit 模块的 write_at 完全一致）。
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
    // 与 audit::build_event_line 同款拼装—— kv value 转义后 ` | ` 分隔
    // （escape: 剥 \n / |，避免 value 里的换行/管道撕裂日志行）
    let mut line = format!("[{}] WARN | {}", ts, event);
    for (k, v) in kv {
        let esc = escape_for_log_inline(v);
        line.push_str(&format!(" | {}={}", k, esc));
    }
    // 创建即 0600（与 audit::open_log_append 一致）；写失败 eprintln 不阻塞
    let open = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p);
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

/// 与 audit::escape_for_log 等价的内联版：剥 value 里的 `\n` / `| `。
/// 重复实现是因为这条路径不进 audit 模块（避免循环依赖），格式必须
/// 与 audit::escape_for_log 字符级一致——单测 `kv_value_pipe_space_and_newline_stay_escaped`
/// 锁住两路输出不可漂移。
fn escape_for_log_inline(s: &str) -> String {
    // 与 audit::escape_for_log 字符级一致（NEW-1345 后 = 控制字符全集 + U+2028/2029
    // + `|` 倍增 + `| ` 预替换）。不直接复用是避免 db→bot/config 循环依赖。
    let mut out = String::with_capacity(s.len());
    for ch in s.replace("| ", "|  ").chars() {
        match ch {
            '|' => out.push_str("||"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u{2028}"),
            '\u{2029}' => out.push_str("\\u{2029}"),
            c if c.is_control() => out.extend(c.escape_default()),
            c => out.push(c),
        }
    }
    out
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

    /// mock exe 目录：系统 temp 下的目录会被 probe_dir 当「压缩包直跑」跳过便携分支，
    /// 测试用 exe 目录一律建在 current_exe 父目录（target/debug/deps，可写且非 temp）
    fn mock_dir_outside_temp(name: &str) -> std::path::PathBuf {
        let base = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .join(format!(
                "wm-probe-test-{}-{}",
                name,
                uuid::Uuid::new_v4().simple()
            ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn probe_dir_writable_exe_dir_wins() {
        // 分支 1：exe 目录可写 → 用它（便携模式）
        let exe_dir = mock_dir_outside_temp("writable");
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(&exe_dir), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir);
        // 探针文件不得残留
        assert!(!exe_dir.join(".wm-write-probe").exists());
        let _ = std::fs::remove_dir_all(&exe_dir);
    }

    #[cfg(unix)]
    #[test]
    fn probe_dir_readonly_exe_dir_falls_back_to_app_data() {
        // 分支 2：exe 目录不可写（如 Program Files）→ 退 app_data_dir
        use std::os::unix::fs::PermissionsExt;
        let ro = mock_dir_outside_temp("readonly");
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(&ro), Some(app_data.path().to_path_buf()));
        assert_eq!(got, app_data.path());
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&ro);
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

    #[cfg(unix)]
    #[test]
    fn probe_dir_existing_data_traces_force_portable() {
        // exe 旁已有 wmessage.db → 强制便携锚定，目录即使不可写也不翻转
        use std::os::unix::fs::PermissionsExt;
        let exe_dir = tempfile::tempdir().unwrap();
        std::fs::write(exe_dir.path().join("wmessage.db"), b"").unwrap();
        std::fs::set_permissions(exe_dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir.path());
        std::fs::set_permissions(exe_dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn probe_dir_gen_dir_trace_forces_portable_under_temp() {
        // exe 在 temp 但旁边已有 AI_Gen_Files → 数据痕迹优先于 temp 规避，锚定 exe 目录
        let exe_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(exe_dir.path().join("AI_Gen_Files")).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let got = probe_dir(Some(exe_dir.path()), Some(app_data.path().to_path_buf()));
        assert_eq!(got, exe_dir.path());
    }

    /// macOS .app 包内目录（即使可写）不得当便携数据目录——
    /// 否则数据库/日志/AI_Gen_Files 全写进 app 包内（删 app = 删全部数据）
    #[test]
    fn probe_dir_skips_macos_app_bundle_dir() {
        let tmp = std::env::temp_dir().join(format!("wm-test-{}", uuid::Uuid::new_v4().simple()));
        let macos_dir = tmp.join("wmessage.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos_dir).unwrap();
        let app_data = tmp.join("appdata");
        let got = probe_dir(Some(&macos_dir), Some(app_data.clone()));
        assert_eq!(got, app_data, ".app 包内目录必须跳过便携分支直落 app_data");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_macos_app_bundle_dir_detection() {
        assert!(is_macos_app_bundle_dir(Path::new(
            "/Applications/wmessage.app/Contents/MacOS"
        )));
        assert!(is_macos_app_bundle_dir(Path::new(
            "/Users/x/Applications/wmessage.app/Contents/MacOS/"
        )));
        assert!(!is_macos_app_bundle_dir(Path::new(
            "/Applications/wmessage.app/Contents"
        )));
        assert!(!is_macos_app_bundle_dir(Path::new("/opt/wmessage")));
        assert!(!is_macos_app_bundle_dir(Path::new(
            "/Users/x/wmessage.app/Contents/MacOSub"
        )));
    }

    #[test]
    fn probe_dir_no_exe_no_app_data_falls_back_to_temp() {
        // 分支 3：exe 目录不可写 且 app_data_dir 不可用 → 退系统临时目录
        let got = probe_dir(None, None);
        assert_eq!(got, std::env::temp_dir());
    }

    #[test]
    fn probe_log_dir_matches_exe_parent_in_cargo_test() {
        // 调用方一致性：cargo test 下 current_exe 父目录（target/debug/deps）可写，
        // probe_log_dir 必须命中 exe 分支——与抽取前 db_dir/profile data_dir 行为一致
        let app = tauri::test::mock_app();
        let got = probe_log_dir(app.handle());
        let exe_parent = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert_eq!(got, exe_parent);
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
    fn escape_for_log_inline_strips_newline_and_pipe() {
        // 与 audit::escape_for_log 同语义：value 里的 \n → \\n，"| " → "|| "
        assert_eq!(escape_for_log_inline("a\nb"), "a\\nb");
        assert_eq!(escape_for_log_inline("a| b"), "a||  b");
        assert_eq!(escape_for_log_inline("clean"), "clean");
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
            "flags 目录必须仍在数据目录内（便携模式随 exe 走），got {}",
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
