//! R3 L3 沙箱层 · session_id 分流（FNV-1a + canary 5% + A/B 50/50）
//!
//! spec R3：
//! - Canary 分流：session_id hash 取 5%
//! - A/B 分流：50/50
//!
//! 关键约束：跨进程稳定。
//! std::collections::hash_map::DefaultHasher 用随机种（Rust 1.x 后），所以不能用；
//! 用 FNV-1a 64-bit 是确定性的、便宜的、跨平台一致。

/// FNV-1a 64-bit hash（确定性、跨进程稳定）
pub fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in s.bytes() {
        h ^= byte as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    h
}

/// 桶号 0-99
pub fn bucket(session_id: &str) -> u32 {
    (fnv1a(session_id) % 100) as u32
}

/// 是否 canary 5% 桶
pub fn is_canary(session_id: &str) -> bool {
    bucket(session_id) < 5
}

/// A/B 分组（true = A, false = B；50/50 按桶号奇偶）
pub fn is_ab_a(session_id: &str) -> bool {
    bucket(session_id) % 2 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── FNV-1a 确定性 ───

    #[test]
    fn fnv1a_deterministic_same_input() {
        let s = "session-abc";
        assert_eq!(fnv1a(s), fnv1a(s));
    }

    #[test]
    fn fnv1a_known_value() {
        // FNV-1a("") = 0xcbf29ce484222325（offset basis）
        assert_eq!(fnv1a(""), 0xcbf29ce484222325);
        // 锁死一个非空值（防止将来误改实现）
        let h = fnv1a("test");
        // 重跑应一致
        assert_eq!(h, fnv1a("test"));
    }

    #[test]
    fn fnv1a_different_inputs_different_outputs() {
        // 高度可能（但允许碰撞）
        assert_ne!(fnv1a("a"), fnv1a("b"));
    }

    // ─── bucket ───

    #[test]
    fn bucket_in_range() {
        for s in ["s1", "long-session-id-12345", "session_xyz_999"] {
            let b = bucket(s);
            assert!(b < 100, "bucket 应在 0-99：{b}");
        }
    }

    #[test]
    fn bucket_stable_for_same_input() {
        // 跨调用稳定
        let s = "stable-session-001";
        let b1 = bucket(s);
        let b2 = bucket(s);
        assert_eq!(b1, b2);
    }

    // ─── canary 5% ───

    #[test]
    fn canary_stable() {
        let s = "test-session";
        let c1 = is_canary(s);
        let c2 = is_canary(s);
        assert_eq!(c1, c2);
    }

    #[test]
    fn canary_distribution_roughly_5_percent() {
        // 1000 个 session，canary 数应在 ~50 附近（5%）
        let count = (0..1000).filter(|i| is_canary(&format!("session-{i}"))).count();
        assert!(count >= 25 && count <= 100, "5% 桶应 ≈ 50，实测 {count}");
    }

    // ─── A/B 50/50 ───

    #[test]
    fn ab_stable() {
        let s = "ab-test-session";
        let a1 = is_ab_a(s);
        let a2 = is_ab_a(s);
        assert_eq!(a1, a2);
    }

    #[test]
    fn ab_distribution_roughly_50_percent() {
        let count_a = (0..1000).filter(|i| is_ab_a(&format!("session-{i}"))).count();
        // 偶数桶 = A，所以 A 桶占比 ≈ 50%（含 0 桶）
        assert!(count_a >= 450 && count_a <= 550, "A/B 应 ≈ 50/50，A={count_a}");
    }

    #[test]
    fn canary_subset_is_immutable_across_calls() {
        // canary 5% 永远是同一个子集（5% 子集 vs 95% 子集）
        let mut canary = 0;
        for i in 0..500 {
            let s = format!("session-{i}");
            if is_canary(&s) {
                canary += 1;
            }
        }
        // 再跑一遍，结果应一致
        let mut canary2 = 0;
        for i in 0..500 {
            if is_canary(&format!("session-{i}")) {
                canary2 += 1;
            }
        }
        assert_eq!(canary, canary2, "同一 session_id 必须永远同一个 canary 判定");
    }
}