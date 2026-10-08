//! ：MCP env/headers 机密存储——每台服务器一个 blob，存系统凭据
//! 存储；bot-config.json 永不再落 env/headers 明文（`skip_serializing` 收口）。
//!
//! 设计（`docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md` §2）：
//! - keyring：复用 `KEYRING_SERVICE`（同 service = 复用应用钥匙串 ACL），
//!   user = `mcp:<server_id>`（uuid simple 字符集安全）；value = blob JSON；
//! - Linux 无 dbus 降级：单文件 `bot-mcp-secrets.json`（数据目录，0600，
//!   tmp+rename 原子写，`{ "<id>": blob }`），WARN 审计 `mcp.secret_fallback_plaintext`
//!  （每进程一次）；
//! - 进程内缓存：`mcp_status` 等高频读路径不反复敲 keychain（缓存含「读过但空」，
//!   读失败也缓存避免每次轮询都撞故障后端）；
//! - 体积上限：blob 序列化 ≤ 2048 字节（Windows 凭据 blob 上限 2560 留余量）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::bot::config::keyring::KeyBackend;
use crate::bot::config::types::KEYRING_SERVICE;
use crate::db::paths;

/// 单服务器 blob 体积上限（Windows CRED_MAX_CREDENTIAL_BLOB_SIZE 2560 留余量）
pub const MCP_SECRET_BLOB_MAX_BYTES: usize = 2048;

/// 单台服务器的机密集合（与 McpServerConfig.env/headers 同构；
/// BTreeMap 键序稳定 = blob 序列化确定，读回比对才有意义）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpSecrets {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

impl McpSecrets {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.headers.is_empty()
    }
    /// 序列化为 blob（确定性键序），超 [`MCP_SECRET_BLOB_MAX_BYTES`] 报错
    pub fn to_blob(&self) -> Result<String, String> {
        let s = serde_json::to_string(self).map_err(|e| format!("机密 blob 序列化失败：{e}"))?;
        if s.len() > MCP_SECRET_BLOB_MAX_BYTES {
            return Err(format!(
                "机密 blob 过大（{} 字节 > 上限 {MCP_SECRET_BLOB_MAX_BYTES}）；请缩短 token 或拆分 env 条目",
                s.len()
            ));
        }
        Ok(s)
    }
    pub fn from_blob(s: &str) -> Result<Self, String> {
        // 读写同口径：超限 blob 读入即拒（与 to_blob 上限对称）——否则超限值
        // 解析成功照常水合，下次原样重存时才在 to_blob 处爆雷
        if s.len() > MCP_SECRET_BLOB_MAX_BYTES {
            return Err(format!(
                "机密 blob 过大（{} 字节 > 上限 {MCP_SECRET_BLOB_MAX_BYTES}）；请缩短 token 或拆分 env 条目",
                s.len()
            ));
        }
        serde_json::from_str(s).map_err(|e| format!("机密 blob 解析失败：{e}"))
    }
}

// 进程内缓存

/// id → 已读结果（None = 读过且无值 / 读失败按空处理）。
/// 锁中毒按仓库惯例 unwrap_or_else 响亮恢复。
fn cache() -> &'static Mutex<std::collections::HashMap<String, Option<McpSecrets>>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<McpSecrets>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn cache_get(id: &str) -> Option<Option<McpSecrets>> {
    cache()
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] mcp::secrets cache: {e:?}");
            e.into_inner()
        })
        .get(id)
        .cloned()
}

fn cache_put(id: &str, v: Option<McpSecrets>) {
    cache()
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] mcp::secrets cache: {e:?}");
            e.into_inner()
        })
        .insert(id.to_string(), v);
}

fn cache_remove(id: &str) {
    cache()
        .lock()
        .unwrap_or_else(|e| {
            eprintln!("[mutex_poisoned] mcp::secrets cache: {e:?}");
            e.into_inner()
        })
        .remove(id);
}

// 后端分发

/// 降级单文件存储路径（仅 key_backend() == PlaintextFile 时使用）
fn fallback_file(app: &tauri::AppHandle) -> PathBuf {
    paths::data_dir(app).join("bot-mcp-secrets.json")
}

/// 降级单文件 RMW 互斥（评审 M 采纳）：read→parse→mutate→write→rename 是
/// 非原子 RMW，两个命令线程并发会互相覆盖丢条目——进程内串行化（后端仅在
/// Linux 无 dbus 时降级到单文件，进程内锁足够；跨进程场景不存在——单实例应用）
fn fallback_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn fallback_lock_guard() -> std::sync::MutexGuard<'static, ()> {
    fallback_lock().lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] mcp::secrets fallback_lock: {e:?}");
        e.into_inner()
    })
}

static FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);

fn warn_fallback_once(app: &tauri::AppHandle) {
    if !FALLBACK_WARNED.swap(true, Ordering::Relaxed) {
        crate::audit::write_event(
            app,
            crate::audit::AuditLevel::Warn,
            "mcp.secret_fallback_plaintext",
            &[("backend", "plaintext_file".to_string())],
        );
    }
}

fn keyring_entry(id: &str) -> Result<::keyring::Entry, String> {
    ::keyring::Entry::new(KEYRING_SERVICE, &format!("mcp:{id}"))
        .map_err(|e| format!("系统凭据存储不可用：{e}"))
}

/// 读一台服务器的机密（缓存命中不敲后端）。Ok(None) = 无值；
/// 后端读取故障 → Err（hydrate 层转空 + WARN，不炸配置加载）
fn read_backend(app: &tauri::AppHandle, id: &str) -> Result<Option<McpSecrets>, String> {
    let backend = crate::bot::config::keyring::key_backend();
    let file = fallback_file(app);
    let _g = (backend == KeyBackend::PlaintextFile).then_some(fallback_lock_guard());
    read_backend_at(backend, &file, id)
}

/// 后端注入内核（评审/测试：PlaintextFile + 临时文件直打，不碰真实钥匙串）。
/// app 仅用于降级 WARN（None = 测试路径，免审计噪音）
fn read_backend_at(
    backend: KeyBackend,
    file: &std::path::Path,
    id: &str,
) -> Result<Option<McpSecrets>, String> {
    match backend {
        KeyBackend::System => {
            let entry = keyring_entry(id)?;
            match entry.get_password() {
                Ok(s) => Ok(Some(McpSecrets::from_blob(&s)?)),
                Err(::keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(format!("读取机密失败：{e}")),
            }
        }
        KeyBackend::PlaintextFile => {
            // 读失败不静默吞：NotFound = 未落盘即无值；其余 I/O 错误传播
            //（hydrate 层转空 + WARN）——权限/磁盘故障若当「无机密」会被缓存
            // 固化成永久丢机密，与写路径的 NotFound 口径保持一致
            let raw = match std::fs::read_to_string(file) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(format!("读取降级机密文件失败：{e}")),
            };
            if raw.trim().is_empty() {
                return Ok(None);
            }
            let map: std::collections::HashMap<String, McpSecrets> =
                serde_json::from_str(&raw).map_err(|e| format!("降级机密文件解析失败：{e}"))?;
            Ok(map.get(id).cloned())
        }
    }
}

/// 写一台服务器的机密（体积上限前置校验 + 后端分发；System 路径写后读回比对）
fn write_backend(app: &tauri::AppHandle, id: &str, blob: &str) -> Result<(), String> {
    let backend = crate::bot::config::keyring::key_backend();
    let file = fallback_file(app);
    let _g = (backend == KeyBackend::PlaintextFile).then_some(fallback_lock_guard());
    let used_fallback = write_backend_at(backend, &file, id, blob)?;
    if used_fallback.is_some() {
        warn_fallback_once(app);
    }
    Ok(())
}

/// 后端注入内核。返回 Ok(())；PlaintextFile 成功后由调用方补降级 WARN。
/// System 路径写后读回比对（迁移与保存共用的安全不变式）
fn write_backend_at(
    backend: KeyBackend,
    file: &std::path::Path,
    id: &str,
    blob: &str,
) -> Result<Option<()>, String> {
    match backend {
        KeyBackend::System => {
            let entry = keyring_entry(id)?;
            entry
                .set_password(blob)
                .map_err(|e| format!("机密写入系统凭据存储失败：{e}"))?;
            // 读回比对（迁移与保存共用同一条安全不变式：剥离明文的唯一途径是
            // 「keyring 已确认写入」）
            let back = entry
                .get_password()
                .map_err(|e| format!("机密写入后读回校验失败：{e}"))?;
            if back != blob {
                return Err("机密写入后读回不一致（拒绝继续，防剥离后丢失）".to_string());
            }
            Ok(None)
        }
        KeyBackend::PlaintextFile => {
            // 评审 M 采纳：读失败不静默吞——瞬时不可读若当「空 map」会整体
            // 覆盖丢掉其余服务器条目（永久丢失）；NotFound 才算空，其余报错
            let mut map: std::collections::HashMap<String, McpSecrets> =
                match std::fs::read_to_string(file) {
                    Ok(raw) if raw.trim().is_empty() => Default::default(),
                    Ok(raw) => serde_json::from_str(&raw)
                        .map_err(|e| format!("降级机密文件解析失败：{e}"))?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
                    Err(e) => return Err(format!("读取降级机密文件失败：{e}")),
                };
            let secrets = McpSecrets::from_blob(blob)?;
            if secrets.is_empty() {
                map.remove(id);
            } else {
                map.insert(id.to_string(), secrets);
            }
            let out = serde_json::to_string_pretty(&map)
                .map_err(|e| format!("降级机密文件序列化失败：{e}"))?;
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败：{e}"))?;
            }
            let tmp = file.with_extension("tmp");
            write_tmp_0600(&tmp, out.as_bytes())?;
            std::fs::rename(&tmp, file).map_err(|e| format!("降级机密文件落盘失败：{e}"))?;
            Ok(Some(()))
        }
    }
}

fn delete_backend(app: &tauri::AppHandle, id: &str) -> Result<(), String> {
    let backend = crate::bot::config::keyring::key_backend();
    let file = fallback_file(app);
    let _g = (backend == KeyBackend::PlaintextFile).then_some(fallback_lock_guard());
    delete_backend_at(backend, &file, id)
}

/// 后端注入内核（幂等：条目不存在 = Ok）
fn delete_backend_at(backend: KeyBackend, file: &std::path::Path, id: &str) -> Result<(), String> {
    match backend {
        KeyBackend::System => match keyring_entry(id) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) => Ok(()),
                Err(::keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(format!("清除机密失败：{e}")),
            },
            Err(e) => Err(e),
        },
        KeyBackend::PlaintextFile => {
            // NotFound = 文件从未落盘，按「已清」返回（幂等：purge 未写过的 id
            // 不应报错，与 System 后端 NoEntry = Ok 口径一致）；其余 I/O 错误仍
            // 传播——损坏文件不能按「已清」误判
            let raw = match std::fs::read_to_string(file) {
                Ok(s) => s,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(format!("读取降级机密文件失败：{e}")),
            };
            let mut map: std::collections::HashMap<String, McpSecrets> =
                serde_json::from_str(&raw).map_err(|e| format!("降级机密文件解析失败：{e}"))?;
            if map.remove(id).is_none() {
                return Ok(());
            }
            let out = serde_json::to_string_pretty(&map).map_err(|e| format!("序列化失败：{e}"))?;
            let tmp = file.with_extension("tmp");
            // 评审 HIGH 采纳：0600 创建（rename 换 inode，0644 的 tmp 会让
            // 目标文件退化成全局可读）
            write_tmp_0600(&tmp, out.as_bytes())?;
            std::fs::rename(&tmp, file).map_err(|e| format!("落盘失败：{e}"))?;
            Ok(())
        }
    }
}

/// tmp 文件 0600 写入（创建时即 0600，无「先 0644 后 chmod」窗口）
fn write_tmp_0600(tmp: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(tmp)
            .map_err(|e| format!("降级机密文件写入失败：{e}"))?;
        f.write_all(bytes)
            .map_err(|e| format!("降级机密文件写入失败：{e}"))?;
        // mode() 仅新建时生效：tmp 若是上次崩溃残留的 0644 文件，复用 inode 会
        // 带着旧权限走到 rename——落盘前无条件收紧一次
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("降级机密文件权限收紧失败：{e}"))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(tmp, bytes).map_err(|e| format!("降级机密文件写入失败：{e}"))
    }
}

// 对外 API（hydrate / save / delete / purge）

/// 读路径水合：把 keyring 里的机密回填进内存配置（config 文件里没有）。
/// 读失败 → 该服务器 env/headers 留空 + stderr WARN（连接会失败、状态点红
/// 可见——**绝不炸配置加载**）。
pub(crate) fn hydrate_mcp_servers(app: &tauri::AppHandle, cfg: &mut crate::bot::BotConfig) {
    let Some(servers) = cfg.mcp_servers.as_mut() else {
        return;
    };
    for s in servers.iter_mut() {
        if s.env.is_empty() && s.headers.is_empty() {
            // 文件里本就没值：可能服务器本来无机密，也可能还没水合过——
            // 查一次缓存（未读过才敲后端），有值就回填
            match cache_get(&s.id) {
                Some(Some(secrets)) => {
                    s.env = secrets.env;
                    s.headers = secrets.headers;
                }
                Some(None) => {}
                None => match read_backend(app, &s.id) {
                    Ok(Some(secrets)) if !secrets.is_empty() => {
                        s.env = secrets.env.clone();
                        s.headers = secrets.headers.clone();
                        // 查重与写入在同一把锁内完成（check-then-act 原子化）：
                        // 键已存在（含并发 store 刚写入的新值）则不覆盖，
                        // 杜绝本线程读到的旧值把新值冲掉
                        let mut g = cache().lock().unwrap_or_else(|e| {
                            eprintln!("[mutex_poisoned] mcp::secrets cache: {e:?}");
                            e.into_inner()
                        });
                        g.entry(s.id.clone()).or_insert(Some(secrets));
                    }
                    Ok(_) => cache_put(&s.id, None),
                    Err(e) => {
                        // 评审 HIGH 采纳：读取故障**不缓存**——缓存 None 会把
                        // 瞬时故障固化成永久丢机密；不缓存 = 下次 load 重试
                        eprintln!("[mcp] 机密读取失败（id: {}，下次重试）：{e}", s.id);
                    }
                },
            }
        }
    }
}

/// 保存命令落 blob（先 keyring 后配置的关键一步）。空机密 = 清除条目。
/// 返回 Err 时调用方整体报错、配置不动。
pub(crate) fn store_server_secrets(
    app: &tauri::AppHandle,
    id: &str,
    env: &BTreeMap<String, String>,
    headers: &BTreeMap<String, String>,
) -> Result<(), String> {
    let secrets = McpSecrets {
        env: env.clone(),
        headers: headers.clone(),
    };
    if secrets.is_empty() {
        delete_backend(app, id)?;
        cache_put(id, None);
        return Ok(());
    }
    let blob = secrets.to_blob()?;
    write_backend(app, id, &blob)?;
    cache_put(id, Some(secrets));
    Ok(())
}

/// 删除命令清 blob（幂等；失败由调用方 WARN——配置删除已成功，孤儿条目可追溯）
pub(crate) fn purge_server_secrets(app: &tauri::AppHandle, id: &str) -> Result<(), String> {
    let r = delete_backend(app, id);
    cache_remove(id);
    r
}

/// 迁移前探测：blob 是否已在（幂等短路用）。阶段 3 迁移链路预埋口，暂无调用方。
#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> McpSecrets {
        McpSecrets {
            env: BTreeMap::from([("HOME".into(), "/tmp/x".into())]),
            headers: BTreeMap::from([("Authorization".into(), "Bearer tok".into())]),
        }
    }

    // ── 纯内核：blob serde 往返 / 体积上限 ──

    #[test]
    fn blob_serde_roundtrip() {
        let blob = secrets().to_blob().unwrap();
        let back = McpSecrets::from_blob(&blob).unwrap();
        assert_eq!(back, secrets());
    }

    #[test]
    fn blob_size_cap_enforced() {
        let big = McpSecrets {
            env: BTreeMap::from([("K".into(), "v".repeat(MCP_SECRET_BLOB_MAX_BYTES))]),
            headers: BTreeMap::new(),
        };
        assert!(
            big.to_blob().is_err(),
            "超限 blob 必须响亮报错（Windows 2560 上限）"
        );
    }

    // ── config.rs 判据与剥离 ──

    #[test]
    fn has_inline_secrets_detects_either_map() {
        use crate::bot::mcp::config::has_inline_secrets;
        let mut s = crate::bot::mcp::config::McpServerConfig::default();
        assert!(!has_inline_secrets(&s));
        s.env.insert("K".into(), "v".into());
        assert!(has_inline_secrets(&s));
        let mut s2 = crate::bot::mcp::config::McpServerConfig::default();
        s2.headers.insert("H".into(), "v".into());
        assert!(has_inline_secrets(&s2));
    }

    #[test]
    fn server_serialization_never_emits_secrets() {
        //  核心不变式：skip_serializing 使任何写路径都不可能把明文写回盘
        let s = crate::bot::mcp::config::McpServerConfig {
            id: "srv1".into(),
            name: "fs".into(),
            transport: "stdio".into(),
            command: Some("npx".into()),
            env: BTreeMap::from([("TOKEN".into(), "secret-value".into())]),
            headers: BTreeMap::from([("Authorization".into(), "Bearer x".into())]),
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            !json.contains("secret-value"),
            "env 值不得出现在序列化输出：{json}"
        );
        assert!(
            !json.contains("Bearer x"),
            "headers 值不得出现在序列化输出：{json}"
        );
        assert!(!json.contains("\"env\""), "env 键也不应出现：{json}");
        // 读侧仍可反序列化（认老配置：手补 env 字段再解析）
        let legacy = r#"{"id":"srv1","name":"fs","transport":"stdio","env":{"TOKEN":"legacy"}}"#;
        let back: crate::bot::mcp::config::McpServerConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.env.get("TOKEN").map(String::as_str), Some("legacy"));
    }

    // ── 降级后端（PlaintextFile + 临时文件，不碰真实钥匙串）──

    #[test]
    fn plaintext_store_write_read_delete_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bot-mcp-secrets.json");
        let backend = KeyBackend::PlaintextFile;
        // 读缺失文件 = None
        assert_eq!(read_backend_at(backend, &file, "s1").unwrap(), None);
        // 写 → 读回
        let blob = secrets().to_blob().unwrap();
        write_backend_at(backend, &file, "s1", &blob).unwrap();
        assert_eq!(
            read_backend_at(backend, &file, "s1").unwrap(),
            Some(secrets())
        );
        // 0600（unix）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "降级机密文件必须 0600");
        }
        // 删除 → 读回 None（幂等再删 = Ok）
        delete_backend_at(backend, &file, "s1").unwrap();
        assert_eq!(read_backend_at(backend, &file, "s1").unwrap(), None);
        delete_backend_at(backend, &file, "s1").unwrap();
    }

    #[test]
    fn plaintext_store_corrupt_file_is_err_not_crash() {
        // 损坏文件读 = Err（hydrate 层转空 + WARN，不炸配置加载）
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bot-mcp-secrets.json");
        std::fs::write(&file, "{ broken").unwrap();
        assert!(read_backend_at(KeyBackend::PlaintextFile, &file, "s1").is_err());
    }
}
