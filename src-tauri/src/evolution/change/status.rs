//! R2 L2 版本层 · 9 态状态机（spec R2 硬约束）
//!
//! 合法流转：
//!   Pending → Shadowing / Rejected
//!   Shadowing → ShadowPassed / Rejected
//!   ShadowPassed → Approved / Rejected
//!   Approved → Canary / Active / Rejected   (Approved → Active 是 skip-canary 配置)
//!   Canary → Active / Rejected
//!   Active → RolledBack / Expired
//!   任何非终态 → Expired (TTL 14 天)
//!
//! 非法（抛 Err）：
//!   Pending → Active (硬约束 ② 禁止绕过沙箱)
//!   Pending → Approved
//!   Shadowing → Active / Approved
//!   Rejected → * (终态)
//!   RolledBack → * (终态)
//!   Expired → * (终态)

use super::record::ChangeStatus;

/// 状态流转是否合法
pub fn can_transition(from: ChangeStatus, to: ChangeStatus) -> bool {
    use ChangeStatus::*;
    matches!(
        (from, to),
        // Pending
        (Pending, Shadowing)
        | (Pending, Rejected)
        // Shadowing
        | (Shadowing, ShadowPassed)
        | (Shadowing, Rejected)
        // ShadowPassed
        | (ShadowPassed, Approved)
        | (ShadowPassed, Rejected)
        // Approved
        | (Approved, Canary)
        | (Approved, Active)         // skip-canary 配置
        | (Approved, Rejected)
        // Canary
        | (Canary, Active)
        | (Canary, Rejected)
        // Active
        | (Active, RolledBack)
        | (Active, Expired)
        // 任何非终态 → Expired（TTL）
        | (Pending, Expired)
        | (Shadowing, Expired)
        | (ShadowPassed, Expired)
        | (Approved, Expired)
        | (Canary, Expired)
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

    // ─── 合法流转 ───

    #[test]
    fn pending_to_shadowing_ok() {
        assert!(can_transition(Pending, Shadowing));
        assert!(transition(Pending, Shadowing).is_ok());
    }

    #[test]
    fn pending_to_rejected_ok() {
        assert!(can_transition(Pending, Rejected));
    }

    #[test]
    fn shadowing_to_shadow_passed_ok() {
        assert!(can_transition(Shadowing, ShadowPassed));
    }

    #[test]
    fn shadow_passed_to_approved_ok() {
        assert!(can_transition(ShadowPassed, Approved));
    }

    #[test]
    fn approved_to_canary_ok() {
        assert!(can_transition(Approved, Canary));
    }

    #[test]
    fn approved_to_active_skip_canary_ok() {
        // R3 配置：可跳 canary 直 active
        assert!(can_transition(Approved, Active));
    }

    #[test]
    fn canary_to_active_ok() {
        assert!(can_transition(Canary, Active));
    }

    #[test]
    fn active_to_rolled_back_ok() {
        assert!(can_transition(Active, RolledBack));
    }

    #[test]
    fn any_non_terminal_to_expired_ok() {
        // TTL 到期
        for s in [Pending, Shadowing, ShadowPassed, Approved, Canary] {
            assert!(can_transition(s, Expired), "{s:?} -> Expired 应合法");
        }
    }

    // ─── 非法流转（硬约束 ② 禁止绕过沙箱）───

    #[test]
    fn pending_to_active_blocked() {
        // 硬约束 ②：Pending → Active 绕过沙箱，必须禁止
        assert!(!can_transition(Pending, Active));
        let e = transition(Pending, Active).unwrap_err();
        assert!(e.contains("非法"), "错误信息应说明非法：{e}");
    }

    #[test]
    fn pending_to_approved_blocked() {
        assert!(!can_transition(Pending, Approved));
    }

    #[test]
    fn shadowing_to_active_blocked() {
        assert!(!can_transition(Shadowing, Active));
    }

    #[test]
    fn shadowing_to_approved_blocked() {
        assert!(!can_transition(Shadowing, Approved));
    }

    #[test]
    fn shadow_passed_to_active_blocked() {
        assert!(!can_transition(ShadowPassed, Active));
    }

    #[test]
    fn shadow_passed_to_canary_blocked() {
        assert!(!can_transition(ShadowPassed, Canary));
    }

    #[test]
    fn canary_to_approved_blocked() {
        // 不能从 Canary 退回 Approved
        assert!(!can_transition(Canary, Approved));
    }

    // ─── 终态封锁 ───

    #[test]
    fn rejected_is_terminal() {
        for to in [Pending, Shadowing, ShadowPassed, Approved, Canary, Active, RolledBack, Expired] {
            assert!(!can_transition(Rejected, to), "Rejected -> {to:?} 应被禁");
        }
        assert!(Rejected.is_terminal());
    }

    #[test]
    fn rolled_back_is_terminal() {
        for to in [Pending, Shadowing, ShadowPassed, Approved, Canary, Active, Rejected, Expired] {
            assert!(!can_transition(RolledBack, to), "RolledBack -> {to:?} 应被禁");
        }
        assert!(RolledBack.is_terminal());
    }

    #[test]
    fn expired_is_terminal() {
        for to in [Pending, Shadowing, ShadowPassed, Approved, Canary, Active, Rejected, RolledBack] {
            assert!(!can_transition(Expired, to), "Expired -> {to:?} 应被禁");
        }
        assert!(Expired.is_terminal());
    }

    #[test]
    fn active_is_not_terminal_can_be_rolled_back() {
        // Active 可被回滚，所以不是严格终态
        assert!(!Active.is_terminal());
        assert!(can_transition(Active, RolledBack));
        assert!(can_transition(Active, Expired));
    }

    // ─── transition() 错误信息 ───

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

    // ─── 集成测试：完整生命周期 ───

    #[test]
    fn integration_full_lifecycle_pending_to_rolled_back() {
        // 集成验收：spec R2 「完整 status 生命周期」
        use ChangeStatus::*;
        // 合法路径 1：标准 canary 流程
        let path1 = vec![Pending, Shadowing, ShadowPassed, Approved, Canary, Active, RolledBack];
        for window in path1.windows(2) {
            assert!(
                can_transition(window[0], window[1]),
                "{:?} → {:?} 应合法（full lifecycle）",
                window[0],
                window[1]
            );
            assert!(transition(window[0], window[1]).is_ok());
        }
        // 合法路径 2：skip-canary
        let path2 = vec![Pending, Shadowing, ShadowPassed, Approved, Active, RolledBack];
        for window in path2.windows(2) {
            assert!(
                can_transition(window[0], window[1]),
                "{:?} → {:?} 应合法（skip-canary lifecycle）",
                window[0],
                window[1]
            );
        }
        // 合法路径 3：被拒绝
        let path3 = vec![Pending, Rejected];
        assert!(can_transition(path3[0], path3[1]));
        // 终态：Active → Expired
        assert!(can_transition(Active, Expired));
    }
}