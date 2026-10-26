//! 流读取失败的两条处置：重发本轮 / 收尾技能。
//!
//! 回归背景：SSE 读侧两条错误出口（idle 超时 / 连接中断）原先直接穿出循环体，
//! 活动技能 run 停在 Running —— 下一条消息进来还会被当成本会话的活动技能推进，
//! 技能一被暂停/终止就整段作废（用户看到的是「什么也没干」）。

use super::*;

/// 一条完整 data 行（正文已吐给前端），后接截断。
/// 用于「已经吐过内容 → 不得重发」的判据。
const EMITTED_CHUNK: &str = r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"mock-model","choices":[{"delta":{"role":"assistant","content":"已经吐出来了"},"index":0,"finish_reason":null}]}

"#;

/// 断流且什么都没吐 → 重发本轮，重发成功即正常收场。
///
/// 「什么都没吐」= body 里没有完整的 data 行（解析不出任何 delta）。
#[tokio::test]
async fn stream_failure_before_any_output_retries_the_round() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TruncatedStream(String::new()));
    server.push_behavior(MockBehavior::TextReply("重发成功了".into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await
    .expect("断流后重发本轮应成功");

    assert_eq!(text, "重发成功了");
    assert_eq!(
        server.request_count(),
        2,
        "断流（没吐过内容）应重发本轮一次：原请求 + 重发"
    );
    assert!(
        h.events().contains(&"llm.stream_retry"),
        "应留 llm.stream_retry 审计：{:?}",
        h.events()
    );
}

/// 已经吐过内容 → 不重发（重发就是把用户看过的正文再来一遍），直接失败。
#[tokio::test]
async fn stream_failure_after_output_does_not_retry() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TruncatedStream(EMITTED_CHUNK.to_string()));
    server.push_behavior(MockBehavior::TextReply("不该被用到".into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let err = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await
    .expect_err("已吐过内容时不得重发，应直接失败");

    assert!(
        err.to_string().contains("流式读取失败"),
        "错误应说明是流式读取失败：{err}"
    );
    assert_eq!(
        server.request_count(),
        1,
        "已经吐过内容不得重发本轮（重发＝重复内容）"
    );
}

/// 放弃重试时必须收尾技能——不留悬空 run。
#[tokio::test]
async fn stream_failure_finishes_active_skill() {
    let server = MockLlmServer::start();
    // 两次都断：第一次触发重发，第二次才放弃
    server.push_behavior(MockBehavior::TruncatedStream(String::new()));
    server.push_behavior(MockBehavior::TruncatedStream(String::new()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let _ = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await;

    let calls = h.skill_finish_calls();
    assert!(
        calls
            .iter()
            .any(|(ok, reason)| !*ok && reason.contains("流式响应中断")),
        "放弃重试时必须收尾技能（不留悬空 run）：{calls:?}"
    );
}
