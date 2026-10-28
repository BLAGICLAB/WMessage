//! 内置机器人：大模型聊天 + WMessage 任务管理工具调用。
//!
//! 原 bot.rs 3444 行 → `bot/{config/, dispatch.rs, registry.rs, tools.rs}`。
//! 本文件是 **bot 族唯一对外门面**，仅持有：
//! - 顶部跨模块 re-export（BotChatResult / TaskRef / model_loop / StopGuard / AuditLevel / db::* 等）
//! - `pub mod config;` / `pub mod dispatch;` / `pub mod tools;` 子模块声明
//! - 各子模块的 `pub use` / `pub(crate) use` re-export 块，保持 `crate::bot::<item>` 路径 1:1 不变
//!
//! 兄弟模块：
//! - `bot_chat`        — 入口编排（bot_chat / bot_compact / bot_execute_task）
//! - `bot_model_loop`  — 流式 SSE + 工具循环（run_model_loop / parse_sse_chunk / feed_think / TOOLS）
//! - `bot_scheduler`   — ⏰ 定时任务卡自动执行（start_scheduler / occurrence_after / sched_tests）
//! - `bot_slash`       — 旁路基础设施（bot_stop / 确认弹窗 / 机器人开关）
//! - `bot_artifacts`   — 产物登记表（bot 流程结束按 TaskExecOrigin 分流触发汇总弹窗）
//!
//! 安全性（对齐《Harness 安全网关》需求）：
//! - 工具白名单：固定 TOOLS schema（单一来源 bot/registry.rs 的 TOOLS_TABLE 派生）
//!   + execute_tool 查表分发（bot/dispatch.rs），模型编造的工具一律拒绝
//! - 调用熔断：单轮 Function 调用 ≤50 次 + 35 次软警告；默认对话轮数 50（聊天/任务执行/逐步执行统一）
//! - 参数校验：标题/备注/关键词/子任务/截止时间长度上限、标签数量上限（bot/config/types.rs）
//! - 审计日志：bot/config/audit.rs 的 audit_log
//! - API Key 存系统凭据存储（keyring，bot/config/keyring.rs）；Linux 无 secret-service 时降级明文 + WARN

//! 保留对外接口 re-export，避免拆分后 bot_skills / tests/llm_integration 等
//! 已存在的调用方（`crate::bot::TaskRef` / `crate::bot::parse_sse_chunk` /
//! `crate::bot::ToolCallDelta` / `crate::bot::BotChatResult`）中断。
//!
//! **跨族引用一律走本门面**：`crate::bot::<item>` 是 bot 族唯一对外路径，
//! `crate::bot_chat::` / `crate::bot_model_loop::` 这类深层路径只供 bot 族
//! 内部互引（族内免迁移），族外一律先在这里 re-export 再引用——
//! 同一条目只留一条合法路径，新代码不必猜该走哪条。
pub use crate::bot_chat::{BotChatResult, ChatMsg, TaskExecOrigin, TaskRef};
// 记忆整理要拿对话摘要来做合并；逐步执行要自己拼上下文、挂执行闸、
// 标记任务归属——chat 侧只对外转出这五个。
pub(crate) use crate::bot_chat::{
    build_task_block, gen_dir_rule, set_bot_assigned, summarize_messages, ExecGuard,
};
pub use crate::bot_model_loop::{
    accumulate_tool_call_delta, drain_sse_lines, noop_replan, parse_sse_chunk, run_model_loop,
    run_model_loop_core, shared_llm_client, LlmHttp, LoopTrace, ModelLoopDeps, ToolCallDelta,
    TurnStatus,
};
// bot_slash 是私有模块，集成测试（tests/llm_integration.rs）驱动
// run_model_loop_core 需要构造停止守卫，此处转出口径唯一公开。
pub use crate::bot_slash::{StopGuard, StopToken};
// 危险操作确认的回执类型——只有跑 bot 流程的模块需要拿到它。
pub(crate) use crate::bot_slash::ConfirmReply;
// bot_fs 模块私有，但集成测试要模拟「工具写盘并带回凭证」——凭证的**存在**
// 是幻觉守卫「这个路径是工具产物吗」那道判据的依据，测试得能造一个。
pub use crate::bot_fs::{resolve_with_perm, FileChangeReceipt};
// 文件读上限、内容哈希与 diff 生成：trace 侧与读盘类工具共用。
pub(crate) use crate::bot_fs::{build_unified_diff, sha256_hex, READ_MAX_BYTES};
// 技能提示段与运行态：逐步执行要拼技能上下文，退出清理要判技能是不是还在跑。
pub use crate::bot_skills::{build_skill_block, SkillRun, SkillState};
// 技能运行态的测试夹具：退出清理用例要造一个「在跑的技能」再验它被终止。
#[cfg(test)]
pub(crate) use crate::bot_skills::{
    test_insert_skill_run, test_remove_skill_run, test_skill_run_state,
};
// 定时任务的三个命令 + 时间解析与表达式校验：调度与通知侧共用。
//
// 命令宏的产物（__cmd__ / __tauri_command_name__）也得跟着转出：generate_handler
// 按「路径所在模块」找这两个隐藏项——命令本体转走而宏产物留在原地，handler 侧
// 就找不到入口了（bot/config 的命令是同一套做法）。
pub use crate::bot_scheduler::{
    __cmd__schedule_overview, __cmd__schedule_set_enabled, __cmd__scheduled_job_fire,
    __tauri_command_name_schedule_overview, __tauri_command_name_schedule_set_enabled,
    __tauri_command_name_scheduled_job_fire, schedule_overview, schedule_set_enabled,
    scheduled_job_fire,
};
pub(crate) use crate::bot_scheduler::{resolve_local, validate_schedule, SCHED_TICK_SECS};
// 产物登记行类型：运行期状态与产物登记都指着它。
pub use crate::bot_artifacts::RegisteredArtifact;
// 退出标记复位：退出清理用例跑完要把 EXITING 复位，否则后续测试被误拒。
#[cfg(test)]
pub use crate::bot_py::reset_exiting_for_test;
// 同上：audit 模块私有，ModelLoopDeps.audit 回调签名里的 AuditLevel 在 tests/
// 不可命名，测试构造 deps 需要它公开。
pub use crate::audit::AuditLevel;
// 同上：db 模块私有，tests/skill_e2e.rs 的调度器 e2e 用真实 upsert/load 验证
// persist_outcome 注入闭包的落库载荷（临时库文件）。
pub use crate::db::{load_all_skill_outcomes, upsert_skill_outcome, PersistedSkillOutcome};

// 子模块声明
pub mod compaction;
pub mod config;
pub mod constants;
pub mod corrections;
pub mod dispatch;
#[cfg(test)]
mod eval;
pub mod format;
pub mod hooks;
pub mod log_sanitize;
pub mod mcp;
pub mod params;
pub mod plan_state;
pub mod procs;
pub mod reasoning;
pub mod registry;
pub mod services;
pub mod stream_log;
pub mod subagent_digest;
pub mod subagent_repair;
pub mod subagent_runner;
pub mod subagent_spawn;
pub mod subagent_tools;
pub mod tags_consolidate;
#[cfg(test)]
pub mod test_files;
pub mod tools;

// params re-export：参数表命令（设置页「Agent 运行参数」卡的数据源）。
// 走 facade 口径——外部只认 `crate::bot::bot_effective_params`，不直接摸
// `crate::bot::params::*`。
pub use params::{
    __cmd__bot_effective_params, __tauri_command_name_bot_effective_params, bot_effective_params,
    effective_params, ParamDef, ParamView, PARAMS_TABLE,
};

// config re-export
pub use config::{
    __cmd__bot_get_config,
    __cmd__bot_log_read,
    __cmd__bot_set_active_model,
    __cmd__bot_set_config,
    __cmd__bot_set_perm_mode,
    __cmd__memory_tuning_get,
    __cmd__memory_tuning_set,
    __tauri_command_name_bot_get_config,
    __tauri_command_name_bot_log_read,
    __tauri_command_name_bot_set_active_model,
    __tauri_command_name_bot_set_config,
    __tauri_command_name_bot_set_perm_mode,
    __tauri_command_name_memory_tuning_get,
    __tauri_command_name_memory_tuning_set,
    // ── 公开函数（16 个普通 pub）──
    active_model_entry,
    archive_after_days,
    audit_log,
    audit_log_hook,
    // ── 5 个 Tauri command：函数本体 + #[tauri::command] 宏生成物 ──
    bot_get_config,
    bot_log_read,
    bot_set_active_model,
    bot_set_config,
    bot_set_perm_mode,
    check_len,
    config_path,
    effective_inference,
    ensure_vendor_verified,
    has_api_key,
    has_search_key,
    memory_tuning_get,
    memory_tuning_set,
    migrate_bot_config_schema,
    migrate_legacy_key,
    migrate_search_keys,
    perm_mode,
    read_api_key,
    read_bypass_llm_switch,
    read_llm_key,
    read_memory_control,
    read_memory_tuning,
    read_search_key,
    resolve_archive_after_days,
    resolve_max_tokens,
    write_search_key,

    // ── 核心类型（5 struct + 3 enum）──
    ActiveModelId,
    ApiProvider,
    BotConfig,
    BotConfigView,
    EffectiveInference,
    KeySlot,
    MemoryTuning,
    ModelEntry,
    ModelsByProvider,
    PermMode,

    // ── 常量（14 个）──
    DEFAULT_ARCHIVE_DAYS,
    DEFAULT_MAX_TOKENS,
    KEYRING_SERVICE,
    KEYRING_USER,
    MAX_ARCHIVE_DAYS,
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
// base_url_is_safe 不移出：它只在 config 模块内部（SSRF 校验的 base_url 检查）用，
// 移出来 crate 内无人引用，编译器报 unused import。
pub(crate) use config::read_vendor_key;
pub(crate) use config::{
    add_allowed_dir, escape_for_log, load_config, resolve_model_override, truncate_for_log,
    update_config_file,
};

// dispatch re-export
pub(crate) use dispatch::parse_args;
pub use dispatch::{execute_tool, execute_tool_traced, execute_tool_with_stop, ToolCallTrace};

// tools re-export
pub use tools::broadcast_after_mutation;
