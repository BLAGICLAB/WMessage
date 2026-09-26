//! Python / .NET 子进程执行核心
//!
//! **安全关键**：kill_tree / ChildRegGuard / RunLimits 逻辑一字不动；
//! 测试模块用 `super::is_live_group_leader` 等访问是经 `crate::bot_py` facade re-export。

use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;

use crate::bot_slash::StopToken;
use crate::error::CommandError;
use crate::py::audit::py_audit;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PyRunResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    #[serde(default)]
    pub truncated: bool,
}

pub const OUTPUT_CAP: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
pub const MAX_TIMEOUT_SECS: u64 = 300;

/// 解析超时：None → 60s 默认；> 300s → 钳到 300s 并返回 clamped=true
pub fn resolve_timeout(timeout_secs: Option<u64>) -> (u64, bool) {
    match timeout_secs {
        None => (DEFAULT_TIMEOUT_SECS, false),
        Some(s) if s > MAX_TIMEOUT_SECS => (MAX_TIMEOUT_SECS, true),
        Some(s) => (s, false),
    }
}

pub fn mem_limit_bytes(timeout_secs: u64) -> u64 {
    timeout_secs
        .saturating_mul(8)
        .saturating_mul(1024 * 1024)
        .clamp(256 * 1024 * 1024, 2 * 1024 * 1024 * 1024)
}

pub fn cpu_limit_secs(timeout_secs: u64) -> u64 {
    timeout_secs.saturating_add(10)
}

#[cfg(windows)]
mod win_job {
    use std::process::Child;
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
        // SAFETY: Win32 Job Object FFI 块。
        // - CreateJobObjectW(NULL, NULL) = 默认安全描述符、不命名（防跨进程名冲突）；
        // - &info 是有效 Rust 结构体，size_of_val 校验大小匹配 Win32 期望；
        // - SetInformationJobObject 失败时已 CloseHandle 兜底，不泄漏 HANDLE。
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_PROCESS_TIME
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            info.ProcessMemoryLimit = mem_bytes as usize;
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

    pub fn assign(job: &JobGuard, child: &Child) {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: child.as_raw_handle() 由 std 保证是 valid HANDLE（子进程已 spawn）；
        // job.0 由本模块 CreateJobObjectW 创建并由 JobGuard 持有；
        // 返回值忽略：AssignProcessToJobObject 失败时子进程仍可由 TerminateJobObject 强杀。
        unsafe {
            let _ = AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle() as _));
        }
    }

    pub fn terminate(job: &JobGuard) {
        // SAFETY: job.0 是 valid HANDLE（CreateJobObjectW 已返回 Ok）；
        // TerminateJobObject 对 valid HANDLE 调用安全，退出码 1 仅触发清理。
        unsafe {
            let _ = TerminateJobObject(job.0, 1);
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            // SAFETY: self.0 由 CreateJobObjectW 创建并独占持有；Drop 必须 CloseHandle 释放内核对象，
            // 否则 HANDLE 泄漏到进程退出。Drop 中 close 是所有权语义的标准用法。
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

pub struct RunLimits {
    #[cfg(windows)]
    job: Option<win_job::JobGuard>,
}

impl RunLimits {
    pub fn new(_mem_bytes: u64, _cpu_secs: u64) -> Self {
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

    pub fn assign(&self, _child: &Child) {
        #[cfg(windows)]
        if let Some(j) = &self.job {
            win_job::assign(j, _child);
        }
    }

    pub fn terminate(&self) {
        #[cfg(windows)]
        if let Some(j) = &self.job {
            win_job::terminate(j);
        }
    }
}

#[cfg(unix)]
pub fn is_live_group_leader(pid: u32) -> bool {
    // SAFETY: pid 由调用方传入（subprocess 子进程 PID），本函数语义：若 getpgid(pid) == pid 则该 pid 是进程组 leader。
    // POSIX getpgid(0) 返回调用方 PGID 是无害的（这里 pid 是入参，不会传 0）。返回值仅用于 == 比较，无 wrapper 误用风险。
    unsafe { libc::getpgid(pid as i32) == pid as i32 }
}

pub fn kill_tree(child: &mut Child, limits: &RunLimits) {
    limits.terminate();
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        let _ = silent_cmd("kill").args(["-9", &format!("-{pid}")]).status();
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

pub fn cleanup_after_fail(child: &mut Child, dir: &std::path::Path, limits: &RunLimits) {
    kill_tree(child, limits);
    let _ = std::fs::remove_dir_all(dir);
}

pub fn cleanup_after_spawn_fail(dir: &std::path::Path, limits: &RunLimits) {
    limits.terminate();
    let _ = std::fs::remove_dir_all(dir);
}

pub fn py_runs_root() -> std::path::PathBuf {
    std::env::temp_dir().join("wmessage-py-runs")
}

pub fn setup_run_dir(
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

pub static PY_CHILDREN: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

pub fn py_children() -> &'static Mutex<HashSet<u32>> {
    PY_CHILDREN.get_or_init(|| Mutex::new(HashSet::new()))
}

pub struct ChildRegGuard(u32);

impl ChildRegGuard {
    pub fn register(child: &Child) -> Self {
        py_children()
            .lock()
            .unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] py::runtime PY_CHILDREN: {e:?}");
                e.into_inner()
            })
            .insert(child.id());
        Self(child.id())
    }
}

impl Drop for ChildRegGuard {
    fn drop(&mut self) {
        py_children()
            .lock()
            .unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] py::runtime PY_CHILDREN: {e:?}");
                e.into_inner()
            })
            .remove(&self.0);
    }
}

pub fn kill_py_children(pids: &[u32]) -> usize {
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
            false
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

pub fn kill_all_py_children() -> usize {
    let pids: Vec<u32> = py_children()
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] py::runtime PY_CHILDREN: {e:?}");
            e.into_inner()
        })
        .iter()
        .copied()
        .collect();
    kill_py_children(&pids)
}

#[cfg(unix)]
pub const PARENT_WATCHDOG: &str = concat!(
    "import os as _wm_os, threading as _wm_th, time as _wm_tm\n",
    "def _wm_watchdog():\n",
    "    while True:\n",
    "        if _wm_os.getppid() <= 1:\n",
    "            _wm_os._exit(137)\n",
    "        _wm_tm.sleep(2)\n",
    "_wm_th.Thread(target=_wm_watchdog, daemon=True).start()",
);

pub static PY_RUN_GATE: Mutex<()> = Mutex::new(());

pub fn py_run_gate() -> &'static Mutex<()> {
    &PY_RUN_GATE
}

pub static EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mark_exiting() {
    EXITING.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub fn reset_exiting_for_test() {
    EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
}

pub struct RunFail {
    pub msg: String,
    pub spawn_not_found: bool,
}

pub fn run_python(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, CommandError> {
    let _gate = py_run_gate().lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] py::runtime::py_run_gate: {e:?}");
        e.into_inner()
    });
    if EXITING.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(CommandError::DomainRule {
            domain: "python".to_string(),
            reason: "应用正在退出，不再启动新的 Python 任务".to_string(),
        });
    }
    run_python_ungated(app, script, input_json, args, timeout_secs, stop)
}

pub fn run_python_ungated(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, CommandError> {
    #[cfg(unix)]
    let script_owned;
    #[cfg(unix)]
    let script = {
        script_owned = format!("{PARENT_WATCHDOG}\n{script}");
        script_owned.as_str()
    };
    let mut py = match crate::py::env::cached_python() {
        Some(p) => p,
        None => {
            py_audit(app, "run_python err | kind=no_python");
            return Err(CommandError::DomainRule {
                domain: "python".to_string(),
                reason: "本机未检测到 Python。macOS 请安装 Command Line Tools；Windows 请到 python.org 安装并勾选 Add to PATH".to_string(),
            });
        }
    };

    let mut audit_sink = |line: &str| py_audit(app, line);
    let gen = crate::db::gen_dir(app).ok();
    for attempt in 0..2 {
        let dir = match setup_run_dir(&py_runs_root(), script, input_json) {
            Ok(d) => d,
            Err(e) => {
                audit_sink(&format!(
                    "run_python err | kind=setup_fail | {}",
                    crate::audit::escape_for_log(&e, 200)
                ));
                return Err(CommandError::DomainRule {
                    domain: "python".to_string(),
                    reason: format!("准备运行目录失败：{e}"),
                });
            }
        };
        match run_python_at(
            &py,
            Some("run.py"),
            &dir,
            gen.as_deref(),
            args,
            timeout_secs,
            &mut audit_sink,
            stop,
        ) {
            Ok(r) => return Ok(r),
            Err(f) => {
                if attempt == 0 && f.spawn_not_found {
                    crate::py::env::invalidate_python_cache();
                    if let Some(fresh) = crate::py::env::cached_python() {
                        py = fresh;
                        continue;
                    }
                }
                return Err(CommandError::DomainRule {
                    domain: "python".to_string(),
                    reason: f.msg,
                });
            }
        }
    }
    unreachable!("最多 2 次尝试，循环内必然返回")
}

/// macOS/Windows 平台差异的 Command 构造器（强制 UTF-8 环境变量）
#[cfg(windows)]
pub fn silent_cmd(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(0x08000000)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    cmd
}

#[cfg(not(windows))]
pub fn silent_cmd(program: &str) -> Command {
    Command::new(program)
}

// ── S20：setrlimit 可用性探测（子进程一次探测 + 进程内缓存，逐资源结论）──
//
// 背景：pre_exec 里的 setrlimit 在部分环境被拒（实测：裸 macOS 内核即拒
// setrlimit(RLIMIT_AS)，测试 sandbox 连 CPU 一起拒），旧实现静默吞掉
// →「限额看似生效实则没生效」；硬失败方案此前实测打断 py_exec
// （PHASE3-MEDIUM-TRIAGE P3S-20 FIX→回退登记）。折中：首次 py 执行前用同一解释器
// 子进程逐资源探测一次并缓存；未生效项不设限额但必须 audit 留痕，可用项照设。
// 硬约束：**探测失败 ≠ 可用**——ProbeError 项一律按「未探测到」处理，不许静默放行。

/// 单个资源的 setrlimit 探测结论。ProbeError 表示探测自身失败（spawn/超时/输出
/// 缺项/不可解析），**不许当可用**。
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetrlimitProbe {
    Available,
    Unavailable(String),
    ProbeError(String),
}

/// RLIMIT_AS / RLIMIT_CPU 逐资源探测结论。
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetrlimitSupport {
    pub rlimit_as: SetrlimitProbe,
    pub rlimit_cpu: SetrlimitProbe,
}

impl SetrlimitSupport {
    #[cfg(unix)]
    fn all_available(&self) -> bool {
        self.rlimit_as == SetrlimitProbe::Available && self.rlimit_cpu == SetrlimitProbe::Available
    }
}

#[cfg(unix)]
static SETRLIMIT_PROBE: OnceLock<SetrlimitSupport> = OnceLock::new();

#[cfg(unix)]
const SETRLIMIT_PROBE_SCRIPT: &str = concat!(
    "import resource as _r\n",
    "def _t(res, v):\n",
    "    try:\n",
    "        _r.setrlimit(res, (v, v))\n",
    "        return 'OK'\n",
    "    except (OSError, ValueError) as e:\n",
    "        # CPython 把 setrlimit 的 EINVAL 映射成 ValueError（文案固定），一并捕获\n",
    "        return '%s: %s' % (type(e).__name__, e)\n",
    "print('WM_RLIMIT_AS=' + _t(_r.RLIMIT_AS, 2 * 1024 * 1024 * 1024))\n",
    "print('WM_RLIMIT_CPU=' + _t(_r.RLIMIT_CPU, 310))\n",
);
// 探测值取生产上限（AS 2GB = mem_limit_bytes 封顶 / CPU 310s = timeout 300+10 宽限封顶）：
// 上限通过 → 单调 ceiling 下全部生产值可用；上限被拒 → 从严判 Unavailable 留痕。
// 方向性硬约束：宁可误判「不可用」留痕（丢限额但诚实），不可误判「可用」静默（S20 原罪）。
// 已知残差（OCR r1 采纳）：仅拒中间值、放行两端的非单调沙箱不在覆盖；pre_exec 内仍
// best-effort 静默，不做二次探测。

/// 从探测 stdout 逐资源提结论；缺项/空值按探测失败算（绝不猜成可用）。
#[cfg(unix)]
pub fn parse_setrlimit_probe_output(out: &str) -> SetrlimitSupport {
    fn pick(tag: &str, out: &str) -> SetrlimitProbe {
        let line = out
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with(tag))
            .map(|l| &l[tag.len()..]);
        match line {
            Some("OK") => SetrlimitProbe::Available,
            Some("") | None => SetrlimitProbe::ProbeError(format!("探测输出缺 {tag} 项")),
            Some(err) => SetrlimitProbe::Unavailable(err.to_string()),
        }
    }
    SetrlimitSupport {
        rlimit_as: pick("WM_RLIMIT_AS=", out),
        rlimit_cpu: pick("WM_RLIMIT_CPU=", out),
    }
}

/// 未生效/探测失败项的每运行 audit 行（全部可用时返回 None，不另扰）。
/// 硬约束文案：「限额未生效」/「未探测到 setrlimit 可用性」。
#[cfg(unix)]
pub fn rlimit_warn_line(s: &SetrlimitSupport) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (name, p) in [("RLIMIT_AS", &s.rlimit_as), ("RLIMIT_CPU", &s.rlimit_cpu)] {
        match p {
            SetrlimitProbe::Available => {}
            SetrlimitProbe::Unavailable(w) => {
                parts.push(format!("{name}: 限额未生效({w})"));
            }
            SetrlimitProbe::ProbeError(w) => {
                parts.push(format!("{name}: 未探测到 setrlimit 可用性，限额未设({w})"));
            }
        }
    }
    (!parts.is_empty()).then(|| format!("run_python warn | rlimit_off | {}", parts.join("; ")))
}

#[cfg(unix)]
pub fn run_setrlimit_probe(py: &str) -> SetrlimitSupport {
    // 与 run_python_at 同一构造器（参数列表传参，无 shell 参与，无注入面）。
    let mut cmd = silent_cmd(py);
    cmd.arg("-c")
        .arg(SETRLIMIT_PROBE_SCRIPT)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SetrlimitSupport {
                rlimit_as: SetrlimitProbe::ProbeError(format!("探测子进程 spawn 失败: {e}")),
                rlimit_cpu: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
            };
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => {
                return SetrlimitSupport {
                    rlimit_as: SetrlimitProbe::ProbeError(format!("探测子进程 wait 失败: {e}")),
                    rlimit_cpu: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
                };
            }
        }
    };
    let Some(status) = status else {
        let msg = "探测子进程超时（10s）已强杀".to_string();
        return SetrlimitSupport {
            rlimit_as: SetrlimitProbe::ProbeError(msg.clone()),
            rlimit_cpu: SetrlimitProbe::ProbeError(msg),
        };
    };
    if !status.success() {
        let msg = format!("探测子进程退出码异常: {status}");
        return SetrlimitSupport {
            rlimit_as: SetrlimitProbe::ProbeError(msg.clone()),
            rlimit_cpu: SetrlimitProbe::ProbeError(msg),
        };
    }
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut s, &mut out);
    }
    parse_setrlimit_probe_output(&out)
}

/// 进程内一次缓存：rlimit 可用性是 OS/沙箱属性，与具体解释器实例无关，
/// py 路径中途失效不重探。全可用时 audit 一条确认；未生效/探测失败留给每运行 warn。
#[cfg(unix)]
fn setrlimit_probe_cached(py: &str, audit: &mut dyn FnMut(&str)) -> SetrlimitSupport {
    SETRLIMIT_PROBE
        .get_or_init(|| {
            let r = run_setrlimit_probe(py);
            if r.all_available() {
                audit("run_python | rlimit_probe | setrlimit 全可用，限额照设");
            }
            r
        })
        .clone()
}

pub fn run_python_at(
    py: &str,
    entry: Option<&str>,
    dir: &std::path::Path,
    gen_dir: Option<&std::path::Path>,
    args: &[String],
    timeout_secs: Option<u64>,
    audit: &mut dyn FnMut(&str),
    stop: Option<&StopToken>,
) -> Result<PyRunResult, RunFail> {
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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        // S20 探测降级：逐资源——可用项照设；未生效/探测失败项不设但必须 audit 留痕
        // （探测失败 ≠ 可用，见 SetrlimitProbe）。进程组隔离与限额无关，必须保留
        //（kill_tree 的进程组杀依赖它），任何分支都不能丢。
        let support = setrlimit_probe_cached(py, audit);
        if let Some(w) = rlimit_warn_line(&support) {
            audit(&w);
        }
        let as_ok = support.rlimit_as == SetrlimitProbe::Available;
        let cpu_ok = support.rlimit_cpu == SetrlimitProbe::Available;
        if as_ok || cpu_ok {
            // SAFETY: pre_exec 在 fork 后 exec 前运行，处于单线程上下文（POSIX 要求 async-signal-safe）。
            // libc::setrlimit 对 RLIMIT_AS / RLIMIT_CPU 是 async-signal-safe 调用。
            // mem_bytes / cpu_secs 来自 RunLimits，由调用方在子进程 spawn 前已 validate 非零。
            // process_group(0) 已将子进程设为新进程组，避免 setpgid 边界。
            // 部分可用（如裸 macOS：AS 被内核拒、CPU 可设）只设可用项；探测可用后此处
            // 再失败（环境突变的极端组合）仍 best-effort 静默，不回退硬失败。
            unsafe {
                cmd.pre_exec(move || {
                    if as_ok {
                        let mem = libc::rlimit {
                            rlim_cur: mem_bytes as libc::rlim_t,
                            rlim_max: mem_bytes as libc::rlim_t,
                        };
                        libc::setrlimit(libc::RLIMIT_AS, &mem);
                    }
                    if cpu_ok {
                        let cpu = libc::rlimit {
                            rlim_cur: cpu_secs as libc::rlim_t,
                            rlim_max: cpu_secs as libc::rlim_t,
                        };
                        libc::setrlimit(libc::RLIMIT_CPU, &cpu);
                    }
                    Ok(())
                });
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000 | 0x00000200);
    }
    if let Some(e) = entry {
        cmd.arg(e);
    }
    if let Some(g) = gen_dir {
        cmd.env("WM_GEN_DIR", g);
    }
    cmd.env("WM_TMP_DIR", std::env::temp_dir());
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
                crate::audit::escape_for_log(&e.to_string(), 200)
            ));
            cleanup_after_spawn_fail(dir, &limits);
            return Err(RunFail {
                msg: format!("启动 Python 失败：{e}"),
                spawn_not_found: e.kind() == std::io::ErrorKind::NotFound,
            });
        }
    };
    limits.assign(&child);
    let _child_reg = ChildRegGuard::register(&child);

    use std::sync::mpsc;
    let child_stdout = child.stdout.take();
    let child_stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel();
    let tx_out = tx.clone();
    let stop_out = stop.cloned();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut truncated = false;
        if let Some(s) = child_stdout {
            let (b, tr) = crate::py::io::read_capped_drain(
                crate::py::io::StopReader::new(s, stop_out),
                OUTPUT_CAP + 1024,
            );
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
            let (b, tr) = crate::py::io::read_capped_drain(
                crate::py::io::StopReader::new(s, stop_err),
                OUTPUT_CAP + 1024,
            );
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
                cleanup_after_fail(&mut child, dir, &limits);
                audit(&format!("run_python err | kind=wait_fail | {e}"));
                crate::py::io::join_reader_threads(out_handle, err_handle, audit);
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
            crate::py::io::join_reader_threads(out_handle, err_handle, audit);
            return Err(RunFail {
                msg: format!("执行超时（{}s）已强制终止", timeout.as_secs()),
                spawn_not_found: false,
            });
        }
        if stop.is_some_and(|s| s.stopped()) {
            cleanup_after_fail(&mut child, dir, &limits);
            audit("run_python err | kind=stopped");
            crate::py::io::join_reader_threads(out_handle, err_handle, audit);
            return Err(RunFail {
                msg: "已停止".into(),
                spawn_not_found: false,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let (stdout, mut stderr, drained, truncated) =
        crate::py::io::drain_output(&rx, Duration::from_secs(2));
    if !drained {
        kill_tree(&mut child, &limits);
        audit("run_python warn | kind=drain_timeout | 孙进程占用管道已强杀进程组");
        stderr.push_str("\n（输出收集超时：孙进程占用管道，已强杀进程组）");
    }
    crate::py::io::join_reader_threads(out_handle, err_handle, audit);
    if truncated {
        audit(&format!(
            "run_python warn | kind=output_truncated | cap={OUTPUT_CAP}"
        ));
    }
    let duration_ms = start.elapsed().as_millis();
    if let Some(g) = gen_dir {
        crate::py::harvest::harvest_run_outputs(dir, g, audit);
    }
    let _ = std::fs::remove_dir_all(dir);
    let Some(code) = exit_code else {
        audit("run_python err | kind=exit_none | 无退出码，子进程疑似被信号杀死");
        return Err(RunFail {
            msg: "子进程异常终止（被信号杀死，无退出码）".into(),
            spawn_not_found: false,
        });
    };
    Ok(PyRunResult {
        stdout: crate::py::io::truncate_output(stdout),
        stderr: crate::py::io::truncate_output(stderr),
        exit_code: Some(code),
        duration_ms,
        truncated,
    })
}
