//! Bot 配置模块的 tauri 命令（5 个）+ perm_mode 公开 helper。
//!
//! - `bot_get_config` 设置页读取（含老配置迁移兜底）
//! - `bot_set_config` 设置页保存（key 走 keyring，配置文件强制剥 key）
//! - `bot_set_active_model` 聊天区/挂件 🧠 下拉窄口径切 active 模型
//! - `bot_clear_api_key` 设置页清除主 LLM key
//! - `bot_log_read` 设置页读 bot.log 审计日志
//! - `perm_mode` 当前授权模式（chat 入口用）

use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

use super::audit;
use super::io;
use super::keyring;
use super::schema;
use super::types::{
    ActiveModelId, ApiProvider, BotConfig, BotConfigView, ModelsByProvider, PermMode,
};

// ───────────────────────── perm_mode ─────────────────────────

/// 当前授权模式（读 bot-config.json；缺文件/缺字段/非法值都回 Ask）
pub fn perm_mode(app: &AppHandle) -> PermMode {
    PermMode::from_cfg(io::load_config(app).perm_mode.as_deref())
}

// ───────────────────────── bot_get_config ─────────────────────────

#[tauri::command]
pub fn bot_get_config(app: AppHandle) -> CommandResult<BotConfigView> {
    // 兜底：设置页读配置时也确保无明文残留；迁移幂等可重试，失败记审计不阻断读取
    if let Err(e) = io::migrate_legacy_key(&app) {
        audit::audit_log(
            &app,
            &format!(
                "bot_config.migrate_legacy_key_failed | {}",
                audit::escape_for_log(&e, 200)
            ),
        );
    }
    // 兜底：Tavily/Brave 明文 key 同样迁进 keyring；同上记审计不阻断
    if let Err(e) = io::migrate_search_keys(&app) {
        audit::audit_log(
            &app,
            &format!(
                "bot_config.migrate_search_keys_failed | {}",
                audit::escape_for_log(&e, 200)
            ),
        );
    }
    let cfg = io::load_config(&app);

    // keyring 真实故障（钥匙串锁定/权限拒绝）不吞成「未配置」，
    // 结构化 KeyringError 透传给前端，设置页可提示用户检查 keychain
    let has_api_key = keyring::has_api_key()?;
    // 搜索 key 同策略：真实故障透传 Err，不吞成 false
    let has_tavily_key = keyring::has_search_key(super::types::KeySlot::Tavily)?;
    let has_brave_key = keyring::has_search_key(super::types::KeySlot::Brave)?;
    // 老配置（无 models_by_provider、有 base_url/model）在内存里补一份
    // ModelEntry + active_model_id，让设置页能展示出来；不写盘，等用户主动保存
    // 才一并落盘。
    let mut cfg = cfg;
    schema::migrate_legacy_models(&mut cfg);
    // 保证前端拿到永远完整的两协议子字段。
    // get_or_insert_with 保证 cfg.models_by_provider 是 Some；字段 #[serde(default)]
    // 保证反序列化时缺字段补空 Vec。两者联手让前端任何路径都不会拿到 undefined。
    // 前端那 5 处 `?? []` 兑底是最后一道防线。
    let _ = cfg
        .models_by_provider
        .get_or_insert_with(ModelsByProvider::default);
    Ok(BotConfigView {
        base_url: cfg.base_url,
        model: cfg.model,
        has_api_key,
        bypass_llm_on_pre_step_hit: cfg.bypass_llm_on_pre_step_hit,
        allowed_dirs: cfg.allowed_dirs,
        has_tavily_key,
        tavily_enabled: cfg.tavily_enabled,
        has_brave_key,
        brave_enabled: cfg.brave_enabled,
        python_timeout_secs: cfg.python_timeout_secs,
        perm_mode: cfg.perm_mode,
        api_provider: cfg.api_provider,
        max_tokens: cfg.max_tokens,
        reasoning_effort: cfg.reasoning_effort,
        models_by_provider: cfg.models_by_provider,
        active_model_id: cfg.active_model_id,
        ui_font_size: cfg.ui_font_size,
        memory_consolidation: cfg.memory_consolidation.unwrap_or_default(),
        mcp_servers: cfg.mcp_servers.unwrap_or_default(),
    })
}

// ───────────────────────── bot_set_config ─────────────────────────

/// P0-EV1 回填内核（纯函数，单测直打）：**盘上值权威**。前端视图（BotConfigView）
/// 不含 evolution 块，bot_set_config 是整体替换写——不回填则设置页任意一次保存
/// 都把自进化块写丢（运行时配置上实际发生过）。即便前端理论上带了值也以盘上
/// 为准：该块只归 evolution/ 模块读写，设置页没有编辑入口。
pub(crate) fn preserve_evolution(incoming: &mut BotConfig, disk: &BotConfig) {
    incoming.evolution = disk.evolution.clone();
}

/// 保存配置。api_key / tavily_key / brave_key 三个顶层参数同语义：
/// Some(非空) 写入系统凭据存储并覆盖；None/空串不动已存的 key。
/// （Tavily/Brave key 与主 key 同模式，不落配置文件）
#[tauri::command]
pub fn bot_set_config(
    app: AppHandle,
    config: BotConfig,
    api_key: Option<String>,
    tavily_key: Option<String>,
    brave_key: Option<String>,
) -> CommandResult<()> {
    if let Some(k) = api_key {
        let k = k.trim();
        if !k.is_empty() {
            keyring::write_api_key(k)?;
        }
    }
    for (slot, key) in [
        (super::types::KeySlot::Tavily, tavily_key),
        (super::types::KeySlot::Brave, brave_key),
    ] {
        if let Some(k) = key {
            let k = k.trim();
            if !k.is_empty() {
                keyring::write_search_key(slot, k)?;
            }
        }
    }
    // 文件里只留非敏感配置，三个 key 字段强制置 None（双保险：前端误把 key
    // 塞进 config 对象也不落明文）
    // 新结构（models_by_provider + active_model_id）落盘前回填
    // base_url/model/api_provider 三个派生字段——bot_model_loop 只看老字段。
    let mut config = config;
    schema::derive_legacy_fields_from_active(&mut config);
    // P0-EV1：前端整体替换写不丢 evolution 块。读盘回填必须与写**同锁**
    //（评审 HIGH：无锁 load → 有锁 write 之间 persist_last_run 等并发写会被
    // 本写覆盖——丢更新窗口）；write_bot_config_file 自带加锁不可重入，走 _locked 变体
    let _g = super::io::lock_config_write();
    let disk = super::io::load_config(&app);
    preserve_evolution(&mut config, &disk);
    super::io::write_bot_config_file_locked(&crate::db::data_dir(&app), config)
}

// ───────────────────────── bot_set_active_model ─────────────────────────

/// 纯逻辑（单测锚点）：校验 model_id 存在于**任一协议**的模型列表并置为 active。
/// 聊天区 🧠 下拉双协议同列展示（MP-02）：命中**另一协议**的模型 → 连协议一起切
/// （api_provider + 该协议的 active_model_id），derive 老字段自然跟随新协议。
/// 列表为空 / id 两边都不在（已删除）→ InvalidArgument 响亮失败，
/// 不静默回退到 first——用户点的是哪一个就该切哪一个。
pub fn apply_active_model_switch(cfg: &mut BotConfig, model_id: &str) -> CommandResult<()> {
    let mbp = cfg
        .models_by_provider
        .get_or_insert_with(ModelsByProvider::default);
    let in_openai = mbp.openai.iter().any(|e| e.id == model_id);
    let in_anthropic = mbp.anthropic.iter().any(|e| e.id == model_id);
    if !in_openai && !in_anthropic {
        return Err(CommandError::InvalidArgument {
            field: "modelId".into(),
            value: model_id.into(),
            reason: "模型 id 不在任一协议的模型列表中".into(),
        });
    }
    // 跨协议命中 → 协议跟着模型走（id 是前端 UUID，两边同时命中理论不可达；
    // 真发生则保持当前协议不动，防御处理）
    if in_anthropic && !in_openai {
        cfg.api_provider = Some(ApiProvider::Anthropic.as_str().into());
    } else if in_openai && !in_anthropic {
        cfg.api_provider = Some(ApiProvider::Openai.as_str().into());
    }
    let provider = ApiProvider::from_cfg(cfg.api_provider.as_deref());
    let active = cfg
        .active_model_id
        .get_or_insert_with(ActiveModelId::default);
    match provider {
        ApiProvider::Openai => active.openai = Some(model_id.into()),
        ApiProvider::Anthropic => active.anthropic = Some(model_id.into()),
    }
    Ok(())
}

/// 窄口径切换 active 模型（聊天区/挂件 🧠 下拉专用）：读盘上最新配置（单一事实源），
/// 只动目标协议的 active_model_id（MP-02 双协议同列：跨协议选中时连 api_provider
/// 一起切），派生 base_url/model 老字段后落盘。
/// **不做整份配置写回**——避免前端旧快照覆盖设置页并发修改的字段（白名单/开关等）。
/// key 相关路径完全不触碰。返回切换后的 BotConfigView，前端免二次读取。
#[tauri::command]
pub fn bot_set_active_model(app: AppHandle, model_id: String) -> CommandResult<BotConfigView> {
    let mut cfg = io::load_config(&app);
    schema::migrate_legacy_models(&mut cfg);
    apply_active_model_switch(&mut cfg, &model_id)?;
    schema::derive_legacy_fields_from_active(&mut cfg);
    io::write_bot_config_file(&crate::db::data_dir(&app), cfg)?;
    bot_get_config(app)
}

// ───────────────────────── bot_clear_api_key / bot_log_read ─────────────────────────

/// 清除已保存的 API Key
#[tauri::command]
pub fn bot_clear_api_key() -> CommandResult<()> {
    keyring::delete_api_key_at(
        keyring::key_backend(),
        &keyring::plaintext_key_path_for(super::types::KeySlot::Llm),
        super::types::KeySlot::Llm,
    )
}

pub use super::audit::bot_log_read;

// ───────────────────────── bot_reload_config ─────────────────────────

/// 显式 reload bot-config.json（决策 c：最小 reload endpoint，避免依赖重启）
///
/// io::load_config() 每次调用已重读文件 → 此命令作显式触发点。
/// 返回当前 shadow flag 状态便于诊断。
#[tauri::command]
pub fn bot_reload_config(app: AppHandle) -> CommandResult<bool> {
    let _cfg = io::load_config(&app);
    let shadow_enabled = crate::evolution::observe::shadow::is_enabled(&app);
    eprintln!("[bot] config reload: shadow_enabled={shadow_enabled}");
    Ok(shadow_enabled)
}

#[cfg(test)]
mod evolution_preserve_tests {
    use super::*;

    #[test]
    fn preserve_evolution_disk_value_is_authoritative() {
        // P0-EV1 回归：前端视图不含 evolution 块——盘上有则以盘上回填
        let mut incoming: BotConfig = serde_json::from_str("{}").unwrap();
        assert!(incoming.evolution.is_none());
        let disk: BotConfig = serde_json::from_str(
            r#"{"evolution":{"shadow":{"enabled":true},"activation_state":"s0_observe"}}"#,
        )
        .unwrap();
        preserve_evolution(&mut incoming, &disk);
        assert_eq!(
            incoming
                .evolution
                .as_ref()
                .and_then(|v| v.get("activation_state"))
                .and_then(|v| v.as_str()),
            Some("s0_observe")
        );
        // 盘上没有 → 前端即便带了也清掉（该块只归 evolution 模块写，设置页无编辑入口）
        let mut incoming2: BotConfig = serde_json::from_str("{}").unwrap();
        incoming2.evolution = Some(serde_json::json!({"x": 1}));
        let disk2: BotConfig = serde_json::from_str("{}").unwrap();
        preserve_evolution(&mut incoming2, &disk2);
        assert!(incoming2.evolution.is_none());
    }
}
