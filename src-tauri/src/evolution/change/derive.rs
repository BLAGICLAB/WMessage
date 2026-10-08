//!  L2 版本层 · 从 EvolutionProposal 派生 ChangeRecord
//!
//! 派生规则（按 DERIVABILITY.md）：
//! - `change_id`           = `"chg-" + proposal_id`
//! - `schema_version`      = 1（默认；R2+ bump）
//! - `hard_constraint_compliance` = category+impact 判定（仅约束 5/6，
//!   见 strategy::gate 文档；ChangeRecord 字段名=jsonl schema 不动）
//! - `layer`               = 从 category 派生（4/6 层覆盖；Parameter/Code 不在当前 category）
//! - `mem_key`             = `"evo:" + proposal_id`
//!
//! 不可派生（落 jsonl）：status, parent_id, eval_before/eval_after,
//! rolled_back_at, rollback_reason, approval_source, human_approver

use super::record::{ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
use crate::evolution::proposal::{EvolutionProposal, ProposalCategory};

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
/// layer 从 category 派生（DERIVABILITY.md 字段 1，部分覆盖）
///
/// 4/6 层映射；Parameter / Code 两层当前无 category 对应，
/// / 触发后再扩展。
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
    // 判定直连策略层自由函数
    let compliance = matches!(
        crate::evolution::strategy::gate_decision(p),
        crate::evolution::strategy::GateDecision::Approved
    );
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

/// 行级唯一 change_id——基础 `chg-<pid>` 空闲则直接用；被占
/// （上次启用已回滚/已过期）则 `-2`、`-3` 递增。同 id 双行是「二次回滚永久
/// 卡死」的根因（rollback 按 id position() 首匹配）。三家写者（panel toggle /
/// shadow / apply CR）落行前统一经此派生。
pub fn unique_change_id_for(rows: &[ChangeRecord], proposal_id: &str) -> String {
    let base = derive_change_id(proposal_id);
    let taken = |id: &str| rows.iter().any(|c| c.change_id == id);
    if !taken(&base) {
        return base;
    }
    for n in 2..=9999 {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    // 理论不可达兜底：时间戳后缀保证唯一
    format!("{base}-{}", chrono::Utc::now().timestamp_millis())
}

/// 自动应用路径的 CR 构造（生产 apply 与回归测试共用，锁「Active +
/// AutoApplied 经合法流转达成」这一不变量）。
///
/// 自动应用（MemoryHint lesson 直写记忆）不经沙箱/影子——那是整改流水线的
/// 门，不是 lesson 路径的门——但 CR 形态仍走满合法流转
///（Pending → Shadowing → ShadowPassed → Approved → Active，中间态瞬时通过），
/// 不裸写 status 绕状态机（Pending → Active 直跳被 status.rs 硬约束②显式拦截，
/// 评审 HIGH）。入口提案须已过策略层 gate（compliance=false 的
/// from_proposal 是终态 Rejected，流转即 Err）。
pub fn auto_applied_from_proposal(
    p: &EvolutionProposal,
    now_ms: i64,
) -> Result<ChangeRecord, String> {
    let mut cr = from_proposal(p, now_ms);
    use ChangeStatus::{Active, Approved, ShadowPassed, Shadowing};
    for step in [Shadowing, ShadowPassed, Approved, Active] {
        super::status::transition(cr.status, step)?;
        cr.status = step;
    }
    cr.approval_source = ApprovalSource::AutoApplied;
    cr.hard_constraint_compliance = true;
    Ok(cr)
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

    // change_id 派生

    #[test]
    fn change_id_format_locked() {
        // 锁死格式：chg- + proposal_id
        assert_eq!(derive_change_id("abc123"), "chg-abc123");
        assert_eq!(derive_mem_key("abc123"), "evo:abc123");
    }

    // 自动应用门槛检查

    #[test]
    fn gate_true_for_memory_high() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        assert!(matches!(
            crate::evolution::strategy::gate_decision(&p),
            crate::evolution::strategy::GateDecision::Approved
        ));
    }

    #[test]
    fn gate_true_for_memory_medium() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Medium);
        assert!(matches!(
            crate::evolution::strategy::gate_decision(&p),
            crate::evolution::strategy::GateDecision::Approved
        ));
    }

    #[test]
    fn gate_false_for_memory_low() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Low);
        assert!(!matches!(
            crate::evolution::strategy::gate_decision(&p),
            crate::evolution::strategy::GateDecision::Approved
        ));
    }

    #[test]
    fn gate_false_for_non_memory_hint() {
        for cat in [
            ProposalCategory::PromptHint,
            ProposalCategory::ToolSchemaHint,
            ProposalCategory::SkillHint,
        ] {
            let p = mk_proposal("p1", cat, ImpactLevel::High);
            assert!(
                !matches!(
                    crate::evolution::strategy::gate_decision(&p),
                    crate::evolution::strategy::GateDecision::Approved
                ),
                "{cat:?} 不应合规"
            );
        }
    }

    // layer 派生（4/6 层覆盖）

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

    // from_proposal 集成

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
