//! 决策板证据：影子判定 + 冲突标注（纯函数内核）。
//!
//! 回答决策前的两个问题：
//! 1. 采纳后这条 lesson 会不会真的挤进注入的 lesson 槽位 top-3？
//! 2. 它和池内其他提案 / 已生效变更是否瞄着同一目标？
//!
//! 影子判定是静态近似：真实注入按混合打分随 query 变化（`memory::rank`），
//! 这里按 importance 降序 top-3 模拟（importance 映射与 apply 落库同源），
//! 忽略 query 相关项——偏保守，适合当决策参考而非精确预测。
//! 零 LLM、零写库。

use serde::Serialize;
use tauri::AppHandle;

use crate::evolution::candidate::ProposalEntry;
use crate::evolution::change::{ChangeRecord, ChangeStatus};
use crate::evolution::strategy::importance_for;

/// 参与 top-3 模拟的 lesson 快照（现有条目来自 mem_items kind=lesson）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LessonSnapshot {
    pub id: String,
    pub content: String,
    pub importance: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowVerdict {
    /// 采纳后会进 lesson top-3 且榜单内容变化
    Pass,
    /// 采纳后进不了 top-3 / 与现有记忆重复
    Fail,
    /// 库内还没有任何 lesson，无对比基准
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowJudgment {
    pub verdict: ShadowVerdict,
    /// 机读原因：no_baseline / would_enter_top3 / below_top3_threshold /
    /// top3_diff_but_candidate_absent
    pub note: String,
}

/// 影子判定：现有 lesson 按 importance 降序（稳定排序）取 top-3，
/// 插入候选后重排——榜单内容变化且候选在榜 = Pass。
pub fn judge_lesson_adoption(
    candidate: &LessonSnapshot,
    existing: &[LessonSnapshot],
) -> ShadowJudgment {
    if existing.is_empty() {
        return ShadowJudgment {
            verdict: ShadowVerdict::Skipped,
            note: "no_baseline".into(),
        };
    }
    let top3_contents = |mut items: Vec<&LessonSnapshot>| -> Vec<String> {
        items.sort_by_key(|l| std::cmp::Reverse(l.importance));
        items
            .into_iter()
            .take(3)
            .map(|l| l.content.clone())
            .collect()
    };
    let before = top3_contents(existing.iter().collect());
    let after = top3_contents(existing.iter().chain(std::iter::once(candidate)).collect());
    if before == after {
        // 榜单不变 = 候选被现有记忆覆盖 / 重要性不够
        return ShadowJudgment {
            verdict: ShadowVerdict::Fail,
            note: "below_top3_threshold".into(),
        };
    }
    if after.contains(&candidate.content) {
        ShadowJudgment {
            verdict: ShadowVerdict::Pass,
            note: "would_enter_top3".into(),
        }
    } else {
        ShadowJudgment {
            verdict: ShadowVerdict::Fail,
            note: "top3_diff_but_candidate_absent".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// 池内另一条提案瞄着同一目标（谁先批谁生效）
    Pooled,
    /// 已生效变更占着同一目标（再批会互相顶替）
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRef {
    /// `pool:<proposal_id>` / `active:<change_id>`
    pub with: String,
    pub kind: ConflictKind,
}

/// 冲突判定：同层 + 同 target.tag()（target 字符串相等，精确到
/// memory_policy 名 / tool 名 / prompt section 名）。
pub fn find_conflicts(
    entry: &ProposalEntry,
    others: &[ProposalEntry],
    changes: &[ChangeRecord],
) -> Vec<ConflictRef> {
    let tag = entry.target.tag();
    let mut out = Vec::new();
    for o in others {
        if o.proposal_id != entry.proposal_id && o.layer == entry.layer && o.target.tag() == tag {
            out.push(ConflictRef {
                with: format!("pool:{}", o.proposal_id),
                kind: ConflictKind::Pooled,
            });
        }
    }
    for c in changes {
        if c.status == ChangeStatus::Active && c.layer == entry.layer && c.target.tag() == tag {
            out.push(ConflictRef {
                with: format!("active:{}", c.change_id),
                kind: ConflictKind::Active,
            });
        }
    }
    out
}

/// 单条提案的决策证据
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalEvidence {
    pub proposal_id: String,
    pub shadow: ShadowJudgment,
    pub conflicts: Vec<ConflictRef>,
}

/// 批量构建（纯函数，可单测）：候选 impact → importance 与 apply 落库同源。
pub fn build(
    proposals: &[ProposalEntry],
    changes: &[ChangeRecord],
    lessons: &[LessonSnapshot],
) -> Vec<ProposalEvidence> {
    proposals
        .iter()
        .map(|p| {
            let candidate = LessonSnapshot {
                id: p.proposal_id.clone(),
                content: p.suggestion_text.clone(),
                importance: i64::from(importance_for(p.impact)),
            };
            ProposalEvidence {
                proposal_id: p.proposal_id.clone(),
                shadow: judge_lesson_adoption(&candidate, lessons),
                conflicts: find_conflicts(p, proposals, changes),
            }
        })
        .collect()
}

/// 从 mem_items 读现有 lesson 快照（决策板判定基线）。
/// 读侧持 DB 写锁（与 memory::injection_block 同口径——store 读走同连接）。
pub fn load_lesson_snapshots<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Vec<LessonSnapshot>, String> {
    let _g = crate::db::DB_WRITE_LOCK.lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] evolution::evidence DB_WRITE_LOCK: {e:?}");
        e.into_inner()
    });
    let conn = crate::db::open_db(app)?;
    crate::memory::store::ensure_table(&conn)?;
    Ok(crate::memory::store::load_all(&conn)?
        .into_iter()
        .filter(|m| m.kind == "lesson")
        .map(|m| LessonSnapshot {
            id: m.id,
            content: m.content,
            importance: m.importance,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::EvolutionLayer;
    use crate::evolution::proposal::{ImpactLevel, ProposalTarget};

    fn lesson(id: &str, content: &str, importance: i64) -> LessonSnapshot {
        LessonSnapshot {
            id: id.into(),
            content: content.into(),
            importance,
        }
    }

    #[test]
    fn judge_skipped_when_no_baseline() {
        let c = lesson("c", "新教训", 4);
        assert_eq!(
            judge_lesson_adoption(&c, &[]),
            ShadowJudgment {
                verdict: ShadowVerdict::Skipped,
                note: "no_baseline".into()
            }
        );
    }

    #[test]
    fn judge_pass_when_high_importance_displaces_bottom() {
        let existing = vec![
            lesson("a", "甲", 3),
            lesson("b", "乙", 3),
            lesson("d", "丁", 3),
        ];
        let c = lesson("c", "丙（importance 4）", 4);
        let j = judge_lesson_adoption(&c, &existing);
        assert_eq!(j.verdict, ShadowVerdict::Pass);
        assert_eq!(j.note, "would_enter_top3");
    }

    #[test]
    fn judge_fail_when_pool_full_of_equal_importance() {
        // 稳定排序下同分候选排最后：top-3 满员同分 → 挤不进
        let existing = vec![
            lesson("a", "甲", 4),
            lesson("b", "乙", 4),
            lesson("d", "丁", 4),
        ];
        let c = lesson("c", "丙（同分）", 4);
        let j = judge_lesson_adoption(&c, &existing);
        assert_eq!(j.verdict, ShadowVerdict::Fail);
        assert_eq!(j.note, "below_top3_threshold");
    }

    #[test]
    fn judge_pass_when_duplicate_content_fills_open_slot() {
        // 槽位未满（<3 条）：重复内容也会占一个槽 → 字面判定 Pass；
        // 内容级去重是 apply 落库时的职责（store dedup），不归影子判定管
        let existing = vec![lesson("a", "甲", 3), lesson("b", "乙", 3)];
        let c = lesson("c", "甲", 4);
        let j = judge_lesson_adoption(&c, &existing);
        assert_eq!(j.verdict, ShadowVerdict::Pass);
        assert_eq!(j.note, "would_enter_top3");
    }

    #[test]
    fn judge_fail_when_full_house_of_higher_importance() {
        let existing = vec![
            lesson("a", "甲", 5),
            lesson("b", "乙", 5),
            lesson("d", "丁", 5),
        ];
        let c = lesson("c", "丙", 3);
        let j = judge_lesson_adoption(&c, &existing);
        assert_eq!(j.verdict, ShadowVerdict::Fail);
        assert_eq!(j.note, "below_top3_threshold");
    }

    #[test]
    fn judge_pass_when_slots_available() {
        let existing = vec![lesson("a", "甲", 4)];
        let c = lesson("c", "丙", 3);
        let j = judge_lesson_adoption(&c, &existing);
        assert_eq!(j.verdict, ShadowVerdict::Pass);
    }

    fn entry(id: &str, layer: EvolutionLayer, policy: &str) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer,
            impact: crate::evolution::proposal::ImpactLevel::Medium,
            origin: crate::evolution::proposal::ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: policy.into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("sum-{id}"),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: 1_000,
            expires_at_ms: 2_000,
            status: crate::evolution::candidate::ProposalStatus::Pooled,
        }
    }

    fn change(id: &str, layer: EvolutionLayer, policy: &str, status: ChangeStatus) -> ChangeRecord {
        use crate::evolution::proposal::{Evidence, ProposalCategory, ProposalOrigin, Suggestion};
        let p = crate::evolution::proposal::EvolutionProposal {
            proposal_id: id.into(),
            created_at_ms: 1_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: ProposalCategory::MemoryHint,
            target: ProposalTarget::MemoryPolicy {
                policy: policy.into(),
            },
            impact: crate::evolution::proposal::ImpactLevel::Medium,
            evidence: Evidence {
                summary: "s".into(),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: "t".into(),
                structured_patch: None,
            },
        };
        let mut cr = crate::evolution::change::from_proposal(&p, 1_000);
        cr.layer = layer;
        cr.status = status;
        cr
    }

    #[test]
    fn conflicts_detect_pooled_and_active_same_target() {
        let me = entry("me", EvolutionLayer::Policy, "dup-rule");
        let other_pool = vec![
            entry("me", EvolutionLayer::Policy, "dup-rule"), // 自身：不算
            entry("a", EvolutionLayer::Policy, "dup-rule"),  // 同层同 target
            entry("b", EvolutionLayer::Policy, "other-rule"), // 同层不同 target
            entry("c", EvolutionLayer::PromptHint, "dup-rule"), // 不同层同 target
        ];
        let changes = vec![
            change(
                "chg-1",
                EvolutionLayer::Policy,
                "dup-rule",
                ChangeStatus::Active,
            ),
            change(
                "chg-2",
                EvolutionLayer::Policy,
                "dup-rule",
                ChangeStatus::RolledBack,
            ),
        ];
        let got = find_conflicts(&me, &other_pool, &changes);
        assert_eq!(
            got,
            vec![
                ConflictRef {
                    with: "pool:a".into(),
                    kind: ConflictKind::Pooled
                },
                ConflictRef {
                    with: "active:chg-chg-1".into(), // change_id = "chg-" + proposal_id 派生
                    kind: ConflictKind::Active
                },
            ]
        );
    }

    #[test]
    fn build_maps_impact_importance_and_attaches_everything() {
        let mut p = entry("p1", EvolutionLayer::Policy, "r");
        p.impact = ImpactLevel::High;
        let lessons = vec![lesson("x", "旧", 3)];
        let ev = build(std::slice::from_ref(&p), &[], &lessons);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].proposal_id, "p1");
        assert_eq!(ev[0].shadow.verdict, ShadowVerdict::Pass); // High=4 挤掉 3
        assert!(ev[0].conflicts.is_empty());
    }
}
