//! 内置机器人：大模型聊天 + WMessage 任务管理工具调用。
//!
//! 阶段 1 拆分（2026-09-13）：原 bot.rs 3444 行 → `bot/{mod.rs, config.rs, dispatch.rs, tools.rs}`。
//! 本文件作为 facade，仅持有：
//! - 顶部 6 个跨模块 re-export（BotChatResult / TaskRef / model_loop / StopGuard / AuditLevel / db::*）
//! - `pub mod config;` / `pub mod dispatch;` / `pub mod tools;` 子模块声明
//! - 各子模块的 `pub use` / `pub(crate) use` re-export 块，保持 `crate::bot::<item>` 路径 1:1 不变
//!
//! 兄弟模块：
//! - `bot_chat`        — 入口编排（bot_chat / bot_compact / bot_execute_task）
//! - `bot_model_loop`  — 流式 SSE + 工具循环（run_model_loop / parse_sse_chunk / feed_think / TOOLS）
//! - `bot_scheduler`   — ⏰ 定时任务卡自动执行（start_scheduler / occurrence_after / sched_tests）
//! - `bot_slash`       — 旁路基础设施（bot_stop / 确认弹窗 / 机器人开关）
//! - `bot_artifacts`   — 产物登记表（D4d：bot 流程结束按 TaskExecOrigin 分流触发汇总弹窗）
//!
//! 安全性（对齐《Harness 安全网关》需求）：
//! - 工具白名单：固定 TOOLS schema（单一来源 bot/registry.rs 的 TOOLS_TABLE 派生）
//!   + execute_tool 查表分发（bot/dispatch.rs），模型编造的工具一律拒绝
//! - 调用熔断：单轮 Function 调用 ≤50 次 + 35 次软警告；默认对话轮数 50（聊天/任务执行/逐步执行统一）
//! - 参数校验：标题/备注/关键词/子任务/截止时间长度上限、标签数量上限（bot/config.rs）
//! - 审计日志：bot/config.rs 的 audit_log
//! - API Key 存系统凭据存储（keyring，bot/config.rs）；Linux 无 secret-service 时降级明文 + WARN

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

// ──────────────────── 子模块声明 ────────────────────
pub mod config;
pub mod dispatch;
pub mod registry;
pub mod tools;

// ──────────────────── config re-export ────────────────────
pub use config::{
    __cmd__bot_clear_api_key,
    __cmd__bot_get_config,
    __cmd__bot_log_read,
    __cmd__bot_set_config,
    __tauri_command_name_bot_clear_api_key,
    __tauri_command_name_bot_get_config,
    __tauri_command_name_bot_log_read,
    __tauri_command_name_bot_set_config,
    // ── 公开函数（15 个普通 pub）──
    audit_log,
    audit_log_hook,
    bot_clear_api_key,
    // ── 4 个 Tauri command：函数本体 + #[tauri::command] 宏生成物 ──
    bot_get_config,
    bot_log_read,
    bot_set_config,
    check_len,
    config_path,
    has_api_key,
    has_search_key,
    migrate_bot_config_schema,
    migrate_legacy_key,
    migrate_search_keys,
    perm_mode,
    read_api_key,
    read_bypass_llm_switch,
    read_search_key,
    resolve_max_tokens,
    write_search_key,

    // ── 核心类型（5 struct + 3 enum）──
    ActiveModelId,
    ApiProvider,
    BotConfig,
    BotConfigView,
    KeySlot,
    ModelEntry,
    ModelsByProvider,
    PermMode,

    // ── 常量（12 个）──
    DEFAULT_MAX_TOKENS,
    KEYRING_SERVICE,
    KEYRING_USER,
    MAX_DUE,
    MAX_KEYWORD,
    MAX_MAX_TOKENS,
    MAX_NOTE,
    MAX_SUBTASK_TEXT,
    MAX_TAGS,
    MAX_TAG_LEN,
    MAX_TITLE,
    MIN_MAX_TOKENS,
};

// ── config pub(crate) 项：必须用 pub(crate) use 才能保留原可见性 ──
// base_url_is_safe 不移出：它只在 config.rs 内部（SSRF 校验的 base_url 检查）用，
// 移出来 crate 内无人引用，编译器报 unused import。
pub(crate) use config::{
    add_allowed_dir, escape_for_log, load_config, truncate_for_log, update_config_file,
};

// ──────────────────── dispatch re-export ────────────────────
pub(crate) use dispatch::parse_args;
pub use dispatch::{execute_tool, execute_tool_with_stop};

// ──────────────────── tools re-export ────────────────────
pub use tools::{apply_files_to_task, broadcast_after_mutation};
