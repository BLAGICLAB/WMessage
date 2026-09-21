//! R4 L1 候选层 · 冲突检测 + 排序
//!
//! spec R4：
//! - 冲突解决：同层同 target 按 impact 排序；跨层按优先级
//!
//! 同层同 target：保留 higher impact，丢弃 lower（impact_eq 时保留 older）
//! 跨层：按层级优先级（Safety > Memory > ToolSchema > Skill > PromptHint > Parameter > Code）

use super::entry::ProposalEntry;
use crate::evolution::change::EvolutionLayer;
use crate::evolution::proposal::ImpactLevel;

/// 跨层优先级（数字越小优先级越高）
pub fn layer_priority(layer: EvolutionLayer) -> u32 {
    match layer {
        // Parameter 是运行时行为参数（温度、top_p 等），影响最大，先排
        EvolutionLayer::Parameter => 0,
        // Policy 是记忆策略，重要性高
        EvolutionLayer::Policy => 1,
        // ToolSchema 影响工具行为
        EvolutionLayer::ToolSchema => 2,
        // Skill 影响技能调度
        EvolutionLayer::Skill => 3,
        // PromptHint 影响提示词
        EvolutionLayer::PromptHint => 4,
        // Code R8 才引入，未实现
        EvolutionLayer::Code => 5,
    }
}

/// impact 数值化（用于排序）
pub fn impact_ord(impact: ImpactLevel) -> u32 {
    match impact {
        ImpactLevel::High => 2,
        ImpactLevel::Medium => 1,
        ImpactLevel::Low => 0,
    }
}

/// 是否冲突（同 proposal_id 不算自比冲突；同层 + 同 target）
pub fn is_conflict(a: &ProposalEntry, b: &ProposalEntry) -> bool {
    a.proposal_id != b.proposal_id && a.layer == b.layer && a.target.tag() == b.target.tag()
}

/// 在现有 entries 中查找与 new 冲突的条目（按优先级）
///
/// 冲突定义：同 layer + 同 target.tag()
/// 返回第一个冲突项（如有）
pub fn find_conflict<'a>(
    entries: &'a [ProposalEntry],
    new: &ProposalEntry,
) -> Option<&'a ProposalEntry> {
    entries
        .iter()
        .find(|e| is_conflict(e, new) && e.proposal_id != new.proposal_id)
}

/// 冲突解决：保留胜者，丢弃败者
///
/// 规则：
/// - impact 不同：higher impact 胜
/// - impact 相同：older (created_at_ms 更小) 胜
/// - 返回 (winner, loser)
pub fn resolve_conflict<'a>(
    a: &'a ProposalEntry,
    b: &'a ProposalEntry,
) -> (&'a ProposalEntry, &'a ProposalEntry) {
    if impact_ord(a.impact) != impact_ord(b.impact) {
        if impact_ord(a.impact) > impact_ord(b.impact) {
            (a, b)
        } else {
            (b, a)
        }
    } else {
        // impact 相同 → older 胜
        if a.created_at_ms <= b.created_at_ms {
            (a, b)
        } else {
            (b, a)
        }
    }
}

/// 跨层排序：先按 layer_priority 升序，再按 impact 降序，最后按 created_at 升序
pub fn sort_entries_cross_layer(entries: &mut Vec<ProposalEntry>) {
    entries.sort_by(|a, b| {
        layer_priority(a.layer)
            .cmp(&layer_priority(b.layer))
            .then(impact_ord(b.impact).cmp(&impact_ord(a.impact)))
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{ProposalOrigin, ProposalTarget};

    fn mk(
        id: &str,
        layer: EvolutionLayer,
        impact: ImpactLevel,
        target: String,
        created: i64,
    ) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer,
            impact,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy { policy: target },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("s-{id}"),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: created,
            expires_at_ms: created + 86_400_000,
            status: super::super::entry::ProposalStatus::Pooled,
        }
    }

    // ─── 冲突检测 ───

    #[test]
    fn is_conflict_same_layer_same_target() {
        let a = mk(
            "a",
            EvolutionLayer::Policy,
            ImpactLevel::Medium,
            "p1".into(),
            1000,
        );
        let b = mk(
            "b",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            2000,
        );
        assert!(is_conflict(&a, &b));
    }

    #[test]
    fn no_conflict_different_layer() {
        let a = mk(
            "a",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            1000,
        );
        let b = mk(
            "b",
            EvolutionLayer::ToolSchema,
            ImpactLevel::High,
            "p1".into(),
            2000,
        );
        assert!(!is_conflict(&a, &b));
    }

    #[test]
    fn no_conflict_different_target() {
        let a = mk(
            "a",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            1000,
        );
        let b = mk(
            "b",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p2".into(),
            2000,
        );
        assert!(!is_conflict(&a, &b));
    }

    #[test]
    fn no_conflict_same_proposal_id() {
        // 同一个 proposal_id 不算冲突（自比）
        let a = mk(
            "same",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            1000,
        );
        assert!(!is_conflict(&a, &a));
    }

    #[test]
    fn find_conflict_returns_first_match() {
        let entries = vec![
            mk(
                "a",
                EvolutionLayer::Skill,
                ImpactLevel::Medium,
                "other".into(),
                1000,
            ),
            mk(
                "b",
                EvolutionLayer::Policy,
                ImpactLevel::High,
                "p1".into(),
                2000,
            ),
            mk(
                "c",
                EvolutionLayer::Policy,
                ImpactLevel::Medium,
                "p1".into(),
                3000,
            ),
        ];
        let new = mk(
            "new",
            EvolutionLayer::Policy,
            ImpactLevel::Low,
            "p1".into(),
            4000,
        );
        let conflict = find_conflict(&entries, &new).unwrap();
        assert_eq!(conflict.proposal_id, "b");
    }

    // ─── 冲突解决 ───

    #[test]
    fn resolve_higher_impact_wins() {
        let a = mk(
            "a",
            EvolutionLayer::Policy,
            ImpactLevel::Medium,
            "p1".into(),
            1000,
        );
        let b = mk(
            "b",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            2000,
        );
        let (winner, loser) = resolve_conflict(&a, &b);
        assert_eq!(winner.proposal_id, "b"); // High 胜
        assert_eq!(loser.proposal_id, "a");
    }

    #[test]
    fn resolve_same_impact_older_wins() {
        let older = mk(
            "old",
            EvolutionLayer::Policy,
            ImpactLevel::Medium,
            "p1".into(),
            1000,
        );
        let newer = mk(
            "new",
            EvolutionLayer::Policy,
            ImpactLevel::Medium,
            "p1".into(),
            2000,
        );
        let (winner, loser) = resolve_conflict(&older, &newer);
        assert_eq!(winner.proposal_id, "old");
        assert_eq!(loser.proposal_id, "new");
    }

    // ─── 跨层排序 ───

    #[test]
    fn sort_by_layer_priority_then_impact_then_age() {
        let mut entries = vec![
            mk(
                "a",
                EvolutionLayer::PromptHint,
                ImpactLevel::High,
                "x".into(),
                1000,
            ),
            mk(
                "b",
                EvolutionLayer::Policy,
                ImpactLevel::Low,
                "y".into(),
                1000,
            ),
            mk(
                "c",
                EvolutionLayer::Policy,
                ImpactLevel::High,
                "z".into(),
                500,
            ),
            mk(
                "d",
                EvolutionLayer::ToolSchema,
                ImpactLevel::Medium,
                "w".into(),
                1000,
            ),
        ];
        sort_entries_cross_layer(&mut entries);
        // 期望顺序：Policy 优先（priority=1），按 impact 降序，age 升序
        //   c (Policy, High, 500)
        //   b (Policy, Low, 1000)
        //   d (ToolSchema, Medium, 1000)
        //   a (PromptHint, High, 1000)
        assert_eq!(entries[0].proposal_id, "c");
        assert_eq!(entries[1].proposal_id, "b");
        assert_eq!(entries[2].proposal_id, "d");
        assert_eq!(entries[3].proposal_id, "a");
    }

    // ─── 优先级常量锁死 ───

    #[test]
    fn layer_priority_locked() {
        assert_eq!(layer_priority(EvolutionLayer::Parameter), 0);
        assert_eq!(layer_priority(EvolutionLayer::Policy), 1);
        assert_eq!(layer_priority(EvolutionLayer::ToolSchema), 2);
        assert_eq!(layer_priority(EvolutionLayer::Skill), 3);
        assert_eq!(layer_priority(EvolutionLayer::PromptHint), 4);
        assert_eq!(layer_priority(EvolutionLayer::Code), 5);
    }

    #[test]
    fn impact_ord_locked() {
        assert_eq!(impact_ord(ImpactLevel::High), 2);
        assert_eq!(impact_ord(ImpactLevel::Medium), 1);
        assert_eq!(impact_ord(ImpactLevel::Low), 0);
    }
}
