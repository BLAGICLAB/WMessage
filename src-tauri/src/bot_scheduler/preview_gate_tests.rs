use super::*;

/// 预演闸只拦到点触发：手动「立即执行」必须真跑（预演档=到点先看后跑，
/// 不是拦下一切执行）；auto/NULL 档任何路径都不拦。
#[test]
fn gate_hits_scheduled_preview_only() {
    assert!(
        preview_gate_hits(Some(crate::db::JOB_MODE_PREVIEW), false),
        "到点触发 preview 档：拦下，只建提案卡"
    );
    assert!(
        !preview_gate_hits(Some(crate::db::JOB_MODE_PREVIEW), true),
        "手动「立即执行」必须绕过预演闸真跑"
    );
    assert!(!preview_gate_hits(Some(crate::db::JOB_MODE_AUTO), false));
    assert!(!preview_gate_hits(None, false));
    assert!(!preview_gate_hits(Some("off"), false));
}
