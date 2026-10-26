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
}

pub const OUTPUT_CAP: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// 全局天花板：任何一次 Python 执行（run_python / 程序）都不得超过这个秒数。
/// 取值 1800（30 分钟）——上限存在的意义是兜住「跑飞」与「模型乱填」，
/// 不是拿来卡正常活：耗时与输入规模成正比的批量活（语音转写、装依赖、
/// 长文档处理）在 300s 这个量级上是被硬杀的，不是被「保护」住的。
pub const MAX_TIMEOUT_SECS: u64 = 1800;

/// 解析超时：None → 60s 默认；> MAX_TIMEOUT_SECS → 钳到上限并返回 clamped=true
pub fn resolve_timeout(timeout_secs: Option<u64>) -> (u64, bool) {
    match timeout_secs {
        None => (DEFAULT_TIMEOUT_SECS, false),
        Some(s) if s > MAX_TIMEOUT_SECS => (MAX_TIMEOUT_SECS, true),
        Some(s) => (s, false),
    }
}

/// 写侧钳制（1..=MAX_TIMEOUT_SECS）——设置页落盘前用。
///
/// 运行期钳制在 `resolve_timeout`，但那条只在**执行时**生效：设置页写下去的是
/// 原值（10000），页面显示 10000、实际跑 300——用户看到的和生效的不是一回事。
/// 写侧用同一个上界钳一遍，落盘值 = 生效值。
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

/// Python 并发闸从互斥锁改**有界并发**（信号量语义）——std Mutex 全程
/// 持锁把所有 Python 任务串行化，一个长任务能把后面全部排队堵死。许可数 2：
/// 并行 run 各自独立子进程 + 独立产物目录，2 是 CPU/内存争用的保守上界。
pub const PY_RUN_MAX_CONCURRENT: usize = 2;

/// 可用许可数（Condvar 信号量：run_python 跑在 spawn_blocking 阻塞线程上，
/// 保持同步 API；等待带 200ms 超时轮询以便复查 EXITING）
static PY_PERMITS: Mutex<usize> = Mutex::new(PY_RUN_MAX_CONCURRENT);
static PY_PERMITS_CV: std::sync::Condvar = std::sync::Condvar::new();

/// 闸门许可句柄（RAII：Drop 归还许可并唤醒一个等待者）。
/// acquired=false = 因应用退出拒发（不是真持有），Drop 无副作用。
pub struct PyGateGuard {
    acquired: bool,
}

impl Drop for PyGateGuard {
    fn drop(&mut self) {
        if self.acquired {
            let mut avail =
                crate::db::lock::lock_or_recover(&PY_PERMITS, "py::runtime::PY_PERMITS");
            *avail += 1;
            drop(avail);
            PY_PERMITS_CV.notify_one();
        }
    }
}

/// 取一个 Python 执行许可（阻塞至有空位）。EXITING 三点复查（原「拿锁后复查」
/// 语义平移）：等待前、每轮唤醒后（循环顶）、拿到许可后终检——排队者过闸即拒。
/// pub(crate)：py/document 的 doc 生成流持一个许可跨 dotnet+python 兜底
///（内部 run_python_ungated 不重复计闸）。
pub(crate) fn py_gate_acquire() -> Result<PyGateGuard, CommandError> {
    py_gate_acquire_inner(true)
}

fn py_gate_acquire_inner(check_exiting: bool) -> Result<PyGateGuard, CommandError> {
    let mut avail = crate::db::lock::lock_or_recover(&PY_PERMITS, "py::runtime::PY_PERMITS");
    loop {
        // EXITING 复查①：本轮循环顶（= 等待前 / 每轮唤醒后）
        if check_exiting && EXITING.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CommandError::DomainRule {
                domain: "python".to_string(),
                reason: "应用正在退出，不再启动新的 Python 任务".to_string(),
            });
        }
        if *avail > 0 {
            // EXITING 复查②（拿到许可后终检）：等待期间退出标志可能置位，
            // 置位即归还许可并拒绝——不 spawn 无人收割的孤儿进程
            if check_exiting && EXITING.load(std::sync::atomic::Ordering::SeqCst) {
                *avail += 1;
                drop(avail);
                PY_PERMITS_CV.notify_one();
                return Err(CommandError::DomainRule {
                    domain: "python".to_string(),
                    reason: "应用正在退出，不再启动新的 Python 任务".to_string(),
                });
            }
            *avail -= 1;
            return Ok(PyGateGuard { acquired: true });
        }
        let (g, _timeout) = PY_PERMITS_CV
            .wait_timeout(avail, std::time::Duration::from_millis(200))
            .unwrap_or_else(|e| {
                eprintln!("[mutex_poisoned] py::runtime::PY_PERMITS_CV: {e:?}");
                e.into_inner()
            });
        avail = g;
    }
}

/// 测试入口：同一条许可获取路径、不查 EXITING（并行测试里其他用例可能置位）。
/// 阻塞式取许可（非 try 语义），名字按评审 M 采纳与行为对齐。
#[cfg(test)]
pub fn py_gate_acquire_for_test() -> PyGateGuard {
    py_gate_acquire_inner(false).expect("非退出态取许可必须成功")
}

/// 闸门测试的串行锁。
///
/// 许可池（`PY_PERMITS`）与 `EXITING` 标志是**进程级全局**的，而 Rust 测试默认
/// 并行跑：两个用例各拿许可会互相看见「只剩 1 个」（断言漂移），一个用例读到的
/// 许可数被另一个改掉，读到 0 再去减一就直接下溢 panic。碰这两样全局的用例都
/// 要先拿这把锁——锁本身是测试设施，中毒后照常取用（见 `gate_test_lock`）。
#[cfg(test)]
pub(crate) static GATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 取闸门测试锁。守卫活到用例结束即自动释放。
#[cfg(test)]
pub(crate) fn gate_test_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::db::lock::lock_or_recover(&GATE_TEST_LOCK, "py::runtime::GATE_TEST_LOCK")
}

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
    /// 跑起来了，但被停止（用户 /stop、应用退出、子 agent 墙钟耗尽）
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
    stop: Option<&StopToken>,
) -> Result<PyRunResult, PyRunError> {
    // 有界并发闸（许可 2）。EXITING 复查在闸门内完成（等待前/唤醒后/
    // 拿到许可后），排队者过闸即拒——不会 spawn 出无人收割的孤儿进程
    let _gate = py_gate_acquire().map_err(|e| PyRunError {
        kind: PyFailKind::StartFailed,
        msg: e.message(),
    })?;
    run_python_ungated(app, script, input_json, args, timeout_secs, stop)
}

pub fn run_python_ungated(
    app: &AppHandle,
    script: &str,
    input_json: Option<&str>,
    args: &[String],
    timeout_secs: Option<u64>,
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

#[cfg(unix)]
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
        // 探测值来自生产常量（见 SETRLIMIT_PROBE_SCRIPT 注），脚本自己只读 argv
        .arg(mem_limit_bytes(MAX_TIMEOUT_SECS).to_string())
        .arg(cpu_limit_secs(MAX_TIMEOUT_SECS).to_string())
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
    if let Some(g) = gen_dir {
        crate::py::harvest::harvest_run_outputs(dir, g, audit);
    }
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

    /// 生产上限 → 资源限额：天花板 1800s 时内存封顶 2GB、CPU 限额 = 上限 + 10 宽限。
    /// 这两个数是 setrlimit 探针与 pre_exec 设限的依据，改天花板必须同步复核。
    #[test]
    fn production_ceiling_maps_to_resource_limits() {
        assert_eq!(mem_limit_bytes(MAX_TIMEOUT_SECS), 2 * 1024 * 1024 * 1024);
        assert_eq!(cpu_limit_secs(MAX_TIMEOUT_SECS), MAX_TIMEOUT_SECS + 10);
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
