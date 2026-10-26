//! 200 流内错误载荷的处置：可重试的走重发本轮，不可重试的直接报错。
//!
//! 回归背景：上游 Anthropic 兼容端点在 200 流内发 `error` 事件（正文没吐过
//! 一个 token 就报 500）。此前该出口既不重发、又把错误降级成不可恢复的
//! `INTERNAL`——上游明说「Please retry your request」，agent 却一次没试，
//! 用户还拿到「不可自动重试；请反馈日志」。
//!
//! 判据与流读取失败同源：**没向前端吐过内容才重发**（重发＝把用户看过的
//! 正文再来一遍）。

use super::*;

/// 上游 Anthropic 流内 error 事件的原文（真实端点的形态）。
/// 括号里的状态码是上游塞进 message 的——真实环境就是这么发的。
const UPSTREAM_500: &str = "Internal server error. Please retry your request. (500)";

/// 上游过载错误（Anthropic 协议的 `overloaded_error` type）。
const UPSTREAM_OVERLOADED: &str = "Overloaded";

/// 鉴权错误——重发多少次都一样，不该进重试网。
const UPSTREAM_AUTH: &str = "invalid x-api-key";

/// 可重试的上游错误（没吐过内容）→ 重发本轮，重发成功即正常收场。
#[tokio::test]
async fn stream_error_retryable_before_any_output_retries_the_round() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicStreamErrorTyped(
        "api_error".into(),
        UPSTREAM_500.into(),
    ));
    server.push_behavior(MockBehavior::AnthropicTextReply("重发成功了".into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let (text, _) = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await
    .expect("流内可重试错误应重发本轮后成功");

    assert_eq!(text, "重发成功了");
    assert_eq!(
        server.request_count(),
        2,
        "流内 500（没吐过内容）应重发本轮一次：原请求 + 重发"
    );
    assert!(
        h.events().contains(&"llm.stream_retry"),
        "应留 llm.stream_retry 审计：{:?}",
        h.events()
    );
}

/// 已经吐过内容再报错 → 不重发（重发就是给用户看两遍），直接失败。
#[tokio::test]
async fn stream_error_retryable_after_output_does_not_retry() {
    let server = MockLlmServer::start();
    // 先吐一段正文，再发 error 事件
    server.push_behavior(MockBehavior::RawSse(format!(
        "event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\
         \n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"已经吐出来了\"}}}}\n\
         \n\
         event: error\n\
         data: {{\"type\":\"error\",\"error\":{{\"type\":\"api_error\",\"message\":\"{UPSTREAM_500}\"}}}}\n\
         \n"
    )));
    server.push_behavior(MockBehavior::AnthropicTextReply("不该被用到".into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let err = run_model_loop_core(
        &core_http_anthropic(&server),
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
        err.to_string().contains("Internal server error"),
        "错误应带上游错误详情：{err}"
    );
    assert_eq!(
        server.request_count(),
        1,
        "已经吐过内容不得重发本轮（重发＝重复内容）"
    );
}

/// 不可重试的上游错误（鉴权）→ 不进重试网，一次即失败。
#[tokio::test]
async fn stream_error_non_retryable_does_not_retry() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicStreamErrorTyped(
        "authentication_error".into(),
        UPSTREAM_AUTH.into(),
    ));
    server.push_behavior(MockBehavior::AnthropicTextReply("不该被用到".into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let err = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await
    .expect_err("鉴权错误不该重发，应直接失败");

    assert!(
        err.to_string().contains("invalid x-api-key"),
        "错误应带上游错误详情：{err}"
    );
    assert_eq!(
        server.request_count(),
        1,
        "鉴权错误重发多少次都一样，不得重发"
    );
}

/// 分类归属：可重试的上游错误必须报成「可恢复」，
/// 否则前端不给「重试」入口，用户只能干等。
///
/// 此前流内错误走 blanket `From<String>` 降级成 `INTERNAL`——
/// `recoverable=false`，前端 hint 是「不可自动重试；请反馈日志」，
/// 与上游自己说的「Please retry your request」正好相反。
#[tokio::test]
async fn stream_error_retryable_is_recoverable() {
    let server = MockLlmServer::start();
    // 两次都失败：第一次触发重发，第二次才放弃
    server.push_behavior(MockBehavior::AnthropicStreamError(UPSTREAM_500.into()));
    server.push_behavior(MockBehavior::AnthropicStreamError(UPSTREAM_500.into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let err = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await
    .expect_err("两次都失败时应报错");

    assert!(
        err.is_recoverable(),
        "上游瞬时故障属可恢复错误（用户可重试），不得降级成不可恢复的 INTERNAL：{err}"
    );
}

/// 放弃重试时必须收尾技能——不留悬空 run。
#[tokio::test]
async fn stream_error_retry_exhausted_finishes_active_skill() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicStreamError(UPSTREAM_500.into()));
    server.push_behavior(MockBehavior::AnthropicStreamError(UPSTREAM_500.into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let _ = run_model_loop_core(
        &core_http_anthropic(&server),
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
            .any(|(ok, reason)| !*ok && reason.contains("大模型流内错误")),
        "放弃重试时必须收尾技能（不留悬空 run）：{calls:?}"
    );
}

/// 过载（overloaded_error）同样属可重试——上游明确说了重试。
#[tokio::test]
async fn stream_error_overloaded_is_retryable() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicStreamErrorTyped(
        "overloaded_error".into(),
        UPSTREAM_OVERLOADED.into(),
    ));
    server.push_behavior(MockBehavior::AnthropicTextReply("重发成功了".into()));

    let h = CoreHarness::new();
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("ok", Vec::new())
    };

    let (text, _) = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        &exec,
        noop_replan,
    )
    .await
    .expect("过载错误应重发本轮后成功");

    assert_eq!(text, "重发成功了");
    assert_eq!(server.request_count(), 2, "过载错误应重发本轮一次");
}
