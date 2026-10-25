//! 截断摘要复用水位的纯函数面：指纹稳定性、会话隔离、指纹失配 miss。
//! wrapper 的缓存接线（命中免一次摘要请求）依赖这里的判定，行为面由
//! 实机 `chat.summary_reused` 审计行验证。

use super::summary::{cached_summary, dropped_fingerprint, store_summary_reuse};
use super::*;

fn msg(role: &str, content: &str) -> ChatMsg {
    ChatMsg {
        role: role.into(),
        content: content.into(),
    }
}

#[test]
fn fingerprint_is_stable_for_identical_set_and_sensitive_to_change() {
    let a = vec![msg("user", "甲"), msg("assistant", "乙")];
    let b = vec![msg("user", "甲"), msg("assistant", "乙")];
    assert_eq!(dropped_fingerprint(&a), dropped_fingerprint(&b));
    let changed = vec![msg("user", "甲"), msg("assistant", "丙")];
    assert_ne!(dropped_fingerprint(&a), dropped_fingerprint(&changed));
    let longer = vec![msg("user", "甲"), msg("assistant", "乙"), msg("user", "丙")];
    assert_ne!(dropped_fingerprint(&a), dropped_fingerprint(&longer));
}

#[test]
fn reuse_cache_hits_same_set_within_session_and_misses_across() {
    let dropped = vec![msg("user", "湖泊生态观察"), msg("assistant", "已读")];
    store_summary_reuse(Some("sess-reuse-a"), &dropped, "早前聊了湖泊生态");
    assert_eq!(
        cached_summary(Some("sess-reuse-a"), &dropped).as_deref(),
        Some("早前聊了湖泊生态")
    );
    // 同会话 dropped 集合变化（历史增长挤进截断区）→ 指纹失配，必须重摘要
    let grown = vec![
        msg("user", "湖泊生态观察"),
        msg("assistant", "已读"),
        msg("user", "新一段"),
    ];
    assert_eq!(cached_summary(Some("sess-reuse-a"), &grown), None);
    // 别的会话 / 无会话 → 不复用
    assert_eq!(cached_summary(Some("sess-reuse-b"), &dropped), None);
    assert_eq!(cached_summary(None, &dropped), None);
}

#[test]
fn reuse_cache_overwrite_tracks_latest_summary() {
    let dropped = vec![msg("user", "同一段")];
    store_summary_reuse(Some("sess-reuse-c"), &dropped, "旧摘要");
    store_summary_reuse(Some("sess-reuse-c"), &dropped, "新摘要");
    assert_eq!(
        cached_summary(Some("sess-reuse-c"), &dropped).as_deref(),
        Some("新摘要")
    );
}
