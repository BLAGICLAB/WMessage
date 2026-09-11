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

/// 命令错误统一枚举。每个变体对应一个稳定 code + 人类可读 message + recoverable 标志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    // ─────  机器人 / 配置  ─────
    /// 机器人聊天已关闭
    BotDisabled,
    /// API Key 未配置（设置页未填）
    ApiKeyMissing,
    /// Keychain / 系统凭据存储访问失败
    KeyringError(String),

    // ─────  HTTP API  ─────
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

    // ─────  任务 / 数据库  ─────
    /// 任务不存在
    TaskNotFound(String),
    /// 业务状态拒绝：任务存在但当前状态不允许该操作
    /// （执行中重复触发 / 已完成 / 已归档），非内部错误，不得降级 INTERNAL
    TaskInvalidState {
        reason: String,
    },
    /// 参数校验失败
    InvalidArgument {
        field: String,
        value: String,
        reason: String,
    },
    /// 数据库错误
    DbError(String),
    /// IO 错误
    IoError(String),

    // ─────  工具 / Skill  ─────
    /// 未知工具（模型编造的工具名）
    UnknownTool(String),
    /// 原子工具禁止裸调（仅 Skill 内部可用）
    AtomicToolBlocked(String),
    /// Skill 加载失败
    SkillLoadFailed { name: String, reason: String },
    /// Skill 未安装
    SkillNotInstalled(String),

    // ─────  LLM  ─────
    /// 请求大模型失败（网络/超时）
    LlmRequestFailed(String),
    /// 大模型 API 错误（HTTP 非 2xx）
    LlmApiError { status: u16, body_preview: String },

    // ─────  确认流  ─────
    /// 用户确认请求超时（60s 默认）
    ConfirmTimeout,
    /// 用户拒绝确认
    ConfirmRejected,

    // ─────  域规则  ─────
    /// 域规则违反(业务校验 / 状态机 / 前置条件)
    /// 跟 Internal 的区别:recoverable + domain 分类,前端能按 domain switch
    DomainRule { domain: String, reason: String },

    // ─────  兜底  ─────
    /// 内部错误（未分类）
    Internal(String),
}

impl CommandError {
    /// 机器读稳定 code（前端 switch / 监控统计用，不应改文案）
    pub fn code(&self) -> &'static str {
        match self {
            Self::BotDisabled => "BOT_DISABLED",
            Self::ApiKeyMissing => "API_KEY_MISSING",
            Self::KeyringError(_) => "KEYRING_ERROR",
            Self::HttpStartFailed { .. } => "HTTP_START_FAILED",
            Self::PortInUse(_) => "PORT_IN_USE",
            Self::AuthMissing => "AUTH_MISSING",
            Self::AuthInvalid => "AUTH_INVALID",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::TaskNotFound(_) => "TASK_NOT_FOUND",
            Self::TaskInvalidState { .. } => "TASK_INVALID_STATE",
            Self::InvalidArgument { .. } => "INVALID_ARGUMENT",
            Self::DbError(_) => "DB_ERROR",
            Self::IoError(_) => "IO_ERROR",
            Self::UnknownTool(_) => "UNKNOWN_TOOL",
            Self::AtomicToolBlocked(_) => "ATOMIC_TOOL_BLOCKED",
            Self::SkillLoadFailed { .. } => "SKILL_LOAD_FAILED",
            Self::SkillNotInstalled(_) => "SKILL_NOT_INSTALLED",
            Self::LlmRequestFailed(_) => "LLM_REQUEST_FAILED",
            Self::LlmApiError { .. } => "LLM_API_ERROR",
            Self::ConfirmTimeout => "CONFIRM_TIMEOUT",
            Self::ConfirmRejected => "CONFIRM_REJECTED",
            Self::DomainRule { .. } => "DOMAIN_RULE",
            Self::Internal(_) => "INTERNAL",
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

    /// 人类可读 message（前端默认 toast 用，可翻译）
    pub fn message(&self) -> String {
        match self {
            Self::BotDisabled => "机器人聊天已关闭：请到设置页「机器人设置」开启".into(),
            Self::ApiKeyMissing => {
                "机器人 API 未配置：请到设置页「机器人设置」填写 API Key".into()
            }
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
            Self::DbError(s) => format!("数据库错误：{s}"),
            Self::IoError(s) => format!("文件 IO 错误：{s}"),
            Self::UnknownTool(name) => format!("未知工具：{name}"),
            Self::AtomicToolBlocked(name) => format!(
                "原子工具「{name}」禁止裸调：仅 Skill 内部可用，请通过对应 Skill 调用"
            ),
            Self::SkillLoadFailed { name, reason } => {
                format!("Skill 加载失败：{name}（{reason}）")
            }
            Self::SkillNotInstalled(name) => format!("Skill 未安装：{name}"),
            Self::LlmRequestFailed(s) => format!("请求大模型失败：{s}"),
            Self::LlmApiError {
                status,
                body_preview,
            } => {
                format!("大模型 API 错误 {status}：{body_preview}")
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

/// 序列化到前端：JSON 形如 `{ "code": "AUTH_MISSING", "message": "...", "recoverable": true }`
impl Serialize for CommandError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CommandError", 3)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.message())?;
        state.serialize_field("recoverable", &self.is_recoverable())?;
        state.end()
    }
}

// ─────  便捷转换（? 操作符支持）  ─────

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

/// String → CommandError (Internal)，便于从 Result<T, String> 机械迁移到 Result<T, CommandError>：
/// 现有 `Err(format!("xxx"))` （format! 返回 String）经 From<String> 自动转 CommandError::Internal。
/// 后续可逐个改为精确变体（BotDisabled / ApiKeyMissing / TaskNotFound 等）。
impl From<String> for CommandError {
    fn from(s: String) -> Self {
        Self::Internal(s)
    }
}

/// &str → CommandError (Internal)：保留现有 `Err("literal".into())` 语法可继续使用。
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
    fn code_is_stable_string() {
        assert_eq!(CommandError::BotDisabled.code(), "BOT_DISABLED");
        assert_eq!(CommandError::ApiKeyMissing.code(), "API_KEY_MISSING");
        assert_eq!(
            CommandError::TaskNotFound("x".into()).code(),
            "TASK_NOT_FOUND"
        );
        assert_eq!(
            CommandError::SkillLoadFailed {
                name: "minimax-ppt".into(),
                reason: "missing".into()
            }
            .code(),
            "SKILL_LOAD_FAILED"
        );
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
    fn serialize_to_three_fields() {
        let err = CommandError::BotDisabled;
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"code\":\"BOT_DISABLED\""));
        assert!(json.contains("\"message\":"));
        assert!(json.contains("\"recoverable\":true"));
        assert!(!json.contains("\"platform\""), "platform 字段已移除：{json}");
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
        assert_eq!(err.code(), "TASK_INVALID_STATE");
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
        assert_eq!(cmd_err.code(), "IO_ERROR");
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
        assert_eq!(cmd_err.code(), "KEYRING_ERROR");
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
        assert_eq!(err.code(), "DOMAIN_RULE");
    }

    #[test]
    fn domain_rule_is_recoverable_always_true() {
        // 关键 bug 修复：原 Internal 标 false 让前端拿不到「重试」按钮
        // 这 19 个错全是用户可重试的(技能暂停→等确认/URL 内网→换 URL/迁移中→等/...)
        // 全部走 DomainRule,必须 recoverable=true
        for domain in [
            "argument", "task", "skill", "platform", "clipboard",
            "python", "migration", "search", "web", "csv",
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
        assert!(msg.starts_with("[skill] "), "message 应以 [domain] 起头,实际:{msg}");
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
        assert!(json.contains("\"code\":\"DOMAIN_RULE\""), "json 缺 code: {json}");
        assert!(json.contains("\"message\":\"[search] Bing 没有返回结果\""), "json 缺 message: {json}");
        assert!(json.contains("\"recoverable\":true"), "recoverable 必须是 true: {json}");
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
