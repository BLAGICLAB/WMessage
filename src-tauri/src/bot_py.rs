//! Python 执行基础设施：调用本机 Python 处理文档 / 运行模型生成的脚本。
//!
//! 安全设计（对齐 Harness 网关）：
//! - 开关：py-enabled.flag（设置页「允许机器人执行 Python」，默认关闭）——主要防护
//! - 受限执行：每次运行独立临时目录（数据目录 py-runs/<uuid>/），只经 stdin/文件传参，永不拼 shell；
//!   ⚠️ 不是安全沙箱：脚本以当前用户完整权限运行（可读本机文件、可联网），仅隔离工作目录
//! - 熔断：默认超时 60s 强杀（含子进程组），用户可配但硬钳上限 300s；stdout/stderr 读取时硬截断 64KB
//! - 限额：Unix setrlimit RLIMIT_AS（内存按 timeout 比例，256MB~2GB）/ RLIMIT_CPU（timeout+10s）；
//!   Windows Job Object（内存 + CPU，KILL_ON_JOB_CLOSE 兜住孙进程整树）
//! - 审计：bot.log 记录脚本摘要、耗时、退出码、输出摘要
//! - 固定脚本模板：文档处理用预写脚本（extract/make_docx/make_xlsx），模型只填参数；
//!   自由编程走 run_python（需开关开启）

use serde::Serialize;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

use crate::bot_slash::StopToken;
// NEW-C-6：escape_for_log 上提到 audit 模块共享（write_event 同源规则）
use crate::audit::escape_for_log;
// F2（Phase 6b）：doc_* 6 个 command 迁移到结构化错误
use crate::error::{CommandError, CommandResult};

/// Windows 上 GUI 程序调 cmd.exe / python.exe / taskkill.exe 等控制台子进程时，
/// 默认会为子进程开一个控制台窗口（即使立即退出）—— 视觉上就是"黑框闪一下"。
/// CREATE_NO_WINDOW (0x08000000) 抑制父进程继承的控制台窗口创建，是 Tauri / Electron
/// 等 GUI 框架调子进程时的标准做法。Unix 平台无此概念（不走控制台），保持直通。
///
/// 此外 Windows 上 Python 3 stdout/stderr 默认是 cp936，Rust 端按 UTF-8 解码会乱码；
/// 强制 Python 走 UTF-8，对非 Python 子进程（taskkill/sleep 等）无害（这些 env var 被忽略）。
#[cfg(windows)]
fn silent_cmd(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(0x08000000)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    cmd
}

#[cfg(not(windows))]
fn silent_cmd(program: &str) -> Command {
    Command::new(program)
}
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::AppHandle;

// ───────────────────────── 开关 ─────────────────────────

fn py_flag_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("py-enabled.flag")
}

#[tauri::command]
pub fn py_get_enabled(app: AppHandle) -> bool {
    py_flag_path(&app).exists()
}

/// 批次7审计 P2-1：错误类型对齐全量 58 个命令的 CommandError 四字段结构——
/// 原先唯一返回裸 String，前端拿不到结构化 code
#[tauri::command]
pub fn py_set_enabled(app: AppHandle, enabled: bool) -> CommandResult<bool> {
    let dir = crate::db::data_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    if enabled {
        std::fs::write(py_flag_path(&app), b"1")
            .map_err(|e| CommandError::IoError(e.to_string()))?;
    } else {
        let _ = std::fs::remove_file(py_flag_path(&app));
    }
    Ok(enabled)
}

// ───────────────────────── 环境检测 ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyEnv {
    pub available: bool,
    pub python: String,
    pub version: String,
    pub libs: Vec<String>,
}

/// 带超时的版本探测（P2-10）：PATH 里的 python 可能是损坏 shim，
/// 原先 `.output()` 无超时会把探测本身卡死；超过 3s 不退出就杀掉按失败处理
fn probe_version_ok(program: &str, args: &[&str]) -> bool {
    probe_version_ok_with(program, args, Duration::from_secs(3))
}

fn probe_version_ok_with(program: &str, args: &[&str], timeout: Duration) -> bool {
    let mut child = match silent_cmd(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// 检测本机 Python（macOS/Linux: python3/python；Windows: python/python3/py -3）
/// 每个候选最多 3s（P2-10 探测超时），卡死的 shim 直接跳过
pub fn detect_python() -> Option<String> {
    #[cfg(windows)]
    let candidates: &[&str] = &["python", "python3"];
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let candidates: &[&str] = &["python3", "python"];
    // 批次6审计 P2：macOS GUI（Finder 双击）启动 PATH 极简（/usr/bin:/bin:…），
    // brew 装的 python3 探测不到 → 补固定路径候选（不存在由 3s 探针超时跳过）
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "python3",
        "python",
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3",
    ];
    for c in candidates {
        if probe_version_ok(c, &["--version"]) {
            return Some(c.to_string());
        }
    }
    // Windows 兜底：py 启动器
    #[cfg(windows)]
    {
        if probe_version_ok("py", &["-3", "--version"]) {
            return Some("py".to_string());
        }
    }
    None
}

/// 探测结果缓存（P2-10）：None=未探测；Some(inner)=已探测（inner 为 None 表示本机无 Python）。
/// 原先每次 run_python 都 spawn 1-3 次 `python --version`，启动延迟 + 资源浪费。
static PY_CACHE: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);

/// 实际探测次数计数（单测 spy：验证连续调用只探测一次）
static PY_PROBE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 带缓存的探测：首次真正 spawn 探测，之后直接命中缓存
fn cached_python() -> Option<String> {
    let mut g = PY_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = &*g {
        return cached.clone();
    }
    PY_PROBE_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let detected = detect_python();
    *g = Some(detected.clone());
    detected
}

/// 缓存失效（P2-10）：缓存的 python 路径 spawn 失败（NotFound，可能被删/换 PATH）
/// 后调用，下次 cached_python 重新探测
fn invalidate_python_cache() {
    *PY_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ───────────────────────── .NET 修订工具（2026-08-27）─────────────────────────

/// 检测本机 dotnet 运行时（修订版 Word 的 .NET 生成路径前置条件）：
/// `dotnet --version` 探测（复用 3s 超时探针，卡死的 shim 直接跳过）
fn detect_dotnet() -> Option<String> {
    // 批次6审计 P2：macOS GUI 启动 PATH 极简，补官方/brew 固定安装路径
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &["dotnet", "/usr/local/share/dotnet/dotnet", "/opt/homebrew/bin/dotnet"];
    #[cfg(not(target_os = "macos"))]
    let candidates: &[&str] = &["dotnet"];
    for c in candidates {
        if probe_version_ok(c, &["--version"]) {
            return Some(c.to_string());
        }
    }
    None
}

/// 探测结果缓存（与 PY_CACHE 同模式：None=未探测；Some(None)=本机无 dotnet）
static DOTNET_CACHE: std::sync::Mutex<Option<Option<String>>> = std::sync::Mutex::new(None);

fn cached_dotnet() -> Option<String> {
    let mut g = DOTNET_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = &*g {
        return cached.clone();
    }
    let detected = detect_dotnet();
    *g = Some(detected.clone());
    detected
}

/// 定位 .NET 修订工具入口：(程序, 入口 dll 参数)。
/// 优先绿色包随包 apphost exe 直跑（2026-08-28 批次6审计 P1：self-contained 发布时
/// 用户无需装 .NET；framework-dependent apphost 也会自动找到已装共享运行时）；
/// 其次 dotnet <dll>（需系统 dotnet 运行时）。
/// 返回 None = 工具未随包发布（调用方回退 Python 脚本路径）。
fn dotnet_revisions_entry() -> Option<(String, Option<String>)> {
    const REL_DLL: &str = "wm-docx-revisions.dll";
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
    {
        #[cfg(windows)]
        let tool_exe = exe_dir.join("dotnet").join("wm-docx-revisions.exe");
        #[cfg(not(windows))]
        let tool_exe = exe_dir.join("dotnet").join("wm-docx-revisions");
        if tool_exe.is_file() {
            return Some((tool_exe.to_string_lossy().into_owned(), None));
        }
        let dll = exe_dir.join("dotnet").join(REL_DLL);
        if dll.is_file() {
            return cached_dotnet().map(|d| (d, Some(dll.to_string_lossy().into_owned())));
        }
    }
    // 开发模式候选仅 debug 构建保留（批次6审计 P1）：env!("CARGO_MANIFEST_DIR")
    // 会把构建机绝对路径烧进发布二进制（信息泄露 + 发布版纯死路径）
    #[cfg(debug_assertions)]
    {
        let dll = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("dotnet/WmDocxRevisions/bin/Release/net8.0")
            .join(REL_DLL);
        if dll.is_file() {
            return cached_dotnet().map(|d| (d, Some(dll.to_string_lossy().into_owned())));
        }
    }
    None
}

/// 跑 .NET 修订工具：与 Python 同一执行内核（run_python_at：超时/限额/进程组强杀/审计），
/// 只是入口从 run.py 换成 dll（dotnet <dll> params.json）。
/// 返回 None = dotnet 或工具不可用（调用方回退 Python 脚本路径）。
fn run_dotnet_revisions(app: &AppHandle, input_json: &str) -> Option<Result<PyRunResult, String>> {
    // 批次6审计 P1：随包 exe 直跑不依赖系统 dotnet（self-contained 免装运行时）
    let (prog, entry) = dotnet_revisions_entry()?;
    // 并发闸门由调用方持有（run_doc_revisions 入口统一上锁，覆盖 dotnet + 回退 Python
    // 全程；std Mutex 不可重入，这里不能再锁）
    let dir = crate::db::data_dir(app)
        .join("py-runs")
        .join(uuid::Uuid::new_v4().simple().to_string());
    if let Err(e) = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(dir.join("params.json"), input_json))
    {
        py_audit(app, &format!("doc_revisions_dotnet err | kind=setup_fail | {e}"));
        return None; // 目录都建不了 → 回退 Python 路径更稳妥
    }
    let mut audit_sink = |line: &str| py_audit(app, line);
    let args = vec!["params.json".to_string()];
    // 首次跑要 JIT，给 120s（与 doc_* 脚本同款）
    Some(run_python_at(
        &prog, entry.as_deref(), &dir, &args, Some(120), &mut audit_sink, None,
    ).map_err(|f| f.msg))
}

/// 同步检测核心（后台线程运行）
fn py_env_check_blocking() -> PyEnv {
    let Some(py) = detect_python() else {
        return PyEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            libs: Vec::new(),
        };
    };
    let version = silent_cmd(&py)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // 单进程一次探测全部库（原 4 次独立 spawn 冷启动，同步命令下卡主线程几秒）
    let probe = r#"import importlib
for m in ["openpyxl", "docx", "pptx", "pypdf", "reportlab"]:
    try:
        importlib.import_module(m)
        print(f"{m}:OK")
    except Exception:
        print(f"{m}:缺")
"#;
    let mut libs = Vec::new();
    if let Ok(out) = silent_cmd(&py).args(["-c", probe]).output() {
        if out.status.success() {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if line.contains(':') {
                    libs.push(line.to_string());
                }
            }
        }
    }
    PyEnv {
        available: true,
        python: py,
        version,
        libs,
    }
}

/// 检查本机 Python 环境：后台线程执行，不阻塞主线程（审计：同步命令会冻结 UI）
#[tauri::command]
pub async fn py_env_check() -> PyEnv {
    tauri::async_runtime::spawn_blocking(py_env_check_blocking)
        .await
        .unwrap_or_else(|_| PyEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            libs: Vec::new(),
        })
}

// ───────────────────────── 执行核心 ─────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyRunResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    /// 输出触顶被截断（NEW-C-5）：stdout/stderr 超 OUTPUT_CAP 时为 true
    #[serde(default)]
    pub truncated: bool,
}

const OUTPUT_CAP: usize = 64 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// 超时硬钳上限（C2）：超过一律钳到 300s（py_exec_sync 层面对用户请求直接拒绝）
const MAX_TIMEOUT_SECS: u64 = 300;

/// 解析超时：None → 60s 默认；> 300s → 钳到 300s 并返回 clamped=true（C2）
fn resolve_timeout(timeout_secs: Option<u64>) -> (u64, bool) {
    match timeout_secs {
        None => (DEFAULT_TIMEOUT_SECS, false),
        Some(s) if s > MAX_TIMEOUT_SECS => (MAX_TIMEOUT_SECS, true),
        Some(s) => (s, false),
    }
}

/// 内存限额：按 timeout 比例（8MB/s），下限 256MB、上限 2GB（C2）
fn mem_limit_bytes(timeout_secs: u64) -> u64 {
    timeout_secs
        .saturating_mul(8)
        .saturating_mul(1024 * 1024)
        .clamp(256 * 1024 * 1024, 2 * 1024 * 1024 * 1024)
}

/// CPU 限额：timeout + 10s 宽限（C2）
fn cpu_limit_secs(timeout_secs: u64) -> u64 {
    timeout_secs.saturating_add(10)
}

/// Windows 资源限额：Job Object（内存 + 单进程 CPU 用户时间）。
/// KILL_ON_JOB_CLOSE：句柄关闭即整树强杀，兜住 breakaway 之外的孙进程。
#[cfg(windows)]
mod win_job {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOB_OBJECT_LIMIT_PROCESS_TIME,
    };

    pub struct JobGuard(HANDLE);

    pub fn create(mem_bytes: u64, cpu_secs: u64) -> Option<JobGuard> {
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_PROCESS_TIME
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info.ProcessMemoryLimit = mem_bytes as usize;
            // PerProcessUserTimeLimit 单位 100ns
            info.BasicLimitInformation.PerProcessUserTimeLimit =
                cpu_secs.saturating_mul(10_000_000) as i64;
            let r = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of_val(&info) as u32,
            );
            if r.is_err() {
                let _ = CloseHandle(job);
                return None;
            }
            Some(JobGuard(job))
        }
    }

    pub fn assign(job: &JobGuard, child: &std::process::Child) {
        use std::os::windows::io::AsRawHandle;
        unsafe {
            let _ = AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle() as _));
        }
    }

    pub fn terminate(job: &JobGuard) {
        unsafe {
            let _ = TerminateJobObject(job.0, 1);
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// 子进程资源限额守卫（C2）：Unix 在 spawn 时经 pre_exec 设 RLIMIT_AS / RLIMIT_CPU
///（无运行时状态）；Windows 为 Job Object（终止时 TerminateJobObject 整树杀）。
struct RunLimits {
    #[cfg(windows)]
    job: Option<win_job::JobGuard>,
}

impl RunLimits {
    fn new(_mem_bytes: u64, _cpu_secs: u64) -> Self {
        #[cfg(windows)]
        {
            Self {
                job: win_job::create(_mem_bytes, _cpu_secs),
            }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }

    /// 把子进程挂进 Job（Windows）；Unix 限额在 pre_exec 已生效，空操作
    fn assign(&self, _child: &std::process::Child) {
        #[cfg(windows)]
        if let Some(j) = &self.job {
            win_job::assign(j, _child);
        }
    }

    /// 整树终止（Windows：TerminateJobObject）；Unix 由 kill_tree 进程组杀承担，空操作
    fn terminate(&self) {
        #[cfg(windows)]
        if let Some(j) = &self.job {
            win_job::terminate(j);
        }
    }
}

/// 批次6审计 P2：kill 整组前校验目标 pid 仍是组首——退出清理路径的注册表 pid
/// 可能已被 OS 回收复用，`kill -9 -pid` 会命中无关进程组。getpgid 失败（进程已死）
/// 或返回值不等于 pid（非组首）→ 不杀。注意：只用于 kill_py_children（退出清理，
/// 距收割时间久、复用窗口真实）；kill_tree 的 drain_timeout 路径（收割后 2s 内）
/// 必须无条件组杀，否则孙进程占管道写端，reader 永不 EOF。
#[cfg(unix)]
fn is_live_group_leader(pid: u32) -> bool {
    unsafe { libc::getpgid(pid as i32) == pid as i32 }
}

/// 终止 Python 进程及其全部子进程：Unix 按进程组（spawn 时 process_group(0) 成为组首），
/// Windows 先 TerminateJobObject 整树杀（Job 覆盖孙进程）再 taskkill /T 兜底。
/// 先组杀再兜底 kill + wait。drain_timeout 收割后组首虽死，pgid 随存活孙进程仍有效，
/// 组杀必须照常执行（见 is_live_group_leader 注释的取舍说明）。
fn kill_tree(child: &mut std::process::Child, limits: &RunLimits) {
    limits.terminate();
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        let _ = silent_cmd("kill")
            .args(["-9", &format!("-{pid}")])
            .status();
    }
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = silent_cmd("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 失败路径统一收尾（NEW-C-3）：杀整树（含孙进程兜底）+ 清临时目录。
/// try_wait Err / timeout / stopped 等危险路径共用，防孤儿进程与磁盘泄漏。
fn cleanup_after_fail(child: &mut std::process::Child, dir: &std::path::Path, limits: &RunLimits) {
    kill_tree(child, limits);
    let _ = std::fs::remove_dir_all(dir);
}

/// spawn 失败收尾（P2-25）：进程没起来（无 child 可杀），但仍显式 terminate
/// 资源限额（Windows Job Object 在 spawn 前已建，不止靠 Drop 兜底）+ 清临时目录
/// —— 与 cleanup_after_fail 同族，防临时目录即时泄漏。
fn cleanup_after_spawn_fail(dir: &std::path::Path, limits: &RunLimits) {
    limits.terminate();
    let _ = std::fs::remove_dir_all(dir);
}

/// 运行目录初始化（P2-25）：建目录 + 写 run.py / params.json；任一步失败
/// 清理已建目录再返回 Err —— 原先 `?` 直返，写失败时临时目录即时泄漏。
fn setup_run_dir(
    base: &std::path::Path,
    script: &str,
    input_json: Option<&str>,
) -> Result<std::path::PathBuf, String> {
    let dir = base.join(uuid::Uuid::new_v4().simple().to_string());
    let r = (|| -> Result<(), String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("run.py"), script).map_err(|e| e.to_string())?;
        if let Some(j) = input_json {
            std::fs::write(dir.join("params.json"), j).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    r.map(|_| dir)
}

// ───────────────────────── 退出清理（P2-24）─────────────────────────

/// 在途 Python 子进程注册表（P2-24）：spawn 成功即登记 pid，运行结束（任意返回路径）
/// 经 ChildRegGuard Drop 注销。子进程不随父进程退出 —— 应用退出（ExitRequested）时
/// 按注册表整树强杀，防父进程先退留下孤儿。
static PY_CHILDREN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<u32>>> =
    std::sync::OnceLock::new();

fn py_children() -> &'static std::sync::Mutex<std::collections::HashSet<u32>> {
    PY_CHILDREN.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 注册表条目守卫：Drop 即注销，覆盖正常 / wait_fail / timeout / stopped 全部返回路径
struct ChildRegGuard(u32);

impl ChildRegGuard {
    fn register(child: &std::process::Child) -> Self {
        py_children()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(child.id());
        Self(child.id())
    }
}

impl Drop for ChildRegGuard {
    fn drop(&mut self) {
        py_children()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// 按 pid 整树强杀（可测内核，P2-24）：Unix 子进程 spawn 时 process_group(0) 自成组首，
/// `kill -9 -pid` 杀整组（含孙进程，与 kill_tree 同策略）；Windows `taskkill /T /F` 杀整树。
/// 已退出的 pid（组不存在）按未杀计，不报错。
fn kill_py_children(pids: &[u32]) -> usize {
    let mut killed = 0;
    for pid in pids {
        #[cfg(unix)]
        let ok = if is_live_group_leader(*pid) {
            silent_cmd("kill")
                .args(["-9", &format!("-{pid}")])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            false // pid 已回收/非组首：不杀，防误伤复用该 pgid 的无关进程组（批次6审计 P2）
        };
        #[cfg(windows)]
        let ok = silent_cmd("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            killed += 1;
        }
    }
    killed
}

/// 应用退出清理（P2-24）：杀掉全部在途 Python 子进程，返回杀掉的数量。
/// 由 lib.rs ExitRequested 清理路径调用；注册表为空的正常退出零开销。
pub fn kill_all_py_children() -> usize {
    let pids: Vec<u32> = py_children()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .collect();
    kill_py_children(&pids)
}

/// 有界读取 + 排空（NEW-C-5）：先按 cap+1 探测是否超限；超限则截断到 cap，
/// 并继续把剩余输出读到 EOF 丢弃 —— 原先 take() 到顶即 drop 管道读端，
/// Unix 子进程下次 write 会吃 SIGPIPE 被静默杀死（exit_code=None），用户只见莫名失败。
/// 返回 (截断后的内容, 是否发生截断)。
fn read_capped_drain<R: Read>(src: R, cap: usize) -> (Vec<u8>, bool) {
    let mut probe = src.take(cap as u64 + 1);
    let mut buf = Vec::new();
    let _ = probe.read_to_end(&mut buf);
    let truncated = buf.len() > cap;
    if truncated {
        buf.truncate(cap);
        // 排空剩余输出，让子进程正常写完退出（不进返回值）
        let _ = std::io::copy(&mut probe.into_inner(), &mut std::io::sink());
    }
    (buf, truncated)
}

/// StopToken 感知的 Read 适配器（G2）：每次底层 read 前查停止令牌，置位即返回
/// EOF（Ok(0)），read_capped_drain 据此提前收尾、返回已读部分数据。
/// /stop 时主循环杀进程组的同时 reader 主动退出，不再盲等管道 EOF。
struct StopReader<R> {
    inner: R,
    stop: Option<StopToken>,
}

impl<R> StopReader<R> {
    fn new(inner: R, stop: Option<StopToken>) -> Self {
        Self { inner, stop }
    }
}

impl<R: Read> Read for StopReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.as_ref().is_some_and(|s| s.stopped()) {
            return Ok(0);
        }
        self.inner.read(buf)
    }
}

/// 主进程退出后收输出的兜底（C1）：孙进程继承 stdout/stderr 管道写端且不退出时，
/// reader 子线程的 read_to_end 永不 EOF，`rx.iter()` 会永久阻塞 → run_python 挂死。
/// 改为带总宽限（≤ grace）的 recv_timeout 收满 2 条（out/err）为止；
/// 超时返回已收部分 + complete=false，调用方据此再杀一次进程组兜底。
/// 返回 (stdout, stderr, complete, truncated)（NEW-C-5 增 truncated：任一方向触顶）。
fn drain_output(
    rx: &mpsc::Receiver<(&'static str, Vec<u8>, bool)>,
    grace: Duration,
) -> (String, String, bool, bool) {
    let deadline = Instant::now() + grace;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut truncated = false;
    let mut got = 0;
    while got < 2 {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            break;
        }
        match rx.recv_timeout(remain) {
            Ok((kind, buf, tr)) => {
                got += 1;
                truncated |= tr;
                let text = String::from_utf8_lossy(&buf).into_owned();
                if kind == "out" {
                    stdout = text;
                } else {
                    stderr = text;
                }
            }
            // Timeout（孙进程占管道）或 Disconnected（线程异常）都止损退出
            Err(_) => break,
        }
    }
    (stdout, stderr, got == 2, truncated)
}

/// reader 线程收尾的 join 超时（G2）：整体 deadline = timeout + 2s drain 宽限
/// + 2s reader 收尾，超时的 reader 记 ERROR 审计后 detach
const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// 带超时 join 两个 reader 线程（G2）：原先 reader 裸 spawn detach，进程退出时
/// 变孤儿线程无人知晓；现在所有返回路径（正常 / wait_fail / timeout / stopped）
/// 都必须经过这里。超时仍不退出的 drop handle（detach）+ ERROR 审计
/// 「run_python_reader_timeout / reader_leaked」，泄漏留痕可诊断。
fn join_reader_threads(
    out_handle: std::thread::JoinHandle<()>,
    err_handle: std::thread::JoinHandle<()>,
    audit: &mut dyn FnMut(&str),
) {
    let deadline = Instant::now() + READER_JOIN_TIMEOUT;
    for (kind, h) in [("stdout", out_handle), ("stderr", err_handle)] {
        loop {
            if h.is_finished() {
                let _ = h.join();
                break;
            }
            if Instant::now() >= deadline {
                audit(&format!(
                    "run_python err | kind=reader_timeout | reader={kind} | join 超时（reader_leaked），已 detach"
                ));
                break; // drop(h) = detach
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn truncate_output(s: String) -> String {
    let count = s.chars().count();
    if count <= OUTPUT_CAP {
        s
    } else {
        let mut out: String = s.chars().take(OUTPUT_CAP).collect();
        out.push_str("\n…（输出已截断）");
        out
    }
}

/// Unix 父进程看门狗前导（2026-08-28 批次6审计 P1）：macOS 无 Windows Job Object /
/// KILL_ON_JOB_CLOSE 等价物，主进程崩溃（非 ExitRequested 正常清理路径）时 Python
/// 子进程成孤儿，RLIMIT_CPU 限的是 CPU 时间——睡眠型失控脚本可永久驻留。
/// 看门狗线程 2s 轮询 ppid，变 ≤1（被 launchd/init 收养 = 父死）即自退。
/// 副作用：占用脚本前 7 行，traceback 行号整体偏移（仅影响审计可读性）。
#[cfg(unix)]
const PARENT_WATCHDOG: &str = concat!(
    "import os as _wm_os, threading as _wm_th, time as _wm_tm\n",
    "def _wm_watchdog():\n",
    "    while True:\n",
    "        if _wm_os.getppid() <= 1:\n",
    "            _wm_os._exit(137)\n",
    "        _wm_tm.sleep(2)\n",
    "_wm_th.Thread(target=_wm_watchdog, daemon=True).start()",
);

/// run_python 并发闸门（P2-12）：同一时刻只允许一个 Python 任务在执行，
/// 多余请求排队等待（不报错）—— 防多任务并行 spawn 互相挤兑资源。
/// 用 std Mutex 而非 tokio Semaphore：run_python 是 sync（调用方经
/// spawn_blocking 进入，锁不跨 .await），最朴素且正确。
static PY_RUN_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn py_run_gate() -> &'static std::sync::Mutex<()> {
    &PY_RUN_GATE
}

/// 应用退出标志（2026-08-28 批次5审计 P2）：cleanup_on_exit 在 kill_all_py_children
/// 之前置位；过闸门的排队任务复查后直接拒绝——原先退出 kill 完在途进程，闸门上
/// 排队者拿到锁仍 spawn 新 Python，变无人收割的孤儿进程。
static EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 退出清理入口置位（lib.rs cleanup_on_exit 调用，须在 kill_all_py_children 之前）
pub fn mark_exiting() {
    EXITING.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 批次8审计 P2（2026-09-02）：测试专用复位——lib.rs 退出清理测试经
/// cleanup_on_exit_with 置位 EXITING 后原先进程内永不复位，后续任何走生产
/// run_python() 入口的测试（当前没有，今后加了必挂）会被「应用正在退出」误拒。
#[cfg(test)]
pub(crate) fn reset_exiting_for_test() {
    EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// run_python_at 的失败（P2-10）：区分「spawn NotFound」—— 缓存的 python 路径
/// 被删/换 PATH 时的重探测重试依据
struct RunFail {
    msg: String,
    spawn_not_found: bool,
}

/// 执行一段 Python 脚本（写入独立临时目录运行）。
/// `input_json`：可选，写入 params.json 供脚本读取；`args`：附加命令行参数。
/// `stop`：可选停止令牌（NEW-C-4），/stop 时在途执行轮询到标志即杀进程组退出。
pub fn run_python(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, String> {
    // P2-12：并发闸门 —— 同一时刻只跑一个 Python 任务，多余请求排队等待
    let _gate = py_run_gate().lock().unwrap_or_else(|e| e.into_inner());
    // 批次5审计 P2：退出标志必须在拿到闸门【之后】复查——退出清理杀完在途进程后，
    // 本任务若才拿到锁，spawn 出去就是孤儿进程
    if EXITING.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("应用正在退出，不再启动新的 Python 任务".into());
    }
    run_python_ungated(app, script, input_json, args, timeout_secs, stop)
}

/// run_python 的无闸门内核（2026-08-27 SEC-P2）：供 run_doc_revisions 这类
/// 「入口已持锁、内部要多步执行（dotnet 试跑 + 失败回退 Python）」的调用方使用——
/// std Mutex 不可重入，持锁后再进 run_python 会死锁。
fn run_python_ungated(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, String> {
    // 批次6审计 P1（Unix）：注入父进程看门狗（见 PARENT_WATCHDOG 注释）
    #[cfg(unix)]
    let script_owned;
    #[cfg(unix)]
    let script = {
        script_owned = format!("{PARENT_WATCHDOG}\n{script}");
        script_owned.as_str()
    };
    let mut py = match cached_python() {
        Some(p) => p,
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        None => {
            py_audit(app, "run_python err | kind=no_python");
            return Err("本机未检测到 Python。macOS 请安装 Command Line Tools；Windows 请到 python.org 安装并勾选 Add to PATH".into());
        }
    };

    let mut audit_sink = |line: &str| py_audit(app, line);
    // 最多 2 次尝试：首次 spawn NotFound 说明缓存的 python 已失效（P2-10：
    // 路径被删 / PATH 变了），作废缓存重新探测后重试一次
    for attempt in 0..2 {
        // 独立临时目录（P2-25：初始化失败清理已建目录 + setup_fail 审计，不泄漏）
        let dir = match setup_run_dir(
            &crate::db::data_dir(app).join("py-runs"),
            script,
            input_json,
        ) {
            Ok(d) => d,
            Err(e) => {
                audit_sink(&format!(
                    "run_python err | kind=setup_fail | {}",
                    escape_for_log(&e, 200)
                ));
                return Err(format!("准备运行目录失败：{e}"));
            }
        };
        match run_python_at(&py, Some("run.py"), &dir, args, timeout_secs, &mut audit_sink, stop) {
            Ok(r) => return Ok(r),
            Err(f) => {
                if attempt == 0 && f.spawn_not_found {
                    invalidate_python_cache();
                    if let Some(fresh) = cached_python() {
                        py = fresh;
                        continue;
                    }
                }
                return Err(f.msg);
            }
        }
    }
    unreachable!("最多 2 次尝试，循环内必然返回")
}

/// 执行核心（C3：不依赖 AppHandle，审计经闭包注入 —— 单测可用临时目录 + 内存收集
/// 跑全路径）。前置：dir 已创建且 run.py / params.json 已写入。
/// `entry`：入口参数（Python 传 Some("run.py")；dotnet dll 形态传 Some(dll 路径)；
/// 随包 apphost exe 直跑传 None，见 run_dotnet_revisions）。
/// 所有失败路径（spawn_fail / wait_fail / timeout / drain_timeout）必记审计，
/// 危险路径不留零痕迹。
fn run_python_at(
    py: &str,
    entry: Option<&str>,
    dir: &std::path::Path,
    args: &[String],
    timeout_secs: Option<u64>,
    audit: &mut dyn FnMut(&str),
    stop: Option<&StopToken>,
) -> Result<PyRunResult, RunFail> {
    // C2：超时硬钳上限 300s（钳制记审计，防 timeout_secs=None/超大值把系统跑死）
    let (timeout_eff, clamped) = resolve_timeout(timeout_secs);
    if clamped {
        audit(&format!(
            "run_python | timeout clamped | requested={} cap={MAX_TIMEOUT_SECS}",
            timeout_secs.unwrap_or(0)
        ));
    }
    let timeout = Duration::from_secs(timeout_eff);
    let mem_bytes = mem_limit_bytes(timeout_eff);
    let cpu_secs = cpu_limit_secs(timeout_eff);
    let limits = RunLimits::new(mem_bytes, cpu_secs);

    let mut cmd = silent_cmd(py);
    // Unix：子进程自成进程组（组首），超时可整组强杀，不残留孙进程；
    // 同时经 pre_exec 设资源限额（C2）：RLIMIT_AS 内存 / RLIMIT_CPU CPU
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        unsafe {
            cmd.pre_exec(move || {
                let mem = libc::rlimit {
                    rlim_cur: mem_bytes as libc::rlim_t,
                    rlim_max: mem_bytes as libc::rlim_t,
                };
                libc::setrlimit(libc::RLIMIT_AS, &mem);
                let cpu = libc::rlimit {
                    rlim_cur: cpu_secs as libc::rlim_t,
                    rlim_max: cpu_secs as libc::rlim_t,
                };
                libc::setrlimit(libc::RLIMIT_CPU, &cpu);
                Ok(())
            });
        }
    }
    // Windows：CREATE_NEW_PROCESS_GROUP 让子进程独立进程组（配合 Job Object 整树杀）；
    // silent_cmd 已带 CREATE_NO_WINDOW，这里合并两个 flag
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000 | 0x00000200);
    }
    if let Some(e) = entry {
        cmd.arg(e);
    }
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            audit(&format!(
                "run_python err | kind=spawn_fail | {}",
                escape_for_log(&e.to_string(), 200)
            ));
            // P2-25：spawn 失败走 cleanup_after_fail 同族收尾（terminate 限额 + 清目录）
            cleanup_after_spawn_fail(dir, &limits);
            return Err(RunFail {
                msg: format!("启动 Python 失败：{e}"),
                spawn_not_found: e.kind() == std::io::ErrorKind::NotFound,
            });
        }
    };
    limits.assign(&child);
    // P2-24：登记在途子进程（守卫 Drop 注销，覆盖全部返回路径）；
    // 应用退出时 kill_all_py_children 按注册表整树强杀，防子进程变孤儿
    let _child_reg = ChildRegGuard::register(&child);

    // 双线程读输出防管道死锁（stdout/stderr 先取出再交给线程）
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    // G2：reader 线程持 JoinHandle（不再裸 spawn detach），所有返回路径统一
    // join_reader_threads 带超时 join 兜底；StopReader 让 /stop 置位时 read
    // 立即返回 EOF，reader 随主循环杀进程组同步退出，不变孤儿线程
    let tx_out = tx.clone();
    let stop_out = stop.cloned();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut truncated = false;
        if let Some(s) = child_stdout {
            // 读取时硬截断（审计 P1：原先 read_to_end 无上限，失控脚本 60s 可刷出数百 MB）；
            // NEW-C-5：到顶后继续排空（丢弃），防子进程被 SIGPIPE 静默杀死
            let (b, tr) = read_capped_drain(StopReader::new(s, stop_out), OUTPUT_CAP + 1024);
            buf = b;
            truncated = tr;
        }
        let _ = tx_out.send(("out", buf, truncated));
    });
    let tx_err = tx.clone();
    let stop_err = stop.cloned();
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut truncated = false;
        if let Some(s) = child_stderr {
            let (b, tr) = read_capped_drain(StopReader::new(s, stop_err), OUTPUT_CAP + 1024);
            buf = b;
            truncated = tr;
        }
        let _ = tx_err.send(("err", buf, truncated));
    });
    drop(tx);

    let start = Instant::now();
    let exit_code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) => {}
            Err(e) => {
                // NEW-C-3：原 `?` 直返会把仍在跑的子进程留成孤儿、临时目录泄漏、无审计。
                // 先杀进程组（含孙进程兜底）+ 清临时目录，再记审计返回。
                cleanup_after_fail(&mut child, dir, &limits);
                audit(&format!("run_python err | kind=wait_fail | {e}"));
                join_reader_threads(out_handle, err_handle, audit);
                return Err(RunFail {
                    msg: format!("等待子进程状态失败：{e}"),
                    spawn_not_found: false,
                });
            }
        }
        if start.elapsed() > timeout {
            cleanup_after_fail(&mut child, dir, &limits);
            audit(&format!(
                "run_python err | kind=timeout | timeout_secs={}",
                timeout.as_secs()
            ));
            join_reader_threads(out_handle, err_handle, audit);
            return Err(RunFail {
                msg: format!("执行超时（{}s）已强制终止", timeout.as_secs()),
                spawn_not_found: false,
            });
        }
        // NEW-C-4：/stop 注入检查 —— 原先只在 tool 轮次之间看 StopGuard，
        // 在途 Python 跑满超时都停不下来；这里每 50ms 轮询一次停止令牌
        if stop.is_some_and(|s| s.stopped()) {
            cleanup_after_fail(&mut child, dir, &limits);
            audit("run_python err | kind=stopped");
            join_reader_threads(out_handle, err_handle, audit);
            return Err(RunFail {
                msg: "已停止".into(),
                spawn_not_found: false,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (stdout, mut stderr, drained, truncated) = drain_output(&rx, Duration::from_secs(2));
    if !drained {
        // 孙进程继承管道写端不肯退出（C1）：主进程已退但 reader 线程等不到 EOF，
        // 整组再杀一次兜底，绝不在 rx 上永久阻塞
        kill_tree(&mut child, &limits);
        audit("run_python warn | kind=drain_timeout | 孙进程占用管道已强杀进程组");
        stderr.push_str("\n（输出收集超时：孙进程占用管道，已强杀进程组）");
    }
    // G2：reader 线程带超时 join 兜底 —— 进程组已杀，正常秒回；
    // 2s 仍不 EOF 的极端场景 detach + ERROR 审计，不留无记录孤儿线程
    join_reader_threads(out_handle, err_handle, audit);
    if truncated {
        // NEW-C-5：输出触顶已截断（剩余部分已排空，子进程未受 SIGPIPE 影响）
        audit(&format!(
            "run_python warn | kind=output_truncated | cap={OUTPUT_CAP}"
        ));
    }
    let duration_ms = start.elapsed().as_millis();
    let _ = std::fs::remove_dir_all(dir);
    // NEW-C-5：exit_code=None 表示子进程被信号杀死（历史上多因 SIGPIPE），
    // 原先静默当成功返回，用户只见莫名空结果 —— 记审计并按失败返回
    let Some(code) = exit_code else {
        audit("run_python err | kind=exit_none | 无退出码，子进程疑似被信号杀死");
        return Err(RunFail {
            msg: "子进程异常终止（被信号杀死，无退出码）".into(),
            spawn_not_found: false,
        });
    };
    Ok(PyRunResult {
        stdout: truncate_output(stdout),
        stderr: truncate_output(stderr),
        exit_code: Some(code),
        duration_ms,
        truncated,
    })
}

/// 启动清扫（P2-9）：删掉 py-runs 下超过 1 小时未动的残留临时目录
///（spawn 失败/进程崩溃的兜底；正常路径用完即删，扫到的都是残留）。
pub fn sweep_stale_py_runs(app: &AppHandle) {
    let root = crate::db::data_dir(app).join("py-runs");
    let removed =
        sweep_stale_py_runs_in(&root, Duration::from_secs(3600), std::time::SystemTime::now());
    if removed > 0 {
        py_audit(app, &format!("py_runs sweep | removed={removed}"));
    }
}

/// 清扫实现（路径参数版便于单测）：只删目录，返回删除数
fn sweep_stale_py_runs_in(
    root: &std::path::Path,
    max_age: Duration,
    now: std::time::SystemTime,
) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let stale = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= max_age);
        if stale {
            let _ = std::fs::remove_dir_all(&p);
            removed += 1;
        }
    }
    removed
}

/// 把阻塞调用挪到 blocking 线程池：doc_* 是 async 命令，直接调 run_python 会把
/// 最长 120s 的阻塞压在 async runtime worker 上（并发几个文档操作即可拖垮 runtime，
/// 与 py_env_check 的 spawn_blocking 同一修复模式）。
async fn spawn_blocking_map<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("执行线程异常：{e}"))?
}

/// doc_* 统一执行入口（C3）：保持 NEW-C-1 的 spawn_blocking 隔离，同时给
/// Err 分支（超时 / spawn 失败 / panic / 线程异常）补审计 —— 此前这些危险路径零痕迹。
async fn run_doc_script(
    app: &AppHandle,
    name: &str,
    script: &'static str,
    input: String,
) -> Result<PyRunResult, String> {
    let handle = app.clone();
    // doc_* 由 UI/工具触发，暂无 /stop 令牌（None）；自由编程 run_python 链路才有
    match spawn_blocking_map(move || run_python(&handle, script, Some(&input), &[], Some(120), None))
        .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            py_audit(app, &format!("{name} err | {}", escape_for_log(&e, 300)));
            Err(e)
        }
    }
}

/// 修订版 Word 统一执行入口（2026-08-27）：**强制 .NET OpenXML 引擎优先**
///（w:ins/w:del + delText 铁律由 SDK 类型系统保证），dotnet 运行时/工具不可用
/// 或执行失败时自动回退 Python 脚本（行为不变）。整体走 spawn_blocking 隔离——
/// 此前 dotnet 分支在 async fn 里同步直跑，最长 120s 阻塞会压 async runtime worker
///（NEW-C-1 同款问题，与 run_doc_script 的修复模式对齐）；Err 分支统一补审计。
/// 返回 (执行结果, 引擎标记 "dotnet"/"python")，供调用方审计与结果标注。
async fn run_doc_revisions(
    app: &AppHandle,
    name: &str,
    script: &'static str,
    input: String,
) -> Result<(PyRunResult, &'static str), String> {
    let handle = app.clone();
    let name_in = name.to_string();
    match spawn_blocking_map(move || {
        // 并发闸门提到入口层（2026-08-27 SEC-P2）：dotnet 试跑 + 失败回退 Python 全程持锁，
        // 与 run_python 的「同一时刻只跑一个」语义对齐（std Mutex 不可重入，
        // 故回退走 run_python_ungated）
        let _gate = py_run_gate().lock().unwrap_or_else(|e| e.into_inner());
        // 强制 .NET 优先：不可用（无运行时/无 dll）或跑了失败都回退 Python，均记审计
        if let Some(r) = run_dotnet_revisions(&handle, &input) {
            match r {
                Ok(res) if res.exit_code == Some(0) => return Ok((res, "dotnet")),
                Ok(res) => py_audit(
                    &handle,
                    &format!(
                        "{name_in} dotnet failed, fallback python | {}",
                        escape_for_log(&res.stderr, 200)
                    ),
                ),
                Err(e) => py_audit(
                    &handle,
                    &format!(
                        "{name_in} dotnet err, fallback python | {}",
                        escape_for_log(&e, 200)
                    ),
                ),
            }
        }
        run_python_ungated(&handle, script, Some(&input), &[], Some(120), None)
            .map(|res| (res, "python"))
    })
    .await
    {
        Ok(v) => Ok(v),
        Err(e) => {
            py_audit(app, &format!("{name} err | {}", escape_for_log(&e, 300)));
            Err(e)
        }
    }
}

/// 审计日志钩子（bot.rs 的 audit_log 已存在，这里复用数据目录 bot.log）
pub fn py_audit(app: &AppHandle, line: &str) {
    let p = crate::db::data_dir(app).join("bot.log");
    py_audit_to(&p, line);
}

/// 锁内追加一行到指定日志文件：与 audit::write_event / bot::audit_log 共用同一把
/// BOT_LOG_LOCK（rotate + open + write 必须在同一把锁内，否则并发 append 交错错行——
/// 2026-08-18 事故后统一上锁，py_audit 此前漏网）。
/// 抽成路径参数版便于单测（mock_app 的 AppHandle<MockRuntime> 与 Wry 签名不兼容）。
fn py_audit_to(path: &std::path::Path, line: &str) {
    let _g = crate::audit::BOT_LOG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    crate::db::rotate_log_if_large(path, 5 * 1024 * 1024);
    if let Ok(mut f) = crate::audit::open_log_append(path) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {line}");
    }
}

// ───────────────────────── 固定文档脚本模板 ─────────────────────────

/// 提取文档文本（docx/xlsx/pptx/pdf），stdin 读 params.json {path}
pub const EXTRACT_SCRIPT: &str = r#"import json, sys, os
p = json.load(open('params.json', encoding='utf-8'))['path']
ext = os.path.splitext(p)[1].lower()
if not os.path.exists(p):
    print('文件不存在：' + p); sys.exit(1)
if ext == '.docx':
    import docx
    d = docx.Document(p)
    parts = [para.text for para in d.paragraphs if para.text.strip()]
    for tbl in d.tables:
        for row in tbl.rows:
            cells = [c.text.strip() for c in row.cells]
            parts.append(' | '.join(cells))
    print('\n'.join(parts))
elif ext == '.xlsx':
    import openpyxl
    wb = openpyxl.load_workbook(p, data_only=True)
    for ws in wb.worksheets:
        print(f'=== 工作表：{ws.title} ===')
        for row in ws.iter_rows(values_only=True):
            vals = ['' if v is None else str(v) for v in row]
            if any(v.strip() for v in vals):
                print(' | '.join(vals))
elif ext == '.pptx':
    from pptx import Presentation
    prs = Presentation(p)
    # 2026-08-20 修复：此前只读 text_frame，表格（GraphicFrame）和组合形状内的内容全漏；
    # walk 递归组合形状（shape_type 6 = GROUP），表格按行输出 单元格 | 分隔
    def walk(shapes):
        for shape in shapes:
            if shape.shape_type == 6:
                walk(shape.shapes)
            elif shape.has_table:
                for row in shape.table.rows:
                    cells = [c.text.strip() for c in row.cells]
                    print(' | '.join(cells))
            elif shape.has_text_frame:
                t = shape.text_frame.text.strip()
                if t: print(t)
    for i, slide in enumerate(prs.slides, 1):
        print(f'=== 第{i}页 ===')
        walk(slide.shapes)
elif ext == '.pdf':
    from pypdf import PdfReader
    r = PdfReader(p)
    for i, page in enumerate(r.pages, 1):
        print(f'=== 第{i}页 ===')
        print(page.extract_text() or '')
else:
    print('不支持的格式：' + ext); sys.exit(1)
"#;

/// 生成 Word：stdin 读 params.json {title, paragraphs: [..], tables?: [{title?, rows: [[..]]}], out}
/// tables（2026-08-20）：可选表格列表，按顺序追加在正文段落之后；首行当表头加粗
pub const MAKE_DOCX_SCRIPT: &str = r#"import json, os
import docx
from docx.shared import Pt, Cm
p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
paras = p.get('paragraphs', [])
tables = p.get('tables', [])
out = p['out']
d = docx.Document()
# 正文基础样式：宋体 12pt
style = d.styles['Normal']
style.font.name = '宋体'
style.font.size = Pt(12)
if title:
    h = d.add_heading('', level=1)
    r = h.add_run(title)
    r.font.name = '黑体'
    r.font.size = Pt(16)
for para in paras:
    if para == '':
        d.add_paragraph('')
    else:
        pr = d.add_paragraph(para)
        # 正文首行缩进两字符（12pt × 2 = 24pt ≈ 0.85cm，公文排版规范）
        pr.paragraph_format.first_line_indent = Pt(24)
for t in tables:
    rows = t.get('rows', [])
    if not rows:
        continue
    if t.get('title'):
        tp = d.add_paragraph()
        tr = tp.add_run(t['title'])
        tr.font.bold = True
    tb = d.add_table(rows=len(rows), cols=len(rows[0]))
    tb.style = 'Table Grid'
    for i, row in enumerate(rows):
        for j, cell in enumerate(row):
            tb.cell(i, j).text = str(cell)
    # 首行表头加粗
    for cell in tb.rows[0].cells:
        for cpara in cell.paragraphs:
            for run in cpara.runs:
                run.font.bold = True
    d.add_paragraph('')
d.save(out)
print('已生成：' + out)
"#;

/// 生成修订模式 Word（track changes）：stdin 读 params.json {title, original_path, original, revised, out}
/// 2026-09-02：original_path 可读时优先「原文档副本就地修订」——保留原文格式/字体/表格
/// （与 .NET 工具同语义）；就地失败或无路径时回退新建模式（模型传的 original 行列表）
pub const MAKE_DOCX_REVISIONS_SCRIPT: &str = r#"import json, os, sys, difflib, shutil, copy
import docx
from docx.shared import Pt
from docx.oxml import OxmlElement
from docx.oxml.ns import qn

p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
path = p.get('original_path', '')
orig_model = p.get('original') or []
rev = p.get('revised', [])
out = p['out']

if not rev:
    print('revised 不能为空'); sys.exit(1)

AUTHOR = 'WMessage AI'
# 2026-09-02 老板拍板：修订不写 w:date（修订日期不要了）
_id = [1000]
def nid():
    _id[0] += 1
    return _id[0]

# ──────────── 就地修订（保留原文格式）：w:ins 用 w:t、w:del 用 w:delText ────────────
# 文本口径（2026-09-05 修复，与 extract_document 的 python-docx para.text 严格对齐）：
# 只数直接子级 w:r 和 w:hyperlink 内的 run——w:t 原文、w:tab/w:ptab→\t、w:br/w:cr→\n、
# w:noBreakHyphen→'-'；域代码、已有修订等不计。此前 run_text 只读 w:t 而单元文本用
# para.text（含 \t/\n/超链接文本），口径不一致导致含 tab/换行/超链接的段落必中
# 「整段删+整段增」保底，看不出究竟改了哪几个字。

def run_text(r):
    parts = []
    for node in r:
        tag = node.tag
        if tag == qn('w:t'):
            parts.append(node.text or '')
        elif tag in (qn('w:tab'), qn('w:ptab')):
            parts.append('\t')
        elif tag in (qn('w:br'), qn('w:cr')):
            parts.append('\n')
        elif tag == qn('w:noBreakHyphen'):
            parts.append('-')
    return ''.join(parts)

def inner_runs(p_el):
    """段落文本载体 run：直接子级 w:r + w:hyperlink 内的 w:r（文档序，同 para.text 口径）"""
    rs = []
    for child in p_el:
        if child.tag == qn('w:r'):
            rs.append(child)
        elif child.tag == qn('w:hyperlink'):
            rs.extend(child.findall(qn('w:r')))
    return rs

def para_text(p_el):
    return ''.join(run_text(r) for r in inner_runs(p_el))

def clone_rpr(r):
    rpr = r.find(qn('w:rPr'))
    return copy.deepcopy(rpr) if rpr is not None else None

def fill_run_text(r, text, deleted=False):
    """\t→w:tab、\n→w:br，其余进 w:t（del 用 w:delText）：与提取口径互逆，
    equal 片段里的 tab/换行重建后不变形"""
    tag = 'w:delText' if deleted else 'w:t'
    i = 0
    for j in range(len(text) + 1):
        if j == len(text) or text[j] in '\t\n':
            if j > i:
                t = OxmlElement(tag)
                t.set(qn('xml:space'), 'preserve')
                t.text = text[i:j]
                r.append(t)
            if j < len(text):
                r.append(OxmlElement('w:tab' if text[j] == '\t' else 'w:br'))
            i = j + 1

def make_run(text, rpr, deleted=False):
    r = OxmlElement('w:r')
    if rpr is not None:
        r.append(copy.deepcopy(rpr))
    fill_run_text(r, text, deleted)
    return r

def wrap(kind, r):
    el = OxmlElement('w:ins' if kind == 'ins' else 'w:del')
    el.set(qn('w:id'), str(nid()))
    el.set(qn('w:author'), AUTHOR)
    el.append(r)
    return el

def unwrap_hyperlinks(p_el):
    """hyperlink 元素原位替换为其内部 run（rPr 保留 Hyperlink 样式外观）：
    修订标记不维护 hyperlink 嵌套，整段标删前先解包，避免链接文本漏标删"""
    for h in p_el.findall(qn('w:hyperlink')):
        idx = list(p_el).index(h)
        rs = h.findall(qn('w:r'))
        p_el.remove(h)
        for k, r in enumerate(rs):
            p_el.insert(idx + k, r)

def mark_para_deleted(p_el):
    """整段标删：含文本的 run 转 w:del（保留各 run 原 rPr），无文本 run（图片等）不动"""
    unwrap_hyperlinks(p_el)
    runs = p_el.findall(qn('w:r'))
    dels = []
    for r in runs:
        t = run_text(r)
        if t:
            dels.append(wrap('del', make_run(t, clone_rpr(r), deleted=True)))
    for r in runs:
        if run_text(r):
            p_el.remove(r)
    for el in dels:
        p_el.append(el)

def revise_para(p_el, old, new):
    """行内字符级 diff：equal 片段沿用原 run（克隆 rPr 拆段），del/ins 克隆锚点 rPr。
    run 映射与段落文本同一口径（para_text），正常情况下恒一致；段落含域代码/
    内容控件等口径外结构导致对不上时，才保底整段删+整段增。"""
    runs = inner_runs(p_el)
    pos = 0
    mp = []
    for r in runs:
        t = run_text(r)
        mp.append((pos, pos + len(t), r))
        pos += len(t)
    if pos != len(old) or ''.join(run_text(r) for _, _, r in mp) != old:
        anchor_rpr = clone_rpr(mp[-1][2]) if mp else None
        mark_para_deleted(p_el)
        if new:
            p_el.append(wrap('ins', make_run(new, anchor_rpr)))
        return
    def rpr_at(i):
        for s, e, r in mp:
            if s <= i < e:
                return clone_rpr(r)
        return clone_rpr(mp[-1][2]) if mp else None
    def pieces(a, b):
        for s, e, r in mp:
            lo, hi = max(a, s), min(b, e)
            if lo < hi:
                yield run_text(r)[lo - s:hi - s], clone_rpr(r)
    content = []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, old, new).get_opcodes():
        if tag == 'equal':
            for piece, rpr in pieces(i1, i2):
                content.append(make_run(piece, rpr))
        elif tag == 'delete':
            for piece, rpr in pieces(i1, i2):
                content.append(wrap('del', make_run(piece, rpr, deleted=True)))
        elif tag == 'insert':
            content.append(wrap('ins', make_run(new[j1:j2], rpr_at(i1))))
        else:
            for piece, rpr in pieces(i1, i2):
                content.append(wrap('del', make_run(piece, rpr, deleted=True)))
            content.append(wrap('ins', make_run(new[j1:j2], rpr_at(i1))))
    # 重建：移除原文本载体（直接子级 run + hyperlink，连带其内 run），追加 diff 后的新内容
    for child in list(p_el):
        if child.tag in (qn('w:r'), qn('w:hyperlink')):
            p_el.remove(child)
    for el in content:
        p_el.append(el)

def insert_para_after(body_el, anchor_el, text, style_src=None):
    """锚点后插新段落（整段 w:ins）：pPr/rPr 克隆自 style_src 段落继承样式字体；
    无锚点插到正文开头（sectPr 之前）。返回新段落元素作下一个锚点。"""
    p = OxmlElement('w:p')
    rpr = None
    if style_src is not None:
        ppr = style_src.find(qn('w:pPr'))
        if ppr is not None:
            p.append(copy.deepcopy(ppr))
        for r in style_src.findall(qn('w:r')):
            if run_text(r):
                rpr = clone_rpr(r)
                break
    p.append(wrap('ins', make_run(text, rpr)))
    if anchor_el is None:
        sect = body_el.find(qn('w:sectPr'))
        if sect is not None:
            sect.addprevious(p)
        else:
            body_el.append(p)
    else:
        anchor_el.addnext(p)
    return p

def revise_in_place():
    d = docx.Document(out)  # out 已是原文档副本
    body_el = d.element.body
    # 对齐单元（与提取脚本同一口径）：正文非空段落文档序在前，表格行在后
    units = []
    for para in d.paragraphs:
        if para.text.strip():
            units.append(('p', para._p, para.text, para._p))
    for tbl in d.tables:
        for row in tbl.rows:
            units.append(('row', row._tr, ' | '.join(c.text.strip() for c in row.cells), tbl._tbl))
    orig = [u[2] for u in units]

    def anchor_of(i):
        kind, el, _, aux = units[i]
        return (el, aux if kind == 'p' else None)  # (锚点元素, 样式来源段落)

    def replace_unit(i, new_text):
        kind, el, old_text, aux = units[i]
        if kind == 'p':
            revise_para(el, old_text, new_text)
            return el
        # 表格行：按 " | " 拆回单元格逐格 diff；格数对不上整行标删 + 表后插新段落
        cells = el.findall(qn('w:tc'))
        parts = [x.strip() for x in new_text.split(' | ')]
        if len(parts) == len(cells):
            for tc, part in zip(cells, parts):
                paras = tc.findall(qn('w:p'))
                text_paras = [x for x in paras if para_text(x).strip()]
                if text_paras:
                    revise_para(text_paras[0], para_text(text_paras[0]), part)
                    for extra in text_paras[1:]:
                        mark_para_deleted(extra)
                elif paras:
                    paras[0].append(wrap('ins', make_run(part, None)))
            return aux
        for tc in cells:
            for x in tc.findall(qn('w:p')):
                mark_para_deleted(x)
        return insert_para_after(body_el, aux, new_text)

    sm = difflib.SequenceMatcher(None, orig, rev, autojunk=False)
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == 'equal':
            continue  # 原样不动：格式自然保留
        elif tag == 'delete':
            for i in range(i1, i2):
                kind, el, _, _2 = units[i]
                if kind == 'p':
                    mark_para_deleted(el)
                else:
                    for tc in el.findall(qn('w:tc')):
                        for x in tc.findall(qn('w:p')):
                            mark_para_deleted(x)
        elif tag == 'insert':
            anchor_el, style_src = anchor_of(i1 - 1) if i1 > 0 else (None, None)
            for j in range(j1, j2):
                if rev[j]:
                    anchor_el = insert_para_after(body_el, anchor_el, rev[j], style_src)
                    style_src = anchor_el
        else:
            n = min(i2 - i1, j2 - j1)
            anchor_el, style_src = anchor_of(i1 - 1) if i1 > 0 else (None, None)
            for k in range(n):
                anchor_el = replace_unit(i1 + k, rev[j1 + k])
                style_src = anchor_el if anchor_el.tag == qn('w:p') else None
            for i in range(i1 + n, i2):
                kind, el, _, _2 = units[i]
                if kind == 'p':
                    mark_para_deleted(el)
                else:
                    for tc in el.findall(qn('w:tc')):
                        for x in tc.findall(qn('w:p')):
                            mark_para_deleted(x)
                anchor_el, style_src = anchor_of(i)
            for j in range(j1 + n, j2):
                if rev[j]:
                    anchor_el = insert_para_after(body_el, anchor_el, rev[j], style_src)
                    style_src = anchor_el
    d.save(out)

if path and os.path.exists(path) and path.lower().endswith('.docx'):
    try:
        shutil.copyfile(path, out)
        revise_in_place()
        print('已生成（修订模式·保留原文格式，原文取自文件）：' + out)
        sys.exit(0)
    except Exception as ex:
        # 就地失败（文件损坏/结构异常）不硬挂：回退新建模式，保证有产物
        print('就地修订失败（回退新建模式）：' + str(ex), file=sys.stderr)

# ──────────── 回退：新建文档（无原文档可用时；硬编码可见修订格式） ────────────

# 原文：优先从文件回读（与提取脚本同一套逻辑），读不到/长度超出模型所见时用模型传的原文行
orig_file = []
if path and os.path.exists(path) and path.lower().endswith('.docx'):
    d0 = docx.Document(path)
    orig_file = [para.text for para in d0.paragraphs if para.text.strip()]
    for tbl in d0.tables:
        for row in tbl.rows:
            cells = [c.text.strip() for c in row.cells]
            orig_file.append(' | '.join(cells))
if orig_file and (not orig_model or len(orig_file) <= len(orig_model)):
    orig = orig_file
    src_note = '原文取自文件'
elif orig_model:
    orig = orig_model
    src_note = '原文取自提取列表'
else:
    print('缺少原文：请提供 original_path（Word 路径）或 original（原文行列表）'); sys.exit(1)

d = docx.Document()
st = d.styles['Normal']
st.font.name = '宋体'
st.font.size = Pt(12)
if title:
    h = d.add_heading('', level=1)
    r = h.add_run(title)
    r.font.name = '黑体'
    r.font.size = Pt(16)

def run_el(text, kind):
    r = OxmlElement('w:r')
    rPr = OxmlElement('w:rPr')
    rf = OxmlElement('w:rFonts')
    rf.set(qn('w:ascii'), '宋体')
    rf.set(qn('w:hAnsi'), '宋体')
    rf.set(qn('w:eastAsia'), '宋体')
    rPr.append(rf)
    sz = OxmlElement('w:sz'); sz.set(qn('w:val'), '24')
    szcs = OxmlElement('w:szCs'); szcs.set(qn('w:val'), '24')
    rPr.append(sz); rPr.append(szcs)
    if kind == 'del':
        rPr.append(OxmlElement('w:strike'))
        c = OxmlElement('w:color'); c.set(qn('w:val'), 'C00000'); rPr.append(c)
    elif kind == 'ins':
        u = OxmlElement('w:u'); u.set(qn('w:val'), 'single'); rPr.append(u)
        c = OxmlElement('w:color'); c.set(qn('w:val'), 'C00000'); rPr.append(c)
    r.append(rPr)
    t = OxmlElement('w:delText' if kind == 'del' else 'w:t')
    t.set(qn('xml:space'), 'preserve')
    t.text = text
    r.append(t)
    return r

def append_change(para, kind, text):
    el = OxmlElement('w:ins' if kind == 'ins' else 'w:del')
    el.set(qn('w:id'), str(nid()))
    el.set(qn('w:author'), AUTHOR)
    el.append(run_el(text, kind))
    para._p.append(el)

def diff_into(para, a, b):
    sm = difflib.SequenceMatcher(None, a, b)
    for tag, i1, i2, j1, j2 in sm.get_opcodes():
        if tag == 'equal':
            if i2 > i1:
                para._p.append(run_el(a[i1:i2], 'plain'))
        elif tag == 'delete':
            if i2 > i1:
                append_change(para, 'del', a[i1:i2])
        elif tag == 'insert':
            if j2 > j1:
                append_change(para, 'ins', b[j1:j2])
        else:
            if i2 > i1:
                append_change(para, 'del', a[i1:i2])
            if j2 > j1:
                append_change(para, 'ins', b[j1:j2])

# 段落级对齐（autojunk=False 防长列表相似段落被吞）
sm = difflib.SequenceMatcher(None, orig, rev, autojunk=False)
for tag, i1, i2, j1, j2 in sm.get_opcodes():
    if tag == 'equal':
        for t in orig[i1:i2]:
            para = d.add_paragraph()
            if t:
                para._p.append(run_el(t, 'plain'))
    elif tag == 'delete':
        for t in orig[i1:i2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'del', t)
    elif tag == 'insert':
        for t in rev[j1:j2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'ins', t)
    else:
        n = min(i2 - i1, j2 - j1)
        for k in range(n):
            para = d.add_paragraph()
            diff_into(para, orig[i1 + k], rev[j1 + k])
        for t in orig[i1 + n:i2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'del', t)
        for t in rev[j1 + n:j2]:
            para = d.add_paragraph()
            if t:
                append_change(para, 'ins', t)

d.save(out)
print('已生成（修订模式，' + src_note + '）：' + out)
"#;

/// 生成 Excel：stdin 读 params.json {sheets: [{name, rows: [[..]]}], out}
/// 单元格以 = 开头的字符串按原生公式写入（SUM/IF/VLOOKUP 等）
pub const MAKE_XLSX_SCRIPT: &str = r#"import json
import openpyxl
p = json.load(open('params.json', encoding='utf-8'))
sheets = p.get('sheets', [])
out = p['out']
wb = openpyxl.Workbook()
wb.remove(wb.active)
for i, sh in enumerate(sheets):
    ws = wb.create_sheet(sh.get('name', f'Sheet{i+1}'))
    for row in sh.get('rows', []):
        ws.append([v if not (isinstance(v, str) and v.startswith('=')) else v for v in row])
wb.save(out)
print('已生成：' + out)
"#;

/// 生成 PDF：stdin 读 params.json {title, paragraphs: [..], out}（reportlab，中文字体兜底）
pub const MAKE_PDF_SCRIPT: &str = r#"import json
from reportlab.pdfgen import canvas
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
p = json.load(open('params.json', encoding='utf-8'))
out = p['out']
title = p.get('title', '')
paras = p.get('paragraphs', [])
pdfmetrics.registerFont(UnicodeCIDFont('STSong-Light'))
c = canvas.Canvas(out, pagesize=A4)
w, h = A4
y = h - 60
if title:
    c.setFont('STSong-Light', 18)
    c.drawString(60, y, title)
    y -= 40
c.setFont('STSong-Light', 11)
for para in paras:
    # 简单按行折行（每行约 42 个中文字符）
    for i in range(0, len(para), 42):
        if y < 60:
            c.showPage()
            c.setFont('STSong-Light', 11)
            y = h - 60
        c.drawString(60, y, para[i:i+42])
        y -= 18
    y -= 8
c.save()
print('已生成：' + out)
"#;

/// 生成 PPT：stdin 读 params.json {title, slides: [{title, bullets: [..]}], out}（python-pptx 标准版式）
pub const MAKE_PPTX_SCRIPT: &str = r#"import json, datetime
from pptx import Presentation
from pptx.util import Inches, Pt
from pptx.dml.color import RGBColor
from pptx.enum.text import PP_ALIGN, MSO_ANCHOR

p = json.load(open('params.json', encoding='utf-8'))
out = p['out']
title = p.get('title', '')
slides = p.get('slides', [])
theme = p.get('theme', 'blue')

THEMES = {
    # 来源：color-font-skill 18 套配色精选（文字型 PPT 对比度适配后取色）。
    # band=大面积色块（章节页/标题条/表头，必深色配 bandtext）；accent=点缀（竖条/强调字）；alt=表格斑马纹。
    'blue':  dict(bg='FFFFFF', accent='2B2D42', text='2B2B2B', sub='6B6B6B', band='2B2D42', bandtext='FFFFFF', alt='F2F4F7'),   # 商务与权威
    'navy':  dict(bg='000814', accent='FFC300', text='F0F0F0', sub='9AA5B1', band='003566', bandtext='FFFFFF', alt='0F1E33'),    # 科技与夜景（深色）
    'teal':  dict(bg='FFFFFF', accent='006D77', text='2B2B2B', sub='6B6B6B', band='006D77', bandtext='FFFFFF', alt='F2F7F7'),   # 现代与健康
    'forest':dict(bg='FEFAE0', accent='606C38', text='283618', sub='7A7A5C', band='283618', bandtext='FEFAE0', alt='F6F1DA'),   # 自然与户外
    'wine':  dict(bg='FDF0D5', accent='780000', text='2B2B2B', sub='8A6A4A', band='780000', bandtext='FFFFFF', alt='F9E8C8'),   # 复古与学院
    'sky':   dict(bg='FFFFFF', accent='0077B6', text='03045E', sub='6B6B6B', band='03045E', bandtext='FFFFFF', alt='F0F6FB'),   # 纯净科技蓝
    'plum':  dict(bg='F2E9E4', accent='22223B', text='2B2B2B', sub='8A8A8A', band='22223B', bandtext='FFFFFF', alt='EBE0D8'),   # 轻奢与神秘
    'coral': dict(bg='FDFCDC', accent='0081A7', text='2B2B2B', sub='6B6B6B', band='0081A7', bandtext='FFFFFF', alt='F8F5D2'),   # 海岸珊瑚
    'dark':  dict(bg='1E1E1E', accent='4A90D9', text='F0F0F0', sub='B0B0B0', band='2D2D2D', bandtext='FFFFFF', alt='2D2D2D'),   # 深色通用
    'green': dict(bg='FFFFFF', accent='1E7145', text='2B2B2B', sub='6B6B6B', band='1E7145', bandtext='FFFFFF', alt='F2F6F2'),   # 清新绿
}
T = THEMES.get(theme, THEMES['blue']).copy()
# customColors（2026-08-20 老板拍板：骨架固定、皮肤开放）：可选覆盖主题配色，
# 键 bg/accent/text/sub/band/bandtext/alt，值为 6 位 hex（可带 # 前缀）；非法值忽略保底
import re as _re
custom = p.get('customColors') or {}
if isinstance(custom, dict):
    for k in ('bg', 'accent', 'text', 'sub', 'band', 'bandtext', 'alt'):
        v = custom.get(k)
        if isinstance(v, str):
            v = v.strip().lstrip('#')
            if _re.fullmatch(r'[0-9a-fA-F]{6}', v):
                T[k] = v.upper()
C = lambda h: RGBColor.from_string(h)

prs = Presentation()
prs.slide_width = Inches(13.333)
prs.slide_height = Inches(7.5)
BLANK = prs.slide_layouts[6]
W, H = prs.slide_width, prs.slide_height

def rect(s, x, y, w, h, fill, line=None):
    from pptx.enum.shapes import MSO_SHAPE
    sp = s.shapes.add_shape(MSO_SHAPE.RECTANGLE, x, y, w, h)
    sp.fill.solid(); sp.fill.fore_color.rgb = C(fill)
    if line is None:
        sp.line.fill.background()
    else:
        sp.line.color.rgb = C(line)
    sp.shadow.inherit = False
    return sp

def textbox(s, x, y, w, h, runs, size, color, bold=False, align=PP_ALIGN.LEFT,
            anchor=MSO_ANCHOR.TOP, line_spacing=1.1):
    """runs: str 或 [(text, dict)]；dict 可含 size/color/bold/italic"""
    tb = s.shapes.add_textbox(x, y, w, h)
    tf = tb.text_frame
    tf.word_wrap = True
    tf.vertical_anchor = anchor
    if isinstance(runs, str):
        runs = [(runs, {})]
    else:
        runs = [(str(r), {}) if not isinstance(r, (tuple, list)) else r for r in runs]
    for i, (txt, st) in enumerate(runs):
        para = tf.paragraphs[0] if i == 0 else tf.add_paragraph()
        para.alignment = st.get('align', align)
        para.line_spacing = st.get('line_spacing', line_spacing)
        r = para.add_run(); r.text = txt
        f = r.font
        f.size = Pt(st.get('size', size))
        f.bold = st.get('bold', bold)
        f.color.rgb = C(st.get('color', color))
        f.name = 'Microsoft YaHei'
        try:
            # 中文字符需设置 ea typeface（latin 字体只对西文生效）
            from lxml import etree
            rPr = r._r.get_or_add_rPr()
            ea = rPr.find('{http://schemas.openxmlformats.org/drawingml/2006/main}ea')
            if ea is None:
                ea = etree.SubElement(rPr, '{http://schemas.openxmlformats.org/drawingml/2006/main}ea')
            ea.set('typeface', 'Microsoft YaHei')
        except Exception:
            pass
    return tb

def page_number(s, idx, total):
    textbox(s, W - Inches(1.2), H - Inches(0.5), Inches(0.9), Inches(0.3),
            f'{idx} / {total}', 10, T['sub'], align=PP_ALIGN.RIGHT)

def bullet_font_size(n):
    if n <= 5: return 20
    if n <= 8: return 16
    return 13

total = len(slides)

def render_cover(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, Inches(0.28), H, T['accent'])
    main = sl.get('title') or title or '演示文稿'
    sub = sl.get('subtitle', '')
    textbox(s, Inches(1.1), Inches(2.4), Inches(11), Inches(1.6),
            main, 48, T['text'], bold=True)
    if sub:
        textbox(s, Inches(1.15), Inches(4.2), Inches(11), Inches(0.8),
                sub, 20, T['sub'])
    textbox(s, Inches(1.15), Inches(6.5), Inches(6), Inches(0.5),
            datetime.date.today().strftime('%Y年%m月%d日'), 12, T['sub'])

def render_toc(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, W, Inches(1.1), T['band'])
    textbox(s, Inches(0.9), Inches(0.25), Inches(11.5), Inches(0.7),
            sl.get('title', '目录'), 30, T['bandtext'], bold=True)
    items = sl.get('items') or [b.get('title', '') for b in slides if b.get('type') != 'cover']
    y = Inches(1.7)
    for i, it in enumerate(items, 1):
        textbox(s, Inches(1.5), y, Inches(10.3), Inches(0.55),
                f'{i:02d}   {it}', 18, T['text'], bold=(i <= 9))
        y += Inches(0.62)

def render_section(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['band'])
    textbox(s, Inches(1.0), Inches(2.8), Inches(11.3), Inches(1.4),
            sl.get('title', ''), 40, T['bandtext'], bold=True, align=PP_ALIGN.CENTER)
    sub = sl.get('subtitle', '')
    if sub:
        textbox(s, Inches(1.0), Inches(4.5), Inches(11.3), Inches(0.7),
                sub, 18, T['bandtext'], align=PP_ALIGN.CENTER)

def render_content(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, Inches(0.18), H, T['accent'])
    textbox(s, Inches(0.85), Inches(0.5), Inches(11.6), Inches(0.9),
            sl.get('title', ''), 32, T['text'], bold=True)
    bullets = sl.get('bullets', [])
    n = len(bullets)
    size = bullet_font_size(n)
    two_col = n > 7
    if not two_col:
        textbox(s, Inches(1.05), Inches(1.7), Inches(11.3), Inches(5.1),
                bullets, size, T['text'], line_spacing=1.35)
    else:
        half = (n + 1) // 2
        for col, part in enumerate([bullets[:half], bullets[half:]]):
            x = Inches(1.05) + col * Inches(5.75)
            textbox(s, x, Inches(1.7), Inches(5.5), Inches(5.1),
                    part, size, T['text'], line_spacing=1.3)

def render_table(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, 0, Inches(0.18), H, T['accent'])
    textbox(s, Inches(0.85), Inches(0.5), Inches(11.6), Inches(0.9),
            sl.get('title', ''), 32, T['text'], bold=True)
    rows = sl.get('rows', [])
    if not rows:
        return
    nr, nc = len(rows), max(len(r) for r in rows)
    from pptx.util import Inches as I_
    tbl_w, tbl_h = Inches(11.6), Inches(4.9)
    x, y = Inches(0.9), Inches(1.7)
    gfx = s.shapes.add_table(nr, nc, x, y, tbl_w, tbl_h)
    table = gfx.table
    for ri, row in enumerate(rows):
        for ci in range(nc):
            cell = table.cell(ri, ci)
            cell.text = str(row[ci]) if ci < len(row) else ''
            for para in cell.text_frame.paragraphs:
                for r in para.runs:
                    r.font.size = Pt(14 if ri else 15)
                    r.font.bold = (ri == 0)
                    r.font.color.rgb = C(T['bandtext'] if ri == 0 else T['text'])
                    r.font.name = 'Microsoft YaHei'
            cell.fill.solid()
            cell.fill.fore_color.rgb = C(T['band'] if ri == 0 else T['alt'])

def render_closing(sl):
    s = prs.slides.add_slide(BLANK)
    rect(s, 0, 0, W, H, T['bg'])
    rect(s, 0, H - Inches(0.28), W, Inches(0.28), T['accent'])
    textbox(s, Inches(1.0), Inches(2.9), Inches(11.3), Inches(1.2),
            sl.get('title', '谢谢'), 40, T['accent'], bold=True, align=PP_ALIGN.CENTER)
    sub = sl.get('subtitle', '')
    if sub:
        textbox(s, Inches(1.0), Inches(4.4), Inches(11.3), Inches(0.6),
                sub, 16, T['sub'], align=PP_ALIGN.CENTER)

def render_slide(sl):
    stype = sl.get('type', 'content')
    if stype == 'cover':
        render_cover(sl)
    elif stype == 'toc':
        render_toc(sl)
    elif stype == 'section':
        render_section(sl)
    elif stype == 'table':
        render_table(sl)
    elif stype == 'closing':
        render_closing(sl)
    else:
        render_content(sl)

body = [s for s in slides if s.get('type') != 'cover']
total_pages = len(body) + 1
cover = next((s for s in slides if s.get('type') == 'cover'),
             {'type': 'cover', 'title': title or '演示文稿', 'subtitle': ''})
render_slide(cover)
page_number(prs.slides[-1], 1, total_pages)
for i, sl in enumerate(body):
    render_slide(sl)
    page_number(prs.slides[-1], i + 2, total_pages)

prs.save(out)
print('已生成：' + out)

"#;

// ───────────────────────── 对外命令 ─────────────────────────

/// 执行同步核心（工具链在 async 上下文直接调用）
/// `stop`：/stop 令牌（NEW-C-4），在途执行可被中断；UI 直调传 None
pub fn py_exec_sync(
    app: &AppHandle,
    code: String,
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, String> {
    // 2026-08-26 授权模式：yolo = 文件+Python 全放行不弹窗（老板拍板），跳过开关检查；
    // ask/strict 维持原有「设置页开启 Python 编程」门控
    let yolo = crate::bot::perm_mode(app) == crate::bot::PermMode::Yolo;
    let flag_on = py_get_enabled(app.clone());
    if !yolo && !flag_on {
        return Err(
            "Python 编程未开启：请到设置页「机器人设置」打开「允许机器人执行 Python」（或将授权模式切为 yolo）".into(),
        );
    }
    if yolo && !flag_on {
        py_audit(app, "py_exec | yolo_bypass | 授权模式 yolo，跳过 py-enabled 开关检查");
    }
    // C2：用户/模型请求的超时硬上限 300s，超限直接拒绝并记审计
    //（run_python 内部另有钳制兜底，双保险）
    if let Some(t) = timeout_secs {
        if t > MAX_TIMEOUT_SECS {
            py_audit(
                app,
                &format!("py_exec err | kind=timeout_cap | requested={t} cap={MAX_TIMEOUT_SECS}"),
            );
            return Err(format!("timeout 超过 {MAX_TIMEOUT_SECS}s 上限"));
        }
    }
    py_audit(
        app,
        &format!("py_exec | script: {}", escape_for_log(&code, 300)),
    );
    let r = match run_python(app, &code, None, &[], timeout_secs, stop) {
        Ok(r) => r,
        Err(e) => {
            py_audit(app, &format!("py_exec err | {}", escape_for_log(&e, 300)));
            return Err(e);
        }
    };
    py_audit(
        app,
        &format!(
            "py_exec done | exit={:?} {}ms | out: {}",
            r.exit_code,
            r.duration_ms,
            escape_for_log(&r.stdout, 300)
        ),
    );
    Ok(r)
}

/// py_exec_sync 的 async 包装（C4）：阻塞执行挪到 blocking 线程池，
/// 与 NEW-C-1 doc_* 同一模式 —— 调用方（tool_run_python）在 async runtime 内
/// 不得直接调 sync 版占住 worker。
/// `stop` 取 owned StopToken（而非 &StopGuard）：闭包要进 spawn_blocking，必须 'static。
pub async fn py_exec_sync_async(
    app: AppHandle,
    code: String,
    timeout_secs: Option<u64>,
    stop: Option<StopToken>,
) -> Result<PyRunResult, String> {
    spawn_blocking_map(move || py_exec_sync(&app, code, timeout_secs, stop.as_ref())).await
}

/// 提取结果：文件路径 + 文本（修订模式需要原文路径回读原文）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocExtract {
    pub path: String,
    pub text: String,
}

/// 提取文档文本（弹框选文件或给定路径，按扩展名走固定脚本）
pub async fn doc_extract(app: AppHandle, path: Option<String>) -> CommandResult<DocExtract> {
    let path = match path {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            // 弹系统选择框由用户挑选（模型不知道路径，不得编造）
            let handle = app.clone();
            let picked = tauri::async_runtime::spawn_blocking(move || {
                use tauri_plugin_dialog::DialogExt;
                handle.dialog().file().blocking_pick_file()
            })
            .await
            .unwrap_or(None);
            resolve_doc_path(picked.and_then(file_path_to_string))?
        }
    };
    // NEW-C-6：path 来自用户/系统对话框，可能含换行，审计前必须转义
    py_audit(&app, &format!("doc_extract | path: {}", escape_for_log(&path, 300)));
    let input = serde_json::json!({ "path": path }).to_string();
    let r = run_doc_script(&app, "doc_extract", EXTRACT_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_extract failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(script_fail_err("提取失败", &r.stderr));
    }
    Ok(DocExtract {
        path,
        text: r.stdout,
    })
}

/// F2（Phase 6b）：对话框未选中文件的错误构造，抽纯函数便于单测
/// （tauri command 绑定 Wry AppHandle，mock_app 无法直接调用）。
/// TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
fn resolve_doc_path(picked: Option<String>) -> CommandResult<String> {
    picked.ok_or_else(|| CommandError::Internal("用户取消了选择".into()))
}

/// F2（Phase 6b）：doc_* 脚本非零退出的统一错误构造（纯函数，6 个 command 共用）。
/// Python 脚本失败无 1:1 CommandError 变体，走 Internal 兜底（结构化 code 一致）。
fn script_fail_err(what: &str, stderr: &str) -> CommandError {
    CommandError::Internal(format!("{what}：{}", stderr.trim()))
}

fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        tauri_plugin_dialog::FilePath::Url(u) => Some(u.to_string()),
    }
}

/// 生成 Word 到 AI_Gen_Files（不覆盖：同名自动加序号）
/// tables（2026-08-20）：可选表格列表 [{title?, rows: [[..]]}]，透传给脚本追加在段落之后
pub async fn doc_make_word(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
    tables: Option<serde_json::Value>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input = serde_json::json!({
        "title": title,
        "paragraphs": paragraphs,
        "tables": tables.unwrap_or(serde_json::json!([])),
        "out": out,
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_word", MAKE_DOCX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_word failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(script_fail_err("生成 Word 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_word | out: {out}"));
    Ok(out)
}

/// 生成修订模式 Word（track changes）：original_path 可读时在原文档副本上就地打修订标记
/// （2026-09-02：保留原文格式/字体/表格结构，equal 段落原样不动，改动段落行内字符级 diff）；
/// 无路径时用 original 行列表新建文档。可在 Word 审阅中逐条接受/拒绝。
/// 引擎走 run_doc_revisions 统一入口：强制 .NET OpenXML 优先，Python 脚本兜底。
/// 返回 (输出路径, 引擎标记 "dotnet"/"python")，调用方在结果/审计里标注实际引擎。
pub async fn doc_make_word_revisions(
    app: AppHandle,
    title: String,
    original_path: Option<String>,
    original: Vec<String>,
    revised: Vec<String>,
    filename: Option<String>,
) -> CommandResult<(String, &'static str)> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let src = original_path.clone().unwrap_or_default();
    let input = serde_json::json!({
        "title": title,
        "original_path": original_path.unwrap_or_default(),
        "original": original,
        "revised": revised,
        "out": out
    })
    .to_string();
    let (r, engine) =
        run_doc_revisions(&app, "doc_make_word_revisions", MAKE_DOCX_REVISIONS_SCRIPT, input)
            .await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_word_revisions failed | engine: {engine} | {}",
                escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(script_fail_err("生成修订版 Word 失败", &r.stderr));
    }
    py_audit(
        &app,
        &format!("doc_make_word_revisions | engine: {engine} | src: {src} | out: {out}"),
    );
    Ok((out, engine))
}

/// 生成 Excel 到 AI_Gen_Files（支持 =公式 单元格）
pub async fn doc_make_excel(
    app: AppHandle,
    sheets: Vec<serde_json::Value>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "xlsx")?;
    let input = serde_json::json!({ "sheets": sheets, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_excel", MAKE_XLSX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_excel failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(script_fail_err("生成 Excel 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_excel | out: {out}"));
    Ok(out)
}

/// 生成 PDF 到 AI_Gen_Files
pub async fn doc_make_pdf(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pdf")?;
    let input =
        serde_json::json!({ "title": title, "paragraphs": paragraphs, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_pdf", MAKE_PDF_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_pdf failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(script_fail_err("生成 PDF 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_pdf | out: {out}"));
    Ok(out)
}

/// 生成 PPT 到 AI_Gen_Files
/// custom_colors（2026-08-20）：可选 {bg?, accent?, text?, sub?, band?, bandtext?, alt?}（6 位 hex），
/// 覆盖所选 theme 的对应配色项，脚本侧校验非法值忽略
pub async fn doc_make_ppt(
    app: AppHandle,
    title: String,
    slides: Vec<serde_json::Value>,
    filename: Option<String>,
    theme: Option<String>,
    custom_colors: Option<serde_json::Value>,
) -> CommandResult<String> {
    let out = gen_out_path(&app, filename.as_deref(), "pptx")?;
    let theme = theme
        .map(|t| t.trim().to_lowercase())
        .filter(|t| {
            matches!(
                t.as_str(),
                "blue"
                    | "navy"
                    | "teal"
                    | "forest"
                    | "wine"
                    | "sky"
                    | "plum"
                    | "coral"
                    | "dark"
                    | "green"
            )
        })
        .unwrap_or_else(|| "blue".into());
    let input = serde_json::json!({
        "title": title,
        "slides": slides,
        "out": out,
        "theme": theme,
        "customColors": custom_colors.unwrap_or(serde_json::json!({})),
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_ppt", MAKE_PPTX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_ppt failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(script_fail_err("生成 PPT 失败", &r.stderr));
    }
    py_audit(&app, &format!("doc_make_ppt | out: {out}"));
    Ok(out)
}

/// 剥掉已知扩展名（NEW-C-7，大小写不敏感）：「周报.DOCX」→「周报」；
/// 不匹配（无扩展名或别的扩展名）则原样返回。原 trim_end_matches 大小写敏感，
/// 「周报.DOCX」会生成「周报.DOCX.docx」双扩展名。
fn strip_known_ext(name: &str, ext: &str) -> String {
    let suffix = format!(".{}", ext.to_lowercase());
    if name.to_lowercase().ends_with(&suffix) {
        // 按字符数截取（名称可能含多字节字符，不能按字节切）
        let keep = name.chars().count() - suffix.chars().count();
        name.chars().take(keep).collect()
    } else {
        name.to_string()
    }
}

/// 输出路径：AI_Gen_Files/<文件名>；同名自动加 (n) 序号，永不覆盖（Harness 第 6 层）
fn gen_out_path(app: &AppHandle, filename: Option<&str>, ext: &str) -> CommandResult<String> {
    gen_out_path_in(&crate::db::data_dir(app).join("AI_Gen_Files"), filename, ext)
}

/// F2（Phase 6b）：纯目录参数版便于单测（mock_app 的 AppHandle 与 Wry 签名不兼容）。
/// create_dir_all 失败经 From<io::Error> → CommandError::IoError（结构化，不走 String 逃生舱）。
fn gen_out_path_in(
    dir: &std::path::Path,
    filename: Option<&str>,
    ext: &str,
) -> CommandResult<String> {
    std::fs::create_dir_all(dir)?;
    // 文件名只取 basename，防路径穿越
    let base = filename
        .map(|f| {
            std::path::Path::new(f)
                .file_name()
                .map(|b| b.to_string_lossy().to_string())
                .unwrap_or_default()
        })
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| format!("文档-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let base = strip_known_ext(&base, ext);
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    Ok(candidate.to_string_lossy().to_string())
}

// escape_for_log 已上提到 crate::audit（NEW-C-6）；本文件经顶部 use 引入，规则不变。
// 历史说明：P2-11 原生于本模块（剥换行/管道符防伪造日志行），现与 write_event 共享同一实现。

#[cfg(test)]
mod tests {
    use super::*;

    // ── drain_output（C1：孙进程继承管道时 rx 收集不得永久阻塞）──

    #[test]
    fn drain_output_collects_both_channels() {
        let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>, bool)>();
        tx.send(("out", b"hello".to_vec(), false)).unwrap();
        tx.send(("err", b"warn".to_vec(), false)).unwrap();
        drop(tx);
        let (out, err, complete, truncated) = drain_output(&rx, Duration::from_secs(1));
        assert!(complete);
        assert!(!truncated);
        assert_eq!(out, "hello");
        assert_eq!(err, "warn");
    }

    #[test]
    fn drain_output_times_out_when_grandchild_holds_pipe() {
        // 模拟孙进程 fork 后 sleep 远超宽限、一直占着管道写端：
        // sender 线程 10s 后才发送（且只有一个方向），drain 必须在宽限到期后返回，
        // 而不是像原 rx.iter() 那样永久挂死
        let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>, bool)>();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            let _ = tx.send(("out", b"late".to_vec(), false));
        });
        let start = Instant::now();
        let (_out, _err, complete, _tr) = drain_output(&rx, Duration::from_millis(300));
        assert!(!complete, "发送方卡死时不得报告收齐");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "drain 挂死了：{:?}",
            start.elapsed()
        );
    }

    // ── read_capped_drain（NEW-C-5：触顶截断 + 排空防 SIGPIPE）──

    #[test]
    fn read_capped_drain_under_cap_not_truncated() {
        let data = b"short output".to_vec();
        let (buf, truncated) = read_capped_drain(std::io::Cursor::new(data.clone()), 1024);
        assert!(!truncated);
        assert_eq!(buf, data);
    }

    #[test]
    fn read_capped_drain_over_cap_truncates_and_drains() {
        // 64KB+ 输出：内容截断到 cap、标记 truncated，且剩余部分被排空读完
        //（排空的验证：Cursor 位置必须走到末尾，等价于管道读到 EOF，子进程不会吃 SIGPIPE）
        let cap = OUTPUT_CAP + 1024;
        let data = vec![b'x'; cap + 100];
        let cursor = std::io::Cursor::new(data);
        let (buf, truncated) = read_capped_drain(cursor, cap);
        assert!(truncated);
        assert_eq!(buf.len(), cap);
    }

    #[test]
    fn drain_output_marks_truncated_from_reader() {
        // 模拟 reader 线程发 64KB+ 触顶数据（truncated=true），drain 必须透出标记
        let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>, bool)>();
        tx.send(("out", vec![b'x'; OUTPUT_CAP + 1024], true)).unwrap();
        tx.send(("err", Vec::new(), false)).unwrap();
        drop(tx);
        let (out, _err, complete, truncated) = drain_output(&rx, Duration::from_secs(1));
        assert!(complete);
        assert!(truncated, "触顶标记必须透出");
        assert_eq!(out.len(), OUTPUT_CAP + 1024);
    }

    // ── reader 线程生命周期（G2：StopReader 取消 + 带超时 join 兜底）──

    #[test]
    fn stop_reader_returns_partial_when_stopped() {
        // 批次8审计 P1：本用例断言「未停时完整读取」，stop_all_executions（lib.rs 退出
        // 清理测试 / bot_slash stop_all 用例）并行广播会提前置位 → 首读即 EOF 随机挂
        let _serial = crate::bot_slash::STOP_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let data = vec![b'x'; 4096];
        let guard = crate::bot_slash::StopGuard::new(false, None);
        let token = guard.token();
        // 未停止：完整读取，行为与裸 read_capped_drain 一致
        let (buf, tr) = read_capped_drain(
            StopReader::new(std::io::Cursor::new(data.clone()), Some(token.clone())),
            1024 * 1024,
        );
        assert!(!tr);
        assert_eq!(buf, data);
        // 已停止：第一次 read 即 EOF，提前返回部分（此处为空）数据，不盲等
        guard.force_stop();
        let (buf, tr) = read_capped_drain(
            StopReader::new(std::io::Cursor::new(data), Some(token)),
            1024 * 1024,
        );
        assert!(!tr);
        assert!(buf.is_empty(), "stop 置位后应立即收尾返回部分数据");
    }

    #[test]
    fn join_reader_threads_finished_no_audit() {
        // 正常场景：reader 已完成 → 秒 join，无审计
        let h1 = std::thread::spawn(|| {});
        let h2 = std::thread::spawn(|| {});
        let mut lines: Vec<String> = Vec::new();
        join_reader_threads(h1, h2, &mut |l: &str| lines.push(l.to_string()));
        assert!(lines.is_empty(), "正常退出不应记审计: {lines:?}");
    }

    #[test]
    fn join_reader_threads_stuck_detaches_with_audit() {
        // 卡死场景：stdout reader 永不退出 → 超时 detach + ERROR 审计，不得永久挂住
        let h1 = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(30)));
        let h2 = std::thread::spawn(|| {});
        let mut lines: Vec<String> = Vec::new();
        let start = Instant::now();
        join_reader_threads(h1, h2, &mut |l: &str| lines.push(l.to_string()));
        assert!(
            start.elapsed() < READER_JOIN_TIMEOUT + Duration::from_secs(2),
            "卡死 reader 不得拖住收尾：{:?}",
            start.elapsed()
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("reader_timeout") && l.contains("reader=stdout")),
            "缺 reader_timeout 审计（含 reader 标识）: {lines:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_python_at_grandchild_pipe_readers_joined() {
        // 孙进程继承 stdout/stderr 管道写端且不退（sleep 30）：主进程退出后
        // drain 2s 宽限到期 → 强杀进程组 → reader 拿到 EOF 必须能 join，
        // 不得记 reader_timeout（整体 deadline = timeout + 2s drain + 2s reader）
        let Some(py) = detect_python() else {
            return; // 无 Python 环境跳过
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-grandchild");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("run.py"),
            "import subprocess, sys\nsubprocess.Popen(['sleep', '30'], stdout=sys.stdout, stderr=sys.stderr)\nprint('main-done', flush=True)\n",
        )
        .unwrap();
        let mut lines: Vec<String> = Vec::new();
        let start = Instant::now();
        let r = run_python_at(
            &py,
            Some("run.py"),
            &dir,
            &[],
            Some(30),
            &mut |l: &str| lines.push(l.to_string()),
            None,
        );
        let ok = match r {
            Ok(r) => r,
            Err(f) => panic!("主进程正常退出应返回结果，got err: {}", f.msg),
        };
        assert_eq!(ok.exit_code, Some(0));
        // drain 超时后主进程输出随 reader 在 kill 后才 EOF，按 C1 语义不收回，
        // 但 stderr 必须带「输出收集超时」提示
        assert!(
            ok.stderr.contains("输出收集超时"),
            "got stderr: {}",
            ok.stderr
        );
        assert!(
            lines.iter().any(|l| l.contains("drain_timeout")),
            "缺 drain_timeout 审计: {lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.contains("reader_timeout")),
            "进程组已杀，reader 应能 join: {lines:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "整体兜底超时失效：{:?}",
            start.elapsed()
        );
    }

    // ── resolve_timeout / 资源限额（C2：默认 60s、硬钳上限 300s、限额按 timeout 比例）──

    #[test]
    fn resolve_timeout_default_60s() {
        assert_eq!(resolve_timeout(None), (DEFAULT_TIMEOUT_SECS, false));
        assert_eq!(resolve_timeout(Some(30)), (30, false));
        assert_eq!(resolve_timeout(Some(300)), (300, false));
    }

    #[test]
    fn resolve_timeout_clamps_over_300s() {
        // 传入 10000 应被钳到 300 并报告 clamped
        assert_eq!(resolve_timeout(Some(10000)), (MAX_TIMEOUT_SECS, true));
        assert_eq!(resolve_timeout(Some(u64::MAX)), (MAX_TIMEOUT_SECS, true));
    }

    /// 2026-08-27：.NET 修订工具端到端——dotnet + dll 都在才跑（缺则跳过，CI 无 dotnet 不红）。
    /// 用真实工具生成 docx，验证 OpenXML 修订标记（w:ins 用 w:t / w:del 用 w:delText）。
    #[test]
    fn dotnet_revisions_tool_generates_valid_track_changes() {
        // 批次6改造后：入口定位收敛到 dotnet_revisions_entry（exe 优先/dll 兜底）
        let Some((prog, entry)) = dotnet_revisions_entry() else {
            eprintln!("skip: 未找到 wm-docx-revisions（先 dotnet build -c Release）");
            return;
        };
        let Some(dll) = entry else {
            eprintln!("skip: 命中随包 exe 形态（本用例只验 dotnet <dll> 直跑）");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run");
        std::fs::create_dir_all(&dir).unwrap();
        let out = tmp.path().join("out.docx");
        let params = serde_json::json!({
            "title": "", "original_path": "",
            "original": ["保持不动。", "这句要删掉。"],
            "revised": ["保持不动。", "这句改写法。"],
            "out": out,
        });
        std::fs::write(dir.join("params.json"), params.to_string()).unwrap();
        let dll_s = dll;
        let mut lines: Vec<String> = Vec::new();
        let r = run_python_at(
            &prog,
            Some(&dll_s),
            &dir,
            &["params.json".to_string()],
            Some(120),
            &mut |l: &str| lines.push(l.to_string()),
            None,
        )
        .map_err(|f| f.msg)
        .expect("dotnet 工具应正常运行");
        assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
        // 验证修订标记（zip 里 word/document.xml）
        let f = std::fs::File::open(&out).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let mut xml = String::new();
        use std::io::Read as _;
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("<w:ins "), "应有插入修订：{xml}");
        assert!(xml.contains("<w:del "), "应有删除修订：{xml}");
        assert!(xml.contains("<w:delText"), "w:del 内必须是 w:delText：{xml}");
        assert!(xml.contains("WMessage AI"), "修订应有作者：{xml}");
        // 2026-09-02 老板拍板：修订不写日期
        assert!(!xml.contains("w:date="), "修订不应带 w:date：{xml}");
    }

    /// 2026-09-02：就地修订保留原文格式——夹具 docx（标题样式 + 加粗 run + 普通段落），
    /// 修订后：equal 段落原样不动（pStyle / <w:b/> 保留），改动段落行内 w:ins/w:del，
    /// 无 w:date。dotnet + dll 都在才跑（CI 无 dotnet 跳过）。
    /// 2026-09-05 回归：含 tab + 超链接的段落改几个字必须走字符级 diff
    /// （此前口径不一致必中整段删+整段增保底）。
    #[test]
    fn dotnet_revisions_in_place_preserves_formatting() {
        let Some((prog, entry)) = dotnet_revisions_entry() else {
            eprintln!("skip: 未找到 wm-docx-revisions（先 dotnet build -c Release）");
            return;
        };
        let Some(dll) = entry else {
            eprintln!("skip: 命中随包 exe 形态（本用例只验 dotnet <dll> 直跑）");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        // 最小 docx 夹具（zip + 手写 document.xml）：标题样式段 + 加粗段 + 待改段
        // + tab/超链接混合段（回归：字符级 diff 而非整段标删）
        let orig = tmp.path().join("orig.docx");
        {
            const CT: &str = r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
            const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
            const DOC: &str = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>报告标题</w:t></w:r></w:p><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>加粗内容保留</w:t></w:r></w:p><w:p><w:r><w:t>这句要润色。</w:t></w:r></w:p><w:p><w:r><w:t>含</w:t></w:r><w:r><w:tab/></w:r><w:hyperlink><w:r><w:t>链接文字</w:t></w:r></w:hyperlink><w:r><w:t>保留，改三字。</w:t></w:r></w:p></w:body></w:document>"#;
            let f = std::fs::File::create(&orig).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opt = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("[Content_Types].xml", CT),
                ("_rels/.rels", RELS),
                ("word/document.xml", DOC),
            ] {
                zw.start_file(name, opt).unwrap();
                std::io::Write::write_all(&mut zw, body.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        let dir = tmp.path().join("run");
        std::fs::create_dir_all(&dir).unwrap();
        let out = tmp.path().join("out.docx");
        let params = serde_json::json!({
            "title": "", "original_path": orig,
            "original": [],
            "revised": ["报告标题", "加粗内容保留", "这句润色过了。", "含\t链接文字保留，改四字。"],
            "out": out,
        });
        std::fs::write(dir.join("params.json"), params.to_string()).unwrap();
        let dll_s = dll;
        let mut lines: Vec<String> = Vec::new();
        let r = run_python_at(
            &prog,
            Some(&dll_s),
            &dir,
            &["params.json".to_string()],
            Some(120),
            &mut |l: &str| lines.push(l.to_string()),
            None,
        )
        .map_err(|f| f.msg)
        .expect("dotnet 工具应正常运行");
        assert_eq!(r.exit_code, Some(0), "stderr: {}", r.stderr);
        assert!(
            r.stdout.contains("保留原文格式"),
            "应走在地修订路径；stdout: {}",
            r.stdout
        );
        let f = std::fs::File::open(&out).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let mut xml = String::new();
        use std::io::Read as _;
        zip.by_name("word/document.xml")
            .unwrap()
            .read_to_string(&mut xml)
            .unwrap();
        // 格式保留：标题样式 + 加粗 rPr 原样还在（equal 段落不动）
        assert!(xml.contains("w:val=\"Heading1\""), "标题样式应保留：{xml}");
        assert!(xml.contains("<w:b/>") || xml.contains("<w:b />"), "加粗 rPr 应保留：{xml}");
        // 修订标记：改动段落行内 w:del + w:ins（文本拆 run，断言片段而非整串），无日期
        assert!(xml.contains("<w:del "), "应有删除修订：{xml}");
        assert!(xml.contains("<w:ins "), "应有插入修订：{xml}");
        assert!(xml.contains("<w:delText xml:space=\"preserve\">要</w:delText>"), "删除片段应在：{xml}");
        assert!(xml.contains("<w:t xml:space=\"preserve\">过了</w:t>"), "插入片段应在：{xml}");
        // 2026-09-05 回归：tab/超链接段落改一个字走字符级 diff——只删「三」增「四」，
        // tab 保留、超链接文本作为 equal 片段保留（hyperlink 解包后文字不丢），
        // 不得整段标删（整段删会含完整旧句）
        assert!(xml.contains("<w:delText xml:space=\"preserve\">三</w:delText>"), "应只删「三」：{xml}");
        assert!(xml.contains("<w:t xml:space=\"preserve\">四</w:t>"), "应只增「四」：{xml}");
        assert!(xml.contains("<w:tab/>") || xml.contains("<w:tab />"), "tab 应保留：{xml}");
        assert!(xml.contains(">链接文字</w:t>"), "超链接文本应作为 equal 片段保留：{xml}");
        assert!(!xml.contains("改三字。</w:delText>"), "不得整段标删：{xml}");
        assert!(!xml.contains("w:date="), "修订不应带 w:date：{xml}");
    }

    #[test]
    fn resource_limits_scale_with_timeout() {
        // 内存：8MB/s 比例，下限 256MB，上限 2GB
        assert_eq!(mem_limit_bytes(1), 256 * 1024 * 1024);
        assert_eq!(mem_limit_bytes(60), 480 * 1024 * 1024);
        assert_eq!(mem_limit_bytes(300), 2 * 1024 * 1024 * 1024);
        // CPU：timeout + 10s 宽限，且不溢出
        assert_eq!(cpu_limit_secs(60), 70);
        assert_eq!(cpu_limit_secs(u64::MAX), u64::MAX);
    }

    // ── 失败路径审计（C3：超时 / spawn_fail 必留痕）──

    #[test]
    fn run_python_at_timeout_writes_audit_line() {
        let Some(py) = detect_python() else {
            return; // 无 Python 环境跳过（测试不强制依赖真实子进程）
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-timeout");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "import time\ntime.sleep(30)\n").unwrap();
        let mut lines: Vec<String> = Vec::new();
        let r = run_python_at(&py, Some("run.py"), &dir, &[], Some(1), &mut |l: &str| {
            lines.push(l.to_string())
        }, None);
        let e = match r {
            Err(f) => f.msg,
            Ok(_) => panic!("1s 超时的 sleep 30 脚本不应成功"),
        };
        assert!(e.contains("超时"), "got: {e}");
        assert!(
            lines.iter().any(|l| l.contains("kind=timeout")),
            "缺 timeout 审计行: {lines:?}"
        );
        assert!(!dir.exists(), "超时路径应清理临时目录");
    }

    #[test]
    fn run_python_at_spawn_fail_writes_audit_line() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-spawn-fail");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
        let mut lines: Vec<String> = Vec::new();
        let r = run_python_at("/nonexistent/python-zzz", Some("run.py"), &dir, &[], Some(1), &mut |l: &str| {
            lines.push(l.to_string())
        }, None);
        let e = match r {
            Err(f) => f.msg,
            Ok(_) => panic!("无效 python 路径不应成功"),
        };
        assert!(e.contains("启动 Python 失败"), "got: {e}");
        assert!(
            lines.iter().any(|l| l.contains("kind=spawn_fail")),
            "缺 spawn_fail 审计行: {lines:?}"
        );
        // P2-9：spawn 失败必须清理已创建的临时目录，否则磁盘泄漏
        assert!(!dir.exists(), "spawn 失败后临时目录应被清理");
    }

    // ── P2-25：spawn 失败 / 目录初始化失败的即时泄漏收尾 ──

    #[test]
    fn spawn_fail_cleans_populated_dir_and_audits() {
        // 非法 py 路径 → spawn 失败：含 run.py + params.json 的临时目录必须整体清理，
        // 且有 spawn_fail 审计（cleanup_after_spawn_fail 同族路径）
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-spawn-fail-p25");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
        std::fs::write(dir.join("params.json"), "{}").unwrap();
        let mut lines: Vec<String> = Vec::new();
        let r = run_python_at("/nonexistent/python-zzz", Some("run.py"), &dir, &[], Some(1), &mut |l: &str| {
            lines.push(l.to_string())
        }, None);
        match r {
            Err(f) => assert!(f.msg.contains("启动 Python 失败"), "got: {}", f.msg),
            Ok(_) => panic!("无效 python 路径不应成功"),
        }
        assert!(
            lines.iter().any(|l| l.contains("kind=spawn_fail")),
            "缺 spawn_fail 审计行: {lines:?}"
        );
        assert!(!dir.exists(), "spawn 失败后临时目录应被清理");
        assert!(
            std::fs::read_dir(tmp.path()).unwrap().next().is_none(),
            "不得残留任何文件"
        );
    }

    #[test]
    fn setup_run_dir_success_writes_script_and_params() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = setup_run_dir(tmp.path(), "print(1)\n", Some("{\"a\":1}")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("run.py")).unwrap(), "print(1)\n");
        assert_eq!(
            std::fs::read_to_string(dir.join("params.json")).unwrap(),
            "{\"a\":1}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn setup_run_dir_write_failure_cleans_up() {
        // base 只读 → create_dir_all 失败：返回 Err 且不得残留半成品目录
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let ro = tmp.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = setup_run_dir(&ro, "print(1)\n", None);
        assert!(r.is_err(), "只读 base 下初始化应失败");
        assert!(
            std::fs::read_dir(&ro).unwrap().next().is_none(),
            "失败后不得残留临时目录"
        );
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // ── cleanup_after_fail（NEW-C-3：失败路径杀进程组 + 清临时目录）──

    #[test]
    fn cleanup_after_fail_kills_child_and_removes_dir() {
        // mock Child 难，直接 spawn `sleep` 验证：调用后子进程已退出、临时目录已删
        //（Unix 下 RunLimits::terminate 是 no-op，kill_tree 兜底 child.kill()）
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-cleanup");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "x").unwrap();
        let sleeper = if cfg!(windows) { "cmd" } else { "sleep" };
        let args: &[&str] = if cfg!(windows) {
            &["/C", "ping", "-n", "30", "127.0.0.1"]
        } else {
            &["30"]
        };
        let mut child = silent_cmd(sleeper).args(args).spawn().unwrap();
        let limits = RunLimits::new(0, 0);
        cleanup_after_fail(&mut child, &dir, &limits);
        assert!(!dir.exists(), "失败路径必须清理临时目录");
        assert!(
            child.try_wait().unwrap().is_some(),
            "子进程应已被杀死，不得留孤儿"
        );
    }

    // ── 退出清理注册表（P2-24：kill_py_children 整树杀 + 守卫注销）──

    #[cfg(unix)]
    #[test]
    fn kill_py_children_kills_registered_process_group() {
        // 模拟生产 spawn（process_group(0) 自成组首）：注册后按 pid 整组强杀，
        // 子进程必须已退出（不得留孤儿）；守卫 Drop 后注册表必须清空
        use std::os::unix::process::CommandExt;
        let mut cmd = silent_cmd("sleep");
        cmd.arg("30");
        cmd.process_group(0);
        let mut child = cmd.spawn().unwrap();
        let pid = child.id();
        {
            let _guard = ChildRegGuard::register(&child);
            assert!(
                py_children()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains(&pid),
                "注册后必须能查到 pid"
            );
            assert_eq!(kill_py_children(&[pid]), 1, "应杀掉 1 个进程组");
            let status = child.wait().unwrap();
            assert!(!status.success(), "子进程应已被强杀");
        }
        assert!(
            !py_children()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&pid),
            "守卫 Drop 后必须注销（防 pid 复用误杀）"
        );
        // 已退出的 pid 再杀：组不存在按未杀计，不 panic
        assert_eq!(kill_py_children(&[pid]), 0);
    }

    // ── 残留目录清扫（P2-9：启动时清 py-runs 超龄目录）──

    #[test]
    fn sweep_stale_py_runs_removes_only_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("py-runs");
        let stale = root.join("stale1");
        let fresh = root.join("fresh1");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::create_dir_all(&fresh).unwrap();
        std::fs::write(root.join("stray.txt"), b"x").unwrap();
        let now = std::time::SystemTime::now();
        // max_age=0 → 所有目录视为过期，全删（非目录文件不动）
        let removed = sweep_stale_py_runs_in(&root, Duration::ZERO, now);
        assert_eq!(removed, 2);
        assert!(!stale.exists() && !fresh.exists());
        assert!(root.join("stray.txt").exists(), "非目录文件不应被清扫");
        // 重建 fresh：max_age=1h → 刚建的目录必须保留
        std::fs::create_dir_all(&fresh).unwrap();
        let removed = sweep_stale_py_runs_in(&root, Duration::from_secs(3600), now);
        assert_eq!(removed, 0);
        assert!(fresh.exists());
        // root 不存在时静默返回 0
        assert_eq!(
            sweep_stale_py_runs_in(
                &tmp.path().join("no-such-dir"),
                Duration::ZERO,
                now
            ),
            0
        );
    }

    // ── 探测缓存（P2-10：连续调用只探测一次；探测本身带超时）──

    #[test]
    fn cached_python_probes_only_once() {
        invalidate_python_cache();
        let _ = cached_python(); // 首次：真正探测
        let n1 = PY_PROBE_COUNT.load(std::sync::atomic::Ordering::SeqCst);
        let a = cached_python();
        let b = cached_python();
        let n2 = PY_PROBE_COUNT.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(a, b, "缓存结果应稳定");
        assert_eq!(n2, n1, "缓存命中后不得重复 spawn 探测");
    }

    #[cfg(unix)]
    #[test]
    fn probe_version_times_out_on_hanging_shim() {
        // 卡死的 shim（sleep 远探测超时）必须在超时内按失败返回，不得挂住
        let start = Instant::now();
        let ok = probe_version_ok_with("sleep", &["10"], Duration::from_millis(300));
        assert!(!ok);
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "探测挂死了：{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn spawn_not_found_flagged_for_cache_retry() {
        // 无效路径 spawn → RunFail.spawn_not_found=true（run_python 据此作废缓存重试）
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-notfound");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "print(1)\n").unwrap();
        let r = run_python_at("/nonexistent/python-zzz", Some("run.py"), &dir, &[], Some(1), &mut |_| {}, None);
        match r {
            Err(f) => assert!(f.spawn_not_found),
            Ok(_) => panic!("无效 python 路径不应成功"),
        }
    }

    // ── /stop 中断在途执行（NEW-C-4：停止令牌注入轮询循环）──

    #[test]
    fn run_python_at_stop_token_interrupts_promptly() {
        let Some(py) = detect_python() else {
            return; // 无 Python 环境跳过
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run-stop");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("run.py"), "import time\ntime.sleep(30)\n").unwrap();
        let guard = crate::bot_slash::StopGuard::new(false, None);
        let token = guard.token();
        let mut lines: Vec<String> = Vec::new();
        let start = Instant::now();
        // 50ms 后置位停止标志（模拟 /stop）；timeout 给足 60s，
        // 若停止检查失效就要等满 60s 才返回 —— 用耗时断言区分
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                guard.force_stop();
            });
            let r = run_python_at(&py, Some("run.py"), &dir, &[], Some(60), &mut |l: &str| {
                lines.push(l.to_string())
            }, Some(&token));
            let e = match r {
                Err(f) => f.msg,
                Ok(_) => panic!("被停止的脚本不应成功返回"),
            };
            assert!(e.contains("已停止"), "got: {e}");
        });
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "/stop 后应立即中断，实际耗时 {:?}",
            start.elapsed()
        );
        assert!(
            lines.iter().any(|l| l.contains("kind=stopped")),
            "缺 stopped 审计行: {lines:?}"
        );
        assert!(!dir.exists(), "停止路径应清理临时目录");
    }

    // ── strip_known_ext（NEW-C-7：扩展名剥离大小写不敏感）──

    #[test]
    fn strip_known_ext_case_insensitive() {
        // 「周报.DOCX」+ docx → 剥掉后 gen_out_path 产出「周报.docx」而非「周报.DOCX.docx」
        assert_eq!(strip_known_ext("周报.DOCX", "docx"), "周报");
        assert_eq!(strip_known_ext("a.docx", "docx"), "a");
        assert_eq!(strip_known_ext("b.Docx", "docx"), "b");
        // 不匹配：无扩展名 / 别的扩展名 / 仅后缀相似（非 .ext），一律原样
        assert_eq!(strip_known_ext("报告", "docx"), "报告");
        assert_eq!(strip_known_ext("a.txt", "docx"), "a.txt");
        assert_eq!(strip_known_ext("adocx", "docx"), "adocx");
        assert_eq!(strip_known_ext("a.docx.docx", "docx"), "a.docx");
    }

    // ── escape_for_log（P2-11：剥换行/管道符，防伪造日志行）──

    #[test]
    fn escape_for_log_strips_newlines_and_pipes() {
        // 验收用例："a\nb| c" → "a\\nb||  c"
        assert_eq!(escape_for_log("a\nb| c", 300), "a\\nb||  c");
        assert_eq!(escape_for_log("x\ry", 300), "x\\ry");
        assert_eq!(escape_for_log("plain", 300), "plain");
        // 伪造前缀注入：剥完后无法再伪装成行首分隔符
        assert_eq!(
            escape_for_log("evil\n[2026-01-01 00:00:00] INFO | fake", 300),
            "evil\\n[2026-01-01 00:00:00] INFO ||  fake"
        );
    }

    #[test]
    fn escape_for_log_truncates_after_escape() {
        let long = "x".repeat(400);
        let out = escape_for_log(&long, 300);
        assert_eq!(out.chars().count(), 301);
        assert!(out.ends_with('…'));
    }

    // ── 并发闸门（P2-12：同一时刻只允许一个 Python 任务在执行）──

    #[test]
    fn py_run_gate_serializes_concurrent_runs() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CUR: AtomicUsize = AtomicUsize::new(0);
        static MAX: AtomicUsize = AtomicUsize::new(0);
        let mut joins = Vec::new();
        // 10 个并发请求同时抢闸门，临界区内 sleep 100ms 放大竞争窗口
        for _ in 0..10 {
            joins.push(std::thread::spawn(|| {
                let _g = py_run_gate().lock().unwrap_or_else(|e| e.into_inner());
                let cur = CUR.fetch_add(1, Ordering::SeqCst) + 1;
                MAX.fetch_max(cur, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(100));
                CUR.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for j in joins {
            j.join().unwrap();
        }
        assert_eq!(
            MAX.load(Ordering::SeqCst),
            1,
            "闸门内并发数必须恒为 1（多余请求排队，不并行）"
        );
    }

    // ── spawn_blocking_map（NEW-C-1：doc_* async 命令不得把阻塞压在 runtime worker 上）──

    #[test]
    fn spawn_blocking_map_ok_passthrough() {
        let r = tauri::async_runtime::block_on(spawn_blocking_map(|| Ok::<_, String>(42)));
        assert_eq!(r.unwrap(), 42);
    }

    #[test]
    fn spawn_blocking_map_err_passthrough() {
        let r: Result<i32, String> =
            tauri::async_runtime::block_on(spawn_blocking_map(|| Err("业务错误".to_string())));
        assert_eq!(r.unwrap_err(), "业务错误");
    }

    #[test]
    fn spawn_blocking_map_panic_mapped_to_err() {
        // 闭包 panic → JoinError → 映射为错误字符串，而不是扩散到调用方
        let r: Result<(), String> =
            tauri::async_runtime::block_on(spawn_blocking_map(|| panic!("boom")));
        let e = r.unwrap_err();
        assert!(e.starts_with("执行线程异常"), "got: {e}");
    }

    // ── py_audit_to（NEW-C-2：py_audit 必须与 write_event/audit_log 共用 BOT_LOG_LOCK）──

    #[test]
    fn py_audit_to_writes_wellformed_line() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        py_audit_to(&p, "py_exec | script: hello");
        let content = std::fs::read_to_string(&p).unwrap();
        let line = content.trim_end();
        assert!(
            line.starts_with('[') && line.contains("] py_exec | script: hello"),
            "got: {line}"
        );
    }

    #[test]
    fn py_audit_to_concurrent_no_interleaved_lines() {
        // 8 线程 × 100 行并发写：上锁后每行必须完整（无撕裂/合并），行数精确
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        let mut joins = Vec::new();
        for t in 0..8 {
            let p = p.clone();
            joins.push(std::thread::spawn(move || {
                for i in 0..100 {
                    py_audit_to(&p, &format!("marker-{t}-{i}-{}", "x".repeat(64)));
                }
            }));
        }
        for j in joins {
            j.join().unwrap();
        }
        let content = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 800, "行数不符（可能有错行合并/撕裂）");
        for line in lines {
            assert!(
                line.starts_with("[20") && line.ends_with(&"x".repeat(64)),
                "错行: {line}"
            );
        }
    }

    // ── F2（Phase 6b）：doc_* command 结构化错误迁移 ──

    /// doc_extract 错误路径：对话框未选中文件 → CommandError::Internal（code 稳定）
    #[test]
    fn f2_resolve_doc_path_cancel_returns_internal() {
        let err = resolve_doc_path(None).unwrap_err();
        assert_eq!(err.code(), "INTERNAL");
        assert!(err.message().contains("用户取消了选择"));
    }

    /// doc_extract happy path（纯函数层）：选中文件 → 路径透传
    #[test]
    fn f2_resolve_doc_path_picked_passes_through() {
        let p = resolve_doc_path(Some("/tmp/a.docx".into())).unwrap();
        assert_eq!(p, "/tmp/a.docx");
    }

    /// 6 个 doc_* command 的脚本失败分支：统一 Internal 变体，stderr trim 后入 message
    #[test]
    fn f2_script_fail_err_covers_all_doc_commands() {
        for (cmd, what) in [
            ("doc_extract", "提取失败"),
            ("doc_make_word", "生成 Word 失败"),
            ("doc_make_word_revisions", "生成修订版 Word 失败"),
            ("doc_make_excel", "生成 Excel 失败"),
            ("doc_make_pdf", "生成 PDF 失败"),
            ("doc_make_ppt", "生成 PPT 失败"),
        ] {
            let err = script_fail_err(what, "  boom\n");
            assert_eq!(err.code(), "INTERNAL", "{cmd} 错误 code 应为 INTERNAL");
            assert!(
                err.message().contains(what) && err.message().contains("boom"),
                "{cmd} message 应含操作名 + stderr：{}",
                err.message()
            );
            assert!(!err.is_recoverable(), "{cmd} Internal 不可重试");
        }
    }

    /// doc_make_* happy path（共享逻辑层）：输出路径落在目标目录 + 扩展名正确
    #[test]
    fn f2_gen_out_path_in_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("AI_Gen_Files");
        let out = gen_out_path_in(&dir, Some("周报"), "docx").unwrap();
        assert!(out.starts_with(dir.to_str().unwrap()));
        assert!(out.ends_with("周报.docx"), "out={out}");
        assert!(dir.is_dir(), "create_dir_all 应已建目录");
    }

    /// 同名不覆盖：已存在 周报.docx → 产出 周报 (1).docx
    #[test]
    fn f2_gen_out_path_in_conflict_appends_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("AI_Gen_Files");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("周报.docx"), b"x").unwrap();
        let out = gen_out_path_in(&dir, Some("周报"), "docx").unwrap();
        assert!(out.ends_with("周报 (1).docx"), "out={out}");
    }

    /// 防路径穿越：../../etc/evil → 只取 basename
    #[test]
    fn f2_gen_out_path_in_strips_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("AI_Gen_Files");
        let out = gen_out_path_in(&dir, Some("../../etc/evil"), "xlsx").unwrap();
        assert!(out.ends_with("evil.xlsx"), "out={out}");
        assert!(!out.contains("etc"), "out={out}");
    }

    /// doc_make_* 错误路径（共享逻辑层）：AI_Gen_Files 被文件占用 →
    /// create_dir_all 失败 → CommandError::IoError（不再走 String 逃生舱）
    #[test]
    fn f2_gen_out_path_in_io_error_maps_to_io_error_variant() {
        let tmp = tempfile::tempdir().unwrap();
        let file_as_dir = tmp.path().join("AI_Gen_Files");
        std::fs::write(&file_as_dir, b"not a dir").unwrap();
        let err = gen_out_path_in(&file_as_dir, Some("x"), "pdf").unwrap_err();
        assert_eq!(err.code(), "IO_ERROR");
        assert!(!err.is_recoverable());
    }

    /// Windows 子进程直拉 Python 时强制 UTF-8 IO encoding，
    /// 否则 stdout 默认 cp936，Rust 端按 UTF-8 解码看到乱码。
    /// （Fix 2026-08-21：老板 09:53 拍板走方案 A，加两个 env var）
    #[cfg(windows)]
    #[test]
    fn silent_cmd_on_windows_sets_python_utf8_env() {
        let cmd = silent_cmd("python");
        let mut found_io = false;
        let mut found_utf8 = false;
        for (k, v) in cmd.get_envs().flatten() {
            if k.to_str() == Some("PYTHONIOENCODING") && v.to_str() == Some("utf-8") {
                found_io = true;
            }
            if k.to_str() == Some("PYTHONUTF8") && v.to_str() == Some("1") {
                found_utf8 = true;
            }
        }
        assert!(found_io, "PYTHONIOENCODING=utf-8 未设置");
        assert!(found_utf8, "PYTHONUTF8=1 未设置");
    }
}

#[cfg(test)]
mod batch5_exiting_tests {
    /// 批次5审计 P2 回归锁：EXITING 复查必须在 PY_RUN_GATE 拿锁之后
    ///（锁前检查挡不住「kill 完成后才拿到锁的排队者」）。
    /// 全局标志不在测试里翻转（会污染并行测试的 run_python），源码锁防回退。
    #[test]
    fn exiting_check_is_after_gate_lock() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_py.rs"))
            .unwrap();
        let fn_pos = text.find("pub fn run_python(").expect("run_python 必须存在");
        let body = &text[fn_pos..];
        let gate = body.find("py_run_gate().lock()").expect("必须过并发闸门");
        let check = body.find("EXITING.load").expect("必须有退出标志复查");
        assert!(check > gate, "EXITING 复查必须在闸门拿锁之后");
    }
}

#[cfg(test)]
mod batch6_platform_tests {
    /// 批次6审计 P1 回归锁：开发模式 dll 候选（env!("CARGO_MANIFEST_DIR") 绝对路径）
    /// 必须 cfg(debug_assertions) 门控——否则构建机路径烧进发布二进制
    #[test]
    fn dev_dll_candidate_is_debug_gated() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_py.rs"))
            .unwrap();
        let pos = text.find("dotnet/WmDocxRevisions/bin/Release/net8.0")
            .expect("开发模式 dll 候选必须存在");
        // 字符安全截取（中文注释多字节，字节下标切片会 panic）
        let tail: String = text[..pos].chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect();
        assert!(
            tail.contains("#[cfg(debug_assertions)]"),
            "CARGO_MANIFEST_DIR dll 候选必须 debug 门控: {tail:?}"
        );
    }

    /// 批次6审计 P1 回归锁：绿色包随包 apphost exe 直跑候选必须存在且优先于 dll
    ///（self-contained 发布免装 .NET；framework-dependent apphost 自动找共享运行时）
    #[test]
    fn bundled_exe_entry_preferred_over_dll() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_py.rs"))
            .unwrap();
        let fn_pos = text.find("fn dotnet_revisions_entry()").expect("入口定位函数必须存在");
        let body: String = text[fn_pos..].chars().take(1500).collect();
        let exe_hit = body.find("tool_exe.is_file()").expect("必须有随包 exe 候选");
        let dll_hit = body.find("dll.is_file()").expect("必须有 dll 候选");
        assert!(exe_hit < dll_hit, "随包 exe 候选必须先于 dll 判定");
    }

    /// 批次6审计 P1 回归锁：Unix 脚本必须注入父进程看门狗（macOS 无 Job Object 等价物，
    /// 主进程崩溃时睡眠型失控脚本靠 RLIMIT_CPU 管不住）
    #[cfg(unix)]
    #[test]
    fn unix_scripts_get_parent_watchdog() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot_py.rs"))
            .unwrap();
        assert!(text.contains("PARENT_WATCHDOG"), "必须有看门狗前导常量");
        let fn_pos = text.find("fn run_python_ungated(").expect("run_python_ungated 必须存在");
        let body: String = text[fn_pos..].chars().take(1200).collect();
        assert!(
            body.contains("PARENT_WATCHDOG"),
            "看门狗必须在 run_python_ungated 注入（覆盖全部 Python 执行入口）"
        );
    }

    /// 批次6审计 P2：pid 复用防护——死 pid / 非组首不得杀（getpgid 失败返回 -1）
    #[cfg(unix)]
    #[test]
    fn group_leader_check_rejects_dead_pid() {
        assert!(!super::is_live_group_leader(u32::MAX - 1), "不存在的 pid 不得判为组首");
    }
}
