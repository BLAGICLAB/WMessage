//! 自进化系统：经验捕获 + 提案产出（Phase 1）+ 提案应用闭环（Phase 2）。
//!
//! 设计原则（spec 硬约束）：
//! - 依赖方向：本模块 import `memory::consolidate` 的公开类型
//!   (`ConsolidateOp` / `ConsolidateReport`)，memory 模块不感知 evolution 存在；
//! - 不调第二次 LLM（必须复用 consolidate 已有反思调用）；
//! - 不写新数据库表 / 不改 schema（Phase 2 应用复用现有 mem_items 表）；
//! - 不改 prompt / TOOLS schema / 命令名 / 事件名 / JSON 字段 / 错误码；
//! - 不接收 session_id / 不接收任何反向依赖 memory 内部的状态。
//!
//! 模块分层：
//! - `trace`    数据结构：ExecutionTrace / ToolCallSummary / TraceOutcome（Phase 1.1）
//! - `proposal` 数据结构：EvolutionProposal + 枚举 + proposal_id 归一化（Phase 1.2）
//! - `derive`   纯函数：从 `[ConsolidateOp]` 启发式派生 proposals（Phase 1.3）
//! - `emit`     副作用：写 audit + 进程内 24h dedup（Phase 1.4）
//! - `apply`    应用：MemoryHint + High/Medium 提案落 lesson 记忆闭环生效（Phase 2）
//!
//! Phase 2 闭环：达门槛提案写入 mem_items（kind=lesson，tags[0]=evo:<proposal_id>
//! 持久幂等），下轮对话经 `memory::injection_block` 的 lesson 槽位自动带出，
//! 行为随之改变；留痕 evolution-applied.jsonl，回滚 = 删同 key 记忆。
//! PromptHint / ToolSchemaHint / SkillHint 永不自动应用，仍只写 audit。

pub mod apply;
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
    // Phase 2：emit（audit）之前先把达门槛的子集挑出来交给 apply——
    // emit 的 24h dedup 会吞掉重复提案，apply 侧靠 evo:<proposal_id> 持久幂等，
    // 两条去重链路互不影响。
    let gated: Vec<_> = proposals
        .iter()
        .filter(|p| apply::auto_apply_gate(p))
        .cloned()
        .collect();
    emit::emit_proposals(proposals);
    apply::apply_from_consolidation(gated);
}
