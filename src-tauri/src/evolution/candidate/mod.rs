//! R4 L1 候选层（spec R4）
//!
//! 模块结构：
//! - `entry`     ProposalEntry + ProposalStatus + jsonl IO
//! - `derive`    从 EvolutionProposal 派生（纯函数，不调 LLM）
//! - `ttl`       TTL 14 天 + 过期标记（软标记）
//! - `mapping`   ProposalEntry → ChangeRecord 映射
//!
//! 冲突消解（同层同 target 留高 impact）零生产调用已删除，
//! 判定语义由决策板证据模块按需重建。历史实现见 git log。
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

pub mod derive;
pub mod entry;
pub mod mapping;
pub mod ttl;

pub use derive::from_proposal;
pub use entry::{append, filter_by_status, find_by_id, read_all, ProposalEntry, ProposalStatus};
pub use mapping::{map_proposal_status_to_change_status, to_change_record};
pub use ttl::compute_expires_at;

/// 批量落盘 proposals 到 evolution-proposals.jsonl（dedup by proposal_id）
///
/// 路径：`{data_dir}/evolution-proposals.jsonl`（沿用 panel/commands.rs:24 模式）
/// 返回实际新写入的条目（被 dedup 跳过的不计）——通知中心按此逐条落消息。
/// 调用方：evolution::post_consolidation。
///
/// **重要性（老板 12:29 拍板修复）**：R6 A shadow 钩子依赖此文件。补上让整条
/// observation pipeline 真正通。
pub fn write_proposals<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    proposals: &[crate::evolution::proposal::EvolutionProposal],
) -> Result<Vec<ProposalEntry>, String> {
    use std::collections::HashSet;
    let path = crate::db::paths::data_dir(app).join("evolution-proposals.jsonl");
    // read_all Err（首行结构级损坏）必须传播——dedup 基线不可得时盲写会重复追加
    let existing = entry::read_all(&path)?;
    let existing_ids: HashSet<String> = existing.iter().map(|e| e.proposal_id.clone()).collect();
    let mut written: Vec<ProposalEntry> = Vec::new();
    let now_ms = chrono::Utc::now().timestamp_millis();
    for p in proposals {
        if existing_ids.contains(&p.proposal_id) {
            continue;
        }
        let entry = derive::from_proposal(p, now_ms);
        entry::append(&path, &entry)?;
        written.push(entry);
    }
    Ok(written)
}
