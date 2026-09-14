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
    if enabled {
        std::fs::write(py_flag_path(&app), b"1")
            .map_err(|e| CommandError::IoError(e.to_string()))?;
    } else {
        let _ = std::fs::remove_file(py_flag_path(&app));
    }
    Ok(enabled)
}

#[tauri::command]
pub async fn py_env_check() -> PyEnv {
    tauri::async_runtime::spawn_blocking(py_env_check_blocking)
        .await
        .unwrap_or_else(|_| PyEnv {
            available: false,
            python: String::new(),
            version: String::new(),
            libs: Vec::new(),
        })
}
