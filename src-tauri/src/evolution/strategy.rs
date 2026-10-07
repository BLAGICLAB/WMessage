//! Evolution 策略层：gate 判定 / 冲突消解 / 跨层排序的可替换实现。
//!
//! 分层契约（docs/EVOLUTION-LAYERING-BATCH-A.md）：
//! - 实现必须**纯**：不得读文件、读时钟、持锁、用随机源；一切环境输入经
//!   `LedgerView` / `EvalContext` 注入（本批尚未引入 EvalContext，签名为纯
//!   参数形态，后续收拢时不改 trait 语义）。
//! - gate 的 `Rejected` 是正常业务结果，不进 Err 通道。
//! - 默认实现 = 迁移前行为逐函数搬运，旧函数零逻辑委托（等价对照测试钉死）。
//!
//! 现状说明：本模块承接的消解/排序函数当前无生产调用方（候选池冲突淘汰
//! 待真数据后接线，见 conflict.rs 模块头），迁移零回归面。

use crate::evolution::candidate::entry::ProposalEntry;
use crate::evolution::change::record::EvolutionLayer;
use crate::evolution::proposal::{EvolutionProposal, ImpactLevel, ProposalCategory};

/// gate 判定结果。Reject 是正常业务结果，不进 Err 通道。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// 够格进自动轨（是否真自动落库仍由调用方按 applyPolicy 分流——后续
    /// EvalContext 引入后由 ctx 判定）
    Approved,
    /// 不过 gate，进候选池或丢弃；reason 供审计与测试断言指认
    Rejected { reason: &'static str },
}

/// 冲突消解结果：胜者保留、败者丢弃
/// （仅 PartialEq：ProposalEntry 未实现 Eq，断言按 proposal_id 比较）
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionResult<'a> {
    pub winner: &'a ProposalEntry,
    pub loser: &'a ProposalEntry,
}

/// 策略层 trait。Send + Sync：现状自由函数已被 spawn_blocking / 命令线程
/// 并发触达（批次 A §9-P7 拍板），不写死会阻塞后续并发模型演进。
pub trait EvolutionPolicy: Send + Sync {
    /// gate：proposal 是否够格自动轨。白名单形式——未来新增 impact 变体
    /// 必须显式 opt-in（沿 change/derive.rs 注释明示的设计）。
    fn gate(&self, p: &EvolutionProposal) -> GateDecision;

    /// 冲突消解：impact 高者胜，相同则 older（created_at_ms 小）胜
    fn resolve<'a>(&self, a: &'a ProposalEntry, b: &'a ProposalEntry) -> SelectionResult<'a>;

    /// 跨层排序：layer 优先级升序 → impact 降序 → created_at 升序
    fn order_entries(&self, entries: &mut [ProposalEntry]);

    /// importance 映射（写 lesson 记忆时落 mem_items.importance）。
    /// 数值契约 1..=5：默认实现输出恒在界内；越界防御由 store 层 clamp 兜底。
    fn importance(&self, impact: ImpactLevel) -> u8;
}

/// 默认实现：迁移前行为逐函数搬运，一行逻辑不改（等价对照测试钉死）。
pub struct DefaultEvolutionPolicy;

/// 跨层优先级（数字越小优先级越高）——自 conflict.rs 原样搬入
/// （pub(crate)：conflict.rs 旧公开入口零逻辑委托的目标）
pub(crate) fn layer_priority(layer: EvolutionLayer) -> u32 {
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

/// impact 数值化（用于排序）——自 conflict.rs 原样搬入
/// （pub(crate)：conflict.rs 旧公开入口零逻辑委托的目标）
pub(crate) fn impact_ord(impact: ImpactLevel) -> u32 {
    match impact {
        ImpactLevel::High => 2,
        ImpactLevel::Medium => 1,
        ImpactLevel::Low => 0,
    }
}

impl EvolutionPolicy for DefaultEvolutionPolicy {
    fn gate(&self, p: &EvolutionProposal) -> GateDecision {
        // 白名单形式（passes_auto_apply_gate 原语义）：与旧 `!= Low` 在当前
        // 3 变体下逐输入等价；未来新增 impact 变体须显式 opt-in
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

    fn resolve<'a>(&self, a: &'a ProposalEntry, b: &'a ProposalEntry) -> SelectionResult<'a> {
        let (winner, loser) = if impact_ord(a.impact) != impact_ord(b.impact) {
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
        };
        SelectionResult { winner, loser }
    }

    fn order_entries(&self, entries: &mut [ProposalEntry]) {
        entries.sort_by(|a, b| {
            layer_priority(a.layer)
                .cmp(&layer_priority(b.layer))
                .then(impact_ord(b.impact).cmp(&impact_ord(a.impact)))
                .then(a.created_at_ms.cmp(&b.created_at_ms))
        });
    }

    fn importance(&self, impact: ImpactLevel) -> u8 {
        // 迁移前 apply_one 内联映射原样：High=4，其余=3
        match impact {
            ImpactLevel::High => 4,
            _ => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::candidate::entry::ProposalStatus;
    use crate::evolution::proposal::{Evidence, ProposalOrigin, ProposalTarget, Suggestion};

    /// ProposalEntry 夹具（与 conflict.rs 测试同形，跨模块不共享以保持
    /// 各文件测试自包含）
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
            status: ProposalStatus::Pooled,
        }
    }

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
                summary: "gate 等价对照".into(),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: "gate 等价对照文本".into(),
                structured_patch: None,
            },
        }
    }

    // ─── gate：12 例等价穷举（3 impact × 4 category，全真值表超集）───
    // 旧语义字面保留作对照基准（迁移前两实现 + 新 trait 路径三方一致）；
    // 批次 C 删除旧入口时一并清理。

    #[test]
    fn gate_equivalence_all_impact_category_combinations() {
        let impacts = [ImpactLevel::Low, ImpactLevel::Medium, ImpactLevel::High];
        let categories = [
            ProposalCategory::MemoryHint,
            ProposalCategory::PromptHint,
            ProposalCategory::ToolSchemaHint,
            ProposalCategory::SkillHint,
        ];
        let policy = DefaultEvolutionPolicy;
        for impact in impacts {
            for category in categories {
                let p = mk_proposal(category, impact);
                // 旧实现 1（apply.rs 迁移前原文）
                let old_auto =
                    p.category == ProposalCategory::MemoryHint && p.impact != ImpactLevel::Low;
                // 旧实现 2（change/derive.rs 迁移前原文）
                let old_passes = matches!(p.category, ProposalCategory::MemoryHint)
                    && matches!(p.impact, ImpactLevel::High | ImpactLevel::Medium);
                // 新路径
                let new_gate = matches!(policy.gate(&p), GateDecision::Approved);
                // 三方一致：两旧实现彼此等价，且与 trait 路径等价
                assert_eq!(
                    old_auto, old_passes,
                    "两旧实现不等价：{impact:?}×{category:?}"
                );
                assert_eq!(old_passes, new_gate, "新路径漂移：{impact:?}×{category:?}");
            }
        }
    }

    #[test]
    fn gate_reject_reasons_are_actionable() {
        let policy = DefaultEvolutionPolicy;
        let low = mk_proposal(ProposalCategory::MemoryHint, ImpactLevel::Low);
        let offcategory = mk_proposal(ProposalCategory::PromptHint, ImpactLevel::High);
        match policy.gate(&low) {
            GateDecision::Rejected { reason } => assert!(reason.contains("Low"), "{reason}"),
            GateDecision::Approved => panic!("Low 应拒"),
        }
        match policy.gate(&offcategory) {
            GateDecision::Rejected { reason } => assert!(reason.contains("MemoryHint"), "{reason}"),
            GateDecision::Approved => panic!("非 MemoryHint 应拒"),
        }
    }

    #[test]
    fn importance_maps_high_to_4_others_to_3() {
        // 迁移前 apply_one 内联映射（High=4 / 其余=3）原样收拢进 trait；
        // 数值契约 1..=5 由 store 层 clamp 兜底（批次 A §4.3）
        let policy = DefaultEvolutionPolicy;
        assert_eq!(policy.importance(ImpactLevel::High), 4);
        assert_eq!(policy.importance(ImpactLevel::Medium), 3);
        assert_eq!(policy.importance(ImpactLevel::Low), 3);
    }

    // ─── 消解/排序：新旧路径快照对照（serde_json 字符串相等 + 字面量预期）───

    #[test]
    fn snapshot_resolve_and_order_old_new_paths_identical() {
        let policy = DefaultEvolutionPolicy;
        let a = mk(
            "a",
            EvolutionLayer::Policy,
            ImpactLevel::Medium,
            "p1".into(),
            2000,
        );
        let b = mk(
            "b",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            1000,
        );
        let c = mk(
            "c",
            EvolutionLayer::Parameter,
            ImpactLevel::Low,
            "p2".into(),
            3000,
        );

        // 旧路径（conflict.rs 公开入口，零逻辑委托——委托目标即本 trait 实现）
        let mut old_sorted = vec![b.clone(), c.clone(), a.clone()];
        super::super::candidate::conflict::sort_entries_cross_layer(&mut old_sorted);
        let old_pair = super::super::candidate::conflict::resolve_conflict(&a, &b);

        // 新路径（trait 直调）
        let mut new_sorted = vec![b.clone(), c.clone(), a.clone()];
        policy.order_entries(&mut new_sorted);
        let new_pair = policy.resolve(&a, &b);

        let old_json = serde_json::to_string(&serde_json::json!({
            "order": old_sorted.iter().map(|e| e.proposal_id.clone()).collect::<Vec<_>>(),
            "winner": old_pair.0.proposal_id,
            "loser": old_pair.1.proposal_id,
        }))
        .unwrap();
        let new_json = serde_json::to_string(&serde_json::json!({
            "order": new_sorted.iter().map(|e| e.proposal_id.clone()).collect::<Vec<_>>(),
            "winner": new_pair.winner.proposal_id,
            "loser": new_pair.loser.proposal_id,
        }))
        .unwrap();
        assert_eq!(old_json, new_json);

        // 字面量预期（fixture 定值）：c(Parameter/Low) 先、b(High) 次、a(Medium)
        // 后；b impact 高于 a → b 胜 a 败
        assert_eq!(
            old_json,
            r#"{"loser":"a","order":["c","b","a"],"winner":"b"}"#
        );
    }

    #[test]
    fn order_stable_for_equal_keys() {
        let policy = DefaultEvolutionPolicy;
        // 同层同 impact 同 created_at：sort_by 稳定排序保原相对序
        let x1 = mk(
            "x1",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            1000,
        );
        let x2 = mk(
            "x2",
            EvolutionLayer::Policy,
            ImpactLevel::High,
            "p1".into(),
            1000,
        );
        let mut v = vec![x2.clone(), x1.clone()];
        policy.order_entries(&mut v);
        let ids: Vec<&str> = v.iter().map(|e| e.proposal_id.as_str()).collect();
        assert_eq!(ids, vec!["x2", "x1"], "等键条目应保持稳定排序");
    }
}
