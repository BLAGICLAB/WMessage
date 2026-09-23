//! Token 管理 / 鉴权 / 开关持久化
//!
//! 完整模块组：
//! - `api_server`   : 服务生命周期 / 端口绑定 / EventHub
//! - `api_auth`     : 此文件 — token 管理 / 鉴权 / 开关持久化
//! - `api_handlers` : endpoint handlers + tauri commands
//! - `api`          : 数据层 `TaskStore` + `MemStore` + `TauriStore`
//!
//! 持久化文件（统一收口在数据目录 `runtime/flags/`，便携模式随 exe 走）：
//! - `api-token.txt`     — Bearer token（`load_or_create_token` 读写）
//! - `api-enabled.flag`  — 开关标志（`write_enabled_flag` / `clear_enabled_flag`）
//! - 启动时 `should_autostart` 检 flag 决定是否自动恢复 API
//!
//! 旧版本文件散在数据目录根，启动时由 `paths::migrate_legacy_runtime_files` 迁入。

use std::path::PathBuf;

use tauri::AppHandle;
use tiny_http::Request;

/// 运行期文件名（都落在 `paths::flags_dir` 下）
const TOKEN_FILE: &str = "api-token.txt";
const ENABLED_FLAG_FILE: &str = "api-enabled.flag";

/// 读取或生成 Bearer token（存在 `runtime/flags/api-token.txt`，随便携库走）。
/// 已存在且非空就直接返回；否则生成 UUID 写回。
pub fn load_or_create_token(app: &AppHandle) -> Result<String, String> {
    let dir = crate::paths::flags_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(TOKEN_FILE);
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            // 存量文件权限可能过宽（历史版本 0644 写入窗口遗留）
            // 永不补 chmod 的话会一直裸奔，读取时顺手收紧（best-effort，失败不挡读）
            let _ = tighten_token_permissions(&path);
            return Ok(s);
        }
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    write_token_file(&path, &token)?;
    Ok(token)
}

/// unix：把 token 文件权限收紧到 0600；其他平台无操作（返回 Ok）。
#[cfg(unix)]
fn tighten_token_permissions(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .map_err(|e| e.to_string())?
        .permissions()
        .mode()
        & 0o777;
    if mode != 0o600 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 非 unix 平台无文件权限位语义，恒 Ok。
#[cfg(not(unix))]
fn tighten_token_permissions(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

/// 写 token 文件并把权限收紧到 0600（token 等价于密码，默认 0644 可被同机其他用户读）。
/// 抽出独立函数便于单测（load_or_create_token 依赖 AppHandle 无法直测）。
///
/// tmp + rename 原子写，一次杀两个缺口（C5-AP-02）：
/// - tmp 用 `OpenOptions` + `.mode(0o600)` 创建即收紧，全程无旧权限可读窗口
///   （truncate 直写存量 0644 文件时，chmod 前 token 以旧权限落盘）；
/// - `rename` 替换目录项本身，**不跟随 symlink**——token 路径被预置 symlink 时
///   旧实现会把 token 写进 symlink 指向的文件，新实现把 symlink 整个替换成普通文件。
/// tmp 名带 pid+自增序号：api_start / api_rotate_token 的写路径互不加锁，固定 tmp 名
/// 会被并发写双方 truncate 撕裂。`create_new` 打开：tmp 必为新建 inode（mode(0o600)
/// 只对新建生效，残留/预置 tmp 不复用），撞名则报错不静默。写或 rename 失败都
/// best-effort 清理 tmp（里面装着明文 token，不能遗留）。
pub(crate) fn write_token_file(path: &std::path::Path, token: &str) -> Result<(), String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static TMP_SEQ: AtomicU32 = AtomicU32::new(0);
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("无效的目标路径：{}", path.display()))?;
    let tmp = path.with_file_name(format!(
        "{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| format!("打开临时文件 {} 失败：{e}", tmp.display()))?;
        use std::io::Write;
        f.write_all(token.as_bytes())
            .map_err(|e| format!("写入临时文件 {} 失败：{e}", tmp.display()))
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("落盘重命名失败：{e}"));
    }
    // belt & suspenders：tmp 创建即 0600，补 chmod 防极端文件系统 rename 权限继承差异
    tighten_token_permissions(path)?;
    Ok(())
}

/// 恒定时间字符串比较（字符串 `==` 提前退出，响应时间泄露前缀匹配长度，
/// 本机 API token 虽只在 127.0.0.1 暴露，仍按密钥标准处理）。
/// 不引 subtle 依赖（项目约束不加新依赖），XOR 折叠实现语义等价。
/// 长度不同直接 false（长度泄露可接受——token 固定 32 字符 UUID）。
pub(crate) fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// 开关标志文件路径：`{data_dir}/runtime/flags/api-enabled.flag`
pub fn enabled_flag_path<R: tauri::Runtime>(app: &AppHandle<R>) -> PathBuf {
    crate::paths::flags_dir(app).join(ENABLED_FLAG_FILE)
}

/// 上次退出时 API 是否处于开启状态（仅做 flag 文件存在判断；供启动自动恢复）。
pub fn should_autostart<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    enabled_flag_path(app).exists()
}

/// 写开关标志（`api_start` 成功后调用）。
pub fn write_enabled_flag<R: tauri::Runtime>(app: &AppHandle<R>) {
    let dir = crate::paths::flags_dir(app);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(ENABLED_FLAG_FILE), b"1");
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

    /// 存量 0644 文件（历史版本的写入窗口遗留）覆写后
    /// 必须补收紧到 0600——OpenOptions 的 mode() 只对新建文件生效
    #[cfg(unix)]
    #[test]
    fn write_token_file_tightens_existing_0644() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        std::fs::write(&path, "old-token").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_token_file(&path, "new-token").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-token");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "存量 0644 文件必须被补收紧，实际 {mode:o}");
    }

    /// token 路径是 symlink 时不得跟随写入：rename 替换目录项本身，
    /// symlink 目标的内容必须原样保留，token 路径变成普通文件。
    #[cfg(unix)]
    #[test]
    fn write_token_file_replaces_symlink_not_follow() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "victim-content").unwrap();
        let path = dir.path().join("api-token.txt");
        symlink(&victim, &path).unwrap();
        write_token_file(&path, "tok123").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "victim-content");
        assert!(!std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tok123");
        assert!(
            !dir.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "成功后不得遗留 tmp 残渣"
        );
    }

    /// 覆盖写（token 轮换）必须正常工作，且不留 tmp 残渣。
    #[test]
    fn write_token_file_overwrite_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        write_token_file(&path, "tok-v1").unwrap();
        write_token_file(&path, "tok-v2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tok-v2");
        assert!(
            !dir.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "成功后不得遗留 tmp 残渣"
        );
    }
}

/// 校验请求 Authorization 头是否为 `Bearer <token>`。
/// 恒定时间比较，防响应时间侧信道枚举 token 前缀。
pub fn verify_bearer(req: &Request, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    req.headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && ct_eq(h.value.as_str(), &expected))
}
