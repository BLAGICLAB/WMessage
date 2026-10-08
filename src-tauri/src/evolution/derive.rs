//! Phase 1.3：从 consolidate 反思产出派生 `EvolutionProposal`。
//!
//! ## 启发式规则（2026-10-09 调参：原 Merge≥3/Distill≥5 的保守值让常规
//! 整理永远产不出提案——个人库单轮合并/提炼普遍只有 2 条，候选池长期为空）
//!
//! | 输入                  | 阈值           | 产出           | 影响等级 |
//! |---------------------|--------------|--------------|------|
//! | `Merge { ids }`     | `ids.len ≥ 2` | 1 条 MemoryHint | Medium |
//! | `Contradiction`     | 任意（≥1）    | 1 条 MemoryHint | High |
//! | `Distill { ids }`   | `ids.len ≥ 2` | 1 条 MemoryHint | Low |
//!
//! Low 产出不进自动轨（gate 拦），但**照常进候选池**等人工决策——
//! 面板可见、可手动启用。
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

use serde::{Deserialize, Serialize};

use crate::memory::consolidate::{ConsolidateOp, ConsolidateReport};

use super::proposal::{
    proposal_id, short_hash, Evidence, EvolutionProposal, ImpactLevel, ProposalCategory,
    ProposalOrigin, ProposalTarget, Suggestion,
};

/// 派生门槛（可设置项：设置页自进化区「提案派生门槛」卡）。
/// 语义：一次整理操作涉及的**条目数**达到门槛，才派生对应提案。
/// 存 bot-config.json `evolution.deriveThresholds`；读取/落盘经 `clamped`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeriveThresholds {
    /// Merge 提案门槛：一次合并 ≥N 条同源记忆才产提案
    pub merge_min_ids: usize,
    /// Distill 提案门槛：一次提炼汇总 ≥N 条记忆才产提案
    pub distill_min_ids: usize,
    /// Contradiction 提案门槛：矛盾裁决恒涉及 2 条（keep/drop），
    /// 默认 1 = 任何矛盾都产提案；设 3 及以上 ≈ 关闭矛盾提案（2 到不了 3）
    pub contradiction_min_ids: usize,
}

impl Default for DeriveThresholds {
    fn default() -> Self {
        Self {
            merge_min_ids: 2,
            distill_min_ids: 2,
            contradiction_min_ids: 1,
        }
    }
}

impl DeriveThresholds {
    /// 读取/落盘前统一钳制（防手改配置塞 0 或天文数字）
    pub fn clamped(mut self) -> Self {
        self.merge_min_ids = self.merge_min_ids.clamp(2, 20);
        self.distill_min_ids = self.distill_min_ids.clamp(2, 20);
        self.contradiction_min_ids = self.contradiction_min_ids.clamp(1, 99);
        self
    }
}

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
    report: &ConsolidateReport,
) -> Vec<EvolutionProposal> {
    derive_proposals_with(ops, report, &DeriveThresholds::default())
}

/// 带派生门槛的版本（post_consolidation 从设置项读入）。
pub fn derive_proposals_with(
    ops: &[ConsolidateOp],
    report: &ConsolidateReport,
    t: &DeriveThresholds,
) -> Vec<EvolutionProposal> {
    let mut out: Vec<EvolutionProposal> = Vec::new();
    let now_ms = chrono::Utc::now().timestamp_millis();

    for op in ops {
        if out.len() >= MAX_PROPOSALS_PER_ROUND {
            break;
        }
        if let Some(p) = derive_one(op, now_ms, t) {
            out.push(p);
        }
    }

    out
}

/// 单 op → 0 或 1 条 proposal。不符合门槛返回 `None`。
fn derive_one(op: &ConsolidateOp, now_ms: i64, t: &DeriveThresholds) -> Option<EvolutionProposal> {
    match op {
        ConsolidateOp::Merge { ids, content } if ids.len() >= t.merge_min_ids => {
            let summary = format!("merge of {} similar memory entries", ids.len());
            let category = ProposalCategory::MemoryHint;
            let target = ProposalTarget::MemoryPolicy {
                policy: "merge_threshold".into(),
            };
            // 与 contradiction 分支同问题：base_id 对同长度 merge 全同（summary
            // 固定串 + normalize_for_hash 把数字位归一，UUID hex 记忆 id 仅数字
            // 不同也坍缩），一轮多个同规模分组会共享同 id 被 dedup 坍缩。
            // 把排序后的原始 refs 拼进二次 hash 作判别位（绕开归一化）。
            let base_id = proposal_id(category, &target, &summary);
            let mut refs = ids.clone();
            refs.sort();
            let id = short_hash(&format!("{base_id}|{}", refs.join("|")));
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
        } if 2 >= t.contradiction_min_ids => {
            let summary = "contradiction ruled between two memories".to_string();
            let category = ProposalCategory::MemoryHint;
            let target = ProposalTarget::MemoryPolicy {
                policy: "contradiction".into(),
            };
            // 一轮 N 条矛盾的 base_id 相同（summary 固定串；且 normalize_for_hash
            // 把数字位归一成 'N'，UUID hex 记忆 id 仅数字不同也坍缩）——把原始
            // refs 拼进二次 hash 输入作判别位（绕开归一化），保持 16 hex 形态。
            // 哪两条记忆由 evidence.related_refs 承载，不进 summary。
            let base_id = proposal_id(category, &target, &summary);
            let id = short_hash(&format!("{base_id}|{keep}|{drop_id}"));
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
        ConsolidateOp::Distill { ids, content } if ids.len() >= t.distill_min_ids => {
            let summary = format!("distillation of {} entries into a pattern", ids.len());
            let category = ProposalCategory::MemoryHint;
            let target = ProposalTarget::MemoryPolicy {
                policy: "distill_threshold".into(),
            };
            // 与 merge 分支同理：把排序后的原始 refs 拼进二次 hash，
            // 防同长度不同分组的 distill 共享同 id 被 dedup 坍缩
            let base_id = proposal_id(category, &target, &summary);
            let mut refs = ids.clone();
            refs.sort();
            let id = short_hash(&format!("{base_id}|{}", refs.join("|")));
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
        // 门槛不达标的 op（单条 ids 的 merge/distill）静默忽略
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

// 单元测试（spec 1.6 之 4/5/6/9）

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

    // 4. 中性反思 → 空

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
    fn derive_returns_empty_only_for_single_id_ops() {
        // 2026-10-09 调参（Merge≥2 / Distill≥2）后：只有单条 ids 的 op 不到阈值。
        // 注意不能放 contradiction（任意都产）
        let ops = vec![
            merge_op(&["a"], "single source"),
            distill_op(&["b"], "single source"),
        ];
        let proposals = derive_proposals(&ops, &empty_report());
        assert!(
            proposals.is_empty(),
            "单条 ids 不到阈值时应空，实测 {} 条",
            proposals.len()
        );
    }

    #[test]
    fn merge_with_two_ids_emits_medium_proposal() {
        // 常规整理最常见形态：两条同源合并 → Medium 提案进候选池
        let ops = vec![merge_op(&["a", "b"], "routine merge")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1, "ids.len=2 应产提案（调参后阈值）");
        assert_eq!(proposals[0].impact, ImpactLevel::Medium);
    }

    #[test]
    fn distill_with_two_ids_emits_low_proposal() {
        // Distill 产 Low：不进自动轨（gate 拦），但照常进候选池等人工决策
        let ops = vec![distill_op(&["a", "b"], "pattern from two")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1, "ids.len=2 应产提案（调参后阈值）");
        assert_eq!(proposals[0].impact, ImpactLevel::Low);
    }

    // 5. 检测规则

    #[test]
    fn derive_proposals_with_honors_custom_thresholds() {
        // 门槛调高：merge 3 条在默认门槛（2）下产提案、门槛 3 时也产、门槛 4 时不产
        let ops = vec![merge_op(&["a", "b", "c"], "three sources")];
        let report = empty_report();
        assert_eq!(
            derive_proposals_with(&ops, &report, &DeriveThresholds::default()).len(),
            1
        );
        let t3 = DeriveThresholds {
            merge_min_ids: 3,
            ..DeriveThresholds::default()
        };
        assert_eq!(derive_proposals_with(&ops, &report, &t3).len(), 1);
        let t4 = DeriveThresholds {
            merge_min_ids: 4,
            ..DeriveThresholds::default()
        };
        assert_eq!(derive_proposals_with(&ops, &report, &t4).len(), 0);

        // contradiction_min_ids=3 ≈ 关闭矛盾提案（矛盾恒涉及 2 条，2 到不了 3）
        let c_ops = vec![contradiction_op("keep-1", "drop-1", "these contradict")];
        let t_off = DeriveThresholds {
            contradiction_min_ids: 3,
            ..DeriveThresholds::default()
        };
        assert_eq!(derive_proposals_with(&c_ops, &report, &t_off).len(), 0);
        assert_eq!(
            derive_proposals_with(&c_ops, &report, &DeriveThresholds::default()).len(),
            1
        );
    }

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
    fn contradiction_proposals_have_distinct_ids_per_pair() {
        // 每条矛盾的 id 含绕开数字归一化的 refs 判别位 → 相异，
        // 下游 dedup 不再把一轮 N 条矛盾坍缩成 1 条。
        // 用仅数字不同的 id（UUID hex 的真实形态）回归数字归一化坍缩。
        let ops = vec![
            contradiction_op("ab12cd", "ef12ab", "c1"),
            contradiction_op("ab34cd", "ef34ab", "c2"),
        ];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 2);
        assert_ne!(proposals[0].proposal_id, proposals[1].proposal_id);
        // 完整 32 字符 UUID hex、仅数字不同的对也相异
        let ops32 = vec![
            contradiction_op(
                "abcdef0123456789abcdef0123456789",
                "0123456789abcdef0123456789abcdef",
                "x",
            ),
            contradiction_op(
                "abcdef9987654321abcdef9987654321",
                "9987654321abcdef9987654321abcdef",
                "y",
            ),
        ];
        let p32 = derive_proposals(&ops32, &empty_report());
        assert_eq!(p32.len(), 2);
        assert_ne!(p32[0].proposal_id, p32[1].proposal_id);
        // 16 hex 形态不破（与 Merge/Distill 的 id 同构）
        assert_eq!(p32[0].proposal_id.len(), 16);
        // 同输入仍同 id（dedup 前提的确定性不破）
        let again = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals[0].proposal_id, again[0].proposal_id);
    }

    #[test]
    fn derive_detects_distill_pattern() {
        let ops = vec![distill_op(&["a", "b", "c", "d", "e"], "pattern emerges")];
        let proposals = derive_proposals(&ops, &empty_report());
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].impact, ImpactLevel::Low);
        assert_eq!(proposals[0].evidence.occurrence_count, 5);
    }

    #[test]
    fn merge_and_distill_proposals_have_distinct_ids_per_group() {
        // 与 contradiction 的判别位同思路：同长度不同分组的 merge/distill
        // 必须各自成案，dedup 不把一轮里多个同规模分组坍缩成 1 条。
        // 用仅数字不同的 id（UUID hex 的真实形态）回归数字归一化坍缩。
        let merges = vec![
            merge_op(&["ab12cd", "ef12ab", "12abcd"], "m1"),
            merge_op(&["ab34cd", "ef34ab", "34abcd"], "m2"),
        ];
        let p = derive_proposals(&merges, &empty_report());
        assert_eq!(p.len(), 2);
        assert_ne!(p[0].proposal_id, p[1].proposal_id);
        let distills = vec![
            distill_op(&["a1", "a2", "a3", "a4", "a5"], "d1"),
            distill_op(&["b1", "b2", "b3", "b4", "b5"], "d2"),
        ];
        let pd = derive_proposals(&distills, &empty_report());
        assert_eq!(pd.len(), 2);
        assert_ne!(pd[0].proposal_id, pd[1].proposal_id);
        // 16 hex 形态保持；同输入仍同 id（dedup 前提的确定性不破）
        assert_eq!(p[0].proposal_id.len(), 16);
        let again = derive_proposals(&merges, &empty_report());
        assert_eq!(p[0].proposal_id, again[0].proposal_id);
    }

    // 6. 上限

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

    // 9. 异常输入不 panic

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

    // 稳定 / 可重入

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
