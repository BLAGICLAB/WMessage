/// F-6 step 2：Mock LLM HTTP server（2026-08-18）
///
/// 模拟 OpenAI 兼容的流式响应，跑通 wmessage bot.rs 的 SSE 解析路径。
///
/// 设计要点：
/// - std::net::TcpListener 后台线程，零额外 deps（避免给测试加 ureq/hyper）
/// - 每个请求独立计数 + 可配置 tool_call 响应队列
/// - URL: `http://127.0.0.1:{port}/v1/chat/completions`（与 bot_model_loop.rs 的
///   run_model_loop 请求路径一致）
///
/// 测试覆盖：
/// 1. server 返回合法 OpenAI 兼容 SSE（`data: {...}\n\n` + `data: [DONE]\n\n`）
/// 2. 多轮请求独立计数（chat loop 场景）
/// 3. tool_call 响应格式（interactive Skill 场景）
/// 4. 普通文本响应（auto Skill 之外场景）
///
/// 2026-09-03 T1-2 已完成：mock LLM server 已接到抽离后的 run_model_loop_core 真路径
/// （见 tests/llm_integration.rs 的 core_* 用例）。
///
/// 注：上面用 `///` 不用 `//!`，是因为 `llm_integration.rs` 用 `include!` 把本文件作为子模块引入，
/// `//!` 内文档注释会跟 `use` 冲突（E0753 expected outer doc comment）。
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ────────────────────────────────────────────────────────────────────
// Mock LLM server
// ────────────────────────────────────────────────────────────────────

/// 预配置的 tool_call 响应（按请求顺序消费）
#[derive(Clone, Debug)]
pub struct ToolCallResponse {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone)]
pub enum MockBehavior {
    /// 默认文本回复
    TextReply(String),
    /// 模拟 LLM 返回 tool_call
    ToolCall(ToolCallResponse),
    /// 模拟 HTTP 错误（如 API key 失效 / 限流）
    HttpError(u16, String),
    /// 200 SSE 流内错误载荷（OneAPI 类网关行为，批次3审计 P1-3）
    StreamError(String),
    /// 正文按 N 字节切片逐片 write+flush（可在多字节 UTF-8 字符中间切断，
    /// 模拟真实 TCP 分片，批次3审计 T-2 / P1-2）；片间 sleep 5ms
    FragmentedTextReply(String, usize),
    /// 原样 SSE body（2026-09-03 T1-2）：测试自己拼 data 行——用于干净 EOF 截断流
    /// （无 [DONE] / 无 finish_reason 直接 Connection: close，批次3审计 P1-1 防线）
    /// 等预置变体表达不了的场景
    RawSse(String),
    /// ── Anthropic 协议应答（2026-09-05 Anthropic 兼容模式）──
    /// 流式文本回复（message_start/content_block_*/message_delta/message_stop 全套事件）
    AnthropicTextReply(String),
    /// 流式 tool_use 回复（content_block_start tool_use + input_json_delta + stop_reason=tool_use）
    AnthropicToolCall(ToolCallResponse),
    /// 200 流内 error 事件（Anthropic 协议 {"type":"error",...}，对齐 OpenAI 侧 StreamError）
    AnthropicStreamError(String),
    /// 非流式 Anthropic message JSON 回复（Planner/摘要走 stream:false + resp.json()）
    AnthropicJsonReply(String),
    /// 文本块在前 + tool_use 块在后（2026-09-05 真实环境 400 回归基准）：
    /// text 块占 index 0、tool_use 块 index 1——Anthropic 块序号连文本一起数，
    /// 重映射防线（bot_anthropic::ToolSlotMapper）必须把它归位到工具槽位 0
    AnthropicTextThenToolCall(String, ToolCallResponse),
}

pub struct MockLlmServer {
    pub base_url: String,
    pub port: u16,
    request_count: Arc<AtomicUsize>,
    #[allow(dead_code)] // 仅内部线程闭包通过 Arc 访问；Rust 静态分析看不到闭包内读
    consumed: Arc<AtomicUsize>,
    behaviors: Arc<Mutex<Vec<MockBehavior>>>,
    /// 每个请求的 body 原文（2026-09-05 记忆回归基准）：断言「发给 LLM 的请求里
    /// 有什么」（记忆块 system 消息 / 工具结果回填）必须看真实出站请求，
    /// 不能只断言测试侧自拼的消息数组
    bodies: Arc<Mutex<Vec<String>>>,
    /// 每个请求的请求行（2026-09-05 Anthropic 兼容模式）："POST /v1/messages HTTP/1.1"
    /// ——断言协议分支打对了路径（/chat/completions vs /v1/messages）
    paths: Arc<Mutex<Vec<String>>>,
    _handle: Option<thread::JoinHandle<()>>,
}

impl MockLlmServer {
    /// 启动 mock LLM server，监听 127.0.0.1:0（自动分配端口）
    pub fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}/v1");

        let request_count = Arc::new(AtomicUsize::new(0));
        let consumed = Arc::new(AtomicUsize::new(0));
        let behaviors = Arc::new(Mutex::new(Vec::<MockBehavior>::new()));
        let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));

        let req_count_clone = request_count.clone();
        let consumed_clone = consumed.clone();
        let behaviors_clone = behaviors.clone();
        let bodies_clone = bodies.clone();
        let paths_clone = paths.clone();

        let handle = thread::spawn(move || {
            // 每个连接独立处理（一线程一连接，简化测试并发）
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let req_count = req_count_clone.clone();
                let consumed = consumed_clone.clone();
                let behaviors = behaviors_clone.clone();

                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

                // 读 HTTP 请求（简单读取到 \r\n\r\n 后 body）
                let mut buf = Vec::with_capacity(16384);
                let mut tmp = [0u8; 4096];
                loop {
                    match stream.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                // 读完 header，继续读 body（如果有 Content-Length）
                                if let Some(body_start) = find_body_start(&buf) {
                                    if let Some(content_length) = parse_content_length(&buf) {
                                        while buf.len() < body_start + content_length {
                                            match stream.read(&mut tmp) {
                                                Ok(0) => break,
                                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                                Err(_) => break,
                                            }
                                        }
                                    }
                                }
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }

                let _ = req_count.fetch_add(1, Ordering::SeqCst);
                let idx = consumed.fetch_add(1, Ordering::SeqCst);
                // 记录请求 body 原文（记忆回归基准断言出站请求内容用）
                let body_text = find_body_start(&buf)
                    .map(|s| String::from_utf8_lossy(&buf[s..]).to_string())
                    .unwrap_or_default();
                bodies_clone.lock().unwrap().push(body_text);
                // 记录请求行（协议路径断言：/chat/completions vs /v1/messages）
                let req_line = buf
                    .iter()
                    .position(|&b| b == b'\n')
                    .map(|p| String::from_utf8_lossy(&buf[..p]).trim().to_string())
                    .unwrap_or_default();
                paths_clone.lock().unwrap().push(req_line);

                // 决定行为：按 consumed index 读取预存队列，超出默认 fallback
                let behavior = {
                    let q = behaviors.lock().unwrap();
                    if idx < q.len() {
                        q[idx].clone()
                    } else {
                        MockBehavior::TextReply("mock reply".to_string())
                    }
                };

                match &behavior {
                    // 分片写出：先写 HTTP 头，body 按 N 字节切片逐片 write+flush
                    //（可在多字节 UTF-8 字符中间切断，模拟真实 TCP 分片）
                    MockBehavior::FragmentedTextReply(content, chunk_bytes) => {
                        let body = sse_text_reply(content);
                        let head = format!(
                            "HTTP/1.1 200 OK\r\n\
                             Content-Type: text/event-stream\r\n\
                             Content-Length: {}\r\n\
                             Connection: close\r\n\
                             \r\n",
                            body.len()
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.flush();
                        for slice in body.as_bytes().chunks((*chunk_bytes).max(1)) {
                            let _ = stream.write_all(slice);
                            let _ = stream.flush();
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                    // 普通 behavior：整响应一次 write（原逻辑不变）
                    _ => {
                        let http_response = build_http_response(&behavior);
                        let _ = stream.write_all(http_response.as_bytes());
                        let _ = stream.flush();
                    }
                }
            }
        });

        Self {
            base_url,
            port,
            request_count,
            consumed,
            behaviors,
            bodies,
            paths,
            _handle: Some(handle),
        }
    }

    /// 预存下个请求的响应（按 consumed 索引位置访问）
    pub fn push_behavior(&self, b: MockBehavior) {
        self.behaviors.lock().unwrap().push(b);
    }

    pub fn request_count(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }

    /// 已收到的全部请求 body 原文（按到达顺序；2026-09-05 记忆回归基准用）
    #[allow(dead_code)] // mock_llm.rs 同时被 include! 进 llm_integration.rs，那边不一定用
    pub fn request_bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }

    /// 已收到的全部请求行（按到达顺序；2026-09-05 Anthropic 协议路径断言用）
    #[allow(dead_code)] // 同上：include! 进 llm_integration.rs，那边不一定用
    pub fn request_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}

fn find_body_start(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_content_length(buf: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(buf);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            return rest.trim().parse().ok();
        }
        if let Some(rest) = line.strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

// ────────────────────────────────────────────────────────────────────
// HTTP 响应构造（OpenAI 兼容 SSE）
// ────────────────────────────────────────────────────────────────────

fn build_http_response(behavior: &MockBehavior) -> String {
    match behavior {
        MockBehavior::HttpError(status, body) => build_error_response(*status, body),
        MockBehavior::AnthropicJsonReply(content) => {
            // 非流式 Anthropic message 单体响应（2026-09-05 Anthropic 兼容模式：
            // Planner/摘要走 stream:false + resp.json()）
            let escaped = content.replace('\\', "\\\\").replace('"', "\\\"");
            let body = format!(
                r#"{{"id":"msg_mock","type":"message","role":"assistant","model":"mock-model","content":[{{"type":"text","text":"{escaped}"}}],"stop_reason":"end_turn","usage":{{"input_tokens":10,"output_tokens":5}}}}"#
            );
            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
                body.len(),
                body
            )
        }
        _ => {
            let body = match behavior {
                MockBehavior::TextReply(content) => sse_text_reply(content),
                MockBehavior::ToolCall(tc) => sse_tool_call_reply(&tc.name, &tc.arguments),
                MockBehavior::StreamError(msg) => sse_stream_error_reply(msg),
                MockBehavior::RawSse(raw) => raw.clone(),
                MockBehavior::AnthropicTextReply(content) => anthropic_sse_text_reply(content),
                MockBehavior::AnthropicToolCall(tc) => {
                    anthropic_sse_tool_call_reply(&tc.name, &tc.arguments)
                }
                MockBehavior::AnthropicTextThenToolCall(text, tc) => {
                    anthropic_sse_text_then_tool_call_reply(text, &tc.name, &tc.arguments)
                }
                MockBehavior::AnthropicStreamError(msg) => anthropic_sse_error_reply(msg),
                _ => unreachable!(), // FragmentedTextReply 在连接处理分支里单独分片写出；AnthropicJsonReply 上面已处理
            };
            format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
                body.len(),
                body
            )
        }
    }
}

fn build_error_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        status_to_reason(status),
        body.len(),
        body
    )
}

fn status_to_reason(status: u16) -> &'static str {
    match status {
        401 => "Unauthorized",
        403 => "Forbidden",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// 流式文本回复（与 bot_model_loop.rs 的 parse_sse_chunk 解析路径对齐）
/// 格式：`data: {json}\n\n` + `data: {json}\n\n` + `data: [DONE]\n\n`
fn sse_text_reply(content: &str) -> String {
    let escaped = content.replace('\\', "\\\\").replace('"', "\\\"");
    let chunk1 = format!(
        r#"data: {{"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-chat","choices":[{{"delta":{{"role":"assistant","content":"{escaped}"}},"index":0,"finish_reason":null}}]}}"#
    );
    let chunk2 = r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-chat","choices":[{"delta":{},"index":0,"finish_reason":"stop"}]}"#;
    format!("{chunk1}\n\n{chunk2}\n\ndata: [DONE]\n\n")
}

/// 流式 tool_call 回复（与 bot_model_loop.rs 的 parse_sse_chunk 解析路径对齐）
fn sse_tool_call_reply(name: &str, arguments: &str) -> String {
    let args_escaped = arguments.replace('\\', "\\\\").replace('"', "\\\"");
    let chunk = format!(
        r#"data: {{"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-chat","choices":[{{"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"call_mock","type":"function","function":{{"name":"{name}","arguments":"{args_escaped}"}}}}]}},"index":0,"finish_reason":null}}]}}"#
    );
    let final_chunk = r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-chat","choices":[{"delta":{},"index":0,"finish_reason":"tool_calls"}]}"#;
    format!("{chunk}\n\n{final_chunk}\n\ndata: [DONE]\n\n")
}

/// 200 SSE 流内错误载荷（OneAPI 类网关行为，批次3审计 P1-3）：
/// `data: {"error":{"message":"<msg>","type":"stream_error"}}` + `data: [DONE]`
fn sse_stream_error_reply(message: &str) -> String {
    // 与 sse_text_reply 同一 JSON 转义法
    let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
    format!("data: {{\"error\":{{\"message\":\"{escaped}\",\"type\":\"stream_error\"}}}}\n\ndata: [DONE]\n\n")
}

// ────────────────────────────────────────────────────────────────────
// Anthropic 协议 SSE 构造（2026-09-05 Anthropic 兼容模式）
// 与 bot_anthropic.rs 的 parse_anthropic_event 解析路径对齐：
// 完整事件序列 message_start → content_block_start → content_block_delta →
// content_block_stop → message_delta(stop_reason+usage) → message_stop，
// 事件行用真实 `event: xxx` + `data: {...}` 双行形态（event 行应被解析器忽略）。
// ────────────────────────────────────────────────────────────────────

/// Anthropic 流式文本回复
fn anthropic_sse_text_reply(content: &str) -> String {
    let escaped = content.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"mock-model\",\"stop_reason\":null,\"usage\":{{\"input_tokens\":12,\"output_tokens\":1}}}}}}\n\
         \n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\
         \n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{escaped}\"}}}}\n\
         \n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\
         \n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":7}}}}\n\
         \n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\
         \n"
    )
}

/// Anthropic 流式 tool_use 回复（arguments 作为单个 input_json_delta 整段发出）
fn anthropic_sse_tool_call_reply(name: &str, arguments: &str) -> String {
    let args_escaped = arguments.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"mock-model\",\"stop_reason\":null,\"usage\":{{\"input_tokens\":12,\"output_tokens\":1}}}}}}\n\
         \n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_mock\",\"name\":\"{name}\"}}}}\n\
         \n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{args_escaped}\"}}}}\n\
         \n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\
         \n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":9}}}}\n\
         \n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\
         \n"
    )
}

/// Anthropic 200 流内 error 事件（真实 API 在流中途失败时的形态；之后直接断流无 message_stop）
fn anthropic_sse_error_reply(message: &str) -> String {
    let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "event: error\n\
         data: {{\"type\":\"error\",\"error\":{{\"type\":\"api_error\",\"message\":\"{escaped}\"}}}}\n\
         \n"
    )
}

/// 文本块在前 + tool_use 块在后（2026-09-05 真实环境 400 回归基准）：
/// text 块占 index 0，tool_use 块占 index 1（块序号连文本一起数，与真实 Anthropic 一致）
fn anthropic_sse_text_then_tool_call_reply(text: &str, name: &str, arguments: &str) -> String {
    let text_escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let args_escaped = arguments.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "event: message_start\n\
         data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"mock-model\",\"stop_reason\":null,\"usage\":{{\"input_tokens\":12,\"output_tokens\":1}}}}}}\n\
         \n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\
         \n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text_escaped}\"}}}}\n\
         \n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\
         \n\
         event: content_block_start\n\
         data: {{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_real_1\",\"name\":\"{name}\"}}}}\n\
         \n\
         event: content_block_delta\n\
         data: {{\"type\":\"content_block_delta\",\"index\":1,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{args_escaped}\"}}}}\n\
         \n\
         event: content_block_stop\n\
         data: {{\"type\":\"content_block_stop\",\"index\":1}}\n\
         \n\
         event: message_delta\n\
         data: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":15}}}}\n\
         \n\
         event: message_stop\n\
         data: {{\"type\":\"message_stop\"}}\n\
         \n"
    )
}

// ────────────────────────────────────────────────────────────────────
// HTTP 客户端（同步 std::net，避免给测试加 ureq/hyper）
// ────────────────────────────────────────────────────────────────────

fn http_post_raw(url: &str, body: &str) -> Result<String, String> {
    let url = url.strip_prefix("http://").ok_or("URL 必须 http://")?;
    let (host_port, path) = url.split_once('/').unwrap_or((url, ""));
    let path = format!("/{path}");
    let (host, port_str) = host_port.split_once(':').ok_or("URL 缺端口")?;
    let port: u16 = port_str.parse().map_err(|_| "端口非数字")?;

    let mut stream = TcpStream::connect((host, port)).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

    let req = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read: {e}"))?;
    Ok(response)
}

fn split_response(raw: &str) -> (String, String) {
    if let Some(idx) = raw.find("\r\n\r\n") {
        let (head, body) = raw.split_at(idx + 4);
        (head.to_string(), body.to_string())
    } else {
        (raw.to_string(), String::new())
    }
}

// ────────────────────────────────────────────────────────────────────
// 测试
// ────────────────────────────────────────────────────────────────────

#[test]
fn mock_llm_server_starts_and_listens_on_random_port() {
    let server = MockLlmServer::start();
    assert!(server.base_url.starts_with("http://127.0.0.1:"));
    assert!(server.base_url.ends_with("/v1"));
    assert!(server.port > 0, "应分配有效端口；got {}", server.port);
    assert_eq!(server.request_count(), 0, "新启动 server 应无请求");
}

#[test]
fn mock_llm_server_returns_valid_sse_text_reply() {
    let server = MockLlmServer::start();

    let raw = http_post_raw(
        &format!("{}/chat/completions", server.base_url),
        r#"{"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
    )
    .expect("POST 成功");

    let (head, body) = split_response(&raw);

    // 验证 HTTP header
    assert!(head.starts_with("HTTP/1.1 200 OK"), "got: {head}");
    assert!(
        head.contains("Content-Type: text/event-stream"),
        "SSE 必须声明 text/event-stream"
    );

    // 验证 SSE body 格式（与 bot.rs 解析路径对齐）
    assert!(body.starts_with("data: "), "SSE 第一行必须 data: 开头");
    assert!(
        body.contains("\n\ndata: "),
        "SSE chunk 之间必须 \\n\\n 分隔"
    );
    assert!(
        body.trim_end().ends_with("data: [DONE]"),
        "SSE 必须以 [DONE] 结尾"
    );

    // 验证 JSON 内容包含 role/content
    assert!(body.contains(r#""role":"assistant""#), "delta 应包含 role");
    assert!(
        body.contains(r#""content":"mock reply""#),
        "默认 TextReply 应返回固定 mock reply 文本"
    );
    assert!(
        body.contains(r#""finish_reason":"stop""#),
        "末尾 chunk 应有 finish_reason=stop"
    );

    // 验证 server 计数
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 1, "应处理 1 个请求");
}

#[test]
fn mock_llm_server_handles_multiple_sequential_requests_chat_loop() {
    let server = MockLlmServer::start();

    // 预存 5 个不同文本（验证 queue 消费 + chat loop 场景）
    for i in 0..5 {
        server.push_behavior(MockBehavior::TextReply(format!("reply #{i}")));
    }

    for i in 0..5 {
        let body = format!(
            r#"{{"model":"deepseek-chat","messages":[{{"role":"user","content":"msg {i}"}}],"stream":true}}"#
        );
        let raw = http_post_raw(&format!("{}/chat/completions", server.base_url), &body)
            .expect("POST 成功");
        let (_head, body_str) = split_response(&raw);
        assert!(
            body_str.contains(&format!("\"content\":\"reply #{i}\"")),
            "轮次 {i} 应返预存 reply #{i}"
        );
    }

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 5, "应处理 5 个 chat loop 请求");
}

#[test]
fn mock_llm_server_serves_tool_call_response_for_interactive_skill() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".to_string(),
        arguments: "{}".to_string(),
    }));

    let raw = http_post_raw(
        &format!("{}/chat/completions", server.base_url),
        r#"{"model":"deepseek-chat","messages":[{"role":"user","content":"列出任务"}],"stream":true}"#,
    )
    .expect("POST 成功");

    let (_head, body) = split_response(&raw);

    // 验证 tool_call 响应格式（与 bot_model_loop.rs 的 parse_sse_chunk 解析对齐）
    assert!(body.contains(r#""tool_calls""#), "应包含 tool_calls 字段");
    assert!(
        body.contains(r#""function""#),
        "tool_call 应有 function 嵌套"
    );
    assert!(
        body.contains(r#""name":"list_tasks""#),
        "tool_call.function.name 应为 list_tasks"
    );
    assert!(
        body.contains(r#""arguments":"{}""#),
        "tool_call 应传 arguments"
    );
    assert!(
        body.contains(r#""finish_reason":"tool_calls""#),
        "tool_call 响应末尾 finish_reason 应为 tool_calls"
    );
    assert!(
        body.contains(r#""id":"call_mock""#),
        "tool_call 应有 id 字段"
    );

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 1);
}

#[test]
fn mock_llm_server_consumes_behavior_queue_sequentially() {
    let server = MockLlmServer::start();

    // 预存 3 个行为：tool_call → text → tool_call
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".to_string(),
        arguments: r#"{"code":"1+1"}"#.to_string(),
    }));
    server.push_behavior(MockBehavior::TextReply("好的，结果是 2".to_string()));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".to_string(),
        arguments: r#"{"title":"test"}"#.to_string(),
    }));

    let url = format!("{}/chat/completions", server.base_url);
    let body =
        r#"{"model":"deepseek-chat","messages":[{"role":"user","content":"x"}],"stream":true}"#;

    // 第 1 个请求 → tool_call run_python
    let raw = http_post_raw(&url, body).unwrap();
    let (_, b) = split_response(&raw);
    assert!(
        b.contains(r#""name":"run_python""#),
        "轮次 1 应返 run_python"
    );

    // 第 2 个请求 → text "好的，结果是 2"
    let raw = http_post_raw(&url, body).unwrap();
    let (_, b) = split_response(&raw);
    assert!(
        b.contains(r#""content":"好的，结果是 2""#),
        "轮次 2 应返中文文本"
    );

    // 第 3 个请求 → tool_call create_task
    let raw = http_post_raw(&url, body).unwrap();
    let (_, b) = split_response(&raw);
    assert!(
        b.contains(r#""name":"create_task""#),
        "轮次 3 应返 create_task"
    );

    // 第 4 个请求 → 队列空，回退默认 echo
    let raw = http_post_raw(&url, body).unwrap();
    let (_, b) = split_response(&raw);
    assert!(
        b.contains(r#""content":"mock reply""#),
        "队列空时应回退默认 mock reply"
    );

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 4);
}

#[test]
fn mock_llm_server_returns_http_error_for_401() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        401,
        r#"{"error":{"message":"Invalid API key","type":"auth_error"}}"#.to_string(),
    ));

    let raw = http_post_raw(
        &format!("{}/chat/completions", server.base_url),
        r#"{"model":"deepseek-chat","messages":[],"stream":true}"#,
    )
    .expect("POST 应返回 HTTP 错误");

    let (head, body) = split_response(&raw);
    assert!(head.starts_with("HTTP/1.1 401 Unauthorized"), "got: {head}");
    assert!(
        body.contains("Invalid API key"),
        "error body 应包含错误详情；got: {body}"
    );

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 1);
}

#[test]
fn mock_llm_server_returns_in_stream_error_payload() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::StreamError("auth_failed".to_string()));

    let raw = http_post_raw(
        &format!("{}/chat/completions", server.base_url),
        r#"{"model":"deepseek-chat","messages":[],"stream":true}"#,
    )
    .expect("POST 成功");

    let (head, body) = split_response(&raw);
    // 流内错误载荷仍是 HTTP 200（OneAPI 类网关行为）
    assert!(head.starts_with("HTTP/1.1 200 OK"), "got: {head}");
    assert!(
        body.contains(r#""error""#),
        "body 应含 error 载荷；got: {body}"
    );
    assert!(
        body.contains("auth_failed"),
        "body 应含错误消息；got: {body}"
    );
    assert!(
        body.trim_end().ends_with("data: [DONE]"),
        "流内错误后仍应以 [DONE] 收尾"
    );

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 1);
}

#[test]
fn mock_llm_server_anthropic_text_reply_format() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicTextReply("你好".to_string()));

    let raw = http_post_raw(
        &format!("{}/messages", server.base_url),
        r#"{"model":"claude-x","messages":[{"role":"user","content":"hi"}],"max_tokens":8192,"stream":true}"#,
    )
    .expect("POST 成功");

    let (head, body) = split_response(&raw);
    assert!(head.starts_with("HTTP/1.1 200 OK"), "got: {head}");
    assert!(head.contains("text/event-stream"), "应为 SSE");
    assert!(
        body.contains(r#""type":"message_start""#),
        "应有 message_start"
    );
    assert!(body.contains(r#""type":"text_delta""#), "应有 text_delta");
    assert!(
        body.contains(r#""stop_reason":"end_turn""#),
        "应有 stop_reason"
    );
    assert!(
        body.contains(r#""type":"message_stop""#),
        "应有 message_stop"
    );
    assert!(body.contains("event: message_start"), "应有 event 行");

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 1);
    // 请求行记录（协议路径断言用）
    assert!(
        server.request_paths()[0].contains("POST /v1/messages"),
        "应记录请求行：{:?}",
        server.request_paths()
    );
}

#[test]
fn mock_llm_server_anthropic_tool_call_and_json_reply() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicToolCall(ToolCallResponse {
        name: "list_tasks".to_string(),
        arguments: "{}".to_string(),
    }));
    server.push_behavior(MockBehavior::AnthropicJsonReply("摘要文本".to_string()));

    let url = format!("{}/messages", server.base_url);
    let raw = http_post_raw(
        &url,
        r#"{"model":"m","messages":[],"max_tokens":8192,"stream":true}"#,
    )
    .expect("POST 1");
    let (_, body) = split_response(&raw);
    assert!(body.contains(r#""type":"tool_use""#), "应有 tool_use 块");
    assert!(body.contains(r#""id":"toolu_mock""#), "应有 tool_use id");
    assert!(body.contains(r#""name":"list_tasks""#));
    assert!(body.contains(r#""type":"input_json_delta""#));
    assert!(body.contains(r#""stop_reason":"tool_use""#));

    let raw = http_post_raw(
        &url,
        r#"{"model":"m","messages":[],"max_tokens":8192,"stream":false}"#,
    )
    .expect("POST 2");
    let (head, body) = split_response(&raw);
    assert!(head.contains("application/json"), "非流式应为 JSON");
    assert!(body.contains(r#""type":"message""#));
    assert!(body.contains("摘要文本"));

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 2);
}

#[test]
fn mock_llm_server_fragmented_multibyte_reply_intact() {
    let server = MockLlmServer::start();
    // 「你好世界」每字符 3 字节，按 2 字节切片必然在多字节字符中间切断
    server.push_behavior(MockBehavior::FragmentedTextReply("你好世界".to_string(), 2));

    let raw = http_post_raw(
        &format!("{}/chat/completions", server.base_url),
        r#"{"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
    )
    .expect("POST 成功");

    let (head, body) = split_response(&raw);
    assert!(head.starts_with("HTTP/1.1 200 OK"), "got: {head}");
    // http_post_raw 用 read_to_string 整收：分片重组后内容应完整无损
    assert!(
        body.contains("你好世界"),
        "分片重组后正文应完整；got: {body}"
    );
    assert!(
        !body.contains('\u{FFFD}'),
        "多字节字符跨片不应产生 U+FFFD 替换符；got: {body}"
    );

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(server.request_count(), 1);
}
