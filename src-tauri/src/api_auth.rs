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
    load_or_create_token_at(&dir.join(TOKEN_FILE))
}

/// token 文件的读写决策（抽出便于单测：load_or_create_token 依赖 AppHandle 无法直测）。
///
/// 三种盘上状态：
/// - 非空 → 直接用（顺手收紧权限）
/// - 存在但为空 → 坏态（写 token 的进程崩在 create 与 write 之间），
///   原子覆写自愈：create_new 在空文件上永远撞 AlreadyExists，
///   走首建路径只会空转重试，「下次调用自愈」并不成立
/// - 不存在 → 原子首建（并发首跑只有一个胜者）
fn load_or_create_token_at(path: &std::path::Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let s = s.trim().to_string();
            if !s.is_empty() {
                // 存量文件权限可能过宽（历史版本 0644 写入窗口遗留）
                // 永不补 chmod 的话会一直裸奔，读取时顺手收紧（best-effort，失败不挡读）
                if let Err(e) = tighten_token_permissions(path) {
                    eprintln!("[api_auth] 警告：存量 token 文件权限收紧失败（可能仍可读）：{e}");
                }
                return Ok(s);
            }
            return repair_empty_token_file(path);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("读取 token 文件失败：{e}")),
    }
    let token = uuid::Uuid::new_v4().simple().to_string();
    // 原子首建：并发首跑（双开应用/后台任务）两进程都可能
    // 观测不到 token 各生成 UUID——check-then-act 让后写覆盖先写，输家返回
    // 与磁盘不符的 token。create_new 原子占位：唯一胜者写自己的 token；
    // 输家重读胜者落盘的那份（胜者 create 与 write_all 之间有空窗，短暂重试）。
    if create_token_file_atomic(path, &token)? {
        return Ok(token);
    }
    for _ in 0..50 {
        // 同步阻塞重试（上限 500ms 与胜者 create→write_all 空窗同量级）：
        // 本函数仅供 sync Tauri command 调用，勿在 async/UI 线程直接用。
        if let Ok(s) = std::fs::read_to_string(path) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return Ok(s);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err("token 文件被并发创建但内容一直为空（读超时，下次调用自愈）".into())
}

/// 空 token 文件自愈：原子覆写（tmp + rename），不用 create_new——
/// 空文件已经占着路径，create_new 必然撞 AlreadyExists，
/// 覆写后以盘上值为准（并发自愈时双方读到同一份）。
fn repair_empty_token_file(path: &std::path::Path) -> Result<String, String> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    write_token_file(path, &token)?;
    let on_disk =
        std::fs::read_to_string(path).map_err(|e| format!("自愈后读取 token 文件失败：{e}"))?;
    let on_disk = on_disk.trim().to_string();
    if on_disk.is_empty() {
        return Err("token 文件自愈后仍为空".into());
    }
    Ok(on_disk)
}

/// 原子首建 token 文件（create_new 占位 + unix 0600）：Ok(true)=本次创建成功，
/// Ok(false)=已存在（并发首跑胜者占位）。仅 load_or_create_token 首建路径用；
/// 覆写/轮换走 write_token_file（tmp+rename）。
fn create_token_file_atomic(path: &std::path::Path, token: &str) -> Result<bool, String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write;
    match opts.open(path) {
        Ok(mut f) => {
            f.write_all(token.as_bytes())
                .map_err(|e| format!("写入 token 文件失败：{e}"))?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(format!("创建 token 文件 {} 失败：{e}", path.display())),
    }
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
        if let Err(cleanup_err) = std::fs::remove_file(&tmp) {
            eprintln!("[api_auth] 警告：临时 token 文件清理失败（内含明文 token，请手动删除）：{}：{cleanup_err}", tmp.display());
        }
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        if let Err(cleanup_err) = std::fs::remove_file(&tmp) {
            eprintln!("[api_auth] 警告：临时 token 文件清理失败（内含明文 token，请手动删除）：{}：{cleanup_err}", tmp.display());
        }
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

/// 上次退出时 API 是否处于开启状态（校验 flag 内容而非仅凭存在：
/// 半截/空文件不应触发自动恢复；供启动自动恢复）。
pub fn should_autostart<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    std::fs::read(enabled_flag_path(app))
        .map(|b| b == b"1")
        .unwrap_or(false)
}

/// 写开关标志（`api_start` 成功后调用）。
pub fn write_enabled_flag<R: tauri::Runtime>(app: &AppHandle<R>) {
    let dir = crate::paths::flags_dir(app);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        // flag 是「上次退出时 API 开启」的盘上事实源，写失败必须留痕
        eprintln!("[api_auth] 建标志目录失败（下次启动不会自动恢复 API）：{e}");
        return;
    }
    if let Err(e) = std::fs::write(dir.join(ENABLED_FLAG_FILE), b"1") {
        eprintln!("[api_auth] 写开关标志失败（下次启动不会自动恢复 API）：{e}");
    }
}

/// 清开关标志（`api_stop` 后调用）。
pub fn clear_enabled_flag<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Err(e) = std::fs::remove_file(enabled_flag_path(app)) {
        // NotFound = 本来就没开，正常；其余失败留痕——残留 flag 会让下次启动意外自动恢复
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[api_auth] 清开关标志失败（下次启动可能意外自动恢复 API）：{e}");
        }
    }
}

/// 校验请求 Authorization 头是否为 `Bearer <token>`。
/// 恒定时间比较，防响应时间侧信道枚举 token 前缀。
/// scheme 按 RFC 7235 大小写不敏感（`bearer `/`BEARER ` 同样放行），
/// token 本体仍走精确 ct_eq（token 是小写 hex UUID，大小写变体不接受）。
pub fn verify_bearer(req: &Request, token: &str) -> bool {
    req.headers().iter().any(|h| {
        h.field.equiv("Authorization")
            && h.value
                .as_str()
                .split_once(' ')
                .map(|(scheme, rest)| scheme.eq_ignore_ascii_case("Bearer") && ct_eq(rest, token))
                .unwrap_or(false)
    })
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

    /// 原子首建——首个调用者占位成功，后续调用者不覆盖。
    #[cfg(unix)]
    #[test]
    fn create_token_file_atomic_first_wins() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        assert!(create_token_file_atomic(&path, "tok-first").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tok-first");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "首建 token 文件必须 0600，实际 {mode:o}");
        // 并发首跑输家：不覆盖胜者内容
        assert!(!create_token_file_atomic(&path, "tok-second").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "tok-first");
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

    /// 空 token 文件必须自愈成可用 token：create_new 撞 AlreadyExists 的旧路
    /// 只会空转重试，api_start 恒失败。
    #[test]
    fn load_or_create_token_at_repairs_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        std::fs::write(&path, "").unwrap();
        let token = load_or_create_token_at(&path).unwrap();
        assert!(!token.is_empty(), "空文件自愈后必须拿到非空 token");
        // 盘上值 = 返回值（下次调用读到的是同一份）
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), token);
        // 再次调用不再重新生成（内容稳定）
        assert_eq!(load_or_create_token_at(&path).unwrap(), token);
    }

    /// 空文件自愈不得留 tmp 残渣（tmp 里是明文 token）。
    #[test]
    fn load_or_create_token_at_repair_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        std::fs::write(&path, "   \n").unwrap();
        load_or_create_token_at(&path).unwrap();
        assert!(
            !dir.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "自愈后不得遗留 tmp 残渣"
        );
    }

    /// 文件不存在 → 原子首建，token 非空且落盘。
    #[test]
    fn load_or_create_token_at_creates_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        let token = load_or_create_token_at(&path).unwrap();
        assert!(!token.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), token);
    }

    /// 非空存量 token 直接沿用（不得被重新生成覆盖）。
    #[test]
    fn load_or_create_token_at_keeps_existing_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("api-token.txt");
        std::fs::write(&path, "existing-token").unwrap();
        assert_eq!(load_or_create_token_at(&path).unwrap(), "existing-token");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "existing-token",
            "存量 token 不得被改写"
        );
    }
}
