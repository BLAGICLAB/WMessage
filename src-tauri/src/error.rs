//! 结构化 Command 错误类型
//!
//! 设计目标：
//! - 全量替换 `Result<T, String>` 为 `Result<T, CommandError>`，便于前端按 code 分流
//! - 每个错误有稳定 `code`（机器读）+ 可翻译 `message`（人读）+ `recoverable`（是否可重试）
//! - 通过 `Serialize` 实现序列化：前端 invoke 拒绝时拿到 `{ code, message, recoverable }`
//! - 不引入 thiserror 依赖（项目 Cargo.toml 无），用纯 enum + manual Display
//!
//! 使用：
//! ```rust,ignore
//! use crate::error::{CommandError, CommandResult};
//!
//! #[tauri::command]
//! pub async fn my_cmd(...) -> CommandResult<MyType> {
//!     do_something().map_err(|e| CommandError::DbError(e.to_string()))?;
//!     ...
//! }
//! ```

use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

/// 机器读错误码（CommandError::code() 的返回类型）。
///
/// 为什么是枚举而不是 `&str`：
/// - **编译期可查**：拼错的 code 编译不过，字符串比较的沉默漂移消失；
/// - **单一真相 + 协议一致性**：`#[serde(rename = "…")]` 就是线协议字符串，
///   `as_str()` 与之由单测锁死（`as_str_matches_serde_rename_for_all_codes`）；
/// - **前端 TS 可对齐**：`src/lib/errorHandler.ts` 的 `CommandErrorCode` 联合类型与
///   `ALL_COMMAND_ERROR_CODES` 由 `tests-audit/audit_error_codes.py` 在提交门禁上核对。
///
/// 增删 code 的动作固定为三步：加变体 + 加 `ALL` 项 + 同步前端联合类型与 `hintForCode`；
/// 漏任何一步都有测试/门禁拦下（Rust 侧 `all_codes_listed_and_unique`、
/// 前端 `hintForCode 全覆盖`、跨语言 parity 脚本）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum CommandErrorCode {
    #[serde(rename = "BOT_DISABLED")]
    BotDisabled,
    #[serde(rename = "API_KEY_MISSING")]
    ApiKeyMissing,
    #[serde(rename = "KEYRING_ERROR")]
    KeyringError,
    #[serde(rename = "HTTP_START_FAILED")]
    HttpStartFailed,
    #[serde(rename = "PORT_IN_USE")]
    PortInUse,
    #[serde(rename = "AUTH_MISSING")]
    AuthMissing,
    #[serde(rename = "AUTH_INVALID")]
    AuthInvalid,
    #[serde(rename = "PAYLOAD_TOO_LARGE")]
    PayloadTooLarge,
    #[serde(rename = "TASK_NOT_FOUND")]
    TaskNotFound,
    #[serde(rename = "TASK_INVALID_STATE")]
    TaskInvalidState,
    #[serde(rename = "INVALID_ARGUMENT")]
    InvalidArgument,
    #[serde(rename = "INVALID_WORKSPACE_LINK_KIND")]
    InvalidWorkspaceLinkKind,
    #[serde(rename = "DB_ERROR")]
    DbError,
    #[serde(rename = "IO_ERROR")]
    IoError,
    #[serde(rename = "UNKNOWN_TOOL")]
    UnknownTool,
    #[serde(rename = "ATOMIC_TOOL_BLOCKED")]
    AtomicToolBlocked,
    #[serde(rename = "SKILL_LOAD_FAILED")]
    SkillLoadFailed,
    #[serde(rename = "SKILL_NOT_INSTALLED")]
    SkillNotInstalled,
    #[serde(rename = "LLM_REQUEST_FAILED")]
    LlmRequestFailed,
    #[serde(rename = "LLM_API_ERROR")]
    LlmApiError,
    #[serde(rename = "CONFIRM_TIMEOUT")]
    ConfirmTimeout,
    #[serde(rename = "CONFIRM_REJECTED")]
    ConfirmRejected,
    #[serde(rename = "DOMAIN_RULE")]
    DomainRule,
    #[serde(rename = "INTERNAL")]
    Internal,
}

impl CommandErrorCode {
    /// 全部 code（顺序与本枚举声明一致；审计统计 / 前端 parity 脚本的真相源）
    pub const ALL: &'static [CommandErrorCode] = &[
        Self::BotDisabled,
        Self::ApiKeyMissing,
        Self::KeyringError,
        Self::HttpStartFailed,
        Self::PortInUse,
        Self::AuthMissing,
        Self::AuthInvalid,
        Self::PayloadTooLarge,
        Self::TaskNotFound,
        Self::TaskInvalidState,
        Self::InvalidArgument,
        Self::InvalidWorkspaceLinkKind,
        Self::DbError,
        Self::IoError,
        Self::UnknownTool,
        Self::AtomicToolBlocked,
        Self::SkillLoadFailed,
        Self::SkillNotInstalled,
        Self::LlmRequestFailed,
        Self::LlmApiError,
        Self::ConfirmTimeout,
        Self::ConfirmRejected,
        Self::DomainRule,
        Self::Internal,
    ];

    /// 线协议字符串（写日志 / 前端提示分流用；与 `#[serde(rename)]` 单测锁死）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BotDisabled => "BOT_DISABLED",
            Self::ApiKeyMissing => "API_KEY_MISSING",
            Self::KeyringError => "KEYRING_ERROR",
            Self::HttpStartFailed => "HTTP_START_FAILED",
            Self::PortInUse => "PORT_IN_USE",
            Self::AuthMissing => "AUTH_MISSING",
            Self::AuthInvalid => "AUTH_INVALID",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::TaskNotFound => "TASK_NOT_FOUND",
            Self::TaskInvalidState => "TASK_INVALID_STATE",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::InvalidWorkspaceLinkKind => "INVALID_WORKSPACE_LINK_KIND",
            Self::DbError => "DB_ERROR",
            Self::IoError => "IO_ERROR",
            Self::UnknownTool => "UNKNOWN_TOOL",
            Self::AtomicToolBlocked => "ATOMIC_TOOL_BLOCKED",
            Self::SkillLoadFailed => "SKILL_LOAD_FAILED",
            Self::SkillNotInstalled => "SKILL_NOT_INSTALLED",
            Self::LlmRequestFailed => "LLM_REQUEST_FAILED",
            Self::LlmApiError => "LLM_API_ERROR",
            Self::ConfirmTimeout => "CONFIRM_TIMEOUT",
            Self::ConfirmRejected => "CONFIRM_REJECTED",
            Self::DomainRule => "DOMAIN_RULE",
            Self::Internal => "INTERNAL",
        }
    }
}

impl std::fmt::Display for CommandErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 命令错误统一枚举。每个变体对应一个稳定 code + 人类可读 message + recoverable 标志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    // 机器人 / 配置
    /// 机器人聊天已关闭
    BotDisabled,
    /// API Key 未配置（设置页未填）
    ApiKeyMissing,
    /// Keychain / 系统凭据存储访问失败
    KeyringError(String),

    // HTTP API
    /// HTTP 服务启动失败（端口/绑定）
    HttpStartFailed { port: u16, reason: String },
    /// 端口已被占用
    PortInUse(u16),
    /// 缺少 Authorization Bearer token
    AuthMissing,
    /// Authorization token 不正确
    AuthInvalid,
    /// 请求体超限（>1MB）
    PayloadTooLarge,

    // 任务 / 数据库
    /// 任务不存在
    TaskNotFound(String),
    /// 业务状态拒绝：任务存在但当前状态不允许该操作
    /// （执行中重复触发 / 已完成 / 已归档），非内部错误，不得降级 INTERNAL
    TaskInvalidState { reason: String },
    /// 参数校验失败
    InvalidArgument {
        field: String,
        value: String,
        reason: String,
    },
    /// 工作区链接 kind 非法（写入侧 fail-closed；白名单 {url,file,folder}）
    InvalidWorkspaceLinkKind {
        /// 实际收到的 kind 值
        kind: String,
        /// 来源命令（workspace_upsert / workspace_import_merge）
        source: String,
        /// 出错的链接目标（定位用）
        link: String,
    },
    /// 数据库错误
    DbError(String),
    /// IO 错误
    IoError(String),

    // 工具 / Skill
    /// 未知工具（模型编造的工具名）
    UnknownTool(String),
    /// 原子工具禁止裸调（仅 Skill 内部可用）
    AtomicToolBlocked(String),
    /// Skill 加载失败
    SkillLoadFailed { name: String, reason: String },
    /// Skill 未安装
    SkillNotInstalled(String),

    // LLM
    /// 请求大模型失败（网络/超时）
    LlmRequestFailed(String),
    /// 大模型 API 错误（HTTP 非 2xx）
    ///
    /// `detail` 是给用户看的人话正文（状态码指引 + 上游自己说的那句），
    /// 构造走 `llm_error_detail()`：上游原始错误体（JSON 信封 / HTML 错误页）
    /// 不直接外泄到气泡。
    LlmApiError { status: u16, detail: String },

    // 确认流
    /// 用户确认请求超时（60s 默认）
    ConfirmTimeout,
    /// 用户拒绝确认
    ConfirmRejected,

    // 域规则
    /// 域规则违反(业务校验 / 状态机 / 前置条件)
    /// 跟 Internal 的区别:recoverable + domain 分类,前端能按 domain switch
    DomainRule { domain: String, reason: String },

    // 兜底
    /// 内部错误（未分类）
    Internal(String),
}

impl CommandError {
    /// 机器读稳定 code（前端 switch / 监控统计用，不应改文案）。
    /// 返回枚举而非 `&str`：调用点编译期可查，拼写漂移不再可能。
    pub fn code(&self) -> CommandErrorCode {
        match self {
            Self::BotDisabled => CommandErrorCode::BotDisabled,
            Self::ApiKeyMissing => CommandErrorCode::ApiKeyMissing,
            Self::KeyringError(_) => CommandErrorCode::KeyringError,
            Self::HttpStartFailed { .. } => CommandErrorCode::HttpStartFailed,
            Self::PortInUse(_) => CommandErrorCode::PortInUse,
            Self::AuthMissing => CommandErrorCode::AuthMissing,
            Self::AuthInvalid => CommandErrorCode::AuthInvalid,
            Self::PayloadTooLarge => CommandErrorCode::PayloadTooLarge,
            Self::TaskNotFound(_) => CommandErrorCode::TaskNotFound,
            Self::TaskInvalidState { .. } => CommandErrorCode::TaskInvalidState,
            Self::InvalidArgument { .. } => CommandErrorCode::InvalidArgument,
            Self::InvalidWorkspaceLinkKind { .. } => CommandErrorCode::InvalidWorkspaceLinkKind,
            Self::DbError(_) => CommandErrorCode::DbError,
            Self::IoError(_) => CommandErrorCode::IoError,
            Self::UnknownTool(_) => CommandErrorCode::UnknownTool,
            Self::AtomicToolBlocked(_) => CommandErrorCode::AtomicToolBlocked,
            Self::SkillLoadFailed { .. } => CommandErrorCode::SkillLoadFailed,
            Self::SkillNotInstalled(_) => CommandErrorCode::SkillNotInstalled,
            Self::LlmRequestFailed(_) => CommandErrorCode::LlmRequestFailed,
            Self::LlmApiError { .. } => CommandErrorCode::LlmApiError,
            Self::ConfirmTimeout => CommandErrorCode::ConfirmTimeout,
            Self::ConfirmRejected => CommandErrorCode::ConfirmRejected,
            Self::DomainRule { .. } => CommandErrorCode::DomainRule,
            Self::Internal(_) => CommandErrorCode::Internal,
        }
    }

    /// 是否可恢复（前端可显示「重试」按钮）
    pub fn is_recoverable(&self) -> bool {
        match self {
            // 用户可去设置开启 / 配置 / 重试
            Self::BotDisabled => true,
            Self::ApiKeyMissing => true,
            Self::KeyringError(_) => false,
            Self::HttpStartFailed { .. } => true,
            Self::PortInUse(_) => false, // 端口冲突需改配置
            Self::AuthMissing => true,
            Self::AuthInvalid => true,
            Self::PayloadTooLarge => true,
            Self::TaskNotFound(_) => false,
            // 业务状态拒绝：用户可修正状态后重试（等执行完 / 取消完成 / 恢复归档）
            Self::TaskInvalidState { .. } => true,
            Self::InvalidArgument { .. } => true,
            // 用户可修正 kind 后重试
            Self::InvalidWorkspaceLinkKind { .. } => true,
            Self::DbError(_) => false,
            Self::IoError(_) => false,
            Self::UnknownTool(_) => false,
            Self::AtomicToolBlocked(_) => false,
            Self::SkillLoadFailed { .. } => true,
            Self::SkillNotInstalled(_) => true,
            Self::LlmRequestFailed(_) => true,
            Self::LlmApiError { .. } => true,
            Self::ConfirmTimeout => true,
            Self::ConfirmRejected => true,
            Self::DomainRule { .. } => true,
            Self::Internal(_) => false,
        }
    }

    /// 把补充说明接到用户可见文案的末尾（技能回滚建议等）。
    ///
    /// 错误本体是在「要不要重发本轮」判定**之前**造出来的，而回滚建议只有
    /// 确定不重发了才有——故留一个后补的口子，判定完了再接上。
    ///
    /// 只接在带自由正文的变体上；文案固定的变体原样返回（往固定文案的尾巴
    /// 上拼东西会破坏它们的语义）。
    pub fn append_detail(self, suffix: &str) -> Self {
        if suffix.is_empty() {
            return self;
        }
        match self {
            Self::LlmApiError { status, detail } => Self::LlmApiError {
                status,
                detail: format!("{detail}{suffix}"),
            },
            Self::LlmRequestFailed(detail) => Self::LlmRequestFailed(format!("{detail}{suffix}")),
            Self::Internal(detail) => Self::Internal(format!("{detail}{suffix}")),
            Self::DbError(detail) => Self::DbError(format!("{detail}{suffix}")),
            Self::IoError(detail) => Self::IoError(format!("{detail}{suffix}")),
            other => other,
        }
    }

    /// 人类可读 message（前端默认 toast 用，可翻译）
    pub fn message(&self) -> String {
        match self {
            Self::BotDisabled => "机器人聊天已关闭：请到设置页「机器人设置」开启".into(),
            Self::ApiKeyMissing => "机器人 API 未配置：请到设置页「机器人设置」填写 API Key".into(),
            Self::KeyringError(s) => format!("系统凭据存储访问失败：{s}"),
            Self::HttpStartFailed { port, reason } => {
                format!("HTTP 服务启动失败（端口 {port}）：{reason}")
            }
            Self::PortInUse(port) => format!("端口 {port} 已被占用"),
            Self::AuthMissing => "缺少 Authorization Bearer token".into(),
            Self::AuthInvalid => "Authorization token 不正确".into(),
            Self::PayloadTooLarge => "请求体超过 1MB 上限".into(),
            Self::TaskNotFound(id) => format!("任务不存在：{id}"),
            Self::TaskInvalidState { reason } => format!("任务状态不允许该操作：{reason}"),
            Self::InvalidArgument {
                field,
                value,
                reason,
            } => {
                format!("参数校验失败：{field}={value}（{reason}）")
            }
            Self::InvalidWorkspaceLinkKind { kind, source, link } => format!(
                "工作区链接类型非法：kind={kind}（来源 {source}，链接 {link}）；仅允许 url/file/folder"
            ),
            Self::DbError(s) => format!("数据库错误：{s}"),
            Self::IoError(s) => format!("文件 IO 错误：{s}"),
            Self::UnknownTool(name) => format!("未知工具：{name}"),
            Self::AtomicToolBlocked(name) => {
                format!("原子工具「{name}」禁止裸调：仅 Skill 内部可用，请通过对应 Skill 调用")
            }
            Self::SkillLoadFailed { name, reason } => {
                format!("Skill 加载失败：{name}（{reason}）")
            }
            Self::SkillNotInstalled(name) => format!("Skill 未安装：{name}"),
            Self::LlmRequestFailed(s) => format!("请求大模型失败：{s}"),
            Self::LlmApiError { status, detail } => {
                // 剥控制字符（保留 \t\n\r；DEL 0x7F 与 C1 区 U+0080-U+009F
                // 同属控制字符一并剥）：上游错误体可能混入脏字节，防 toast/日志串行
                let clean = detail
                    .chars()
                    .filter(|c| {
                        !matches!(
                            c,
                            '\x00'..='\x08'
                                | '\x0B'
                                | '\x0C'
                                | '\x0E'..='\x1F'
                                | '\x7F'
                                | '\u{80}'..='\u{9F}'
                        )
                    })
                    .collect::<String>();
                // status 0 = 上游没给可判定的状态码（流内错误且 type/message 都
                // 认不出）：不编一个「错误 0」给用户看，只给正文
                if *status == 0 {
                    format!("大模型 API 错误：{clean}")
                } else {
                    format!("大模型 API 错误 {status}：{clean}")
                }
            }
            Self::ConfirmTimeout => "确认请求超时（默认 60s）".into(),
            Self::ConfirmRejected => "用户拒绝确认".into(),
            Self::DomainRule { domain, reason } => format!("[{domain}] {reason}"),
            Self::Internal(s) => format!("内部错误：{s}"),
        }
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

// ── LLM API 错误的人话化 ─────────────────────────────────────────────
//
// 上游错误体是给开发者看的：OpenAI/Anthropic 信封、网关自定义形状、
// MiniMax 的 base_resp……直接塞进气泡，用户只看到一坨 JSON。
// 2026-10-24 实机就是这么把 402「账户余额不足」显示成
// `{"type":"error","error":{"type":"insufficient_balance_error",...}}` 的。
//
// 折成两句：①状态码告诉用户下一步做什么 ②上游自己说的那句。

/// 状态码 → 用户下一步该做什么（人话）。
///
/// 只列「状态码本身就能定性」的情形。**400 不在表里**：它可能是上下文超长
/// （调用点有更具体的指引），也可能是参数错（上游原文才说得清）——
/// 一律回退到上游原文，不猜。
pub(crate) fn llm_status_advice(status: u16) -> Option<&'static str> {
    match status {
        401 => Some("API Key 无效或未配置：请到设置页「机器人设置」检查密钥"),
        402 => Some("账户余额或额度不足：请到厂商控制台充值，或在设置页切换其他大模型"),
        403 => Some("该模型无权访问：请检查账号权限，或在设置页换用其他模型"),
        404 => Some("模型或接口不存在（可能已下线）：请到设置页确认当前模型"),
        429 => Some("上游限流：请稍后重试"),
        s if (500..=599).contains(&s) => Some("上游服务异常：请稍后重试"),
        _ => None,
    }
}

/// 取 JSON 对象里第一个能当字符串用的键值。
fn json_pick_str<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()))
}

/// 上游错误体 → 人读的一句（最多 160 字）。
///
/// 取上游自己说的那句话：`error.message`（OpenAI/Anthropic 信封）、
/// `base_resp.status_msg`（MiniMax 原生）、`message` / `msg`（网关自定义形状）。
/// **取不到就回退截断后的原文**——形状没覆盖不等于原文没有价值。
fn upstream_error_detail(body: &str) -> Option<String> {
    const MAX_DETAIL_CHARS: usize = 160;
    /// 只在体量像错误信封时才尝试解析：上游偶尔回整页 HTML，
    /// 解析它白烧 CPU，正文里也取不到东西。
    const MAX_PARSE_BYTES: usize = 8 * 1024;
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = if trimmed.len() <= MAX_PARSE_BYTES {
        serde_json::from_str::<serde_json::Value>(trimmed).ok()
    } else {
        None
    };
    let picked = parsed.as_ref().and_then(|v| {
        v.get("error")
            .and_then(|e| json_pick_str(e, &["message", "msg", "type"]))
            .or_else(|| {
                v.get("base_resp")
                    .and_then(|b| json_pick_str(b, &["status_msg", "message"]))
            })
            .or_else(|| json_pick_str(v, &["message", "msg", "detail"]))
    });
    Some(match picked {
        Some(t) if !t.trim().is_empty() => t.trim().chars().take(MAX_DETAIL_CHARS).collect(),
        // 回退原文：直接按上限截，不先复制整段（上游可能是几百 KB 的错误页）
        _ => trimmed.chars().take(MAX_DETAIL_CHARS).collect(),
    })
}

/// LLM API 错误的人话正文：状态码指引 + 上游自己说的那句。
///
/// `advice`：调用点已知的更具体原因（如上下文超长要给 `/compact` 指引），
/// 给了就盖过状态码的通用指引。
pub fn llm_error_detail(status: u16, body: &str, advice: Option<&str>) -> String {
    let advice = advice
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .or_else(|| llm_status_advice(status));
    match (advice, upstream_error_detail(body)) {
        (Some(a), Some(d)) => format!("{a}（上游：{d}）"),
        (Some(a), None) => a.to_string(),
        (None, Some(d)) => d,
        (None, None) => "上游未给出错误详情".to_string(),
    }
}

/// 序列化到前端：JSON 形如 `{ "code": "AUTH_MISSING", "message": "...", "recoverable": true }`
impl Serialize for CommandError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CommandError", 3)?;
        state.serialize_field("code", &self.code())?;
        state.serialize_field("message", &self.message())?;
        state.serialize_field("recoverable", &self.is_recoverable())?;
        state.end()
    }
}

// 便捷转换（? 操作符支持）

impl From<std::io::Error> for CommandError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e.to_string())
    }
}

impl From<serde_json::Error> for CommandError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(format!("JSON 序列化错误：{e}"))
    }
}

impl From<rusqlite::Error> for CommandError {
    fn from(e: rusqlite::Error) -> Self {
        Self::DbError(e.to_string())
    }
}

/// String → CommandError (Internal)：**这是个陷阱**，新代码不要用。
///
/// 写 `Err(format!("…"))` 或 `?` 一个 `Result<_, String>`，就自动变成
/// `Internal`——而 `Internal` 恒为 `recoverable=false`，前端只提示
/// 「不可自动重试；请反馈日志」。作者不写错也踩：语义性失败（用户改一下
/// 就能重试的那种）被降级成不可恢复，用户连重试入口都没有。
///
/// 故新代码**显式选变体**：`CommandError::Internal(..)` / `DbError(..)` /
/// `LlmApiError{..}` / `InvalidArgument{..}` / `TaskNotFound(..)` …。
/// 确实要保留这个 blanket（存量 78 处 + 内部 `Result<_, String>` 生态），
/// 故不删；但 `scripts/test-fast.sh` 的「CommandError 降级防线」会拦新增的
/// blanket 用法（行内 `audit-ok` 可放行）。背景与迁移口径见
/// `docs/FROM-STRING-GUARD-PROPOSAL-2026-10-26.md`。
impl From<String> for CommandError {
    fn from(s: String) -> Self {
        Self::Internal(s)
    }
}

/// &str → CommandError (Internal)：同样是个陷阱，见上面的 `From<String>`。
///
/// 保留只为 `Err("字面量".into())` 这类存量写法还能编译；新代码显式选变体。
impl From<&str> for CommandError {
    fn from(s: &str) -> Self {
        Self::Internal(s.to_string())
    }
}

/// CommandError → String (有损,仅 tool→model 边界用,丢失变体代码,仅保 message 文本)
/// 让 `?` 在 Result<_, CommandError> → Result<_, String> 处自动转换
impl From<CommandError> for String {
    fn from(e: CommandError) -> Self {
        e.to_string()
    }
}

// keyring::Error / tiny_http::Error 不直接 impl From（避免污染 CommandError 依赖）：
// 调用方在 ? 失败时显式 .map_err(|e| CommandError::KeyringError(e.to_string())) 即可。

/// 常用 Result 别名
pub type CommandResult<T> = std::result::Result<T, CommandError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_stable_enum() {
        assert_eq!(
            CommandError::BotDisabled.code(),
            CommandErrorCode::BotDisabled
        );
        assert_eq!(
            CommandError::ApiKeyMissing.code(),
            CommandErrorCode::ApiKeyMissing
        );
        assert_eq!(
            CommandError::TaskNotFound("x".into()).code(),
            CommandErrorCode::TaskNotFound
        );
        assert_eq!(
            CommandError::SkillLoadFailed {
                name: "minimax-ppt".into(),
                reason: "missing".into()
            }
            .code(),
            CommandErrorCode::SkillLoadFailed
        );
    }

    /// 协议锁：as_str() 与 #[serde(rename = "…")] 必须逐字一致。
    /// 两处字符串重复是刻意的（serde 不能从 as_str 反推），本测钉死不许漂移。
    #[test]
    fn as_str_matches_serde_rename_for_all_codes() {
        for code in CommandErrorCode::ALL {
            let wire = serde_json::to_value(code).unwrap();
            assert_eq!(
                wire,
                serde_json::json!(code.as_str()),
                "code {code:?} 的 serde rename 与 as_str 不一致"
            );
            // Display 与 as_str 同源
            assert_eq!(code.to_string(), code.as_str());
        }
    }

    /// ALL 必须无重复且覆盖枚举每一个变体（漏登记会让前端 parity 脚本静默放过）
    #[test]
    fn all_codes_listed_and_unique() {
        let mut seen: Vec<&'static str> =
            CommandErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        seen.sort_unstable();
        let mut deduped = seen.clone();
        deduped.dedup();
        assert_eq!(seen, deduped, "CommandErrorCode::ALL 存在重复项");

        // 逐变体取 code() 必须都能在 ALL 里找到
        let every: Vec<CommandErrorCode> = vec![
            CommandError::BotDisabled.code(),
            CommandError::ApiKeyMissing.code(),
            CommandError::KeyringError("x".into()).code(),
            CommandError::HttpStartFailed {
                port: 1,
                reason: "x".into(),
            }
            .code(),
            CommandError::PortInUse(1).code(),
            CommandError::AuthMissing.code(),
            CommandError::AuthInvalid.code(),
            CommandError::PayloadTooLarge.code(),
            CommandError::TaskNotFound("x".into()).code(),
            CommandError::TaskInvalidState { reason: "x".into() }.code(),
            CommandError::InvalidArgument {
                field: "f".into(),
                value: "v".into(),
                reason: "r".into(),
            }
            .code(),
            CommandError::InvalidWorkspaceLinkKind {
                kind: "app".into(),
                source: "workspace_upsert".into(),
                link: "/x".into(),
            }
            .code(),
            CommandError::DbError("x".into()).code(),
            CommandError::IoError("x".into()).code(),
            CommandError::UnknownTool("x".into()).code(),
            CommandError::AtomicToolBlocked("x".into()).code(),
            CommandError::SkillLoadFailed {
                name: "n".into(),
                reason: "r".into(),
            }
            .code(),
            CommandError::SkillNotInstalled("x".into()).code(),
            CommandError::LlmRequestFailed("x".into()).code(),
            CommandError::LlmApiError {
                status: 500,
                detail: "x".into(),
            }
            .code(),
            CommandError::ConfirmTimeout.code(),
            CommandError::ConfirmRejected.code(),
            CommandError::DomainRule {
                domain: "d".into(),
                reason: "r".into(),
            }
            .code(),
            CommandError::Internal("x".into()).code(),
        ];
        for code in every {
            assert!(
                CommandErrorCode::ALL.contains(&code),
                "code {code:?} 未登记进 CommandErrorCode::ALL"
            );
        }
    }

    /// 新增 code 的 code()/recoverable/message/序列化契约样例。
    #[test]
    fn invalid_workspace_link_kind_contract() {
        let e = CommandError::InvalidWorkspaceLinkKind {
            kind: "app".into(),
            source: "workspace_upsert".into(),
            link: "/tmp/x.app".into(),
        };
        assert_eq!(e.code(), CommandErrorCode::InvalidWorkspaceLinkKind);
        assert_eq!(e.code().as_str(), "INVALID_WORKSPACE_LINK_KIND");
        assert!(e.is_recoverable(), "用户可修正 kind 后重试");
        let m = e.message();
        assert!(
            m.contains("app") && m.contains("workspace_upsert") && m.contains("/tmp/x.app"),
            "message 必须含 kind/source/link：{m}"
        );
        assert!(m.contains("url/file/folder"), "message 应含白名单提示：{m}");
        let j = serde_json::to_value(&e).unwrap();
        assert_eq!(j["code"], "INVALID_WORKSPACE_LINK_KIND");
        assert_eq!(j["recoverable"], true);
    }

    #[test]
    fn recoverable_classification_correct() {
        // 用户可重试/修正的
        assert!(CommandError::BotDisabled.is_recoverable());
        assert!(CommandError::ApiKeyMissing.is_recoverable());
        assert!(CommandError::AuthMissing.is_recoverable());
        // 用户改不了 / 重试也无效的
        assert!(!CommandError::PortInUse(0).is_recoverable());
        assert!(!CommandError::TaskNotFound("x".into()).is_recoverable());
        assert!(!CommandError::IoError("x".into()).is_recoverable());
    }

    #[test]
    fn message_contains_key_info() {
        assert!(
            CommandError::TaskNotFound("abc123".into())
                .message()
                .contains("abc123"),
            "TaskNotFound message 应含 id"
        );
        assert!(
            CommandError::PortInUse(4763).message().contains("4763"),
            "PortInUse message 应含端口"
        );
        assert!(
            CommandError::AuthMissing
                .message()
                .to_lowercase()
                .contains("authorization"),
            "AuthMissing message 应含 Authorization"
        );
    }

    #[test]
    fn display_eq_message() {
        let err = CommandError::BotDisabled;
        assert_eq!(format!("{err}"), err.message());
    }

    #[test]
    fn llm_api_error_message_strips_control_chars_keeps_tab_newline_cr() {
        // 剥离契约与注释一致：C0 控制字符（除 \t\n\r）+ DEL(0x7F) +
        // C1 区 U+0080-U+009F 全剥；\t\n\r 保留（audit 侧 escape_for_log 再转义）
        let e = CommandError::LlmApiError {
            status: 500,
            detail: "a\u{0}b\u{7F}c\td\ne\rf\u{1}g\u{9F}h".into(),
        };
        assert_eq!(e.message(), "大模型 API 错误 500：abc\td\ne\rfgh");
        let m = e.message();
        assert!(!m.contains('\u{7F}'), "DEL 必须被剥离：{m:?}");
        assert!(!m.contains('\u{9F}'), "C1 控制字符必须被剥离：{m:?}");
    }

    // ── LLM API 错误的人话化 ──

    #[test]
    fn llm_error_detail_402_says_balance_instead_of_raw_json() {
        // 2026-10-24 实机：上游 402 原样甩进气泡，用户只看到一坨 JSON
        let body = r#"{"type":"error","error":{"type":"insufficient_balance_error","message":"insufficient balance (1008)"},"request_id":"072bb75eb78196241b5ab6e12744a166"}"#;
        let d = llm_error_detail(402, body, None);
        assert!(d.contains("余额"), "应说清是余额问题：{d}");
        assert!(d.contains("充值"), "应给出下一步（充值/换模型）：{d}");
        assert!(
            d.contains("insufficient balance (1008)"),
            "上游自己说的那句要留着：{d}"
        );
        assert!(!d.contains("{\"type\""), "上游原始 JSON 不该外泄：{d}");
        assert!(!d.contains("request_id"), "上游原始 JSON 不该外泄：{d}");
    }

    #[test]
    fn llm_status_advice_names_the_next_step_for_known_codes() {
        for (status, needle) in [
            (401u16, "API Key"),
            (402, "余额"),
            (403, "权限"),
            (404, "已下线"),
            (429, "限流"),
            (500, "服务异常"),
            (503, "服务异常"),
        ] {
            let d = llm_error_detail(status, "", None);
            assert!(d.contains(needle), "{status} 应含「{needle}」：{d}");
        }
        // 400 不猜：可能是超长也可能是参数错，回退上游原文
        assert!(llm_status_advice(400).is_none());
        assert!(llm_status_advice(200).is_none());
    }

    #[test]
    fn llm_error_detail_advice_override_wins_over_status_advice() {
        let d = llm_error_detail(
            400,
            r#"{"error":{"message":"prompt is too long"}}"#,
            Some("上下文超长：请 /compact 压缩历史"),
        );
        assert!(d.contains("/compact"), "{d}");
        assert!(
            !d.contains("API Key"),
            "调用点给了指引就不该再套状态码指引：{d}"
        );
    }

    #[test]
    fn llm_error_detail_400_without_advice_falls_back_to_upstream_text() {
        let d = llm_error_detail(
            400,
            r#"{"error":{"message":"tools[0].function.parameters: invalid schema"}}"#,
            None,
        );
        assert!(d.contains("invalid schema"), "{d}");
        assert!(!d.contains("上游："), "没有指引时不套「上游：」前缀：{d}");
    }

    #[test]
    fn llm_error_detail_reads_minimax_native_base_resp() {
        let d = llm_error_detail(
            402,
            r#"{"base_resp":{"status_code":1008,"status_msg":"insufficient balance"}}"#,
            None,
        );
        assert!(d.contains("insufficient balance"), "{d}");
    }

    #[test]
    fn llm_error_detail_non_json_body_keeps_truncated_text() {
        let d = llm_error_detail(502, "<html><body>502 Bad Gateway</body></html>", None);
        assert!(d.contains("502 Bad Gateway"), "{d}");
        let long = llm_error_detail(500, &"x".repeat(5000), None);
        assert!(
            long.chars().filter(|c| *c == 'x').count() <= 160,
            "上游细节要有上限：{}",
            long.chars().count()
        );
    }

    #[test]
    fn llm_error_detail_huge_body_is_truncated_not_parsed() {
        // 上游回一整页 HTML：超体量门槛不解析，直接截断原文
        let big = format!("<html>{}</html>", "x".repeat(50_000));
        let d = llm_error_detail(500, &big, None);
        assert!(
            d.chars().filter(|c| *c == 'x').count() <= 160,
            "上游细节要有上限：{}",
            d.chars().count()
        );
    }

    #[test]
    fn llm_error_detail_empty_body_keeps_advice_only() {
        let d = llm_error_detail(429, "   ", None);
        assert!(d.contains("限流"), "{d}");
        assert!(
            !d.contains("上游："),
            "没有上游细节就不该出现「上游：」：{d}"
        );
    }

    #[test]
    fn serialize_to_three_fields() {
        let err = CommandError::BotDisabled;
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"code\":\"BOT_DISABLED\""));
        assert!(json.contains("\"message\":"));
        assert!(json.contains("\"recoverable\":true"));
        assert!(
            !json.contains("\"platform\""),
            "platform 字段已移除：{json}"
        );
    }

    #[test]
    fn serialize_recoverable_false_for_io() {
        let err = CommandError::IoError("disk full".into());
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"recoverable\":false"));
    }

    // ── 业务状态拒绝专用变体，不降级 INTERNAL ──

    #[test]
    fn task_invalid_state_is_recoverable_business_rejection() {
        // mock 一个 invalid state 场景（执行中重复触发 / 已完成 / 已归档共用此变体）
        let err = CommandError::TaskInvalidState {
            reason: "该任务卡正在执行中，请等待完成后再触发".into(),
        };
        assert_eq!(err.code(), CommandErrorCode::TaskInvalidState);
        assert!(err.is_recoverable(), "业务状态拒绝应可由用户修正后重试");
        assert!(err.message().contains("执行中"));
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"code\":\"TASK_INVALID_STATE\""));
        assert!(json.contains("\"recoverable\":true"));
    }

    #[test]
    fn from_io_error_maps_correctly() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let cmd_err: CommandError = io_err.into();
        assert_eq!(cmd_err.code(), CommandErrorCode::IoError);
        assert!(cmd_err.message().contains("file missing"));
    }

    #[test]
    fn keyring_error_constructs_via_map_err() {
        // keyring::Error 不直接 impl From（避免 CommandError 依赖 keyring crate）：
        // 调用方需显式 .map_err。验证构造出的 CommandError code 正确。
        let kr_err = keyring::Error::NoEntry;
        let result: Result<(), CommandError> =
            Err(kr_err).map_err(|e| CommandError::KeyringError(e.to_string()));
        let cmd_err = result.unwrap_err();
        assert_eq!(cmd_err.code(), CommandErrorCode::KeyringError);
        assert!(!cmd_err.message().is_empty(), "message 应非空");
    }

    // ── DomainRule 变体专项：
    // 覆盖 4 项直接断言 + 1 条同 code 不同 reason 序列化稳定 ──

    #[test]
    fn domain_rule_code_is_stable() {
        let err = CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: "技能已暂停".to_string(),
        };
        assert_eq!(err.code(), CommandErrorCode::DomainRule);
    }

    #[test]
    fn domain_rule_is_recoverable_always_true() {
        // 关键 bug 修复：原 Internal 标 false 让前端拿不到「重试」按钮
        // 这 19 个错全是用户可重试的(技能暂停→等确认/URL 内网→换 URL/迁移中→等/...)
        // 全部走 DomainRule,必须 recoverable=true
        for domain in [
            "argument",
            "task",
            "skill",
            "platform",
            "clipboard",
            "python",
            "migration",
            "search",
            "web",
            "csv",
        ] {
            let err = CommandError::DomainRule {
                domain: domain.to_string(),
                reason: format!("{domain} 失败"),
            };
            assert!(
                err.is_recoverable(),
                "DomainRule domain={domain} 必须 recoverable=true,否则前端不显示重试按钮"
            );
        }
    }

    #[test]
    fn domain_rule_message_format_includes_bracket_domain() {
        // message 格式: "[{domain}] {reason}" —— 前端能直接定位是哪一类失败
        let err = CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: "技能已暂停，等待用户确认".to_string(),
        };
        let msg = err.message();
        assert!(
            msg.starts_with("[skill] "),
            "message 应以 [domain] 起头,实际:{msg}"
        );
        assert!(msg.contains("技能已暂停"), "message 应含 reason");
        // Display 与 message 一致
        assert_eq!(format!("{err}"), msg);
    }

    #[test]
    fn domain_rule_serialization_contains_code_message_recoverable() {
        // 序列化走标准 3 字段:code / message / recoverable
        // recoverable 必须 true(前端据此显示「重试」按钮)
        let err = CommandError::DomainRule {
            domain: "search".to_string(),
            reason: "Bing 没有返回结果".to_string(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(
            json.contains("\"code\":\"DOMAIN_RULE\""),
            "json 缺 code: {json}"
        );
        assert!(
            json.contains("\"message\":\"[search] Bing 没有返回结果\""),
            "json 缺 message: {json}"
        );
        assert!(
            json.contains("\"recoverable\":true"),
            "recoverable 必须是 true: {json}"
        );
    }

    #[test]
    fn domain_rule_same_code_different_reason_serialization_stable() {
        // 同 code 不同 reason 序列化必须稳定(message 字段区分,code 不变)
        // —— 监控/日志/未来 metrics 按 domain 聚合不会因 reason 不同打散
        let err1 = CommandError::DomainRule {
            domain: "web".to_string(),
            reason: "网址缺少主机名".to_string(),
        };
        let err2 = CommandError::DomainRule {
            domain: "web".to_string(),
            reason: "已拒绝访问本机/内网地址".to_string(),
        };
        assert_eq!(err1.code(), err2.code());
        assert_ne!(err1.message(), err2.message());
        // code 字段在 JSON 中一致
        let j1 = serde_json::to_string(&err1).unwrap();
        let j2 = serde_json::to_string(&err2).unwrap();
        assert!(j1.contains("\"code\":\"DOMAIN_RULE\""));
        assert!(j2.contains("\"code\":\"DOMAIN_RULE\""));
        assert_ne!(j1, j2, "不同 reason 序列化应不同(message 字段不同)");
    }
}
