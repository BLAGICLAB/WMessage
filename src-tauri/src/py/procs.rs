//! py 执行的进程树管理族（自 runtime.rs 按节拆出）：silent_cmd / 进程组存活判定
//! / kill_tree / 失败清理 / 子进程登记（PY_CHILDREN + ChildRegGuard）。
//! RunLimits 留在 runtime；kill 路径按 `super::runtime::RunLimits` 反向引用。

use std::collections::HashSet;
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};

use super::runtime::RunLimits;

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
