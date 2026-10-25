//! Python 执行基础设施（facade）
//!
//! 原 3694 行 bot_py.rs 按 SRP 切片到 `py/` 子模块（env / runtime /
//! io / harvest / audit / document / commands）。本文件保留为 facade：
//! `pub use py::*` 重导出所有 `pub` 项，`crate::bot_py::X` 路径全部兼容（lib.rs /
//! tools.rs / 测试模块调用无修改）。
//!
//! **测试模块已搬到 `bot_py/` 子文件**（`tests` / `exiting_tests` / `platform_tests`）：
//! 模块路径不变（`bot_py::tests` 等），`super::X` 仍经 pub use 解析为 `py::X`，
//! 测试内容无需改动。
//! **4 个回归锁除外**（读源字符串的），已改读 /src/py/ 对应子模块（runtime.rs / env.rs）。

// 显式 re-export 每个子模块的 pub 项（glob `pub use crate::py::*` 只展开
// py 的顶层项即子模块声明本身，不递归 re-export 子模块内部 pub 项）
// audit/env/io 三行「看似未用」是有意的 facade：测试模块经 super::X 解析
// （教训：cargo fix 会误删它们，勿让自动 fix 碰本文件 use 区；
// allow 显式压制警告，同时保住 re-export）
#[allow(unused_imports)]
pub use crate::py::audit::*;
pub use crate::py::commands::*;
pub use crate::py::document::*;
#[allow(unused_imports)]
pub use crate::py::env::*;
pub use crate::py::harvest::*;
#[allow(unused_imports)]
pub use crate::py::io::*;
pub use crate::py::runtime::*;
#[cfg(test)]
fn guard_test_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    tauri::Manager::manage(&app, crate::app_state::AppState::default());
    app.handle().clone()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod exiting_tests;

#[cfg(test)]
mod platform_tests;
