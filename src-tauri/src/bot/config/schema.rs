//! Schema 迁移（双协议模型派生 + bot-config.json schema 版本号）+ max_tokens。
//!
//! - `migrate_legacy_models` 老配置（无 models_by_provider）→ 新结构（内存内补一份）
//! - `derive_default_label` 从 base_url 提个像样的名字
//! - `derive_legacy_fields_from_active` bot_set_config 落盘前回填老字段
//! - `resolve_max_tokens` + DEFAULT/MAX/MIN_MAX_TOKENS
//! - `migrate_config_value` / `migrate_config_file` / `migrate_bot_config_schema`
//! - SCHEMA_VERSION_KEY + SCHEMA_MIGRATION_CHECKED
//!
//! 无 IO 副作用（除了 migrate_bot_config_schema 调 audit_event + 写盘）。

use std::sync::atomic::AtomicBool;

use tauri::AppHandle;

use super::types::{
    ActiveModelId, ApiProvider, BotConfig, ModelEntry, ModelsByProvider, BOT_CONFIG_SCHEMA_VERSION,
};

// ─────────────── 双协议下大模型列表迁移/派生 ───────────────

/// 老配置 → 新结构一次性迁移：无 models_by_provider、但 base_url 或
/// model 非空时，在内存里补一份单条 ModelEntry + active_model_id，挂在
/// api_provider 协议下。**不写文件**——只在下一次用户保存时由 bot_set_config 一并
/// 落盘，避免无谓写盘 + 误清空老用户已配好的 key/base_url。
///
/// 老配置也是空（base_url+model 都空）→ 按老板「不设置默认厂商」要求保持空列表，
/// 让用户点「添加大模型」自己加。
pub(crate) fn migrate_legacy_models(cfg: &mut BotConfig) {
    if cfg.models_by_provider.is_some() {
        return; // 已是新结构 / 已迁移过
    }
    let has_legacy = !cfg.base_url.trim().is_empty() || !cfg.model.trim().is_empty();
    if !has_legacy {
        return; // 老配置也空 = 新用户，列表留空
    }
    let entry = ModelEntry {
        id: "migrated".into(),
        label: derive_default_label(&cfg.base_url),
        base_url: cfg.base_url.clone(),
        model: cfg.model.clone(),
    };
    let provider = ApiProvider::from_cfg(cfg.api_provider.as_deref());
    let mut mbp = ModelsByProvider::default();
    let mut active = ActiveModelId::default();
    match provider {
        ApiProvider::Openai => {
            mbp.openai.push(entry);
            active.openai = Some("migrated".into());
        }
        ApiProvider::Anthropic => {
            mbp.anthropic.push(entry);
            active.anthropic = Some("migrated".into());
        }
    }
    cfg.models_by_provider = Some(mbp);
    cfg.active_model_id = Some(active);
}

/// 老配置迁移用的默认 label：尽量从 URL 提个像样的名字（覆盖 MiniMax/Kimi/
/// DeepSeek/OpenAI/Anthropic 几个常用供应商），其它情况回退 "默认"。用户后续
/// 可以在设置页改 label。
fn derive_default_label(base_url: &str) -> String {
    let u = base_url.trim().to_lowercase();
    if u.is_empty() {
        return "默认".into();
    }
    if u.contains("deepseek") {
        return "DeepSeek".into();
    }
    if u.contains("moonshot") || u.contains("kimi") {
        return "Kimi".into();
    }
    if u.contains("minimaxi") {
        return "MiniMax".into();
    }
    if u.contains("openai") {
        return "OpenAI".into();
    }
    if u.contains("anthropic") {
        return "Anthropic".into();
    }
    "默认".into()
}

/// 设置页保存前回填派生字段：bot_model_loop 只看 base_url/model/
/// max_tokens/api_provider 四个老字段，新结构 models_by_provider + active_model_id
/// 落到这里：当前 api_provider 协议下找 active 模型 → 找不到用第一个 → 把它的
/// base_url/model 写回 cfg。**列表为空时不动 base_url/model**——避免用户删完
/// 列表后误清空已配好的派生字段。
pub(crate) fn derive_legacy_fields_from_active(cfg: &mut BotConfig) {
    let mbp = match cfg.models_by_provider.as_ref() {
        Some(m) => m,
        None => return,
    };
    let active = match cfg.active_model_id.as_ref() {
        Some(a) => a,
        None => return,
    };
    let provider = ApiProvider::from_cfg(cfg.api_provider.as_deref());
    let (list, active_id) = match provider {
        ApiProvider::Openai => (&mbp.openai, active.openai.as_ref()),
        ApiProvider::Anthropic => (&mbp.anthropic, active.anthropic.as_ref()),
    };
    let entry = active_id
        .and_then(|id| list.iter().find(|e| &e.id == id))
        .or_else(|| list.first());
    if let Some(entry) = entry {
        cfg.base_url = entry.base_url.clone();
        cfg.model = entry.model.clone();
    }
    // 列表为空或没有新结构：保持 base_url/model 不动（防御性）
}

// ───────────────────────── max_tokens ─────────────────────────

/// max_tokens 默认值与合法范围（默认 8192；仅 Anthropic 模式发送）
pub const DEFAULT_MAX_TOKENS: u32 = 8192;
pub const MIN_MAX_TOKENS: u32 = 256;
pub const MAX_MAX_TOKENS: u32 = 200_000;

/// max_tokens 配置解析：None = 默认 8192；Some 钳制到 256..=200000
pub fn resolve_max_tokens(v: Option<u32>) -> u32 {
    v.unwrap_or(DEFAULT_MAX_TOKENS)
        .clamp(MIN_MAX_TOKENS, MAX_MAX_TOKENS)
}

// ───────────────────────── schema 迁移 ─────────────────────────

/// schemaVersion 在 JSON 里的键名（camelCase）。
pub(crate) const SCHEMA_VERSION_KEY: &str = "schemaVersion";

/// schema 迁移内核（纯 value，便于单测）：缺失/落后当前版本 → 就地补 `schemaVersion`，
/// 返回 (from, to)；已是当前或更高版本 → None（不降级「装过更新版后回退」的配置）。
/// v0 = 带版本号方案之前、没有该字段的老配置。
///
/// 逐版本迁移点：v0→v1 只补版本号字段本身；后续版本在此按 from 分支改字段
/// （直接改 `value` 上的键即可——未识别的字段原样保留，不会丢用户数据）。
pub(crate) fn migrate_config_value(value: &mut serde_json::Value) -> Option<(u32, u32)> {
    if !value.is_object() {
        return None; // 非对象 JSON：读取侧本来就回默认，不在这里纠错（也避免索引 panic）
    }
    let from = value
        .get(SCHEMA_VERSION_KEY)
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if from >= BOT_CONFIG_SCHEMA_VERSION {
        return None;
    }
    value[SCHEMA_VERSION_KEY] = serde_json::json!(BOT_CONFIG_SCHEMA_VERSION);
    Some((from, BOT_CONFIG_SCHEMA_VERSION))
}

/// 文件级迁移内核（无 AppHandle，便于单测）：命中迁移才写回，返回 (from, to)。
/// 文件不存在 / JSON 损坏 → Ok(None)（读取侧本来就会回默认，不在这里纠错）。
/// **保留**文件里的全部字段——含尚未迁进 keyring 的明文 apiKey；
/// 明文清除是 migrate_legacy_key / migrate_search_keys 的职责，调用顺序不能反。
pub(crate) fn migrate_config_file(path: &std::path::Path) -> Result<Option<(u32, u32)>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(pair) = migrate_config_value(&mut value) else {
        return Ok(None);
    };
    let out = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, out).map_err(|e| e.to_string())?;
    Ok(Some(pair))
}

/// 每进程只探测一次（load_config 是热路径，避免每次读盘解析）；
/// 成功判定/写回后置位，失败不置位 → 下次读配置再试。
pub(crate) static SCHEMA_MIGRATION_CHECKED: AtomicBool = AtomicBool::new(false);

/// bot-config.json schema 迁移钩子：读取时缺失就补默认 + 写回。
/// 调用点与 migrate_legacy_key 同：App 启动（lib.rs setup）+ 每次 load_config 兜底。
/// 幂等：已是当前版本不写盘；写回保留全部既有字段（明文 key 由后续迁移负责清）。
pub fn migrate_bot_config_schema(app: &AppHandle) -> Result<(), String> {
    if SCHEMA_MIGRATION_CHECKED.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    match migrate_config_file(&super::io::config_path(app)) {
        Ok(pair) => {
            SCHEMA_MIGRATION_CHECKED.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some((from, to)) = pair {
                crate::audit_event!(
                    app,
                    crate::audit::AuditLevel::Info,
                    "config.schema_migrated",
                    "from" => from,
                    "to" => to,
                );
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}
