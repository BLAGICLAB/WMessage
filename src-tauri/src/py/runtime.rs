//! Python / .NET 子进程执行核心
//!
//! **安全关键**：kill_tree / ChildRegGuard / RunLimits 逻辑一字不动；
//! 测试模块用 `super::is_live_group_leader` 等访问是经 `crate::bot_py` facade re-export。

use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::AppHandle;

use crate::bot::StopToken;
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
    /// 失败时保留下来的运行目录（脚本还在里面，可读回后改几行再跑）。
    /// 成功时为空——目录已连同产物一起清掉。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_dir: Option<String>,
    /// 搬进产物目录的产物文件绝对路径：工具侧据此给每个产物补落盘凭证
    /// （路径 + 内容哈希），否则它们只躺在盘上，谁都看不见
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harvested: Vec<String>,
}

pub const OUTPUT_CAP: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = crate::bot::constants::PYTHON_TIMEOUT_SECS;
/// run_python 工具路径的单次执行天花板（秒，Claude Code Bash 同值），也是子 agent
/// 停滞超时（600s）的上界——两个数必须一起动；apps 用更宽的 1800s（用户直跑）。
pub const MAX_TIMEOUT_SECS: u64 = 600;
pub const APPS_MAX_TIMEOUT_SECS: u64 = 1800;
pub fn resolve_timeout(timeout_secs: Option<u64>) -> (u64, bool) {
    resolve_timeout_with_cap(timeout_secs, MAX_TIMEOUT_SECS)
}

/// 带调用方天花板的钳制：工具路径传 `MAX_TIMEOUT_SECS`，apps 用户直跑传
/// `APPS_MAX_TIMEOUT_SECS`——apps 声明的 1800s 天花板在执行层真实生效，
/// 而不是声明完被这条统一钳制悄悄折回 600。cap 不得低于 `DEFAULT_TIMEOUT_SECS`
/// （现役两个 cap 600/1800 都满足）。
pub fn resolve_timeout_with_cap(timeout_secs: Option<u64>, cap: u64) -> (u64, bool) {
    match timeout_secs {
        Some(s) if s > cap => (cap, true),
        Some(s) => (s, false),
        None => (DEFAULT_TIMEOUT_SECS, false),
    }
}

/// 写侧钳制（1..=MAX_TIMEOUT_SECS）——设置页落盘前用。运行期钳制在
/// `resolve_timeout` 但只在执行时生效：写下 10000、显示 10000、实跑 600——
/// 落盘值必须等于生效值，写侧先钳一遍。
pub fn clamp_timeout_secs(n: u64) -> u64 {
    n.clamp(1, MAX_TIMEOUT_SECS)
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

/// 进程数上限（RLIMIT_NPROC / Job ACTIVE_PROCESS）：fork 炸弹在运行窗口内
/// 可打满系统资源，超时杀组只是事后回收。
///
/// **RLIMIT_NPROC 的语义是按用户计数**（不是按进程组）：上限必须盖住当前
/// 用户已有的全部进程再加余量，否则 Python 一启动自己就 EAGAIN（本机实测
/// 单用户数百进程时设 64 = 探测都过、首次 spawn 即死）。基数取探测时刻的
/// 用户进程数 + 64 余量：挡得住运行窗口内的进程风暴（风暴要在基数之上
/// 再造几百个进程才撞限），又不误杀正常并发。
#[cfg(unix)]
pub(crate) fn nproc_limit() -> u64 {
    nproc_limit_for(user_proc_count())
}

/// 基数 + 余量（纯函数，可测）。
#[cfg(unix)]
pub(crate) fn nproc_limit_for(base: u64) -> u64 {
    base.saturating_add(64).max(128)
}

/// 当前用户名关联的进程数（`ps -u <user>` 行数减表头；拿不到用户名/执行
/// 失败按 0 算——上限退到纯余量 128，仍比无限 Lind强者）。
#[cfg(unix)]
fn user_proc_count() -> u64 {
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() {
        return 0;
    }
    std::process::Command::new("ps")
        .args(["-u", &user, "-o", "pid="])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count() as u64
        })
        .unwrap_or(0)
}

#[cfg(windows)]
pub(crate) mod win_job {
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

    // SAFETY: HANDLE 是内核对象句柄（不透明整数），不绑定线程也不绑定进程；
    // 换持有者只是换了谁负责 CloseHandle，句柄上的操作
    // （SetInformation / Assign / Terminate / CloseHandle）本身都是线程安全的。
    unsafe impl Send for JobGuard {}

    /// 只要「宿主一死就杀光」这一条（不限内存 / CPU）——受管服务层用：
    /// 长驻 dev server 不该被内存限额误杀，但宿主崩了必须跟着走。
    pub fn create_kill_on_close() -> Option<JobGuard> {
        // SAFETY: 同 create()：CreateJobObjectW 失败返回 None；
        // SetInformationJobObject 失败时 CloseHandle 兜底，不泄漏 HANDLE。
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
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
    // 超过 i32::MAX 的 pid 转换会回绕成负数（极端情况 -1），getpgid 的错误返回
    // 值恰好也是 -1，等值比较会误判「存活 leader」→ 后续 kill -9 --1 误伤全系统。
    // 真实内核 pid 远到不了这个值，纯防御；拒判即可。
    if pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: pid 由调用方传入（subprocess 子进程 PID），本函数语义：若 getpgid(pid) == pid 则该 pid 是进程组 leader。
    // POSIX getpgid(0) 返回调用方 PGID 是无害的（这里 pid 是入参，不会传 0）。返回值仅用于 == 比较，无 wrapper 误用风险。
    unsafe { libc::getpgid(pid as i32) == pid as i32 }
}

/// 进程组是否仍存在（组长已死、组里还有孙进程时也为真）。
///
/// 用于「能不能按组号杀」的判定。`kill(-pgid, 0)` 对不存在的组回 ESRCH、
/// 对存在但无权限的组回 EPERM——两者都按「不在」处理（真去杀也会失败）。
///
/// 问的是**组还在不在**而不是「组长还在不在」：主循环已 reap 子进程，
/// 组长已死时 getpgid 失败，只问组长会让「孙进程占住管道」那条路径
/// 再也杀不掉进程组——而那正是 kill_tree 存在的理由。
#[cfg(unix)]
pub fn process_group_alive(pid: u32) -> bool {
    // pid 0 = 调用方自己的进程组（kill -0 语义），绝不能当目标；
    // 超 i32::MAX 的 pid 转 i32 会回绕成负数 → `kill -9 --1` = 杀光本用户
    // 所有进程（同文件 is_live_group_leader 的注释）。
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: pid 已过区间校验（非 0、非负）；kill 的负 pid 表示进程组，
    // 信号 0 只做存在性探测、不投递任何信号。
    unsafe { libc::kill(-(pid as i32), 0) == 0 }
}

pub fn kill_tree(child: &mut Child, limits: &RunLimits) {
    limits.terminate();
    #[cfg(unix)]
    {
        let pid = child.id();
        // 组号 = 子进程 pid（spawn 时 process_group(0) 起组，组号恒等于 pid）。
        // 两道护栏都在「误伤系统」的位置上：
        // - pid 必须落在 i32 有效区间：转 i32 回绕成负数 → kill -9 --1
        //   杀光本用户所有进程；
        // - 该组必须还在：组号可能被别的进程组占用（pid 回收），杀错组
        //   等于杀无关进程。
        if process_group_alive(pid) {
            let _ = silent_cmd("kill").args(["-9", &format!("-{pid}")]).status();
        }
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

/// 运行目录里的脚本正文文件名（模型写的那份，原样落盘）。
///
/// 正文与执行入口**分文件**：正文保持模型写的样子，报错行号才与模型自己的
/// 文件一一对应；入口只负责起看门狗后转交正文。
pub const SCRIPT_FILE: &str = "script.py";
/// 运行目录里的执行入口文件名（宿主生成）。
pub const ENTRY_FILE: &str = "run.py";

/// 执行入口正文：起看门狗后转交脚本正文。
///
/// 看门狗**不能拼进脚本正文**：拼进去 traceback 的行号会整体下移（unix 上 7 行），
/// 模型按报错行号去改自己的脚本就会改错行——而「照着报错行号只改那一行」正是
/// 长脚本不再整段重写的关键。正文单独放 script.py，行号才对得上。
///
/// 转交方式用 exec 而不是 runpy.run_path：后者会把 `sys.modules['__main__']`
/// 换成临时模块，multiprocessing 的 spawn 子进程据此回读主模块时读到的就是
/// script.py ——那份里没有看门狗，宿主崩了子进程会留下。exec 进 run.py 自己的
/// 全局里，`__file__` / `sys.modules['__main__'].__file__` 都还指着入口，
/// 与拆分前一致。
fn entry_source() -> String {
    let mut src = String::new();
    #[cfg(unix)]
    {
        // 看门狗自带 import os/threading/time
        src.push_str(PARENT_WATCHDOG);
        src.push('\n');
    }
    #[cfg(not(unix))]
    {
        src.push_str("import os as _wm_os\n");
    }
    src.push_str(
        "_wm_script = _wm_os.path.join(_wm_os.path.dirname(_wm_os.path.abspath(__file__)), 'script.py')\n",
    );
    // utf-8-sig：带 BOM 的脚本也能正常执行（拆前由解释器剥 BOM，拆后自己读要自己剥）
    src.push_str("with open(_wm_script, encoding='utf-8-sig') as _wm_f:\n");
    src.push_str("    exec(compile(_wm_f.read(), _wm_script, 'exec'), globals())\n");
    src
}

pub fn cleanup_after_fail(child: &mut Child, limits: &RunLimits) {
    kill_tree(child, limits);
    // 运行目录**不删**：脚本留在里面，模型失败后能读回「刚才到底跑了什么」。
    // 目录由超龄清扫回收（见 py::harvest::sweep_stale_py_runs_in）。
}

pub fn cleanup_after_spawn_fail(dir: &std::path::Path, limits: &RunLimits) {
    limits.terminate();
    // spawn 失败 = 脚本根本没跑起来，没有「刚才跑了什么」可读回，直接清掉。
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
        // 正文（模型写的那份，原样）与执行入口（看门狗 + 转交正文）分文件：
        // 正文行号与模型自己的文件一一对应，报错行号才能直接照着改。
        std::fs::write(dir.join(SCRIPT_FILE), script).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(ENTRY_FILE), entry_source()).map_err(|e| e.to_string())?;
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
        crate::db::lock::lock_or_recover(py_children(), "py::runtime PY_CHILDREN")
            .insert(child.id());
        Self(child.id())
    }
}

impl Drop for ChildRegGuard {
    fn drop(&mut self) {
        crate::db::lock::lock_or_recover(py_children(), "py::runtime PY_CHILDREN").remove(&self.0);
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
    let pids: Vec<u32> = crate::db::lock::lock_or_recover(py_children(), "py::runtime PY_CHILDREN")
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

// 闸门族（许可信号量 + EXITING / 停止复查）在 gate.rs：纯闸门语义与执行器
// 无依赖。这里 re-export 保持原调用路径不变（document.rs 仍从 py::runtime
// 取 py_gate_acquire；run_python 走带停止令牌的入口）。
#[cfg(test)]
pub(crate) use super::gate::{
    gate_test_lock, py_gate_acquire_for_test, PY_PERMITS, PY_RUN_MAX_CONCURRENT,
};
pub(crate) use super::gate::{py_gate_acquire, py_gate_acquire_with_stop};

pub static EXITING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mark_exiting() {
    EXITING.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub fn reset_exiting_for_test() {
    EXITING.store(false, std::sync::atomic::Ordering::SeqCst);
}

/// Python 执行的失败种类（机器读）。
///
/// 为什么要有「种类」而不只是一句话：调用方要**如实上报**这次运行到底怎么了——
/// 「没能开始跑」「跑了但超时被杀」「跑了但被停止」是三回事，混成一句
/// 「运行启动失败」就是说谎了（超时那次程序明明跑起来了）。文案会改，种类是稳定的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyFailKind {
    /// 没能开始跑：没装 Python、spawn 失败、运行目录建不出来、闸门拒发
    StartFailed,
    /// 跑起来了，但超过时长上限被强杀
    Timeout,
    /// 跑起来了，但被停止（用户 /stop、应用退出、子 agent 停滞超时收割）
    Stopped,
    /// 跑起来了，但出了别的错（等待子进程状态失败、被信号杀死等）
    Runtime,
}

/// Python 执行的失败：种类 + 人读原因。
///
/// 转 `CommandError` 仍是 DomainRule(python)——对外协议不变，只是把种类带到了
/// 调用方手上（`From<PyRunError>`）。
#[derive(Debug, Clone)]
pub struct PyRunError {
    pub kind: PyFailKind,
    pub msg: String,
}

impl std::fmt::Display for PyRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl From<PyRunError> for CommandError {
    fn from(e: PyRunError) -> Self {
        CommandError::DomainRule {
            domain: "python".to_string(),
            reason: e.msg,
        }
    }
}

#[derive(Debug)]
pub struct RunFail {
    pub kind: PyFailKind,
    pub msg: String,
    pub spawn_not_found: bool,
}

pub fn run_python(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
    timeout_cap: u64,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, PyRunError> {
    // 有界并发闸（许可 2）。EXITING 与停止复查在闸门内完成（等待前/唤醒后/
    // 拿到许可后），排队者过闸即拒——不会 spawn 出无人收割的孤儿进程
    let _gate = py_gate_acquire_with_stop(stop).map_err(|e| PyRunError {
        kind: PyFailKind::StartFailed,
        msg: e.message(),
    })?;
    run_python_ungated(
        app,
        script,
        input_json,
        args,
        timeout_secs,
        timeout_cap,
        stop,
    )
}

pub fn run_python_ungated(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
    timeout_cap: u64,
    stop: Option<&StopToken>,
) -> Result<PyRunResult, PyRunError> {
    // 看门狗不再拼进脚本正文：拼接会让 traceback 行号整体下移，模型按报错
    // 行号改自己的脚本就会改错行。看门狗改由执行入口承担（见 entry_source）。
    let mut py = match crate::py::env::cached_python() {
        Some(p) => p,
        None => {
            py_audit(app, "run_python err | kind=no_python");
            return Err(PyRunError {
                kind: PyFailKind::StartFailed,
                msg: "本机未检测到 Python。macOS 请安装 Command Line Tools；Windows 请到 python.org 安装并勾选 Add to PATH".to_string(),
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
                return Err(PyRunError {
                    kind: PyFailKind::StartFailed,
                    msg: format!("准备运行目录失败：{e}"),
                });
            }
        };
        match run_python_at(
            &py,
            Some(ENTRY_FILE),
            &dir,
            gen.as_deref(),
            args,
            timeout_secs,
            timeout_cap,
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
                return Err(PyRunError {
                    kind: f.kind,
                    msg: f.msg,
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
    cmd.creation_flags(crate::bot::procs::CREATE_NO_WINDOW)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    cmd
}

#[cfg(not(windows))]
pub fn silent_cmd(program: &str) -> Command {
    Command::new(program)
}

/// 子进程环境裁剪 + 编码基线：子进程默认继承宿主全量环境，用户 shell 里
/// 导出的密钥（API token、数据库连接串之类）会原样进 app / run_python 脚本
/// 的可见范围。改白名单：只带运行 Python 必需的系统变量（见 py_env_allowed）；
/// WM_* 业务变量由调用方在裁剪后显式补。编码基线原先只在 Windows 的
/// silent_cmd 里设，裁剪后统一显式补，不依赖宿主 shell 恰好带没带。
fn apply_child_env(cmd: &mut Command) {
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        if k.to_str().is_some_and(crate::bot::procs::child_env_allowed) {
            cmd.env(k, v);
        }
    }
    cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
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
    pub rlimit_nproc: SetrlimitProbe,
}

#[cfg(unix)]
impl SetrlimitSupport {
    #[cfg(unix)]
    fn all_available(&self) -> bool {
        self.rlimit_as == SetrlimitProbe::Available
            && self.rlimit_cpu == SetrlimitProbe::Available
            && self.rlimit_nproc == SetrlimitProbe::Available
    }
}

#[cfg(unix)]
static SETRLIMIT_PROBE: OnceLock<SetrlimitSupport> = OnceLock::new();

#[cfg(unix)]
const SETRLIMIT_PROBE_SCRIPT: &str = concat!(
    "import resource as _r, sys\n",
    "def _t(res, v):\n",
    "    try:\n",
    "        _r.setrlimit(res, (v, v))\n",
    "        return 'OK'\n",
    "    except (OSError, ValueError) as e:\n",
    "        # CPython 把 setrlimit 的 EINVAL 映射成 ValueError（文案固定），一并捕获\n",
    "        return '%s: %s' % (type(e).__name__, e)\n",
    "print('WM_RLIMIT_AS=' + _t(_r.RLIMIT_AS, int(sys.argv[1])))\n",
    "print('WM_RLIMIT_CPU=' + _t(_r.RLIMIT_CPU, int(sys.argv[2])))\n",
    // NPROC 上限按生产值（argv[3]）：脚本进程自身 + 看门狗 + 用户脚本，
    // 太紧会挡掉 multiprocessing 的正常并发，取值见 nproc_limit()
    "print('WM_RLIMIT_NPROC=' + _t(_r.RLIMIT_NPROC, int(sys.argv[3])))\n",
);
// 探测值取生产上限，且**由 argv 从 Rust 常量传入**（不在脚本文本里写死数字）：
// AS 2GB = mem_limit_bytes 封顶 / CPU = timeout 上限 + 10 宽限。脚本与生产共用
// 同一组常量，天花板一改两边同时跟着变，不会出现「探针按旧值判可用、生产按新值
// 设置」的错位（那会让限额静默失效）。
// 上限通过 → 单调 ceiling 下全部生产值可用；上限被拒 → 从严判 Unavailable 留痕。
// 方向性硬约束：宁可误判「不可用」留痕（丢限额但诚实），不可误判「可用」静默（S20 原罪）。
// 已知残差：仅拒中间值、放行两端的非单调沙箱不在覆盖；pre_exec 内仍
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
        rlimit_nproc: pick("WM_RLIMIT_NPROC=", out),
    }
}

/// 未生效/探测失败项的每运行 audit 行（全部可用时返回 None，不另扰）。
/// 硬约束文案：「限额未生效」/「未探测到 setrlimit 可用性」。
#[cfg(unix)]
pub fn rlimit_warn_line(s: &SetrlimitSupport) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (name, p) in [
        ("RLIMIT_AS", &s.rlimit_as),
        ("RLIMIT_CPU", &s.rlimit_cpu),
        ("RLIMIT_NPROC", &s.rlimit_nproc),
    ] {
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
        // 探测值来自生产常量（见 SETRLIMIT_PROBE_SCRIPT 注），脚本自己只读 argv。
        // 取两条路径（工具 600s / apps 1800s）中更宽的限额探测：能设宽才代表真可用。
        .arg(mem_limit_bytes(APPS_MAX_TIMEOUT_SECS).to_string())
        .arg(cpu_limit_secs(APPS_MAX_TIMEOUT_SECS).to_string())
        .arg(nproc_limit().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return SetrlimitSupport {
                rlimit_as: SetrlimitProbe::ProbeError(format!("探测子进程 spawn 失败: {e}")),
                rlimit_cpu: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
                rlimit_nproc: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
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
                    rlimit_nproc: SetrlimitProbe::ProbeError(
                        "同上（探测子进程未运行）".to_string(),
                    ),
                };
            }
        }
    };
    let Some(status) = status else {
        let msg = "探测子进程超时（10s）已强杀".to_string();
        return SetrlimitSupport {
            rlimit_as: SetrlimitProbe::ProbeError(msg.clone()),
            rlimit_cpu: SetrlimitProbe::ProbeError(msg),
            rlimit_nproc: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
        };
    };
    if !status.success() {
        let msg = format!("探测子进程退出码异常: {status}");
        return SetrlimitSupport {
            rlimit_as: SetrlimitProbe::ProbeError(msg.clone()),
            rlimit_cpu: SetrlimitProbe::ProbeError(msg),
            rlimit_nproc: SetrlimitProbe::ProbeError("同上（探测子进程未运行）".to_string()),
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

/// 失败后给模型的提示：脚本还在哪、怎么改。
///
/// 「脚本没跑成」最贵的代价是模型把整段代码重写一遍（每次重写都重付一遍全文
/// 的输出 token）。留住脚本并给出路径，模型就能读回来只改出错的那几行。
fn retained_note(dir: &std::path::Path) -> String {
    format!(
        "（脚本已保留在 {}/{}，可 read_text_file 读回后只改出错的那几行，再用 run_python 的 scriptPath 重跑；报错行号与该文件一致）",
        dir.display(),
        SCRIPT_FILE
    )
}

#[allow(clippy::too_many_lines)]
pub fn run_python_at(
    py: &str,
    entry: Option<&str>,
    dir: &std::path::Path,
    gen_dir: Option<&std::path::Path>,
    args: &[String],
    timeout_secs: Option<u64>,
    timeout_cap: u64,
    audit: &mut dyn FnMut(&str),
    stop: Option<&StopToken>,
) -> Result<PyRunResult, RunFail> {
    let (timeout_eff, clamped) = resolve_timeout_with_cap(timeout_secs, timeout_cap);
    if clamped {
        audit(&format!(
            "run_python | timeout clamped | requested={} cap={timeout_cap}",
            timeout_secs.unwrap_or(0)
        ));
    }
    let timeout = Duration::from_secs(timeout_eff);
    let mem_bytes = mem_limit_bytes(timeout_eff);
    let cpu_secs = cpu_limit_secs(timeout_eff);
    let limits = RunLimits::new(mem_bytes, cpu_secs);

    let mut cmd = {
        #[cfg(target_os = "macos")]
        {
            // seatbelt 包装把 python 变成 sandbox-exec 的参数——python 路径坏时
            // spawn 恒成功（退出码 71 走脚本失败路径），NotFound 语义被吞。
            // 包装前对**含路径分隔符**的 py 做 metadata 校验（裸名走 PATH 解析，
            // 与未包装时语义一致，不能拿 metadata 误杀）；保住 spawn_not_found
            // 的缓存重试语义。
            if py.contains('/') {
                if let Err(e) = std::fs::metadata(py) {
                    audit(&format!(
                        "run_python err | kind=spawn_fail | {}",
                        crate::audit::escape_for_log(&e.to_string(), 200)
                    ));
                    cleanup_after_spawn_fail(dir, &limits);
                    return Err(RunFail {
                        kind: PyFailKind::StartFailed,
                        msg: format!("启动 Python 失败：{e}"),
                        spawn_not_found: e.kind() == std::io::ErrorKind::NotFound,
                    });
                }
            }
            // seatbelt 出网硬闸（fail-closed）：闸失效拒绝执行，无放行开关。
            // apps 与 run_python 走同一组装函数，闸自动同享。
            if let Err(why) = crate::py::seatbelt::seatbelt_probe_cached(audit) {
                audit(&format!(
                    "run_python err | kind=seatbelt_unavailable | {why}"
                ));
                cleanup_after_spawn_fail(dir, &limits);
                return Err(RunFail {
                    kind: PyFailKind::StartFailed,
                    msg: format!(
                        "Python 出网闸（macOS 沙箱）未生效，已拒绝执行：{why}；这是安全失败（fail-closed），请反馈开发者"
                    ),
                    spawn_not_found: false,
                });
            }
            let mut c = silent_cmd("sandbox-exec");
            c.arg("-p")
                .arg(crate::py::seatbelt::SEATBELT_PROFILE)
                .arg(py);
            c
        }
        #[cfg(not(target_os = "macos"))]
        {
            silent_cmd(py)
        }
    };
    apply_child_env(&mut cmd);
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
        let nproc_ok = support.rlimit_nproc == SetrlimitProbe::Available;
        let nproc = nproc_limit();
        if as_ok || cpu_ok || nproc_ok {
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
                    if nproc_ok {
                        let np = libc::rlimit {
                            rlim_cur: nproc as libc::rlim_t,
                            rlim_max: nproc as libc::rlim_t,
                        };
                        libc::setrlimit(libc::RLIMIT_NPROC, &np);
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
    // TEMP/TMP/TMPDIR 指到本次运行的目录（子进程 cwd 就是它）：脚本 tempfile
    // 的临时文件不再落共享系统 /tmp——多用户机器上 params.json / 中间产物可被
    // 同机其他用户读；运行目录由超龄清扫统一回收，不残留。
    // WM_TMP_DIR 仍指系统临时目录：提示词拿它告诉模型「临时文件放哪」，模型
    // 可见的合法落点不该悄悄变成随时会被清扫的 run 目录。
    for tmp_key in ["TEMP", "TMP", "TMPDIR"] {
        cmd.env(tmp_key, dir);
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
                kind: PyFailKind::StartFailed,
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
                cleanup_after_fail(&mut child, &limits);
                audit(&format!("run_python err | kind=wait_fail | {e}"));
                crate::py::io::join_reader_threads(out_handle, err_handle, audit);
                return Err(RunFail {
                    kind: PyFailKind::Runtime,
                    msg: format!("等待子进程状态失败：{e}{}", retained_note(dir)),
                    spawn_not_found: false,
                });
            }
        }
        if start.elapsed() > timeout {
            cleanup_after_fail(&mut child, &limits);
            audit(&format!(
                "run_python err | kind=timeout | timeout_secs={}",
                timeout.as_secs()
            ));
            crate::py::io::join_reader_threads(out_handle, err_handle, audit);
            return Err(RunFail {
                kind: PyFailKind::Timeout,
                msg: format!(
                    "执行超时（{}s）已强制终止{}",
                    timeout.as_secs(),
                    retained_note(dir)
                ),
                spawn_not_found: false,
            });
        }
        if stop.is_some_and(|s| s.stopped()) {
            cleanup_after_fail(&mut child, &limits);
            audit("run_python err | kind=stopped");
            crate::py::io::join_reader_threads(out_handle, err_handle, audit);
            return Err(RunFail {
                kind: PyFailKind::Stopped,
                msg: format!("已停止{}", retained_note(dir)),
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
    let harvested = match gen_dir {
        Some(g) => crate::py::harvest::harvest_run_outputs(dir, g, audit),
        None => Vec::new(),
    };
    let Some(code) = exit_code else {
        // 无退出码 = 被信号杀死：脚本跑过了（可能跑了一半），目录留着给模型读回
        audit("run_python err | kind=exit_none | 无退出码，子进程疑似被信号杀死");
        let _ = finish_run_dir(dir, false, audit);
        return Err(RunFail {
            kind: PyFailKind::Runtime,
            msg: format!(
                "子进程异常终止（被信号杀死，无退出码）{}",
                retained_note(dir)
            ),
            spawn_not_found: false,
        });
    };
    // 退出码 0 = 脚本跑成了：目录连同产物一起清掉。
    // 非 0 = 脚本跑了但没跑成：目录留着（脚本在里面），模型读回改几行再跑。
    let retained_dir = finish_run_dir(dir, code == 0, audit);
    Ok(PyRunResult {
        stdout: crate::py::io::truncate_output(stdout),
        stderr: crate::py::io::truncate_output(stderr),
        exit_code: Some(code),
        duration_ms,
        truncated,
        retained_dir,
        harvested,
    })
}

/// 跑完后的目录收尾：跑成了才清；没跑成留着（脚本在里面，模型要能读回）。
///
/// 返回留存目录路径（跑成时 None）。
fn finish_run_dir(
    dir: &std::path::Path,
    succeeded: bool,
    audit: &mut dyn FnMut(&str),
) -> Option<String> {
    if succeeded {
        let _ = std::fs::remove_dir_all(dir);
        return None;
    }
    prune_stale_run_dirs(audit);
    Some(dir.to_string_lossy().to_string())
}

/// 留存运行目录后顺手清一遍超龄的（保留期 1h，与启动时清扫同一口径）。
///
/// 只在启动时清扫的话，长会话里一次次失败会把临时目录堆起来；每次留存时
/// 顺带扫一遍，堆积就被压在一小时窗口内。
fn prune_stale_run_dirs(audit: &mut dyn FnMut(&str)) {
    let removed = crate::py::harvest::sweep_stale_py_runs_in(
        &py_runs_root(),
        Duration::from_secs(3600),
        std::time::SystemTime::now(),
    );
    if removed > 0 {
        audit(&format!("py_runs prune | removed={removed}"));
    }
}

/// 安全边界测试：许可闸 / 子进程登记 / 进程组判定 / kill_tree 收割。
///
/// 这四处都在「出事了会误伤系统或留下孤儿进程」的位置上，而正文全是平台相关
/// 的 spawn / signal 调用，靠 e2e 很难稳定复现失败路径，所以对可判定的部分
/// 直接断言。
#[cfg(test)]
mod tests {
    use super::*;

    /// 子进程不继承宿主全量环境：宿主进程里放一个伪造密钥，脚本打印自己的
    /// 环境里是否可见——白名单裁剪后必须不可见；PATH 这类地基变量仍在。
    #[test]
    fn py_child_env_does_not_inherit_host_secrets() {
        let Some(py) = crate::py::env::cached_python().or_else(crate::py::env::detect_python)
        else {
            return; // 本机无 python：跑不了真脚本，跳过
        };
        const CANARY: &str = "WM_HOST_SECRET_CANARY";
        std::env::set_var(CANARY, "leak-me");
        let script = format!(
            "import os\nprint('LEAK' if '{CANARY}' in os.environ else 'CLEAN', bool(os.environ.get('PATH')))"
        );
        let tmp = tempfile::tempdir().expect("临时目录");
        let dir = setup_run_dir(tmp.path(), &script, None).expect("运行目录应能建");
        let mut audit = |_line: &str| {};
        let r = run_python_at(
            &py,
            Some("run.py"),
            &dir,
            None,
            &[],
            Some(20),
            MAX_TIMEOUT_SECS,
            &mut audit,
            None,
        )
        .expect("脚本应正常跑完");
        std::env::remove_var(CANARY);
        let out = r.stdout.trim().to_string();
        assert_eq!(
            out, "CLEAN True",
            "宿主密钥不得进子进程环境，PATH 等地基变量必须还在：{out:?}"
        );
    }

    /// 生产上限 → 资源限额：apps 天花板 1800s 时内存封顶 2GB、CPU = 上限 + 10。
    /// 这两个数是探针与 pre_exec 设限的依据，改天花板必须同步复核。
    #[test]
    fn production_ceiling_maps_to_resource_limits() {
        assert_eq!(
            mem_limit_bytes(APPS_MAX_TIMEOUT_SECS),
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(
            cpu_limit_secs(APPS_MAX_TIMEOUT_SECS),
            APPS_MAX_TIMEOUT_SECS + 10
        );
    }

    /// 探针实跑：脚本从 argv 取生产上限值——改成 argv 传参后必须仍能跑出可解析结论
    /// （argv 索引写错 → stdout 缺标记 → ProbeError，这里会露馅）。
    #[cfg(unix)]
    #[test]
    fn setrlimit_probe_runs_with_argv_limits() {
        let Some(py) = crate::py::env::cached_python().or_else(crate::py::env::detect_python)
        else {
            // 本机无可用 python3：探针无从跑起，跳过（不是失败）
            return;
        };
        let support = run_setrlimit_probe(&py);
        assert!(
            !matches!(support.rlimit_as, SetrlimitProbe::ProbeError(_)),
            "AS 探测不该是「探测失败」：{support:?}"
        );
        assert!(
            !matches!(support.rlimit_cpu, SetrlimitProbe::ProbeError(_)),
            "CPU 探测不该是「探测失败」：{support:?}"
        );
    }

    /// 失败种类必须与实际发生的事对上：超时就是 Timeout，不能混成「启动失败」。
    /// 靠这个，调用方（程序页运行记录）才能如实上报而不是一律说启动失败。
    #[test]
    fn timeout_failure_is_tagged_as_timeout() {
        let Some(py) = crate::py::env::cached_python().or_else(crate::py::env::detect_python)
        else {
            // 本机无可用 python3：跑不了真脚本，跳过（不是失败）
            return;
        };
        let tmp = tempfile::tempdir().expect("临时目录");
        let dir = setup_run_dir(tmp.path(), "import time\ntime.sleep(30)\n", None)
            .expect("运行目录应能建");
        let mut audit = |_line: &str| {};
        let err = run_python_at(
            &py,
            Some("run.py"),
            &dir,
            None,
            &[],
            Some(1),
            MAX_TIMEOUT_SECS,
            &mut audit,
            None,
        )
        .err()
        .expect("睡 30 秒而只给 1 秒必须失败");
        assert_eq!(err.kind, PyFailKind::Timeout, "超时必须是 Timeout 种类");
        assert!(err.msg.contains("超时"), "文案要说明是超时：{}", err.msg);
    }

    /// 许可归还：Drop 后可用许可数必须回到原值，否则闸会单向漏光，
    /// 跑着跑着所有 Python 任务一起挂死。
    #[test]
    fn gate_guard_returns_permit_on_drop() {
        // 许可池是进程级全局：并行跑会互抢（断言漂移 / 读到 0 再减一的下溢 panic）
        let _serial = gate_test_lock();
        let before = *crate::db::lock::lock_or_recover(&PY_PERMITS, "test");
        {
            let _g = py_gate_acquire_for_test();
            assert_eq!(
                *crate::db::lock::lock_or_recover(&PY_PERMITS, "test"),
                before - 1,
                "持有期间应占掉一个许可"
            );
        }
        assert_eq!(
            *crate::db::lock::lock_or_recover(&PY_PERMITS, "test"),
            before,
            "Drop 后许可必须归还"
        );
    }

    /// 有界并发：许可发完就不再发，拿到最后一个之后计数必须为 0。
    /// 这个断言锁住「许可数恒等于并发上限」——串行化过一次（std Mutex），
    /// 也可能不小心被改成无限发。
    #[test]
    fn gate_permits_are_bounded_and_exhaustible() {
        let _serial = gate_test_lock();
        let start = *crate::db::lock::lock_or_recover(&PY_PERMITS, "test");
        let mut held = Vec::new();
        for _ in 0..start {
            held.push(py_gate_acquire_for_test());
        }
        assert_eq!(
            *crate::db::lock::lock_or_recover(&PY_PERMITS, "test"),
            0,
            "许可发完后必须为 0（PY_RUN_MAX_CONCURRENT={PY_RUN_MAX_CONCURRENT}）"
        );
        drop(held);
        assert_eq!(
            *crate::db::lock::lock_or_recover(&PY_PERMITS, "test"),
            start,
            "全部归还后回到起始值"
        );
    }

    /// EXITING 置位后拒绝发许可，且**不**占用名额——否则每次退出都会把闸漏光。
    #[test]
    fn gate_refuses_when_exiting_without_leaking_permit() {
        let _serial = gate_test_lock();
        reset_exiting_for_test();
        let start = *crate::db::lock::lock_or_recover(&PY_PERMITS, "test");
        mark_exiting();
        let r = py_gate_acquire();
        reset_exiting_for_test();
        let err = match r {
            Ok(_) => panic!("EXITING 时不该发许可"),
            Err(e) => e,
        };
        assert!(matches!(err, CommandError::DomainRule { .. }), "{err:?}");
        assert_eq!(
            *crate::db::lock::lock_or_recover(&PY_PERMITS, "test"),
            start,
            "被拒的申请不得改变许可数"
        );
    }

    /// 子进程登记生命周期：register 进表、Drop 出表。
    /// 漏掉 Drop 会让 kill_all_py_children 对着已回收的 pid 反复 kill；
    /// 漏掉 register 则崩溃时收割不到任何进程。
    #[test]
    fn child_reg_guard_tracks_lifecycle() {
        let mut child = silent_cmd("sleep").arg("30").spawn().expect("应能起 sleep");
        let pid = child.id();
        {
            let _guard = ChildRegGuard::register(&child);
            assert!(
                crate::db::lock::lock_or_recover(py_children(), "test").contains(&pid),
                "register 后应在表内"
            );
        }
        assert!(
            !crate::db::lock::lock_or_recover(py_children(), "test").contains(&pid),
            "guard 析构后应出表"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// kill_tree 必须真正收割子进程，且调用后 child 已 wait 过（不留僵尸）。
    #[test]
    fn kill_tree_reaps_the_child() {
        let mut child = silent_cmd("sleep").arg("30").spawn().expect("应能起 sleep");
        // pid 必须在 kill 之前取（kill 后 pid 可能已被回收）；unix 专属断言才用它
        #[cfg(unix)]
        let pid = child.id();
        kill_tree(&mut child, &RunLimits::new(64 * 1024 * 1024, 10));
        assert!(
            !child
                .try_wait()
                .expect("wait 应可用")
                .is_some_and(|s| s.success()),
            "kill_tree 后子进程不该是成功退出状态"
        );
        // 进程确实不在了：unix 下 kill -0 对已回收 pid 返 ESRCH
        #[cfg(unix)]
        {
            let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
            assert!(!alive, "pid {pid} 应已不存在");
        }
    }

    /// 进程组判定：越界 pid 必须直接拒判。getpgid 失败返回 -1，
    /// 而 i32::MAX 回绕也可能是 -1，等值比较会误判成「存活 leader」，
    /// 随后 kill_py_children 会执行 `kill -9 --1`——那会打到全系统所有进程。
    #[cfg(unix)]
    #[test]
    fn oversized_pid_never_counts_as_group_leader() {
        assert!(!is_live_group_leader(i32::MAX as u32 + 1));
        assert!(!is_live_group_leader(u32::MAX));
        // 常规范围内的死 pid 也必须判 false（getpgid 返 -1 ≠ pid）
        assert!(!is_live_group_leader(0));
    }

    /// kill_py_children 对非组长 / 不存在的 pid 一律不计数——
    /// 计数只反映「确实杀掉了几棵进程树」。
    #[cfg(unix)]
    #[test]
    fn kill_py_children_counts_only_live_group_leaders() {
        assert_eq!(
            kill_py_children(&[0, i32::MAX as u32 + 1]),
            0,
            "非组长/越界 pid 不应计入击杀数"
        );
        assert_eq!(kill_py_children(&[]), 0, "空列表应返 0");
    }

    /// 组存活判定的护栏：pid 0 与超 i32 上限的 pid 一律判「不在」。
    ///
    /// pid 0 在 kill 语义里是「调用方自己的进程组」；超上限的 pid 转 i32
    /// 会回绕成负数 → `kill -9 --1` 打到全系统所有进程。
    #[cfg(unix)]
    #[test]
    fn process_group_alive_rejects_dangerous_pids() {
        assert!(
            !process_group_alive(0),
            "pid 0 = 调用方自己的组，不能当目标"
        );
        assert!(
            !process_group_alive(i32::MAX as u32 + 1),
            "超 i32 上限（回绕成 --1）必须拒判"
        );
        assert!(!process_group_alive(u32::MAX));
    }

    /// 本进程所在的组必然还在——判据不能把「自己人」也拒掉。
    #[cfg(unix)]
    #[test]
    fn process_group_alive_accepts_the_group_we_are_in() {
        // SAFETY: getpgrp 无参数、无内存影响。
        let pgid = unsafe { libc::getpgrp() };
        assert!(pgid > 0);
        assert!(
            process_group_alive(pgid as u32),
            "本进程所在组应判为在（否则孙进程占管道那条路径再也杀不掉组）"
        );
    }

    /// 接线守卫：kill_tree 的进程组杀必须先过护栏。
    ///
    /// 少了护栏就是 `kill -9 -{pid}` 无条件执行——pid 转 i32 回绕即
    /// `kill -9 --1`，杀光本用户所有进程。纯函数护栏测不到调用点，
    /// 故扫源码钉住。
    #[cfg(unix)]
    #[test]
    fn kill_tree_checks_the_group_before_killing_it() {
        let src =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/py/runtime.rs"))
                .expect("应能读到自己");
        let start = src.find("pub fn kill_tree").expect("应有 kill_tree");
        let rest = &src[start..];
        let end = rest[1..]
            .find("\npub fn ")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        let func = &rest[..end];
        assert!(
            func.contains("process_group_alive"),
            "进程组杀前必须过护栏（pid 0 / 回绕 / 组已不在）"
        );
        assert!(
            !func.contains("as i32"),
            "pid 不得直接转 i32（回绕成负数即 kill -9 --1）"
        );
    }
}
