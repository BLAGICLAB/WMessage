//! bot-config.json IO + 旧版本 key 迁移。
//!
//! - `config_path` / `load_config` / `add_allowed_dir` / `update_config_file`
//! - `write_bot_config_file` 强制剥离 key 字段（双保险）+ base_url 安全告警
//! - `read_bypass_llm_switch` 轻量开关读取（bot_chat 入口用）
//! - `base_url_is_safe` SSRF / 明文传输警告
//! - `migrate_legacy_key` / `migrate_search_keys` 老配置明文 → keyring

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json;
use tauri::AppHandle;

use crate::db;
use crate::error::{CommandError, CommandResult};

use super::keyring;
use super::schema;
use super::types::{BotConfig, KeySlot};

// ───────────────────────── 路径 ─────────────────────────

pub fn config_path(app: &AppHandle) -> PathBuf {
    db::data_dir(app).join("bot-config.json")
}

/// bypass_llm 开关读取 helper：bot-config.json 缺字段 / 文件不存在 / 解析失败都默认 true（bypass 行为）。
/// 比 bot_get_config 轻量：跳过 BotConfigView 构造 + key 校验，bot_chat 入口用。
pub fn read_bypass_llm_switch(app: &AppHandle) -> bool {
    read_bypass_llm_switch_at(&config_path(app))
}

/// 可测内核（纯路径参数）：文件缺失 / 读失败 / JSON 损坏 / 缺字段 → true。
/// 只有显式 `bypassLlmOnPreStepHit: false` 才关掉 bypass——老配置零迁移语义，
/// 也是 F-1 拍板的默认行为（默认走 bypass，避免命中 Skill 后还要多烧一次外层 LLM）。
pub(crate) fn read_bypass_llm_switch_at(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return true;
    };
    serde_json::from_str::<BotConfig>(&raw)
        .map(|c| c.bypass_llm_on_pre_step_hit)
        .unwrap_or(true)
}

// ───────────────────────── load_config / add_allowed_dir ─────────────────────────

/// 读 bot-config.json（不存在/解析失败回默认）。内部共用（bot_fs 白名单等）
pub(crate) fn load_config(app: &AppHandle) -> BotConfig {
    // 迁移钩子：老配置缺 schemaVersion → 补默认 + 写回（幂等，已迁移不写盘；
    // 失败不阻塞读取——本次仍按下面常规路径读，下次再试）
    let _ = schema::migrate_bot_config_schema(app);
    let p = config_path(app);
    if p.exists() {
        if let Ok(raw) = std::fs::read_to_string(&p) {
            if let Ok(cfg) = serde_json::from_str::<BotConfig>(&raw) {
                return cfg;
            }
        }
    }
    BotConfig::default()
}

/// 授权弹窗「始终允许该目录」落盘：把目录追加进 allowedDirs 并写回
/// bot-config.json。allowedDirs 语义 = 内置默认（桌面/下载/文档+绑定文件夹）之上的
/// 追加放行，直接 push 去重即可；已存在/空白为幂等 no-op。
pub(crate) fn add_allowed_dir(app: &AppHandle, dir: &str) -> Result<(), String> {
    let d = dir.trim().to_string();
    if d.is_empty() {
        return Ok(());
    }
    let mut cfg = load_config(app);
    if cfg.allowed_dirs.iter().any(|x| x.trim() == d) {
        return Ok(());
    }
    cfg.allowed_dirs.push(d);
    let data_dir = db::data_dir(app);
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app), raw).map_err(|e| e.to_string())
}

// ───────────────────────── write_bot_config_file / update_config_file ─────────────────────────

/// bot_set_config 落盘内核（抽出便于单测）：强制剥离三个 key 字段
/// （api_key/tavily_key/brave_key 一律 None）后写 bot-config.json。
/// base_url 非 https 且非回环 → 警告（api_key 明文传输风险）；
/// 只警告不拒写——本地推理服务是合法场景，且不能破坏存量用户配置。
pub(crate) fn write_bot_config_file(dir: &Path, config: BotConfig) -> CommandResult<()> {
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
    std::fs::write(dir.join("bot-config.json"), raw)
        .map_err(|e| CommandError::IoError(e.to_string()))
}

/// 读-改-写 bot-config.json（记忆整理的 last_run_at 回写等内部配置更新用）：
/// 与 bot_set_config 同落盘路径（key 字段剥离由 write_bot_config_file 保证）。
pub(crate) fn update_config_file(
    app: &AppHandle,
    f: impl FnOnce(&mut BotConfig),
) -> CommandResult<()> {
    let mut cfg = load_config(app);
    f(&mut cfg);
    write_bot_config_file(&db::data_dir(app), cfg)
}

// ───────────────────────── base_url 安全判定 ─────────────────────────

/// base_url 安全判定：空 / https:// / 回环地址（localhost、127.x、::1）
/// 视为安全；其余（http:// 公网/内网 IP 域名等）不安全——调用方打警告，不拒写。
pub(crate) fn base_url_is_safe(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() || u.starts_with("https://") {
        return true;
    }
    let lower = u.to_lowercase();
    lower.starts_with("http://localhost")
        || lower.starts_with("http://127.")
        || lower.starts_with("http://[::1]")
}

// ───────────────────────── 老版本 key 迁移 ─────────────────────────

/// 旧版本迁移：bot-config.json 里有明文 key → 迁入系统凭据存储并清掉文件里的明文。
/// App 启动时调用一次（设置页读配置时也会兜底触发）。
pub fn migrate_legacy_key(app: &AppHandle) -> Result<(), String> {
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
    std::fs::write(&p, raw).map_err(|e| e.to_string())
}

/// 搜索 key 迁移（Tavily/Brave 不再明文落 bot-config.json）：
/// bot-config.json 里仍含明文 tavily_key/brave_key → keyring 里还没有对应 key 时
/// 写入（不覆盖更新值）→ 配置文件里这两个字段置 None 写回 → 审计留痕；
/// keyring 写失败保留文件明文下次再试（数据保留优先，与 migrate_legacy_key 同策略）。
/// App 启动时调用一次（设置页读配置时也会兜底触发，双调用点与 migrate_legacy_key 一致）。
pub fn migrate_search_keys(app: &AppHandle) -> Result<(), String> {
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
    for (slot, field) in [
        (KeySlot::Tavily, cfg.tavily_key.clone()),
        (KeySlot::Brave, cfg.brave_key.clone()),
    ] {
        let has_in_store = keyring::has_search_key(slot).unwrap_or(false);
        let mut write = |key: &str| keyring::write_search_key(slot, key).map_err(|e| e.message());
        let (remaining, migrated) =
            migrate_search_key_slot(field.as_deref(), has_in_store, &mut write);
        if migrated {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "config.search_key_migrated",
                "slot" => slot.keyring_user(),
            );
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
    std::fs::write(&p, raw).map_err(|e| e.to_string())
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
#[allow(dead_code)]
fn _write_marker(_w: &mut dyn Write) {}
