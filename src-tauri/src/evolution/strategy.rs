//! Evolution 策略层：gate 判定 + importance 映射（自由函数）。
//!
//! 分层契约（docs/EVOLUTION-LAYERING-BATCH-A.md）：策略实现必须**纯**——
//! 不读文件、不读时钟、不持锁、无随机源。gate 的 `Rejected` 是正常业务
//! 结果，不进 Err 通道。
//!
//! 原单实现 trait（EvolutionPolicy/DefaultEvolutionPolicy）与零生产调用的
//! resolve/order_entries 消解排序件已删——第二个策略实现出现时再抽象
//! （历史实现见 git log）。

use crate::evolution::proposal::{EvolutionProposal, ImpactLevel, ProposalCategory};
use crate::evolution::sandbox::kill_switch::KillSwitch;
use serde::{Deserialize, Serialize};

/// 应用策略二档（决策词汇归策略层所有；policy.rs 负责配置 IO 与转发）。
///
/// serde 小写序列化（"auto"/"confirm"）与 as_str()/配置键口径一致——
/// `evolution_get_apply_policy` 直接把本枚举过 Tauri 边界，前端拿到的就是
/// 这两个小写字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPolicy {
    /// 自动生效（默认；缺字段/非法值同此档）
    Auto,
    /// 需确认：达门槛提案只进候选池，等决策板人工批准
    Confirm,
}

impl ApplyPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Confirm => "confirm",
        }
    }

    /// 配置字符串解析：仅认 "auto" / "confirm"，其余（含缺字段）按 Auto。
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "confirm" => Some(Self::Confirm),
            _ => None,
        }
    }
}

/// 运行时上下文只读快照：每个 evolution 周期在**决策点**构造一次，
/// 构造后不可变；策略纯函数不消费本结构，它供编排侧做分流判定
/// （applyPolicy 分流 / kill 中断 / shadow 观察）。
/// 不含随机源——全域禁随机（不变式 7），未来引入必须加字段由调用方注入。
#[derive(Debug, Clone)]
pub struct EvalContext {
    /// applyPolicy 档位快照（缺省/读失败 = Auto，与现状一致）
    pub apply_policy: ApplyPolicy,
    /// kill_switch 只读快照（读取失败 = 全关默认，与现状一致）
    pub kill_switch: KillSwitch,
    /// shadow 观察是否开启
    pub shadow_enabled: bool,
    /// 本周期基准时间
    pub now_ms: i64,
}

/// gate 判定结果。Reject 是正常业务结果，不进 Err 通道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// 够格进自动轨（是否真自动落库仍由调用方按 applyPolicy 分流）
    Approved,
    /// 不过 gate，进候选池或丢弃；reason 供审计与测试断言指认
    Rejected { reason: &'static str },
}

/// gate：proposal 是否够格自动轨。白名单形式——未来新增 impact 变体
/// 必须显式 opt-in（沿 change/derive.rs 注释明示的设计）。
pub fn gate_decision(p: &EvolutionProposal) -> GateDecision {
    if !matches!(p.category, ProposalCategory::MemoryHint) {
        return GateDecision::Rejected {
            reason: "非记忆类提案（仅 MemoryHint 进自动轨）",
        };
    }
    if !matches!(p.impact, ImpactLevel::High | ImpactLevel::Medium) {
        return GateDecision::Rejected {
            reason: "影响面不足（Low 不进自动轨）",
        };
    }
    GateDecision::Approved
}

/// importance 映射（写 lesson 记忆时落 mem_items.importance）。
/// 数值契约 1..=5：输出恒在界内；越界防御由 store 层 clamp 兜底。
pub fn importance_for(impact: ImpactLevel) -> u8 {
    // apply_one 迁移前内联映射原样：High=4，其余=3
    match impact {
        ImpactLevel::High => 4,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{Evidence, ProposalOrigin, ProposalTarget, Suggestion};

    /// EvolutionProposal 夹具（仅 category/impact 参与判定，其余取占位）
    fn mk_proposal(category: ProposalCategory, impact: ImpactLevel) -> EvolutionProposal {
        EvolutionProposal {
            proposal_id: format!("p-{category:?}-{impact:?}"),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category,
            target: ProposalTarget::MemoryPolicy {
                policy: "strategy-test".into(),
            },
            impact,
            evidence: Evidence {
                summary: "gate 真值表".into(),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: "gate 真值表文本".into(),
                structured_patch: None,
            },
        }
    }

    /// gate 真值表：3 impact × 4 category 全组合
    #[test]
    fn gate_truth_table_all_impact_category_combinations() {
        let impacts = [ImpactLevel::Low, ImpactLevel::Medium, ImpactLevel::High];
        let categories = [
            ProposalCategory::MemoryHint,
            ProposalCategory::PromptHint,
            ProposalCategory::ToolSchemaHint,
            ProposalCategory::SkillHint,
        ];
        for impact in impacts {
            for category in categories {
                let p = mk_proposal(category, impact);
                let expected = category == ProposalCategory::MemoryHint
                    && matches!(impact, ImpactLevel::High | ImpactLevel::Medium);
                assert_eq!(
                    matches!(gate_decision(&p), GateDecision::Approved),
                    expected,
                    "{impact:?}×{category:?} 判定漂移"
                );
            }
        }
    }

    #[test]
    fn gate_reject_reasons_are_actionable() {
        let low = mk_proposal(ProposalCategory::MemoryHint, ImpactLevel::Low);
        let offcategory = mk_proposal(ProposalCategory::PromptHint, ImpactLevel::High);
        match gate_decision(&low) {
            GateDecision::Rejected { reason } => assert!(reason.contains("Low"), "{reason}"),
            GateDecision::Approved => panic!("Low 应拒"),
        }
        match gate_decision(&offcategory) {
            GateDecision::Rejected { reason } => {
                assert!(reason.contains("MemoryHint"), "{reason}")
            }
            GateDecision::Approved => panic!("非 MemoryHint 应拒"),
        }
    }

    #[test]
    fn importance_maps_high_to_4_others_to_3() {
        // apply_one 迁移前内联映射原样；数值契约 1..=5 由 store 层 clamp 兜底
        assert_eq!(importance_for(ImpactLevel::High), 4);
        assert_eq!(importance_for(ImpactLevel::Medium), 3);
        assert_eq!(importance_for(ImpactLevel::Low), 3);
    }
}
