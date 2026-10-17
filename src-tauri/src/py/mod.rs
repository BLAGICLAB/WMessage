//! `bot_py` 切片层
//!
//! 原 `bot_py.rs`（3694 行）按 SRP 拆为 8 个子模块：
//! - `env` — Python/dotnet 环境探测 + 缓存
//! - `runtime` — 子进程执行核心（RunLimits / kill_tree / ChildRegGuard / run_python / silent_cmd）
//! - `io` — 有界 IO 读取 + 线程 join + 截断
//! - `harvest` — 运行目录产物回收 + 旧 run 清扫
//! - `audit` — 结构化审计日志
//! - `document` — Word/Excel/PPT/PDF 文档生成 + py_exec_sync
//! - `ast` — Python AST 静态探查（ast_inspect 工具内核：授权闸在 Rust 侧，符号解析在沙箱侧）
//! - `commands` — 3 个 tauri command 入口
//!
//! **路径兼容**：`bot_py.rs` facade 用 `pub use py::*;` 重导出所有 `pub` 项，
//! `crate::bot_py::X` 仍可用（lib.rs / tools.rs / 测试模块调用无修改）。
//!
//! **安全纪律**：kill_tree / ChildRegGuard / RunLimits 逻辑一字不动；
//! `git diff --cached --color-moved` 应识别为移动而非修改。

pub mod ast;
pub mod audit;
pub mod commands;
pub mod document;
pub mod env;
pub mod harvest;
pub mod io;
pub mod runtime;
