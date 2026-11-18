//! Windows Job Object 限额（从 runtime.rs 拆出，实现逐字未改）。
//!
//! 为什么独立成文件：这段是 `#[cfg(windows)]` 专属实现，unix 构建根本不编译
//! 它——留在 runtime.rs 里等于让 unix 侧的主文件白扛这百行（runtime.rs 已在
//! 1500 行门禁线下沿）。平台分界天然清晰，拆开后两边都好读。

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
