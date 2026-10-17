//!  L2 版本层 · 状态流转校验
//!
//! 合法流转：
//!   Pending → Active（自动应用落记忆） / Rejected（合规或人工拒绝）/ Expired（TTL）
//!   Active → RolledBack（回滚） / Expired
//!   任何非终态 → Expired (TTL 14 天)
//!
//! 非法（抛 Err）：
//!   Active → Pending / Rejected（已生效变更只能回滚或到期，不可直接退回）
//!   Rejected / RolledBack / Expired → *（终态）
//!
//! 影子测试 / 批准 / 金丝雀三态已删：它们零生产写入点，而「必须先过批准态」
//! 这条硬约束真正由 `ApprovalSource` 与 `hard_constraint_compliance` 两个字段
//! 承担（见 record.rs），本表不再重复表达。

use super::record::ChangeStatus;

/// 状态流转是否合法（`transition` 内部校验用；模块内私有）。
///
/// Active 的退出只有 RolledBack / Expired（**无** Active→Rejected，有意设计：
/// 已生效变更只能回滚或到期，不可直接拒）。
fn can_transition(from: ChangeStatus, to: ChangeStatus) -> bool {
    use ChangeStatus::*;
    matches!(
        (from, to),
        (Pending, Active)
            | (Pending, Rejected)
            | (Pending, Expired)
            | (Active, RolledBack)
            | (Active, Expired)
    )
}

/// 状态流转入口：合法 → Ok(()), 非法 → Err
pub fn transition(from: ChangeStatus, to: ChangeStatus) -> Result<(), String> {
    if from == to {
        return Err(format!("状态未变：{from:?}"));
    }
    if from.is_terminal() {
        return Err(format!("终态不可流转：{from:?} -> {to:?}"));
    }
    if can_transition(from, to) {
        Ok(())
    } else {
        Err(format!("非法 status 流转：{from:?} -> {to:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ChangeStatus::*;

    // 合法流转

    #[test]
    fn pending_can_reach_every_live_state() {
        // 生产只有这三条出边：自动应用生效 / 合规拒绝 / TTL 到期
        for to in [Active, Rejected, Expired] {
            assert!(can_transition(Pending, to), "Pending -> {to:?} 应合法");
            assert!(transition(Pending, to).is_ok());
        }
    }

    #[test]
    fn active_can_rollback_or_expire_only() {
        assert!(can_transition(Active, RolledBack));
        assert!(can_transition(Active, Expired));
        // 已生效变更不可直接退回候选 / 被拒
        assert!(!can_transition(Active, Pending));
        assert!(!can_transition(Active, Rejected));
    }

    // 终态封锁

    #[test]
    fn terminal_states_never_transition_again() {
        for from in [Rejected, RolledBack, Expired] {
            assert!(from.is_terminal(), "{from:?} 应是终态");
            for to in [Pending, Active, Rejected, RolledBack, Expired] {
                assert!(!can_transition(from, to), "{from:?} -> {to:?} 应被禁");
            }
        }
    }

    #[test]
    fn active_is_not_terminal() {
        // Active 可被回滚 / 到期，所以不是严格终态
        assert!(!Active.is_terminal());
        assert!(!Pending.is_terminal());
    }

    // transition() 错误信息

    #[test]
    fn transition_same_state_errors() {
        let e = transition(Pending, Pending).unwrap_err();
        assert!(e.contains("未变"), "同状态应报错：{e}");
    }

    #[test]
    fn transition_terminal_errors() {
        let e = transition(Rejected, Pending).unwrap_err();
        assert!(e.contains("终态"), "终态应报错：{e}");
    }

    #[test]
    fn transition_illegal_edge_errors() {
        let e = transition(Active, Pending).unwrap_err();
        assert!(e.contains("非法"), "非法流转应报错：{e}");
    }

    // 集成：自动应用路径 Pending → Active 是唯一让 CR 进入 Active 的合法边

    #[test]
    fn full_lifecycle_pending_to_rolled_back() {
        let path = [Pending, Active, RolledBack];
        for window in path.windows(2) {
            assert!(
                can_transition(window[0], window[1]),
                "{:?} → {:?} 应合法",
                window[0],
                window[1]
            );
            assert!(transition(window[0], window[1]).is_ok());
        }
    }
}
