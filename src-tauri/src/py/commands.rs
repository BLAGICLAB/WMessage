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

/// Python 执行面统一闸（run_python 工具与程序页 apps 共用）：
/// 「允许机器人执行 Python」关着且非 yolo → Err（用户到设置页打开即可重试）；
/// yolo 绕过时留审计——同一执行面同一口径，关掉开关不能只挡工具不挡程序页。
pub fn ensure_py_enabled(app: &AppHandle) -> Result<(), String> {
    let yolo = crate::bot::perm_mode(app) == crate::bot::PermMode::Yolo;
    let flag_on = py_get_enabled(app.clone());
    if !yolo && !flag_on {
        return Err(
            "Python 编程未开启：请到设置页「机器人设置」打开「允许机器人执行 Python」（或将授权模式切为 yolo）"
                .into(),
        );
    }
    if yolo && !flag_on {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "py.yolo_bypass",
            "detail" => "授权模式 yolo，跳过 py-enabled 开关检查",
        );
    }
    Ok(())
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
