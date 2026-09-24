//! R4 L1 候选层 · ProposalEntry → ChangeRecord 映射
//!
//! spec R4：ProposalStatus ↔ ChangeStatus 映射
//! 映射规则：
//! - Pooled / Promoted → ChangeStatus::Pending
//! - Expired → ChangeStatus::Expired
//! - Rejected → ChangeStatus::Rejected

use super::entry::{ProposalEntry, ProposalStatus};
use crate::evolution::change::{
    ApprovalSource, ChangeRecord, ChangeStatus, DEFAULT_SCHEMA_VERSION,
};

/// ProposalStatus → ChangeStatus 映射
pub fn map_proposal_status_to_change_status(s: ProposalStatus) -> ChangeStatus {
    match s {
        ProposalStatus::Pooled => ChangeStatus::Pending,
        ProposalStatus::Promoted => ChangeStatus::Pending,
        ProposalStatus::Expired => ChangeStatus::Expired,
        ProposalStatus::Rejected => ChangeStatus::Rejected,
    }
}

/// 从 ProposalEntry 派生 ChangeRecord
///
/// 注意：approval_source 取决于合规检查与 resolved status（两者一起派生，
/// 避免「status=Rejected 却 approval_source=Pending」的内部不一致）。
/// Expired 保持 Pending：ApprovalSource 无 Expired 对应变体（从未批准也未被拒）。
pub fn to_change_record(entry: &ProposalEntry, hard_constraint_compliance: bool) -> ChangeRecord {
    let status = if !hard_constraint_compliance {
        ChangeStatus::Rejected
    } else {
        map_proposal_status_to_change_status(entry.status)
    };
    // approval_source 与 status 一起派生：不合规/被拒 → SystemRejected，
    // 其余 → Pending（不合规时 status 已被压成 Rejected，走同一分支）。
    let approval_source = match status {
        ChangeStatus::Rejected => ApprovalSource::SystemRejected,
        _ => ApprovalSource::Pending,
    };
    ChangeRecord {
        change_id: entry.change_id.clone(),
        parent_id: None, // 根
        schema_version: DEFAULT_SCHEMA_VERSION,
        layer: entry.layer,
        origin: entry.origin,
        proposal_id: entry.proposal_id.clone(),
        target: entry.target.clone(),
        suggestion_text: entry.suggestion_text.clone(),
        mem_key: entry.mem_key.clone(),
        impact: entry.impact,
        eval_before: None,
        eval_after: None,
        status,
        hard_constraint_compliance,
        approval_source,
        human_approver: None,
        created_at_ms: entry.created_at_ms,
        rolled_back_at: None,
        rollback_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::EvolutionLayer;
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk_entry(id: &str, status: ProposalStatus) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer: EvolutionLayer::Policy,
            impact: ImpactLevel::High,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("s-{id}"),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: 1_700_000_000_000,
            expires_at_ms: 1_700_000_000_000 + 86_400_000 * 14,
            status,
        }
    }

    // ─── 映射表锁死 ───

    #[test]
    fn pooled_maps_to_pending() {
        assert_eq!(
            map_proposal_status_to_change_status(ProposalStatus::Pooled),
            ChangeStatus::Pending
        );
    }

    #[test]
    fn promoted_maps_to_pending() {
        assert_eq!(
            map_proposal_status_to_change_status(ProposalStatus::Promoted),
            ChangeStatus::Pending
        );
    }

    #[test]
    fn expired_maps_to_expired() {
        assert_eq!(
            map_proposal_status_to_change_status(ProposalStatus::Expired),
            ChangeStatus::Expired
        );
    }

    #[test]
    fn rejected_maps_to_rejected() {
        assert_eq!(
            map_proposal_status_to_change_status(ProposalStatus::Rejected),
            ChangeStatus::Rejected
        );
    }

    // ─── to_change_record 字段透传 ───

    #[test]
    fn to_change_record_carries_entry_fields() {
        let entry = mk_entry("p1", ProposalStatus::Pooled);
        let cr = to_change_record(&entry, true);
        assert_eq!(cr.change_id, "chg-p1");
        assert_eq!(cr.proposal_id, "p1");
        assert_eq!(cr.layer, EvolutionLayer::Policy);
        assert_eq!(cr.target, entry.target);
        assert_eq!(cr.suggestion_text, "text-p1");
        assert_eq!(cr.mem_key, "evo:p1");
        assert_eq!(cr.impact, ImpactLevel::High);
        assert_eq!(cr.status, ChangeStatus::Pending);
        assert_eq!(cr.approval_source, ApprovalSource::Pending);
        assert!(cr.hard_constraint_compliance);
        assert_eq!(cr.schema_version, DEFAULT_SCHEMA_VERSION);
        assert_eq!(cr.created_at_ms, entry.created_at_ms);
    }

    #[test]
    fn to_change_record_non_compliant_overrides_to_rejected() {
        // 即使 entry.status=Pooled，compliance=false 仍 → Rejected
        let entry = mk_entry("p2", ProposalStatus::Pooled);
        let cr = to_change_record(&entry, false);
        assert_eq!(cr.status, ChangeStatus::Rejected);
        assert_eq!(cr.approval_source, ApprovalSource::SystemRejected);
        assert!(!cr.hard_constraint_compliance);
    }

    #[test]
    fn to_change_record_expired_entry() {
        let entry = mk_entry("p3", ProposalStatus::Expired);
        let cr = to_change_record(&entry, true);
        assert_eq!(cr.status, ChangeStatus::Expired);
        assert_eq!(cr.approval_source, ApprovalSource::Pending);
    }

    #[test]
    fn to_change_record_rejected_entry() {
        let entry = mk_entry("p4", ProposalStatus::Rejected);
        let cr = to_change_record(&entry, true);
        assert_eq!(cr.status, ChangeStatus::Rejected);
        // status 与 approval_source 一起派生：Rejected 不再配 Pending
        assert_eq!(cr.approval_source, ApprovalSource::SystemRejected);
    }
}
