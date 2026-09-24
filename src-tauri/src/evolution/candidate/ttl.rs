//! R4 L1 候选层 · TTL 14 天过期机制
//!
//! spec R4：候选池 evolution-proposals.jsonl TTL = 14 天
//! 超期后 status 转为 Expired（软淘汰，保留历史）。
//!
//! 软淘汰为默认路径；硬淘汰（evict_expired）只删 Pooled/Expired 条目——
//! Promoted/Rejected 超期也**永不硬删**（保审计轨迹，与 mark_expired 可组合）。
//! - mark_expired：把 status 改为 Expired（保留行）
//! - evict_expired：从 vec 中移除过期的 Pooled/Expired 行（非活跃状态的历史）
//!
//! ## 调用约定
//!
//! 典型顺序：**先 mark_expired 再 evict_expired**。evict 是不可逆硬删、
//! 无留痕——调用方应在 evict 前持久化被删条目的快照（当前零生产调用方，
//! 仅测试/re-export 触达）。
//!
//! ## 时间源契约
//!
//! 所有 `now_ms` 入参必须是与 `created_at_ms` / `expires_at_ms` **同源的
//! wall-clock Unix 毫秒**（`chrono::Utc::now().timestamp_millis()`）。
//! 条目跨重启持久化在 jsonl，故不可用单调时钟。已知脆弱性：NTP 回拨/
//! 时钟回跳可能把刚入池条目提前判过期——14 天 TTL 粒度下影响有限，
//! 接受该风险；调用方不得混用时间源。

use super::entry::{ProposalEntry, ProposalStatus};

/// TTL = 14 天（spec）
pub const TTL_DAYS: i64 = 14;
/// TTL 毫秒数
pub const TTL_MS: i64 = TTL_DAYS * 86_400_000;

/// 该 entry 是否已过期（expires_at_ms <= now_ms）
pub fn is_expired(entry: &ProposalEntry, now_ms: i64) -> bool {
    entry.expires_at_ms <= now_ms
}

/// 把过期条目的 status 改为 Expired（in-place）；返回标记数
pub fn mark_expired(entries: &mut Vec<ProposalEntry>, now_ms: i64) -> usize {
    let mut count = 0;
    for e in entries.iter_mut() {
        if e.status == ProposalStatus::Pooled && is_expired(e, now_ms) {
            e.status = ProposalStatus::Expired;
            count += 1;
        }
    }
    count
}

/// 硬淘汰：移除「过期且 status ∈ {Pooled, Expired}」的条目（返回淘汰数）。
/// Promoted / Rejected 超期不删——它们承载审计/回滚轨迹，硬删会丢历史。
pub fn evict_expired(entries: &mut Vec<ProposalEntry>, now_ms: i64) -> usize {
    let before = entries.len();
    entries.retain(|e| {
        !(is_expired(e, now_ms)
            && matches!(e.status, ProposalStatus::Pooled | ProposalStatus::Expired))
    });
    before - entries.len()
}

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
    fn is_expired_true_when_now_past_expiry() {
        let e = mk("a", 1000);
        assert!(is_expired(&e, 1001));
        assert!(is_expired(&e, 1000)); // 边界：now == expires_at 也算过期
    }

    #[test]
    fn is_expired_false_when_before_expiry() {
        let e = mk("a", 1000);
        assert!(!is_expired(&e, 999));
    }

    #[test]
    fn mark_expired_only_affects_pooled() {
        let mut entries = vec![
            mk("a", 100),  // expired + Pooled → 改 Expired
            mk("b", 100),  // expired + Pooled → 改 Expired
            mk("c", 5000), // not expired → 不动
        ];
        let n = mark_expired(&mut entries, 1000);
        assert_eq!(n, 2);
        assert_eq!(entries[0].status, ProposalStatus::Expired);
        assert_eq!(entries[1].status, ProposalStatus::Expired);
        assert_eq!(entries[2].status, ProposalStatus::Pooled);
    }

    #[test]
    fn mark_expired_skips_non_pooled() {
        // 已 Promoted / Expired / Rejected 不再改
        let mut entries = vec![
            {
                let mut e = mk("a", 100);
                e.status = ProposalStatus::Promoted;
                e
            },
            {
                let mut e = mk("b", 100);
                e.status = ProposalStatus::Rejected;
                e
            },
        ];
        let n = mark_expired(&mut entries, 1000);
        assert_eq!(n, 0);
        assert_eq!(entries[0].status, ProposalStatus::Promoted);
        assert_eq!(entries[1].status, ProposalStatus::Rejected);
    }

    #[test]
    fn evict_expired_removes_only_expired() {
        let mut entries = vec![mk("a", 100), mk("b", 5000), mk("c", 100)];
        let n = evict_expired(&mut entries, 1000);
        assert_eq!(n, 2);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].proposal_id, "b");
    }

    #[test]
    fn evict_expired_never_drops_promoted_or_rejected() {
        // 过期 Promoted/Rejected 保审计轨迹，永不硬删
        let mut promoted = mk("p", 100);
        promoted.status = ProposalStatus::Promoted;
        let mut rejected = mk("r", 100);
        rejected.status = ProposalStatus::Rejected;
        let mut expired = mk("e", 100);
        expired.status = ProposalStatus::Expired;
        let mut entries = vec![promoted, rejected, expired, mk("pool", 100)];
        let n = evict_expired(&mut entries, 1000);
        assert_eq!(n, 2); // 只删 expired + pooled
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.proposal_id == "p"));
        assert!(entries.iter().any(|e| e.proposal_id == "r"));
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
        // 不得 wrap 成负数（那会让 is_expired 恒 true = 静默立刻淘汰）
        assert_eq!(compute_expires_at(i64::MAX), i64::MAX);
        assert_eq!(compute_expires_at(i64::MAX - 1000), i64::MAX);
        let e = mk("corrupt", compute_expires_at(i64::MAX - 1000));
        assert!(!is_expired(&e, 0)); // 固定 now_ms，不依赖墙钟
    }
}
