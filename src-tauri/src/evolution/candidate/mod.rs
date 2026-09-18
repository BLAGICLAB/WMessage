//! R4 L1 候选层（spec R4）
//!
//! 模块结构：
//! - `entry`     ProposalEntry + ProposalStatus + jsonl IO
//! - `derive`    从 EvolutionProposal 派生（纯函数，不调 LLM）
//! - `ttl`       TTL 14 天 + 过期机制（软标记 / 硬淘汰）
//! - `conflict`  同层同 target 冲突检测 + 跨层排序
//! - `mapping`   ProposalEntry → ChangeRecord 映射
//!
//! 关键设计（接 R0 DERIVABILITY.md 结论）：
//! - 不动 EvolutionProposal（emit.rs:107 锁死 audit schema）
//! - 扩展字段（layer / change_id / related_refs / TTL）落独立 evolution-proposals.jsonl
//! - ProposalEntry 是 superset：含足够信息派生 ChangeRecord
//!
//! 硬约束：
//! - 不调 LLM（规则化映射）
//! - 不写新数据库表（jsonl only）
//! - 依赖方向单向：evolution → memory 不存在

pub mod conflict;
pub mod derive;
pub mod entry;
pub mod mapping;
pub mod ttl;

pub use conflict::{find_conflict, impact_ord, is_conflict, layer_priority, resolve_conflict, sort_entries_cross_layer};
pub use derive::{derive_change_id as derive_change_id_in_candidate, derive_layer as derive_layer_in_candidate, derive_mem_key, from_proposal};
pub use entry::{
    append, filter_by_status, find_by_id, read_all, ProposalEntry, ProposalStatus,
};
pub use mapping::{map_proposal_status_to_change_status, to_change_record};
pub use ttl::{compute_expires_at, evict_expired, is_expired, mark_expired, TTL_DAYS, TTL_MS};