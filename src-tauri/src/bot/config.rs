//! Bot 配置 + 审计底座（阶段 1 拆分后位置）。
//!
//! 原 bot.rs 行 55–1193 全部配置/keyring/审计/constants/check_len 代码搬入。
//! 七个测试段（2663–2763 / 2765–2814 / 2816–2909 / 2911–3020 /
//! 3022–3068 / 3392–3423 / 3425–3444）后续阶段搬入（本阶段不含）。
//!
//! 安全性（对齐《Harness 安全网关》需求）：
//! - API Key 存系统凭据存储（keyring），文件不落明文；
//!   例外：Linux 无 secret-service（无 dbus 会话）时降级 `bot-api-key.txt`
//!   明文文件（chmod 0600）+ WARN 审计，否则 key 根本存不住。

use crate::error::{CommandError, CommandResult};
use serde::{Deserialize, Serialize};
use std::io::Write;
use tauri::AppHandle;

// ───────────────────────── API 配置 ─────────────────────────

/// 凭据存储条目：macOS 钥匙串 / Windows 凭据管理器。
/// service 名带版本后缀——key 存储格式升级时新开 `wmessage.bot.vN`，
/// 首次读取自动从上一版 service 迁移（见 migrate_legacy_keyring_entry），
/// 用户不必重新输入 key。
pub const KEYRING_SERVICE: &str = "wmessage.bot.v1";
/// 无版本后缀的历史 service 名（v0，带后缀方案之前的版本写入）。
/// 仅用于迁移：新条目为空时从它搬运，成功后删除旧条目。
pub const LEGACY_KEYRING_SERVICE: &str = "wmessage-bot";
pub const KEYRING_USER: &str = "api-key";

/// bot-config.json 当前 schema 版本。结构变更 +1 并在此登记迁移步骤
/// （风格对齐 migration.rs 的 `RulesFile::version`：文件自带版本字段 + serde 默认兜底）。
pub const BOT_CONFIG_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    BOT_CONFIG_SCHEMA_VERSION
}

/// Key 用途槽位：Tavily/Brave 搜索 key 统一进系统凭据存储，
/// 不再明文落 bot-config.json（无「低风险搜索 key」例外）。
/// 每个用途 = 独立的 keyring 用户名 + Linux 降级文件名；
/// 主 LLM key 保持既有条目（"api-key" / bot-api-key.txt）不变，存量用户零迁移感。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySlot {
    /// 主 LLM API Key（既有条目，不动）
    Llm,
    /// Tavily 搜索 key（原明文落 bot-config.json，迁移后进 keyring）
    Tavily,
    /// Brave 搜索 key（同上）
    Brave,
}

impl KeySlot {
    /// keyring 用户名（同一 service 下的条目名）
    pub fn keyring_user(&self) -> &'static str {
        match self {
            KeySlot::Llm => KEYRING_USER,
            KeySlot::Tavily => "tavily_api_key",
            KeySlot::Brave => "brave_api_key",
        }
    }
    /// Linux 降级明文文件名（数据目录下，0600）
    pub fn plaintext_filename(&self) -> &'static str {
        match self {
            KeySlot::Llm => "bot-api-key.txt",
            KeySlot::Tavily => "bot-tavily-key.txt",
            KeySlot::Brave => "bot-brave-key.txt",
        }
    }
    /// 槽位下标（进程级「service 迁移已探测」标记数组用）
    fn index(&self) -> usize {
        match self {
            KeySlot::Llm => 0,
            KeySlot::Tavily => 1,
            KeySlot::Brave => 2,
        }
    }
}

fn key_entry(slot: KeySlot) -> CommandResult<keyring::Entry> {
    key_entry_for_service(KEYRING_SERVICE, slot)
}

/// 指定 service 名的条目（v0 → v1 迁移需要同时访问新旧两个 service）。
fn key_entry_for_service(service: &str, slot: KeySlot) -> CommandResult<keyring::Entry> {
    keyring::Entry::new(service, slot.keyring_user())
        .map_err(|e| CommandError::KeyringError(format!("系统凭据存储不可用：{e}")))
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct BotConfig {
    /// OpenAI 兼容接口地址，如 https://api.deepseek.com/v1
    pub base_url: String,
    pub model: String,
    /// 仅用于旧版本迁移：老 bot-config.json 里的明文 key，读出迁入凭据存储后置 None 写回
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// pre-step 命中 Skill 时是否跳过外层主 LLM。
    /// true = auto 模式 bypass LLM，interactive 模式仍走 LLM 但 Skill body 注入 system prompt；
    /// false = LEGACY 旧链路（强制 pre_routed_skill = None，让 LLM 自由选 Skill）。
    /// 默认 true，老 bot-config.json 自动兼容（struct 级 #[serde(default)] + Default::default()）。
    pub bypass_llm_on_pre_step_hit: bool,
    /// 本地文件工具白名单目录：read_text_file/grep_files/list_files
    /// 只允许访问这些目录内路径。空 = 用内置默认（~/Desktop ~/Downloads ~/Documents + 任务卡绑定文件夹）；
    /// 非空 = 用户列表整体替换默认。
    pub allowed_dirs: Vec<String>,
    /// 仅用于旧版本迁移（Tavily key 存系统凭据存储，不再明文落盘）：
    /// 老 bot-config.json 里的明文 Tavily key，由 migrate_search_keys 读出迁入
    /// keyring 后置 None 写回。新代码读写 Tavily key 一律走 read/write_search_key。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tavily_key: Option<String>,
    /// 「Tavily 搜索」开关（可选）：None = 未显式设置，按旧行为自动
    /// （配了 tavilyKey 就当开启）；Some(true) = 强制走 Tavily；Some(false) = 强制
    /// Bing+百度双引擎（即使配了 key）。分流逻辑见 bot_web::resolve_search_route。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tavily_enabled: Option<bool>,
    /// 仅用于旧版本迁移（Brave key 存系统凭据存储，不再明文落盘）：
    /// 老 bot-config.json 里的明文 Brave key，由 migrate_search_keys 读出迁入
    /// keyring 后置 None 写回。新代码读写 Brave key 一律走 read/write_search_key。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brave_key: Option<String>,
    /// 「Brave 搜索」开关（可选）：语义与 tavily_enabled 对齐——
    /// None = 未显式设置，配了 braveKey 就当开启；Some(false) 强制不走 Brave。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brave_enabled: Option<bool>,
    /// run_python 默认超时秒数（可选）：None = 60s；工具参数 timeoutSecs 优先于此；
    /// 硬钳上限 300s（bot_py::resolve_timeout）。大计算（pandas 等）可调大。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub python_timeout_secs: Option<u64>,
    /// 文件/代码执行授权模式（Kimi CLI 风格）：
    /// "strict" = 白名单外硬拒（旧行为）；
    /// "ask"    = 白名单外弹授权窗（允许一次/始终允许该目录/拒绝）——默认；
    /// "yolo"   = 全放行不弹窗（文件工具 + run_python 免开关），仍记审计。
    /// None（老配置文件缺字段）= "ask"。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perm_mode: Option<String>,
    /// API 协议："openai" = OpenAI 兼容
    ///（/chat/completions + Bearer，旧行为）；"anthropic" = Anthropic 兼容
    ///（/v1/messages + x-api-key + anthropic-version）。None = openai，老配置零影响；
    /// 非法值按 openai 处理（ApiProvider::from_cfg 防御回退，与 perm_mode 同风格）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_provider: Option<String>,
    /// max_tokens（可选）：仅 Anthropic 模式使用（Anthropic 必填 max_tokens）；
    /// None = 默认 8192，钳制 256..=200000（resolve_max_tokens）。
    /// OpenAI 兼容模式不发送该字段（多数兼容网关不认识）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 每协议下的大模型列表：双协议各自独立维护一个
    /// ModelEntry 列表。设置页协议切换时整体切换显示；新增的 ModelEntry 落在当前
    /// 协议下。None = 老配置未迁移过来（load_config 时会从 base_url/model 兜底迁移）；
    /// 迁移完后写回落盘。
    /// 派生关系：当前协议 + active_model_id 决定 base_url/model/max_tokens 的真实值
    /// （bot_model_loop 不感知新结构，由 bot_set_config 落盘前回填派生字段）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub models_by_provider: Option<ModelsByProvider>,
    /// 每协议当前选中的模型 id；None = 该协议还没选 active。
    /// 切换协议时设置页据此取对应协议的 active 模型来填 baseUrl/model 输入框。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_model_id: Option<ActiveModelId>,
    /// 界面字体大小：small / standard / large / xlarge
    /// 老板拍板"目前字号为小"=默认 small。设置页「通用设置 → 外观」调。
    /// 全局 css 通过 documentElement[data-font-size] 走缩放。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ui_font_size: Option<String>,
    /// 定时记忆整理：开关 + 频率 + 上次整理时间。
    /// None = 默认（启用 + daily；见 ConsolidationConfig::default）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_consolidation: Option<crate::memory::consolidate::ConsolidationConfig>,
    /// 配置 schema 版本（迁移钩子）：缺失就补默认 + 写回（migrate_bot_config_schema）。
    /// 字段级 default 让老配置（无此字段）反序列化即拿到当前版本，「文件里没有这个
    /// key」的判定走 raw JSON（见 migrate_config_value），不靠反序列化结果。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
}

/// 单个模型条目：一个 (label, baseUrl, model) 三元组 + 稳定 id。
/// id 是前端 crypto.randomUUID() 生成的字符串，仅用于 React key + 标识 active；
/// 不参与 API 调用。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
}

/// 双协议下各自的模型列表；Vec 为空序列化时跳过，保持配置文件干净。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelsByProvider {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub openai: Vec<ModelEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub anthropic: Vec<ModelEntry>,
}

/// 双协议下各自的 active 模型 id；None = 该协议还没选 active。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ActiveModelId {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<String>,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com/v1".into(),
            // deepseek-chat 已被官方废弃，默认改 V4 Flash
            model: "deepseek-v4-flash".into(),
            api_key: None,
            bypass_llm_on_pre_step_hit: true, // 默认开启（bypass 外层主 LLM）
            allowed_dirs: Vec::new(),         // 空 = 内置默认白名单
            tavily_key: None,                 // 未配置 = 双引擎抓取
            tavily_enabled: None,             // 未显式设置 = 配了 key 就自动启用（旧行为）
            brave_key: None,                  // 未配置 = 不走 Brave
            brave_enabled: None,              // 未显式设置 = 配了 key 就自动启用（同 Tavily）
            python_timeout_secs: None,        // 未配置 = 60s 默认
            perm_mode: None,                  // 未配置 = ask（弹授权）
            api_provider: None,               // 未配置 = openai（旧行为）
            max_tokens: None,                 // 未配置 = 8192 默认（仅 Anthropic 模式用）
            models_by_provider: None,         // 未配置 = 设置页空列表（无默认厂商）
            active_model_id: None,            // 未配置 = 两协议都没选 active
            ui_font_size: None,               // 未配置 = small（老板拍板默认；前端读取时回退）
            memory_consolidation: None, // 未配置 = 启用 + daily（ConsolidationConfig::default）
            schema_version: BOT_CONFIG_SCHEMA_VERSION, // 新建配置即当前版本
        }
    }
}

/// API 协议枚举：配置字符串归一化，
/// 非法值回退 Openai（防御回退，与 PermMode::from_cfg 同风格）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiProvider {
    Openai,
    Anthropic,
}

impl ApiProvider {
    pub fn from_cfg(s: Option<&str>) -> Self {
        match s.map(|v| v.trim()) {
            Some("anthropic") => ApiProvider::Anthropic,
            _ => ApiProvider::Openai,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiProvider::Openai => "openai",
            ApiProvider::Anthropic => "anthropic",
        }
    }
}

// ─────────────── 双协议下大模型列表迁移/派生 ───────────────

/// 老配置 → 新结构一次性迁移：无 models_by_provider、但 base_url 或
/// model 非空时，在内存里补一份单条 ModelEntry + active_model_id，挂在
/// api_provider 协议下。**不写文件**——只在下一次用户保存时由 bot_set_config 一并
/// 落盘，避免无谓写盘 + 误清空老用户已配好的 key/base_url。
///
/// 老配置也是空（base_url+model 都空）→ 按老板「不设置默认厂商」要求保持空列表，
/// 让用户点「添加大模型」自己加。
fn migrate_legacy_models(cfg: &mut BotConfig) {
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
fn derive_legacy_fields_from_active(cfg: &mut BotConfig) {
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

/// max_tokens 默认值与合法范围（默认 8192；仅 Anthropic 模式发送）
pub const DEFAULT_MAX_TOKENS: u32 = 8192;
pub const MIN_MAX_TOKENS: u32 = 256;
pub const MAX_MAX_TOKENS: u32 = 200_000;

/// max_tokens 配置解析：None = 默认 8192；Some 钳制到 256..=200000
pub fn resolve_max_tokens(v: Option<u32>) -> u32 {
    v.unwrap_or(DEFAULT_MAX_TOKENS)
        .clamp(MIN_MAX_TOKENS, MAX_MAX_TOKENS)
}

/// 授权模式枚举：配置字符串归一化，非法值回退 Ask（安全默认偏严一侧的可用形态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermMode {
    Strict,
    Ask,
    Yolo,
}

impl PermMode {
    pub fn from_cfg(s: Option<&str>) -> Self {
        match s.map(|v| v.trim()) {
            Some("strict") => PermMode::Strict,
            Some("yolo") => PermMode::Yolo,
            _ => PermMode::Ask,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            PermMode::Strict => "strict",
            PermMode::Ask => "ask",
            PermMode::Yolo => "yolo",
        }
    }
}

/// 当前授权模式（读 bot-config.json；缺文件/缺字段/非法值都回 Ask）
pub fn perm_mode(app: &AppHandle) -> PermMode {
    PermMode::from_cfg(load_config(app).perm_mode.as_deref())
}

pub fn config_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("bot-config.json")
}

/// bypass_llm 开关读取 helper：bot-config.json 缺字段 / 文件不存在 / 解析失败都默认 true（bypass 行为）。
/// 比 bot_get_config 轻量：跳过 BotConfigView 构造 + key 校验，bot_chat 入口用。
pub fn read_bypass_llm_switch(app: &AppHandle) -> bool {
    let p = config_path(app);
    if !p.exists() {
        return true;
    }
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return true;
    };
    serde_json::from_str::<BotConfig>(&raw)
        .map(|c| c.bypass_llm_on_pre_step_hit)
        .unwrap_or(true)
}

// ───────────────────────── Linux secret-service 探测 + 降级 ─────────────────────────

/// 凭据后端。Linux 的 keyring 走 secret-service（zbus/dbus）—— headless
/// 服务器/容器/最小桌面无 dbus 会话时，keyring 调用直接 PlatformFailure，用户 key
/// 存不住且只会看到「系统凭据存储访问失败」。运行时探测，不可用降级明文文件
/// （chmod 0600，与 api-token.txt 同策略）+ WARN 审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyBackend {
    /// 系统凭据存储（macOS 钥匙串 / Windows 凭据管理器 / Linux secret-service）
    System,
    /// 降级：数据目录 bot-api-key.txt 明文文件（仅 Linux 无 secret-service 时启用）
    PlaintextFile,
}

/// 可测内核：后端选择纯函数
fn backend_for(secret_service_ok: bool) -> KeyBackend {
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
fn secret_service_available_with(has_dbus_addr: bool, xdg_runtime_dir: Option<&str>) -> bool {
    if has_dbus_addr {
        return true;
    }
    xdg_runtime_dir
        .map(|d| std::path::Path::new(d).join("bus").exists())
        .unwrap_or(false)
}

fn key_backend() -> KeyBackend {
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
fn plaintext_key_path_for(slot: KeySlot) -> std::path::PathBuf {
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
/// 与 api-token.txt 同策略）——「先写后 chmod」存在
/// umask 默认权限窗口，且 chmod 失败会被静默吞（key 以 0644 留存无告警）。
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
    use std::io::Write as _;
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

/// 按后端分发读取（可测：PlaintextFile + 注入路径即「mock secret-service 不可用」）；
/// 带 slot（System 后端按 slot 选 keyring 条目）
fn read_api_key_at(
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
fn has_api_key_at(
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
fn write_api_key_at(
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
fn delete_api_key_at(
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
            // 用户「清除」后 key 复活
            if let Ok(old) = key_entry_for_service(LEGACY_KEYRING_SERVICE, slot) {
                let _ = old.delete_credential();
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
fn classify_get_password(r: Result<String, keyring::Error>) -> CommandResult<String> {
    r.map_err(|e| CommandError::KeyringError(format!("读取 API Key 失败：{e}")))
}

/// 「key 不存在」（NoEntry）→ Ok(false)；
/// keyring 真实故障（钥匙串锁定 / 权限拒绝）→ Err(KeyringError)，不静默吞成 false。
fn classify_has_key(r: Result<String, keyring::Error>) -> CommandResult<bool> {
    match r {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(CommandError::KeyringError(format!(
            "检查 API Key 失败：{e}"
        ))),
    }
}

pub fn read_api_key() -> CommandResult<String> {
    read_key_of_slot(KeySlot::Llm)
}

pub fn has_api_key() -> CommandResult<bool> {
    has_key_of_slot(KeySlot::Llm)
}

/// 按 slot 读取：降级后端记 WARN；System 后端顺带做降级文件回迁 + v0 service 迁移
fn read_key_of_slot(slot: KeySlot) -> CommandResult<String> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    } else {
        prepare_system_backend(slot);
    }
    read_api_key_at(backend, &plaintext_key_path_for(slot), slot)
}

/// 按 slot 存在性检查
fn has_key_of_slot(slot: KeySlot) -> CommandResult<bool> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    } else {
        prepare_system_backend(slot);
    }
    has_api_key_at(backend, &plaintext_key_path_for(slot), slot)
}

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
static KEYRING_SERVICE_MIGRATED: [std::sync::atomic::AtomicBool; 3] = [
    std::sync::atomic::AtomicBool::new(false),
    std::sync::atomic::AtomicBool::new(false),
    std::sync::atomic::AtomicBool::new(false),
];

/// v0 → v1 service 名迁移（幂等）：
/// 新（带版本后缀）条目已有非空值 → no-op（不覆盖用户新 key）；
/// v0 有条目而新条目为空 → 复制过来，成功后删除 v0 条目。
/// keyring 故障静默返回且不置「已探测」标记（下次读取再试）——
/// 迁移绝不能挡住用户已有的 key（数据保留优先，与 migrate_plaintext_key_if_system 同策略）。
fn migrate_legacy_keyring_entry(slot: KeySlot) {
    use std::sync::atomic::Ordering;
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

fn write_api_key(key: &str) -> CommandResult<()> {
    write_key_of_slot(KeySlot::Llm, key)
}

/// 按 slot 写入
fn write_key_of_slot(slot: KeySlot, key: &str) -> CommandResult<()> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    }
    write_api_key_at(backend, &plaintext_key_path_for(slot), key, slot)
}

// ─────────────────── 搜索 key（Tavily/Brave，存系统凭据存储） ───────────────────

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

/// 返回给前端的配置视图：不含任何 key 本体，只有 has 标志
///（Tavily/Brave key 也进系统凭据存储，view 不透传 key 明文）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotConfigView {
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
    /// pre-step 命中 Skill 时是否跳过外层主 LLM。
    /// 前端设置页 Toggle 直接透传到 bot-config.json。
    pub bypass_llm_on_pre_step_hit: bool,
    /// 本地文件工具白名单目录（原样透传；空 = 后端用内置默认）
    pub allowed_dirs: Vec<String>,
    /// Tavily key 是否已存系统凭据存储（key 本体不进 view）
    pub has_tavily_key: bool,
    /// 「Tavily 搜索」开关原样透传（None = 未显式设置，前端按 key 有无显示自动态）
    pub tavily_enabled: Option<bool>,
    /// Brave key 是否已存系统凭据存储（key 本体不进 view）
    pub has_brave_key: bool,
    /// 「Brave 搜索」开关原样透传（None = 未显式设置，前端按 key 有无显示自动态）
    pub brave_enabled: Option<bool>,
    /// run_python 默认超时秒数（None = 60s 默认；设置页可改，硬钳 300s）
    pub python_timeout_secs: Option<u64>,
    /// 授权模式原样透传给设置页（None = ask 默认；非法值前端按 ask 显示）
    pub perm_mode: Option<String>,
    /// API 协议原样透传给设置页（None = openai 旧行为，非法值前端按 openai 显示）
    pub api_provider: Option<String>,
    /// max_tokens 原样透传（None = 8192 默认；仅 Anthropic 模式用，后端钳 256..=200000）
    pub max_tokens: Option<u32>,
    /// 每协议下的模型列表：None = 老配置未迁移（前端显示空列表让用户点「添加大模型」）；
    /// 已有数据则透传。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_by_provider: Option<ModelsByProvider>,
    /// 每协议 active 模型 id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model_id: Option<ActiveModelId>,
    /// 界面字体大小：small/standard/large/xlarge
    /// None = small；前端根据实际值走 documentElement[data-font-size] 套用
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_font_size: Option<String>,
    /// 定时记忆整理配置：None 时解析为默认（启用 + daily）透传前端
    pub memory_consolidation: crate::memory::consolidate::ConsolidationConfig,
}

// ───────────────────────── bot-config.json schema 迁移 ─────────────────────────

/// schemaVersion 在 JSON 里的键名（camelCase）。
const SCHEMA_VERSION_KEY: &str = "schemaVersion";

/// schema 迁移内核（纯 value，便于单测）：缺失/落后当前版本 → 就地补 `schemaVersion`，
/// 返回 (from, to)；已是当前或更高版本 → None（不降级「装过更新版后回退」的配置）。
/// v0 = 带版本号方案之前、没有该字段的老配置。
///
/// 逐版本迁移点：v0→v1 只补版本号字段本身；后续版本在此按 from 分支改字段
/// （直接改 `value` 上的键即可——未识别的字段原样保留，不会丢用户数据）。
fn migrate_config_value(value: &mut serde_json::Value) -> Option<(u32, u32)> {
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
fn migrate_config_file(path: &std::path::Path) -> Result<Option<(u32, u32)>, String> {
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
static SCHEMA_MIGRATION_CHECKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// bot-config.json schema 迁移钩子：读取时缺失就补默认 + 写回。
/// 调用点与 migrate_legacy_key 同：App 启动（lib.rs setup）+ 每次 load_config 兜底。
/// 幂等：已是当前版本不写盘；写回保留全部既有字段（明文 key 由后续迁移负责清）。
pub fn migrate_bot_config_schema(app: &AppHandle) -> Result<(), String> {
    if SCHEMA_MIGRATION_CHECKED.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    match migrate_config_file(&config_path(app)) {
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
    if !has_api_key().unwrap_or(false) && write_api_key(&k).is_err() {
        return Ok(()); // 写入失败：保留文件明文，下次再试
    }
    let dir = crate::db::data_dir(app);
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
        let has_in_store = has_search_key(slot).unwrap_or(false);
        let mut write = |key: &str| write_search_key(slot, key).map_err(|e| e.message());
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
    let dir = crate::db::data_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&p, raw).map_err(|e| e.to_string())
}

/// 单 slot 迁移内核（注入 has/write 便于单测，不碰真实 keyring）：
/// - 无明文（None/空串）→ (None, false)：字段归 None，幂等
/// - keyring 已有值 → (None, false)：不覆盖更新值，直接清明文
/// - 写入成功 → (None, true)：清明文 + 记迁移
/// - 写入失败 → (保留明文, false)：下次再试（数据保留优先）
fn migrate_search_key_slot(
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

/// 读 bot-config.json（不存在/解析失败回默认）。内部共用（bot_fs 白名单等）
pub(crate) fn load_config(app: &AppHandle) -> BotConfig {
    // 迁移钩子：老配置缺 schemaVersion → 补默认 + 写回（幂等，已迁移不写盘；
    // 失败不阻塞读取——本次仍按下面常规路径读，下次再试）
    let _ = migrate_bot_config_schema(app);
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
    let data_dir = crate::db::data_dir(app);
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app), raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn bot_get_config(app: AppHandle) -> CommandResult<BotConfigView> {
    let _ = migrate_legacy_key(&app); // 兜底：设置页读配置时也确保无明文残留
    let _ = migrate_search_keys(&app); // 兜底：Tavily/Brave 明文 key 同样迁进 keyring
    let cfg = load_config(&app);

    // keyring 真实故障（钥匙串锁定/权限拒绝）不吞成「未配置」，
    // 结构化 KeyringError 透传给前端，设置页可提示用户检查 keychain
    let has_api_key = has_api_key()?;
    // 搜索 key 同策略：真实故障透传 Err，不吞成 false
    let has_tavily_key = has_search_key(KeySlot::Tavily)?;
    let has_brave_key = has_search_key(KeySlot::Brave)?;
    // 老配置（无 models_by_provider、有 base_url/model）在内存里补一份
    // ModelEntry + active_model_id，让设置页能展示出来；不写盘，等用户主动保存
    // 才一并落盘。
    let mut cfg = cfg;
    migrate_legacy_models(&mut cfg);
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
        models_by_provider: cfg.models_by_provider,
        active_model_id: cfg.active_model_id,
        ui_font_size: cfg.ui_font_size,
        memory_consolidation: cfg.memory_consolidation.unwrap_or_default(),
    })
}

/// 保存配置。api_key / tavily_key / brave_key 三个顶层参数同语义：
/// Some(非空) 写入系统凭据存储并覆盖；None/空串不动已存的 key。
///（Tavily/Brave key 与主 key 同模式，不落配置文件）
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
            write_api_key(k)?;
        }
    }
    for (slot, key) in [(KeySlot::Tavily, tavily_key), (KeySlot::Brave, brave_key)] {
        if let Some(k) = key {
            let k = k.trim();
            if !k.is_empty() {
                write_search_key(slot, k)?;
            }
        }
    }
    // 文件里只留非敏感配置，三个 key 字段强制置 None（双保险：前端误把 key
    // 塞进 config 对象也不落明文）
    // 新结构（models_by_provider + active_model_id）落盘前回填
    // base_url/model/api_provider 三个派生字段——bot_model_loop 只看老字段。
    let mut config = config;
    derive_legacy_fields_from_active(&mut config);
    write_bot_config_file(&crate::db::data_dir(&app), config)
}

/// 读-改-写 bot-config.json（记忆整理的 last_run_at 回写等内部配置更新用）：
/// 与 bot_set_config 同落盘路径（key 字段剥离由 write_bot_config_file 保证）。
pub(crate) fn update_config_file(
    app: &AppHandle,
    f: impl FnOnce(&mut BotConfig),
) -> CommandResult<()> {
    let mut cfg = load_config(app);
    f(&mut cfg);
    write_bot_config_file(&crate::db::data_dir(app), cfg)
}

/// bot_set_config 落盘内核（抽出便于单测）：强制剥离三个 key 字段
/// （api_key/tavily_key/brave_key 一律 None）后写 bot-config.json。
/// base_url 非 https 且非回环 → 警告（api_key 明文传输风险）；
/// 只警告不拒写——本地推理服务是合法场景，且不能破坏存量用户配置。
fn write_bot_config_file(dir: &std::path::Path, config: BotConfig) -> CommandResult<()> {
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

/// 清除已保存的 API Key
#[tauri::command]
pub fn bot_clear_api_key() -> CommandResult<()> {
    delete_api_key_at(
        key_backend(),
        &plaintext_key_path_for(KeySlot::Llm),
        KeySlot::Llm,
    )
}

// ───────────────────────── 审计日志 ─────────────────────────

/// 追加机器人审计日志：用户指令、工具名、入参、结果全部留痕（数据目录 bot.log）
/// 审计日志外部钩子（bot_skills 调度器用；bot.rs 内部仍用 audit_log）
pub fn audit_log_hook<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    audit_log(app, line);
}

pub fn audit_log<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    // 与 audit::write_event 共用同一把写锁，防并发 append 交错错行
    let _g = crate::audit::BOT_LOG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let p = crate::db::data_dir(app).join("bot.log");
    crate::db::rotate_log_if_large(&p, 5 * 1024 * 1024);
    let _ = append_bot_log_line(&p, line);
}

/// bot.log 写一行内核：open/write 失败 eprintln 带路径并返回 false，
/// 不 `if let Ok … { let _ = writeln! }` 全静默（对齐 audit::append_line 的做法）。
/// 抽成路径参数版便于单测（与 bot_py.rs 同先例）。
fn append_bot_log_line(p: &std::path::Path, line: &str) -> bool {
    let mut f = match crate::audit::open_log_append(p) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[audit] write failed: {} path={}", e, p.display());
            return false;
        }
    };
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Err(e) = writeln!(f, "[{ts}] {line}") {
        eprintln!("[audit] write failed: {} path={}", e, p.display());
        return false;
    }
    true
}

/// 审计日志安全转义 + 截断：剥换行/管道符，防伪造「INFO |」前缀与多行撕裂。
/// 规则：`| ` → `|  `（双空格），剩余裸 `|` → `||`，`\n` → `\\n`，`\r` → `\\r`；
/// 转义后按字符数截到 max 加省略号。与 bot_py::escape_for_log 同一规则。
pub(crate) fn escape_for_log(s: &str, max: usize) -> String {
    let escaped = s
        .replace("| ", "|  ")
        .replace('|', "||")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let count = escaped.chars().count();
    if count <= max {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 向后兼容别名：bot_chat / bot_model_loop / bot_scheduler 仍用旧名，
/// 行为即 escape_for_log（同族日志伪造问题一并修复）；新代码请直接用 escape_for_log。
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    escape_for_log(s, max)
}

/// 读取机器人审计日志（倒序，最新在前；默认 200 行，上限 2000）
/// 读失败（权限/磁盘/损坏）返回 Err(IoError) + ERROR 审计，
/// 不静默吞成「暂无日志」；仅「文件不存在」返回占位文案。
#[tauri::command]
pub fn bot_log_read(app: AppHandle, limit: Option<usize>) -> CommandResult<String> {
    let p = crate::db::data_dir(&app).join("bot.log");
    match read_log_tail(&p, limit) {
        Ok(s) => Ok(s),
        Err(e) => {
            let msg = e.message();
            crate::audit::write_error_audit(&app, "bot_log_read_fail", &[("err", msg.as_str())]);
            Err(e)
        }
    }
}

/// 纯路径参数版便于单测（tauri command 绑定 Wry AppHandle）。
/// NotFound → Ok("（暂无日志）")（日志确实没东西）；其他 IO 错误 → Err(IoError)。
fn read_log_tail(path: &std::path::Path, limit: Option<usize>) -> CommandResult<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("（暂无日志）".into()),
        Err(e) => return Err(CommandError::IoError(format!("读取日志失败：{e}"))),
    };
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }
    lines.reverse();
    Ok(lines.join("\n"))
}

// ───────────────────────── 参数上限（防幻觉/防刷爆） ─────────────────────────

pub const MAX_TITLE: usize = 200;
pub const MAX_NOTE: usize = 5000;
pub const MAX_KEYWORD: usize = 100;
pub const MAX_SUBTASK_TEXT: usize = 200;
pub const MAX_DUE: usize = 30;
pub const MAX_TAGS: usize = 10;
pub const MAX_TAG_LEN: usize = 30;

/// 校验字符串长度上限，超限返回错误文案
pub fn check_len(value: &str, max: usize, what: &str) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{what}过长（上限 {max} 字）"));
    }
    Ok(())
}
#[cfg(test)]
mod bot_config_tests {
    use super::*;

    #[test]
    fn default_has_bypass_llm_on_pre_step_hit_true() {
        let cfg = BotConfig::default();
        assert!(
            cfg.bypass_llm_on_pre_step_hit,
            "Default impl 应默认开启新行为（bypass=true）"
        );
    }

    #[test]
    fn old_config_without_bypass_field_deserializes_to_true() {
        // 模拟老用户 bot-config.json 没有 bypass_llm_on_pre_step_hit 字段
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig =
            serde_json::from_str(raw).expect("老配置应通过 struct 级 #[serde(default)] 兼容");
        assert!(
            cfg.bypass_llm_on_pre_step_hit,
            "老配置缺字段应默认 true（开启新行为，保证 release 不破现有用户）"
        );
    }

    #[test]
    fn perm_mode_defaults_to_ask() {
        // 老配置缺 permMode 字段 → None → Ask（默认：弹授权而非硬拒）
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(PermMode::from_cfg(cfg.perm_mode.as_deref()), PermMode::Ask);
        assert_eq!(PermMode::from_cfg(None), PermMode::Ask);
    }

    #[test]
    fn perm_mode_parses_known_values_and_falls_back() {
        assert_eq!(PermMode::from_cfg(Some("strict")), PermMode::Strict);
        assert_eq!(PermMode::from_cfg(Some("ask")), PermMode::Ask);
        assert_eq!(PermMode::from_cfg(Some("yolo")), PermMode::Yolo);
        // 非法值/空白回退 Ask（安全默认偏严一侧的可用形态）
        assert_eq!(
            PermMode::from_cfg(Some("YOLO ")),
            PermMode::Ask,
            "大小写不识别，回退 Ask"
        );
        assert_eq!(PermMode::from_cfg(Some("garbage")), PermMode::Ask);
        assert_eq!(PermMode::from_cfg(Some("")), PermMode::Ask);
        // as_str 往返
        assert_eq!(PermMode::Strict.as_str(), "strict");
        assert_eq!(PermMode::Ask.as_str(), "ask");
        assert_eq!(PermMode::Yolo.as_str(), "yolo");
    }

    #[test]
    fn api_provider_defaults_to_openai_and_falls_back() {
        // 老配置缺 apiProvider 字段 → None → Openai（Anthropic 兼容模式对
        // 老配置零影响）；非法值/空白防御回退 Openai（与 PermMode::from_cfg 同风格）
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(
            ApiProvider::from_cfg(cfg.api_provider.as_deref()),
            ApiProvider::Openai
        );
        assert_eq!(ApiProvider::from_cfg(None), ApiProvider::Openai);
        assert_eq!(ApiProvider::from_cfg(Some("openai")), ApiProvider::Openai);
        assert_eq!(
            ApiProvider::from_cfg(Some("anthropic")),
            ApiProvider::Anthropic
        );
        assert_eq!(
            ApiProvider::from_cfg(Some("Anthropic")),
            ApiProvider::Openai,
            "大小写不识别，回退 Openai"
        );
        assert_eq!(ApiProvider::from_cfg(Some("garbage")), ApiProvider::Openai);
        assert_eq!(ApiProvider::Openai.as_str(), "openai");
        assert_eq!(ApiProvider::Anthropic.as_str(), "anthropic");
    }

    #[test]
    fn max_tokens_default_and_clamped() {
        assert_eq!(
            resolve_max_tokens(None),
            DEFAULT_MAX_TOKENS,
            "None = 默认 8192"
        );
        assert_eq!(resolve_max_tokens(Some(4096)), 4096);
        assert_eq!(
            resolve_max_tokens(Some(1)),
            MIN_MAX_TOKENS,
            "低于下限钳 256"
        );
        assert_eq!(
            resolve_max_tokens(Some(999_999)),
            MAX_MAX_TOKENS,
            "高于上限钳 200000"
        );
    }

    // ── bot-config.json schema 迁移 ──

    #[test]
    fn config_schema_migration_fills_missing_version_and_preserves_fields() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot-config.json");
        std::fs::write(
            &p,
            r#"{"baseUrl":"https://api.example.com/v1","model":"m","apiKey":"sk-plain","allowedDirs":["/a"]}"#,
        )
        .unwrap();
        let got = migrate_config_file(&p).unwrap();
        assert_eq!(got, Some((0, BOT_CONFIG_SCHEMA_VERSION)), "缺字段必须补写");
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(
            v[SCHEMA_VERSION_KEY],
            serde_json::json!(BOT_CONFIG_SCHEMA_VERSION)
        );
        // 明文 key / 其他字段原样保留：迁移只管版本号，清明文是 migrate_legacy_key 的职责
        assert_eq!(v["apiKey"], serde_json::json!("sk-plain"));
        assert_eq!(v["allowedDirs"][0], serde_json::json!("/a"));
    }

    #[test]
    fn config_schema_migration_is_idempotent_and_skips_future_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot-config.json");
        // 已是当前版本 → 不再写回（load_config 热路径不产生无谓写盘）
        std::fs::write(
            &p,
            format!(r#"{{"{SCHEMA_VERSION_KEY}":{BOT_CONFIG_SCHEMA_VERSION}}}"#),
        )
        .unwrap();
        assert_eq!(migrate_config_file(&p).unwrap(), None);
        // 未来版本（用户装过更新版后回退）→ 不降级、不动文件
        std::fs::write(&p, format!(r#"{{"{SCHEMA_VERSION_KEY}":999}}"#)).unwrap();
        assert_eq!(migrate_config_file(&p).unwrap(), None);
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v[SCHEMA_VERSION_KEY], serde_json::json!(999));
    }

    #[test]
    fn config_schema_migration_tolerates_missing_or_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        assert_eq!(migrate_config_file(&missing).unwrap(), None);
        // JSON 损坏 / 非对象：不在这里纠错（读取侧回默认），更不能 panic
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{not json").unwrap();
        assert_eq!(migrate_config_file(&broken).unwrap(), None);
        let arr = dir.path().join("array.json");
        std::fs::write(&arr, "[1,2,3]").unwrap();
        assert_eq!(migrate_config_file(&arr).unwrap(), None);
    }

    #[test]
    fn bot_config_default_and_legacy_deserialize_carry_current_version() {
        assert_eq!(
            BotConfig::default().schema_version,
            BOT_CONFIG_SCHEMA_VERSION
        );
        // 老配置（无 schemaVersion 字段）：字段级 default 兜底为当前版本
        let cfg: BotConfig =
            serde_json::from_str(r#"{"baseUrl":"https://api.example.com/v1"}"#).unwrap();
        assert_eq!(cfg.schema_version, BOT_CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn keyring_service_names_are_versioned() {
        // 协议锁：service 名变更必须同步「新 + 旧」两个常量，否则存量用户 key 读不到
        assert_eq!(KEYRING_SERVICE, "wmessage.bot.v1");
        assert_eq!(LEGACY_KEYRING_SERVICE, "wmessage-bot");
    }
}

/// keyring 错误分类纯函数单测。
/// 真实 keychain 在测试环境不可用，用构造的 keyring::Error 注入故障。
#[cfg(test)]
mod f1_keyring_tests {
    use super::*;

    fn platform_failure() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "keychain locked",
        )))
    }

    #[test]
    fn read_api_key_keyring_failure_maps_to_keyring_error_variant() {
        // keyring 故障（钥匙串锁定/权限拒绝）→ KeyringError 结构化变体，不走 String 逃生舱
        let err = classify_get_password(Err(platform_failure())).unwrap_err();
        assert_eq!(err.code(), "KEYRING_ERROR");
        assert!(err.message().contains("读取 API Key 失败"));
        // error.rs 定义：KeyringError recoverable=false（用户需先解锁 keychain，重试无意义）
        assert!(!err.is_recoverable());
    }

    #[test]
    fn read_api_key_success_passes_value_through() {
        let key = classify_get_password(Ok("sk-test".into())).unwrap();
        assert_eq!(key, "sk-test");
    }

    #[test]
    fn has_api_key_no_entry_is_ok_false() {
        // 「key 不存在」不是故障：Ok(false)，前端显示「未配置 API Key」
        let r = classify_has_key(Err(keyring::Error::NoEntry)).unwrap();
        assert!(!r);
    }

    #[test]
    fn has_api_key_keyring_failure_not_swallowed_to_false() {
        // keyring 真实故障不得吞成 false（「反复填 key 仍失败无提示」假象）
        let err = classify_has_key(Err(platform_failure())).unwrap_err();
        assert_eq!(err.code(), "KEYRING_ERROR");
        assert!(err.message().contains("检查 API Key 失败"));
    }

    #[test]
    fn has_api_key_present_is_ok_true() {
        assert!(classify_has_key(Ok("sk-test".into())).unwrap());
    }
}

/// Linux secret-service 探测 + 降级明文文件后端单测。
/// 注入 backend=PlaintextFile + 临时路径即「mock secret-service 不可用」，
/// fallback 路径全链路走通（写 → 读 → has → 删）+ WARN 审计落行。
#[cfg(test)]
mod p2_32_keyring_fallback_tests {
    use super::*;

    #[test]
    fn backend_for_selects_plaintext_when_secret_service_down() {
        assert_eq!(backend_for(true), KeyBackend::System);
        assert_eq!(backend_for(false), KeyBackend::PlaintextFile);
    }

    #[test]
    fn secret_service_probe_dbus_addr_or_bus_socket() {
        assert!(
            secret_service_available_with(true, None),
            "有 dbus 地址即可用"
        );
        assert!(
            !secret_service_available_with(false, None),
            "无任何线索 = 不可用"
        );
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().to_str().unwrap();
        assert!(
            !secret_service_available_with(false, Some(d)),
            "XDG_RUNTIME_DIR 无 bus socket = 不可用"
        );
        std::fs::write(tmp.path().join("bus"), b"").unwrap();
        assert!(
            secret_service_available_with(false, Some(d)),
            "XDG_RUNTIME_DIR/bus 存在即可用"
        );
    }

    #[test]
    fn plaintext_backend_roundtrip_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("bot-api-key.txt");
        // has：文件缺失 → Ok(false)（对齐 NoEntry 语义，不吞错）
        assert!(!has_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap());
        // read：缺失 → KeyringError（与 System 路径 NoEntry 同 code，前端 hint 一致）
        let e = read_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap_err();
        assert_eq!(e.code(), "KEYRING_ERROR");
        // write → 文件落盘 + Unix 0600（与 api-token.txt 同策略）
        write_api_key_at(KeyBackend::PlaintextFile, &f, "sk-test-123", KeySlot::Llm).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                0o600,
                "降级 key 文件必须 0600"
            );
        }
        assert_eq!(
            read_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap(),
            "sk-test-123"
        );
        assert!(has_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap());
        // delete：删后不存在；再删幂等 Ok
        delete_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap();
        assert!(!f.exists());
        delete_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap();
    }

    #[test]
    fn plaintext_write_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("nested").join("bot-api-key.txt");
        write_api_key_at(KeyBackend::PlaintextFile, &f, "sk-x", KeySlot::Llm).unwrap();
        assert_eq!(
            read_api_key_at(KeyBackend::PlaintextFile, &f, KeySlot::Llm).unwrap(),
            "sk-x"
        );
    }

    #[test]
    fn fallback_warn_audit_lands_in_bot_log() {
        // 降级告警：WARN 级别 + 事件名 + reason 落 bot.log（write_warn_audit_to）
        let tmp = tempfile::tempdir().unwrap();
        crate::audit::write_warn_audit_to(
            tmp.path(),
            "keyring_fallback_plaintext",
            &[("reason", "secret-service 不可用")],
        );
        let log = std::fs::read_to_string(tmp.path().join("bot.log")).unwrap();
        assert!(
            log.contains("WARN | keyring_fallback_plaintext"),
            "缺 WARN 审计行: {log:?}"
        );
        assert!(log.contains("reason=secret-service 不可用"), "got: {log:?}");
    }
}

/// 搜索 key 进 keyring：KeySlot 槽位 + 迁移内核单测。
/// 真实 keychain 测试环境不可用，走 PlaintextFile 后端 + 注入路径（与上面 fallback 测试同模式）。
#[cfg(test)]
mod search_key_slot_tests {
    use super::*;

    #[test]
    fn key_slot_keyring_user_and_filename_distinct() {
        // 三个用途的 keyring 条目名 / 降级文件名必须互不相同，且 LLM 保持既有值不变
        assert_eq!(KeySlot::Llm.keyring_user(), "api-key");
        assert_eq!(KeySlot::Llm.plaintext_filename(), "bot-api-key.txt");
        assert_eq!(KeySlot::Tavily.keyring_user(), "tavily_api_key");
        assert_eq!(KeySlot::Tavily.plaintext_filename(), "bot-tavily-key.txt");
        assert_eq!(KeySlot::Brave.keyring_user(), "brave_api_key");
        assert_eq!(KeySlot::Brave.plaintext_filename(), "bot-brave-key.txt");
    }

    #[test]
    fn search_slot_plaintext_backend_roundtrip() {
        // Tavily/Brave 槽位走同一套后端分发（PlaintextFile 注入路径可测）
        let tmp = tempfile::tempdir().unwrap();
        for slot in [KeySlot::Tavily, KeySlot::Brave] {
            let f = tmp.path().join(slot.plaintext_filename());
            assert!(!has_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap());
            write_api_key_at(KeyBackend::PlaintextFile, &f, "tvly-x", slot).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
                    0o600,
                    "搜索 key 降级文件同样必须 0600"
                );
            }
            assert_eq!(
                read_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap(),
                "tvly-x"
            );
            assert!(has_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap());
            // 覆盖写
            write_api_key_at(KeyBackend::PlaintextFile, &f, "tvly-y", slot).unwrap();
            assert_eq!(
                read_api_key_at(KeyBackend::PlaintextFile, &f, slot).unwrap(),
                "tvly-y"
            );
        }
    }

    #[test]
    fn migrate_slot_migrates_plaintext_to_store() {
        let mut written: Vec<String> = Vec::new();
        let (remaining, migrated) = migrate_search_key_slot(Some("tvly-plain"), false, &mut |k| {
            written.push(k.to_string());
            Ok(())
        });
        assert_eq!(written, vec!["tvly-plain"], "应写入 keyring");
        assert_eq!(remaining, None, "写成功 → 配置字段清掉明文");
        assert!(migrated, "应记迁移");
    }

    #[test]
    fn migrate_slot_does_not_overwrite_existing_store_value() {
        // keyring 已有值（用户可能在别处更新过）→ 不覆盖，直接清明文
        let mut written: Vec<String> = Vec::new();
        let (remaining, migrated) =
            migrate_search_key_slot(Some("tvly-old-plain"), true, &mut |k| {
                written.push(k.to_string());
                Ok(())
            });
        assert!(written.is_empty(), "keyring 已有值时不得覆盖写入");
        assert_eq!(remaining, None, "明文仍应清掉（目标 = 文件不留明文）");
        assert!(!migrated, "未发生写入不算迁移（不记迁移审计）");
    }

    #[test]
    fn migrate_slot_write_failure_keeps_plaintext() {
        // keyring 写失败 → 保留文件明文，下次再试（数据保留优先）
        let (remaining, migrated) = migrate_search_key_slot(Some("tvly-plain"), false, &mut |_| {
            Err("keychain locked".to_string())
        });
        assert_eq!(remaining.as_deref(), Some("tvly-plain"), "写失败保留明文");
        assert!(!migrated);
    }

    #[test]
    fn migrate_slot_no_plaintext_is_idempotent() {
        let (r1, m1) = migrate_search_key_slot(None, false, &mut |_| Ok(()));
        assert_eq!((r1, m1), (None, false));
        // 空串/空白视同无明文
        let (r2, m2) = migrate_search_key_slot(Some("  "), false, &mut |_| Ok(()));
        assert_eq!((r2, m2), (None, false));
    }

    #[test]
    fn write_bot_config_file_strips_all_key_fields() {
        // bot_set_config 双保险回归：前端误把 key 塞进 config 对象也不落明文
        let tmp = tempfile::tempdir().unwrap();
        let cfg = BotConfig {
            api_key: Some("sk-llm-plain".into()),
            tavily_key: Some("tvly-plain".into()),
            brave_key: Some("bsa-plain".into()),
            ..Default::default()
        };
        write_bot_config_file(tmp.path(), cfg).unwrap();
        let raw = std::fs::read_to_string(tmp.path().join("bot-config.json")).unwrap();
        assert!(!raw.contains("sk-llm-plain"), "LLM key 不得落盘：{raw}");
        assert!(!raw.contains("tvly-plain"), "Tavily key 不得落盘：{raw}");
        assert!(!raw.contains("bsa-plain"), "Brave key 不得落盘：{raw}");
        // 字段本身也应消失（skip_serializing_if + 强制 None）
        let saved: BotConfig = serde_json::from_str(&raw).unwrap();
        assert!(saved.api_key.is_none() && saved.tavily_key.is_none() && saved.brave_key.is_none());
    }
}

/// bot_log_read 单测：读文件错不吞成「暂无日志」。
#[cfg(test)]
mod f3_log_read_tests {
    use super::*;

    #[test]
    fn read_log_tail_missing_file_returns_placeholder() {
        // 文件不存在 = 日志确实没东西 → Ok 占位文案（前端按「暂无日志」显示）
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        assert_eq!(read_log_tail(&p, None).unwrap(), "（暂无日志）");
    }

    #[test]
    fn read_log_tail_io_error_not_swallowed() {
        // 路径是目录 → read_to_string 失败（非 NotFound）→ Err(IoError)，不静默吞
        let tmp = tempfile::tempdir().unwrap();
        let err = read_log_tail(tmp.path(), None).unwrap_err();
        assert_eq!(err.code(), "IO_ERROR");
        assert!(!err.is_recoverable());
    }

    #[test]
    fn read_log_tail_returns_reversed_tail_with_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        std::fs::write(&p, "l1\nl2\nl3\n").unwrap();
        assert_eq!(read_log_tail(&p, Some(2)).unwrap(), "l3\nl2");
        assert_eq!(read_log_tail(&p, None).unwrap(), "l3\nl2\nl1");
    }

    #[test]
    fn bot_log_read_fail_audit_line_written() {
        // ERROR 审计走 write_error_audit（泛型 Runtime，mock_app 跑同一条生产代码路径）。
        // generic_log_dir 探针命中测试二进制旁目录（target/debug/deps），读完即删。
        let app = tauri::test::mock_app();
        crate::audit::write_error_audit(app.handle(), "bot_log_read_fail", &[("err", "boom")]);
        let exe = std::env::current_exe().unwrap();
        let log = exe.parent().unwrap().join("bot.log");
        let content = std::fs::read_to_string(&log).unwrap();
        assert!(
            content.contains("bot_log_read_fail"),
            "ERROR 审计行应出现: {content}"
        );
        assert!(content.contains("boom"), "审计行应含 err 信息: {content}");
        let _ = std::fs::remove_file(&log);
    }
}
/// 不依赖 Tauri AppHandle，验证分页格式化（位置行 + 截断续读提示 + offset 切片）。
#[cfg(test)]
mod t1_4_audit_log_tests {
    /// audit_log 写盘内核——成功带 [ts] 前缀落行；
    /// 失败 eprintln 带 path + 返回 false（对齐 audit::append_line 的做法），不静默不 panic
    #[test]
    fn append_bot_log_line_ok_writes_with_ts() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bot.log");
        assert!(super::append_bot_log_line(&p, "hello"));
        let content = std::fs::read_to_string(&p).unwrap();
        assert!(
            content.ends_with("] hello\n"),
            "应带 [ts] 前缀落行：{content}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn append_bot_log_line_readonly_dir_fails_visible() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let p = ro.join("bot.log");
        // 写失败：返回 false + eprintln 带 path（不能 if let Ok 全静默，丢日志零痕迹）
        assert!(!super::append_bot_log_line(&p, "x"));
        assert!(!p.exists());
        // 恢复权限让 tempdir 清理不掉链子
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[cfg(test)]
mod t1_6_base_url_tests {
    /// base_url 安全判定——https / 空 / 回环放行；
    /// http 公网/内网地址判不安全（调用方打警告，不拒写）
    #[test]
    fn base_url_safety_classification() {
        // 安全：https
        assert!(super::base_url_is_safe("https://api.deepseek.com/v1"));
        // 安全：空（未配置，无可 warn）
        assert!(super::base_url_is_safe(""));
        assert!(super::base_url_is_safe("   "));
        // 安全：回环（本地推理服务合法场景）
        assert!(super::base_url_is_safe("http://localhost:11434/v1"));
        assert!(super::base_url_is_safe("http://127.0.0.1:8000/v1"));
        assert!(super::base_url_is_safe("http://[::1]:8080"));
        // 不安全：http 公网 / 内网
        assert!(!super::base_url_is_safe("http://api.example.com/v1"));
        assert!(!super::base_url_is_safe("http://192.168.1.10:8000/v1"));
        assert!(!super::base_url_is_safe("ftp://example.com"));
    }
}
