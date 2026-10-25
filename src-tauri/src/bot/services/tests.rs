//! 受管服务层的纯函数与集成测试。
//!
//! 进程树相关的纯函数（快照解析 / ppid 树收集 / PID 复用识别 / 整组杀护栏）
//! 搬到了 `bot/procs`（受管服务层与 MCP stdio 共用），那边的测试在
//! `bot/procs/tests.rs`。这里留下的是服务层自己的：登记表、状态机、
//! 日志截断、资源告警判定，以及真进程的集成用例。
//! 按主题拆到 `tests/` 子文件（每个子文件一个主题面），这里只留共用辅助与 mod 声明。

use super::*;
use crate::bot::procs::pid_exists;

use tauri::Manager;

#[cfg(test)]
mod adjudicate;
#[cfg(test)]
mod canary;
#[cfg(test)]
mod integration;
#[cfg(test)]
mod logs;
#[cfg(test)]
mod observe;
#[cfg(test)]
mod port_gate;
#[cfg(test)]
mod state;

// ── 共用辅助 ──
/// 找一个能长驻的启动器做集成测试。
///
/// 用白名单里的**裸名**（`python3`），不是绝对路径——白名单只认裸名，
/// 绝对路径的第三方二进制是刻意不放行的。
fn long_running_launcher() -> Option<(String, Vec<String>)> {
    if which("python3").is_none() && which("python").is_none() {
        return None;
    }
    let cmd = if which("python3").is_some() {
        "python3"
    } else {
        "python"
    };
    Some((
        cmd.to_string(),
        vec![
            "-c".to_string(),
            "import time\nwhile True: time.sleep(1)\n".to_string(),
        ],
    ))
}

/// 找可执行文件（不新增依赖：走 PATH 手工解析）。
fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}
