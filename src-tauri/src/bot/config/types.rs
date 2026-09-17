//! Bot 配置核心类型 + 常量 + check_len。
//!
//! - KeySlot / BotConfig / ModelEntry / ModelsByProvider / ActiveModelId
//! - ApiProvider / PermMode 协议字符串归一化
//! - BotConfigView（不含 key 本体的对外视图）
//! - 7 个字段上限常量（MAX_TITLE/NOTE/...）
//! - keyring service 名 + schema 版本号
//!
//! 无 IO / 无锁 / 无依赖子模块——bot/config/ 的基础层。

use serde::{Deserialize, Serialize};

// ───────────────────────── KeySlot ─────────────────────────

/// 凭据存储条目：macOS 钥匙串 / Windows 凭据管理器。
/// service 名带版本后缀——key 存储格式升级时新开 `wmessage.bot.vN`，
/// 首次读取自动从上一版 service 迁移（见 migrate_legacy_keyring_entry），
/// 用户不必重新输入 key。
pub const KEYRING_SERVICE: &str = "wmessage.bot.v1";
/// 无版本后缀的历史 service 名（v0，带后缀方案之前的版本写入）。
/// 仅用于迁移：新条目为空时从它搬运，成功后删除旧条目。
pub const LEGACY_KEYRING_SERVICE: &str = "wmessage_bot";
pub const KEYRING_USER: &str = "api-key";

/// bot-config.json 当前 schema 版本。结构变更 +1 并在此登记迁移步骤
/// （风格对齐 migration.rs 的 `RulesFile::version`：文件自带版本字段 + serde 默认兜底）。
pub const BOT_CONFIG_SCHEMA_VERSION: u32 = 1;

pub(crate) fn default_schema_version() -> u32 {
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
    pub(crate) fn index(&self) -> usize {
        match self {
            KeySlot::Llm => 0,
            KeySlot::Tavily => 1,
            KeySlot::Brave => 2,
        }
    }
}

// ───────────────────────── BotConfig ─────────────────────────

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

// ───────────────────────── ApiProvider / PermMode ─────────────────────────

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

// ───────────────────────── BotConfigView ─────────────────────────

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

// ───────────────────────── 字段上限 + check_len ─────────────────────────

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

// keyring entry 构造函数见 keyring.rs（与 secret-service 探测一起管理）
