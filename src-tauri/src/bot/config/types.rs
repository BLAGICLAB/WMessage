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
    /// 仅用于旧版本迁移：老 bot-config.json 里的明文 key，读出迁入凭据存储后置 None 写回；
    /// 永不序列化——明文 key 不落盘，任何未来忘剥 key 的写路径 fail-closed
    #[serde(skip_serializing)]
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
    #[serde(skip_serializing)]
    pub tavily_key: Option<String>,
    /// 「Tavily 搜索」开关（可选）：None = 未显式设置，按旧行为自动
    /// （配了 tavilyKey 就当开启）；Some(true) = 强制走 Tavily；Some(false) = 强制
    /// Bing+百度双引擎（即使配了 key）。分流逻辑见 bot_web::resolve_search_route。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tavily_enabled: Option<bool>,
    /// 仅用于旧版本迁移（Brave key 存系统凭据存储，不再明文落盘）：
    /// 老 bot-config.json 里的明文 Brave key，由 migrate_search_keys 读出迁入
    /// keyring 后置 None 写回。新代码读写 Brave key 一律走 read/write_search_key。
    #[serde(skip_serializing)]
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
    /// 推理强度后台默认（可选）："off" / "low" / "medium" / "high"。
    /// None = "medium"（老板拍板默认中）。这是**抽象档位**，发送前按具体模型族
    /// 映射到各 provider 的线上参数（glm 的 reasoning_effort/thinking、
    /// OpenAI 的 reasoning_effort、Anthropic 的 thinking.budget_tokens，
    /// 映射表见 bot/reasoning.rs）。挂件聊天可按会话覆盖此值（不回写）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// 单次请求 Function 调用熔断上限（可选，全域）：None = 默认 100；
    /// 子 agent 会话不受此字段影响（走各自预算 max_tool_calls）。
    /// 工作流节点等长链任务可调大；软警阈值 = cap*7/10 自动跟随。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_function_calls: Option<u32>,
    /// 任务卡归档天数（可选）：任务完成满 N 天自动归档。前端看板规则
    /// （applyArchiveRule）与 migration 兜底归档同源读取，两侧阈值恒一致。
    /// None = 默认 7 天；读取侧 resolve_archive_after_days 钳制 1..=365。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub archive_after_days: Option<u32>,
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
    /// 记忆可控开关（U15）：注入总闸 + 模型主动记忆门禁。
    /// None（老配置缺字段）= 全开，行为与开关引入前完全一致。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_control: Option<crate::memory::MemoryControl>,
    /// 记忆参数（U17）：注入预算/条数/容量/衰减/去重阈值。
    /// None（老配置缺字段）= 全默认；读取侧 read_memory_tuning 统一钳制。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub memory_tuning: Option<crate::memory::MemoryTuning>,
    /// 外部 MCP 服务器配置（MCP 宿主支持，2026-09-28 拍板 1B）：
    /// None = 老配置无此字段 = 未配置任何服务器。结构见 bot::mcp::config。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mcp_servers: Option<Vec<crate::bot::mcp::config::McpServerConfig>>,
    /// 自进化块原样透传（P0-EV1）：evolution/activation 配置由 evolution/ 模块
    /// 自己的读取器解析（shadow.enabled / activation.mode / activation_state），
    /// BotConfig 不解构只保真——此前未知字段被 serde 静默丢弃，设置页任意一次
    /// 写盘（bot_set_config / update_config_file / persist_last_run）都会把
    /// evolution 块写丢。bot_set_config 是前端整体替换写（视图不含此块），
    /// 落盘前从盘上现值回填（见 config::commands）。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub evolution: Option<serde_json::Value>,
    /// 厂商级禁用列表（厂商详情页总开关）：名单内厂商的模型行在设置页变淡、
    /// 启用开关禁用，聊天选模型入口过滤。空 = 全部启用；
    /// 老配置缺字段 → 空 Vec（struct 级 #[serde(default)] + 字段级 default）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_vendors: Vec<String>,
    /// 连接测试通过的厂商名单（可用性依据：设置页左栏绿点 + 聊天模型下拉过滤）。
    /// 厂商 key 被覆盖/清除、条目 Base URL 或 API 格式变化都会使其失效
    ///（bot_set_config 落盘前对比盘上值剔除，见 prune_verified_vendors）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verified_vendors: Vec<String>,
    /// 配置 schema 版本（迁移钩子）：缺失就补默认 + 写回（migrate_bot_config_schema）。
    /// 字段级 default 让老配置（无此字段）反序列化即拿到当前版本，「文件里没有这个
    /// key」的判定走 raw JSON（见 migrate_config_value），不靠反序列化结果。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
}

/// 单个模型条目：一个 (label, baseUrl, model) 三元组 + 稳定 id。
/// id 是前端 crypto.randomUUID() 生成的字符串，仅用于 React key + 标识 active；
/// 不参与 API 调用。
/// vendor（U10 厂商中心）：条目所属厂商名，前端按它分组渲染厂商页；
/// 老配置缺字段 → None（前端按协议名兜底分组），None 序列化时跳过保持文件干净。
/// enabled（U11 模型列表开关）：false = 聊天 🧠 下拉不显示该模型；缺字段默认启用。
/// context_k（U11 徽标）：上下文窗口（千 token），前端显示「204.8K」样式；None 不显示。
/// capabilities（能力徽标）：如 ["视觉"]；老配置缺字段 → None，None 序列化时跳过。
/// temperature/top_p/max_tokens/system_prompt（每模型推理参数覆盖）：None = 用全局
/// 默认（BotConfig.max_tokens / 后端常量），不写入请求；老配置缺字段 → None，
/// None 序列化时跳过保持文件干净。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_k: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

fn default_true() -> bool {
    true
}

/// 双协议下各自的模型列表；Vec 为空序列化时跳过，保持配置文件干净。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
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
            reasoning_effort: None,           // 未配置 = medium 默认（RE-1，resolve 兜底）
            perm_mode: None,                  // 未配置 = ask（弹授权）
            api_provider: None,               // 未配置 = openai（旧行为）
            max_tokens: None,                 // 未配置 = 8192 默认（仅 Anthropic 模式用）
            models_by_provider: None,         // 未配置 = 设置页空列表（无默认厂商）
            active_model_id: None,            // 未配置 = 两协议都没选 active
            ui_font_size: None,               // 未配置 = small（老板拍板默认；前端读取时回退）
            memory_consolidation: None, // 未配置 = 启用 + daily（ConsolidationConfig::default）
            memory_control: None,       // 未配置 = 注入/主动记忆全开（U15 前行为）
            memory_tuning: None,        // 未配置 = 参数全默认（U17 前行为）
            mcp_servers: None,          // 未配置 = 无外部 MCP 服务器（老配置零影响）
            max_function_calls: None,   // 未配置 = 100 默认（W5-FUSE，全域熔断上限）
            archive_after_days: None,   // 未配置 = 7 天默认（前端设置页数据管理可改）
            evolution: None,            // 未配置 = 无自进化块（evolution 模块自管读写）
            disabled_vendors: Vec::new(), // 未配置 = 无厂商被禁用
            verified_vendors: Vec::new(), // 未配置 = 无厂商通过连接测试
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

// ───────────────────────── 归档天数解析 ─────────────────────────

/// 任务卡归档天数默认/上限。设置页「数据管理」卡可改（1..=365 天）。
pub const DEFAULT_ARCHIVE_DAYS: u32 = 7;
pub const MAX_ARCHIVE_DAYS: u32 = 365;

/// 归档天数解析：None → 7 默认；有值钳制 1..=365（防 0 把规则打死或超大值
/// 形同关闭）。前端设置页同规则钳制（src/lib/archiveRule.ts clampArchiveDays）。
pub fn resolve_archive_after_days(v: Option<u32>) -> u32 {
    v.unwrap_or(DEFAULT_ARCHIVE_DAYS).clamp(1, MAX_ARCHIVE_DAYS)
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
    /// 单次请求 Function 调用熔断上限（W5-FUSE 全域）：None = 默认 100；
    /// 子 agent 会话不受影响（走各自预算）
    pub max_function_calls: Option<u32>,
    /// 任务卡归档天数原样透传（None = 7 默认；设置页「数据管理」卡编辑）
    pub archive_after_days: Option<u32>,
    /// 授权模式原样透传给设置页（None = ask 默认；非法值前端按 ask 显示）
    pub perm_mode: Option<String>,
    /// API 协议原样透传给设置页（None = openai 旧行为，非法值前端按 openai 显示）
    pub api_provider: Option<String>,
    /// max_tokens 原样透传（None = 8192 默认；仅 Anthropic 模式用，后端钳 256..=200000）
    pub max_tokens: Option<u32>,
    /// 推理强度后台默认原样透传（None = "medium"；抽象档位 off/low/medium/high，
    /// 线上参数映射在后端 bot/reasoning.rs，前端只存档位）
    pub reasoning_effort: Option<String>,
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
    /// 记忆可控开关原样透传（None = 全开，前端按开启显示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_control: Option<crate::memory::MemoryControl>,
    /// 记忆参数原样透传（None = 全默认；前端原样回传保存，手改配置不被冲掉）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_tuning: Option<crate::memory::MemoryTuning>,
    /// 外部 MCP 服务器配置原样透传（None 归一为空数组，前端永远拿数组形态）
    pub mcp_servers: Vec<crate::bot::mcp::config::McpServerConfig>,
    /// 厂商级禁用列表原样透传（空 = 全部启用）
    pub disabled_vendors: Vec<String>,
    /// 已存 API Key 的厂商名列表（keyring 无法枚举条目，按配置里出现过的
    /// 厂商名逐个探测 has_vendor_key）：前端厂商页按它显示「已存入 ✓」
    pub vendor_keys: Vec<String>,
    /// 连接测试通过的厂商名单原样透传（空 = 都未验证；聊天下拉过滤用）
    pub verified_vendors: Vec<String>,
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

// ────────────── 每卡模型覆盖解析（W6-MODEL） ──────────────

/// 解析结果：执行链用这四元组整组替换全局 http 配置
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    pub base_url: String,
    pub model: String,
    pub api_provider: String,
    pub api_key: String,
    /// 条目级推理参数（温度/top_p/system_prompt/max_tokens 钳制按覆盖条目，
    /// 防跨协议钳制错位——W6 r1 medium）
    pub inference: crate::bot::EffectiveInference,
}

/// 按模型库条目 id 解析覆盖配置（纯查表 + key 读取，单测锚点）：
/// 先 openai 后 anthropic 列表；条目不存在/禁用 → 响亮报错；
/// key = 条目厂商 key（有则用）→ 全局 key 兜底（read_llm_key 同款回退）。
pub fn resolve_model_override(
    api_provider: Option<&str>,
    active_model_id: Option<&ActiveModelId>,
    models_by_provider: Option<&ModelsByProvider>,
    entry_id: &str,
    global_max_tokens: Option<u32>,
) -> Result<ResolvedModel, String> {
    let lists = models_by_provider;
    fn find<'a>(items: &'a [ModelEntry], entry_id: &str) -> Option<&'a ModelEntry> {
        items.iter().find(|e| e.id == entry_id)
    }
    let found = lists.and_then(|m| {
        find(&m.openai, entry_id)
            .map(|e| (e, "openai"))
            .or_else(|| find(&m.anthropic, entry_id).map(|e| (e, "anthropic")))
    });
    let (entry, provider) = match found {
        Some((e, p)) => (e, p),
        None => {
            return Err(format!(
                "模型条目 {entry_id} 不存在：请到 设置→模型设置 确认（条目可能已删除）"
            ));
        }
    };
    if !entry.enabled {
        return Err(format!(
            "模型条目「{}」已停用：请到 设置→模型设置 启用后重试",
            entry.label
        ));
    }
    // key：厂商 key 优先，缺 → 全局 key 兜底（与 read_llm_key 回退语义一致）
    let api_key = match &entry.vendor {
        Some(v) if crate::bot::has_vendor_key(v).unwrap_or(false) => {
            match crate::bot::read_vendor_key(v) {
                Ok(k) if !k.trim().is_empty() => k,
                _ => crate::bot::read_llm_key(api_provider, active_model_id, models_by_provider)?,
            }
        }
        _ => crate::bot::read_llm_key(api_provider, active_model_id, models_by_provider)?,
    };
    Ok(ResolvedModel {
        base_url: entry.base_url.clone(),
        model: entry.model.clone(),
        api_provider: provider.to_string(),
        api_key,
        inference: super::schema::inference_for_entry(
            entry,
            crate::bot::ApiProvider::from_cfg(Some(provider)),
            global_max_tokens,
        ),
    })
}

#[cfg(test)]
mod model_override_tests {
    use super::*;

    fn models() -> Option<ModelsByProvider> {
        Some(ModelsByProvider {
            openai: vec![
                ModelEntry {
                    id: "glm".into(),
                    label: "GLM-4".into(),
                    base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                    model: "glm-4.6".into(),
                    vendor: None,
                    enabled: true,
                    context_k: None,
                    capabilities: None,
                    temperature: None,
                    top_p: None,
                    system_prompt: None,
                    max_tokens: None,
                },
                ModelEntry {
                    id: "disabled1".into(),
                    label: "停用条目".into(),
                    base_url: "https://x".into(),
                    model: "m".into(),
                    vendor: None,
                    enabled: false,
                    context_k: None,
                    capabilities: None,
                    temperature: None,
                    top_p: None,
                    system_prompt: None,
                    max_tokens: None,
                },
            ],
            anthropic: vec![],
        })
    }

    #[test]
    fn resolve_finds_entry_across_protocols() {
        let r = resolve_model_override(None, None, models().as_ref(), "glm", None).unwrap();
        assert!(r.inference.max_tokens >= 256);
        assert_eq!(r.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(r.model, "glm-4.6");
        assert_eq!(r.api_provider, "openai");
        assert!(
            !r.api_key.is_empty(),
            "key 回退全局（default 配置有占位或空）"
        );
    }

    #[test]
    fn resolve_rejects_missing_and_disabled() {
        assert!(resolve_model_override(None, None, models().as_ref(), "ghost", None).is_err());
        let err =
            resolve_model_override(None, None, models().as_ref(), "disabled1", None).unwrap_err();
        assert!(err.contains("停用"));
    }
}
