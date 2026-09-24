//! R4 L1 候选层 · 从 EvolutionProposal 派生 ProposalEntry
//!
//! 纯函数；不调 LLM（spec R4 硬约束）；规则化映射。

use super::entry::{ProposalEntry, ProposalStatus};
use super::ttl::compute_expires_at;
use crate::evolution::change::EvolutionLayer;
use crate::evolution::proposal::{EvolutionProposal, ProposalCategory};

/// layer 派生（完整 6 层；区别于 ProposalCategory 4 类）
///
/// 4/6 类从 category 直接映射；Parameter / Code 两类当前 category 无对应，
/// R4 仅在 category 能覆盖的 4 层派生；未来扩 category 时再补。
pub fn derive_layer(category: ProposalCategory) -> EvolutionLayer {
    match category {
        ProposalCategory::MemoryHint => EvolutionLayer::Policy,
        ProposalCategory::PromptHint => EvolutionLayer::PromptHint,
        ProposalCategory::ToolSchemaHint => EvolutionLayer::ToolSchema,
        ProposalCategory::SkillHint => EvolutionLayer::Skill,
    }
}

/// change_id 派生
pub fn derive_change_id(proposal_id: &str) -> String {
    format!("chg-{}", proposal_id)
}

/// mem_key 派生（与 apply.rs 一致）
pub fn derive_mem_key(proposal_id: &str) -> String {
    format!("evo:{}", proposal_id)
}

/// 从 EvolutionProposal 构造 ProposalEntry（默认状态：Pooled）
pub fn from_proposal(p: &EvolutionProposal, now_ms: i64) -> ProposalEntry {
    ProposalEntry {
        proposal_id: p.proposal_id.clone(),
        change_id: derive_change_id(&p.proposal_id),
        layer: derive_layer(p.category),
        impact: p.impact,
        origin: p.origin,
        target: p.target.clone(),
        suggestion_text: p.suggestion.text.clone(),
        mem_key: derive_mem_key(&p.proposal_id),
        related_refs: p.evidence.related_refs.clone(),
        summary: p.evidence.summary.clone(),
        occurrence_count: p.evidence.occurrence_count,
        window_hours: p.evidence.window_hours,
        created_at_ms: p.created_at_ms,
        expires_at_ms: compute_expires_at(now_ms),
        status: ProposalStatus::Pooled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::candidate::ttl::TTL_MS;
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
                occurrence_count: 5,
                window_hours: 24,
                related_refs: vec![format!("ref-{id}")],
            },
            suggestion: Suggestion {
                text: format!("text-{id}"),
                structured_patch: None,
            },
        }
    }

    #[test]
    fn derive_change_id_and_mem_key_format_locked() {
        assert_eq!(derive_change_id("abc"), "chg-abc");
        assert_eq!(derive_mem_key("abc"), "evo:abc");
    }

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

    #[test]
    fn from_proposal_populates_all_fields() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        let entry = from_proposal(&p, 2_000_000_000_000);
        assert_eq!(entry.proposal_id, "p1");
        assert_eq!(entry.change_id, "chg-p1");
        assert_eq!(entry.mem_key, "evo:p1");
        assert_eq!(entry.layer, EvolutionLayer::Policy);
        assert_eq!(entry.impact, ImpactLevel::High);
        assert_eq!(entry.origin, p.origin);
        assert_eq!(entry.target, p.target);
        assert_eq!(entry.suggestion_text, "text-p1");
        assert_eq!(entry.summary, "s-p1");
        assert_eq!(entry.occurrence_count, 5);
        assert_eq!(entry.window_hours, 24);
        assert_eq!(entry.related_refs, vec!["ref-p1"]);
        assert_eq!(entry.created_at_ms, p.created_at_ms);
        assert_eq!(entry.expires_at_ms, 2_000_000_000_000 + TTL_MS);
        assert_eq!(entry.status, ProposalStatus::Pooled);
    }

    #[test]
    fn from_proposal_starts_pooled() {
        let p = mk_proposal("p2", ProposalCategory::ToolSchemaHint, ImpactLevel::Medium);
        let entry = from_proposal(&p, 1_000_000_000_000);
        assert_eq!(entry.status, ProposalStatus::Pooled);
    }

    #[test]
    fn expires_at_now_plus_ttl() {
        let p = mk_proposal("p3", ProposalCategory::PromptHint, ImpactLevel::Low);
        let entry = from_proposal(&p, 5_000);
        assert_eq!(entry.expires_at_ms, 5_000 + TTL_MS);
    }
}
