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

/// Windows 上 GUI 程序调 cmd.exe / python.exe / taskkill.exe 等控制台子进程时，
/// 默认会为子进程开一个控制台窗口（即使立即退出）—— 视觉上就是"黑框闪一下"。
/// CREATE_NO_WINDOW (0x08000000) 抑制父进程继承的控制台窗口创建，是 Tauri / Electron
/// 等 GUI 框架调子进程时的标准做法。Unix 平台无此概念（不走控制台），保持直通。
#[cfg(windows)]
fn silent_cmd(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(0x08000000);
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

#[tauri::command]
pub fn py_set_enabled(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let dir = crate::db::data_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if enabled {
        std::fs::write(py_flag_path(&app), b"1").map_err(|e| e.to_string())?;
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
    #[cfg(not(windows))]
    let candidates: &[&str] = &["python3", "python"];
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

/// 终止 Python 进程及其全部子进程：Unix 按进程组（spawn 时 process_group(0) 成为组首），
/// Windows 先 TerminateJobObject 整树杀（Job 覆盖孙进程）再 taskkill /T 兜底。
/// 先组杀再兜底 kill + wait。
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

/// 主进程退出后收输出的兜底（C1）：孙进程继承 stdout/stderr 管道写端且不退出时，
/// reader 子线程的 read_to_end 永不 EOF，`rx.iter()` 会永久阻塞 → run_python 挂死。
/// 改为带总宽限（≤ grace）的 recv_timeout 收满 2 条（out/err）为止；
/// 超时返回已收部分 + complete=false，调用方据此再杀一次进程组兜底。
fn drain_output(
    rx: &mpsc::Receiver<(&'static str, Vec<u8>)>,
    grace: Duration,
) -> (String, String, bool) {
    let deadline = Instant::now() + grace;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut got = 0;
    while got < 2 {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            break;
        }
        match rx.recv_timeout(remain) {
            Ok((kind, buf)) => {
                got += 1;
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
    (stdout, stderr, got == 2)
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

/// run_python 并发闸门（P2-12）：同一时刻只允许一个 Python 任务在执行，
/// 多余请求排队等待（不报错）—— 防多任务并行 spawn 互相挤兑资源。
/// 用 std Mutex 而非 tokio Semaphore：run_python 是 sync（调用方经
/// spawn_blocking 进入，锁不跨 .await），最朴素且正确。
static PY_RUN_GATE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn py_run_gate() -> &'static std::sync::Mutex<()> {
    &PY_RUN_GATE
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
        // 独立临时目录
        let dir = crate::db::data_dir(app)
            .join("py-runs")
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("run.py"), script).map_err(|e| e.to_string())?;
        if let Some(j) = input_json {
            std::fs::write(dir.join("params.json"), j).map_err(|e| e.to_string())?;
        }
        match run_python_at(&py, &dir, args, timeout_secs, &mut audit_sink, stop) {
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
/// 所有失败路径（spawn_fail / wait_fail / timeout / drain_timeout）必记审计，
/// 危险路径不留零痕迹。
fn run_python_at(
    py: &str,
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
    cmd.arg("run.py")
        .args(args)
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
            // P2-9：spawn 失败时临时目录已创建，必须清理，否则磁盘泄漏
            let _ = std::fs::remove_dir_all(dir);
            return Err(RunFail {
                msg: format!("启动 Python 失败：{e}"),
                spawn_not_found: e.kind() == std::io::ErrorKind::NotFound,
            });
        }
    };
    limits.assign(&child);

    // 双线程读输出防管道死锁（stdout/stderr 先取出再交给线程）
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    let tx_out = tx.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_stdout {
            // 读取时硬截断（审计 P1：原先 read_to_end 无上限，失控脚本 60s 可刷出数百 MB）
            let _ = s.take((OUTPUT_CAP + 1024) as u64).read_to_end(&mut buf);
        }
        let _ = tx_out.send(("out", buf));
    });
    let tx_err = tx.clone();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = child_stderr {
            let _ = s.take((OUTPUT_CAP + 1024) as u64).read_to_end(&mut buf);
        }
        let _ = tx_err.send(("err", buf));
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
            return Err(RunFail {
                msg: "已停止".into(),
                spawn_not_found: false,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (stdout, mut stderr, drained) = drain_output(&rx, Duration::from_secs(2));
    if !drained {
        // 孙进程继承管道写端不肯退出（C1）：主进程已退但 reader 线程等不到 EOF，
        // 整组再杀一次兜底，绝不在 rx 上永久阻塞
        kill_tree(&mut child, &limits);
        audit("run_python warn | kind=drain_timeout | 孙进程占用管道已强杀进程组");
        stderr.push_str("\n（输出收集超时：孙进程占用管道，已强杀进程组）");
    }
    let duration_ms = start.elapsed().as_millis();
    let _ = std::fs::remove_dir_all(dir);
    Ok(PyRunResult {
        stdout: truncate_output(stdout),
        stderr: truncate_output(stderr),
        exit_code,
        duration_ms,
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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
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
    for i, slide in enumerate(prs.slides, 1):
        print(f'=== 第{i}页 ===')
        for shape in slide.shapes:
            if shape.has_text_frame:
                t = shape.text_frame.text.strip()
                if t: print(t)
elif ext == '.pdf':
    from pypdf import PdfReader
    r = PdfReader(p)
    for i, page in enumerate(r.pages, 1):
        print(f'=== 第{i}页 ===')
        print(page.extract_text() or '')
else:
    print('不支持的格式：' + ext); sys.exit(1)
"#;

/// 生成 Word：stdin 读 params.json {title, paragraphs: [..], out}
pub const MAKE_DOCX_SCRIPT: &str = r#"import json, os
import docx
from docx.shared import Pt, Cm
p = json.load(open('params.json', encoding='utf-8'))
title = p.get('title', '')
paras = p.get('paragraphs', [])
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
d.save(out)
print('已生成：' + out)
"#;

/// 生成修订模式 Word（track changes）：stdin 读 params.json {title, original_path, original, revised, out}
/// 原文优先回读文件（保真）；提取被截断时回退模型传的原文行，保证对比范围一致
pub const MAKE_DOCX_REVISIONS_SCRIPT: &str = r#"import json, os, sys, difflib
from datetime import datetime, timezone
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

AUTHOR = 'WMessage AI'
DATE = datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')

d = docx.Document()
st = d.styles['Normal']
st.font.name = '宋体'
st.font.size = Pt(12)
if title:
    h = d.add_heading('', level=1)
    r = h.add_run(title)
    r.font.name = '黑体'
    r.font.size = Pt(16)

_id = [1000]
def nid():
    _id[0] += 1
    return _id[0]

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
    el.set(qn('w:date'), DATE)
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
T = THEMES.get(theme, THEMES['blue'])
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
    if !py_get_enabled(app.clone()) {
        return Err(
            "Python 编程未开启：请到设置页「机器人设置」打开「允许机器人执行 Python」".into(),
        );
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
#[tauri::command]
pub async fn doc_extract(app: AppHandle, path: Option<String>) -> Result<DocExtract, String> {
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
            match picked.and_then(file_path_to_string) {
                Some(p) => p,
                // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
                None => return Err("用户取消了选择".into()),
            }
        }
    };
    py_audit(&app, &format!("doc_extract | path: {path}"));
    let input = serde_json::json!({ "path": path }).to_string();
    let r = run_doc_script(&app, "doc_extract", EXTRACT_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_extract failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(format!("提取失败：{}", r.stderr.trim()));
    }
    Ok(DocExtract {
        path,
        text: r.stdout,
    })
}

fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        tauri_plugin_dialog::FilePath::Url(u) => Some(u.to_string()),
    }
}

/// 生成 Word 到 AI_Gen_Files（不覆盖：同名自动加序号）
#[tauri::command]
pub async fn doc_make_word(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input =
        serde_json::json!({ "title": title, "paragraphs": paragraphs, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_word", MAKE_DOCX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_word failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(format!("生成 Word 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_word | out: {out}"));
    Ok(out)
}

/// 生成修订模式 Word（track changes）：回读原文与修订段落 diff，删除标删除线、新增标红色下划线，
/// 可在 Word 审阅中逐条接受/拒绝。original_path 优先回读文件保真；无路径时用 original 行列表。
#[tauri::command]
pub async fn doc_make_word_revisions(
    app: AppHandle,
    title: String,
    original_path: Option<String>,
    original: Vec<String>,
    revised: Vec<String>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "docx")?;
    let input = serde_json::json!({
        "title": title,
        "original_path": original_path.clone().unwrap_or_default(),
        "original": original,
        "revised": revised,
        "out": out
    })
    .to_string();
    let r = run_doc_script(&app, "doc_make_word_revisions", MAKE_DOCX_REVISIONS_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!(
                "doc_make_word_revisions failed | {}",
                escape_for_log(&r.stderr, 200)
            ),
        );
        return Err(format!("生成修订版 Word 失败：{}", r.stderr.trim()));
    }
    py_audit(
        &app,
        &format!(
            "doc_make_word_revisions | src: {} | out: {out}",
            original_path.unwrap_or_default()
        ),
    );
    Ok(out)
}

/// 生成 Excel 到 AI_Gen_Files（支持 =公式 单元格）
#[tauri::command]
pub async fn doc_make_excel(
    app: AppHandle,
    sheets: Vec<serde_json::Value>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "xlsx")?;
    let input = serde_json::json!({ "sheets": sheets, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_excel", MAKE_XLSX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_excel failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(format!("生成 Excel 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_excel | out: {out}"));
    Ok(out)
}

/// 生成 PDF 到 AI_Gen_Files
#[tauri::command]
pub async fn doc_make_pdf(
    app: AppHandle,
    title: String,
    paragraphs: Vec<String>,
    filename: Option<String>,
) -> Result<String, String> {
    let out = gen_out_path(&app, filename.as_deref(), "pdf")?;
    let input =
        serde_json::json!({ "title": title, "paragraphs": paragraphs, "out": out }).to_string();
    let r = run_doc_script(&app, "doc_make_pdf", MAKE_PDF_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_pdf failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(format!("生成 PDF 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_pdf | out: {out}"));
    Ok(out)
}

/// 生成 PPT 到 AI_Gen_Files
#[tauri::command]
pub async fn doc_make_ppt(
    app: AppHandle,
    title: String,
    slides: Vec<serde_json::Value>,
    filename: Option<String>,
    theme: Option<String>,
) -> Result<String, String> {
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
    let input = serde_json::json!({ "title": title, "slides": slides, "out": out, "theme": theme })
        .to_string();
    let r = run_doc_script(&app, "doc_make_ppt", MAKE_PPTX_SCRIPT, input).await?;
    if r.exit_code != Some(0) {
        py_audit(
            &app,
            &format!("doc_make_ppt failed | {}", escape_for_log(&r.stderr, 200)),
        );
        return Err(format!("生成 PPT 失败：{}", r.stderr.trim()));
    }
    py_audit(&app, &format!("doc_make_ppt | out: {out}"));
    Ok(out)
}

/// 输出路径：AI_Gen_Files/<文件名>；同名自动加 (n) 序号，永不覆盖（Harness 第 6 层）
fn gen_out_path(app: &AppHandle, filename: Option<&str>, ext: &str) -> Result<String, String> {
    let dir = crate::db::data_dir(app).join("AI_Gen_Files");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
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
    let base = base.trim_end_matches(&format!(".{ext}"));
    let mut candidate = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    Ok(candidate.to_string_lossy().to_string())
}

/// 审计日志安全转义 + 截断（P2-11）：剥换行/管道符，防伪造「INFO |」前缀与多行撕裂。
/// 规则：`| ` → `|  `（双空格），剩余裸 `|` → `||`，`\n` → `\\n`，`\r` → `\\r`；
/// 转义后按字符数截到 max 加省略号。
/// 例：`"a\nb| c"` → `"a\\nb||  c"`
fn escape_for_log(s: &str, max: usize) -> String {
    let escaped = s
        .replace("| ", "|  ")
        .replace('|', "||")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let count = escaped.chars().count();
    if count <= max {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── drain_output（C1：孙进程继承管道时 rx 收集不得永久阻塞）──

    #[test]
    fn drain_output_collects_both_channels() {
        let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>)>();
        tx.send(("out", b"hello".to_vec())).unwrap();
        tx.send(("err", b"warn".to_vec())).unwrap();
        drop(tx);
        let (out, err, complete) = drain_output(&rx, Duration::from_secs(1));
        assert!(complete);
        assert_eq!(out, "hello");
        assert_eq!(err, "warn");
    }

    #[test]
    fn drain_output_times_out_when_grandchild_holds_pipe() {
        // 模拟孙进程 fork 后 sleep 远超宽限、一直占着管道写端：
        // sender 线程 10s 后才发送（且只有一个方向），drain 必须在宽限到期后返回，
        // 而不是像原 rx.iter() 那样永久挂死
        let (tx, rx) = mpsc::channel::<(&'static str, Vec<u8>)>();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            let _ = tx.send(("out", b"late".to_vec()));
        });
        let start = Instant::now();
        let (_out, _err, complete) = drain_output(&rx, Duration::from_millis(300));
        assert!(!complete, "发送方卡死时不得报告收齐");
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "drain 挂死了：{:?}",
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
        let r = run_python_at(&py, &dir, &[], Some(1), &mut |l: &str| {
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
        let r = run_python_at("/nonexistent/python-zzz", &dir, &[], Some(1), &mut |l: &str| {
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
        let r = run_python_at("/nonexistent/python-zzz", &dir, &[], Some(1), &mut |_| {}, None);
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
        let guard = crate::bot_slash::StopGuard::new(false);
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
            let r = run_python_at(&py, &dir, &[], Some(60), &mut |l: &str| {
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
}
