//! tauri command 入口（py_get_enabled / py_set_enabled / py_env_check）

use std::path::PathBuf;

use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};
use crate::py::env::{py_env_check_blocking, PyEnv};

fn py_flag_path(app: &AppHandle) -> PathBuf {
    crate::paths::flags_dir(app).join("py-enabled.flag")
}

#[tauri::command]
pub fn py_get_enabled(app: AppHandle) -> bool {
    py_flag_path(&app).exists()
}

#[tauri::command]
pub fn py_set_enabled(app: AppHandle, enabled: bool) -> CommandResult<bool> {
    let dir = crate::paths::flags_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let path = py_flag_path(&app);
    let before = path.exists();
    if enabled {
        // 0600 与 UI 自动化 / 服务两道开关同款：0644 时同机其它进程能创建
        // 这个文件把「允许跑 Python」顶开
        crate::ui::write_flag_file(&path).map_err(|e| CommandError::IoError(e.to_string()))?;
    } else {
        // 只吞 NotFound；权限/占用等失败必须上抛——否则盘上残留 flag，py_get_enabled
        // 仍返回 true，与刚返回给调用方的「已禁用」应答自相矛盾
        match std::fs::remove_file(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(CommandError::IoError(format!(
                    "删除 py 启用标志文件失败：{e}"
                )))
            }
        }
    }
    let after = path.exists();
    if before != after {
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "py.enable_toggle",
            &[
                ("enabled", after.to_string()),
                ("user", crate::ui::os_user()),
            ],
        );
    }
    Ok(enabled)
}

#[tauri::command]
pub async fn py_env_check() -> PyEnv {
    tauri::async_runtime::spawn_blocking(py_env_check_blocking)
        .await
        .unwrap_or_else(|e| {
            // Join 失败（检测任务 panic）按未安装降级，但必须留痕——
            // 否则与「Python 真不存在」无法区分
            eprintln!("[py] py_env_check 检测线程失败（按未安装降级）：{e}");
            PyEnv {
                available: false,
                python: String::new(),
                version: String::new(),
                libs: Vec::new(),
            }
        })
}
