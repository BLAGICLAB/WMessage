//! 平台差异封装层
//!
//! 把 `#[cfg(target_os = ...)]` 三段块从 `lib.rs` 抽出来，按平台 / 能力分文件，
//! 顶层只暴露 `pub use` 的稳定接口。
//!
//! 当前子模块：
//! - `copy_file` — 跨平台「复制文件 + 任务标题到剪贴板」helper
//!   （macOS 走 NSPasteboard 三类粘贴板，Windows 走 CF_HDROP + CF_UNICODETEXT）

pub mod copy_file;
