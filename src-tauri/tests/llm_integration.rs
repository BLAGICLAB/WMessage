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
    Box::leak(Box::new(tauri::test::mock_app()))
        .handle()
        .clone()
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

// reqwest_handles_401_auth_error_response 已删除：只测 reqwest 库行为（弱断言），
// 其覆盖点（mock 401 → 状态/错误体可见）已被 core_http_401_wrapped_no_retry 经
// run_model_loop_core 真路径覆盖（且多验证「401 不重试、包装为 LlmApiError」）。

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
    let delta =
        |index: usize, id: Option<&str>, name: Option<&str>, args: Option<&str>| ToolCallDelta {
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
// 被越权（提示词注入 / 越狱）尝试调 link_file_to_task
// 这类底层原子工具，middleware 也会按状态拒绝。
// （create_word_revisions 2026-09-02 起移出黑名单，去 Skill 化，聊天直调放行）

/// F-6 step 4 refactor 后的 helper：从 parsed.tool_calls 提取第一个的 name
///（累积后是 (id, name, arguments) 元组，批次3审计 T-1）
fn first_tool_name(parsed: &ParsedStream) -> String {
    parsed.tool_calls[0].1.clone()
}

/// 2026-09-02 老板拍板：create_word_revisions 去 Skill 化（移出原子黑名单）——
/// 回归锁：聊天态（无活动 Skill）直接调用也必须放行，绝不能再被 middleware 拦回
/// 「不允许裸调」（否则修订模式在聊天里又失效）。
#[tokio::test]
async fn llm_create_word_revisions_allowed_without_skill() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_word_revisions".to_string(), // 已移出黑名单
        arguments: r#"{"originalPath":"/tmp/a.docx","revised":["润色后"]}"#.to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body("润色这个 Word"))
        .send()
        .await
        .expect("POST 成功");

    let bytes = resp.bytes().await.expect("read body");
    let parsed = parse_sse_bytes(&bytes);
    assert_eq!(parsed.tool_calls.len(), 1, "应解析出 1 个 tool_call");
    let tool = first_tool_name(&parsed);
    assert_eq!(tool, "create_word_revisions", "SSE 解析后 tool name 应正确");

    // 非 Skill 状态（聊天直调）也必须放行
    let registry = middleware::build_default_registry();
    let blocked = registry.run_pre_execute(&mock_handle(), &tool, false);
    assert!(
        blocked.is_none(),
        "create_word_revisions 已移出黑名单，非 Skill 状态应放行；got: {blocked:?}"
    );
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
        name: "link_file_to_task".to_string(),
        arguments: r#"{"taskId":"t1","path":"legitimate.docx"}"#.to_string(),
    }));

    let resp = reqwest::Client::new()
        .post(format!("{}/chat/completions", server.base_url))
        .json(&make_body(
            "Skill 内 LLM 调用 link_file_to_task 绑产物（合法）",
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
        "link_file_to_task + Skill Running 状态应放行（Skill 内合法调用）；got: {blocked:?}"
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
        name: "link_file_to_task".to_string(),
        arguments: "{}".to_string(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".to_string(), // 白名单
        arguments: "{}".to_string(),
    }));

    let client = reqwest::Client::new();

    // 轮次 1：LLM 试 link_file_to_task（应被 middleware 阻断）
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
        registry
            .run_pre_execute(&mock_handle(), &tool1, false)
            .is_some(),
        "轮次 1 link_file_to_task 应被阻断"
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
        registry
            .run_pre_execute(&mock_handle(), &tool2, false)
            .is_none(),
        "轮次 2 list_tasks 应放行"
    );
}

// ────────────────────────────────────────────────────────────────────
// T1-2（2026-09-03，原审计 #3）：run_model_loop_core 真路径集成测试
//
// 背景：P0-1/P0-5 等修复密集区（HTTP 错误包装 / 流截断防线 / 重试退避）
// 原先沉在 run_model_loop 函数体内、直连 AppHandle，无法集成测试。
// 重构后核心 loop 可注入 LlmHttp + 副作用出口（ModelLoopDeps），
// 以下用例全部对 mock LLM server 跑生产同一份代码。
// ────────────────────────────────────────────────────────────────────

use std::sync::{Arc, Mutex};
use wmessage_lib::bot::{
    noop_replan, run_model_loop_core, ApiProvider, AuditLevel, LlmHttp, ModelLoopDeps, StopGuard,
    DEFAULT_MAX_TOKENS,
};
use wmessage_lib::error::CommandError;

/// 指向 mock server 的连接参数（与生产薄壳同一构造路径：trim_end_matches('/') 等
/// 在 core 内处理，这里直接给 base_url）
fn core_http(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        client: reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("build client"),
        base_url: server.base_url.clone(),
        api_key: "test-key".into(),
        model: "mock-model".into(),
        provider: ApiProvider::Openai,
        max_tokens: DEFAULT_MAX_TOKENS,
    }
}

/// Anthropic 协议变体（2026-09-05 Anthropic 兼容模式）：同一 mock server，
/// 主循环应打 /v1/messages 并用 x-api-key 鉴权
fn core_http_anthropic(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        provider: ApiProvider::Anthropic,
        ..core_http(server)
    }
}

/// 收集结构化审计事件名
type AuditEvents = Arc<Mutex<Vec<&'static str>>>;

struct CoreHarness {
    stop: StopGuard,
    emit: Box<dyn Fn(&str, serde_json::Value) + Send + Sync>,
    audit: Box<dyn Fn(AuditLevel, &'static str, Vec<(&'static str, String)>) + Send + Sync>,
    audit_log: Box<dyn Fn(&str) + Send + Sync>,
    skill_finish: Box<dyn Fn(bool, &str) -> String + Send + Sync>,
    audit_events: AuditEvents,
}

impl CoreHarness {
    fn new() -> Self {
        let events: AuditEvents = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        Self {
            // 非交互实例：不发流式事件，stop 标志未置位
            stop: StopGuard::new(false, None),
            emit: Box::new(|_, _| {}),
            audit: Box::new(move |_, event, _| {
                events2.lock().unwrap().push(event);
            }),
            audit_log: Box::new(|_| {}),
            skill_finish: Box::new(|_, _| String::new()),
            audit_events: events,
        }
    }

    fn deps(&self) -> ModelLoopDeps<'_> {
        ModelLoopDeps {
            emit: &*self.emit,
            audit: &*self.audit,
            audit_log: &*self.audit_log,
            skill_finish: &*self.skill_finish,
        }
    }

    fn events(&self) -> Vec<&'static str> {
        self.audit_events.lock().unwrap().clone()
    }
}

fn user_msgs() -> Vec<serde_json::Value> {
    vec![serde_json::json!({"role": "user", "content": "hi"})]
}

/// 工具执行 stub 永不调用版（纯文本/错误路径不应触发任何工具执行）
async fn exec_never(name: String, args: String) -> (String, Vec<wmessage_lib::bot::TaskRef>) {
    panic!("此路径不应执行工具：{name} {args}");
}

#[tokio::test]
async fn core_text_reply_happy_path() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("你好，世界".into()));
    let h = CoreHarness::new();

    let (text, refs) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect("纯文本回复应 Ok");

    assert_eq!(text, "你好，世界");
    assert!(refs.is_empty());
    assert_eq!(server.request_count(), 1);
    let events = h.events();
    assert!(
        events.contains(&"llm.request") && events.contains(&"llm.response"),
        "应记 llm.request/response 审计：{events:?}"
    );
}

#[tokio::test]
async fn core_tool_call_round_trip_executes_and_continues() {
    // 轮次 1：模型请求 list_tasks → 注入 executor 执行 → 结果回填 →
    // 轮次 2：模型给最终文本。覆盖工具编排 + tool 消息协议回填真路径。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply("共 2 个任务".into()));
    let h = CoreHarness::new();

    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |name: String, args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((name, args));
            ("共 2 个任务明细".into(), Vec::new())
        }
    };

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("工具回路应 Ok");

    assert_eq!(text, "共 2 个任务");
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[("list_tasks".to_string(), "{}".to_string())],
        "注入 executor 应被以解析出的 (name, arguments) 调用一次"
    );
    assert_eq!(server.request_count(), 2, "工具执行后应续聊第二轮");
}

#[tokio::test]
async fn core_http_401_wrapped_no_retry() {
    // 401 不在重试白名单（is_retryable_llm_status）：一次即败，包装成 LlmApiError
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        401,
        r#"{"error":{"message":"Invalid API key","type":"auth_error"}}"#.into(),
    ));
    let h = CoreHarness::new();

    let err = match run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("401 应报错"),
    };

    match err {
        CommandError::LlmApiError {
            status,
            body_preview,
        } => {
            assert_eq!(status, 401);
            assert!(
                body_preview.contains("Invalid API key"),
                "body_preview 应含服务端错误详情（截 300 字）：{body_preview}"
            );
        }
        other => panic!("401 应包装为 LlmApiError，got {other:?}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(server.request_count(), 1, "401 不可重试，只发 1 次请求");
}

#[tokio::test]
async fn core_http_429_retried_once_then_success() {
    // 重试退避（MAX_LLM_ATTEMPTS=2，P1-4）：429 第一次 → 记 llm.retry → 第二次成功
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        429,
        r#"{"error":{"message":"rate limited"}}"#.into(),
    ));
    server.push_behavior(MockBehavior::TextReply("重试后成功".into()));
    let h = CoreHarness::new();

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect("429 重试后成功应 Ok");

    assert_eq!(text, "重试后成功");
    assert_eq!(server.request_count(), 2, "429 应重试一次");
    assert!(
        h.events().contains(&"llm.retry"),
        "重试应记 llm.retry 审计：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_http_500_exhausts_attempts_then_error() {
    // 重试预算耗尽（MAX_LLM_ATTEMPTS=2）：连续 500 → 第二次不再重试，包装报错
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(500, r#"{"error":"boom"}"#.into()));
    server.push_behavior(MockBehavior::HttpError(
        500,
        r#"{"error":"boom again"}"#.into(),
    ));
    server.push_behavior(MockBehavior::TextReply("不应到达".into()));
    let h = CoreHarness::new();

    let err = match run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("500 连续两次应报错"),
    };

    match err {
        CommandError::LlmApiError { status, .. } => assert_eq!(status, 500),
        other => panic!("500 应包装为 LlmApiError，got {other:?}"),
    }
    assert_eq!(
        server.request_count(),
        2,
        "MAX_LLM_ATTEMPTS=2：恰好 2 次请求后放弃"
    );
}

#[tokio::test]
async fn core_in_stream_error_payload_surfaced() {
    // P1-3：200 流内错误载荷（OneAPI 类网关）必须显式报错，不再返回空白回复
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::StreamError("auth_failed".into()));
    let h = CoreHarness::new();

    let err = match run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("流内错误载荷应报错"),
    };

    let msg = err.to_string();
    assert!(
        msg.contains("大模型返回错误") && msg.contains("auth_failed"),
        "错误应带流内错误详情：{msg}"
    );
    assert!(
        h.events().contains(&"llm.stream_error"),
        "应记 llm.stream_error 审计：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_truncated_stream_partial_tool_calls_rejected() {
    // P1-1 防线：干净 EOF（无 [DONE] / 无 finish_reason）+ 残缺 tool_calls →
    // 拒绝执行工具，显式报「流被截断」（原先靠 parse_args 失败侥幸兜底）
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::RawSse(
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_x\",\"function\":{\"name\":\"delete_task\",\"arguments\":\"{}\"}}]}}]}\n\n"
            .into(),
    ));
    let h = CoreHarness::new();

    let err = match run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never, // 关键：残缺 tool_calls 绝不得执行（exec_never 被调即 panic）
        noop_replan,
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("截断流 + 残缺 tool_calls 应报错"),
    };

    let msg = err.to_string();
    assert!(
        msg.contains("大模型响应中断（流被截断），工具调用未执行"),
        "got: {msg}"
    );
    assert!(
        h.events().contains(&"llm.stream_truncated"),
        "应记 llm.stream_truncated 审计：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_truncated_stream_text_only_warns_but_returns() {
    // P1-1 防线另一支：干净 EOF 无 tool_calls 但有正文 → 不报错，
    // 正文追加「可能被截断」提示返回（用户已看到的内容不丢弃）
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::RawSse(
        "data: {\"choices\":[{\"delta\":{\"content\":\"半截回复\"}}]}\n\n".into(),
    ));
    let h = CoreHarness::new();

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect("纯文本截断应放行返回");

    assert!(text.contains("半截回复"));
    assert!(text.contains("响应可能被截断"), "应追加截断提示：{text}");
}

// ────────────────────────────────────────────────────────────────────
// 2026-09-05 Anthropic 兼容模式：run_model_loop_core 的 Anthropic 分支
// 跑真路径（同一 mock server，Anthropic 协议应答变体）。
// 覆盖：流式文本 + usage 审计 + 协议路径/请求体形态断言、
//       一次工具调用全回路（tool_use 解析 → 执行 → tool_result 回填 → 续聊）、
//       流内 error 事件冒出。
// ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn core_anthropic_text_reply_happy_path() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicTextReply("你好，Claude".into()));
    let h = CoreHarness::new();

    let (text, refs) = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect("Anthropic 纯文本回复应 Ok");

    assert_eq!(text, "你好，Claude");
    assert!(refs.is_empty());
    assert_eq!(server.request_count(), 1);
    // 协议路径断言：打 /v1/messages 而不是 /chat/completions
    let paths = server.request_paths();
    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].contains("POST /v1/messages"),
        "Anthropic 模式应打 /v1/messages：{paths:?}"
    );
    // 请求体形态：Anthropic 形状（max_tokens 必填 + stream:true），无 OpenAI 的扁平 messages+tools 同层
    let body = &server.request_bodies()[0];
    let v: serde_json::Value = serde_json::from_str(body).expect("请求体应为合法 JSON");
    assert_eq!(
        v["max_tokens"],
        serde_json::json!(8192),
        "Anthropic 必填 max_tokens"
    );
    assert_eq!(v["stream"], serde_json::json!(true));
    assert_eq!(v["messages"][0]["role"], serde_json::json!("user"));
    assert!(
        v["messages"][0]["content"].is_array(),
        "Anthropic 消息 content 应为块数组：{body}"
    );
    // tools 转 Anthropic 形态（name/input_schema 平铺，无 type:"function" 包装）
    assert!(v["tools"][0].get("name").is_some());
    assert!(v["tools"][0].get("input_schema").is_some());
    assert!(v["tools"][0].get("function").is_none());
    // usage 审计：message_start 的 input=12 + message_delta 的 output=7
    let events = h.events();
    assert!(
        events.contains(&"llm.request")
            && events.contains(&"llm.response")
            && events.contains(&"llm.usage"),
        "应记 llm.request/response/usage 审计：{events:?}"
    );
}

#[tokio::test]
async fn core_anthropic_tool_call_round_trip_executes_and_continues() {
    // 轮次 1：Anthropic tool_use（stop_reason=tool_use）→ 执行 → tool_result 回填 →
    // 轮次 2：纯文本。覆盖 tool_use 解析 + OpenAI 形状历史 → Anthropic 回填转换真路径。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::AnthropicTextReply("共 2 个任务".into()));
    let h = CoreHarness::new();

    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |name: String, args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((name, args));
            ("共 2 个任务明细".into(), Vec::new())
        }
    };

    let (text, _) = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("Anthropic 工具回路应 Ok");

    assert_eq!(text, "共 2 个任务");
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[("list_tasks".to_string(), "{}".to_string())],
        "注入 executor 应被以解析出的 (name, arguments) 调用一次"
    );
    assert_eq!(server.request_count(), 2, "工具执行后应续聊第二轮");
    // 第二轮请求体：assistant 的 tool_calls 已转 tool_use 块，tool 结果已转 user 消息里的
    // tool_result 块（严格交替 + tool_use_id 配对）
    let bodies = server.request_bodies();
    let v2: serde_json::Value =
        serde_json::from_str(&bodies[1]).expect("第二轮请求体应为合法 JSON");
    let msgs = v2["messages"].as_array().expect("messages 应为数组");
    let roles: Vec<&str> = msgs.iter().filter_map(|m| m["role"].as_str()).collect();
    assert_eq!(
        roles,
        ["user", "assistant", "user"],
        "消息应严格交替：{roles:?}"
    );
    let asst_blocks = msgs[1]["content"]
        .as_array()
        .expect("assistant content 应为块数组");
    assert_eq!(asst_blocks[0]["type"], serde_json::json!("tool_use"));
    assert_eq!(asst_blocks[0]["id"], serde_json::json!("toolu_mock"));
    assert_eq!(asst_blocks[0]["name"], serde_json::json!("list_tasks"));
    let user_blocks = msgs[2]["content"]
        .as_array()
        .expect("user content 应为块数组");
    assert_eq!(user_blocks[0]["type"], serde_json::json!("tool_result"));
    assert_eq!(
        user_blocks[0]["tool_use_id"],
        serde_json::json!("toolu_mock")
    );
    assert_eq!(
        user_blocks[0]["content"],
        serde_json::json!("共 2 个任务明细")
    );
}

#[tokio::test]
async fn core_anthropic_stream_error_event_surfaced() {
    // Anthropic 协议流内 error 事件（200 流内错误）必须显式报错，对齐 OpenAI 侧 P1-3 防线
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicStreamError("overloaded".into()));
    let h = CoreHarness::new();

    let err = match run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    {
        Err(e) => e,
        Ok(_) => panic!("Anthropic 流内 error 事件应报错"),
    };

    let msg = err.to_string();
    assert!(
        msg.contains("大模型返回错误") && msg.contains("overloaded"),
        "错误应带流内错误详情：{msg}"
    );
    assert!(
        h.events().contains(&"llm.stream_error"),
        "应记 llm.stream_error 审计：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_anthropic_text_block_first_tool_use_remapped_no_ghost() {
    // 2026-09-05 真实环境 400 回归（MiniMax Anthropic 端点
    // "tool result's tool id(call_synth_0) not found"）：text 块占 index 0、
    // tool_use 块占 index 1——ToolSlotMapper 必须把块序号重映射为稠密工具槽位，
    // 否则累积层在 index 0 留幽灵空条目，空 id 被合成 call_synth_0 并以
    // 「未知工具」执行回填，下一轮 tool_result 引用服务端从未签发的 id → 400。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicTextThenToolCall(
        "我先读一下文件".into(),
        ToolCallResponse {
            name: "extract_document".into(),
            arguments: r#"{"path":"/tmp/a.docx"}"#.into(),
        },
    ));
    server.push_behavior(MockBehavior::AnthropicTextReply("读完了".into()));
    let h = CoreHarness::new();

    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |name: String, args: String| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((name, args));
            ("文件内容".into(), Vec::new())
        }
    };

    let (text, _) = run_model_loop_core(
        &core_http_anthropic(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("文本在前工具在后应 Ok");

    assert_eq!(text, "读完了");
    // 恰好执行 1 次真实工具（幽灵条目不得被当 tool_call 执行）
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[(
            "extract_document".to_string(),
            r#"{"path":"/tmp/a.docx"}"#.to_string()
        )],
        "应只执行真实工具一次"
    );
    assert_eq!(server.request_count(), 2);
    // 第二轮请求体：assistant 的 tool_use 用服务端真实 id，tool_result 配对同一 id，
    // 且全请求体不得出现本地合成的 call_synth 占位 id
    let bodies = server.request_bodies();
    let v2: serde_json::Value =
        serde_json::from_str(&bodies[1]).expect("第二轮请求体应为合法 JSON");
    let msgs = v2["messages"].as_array().expect("messages 应为数组");
    let asst = msgs
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("应有 assistant 消息");
    let asst_blocks = asst["content"]
        .as_array()
        .expect("assistant content 应为块数组");
    let tool_uses: Vec<&serde_json::Value> = asst_blocks
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .collect();
    assert_eq!(
        tool_uses.len(),
        1,
        "幽灵条目不得回填进历史：{asst_blocks:?}"
    );
    assert_eq!(tool_uses[0]["id"], serde_json::json!("toolu_real_1"));
    assert_eq!(tool_uses[0]["name"], serde_json::json!("extract_document"));
    let last = msgs.last().expect("应有末条消息");
    let tool_results: Vec<&serde_json::Value> = last["content"]
        .as_array()
        .expect("user content 应为块数组")
        .iter()
        .filter(|b| b["type"] == "tool_result")
        .collect();
    assert_eq!(tool_results.len(), 1, "tool_result 应与 tool_use 一一配对");
    assert_eq!(
        tool_results[0]["tool_use_id"],
        serde_json::json!("toolu_real_1")
    );
    assert!(
        !bodies[1].contains("call_synth"),
        "请求体不得出现合成占位 id：{}",
        bodies[1]
    );
}

#[tokio::test]
async fn core_anthropic_summarize_http_non_stream() {
    // summarize_http 的 Anthropic 分支：/v1/messages + 非流式 JSON 响应解析
    //（content 数组 text 块拼接）
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicJsonReply(
        "用户在做 Anthropic 适配".into(),
    ));

    let client = reqwest::Client::new();
    let text = summarize_http(
        &client,
        &server.base_url,
        "test-key",
        "mock-model",
        "总结",
        &[chat_msg("user", "聊聊适配方案")],
        ApiProvider::Anthropic,
        DEFAULT_MAX_TOKENS,
    )
    .await
    .expect("Anthropic 摘要应成功");

    assert_eq!(text, "用户在做 Anthropic 适配");
    let paths = server.request_paths();
    assert!(
        paths[0].contains("POST /v1/messages"),
        "摘要应打 /v1/messages：{paths:?}"
    );
    let body = &server.request_bodies()[0];
    let v: serde_json::Value = serde_json::from_str(body).expect("请求体应为合法 JSON");
    assert_eq!(v["stream"], serde_json::json!(false), "摘要为非流式");
    assert!(v.get("tools").is_none(), "摘要请求不带 tools");
    assert!(
        v.get("system").is_some(),
        "system prompt 应转顶层 system 字段"
    );
}

use wmessage_lib::bot_chat::{summarize_http, ChatMsg};

fn chat_msg(role: &str, content: &str) -> ChatMsg {
    ChatMsg {
        role: role.into(),
        content: content.into(),
    }
}
