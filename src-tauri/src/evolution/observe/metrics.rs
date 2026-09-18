//! R6 L4 观察层 · 4 个核心指标（spec R6）
//!
//! 4 个指标：
//! 1. 候选生成率 candidate_generation_rate = proposals_in_window / days_in_window
//! 2. 通过率 approval_rate = promoted_count / proposal_total
//! 3. 回滚率 rollback_rate = rolled_back_count / promoted_count
//! 4. 污染存活期 pollution_survival_days = avg(now - applied_at_ms) for live lessons
//!
//! 纯函数计算；不调 LLM；只读 evolution-proposals.jsonl + evolution-changes.jsonl + evolution-applied.jsonl。

use serde::{Deserialize, Serialize};

use crate::evolution::candidate::ProposalEntry;
use crate::evolution::candidate::ProposalStatus;
use crate::evolution::change::ChangeRecord;
use crate::evolution::change::ChangeStatus;
use crate::eval::metrics::AppliedRecord;

/// R6 4 个核心指标（一轮观察的输出）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ObserveMetrics {
    // 主要 4 个指标
    /// 候选生成率（条/天）
    pub candidate_generation_rate: f64,
    /// 通过率（promoted / total）
    pub approval_rate: f64,
    /// 回滚率（rolled_back / promoted）
    pub rollback_rate: f64,
    /// 污染存活期（天）
    pub pollution_survival_days: f64,

    // 辅助数据（用于报告和调试）
    pub proposal_total: usize,
    pub promoted_count: usize,
    pub rolled_back_count: usize,
    pub active_lessons: usize,
    pub observation_window_days: f64,
    pub observation_window_start_ms: i64,
    pub observation_window_end_ms: i64,
    pub evaluated_at_ms: i64,
}

/// 主入口：聚合计算 4 个指标
///
/// 入参：
/// - `proposals`：evolution-proposals.jsonl 内容（全部）
/// - `changes`：evolution-changes.jsonl 内容（全部）
/// - `applied`：evolution-applied.jsonl 内容（全部）
/// - `now_ms`：评估时刻
/// - `window_start_ms`：观察窗口起点
pub fn compute(
    proposals: &[ProposalEntry],
    changes: &[ChangeRecord],
    applied: &[AppliedRecord],
    now_ms: i64,
    window_start_ms: i64,
) -> ObserveMetrics {
    let proposal_total = proposals.len();

    // 窗口内候选数（created_at_ms >= window_start_ms && < now_ms）
    let proposals_in_window: usize = proposals
        .iter()
        .filter(|p| p.created_at_ms >= window_start_ms && p.created_at_ms <= now_ms)
        .count();

    let window_days = ((now_ms - window_start_ms) as f64 / 86_400_000.0).max(1.0);

    // 通过：proposal.status == Promoted
    let promoted_count = proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Promoted)
        .count();

    // 回滚：ChangeRecord.status == RolledBack
    let rolled_back_count = changes
        .iter()
        .filter(|c| c.status == ChangeStatus::RolledBack)
        .count();

    // 当前 active 的 ChangeRecord 数（仍生效）
    let active_changes = changes
        .iter()
        .filter(|c| c.status == ChangeStatus::Active)
        .count();

    // 污染存活期：active ChangeRecord 对应的 applied_at 在 (now - applied_at) 上平均
    let mut survival_total_days = 0.0;
    let mut survival_n = 0u64;
    for change in changes.iter().filter(|c| c.status == ChangeStatus::Active) {
        // 查 applied.jsonl：找 mem_key 对应的 applied_at_ms
        if let Some(app) = applied.iter().find(|a| a.mem_key == change.mem_key) {
            let days = (now_ms - app.applied_at_ms) as f64 / 86_400_000.0;
            if days >= 0.0 {
                survival_total_days += days;
                survival_n += 1;
            }
        }
    }
    let pollution_survival_days = if survival_n == 0 {
        0.0
    } else {
        survival_total_days / survival_n as f64
    };

    let candidate_generation_rate = proposals_in_window as f64 / window_days;
    let approval_rate = if proposal_total == 0 {
        0.0
    } else {
        promoted_count as f64 / proposal_total as f64
    };
    let rollback_rate = if promoted_count == 0 {
        0.0
    } else {
        rolled_back_count as f64 / promoted_count as f64
    };

    ObserveMetrics {
        candidate_generation_rate,
        approval_rate,
        rollback_rate,
        pollution_survival_days,
        proposal_total,
        promoted_count,
        rolled_back_count,
        active_lessons: active_changes,
        observation_window_days: window_days,
        observation_window_start_ms: window_start_ms,
        observation_window_end_ms: now_ms,
        evaluated_at_ms: now_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::candidate::{ProposalEntry, ProposalStatus};
    use crate::evolution::change::{
        ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer,
    };
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};
    use crate::eval::metrics::AppliedRecord;

    fn mk_proposal(
        id: &str,
        status: ProposalStatus,
        created_at_ms: i64,
    ) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer: EvolutionLayer::Policy,
            impact: ImpactLevel::Medium,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy { policy: "test".into() },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("s-{id}"),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms,
            expires_at_ms: created_at_ms + 86_400_000 * 14,
            status,
        }
    }

    fn mk_change(
        id: &str,
        status: ChangeStatus,
        proposal_id: &str,
    ) -> ChangeRecord {
        ChangeRecord {
            change_id: id.into(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: proposal_id.into(),
            target: ProposalTarget::MemoryPolicy { policy: "test".into() },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{proposal_id}"),
            impact: ImpactLevel::Medium,
            eval_before: None,
            eval_after: None,
            status,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::AutoApplied,
            human_approver: None,
            created_at_ms: 1_700_000_000_000,
            rolled_back_at: None,
            rollback_reason: None,
        }
    }

    fn mk_applied(proposal_id: &str, applied_at_ms: i64) -> AppliedRecord {
        AppliedRecord {
            proposal_id: proposal_id.into(),
            mem_key: format!("evo:{proposal_id}"),
            applied_at_ms,
            impact: "medium".into(),
            summary: format!("s-{proposal_id}"),
        }
    }

    // ─── 零数据场景 ───

    #[test]
    fn zero_proposals_zero_rates() {
        let r = compute(&[], &[], &[], 1000, 0);
        assert_eq!(r.candidate_generation_rate, 0.0);
        assert_eq!(r.approval_rate, 0.0);
        assert_eq!(r.rollback_rate, 0.0);
        assert_eq!(r.pollution_survival_days, 0.0);
        assert_eq!(r.proposal_total, 0);
    }

    // ─── 候选生成率 ───

    #[test]
    fn candidate_generation_rate_per_day() {
        // 30 天窗口，10 条候选 → 0.33 条/天
        let now_ms = 86_400_000 * 30;
        let proposals: Vec<_> = (0..10)
            .map(|i| mk_proposal(&format!("p{i}"), ProposalStatus::Pooled, i * 86_400_000))
            .collect();
        let r = compute(&proposals, &[], &[], now_ms, 0);
        assert!((r.candidate_generation_rate - 10.0 / 30.0).abs() < 1e-9);
    }

    #[test]
    fn candidate_generation_rate_window_filter() {
        // 100 条：50 条在窗口内 + 50 条在窗口外
        let now_ms = 86_400_000 * 100;
        let window_start_ms = 86_400_000 * 50;
        let mut proposals = Vec::new();
        for i in 0..50 {
            proposals.push(mk_proposal(&format!("p{i}"), ProposalStatus::Pooled, i * 86_400_000));
        }
        for i in 50..100 {
            proposals.push(mk_proposal(&format!("q{i}"), ProposalStatus::Pooled, i * 86_400_000));
        }
        let r = compute(&proposals, &[], &[], now_ms, window_start_ms);
        assert_eq!(r.proposal_total, 100);
        assert!((r.candidate_generation_rate - 50.0 / 50.0).abs() < 1e-9); // 50 条 / 50 天 = 1
    }

    // ─── 通过率 ───

    #[test]
    fn approval_rate_fraction() {
        // 10 条：6 promoted + 4 pooled → 60%
        let mut proposals = Vec::new();
        for i in 0..6 {
            proposals.push(mk_proposal(&format!("promo{i}"), ProposalStatus::Promoted, i * 1000));
        }
        for i in 0..4 {
            proposals.push(mk_proposal(&format!("pool{i}"), ProposalStatus::Pooled, i * 1000));
        }
        let r = compute(&proposals, &[], &[], 100_000, 0);
        assert!((r.approval_rate - 0.6).abs() < 1e-9);
    }

    // ─── 回滚率 ───

    #[test]
    fn rollback_rate_per_promoted() {
        // 6 promoted：3 仍在 active，3 已 rolled back → 50% 回滚率
        let mut proposals = Vec::new();
        let mut changes = Vec::new();
        for i in 0..3 {
            let id = format!("active{i}");
            proposals.push(mk_proposal(&id, ProposalStatus::Promoted, i * 1000));
            changes.push(mk_change(&format!("chg-{id}"), ChangeStatus::Active, &id));
        }
        for i in 0..3 {
            let id = format!("rolled{i}");
            proposals.push(mk_proposal(&id, ProposalStatus::Promoted, i * 1000));
            changes.push(mk_change(&format!("chg-{id}"), ChangeStatus::RolledBack, &id));
        }
        let r = compute(&proposals, &changes, &[], 100_000, 0);
        assert!((r.rollback_rate - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rollback_rate_zero_when_no_promoted() {
        // 0 promoted → 0% 回滚率（不抛除零错）
        let proposals = vec![mk_proposal("p1", ProposalStatus::Pooled, 1000)];
        let r = compute(&proposals, &[], &[], 100_000, 0);
        assert_eq!(r.rollback_rate, 0.0);
    }

    // ─── 污染存活期 ───

    #[test]
    fn pollution_survival_days_avg() {
        // 3 active lessons：分别存活 5/10/15 天 → 均值 10 天
        let day_ms = 86_400_000;
        let now_ms = day_ms * 30;
        let mut proposals = Vec::new();
        let mut changes = Vec::new();
        let mut applied = Vec::new();
        for (i, days_alive) in [(0, 5), (1, 10), (2, 15)] {
            let id = format!("p{i}");
            proposals.push(mk_proposal(&id, ProposalStatus::Promoted, i * day_ms));
            changes.push(mk_change(&format!("chg-{id}"), ChangeStatus::Active, &id));
            applied.push(mk_applied(&id, now_ms - days_alive * day_ms));
        }
        let r = compute(&proposals, &changes, &applied, now_ms, 0);
        assert!((r.pollution_survival_days - 10.0).abs() < 1e-9);
    }

    #[test]
    fn pollution_survival_days_skips_rolled_back() {
        // 2 lessons：1 active + 1 rolled_back → 只算 active 的
        let day_ms = 86_400_000;
        let now_ms = day_ms * 30;
        let proposals = vec![
            mk_proposal("active1", ProposalStatus::Promoted, 0),
            mk_proposal("rolled1", ProposalStatus::Promoted, 0),
        ];
        let changes = vec![
            mk_change("chg-active1", ChangeStatus::Active, "active1"),
            mk_change("chg-rolled1", ChangeStatus::RolledBack, "rolled1"),
        ];
        let applied = vec![
            mk_applied("active1", now_ms - 7 * day_ms),
            mk_applied("rolled1", now_ms - 100 * day_ms), // 100 天前，不计入
        ];
        let r = compute(&proposals, &changes, &applied, now_ms, 0);
        assert!((r.pollution_survival_days - 7.0).abs() < 1e-9);
    }

    #[test]
    fn pollution_survival_zero_when_no_active() {
        let proposals = vec![mk_proposal("p1", ProposalStatus::Pooled, 1000)];
        let r = compute(&proposals, &[], &[], 100_000, 0);
        assert_eq!(r.pollution_survival_days, 0.0);
    }
}