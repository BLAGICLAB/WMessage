//! R4 L1 候选层 · 冲突检测 + 排序
//! **实验态（B4-5 登记）**：S0 观察态设计（OBSERVATION_STATUS §2/§3）——当前无生产调用
//!（候选池暂不做冲突淘汰），等真数据后接线；接口按 spec 冻结。
//!
//! 批次 C（2026-10-08）：消解/排序/数值化的旧委托入口已按计划删除，逻辑唯
//! 一存活于 `crate::evolution::strategy`（trait 默认实现）；本文件保留冲突
//! 谓词（is_conflict/find_conflict，数据层语义）与回归测试（走 trait 路径）。
//!
//! spec R4：
//! - 冲突解决：同层同 target 按 impact 排序；跨层按优先级
//!
//! 同层同 target：保留 higher impact，丢弃 lower（impact_eq 时保留 older）
//! 跨层：按层级优先级（Safety > Memory > ToolSchema > Skill > PromptHint > Parameter > Code）

use super::entry::ProposalEntry;
use crate::evolution::change::EvolutionLayer;
use crate::evolution::proposal::ImpactLevel;
use crate::evolution::strategy::EvolutionPolicy;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{ProposalOrigin, ProposalTarget};
    use crate::evolution::strategy::{DefaultEvolutionPolicy, EvolutionPolicy};

    fn policy() -> DefaultEvolutionPolicy {
        DefaultEvolutionPolicy
    }

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
        let r = policy().resolve(&a, &b);
        assert_eq!(r.winner.proposal_id, "b"); // High 胜
        assert_eq!(r.loser.proposal_id, "a");
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
        let r = policy().resolve(&older, &newer);
        assert_eq!(r.winner.proposal_id, "old");
        assert_eq!(r.loser.proposal_id, "new");
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
        policy().order_entries(&mut entries);
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
        assert_eq!(
            crate::evolution::strategy::layer_priority(EvolutionLayer::Parameter),
            0
        );
        assert_eq!(
            crate::evolution::strategy::layer_priority(EvolutionLayer::Policy),
            1
        );
        assert_eq!(
            crate::evolution::strategy::layer_priority(EvolutionLayer::ToolSchema),
            2
        );
        assert_eq!(
            crate::evolution::strategy::layer_priority(EvolutionLayer::Skill),
            3
        );
        assert_eq!(
            crate::evolution::strategy::layer_priority(EvolutionLayer::PromptHint),
            4
        );
        assert_eq!(
            crate::evolution::strategy::layer_priority(EvolutionLayer::Code),
            5
        );
    }

    #[test]
    fn impact_ord_locked() {
        assert_eq!(crate::evolution::strategy::impact_ord(ImpactLevel::High), 2);
        assert_eq!(
            crate::evolution::strategy::impact_ord(ImpactLevel::Medium),
            1
        );
        assert_eq!(crate::evolution::strategy::impact_ord(ImpactLevel::Low), 0);
    }
}
