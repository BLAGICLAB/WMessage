//! 模型循环单元测试（request 桶）：发送段失败的分层文案。

use crate::bot_model_loop::request::{send_failure_user_text, SendFail};

/// 发送段用户文案：成因短语 + 重试指引，原始 reqwest 串不出现（读侧同口径）。
#[test]
fn send_failure_user_text_is_layered_with_retry_guidance() {
    let t = send_failure_user_text("连接失败");
    assert!(t.contains("网络连接中断（连接失败）"));
    assert!(t.contains("「重试」"));
    assert!(!t.contains("error sending request"));
    assert!(!t.contains("reqwest"));
}

/// 响应头超时分支：用户文案走同一分层句式；审计 raw 保留旧判据串
/// （日志对账口径不变——超时秒数只在 raw 里）。
#[test]
fn send_fail_header_timeout_splits_user_text_and_raw() {
    let f = SendFail::header_timeout(60);
    assert!(f.user_text.contains("网络连接中断（响应头 60s 无响应）"));
    assert!(f.user_text.contains("「重试」"));
    assert_eq!(f.raw, "响应头超时（60s 无响应，判定服务端挂起）");
}
