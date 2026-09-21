//! R2 L2 版本层（spec 硬约束：依赖方向单向 memory → evolution）
//!
//! 模块结构：
//! - `record`  ChangeRecord + 5 个枚举 + jsonl IO
//! - `status`  9 态状态机 + 非法路径拦截
//! - `derive`  从 EvolutionProposal 派生 ChangeRecord
//!
//! 不调 LLM；不写新数据库表；持久化一律 evolution-changes.jsonl。

pub mod derive;
pub mod record;
pub mod status;

pub use derive::{
    derive_change_id, derive_layer, derive_mem_key, from_proposal, hard_constraint_compliance,
    DEFAULT_SCHEMA_VERSION,
};
pub use record::{
    append as append_change, find_by_id, find_children, find_roots, read_all, ApprovalSource,
    ChangeRecord, ChangeStatus, EvalResult, EvolutionLayer,
};
pub use status::{can_transition, transition};
