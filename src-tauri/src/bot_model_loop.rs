//! 模型流式调用 + 工具循环 核心：
//!
//! 经典 Agent 框架（LangChain AgentExecutor / Claude Agent SDK / AutoGen）
//! 的「决策/调用」与「执行/工具循环」层职责：
//! - 流式 SSE 解析（parse_sse_chunk）
//! - 思考块拆分（feed_think + tail_prefix_len）
//! - 工具循环（run_model_loop）：单轮 Function 调用熔断 + 软警告 +
//!   Skill 状态机推进 + 进程内执行工具 + 续聊
//! - TOOLS schema 编译期保证合法（tests/llm_integration.rs 接入）
//!
//! 本模块与 bot_chat 的边界：bot_chat.rs 调 run_model_loop 拿到最终文本；
//! 本模块只关心「怎么流式拿到最终文本 + 工具执行结果」，不关心输入侧组装。
// 从 crate::bot::registry 单源派生的 TOOLS / MUTATING_TOOLS。
// 原 const 字符串 / 数组现为函数（OnceLock 缓存）——保持向后兼容路径。
#[allow(unused_imports)]
pub use crate::bot::registry::mutating_tools as MUTATING_TOOLS;

#[allow(unused_imports)]
pub use crate::bot::registry::claims_mutation;

// ── 拆分模块声明 ──
mod context;
mod core;
mod evidence;
mod finalize;
mod fuse;
mod guard;
mod output;
mod request;
mod shell;
mod sse;
mod stream;
mod types;

// 旧路径 1:1 保持：拆分前就是 pub 的项，拆分后仍从 crate::bot_model_loop:: 取。
// bot_model_loop 是私有模块，这些 pub 项对外不可达——「没人用」不等于写错，
// 故逐条豁免 unused_imports（属性只作用于紧邻的那条语句）。
#[allow(unused_imports)]
pub use self::core::run_model_loop_core;
#[allow(unused_imports)]
pub use self::request::apply_inference_params;
#[allow(unused_imports)]
pub use self::shell::{noop_replan, resolve_temperature, run_model_loop, shared_llm_client};
#[allow(unused_imports)]
pub use self::sse::STREAM_CHUNK_IDLE_TIMEOUT;
#[allow(unused_imports)]
pub use self::stream::{
    accumulate_tool_call_delta, drain_sse_lines, merge_stream_usage, parse_openai_usage,
    parse_sse_chunk, MAX_TOOL_CALL_ARGUMENTS_BYTES, MAX_TOOL_CALL_INDEX,
};
#[allow(unused_imports)]
pub use self::types::{
    LlmHttp, LoopTrace, ModelLoopDeps, ParsedChunk, StreamError, TokenUsage, ToolCallDelta,
    TurnStatus, WallBudget,
};

// 跨模块共用的私有项：门面转出，子模块之间零直接依赖。
pub(crate) use self::context::{
    apply_tool_window, is_context_overflow, is_retryable_llm_status, msg_chars, reindex_after_shed,
    shed_oldest_history, tool_result_chars, user_intent_of, TOOL_WINDOW_KEEP_RECENT,
    TOOL_WINDOW_TRIGGER_CHARS,
};
pub(crate) use self::evidence::{
    context_sources, number_sourced, ToolEvidence, EVIDENCE_OUTPUT_CHARS,
};
pub(crate) use self::finalize::{
    decide_selfcheck, finalize_exit, finalize_selfcheck, FinalizeOutcome, SelfcheckDecision,
};
pub(crate) use self::fuse::{
    check_fuses, fuse_cap_for, fuse_message, inject_soft_warnings, rounds_fuse_exit, should_fuse,
    FUSE_MARKER, MAX_FUNCTION_CALLS_PER_REQUEST, ROUNDS_FUSE_MARKER, WALL_FUSE_MARKER,
};
pub(crate) use self::guard::{
    finalize_without_tools, selfcheck_retry_hint, side_selfcheck_verdict, SELFCHECK_FAIL_EMIT,
    SELFCHECK_PASS_EMIT, SELFCHECK_PASS_HINT,
};
pub(crate) use self::output::{apply_output_cap, DEFAULT_TOOL_OUTPUT_CAP_BYTES};
pub(crate) use self::request::{
    compact_before_round, prepare_round, request_round, LLM_RETRY_DELAY, MAX_LLM_ATTEMPTS,
    MAX_STREAM_RETRIES,
};
pub(crate) use self::shell::resolve_max_rounds;
pub(crate) use self::sse::{read_sse_stream, stream_round};
pub(crate) use self::stream::{
    claim_mutation_fires, feed_think, mutation_succeeded, stamp_last_assistant_reasoning,
    strip_thinking_key, unparsable_sse_payload, with_thinking,
};
pub(crate) use self::types::{
    ErrorDisposition, ErrorResponse, RequestOutcome, RequestPlan, RequestState, RequestWork,
    RoundCtx,
};

// 只有测试在用的私有项：按构建期切换——非测试构建里它们没人用，
// 转出会被 rustc 判 unused import。
#[cfg(test)]
pub(crate) use self::fuse::{fuse_cap_upper_bound, rounds_fuse_message, wall_fuse_message};
// ── 拆分模块声明 结束 ──

// 防幻觉汇报守卫
// 实锤事故：MiniMax-M3 多次不调任何工具就回复「已添加子任务」「已移至回收站」，
// 数据实际没变，用户以为操作成功。提示词约束（SYSTEM_PROMPT 规则 8）不够，
// 这里在循环出口做确定性拦截：最终文本声称完成变更、但本轮 0 次变更类工具调用 →
// 注入系统提醒并补一轮（每次对话最多补一次），让模型实际调工具或如实说明。

/// 从 ToolDef 注册表派生的变更声称检测。
/// 原「14 个动词 + 4 个整段」硬编码迁到 `bot::registry::ToolDef.claims_patterns`，
/// 加新 mutating 工具只需填自己的 pattern，检测逻辑零修改。
#[cfg(test)]
use crate::bot::registry::{claimed_doc_paths, claims_execution_with, path_missing_on_disk};

// TOOLS schema（编译期字符串，运行期 JSON 解析）

// TOOLS / MUTATING_TOOLS 已在阶段 2 拆出到 crate::bot::registry：
// - `crate::bot::registry::tools_json()` → 原 TOOLS JSON（OnceLock 缓存）
// - `crate::bot::registry::mutating_tools()` → 原 MUTATING_TOOLS（15 个 mutating 工具名）
// 公开路径仍走 `pub use crate::bot::registry::tools_json as TOOLS;` / `as MUTATING_TOOLS;`
// 但因 const → fn 类型变化（保持向后兼容需按函数调用），下游消费者已同步更新。

#[cfg(test)]
mod tests;
