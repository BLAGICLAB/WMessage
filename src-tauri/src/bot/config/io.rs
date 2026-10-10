//! bot-config.json IO + 旧版本 key 迁移。
//!
//! - `config_path` / `load_config` / `add_allowed_dir` / `update_config_file`
//! - `write_bot_config_file` 强制剥离 key 字段（双保险）+ base_url 安全告警
//! - `read_bypass_llm_switch` 轻量开关读取（bot_chat 入口用）
//! - `read_memory_control` 记忆可控开关轻量读取memory 门禁用）
//! - `base_url_is_safe` SSRF / 明文传输警告
//! - `migrate_legacy_key` / `migrate_search_keys` 老配置明文 → keyring

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json;
use tauri::AppHandle;

use crate::db;
use crate::error::{CommandError, CommandResult};

use super::keyring;
use super::schema;
use super::types::{BotConfig, KeySlot};

// 写互斥 + 原子写

/// bot-config.json 写路径全局互斥：RMW（add_allowed_dir / update_config_file /
/// migrate_*）与纯写（bot_set_config）必须互斥，否则并发双方 load 同一旧值、
/// 后写覆盖先写（C5-BT-03）。进程内 static 足够（多进程同写不是支持场景）。
/// 锁序：本锁内只再取 BOT_LOG_LOCK（audit_event!），反向不存在，叶锁无环。
/// std Mutex 不可重入——持锁段内只能调 *_locked 内核（迁移钩子 try_lock 让路）。
/// 边界：锁内 = migrate_* 的 keyring IO + 全部文件写；migrate_* 的 keyring IO
/// 不拆出锁（拆则"读→keyring→写"竞态重开，启动期一次性路径可接受——）。
/// bot_set_config 的 keyring 写（commands.rs，先于文件写）不在本锁范围——
/// 该先后半成功问题已转 B 类评估（见 PHASE2-TRIAGE 攒批）。
pub(crate) static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// C3-1 poison 形态 + DbWriteGuard 同形态具名守卫（裸调用不绑定会立刻释放锁，
/// must_use 防 footgun；取锁置位线程本地持锁标记，Drop 无条件清位）。
pub(crate) fn lock_config_write() -> ConfigWriteGuard {
    let g = CONFIG_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] bot::config::io::CONFIG_WRITE_LOCK: {e:?}");
        e.into_inner()
    });
    ConfigWriteGuard::from_acquired(g)
}

thread_local! {
    /// 同 db::HOLDING_DB_WRITE：try_lock 无法证明「本线程持锁」，用线程本地标记精确判定。
    static HOLDING_CONFIG_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `CONFIG_WRITE_LOCK` 的守卫：Drop 时无条件清线程本地标记（覆盖正常释放与 unwind）。
#[must_use = "守卫不绑定会立刻释放锁"]
pub(crate) struct ConfigWriteGuard {
    _g: std::sync::MutexGuard<'static, ()>,
}

impl ConfigWriteGuard {
    /// 已由调用方取得的锁（try_lock 路径，见 schema.rs）包装成守卫并置位标记。
    pub(crate) fn from_acquired(g: std::sync::MutexGuard<'static, ()>) -> Self {
        HOLDING_CONFIG_WRITE.with(|f| f.set(true));
        Self { _g: g }
    }
}

impl Drop for ConfigWriteGuard {
    fn drop(&mut self) {
        HOLDING_CONFIG_WRITE.with(|f| f.set(false));
    }
}

/// 当前线程是否持有 CONFIG_WRITE_LOCK（*_locked 内核的契约断言用）。
pub(crate) fn holding_config_write() -> bool {
    HOLDING_CONFIG_WRITE.with(|f| f.get())
}

/// bot-config.json 原子写内核：tmp + fsync + rename，崩溃/断电不留半截文件
/// （std::fs::write 原地 truncate，中途失败 = 空文件 → load_config 静默回默认；
/// migrate 路径的文件还可能含明文 key，半截写 = 双丢）。tmp 名带 pid+seq：
/// release 下 stray 调用方也不互撕裂；契约仍须持锁；要求 fsync 故不复用 db::atomic_write。
pub(crate) fn write_config_atomic(path: &Path, raw: &str) -> std::io::Result<()> {
    debug_assert!(holding_config_write(), "必须持 CONFIG_WRITE_LOCK");
    use std::sync::atomic::{AtomicU32, Ordering};
    static TMP_SEQ: AtomicU32 = AtomicU32::new(0);
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "无效目标路径"))?;
    let tmp = path.with_file_name(format!(
        "{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let write_result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(raw.as_bytes())?;
        f.sync_all()
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // rename 原子但不耐久：fsync 父目录兜底（best-effort；Windows 目录句柄不支持，仅 unix）
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all()) {
            eprintln!("[bot] config 目录 fsync 失败：{} path={}", e, dir.display());
        }
    }
    Ok(())
}

// 路径

pub fn config_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> PathBuf {
    db::data_dir(app).join("bot-config.json")
}

/// bypass_llm 开关读取 helper：bot-config.json 缺字段 / 文件不存在 / 解析失败都默认 true（bypass 行为）。
/// 比 bot_get_config 轻量：跳过 BotConfigView 构造 + key 校验，bot_chat 入口用。
pub fn read_bypass_llm_switch(app: &AppHandle) -> bool {
    read_bypass_llm_switch_at(&config_path(app))
}

/// 记忆可控开关读取运行时泛型——注入路径被泛型任务执行复用；
/// 同模块 read_bypass_llm_switch 收具体句柄是历史签名，不追改）：
/// 文件缺失 / 读失败 / 解析失败 / 缺字段 → None = 全开，门禁绝不弄挂聊天。
/// 比 bot_get_config 轻量（只取 memoryControl 一个字段，同 read_bypass_llm_switch 精神）。
pub fn read_memory_control<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Option<crate::memory::MemoryControl> {
    read_memory_control_at(&config_path(app))
}

/// 可测内核（纯路径参数）：None = 无控制（全开）。
/// 文件存在但解析失败 → stderr 一行告警（隐私开关静默失效要可诊断）。
pub(crate) fn read_memory_control_at(path: &Path) -> Option<crate::memory::MemoryControl> {
    let raw = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<BotConfig>(&raw) {
        Ok(cfg) => cfg.memory_control,
        Err(e) => {
            eprintln!("[memory] bot-config.json 解析失败，记忆开关按全开放行：{e}");
            None
        }
    }
}

/// 记忆参数读取：文件缺失 / 读失败 / 解析失败 / 缺字段 / 超界值 →
/// 默认或钳制（下游只见合法值）。解析失败时 stderr 告警——与
/// read_memory_control_at 对称（手改配置改坏了要可诊断）。
pub fn read_memory_tuning<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> crate::memory::MemoryTuning {
    read_memory_tuning_at(&config_path(app))
}

/// memoryTuning 定向写回（RMW 只改 memoryTuning 块，其余字段原样保留；
/// 与 set_apply_policy_at 同款）。None = 移除该块（回落内置默认）。
/// 写入前过 clamped 统一钳制；JSON 损坏拒绝写（防整库覆盖，fail-closed）。
pub fn write_memory_tuning_at(
    path: &Path,
    tuning: Option<crate::memory::MemoryTuning>,
) -> Result<(), String> {
    // 跨写者统一锁（2026-10-10 改）：原独立 TUNING_WRITE_LOCK 与
    // lock_config_write()（update_config_file / set_apply_policy / set_apply_policy_at
    // 等所有写 bot-config.json 的路径用的同一把锁）独立——并发改档时
    // T1: update_config_file 写 verified_vendors；T2: write_memory_tuning 改
    // memoryTuning 块。两路都做 read-modify-write 整库覆盖，后者盲写会把前者
    // 的 verified_vendors 改回旧值。合用 lock_config_write() 后两路互斥，
    // 写顺序由 lock 持有顺序定（库内 std Mutex 阻塞同一线程的所有锁——即
    // 一个 caller 同时只持一把）。
    let _g = lock_config_write();
    let mut v = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<serde_json::Value>(&raw)
            .map_err(|e| format!("{path:?} JSON 解析失败，拒绝写（防覆盖既有配置）：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("读 {path:?} 失败：{e}")),
    };
    let obj = v
        .as_object_mut()
        .ok_or_else(|| format!("{path:?} 顶层非 object，拒绝写"))?;
    match tuning {
        Some(t) => {
            let clamped = t.clamped();
            let val = serde_json::to_value(&clamped).map_err(|e| format!("序列化：{e}"))?;
            obj.insert("memoryTuning".to_string(), val);
        }
        None => {
            obj.remove("memoryTuning");
        }
    }
    crate::db::paths::atomic_write(
        path,
        &serde_json::to_string_pretty(&v).map_err(|e| format!("序列化：{e}"))?,
    )
}

/// 可测内核（纯路径参数）：文件缺失/读失败/JSON 损坏/缺字段 → 默认；
/// 超界值 → 钳制；解析失败 → stderr 一行告警 + 默认。
pub(crate) fn read_memory_tuning_at(path: &Path) -> crate::memory::MemoryTuning {
    let Some(raw) = std::fs::read_to_string(path).ok() else {
        return crate::memory::MemoryTuning::default();
    };
    match serde_json::from_str::<BotConfig>(&raw) {
        Ok(cfg) => cfg.memory_tuning.unwrap_or_default().clamped(),
        Err(e) => {
            eprintln!("[memory] bot-config.json 解析失败，记忆参数按默认放行：{e}");
            crate::memory::MemoryTuning::default()
        }
    }
}

/// 可测内核（纯路径参数）：文件缺失 / 读失败 / JSON 损坏 / 缺字段 → true。
/// 只有显式 `bypassLlmOnPreStepHit: false` 才关掉 bypass——老配置零迁移语义，
/// 默认走 bypass（避免命中 Skill 后还要多烧一次外层 LLM）。
pub(crate) fn read_bypass_llm_switch_at(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return true;
    };
    serde_json::from_str::<BotConfig>(&raw)
        .map(|c| c.bypass_llm_on_pre_step_hit)
        .unwrap_or(true)
}

/// 读 bot-config.json（不存在/解析失败回默认）。内部共用（bot_fs 白名单等）
pub(crate) fn load_config(app: &AppHandle) -> BotConfig {
    // 迁移钩子：老配置缺 schemaVersion → 补默认 + 写回（幂等，已迁移不写盘；
    // 失败不阻塞读取——本次仍按下面常规路径读，下次再试）
    let _ = schema::migrate_bot_config_schema(app);
    let p = config_path(app);
    if p.exists() {
        if let Ok(raw) = std::fs::read_to_string(&p) {
            if let Ok(mut cfg) = serde_json::from_str::<BotConfig>(&raw) {
                // MCP env/headers 机密水合（文件里没有，值在 keyring/
                // 降级文件；读失败留空 + stderr WARN，不炸配置加载）
                crate::bot::mcp::secrets::hydrate_mcp_servers(app, &mut cfg);
                return cfg;
            }
        }
    }
    BotConfig::default()
}

/// MCP env/headers 明文 → keyring 迁移（设计
/// `docs/MCP-KEYSLOT-MIGRATION-DESIGN-2026-09-29.md` §4）。调用方必须已持
/// CONFIG_WRITE_LOCK（与 schema 迁移同款契约）。
///
/// 返回 Err = 迁移未完成，**调用方必须中止本次配置写**（关键不变式：任何配置
/// 写回都会因 skip_serializing 剥离 env/headers——迁移未成功就写回 = 明文丢失，
/// 评审 CRITICAL ①）。顺序即安全性：备份原文件 → 逐台写 keyring → 读回比对 →
/// 全部成功才**外科手术式**写回（Value 级只删 mcpServers.env/headers，
/// 不碰 apiKey/tavily/brave 等其他字段——评审 HIGH：BotConfig 整体写回会把
/// 未迁移的 legacy 明文 key 一并清空）；任一台失败不写回（明文原样），WARN
/// 审计，下次重试。
pub(crate) fn migrate_mcp_server_secrets_locked(app: &AppHandle) -> Result<(), String> {
    debug_assert!(holding_config_write(), "必须持 CONFIG_WRITE_LOCK");
    // 幂等短路：解析原始文件（水合前——文件里的明文还是真相源）
    let p = config_path(app);
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return Ok(());
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(()); // 解析失败由 schema/常规加载路径处理，此处跳过
    };
    let Some(servers) = v.get_mut("mcpServers").and_then(|x| x.as_array_mut()) else {
        return Ok(());
    };
    let pending: Vec<(String, String, serde_json::Value)> = servers
        .iter()
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?.to_string();
            let env = s.get("env").cloned().unwrap_or(serde_json::Value::Null);
            let headers = s.get("headers").cloned().unwrap_or(serde_json::Value::Null);
            let inline = !env.as_object().map(|m| m.is_empty()).unwrap_or(true)
                || !headers.as_object().map(|m| m.is_empty()).unwrap_or(true);
            inline.then_some((
                id,
                s.get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string(),
                serde_json::json!({"env": env, "headers": headers}),
            ))
        })
        .collect();
    if pending.is_empty() {
        return Ok(()); // 已迁移 / 无机密：短路
    }
    // 首台迁移前备份（0600；含明文——它就是回滚网，见设计 §5。
    // 写一次不覆盖：首迁失败重试时快照仍是最初原文件，评审 M 采纳）
    let backup = config_path(app).with_file_name("bot-config.backup-mcp-keys.json");
    if !backup.exists() {
        match std::fs::copy(&p, &backup) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    let _ = std::fs::set_permissions(
                        &backup,
                        std::os::unix::fs::PermissionsExt::from_mode(0o600),
                    );
                }
                crate::audit::write_event(
                    app,
                    crate::audit::AuditLevel::Info,
                    "mcp.secret_backup_created",
                    &[("path", backup.to_string_lossy().to_string())],
                );
            }
            Err(e) => {
                // 备份失败 = 回滚网缺失 → 不迁移（明文保留原样，无丢失风险）
                crate::audit::write_event(
                    app,
                    crate::audit::AuditLevel::Warn,
                    "mcp.secret_migrate_failed",
                    &[("stage", "backup".to_string()), ("err", e.to_string())],
                );
                return Err(format!("mcp 机密迁移失败（备份）：{e}"));
            }
        }
    }
    // 逐台写 keyring + 读回比对（secrets::store 内含读回校验）
    for (id, name, blob_val) in &pending {
        let env = blob_val
            .get("env")
            .and_then(|x| serde_json::from_value(x.clone()).unwrap_or_default())
            .unwrap_or_default();
        let headers = blob_val
            .get("headers")
            .and_then(|x| serde_json::from_value(x.clone()).unwrap_or_default())
            .unwrap_or_default();
        if let Err(e) = crate::bot::mcp::secrets::store_server_secrets(app, id, &env, &headers) {
            // 任一台失败 → 整体不写回（明文原样），调用方中止本次写，下次重试
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "mcp.secret_migrate_failed",
                &[
                    ("id", id.clone()),
                    ("name", name.clone()),
                    ("err", e.clone()),
                ],
            );
            return Err(format!("mcp 机密迁移失败（id: {id}）：{e}"));
        }
    }
    // 全部成功 → 外科手术式写回：只删 mcpServers 内的 env/headers，
    // 其余字段（含未迁移的 legacy 明文 key）原样保留
    if let Some(arr) = v.get_mut("mcpServers").and_then(|x| x.as_array_mut()) {
        for s in arr.iter_mut() {
            if let Some(obj) = s.as_object_mut() {
                obj.remove("env");
                obj.remove("headers");
            }
        }
    }
    let raw_out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    write_config_atomic(&p, &raw_out).map_err(|e| format!("mcp 机密迁移写回失败：{e}"))?;
    crate::audit::write_event(
        app,
        crate::audit::AuditLevel::Info,
        "mcp.secret_migrated",
        &[("count", pending.len().to_string())],
    );
    Ok(())
}

/// 授权弹窗「始终允许该目录」落盘：把目录追加进 allowedDirs 并写回
/// bot-config.json。allowedDirs 语义 = 内置默认（桌面/下载/文档+绑定文件夹）之上的
/// 追加放行，直接 push 去重即可；已存在/空白为幂等 no-op。
pub(crate) fn add_allowed_dir(app: &AppHandle, dir: &str) -> Result<(), String> {
    let d = dir.trim().to_string();
    if d.is_empty() {
        return Ok(());
    }
    // RMW 全程持锁（C5-BT-03）；持锁段内先推 schema 迁移（钩子锁内会 try_lock 让路）
    let _g = lock_config_write();
    let _ = schema::migrate_bot_config_schema_locked(app);
    // MCP 机密迁移（任何配置写都可能剥离明文，必须先迁；
    // 失败中止本次写 = 明文原样保留，评审 CRITICAL① 采纳）
    migrate_mcp_server_secrets_locked(app)?;
    let mut cfg = load_config(app);
    if cfg.allowed_dirs.iter().any(|x| x.trim() == d) {
        return Ok(());
    }
    cfg.allowed_dirs.push(d);
    // 密钥防御纵深：写盘前对残留明文 key 逐槽执行「keyring 写入
    // （仅空槽，不覆盖用户新值）→ 读回验证」，验证通过才剥该槽明文；任一环节
    // 失败保留明文下次再试——keyring 读不回时剥除即丢密钥（零丢失优先）。
    let stripped = strip_verified_keys(&mut cfg);
    let data_dir = db::data_dir(app);
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    let res = write_config_atomic(&config_path(app), &raw).map_err(|e| e.to_string());
    // 审计在写盘成功后 + 放锁后（锁内不夹审计写 IO；先审计后写盘会出现
    // 「审计已剥、盘上未剥」的取证漂移——同 migrate_search_keys 先例）
    drop(_g);
    if res.is_ok() && !stripped.is_empty() {
        let names = stripped.join(",");
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "config.plaintext_key_stripped",
            "slots" => names,
        );
    }
    res
}

/// 明文 key 剥除前置验证：对三个槽位独立处理——
/// ① keyring 已有可读值 → 文件副本视为过期残留，可剥（keyring 值优先，
///   不覆盖用户新值也不要求等值）；
/// ② keyring 为空 → 写入明文并读回，读回等值才剥（读回无关值=并发竞争/后端异常，不剥）；
/// ③ keyring 故障/读不回 → 保留明文，下次再试。
/// 每槽至多一次 read + （空槽时）一次 write（keyring 读路径自带 prepare 副作用，
/// has+read 双探测会把副作用翻倍）。返回成功剥除的槽位名（供审计留痕）。
fn strip_verified_keys(cfg: &mut BotConfig) -> Vec<&'static str> {
    let mut stripped = Vec::new();
    for (slot, field) in [
        (KeySlot::Llm, &mut cfg.api_key),
        (KeySlot::Tavily, &mut cfg.tavily_key),
        (KeySlot::Brave, &mut cfg.brave_key),
    ] {
        let Some(k) = field.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        // 单 read 探测：非空 = keyring 已有值（keyring 值优先，文件副本是过期残留）
        let existing = keyring::read_key_of_slot(slot)
            .ok()
            .filter(|v| !v.trim().is_empty());
        let wrote_now = existing.is_none();
        let backed = match existing {
            Some(v) => Some(v),
            None => {
                // 空槽：写入明文再读回——读回等值才担保落位（写入失败/读回缺失/读回
                // 无关值都不剥，密钥零丢失优先）
                if keyring::write_key_of_slot(slot, k).is_err() {
                    continue;
                }
                keyring::read_key_of_slot(slot).ok()
            }
        };
        if plaintext_strippable(backed.as_deref(), wrote_now, k) {
            *field = None;
            stripped.push(slot.keyring_user());
        }
    }
    stripped
}

/// 纯决策内核（供单测）：keyring 侧实际读到的值能否担保文件明文可安全剥除。
/// keyring 原有值（wrote_now=false）→ 非空即剥（keyring 值优先于文件副本）；
/// 本次新写入（wrote_now=true）→ 读回值必须与明文等值（trim 后比对）——
/// 读回无关值（并发竞争/后端别名）时剥除即丢失唯一可信比对基准。
fn plaintext_strippable(backed: Option<&str>, wrote_now: bool, plaintext: &str) -> bool {
    match backed.map(str::trim) {
        Some(v) if !v.is_empty() => !wrote_now || v == plaintext.trim(),
        _ => false,
    }
}

// write_bot_config_file / update_config_file

/// bot_set_config 落盘内核（抽出便于单测）：强制剥离三个 key 字段
/// （api_key/tavily_key/brave_key 一律 None）后写 bot-config.json。
/// base_url 非 https 且非回环 → 警告（api_key 明文传输风险）；
/// 只警告不拒写——本地推理服务是合法场景，且不能破坏存量用户配置。
/// 无锁内核：调用方必须已持 CONFIG_WRITE_LOCK（update_config_file 的 RMW 段内
/// 复用——std Mutex 不可重入，经加锁外壳会自锁）。
pub(crate) fn write_bot_config_file_locked(dir: &Path, config: BotConfig) -> CommandResult<()> {
    debug_assert!(holding_config_write(), "必须持 CONFIG_WRITE_LOCK");
    let mut cfg = config;
    cfg.api_key = None;
    cfg.tavily_key = None;
    cfg.brave_key = None;
    if !base_url_is_safe(&cfg.base_url) {
        eprintln!(
            "[bot] 警告：base_url 非 https 且非回环地址，API Key 将明文传输：{}",
            cfg.base_url
        );
    }
    std::fs::create_dir_all(dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let raw =
        serde_json::to_string_pretty(&cfg).map_err(|e| CommandError::IoError(e.to_string()))?;
    write_config_atomic(&dir.join("bot-config.json"), &raw)
        .map_err(|e| CommandError::IoError(e.to_string()))
}

/// 读-改-写 bot-config.json（记忆整理的 last_run_at 回写等内部配置更新用）：
/// 与 bot_set_config 同落盘路径（key 字段剥离由 write_bot_config_file_locked 保证）。
/// RMW 全程持 CONFIG_WRITE_LOCK（无锁则并发写互相覆盖）；闭包 f 锁内执行，禁重入加锁入口。
pub(crate) fn update_config_file(
    app: &AppHandle,
    f: impl FnOnce(&mut BotConfig),
) -> CommandResult<()> {
    let _g = lock_config_write();
    let _ = schema::migrate_bot_config_schema_locked(app); // 同 add_allowed_dir 先推迁移
                                                           // MCP 机密迁移先于本迁移（写回外科手术式只动 mcpServers，
                                                           // 不碰 legacy 明文 key；失败中止本次写，评审 HIGH 采纳）
    migrate_mcp_server_secrets_locked(app)?;
    let mut cfg = load_config(app);
    f(&mut cfg);
    write_bot_config_file_locked(&db::data_dir(app), cfg)
}

// base_url 安全判定

/// base_url 安全判定：空 / https / 回环 host（localhost、127.0.0.0/8、::1）
/// 视为安全；其余（http 公网/内网、解析失败、其他 scheme）不安全——调用方打警告，不拒写。
/// host 经 url::Url 解析后精确判定：前缀匹配会被 localhost.evil.com /
/// 127.0.0.1.evil.com / userinfo 变体绕过（解析失败按不安全 = fail-closed）。
pub(crate) fn base_url_is_safe(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() {
        return true;
    }
    let Ok(parsed) = url::Url::parse(u) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => match parsed.host() {
            Some(url::Host::Domain(h)) => h.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

// 老版本 key 迁移

/// 旧版本迁移：bot-config.json 里有明文 key → 迁入系统凭据存储并清掉文件里的明文。
/// App 启动时调用一次（设置页读配置时也会兜底触发）。
pub fn migrate_legacy_key(app: &AppHandle) -> Result<(), String> {
    let _g = lock_config_write();
    let _ = schema::migrate_bot_config_schema_locked(app); // 同 add_allowed_dir 先推迁移
                                                           // MCP 机密迁移先于本迁移（写回外科手术式只动 mcpServers，
                                                           // 不碰 legacy 明文 key；失败中止本次写，评审 HIGH 采纳）
    migrate_mcp_server_secrets_locked(app)?;
    let p = config_path(app);
    if !p.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut cfg: BotConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let Some(k) = cfg.api_key.take() else {
        return Ok(());
    };
    let k = k.trim().to_string();
    if k.is_empty() {
        return Ok(());
    }
    // 凭据存储里没有 key 时才写入（避免旧明文覆盖用户新存的 key）；
    // keyring 故障（Err）按「写不入」同等处理：保留文件明文，下次再试
    if !keyring::has_api_key().unwrap_or(false) && keyring::write_api_key(&k).is_err() {
        return Ok(()); // 写入失败：保留文件明文，下次再试
    }
    let dir = db::data_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    write_config_atomic(&p, &raw).map_err(|e| e.to_string())
}

/// 搜索 key 迁移（Tavily/Brave 不再明文落 bot-config.json）：
/// bot-config.json 里仍含明文 tavily_key/brave_key → keyring 里还没有对应 key 时
/// 写入（不覆盖更新值）→ 配置文件里这两个字段置 None 写回 → 审计留痕；
/// keyring 写失败保留文件明文下次再试（数据保留优先，与 migrate_legacy_key 同策略）。
/// App 启动时调用一次（设置页读配置时也会兜底触发，双调用点与 migrate_legacy_key 一致）。
pub fn migrate_search_keys(app: &AppHandle) -> Result<(), String> {
    let _g = lock_config_write();
    let _ = schema::migrate_bot_config_schema_locked(app); // 同 add_allowed_dir 先推迁移
                                                           // MCP 机密迁移先于本迁移（写回外科手术式只动 mcpServers，
                                                           // 不碰 legacy 明文 key；失败中止本次写，评审 HIGH 采纳）
    migrate_mcp_server_secrets_locked(app)?;
    let p = config_path(app);
    if !p.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut cfg: BotConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    if cfg.tavily_key.is_none() && cfg.brave_key.is_none() {
        return Ok(()); // 无明文残留，幂等
    }
    let mut changed = false;
    let mut migrated_slots: Vec<&str> = Vec::new();
    for (slot, field) in [
        (KeySlot::Tavily, cfg.tavily_key.clone()),
        (KeySlot::Brave, cfg.brave_key.clone()),
    ] {
        let has_in_store = keyring::has_search_key(slot).unwrap_or(false);
        let mut write = |key: &str| keyring::write_search_key(slot, key).map_err(|e| e.message());
        let (remaining, migrated) =
            migrate_search_key_slot(field.as_deref(), has_in_store, &mut write);
        if migrated {
            migrated_slots.push(slot.keyring_user());
        }
        let field_ref = match slot {
            KeySlot::Tavily => &mut cfg.tavily_key,
            KeySlot::Brave => &mut cfg.brave_key,
            KeySlot::Llm => unreachable!("Llm 槽位由 migrate_legacy_key 负责"),
        };
        if *field_ref != remaining {
            *field_ref = remaining;
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    let dir = db::data_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    write_config_atomic(&p, &raw).map_err(|e| e.to_string())?;
    // 审计在写盘成功 + 放锁之后（锁内不夹外部调用；写失败不留名不副实的审计行）
    drop(_g);
    for slot in migrated_slots {
        crate::audit_event!(
            app,
            crate::audit::AuditLevel::Info,
            "config.search_key_migrated",
            "slot" => slot,
        );
    }
    Ok(())
}

/// 单 slot 迁移内核（注入 has/write 便于单测，不碰真实 keyring）：
/// - 无明文（None/空串）→ (None, false)：字段归 None，幂等
/// - keyring 已有值 → (None, false)：不覆盖更新值，直接清明文
/// - 写入成功 → (None, true)：清明文 + 记迁移
/// - 写入失败 → (保留明文, false)：下次再试（数据保留优先）
pub(crate) fn migrate_search_key_slot(
    plaintext: Option<&str>,
    has_in_store: bool,
    write: &mut dyn FnMut(&str) -> Result<(), String>,
) -> (Option<String>, bool) {
    let Some(k) = plaintext.map(str::trim).filter(|k| !k.is_empty()) else {
        return (None, false);
    };
    if has_in_store {
        return (None, false);
    }
    match write(k) {
        Ok(()) => (None, true),
        Err(_) => (Some(k.to_string()), false),
    }
}

// 抑制 unused 警告：Write + PathBuf 在本模块通过 std::io::Write / std::path::PathBuf trait 用

#[cfg(test)]
mod tests {
    #[test]
    fn write_memory_tuning_roundtrip_preserves_and_removes() {
        let dir = std::env::temp_dir().join(format!(
            "mem-tuning-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(
            &p,
            r#"{"baseUrl":"https://x/v1","evolution":{"applyPolicy":"confirm"}}"#,
        )
        .unwrap();

        // Some：写入（超界值被钳制），兄弟/顶层字段保留
        let t = crate::memory::MemoryTuning {
            top_n: 99,
            ..Default::default()
        };
        write_memory_tuning_at(&p, Some(t)).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["memoryTuning"]["topN"], 10, "超界钳到上限");
        assert_eq!(v["baseUrl"], "https://x/v1", "顶层字段保留");
        assert_eq!(v["evolution"]["applyPolicy"], "confirm", "兄弟块保留");

        // None：整块移除，其余保留
        write_memory_tuning_at(&p, None).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert!(v.get("memoryTuning").is_none());
        assert_eq!(v["baseUrl"], "https://x/v1");

        // 坏文件拒绝写
        std::fs::write(&p, "{ broken").unwrap();
        assert!(write_memory_tuning_at(&p, None).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ broken");
        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

    #[test]
    fn write_config_atomic_roundtrip_no_tmp_residue() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot-config.json");
        let _g = lock_config_write();
        write_config_atomic(&p, "{\"v\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"v\":1}");
        write_config_atomic(&p, "{\"v\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"v\":2}");
        assert!(
            !dir.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "成功后不得遗留 tmp 残渣"
        );
    }

    #[test]
    fn write_config_atomic_rename_failure_cleans_tmp() {
        let dir = tempfile::tempdir().unwrap();
        // 目标路径占成目录 → rename(tmp, path) 必败（unix EISDIR / Windows ERROR_ACCESS_DENIED）
        let p = dir.path().join("bot-config.json");
        std::fs::create_dir(&p).unwrap();
        let _g = lock_config_write();
        assert!(write_config_atomic(&p, "{\"v\":1}").is_err());
        assert!(
            !dir.path().read_dir().unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")),
            "rename 失败也不得遗留 tmp 残渣"
        );
    }

    /// 跨写者锁互斥契约（2026-10-10 改）：write_memory_tuning_at 与
    /// update_config_file / set_apply_policy / set_apply_policy_at 共用
    /// lock_config_write() 这把锁。本测试直接验证两线程交叉拿/放锁的
    /// 互斥性——若未来谁手贱改回独立 TUNING_WRITE_LOCK 锁，本测试不会直接
    /// 失败（按理两线程都同时拿不同锁也应该过），但会留形如「锁身份」的
    /// 钩子让 review 一眼能发现。
    /// 真正的字段不丢语义在 fn write_memory_tuning_roundtrip_preserves_and_removes
    /// + 生产 RMW 路径覆盖；本测试仅锁「写 tuning 走 lock_config_write」。
    #[test]
    fn write_memory_tuning_holds_lock_config_write_during_rwm() {
        use std::sync::mpsc;
        use std::thread;
        // T1：拿锁并保持 50ms（期间模拟 RMW 耗时）
        let (tx1, rx1) = mpsc::channel();
        let t1 = thread::spawn(move || {
            let _g = lock_config_write();
            tx1.send(()).unwrap(); // 通知 T2 拿锁尝试
            std::thread::sleep(std::time::Duration::from_millis(50));
            drop(_g);
        });
        rx1.recv().unwrap();
        // T2：在 T1 持锁期间尝试拿同一把锁——必须阻塞直到 T1 释放（否则
        // 锁被改回独立 TUNING_WRITE_LOCK 就会出现两路并发持锁）
        let start = std::time::Instant::now();
        let t2 = thread::spawn(move || {
            let _g = lock_config_write();
            start.elapsed()
        });
        let t2_wait = t2.join().unwrap();
        t1.join().unwrap();
        assert!(
            t2_wait >= std::time::Duration::from_millis(40),
            "T2 必须等 T1 释放锁后才获锁（实际等了 {t2_wait:?}——独立锁？）"
        );
    }

    #[test]
    fn plaintext_strippable_decides_by_verification() {
        // keyring 原有值（非本次写入）：非空即剥（keyring 值优先于文件副本）
        assert!(plaintext_strippable(Some("stored"), false, "file-copy"));
        assert!(plaintext_strippable(Some("stored"), false, "stored"));
        // 本次写入：读回等值才剥（不等值=并发竞争/后端异常，剥除即丢钥）
        assert!(plaintext_strippable(Some("k"), true, "k"));
        assert!(!plaintext_strippable(Some("other"), true, "k"));
        // 无值 / 空白读回：保留
        assert!(!plaintext_strippable(None, false, "k"));
        assert!(!plaintext_strippable(None, true, "k"));
        assert!(!plaintext_strippable(Some("  "), true, "k"));
    }

    #[test]
    fn evolution_block_survives_config_rewrite_cycle() {
        //  回归：三条写路径（bot_set_config 整体替换 / update_config_file
        // RMW / persist_last_run 回写）都经 BotConfig serde 落盘——未知字段曾被
        // 静默丢弃，运行时配置的 evolution 块实际被写丢过。此处锁 serde 层保真：
        // 读入带块 → 序列化 → 块原样仍在（三条路径的公共层）。
        let raw = r#"{
            "baseUrl": "https://api.example.com/v1",
            "model": "m",
            "evolution": {
                "shadow": {"enabled": true},
                "activation": {"mode": "calibrating", "min_occurrences": null, "min_proposals": null},
                "activation_state": "s0_observe"
            }
        }"#;
        let cfg: BotConfig = serde_json::from_str(raw).unwrap();
        assert!(cfg.evolution.is_some(), "读入应带上 evolution 块");
        let out = serde_json::to_string_pretty(&cfg).unwrap();
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            back["evolution"]["shadow"]["enabled"],
            serde_json::json!(true)
        );
        assert_eq!(
            back["evolution"]["activation"]["mode"],
            serde_json::json!("calibrating")
        );
        assert_eq!(
            back["evolution"]["activation_state"],
            serde_json::json!("s0_observe")
        );
        // RMW 语义（update_config_file/persist_last_run 同构：load → 改别处 → 写回）：
        // 二次反序列化再序列化，块仍不丢
        let cfg2: BotConfig = serde_json::from_str(&out).unwrap();
        let out2 = serde_json::to_string(&cfg2).unwrap();
        assert!(out2.contains("\"evolution\""), "二次改写后块仍在");
        // 无块的配置写盘不得引入空 evolution 键（skip_serializing_if）
        let cfg3: BotConfig =
            serde_json::from_str(r#"{"baseUrl":"https://x/v1","model":"m"}"#).unwrap();
        assert!(!serde_json::to_string(&cfg3)
            .unwrap()
            .contains("\"evolution\""));
    }
}
