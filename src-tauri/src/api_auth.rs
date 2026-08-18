//! Token 管理 / 鉴权 / 开关持久化
//!
//! 完整模块组：
//! - `api_server`   : 服务生命周期 / 端口绑定 / EventHub
//! - `api_auth`     : 此文件 — token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : endpoint handlers + tauri commands
//! - `api`          : 数据层 `TaskStore` + `MemStore` + `TauriStore`
//!
//! 持久化文件：
//! - `api-token.txt`     — Bearer token（`load_or_create_token` 读写）
//! - `api-enabled.flag`  — 开关标志（`write_enabled_flag` / `clear_enabled_flag`）
//! - 启动时 `should_autostart` 检 flag 决定是否自动恢复 API

use std::path::PathBuf;

use tauri::AppHandle;
use tiny_http::Request;

use crate::db;

/// 读取或生成 Bearer token（存在数据目录 `api-token.txt`，随便携库走）。
/// 已存在且非空就直接返回；否则生成 UUID 写回。
pub fn load_or_create_token(app: &AppHandle) -> Result<String, String> {
    let dir = db::data_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("api-token.txt");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(&path, &token).map_err(|e| e.to_string())?;
    Ok(token)
}

/// 开关标志文件路径：`{data_dir}/api-enabled.flag`
pub fn enabled_flag_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join("api-enabled.flag")
}

/// 读取开关标志：上次退出时 API 是否处于开启状态（仅做 flag 文件存在判断）。
/// 与 `should_autostart` 同义，保留两个名字给不同调用方。
pub fn read_enabled_flag(app: &AppHandle) -> bool {
    enabled_flag_path(app).exists()
}

/// 上次退出时 API 是否处于开启状态（供启动自动恢复）。
pub fn should_autostart(app: &AppHandle) -> bool {
    read_enabled_flag(app)
}

/// 写开关标志（`api_start` 成功后调用）。
pub fn write_enabled_flag(app: &AppHandle) {
    let dir = db::data_dir(app);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(enabled_flag_path(app), b"1");
    }
}

/// 清开关标志（`api_stop` 后调用）。
pub fn clear_enabled_flag(app: &AppHandle) {
    let _ = std::fs::remove_file(enabled_flag_path(app));
}

/// 校验请求 Authorization 头是否为 `Bearer <token>`。
pub fn verify_bearer(req: &Request, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    req.headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == expected)
}