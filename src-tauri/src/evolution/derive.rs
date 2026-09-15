//! Phase 1.3：从 consolidate 反思产出派生 `EvolutionProposal`。
//!
//! ## 启发式规则（spec Q2=1 锁定，保守阈值）
//!
//! | 输入                  | 阈值           | 产出           | 影响等级 |
//! |---------------------|--------------|--------------|------|
//! | `Merge { ids }`     | `ids.len ≥ 3` | 1 条 MemoryHint | Medium |
//! | `Contradiction`     | 任意（≥1）    | 1 条 MemoryHint | High |
//! | `Distill { ids }`   | `ids.len ≥ 5` | 1 条 MemoryHint | Low |
//!
//! 所有 ops 统一映射到 `MemoryHint`：Phase 1 主动放弃分类粒度，好处是风险最低
//! （MemoryHint 后续不可能触发 prompt/tool 应用）。Phase 2 可根据一周数据决定
//! 是否拆分到 `ToolSchemaHint` / `PromptHint` 等。
//!
//! ## 占位语义（必须记 HANDOFF.md）
//!
//! `evidence.occurrence_count = ids.len()` 是**单次反思内**的语义，不是跨次累计：
//! `Merge { ids: [a,b,c] }` 表示「这次反思认为这 3 条可以合并」，**不等于**
//! 「同类问题发生 3 次」。Phase 1 先用它作占位阈值；Phase 2 若要真正的跨次累计，
//! 需要在 evolution 侧加一个短窗口计数器（进程内 map，不落盘）。
//!
//! ## 行为契约
//!
//! - **不调 LLM**；
//! - **不读 DB**；
//! - **不 panic**（空 ops / 空 content / 长 content / unicode / 异常 ids 全安全）；
//! - 产出上限 `MAX_PROPOSALS_PER_ROUND`（默认 5）；
//! - 启发式只挑符合阈值条件的 ops，其余静默忽略；
//! - 纯函数，输入相同 → 输出相同（proposal_id 稳定，dedup 前提）。

use crate::memory::consolidate::{ConsolidateOp, ConsolidateReport};

use super::proposal::{
    proposal_id, Evidence, EvolutionProposal, ImpactLevel, ProposalCategory, ProposalOrigin,
    ProposalTarget, Suggestion,
};

/// 单次反思最多产出 proposal 数（spec 1.3 上限）。
const MAX_PROPOSALS_PER_ROUND: usize = 5;

/// `evidence.related_refs` 关联引用上限（避免 evidence 膨胀）。
const MAX_RELATED_REFS: usize = 3;

/// `suggestion.text` 长度上限（字符数）。
const SUGGESTION_MAX_CHARS: usize = 400;

/// 纯函数：从 consolidate 反思产出派生 `EvolutionProposal`。
///
/// `_report` 当前未参与派生——保留参数是 spec 1.4 的入接口设计
/// （reflecton_output = ops + report），未来按报告统计量派生时可加，
/// 当前阶段不必预先耦合。
pub fn derive_proposals(
    ops: &[ConsolidateOp],
    _report: &ConsolidateReport,
) -> Vec<EvolutionProposal> {
    let mut out: Vec<EvolutionProposal> = Vec::new();
    let now_ms = chrono::Utc::now().timestamp_millis();

    for op in ops {
        if out.len() >= MAX_PROPOSALS_PER_ROUND {
            break;
        }
        if let Some(p) = derive_one(op, now_ms) {
            out.push(p);
        }
    }

    out
}

/// 单 op → 0 或 1 条 proposal。不符合阈值返回 `None`。
fn derive_one(op: &ConsolidateOp, now_ms: i64) -> Option<EvolutionProposal> {
    match op {
        ConsolidateOp::Merge { ids, content } if ids.len() >= 3 => {
            let summary = format!("merge of {} similar memory entries", ids.len());
            let category = ProposalCategory::MemoryHint;
            let target = ProposalTarget::MemoryPolicy {
                policy: "merge_threshold".into(),
            };
            let id = proposal_id(category, &target, &summary);
            Some(EvolutionProposal {
                proposal_id: id,
                created_at_ms: now_ms,
                origin: ProposalOrigin::ConsolidationReflection,
                category,
                target,
                impact: ImpactLevel::Medium,
                evidence: Evidence {
                    summary,
                    occurrence_count: ids.len() as u32,
                    window_hours: 24,
                    related_refs: ids.iter().take(MAX_RELATED_REFS).cloned().collect(),
                },
                suggestion: Suggestion {
                    text: format!(
                        "LLM suggested merging {} entries: {}",
                        ids.len(),
                        truncate_chars(content, SUGGESTION_MAX_CHARS)
                    ),
                    structured_patch: None,
                },
            })
        }
        ConsolidateOp::Contradiction {
            keep,
            drop_id,
            content,
        } => {
            let summary = "contradiction ruled between two memories".to_string();
            let category = ProposalCategory::MemoryHint;
            let target = ProposalTarget::MemoryPolicy {
                policy: "contradiction".into(),
            };
            let id = proposal_id(category, &target, &summary);
            Some(EvolutionProposal {
                proposal_id: id,
                created_at_ms: now_ms,
                origin: ProposalOrigin::ConsolidationReflection,
                category,
                target,
                impact: ImpactLevel::High,
                evidence: Evidence {
                    summary,
                    occurrence_count: 1,
                    window_hours: 24,
                    related_refs: vec![keep.clone(), drop_id.clone()],
                },
                suggestion: Suggestion {
                    text: format!(
                        "LLM resolved contradiction (keep vs drop): {}",
                        truncate_chars(content, SUGGESTION_MAX_CHARS)
                    ),
                    structured_patch: None,
                },
            })
        }
        ConsolidateOp::Distill { ids, content } if ids.len() >= 5 => {
            let summary = format!("distillation of {} entries into a pattern", ids.len());
            let category = ProposalCategory::MemoryHint;
            let target = ProposalTarget::MemoryPolicy {
                policy: "distill_threshold".into(),
            };
            let id = proposal_id(category, &target, &summary);
            Some(EvolutionProposal {
                proposal_id: id,
                created_at_ms: now_ms,
                origin: ProposalOrigin::ConsolidationReflection,
                category,
                target,
                impact: ImpactLevel::Low,
                evidence: Evidence {
                    summary,
                    occurrence_count: ids.len() as u32,
                    window_hours: 24,
                    related_refs: ids.iter().take(MAX_RELATED_REFS).cloned().collect(),
                },
                suggestion: Suggestion {
                    text: format!(
                        "LLM distilled {} entries into a rule: {}",
                        ids.len(),
                        truncate_chars(content, SUGGESTION_MAX_CHARS)
                    ),
                    structured_patch: None,
                },
            })
        }
        // 阈值不达标的 op（ids.len < 3 merge / ids.len < 5 distill）静默忽略
        _ => None,
    }
}

/// 字符级截断（中文友好）。中文一字 3 字节，按 chars 计数。
/// 截断到 `max` 字符 + 省略号 `…`。
fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ───────────────────────── 单元测试（spec 1.6 之 4/5/6/9）─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_report() -> ConsolidateReport {
        ConsolidateReport::default()
    }

    fn merge_op(ids: &[&str], content: &str) -> ConsolidateOp {
        ConsolidateOp::Merge {
            ids: ids.iter().map(|s| s.to_string()).collect(),
            content: content.into(),
        }
    }

    fn contradiction_op(keep: &str, drop_id: &str, content: &str) -> ConsolidateOp {
        ConsolidateOp::Contradiction {
            keep: keep.into(),
            drop_id: drop_id.into(),
            content: content.into(),
        }
    }

    fn distill_op(ids: &[&str], content: &str) -> ConsolidateOp {
        ConsolidateOp::Distill {
            ids: ids.iter().map(|s| s.to_string()).collect(),
            content: content.into(),
        }
    }

    // ─── 4. 中性反思 → 空 ───

    #[test]
    fn derive_returns_empty_for_empty_ops() {
        let proposals = derive_proposals(&[], &empty_report());
        assert!(
            proposals.is_empty(),
            "空 ops 必须产出空 Vec，实测 {} 条",
            proposals.len()
        );
    }

    #[test]
    fn derive_returns_empty_when_no_op_meets_threshold() {
        // 2 个 merge（ids.len=2 < 3）+ 1 个 distill（ids.len=4 < 5），
        // 都不达阈值 → 空 Vec。注意不能放 contradiction（任意都产）
        let ops = vec![
            merge_op(&["a", "b"], "low content"),
            distill_op(&["a", "b", "c", "d"], "low content"),
        ];
        let proposals = derive_proposals(&ops, &empty_report());
        assert!(
            proposals.is_empty(),
            "都不到阈值时应空，实测 {} 条",
            proposals.len()
        );
    }

    #[test]
    fn merge_with_two_ids_does_not_emit_proposal() {
        let ops = vec![merge_op(&["a", "b"], "too few")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert!(proposals.is_empty(), "ids.len=2 < 3 阈值，不应产");
    }

    #[test]
    fn distill_with_four_ids_does_not_emit_proposal() {
        let ops = vec![distill_op(&["a", "b", "c", "d"], "few sources")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert!(proposals.is_empty(), "ids.len=4 < 5 阈值，不应产");
    }

    // ─── 5. 检测规则 ───

    #[test]
    fn derive_detects_repeated_merge_pattern() {
        let ops = vec![merge_op(&["a", "b", "c"], "they look similar")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].category, ProposalCategory::MemoryHint);
        assert_eq!(proposals[0].impact, ImpactLevel::Medium);
        assert_eq!(proposals[0].evidence.occurrence_count, 3);
        assert_eq!(proposals[0].evidence.related_refs, vec!["a", "b", "c"]);
        assert_eq!(proposals[0].evidence.window_hours, 24);
    }

    #[test]
    fn derive_detects_contradiction() {
        let ops = vec![contradiction_op("keep-1", "drop-1", "these contradict")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].impact, ImpactLevel::High);
        assert_eq!(proposals[0].evidence.occurrence_count, 1);
        assert_eq!(proposals[0].evidence.related_refs, vec!["keep-1", "drop-1"]);
    }

    #[test]
    fn derive_detects_distill_pattern() {
        let ops = vec![distill_op(&["a", "b", "c", "d", "e"], "pattern emerges")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].impact, ImpactLevel::Low);
        assert_eq!(proposals[0].evidence.occurrence_count, 5);
    }

    // ─── 6. 上限 ───

    #[test]
    fn derive_caps_at_five() {
        // 10 个 contradiction（任意都产）→ 应只产出 5 条
        let ops: Vec<ConsolidateOp> = (0..10)
            .map(|i| contradiction_op(&format!("keep-{i}"), &format!("drop-{i}"), "c"))
            .collect();
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(
            proposals.len(),
            5,
            "单次反思上限 5 条 proposal，实测 {} 条",
            proposals.len()
        );
    }

    #[test]
    fn derive_caps_across_mixed_ops() {
        // 3 个 merge + 3 个 contradiction + 3 个 distill → 应只产出 5 条
        let mut ops: Vec<ConsolidateOp> = Vec::new();
        for i in 0..3 {
            ops.push(merge_op(
                &[&format!("m{i}-1"), &format!("m{i}-2"), &format!("m{i}-3")],
                "merge",
            ));
            ops.push(contradiction_op(
                &format!("k{i}"),
                &format!("d{i}"),
                "contradiction",
            ));
            ops.push(distill_op(
                &[
                    &format!("p{i}-1"),
                    &format!("p{i}-2"),
                    &format!("p{i}-3"),
                    &format!("p{i}-4"),
                    &format!("p{i}-5"),
                ],
                "distill",
            ));
        }
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(
            proposals.len(),
            5,
            "混合 ops 上限也是 5 条，实测 {} 条",
            proposals.len()
        );
    }

    // ─── 9. 异常输入不 panic ───

    #[test]
    fn derive_does_not_panic_on_empty_content() {
        let ops = vec![merge_op(&["a", "b", "c"], "")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1, "空 content 仍应产 proposal");
        assert!(
            proposals[0]
                .suggestion
                .text
                .starts_with("LLM suggested merging 3 entries: "),
            "空 content 时 suggestion.text 应保留前缀说明，实测：{}",
            proposals[0].suggestion.text
        );
    }

    #[test]
    fn derive_does_not_panic_on_long_content() {
        let long = "x".repeat(10_000);
        let ops = vec![merge_op(&["a", "b", "c"], &long)];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        // suggestion.text = "LLM suggested merging 3 entries: " (31 chars) +
        //                    truncate(content, SUGGESTION_MAX_CHARS)（<= 400 + 1 ellipsis）
        // 总长上限 = 31 + 400 + 1 = 432。给 50 chars 宽限。
        let len = proposals[0].suggestion.text.chars().count();
        assert!(
            len <= SUGGESTION_MAX_CHARS + 50,
            "长 content 应被截断到合理上限，实测长度 {len}"
        );
        assert!(
            proposals[0].suggestion.text.ends_with('…'),
            "截断后应以省略号结尾"
        );
    }

    #[test]
    fn derive_does_not_panic_on_unicode_content() {
        let ops = vec![merge_op(
            &["a", "b", "c"],
            "工具调用失败：超时 timeout 500ms（中文标点）",
        )];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert!(
            proposals[0].suggestion.text.contains("工具调用失败"),
            "unicode content 应保留，实测：{}",
            proposals[0].suggestion.text
        );
    }

    #[test]
    fn derive_does_not_panic_on_extreme_ids_count() {
        // ids 很多——related_refs 必须截断到 MAX_RELATED_REFS
        let many: Vec<String> = (0..100).map(|i| format!("id-{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let ops = vec![merge_op(&refs, "huge merge")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert_eq!(
            proposals[0].evidence.related_refs.len(),
            MAX_RELATED_REFS,
            "related_refs 应截断到 {}，实测 {}",
            MAX_RELATED_REFS,
            proposals[0].evidence.related_refs.len()
        );
        // 但 occurrence_count 仍是真实 ids 数（占位语义）
        assert_eq!(proposals[0].evidence.occurrence_count, 100);
    }

    // ─── 稳定 / 可重入 ───

    #[test]
    fn derive_is_deterministic_given_same_ops() {
        // 同 ops 同 report → 同 proposals（dedup 前提）
        let ops = vec![merge_op(&["a", "b", "c"], "stable")];
        let p1 = derive_proposals(&ops, &empty_report());
        let p2 = derive_proposals(&ops, &empty_report());
        assert_eq!(p1.len(), p2.len());
        assert_eq!(p1[0].proposal_id, p2[0].proposal_id);
    }

    #[test]
    fn derive_ignores_report_field_for_id_stability() {
        // 当前 report 不参与派生；将来若加 report-based 启发式，
        // 应保证同一 ops 不同 report 仍产同 id（避免 dedup 失效）
        let ops = vec![merge_op(&["a", "b", "c"], "same")];
        let p1 = derive_proposals(&ops, &empty_report());
        let mut report2 = empty_report();
        report2.merged = 99;
        report2.distilled = 99;
        report2.contradictions = 99;
        let p2 = derive_proposals(&ops, &report2);
        assert_eq!(p1.len(), p2.len());
        assert_eq!(p1[0].proposal_id, p2[0].proposal_id);
    }

    #[test]
    fn derive_evidence_summary_does_not_leak_raw_paths() {
        // 即使 content 含路径 / 用户输入，summary 是稳定的中性文案
        // （spec：summary 不应存原始路径）
        let ops = vec![merge_op(
            &["a", "b", "c"],
            "/Users/renshi/secret/file.txt contains PII",
        )];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert!(
            !proposals[0].evidence.summary.contains("/Users/"),
            "summary 不应泄露原始路径，实测：{}",
            proposals[0].evidence.summary
        );
        assert!(
            !proposals[0].evidence.summary.contains("PII"),
            "summary 不应泄露敏感词，实测：{}",
            proposals[0].evidence.summary
        );
        // suggestion.text 可以引用（这是给用户看的），content 已被截断
        assert!(
            proposals[0].suggestion.text.contains("/Users/"),
            "suggestion.text 可保留原始 content（这是审计信号）"
        );
    }
}
