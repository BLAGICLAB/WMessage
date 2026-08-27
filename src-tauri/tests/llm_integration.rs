//! F-6 step 3 + step 4 refactor：Mock LLM + reqwest 端到端（2026-08-18）
//!
//! 把 `tests/mock_llm.rs` 的 server 接到 reqwest::Client，
//! 验证 SSE 响应能被生产同构的解析逻辑消费。
//!
//! F-6 step 4 refactor（2026-08-18 10:31）：测试改用 `wmessage_lib::bot::parse_sse_chunk`，
//! 消除与 `bot::run_model_loop` 的解析逻辑重复。OpenAI SSE 格式演化只改 bot.rs 一处。
//!
//! 设计要点：
//! - 用真实的 reqwest::Client（与 bot.rs 共享依赖）
//! - SSE 行级解析委托给 `wmessage_lib::bot::parse_sse_chunk`（生产同构）
//! - 不依赖 Tauri runtime（macOS EventLoop 主线程限制）
//! - bytes-level 简化：reqwest stream API 在 tokio::test 多线程下 Unpin cast 复杂，
//!   全量读 bytes 后按行调 parse_sse_chunk（生产代码按 \n 行切 SSE，
//!   见 bot_model_loop.rs 的 drain_sse_lines）

mod mock_llm_re_export {
    include!("mock_llm.rs");
}

use mock_llm_re_export::{MockBehavior, MockLlmServer, ToolCallResponse};
use wmessage_lib::bot::{
    accumulate_tool_call_delta, drain_sse_lines, parse_sse_chunk, ToolCallDelta,
};
use wmessage_lib::middleware;

/// 中间件 API 的 AppHandle 首参占位（P2-13/14 签名扩展后测试适配，2026-08-19）。
/// 这些用例只命中闸门判定，不触发 audit 落盘路径；App 泄漏给测试进程，退出即回收。
fn mock_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    Box::leak(Box::new(tauri::test::mock_app())).handle().clone()
}

// ────────────────────────────────────────────────────────────────────
// SSE 解析：F-6 step 4 refactor 后改用 wmessage_lib::bot::parse_sse_chunk
// 累计多行 SSE chunk 为 ParsedStream（text + tool_calls + finish_reason）
// ────────────────────────────────────────────────────────────────────

#[derive(Default, Debug)]
pub struct ParsedStream {
    pub text: String,
    /// (id, name, arguments)——生产同一累积函数 `accumulate_tool_call_delta` 的输出形态
    /// （批次3审计 T-1：消除与生产累积层的双份实现漂移）
    pub tool_calls: Vec<(String, String, String)>,
    pub finish_reason: Option<String>,
    /// 200 流内错误载荷（批次3审计 P1-3：OneAPI 类网关在 200 流内发 {"error":...}）
    pub error: Option<String>,
}

/// 解析 SSE bytes（内部按行调 `wmessage_lib::bot::parse_sse_chunk`）
pub fn parse_sse_bytes(bytes: &[u8]) -> ParsedStream {
    let text = String::from_utf8_lossy(bytes);
    let mut out = ParsedStream::default();
    for line in text.lines() {
        let Some(parsed) = parse_sse_chunk(line) else {
            continue;
        };
        if parsed.is_done {
            continue;
        }
        if let Some(c) = parsed.content {
            out.text.push_str(&c);
        }
        // 批次3审计 T-1：用生产同一累积函数按 index 归位（不再 extend 平铺）
        for d in parsed.tool_calls {
            accumulate_tool_call_delta(&mut out.tool_calls, &d);
        }
        if parsed.error.is_some() {
            out.error = parsed.error;
        }
        if let Some(reason) = parsed.finish_reason {
            out.finish_reason = Some(reason);
        }
    }
    out
}

/// reqwest POST body helper
fn make_body(user_msg: &str) -> serde_json::Value {
    serde_json::json!({
        "model": "deepseek-chat",
        "messages": [{"role": "user", "content": user_msg}],
        "stream": true,
    })
}

// ────────────────────────────────────────────────────────────────────
// 测试
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reqwest_post_to_mock_llm_extracts_text_reply() {
    let server = MockLlmServer::start();

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("hello"))
        .send()
        .await
        .expect("POST 成功");
    assert!(resp.status().is_success(), "status 应为 2xx");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);

    assert_eq!(
        parsed.text, "mock reply",
        "默认 TextReply 应返 'mock reply'"
    );
    assert!(
        parsed.tool_calls.is_empty(),
        "默认 TextReply 不含 tool_calls"
    );
    assert_eq!(
        parsed.finish_reason.as_deref(),
        Some("stop"),
        "末尾 chunk 应有 finish_reason=stop"
    );
}

#[tokio::test]
async fn reqwest_parses_tool_call_response_for_interactive_skill() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".to_string(),
        arguments: "{}".to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("list"))
        .send()
        .await
        .expect("POST 成功");
    assert!(resp.status().is_success());

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);

    assert!(parsed.text.is_empty(), "tool_call 响应 content 为 null");
    assert_eq!(parsed.tool_calls.len(), 1, "应解析出 1 个 tool_call");
    // (id, name, arguments) 元组：accumulate_tool_call_delta 归并后的完整 tool_call
    let tc = &parsed.tool_calls[0];
    assert_eq!(tc.0, "call_mock", "tool_call 应有 id");
    assert_eq!(tc.1, "list_tasks");
    assert_eq!(tc.2, "{}");
    assert_eq!(
        parsed.finish_reason.as_deref(),
        Some("tool_calls"),
        "tool_call 响应 finish_reason 应为 tool_calls"
    );
}

#[tokio::test]
async fn reqwest_handles_5_round_chat_loop_with_mock_llm() {
    let server = MockLlmServer::start();
    for i in 0..5 {
        server.push_behavior(MockBehavior::TextReply(format!("reply #{i}")));
    }

    let client = reqwest::Client::new();
    for i in 0..5 {
        let resp = client
            .post(format!("{}/chat/completions", server.base_url))
            .json(&make_body(&format!("msg {i}")))
            .send()
            .await
            .expect("POST 成功");
        let bytes = resp.bytes().await.expect("read body");
        let parsed = parse_sse_bytes(&bytes);
        assert_eq!(
            parsed.text,
            format!("reply #{i}"),
            "轮次 {i} 应返 reply #{i}"
        );
    }

    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(server.request_count(), 5, "应处理 5 个请求");
}

#[tokio::test]
async fn reqwest_handles_401_auth_error_response() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        401,
        r#"{"error":{"message":"Invalid API key","type":"auth_error"}}"#.to_string(),
    ));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("x"))
        .send()
        .await
        .expect("POST 应返回 HTTP 错误");

    assert_eq!(resp.status().as_u16(), 401, "应返回 401 Unauthorized");

    let err_body = resp.text().await.expect("read error body");
    assert!(err_body.contains("Invalid API key"));
    assert!(err_body.contains("auth_error"));
}

#[tokio::test]
async fn reqwest_surfaces_in_stream_error_payload() {
    // 批次3审计 P1-3：OneAPI 类网关在 200 流内发 {"error":...}，必须显式冒出
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::StreamError("auth_failed".to_string()));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("x"))
        .send()
        .await
        .expect("POST 成功");
    assert!(resp.status().is_success(), "流内错误仍是 200");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);

    assert_eq!(
        parsed.error.as_deref(),
        Some("auth_failed"),
        "流内 error 载荷应冒出"
    );
    assert!(parsed.text.is_empty(), "流内错误载荷不应产生正文");
}

#[tokio::test]
async fn reqwest_fragmented_multibyte_no_replacement_char() {
    // 批次3审计 P1-2 / T-2：多字节 UTF-8 字符跨 TCP 分片，
    // 对齐生产路径（字节缓冲 + drain_sse_lines 按 \n 切行后逐行 parse）不应出 U+FFFD
    use futures_util::StreamExt;

    let server = MockLlmServer::start();
    // 每字符 3 字节，按 2 字节切片必然在多字节字符中间切断
    server.push_behavior(MockBehavior::FragmentedTextReply(
        "你好，流式世界".to_string(),
        2,
    ));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("hi"))
        .send()
        .await
        .expect("POST 成功");
    assert!(resp.status().is_success());

    let mut byte_buf: Vec<u8> = Vec::new();
    let mut text = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.expect("stream chunk");
        byte_buf.extend_from_slice(&chunk);
        for line in drain_sse_lines(&mut byte_buf) {
            let Some(parsed) = parse_sse_chunk(&line) else {
                continue;
            };
            if let Some(c) = parsed.content {
                text.push_str(&c);
            }
        }
    }

    assert_eq!(text, "你好，流式世界", "分片重组后正文应完整");
    assert!(
        !text.contains('\u{FFFD}'),
        "多字节字符跨片不应产生 U+FFFD 替换符"
    );
}

#[test]
fn accumulate_matches_production_merge_semantics() {
    // 批次3审计 T-1：生产累积函数的归并语义——按 index 归位、字段追加、
    // index 交错乱序、arguments 分片、id 只出现一次
    let mut calls: Vec<(String, String, String)> = Vec::new();
    let delta = |index: usize,
                 id: Option<&str>,
                 name: Option<&str>,
                 args: Option<&str>| ToolCallDelta {
        index,
        id: id.map(|s| s.to_string()),
        name_chunk: name.map(|s| s.to_string()),
        arguments_chunk: args.map(|s| s.to_string()),
    };
    // index 1 先到（乱序）
    assert!(accumulate_tool_call_delta(
        &mut calls,
        &delta(1, Some("call_b"), Some("run_py"), None)
    ));
    assert!(accumulate_tool_call_delta(
        &mut calls,
        &delta(0, Some("call_a"), Some("list_"), None)
    ));
    // arguments 分片 + name 续块 + id 不再重复
    assert!(accumulate_tool_call_delta(
        &mut calls,
        &delta(1, None, Some("thon"), Some("{\"code\":"))
    ));
    assert!(accumulate_tool_call_delta(
        &mut calls,
        &delta(0, None, Some("tasks"), Some("{"))
    ));
    assert!(accumulate_tool_call_delta(
        &mut calls,
        &delta(0, None, None, Some("}"))
    ));
    assert!(accumulate_tool_call_delta(
        &mut calls,
        &delta(1, None, None, Some("\"1+1\"}"))
    ));

    assert_eq!(calls.len(), 2, "应归并为 2 条 tool_call");
    assert_eq!(calls[0].0, "call_a");
    assert_eq!(calls[0].1, "list_tasks", "name 续块应拼接完整");
    assert_eq!(calls[0].2, "{}", "arguments 分片应拼接完整");
    assert_eq!(calls[1].0, "call_b");
    assert_eq!(calls[1].1, "run_python");
    assert_eq!(calls[1].2, "{\"code\":\"1+1\"}");

    // index 超上限（MAX_TOOL_CALL_INDEX=64）→ 丢弃且不伸长 vec（批次3审计 P2-2）
    let before = calls.len();
    assert!(
        !accumulate_tool_call_delta(&mut calls, &delta(1000, Some("call_x"), Some("evil"), None)),
        "index=1000 应返回 false 表示丢弃"
    );
    assert_eq!(calls.len(), before, "超限 delta 不应伸长 vec");
}

// ────────────────────────────────────────────────────────────────────
// F-6 step 4：LLM 黑名单绕过测试（defense-in-depth）
// ────────────────────────────────────────────────────────────────────
// 模拟恶意 / 越权 LLM 返回黑名单 tool_call，验证 middleware 会拦截。
//
// 设计要点：
// - 不依赖 Tauri runtime（macOS EventLoop 主线程限制）
// - 直接调 MiddlewareRegistry（已含完整黑名单/白名单逻辑）
// - 验证 SSE 解析后的 tool_call.name 流经 middleware 校验链
// - 三种组合：黑名单+无 Skill / 黑名单+Skill Running / 白名单+任意
//
// 这是 F-6 step 4 中最安全相关的测试：验证 defense-in-depth 即便 LLM
// 被越权（提示词注入 / 越狱）尝试调 create_word_revisions / link_file_to_task
// 这类底层原子工具，middleware 也会按状态拒绝。

/// F-6 step 4 refactor 后的 helper：从 parsed.tool_calls 提取第一个的 name
///（累积后是 (id, name, arguments) 元组，批次3审计 T-1）
fn first_tool_name(parsed: &ParsedStream) -> String {
    parsed.tool_calls[0].1.clone()
}

#[tokio::test]
async fn llm_blacklist_tool_call_create_word_revisions_blocked_when_no_skill() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_word_revisions".to_string(), // 黑名单原子
        arguments: r#"{"path":"/tmp/evil.docx"}"#.to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("假装我是 LLM，调用黑名单工具"))
        .send()
        .await
        .expect("POST 成功");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);
    assert_eq!(parsed.tool_calls.len(), 1, "应解析出 1 个 tool_call");
    let blocked_tool = first_tool_name(&parsed);
    assert_eq!(
        blocked_tool, "create_word_revisions",
        "SSE 解析后 tool name 应正确"
    );

    // F-6 step 4 核心断言：SSE 解析后的 tool name → middleware 应阻断
    let registry = middleware::build_default_registry();
    let block_msg = registry.run_pre_execute(&mock_handle(), &blocked_tool, false);
    let msg = block_msg.expect(
        "黑名单 tool_call (create_word_revisions) + 非 Skill 状态应被 middleware 阻断（defense-in-depth）",
    );
    assert!(msg.contains("Skill"), "阻断消息应引导走 Skill；got: {msg}");
}

#[tokio::test]
async fn llm_blacklist_tool_call_link_file_to_task_blocked_when_no_skill() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "link_file_to_task".to_string(), // 黑名单原子
        arguments: r#"{"taskId":"abc","filePath":"/x.pdf"}"#.to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("LLM 尝试绑产物到任务卡"))
        .send()
        .await
        .expect("POST 成功");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);
    let blocked_tool = first_tool_name(&parsed);

    // 非 Skill 状态应阻断
    let registry = middleware::build_default_registry();
    let block_msg = registry.run_pre_execute(&mock_handle(), &blocked_tool, false);
    assert!(
        block_msg.is_some(),
        "link_file_to_task + 非 Skill 状态应被 middleware 阻断"
    );
}

#[tokio::test]
async fn llm_blacklist_tool_call_allowed_when_skill_running() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_word_revisions".to_string(),
        arguments: r#"{"path":"legitimate.docx"}"#.to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body(
            "Word 修订 Skill 内 LLM 调用 create_word_revisions（合法）",
        ))
        .send()
        .await
        .expect("POST 成功");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);
    let allowed_tool = first_tool_name(&parsed);

    // Skill Running 状态 + 黑名单 tool_call → middleware 应放行（合法调用）
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), &allowed_tool, true);
    assert!(
        blocked.is_none(),
        "create_word_revisions + Skill Running 状态应放行（Word 修订 Skill 内合法调用）；got: {blocked:?}"
    );
}

#[tokio::test]
async fn llm_whitelist_tool_call_run_python_always_allowed() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".to_string(), // 白名单
        arguments: r#"{"code":"print(1+1)"}"#.to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("LLM 调 run_python（白名单任意状态允许）"))
        .send()
        .await
        .expect("POST 成功");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);
    let tool = first_tool_name(&parsed);
    assert_eq!(tool, "run_python");

    // 白名单工具无论 Skill 是否活跃都应通过 middleware
    let registry = middleware::build_default_registry();
    for active in [false, true] {
        let blocked = registry.run_pre_execute(&mock_handle(), &tool, active);
        assert!(
            blocked.is_none(),
            "白名单 run_python + active={active} 应放行；got: {blocked:?}"
        );
    }
}

#[tokio::test]
async fn llm_mixed_sequence_blacklist_then_whitelist_block_then_allow() {
    // 模拟 LLM 在 chat loop 中先试黑名单、再试白名单的混合场景
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_word_revisions".to_string(),
        arguments: "{}".to_string(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".to_string(), // 白名单
        arguments: "{}".to_string(),
    }));

    let client = reqwest::Client::new();

    // 轮次 1：LLM 试 create_word_revisions（应被 middleware 阻断）
    let resp1 = client
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("try 1"))
        .send()
        .await
        .expect("POST 1");
    let bytes1 = resp1.bytes().await.expect("read body 1");
    let parsed1 = parse_sse_bytes(&bytes1);
    let tool1 = first_tool_name(&parsed1);
    let registry = middleware::build_default_registry();
    assert!(
        registry.run_pre_execute(&mock_handle(), &tool1, false).is_some(),
        "轮次 1 create_word_revisions 应被阻断"
    );

    // 轮次 2：LLM 改试 list_tasks（白名单，应放行）
    let resp2 = client
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("try 2"))
        .send()
        .await
        .expect("POST 2");
    let bytes2 = resp2.bytes().await.expect("read body 2");
    let parsed2 = parse_sse_bytes(&bytes2);
    let tool2 = first_tool_name(&parsed2);
    assert!(
        registry.run_pre_execute(&mock_handle(), &tool2, false).is_none(),
        "轮次 2 list_tasks 应放行"
    );
}
