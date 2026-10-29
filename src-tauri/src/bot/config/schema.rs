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

// 双协议下大模型列表迁移/派生

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
        vendor: None,
        enabled: true,
        context_k: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        system_prompt: None,
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

// 条目级推理参数

/// 当前协议 + active_model_id → active 模型条目。
/// 与 keyring::active_vendor_of 同构的拆参签名（BotConfig 与对外视图
/// BotConfigView 都能传，聊天主循环/摘要/Planner 拿到的是 view）。
/// 解析与 `derive_legacy_fields_from_active` 同源：active id 命中优先，
/// id 悬空（指向已删条目）/ 协议槽位未选 → 回退第一条——请求实际打向的
/// base_url/model 正是 derive 回填的那条，条目级参数必须跟同一条目走；
/// active_model_id 整体缺失 / 无新结构 / 列表空 → None。
pub fn active_model_entry<'a>(
    api_provider: Option<&str>,
    active_model_id: Option<&ActiveModelId>,
    models_by_provider: Option<&'a ModelsByProvider>,
) -> Option<&'a ModelEntry> {
    let active = active_model_id?;
    let mbp = models_by_provider?;
    let (list, active_id) = match ApiProvider::from_cfg(api_provider) {
        ApiProvider::Openai => (&mbp.openai, active.openai.as_ref()),
        ApiProvider::Anthropic => (&mbp.anthropic, active.anthropic.as_ref()),
    };
    active_id
        .and_then(|id| list.iter().find(|e| &e.id == id))
        .or_else(|| list.first())
}

/// 厂商可用性守卫（请求装配前 / 切 active 前调用）：条目带 vendor（非空）且该
/// 厂商不在 verified_vendors（连接测试未通过 / 失败被移出 / Base URL 或协议
/// 变更后失效）→ Err 可行动文案。verified_vendors 此前只是聊天下拉的 UI 过滤，
/// 设置页「设为当前使用」仍能让未验证厂商成为全局 active，任务执行/聊天带无效
/// key 出门换 401——本守卫是请求侧的统一防线。
/// 无 vendor 的老条目没有验证概念 → 放行（与聊天下拉过滤同口径）。
pub fn ensure_vendor_verified(
    vendor: Option<&str>,
    verified_vendors: &[String],
) -> Result<(), String> {
    let Some(v) = vendor.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    if verified_vendors.iter().any(|x| x == v) {
        return Ok(());
    }
    Err(format!(
        "厂商「{v}」未通过连接测试，已拦截请求：请到 设置→模型厂商 完成「测试连接」后再试；或在聊天区 🧠 下拉切换到已验证的模型"
    ))
}

/// 三处请求装配（聊天主循环 / 摘要 / Planner）共用的条目级推理参数解析结果。
/// `max_tokens` 已过优先级与钳制，可直接用于 Anthropic 请求体与
/// reasoning::resolve 的 thinking budget 夹紧。
#[derive(Debug, Clone)]
pub struct EffectiveInference {
    pub max_tokens: u32,
    /// Some = 写入请求体（OpenAI / Anthropic 顶层字段同名）；None = 不发。
    /// 已按协议钳制（Anthropic 0..=1 / OpenAI 0..=2，超界上游直接 400）
    pub temperature: Option<f64>,
    /// 已钳制 0..=1（两协议同界）
    pub top_p: Option<f64>,
    /// active 条目的 system prompt（trim 后非空才保留）：聊天主循环追加到
    /// 消息栈末尾；摘要 / Planner 有各自的固定任务提示词，不追加
    pub system_prompt: Option<String>,
}

/// temperature 合法上限：OpenAI 兼容 0..=2 / Anthropic 0..=1（top_p 两协议均 0..=1）。
/// 条目值是自由输入（前端只挡非数字），超界值上游会 400——与 resolve_max_tokens
/// 同策略在装配前钳制，不让一次超界采样参数毒断整轮对话。
pub const MAX_TEMPERATURE_OPENAI: f64 = 2.0;
pub const MAX_TEMPERATURE_ANTHROPIC: f64 = 1.0;
pub const MAX_TOP_P: f64 = 1.0;

/// 条目 temperature 解析：按协议钳制 0..=max；None = 不写请求体；
/// 非有限值（NaN/±∞）→ None 不发，防毒请求体（clamp 对 NaN 原样透传）
pub fn resolve_temperature(v: Option<f64>, provider: ApiProvider) -> Option<f64> {
    let max = match provider {
        ApiProvider::Openai => MAX_TEMPERATURE_OPENAI,
        ApiProvider::Anthropic => MAX_TEMPERATURE_ANTHROPIC,
    };
    v.filter(|t| t.is_finite()).map(|t| t.clamp(0.0, max))
}

/// 条目 top_p 解析：钳制 0..=1；None = 不写请求体；
/// 非有限值（NaN/±∞）→ None 不发（同 resolve_temperature）
pub fn resolve_top_p(v: Option<f64>) -> Option<f64> {
    v.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, MAX_TOP_P))
}

/// 条目级推理参数解析：优先级 条目值 > 全局值 > 内置默认。
/// - max_tokens：条目值覆盖全局 BotConfig.max_tokens，再统一过 resolve_max_tokens
///   （32768 兜底 + 256..=200000 钳制）；OpenAI 分支不发送（多数兼容网关不认识），
///   但有效值仍供 reasoning::resolve 夹紧 thinking budget 用。
/// - temperature / top_p：配置无全局字段，条目有值才注入（否则用模型服务端默认），
///   注入前按协议钳制（resolve_temperature / resolve_top_p）。
pub fn effective_inference(
    api_provider: Option<&str>,
    max_tokens: Option<u32>,
    active_model_id: Option<&ActiveModelId>,
    models_by_provider: Option<&ModelsByProvider>,
) -> EffectiveInference {
    let provider = ApiProvider::from_cfg(api_provider);
    let entry = active_model_entry(api_provider, active_model_id, models_by_provider);
    match entry {
        Some(e) => inference_for_entry(e, provider, max_tokens),
        None => EffectiveInference {
            max_tokens: resolve_max_tokens(max_tokens),
            temperature: resolve_temperature(None, provider),
            top_p: resolve_top_p(None),
            system_prompt: None,
        },
    }
}

// max_tokens

/// max_tokens 默认值与合法范围（仅 Anthropic 模式发送）
///
/// 默认 16K（constants::MAX_TOKENS_DEFAULT），硬顶 32768——单轮输出再大也
/// 不该超过这个量级。
pub const DEFAULT_MAX_TOKENS: u32 = crate::bot::constants::MAX_TOKENS_DEFAULT;
pub const MIN_MAX_TOKENS: u32 = 256;
pub const MAX_MAX_TOKENS: u32 = crate::bot::params::MAX_TOKENS_CAP;

/// max_tokens 配置解析：None = 内置默认；Some 钳制到 256..=硬顶。
pub fn resolve_max_tokens(v: Option<u32>) -> u32 {
    v.unwrap_or(DEFAULT_MAX_TOKENS)
        .clamp(MIN_MAX_TOKENS, MAX_MAX_TOKENS)
}

/// max_tokens 写侧钳制（256..=硬顶）——落盘前用。
///
/// 钳制原本只在读侧 `resolve_max_tokens`：落盘的是用户填的原值，运行时按钳过的
/// 值走。写侧用同一组边界钳一遍，落盘值 = 生效值。
pub fn clamp_max_tokens(n: u32) -> u32 {
    n.clamp(MIN_MAX_TOKENS, MAX_MAX_TOKENS)
}

// schema 迁移

/// schemaVersion 在 JSON 里的键名（camelCase）。
pub(crate) const SCHEMA_VERSION_KEY: &str = "schemaVersion";

/// schema 迁移内核（纯 value，便于单测）：缺失/落后当前版本 → 就地补 `schemaVersion`，
/// 返回 (from, to)；已是当前或更高版本 → None（不降级「装过更新版后回退」的配置）。
/// v0 = 带版本号方案之前、没有该字段的老配置。
///
/// 逐版本迁移点：v0→v1 只补版本号字段本身；v1→v2 把工具输出上限的键从
/// `maxToolOutputChars` 换成 `maxToolOutputBytes`（单位从字符改成字节）。
/// 后续版本在此按 from 分支改字段（直接改 `value` 上的键即可——未识别的字段
/// 原样保留，不会丢用户数据）。
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
    if from < 2 {
        migrate_tool_output_cap_to_bytes(value);
    }
    value[SCHEMA_VERSION_KEY] = serde_json::json!(BOT_CONFIG_SCHEMA_VERSION);
    Some((from, BOT_CONFIG_SCHEMA_VERSION))
}

/// v1→v2：工具输出上限的键改名 + 口径从字符换成字节。
///
/// 数值原样搬过去（不做 ×3 之类的换算）：这个上限本来就是一条「体积预算」，
/// 用户的意图是「别超过这么大」，不是「放这么多个汉字」。老值当字节用，
/// 对 ASCII 输出（代码、路径、工具回显）完全等价；对中文输出偏紧 3 倍，
/// 用户可自行调大。
///
/// 键已存在新名时不覆盖——新名优先（用户可能已手改过）。
fn migrate_tool_output_cap_to_bytes(value: &mut serde_json::Value) {
    const OLD_KEY: &str = "maxToolOutputChars";
    const NEW_KEY: &str = "maxToolOutputBytes";
    let Some(old) = value.get(OLD_KEY).cloned() else {
        return;
    };
    if value.get(NEW_KEY).is_none() {
        value[NEW_KEY] = old;
    }
    if let Some(obj) = value.as_object_mut() {
        obj.remove(OLD_KEY);
    }
}

/// 文件级迁移内核（无 AppHandle，便于单测）：命中迁移才写回，返回 (from, to)。
/// 文件不存在 / JSON 损坏 → Ok(None)（读取侧本来就会回默认，不在这里纠错）。
/// **保留**文件里的全部字段——含尚未迁进 keyring 的明文 apiKey；
/// 明文清除是 migrate_legacy_key / migrate_search_keys 的职责，调用顺序不能反。
/// 加锁外壳：与其他写路径互斥（C5-BT-03）。
#[cfg(test)]
pub(crate) fn migrate_config_file(path: &std::path::Path) -> Result<Option<(u32, u32)>, String> {
    let _g = super::io::lock_config_write();
    migrate_config_file_locked(path)
}

/// 无锁内核：调用方必须已持 CONFIG_WRITE_LOCK（migrate_bot_config_schema 的
/// 双检段内复用——std Mutex 不可重入，经加锁外壳会自锁）。
pub(crate) fn migrate_config_file_locked(
    path: &std::path::Path,
) -> Result<Option<(u32, u32)>, String> {
    debug_assert!(
        super::io::holding_config_write(),
        "必须持 CONFIG_WRITE_LOCK"
    );
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
    // 原子写：文件可能仍含明文 apiKey，std::fs::write 半截写 = 配置+密钥同毁
    super::io::write_config_atomic(path, &out).map_err(|e| e.to_string())?;
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
    // try_lock 让路：持锁 RMW 段内经 load_config 调进这里，直接 lock 会自锁。
    // 让路不丢迁移：持锁写路径进段已先调 migrate_bot_config_schema_locked。
    let _g = match super::io::CONFIG_WRITE_LOCK.try_lock() {
        Ok(g) => super::io::ConfigWriteGuard::from_acquired(g),
        Err(std::sync::TryLockError::WouldBlock) => {
            eprintln!("[bot] schema 迁移让路：CONFIG_WRITE_LOCK 持锁中，持锁写路径会补跑");
            return Ok(());
        }
        Err(std::sync::TryLockError::Poisoned(e)) => {
            eprintln!("[mutex_poisoned] bot::config::io::CONFIG_WRITE_LOCK: {e:?}");
            super::io::ConfigWriteGuard::from_acquired(e.into_inner())
        }
    };
    migrate_bot_config_schema_locked(app)
}

/// 持锁调用方专用（RMW 段内 + try_lock 包装）：推进迁移 + 置位 + 审计；
/// 锁内复查消除 check-then-act 窗口。审计在锁内（锁序允许 CONFIG→BOT_LOG）。
pub(crate) fn migrate_bot_config_schema_locked(app: &AppHandle) -> Result<(), String> {
    debug_assert!(
        super::io::holding_config_write(),
        "必须持 CONFIG_WRITE_LOCK"
    );
    if SCHEMA_MIGRATION_CHECKED.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    match migrate_config_file_locked(&super::io::config_path(app)) {
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

/// 条目级推理参数：每卡模型覆盖时按覆盖条目计算（原 effective_inference
/// 只按 active 条目）。global_max_tokens = 全局 max_tokens 兜底。
pub fn inference_for_entry(
    entry: &ModelEntry,
    provider: ApiProvider,
    global_max_tokens: Option<u32>,
) -> EffectiveInference {
    EffectiveInference {
        max_tokens: resolve_max_tokens(entry.max_tokens.or(global_max_tokens)),
        temperature: resolve_temperature(entry.temperature, provider),
        top_p: resolve_top_p(entry.top_p),
        system_prompt: entry
            .system_prompt
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

#[cfg(test)]
mod vendor_guard_tests {
    use super::*;

    #[test]
    fn guard_blocks_unverified_vendor_with_actionable_message() {
        let err = ensure_vendor_verified(Some("DeepSeek"), &["MiniMax".into()])
            .expect_err("未验证厂商必须拦截");
        assert!(
            err.contains("DeepSeek") && err.contains("测试连接"),
            "{err}"
        );
        // 前后空白不影响判定：trim 后命中名单即放行
        assert!(ensure_vendor_verified(Some(" MiniMax "), &["MiniMax".into()]).is_ok());
    }

    #[test]
    fn guard_passes_vendorless_legacy_entries_and_empty_vendor() {
        // 无 vendor 老条目 / 空 vendor / 空白 vendor 都没有验证概念 → 放行
        assert!(ensure_vendor_verified(None, &[]).is_ok());
        assert!(ensure_vendor_verified(Some(""), &[]).is_ok());
        assert!(ensure_vendor_verified(Some("  "), &[]).is_ok());
    }

    #[test]
    fn guard_blocks_when_verified_list_empty() {
        // 迁移期存量：条目带 vendor 但从未点过测试连接 → 拦截（与聊天下拉
        // 过滤同口径；收尾引导横幅已指路逐厂商补测）
        assert!(ensure_vendor_verified(Some("MiniMax"), &[]).is_err());
    }
}

#[cfg(test)]
mod inference_tests {
    use super::*;
    use crate::bot::reasoning::{self, EffortLevel, ReasoningWire};

    /// 测试侧便捷封装：与生产调用方同路径拆参（BotConfig 字段 → 拆参签名）
    fn aentry(cfg: &BotConfig) -> Option<&ModelEntry> {
        active_model_entry(
            cfg.api_provider.as_deref(),
            cfg.active_model_id.as_ref(),
            cfg.models_by_provider.as_ref(),
        )
    }
    fn eff(cfg: &BotConfig) -> EffectiveInference {
        effective_inference(
            cfg.api_provider.as_deref(),
            None,
            cfg.active_model_id.as_ref(),
            cfg.models_by_provider.as_ref(),
        )
    }

    fn entry(id: &str, model: &str) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            label: id.into(),
            base_url: "https://api.example.com/v1".into(),
            model: model.into(),
            vendor: None,
            enabled: true,
            context_k: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            system_prompt: None,
        }
    }

    /// openai + anthropic 各一条、active 各指一条的基准配置
    fn cfg_with(
        openai: Vec<ModelEntry>,
        anthropic: Vec<ModelEntry>,
        active_openai: Option<&str>,
        active_anthropic: Option<&str>,
        api_provider: &str,
    ) -> BotConfig {
        BotConfig {
            api_provider: Some(api_provider.into()),
            models_by_provider: Some(ModelsByProvider { openai, anthropic }),
            active_model_id: Some(ActiveModelId {
                openai: active_openai.map(str::to_string),
                anthropic: active_anthropic.map(str::to_string),
            }),
            ..BotConfig::default()
        }
    }

    #[test]
    fn active_entry_resolves_active_id_per_protocol() {
        let mut o = entry("m1", "a");
        o.vendor = Some("DeepSeek".into());
        let a = entry("m2", "claude");
        let cfg = cfg_with(
            vec![o],
            vec![a.clone()],
            Some("m1"),
            Some("m2"),
            "anthropic",
        );
        // anthropic 协议只看 anthropic 列表 + anthropic active id
        assert_eq!(aentry(&cfg).unwrap().id, "m2");
        // 无 vendor 的条目也解析（老配置条目）
        let cfg2 = cfg_with(
            vec![entry("m1", "a")],
            vec![a],
            Some("m1"),
            Some("m2"),
            "openai",
        );
        assert_eq!(aentry(&cfg2).unwrap().id, "m1");
    }

    #[test]
    fn active_entry_falls_back_to_first_on_stale_active_id() {
        // active id 指向已删条目 → 回退第一条（与 derive_legacy_fields 同源）
        let cfg = cfg_with(
            vec![entry("m1", "a"), entry("m2", "b")],
            vec![],
            Some("gone"),
            None,
            "openai",
        );
        assert_eq!(aentry(&cfg).unwrap().id, "m1");
        // 当前协议槽位没选 active 但列表非空 → 同样回退第一条
        let cfg2 = cfg_with(vec![entry("m1", "a")], vec![], None, None, "openai");
        assert_eq!(aentry(&cfg2).unwrap().id, "m1");
    }

    #[test]
    fn active_entry_none_without_active_or_list() {
        // active_model_id 整体缺失 → None（derive 同样不派生，老字段保持不动）
        let mut cfg = cfg_with(vec![entry("m1", "a")], vec![], None, None, "openai");
        cfg.active_model_id = None;
        assert_eq!(aentry(&cfg), None);
        // 无新结构（老配置未迁移）
        cfg.models_by_provider = None;
        cfg.active_model_id = Some(ActiveModelId::default());
        assert_eq!(aentry(&cfg), None);
        // 当前协议列表为空 → None
        let empty = cfg_with(vec![], vec![], None, None, "openai");
        assert_eq!(aentry(&empty), None);
    }

    #[test]
    fn entry_max_tokens_overrides_global_then_clamped() {
        let mut e = entry("m1", "a");
        e.max_tokens = Some(4_096);
        let mut cfg = cfg_with(vec![e], vec![], Some("m1"), None, "openai");
        // 条目值生效
        assert_eq!(eff(&cfg).max_tokens, 4_096);
        // 条目无值 → 内置默认（全局键已退役）
        cfg.models_by_provider.as_mut().unwrap().openai[0].max_tokens = None;
        assert_eq!(eff(&cfg).max_tokens, DEFAULT_MAX_TOKENS);
        // 钳制在覆盖之后：条目 50 → 256，条目 999999 → 200000
        for (raw, clamped) in [(50u32, MIN_MAX_TOKENS), (999_999, MAX_MAX_TOKENS)] {
            cfg.models_by_provider.as_mut().unwrap().openai[0].max_tokens = Some(raw);
            assert_eq!(eff(&cfg).max_tokens, clamped);
        }
    }

    /// 写侧钳制与读侧 resolve 同一组边界：落盘值 = 生效值。
    #[test]
    fn clamp_max_tokens_matches_resolve_bounds() {
        for raw in [0u32, 50, MIN_MAX_TOKENS, 32_768, MAX_MAX_TOKENS, u32::MAX] {
            assert_eq!(
                clamp_max_tokens(raw),
                resolve_max_tokens(Some(raw)),
                "写侧与读侧边界必须一致：{raw}"
            );
        }
        assert_eq!(clamp_max_tokens(0), MIN_MAX_TOKENS);
        assert_eq!(clamp_max_tokens(u32::MAX), MAX_MAX_TOKENS);
    }

    #[test]
    fn entry_params_passthrough_and_blank_prompt_dropped() {
        let mut e = entry("m1", "a");
        e.temperature = Some(0.3);
        e.top_p = Some(0.9);
        e.system_prompt = Some("  ".into());
        let cfg = cfg_with(vec![e], vec![], Some("m1"), None, "openai");
        let inf = eff(&cfg);
        assert_eq!(inf.temperature, Some(0.3));
        assert_eq!(inf.top_p, Some(0.9));
        assert_eq!(inf.system_prompt, None, "全空白 system prompt 视为未配置");
        // 无条目 → 全部 None（temperature/top_p 无全局字段可回退）
        let bare = cfg_with(vec![], vec![], None, None, "openai");
        let inf = eff(&bare);
        assert_eq!(inf.temperature, None);
        assert_eq!(inf.top_p, None);
        assert_eq!(inf.system_prompt, None);
    }

    #[test]
    fn temperature_top_p_clamped_per_protocol() {
        let mut e = entry("m1", "a");
        e.temperature = Some(1.5);
        e.top_p = Some(1.7);
        // Anthropic：temperature 0..=1 / top_p 0..=1，超界收敛（上游 1.0 以上直接 400）
        let anthropic = cfg_with(vec![], vec![e.clone()], None, Some("m1"), "anthropic");
        let inf = eff(&anthropic);
        assert_eq!(inf.temperature, Some(1.0));
        assert_eq!(inf.top_p, Some(1.0));
        // OpenAI 兼容：temperature 1.5 在 0..=2 内原样保留，top_p 仍钳 1.0
        let openai = cfg_with(vec![e], vec![], Some("m1"), None, "openai");
        let inf = eff(&openai);
        assert_eq!(inf.temperature, Some(1.5));
        assert_eq!(inf.top_p, Some(1.0));
        // 负值钳到 0；OpenAI 超 2.0 收敛
        let mut e2 = entry("m1", "a");
        e2.temperature = Some(-0.5);
        let neg = cfg_with(vec![e2], vec![], Some("m1"), None, "openai");
        assert_eq!(eff(&neg).temperature, Some(0.0));
        let mut e3 = entry("m1", "a");
        e3.temperature = Some(2.5);
        let over = cfg_with(vec![e3], vec![], Some("m1"), None, "openai");
        assert_eq!(eff(&over).temperature, Some(MAX_TEMPERATURE_OPENAI));
    }

    #[test]
    fn system_prompt_trimmed_before_use() {
        // trim 后非空才保留，且保留的是 trim 后的值（消息栈不带首尾空白）
        let mut e = entry("m1", "a");
        e.system_prompt = Some("  用中文回复  ".into());
        let cfg = cfg_with(vec![e], vec![], Some("m1"), None, "openai");
        assert_eq!(eff(&cfg).system_prompt.as_deref(), Some("用中文回复"));
    }

    #[test]
    fn active_entry_resolution_ignores_enabled_flag() {
        // enabled 是 UI 层过滤语义（聊天 🧠 下拉不显示）；后端按 active_model_id
        // 显式选中的条目解析参数不受 enabled 影响（与 base_url/model 派生同口径）。
        // 若日后要改契约，此测试即显式决策点。
        let mut e = entry("m1", "a");
        e.enabled = false;
        e.temperature = Some(0.5);
        let cfg = cfg_with(vec![e], vec![], Some("m1"), None, "openai");
        assert_eq!(aentry(&cfg).unwrap().id, "m1");
        assert_eq!(
            eff(&cfg).temperature,
            Some(0.5),
            "禁用的 active 条目参数仍生效"
        );
    }

    #[test]
    fn entry_system_prompt_passes_through() {
        let mut e = entry("m1", "a");
        e.system_prompt = Some("总是用中文回复".into());
        let cfg = cfg_with(vec![e], vec![], Some("m1"), None, "openai");
        assert_eq!(eff(&cfg).system_prompt.as_deref(), Some("总是用中文回复"));
    }

    #[test]
    fn entry_max_tokens_feeds_thinking_budget_clamp() {
        // 覆盖后的 max_tokens 必须参与 reasoning::resolve 的 budget < max_tokens 夹紧：
        // 条目 1500 → 1500-1024=476 < 1024 放不下思维链 → 放弃注入
        let mut e = entry("m1", "claude-3-7-sonnet");
        e.max_tokens = Some(1_500);
        let cfg = cfg_with(vec![], vec![e], None, Some("m1"), "anthropic");
        let inf = eff(&cfg);
        assert_eq!(inf.max_tokens, 1_500);
        assert_eq!(
            reasoning::resolve(
                ApiProvider::Anthropic,
                "claude-3-7-sonnet",
                EffortLevel::High,
                inf.max_tokens
            ),
            ReasoningWire::None
        );
        // 条目放大到 8192 后同一档位放得下 → budget 回来了（6144 基准 < 8192-1024）
        let mut e2 = entry("m1", "claude-3-7-sonnet");
        e2.max_tokens = Some(8_192);
        let cfg2 = cfg_with(vec![], vec![e2], None, Some("m1"), "anthropic");
        assert_eq!(
            reasoning::resolve(
                ApiProvider::Anthropic,
                "claude-3-7-sonnet",
                EffortLevel::High,
                eff(&cfg2).max_tokens
            ),
            ReasoningWire::AnthropicBudget(6_144)
        );
    }
}

#[cfg(test)]
mod schema_migration_tests {
    use super::*;

    /// v1 配置里的工具输出上限键改名后不能丢：老用户设的截断值必须原样生效，
    /// 否则改一次单位就把用户的设置静默清空。
    #[test]
    fn v1_tool_output_cap_key_is_renamed_to_bytes() {
        let mut v = serde_json::json!({
            "schemaVersion": 1,
            "maxToolOutputChars": 12_345,
        });
        let pair = migrate_config_value(&mut v);
        assert_eq!(pair, Some((1, 2)), "应从 v1 迁到 v2");
        assert_eq!(v["maxToolOutputBytes"], serde_json::json!(12_345));
        assert!(
            v.get("maxToolOutputChars").is_none(),
            "旧键必须移除，否则 serde 读 config 时会两个都看"
        );
    }

    /// 无版本号的老配置（v0）也走同一条改名路径。
    #[test]
    fn v0_config_gets_tool_output_cap_renamed() {
        let mut v = serde_json::json!({ "maxToolOutputChars": 800 });
        assert_eq!(migrate_config_value(&mut v), Some((0, 2)));
        assert_eq!(v["maxToolOutputBytes"], serde_json::json!(800));
    }

    /// 已是当前版本 → 不动（不降级、不重复迁移）。
    #[test]
    fn current_version_config_is_untouched() {
        let mut v = serde_json::json!({
            "schemaVersion": BOT_CONFIG_SCHEMA_VERSION,
            "maxToolOutputBytes": 999,
        });
        assert_eq!(migrate_config_value(&mut v), None);
        assert_eq!(v["maxToolOutputBytes"], serde_json::json!(999));
    }

    /// 新旧键同时存在时以新键为准，老键被清掉——不能出现两个值打架。
    #[test]
    fn new_key_wins_when_both_present() {
        let mut v = serde_json::json!({
            "schemaVersion": 1,
            "maxToolOutputChars": 1,
            "maxToolOutputBytes": 2,
        });
        migrate_config_value(&mut v);
        assert_eq!(v["maxToolOutputBytes"], serde_json::json!(2));
        assert!(v.get("maxToolOutputChars").is_none());
    }

    /// 迁移不得动其它字段——用户配置里的一堆无关键必须原样保留。
    #[test]
    fn migration_preserves_other_fields() {
        let mut v = serde_json::json!({
            "schemaVersion": 1,
            "maxToolOutputChars": 4_096,
            "baseUrl": "https://api.example.com/v1",
            "allowedDirs": ["/tmp/a"],
        });
        migrate_config_value(&mut v);
        assert_eq!(
            v["baseUrl"],
            serde_json::json!("https://api.example.com/v1")
        );
        assert_eq!(v["allowedDirs"], serde_json::json!(["/tmp/a"]));
    }
}
