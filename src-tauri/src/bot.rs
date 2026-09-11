//! 内置机器人：大模型聊天 + WMessage 任务管理工具调用。
//!
//! 模块拆分后，本文件聚焦「工具调度核心 + 配置/审计底座」，
//! 编排/决策/工具循环拆到 4 个兄弟模块：
//! - `bot_chat`        — 入口编排（bot_chat / bot_compact / bot_execute_task）
//! - `bot_model_loop`  — 流式 SSE + 工具循环（run_model_loop / parse_sse_chunk / feed_think / TOOLS）
//! - `bot_scheduler`   — ⏰ 定时任务卡自动执行（start_scheduler / occurrence_after / sched_tests）
//! - `bot_slash`       — 旁路基础设施（bot_stop / 确认弹窗 / 机器人开关）
//!
//! 本模块保留：
//! - 工具分发核心：execute_tool + 20+ 个 tool_* 实现 + TaskRef 构造 helpers
//! - 配置底座：BotConfig / BotConfigView / keyring / migrate_legacy_key
//! - 审计底座：audit_log / audit_log_hook / escape_for_log / bot_log_read
//! - 参数上限常量 + check_len（bot_model_loop 拼装 tool_call 时也要用）
//!
//! 安全性（对齐《Harness 安全网关》需求）：
//! - 工具白名单：固定 TOOLS schema（在 bot_model_loop.rs）+ execute_tool match，模型编造的工具一律拒绝
//! - 调用熔断：单轮 Function 调用 ≤50 次 + 35 次软警告；默认对话轮数 50（聊天/任务执行/逐步执行统一）；
//!   多步 Skill 可在 frontmatter 自报 max_rounds 覆盖默认值；HTTP connect 15s / 总超时 300s
//! - 参数校验：标题/备注/关键词/子任务/截止时间长度上限、标签数量上限
//! - 审计日志：数据目录 bot.log 记录用户指令、工具名、参数、结果
//! - API Key 存系统凭据存储（keyring），文件不落明文；
//!   例外：Linux 无 secret-service（无 dbus 会话）时降级 bot-api-key.txt
//!   明文文件（chmod 0600）+ WARN 审计，否则 key 根本存不住

use crate::bot_skills::tool_use_skill;
use crate::error::{CommandError, CommandResult};

// 保留对外接口 re-export，避免拆分后 bot_skills / tests/llm_integration 等
// 已存在的调用方（`crate::bot::TaskRef` / `crate::bot::parse_sse_chunk` /
// `crate::bot::ToolCallDelta` / `crate::bot::BotChatResult`）中断。
// 新代码应优先直接引用 bot_chat / bot_model_loop 模块。
pub use crate::bot_chat::{BotChatResult, TaskRef};
pub use crate::bot_model_loop::{
    accumulate_tool_call_delta, drain_sse_lines, noop_replan, parse_sse_chunk, run_model_loop_core,
    LlmHttp, ModelLoopDeps, ToolCallDelta,
};
// bot_slash 是私有模块，集成测试（tests/llm_integration.rs）驱动
// run_model_loop_core 需要构造停止守卫，此处转出口径唯一公开。
pub use crate::bot_slash::StopGuard;
// 同上：audit 模块私有，ModelLoopDeps.audit 回调签名里的 AuditLevel 在 tests/
// 不可命名，测试构造 deps 需要它公开。
pub use crate::audit::AuditLevel;
// 同上：db 模块私有，tests/skill_e2e.rs 的调度器 e2e 用真实 upsert/load 验证
// persist_outcome 注入闭包的落库载荷（临时库文件）。
pub use crate::db::{load_all_skill_outcomes, upsert_skill_outcome, PersistedSkillOutcome};

use serde::{Deserialize, Serialize};
use std::io::Write;
use tauri::{AppHandle, Emitter};

// ───────────────────────── API 配置 ─────────────────────────

/// 凭据存储条目：macOS 钥匙串 / Windows 凭据管理器
pub const KEYRING_SERVICE: &str = "wmessage-bot";
pub const KEYRING_USER: &str = "api-key";

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
}

fn key_entry(slot: KeySlot) -> CommandResult<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, slot.keyring_user())
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
    if let Some(cached) = crate::audit::cached_probe_dir() {
        return cached.join(slot.plaintext_filename());
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    crate::audit::probe_dir(exe_dir.as_deref(), linux_app_data_dir())
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
        KeyBackend::System => key_entry(slot)?
            .delete_credential()
            .map_err(|e| CommandError::KeyringError(format!("清除 API Key 失败：{e}"))),
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

/// 按 slot 读取：降级后端记 WARN；System 后端顺带做降级文件回迁
fn read_key_of_slot(slot: KeySlot) -> CommandResult<String> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    } else {
        migrate_plaintext_key_if_system(slot);
    }
    read_api_key_at(backend, &plaintext_key_path_for(slot), slot)
}

/// 按 slot 存在性检查
fn has_key_of_slot(slot: KeySlot) -> CommandResult<bool> {
    let backend = key_backend();
    if backend == KeyBackend::PlaintextFile {
        warn_fallback_once(slot);
    } else {
        migrate_plaintext_key_if_system(slot);
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

// ───────────────────────── 工具调度核心（execute_tool dispatch） ─────────────────────────

/// 早退路径（pre_execute 拦截 / skill_on_step 熔断）的审计事件序列。
/// `tool.call` 已在入口发出，这里按写入顺序补齐后续事件并以 `tool.return` 配平，
/// 否则统计面板出现「悬挂调用」（call > return）。
/// 抽成纯函数：事件名 + kv + 顺序可单测（execute_tool 是 Wry 签名，无法 mock runtime 直调，
/// 与 skill_e2e.rs 注释记录的「泛型化重构暂缓」一致）；调用点只负责逐条 emit。
/// - 拦截路径（err=None）：pre_execute.deny + tool.return(reason=denied)
/// - 熔断路径（err=Some）：skill_on_step_error（补 Warn 可见性）+ tool.return(reason=skill_step_failed)
fn early_return_events(
    name: &str,
    reason: &str,
    dur_ms: u64,
    err: Option<&str>,
) -> Vec<(
    crate::audit::AuditLevel,
    &'static str,
    Vec<(&'static str, String)>,
)> {
    let mut events = Vec::with_capacity(2);
    match err {
        Some(e) => events.push((
            crate::audit::AuditLevel::Warn,
            "skill_on_step_error",
            vec![("tool", name.to_string()), ("err", e.to_string())],
        )),
        None => events.push((
            crate::audit::AuditLevel::Warn,
            "pre_execute.deny",
            vec![("tool", name.to_string())],
        )),
    }
    events.push((
        crate::audit::AuditLevel::Warn,
        "tool.return",
        vec![
            ("tool", name.to_string()),
            ("reason", reason.to_string()),
            ("exit_code", "none".to_string()),
            ("duration_ms", dur_ms.to_string()),
        ],
    ));
    events
}

/// 进程内执行工具，返回 (给模型的文本结果, 涉及的任务引用)
/// `pub` 让 `bot_skills::run_skill_scheduler`（DSL 调度器）可调用，
/// 不暴露给前端 — 通过 `is_atomic_tool` 黑名单 + pre-execute 校验保护。
/// 会话隔离：session_id 随调用链透传（DSL 调度器从 bot_chat 带下来），
/// 无 StopGuard 时按交互执行处理（DSL 调度器只在聊天上下文里跑）。
pub async fn execute_tool(
    app: &AppHandle,
    name: &str,
    args: &str,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    execute_tool_impl(app, name, args, None, true, session_id).await
}

/// execute_tool 的可停止版本：携带 /stop 守卫，run_python 等长耗时工具
/// 在执行中即可被中断；Skill 调度器等无守卫调用方走 execute_tool（stop=None）。
pub async fn execute_tool_with_stop(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    // 会话隔离：交互属性与会话归属从 StopGuard 取（无守卫 = 后台调度器路径
    // 不会出现——调度器走 run_task_in_chat 也持 StopGuard；None 仅 DSL 调度器遗留路径）
    let interactive = stop.map(|s| s.is_interactive()).unwrap_or(true);
    let session_id = stop.and_then(|s| s.session_id());
    execute_tool_impl(app, name, args, stop, interactive, session_id).await
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_impl(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let start = std::time::Instant::now();
    // 0. tool.call 结构化
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "tool.call",
        "tool" => name,
        "args_preview" => args.chars().take(80).collect::<String>(),
    );
    // 1. 后置拦截：原子黑名单（老板拍板）
    //    仅作为 Skill 内部子步骤、不允许裸调的底层原子 Function → 硬锁阻断
    //    只有 Skill 在 Running 状态时才放行；其他时候直接返回错误 + 提示走对应 Skill
    // 抽象层：execute_tool 通过 middleware::run_pre_execute 调 pre-execute
    // 任务卡执行流程（StopGuard.allow_atomic）视同 Skill 上下文放行——
    // EXECUTE_SYSTEM_PROMPT 把 link_file_to_task 列为收尾动作，该流程没有 SkillRun，
    // 不放行则 prompt 要求的核心动作必被自家网关否决。
    // （create_word_revisions 不在原子黑名单，聊天/执行均可直调）
    let active =
        crate::tool_guard::is_skill_active(session_id) || stop.is_some_and(|s| s.allow_atomic());
    if let Some(msg) = crate::middleware::run_pre_execute(app, name, active) {
        // tool.call 已发出，早退前必须配平 tool.return（reason=denied），
        // 否则统计面板出现「悬挂调用」（call > return）
        for (level, event, kv) in
            early_return_events(name, "denied", start.elapsed().as_millis() as u64, None)
        {
            crate::audit::write_event(app, level, event, &kv);
        }
        return (msg, Vec::new());
    }
    // 2. Skill 调度器步骤钩子：活动技能时计数/熔断/动作记录（use_skill 自身跳过）
    if name != "use_skill" {
        if let Err(e) = crate::bot_skills::skill_on_step(app, name, args, session_id) {
            // 补 Warn 可见性（skill_on_step_error）+ tool.return 配平（reason=skill_step_failed）
            for (level, event, kv) in early_return_events(
                name,
                "skill_step_failed",
                start.elapsed().as_millis() as u64,
                Some(&e.to_string()),
            ) {
                crate::audit::write_event(app, level, event, &kv);
            }
            return (e.into(), Vec::new());
        }
    }
    let (text, refs): (String, Vec<crate::bot_chat::TaskRef>) = match name {
        "list_tasks" => tool_list_tasks(app).await,
        "query_single_task" => tool_query_single_task(app, args).await,
        "create_task" => tool_create_task(app, args).await,
        "complete_task" => tool_complete_task(app, args).await,
        "delete_task" => tool_delete_task(app, args, interactive, session_id).await,
        "edit_task" => tool_edit_task(app, args).await,
        "add_subtask" => tool_add_subtask(app, args).await,
        "toggle_subtask" => tool_toggle_subtask(app, args).await,
        "remove_subtask" => tool_remove_subtask(app, args).await,
        "read_text_file" => {
            crate::bot_fs::tool_read_text_file(app, args, interactive, session_id).await
        }
        "ocr_image" => crate::ocr::tool_ocr_image(app, args, interactive, session_id).await,
        "grep_files" => crate::bot_fs::tool_grep_files(app, args, interactive, session_id).await,
        "list_files" => crate::bot_fs::tool_list_files(app, args, interactive, session_id).await,
        "link_file_to_task" => tool_link_file_to_task(app, args, session_id).await,
        "search_tasks" => tool_search_tasks(app, args).await,
        "extract_document" => tool_extract_document(app, args, interactive, session_id).await,
        "create_word" => tool_create_word(app, args).await,
        "create_word_revisions" => {
            tool_create_word_revisions(app, args, interactive, session_id).await
        }
        "create_excel" => tool_create_excel(app, args).await,
        "create_ppt" => tool_create_ppt(app, args).await,
        "create_pdf" => tool_create_pdf(app, args).await,
        "run_python" => tool_run_python(app, args, stop).await,
        "web_search" => tool_web_search(app, args).await,
        "fetch_url" => tool_fetch_url(app, args).await,
        "get_current_time" => tool_get_current_time(),
        "remember_fact" => crate::memory::tool_remember_fact(app, args).await,
        "recall_facts" => crate::memory::tool_recall_facts(app, args).await,
        "record_lesson" => crate::memory::tool_record_lesson(app, args).await,
        "use_skill" => tool_use_skill(app, args, session_id),
        other => (format!("未知工具：{other}"), Vec::new()),
    };

    // 3. post-execute 洋葱管线「出」钩子
    //    - 结构化审计事件（工具名/耗时/返回引用数/结果预览）写到 bot.log
    //    - 失败分类：未知工具→Error；含「失败/错误/error:」→Warn；其他→Info
    //    - 镜像调用 skill_on_step_post：技能步骤结果/失败检测
    let dur_ms = start.elapsed().as_millis() as u64;
    let level = crate::audit::classify_text(name, &text);
    // create_task/edit_task 带 files 参数时补 files_count/truncated kv（多文件绑定）
    let mut kv: Vec<(&str, String)> = vec![
        ("tool", name.to_string()),
        ("ms", dur_ms.to_string()),
        ("refs", refs.len().to_string()),
        ("preview", text.chars().take(80).collect::<String>()),
    ];
    if matches!(name, "create_task" | "edit_task") {
        if let Some((cnt, truncated)) = files_audit_kv(args) {
            kv.push(("files_count", cnt.to_string()));
            kv.push(("truncated", truncated.to_string()));
        }
    }
    crate::audit::write_event(app, level, "tool.return", &kv);
    if name != "use_skill" {
        crate::bot_skills::skill_on_step_post(app, name, &text, dur_ms, level, session_id);
    }

    (text, refs)
}

pub(crate) fn parse_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or(serde_json::Value::Null)
}

// ───────────────────────── 时间 + 长期记忆工具 ─────────────────────────

/// get_current_time：返回本地日期时间+星期（模型做「今天/明天/周几」判断的锚点，禁止猜日期）
fn tool_get_current_time() -> (String, Vec<crate::bot_chat::TaskRef>) {
    let now = chrono::Local::now();
    let week = [
        "星期一",
        "星期二",
        "星期三",
        "星期四",
        "星期五",
        "星期六",
        "星期日",
    ][chrono::Datelike::weekday(&now).num_days_from_monday() as usize];
    (
        format!("现在：{} {week}", now.format("%Y-%m-%d %H:%M:%S")),
        Vec::new(),
    )
}

/// 解析 create_task/edit_task 的 files 参数（[{path, isDir}]）：
/// 去重保序、空路径丢弃、超 MAX_TASK_FILES 截断（Rust 侧硬上限）。
/// 返回 Some((files, truncated))；无 files 字段返回 None（不改绑定）。
fn parse_task_files_arg(v: &serde_json::Value) -> Option<(Vec<crate::db::TaskFile>, bool)> {
    let arr = v["files"].as_array()?;
    let truncated = arr.len() > crate::db::MAX_TASK_FILES;
    let mut out: Vec<crate::db::TaskFile> = Vec::new();
    for item in arr {
        let Some(path) = item["path"].as_str().map(|s| s.trim().to_string()) else {
            continue;
        };
        if path.is_empty() || out.iter().any(|f| f.path == path) {
            continue;
        }
        out.push(crate::db::TaskFile {
            path,
            is_dir: item["isDir"].as_bool().unwrap_or(false),
        });
        if out.len() >= crate::db::MAX_TASK_FILES {
            break;
        }
    }
    Some((out, truncated))
}

/// 模型来源 files 的安全校验：create_task/edit_task 的
/// files 参数直接来自模型，不加校验模型可把任意目录标 isDir=true 绑进任务卡，
/// `allowed_dirs` 会把它并入文件白名单（且先于 permMode 分流），strict 模式也被架空。
/// 收窄（对齐 link_file_to_task）：仅放行 AI_Gen_Files 目录内的已存在文件，强制 isDir=false；
/// 被拒条目记审计。用户亲手绑定走 bind_file 系统弹框，不在此限。
fn sanitize_task_files_arg(
    app: &AppHandle,
    v: &serde_json::Value,
) -> Option<(Vec<crate::db::TaskFile>, bool)> {
    let (files, truncated) = parse_task_files_arg(v)?;
    let gen_canon = std::fs::canonicalize(crate::db::data_dir(app).join("AI_Gen_Files")).ok();
    let (out, dropped) = sanitize_task_files_in(gen_canon.as_deref(), files);
    if dropped > 0 {
        audit_log(
            app,
            &format!("task_files_sanitized | dropped: {dropped} | 模型来源 files 仅放行 AI_Gen_Files 内已存在文件"),
        );
    }
    Some((out, truncated))
}

/// sanitize 的纯内核（单测可注入 gen 目录）：仅放行 gen_canon 目录内的已存在文件，
/// 强制 isDir=false（目录绑定一律丢——目录绑定的授权只能来自用户手选）。
/// 返回 (保留列表, 丢弃数)。
fn sanitize_task_files_in(
    gen_canon: Option<&std::path::Path>,
    files: Vec<crate::db::TaskFile>,
) -> (Vec<crate::db::TaskFile>, usize) {
    let mut out: Vec<crate::db::TaskFile> = Vec::new();
    let mut dropped = 0usize;
    for f in files {
        let ok = !f.is_dir
            && gen_canon.is_some_and(|g| {
                std::fs::canonicalize(&f.path)
                    .map(|c| c.starts_with(g))
                    .unwrap_or(false)
            });
        if ok {
            out.push(f);
        } else {
            dropped += 1;
        }
    }
    (out, dropped)
}

/// files 写回任务时的双写：新 files 列 + 旧 file_path/file_is_dir 首条（过渡期旧版本可读）
pub fn apply_files_to_task(t: &mut crate::db::Task, files: Vec<crate::db::TaskFile>) {
    t.file_path = files.first().map(|f| f.path.clone());
    t.file_is_dir = files.first().map(|f| f.is_dir);
    t.files = if files.is_empty() { None } else { Some(files) };
}

/// tool.return 审计补充 kv（create_task/edit_task 带 files 时）：(原始条数, 是否截断)
fn files_audit_kv(args: &str) -> Option<(usize, bool)> {
    let v = parse_args(args);
    let arr = v["files"].as_array()?;
    Some((arr.len(), arr.len() > crate::db::MAX_TASK_FILES))
}

/// 改库后广播：挂件重读（tasks-changed）+ 主窗口合并 UI 不回写（tasks-updated, source: Bot）
pub fn broadcast_after_mutation<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    upserts: Vec<crate::db::Task>,
    deletes: Vec<String>,
) {
    if !upserts.is_empty() || !deletes.is_empty() {
        let _ = app.emit("tasks-changed", ());
        let _ = app.emit(
            "tasks-updated",
            serde_json::json!({ "source": crate::mutation::MutationOrigin::Bot.as_str(), "upserts": upserts, "deletes": deletes }),
        );
    }
}

async fn active_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    crate::db::db_load(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.deleted_at.is_none() && t.archived != Some(true) && t.column != "done")
        .collect()
}

// ───────────────────────── 任务管理工具实现（20+ functions） ─────────────────────────

async fn tool_list_tasks(app: &AppHandle) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let tasks = active_tasks(app).await;
    if tasks.is_empty() {
        return ("当前没有未完成的任务".into(), Vec::new());
    }
    let mut lines: Vec<String> = Vec::new();
    for t in &tasks {
        let col = match t.column.as_str() {
            "doing" => "进行中",
            "done" => "已完成",
            _ => "待办",
        };
        let due = t
            .due
            .as_deref()
            .map(|d| format!("，截止 {d}"))
            .unwrap_or_default();
        lines.push(format!("- [{}] {}{}（id={}）", col, t.title, due, t.id));
    }
    let refs: Vec<crate::bot_chat::TaskRef> = tasks
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

/// 单卡查询（白名单单点工具）：
/// 按 id 取单张任务卡的完整详情（区别于 list_tasks 的批量清单 + search_tasks 的关键词检索）。
/// - 必填参数：id（任务卡 UUID）
/// - 输出：标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除/机器人执行状态指示
/// - 单点白名单工具（非原子黑名单），LLM 可裸调
/// - 返回的 TaskRef 供后续 taskId 操作（complete_task / edit_task / bind_file 等）跟随引用
async fn tool_query_single_task(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(id) = v["id"].as_str().map(|s| s.trim().to_string()) else {
        return ("query_single_task 缺少 id 参数".into(), Vec::new());
    };
    if id.is_empty() {
        return ("query_single_task id 不能为空".into(), Vec::new());
    }
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
        return ("查询失败：数据库读取错误".into(), Vec::new());
    };
    let Some(t) = tasks.into_iter().find(|t| t.id == id) else {
        return (format!("未找到 id={id} 的任务卡"), Vec::new());
    };
    let col = match t.column.as_str() {
        "doing" => "进行中",
        "done" => "已完成",
        _ => "待办",
    };
    let mut lines: Vec<String> = vec![format!("- [{}] {}（id={}）", col, t.title, t.id)];
    if let Some(note) = &t.note {
        if !note.is_empty() {
            lines.push(format!("  备注：{note}"));
        }
    }
    if let Some(due) = &t.due {
        if !due.is_empty() {
            lines.push(format!("  截止：{due}"));
        }
    }
    if let Some(subtasks) = &t.subtasks {
        if !subtasks.is_empty() {
            lines.push(format!("  子任务（{}）：", subtasks.len()));
            for st in subtasks {
                let mark = if st.done { "✓" } else { "·" };
                lines.push(format!("    [{mark}] {}（id={}）", st.text, st.id));
            }
        }
    }
    if let Some(tags) = &t.tags {
        if !tags.is_empty() {
            lines.push(format!("  标签：{}", tags.join(", ")));
        }
    }
    let bound_files = t.effective_files();
    if !bound_files.is_empty() {
        lines.push("  绑定文件：".to_string());
        for f in &bound_files {
            let kind = if f.is_dir { "目录" } else { "文件" };
            lines.push(format!("    - [{kind}] {}", f.path));
        }
    }
    let mut status: Vec<&str> = Vec::new();
    if t.archived == Some(true) {
        status.push("已归档");
    }
    if t.deleted_at.is_some() {
        status.push("已删除（回收站）");
    }
    if t.bot_assigned == Some(true) {
        status.push("机器人执行中");
    }
    if !status.is_empty() {
        lines.push(format!("  状态：{}", status.join(" / ")));
    }
    (
        lines.join("\n"),
        vec![crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        }],
    )
}

/// 搜索任务：搜所有任务卡（待办/进行中/已完成/已归档；不含回收站软删）。
/// 关键词匹配标题/备注/标签/子任务（大小写不敏感 contains）；结果带 id 供后续 taskId 操作
async fn tool_search_tasks(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("search_tasks 缺少 query".into(), Vec::new());
    };
    if query.is_empty() {
        return ("搜索关键词不能为空".into(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e.into(), Vec::new());
    }
    let tasks = crate::db::db_load(app.clone()).await.unwrap_or_default();
    let mut hits: Vec<crate::db::Task> = tasks
        .into_iter()
        .filter(|t| {
            t.deleted_at.is_none() && {
                let title_hit = t.title.to_lowercase().contains(&query);
                let note_hit = t
                    .note
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&query))
                    .unwrap_or(false);
                let tag_hit = t
                    .tags
                    .as_deref()
                    .map(|tags| tags.iter().any(|tg| tg.to_lowercase().contains(&query)))
                    .unwrap_or(false);
                let sub_hit = t
                    .subtasks
                    .as_deref()
                    .map(|subs| subs.iter().any(|s| s.text.to_lowercase().contains(&query)))
                    .unwrap_or(false);
                title_hit || note_hit || tag_hit || sub_hit
            }
        })
        .collect();
    if hits.is_empty() {
        return (
            format!(
                "没有找到匹配「{}」的任务",
                v["query"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    }
    hits.sort_by(|a, b| {
        a.order
            .unwrap_or(f64::MAX)
            .partial_cmp(&b.order.unwrap_or(f64::MAX))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut lines: Vec<String> = Vec::new();
    for t in &hits {
        let col = match t.column.as_str() {
            "doing" => "进行中",
            "done" => "已完成",
            _ => "待办",
        };
        let arch = if t.archived == Some(true) {
            "（已归档）"
        } else {
            ""
        };
        let due = t
            .due
            .as_deref()
            .map(|d| format!("，截止 {d}"))
            .unwrap_or_default();
        let tags = t
            .tags
            .as_deref()
            .map(|ts| format!("，标签：{}", ts.join("/")))
            .unwrap_or_default();
        lines.push(format!(
            "- [{}] {}{}{}{}（id={}）",
            col, t.title, arch, due, tags, t.id
        ));
    }
    let refs: Vec<crate::bot_chat::TaskRef> = hits
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

async fn tool_create_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(title) = v["title"].as_str() else {
        return ("create_task 缺少 title".into(), Vec::new());
    };
    let title = title.trim();
    if title.is_empty() {
        return ("任务标题不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(title, MAX_TITLE, "任务标题") {
        return (e.into(), Vec::new());
    }
    if let Some(n) = v["note"].as_str() {
        if let Err(e) = check_len(n, MAX_NOTE, "备注") {
            return (e.into(), Vec::new());
        }
    }
    if let Some(d) = v["due"].as_str() {
        if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
            return (e.into(), Vec::new());
        }
    }
    let mut task = crate::db::Task {
        id: uuid::Uuid::new_v4().simple().to_string(),
        title: title.to_string(),
        due: v["due"].as_str().map(|s| s.to_string()),
        note: v["note"].as_str().map(|s| s.to_string()),
        tags: None,
        files: None,
        file_path: None,
        file_is_dir: None,
        column: match v["column"].as_str() {
            Some("doing") => "doing".into(),
            _ => "todo".into(),
        },
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(chrono::Utc::now().timestamp_millis()),
        schedule: None,
        sched_last: None,
        bot_assigned: None,
        expected_updated_at: None, // 新建任务：无读快照基线
    };
    // 多文件绑定：files 参数 [{path,isDir}]，超 10 截断 + 警告
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件）
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v) {
        if truncated {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
        apply_files_to_task(&mut task, files);
    }
    // 插到列表顶部：取当前最小 order 减 1
    if let Ok(all) = crate::db::db_load(app.clone()).await {
        let min = all
            .iter()
            .filter_map(|t| t.order)
            .fold(f64::INFINITY, f64::min);
        task.order = Some(if min.is_finite() { min - 1.0 } else { 0.0 });
    }
    match crate::db::db_upsert(app.clone(), vec![task.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![task.clone()], vec![]);
            (
                format!("已新建任务「{}」{files_warn}", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("新建任务失败：{e}"), Vec::new()),
    }
}

async fn tool_complete_task(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    // 走 resolve_task（taskId 精确匹配优先、title 关键词兜底 +
    // taskId/title 交叉校验），与其它任务操作工具对齐——只读 title 会让
    // schema 声明的「taskId 优先」被完全忽略，任务卡执行路径只传 taskId 时确定性失败。
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let mut next = task.clone();
    next.column = "done".into();
    next.completed_at = Some(chrono::Utc::now().timestamp_millis());
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at
    next.updated_at = next.completed_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已完成任务「{}」", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("完成任务失败：{e}"), Vec::new()),
    }
}

/// 删除任务到回收站：**弹窗确认后才执行**（危险操作护栏；60s 无响应默认拒绝）
async fn tool_delete_task(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let approved = crate::bot_slash::ask_user_confirm(
        app,
        "delete_task",
        &task.title,
        interactive,
        session_id,
    )
    .await;
    if !approved {
        return ("用户拒绝了删除，任务未删除".into(), Vec::new());
    }
    let mut next = task.clone();
    next.deleted_at = Some(chrono::Utc::now().timestamp_millis());
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at
    next.updated_at = next.deleted_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已删除任务「{}」（进回收站）", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("删除任务失败：{e}"), Vec::new()),
    }
}

/// 按标题关键词找第一条未完成任务（大小写不敏感）
async fn find_task_by_keyword(app: &AppHandle, kw: &str) -> Option<crate::db::Task> {
    active_tasks(app)
        .await
        .into_iter()
        .find(|t| t.title.to_lowercase().contains(kw))
}

/// 定位任务：优先 taskId 精确匹配，其次标题关键词模糊匹配。
/// 返回 (task, 定位说明)；找不到返回错误文案。
async fn resolve_task(
    app: &AppHandle,
    v: &serde_json::Value,
) -> Result<crate::db::Task, CommandError> {
    if let Some(id) = v["taskId"].as_str() {
        let id = id.trim();
        if !id.is_empty() {
            if let Some(t) = active_tasks(app).await.into_iter().find(|t| t.id == id) {
                // 交叉校验：同窗格连续操作不同任务卡时，模型会沿用上一张卡的
                // taskId 张冠李戴。taskId 与 title 关键词同时给出且对不上 → 不信 id，
                // 改用 title 重新定位（定位不到就报错，让模型/用户确认）
                if let Some(kw) = v["title"].as_str().map(|s| s.trim().to_lowercase()) {
                    if !kw.is_empty() && !t.title.to_lowercase().contains(&kw) {
                        if let Some(t2) = find_task_by_keyword(app, &kw).await {
                            return Ok(t2);
                        }
                        return Err(CommandError::DomainRule {
                            domain: "argument".to_string(),
                            reason: format!(
                                "taskId 命中的任务「{}」与标题关键词「{}」不符，且按标题未找到未完成任务，请确认操作对象",
                                t.title,
                                v["title"].as_str().unwrap_or("")
                            ),
                        });
                    }
                }
                return Ok(t);
            }
            return Err(CommandError::DomainRule {
                domain: "task".to_string(),
                reason: format!("未找到 id={id} 的未完成任务（可能已完成或已删除）"),
            });
        }
    }
    if let Some(kw) = v["title"].as_str() {
        let kw = kw.trim().to_lowercase();
        if !kw.is_empty() {
            if let Some(t) = find_task_by_keyword(app, &kw).await {
                return Ok(t);
            }
            return Err(CommandError::DomainRule {
                domain: "task".to_string(),
                reason: format!(
                    "未找到匹配「{}」的未完成任务",
                    v["title"].as_str().unwrap_or("")
                ),
            });
        }
    }
    Err(CommandError::DomainRule {
        domain: "argument".to_string(),
        reason: "缺少 taskId 或 title 参数".to_string(),
    })
}

async fn tool_edit_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let mut next = task.clone();
    let mut changed: Vec<&str> = Vec::new();
    if let Some(nt) = v["newTitle"].as_str() {
        let nt = nt.trim();
        if !nt.is_empty() {
            if let Err(e) = check_len(nt, MAX_TITLE, "新标题") {
                return (e.into(), Vec::new());
            }
            next.title = nt.to_string();
            changed.push("标题");
        }
    }
    if let Some(n) = v["note"].as_str() {
        if !n.trim().is_empty() {
            if let Err(e) = check_len(n, MAX_NOTE, "备注") {
                return (e.into(), Vec::new());
            }
        }
        next.note = if n.trim().is_empty() {
            None
        } else {
            Some(n.to_string())
        };
        changed.push("备注");
    }
    if let Some(d) = v["due"].as_str() {
        if !d.trim().is_empty() {
            if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
                return (e.into(), Vec::new());
            }
        }
        next.due = if d.trim().is_empty() {
            None
        } else {
            Some(d.to_string())
        };
        changed.push("截止时间");
    }
    if let Some(tags) = v["tags"].as_array() {
        if tags.len() > MAX_TAGS {
            return (format!("标签数量超上限（最多 {MAX_TAGS} 个）"), Vec::new());
        }
        for t in tags {
            if let Some(ts) = t.as_str() {
                if let Err(e) = check_len(ts, MAX_TAG_LEN, "标签") {
                    return (e.into(), Vec::new());
                }
            }
        }
        let list: Vec<String> = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        next.tags = if list.is_empty() { None } else { Some(list) };
        changed.push("标签");
    }
    // 多文件绑定：files 参数 [{path,isDir}] 整体替换列表；空数组清除；超 10 截断 + 警告
    // 模型来源 files 经安全校验（仅 AI_Gen_Files 内文件）
    let mut files_warn = "";
    if let Some((files, truncated)) = sanitize_task_files_arg(app, &v) {
        if truncated {
            files_warn = "（绑定文件超上限，已截断为前 10 个）";
        }
        apply_files_to_task(&mut next, files);
        changed.push("绑定文件");
    }
    if let Some(c) = v["column"].as_str() {
        let c = c.trim();
        if matches!(c, "todo" | "doing" | "done") && c != next.column {
            next.column = c.to_string();
            // 列变更补完成语义（与主窗口一致）
            if c == "done" {
                next.completed_at = Some(chrono::Utc::now().timestamp_millis());
                next.archived = Some(false);
            } else {
                next.completed_at = None;
                next.archived = None;
            }
            changed.push("状态列");
        }
    }
    if changed.is_empty() {
        return ("没有可修改的字段".into(), Vec::new());
    }
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!(
                    "已更新任务「{}」（{}）{files_warn}",
                    next.title,
                    changed.join("、")
                ),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("编辑任务失败：{e}"), Vec::new()),
    }
}

async fn tool_add_subtask(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(text) = v["text"].as_str().map(|s| s.trim()) else {
        return ("add_subtask 缺少 text".into(), Vec::new());
    };
    if text.is_empty() {
        return ("子任务内容不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(text, MAX_SUBTASK_TEXT, "子任务内容") {
        return (e.into(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let mut next = task.clone();
    let mut subs = next.subtasks.unwrap_or_default();
    subs.push(crate::db::Subtask {
        id: uuid::Uuid::new_v4().simple().to_string(),
        text: text.to_string(),
        done: false,
    });
    next.subtasks = Some(subs);
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已给任务「{}」添加子任务「{}」", next.title, text),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("添加子任务失败：{e}"), Vec::new()),
    }
}

async fn tool_toggle_subtask(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(skw) = v["text"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("toggle_subtask 缺少 text".into(), Vec::new());
    };
    if let Err(e) = check_len(&skw, MAX_SUBTASK_TEXT, "子任务关键词") {
        return (e.into(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let Some(idx) = subs
        .iter()
        .position(|s| s.text.to_lowercase().contains(&skw))
    else {
        return (
            format!(
                "任务「{}」没有匹配「{}」的子任务",
                task.title,
                v["text"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    };
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2[idx].done = !subs2[idx].done;
    next.subtasks = Some(subs2);
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            let st_text = next
                .subtasks
                .as_ref()
                .and_then(|s| s.get(idx))
                .map(|s| s.text.clone())
                .unwrap_or_default();
            let done_mark = next
                .subtasks
                .as_ref()
                .and_then(|s| s.get(idx))
                .map(|s| s.done)
                .unwrap_or(false);
            (
                format!(
                    "子任务「{st_text}」已{}",
                    if done_mark {
                        "勾选 ✓"
                    } else {
                        "取消勾选"
                    }
                ),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("切换子任务状态失败：{e}"), Vec::new()),
    }
}

/// 删除单条子任务
async fn tool_remove_subtask(
    app: &AppHandle,
    args: &str,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(skw) = v["text"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("remove_subtask 缺少 text".into(), Vec::new());
    };
    if skw.is_empty() {
        return ("子任务关键词不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(&skw, MAX_SUBTASK_TEXT, "子任务关键词") {
        return (e.into(), Vec::new());
    }
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e.into(), Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let Some(idx) = subs
        .iter()
        .position(|s| s.text.to_lowercase().contains(&skw))
    else {
        return (
            format!(
                "任务「{}」没有匹配「{}」的子任务",
                task.title,
                v["text"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    };
    let removed_text = subs[idx].text.clone();
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2.remove(idx);
    next.subtasks = Some(subs2);
    next.expected_updated_at = next.updated_at; // RMW 写回基线 = 快照 updated_at（写前比对，防整行覆盖 lost-update）
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已删除任务「{}」的子任务「{}」", next.title, removed_text),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("删除子任务失败：{e}"), Vec::new()),
    }
}

/// 登记产物到任务卡执行流程的产物清单（不立即绑）
///
/// 设计：bot 流程内调用是「登记」语义——记到内存登记表 `bot_artifacts::REGISTRY`，
/// 流程结束按 `TaskExecOrigin` 分流（D4d）弹汇总窗口让用户勾选绑定。
/// 普通 chat 场景（无 TaskExecOrigin 上下文）直接拒，避免登记表被反复污染。
///
/// 路径白名单：仅接受 AI_Gen_Files 目录内的文件（产物必经此目录生成），
/// 防止 LLM 借 bind_files 间接读 ~/.ssh/id_rsa 等敏感文件。
async fn tool_link_file_to_task(
    app: &AppHandle,
    args: &str,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    if !crate::tool_guard::is_task_execution_flow(session_id) {
        return (
            "link_file_to_task 仅在任务卡执行流程（🤖 按钮 / ⏰ 定时 / 📦 批量）内有效；普通对话场景调用无效果（不报错也不绑），不要反复尝试".into(),
            Vec::new(),
        );
    }
    let v = parse_args(args);
    let Some(path) = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("link_file_to_task 缺少 path".into(), Vec::new());
    };
    if !std::path::Path::new(&path).exists() {
        return (format!("路径不存在，拒绝登记：{path}"), Vec::new());
    }
    let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| std::path::PathBuf::from(&path));
    let gen_dir = crate::db::data_dir(app).join("AI_Gen_Files");
    let in_gen = match std::fs::canonicalize(&gen_dir) {
        Ok(gen) => canon.starts_with(&gen),
        Err(_) => false,
    };
    if !in_gen {
        return (
            "已拒绝登记该路径：link_file_to_task 只能登记 AI_Gen_Files 目录内的产物；其他文件请在任务卡上手动「绑定文件」".into(),
            Vec::new(),
        );
    }
    // taskId / title 任一必传（绑定目标）
    let task_key = v["taskId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            v["title"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| format!("title:{s}"))
        });
    let Some(key) = task_key else {
        return ("link_file_to_task 缺少 taskId / title".into(), Vec::new());
    };
    let kind = match v["kind"].as_str().unwrap_or("final") {
        "intermediate" => crate::bot_artifacts::ArtifactKind::Intermediate,
        _ => crate::bot_artifacts::ArtifactKind::Final,
    };
    crate::bot_artifacts::register(&key, path.clone(), kind).await;
    let kind_label = match kind {
        crate::bot_artifacts::ArtifactKind::Final => "最终产物",
        crate::bot_artifacts::ArtifactKind::Intermediate => "中间产物",
    };
    (
        format!(
            "已登记{kind_label}「{path}」。流程结束、任务完成时会弹汇总窗口让你勾选绑定；不要在此刻绑定——任务未完成或中断不绑定。"
        ),
        Vec::new(),
    )
}

// ───────────────────────── 文档 / Python 工具（bot_py 桥接） ─────────────────────────

/// 提取文档文本：path 给定则直读（任务卡绑定文件），否则弹框选文件；返回路径 + 文本供模型阅读/润色
/// extract_document path 白名单：任务卡绑定文件 / AI_Gen_Files 目录内文件静默放行。
/// 规范化路径比较，防 ../ 绕过。无 path 时走弹框（用户亲手选，不受此限）。
/// 授权分流：其余路径不硬拒，走 bot_fs::resolve_with_perm 分流
///（strict 硬拒 / ask 弹授权窗 / yolo 放行），拒绝文案透传给模型。
/// interactive/session_id 透传给授权弹窗：后台执行（interactive=false）不弹窗直接拒。
async fn extract_path_check(
    app: &AppHandle,
    path: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> Result<(), String> {
    let Ok(canon) = std::fs::canonicalize(path) else {
        return Err(format!("路径不存在或不可访问：{path}"));
    };
    // 1) AI_Gen_Files 目录内
    let gen_dir = crate::db::data_dir(app).join("AI_Gen_Files");
    if let Ok(gen) = std::fs::canonicalize(&gen_dir) {
        if canon.starts_with(&gen) {
            return Ok(());
        }
    }
    // 2) 任务卡绑定文件（多文件绑定：files 列表 + 旧字段兜底走 effective_files）
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
        for t in tasks {
            for f in t.effective_files() {
                if let Ok(fc) = std::fs::canonicalize(&f.path) {
                    if fc == canon {
                        return Ok(());
                    }
                }
            }
        }
    }
    // 3) 授权分流（与 bot_fs 同一口径：白名单静默放行 / strict 拒 /
    //    ask 弹窗 / yolo 放）
    crate::bot_fs::resolve_with_perm(app, "extract_document", path, interactive, session_id)
        .await
        .map(|_| ())
}

async fn tool_extract_document(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let path_opt = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 无 path 时 bot_py::doc_extract 会弹系统选择框，
    // 后台定时执行弹框 = 永久阻塞卡死，必须直接拒绝并引导模型传 path
    if path_opt.is_none() && !interactive {
        return (
            "失败：后台执行不能弹窗选文件，请提供 path 参数指定文档路径".into(),
            Vec::new(),
        );
    }
    // 分页参数：offset 字符偏移续读；limit 默认 30000、硬钳 60000
    let offset = v["offset"].as_u64().unwrap_or(0) as usize;
    let limit = v["limit"]
        .as_u64()
        .map(|n| (n as usize).min(EXTRACT_MAX_LIMIT))
        .filter(|&n| n > 0)
        .unwrap_or(EXTRACT_DEFAULT_LIMIT);
    // 模型直传 path 时授权校验（无 path 走弹框，用户亲手选不受限）
    if let Some(p) = path_opt.as_deref() {
        if let Err(e) = extract_path_check(app, p, interactive, session_id).await {
            return (e.into(), Vec::new());
        }
    }
    match crate::bot_py::doc_extract(app.clone(), path_opt).await {
        Ok(res) => (
            format_extract_output(&res.path, &res.text, offset, limit),
            Vec::new(),
        ),
        Err(e) => (format!("提取失败：{e}"), Vec::new()),
    }
}

/// extract_document 输出格式化：字符级分页（可续读，不硬截断）。
/// 头部 [位置] 行让模型知道总量与续读点；还有更多时尾部给 offset 续读提示。
/// offset 按字符计（非字节）；limit 默认 30000、硬钳 60000（防爆上下文）。
const EXTRACT_DEFAULT_LIMIT: usize = 30000;
const EXTRACT_MAX_LIMIT: usize = 60000;

fn format_extract_output(path: &str, text: &str, offset: usize, limit: usize) -> String {
    let total = text.chars().count();
    if offset >= total && total > 0 {
        return format!(
            "[文档路径] {path}\noffset {offset} 已超出文档总长 {total} 字符，没有更多内容"
        );
    }
    let slice: String = text.chars().skip(offset).take(limit).collect();
    let end = offset + slice.chars().count();
    let mut out =
        format!("[文档路径] {path}\n[位置] {offset}–{end} / 共 {total} 字符\n[文档内容]\n{slice}");
    if end < total {
        out.push_str(&format!(
            "\n\n（已截断：还有 {} 字符未读，用 offset={end} 参数续读；修订模式请把 original 参数填上你实际收到的原文行列表）",
            total - end
        ));
    }
    out
}

/// 解析文档工具共用的 filename 参数
fn opt_filename(v: &serde_json::Value) -> Option<String> {
    v["filename"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 生成 Word：润色后的段落写新文档（只产出、不覆盖，落 AI_Gen_Files）
async fn tool_create_word(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ("create_word 缺少 paragraphs".into(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ("paragraphs 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    let tables = v.get("tables").cloned();
    match crate::bot_py::doc_make_word(app.clone(), title, paragraphs, opt_filename(&v), tables)
        .await
    {
        Ok(out) => (format!("已生成 Word 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 修订模式 Word：回读原文 + 修订段落 diff，产出带 track changes 标记的文档
async fn tool_create_word_revisions(
    app: &AppHandle,
    args: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    // originalPath 由模型转述，同样过授权校验（防回读任意文件，走分流）
    if let Some(op) = v["originalPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(e) = extract_path_check(app, op, interactive, session_id).await {
            return (e.into(), Vec::new());
        }
    }
    let Some(arr) = v["revised"].as_array() else {
        return (
            "create_word_revisions 缺少 revised（润色后的段落列表）".into(),
            Vec::new(),
        );
    };
    let revised: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if revised.is_empty() {
        return ("revised 不能为空".into(), Vec::new());
    }
    let path = v["originalPath"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let original: Vec<String> = v["original"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if path.is_none() && original.is_empty() {
        return (
            "create_word_revisions 缺少原文：请传 originalPath（来自 extract_document 的 [文档路径]）或 original 行列表"
                .into(),
            Vec::new(),
        );
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_word_revisions(app.clone(), title, path, original, revised, opt_filename(&v)).await
    {
        Ok((out, engine)) => (
            format!(
                "已生成修订版 Word（修订模式：删除线=删、红色下划线=增，可在 Word「审阅」里逐条接受/拒绝；引擎：{}）：{out}",
                if engine == "dotnet" { ".NET OpenXML" } else { "Python 兜底（.NET 不可用或执行失败）" }
            ),
            Vec::new(),
        ),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}
async fn tool_create_excel(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(sheets) = v["sheets"].as_array() else {
        return ("create_excel 缺少 sheets".into(), Vec::new());
    };
    if sheets.is_empty() {
        return ("sheets 不能为空".into(), Vec::new());
    }
    match crate::bot_py::doc_make_excel(app.clone(), sheets.clone(), opt_filename(&v)).await {
        Ok(out) => (format!("已生成 Excel 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PPT：slides 结构 [{title, bullets: [..]}]
async fn tool_create_ppt(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(slides) = v["slides"].as_array() else {
        return ("create_ppt 缺少 slides".into(), Vec::new());
    };
    if slides.is_empty() {
        return ("slides 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    // theme：blue/navy/teal/forest/wine/sky/plum/coral/dark/green 十套；模型自选，非法回退 blue
    let theme = v["theme"].as_str().map(|s| s.to_string());
    // customColors：可选配色覆盖（脚本侧校验 hex，非法忽略）
    let custom_colors = v.get("customColors").cloned();
    match crate::bot_py::doc_make_ppt(
        app.clone(),
        title,
        slides.clone(),
        opt_filename(&v),
        theme,
        custom_colors,
    )
    .await
    {
        Ok(out) => (format!("已生成 PPT 演示文稿：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PDF：title + 段落列表
async fn tool_create_pdf(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ("create_pdf 缺少 paragraphs".into(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ("paragraphs 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_pdf(app.clone(), title, paragraphs, opt_filename(&v)).await {
        Ok(out) => (format!("已生成 PDF 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 联网搜索：本机执行（引擎路由 Tavily/Brave/双引擎抓取，见 bot_web::resolve_search_route），
/// 结果回传给模型
async fn tool_web_search(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("web_search 缺少 query".into(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e.into(), Vec::new());
    }
    audit_log(
        app,
        &format!("web_search | query: {}", escape_for_log(&query, 100)),
    );
    match crate::bot_web::web_search_with_config(app, &query).await {
        Ok(results) => (results, Vec::new()),
        Err(e) => (format!("搜索失败：{e}"), Vec::new()),
    }
}

/// 抓取网页正文：http/https 公网地址，转纯文本回传（截 30000 字）
async fn tool_fetch_url(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(u) = v["url"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("fetch_url 缺少 url".into(), Vec::new());
    };
    if let Err(e) = check_len(&u, MAX_KEYWORD, "网址") {
        return (e.into(), Vec::new());
    }
    audit_log(
        app,
        &format!("fetch_url | url: {}", escape_for_log(&u, 100)),
    );
    match crate::bot_web::fetch_text(&u).await {
        Ok(text) => {
            let mut out: String = text.chars().take(30000).collect();
            if out.chars().count() >= 30000 {
                out.push_str("\n\n（内容过长已截断）");
            }
            (out, Vec::new())
        }
        Err(e) => (format!("抓取失败：{e}"), Vec::new()),
    }
}

/// 自由 Python 编程：开关开启才放行（超时 60s、独立临时目录、输出截断）
/// py_exec_sync 是 sync 阻塞（最长 300s），必须经 async 包装挪到 blocking
/// 线程池，不得占住 async runtime worker（与 doc_* 同模式）。
/// 透传 /stop 令牌，在途 Python 可被中断（StopGuard → owned StopToken）。
async fn tool_run_python(
    app: &AppHandle,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(code) = v["code"].as_str() else {
        return ("run_python 缺少 code".into(), Vec::new());
    };
    // 超时优先级：工具参数 timeoutSecs > 设置页配置 python_timeout_secs > 内置 60s
    //（pandas 大计算 60s 偏紧；硬钳 300s 在 bot_py::resolve_timeout）
    let timeout_secs = v["timeoutSecs"]
        .as_u64()
        .or_else(|| load_config(app).python_timeout_secs);
    match crate::bot_py::py_exec_sync_async(
        app.clone(),
        code.to_string(),
        timeout_secs,
        stop.map(|s| s.token()),
    )
    .await
    {
        Ok(r) => {
            let mut out = String::new();
            if !r.stdout.trim().is_empty() {
                out.push_str(&r.stdout);
            }
            if !r.stderr.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[stderr] {}", r.stderr.trim()));
            }
            if out.is_empty() {
                out = "执行完成（无输出）".into();
            }
            (out, Vec::new())
        }
        Err(e) => (format!("执行失败：{e}"), Vec::new()),
    }
}

// ────────────────────────────────────────────────────────────────────
// 测试：BotConfig 序列化 + extract_document 输出格式化
// ────────────────────────────────────────────────────────────────────

/// BotConfig 序列化与默认值单测
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
        let mut cfg = BotConfig::default();
        cfg.api_key = Some("sk-llm-plain".into());
        cfg.tavily_key = Some("tvly-plain".into());
        cfg.brave_key = Some("bsa-plain".into());
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
mod tool_extract_document_tests {
    use super::*;

    #[test]
    fn format_extract_output_short_text_returns_full_text_no_truncation_suffix() {
        let text = "短文本".repeat(100); // 100 个汉字 = 300 chars，远低于默认 limit
        let out = format_extract_output("/tmp/sample.md", &text, 0, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains(&text),
            "短文本应原样保留：\n--out--\n{out}\n--text--\n{text}"
        );
        assert!(
            !out.contains("已截断"),
            "短文本不应出现截断提示，实际输出：\n{out}"
        );
        assert!(
            out.starts_with("[文档路径] /tmp/sample.md\n[位置] 0–300 / 共 300 字符\n[文档内容]\n"),
            "头部应带位置行，实际：\n{out}"
        );
    }

    #[test]
    fn format_extract_output_long_text_truncates_with_suffix() {
        // 35000 个 'A'，超默认 30000
        let text = "A".repeat(35000);
        let out = format_extract_output("/tmp/big.md", &text, 0, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains("已截断"),
            "长文本必须出现截断提示，实际输出末尾：\n{}",
            &out[out.len().saturating_sub(200)..]
        );
        // 输出含有的 'A' 数量应 == 30000（截断后）
        let a_count = out.matches('A').count();
        assert_eq!(
            a_count, 30000,
            "长文本截断后应剩 30000 个 'A'，实际 {a_count}"
        );
        // 续读提示给出下一页起点
        assert!(
            out.contains("offset=30000"),
            "截断提示应给续读 offset：\n{out}"
        );
    }

    #[test]
    fn format_extract_output_offset_reads_next_page() {
        let text = "A".repeat(35000);
        let out = format_extract_output("/tmp/big.md", &text, 30000, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains("[位置] 30000–35000 / 共 35000 字符"),
            "第二页位置行：\n{out}"
        );
        assert_eq!(out.matches('A').count(), 5000, "第二页应只有剩余 5000 字符");
        assert!(!out.contains("已截断"), "读完最后一页不应再有截断提示");
    }

    #[test]
    fn format_extract_output_offset_beyond_total() {
        let text = "短";
        let out = format_extract_output("/tmp/a.md", text, 100, EXTRACT_DEFAULT_LIMIT);
        assert!(
            out.contains("超出文档总长"),
            "offset 越界应明确提示：\n{out}"
        );
    }
}
/// 早退路径审计事件序列单测（tool.call 配平 tool.return）。
/// execute_tool 是 Wry AppHandle 签名，无法 mock runtime 直调（见 tests/skill_e2e.rs 注释），
/// 故事件序列抽为纯函数 early_return_events，这里验证事件名/顺序/reason kv。
#[cfg(test)]
mod early_return_events_tests {
    use super::*;

    fn event_names(
        evs: &[(
            crate::audit::AuditLevel,
            &'static str,
            Vec<(&'static str, String)>,
        )],
    ) -> Vec<&'static str> {
        evs.iter().map(|(_, e, _)| *e).collect()
    }

    fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn deny_path_emits_deny_then_balanced_tool_return() {
        // 拦截路径：pre_execute.deny → tool.return（顺序敏感），含 reason=denied / exit_code=none / duration_ms
        let evs = early_return_events("create_word_revisions", "denied", 3, None);
        assert_eq!(event_names(&evs), ["pre_execute.deny", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "tool"), Some("create_word_revisions"));
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "tool"), Some("create_word_revisions"));
        assert_eq!(kv_get(ret, "reason"), Some("denied"));
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert_eq!(kv_get(ret, "duration_ms"), Some("3"));
    }

    #[test]
    fn skill_step_error_path_warns_then_balanced_tool_return() {
        // 熔断路径：skill_on_step_error（补 Warn 可见性）→ tool.return(reason=skill_step_failed)
        let evs = early_return_events(
            "run_python",
            "skill_step_failed",
            5,
            Some("超过最大步数上限（8 步）"),
        );
        assert_eq!(event_names(&evs), ["skill_on_step_error", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "err"), Some("超过最大步数上限（8 步）"));
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "reason"), Some("skill_step_failed"));
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert!(kv_get(ret, "duration_ms").is_some());
    }
}

#[cfg(test)]
mod task_files_arg_tests {
    use super::*;

    /// files 参数解析：去重保序、空白丢弃、isDir 读取、超 10 截断标记
    #[test]
    fn parse_task_files_arg_dedup_cap() {
        // 正常解析 + isDir
        let v = serde_json::json!({"files": [
            {"path": "/a/1.pdf", "isDir": false},
            {"path": "/a/dir", "isDir": true},
        ]});
        let (files, truncated) = parse_task_files_arg(&v).unwrap();
        assert_eq!(files.len(), 2);
        assert!(!files[0].is_dir && files[1].is_dir);
        assert!(!truncated);

        // 去重保序 + 空路径丢弃
        let v = serde_json::json!({"files": [
            {"path": "/a/1.pdf"}, {"path": "/a/1.pdf"}, {"path": "  "}, {"path": "/a/2.pdf"},
        ]});
        let (files, _) = parse_task_files_arg(&v).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "/a/1.pdf");
        assert_eq!(files[1].path, "/a/2.pdf");

        // 超上限：12 → 10 且 truncated=true
        let many: Vec<serde_json::Value> = (0..12)
            .map(|i| serde_json::json!({"path": format!("/f/{i}.txt"), "isDir": false}))
            .collect();
        let v = serde_json::json!({"files": many});
        let (files, truncated) = parse_task_files_arg(&v).unwrap();
        assert_eq!(files.len(), crate::db::MAX_TASK_FILES, "超 10 截断");
        assert_eq!(files[9].path, "/f/9.txt", "保序截前 10");
        assert!(truncated, "超上限必须标记 truncated");

        // 无 files 字段 → None（不动绑定）；空数组 → Some(空)（清除绑定）
        assert!(parse_task_files_arg(&serde_json::json!({"title": "x"})).is_none());
        let (files, truncated) = parse_task_files_arg(&serde_json::json!({"files": []})).unwrap();
        assert!(files.is_empty() && !truncated);
    }

    /// 模型来源 files 仅放行 AI_Gen_Files 内已存在文件，
    /// 目录绑定一律丢（目录授权只能来自用户手选 bind_file）
    #[test]
    fn sanitize_task_files_drops_dirs_and_outside_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let gen = tmp.path().join("AI_Gen_Files");
        std::fs::create_dir_all(&gen).unwrap();
        let inside = gen.join("report.docx");
        std::fs::write(&inside, b"x").unwrap();
        let outside = tmp.path().join("secret.txt");
        std::fs::write(&outside, b"s").unwrap();
        let gen_canon = std::fs::canonicalize(&gen).unwrap();

        let files = vec![
            crate::db::TaskFile {
                path: inside.to_string_lossy().to_string(),
                is_dir: false,
            },
            crate::db::TaskFile {
                path: outside.to_string_lossy().to_string(),
                is_dir: false,
            },
            // 目录绑定（哪怕是 gen 目录本身）一律丢
            crate::db::TaskFile {
                path: gen.to_string_lossy().to_string(),
                is_dir: true,
            },
            // 不存在的路径也丢
            crate::db::TaskFile {
                path: gen.join("nope.txt").to_string_lossy().to_string(),
                is_dir: false,
            },
        ];
        let (kept, dropped) = sanitize_task_files_in(Some(&gen_canon), files);
        assert_eq!(kept.len(), 1, "只有 AI_Gen_Files 内已存在文件保留");
        assert!(kept[0].path.ends_with("report.docx"));
        assert_eq!(dropped, 3);

        // gen 目录不可用（None）→ 全丢（fail-closed）
        let (kept, dropped) = sanitize_task_files_in(
            None,
            vec![crate::db::TaskFile {
                path: inside.to_string_lossy().to_string(),
                is_dir: false,
            }],
        );
        assert!(kept.is_empty() && dropped == 1);
    }

    /// 双写：files 首条同步进旧 file_path/file_is_dir；空列表清三字段
    #[test]
    fn apply_files_to_task_dual_writes_legacy_fields() {
        let mut t = crate::db::Task {
            id: "t".into(),
            title: "x".into(),
            due: None,
            note: None,
            tags: None,
            files: None,
            file_path: Some("/old.txt".into()),
            file_is_dir: Some(false),
            column: "todo".into(),
            subtasks: None,
            completed_at: None,
            archived: None,
            deleted_at: None,
            collapsed: None,
            order: None,
            updated_at: None,
            schedule: None,
            sched_last: None,
            bot_assigned: None,
            expected_updated_at: None,
        };
        apply_files_to_task(
            &mut t,
            vec![
                crate::db::TaskFile {
                    path: "/n/1.pdf".into(),
                    is_dir: false,
                },
                crate::db::TaskFile {
                    path: "/n/dir".into(),
                    is_dir: true,
                },
            ],
        );
        assert_eq!(t.file_path.as_deref(), Some("/n/1.pdf"), "旧字段=首条");
        assert_eq!(t.file_is_dir, Some(false));
        assert_eq!(t.files.as_deref().unwrap().len(), 2);

        apply_files_to_task(&mut t, vec![]);
        assert!(t.files.is_none() && t.file_path.is_none() && t.file_is_dir.is_none());
    }

    /// tool.return 审计 kv：files_count=原始条数（截断前）、truncated 标记；无 files 不补 kv
    #[test]
    fn files_audit_kv_counts_raw_and_flags_truncation() {
        let args = r#"{"files":[{"path":"/a"},{"path":"/b"}]}"#;
        assert_eq!(files_audit_kv(args), Some((2, false)));
        let many: Vec<String> = (0..11)
            .map(|i| format!("{{\"path\":\"/f/{i}\"}}"))
            .collect();
        let args = format!("{{\"files\":[{}]}}", many.join(","));
        assert_eq!(files_audit_kv(&args), Some((11, true)), "超 10 → truncated");
        assert_eq!(files_audit_kv(r#"{"title":"x"}"#), None);
    }
}

#[cfg(test)]
mod phase4_facts_tests {
    use super::*;

    #[test]
    fn get_current_time_format_has_date_and_weekday() {
        let (text, refs) = tool_get_current_time();
        assert!(refs.is_empty());
        assert!(text.starts_with("现在："), "实际：{text}");
        assert!(
            [
                "星期一",
                "星期二",
                "星期三",
                "星期四",
                "星期五",
                "星期六",
                "星期日"
            ]
            .iter()
            .any(|w| text.contains(w)),
            "应含中文星期，实际：{text}"
        );
    }
}

#[cfg(test)]
mod background_dialog_tests {
    /// 回归锁：后台执行（interactive=false）不得弹系统文件选择框——
    /// tool_bind_file 已下线（bind_files 复数形是前端走的），现仅 extract_document
    /// 无 path 后台必须拒绝。源码锁防回退。
    #[test]
    fn background_execution_never_pops_file_dialog() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot.rs")).unwrap();
        assert!(
            !text.contains(concat!("tool_bind", "_file(")),
            "tool_bind_file 死函数不得回退（前端走 bind_files 复数形）"
        );
        assert!(
            text.contains("后台执行不能弹窗选文件，请提供 path 参数"),
            "extract_document 无 path 后台必须拒绝"
        );
    }
}

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
