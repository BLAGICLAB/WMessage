//! R2 L2 版本层 · 从 EvolutionProposal 派生 ChangeRecord
//!
//! 派生规则（按 DERIVABILITY.md）：
//! - `change_id`           = `"chg-" + proposal_id`
//! - `schema_version`      = 1（默认；R2+ bump）
//! - `hard_constraint_compliance` = category+impact 判定（仅约束 5/6，
//!   见 passes_auto_apply_gate 文档；ChangeRecord 字段名=jsonl schema 不动）
//! - `layer`               = 从 category 派生（4/6 层覆盖；Parameter/Code 不在当前 category）
//! - `mem_key`             = `"evo:" + proposal_id`
//!
//! 不可派生（落 jsonl）：status, parent_id, eval_before/eval_after,
//! rolled_back_at, rollback_reason, approval_source, human_approver

use super::record::{ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
use crate::evolution::proposal::{EvolutionProposal, ImpactLevel, ProposalCategory};

/// change_id 派生函数（DERIVABILITY.md 字段 2）
pub fn derive_change_id(proposal_id: &str) -> String {
    format!("chg-{}", proposal_id)
}

/// mem_key 派生（与 apply.rs 一致：tags[0] = "evo:<proposal_id>"）
pub fn derive_mem_key(proposal_id: &str) -> String {
    format!("evo:{}", proposal_id)
}

/// schema_version 默认值
pub const DEFAULT_SCHEMA_VERSION: u32 = 1;

/// 自动应用门槛判定（构造时算，DERIVABILITY.md 字段 8）。
///
/// **只检硬约束 5/6**（仅 MemoryHint + High/Medium 可自动应用）——其余约束
/// 由 apply 入口校验，本函数**不代表 9 条全合规**——这正是改名的原因
///（旧名让调用方按名字推断成全合规，可能跳过下游复核）。
/// impact 用显式白名单（High | Medium）：未来新增变体必须显式 opt-in。
pub fn passes_auto_apply_gate(p: &EvolutionProposal) -> bool {
    matches!(p.category, ProposalCategory::MemoryHint)
        && matches!(p.impact, ImpactLevel::High | ImpactLevel::Medium)
}

/// layer 从 category 派生（DERIVABILITY.md 字段 1，部分覆盖）
///
/// 4/6 层映射；Parameter / Code 两层当前无 category 对应，
/// R4/R8 触发后再扩展。
pub fn derive_layer(category: ProposalCategory) -> EvolutionLayer {
    match category {
        ProposalCategory::MemoryHint => EvolutionLayer::Policy,
        ProposalCategory::PromptHint => EvolutionLayer::PromptHint,
        ProposalCategory::ToolSchemaHint => EvolutionLayer::ToolSchema,
        ProposalCategory::SkillHint => EvolutionLayer::Skill,
    }
}

/// 从 EvolutionProposal 构造 ChangeRecord
///
/// 初始 status 由 compliance 决定：
/// - compliance=true  → Pending（待沙箱或批准）
/// - compliance=false → Rejected（SystemRejected）
pub fn from_proposal(p: &EvolutionProposal, now_ms: i64) -> ChangeRecord {
    let compliance = passes_auto_apply_gate(p);
    let (status, approval_source) = if compliance {
        (ChangeStatus::Pending, ApprovalSource::Pending)
    } else {
        (ChangeStatus::Rejected, ApprovalSource::SystemRejected)
    };
    ChangeRecord {
        change_id: derive_change_id(&p.proposal_id),
        parent_id: None, // 根节点；迭代版本后续设
        schema_version: DEFAULT_SCHEMA_VERSION,
        layer: derive_layer(p.category),
        origin: p.origin,
        proposal_id: p.proposal_id.clone(),
        target: p.target.clone(),
        suggestion_text: p.suggestion.text.clone(),
        mem_key: derive_mem_key(&p.proposal_id),
        impact: p.impact,
        eval_before: None,
        eval_after: None,
        status,
        hard_constraint_compliance: compliance,
        approval_source,
        human_approver: None,
        created_at_ms: now_ms,
        rolled_back_at: None,
        rollback_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{
        Evidence, ImpactLevel, ProposalCategory, ProposalOrigin, ProposalTarget, Suggestion,
    };

    fn mk_proposal(id: &str, cat: ProposalCategory, impact: ImpactLevel) -> EvolutionProposal {
        EvolutionProposal {
            proposal_id: id.into(),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: cat,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            impact,
            evidence: Evidence {
                summary: format!("s-{id}"),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: format!("text-{id}"),
                structured_patch: None,
            },
        }
    }

    // ─── change_id 派生 ───

    #[test]
    fn change_id_format_locked() {
        // 锁死格式：chg- + proposal_id
        assert_eq!(derive_change_id("abc123"), "chg-abc123");
        assert_eq!(derive_mem_key("abc123"), "evo:abc123");
    }

    // ─── 自动应用门槛检查 ───

    #[test]
    fn gate_true_for_memory_high() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        assert!(passes_auto_apply_gate(&p));
    }

    #[test]
    fn gate_true_for_memory_medium() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Medium);
        assert!(passes_auto_apply_gate(&p));
    }

    #[test]
    fn gate_false_for_memory_low() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Low);
        assert!(!passes_auto_apply_gate(&p));
    }

    #[test]
    fn gate_false_for_non_memory_hint() {
        for cat in [
            ProposalCategory::PromptHint,
            ProposalCategory::ToolSchemaHint,
            ProposalCategory::SkillHint,
        ] {
            let p = mk_proposal("p1", cat, ImpactLevel::High);
            assert!(!passes_auto_apply_gate(&p), "{cat:?} 不应合规");
        }
    }

    // ─── layer 派生（4/6 层覆盖）───

    #[test]
    fn layer_mapping_partial() {
        assert_eq!(
            derive_layer(ProposalCategory::MemoryHint),
            EvolutionLayer::Policy
        );
        assert_eq!(
            derive_layer(ProposalCategory::PromptHint),
            EvolutionLayer::PromptHint
        );
        assert_eq!(
            derive_layer(ProposalCategory::ToolSchemaHint),
            EvolutionLayer::ToolSchema
        );
        assert_eq!(
            derive_layer(ProposalCategory::SkillHint),
            EvolutionLayer::Skill
        );
    }

    // ─── from_proposal 集成 ───

    #[test]
    fn from_compliant_proposal_starts_pending() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        let r = from_proposal(&p, 1000);
        assert_eq!(r.change_id, "chg-p1");
        assert_eq!(r.proposal_id, "p1");
        assert_eq!(r.mem_key, "evo:p1");
        assert_eq!(r.schema_version, 1);
        assert_eq!(r.status, ChangeStatus::Pending);
        assert_eq!(r.approval_source, ApprovalSource::Pending);
        assert!(r.hard_constraint_compliance);
        assert!(r.parent_id.is_none());
        assert_eq!(r.suggestion_text, "text-p1");
        assert_eq!(r.created_at_ms, 1000);
        assert!(r.rolled_back_at.is_none());
    }

    #[test]
    fn from_non_compliant_proposal_starts_rejected() {
        // PromptHint + High：非 MemoryHint，compliance=false
        let p = mk_proposal("p2", ProposalCategory::PromptHint, ImpactLevel::High);
        let r = from_proposal(&p, 2000);
        assert_eq!(r.status, ChangeStatus::Rejected);
        assert_eq!(r.approval_source, ApprovalSource::SystemRejected);
        assert!(!r.hard_constraint_compliance);
    }

    #[test]
    fn from_low_impact_memory_starts_rejected() {
        // MemoryHint + Low：合规失败
        let p = mk_proposal("p3", ProposalCategory::MemoryHint, ImpactLevel::Low);
        let r = from_proposal(&p, 3000);
        assert_eq!(r.status, ChangeStatus::Rejected);
        assert_eq!(r.approval_source, ApprovalSource::SystemRejected);
    }
}
