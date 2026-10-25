//! Bot 配置模块的 tauri 命令（7 个）+ perm_mode 公开 helper。
//!
//! - `bot_get_config` 设置页读取（含老配置迁移兜底）
//! - `bot_set_config` 设置页保存（key 走 keyring，配置文件强制剥 key）
//! - `bot_set_active_model` 聊天区 🧠 下拉窄口径切 active 模型
//! - `bot_log_read` 设置页读 bot.log 审计日志
//! - `bot_test_connection` 厂商详情页模型行「测试连接」（只判 HTTP 状态）
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

// perm_mode

/// 当前授权模式（读 bot-config.json；缺文件/缺字段/非法值都回 Ask）
pub fn perm_mode(app: &AppHandle) -> PermMode {
    PermMode::from_cfg(io::load_config(app).perm_mode.as_deref())
}

/// 当前任务卡归档天数（读 bot-config.json；None → 7 默认；钳 1..=365）。
/// migration 兜底归档（run.rs 阶段一）与前端 applyArchiveRule 同源此值，
/// 两侧阈值恒一致（前端经 BotConfigView.archiveAfterDays）。
pub fn archive_after_days(app: &AppHandle) -> u32 {
    super::types::resolve_archive_after_days(io::load_config(app).archive_after_days)
}

// bot_get_config

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
    // 已存 key 的厂商列表：keyring 无法枚举条目，按配置里出现过的厂商名逐个探测。
    // 真实 keyring 故障透传 Err（与 has_api_key 同策略）
    let mut vendor_names: Vec<String> = cfg
        .models_by_provider
        .as_ref()
        .map(|m| {
            m.openai
                .iter()
                .chain(m.anthropic.iter())
                .filter_map(|e| e.vendor.clone())
                .collect()
        })
        .unwrap_or_default();
    vendor_names.sort();
    vendor_names.dedup();
    let mut vendor_keys = Vec::new();
    for v in vendor_names {
        if keyring::has_vendor_key(&v)? {
            vendor_keys.push(v);
        }
    }
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
        max_function_calls: cfg.max_function_calls,
        final_selfcheck_round_gap: cfg.final_selfcheck_round_gap,
        selfcheck_model_id: cfg.selfcheck_model_id,
        // 回落成内置默认再给前端：设置页编辑的是「当前生效表」，
        // 让它对着空列表编辑会以为内置词都被人删了
        guard_claim_keywords: cfg
            .guard_claim_keywords
            .clone()
            .unwrap_or_else(crate::bot::registry::default_execution_claim_patterns),
        guard_claim_default_keywords: crate::bot::registry::default_execution_claim_patterns(),
        verifier_retry_max: cfg.verifier_retry_max,
        archive_after_days: cfg.archive_after_days,
        perm_mode: cfg.perm_mode,
        api_provider: cfg.api_provider,
        max_tokens: cfg.max_tokens,
        reasoning_effort: cfg.reasoning_effort,
        models_by_provider: cfg.models_by_provider,
        active_model_id: cfg.active_model_id,
        ui_font_size: cfg.ui_font_size,
        memory_consolidation: cfg.memory_consolidation.unwrap_or_default(),
        memory_control: cfg.memory_control,
        memory_tuning: cfg.memory_tuning,
        mcp_servers: cfg.mcp_servers.unwrap_or_default(),
        disabled_vendors: cfg.disabled_vendors,
        vendor_keys,
        verified_vendors: cfg.verified_vendors,
        max_rounds: cfg.max_rounds,
        history_budget_chars: cfg.history_budget_chars,
        history_truncate_target_pct: cfg.history_truncate_target_pct,
        reasoning_echo_enabled: cfg.reasoning_echo_enabled,
        subagent_max_turns: cfg.subagent_max_turns,
        subagent_max_wall_secs: cfg.subagent_max_wall_secs,
        search_max_results: cfg.search_max_results,
        max_tool_output_bytes: cfg.max_tool_output_bytes,
        ask_budget: cfg.ask_budget,
        tool_rules: cfg.tool_rules,
    })
}

/// tool_rules 落盘清洗（纯函数单测直打）：tool 名 trim 后非空、action ∈
/// {allow, ask, deny}、同 tool 首条保留（评估本就首中即停，存重复无意义）；
/// 清洗后为空 → None（文件不留空数组）。
pub(crate) fn sanitize_tool_rules(
    rules: Option<Vec<super::types::ToolRule>>,
) -> Option<Vec<super::types::ToolRule>> {
    let rules = rules?;
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<super::types::ToolRule> = Vec::new();
    for r in rules {
        let tool = r.tool.trim().to_string();
        // dedup 按 trim 后的 tool 名（与评估侧 r.tool != tool 的字面比较一致）
        if tool.is_empty()
            || !matches!(r.action.as_str(), "allow" | "ask" | "deny")
            || !seen.insert(tool.clone())
        {
            continue;
        }
        out.push(super::types::ToolRule {
            tool,
            action: r.action,
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// bot_set_config

/// 触发词表防丢（纯函数，单测直打）：incoming 为 None 说明前端这次**没带**
/// 这个字段（整体替换写的老毛病），盘上有值就留着。
///
/// 只挡「漏传」，不挡「清空」——用户显式存空数组（Some(vec![])）是关掉
/// 执行类触发器，语义是另一回事，照写不误。
pub(crate) fn preserve_guard_keywords(incoming: &mut BotConfig, disk: &BotConfig) {
    if incoming.guard_claim_keywords.is_none() {
        incoming.guard_claim_keywords = disk.guard_claim_keywords.clone();
    }
}

/// 设置页整体替换写不丢「设置页无编辑入口」的字段（纯函数，单测直打）：
/// **盘上值权威**。
///
/// 这四个字段前端视图与保存载荷里都没有：
/// - `final_selfcheck_round_gap` / `selfcheck_model_id` / `verifier_retry_max`
///   —— 只归终稿自检链路读改，设置页没有输入框；
/// - `mcp_servers` —— 由 MCP 面板的 mcp_server_* 命令单独落盘，
///   设置页碰它就等于「改任何一个开关顺手清空所有 MCP 服务器」。
///
/// 盘上值写入时已钳制过，回填不再二次钳制。
pub(crate) fn preserve_non_ui_fields(incoming: &mut BotConfig, disk: &BotConfig) {
    incoming.final_selfcheck_round_gap = disk.final_selfcheck_round_gap;
    incoming.selfcheck_model_id = disk.selfcheck_model_id.clone();
    incoming.verifier_retry_max = disk.verifier_retry_max;
    incoming.mcp_servers = disk.mcp_servers.clone();
}

/// 保存配置。api_key / tavily_key / brave_key / vendor_key 几个顶层参数同语义：
/// Some(非空) 写入系统凭据存储并覆盖；None/空串不动已存的 key。
/// （Tavily/Brave key 与主 key 同模式，不落配置文件；
/// vendor_key 按厂商名分条目存储——厂商页一厂一 key）
#[derive(serde::Deserialize)]
pub struct VendorKeyParam {
    pub vendor: String,
    pub key: String,
}

/// 可用性失效（纯函数，单测直打）：已验证厂商名单剔除三类——
/// 1) key 被本次保存覆盖的厂商（新 key 未验证过）；
/// 2) 配置里已消失的厂商（删除厂商/重命名）；
/// 3) 条目签名（协议 + Base URL）集合变了的厂商——改 URL、换协议搬移、
///    增删条目都体现为集合差异；集合无序（BTreeSet），条目顺序变化不误剔。
pub(crate) fn prune_verified_vendors(
    incoming: &mut BotConfig,
    disk: &BotConfig,
    key_overwritten: Option<&str>,
) {
    // 厂商 → 条目签名集合：(协议, base_url) 无序去重
    fn sig_sets(
        cfg: &BotConfig,
    ) -> std::collections::BTreeMap<String, std::collections::BTreeSet<(String, String)>> {
        let mut m: std::collections::BTreeMap<
            String,
            std::collections::BTreeSet<(String, String)>,
        > = Default::default();
        if let Some(mbp) = &cfg.models_by_provider {
            for (proto, list) in [("openai", &mbp.openai), ("anthropic", &mbp.anthropic)] {
                for e in list {
                    if let Some(v) = &e.vendor {
                        m.entry(v.clone())
                            .or_default()
                            .insert((proto.to_string(), e.base_url.clone()));
                    }
                }
            }
        }
        m
    }
    let new_sigs = sig_sets(incoming);
    let old_sigs = sig_sets(disk);
    incoming.verified_vendors.retain(|v| {
        if Some(v.as_str()) == key_overwritten {
            return false;
        }
        match (new_sigs.get(v), old_sigs.get(v)) {
            (Some(n), Some(o)) => n == o,
            _ => false,
        }
    });
}

/// Agent 运行参数落盘前钳制（None = 内置默认不动）。
///
/// 边界单源：bot::params clamp_* / db::clamp_subagent_max_turns /
/// py::runtime::clamp_timeout_secs，与读侧 resolver 共用同一组数字——写侧钳过的
/// 值就是运行时生效的值，设置页显示的与实际跑的是同一个数。
///
/// 单独成函数是为了能单测：`bot_set_config` 要 AppHandle，测不到里面这一段。
fn clamp_agent_params(config: &mut BotConfig) {
    config.max_rounds = config.max_rounds.map(crate::bot::params::clamp_max_rounds);
    config.history_budget_chars = config
        .history_budget_chars
        .map(crate::bot::params::clamp_history_budget_chars);
    config.history_truncate_target_pct = config
        .history_truncate_target_pct
        .map(crate::bot::params::clamp_history_truncate_target_pct);
    config.reasoning_echo_enabled = config
        .reasoning_echo_enabled
        .map(crate::bot::params::clamp_reasoning_echo_enabled);
    config.subagent_max_turns = config
        .subagent_max_turns
        .map(crate::db::clamp_subagent_max_turns);
    config.subagent_max_wall_secs = config
        .subagent_max_wall_secs
        .map(crate::bot::params::clamp_subagent_max_wall_secs);
    config.search_max_results = config
        .search_max_results
        .map(crate::bot::params::clamp_search_max_results);
    config.ask_budget = config.ask_budget.map(crate::bot::params::clamp_ask_budget);
    // 工具调用熔断上限：写侧此前**没有**钳制，只有模型循环派生链末端的一行
    // `.clamp(1, 500)`——于是设置页显示用户设的值（999）、运行时按另一个值
    // 走（500），用户看到的和实际生效的不是一回事。写侧补上钳制后落盘值
    // = 生效值；边界仍是 params::clamp_max_function_calls 这一个数。
    config.max_function_calls = config
        .max_function_calls
        .map(crate::bot::params::clamp_max_function_calls);
    // 工具输出截断上限（0 = 显式关闭不动，其余钳到 params 的上限）——原先这个
    // 字段没有任何写侧钳制，只有模型循环生效点的一行 `.min(200_000)`，于是设置页
    // 显示用户设的值、运行时按另一个值走
    config.max_tool_output_bytes = config
        .max_tool_output_bytes
        .map(crate::bot::params::clamp_max_tool_output_bytes);
    // Python 执行超时：运行期钳在 py::runtime::resolve_timeout，但那条只在
    // **执行时**生效——写侧不钳的话设置页写下去 10000、显示 10000、实际跑 300。
    // 写侧用同一个上界钳一遍（边界仍是 py::runtime 那一个常量）。
    config.python_timeout_secs = config
        .python_timeout_secs
        .map(crate::py::runtime::clamp_timeout_secs);
    // 全局 max_tokens：读侧 resolve_max_tokens 会钳到 256..=200000，写侧不钳的话
    // 落盘的是原值、运行时按钳过的值走（手改过 config 的场景）
    config.max_tokens = config
        .max_tokens
        .map(crate::bot::config::schema::clamp_max_tokens);
}

#[tauri::command]
pub fn bot_set_config(
    app: AppHandle,
    config: BotConfig,
    api_key: Option<String>,
    tavily_key: Option<String>,
    brave_key: Option<String>,
    vendor_key: Option<VendorKeyParam>,
) -> CommandResult<()> {
    if let Some(k) = api_key {
        let k = k.trim();
        if !k.is_empty() {
            keyring::write_api_key(k)?;
        }
    }
    let mut key_overwritten: Option<String> = None;
    if let Some(vk) = vendor_key {
        let (v, k) = (vk.vendor.trim(), vk.key.trim());
        if !v.is_empty() && !k.is_empty() {
            keyring::write_vendor_key(v, k)?;
            key_overwritten = Some(v.to_string());
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
    // 归档天数落盘前钳制（None 保留 = 默认 7；Some(0)/超界钳到 1..=365）
    config.archive_after_days = config
        .archive_after_days
        .map(|n| n.clamp(1, super::types::MAX_ARCHIVE_DAYS));
    // Agent 运行参数落盘前钳制（None = 内置默认不动；与 archive 同款）——
    // 边界单源：bot::params clamp_* / db::clamp_subagent_max_turns，与读侧 resolver 共用
    clamp_agent_params(&mut config);
    // 终稿自检间隔落盘前钳制（0..=100：0 = 关闭；上限放宽到 100 留充足调整空间，
    // 真实触发频率受 final_text 字数 / function_calls / round gap 共同约束）
    config.final_selfcheck_round_gap = config.final_selfcheck_round_gap.map(|n| n.clamp(0, 100));
    // 侧调模型条目 id：trim + 空串归一 None（设置页清空输入框 = 关闭侧调）
    config.selfcheck_model_id = config
        .selfcheck_model_id
        .take()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // verifier 重试上限落盘前钳制（0..=VERIFIER_RETRY_MAX_CAP：0 = 不重试；上限 3
    // 防止无限循环烧 token——真实 3 次还失败的任务属于硬阻塞不如直接如实汇报用户）
    config.verifier_retry_max = config
        .verifier_retry_max
        .map(crate::bot::params::clamp_verifier_retry_max);
    // tool_rules 清洗——空 tool 名/非法 action/重复 tool 剔除；空表归一 None
    config.tool_rules = sanitize_tool_rules(config.tool_rules.take());
    // 前端整体替换写不丢「设置页无编辑入口」的字段。读盘回填必须与写**同锁**
    //（评审 HIGH：无锁 load → 有锁 write 之间 persist_last_run 等并发写会被
    // 本写覆盖——丢更新窗口）；write_bot_config_file 自带加锁不可重入，走 _locked 变体
    let _g = super::io::lock_config_write();
    let disk = super::io::load_config(&app);
    preserve_guard_keywords(&mut config, &disk);
    preserve_non_ui_fields(&mut config, &disk);
    prune_verified_vendors(&mut config, &disk, key_overwritten.as_deref());
    super::io::write_bot_config_file_locked(&crate::db::data_dir(&app), config)
}

// bot_set_active_model

/// 纯逻辑（单测锚点）：校验 model_id 存在于**任一协议**的模型列表并置为 active。
/// 聊天区 🧠 下拉双协议同列展示：命中**另一协议**的模型 → 连协议一起切
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
    // 目标条目带厂商且未通过连接测试 → 拒绝切换（与请求侧 ensure_vendor_verified
    // 同一不变量：active 只指向已验证厂商；聊天下拉已过滤，这里兜非下拉入口）
    let target_vendor = mbp
        .openai
        .iter()
        .chain(mbp.anthropic.iter())
        .find(|e| e.id == model_id)
        .and_then(|e| e.vendor.clone());
    schema::ensure_vendor_verified(target_vendor.as_deref(), &cfg.verified_vendors)
        .map_err(CommandError::from)?;
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

/// 窄口径切换 active 模型（聊天区 🧠 下拉专用）：读盘上最新配置（单一事实源），
/// 只动目标协议的 active_model_id（双协议同列：跨协议选中时连 api_provider
/// 一起切），派生 base_url/model 老字段后落盘。
/// **不做整份配置写回**——避免前端旧快照覆盖设置页并发修改的字段（白名单/开关等）。
/// key 相关路径完全不触碰。返回切换后的 BotConfigView，前端免二次读取。
#[tauri::command]
pub fn bot_set_active_model(app: AppHandle, model_id: String) -> CommandResult<BotConfigView> {
    // 读改写全程持 CONFIG_WRITE_LOCK——与 bot_set_config 同锁，消除并发丢更新
    let _g = io::lock_config_write();
    let mut cfg = io::load_config(&app);
    schema::migrate_legacy_models(&mut cfg);
    apply_active_model_switch(&mut cfg, &model_id)?;
    schema::derive_legacy_fields_from_active(&mut cfg);
    io::write_bot_config_file_locked(&crate::db::data_dir(&app), cfg)?;
    // bot_get_config 内 migrate_legacy_key/migrate_search_keys 会再取同一把非重入
    // CONFIG_WRITE_LOCK——必须先放本函数的写锁再读，否则必然自锁死。
    drop(_g);
    bot_get_config(app)
}

/// 授权模式落盘的纯函数：只认四个合法值，非法值按 Ask 归一（与
/// `PermMode::from_cfg` 同口径——安全默认偏严，不因为前端传了脏值就放宽成 yolo）。
pub fn apply_perm_mode(cfg: &mut BotConfig, mode: &str) -> PermMode {
    let m = PermMode::from_cfg(Some(mode));
    cfg.perm_mode = Some(m.as_str().to_string());
    m
}

/// 窄口径改授权模式（聊天区 🛡 下拉专用）：读盘上最新配置（单一事实源），
/// 只动 perm_mode 一个字段后落盘。
/// **不做整份配置写回**——与 bot_set_active_model 同理：整份回写会用前端旧快照
/// 覆盖设置页并发修改的字段（白名单/开关/密钥开关等）。
/// key 相关路径完全不触碰。返回更新后的 BotConfigView，前端免二次读取。
#[tauri::command]
pub fn bot_set_perm_mode(app: AppHandle, mode: String) -> CommandResult<BotConfigView> {
    // 读改写全程持 CONFIG_WRITE_LOCK——与 bot_set_config / bot_set_active_model
    // 同锁，消除并发丢更新
    let _g = io::lock_config_write();
    let mut cfg = io::load_config(&app);
    let applied = apply_perm_mode(&mut cfg, &mode);
    io::write_bot_config_file_locked(&crate::db::data_dir(&app), cfg)?;
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "bot.perm_mode_set",
        &[("mode", applied.as_str().to_string())],
    );
    // bot_get_config 内的 migrate_* 会再取同一把非重入 CONFIG_WRITE_LOCK——
    // 必须先放写锁再读，否则必然自锁死。
    drop(_g);
    bot_get_config(app)
}

/// 清除指定厂商的 API Key（keyring 条目按厂商名分存）；
/// 先把厂商移出已验证名单（配置落盘成功）再删 key——顺序保证失败时
/// fail-closed（宁可用不了，不留「key 没了但还显示已验证」的假绿）。
/// 残余中间态（第二步删 key 失败）：名单已移除但 key 还在，用户会看到报错，
/// 下次 test_connection 会凭仍可读的 key 重新验证回绿——失败方向与
/// fail-closed 一致且可重试恢复，不改先移名单后删 key 的次序。
#[tauri::command]
pub fn bot_clear_vendor_key(app: AppHandle, vendor: String) -> CommandResult<()> {
    let vendor = vendor.trim();
    super::io::update_config_file(&app, |cfg| {
        cfg.verified_vendors.retain(|v| v != vendor);
    })?;
    keyring::delete_vendor_key(vendor)
}

pub use super::audit::bot_log_read;

/// 记忆检索参数读取（memoryTuning；已过 clamped，前端只见合法值）
#[tauri::command]
pub fn memory_tuning_get(app: AppHandle) -> CommandResult<crate::memory::MemoryTuning> {
    Ok(io::read_memory_tuning(&app))
}

/// 记忆检索参数写回（None = 移除配置块回落默认；写入前服务端 clamped 兜底）
#[tauri::command]
pub fn memory_tuning_set(
    app: AppHandle,
    tuning: Option<crate::memory::MemoryTuning>,
) -> CommandResult<()> {
    io::write_memory_tuning_at(&io::config_path(&app), tuning).map_err(CommandError::from)
}

#[cfg(test)]
/// bot_set_config 的「盘上值回填」测试。回填的是前端整体替换写碰不到、
/// 但又不能被整体替换写清掉的字段：设置页无编辑入口的配置项与
/// 触发词表（前端有入口，所以只挡漏传、不挡显式清空）。
mod preserve_tests {
    use super::*;

    /// 设置页保存不得清空「设置页无编辑入口」的字段。
    /// 这四个字段不在前端载荷里，整体替换写会把它们从磁盘抹掉——
    /// 改任何一个开关就会连带清空 MCP 服务器列表。
    #[test]
    fn preserve_non_ui_fields_keeps_disk_values_when_incoming_omits_them() {
        use crate::bot::mcp::config::McpServerConfig;
        let srv = |id: &str| McpServerConfig {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        };
        let disk = BotConfig {
            final_selfcheck_round_gap: Some(7),
            selfcheck_model_id: Some("m-selfcheck".into()),
            verifier_retry_max: Some(2),
            mcp_servers: Some(vec![srv("srv-a"), srv("srv-b")]),
            ..Default::default()
        };
        // 前端整体替换写：这些字段一个都没带
        let mut incoming = BotConfig::default();

        preserve_non_ui_fields(&mut incoming, &disk);

        assert_eq!(incoming.final_selfcheck_round_gap, Some(7));
        assert_eq!(incoming.selfcheck_model_id.as_deref(), Some("m-selfcheck"));
        assert_eq!(incoming.verifier_retry_max, Some(2));
        assert_eq!(
            incoming.mcp_servers.as_deref().map(|v| v.len()),
            Some(2),
            "MCP 服务器列表必须原样保住"
        );
        assert_eq!(incoming.mcp_servers.as_ref().unwrap()[1].id, "srv-b");
    }

    /// 盘上为空时不应凭空造值（回填不是初始化）。
    #[test]
    fn preserve_non_ui_fields_leaves_defaults_when_disk_empty() {
        let mut incoming = BotConfig::default();
        preserve_non_ui_fields(&mut incoming, &BotConfig::default());
        assert_eq!(incoming.final_selfcheck_round_gap, None);
        assert_eq!(incoming.selfcheck_model_id, None);
        assert_eq!(incoming.verifier_retry_max, None);
        assert_eq!(incoming.mcp_servers, None);
    }

    #[test]
    fn prune_verified_vendors_drops_overwritten_missing_and_url_changed() {
        use crate::bot::config::types::{ModelEntry, ModelsByProvider};
        let entry = |vendor: &str, url: &str| ModelEntry {
            id: "m1".into(),
            label: "m".into(),
            base_url: url.into(),
            model: "m".into(),
            vendor: Some(vendor.into()),
            enabled: true,
            context_k: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            system_prompt: None,
        };
        let cfg_with = |entries: Vec<ModelEntry>| BotConfig {
            models_by_provider: Some(ModelsByProvider {
                openai: entries,
                anthropic: vec![],
            }),
            ..Default::default()
        };
        // 盘上：DeepSeek/Kimi 已验证；新配置：DeepSeek URL 未变、Kimi URL 变了、
        // OpenAI 已被删除、MiniMax 本次被覆盖 key
        let disk = cfg_with(vec![
            entry("DeepSeek", "https://a"),
            entry("Kimi", "https://k1"),
        ]);
        let mut incoming = cfg_with(vec![
            entry("DeepSeek", "https://a"),
            entry("Kimi", "https://k2"),
            entry("MiniMax", "https://m"),
        ]);
        incoming.verified_vendors = vec![
            "DeepSeek".into(),
            "Kimi".into(),
            "OpenAI".into(),
            "MiniMax".into(),
        ];
        prune_verified_vendors(&mut incoming, &disk, Some("MiniMax"));
        // 只剩 DeepSeek：Kimi（URL 变）/OpenAI（消失）/MiniMax（key 覆盖）全部失效
        assert_eq!(incoming.verified_vendors, vec!["DeepSeek".to_string()]);

        // 条目顺序变化（重应用预设会把条目挪到列表尾）不误剔
        let disk2 = cfg_with(vec![
            entry("DeepSeek", "https://a"),
            entry("Kimi", "https://k"),
        ]);
        let mut incoming2 = cfg_with(vec![
            entry("Kimi", "https://k"),
            entry("DeepSeek", "https://a"),
        ]);
        incoming2.verified_vendors = vec!["DeepSeek".into(), "Kimi".into()];
        prune_verified_vendors(&mut incoming2, &disk2, None);
        assert_eq!(
            incoming2.verified_vendors,
            vec!["DeepSeek".to_string(), "Kimi".to_string()],
            "仅顺序变化不应失效"
        );

        // 换协议搬移（同 URL）也失效——验证是按旧协议测的
        let disk3 = cfg_with(vec![entry("DeepSeek", "https://a")]);
        let mut incoming3 = BotConfig {
            models_by_provider: Some(crate::bot::config::types::ModelsByProvider {
                openai: vec![],
                anthropic: vec![entry("DeepSeek", "https://a")],
            }),
            ..Default::default()
        };
        incoming3.verified_vendors = vec!["DeepSeek".into()];
        prune_verified_vendors(&mut incoming3, &disk3, None);
        assert!(incoming3.verified_vendors.is_empty(), "换协议应失效");
    }

    #[test]
    fn prune_verified_vendors_kept_when_only_entry_params_change() {
        // 可用性只看 key / URL / 协议：编辑条目级推理参数
        //（temperature/top_p/max_tokens/system_prompt）不改变条目签名，
        // verified_vendors 必须原样保留
        use crate::bot::config::types::{ModelEntry, ModelsByProvider};
        let entry = |vendor: &str, params: bool| ModelEntry {
            id: "m1".into(),
            label: "m".into(),
            base_url: "https://a".into(),
            model: "m".into(),
            vendor: Some(vendor.into()),
            enabled: true,
            context_k: None,
            temperature: params.then_some(0.7),
            top_p: params.then_some(0.9),
            max_tokens: params.then_some(16_384),
            system_prompt: params.then(|| "用中文回复".to_string()),
        };
        let cfg_with = |entries: Vec<ModelEntry>| BotConfig {
            models_by_provider: Some(ModelsByProvider {
                openai: entries,
                anthropic: vec![],
            }),
            ..Default::default()
        };
        let disk = cfg_with(vec![entry("DeepSeek", false)]);
        let mut incoming = cfg_with(vec![entry("DeepSeek", true)]);
        incoming.verified_vendors = vec!["DeepSeek".into()];
        prune_verified_vendors(&mut incoming, &disk, None);
        assert_eq!(
            incoming.verified_vendors,
            vec!["DeepSeek".to_string()],
            "仅条目参数变化不应失效"
        );
    }

    #[test]
    fn preserve_guard_keywords_only_fills_missing_not_explicit_clear() {
        // 前端整体替换写：没带这个字段（None）时保留盘上值，否则设置页任意
        // 一次保存都会把用户的词表清掉
        let disk = BotConfig {
            guard_claim_keywords: Some(vec!["我搞定了".to_string()]),
            ..Default::default()
        };
        let mut incoming = BotConfig::default();
        preserve_guard_keywords(&mut incoming, &disk);
        assert_eq!(
            incoming.guard_claim_keywords,
            Some(vec!["我搞定了".to_string()]),
            "漏传时应保留盘上词表"
        );

        // 用户显式存空数组 = 关掉这道触发器，是合法取值，照写不误
        let mut clearing = BotConfig {
            guard_claim_keywords: Some(Vec::new()),
            ..Default::default()
        };
        preserve_guard_keywords(&mut clearing, &disk);
        assert_eq!(
            clearing.guard_claim_keywords,
            Some(Vec::new()),
            "显式清空不该被回填"
        );

        // 用户自定义过 = 尊重用户的，不被盘上默认覆盖
        let mut custom = BotConfig {
            guard_claim_keywords: Some(vec!["完事了".to_string()]),
            ..Default::default()
        };
        preserve_guard_keywords(&mut custom, &disk);
        assert_eq!(
            custom.guard_claim_keywords,
            Some(vec!["完事了".to_string()])
        );
    }
}

// bot_test_connection

/// 连接测试结果（厂商详情页模型行插头按钮）：只判 HTTP 状态，不解析 body。
/// ok = 2xx；非 2xx（401/403 等）→ ok:false + status；网络错/超时 → ok:false + error。
/// status/error 为 None 时不序列化，前端按字段有无区分失败类别。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 可测内核（注入 client + key，不碰 AppHandle/keyring，集成测试直打）：
/// 探活路径按协议归一化——openai → GET {base}/models；anthropic →
/// GET {base}/v1/models（base 已含 /v1 结尾则直接拼 /models，与
/// anthropic_messages_url 同款「两种填法都可用」）。
/// 鉴权：openai → Authorization: Bearer；anthropic → x-api-key + anthropic-version
///（apply_anthropic_auth）。有 key 才带鉴权头（部分代理 /models 免鉴权）。
/// 5s 超时（连接测试是前台交互，不能让慢厂商挂住 UI）。
pub async fn probe_connection(
    client: &reqwest::Client,
    base_url: &str,
    api_format: &str,
    api_key: &str,
) -> ConnectionTestResult {
    let b = base_url.trim_end_matches('/');
    // 误粘完整端点的填法先剥掉端点尾段（与 anthropic_messages_url 同规则），
    // 否则探测路径叠成 /v1/messages/v1/models、/chat/completions/models
    let b = b
        .strip_suffix("/v1/messages")
        .or_else(|| b.strip_suffix("/messages"))
        .or_else(|| b.strip_suffix("/chat/completions"))
        .unwrap_or(b);
    let url = if api_format == "anthropic" {
        if b.ends_with("/v1") {
            format!("{b}/models")
        } else {
            format!("{b}/v1/models")
        }
    } else {
        format!("{b}/models")
    };
    let mut req = client.get(&url);
    let key = api_key.trim();
    if !key.is_empty() {
        if api_format == "anthropic" {
            req = crate::bot_anthropic::apply_anthropic_auth(req, key);
        } else {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
    }
    match req.timeout(std::time::Duration::from_secs(5)).send().await {
        Ok(resp) => {
            let status = resp.status();
            ConnectionTestResult {
                ok: status.is_success(),
                status: Some(status.as_u16()),
                error: None,
            }
        }
        Err(e) => ConnectionTestResult {
            ok: false,
            status: None,
            error: Some(e.to_string()),
        },
    }
}

/// 厂商详情页模型行「测试连接」（参照 open-webui POST /verify）：
/// key 从系统凭据存储读：传了 vendor 时该厂商的厂商级 key 优先，没配过才回落
/// 全局主 key；厂商 keyring 真实故障（钥匙串锁定/权限拒绝）上抛 KeyringError，
/// 不静默拿全局 key 去探测（会报出误导性的「厂商鉴权失败」401）。
/// 全局主 key 读不到（未配置 / 故障）按无 key 探活——无 key 时厂商会回 401，
/// 状态如实返回。
/// 结果落盘可用性：ok → 厂商进 verified_vendors（设置页绿点 + 聊天下拉放行），
/// 失败 → 移出；写盘失败记审计，不阻断测试结果的返回。
#[tauri::command]
pub async fn bot_test_connection(
    app: AppHandle,
    base_url: String,
    api_format: String,
    vendor: Option<String>,
) -> CommandResult<ConnectionTestResult> {
    use crate::bot_model_loop::shared_llm_client;

    let vendor = vendor
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    // 探活带哪个 key：厂商页用该厂商自己的 key；厂商没配 key 就**不带鉴权**探，
    // 不借用全局主 key——探的 base_url 是用户填的（可能指向任何第三方），
    // 把主 key 发过去等于把它交出去。无厂商的老条目用全局主 key。
    let vendor_key = match &vendor {
        Some(v) => Some(keyring::read_vendor_key(v)?),
        None => None,
    };
    let global_key = keyring::read_api_key().unwrap_or_default();
    let key = match keyring::probe_auth(
        vendor.as_deref(),
        vendor_key.as_deref(),
        global_key.as_str(),
    ) {
        keyring::ProbeAuth::WithKey(k) => k,
        keyring::ProbeAuth::Anonymous => String::new(),
    };
    let authenticated = !key.trim().is_empty();
    let r = probe_connection(shared_llm_client(), &base_url, &api_format, &key).await;
    // 没带鉴权（厂商没配 key）时，2xx 只说明端点通着，不说明 key 有效——
    // 不能凭它把厂商转成「已验证」（转正后请求侧会拿主 key 打这个 base_url）。
    let verified = r.ok && authenticated;
    if let Some(v) = vendor.as_deref() {
        let ok = verified;
        if let Err(e) = super::io::update_config_file(&app, |cfg| {
            if ok {
                if !cfg.verified_vendors.iter().any(|x| x == v) {
                    cfg.verified_vendors.push(v.to_string());
                }
            } else {
                cfg.verified_vendors.retain(|x| x != v);
            }
        }) {
            // 写盘失败 = 验证态没落盘（下次启动回到未验证）：记审计，不回改测试结果
            super::audit::audit_log(
                &app,
                &format!(
                    "bot_config.verified_vendors_persist_failed | {}",
                    super::audit::escape_for_log(&e.to_string(), 200)
                ),
            );
        }
    }
    // 未配 key：如实说清「这次只探了连通性」，别报「连接测试通过」。
    // 无厂商的老条目与厂商条目分开说——unwrap_or_default 会把空厂商名
    // 拼进「厂商「」还没配 API Key」，指路方向也是错的（该配全局 key）
    if r.ok && !authenticated {
        let hint = match vendor.as_deref() {
            Some(v) => format!(
                "厂商「{v}」还没配 API Key：本次只探了端点连通性（未带鉴权），不算连接测试通过。请先在厂商页填该厂商的 Key。"
            ),
            None =>
                "全局还没配 API Key：本次只探了端点连通性（未带鉴权），不算连接测试通过。请先在设置里填全局主 Key。"
                    .to_string(),
        };
        return Ok(ConnectionTestResult {
            ok: false,
            status: r.status,
            error: Some(hint),
        });
    }
    Ok(r)
}

#[cfg(test)]
mod test_connection_tests {
    use super::ConnectionTestResult;

    #[test]
    fn result_serializes_camel_case_and_skips_none() {
        let ok = ConnectionTestResult {
            ok: true,
            status: Some(200),
            error: None,
        };
        let json = serde_json::to_string(&ok).unwrap();
        assert_eq!(json, r#"{"ok":true,"status":200}"#);

        let err = ConnectionTestResult {
            ok: false,
            status: None,
            error: Some("boom".into()),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, r#"{"ok":false,"error":"boom"}"#);
    }
}

#[cfg(test)]
mod tool_rule_sanitize_tests {
    use super::super::types::ToolRule;
    use super::sanitize_tool_rules;

    fn rule(tool: &str, action: &str) -> ToolRule {
        ToolRule {
            tool: tool.into(),
            action: action.into(),
        }
    }

    /// 清洗：非法 action / 空 tool 剔除；同 tool 重复保留首条；tool 名 trim
    #[test]
    fn sanitize_filters_and_dedups() {
        let out = sanitize_tool_rules(Some(vec![
            rule(" edit_file ", "allow"),
            rule("edit_file", "deny"),   // 重复 → 丢（首条已收）
            rule("", "ask"),             // 空 tool → 丢
            rule("run_python", "bogus"), // 非法 action → 丢
            rule("web_search", "ask"),
        ]))
        .expect("有合法项不应归 None");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].tool, "edit_file");
        assert_eq!(out[0].action, "allow", "同 tool 首条保留");
        assert_eq!(out[1].tool, "web_search");
    }

    /// 全非法 / None / 空表 → None（文件不留空数组）
    #[test]
    fn sanitize_none_when_empty_or_dirty() {
        assert_eq!(sanitize_tool_rules(None), None);
        assert_eq!(sanitize_tool_rules(Some(vec![])), None);
        assert_eq!(sanitize_tool_rules(Some(vec![rule("x", "nope")])), None);
    }
}

#[cfg(test)]
mod perm_mode_tests {
    use super::super::types::{BotConfig, PermMode, ToolRule};
    use super::{apply_perm_mode, clamp_agent_params, sanitize_tool_rules};

    fn rule(tool: &str, action: &str) -> ToolRule {
        ToolRule {
            tool: tool.into(),
            action: action.into(),
        }
    }

    /// 授权档位落盘：四档原样写，非法值按 ask 归一。
    ///
    /// 非法值必须落到 **ask**（弹窗）而不是 yolo（全放行）：这里是唯一一处
    /// 「用户在前端点了某个东西」直接变成「机器人放行多大」的边界，
    /// 传错值就当成最宽档处理等于凭空放宽权限。
    #[test]
    fn apply_perm_mode_normalizes_illegal_to_ask() {
        for (input, want) in [
            ("strict", PermMode::Strict),
            ("ask", PermMode::Ask),
            ("auto", PermMode::Auto),
            ("yolo", PermMode::Yolo),
            (" yolo ", PermMode::Yolo),
            ("YOlo", PermMode::Ask),
            ("bogus", PermMode::Ask),
            ("", PermMode::Ask),
        ] {
            let mut cfg = BotConfig::default();
            let got = apply_perm_mode(&mut cfg, input);
            assert_eq!(got, want, "input={input:?}");
            // 落盘的是归一后的字符串，不是原始入参：否则下次读侧又按
            // from_cfg 归一次，界面上显示的档位与文件里的会对不上
            assert_eq!(
                cfg.perm_mode.as_deref(),
                Some(want.as_str()),
                "input={input:?} 落盘值应归一"
            );
        }
    }

    /// 档位切换不能碰别的字段：窄命令只动 perm_mode，
    /// 整份回写会覆盖设置页并发修改的白名单/开关。
    #[test]
    fn apply_perm_mode_touches_only_perm_mode() {
        let mut cfg = BotConfig {
            tool_rules: sanitize_tool_rules(Some(vec![rule("edit_file", "ask")])),
            max_rounds: Some(42),
            perm_mode: Some("ask".into()),
            ..Default::default()
        };
        let before_rules = cfg.tool_rules.clone();
        apply_perm_mode(&mut cfg, "yolo");
        assert_eq!(cfg.perm_mode.as_deref(), Some("yolo"));
        assert_eq!(cfg.max_rounds, Some(42), "max_rounds 不该被动过");
        assert_eq!(cfg.tool_rules, before_rules, "tool_rules 不该被动过");
    }

    /// 落盘值 = 生效值：越界配置必须在写侧就被钳到运行时真正认的那个数。
    ///
    /// 写侧不钳 = 设置页显示用户设的值、运行时按另一个值走（用户看到的和
    /// 实际生效的不是一回事）。`max_function_calls` 此前就漏在钳制列表外。
    #[test]
    fn clamp_agent_params_makes_stored_value_equal_effective_value() {
        let mut cfg = BotConfig {
            max_rounds: Some(999),
            max_function_calls: Some(9999),
            history_budget_chars: Some(1),
            subagent_max_turns: Some(0),
            subagent_max_wall_secs: Some(10),
            search_max_results: Some(99),
            ask_budget: Some(99),
            max_tool_output_bytes: Some(999_999),
            python_timeout_secs: Some(10_000),
            max_tokens: Some(999_999),
            ..Default::default()
        };
        clamp_agent_params(&mut cfg);
        // 每个落盘值都必须等于读侧 resolver 对同一份配置的解析结果
        assert_eq!(
            cfg.max_rounds,
            Some(crate::bot::params::clamp_max_rounds(999))
        );
        assert_eq!(
            cfg.max_function_calls,
            Some(crate::bot::params::clamp_max_function_calls(9999)),
            "熔断上限写侧此前不钳：设置页显示 9999、运行时按 500 走"
        );
        assert_eq!(cfg.max_function_calls, Some(500));
        assert_eq!(
            cfg.history_budget_chars,
            Some(crate::bot::params::clamp_history_budget_chars(1))
        );
        assert_eq!(
            cfg.subagent_max_turns,
            Some(crate::db::clamp_subagent_max_turns(0))
        );
        assert_eq!(
            cfg.subagent_max_wall_secs,
            Some(crate::bot::params::clamp_subagent_max_wall_secs(10))
        );
        assert_eq!(
            cfg.search_max_results,
            Some(crate::bot::params::clamp_search_max_results(99))
        );
        assert_eq!(
            cfg.ask_budget,
            Some(crate::bot::params::clamp_ask_budget(99))
        );
        assert_eq!(
            cfg.max_tool_output_bytes,
            Some(crate::bot::params::clamp_max_tool_output_bytes(999_999))
        );
        assert_eq!(
            cfg.python_timeout_secs,
            Some(crate::py::runtime::clamp_timeout_secs(10_000)),
            "Python 超时写侧此前不钳：设置页显示 10000、运行时按上限跑"
        );
        assert_eq!(
            cfg.python_timeout_secs,
            Some(crate::py::runtime::MAX_TIMEOUT_SECS),
            "落盘值必须等于生效值（天花板常量一改这里跟着走）"
        );
        // 全局 max_tokens：写侧此前不钳，页面显示用户填的原值、运行时按钳过的走
        assert_eq!(
            cfg.max_tokens,
            Some(crate::bot::config::schema::clamp_max_tokens(999_999)),
            "max_tokens 写侧此前不钳：设置页显示 999999、运行时按 200000 走"
        );
        assert_eq!(cfg.max_tokens, Some(200_000));
    }

    /// 钳制不碰「未配置」这一态：None 仍是 None（= 用内置默认），
    /// 且 0（工具输出截断的「显式关闭」）不能被当成非法值抹掉。
    #[test]
    fn clamp_agent_params_keeps_none_and_zero() {
        let mut cfg = BotConfig::default();
        clamp_agent_params(&mut cfg);
        assert_eq!(cfg.max_rounds, None, "未配置不该被写成 0 之类的值");
        assert_eq!(cfg.max_function_calls, None);
        assert_eq!(cfg.max_tool_output_bytes, None);

        let mut cfg = BotConfig {
            max_tool_output_bytes: Some(0),
            ..Default::default()
        };
        clamp_agent_params(&mut cfg);
        assert_eq!(
            cfg.max_tool_output_bytes,
            Some(0),
            "0 = 显式关闭截断，写侧不许抹成 null"
        );
    }
}
