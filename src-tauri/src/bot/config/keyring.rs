//! Keyring 后端 + IO + slot 管理 + 迁移。
//!
//! 三个 KeySlot 共享同一套后端分发（System / PlaintextFile）+ 迁移逻辑；
//! 顺序：
//! - 后端探测（Linux secret-service 不可用时降级明文文件）
//! - 按后端分发 read/has/write/delete
//! - System 后端恢复可用时把降级明文迁回 keychain
//! - v0 service 名 → v1 带版本后缀 service 名迁移

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

use crate::db;
use crate::error::{CommandError, CommandResult};

use super::types::{KeySlot, KEYRING_SERVICE, LEGACY_KEYRING_SERVICE};

// ───────────────────────── 后端探测 ─────────────────────────

/// 凭据后端。Linux 的 keyring 走 secret-service（zbus/dbus）—— headless
/// 服务器/容器/最小桌面无 dbus 会话时，keyring 调用直接 PlatformFailure，用户 key
/// 存不住且只会看到「系统凭据存储访问失败」。运行时探测，不可用降级明文文件
/// （chmod 0600，与 api-token.txt 同策略）+ WARN 审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyBackend {
    /// 系统凭据存储（macOS 钥匙串 / Windows 凭据管理器 / Linux secret-service）
    System,
    /// 降级：数据目录 bot-api-key.txt 明文文件（仅 Linux 无 secret-service 时启用）
    PlaintextFile,
}

/// 可测内核：后端选择纯函数
pub(crate) fn backend_for(secret_service_ok: bool) -> KeyBackend {
    if secret_service_ok {
        KeyBackend::System
    } else {
        KeyBackend::PlaintextFile
    }
}

/// secret-service 运行时探测（Linux）：dbus session 总线存在即认为可用。
/// 判据：DBUS_SESSION_BUS_ADDRESS 环境变量，或 $XDG_RUNTIME_DIR/bus socket。
/// 其他平台恒 true（macOS/Windows 凭据存储无外部服务依赖）。
#[cfg(target_os = "linux")]
fn secret_service_available() -> bool {
    secret_service_available_with(
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|v| !v.is_empty()),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
    )
}

#[cfg(not(target_os = "linux"))]
fn secret_service_available() -> bool {
    true
}

/// 可测内核：dbus 地址存在，或 XDG_RUNTIME_DIR 下有 bus socket
///（非 Linux 仅测试使用：生产 secret_service_available 恒 true）
#[cfg_attr(all(not(target_os = "linux"), not(test)), allow(dead_code))]
pub(crate) fn secret_service_available_with(
    has_dbus_addr: bool,
    xdg_runtime_dir: Option<&str>,
) -> bool {
    if has_dbus_addr {
        return true;
    }
    xdg_runtime_dir
        .map(|d| std::path::Path::new(d).join("bus").exists())
        .unwrap_or(false)
}

pub(crate) fn key_backend() -> KeyBackend {
    backend_for(secret_service_available())
}

/// Linux 应用数据目录（无 AppHandle 场景，降级 key 路径用）：对齐 tauri
/// app_data_dir 规则 —— $XDG_DATA_HOME/com.renshi.wmessage，缺省
/// ~/.local/share/com.renshi.wmessage。非 Linux 返回 None（降级后端不会启用）。
#[cfg(target_os = "linux")]
fn linux_app_data_dir() -> Option<std::path::PathBuf> {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        if !x.is_empty() {
            return Some(std::path::PathBuf::from(x).join("com.renshi.wmessage"));
        }
    }
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/com.renshi.wmessage"))
}

#[cfg(not(target_os = "linux"))]
fn linux_app_data_dir() -> Option<std::path::PathBuf> {
    None
}

/// 降级 key 文件路径：与数据目录同一便携策略——优先复用 probe_log_dir
/// 已定版的缓存结果（防每次探测瞬时失败导致 key 文件与数据库分裂两地）；
/// 未初始化（如启动早期 keyring 迁移先于首次 data_dir 调用）回退原现探逻辑。
/// 按 KeySlot 参数化（LLM/Tavily/Brave 各一个降级文件）。
pub(crate) fn plaintext_key_path_for(slot: KeySlot) -> std::path::PathBuf {
    if let Some(cached) = crate::paths::cached_probe_dir() {
        return cached.join(slot.plaintext_filename());
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    crate::paths::probe_dir(exe_dir.as_deref(), linux_app_data_dir())
        .join(slot.plaintext_filename())
}

/// 降级告警：每进程首用降级后端时记一条 WARN 审计（避免每次读 key 刷屏；
/// 三个 slot 共用同一次告警，审计文案带触发 slot 的文件名）。
/// 写在与 key 文件同目录的 bot.log（无 AppHandle，走 write_warn_audit_to）。
fn warn_fallback_once(slot: KeySlot) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WARNED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Some(dir) = plaintext_key_path_for(slot)
        .parent()
        .map(|p| p.to_path_buf())
    {
        let reason = format!(
            "secret-service 不可用（无 dbus 会话），API Key 降级明文文件存储（0600）：{}",
            slot.plaintext_filename()
        );
        crate::audit::write_warn_audit_to(
            &dir,
            "keyring_fallback_plaintext",
            &[("reason", &reason)],
        );
    }
}

/// 降级文件读取：文件缺失 = 未配置（对齐 System 路径 NoEntry → KeyringError
/// 同 code，前端 hint 一致）；真实 IO 故障 → KeyringError，不静默吞。
fn read_key_file_from(p: &std::path::Path) -> CommandResult<String> {
    match std::fs::read_to_string(p) {
        Ok(s) => {
            let k = s.trim().to_string();
            if k.is_empty() {
                return Err(CommandError::KeyringError(
                    "未配置 API Key（降级文件存储为空）".into(),
                ));
            }
            Ok(k)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CommandError::KeyringError(
            "未配置 API Key（降级文件存储）".into(),
        )),
        Err(e) => Err(CommandError::KeyringError(format!(
            "读取 API Key 失败（降级文件存储）：{e}"
        ))),
    }
}

/// 降级文件写入：父目录不存在则创建；Unix 创建即 0600（OpenOptionsExt::mode，
/// 与 api-token.txt 同策略——无「先写后 chmod」的 umask 窗口，chmod 失败问题不存在）。
/// Windows：无 DACL 等价收紧——权限语义仅在 Unix 承诺（拍板 #13=B wontfix-with-rationale：
/// 本仓无 Windows CI/验证手段，写 ACL 无法保证正确；降级存储本就是 keyring 不可用时的兜底）。
fn write_key_file_to(p: &std::path::Path, key: &str) -> CommandResult<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            CommandError::KeyringError(format!("保存 API Key 失败（降级文件存储）：{e}"))
        })?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600); // 创建时即 0600，无「先 0644 后 chmod」窗口
    }
    let mut f = opts.open(p).map_err(|e| {
        CommandError::KeyringError(format!("保存 API Key 失败（降级文件存储）：{e}"))
    })?;
    f.write_all(key.as_bytes()).map_err(|e| {
        CommandError::KeyringError(format!("保存 API Key 失败（降级文件存储）：{e}"))
    })?;
    #[cfg(unix)]
    {
        // 已存在文件 mode() 不生效，补 chmod；失败记 WARN（不静默吞）
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).is_err() {
            if let Some(dir) = p.parent().map(|d| d.to_path_buf()) {
                crate::audit::write_warn_audit_to(
                    &dir,
                    "keyring_fallback_chmod_failed",
                    &[("file", "bot-api-key.txt")],
                );
            }
        }
    }
    Ok(())
}

// ───────────────────────── keyring entry 构造 ─────────────────────────

pub(crate) fn key_entry(slot: KeySlot) -> CommandResult<keyring::Entry> {
    key_entry_for_service(KEYRING_SERVICE, slot)
}

/// 指定 service 名的条目（v0 → v1 迁移需要同时访问新旧两个 service）。
pub(crate) fn key_entry_for_service(service: &str, slot: KeySlot) -> CommandResult<keyring::Entry> {
    keyring::Entry::new(service, slot.keyring_user())
        .map_err(|e| CommandError::KeyringError(format!("系统凭据存储不可用：{e}")))
}

// ───────────────────────── 按后端分发 ─────────────────────────

/// 按后端分发读取（可测：PlaintextFile + 注入路径即「mock secret-service 不可用」）；
/// 带 slot（System 后端按 slot 选 keyring 条目）
pub(crate) fn read_api_key_at(
    backend: KeyBackend,
    file: &std::path::Path,
    slot: KeySlot,
) -> CommandResult<String> {
    match backend {
        KeyBackend::System => classify_get_password(key_entry(slot)?.get_password()),
        KeyBackend::PlaintextFile => read_key_file_from(file),
    }
}

/// 按后端分发存在性检查：缺失 → Ok(false)（对齐 classify_has_key 的 NoEntry 语义）；
/// 真实读取故障 → Err，不吞成 false
pub(crate) fn has_api_key_at(
    backend: KeyBackend,
    file: &std::path::Path,
    slot: KeySlot,
) -> CommandResult<bool> {
    match backend {
        KeyBackend::System => match key_entry(slot) {
            Ok(e) => classify_has_key(e.get_password()),
            Err(e) => Err(e),
        },
        KeyBackend::PlaintextFile => match std::fs::read_to_string(file) {
            Ok(s) => Ok(!s.trim().is_empty()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(CommandError::KeyringError(format!(
                "检查 API Key 失败（降级文件存储）：{e}"
            ))),
        },
    }
}

/// 按后端分发写入
pub(crate) fn write_api_key_at(
    backend: KeyBackend,
    file: &std::path::Path,
    key: &str,
    slot: KeySlot,
) -> CommandResult<()> {
    match backend {
        KeyBackend::System => key_entry(slot)?
            .set_password(key)
            .map_err(|e| CommandError::KeyringError(format!("保存 API Key 失败：{e}"))),
        KeyBackend::PlaintextFile => write_key_file_to(file, key),
    }
}

/// 按后端分发删除（幂等：文件不存在 = Ok）
pub(crate) fn delete_api_key_at(
    backend: KeyBackend,
    file: &std::path::Path,
    slot: KeySlot,
) -> CommandResult<()> {
    match backend {
        KeyBackend::System => {
            let r = key_entry(slot)?
                .delete_credential()
                .map_err(|e| CommandError::KeyringError(format!("清除 API Key 失败：{e}")));
            // 顺带清 v0 遗留条目：否则下次读取会把它当作「可迁移的旧 key」搬回新条目，
            // 用户「清除」后 key 复活。主删除成功才清（主删除失败时遗留原样保留，
            // 避免「主 key 没删成、旧条目反被清」的半清状态）；清理失败记日志不阻断
            if r.is_ok() {
                if let Ok(old) = key_entry_for_service(LEGACY_KEYRING_SERVICE, slot) {
                    if let Err(e) = old.delete_credential() {
                        eprintln!("[keyring] legacy entry cleanup failed: {e}");
                    }
                }
            }
            r
        }
        KeyBackend::PlaintextFile => match std::fs::remove_file(file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CommandError::KeyringError(format!(
                "清除 API Key 失败（降级文件存储）：{e}"
            ))),
        },
    }
}

/// get_password 结果分类——任何失败都映射为 KeyringError 结构化变体，
/// 不走 String 逃生舱（同类故障产出两种 code，前端 hintForCode 失配）。
/// 抽成纯函数便于单测（keyring 真实存储在测试环境不可用）。
pub(crate) fn classify_get_password(r: Result<String, keyring::Error>) -> CommandResult<String> {
    r.map_err(|e| CommandError::KeyringError(format!("读取 API Key 失败：{e}")))
}

/// 「key 不存在」（NoEntry）→ Ok(false)；
/// keyring 真实故障（钥匙串锁定 / 权限拒绝）→ Err(KeyringError)，不静默吞成 false。
pub(crate) fn classify_has_key(r: Result<String, keyring::Error>) -> CommandResult<bool> {
    match r {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(CommandError::KeyringError(format!(
            "检查 API Key 失败：{e}"
        ))),
    }
}

// ───────────────────────── slot 顶层入口 ─────────────────────────

pub fn read_api_key() -> CommandResult<String> {
    read_key_of_slot(KeySlot::Llm)
}

pub fn has_api_key() -> CommandResult<bool> {
    has_key_of_slot(KeySlot::Llm)
}

/// 按 slot 读取：降级后端记 WARN；System 后端顺带做降级文件回迁 + v0 service 迁移
pub(crate) fn read_key_of_slot(slot: KeySlot) -> CommandResult<String> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    } else {
        prepare_system_backend(slot);
    }
    read_api_key_at(backend, &plaintext_key_path_for(slot), slot)
}

/// 按 slot 存在性检查
pub(crate) fn has_key_of_slot(slot: KeySlot) -> CommandResult<bool> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    } else {
        prepare_system_backend(slot);
    }
    has_api_key_at(backend, &plaintext_key_path_for(slot), slot)
}

pub(crate) fn write_api_key(key: &str) -> CommandResult<()> {
    write_key_of_slot(KeySlot::Llm, key)
}

/// 按 slot 写入
pub(crate) fn write_key_of_slot(slot: KeySlot, key: &str) -> CommandResult<()> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    }
    write_api_key_at(backend, &plaintext_key_path_for(slot), key, slot)
}

// ───────────────────────── 搜索 key（Tavily/Brave）───────────────────────

/// 读搜索 key：未配置返回空串（搜索 key 是可选配置，区别于主 LLM key 的硬错误）；
/// keyring 真实故障（锁定/权限拒绝）透传 Err，不静默吞成空串（与 has_api_key 同策略）。
pub fn read_search_key(slot: KeySlot) -> CommandResult<String> {
    // 先 has 后 read：has 的 NoEntry → Ok(false) 语义即「未配置」；
    // 真实故障在 has 处已透传 Err
    if !has_key_of_slot(slot)? {
        return Ok(String::new());
    }
    read_key_of_slot(slot)
}

/// 搜索 key 存在性检查（bot_get_config 组 view 用）
pub fn has_search_key(slot: KeySlot) -> CommandResult<bool> {
    has_key_of_slot(slot)
}

/// 写搜索 key（bot_set_config 顶层参数用；Some(非空) 覆盖语义在调用方）
pub fn write_search_key(slot: KeySlot, key: &str) -> CommandResult<()> {
    write_key_of_slot(slot, key)
}

// ───────────────────────── System 后端迁移 ─────────────────────────

/// System 后端恢复可用时，把降级明文 key 迁回 keychain
/// 并删除文件——否则降级文件永久残留（clear 走当前后端，PlaintextFile 分支轮不到），
/// 用户以为「早就只用 keychain 了」，明文副本却留在数据目录。幂等：无文件直接返回。
/// 按 slot 处理（LLM/Tavily/Brave 各自的降级文件都回迁）。
fn migrate_plaintext_key_if_system(slot: KeySlot) {
    let p = plaintext_key_path_for(slot);
    if !p.exists() {
        return;
    }
    // 读不出内容不删文件（数据保留优先），迁回 keychain 成功才删
    let migrated = read_key_file_from(&p)
        .ok()
        .and_then(|key| key_entry(slot).ok().map(|e| e.set_password(&key)))
        .and_then(|r| r.ok())
        .is_some();
    if migrated {
        let _ = std::fs::remove_file(&p);
        if let Some(dir) = p.parent().map(|d| d.to_path_buf()) {
            crate::audit::write_warn_audit_to(
                &dir,
                "keyring_migrated_from_plaintext",
                &[("file", slot.plaintext_filename())],
            );
        }
    }
}

/// 每 slot「旧 service 已探测过」标记（进程级）：
/// 稳态（v1 有条目 / 根本没配 key / v0 也没有）后不再多读一次旧 service，
/// 与迁移前的 keyring 调用次数一致。
static KEYRING_SERVICE_MIGRATED: [AtomicBool; 3] = [
    AtomicBool::new(false),
    AtomicBool::new(false),
    AtomicBool::new(false),
];

/// v0 → v1 service 名迁移（幂等）：
/// 新（带版本后缀）条目已有非空值 → no-op（不覆盖用户新 key）；
/// v0 有条目而新条目为空 → 复制过来，成功后删除 v0 条目。
/// keyring 故障静默返回且不置「已探测」标记（下次读取再试）——
/// 迁移绝不能挡住用户已有的 key（数据保留优先，与 migrate_plaintext_key_if_system 同策略）。
fn migrate_legacy_keyring_entry(slot: KeySlot) {
    let idx = slot.index();
    if KEYRING_SERVICE_MIGRATED[idx].load(Ordering::SeqCst) {
        return;
    }
    let Ok(new_entry) = key_entry(slot) else {
        return;
    };
    match new_entry.get_password() {
        Ok(v) if !v.trim().is_empty() => {
            KEYRING_SERVICE_MIGRATED[idx].store(true, Ordering::SeqCst);
            return; // 已是新版条目，无需求
        }
        Ok(_) | Err(keyring::Error::NoEntry) => {} // 空/未配置：继续看 v0 条目
        Err(_) => return,                          // keyring 故障：下次再试
    }
    let Ok(old_entry) = key_entry_for_service(LEGACY_KEYRING_SERVICE, slot) else {
        return;
    };
    match old_entry.get_password() {
        Ok(old) => {
            let old = old.trim().to_string();
            if !old.is_empty() && new_entry.set_password(&old).is_ok() {
                let _ = old_entry.delete_credential(); // 删除失败不致命：v1 已有值，下次早退
                if let Some(dir) = plaintext_key_path_for(slot)
                    .parent()
                    .map(|p| p.to_path_buf())
                {
                    crate::audit::write_warn_audit_to(
                        &dir,
                        "keyring_service_migrated",
                        &[
                            ("slot", slot.keyring_user()),
                            ("from", LEGACY_KEYRING_SERVICE),
                        ],
                    );
                }
            } else {
                return; // 空值不搬 / 写失败：保留 v0，下次再试
            }
        }
        Err(keyring::Error::NoEntry) => {} // 无 v0 遗留条目：迁移态已确认
        Err(_) => return,                  // keyring 故障：下次再试
    }
    KEYRING_SERVICE_MIGRATED[idx].store(true, Ordering::SeqCst);
}

/// System 后端读取前的准备工作（两步都幂等）：
/// 降级明文文件回迁 keychain + v0 service 条目迁进带版本后缀的新条目。
fn prepare_system_backend(slot: KeySlot) {
    migrate_plaintext_key_if_system(slot);
    migrate_legacy_keyring_entry(slot);
}

// 抑制 unused 警告：AppHandle 暂未直接用于本模块（迁移走数据目录）
#[allow(dead_code)]
fn _app_handle_marker(_a: &AppHandle) {}
