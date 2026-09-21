//! R4 L1 候选层 · TTL 14 天过期机制
//!
//! spec R4：候选池 evolution-proposals.jsonl TTL = 14 天
//! 超期后 status 转为 Expired（软淘汰，保留历史）。
//!
//! 也可以选择硬淘汰（从 jsonl 删除）；当前实现支持两种：
//! - mark_expired：把 status 改为 Expired（保留行）
//! - evict_expired：从 vec 中移除（丢失历史）

use super::entry::{ProposalEntry, ProposalStatus};

/// TTL = 14 天（spec）
pub const TTL_DAYS: i64 = 14;
/// TTL 毫秒数
pub const TTL_MS: i64 = TTL_DAYS * 86_400_000;

/// 该 entry 是否已过期（expires_at_ms <= now_ms）
pub fn is_expired(entry: &ProposalEntry, now_ms: i64) -> bool {
    entry.expires_at_ms <= now_ms
}

/// 把过期条目的 status 改为 Expired（in-place）；返回标记数
pub fn mark_expired(entries: &mut Vec<ProposalEntry>, now_ms: i64) -> usize {
    let mut count = 0;
    for e in entries.iter_mut() {
        if e.status == ProposalStatus::Pooled && is_expired(e, now_ms) {
            e.status = ProposalStatus::Expired;
            count += 1;
        }
    }
    count
}

/// 硬淘汰：从 vec 中移除过期条目（返回淘汰数）
pub fn evict_expired(entries: &mut Vec<ProposalEntry>, now_ms: i64) -> usize {
    let before = entries.len();
    entries.retain(|e| !is_expired(e, now_ms));
    before - entries.len()
}

/// 计算 expires_at_ms（from_proposal 用）
pub fn compute_expires_at(created_at_ms: i64) -> i64 {
    created_at_ms + TTL_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::EvolutionLayer;
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk(id: &str, expires_at: i64) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer: EvolutionLayer::Policy,
            impact: ImpactLevel::Medium,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: "x".into(),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: "x".into(),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: 1_000,
            expires_at_ms: expires_at,
            status: ProposalStatus::Pooled,
        }
    }

    #[test]
    fn ttl_constants_locked() {
        assert_eq!(TTL_DAYS, 14);
        assert_eq!(TTL_MS, 14 * 86_400_000);
    }

    #[test]
    fn is_expired_true_when_now_past_expiry() {
        let e = mk("a", 1000);
        assert!(is_expired(&e, 1001));
        assert!(is_expired(&e, 1000)); // 边界：now == expires_at 也算过期
    }

    #[test]
    fn is_expired_false_when_before_expiry() {
        let e = mk("a", 1000);
        assert!(!is_expired(&e, 999));
    }

    #[test]
    fn mark_expired_only_affects_pooled() {
        let mut entries = vec![
            mk("a", 100),  // expired + Pooled → 改 Expired
            mk("b", 100),  // expired + Pooled → 改 Expired
            mk("c", 5000), // not expired → 不动
        ];
        let n = mark_expired(&mut entries, 1000);
        assert_eq!(n, 2);
        assert_eq!(entries[0].status, ProposalStatus::Expired);
        assert_eq!(entries[1].status, ProposalStatus::Expired);
        assert_eq!(entries[2].status, ProposalStatus::Pooled);
    }

    #[test]
    fn mark_expired_skips_non_pooled() {
        // 已 Promoted / Expired / Rejected 不再改
        let mut entries = vec![
            {
                let mut e = mk("a", 100);
                e.status = ProposalStatus::Promoted;
                e
            },
            {
                let mut e = mk("b", 100);
                e.status = ProposalStatus::Rejected;
                e
            },
        ];
        let n = mark_expired(&mut entries, 1000);
        assert_eq!(n, 0);
        assert_eq!(entries[0].status, ProposalStatus::Promoted);
        assert_eq!(entries[1].status, ProposalStatus::Rejected);
    }

    #[test]
    fn evict_expired_removes_only_expired() {
        let mut entries = vec![mk("a", 100), mk("b", 5000), mk("c", 100)];
        let n = evict_expired(&mut entries, 1000);
        assert_eq!(n, 2);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].proposal_id, "b");
    }

    #[test]
    fn compute_expires_at_adds_ttl() {
        assert_eq!(compute_expires_at(0), TTL_MS);
        assert_eq!(
            compute_expires_at(1_000_000_000_000),
            1_000_000_000_000 + TTL_MS
        );
    }
}
