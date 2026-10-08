//! R4 L1 候选层 · TTL 14 天（候选条目过期时间计算）。
//!
//! spec：候选池 TTL = 14 天，超期转 Expired（软淘汰）。过期推进/硬淘汰
//! 函数零生产调用已删除；现行消费方只有 `compute_expires_at`
//! （derive 入池与决策板续期用）。历史实现见 git log。
//!
//! ## 时间源契约
//!
//! `created_at_ms` / `expires_at_ms` 必须与 `now_ms` 同源
//! （wall-clock Unix 毫秒，`chrono::Utc::now().timestamp_millis()`）。
//! 条目跨重启持久化在 jsonl，不可用单调时钟。已知脆弱性：NTP 回拨
//! 可能把刚入池条目提前判过期——14 天粒度下影响有限，接受。

/// TTL = 14 天（spec）
pub const TTL_DAYS: i64 = 14;
/// TTL 毫秒数
pub const TTL_MS: i64 = TTL_DAYS * 86_400_000;

/// 计算 expires_at_ms（候选条目创建/续期用）。
///
/// 饱和加法：created_at_ms 来自 jsonl（不可信输入），近 i64::MAX 的
/// 腐败/对抗值不会让结果 wrap 成负数——溢出时饱和到 i64::MAX =
/// **永不过期**（条目留存保审计轨迹），而非静默立刻过期被淘汰。
pub fn compute_expires_at(created_at_ms: i64) -> i64 {
    created_at_ms.saturating_add(TTL_MS)
}

#[cfg(test)]
mod tests {
    use super::super::entry::{ProposalEntry, ProposalStatus};
    use super::*;
    use crate::evolution::change::EvolutionLayer;
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk(id: &str, expires_at: i64) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer: EvolutionLayer::Policy,
            impact: ImpactLevel::Medium,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: "x".into(),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: "x".into(),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: 1_000,
            expires_at_ms: expires_at,
            status: ProposalStatus::Pooled,
        }
    }

    #[test]
    fn ttl_constants_locked() {
        assert_eq!(TTL_DAYS, 14);
        assert_eq!(TTL_MS, 14 * 86_400_000);
    }

    #[test]
    fn compute_expires_at_adds_ttl() {
        assert_eq!(compute_expires_at(0), TTL_MS);
        assert_eq!(
            compute_expires_at(1_000_000_000_000),
            1_000_000_000_000 + TTL_MS
        );
    }

    #[test]
    fn compute_expires_at_saturates_on_overflow() {
        // 腐败/对抗 created_at_ms 近 i64::MAX：饱和到 MAX（永不过期保轨迹），
        // 不得 wrap 成负数（那会让条目被瞬间判过期）
        assert_eq!(compute_expires_at(i64::MAX), i64::MAX);
        assert_eq!(compute_expires_at(i64::MAX - 1000), i64::MAX);
        let e = mk("corrupt", compute_expires_at(i64::MAX - 1000));
        assert!(e.expires_at_ms > 0);
    }
}
