//! R6 A · 停止条件检测（决策 d）
//!
//! 老板 13:13 拍板停止条件（OR 任一满足即停）：
//! - 观察到 30 个 change 走完 Proposed → 终态
//! - 观察到 14 天（上限）
//! - 观察到 RolledBack ≥ 5
//!
//! 「锁定」≠「有检测」（老板 13:20 批评）。
//! 本模块提供 `check_stop_condition` 函数 + 测试，
//! dev 可每日跑 `observe-run --check-stop` 或类似命令调用。

use crate::evolution::change::{ChangeRecord, ChangeStatus};

/// 停止条件阈值（与 R6_A_DESIGN.md 第 8 节一致）
pub const STOP_COMPLETED_CHANGES: u64 = 30;
pub const STOP_MAX_DAYS: f64 = 14.0;
pub const STOP_MIN_ROLLED_BACK: u64 = 5;

/// 停止条件状态
#[derive(Debug, Clone, PartialEq)]
pub struct StopConditionStatus {
    /// 走完 Proposed → 终态 的 change 数
    pub changes_completed: u64,
    /// 自 start_ms 起的经过天数
    pub days_elapsed: f64,
    /// RolledBack 状态的 change 数
    pub rolled_back_count: u64,
    /// 是否满足任一停止条件
    pub should_stop: bool,
    /// 命中的停止原因列表（OR 关系）
    pub stop_reasons: Vec<StopReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    CompletedChangesReach30,
    FourteenDaysElapsed,
    FiveOrMoreRollbacks,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompletedChangesReach30 => "30+ changes completed (Proposed → terminal)",
            Self::FourteenDaysElapsed => "14 days elapsed (上限)",
            Self::FiveOrMoreRollbacks => "5+ RolledBack (回滚率过高信号)",
        }
    }
}

/// 检测停止条件
///
/// 入参：
/// - `changes`：evolution-changes.jsonl 内容（全部）
/// - `start_ms`：观察窗口起点（R6 A flag 开启时刻）
/// - `now_ms`：评估时刻
pub fn check_stop_condition(
    changes: &[ChangeRecord],
    start_ms: i64,
    now_ms: i64,
) -> StopConditionStatus {
    // OCR C3-3：先按**观察窗**过滤（对齐 observe/metrics.rs 的 created_at_ms ∈ [start_ms, now_ms]），
    // 否则 evolution-changes.jsonl 里的历史行会被计入，立即误触发
    // CompletedChangesReach30 / FiveOrMoreRollbacks（调用方契约 = 传全量，本函数自己开窗）。
    let in_window = |c: &ChangeRecord| c.created_at_ms >= start_ms && c.created_at_ms <= now_ms;

    // 走完 Proposed → 终态 的 change 数（**窗口内**）
    // 终态 = Active | Rejected | RolledBack | Expired（pending/shadowing 等不算）
    let completed = changes
        .iter()
        .filter(|c| {
            in_window(c)
                && matches!(
                    c.status,
                    ChangeStatus::Active
                        | ChangeStatus::Rejected
                        | ChangeStatus::RolledBack
                        | ChangeStatus::Expired
                )
        })
        .count() as u64;

    let rolled_back = changes
        .iter()
        .filter(|c| in_window(c) && c.status == ChangeStatus::RolledBack)
        .count() as u64;

    // OCR C5-EV-3a-01：saturating_sub 防溢出（溢出曾被 max(0.0) 静默压零；
    // 饱和后 days 巨大，时间门正常响）；负时长（时钟回拨/参数颠倒）留痕
    // 不静默（仍计 0.0 天不误触发）。Result 化签名变更属 B 类，不自决。
    if now_ms < start_ms {
        eprintln!(
            "[evolution_stop] negative observation window: now_ms={now_ms} < start_ms={start_ms}（按 0.0 天计，请核时钟/参数）"
        );
    }
    let days_elapsed = (now_ms.saturating_sub(start_ms) as f64 / 86_400_000.0).max(0.0);

    let mut stop_reasons = Vec::new();
    if completed >= STOP_COMPLETED_CHANGES {
        stop_reasons.push(StopReason::CompletedChangesReach30);
    }
    if days_elapsed >= STOP_MAX_DAYS {
        stop_reasons.push(StopReason::FourteenDaysElapsed);
    }
    if rolled_back >= STOP_MIN_ROLLED_BACK {
        stop_reasons.push(StopReason::FiveOrMoreRollbacks);
    }

    StopConditionStatus {
        changes_completed: completed,
        days_elapsed,
        rolled_back_count: rolled_back,
        should_stop: !stop_reasons.is_empty(),
        stop_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::{ApprovalSource, ChangeRecord, EvolutionLayer};
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk_change(id: &str, status: ChangeStatus) -> ChangeRecord {
        mk_change_at(id, status, 1_700_000_000_000)
    }

    /// 指定 `created_at_ms` 的构造（OCR C3-3 后：计数按观察窗过滤，测试须让时间戳落在窗内）。
    fn mk_change_at(id: &str, status: ChangeStatus, created_at_ms: i64) -> ChangeRecord {
        ChangeRecord {
            change_id: id.into(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: id.into(),
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            impact: ImpactLevel::Medium,
            eval_before: None,
            eval_after: None,
            status,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::AutoApplied,
            human_approver: None,
            created_at_ms,
            rolled_back_at: None,
            rollback_reason: None,
        }
    }

    const DAY_MS: i64 = 86_400_000;

    /// C5-EV-3a-01：负时长（now < start）不 panic、按 0.0 天计、不误停（eprintln 留痕）
    #[test]
    fn negative_window_counts_zero_days_without_time_stop() {
        let r = check_stop_condition(&[], DAY_MS * 10, DAY_MS * 5);
        assert_eq!(r.days_elapsed, 0.0);
        assert!(!r.should_stop);
        assert!(!r.stop_reasons.contains(&StopReason::FourteenDaysElapsed));
    }

    /// C5-EV-3a-01：start_ms=i64::MIN 时减法溢出 → saturating_sub 饱和为
    /// i64::MAX（days 巨大）→ 时间门正常触发，不再被 max(0.0) 静默压零
    #[test]
    fn overflow_window_saturates_and_trips_time_gate() {
        let r = check_stop_condition(&[], i64::MIN, 0);
        assert!(r.days_elapsed > 1_000_000.0);
        assert!(r.stop_reasons.contains(&StopReason::FourteenDaysElapsed));
        assert!(r.should_stop);
    }

    #[test]
    fn no_change_no_stop() {
        let now = DAY_MS * 5;
        let r = check_stop_condition(&[], 0, now);
        assert_eq!(r.changes_completed, 0);
        assert_eq!(r.days_elapsed, 5.0);
        assert_eq!(r.rolled_back_count, 0);
        assert!(!r.should_stop);
        assert!(r.stop_reasons.is_empty());
    }

    #[test]
    fn stop_at_30_completed() {
        let now = DAY_MS * 5;
        let changes: Vec<_> = (0..30)
            .map(|i| mk_change_at(&format!("p{i}"), ChangeStatus::Active, now - 1))
            .collect();
        let r = check_stop_condition(&changes, 0, now);
        assert_eq!(r.changes_completed, 30);
        assert!(r.should_stop);
        assert!(r
            .stop_reasons
            .contains(&StopReason::CompletedChangesReach30));
    }

    #[test]
    fn stop_at_14_days_even_with_zero_changes() {
        let now = DAY_MS * 14;
        let r = check_stop_condition(&[], 0, now);
        assert!(r.should_stop);
        assert!(r.stop_reasons.contains(&StopReason::FourteenDaysElapsed));
    }

    #[test]
    fn stop_at_5_rollbacks() {
        let now = DAY_MS * 5;
        let changes: Vec<_> = (0..5)
            .map(|i| mk_change_at(&format!("rb{i}"), ChangeStatus::RolledBack, now - 1))
            .collect();
        let r = check_stop_condition(&changes, 0, now);
        assert_eq!(r.rolled_back_count, 5);
        assert!(r.should_stop);
        assert!(r.stop_reasons.contains(&StopReason::FiveOrMoreRollbacks));
    }

    #[test]
    fn non_terminal_changes_not_counted() {
        // Pending / Shadowing / ShadowPassed / Approved / Canary 不算「completed」
        let now = DAY_MS * 5;
        let changes: Vec<_> = [
            ChangeStatus::Pending,
            ChangeStatus::Shadowing,
            ChangeStatus::ShadowPassed,
            ChangeStatus::Approved,
            ChangeStatus::Canary,
        ]
        .iter()
        .enumerate()
        .map(|(i, s)| mk_change_at(&format!("p{i}"), *s, now - 1))
        .collect();
        let r = check_stop_condition(&changes, 0, now);
        assert_eq!(r.changes_completed, 0, "非终态不计 completed");
    }

    #[test]
    fn multiple_stop_reasons() {
        // 同时满足 30 completed + 5 rollbacks
        let now = DAY_MS * 5;
        let mut changes: Vec<_> = (0..25)
            .map(|i| mk_change_at(&format!("a{i}"), ChangeStatus::Active, now - 1))
            .collect();
        for i in 0..5 {
            changes.push(mk_change_at(
                &format!("rb{i}"),
                ChangeStatus::RolledBack,
                now - 1,
            ));
        }
        let r = check_stop_condition(&changes, 0, now);
        assert!(r.should_stop);
        assert!(r
            .stop_reasons
            .contains(&StopReason::CompletedChangesReach30));
        assert!(r.stop_reasons.contains(&StopReason::FiveOrMoreRollbacks));
    }

    /// OCR C3-3 回归：**窗外**（created_at_ms < start_ms 或 > now_ms）的 change 不得计入
    /// —— 否则 evolution-changes.jsonl 的历史行会立即误触发 CompletedChangesReach30 /
    /// FiveOrMoreRollbacks。
    #[test]
    fn out_of_window_changes_not_counted() {
        let now = DAY_MS * 5;
        let start = DAY_MS * 2;
        // 30 条终态，但全早于 start（窗外）
        let mut changes: Vec<_> = (0..30)
            .map(|i| mk_change_at(&format!("old{i}"), ChangeStatus::Active, DAY_MS))
            .collect();
        // 5 条回滚，但全晚于 now（窗外）
        for i in 0..5 {
            changes.push(mk_change_at(
                &format!("fut{i}"),
                ChangeStatus::RolledBack,
                now + DAY_MS,
            ));
        }
        let r = check_stop_condition(&changes, start, now);
        assert_eq!(r.changes_completed, 0, "窗外 change 不得计入 completed");
        assert_eq!(r.rolled_back_count, 0, "窗外 change 不得计入 rolled_back");
        assert!(!r.should_stop, "窗外 change 不得触发停止条件");
    }

    #[test]
    fn stop_threshold_constants_locked() {
        assert_eq!(STOP_COMPLETED_CHANGES, 30);
        assert_eq!(STOP_MAX_DAYS, 14.0);
        assert_eq!(STOP_MIN_ROLLED_BACK, 5);
    }

    #[test]
    fn stop_reason_str_locked() {
        // 锁死 grep 字符串
        assert!(StopReason::CompletedChangesReach30.as_str().contains("30"));
        assert!(StopReason::FourteenDaysElapsed.as_str().contains("14"));
        assert!(StopReason::FiveOrMoreRollbacks.as_str().contains("5"));
    }
}
