//! 自进化系统 Phase 1：经验捕获 + 提案产出，只写 audit。
//!
//! 设计原则（spec 硬约束）：
//! - 依赖方向：本模块 import `memory::consolidate` 的公开类型
//!   (`ConsolidateOp` / `ConsolidateReport`)，memory 模块不感知 evolution 存在；
//! - 不调第二次 LLM（必须复用 consolidate 已有反思调用）；
//! - 不写数据库表 / 不改 schema / 不改 AppState 字段；
//! - 不改 prompt / TOOLS schema / 命令名 / 事件名 / JSON 字段 / 错误码；
//! - 不接收 session_id / 不接收任何反向依赖 memory 内部的状态。
//!
//! 模块分层：
//! - `trace`    数据结构：ExecutionTrace / ToolCallSummary / TraceOutcome（Phase 1.1）
//! - `proposal` 数据结构：EvolutionProposal + 枚举 + proposal_id 归一化（Phase 1.2）
//! - `derive`   纯函数：从 `[ConsolidateOp]` 启发式派生 proposals（Phase 1.3）
//! - `emit`     副作用：写 audit + 进程内 24h dedup（Phase 1.4）
//!
//! Phase 1 不落盘、不发应用事件、不触发任何应用路径——提案一律只写 audit，
//! 由用户跑一周后人工评估质量再决定 Phase 2 走向。

pub mod derive;
pub mod emit;
pub mod proposal;
pub mod trace;

use crate::memory::consolidate::{ConsolidateOp, ConsolidateReport};

/// 反思完成后的桥接入口（`memory::consolidate::run_consolidation` 末尾调用）。
///
/// 签名严格按 spec：不接收 AppHandle / session_id / 任何反向依赖 memory 内部的状态。
/// emit 所需的 AppHandle 由 `emit::register_app_handle` 在 App 启动时注册一次，
/// 本函数通过全局 OnceLock 取出——consolidate 侧 caller 无需关心。
pub fn post_consolidation(ops: &[ConsolidateOp], report: &ConsolidateReport) {
    let proposals = derive::derive_proposals(ops, report);
    emit::emit_proposals(proposals);
}
