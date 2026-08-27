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
    write_token_file(&path, &token)?;
    Ok(token)
}

/// 写 token 文件并把权限收紧到 0600（P2-1：token 等价于密码，默认 0644 可被同机其他用户读）。
/// 抽出独立函数便于单测（load_or_create_token 依赖 AppHandle 无法直测）。
pub(crate) fn write_token_file(path: &std::path::Path, token: &str) -> Result<(), String> {
    std::fs::write(path, token).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 恒定时间字符串比较（P2-1：字符串 `==` 提前退出，响应时间泄露前缀匹配长度，
/// 本机 API token 虽只在 127.0.0.1 暴露，仍按密钥标准处理）。
/// 不引 subtle 依赖（项目约束不加新依赖），XOR 折叠实现语义等价。
/// 长度不同直接 false（长度泄露可接受——token 固定 32 字符 UUID）。
pub(crate) fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 开关标志文件路径：`{data_dir}/api-enabled.flag`
pub fn enabled_flag_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    db::data_dir(app).join("api-enabled.flag")
}

/// 读取开关标志：上次退出时 API 是否处于开启状态（仅做 flag 文件存在判断）。
/// 与 `should_autostart` 同义，保留两个名字给不同调用方。
pub fn read_enabled_flag<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    enabled_flag_path(app).exists()
}

/// 上次退出时 API 是否处于开启状态（供启动自动恢复）。
pub fn should_autostart<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    read_enabled_flag(app)
}

/// 写开关标志（`api_start` 成功后调用）。
pub fn write_enabled_flag<R: tauri::Runtime>(app: &AppHandle<R>) {
    let dir = db::data_dir(app);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(enabled_flag_path(app), b"1");
    }
}

/// 清开关标志（`api_stop` 后调用）。
pub fn clear_enabled_flag<R: tauri::Runtime>(app: &AppHandle<R>) {
    let _ = std::fs::remove_file(enabled_flag_path(app));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_same_length_different_content_is_false() {
        // 长度相同但内容不同的两个 token 必须判不等（防提前退出侧信道的前提）
        let a = "0123456789abcdef0123456789abcdef";
        let b = "0123456789abcdef0123456789abcde0";
        assert!(!ct_eq(a, b));
        assert!(ct_eq(a, a));
    }

    #[test]
    fn ct_eq_different_length_is_false() {
        assert!(!ct_eq("short", "much-longer-token"));
        assert!(!ct_eq("", "x"));
    }

    #[cfg(unix)]
    #[test]
    fn write_token_file_chmods_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        write_token_file(&path, "tok123").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tok123");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token 文件权限必须收紧到 0600，实际 {mode:o}");
    }
}

/// 校验请求 Authorization 头是否为 `Bearer <token>`。
/// P2-1：恒定时间比较，防响应时间侧信道枚举 token 前缀。
pub fn verify_bearer(req: &Request, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    req.headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && ct_eq(h.value.as_str(), &expected))
}