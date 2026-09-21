//! R3 L3 沙箱层 · ShadowRunner
//!
//! spec R3：
//! - 候选不生效，记录「如果生效会怎样」
//! - **不重跑 LLM**，只记决策点差异（成本考虑）
//!
//! Shadow 决策点：
//! 1. 这条 lesson 会不会出现在 injection_block.lessons？
//!    （基于 importance 启发式 + 现有 top-3 对比）
//! 2. 如果出现，会替换哪几条？
//!
//! 不调 LLM、不写 mem_items。

use serde::{Deserialize, Serialize};

use super::routing::fnv1a;
use crate::evolution::change::ChangeRecord;
use crate::evolution::proposal::ImpactLevel;

/// Shadow 测试结论
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ShadowDecision {
    /// Shadow 通过：候选会在 injection_block 中产生差异
    Pass,
    /// Shadow 失败：候选不会产生差异（重要性不够 / 已被覆盖）
    Fail,
    /// Shadow 跳过：当前没有对比基准（无现有 lesson 等）
    Skipped,
}

impl ShadowDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

/// Shadow 输出
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowOutcome {
    pub change_id: String,
    pub session_id: String,
    /// 该 candidate 会在 injection_block.lessons 出现吗？
    pub would_inject: bool,
    /// hash_before：当前 top-3 lesson 内容的 FNV-1a
    pub hash_before: String,
    /// hash_after：加入 candidate 后新 top-3 lesson 内容的 FNV-1a
    pub hash_after: String,
    pub decision: ShadowDecision,
    pub note: Option<String>,
    pub evaluated_at_ms: i64,
}

/// Shadow 输入（影子测试参数）
///
/// `existing_lessons` 是从 mem_items 当前读出的 kind="lesson" 条目；
/// 如果调用方懒，可以传空（→ Skipped）。
pub struct ShadowInput<'a> {
    pub change: &'a ChangeRecord,
    pub session_id: &'a str,
    pub existing_lessons: &'a [ShadowLesson],
    pub now_ms: i64,
}

/// 影子测试用的最小 lesson 投影（不暴露完整 MemItem 字段）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowLesson {
    pub id: String,
    pub content: String,
    pub importance: i64,
}

/// 跑一轮 shadow，返回 ShadowOutcome
///
/// 算法：
/// 1. 取现有 lessons importance 排序前 3（FNV-1a hash 内容）
/// 2. 把 candidate 加入，按 importance 模拟排序
/// 3. 取新 top-3（FNV-1a hash 内容）
/// 4. 对比：hash_before == hash_after → Fail（无差异）
///           hash_before != hash_after + candidate 在新 top-3 → Pass
///           现有 lessons 为空 → Skipped
pub fn run_shadow(input: ShadowInput) -> ShadowOutcome {
    let candidate = candidate_lesson(input.change);
    let candidate_importance = candidate.importance;

    // 现有 lessons 按 importance 降序取 top-3
    let mut existing = input.existing_lessons.to_vec();
    existing.sort_by(|a, b| b.importance.cmp(&a.importance));
    let before_top3: Vec<&ShadowLesson> = existing.iter().take(3).collect();
    let hash_before = hash_lessons_content(&before_top3);

    // 加入 candidate，按 importance 模拟 top-3
    let mut hypothetical = existing.clone();
    hypothetical.push(candidate.clone());
    hypothetical.sort_by(|a, b| b.importance.cmp(&a.importance));
    let after_top3: Vec<&ShadowLesson> = hypothetical.iter().take(3).collect();
    let hash_after = hash_lessons_content(&after_top3);

    let (decision, would_inject, note) = if input.existing_lessons.is_empty() {
        // 无现有 lesson 作为对比基准
        (
            ShadowDecision::Skipped,
            false,
            Some("no_baseline_lessons".into()),
        )
    } else if hash_before == hash_after {
        // candidate 没进 top-3（重要性不够）
        (
            ShadowDecision::Fail,
            false,
            Some("below_top3_threshold".into()),
        )
    } else {
        // candidate 进了 top-3（有差异）
        // 但还要检查是否真的包含 candidate.id
        let in_top3 = after_top3.iter().any(|l| l.id == candidate.id);
        if in_top3 {
            (ShadowDecision::Pass, true, None)
        } else {
            // hash 不同但 candidate 不在 top-3（理论不应发生；防意外）
            (
                ShadowDecision::Fail,
                false,
                Some("top3_diff_but_candidate_absent".into()),
            )
        }
    };

    ShadowOutcome {
        change_id: input.change.change_id.clone(),
        session_id: input.session_id.into(),
        would_inject,
        hash_before,
        hash_after,
        decision,
        note,
        evaluated_at_ms: input.now_ms,
    }
    // candidate_importance 暂未用，避免 unused 警告
    .with_dummy_for(candidate_importance)
}

/// 构造 candidate lesson（基于 ChangeRecord 模拟 apply 后的样子）
fn candidate_lesson(change: &ChangeRecord) -> ShadowLesson {
    let importance = match change.impact {
        ImpactLevel::High => 4,
        _ => 3,
    };
    ShadowLesson {
        id: change.change_id.clone(),
        content: change.suggestion_text.clone(),
        importance,
    }
}

/// hash 一组 lessons 的 content（顺序敏感，FNV-1a）
fn hash_lessons_content(lessons: &[&ShadowLesson]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for l in lessons {
        for b in l.content.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        // 分隔符避免内容拼接碰撞
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

// ShadowOutcome helper trait：避免 unused variable warning
trait WithDummy {
    fn with_dummy_for(self, _i: i64) -> Self;
}
impl WithDummy for ShadowOutcome {
    fn with_dummy_for(self, _i: i64) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::{ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk_change(id: &str, impact: ImpactLevel) -> ChangeRecord {
        ChangeRecord {
            change_id: id.into(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: id.trim_start_matches("chg-").into(),
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: format!("text for {id}"),
            mem_key: format!("evo:{}", id.trim_start_matches("chg-")),
            impact,
            eval_before: None,
            eval_after: None,
            status: ChangeStatus::Shadowing,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::Pending,
            human_approver: None,
            created_at_ms: 1_700_000_000_000,
            rolled_back_at: None,
            rollback_reason: None,
        }
    }

    fn mk_lesson(id: &str, content: &str, importance: i64) -> ShadowLesson {
        ShadowLesson {
            id: id.into(),
            content: content.into(),
            importance,
        }
    }

    // ─── shadow 不写 mem_items（spec R3 验收）───

    #[test]
    fn shadow_does_not_modify_input() {
        // 验证 run_shadow 是 pure：existing_lessons 不变
        let lessons = vec![mk_lesson("a", "alpha", 4), mk_lesson("b", "beta", 3)];
        let original_len = lessons.len();
        let original_a_content = lessons[0].content.clone();
        let change = mk_change("chg-x", ImpactLevel::High);
        let input = ShadowInput {
            change: &change,
            session_id: "s1",
            existing_lessons: &lessons,
            now_ms: 1000,
        };
        let _ = run_shadow(input);
        // 输入未被修改
        assert_eq!(lessons.len(), original_len);
        assert_eq!(lessons[0].content, original_a_content);
        // 真实测试：ShadowOutcome 也不持有 mut 引用
        // （如果持有，会编译失败或 borrow 冲突）
    }

    // ─── shadow pass / fail / skipped ───

    #[test]
    fn shadow_pass_when_candidate_important() {
        let lessons = vec![
            mk_lesson("a", "alpha", 3),
            mk_lesson("b", "beta", 3),
            mk_lesson("c", "gamma", 2),
        ];
        let change = mk_change("chg-new", ImpactLevel::High); // imp=4
        let outcome = run_shadow(ShadowInput {
            change: &change,
            session_id: "s1",
            existing_lessons: &lessons,
            now_ms: 1000,
        });
        assert_eq!(outcome.decision, ShadowDecision::Pass);
        assert!(outcome.would_inject);
        assert_ne!(outcome.hash_before, outcome.hash_after);
    }

    #[test]
    fn shadow_fail_when_candidate_below_top3() {
        let lessons = vec![
            mk_lesson("a", "alpha", 5), // imp 5（受保护但是模拟数据）
            mk_lesson("b", "beta", 5),
            mk_lesson("c", "gamma", 5),
        ];
        // candidate importance=3 (Medium)，被 imp=5 三连击挡
        let change = mk_change("chg-new", ImpactLevel::Medium);
        let outcome = run_shadow(ShadowInput {
            change: &change,
            session_id: "s1",
            existing_lessons: &lessons,
            now_ms: 1000,
        });
        assert_eq!(outcome.decision, ShadowDecision::Fail);
        assert!(!outcome.would_inject);
        assert_eq!(outcome.hash_before, outcome.hash_after);
        assert_eq!(outcome.note.as_deref(), Some("below_top3_threshold"));
    }

    #[test]
    fn shadow_skipped_when_no_baseline() {
        let change = mk_change("chg-new", ImpactLevel::High);
        let outcome = run_shadow(ShadowInput {
            change: &change,
            session_id: "s1",
            existing_lessons: &[],
            now_ms: 1000,
        });
        assert_eq!(outcome.decision, ShadowDecision::Skipped);
        assert!(!outcome.would_inject);
        assert_eq!(outcome.note.as_deref(), Some("no_baseline_lessons"));
    }

    // ─── hash 稳定性 ───

    #[test]
    fn hash_stable_for_same_input() {
        let lessons = vec![mk_lesson("a", "alpha", 4), mk_lesson("b", "beta", 3)];
        let h1 = hash_lessons_content(&lessons.iter().collect::<Vec<_>>());
        let h2 = hash_lessons_content(&lessons.iter().collect::<Vec<_>>());
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_differs_for_different_content() {
        let l1 = vec![mk_lesson("a", "alpha", 4)];
        let l2 = vec![mk_lesson("a", "alphabet", 4)];
        let h1 = hash_lessons_content(&l1.iter().collect::<Vec<_>>());
        let h2 = hash_lessons_content(&l2.iter().collect::<Vec<_>>());
        assert_ne!(h1, h2);
    }

    // ─── enum 字符串锁死 ───

    #[test]
    fn decision_strings_locked() {
        assert_eq!(ShadowDecision::Pass.as_str(), "pass");
        assert_eq!(ShadowDecision::Fail.as_str(), "fail");
        assert_eq!(ShadowDecision::Skipped.as_str(), "skipped");
    }

    // ─── 集成：影子测试后转换 status ───

    #[test]
    fn integration_shadow_pass_can_transition_to_shadow_passed() {
        // 验证 spec R3 集成链路：shadow pass → status: Shadowing → ShadowPassed
        let lessons = vec![mk_lesson("a", "low", 2), mk_lesson("b", "lower", 1)];
        let mut change = mk_change("chg-new", ImpactLevel::High);
        change.status = ChangeStatus::Shadowing;
        let outcome = run_shadow(ShadowInput {
            change: &change,
            session_id: "s1",
            existing_lessons: &lessons,
            now_ms: 1000,
        });
        // 验证：Pass 后可合法 transition 到 ShadowPassed
        use crate::evolution::change::status::transition;
        assert_eq!(outcome.decision, ShadowDecision::Pass);
        assert!(transition(change.status, ChangeStatus::ShadowPassed).is_ok());
    }

    #[test]
    fn integration_full_chain_shadow_to_canary_to_active() {
        // spec R3 集成验收：shadow → canary → active 全链路
        use super::super::routing::is_canary;
        use crate::evolution::change::status::transition;

        let lessons = vec![mk_lesson("a", "low", 2), mk_lesson("b", "lower", 1)];
        let mut change = mk_change("chg-chain", ImpactLevel::High);

        // 1. Pending → Shadowing
        change.status = ChangeStatus::Pending;
        assert!(transition(change.status, ChangeStatus::Shadowing).is_ok());
        change.status = ChangeStatus::Shadowing;

        // 2. Run shadow (Pass)
        let outcome = run_shadow(ShadowInput {
            change: &change,
            session_id: "s1",
            existing_lessons: &lessons,
            now_ms: 1000,
        });
        assert_eq!(outcome.decision, ShadowDecision::Pass);

        // 3. Shadowing → ShadowPassed
        assert!(transition(change.status, ChangeStatus::ShadowPassed).is_ok());
        change.status = ChangeStatus::ShadowPassed;

        // 4. ShadowPassed → Approved
        assert!(transition(change.status, ChangeStatus::Approved).is_ok());
        change.status = ChangeStatus::Approved;

        // 5. Approved → Canary
        assert!(transition(change.status, ChangeStatus::Canary).is_ok());
        change.status = ChangeStatus::Canary;

        // 6. Canary 路由验证：1000 sessions ≈ 5% 见到
        let mut sees_change = 0;
        for i in 0..1000 {
            if is_canary(&format!("session-{i}")) {
                sees_change += 1;
            }
        }
        assert!(
            sees_change >= 25 && sees_change <= 100,
            "Canary 5% 应 ≈ 50，实测 {sees_change}"
        );

        // 7. Canary → Active
        assert!(transition(change.status, ChangeStatus::Active).is_ok());
        change.status = ChangeStatus::Active;

        // 8. Active → RolledBack（验证回滚路径）
        assert!(transition(change.status, ChangeStatus::RolledBack).is_ok());
    }
}
