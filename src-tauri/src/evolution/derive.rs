//! 从记忆整理的反思产出派生 `EvolutionProposal`——目前只有一个来源：
//! 反思同调用产出的 `lesson` 段（行为准则）。
//!
//! ## 为什么只有 lesson 一条路
//!
//! 曾经这里还有三条：从 `Merge` / `Contradiction` / `Distill` 三类 op 派生
//! 「事实类」提案。它们产出的是英文模板复述：
//!
//! ```text
//! "LLM suggested merging 3 entries: 记忆整理裁决原则：merge 用于内容重复…"
//! "LLM resolved contradiction (keep vs drop): PDF 拆分用 pdfplumber 库"
//! ```
//!
//! 而这些 op 的效果**已经由 `consolidate::apply_ops` 执行完毕**——merge 真的
//! 合并了、contradiction 真的删掉了落空的那条、distill 真的建了 reflection。
//! 再把「刚才合并过 3 条」写成一条英文描述存进长期记忆，是净负收益：
//!
//! 1. 它挤占 `memory::rank` 的 `MEMORY_LESSON_N = 3` 名额——生产库里 6 条
//!    evolution 产出的 lesson 有 3 条 `access_count = 0`（从未被召回过）。
//! 2. 它违反下游自己的约束：`prompts/consolidate.rs` 的提示词明确写了「lesson
//!    禁止复述条目事实内容」「与既有记忆重复时省略」。ops 分支从 op 结构自己
//!    绕开提示词造了一遍复述。
//! 3. 「一次反思认为这 3 条可以合并」被写成一条记忆，等于把**已经发生的动作**
//!    存成**未来的准则**——语义是错的。
//!
//! lesson 段不一样：它是 LLM 对「以后该怎么做」的判断，内容是行为准则而非
//! 动作记录，落进 lesson 库能被下轮对话注入，是这套系统唯一真正有价值的产出。
//!
//! ## 行为契约
//!
//! - **不调 LLM、不读 DB、不 panic**（空串 / 纯空白 / 超长 / 任意 unicode 都安全）；
//! - 纯函数：输入相同 → 输出相同（proposal_id 是内容归一化哈希，dedup 的前提）。

use crate::bot::log_sanitize::truncate_chars;

use super::proposal::{
    proposal_id, Evidence, EvolutionProposal, ImpactLevel, ProposalCategory, ProposalOrigin,
    ProposalTarget, Suggestion,
};

/// `suggestion.text` 长度上限（字符数）。
const SUGGESTION_MAX_CHARS: usize = 400;

/// 行为准则提案：来自反思同调用产出的 `lesson` 段——内容是「机器人以后该怎么做」。
///
/// Medium 影响等级：可进自动轨（`strategy::gate_decision` 放行 MemoryHint ×
/// {High, Medium}），apply 后落一条 `mem_items(kind=lesson, tags=["evo:<id>"])`，
/// 下轮对话由「经验教训」段注入。
///
/// 稳定 id（内容归一化哈希）——同一条准则反复产出时 dedup 幂等，不会在候选池
/// 里堆同义条目。
pub fn derive_lesson_proposal(lesson: &str, now_ms: i64) -> Option<EvolutionProposal> {
    let text = truncate_chars(lesson.trim(), SUGGESTION_MAX_CHARS, true);
    if text.is_empty() {
        return None;
    }
    let category = ProposalCategory::MemoryHint;
    let target = ProposalTarget::MemoryPolicy {
        policy: "behavioral_lesson".into(),
    };
    let proposal_id = proposal_id(category, &target, &text);
    Some(EvolutionProposal {
        proposal_id,
        created_at_ms: now_ms,
        origin: ProposalOrigin::ConsolidationReflection,
        category,
        target,
        impact: ImpactLevel::Medium,
        evidence: Evidence {
            summary: text.clone(),
            occurrence_count: 1,
            window_hours: 24,
            related_refs: vec![],
        },
        suggestion: Suggestion {
            text,
            structured_patch: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_lesson(text: &str) -> EvolutionProposal {
        derive_lesson_proposal(text, 1_700_000_000_000).expect("非空 lesson 应产出提案")
    }

    #[test]
    fn lesson_proposal_shape() {
        let p = one_lesson("遇到两条记忆互相矛盾时，优先保留更具体、更早确立的那条");
        assert_eq!(p.origin, ProposalOrigin::ConsolidationReflection);
        assert_eq!(p.category, ProposalCategory::MemoryHint);
        assert_eq!(p.impact, ImpactLevel::Medium, "Medium 才能进自动轨");
        assert!(matches!(p.target, ProposalTarget::MemoryPolicy { .. }));
        assert_eq!(p.evidence.occurrence_count, 1);
        assert!(
            p.evidence.related_refs.is_empty(),
            "lesson 提案不挂记忆引用：它讲的是准则不是某几条记忆"
        );
        assert!(p.suggestion.structured_patch.is_none());
    }

    /// 空 / 纯空白 → None，不产出占位提案（否则候选池会被空条目填满）
    #[test]
    fn empty_lesson_yields_none() {
        assert!(derive_lesson_proposal("", 0).is_none());
        assert!(derive_lesson_proposal("   ", 0).is_none());
        assert!(derive_lesson_proposal("\n\t  \n", 0).is_none());
    }

    /// 同一条准则反复产出必须得到同一个 id——否则 dedup 失效，候选池会堆同义条目
    #[test]
    fn same_lesson_is_idempotent() {
        let a = one_lesson("合并同义记忆前先看 key 是否已有");
        let b = one_lesson("合并同义记忆前先看 key 是否已有");
        assert_eq!(a.proposal_id, b.proposal_id);
    }

    /// id 只跟内容走，跟时间无关：同一条准则第二轮再反思出来不换 id
    #[test]
    fn id_is_independent_of_timestamp() {
        let a = derive_lesson_proposal("淘汰记忆时看访问次数，不看创建时间", 1_000).unwrap();
        let b = derive_lesson_proposal("淘汰记忆时看访问次数，不看创建时间", 9_999_999).unwrap();
        assert_eq!(a.proposal_id, b.proposal_id);
        assert_eq!(a.created_at_ms, 1_000);
        assert_eq!(b.created_at_ms, 9_999_999);
    }

    #[test]
    fn different_lessons_get_different_ids() {
        let a = one_lesson("A 准则");
        let b = one_lesson("B 准则");
        assert_ne!(a.proposal_id, b.proposal_id);
    }

    /// 超长截断到上限而不是整条丢掉
    #[test]
    fn long_lesson_is_truncated_not_dropped() {
        let long = "很长的准则".repeat(500);
        let p = one_lesson(&long);
        assert!(
            p.suggestion.text.chars().count() <= SUGGESTION_MAX_CHARS + 1,
            "应截断到上限（含省略号）"
        );
        assert!(!p.suggestion.text.is_empty());
    }

    #[test]
    fn unicode_and_control_chars_are_safe() {
        let p = one_lesson("emoji 🧠 与换行\n、以及制表\t都安全");
        assert!(p.suggestion.text.contains("🧠"));
        assert!(!p.proposal_id.is_empty());
    }

    /// 前后空白先 trim 再进提案：模型输出常带缩进/换行
    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let p = one_lesson("\n  准则正文  \n");
        assert_eq!(p.suggestion.text, "准则正文");
        assert_eq!(p.evidence.summary, "准则正文");
    }

    /// 确定性：同输入同输出
    #[test]
    fn deterministic() {
        let text = "确定性检查";
        let a = one_lesson(text);
        let b = one_lesson(text);
        assert_eq!(a.proposal_id, b.proposal_id);
        assert_eq!(a.suggestion.text, b.suggestion.text);
    }
}
