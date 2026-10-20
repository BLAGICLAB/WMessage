//! Mock LLM + reqwest 端到端
//!
//! 把 `tests/mock_llm.rs` 的 server 接到 reqwest::Client，
//! 验证 SSE 响应能被生产同构的解析逻辑消费。
//!
//! 测试改用 `wmessage_lib::bot::parse_sse_chunk`，
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
use wmessage_lib::bot::registry::ToolResult;
use wmessage_lib::bot::{
    accumulate_tool_call_delta, drain_sse_lines, parse_sse_chunk, ToolCallDelta,
};

/// 中间件 API 的 AppHandle 首参占位（签名扩展后测试适配）。
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

    // mock server 在写回响应前已计数，此处无需 sleep 等待即可断言
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
// run_model_loop_core 真路径集成测试
//
// 背景：P0-1/P0-5 等修复密集区（HTTP 错误包装 / 流截断防线 / 重试退避）
// 原先沉在 run_model_loop 函数体内、直连 AppHandle，无法集成测试。
// 重构后核心 loop 可注入 LlmHttp + 副作用出口（ModelLoopDeps），
// 以下用例全部对 mock LLM server 跑生产同一份代码。
// ────────────────────────────────────────────────────────────────────

use std::sync::{Arc, Mutex};
use wmessage_lib::bot::{
    noop_replan, run_model_loop_core, ApiProvider, AuditLevel, LlmHttp, ModelLoopDeps, StopGuard,
    ToolCallTrace, DEFAULT_MAX_TOKENS,
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
        // RE-1：mock 测试不发推理字段（None = 不注入，既有断言不受影响）
        reasoning: wmessage_lib::bot::reasoning::ReasoningWire::None,
        // 条目级采样参数 / system prompt（U13）：既有用例不发不追加
        temperature: None,
        top_p: None,
        system_prompt: None,
    }
}

/// Anthropic 协议变体：同一 mock server，
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
    /// 阶段 3.3：核心只读「活动技能快照」（等价 `bot_skills::active_skill_run_for`）
    active_skill_run:
        Box<dyn Fn(Option<&str>) -> Option<wmessage_lib::bot_skills::SkillRun> + Send + Sync>,
    /// 子 agent 凭证归并的注入点（幻觉守卫用）：默认空 = 本次提问没有子 agent
    /// 调过工具。要验「子 agent 真干过活、主 agent 据实汇报不被拦」的用例用
    /// `set_subagent_tools` 填入子 agent 实际调过的工具名。
    subagent_tools: Box<dyn Fn(Option<&str>) -> Vec<String> + Send + Sync>,
    /// `subagent_tools` 闭包背后的数据，供测试写入
    subagent_tools_data: Arc<Mutex<Vec<String>>>,
    /// 执行类谎报守卫的触发词表。默认填**内置默认表**（与生产一致），
    /// 否则依赖守卫的既有用例会因空词表静默失效；要验自定义表改这个字段。
    guard_claim_keywords: Vec<String>,
    /// 落盘凭证注入点：列出的路径判成「查不到凭证」（幻觉守卫的「这个路径是
    /// 工具产物吗」那道判据）。空表 = 全部有凭证 = 放行。
    file_receipts_missing: Arc<Mutex<Vec<String>>>,
    /// 落盘凭证闭包（从 `file_receipts_missing` 读）。存进 harness 而不是每次
    /// 现造——`deps()` 要的是 `&'a dyn Fn`，现造就得 `Box::leak` 逃逸，
    /// 多轮用例下泄漏随调用次数增长。
    file_receipts: Box<dyn Fn(Option<&str>, &[String]) -> Vec<String> + Send + Sync>,
    audit_events: AuditEvents,
}

impl CoreHarness {
    fn new() -> Self {
        let events: AuditEvents = Arc::new(Mutex::new(Vec::new()));
        let events2 = events.clone();
        let sub_tools: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sub_tools2 = sub_tools.clone();
        let missing: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let missing2 = missing.clone();
        Self {
            // 非交互实例：不发流式事件，stop 标志未置位
            stop: StopGuard::new(&mock_handle(), false, None),
            emit: Box::new(|_, _| {}),
            audit: Box::new(move |_, event, _| {
                events2.lock().unwrap().push(event);
            }),
            audit_log: Box::new(|_| {}),
            skill_finish: Box::new(|_, _| String::new()),
            // 用真 accessor（走兜底实例，与生产语义一致）；本组用例不插 run → 恒 None。
            // handle 构造时捕获一次：闭包里每次调 mock_handle() 会 Box::leak 一个新
            // mock_app，多轮用例下泄漏随调用次数增长；兜底注册表是全局 OnceLock，
            // 复用同一 handle 语义不变
            active_skill_run: {
                let h = mock_handle();
                Box::new(move |sid| wmessage_lib::bot_skills::active_skill_run_for(&h, sid))
            },
            subagent_tools: Box::new(move |_| sub_tools2.lock().unwrap().clone()),
            subagent_tools_data: sub_tools,
            guard_claim_keywords: wmessage_lib::bot::registry::default_execution_claim_patterns(),
            file_receipts_missing: missing,
            file_receipts: Box::new(move |_sid, paths: &[String]| {
                let missing = missing2.lock().unwrap();
                paths
                    .iter()
                    .filter(|p| missing.iter().any(|m| m.eq_ignore_ascii_case(p)))
                    .cloned()
                    .collect()
            }),
            audit_events: events,
        }
    }

    /// 依赖装配（默认关 final 自检，避免对历史用例产生回归干扰；
    /// 新自检测试用 deps_with_gap(opt) opt-in）
    fn deps(&self) -> ModelLoopDeps<'_> {
        // 默认关 final 自检（避免对历史用例产生回归干扰）；新自检测试用 deps_with_gap(opt) opt-in
        ModelLoopDeps {
            fuse_cap_override: None,
            conv_sidecar_dir: None,
            emit: &*self.emit,
            audit: &*self.audit,
            audit_log: &*self.audit_log,
            skill_finish: &*self.skill_finish,
            active_skill_run: &*self.active_skill_run,
            final_selfcheck_round_gap: Some(0),
            final_selfcheck_http: None,
            on_round: &|rounds| {
                let _ = rounds;
            },
            turn_status: None,
            subagent_tools: Some(&*self.subagent_tools),
            guard_claim_keywords: &self.guard_claim_keywords,
            file_receipts: Some(&*self.file_receipts),
        }
    }

    /// opt-in final 自检：gap 传入 None 走默认 8 轮；Some(N) 走 N；Some(0) 关闭
    fn deps_with_gap(&self, gap: Option<u32>) -> ModelLoopDeps<'_> {
        ModelLoopDeps {
            fuse_cap_override: None,
            conv_sidecar_dir: None,
            emit: &*self.emit,
            audit: &*self.audit,
            audit_log: &*self.audit_log,
            skill_finish: &*self.skill_finish,
            active_skill_run: &*self.active_skill_run,
            final_selfcheck_round_gap: gap,
            final_selfcheck_http: None,
            on_round: &|rounds| {
                let _ = rounds;
            },
            turn_status: None,
            subagent_tools: Some(&*self.subagent_tools),
            guard_claim_keywords: &self.guard_claim_keywords,
            file_receipts: Some(&*self.file_receipts),
        }
    }

    /// 侧调终稿自检：gap=1（首次满足即触发），自检请求打到独立的
    /// selfcheck server（非流式 JSON，同 summarize_http 路径）
    fn deps_with_selfcheck<'a>(&'a self, selfcheck: &'a LlmHttp) -> ModelLoopDeps<'a> {
        let mut deps = self.deps_with_gap(Some(1));
        deps.final_selfcheck_http = Some(selfcheck);
        deps
    }

    fn events(&self) -> Vec<&'static str> {
        self.audit_events.lock().unwrap().clone()
    }

    /// 声明本次提问派生的子 agent 成功用过这些工具（凭证归并的阳性输入）
    fn set_subagent_tools(&self, names: &[&str]) {
        let mut g = self.subagent_tools_data.lock().unwrap();
        *g = names.iter().map(|s| s.to_string()).collect();
    }
}

fn user_msgs() -> Vec<serde_json::Value> {
    vec![serde_json::json!({"role": "user", "content": "hi"})]
}

/// 工具执行 stub 永不调用版（纯文本/错误路径不应触发任何工具执行）
async fn exec_never(name: String, args: String, _trace: ToolCallTrace) -> ToolResult {
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

// RE-1（推理强度）：LlmHttp.reasoning 解析结果必须真的落进请求体——
// OpenAI 兼容分支注入 reasoning_effort 字段（GLM-5.3 等映射出 low/high/max）。
#[tokio::test]
async fn core_injects_reasoning_effort_into_openai_body() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("收到".into()));
    let h = CoreHarness::new();
    let mut http = core_http(&server);
    http.model = "glm-5.3-flash".into();
    http.reasoning = wmessage_lib::bot::reasoning::ReasoningWire::OpenAiEffort("high".into());

    let (text, _) = run_model_loop_core(
        &http,
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
    assert_eq!(text, "收到");
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].contains(r#""reasoning_effort":"high""#),
        "OpenAI 分支应注入 reasoning_effort：{}",
        bodies[0]
    );
}

// RE-1：Anthropic 分支注入 thinking 块（type=enabled + budget_tokens）。
#[tokio::test]
async fn core_injects_anthropic_thinking_block() {
    let server = MockLlmServer::start();
    // Anthropic 分支的 mock 响应必须用 AnthropicTextReply（Anthropic SSE 形状）
    server.push_behavior(MockBehavior::AnthropicTextReply("好的".into()));
    let h = CoreHarness::new();
    let mut http = core_http_anthropic(&server);
    http.reasoning = wmessage_lib::bot::reasoning::ReasoningWire::AnthropicBudget(4_096);

    run_model_loop_core(
        &http,
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
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].contains(r#""budget_tokens":4096"#) && bodies[0].contains(r#""type":"enabled""#),
        "Anthropic 分支应注入 thinking 块：{}",
        bodies[0]
    );
}

// RE-1：None 档不发任何推理字段（旧行为零影响）。
#[tokio::test]
async fn core_reasoning_none_keeps_body_clean() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("ok".into()));
    let h = CoreHarness::new();
    let http = core_http(&server); // reasoning: None（测试构造默认）

    run_model_loop_core(
        &http,
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
    let bodies = server.request_bodies();
    assert!(
        !bodies[0].contains("reasoning_effort") && !bodies[0].contains("budget_tokens"),
        "None 档不应出现推理字段：{}",
        bodies[0]
    );
}

/// 回合状态机的事件时机（借鉴 ZCode turn view model）。
///
/// 回归背景：此前工具执行段零事件——`bot-tool`（要调了）到 `bot-tool-done`
/// （调完了）之间可静默 300s（run_python 上限），前端只有一个静态「…」；
/// 轮边界也不可见。用户在 300s 的 pip install 与 0.1s 的 list_files 之间
/// 分辨不出差别，于是「看不出 agent 是否还在跑」。
#[tokio::test]
async fn turn_status_and_tool_running_events_are_emitted_in_order() {
    use wmessage_lib::bot::TurnStatus;
    // 轮 1：工具调用；轮 2：最终文本
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply("共 2 个任务".into()));
    let h = CoreHarness::new();

    // 按到达顺序记录两类事件
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let statuses: Arc<Mutex<Vec<TurnStatus>>> = Arc::new(Mutex::new(Vec::new()));
    let st2 = statuses.clone();
    let emit: Box<dyn Fn(&str, serde_json::Value) + Send + Sync> = {
        let seen = seen2.clone();
        Box::new(move |event, payload| {
            if event == "bot-tool-running" {
                let id = payload["id"].as_str().unwrap_or_default().to_string();
                seen.lock().unwrap().push(format!("running:{id}"));
            }
        })
    };
    let turn_status: Box<dyn Fn(TurnStatus, &str) + Send + Sync> = {
        let st = st2.clone();
        let seen = seen.clone();
        Box::new(move |s, _| {
            seen.lock().unwrap().push(format!("turn:{}", s.as_str()));
            st.lock().unwrap().push(s);
        })
    };
    let exec = |_name: String, _args: String, _t: ToolCallTrace| async {
        ToolResult::ok("明细", Vec::new())
    };
    let deps = ModelLoopDeps {
        emit: &*emit,
        on_round: &|_| {},
        turn_status: Some(&*turn_status),
        ..h.deps()
    };

    run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("应 Ok");

    let log = seen.lock().unwrap().clone();
    // 轮首 running 必须早于工具执行
    let first_running = log
        .iter()
        .position(|e| e == "turn:running")
        .expect("轮首必须发 running");
    let tool_at = log
        .iter()
        .position(|e| e.starts_with("running:"))
        .expect("工具执行前必须发 bot-tool-running");
    assert!(first_running < tool_at, "顺序错：{log:?}");
    // 终态：正常结束 → completed，且是最后一条
    assert_eq!(
        log.last().map(String::as_str),
        Some("turn:completed"),
        "收尾必须发 completed：{log:?}"
    );
    assert_eq!(
        statuses.lock().unwrap().last(),
        Some(&TurnStatus::Completed),
        "终态枚举须为 Completed"
    );
}

/// 用户中断时终态是 interrupted 而非 completed——前端据此收活体指示，
/// 且不该误显示成「正常完成」。
#[tokio::test]
async fn turn_status_is_interrupted_when_stopped() {
    use wmessage_lib::bot::TurnStatus;
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("你好".into()));
    let h = CoreHarness::new();
    h.stop.force_stop();

    let statuses: Arc<Mutex<Vec<TurnStatus>>> = Arc::new(Mutex::new(Vec::new()));
    let st2 = statuses.clone();
    let turn_status: Box<dyn Fn(TurnStatus, &str) + Send + Sync> = {
        let st = st2.clone();
        Box::new(move |s, _| {
            st.lock().unwrap().push(s);
        })
    };
    let deps = ModelLoopDeps {
        on_round: &|_| {},
        turn_status: Some(&*turn_status),
        ..h.deps()
    };

    let _ = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        5,
        &h.stop,
        None,
        &deps,
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("", Vec::new()) },
        noop_replan,
    )
    .await;

    assert_eq!(
        statuses.lock().unwrap().last(),
        Some(&TurnStatus::Interrupted),
        "stop 置位后终态须为 Interrupted"
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
    let exec = move |name: String, args: String, _trace: ToolCallTrace| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((name, args));
            ToolResult::ok("共 2 个任务明细", Vec::new())
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
    // mock server 在写回响应前已计数，此处无需 sleep 等待即可断言
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
async fn core_sse_unparsable_chunk_logged_but_stream_survives() {
    // 坏块可见化：data 载荷 JSON 解析失败曾被 `from_str(..).ok()?` 静默丢弃——
    // 回复凭空少一段，日志里什么都不留。现在回合结束发一条 llm.sse_chunk_unparsable，
    // 且不得中断流（其余 chunk 仍是好的，中断会把好回复一起丢掉）。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::RawSse(
        "data: {\"choices\":[{\"delta\":{\"content\":\"前半句\"}}]}\n\n"
            .to_string()
            + "data: {\"choices\":[{\"delta\":{\"cont\n\n" // 网关吐了半截 JSON
            + "data: {\"choices\":[{\"delta\":{\"content\":\"后半句\"}}]}\n\n"
            + "data: [DONE]\n\n",
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
    .expect("单个坏块不得让整条流失败");

    assert!(
        text.contains("前半句") && text.contains("后半句"),
        "got: {text}"
    );
    assert!(
        h.events().contains(&"llm.sse_chunk_unparsable"),
        "坏块应记审计：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_sse_clean_stream_has_no_unparsable_audit() {
    // 反向：正常流不得误报降级（event: 行 / 空心跳 / [DONE] 都是协议正常成分）
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::RawSse(
        "data: {\"choices\":[{\"delta\":{\"content\":\"干净回复\"},\"finish_reason\":\"stop\"}]}\n\n"
            .to_string()
            + ": keep-alive\n\n"
            + "data:    [DONE]\n\n",
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
    .expect("干净流应放行");

    assert!(text.contains("干净回复"), "got: {text}");
    assert!(
        !h.events().contains(&"llm.sse_chunk_unparsable"),
        "干净流不得记降级：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_anthropic_sse_unparsable_chunk_logged() {
    // Anthropic 分支的 parse_anthropic_event 与 OpenAI 侧同形态 `.ok()?`，
    // 坏块计数必须两协议共用一处（否则这条路径上「回复少一段、日志无痕」原样存在）
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::RawSse(
        "event: message_start\n\
         data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_mock\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"mock-model\",\"stop_reason\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\
         \n\
         event: content_block_start\n\
         data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
         \n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"te\n\
         \n\
         event: content_block_delta\n\
         data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"半程回复\"}}\n\
         \n\
         event: message_stop\n\
         data: {\"type\":\"message_stop\"}\n\
         \n"
            .into(),
    ));
    let h = CoreHarness::new();

    let (text, _) = run_model_loop_core(
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
    .expect("Anthropic 单个坏块不得让整条流失败");

    assert!(text.contains("半程回复"), "got: {text}");
    assert!(
        h.events().contains(&"llm.sse_chunk_unparsable"),
        "Anthropic 坏块也应记审计：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_openai_error_envelope_midstream_counted_as_bad_chunk() {
    // 中转网关（OneAPI 类）在流中间吐 `{"code":401,...}` 错误信封：JSON 合法但
    // 没有 choices，解析器返回 None 后原本零痕迹消失——正文吐一半就没下文了
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::RawSse(
        "data: {\"choices\":[{\"delta\":{\"content\":\"说到一半\"}}]}\n\n".to_string()
            + "data: {\"code\":401,\"msg\":\"invalid api key\"}\n\n"
            + "data: [DONE]\n\n",
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
    .expect("错误信封不得让整条流崩掉");

    assert!(text.contains("说到一半"), "已收到的正文不丢弃：{text}");
    assert!(
        h.events().contains(&"llm.sse_chunk_unparsable"),
        "流中错误信封应计入坏块：{:?}",
        h.events()
    );
}

#[tokio::test]
async fn core_context_overflow_sheds_history_and_retries() {
    // 上下文超长（400 context_length_exceeded）：原先直接抛裸 400，用户看不出
    // 该 /compact 还是换模型。现在砍掉最旧的会话消息降级重发一次。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        400,
        r#"{"error":{"message":"This model's maximum context length is 8192 tokens, however you requested 12000","type":"invalid_request_error","code":"context_length_exceeded"}}"#
            .to_string(),
    ));
    server.push_behavior(MockBehavior::TextReply("降级后正常回复".into()));
    let h = CoreHarness::new();

    // 多条历史才有可砍空间（只有一条时「没得砍」走报错出口，另有用例）
    let msgs: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "系统提示词"}),
        serde_json::json!({"role": "user", "content": "旧问题一"}),
        serde_json::json!({"role": "assistant", "content": "旧回答一"}),
        serde_json::json!({"role": "user", "content": "旧问题二"}),
        serde_json::json!({"role": "assistant", "content": "旧回答二"}),
        serde_json::json!({"role": "user", "content": "本轮问题"}),
    ];

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        msgs,
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect("超长降级后应成功返回");

    assert!(text.contains("降级后正常回复"), "got: {text}");
    assert!(
        h.events().contains(&"llm.context_overflow"),
        "超长必须留痕：{:?}",
        h.events()
    );
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2, "应重发一次：{bodies:?}");
    // 不比 body 长度：44 个工具的 schema 就占 36KB，历史那几百字淹没在里面。
    // 直接看历史消息还在不在
    assert!(bodies[0].contains("旧回答二"), "首次请求应是完整历史");
    assert!(
        !bodies[1].contains("旧回答二") && !bodies[1].contains("旧回答一"),
        "第二次请求应已砍掉旧历史：{}",
        bodies[1]
    );
    assert!(
        bodies[1].contains("本轮问题") && bodies[1].contains("系统提示词"),
        "system 段与本轮输入必须保留：{}",
        bodies[1]
    );
}

#[tokio::test]
async fn core_context_overflow_without_history_to_shed_reports_actionable_error() {
    // 没得砍时（只有 system + 本轮输入）：不再空转重发，直接回带指引的错误
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        400,
        r#"{"error":{"message":"prompt is too long: 210000 tokens > 200000 maximum","type":"invalid_request_error"}}"#
            .to_string(),
    ));
    let h = CoreHarness::new();

    let msgs: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "系统提示词"}),
        serde_json::json!({"role": "user", "content": "本轮问题"}),
    ];

    // 捕获 skill_finish 收到的文案（默认 stub 返回空串，指针进不了错误体）
    let hint_seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let hs = hint_seen.clone();
    let capture: &'static (dyn Fn(bool, &str) -> String + Send + Sync) =
        Box::leak(Box::new(move |_ok: bool, msg: &str| {
            *hs.lock().unwrap() = msg.to_string();
            String::new()
        }));
    let mut deps = h.deps();
    deps.skill_finish = capture;

    let err = run_model_loop_core(
        &core_http(&server),
        msgs,
        5,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect_err("无历史可砍时应报错而不是空转");

    let msg = err.to_string();
    assert!(msg.contains("prompt is too long"), "原始载荷应透出：{msg}");
    // 指引文案经 skill_finish 拼进 body_preview，而 CoreHarness 的默认 stub
    // 返回空串——单独接一个捕获版断言「用户真能拿到可操作指引」
    let hints = hint_seen.lock().unwrap().clone();
    assert!(
        hints.contains("/compact") && hints.contains("上下文超长"),
        "失败指引须可操作（当前：{hints:?}）"
    );
    assert_eq!(
        server.request_bodies().len(),
        1,
        "没得砍就不该重发——空转只会多烧一次请求"
    );
}

#[tokio::test]
async fn core_plain_400_is_not_degraded() {
    // 反向：非超长的 400（鉴权/参数错）不得触发砍历史重发
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        400,
        r#"{"error":{"message":"Invalid API key","type":"auth_error"}}"#.to_string(),
    ));
    let h = CoreHarness::new();

    let msgs: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "系统提示词"}),
        serde_json::json!({"role": "user", "content": "旧问题"}),
        serde_json::json!({"role": "assistant", "content": "旧回答"}),
        serde_json::json!({"role": "user", "content": "本轮问题"}),
    ];

    let err = run_model_loop_core(
        &core_http(&server),
        msgs,
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect_err("普通 400 应报错");

    assert!(
        !err.to_string().contains("/compact"),
        "普通 400 不该带超长指引：{err}"
    );
    assert_eq!(server.request_bodies().len(), 1, "不该重发");
    assert!(
        !h.events().contains(&"llm.context_overflow"),
        "普通 400 不得记超长：{:?}",
        h.events()
    );
}

/// 网关点名 stream_options 不认时：摘字段原样重发一次，回复一字不变
#[tokio::test]
async fn core_stream_options_rejection_degrades_and_resends_once() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        400,
        r#"{"error":{"message":"Unrecognized request argument supplied: stream_options","type":"invalid_request_error"}}"#.to_string(),
    ));
    server.push_behavior(MockBehavior::TextReply("降级后的正常回复".into()));
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
    .expect("降级后应正常返回回复");

    assert_eq!(text, "降级后的正常回复", "回复内容不得因降级而变");
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2, "应恰好重发一次");
    assert!(bodies[0].contains("stream_options"), "首次请求带该字段");
    assert!(
        !bodies[1].contains("stream_options"),
        "重发必须摘掉该字段：{}",
        bodies[1]
    );
    assert!(
        h.events().contains(&"llm.stream_options_unsupported"),
        "降级须留审计：{:?}",
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
// run_model_loop_core 的 Anthropic 分支
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
        serde_json::json!(DEFAULT_MAX_TOKENS),
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
    let exec = move |name: String, args: String, _trace: ToolCallTrace| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((name, args));
            ToolResult::ok("共 2 个任务明细", Vec::new())
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
    // 真实环境 400 回归（MiniMax Anthropic 端点
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
    let exec = move |name: String, args: String, _trace: ToolCallTrace| {
        let calls = calls2.clone();
        async move {
            calls.lock().unwrap().push((name, args));
            ToolResult::ok("文件内容", Vec::new())
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

/// stop_reason=max_tokens 不该把半截文本丢给用户：主循环应拿已生成部分作前缀
/// 自动续跑一轮（新一轮是全新请求，带全新输出预算），用户无需手动打「继续」
#[tokio::test]
async fn core_anthropic_length_truncation_auto_continues() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicTruncatedText(
        "好，我先确认环境：".into(),
    ));
    server.push_behavior(MockBehavior::AnthropicTextReply(
        "已确认，开始执行。".into(),
    ));
    let h = CoreHarness::new();

    let (text, _) = run_model_loop_core(
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
    .expect("截断后自动续跑应 Ok");

    assert_eq!(server.request_count(), 2, "应发第二轮请求续跑");
    assert!(
        text.contains("好，我先确认环境：") && text.contains("已确认，开始执行。"),
        "两段内容应拼成完整答复：{text}"
    );
    assert!(
        !text.contains("因长度限制被截断"),
        "已续跑成功就不该再挂截断提示：{text}"
    );
    let events = h.events();
    assert!(
        events.contains(&"llm.length_continued"),
        "续跑应留审计事件：{events:?}"
    );
}

/// 续跑有上限：模型持续撞上限时不能让循环无限自我续跑耗预算——
/// 用尽配额后退回「追加截断提示」并终止
#[tokio::test]
async fn core_anthropic_length_truncation_continuation_is_bounded() {
    let server = MockLlmServer::start();
    for _ in 0..6 {
        server.push_behavior(MockBehavior::AnthropicTruncatedText("又超了：".into()));
    }
    let h = CoreHarness::new();

    let (text, _) = run_model_loop_core(
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
    .expect("续跑用尽配额应照常 Ok 返回");

    assert_eq!(
        server.request_count(),
        3,
        "1 次原始 + 2 次续跑，用尽后不应再发请求"
    );
    assert!(
        text.contains("因长度限制被截断"),
        "配额用尽应退回截断提示：{text}"
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
        // 条目级采样参数（U13）：本用例不发
        None,
        None,
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

// ───────────── U13 条目级推理参数：请求体实参断言（mock_llm 真路径） ─────────────

// OpenAI 分支：temperature/top_p 有值 → 落进 /chat/completions 请求体。
#[tokio::test]
async fn core_injects_openai_sampling_params_into_body() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("好".into()));
    let h = CoreHarness::new();
    let mut http = core_http(&server);
    http.temperature = Some(0.7);
    http.top_p = Some(0.9);

    run_model_loop_core(
        &http,
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
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&bodies[0]).expect("请求体应为合法 JSON");
    assert_eq!(
        v["temperature"],
        serde_json::json!(0.7),
        "条目 temperature 应出现在请求体：{}",
        bodies[0]
    );
    assert_eq!(v["top_p"], serde_json::json!(0.9));
}

// OpenAI 分支：条目参数全空 → 请求体不出现 temperature/top_p 字段（多数兼容
// 网关不认识额外字段，保持请求体干净）。
#[tokio::test]
async fn core_omits_sampling_params_when_entry_absent() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("好".into()));
    let h = CoreHarness::new();

    run_model_loop_core(
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
    let v: serde_json::Value =
        serde_json::from_str(&server.request_bodies()[0]).expect("请求体应为合法 JSON");
    assert!(
        v.get("temperature").is_none() && v.get("top_p").is_none(),
        "无条目参数不得写出空字段：{v}"
    );
}

// Anthropic 分支：temperature/top_p 注入 /v1/messages 请求体（顶层字段同名）。
#[tokio::test]
async fn core_injects_anthropic_sampling_params_into_body() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicTextReply("好".into()));
    let h = CoreHarness::new();
    let mut http = core_http_anthropic(&server);
    http.temperature = Some(0.3);
    http.top_p = Some(0.8);

    run_model_loop_core(
        &http,
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
    let v: serde_json::Value =
        serde_json::from_str(&server.request_bodies()[0]).expect("请求体应为合法 JSON");
    assert_eq!(v["temperature"], serde_json::json!(0.3));
    assert_eq!(v["top_p"], serde_json::json!(0.8));
    assert_eq!(
        v["max_tokens"],
        serde_json::json!(DEFAULT_MAX_TOKENS),
        "max_tokens 沿用装配值（Anthropic 必填字段不动）"
    );
}

// 条目 system_prompt：追加到消息栈末尾——OpenAI 分支 messages 数组末条
// 应是 role=system、content 为条目值（排在用户消息之后）。
#[tokio::test]
async fn core_appends_entry_system_prompt_at_msgs_tail_openai() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("好".into()));
    let h = CoreHarness::new();
    let mut http = core_http(&server);
    http.system_prompt = Some("总是用中文回复".into());

    run_model_loop_core(
        &http,
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
    let v: serde_json::Value =
        serde_json::from_str(&server.request_bodies()[0]).expect("请求体应为合法 JSON");
    let msgs = v["messages"].as_array().expect("messages 应为数组");
    let last = msgs.last().expect("应有末条消息");
    assert_eq!(last["role"], serde_json::json!("system"), "末条应为 system");
    assert_eq!(last["content"], serde_json::json!("总是用中文回复"));
    assert_eq!(
        msgs[0]["role"],
        serde_json::json!("user"),
        "既有消息序不动（system 追加在尾）"
    );
}

// 条目 system_prompt：Anthropic 分支归并进顶层 system 块数组，条目块在最后。
#[tokio::test]
async fn core_appends_entry_system_prompt_at_msgs_tail_anthropic() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicTextReply("好".into()));
    let h = CoreHarness::new();
    let mut http = core_http_anthropic(&server);
    http.system_prompt = Some("总是用中文回复".into());

    run_model_loop_core(
        &http,
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
    let v: serde_json::Value =
        serde_json::from_str(&server.request_bodies()[0]).expect("请求体应为合法 JSON");
    let blocks = v["system"].as_array().expect("system 应为块数组");
    let last = blocks.last().expect("system 块不应为空");
    assert_eq!(last["text"], serde_json::json!("总是用中文回复"));
    assert!(
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["role"] != "system"),
        "Anthropic 协议 system 不留在 messages 里"
    );
}

// 摘要路径（summarize_http）：条目采样参数同样落进 OpenAI 请求体。
#[tokio::test]
async fn summarize_http_injects_sampling_params() {
    let server = MockLlmServer::start();
    // OpenAI 分支非流式：resp.json() 需要 JSON 单体响应（SSE 会解析失败）
    server.push_behavior(MockBehavior::OpenAiJsonReply("摘要结果".into()));
    let client = reqwest::Client::new();
    let text = summarize_http(
        &client,
        &server.base_url,
        "test-key",
        "mock-model",
        "总结",
        &[chat_msg("user", "正文")],
        ApiProvider::Openai,
        DEFAULT_MAX_TOKENS,
        Some(0.1),
        Some(0.2),
    )
    .await
    .expect("OpenAI 摘要应成功");
    assert_eq!(text, "摘要结果");
    let v: serde_json::Value =
        serde_json::from_str(&server.request_bodies()[0]).expect("请求体应为合法 JSON");
    assert_eq!(v["temperature"], serde_json::json!(0.1));
    assert_eq!(v["top_p"], serde_json::json!(0.2));
}

use wmessage_lib::bot_chat::{summarize_http, ChatMsg};

fn chat_msg(role: &str, content: &str) -> ChatMsg {
    ChatMsg {
        role: role.into(),
        content: content.into(),
    }
}

// ───────────── U15 记忆可控开关：双门禁真路径（mock app 数据目录） ─────────────
//
// ⚠️ db::data_dir 在 cargo test 下解析到 target/debug/deps/——该目录的
// bot-config.json 与 wmessage.db 现按**测试进程**隔离（paths.rs「测试期隔离」
// 一段；nextest 是进程级并行，共用一个库会互踩写锁——本次实证为偶发失败）。
// 本测试原先的「共享库历史脏数据」顾虑随之消失，但断言口径保持增量不放松：
// 进程内仍可能残留上轮同进程用例写入的行。因此：
// - 三个门禁场景合并为一个测试（nextest 下即单进程，配置写读全程本进程有序）；
// - DB 断言一律增量口径（共享库里有历史条目，不做绝对断言）；
// - 结束时恢复共享现场（删本测试写的配置文件 + 清理种子条目与历史遗留）。

fn write_config_with_memory_control(
    handle: &tauri::AppHandle<tauri::test::MockRuntime>,
    injection: bool,
    auto_write: bool,
) {
    let cfg = serde_json::json!({
        "baseUrl": "http://127.0.0.1:1",
        "model": "mock-model",
        "memoryControl": {
            "injectionEnabled": injection,
            "autoWriteEnabled": auto_write,
        },
    });
    std::fs::write(
        wmessage_lib::bot::config_path(handle),
        serde_json::to_string(&cfg).unwrap(),
    )
    .unwrap();
}

fn seed_memory(handle: &tauri::AppHandle<tauri::test::MockRuntime>, content: &str) {
    // insert_item 内部已 ensure_table，无需前置
    let conn = wmessage_lib::db::open_db(handle).unwrap();
    let item = wmessage_lib::memory::store::NewItem {
        kind: "fact".into(),
        content: content.into(),
        tags: vec![],
        importance: 3,
        source: "user_stated".into(),
    };
    // 1_000 = 固定占位时间戳（created/updated_at；检索断言只看内容不看排序）
    match wmessage_lib::memory::store::insert_item(&conn, &item, None, 1_000).unwrap() {
        (wmessage_lib::memory::store::InsertOutcome::Inserted(_), _) => {}
        other => panic!("种记忆应直接插入：{other:?}"),
    }
}

#[tokio::test]
async fn memory_gate_scenarios_injection_and_auto_write() {
    let app = mock_handle();
    let seed_content = SEED_CONTENT_PLACEHOLDER;

    // 清理前移：先扫掉历史运行留在共享库的种子/漏写条目与共享配置文件——
    // 即使本运行中途 panic，下一次运行开场也会自愈（不依赖收尾路径执行）。
    clean_gate_leftovers(&app);

    // 场景一：注入关 → 记忆保留在库但不产出注入块
    write_config_with_memory_control(&app, false, true);
    seed_memory(&app, seed_content);
    assert!(
        wmessage_lib::memory::injection_block(&app, "测试记忆")
            .await
            .is_none(),
        "注入关闭时不得产出记忆块"
    );

    // 场景二：注入开 → 照常产出且含库内内容
    write_config_with_memory_control(&app, true, true);
    let block = wmessage_lib::memory::injection_block(&app, "测试记忆")
        .await
        .expect("注入开启 + 库有记忆应产出记忆块");
    assert!(
        block.contains("## 记忆") && block.contains(seed_content),
        "{block}"
    );
    // 结构窄化：fact 条目应以标准行格式落在记忆块（kind 行格式契约）
    assert!(
        block.contains(&format!("- [fact]{seed_content}")),
        "fact 种子应以标准行格式出现：{block}"
    );

    // 场景三：自动写入关 → 两工具返回「已关闭」提示且不落库。
    // 断言走键位（find_by_key_tag）而非全表计数——共享库可能被并行测试
    // 进程写入别的条目，计数口径会误伤；本工具写入的键唯一可精确判定。
    write_config_with_memory_control(&app, true, false);
    let conn = wmessage_lib::db::open_db(&app).unwrap();
    let r = wmessage_lib::memory::tool_remember_fact(&app, r#"{"key":"测试键","value":"测试值"}"#)
        .await;
    assert!(r.text.contains("无需记录"), "{}", r.text);
    let r = wmessage_lib::memory::tool_record_lesson(&app, r#"{"lesson":"某教训"}"#).await;
    assert!(r.text.contains("无需记录"), "{}", r.text);
    assert!(
        wmessage_lib::memory::store::find_by_key_tag(&conn, "测试键")
            .unwrap()
            .is_none(),
        "门禁关闭时不得写入「测试键」"
    );

    // 收尾同样恢复现场（成功路径；中途 panic 由下一次运行的开场清理兜底）
    clean_gate_leftovers(&app);
}

const SEED_CONTENT_PLACEHOLDER: &str = "测试记忆内容甲乙丙丁";

/// 清共享现场：本测试的配置文件、种子条目（含历史轮次残留）、漏写的「测试键」。
/// 清理失败只 eprintln 不 panic——共享目录的清理是尽力而为，失败信息留诊断。
fn clean_gate_leftovers(app: &tauri::AppHandle<tauri::test::MockRuntime>) {
    if let Err(e) = std::fs::remove_file(wmessage_lib::bot::config_path(app)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[memory_gate] 清理共享 bot-config.json 失败：{e}");
        }
    }
    let conn = wmessage_lib::db::open_db(app).unwrap();
    // mem_items 建表：数据目录已按进程隔离（paths.rs），本进程的新库里这张表
    // 还不存在——过去是蹭共享库里别的进程建好的表，换来的是跨进程依赖
    wmessage_lib::memory::store::ensure_table(&conn).unwrap();
    let junk: Vec<String> = wmessage_lib::memory::store::load_all(&conn)
        .unwrap()
        .into_iter()
        .filter(|m| {
            m.content == SEED_CONTENT_PLACEHOLDER
                || m.tags.first().map(|t| t.as_str()) == Some("测试键")
        })
        .map(|m| m.id)
        .collect();
    if let Err(e) = wmessage_lib::memory::store::delete_by_ids(&conn, &junk) {
        eprintln!("[memory_gate] 清理共享库条目失败：{e}");
    }
}

// ───────────── T1-QUERYTASKS：query_tasks / edit_task / subtaskId 端到端（两段真路径） ─────────────
//
// A. 模型循环接线：mock LLM SSE 发 query_tasks / edit_task tool_call → 真 run_model_loop_core
//    → 注入 executor 收到 (name, args) 原文 → 结果以 role:tool 回填续聊——schema 命名/参数
//    序列化经真模型循环的全链路（真 dispatch 的 AppHandle<Wry> 链路在 mock runtime 下不可调，
//    见 task_chat_exec.rs:156 既有结论，故 executor 注入——skill_e2e 同款接缝）。
// B. DB 往返：db_upsert/db_load 真命令（真写锁 + 真 SQL）验证工具层读写的全部新列
//    （model/owner_id/created_at/schedule/depends_on/subtasks）在 SQLite 的落库与读出，
//    以及 edit_task 的 RMW 写回模式（expected_updated_at 基线 → upsert）。
//
// ⚠️ 共享库（target/debug/deps/wmessage.db，见 U15 地雷注记）：断言一律键位口径
//（唯一 UUID 标题 / 唯一卡 id），开场扫残留、收尾清种子，panic 由下次开场清理兜底。

const T1_TITLE_ACTIVE: &str = "T1E2E未完成卡-量子调试甲乙";
const T1_OWNER_ID: &str = "t1e2e-owner-0001";

fn t1_seed_task(
    id: &str,
    title: &str,
    column: wmessage_lib::db::TaskStatus,
) -> wmessage_lib::db::Task {
    wmessage_lib::db::Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: id.into(),
        title: title.into(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column,
        subtasks: None,
        completed_at: None,
        created_at: Some(1_000),
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(1_000),
        bot_assigned: None,
        assignee: None,
        budget: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: None,
        expected_updated_at: None,
    }
}

fn t1_cleanup(app: &tauri::AppHandle<tauri::test::MockRuntime>) {
    let conn = wmessage_lib::db::open_db(app).unwrap();
    for id in ["t1e2e-active", "t1e2e-done"] {
        if let Err(e) = conn.execute("DELETE FROM tasks WHERE id = ?1", [id]) {
            eprintln!("[t1e2e] 清理任务 {id} 失败：{e}");
        }
    }
    // 定时单源清理：迁移产物 job 行 + 执行历史（t1e2e-active 种 schedule 会触发迁移）
    if let Err(e) = conn.execute(
        "DELETE FROM scheduled_jobs WHERE id = ?1",
        ["job-t1e2e-active"],
    ) {
        eprintln!("[t1e2e] 清理定时作业失败：{e}");
    }
    if let Err(e) = conn.execute(
        "DELETE FROM scheduled_job_runs WHERE job_id = ?1",
        ["job-t1e2e-active"],
    ) {
        eprintln!("[t1e2e] 清理定时作业历史失败：{e}");
    }
    if let Err(e) = conn.execute("DELETE FROM people WHERE id = ?1", [T1_OWNER_ID]) {
        eprintln!("[t1e2e] 清理成员失败：{e}");
    }
}

/// A：模型循环把 query_tasks / edit_task 的 (name, args) 原样送达 executor，
/// 工具结果以 role:tool 回填并续聊到最终文本（工具升级不破循环协议）。
#[tokio::test]
async fn t1_model_loop_delivers_query_and_edit_tool_calls() {
    // 轮次 1：query_tasks（view/tag/limit 新参数）→ 轮次 2：edit_task（model/owner 新参数）→
    // 轮次 3：最终文本
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "query_tasks".into(),
        arguments: r#"{"view":"done","tag":"周报","limit":10}"#.into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "edit_task".into(),
        arguments: r#"{"taskId":"t1e2e-active","model":"mock-model-t1","owner":"T1测试成员甲"}"#
            .into(),
    }));
    server.push_behavior(MockBehavior::TextReply("已更新 1 张卡".into()));
    let h = CoreHarness::new();

    let calls: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let calls2 = calls.clone();
    let exec = move |name: String, args: String, _trace: ToolCallTrace| {
        let calls = calls2.clone();
        async move {
            let is_query = name == "query_tasks";
            calls.lock().unwrap().push((name, args));
            if is_query {
                ToolResult::ok("- [已完成] T1样例卡（id=t1e2e-done）", Vec::new())
            } else {
                ToolResult::ok("已更新任务「样例」（执行模型、归属人）", Vec::new())
            }
        }
    };

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        6,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("工具回路应 Ok");

    assert_eq!(text, "已更新 1 张卡");
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[
            (
                "query_tasks".to_string(),
                r#"{"view":"done","tag":"周报","limit":10}"#.to_string()
            ),
            (
                "edit_task".to_string(),
                r#"{"taskId":"t1e2e-active","model":"mock-model-t1","owner":"T1测试成员甲"}"#
                    .to_string()
            ),
        ],
        "新工具名与新参数应原样抵达 executor"
    );
    assert_eq!(server.request_count(), 3, "两次工具调用后第三轮收尾");
}

/// B：DB 往返 + RMW 写回（真 SQL/真写锁/共享库文件）——工具层读写的全部新列
/// 在 SQLite 的落库与读出；edit_task 的「load → 改 → 自戳 → upsert」模式验证。
/// （db_upsert/db_load 命令是 AppHandle<Wry> 类型，mock runtime 下走 conn 层同函数：
/// open_db + lock_db_write + upsert_tasks + load_all，与生产读写同一代码路径。）
/// schedule 列按 T1 定时单源契约断言：open_db 迁移把非空 tasks.schedule 迁往
/// scheduled_jobs（job-<task_id>）并清空任务卡侧——任务卡 schedule 不再是写路径。
#[tokio::test]
async fn t1_db_roundtrip_new_columns_and_rmw_edit() {
    let app = mock_handle();
    t1_cleanup(&app);

    // 种子：新列全非空的卡（model/owner_id/depends_on/子任务/created_at）
    let mut t = t1_seed_task(
        "t1e2e-active",
        T1_TITLE_ACTIVE,
        wmessage_lib::db::TaskStatus::Todo,
    );
    t.model = Some("mock-model-t1".into());
    t.owner_id = Some(T1_OWNER_ID.into());
    t.depends_on = Some(vec!["t1e2e-dep".into()]);
    t.subtasks = Some(vec![wmessage_lib::db::Subtask {
        id: "t1e2e-sub-1".into(),
        text: "核对参数".into(),
        done: false,
        task_id: None,
    }]);
    {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        let _g = wmessage_lib::db::lock_db_write();
        wmessage_lib::db::upsert_tasks(&conn, std::slice::from_ref(&t)).unwrap();
    }

    // 往返：load_all 读出的字段与写入一致（edit_task/query_single_task 的读取接缝）
    let tasks = {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        wmessage_lib::db::load_all(&conn).unwrap()
    };
    let loaded = tasks.iter().find(|t| t.id == "t1e2e-active").unwrap();
    assert_eq!(loaded.model.as_deref(), Some("mock-model-t1"));
    assert_eq!(loaded.owner_id.as_deref(), Some(T1_OWNER_ID));
    assert_eq!(
        loaded.depends_on.as_deref(),
        Some(&["t1e2e-dep".to_string()][..])
    );
    assert_eq!(loaded.created_at, Some(1_000));
    assert_eq!(loaded.subtasks.as_ref().unwrap()[0].id, "t1e2e-sub-1");

    // RMW 编辑模式（tool_edit_task 同款）：基线 = 快照 updated_at，改 model/owner + 勾子任务
    let mut next = loaded.clone();
    next.model = None; // 空串清除语义 → None
    next.owner_id = None; // 归属本人语义 → None
    next.subtasks.as_mut().unwrap()[0].done = true; // subtaskId 勾选语义
    next.expected_updated_at = next.updated_at; // RMW 写回基线
    next.updated_at = Some(2_000);
    {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        let _g = wmessage_lib::db::lock_db_write();
        wmessage_lib::db::upsert_tasks(&conn, std::slice::from_ref(&next)).unwrap();
    }

    let tasks = {
        let conn = wmessage_lib::db::open_db(&app).unwrap();
        wmessage_lib::db::load_all(&conn).unwrap()
    };
    let after = tasks.iter().find(|t| t.id == "t1e2e-active").unwrap();
    assert_eq!(after.model, None, "清除后应落 NULL");
    assert_eq!(after.owner_id, None);
    assert!(after.subtasks.as_ref().unwrap()[0].done, "勾选应落库");

    t1_cleanup(&app);
}

/// N5：工具随结果附图 → 图作为紧随 tool 消息的 user 消息进上下文。
/// 断言第二轮请求体：tool 消息文本在前，紧跟 user 消息带〔系统附图〕+ data URL 图片。
#[tokio::test]
async fn n5_tool_image_appended_as_user_message() {
    // 临时「截图」文件：image_part_from_file 只校验扩展名/大小，不解码内容
    let dir = std::env::temp_dir().join(format!(
        "wm-n5-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("shot.png");
    std::fs::write(&img, b"\x89PNG\r\n\x1a\n fake-png-bytes").unwrap();
    let img_path = img.display().to_string();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "screenshot".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply("看到了".into()));
    let h = CoreHarness::new();

    let p = img_path.clone();
    let exec = move |_name: String, _args: String, _trace: ToolCallTrace| {
        let p = p.clone();
        async move { ToolResult::ok_with_images(format!("已截屏：{p}"), Vec::new(), vec![p]) }
    };

    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        4,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("工具回路应 Ok");
    assert_eq!(text, "看到了");
    assert_eq!(server.request_count(), 2, "工具后应续聊一轮");

    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2);
    // 字符安全截断（断言失败时的诊断输出，避免多字节字符字节切片 panic）
    let head: String = bodies[1].chars().take(3000).collect();
    // 第二轮请求体：tool 消息文本在前
    assert!(bodies[1].contains("已截屏"), "{head}");
    // 紧随的 user 消息：〔系统附图〕提示 + data URL 图片
    assert!(bodies[1].contains("系统附图"), "缺附图提示：{head}");
    assert!(
        bodies[1].contains("data:image/png;base64,"),
        "缺 base64 图片"
    );
    // 图在 tool 消息之后（user 消息里），不在 tool 消息内（OpenAI 协议 tool 只收文本）
    let tool_pos = bodies[1].find("已截屏").unwrap();
    let user_img_pos = bodies[1].find("data:image/png;base64,").unwrap();
    assert!(user_img_pos > tool_pos, "图应出现在 tool 消息之后");

    std::fs::remove_dir_all(&dir).ok();
}

// ────────────────────────────────────────────────────────────────────
// D-Y/N selfcheck 扩到 final_text 层（Round 7 final-selfcheck 节流闸）：
// 主出口前置触发 LLM 自检，Y 路径静默还原 final_text；N 路径追加 ⚠️ +
// emit + audit_log。节流：final_text 字数 ≥ 80 + function_calls ≥ 3 +
// 距上次 ≥ gap 轮。gap=0 = 关闭。Y/N 字样走 expecting_final_selfcheck
// 视觉过滤（与 SELFCHECK 同套 expecting_selfcheck 流式跳 emit 套路）。
// ────────────────────────────────────────────────────────────────────

/// 长文 + 3 function_calls + Y 响应：trigger 触发后还原 final_text（Y 路径），
/// 原文本保留，Y 字样因视觉过滤不展示给用户。
#[tokio::test]
async fn core_final_selfcheck_y_restores_final_text() {
    let server = MockLlmServer::start();
    // 3 个 tool_calls 让 function_calls_total 凑够 3
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 4 轮模型给长文 final_text（≥ 80 字触发 final 自检 trigger）
    server.push_behavior(MockBehavior::TextReply(
        "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。任务列表展示在文档中，每一条都核对过状态与期限，没有遗漏任何待办事项，可以放心进入下一阶段的安排，若有需要我可以继续细化每个条目。"
            .into(),
    ));
    // 第 5 轮 final 自检响应：模型判 Y
    server.push_behavior(MockBehavior::TextReply("Y".into()));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(1)), // gap=1：第 4 轮首次满足即触发
        exec,
        noop_replan,
    )
    .await
    .expect("Y 路径应 Ok");
    // Y 路径：final_text 还原到原文（Y 字样被丢弃，无 ⚠️ 追加）
    assert!(
        text.contains("已完成 3 项工作"),
        "Y 路径应保留原文：{text:?}"
    );
    assert!(
        !text.contains("⚠️ 自检未达预期"),
        "Y 路径不应追加警告：{text:?}"
    );
    // 5 次 LLM 请求（3 tool + 1 final text + 1 selfcheck response）
    assert_eq!(server.request_count(), 5);
}

/// 长文 + 3 function_calls + N 响应：trigger 触发后追加 ⚠️ + 写 audit_log。
#[tokio::test]
async fn core_final_selfcheck_n_appends_warning_and_audits() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    let long_text = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。所有条目都逐条核对过状态、期限与负责人，异常项已单独标注，完整清单以文档形式存档，随时可以复查或导出。";
    server.push_behavior(MockBehavior::TextReply(long_text.into()));
    // 第 5 轮 final 自检响应：模型判 N
    server.push_behavior(MockBehavior::TextReply("N".into()));

    let h = CoreHarness::new();
    // 收集 audit_log 行（h.audit_log 默认 noop，自己包一个捕获版）
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let logs2 = logs.clone();
    // 直接用 deps() 但覆盖 audit_log 闭包捕获 final_selfcheck.fail 行
    let mut deps = h.deps_with_gap(Some(1));
    deps.audit_log = {
        let logs2 = logs2.clone();
        Box::leak(Box::new(move |line: &str| {
            logs2.lock().unwrap().push(line.to_string());
        }))
    };
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("N 路径应 Ok");
    // N 路径：原文保留 + ⚠️ 追加
    assert!(text.contains(long_text), "N 路径应保留原文：{text:?}");
    assert!(
        text.contains("⚠️ 自检未达预期"),
        "N 路径应追加警告：{text:?}"
    );
    // audit_log 应记 final_selfcheck.fail
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("final_selfcheck.fail")),
        "应记 final_selfcheck.fail 审计：{lines:?}"
    );
}

/// 自检响应附解释（"N，……"非纯 Y/N）：宽松前缀判定应识别出 N——原文 + ⚠️ 追加。
/// 修复前走严格整段匹配得到 None → 当通过处理，幻觉自白被静默丢弃。
#[tokio::test]
async fn core_final_selfcheck_explained_n_appends_warning() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    let long_text = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。所有条目都逐条核对过状态、期限与负责人，异常项已单独标注，完整清单以文档形式存档，随时可以复查或导出。";
    server.push_behavior(MockBehavior::TextReply(long_text.into()));
    // 第 5 轮：模型答「N + 解释」——幻觉自白不能因为不是纯 N 就被丢弃
    server.push_behavior(MockBehavior::TextReply(
        "N，报告中把完成日期写错了，实际是下周".into(),
    ));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(1)),
        exec,
        noop_replan,
    )
    .await
    .expect("附解释 N 路径应 Ok");
    assert!(text.contains(long_text), "原文应保留：{text:?}");
    assert!(
        text.contains("⚠️ 自检未达预期"),
        "附解释的 N 也应追加警告，实际：{text:?}"
    );
}

/// 终稿自检响应轮带工具调用（以行动代回答）：应释放快照与流抑制，
/// 最终返回模型新的正常回答——而不是把旧快照原样顶回去（修复前行为）。
#[tokio::test]
async fn core_final_selfcheck_tool_reply_releases_snapshot() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 4 轮长文触发 final 自检（gap 默认 8，首次满足即触发）
    server.push_behavior(MockBehavior::TextReply(
        "已完成全部工作：本轮列出并逐条核对了所有任务条目，统计了完成与遗留情况，生成了汇总报告并写入文档，任务列表与统计结果均已在会话中展示完毕，没有遗漏任何步骤与细节，可以放心查阅。"
            .into(),
    ));
    // 第 5 轮：模型不答 Y/N 而是继续调工具核验（以行动代回答）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 6 轮：核验后的新最终回答——不应被第 4 轮快照顶掉
    // （gap=8：第 6 轮距上次触发仅 2 轮，不会二次触发；字数 <80 双保险）
    server.push_behavior(MockBehavior::TextReply(
        "复核完成：所有条目与报告一致，任务全部就绪。".into(),
    ));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(None),
        exec,
        noop_replan,
    )
    .await
    .expect("以行动代回答路径应 Ok");
    assert!(
        text.contains("复核完成"),
        "应返回核验后的新回答，实际（旧快照被顶回）：{text:?}"
    );
    // 6 次 LLM 请求：3 tool + 长文 + 行动代答 + 新回答
    assert_eq!(server.request_count(), 6);
}

/// gap=0（关闭）：长文 + 3 function_calls 也不触发 final 自检，无额外 LLM 请求。
#[tokio::test]
async fn core_final_selfcheck_disabled_with_gap_zero() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    let long_text = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。";
    server.push_behavior(MockBehavior::TextReply(long_text.into()));
    // **不**推第 5 个行为：若 trigger 触发会因无响应挂死或 panic

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(0)), // 关闭
        exec,
        noop_replan,
    )
    .await
    .expect("关闭路径应 Ok");
    // 关闭：原文保留，无 ⚠️
    assert_eq!(text, long_text);
    // 4 次 LLM 请求（3 tool + 1 final text），无第 5 次自检
    assert_eq!(server.request_count(), 4);
}

/// final_text < 80 字 + 3 function_calls + gap=1：不触发（trigger 硬条件字数不过）。
#[tokio::test]
async fn core_final_selfcheck_skips_short_final_text() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 短 final_text（< 80 字，触发硬条件不过）
    server.push_behavior(MockBehavior::TextReply("完成了".into()));
    // **不**推第 5 个行为：若 trigger 触发会因无响应挂死或 panic

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(1)), // gap=1，但字数不过也不会触发
        exec,
        noop_replan,
    )
    .await
    .expect("短文应 Ok");
    assert_eq!(text, "完成了");
    assert_eq!(server.request_count(), 4);
}

// ────────────────────────────────────────────────────────────────────
// 工具语义自检（SELFCHECK）回复接线：纯 Y/N 不是给用户的答案。
// 修复前两处断裂：注入点 take() 提前消费 pending → 回复轮解析永远扑空
// （N 换策略路径死代码）；纯 Y/N 无工具调用走主出口直接 return → 回合被
// 自检终结（Y 顶掉正常汇报 / N 的「换策略中」后无任何动作）。
// 修复后：Y → 续跑让模型产出正常汇报；N → 注入换策略提示后续跑；
// 附解释 → 按正常答案终结回合。
// ────────────────────────────────────────────────────────────────────

/// 捕获 bot-chat-delta 事件的 deps（emit 默认 noop，视觉提示断言需要捕获版）
fn deps_capturing_chat_deltas(h: &CoreHarness) -> (ModelLoopDeps<'_>, Arc<Mutex<Vec<String>>>) {
    let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deltas2 = deltas.clone();
    let mut deps = h.deps();
    deps.emit = Box::leak(Box::new(move |event: &str, v: serde_json::Value| {
        if event == "bot-chat-delta" {
            if let Some(t) = v.get("text").and_then(|t| t.as_str()) {
                deltas2.lock().unwrap().push(t.to_string());
            }
        }
    }));
    (deps, deltas)
}

/// create_task 成功触发工具自检，模型答 N：应发出 ⚠️ 视觉提示、注入换策略
/// 提示并续跑一轮拿到恢复汇报——而不是以 "N" 终结回合。
#[tokio::test]
async fn core_tool_selfcheck_n_switches_strategy_and_continues() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    // 第 2 轮：模型对自检答 N（纯 Y/N 不是给用户的答案）
    server.push_behavior(MockBehavior::TextReply("N".into()));
    // 第 3 轮：收到换策略提示后的恢复汇报
    server.push_behavior(MockBehavior::TextReply(
        "已换用正确参数重新创建任务成功".into(),
    ));

    let h = CoreHarness::new();
    let (deps, deltas) = deps_capturing_chat_deltas(&h);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("N 换策略路径应 Ok");
    assert_eq!(
        text.trim(),
        "已换用正确参数重新创建任务成功",
        "N 后应续跑并返回恢复汇报，实际：{text:?}"
    );
    // 3 次 LLM 请求：tool → 自检回复 N → 换策略后的恢复汇报
    assert_eq!(server.request_count(), 3, "N 应触发续跑而非终结回合");
    // 视觉：⚠️ 提示发给用户；N 字样本身不外露
    let joined = deltas.lock().unwrap().join("");
    assert!(
        joined.contains("⚠️ 自检未达预期"),
        "应发出 ⚠️ 视觉提示：{joined:?}"
    );
}

/// create_task 成功触发工具自检，模型答 Y：应续跑一轮拿到正常汇报，
/// 而不是以 "Y"（或孤零零的 ✅）终结回合把汇报顶掉。
#[tokio::test]
async fn core_tool_selfcheck_y_continues_to_report() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply("Y".into()));
    server.push_behavior(MockBehavior::TextReply(
        "任务已创建，可在任务页查看详情".into(),
    ));

    let h = CoreHarness::new();
    let (deps, deltas) = deps_capturing_chat_deltas(&h);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("Y 续跑路径应 Ok");
    assert_eq!(
        text.trim(),
        "任务已创建，可在任务页查看详情",
        "Y 后应续跑返回正常汇报，实际：{text:?}"
    );
    assert_eq!(server.request_count(), 3, "Y 应触发续跑而非终结回合");
    let joined = deltas.lock().unwrap().join("");
    assert!(
        joined.contains("✅ 自检通过"),
        "应发出 ✅ 视觉提示：{joined:?}"
    );
}

/// 自检回复附解释（非纯 Y/N）：当正常答案走主出口终结回合，不额外注入提示。
#[tokio::test]
async fn core_tool_selfcheck_explanation_treated_as_final_answer() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    let explained = "Y，文件已写入项目根目录，无需调整";
    server.push_behavior(MockBehavior::TextReply(explained.into()));
    // **不**推第 3 个行为：若续跑被误触发会因无响应挂死/panic

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("附解释路径应 Ok");
    assert_eq!(text, explained, "附解释应作为正常答案返回");
    assert_eq!(server.request_count(), 2, "附解释不续跑");
}

/// 滚动降密度：连续 7 轮工具调用后，round-0 的工具结果轮龄 ≥5
/// → 触发 `loop.msgs.age_compacted` 软压审计事件；随后文本轮正常收尾。
#[tokio::test]
async fn core_age_based_rolling_compaction_fires() {
    let server = MockLlmServer::start();
    for _ in 0..7 {
        server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    server.push_behavior(MockBehavior::TextReply("全部查完了".into()));

    let h = CoreHarness::new();
    // 长工具结果：太短（≤300 字符）压缩器会跳过，滚动降密度就没有可压对象
    let long_result = "X".repeat(1500);
    let exec = |_n: String, _a: String, _t: ToolCallTrace| {
        let long_result = long_result.clone();
        async move { ToolResult::ok(long_result, Vec::new()) }
    };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        12,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("滚动降密度路径应 Ok");
    assert_eq!(text, "全部查完了");
    assert_eq!(server.request_count(), 8);
    let events = h.events();
    assert!(
        events.contains(&"loop.msgs.age_compacted"),
        "round-0 的工具结果轮龄应到 5 触发软压：{events:?}"
    );
}

// ────────────────────────────────────────────────────────────────────
// 终稿自检侧调（final_selfcheck_http）：自检走独立小请求（非流式 JSON，
// summarize_http 同路径），主循环不多跑一轮——主服务器请求数 = 3 tool +
// 1 终文，判定打到独立的 side server。
// ────────────────────────────────────────────────────────────────────

/// 侧调小模型连接：复用 core_http 的连接参数（鉴权走同一 mock 口径），
/// 只换端点与模型名
fn selfcheck_http(server: &MockLlmServer) -> LlmHttp {
    LlmHttp {
        client: reqwest::Client::new(),
        base_url: server.base_url.clone(),
        model: "mock-small".into(),
        ..core_http(server)
    }
}

const SIDE_LONG_TEXT: &str = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。任务列表展示在文档中，每一条都核对过状态与期限，没有遗漏任何待办事项，可以放心进入下一阶段的安排，若有需要我可以继续细化每个条目。";

/// 侧调 Y：原文还原、无 ⚠️；主服务器 4 次请求（3 tool + 终文），侧服务器 1 次。
#[tokio::test]
async fn core_final_selfcheck_sidecall_y_no_extra_main_round() {
    let main = MockLlmServer::start();
    let side = MockLlmServer::start();
    for _ in 0..3 {
        main.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    main.push_behavior(MockBehavior::TextReply(SIDE_LONG_TEXT.into()));
    side.push_behavior(MockBehavior::OpenAiJsonReply("Y".into()));

    let h = CoreHarness::new();
    let sc = selfcheck_http(&side);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&main),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_selfcheck(&sc),
        exec,
        noop_replan,
    )
    .await
    .expect("侧调 Y 路径应 Ok");
    assert!(text.contains("已完成 3 项工作"), "Y 应还原原文：{text:?}");
    assert!(!text.contains("⚠️"), "Y 不应追加警告：{text:?}");
    assert_eq!(main.request_count(), 4, "侧调不占主循环轮次");
    assert_eq!(side.request_count(), 1, "自检请求应打到侧调服务器");
}

/// 侧调 N（附解释「N，……」）：宽松前缀判定命中 N → 原文 + ⚠️ + 审计。
#[tokio::test]
async fn core_final_selfcheck_sidecall_n_appends_warning() {
    let main = MockLlmServer::start();
    let side = MockLlmServer::start();
    for _ in 0..3 {
        main.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    main.push_behavior(MockBehavior::TextReply(SIDE_LONG_TEXT.into()));
    // 非流式应答带解释——宽松前缀解析取首字符判 N
    side.push_behavior(MockBehavior::OpenAiJsonReply(
        "N，报告中把完成日期写错了".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sc = selfcheck_http(&side);
    let mut deps = h.deps_with_selfcheck(&sc);
    deps.audit_log = {
        let logs2 = logs.clone();
        Box::leak(Box::new(move |line: &str| {
            logs2.lock().unwrap().push(line.to_string());
        }))
    };
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&main),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("侧调 N 路径应 Ok");
    assert!(text.contains("已完成 3 项工作"), "N 应保留原文：{text:?}");
    assert!(text.contains("⚠️ 自检未达预期"), "N 应追加警告：{text:?}");
    assert_eq!(main.request_count(), 4, "侧调不占主循环轮次");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("final_selfcheck.fail")),
        "应记 final_selfcheck.fail 审计：{lines:?}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// 执行类谎报守卫：整场零工具调用 + 终稿叙述执行结果 → 打回一轮。
//
// 事故原话（session 81310e3a message 690，对应 trace turns=0 tools=0 ms=1866）：
// 用户点名 run_python 要它 sleep+打印，模型一次工具都没调，却报「执行完毕，
// sleep 120 秒后工具真实返回，打印了 done」。此前的守卫只有 mutating 一档，
// 非 mutating 工具（run_python / fetch_url / bash）的执行结果可以凭空叙述。
// ────────────────────────────────────────────────────────────────────────────

/// 捕获 audit_log 的 deps，用于断言守卫记了什么审计
fn deps_capturing_logs<'a>(
    h: &'a CoreHarness,
    logs: &'a Arc<Mutex<Vec<String>>>,
) -> ModelLoopDeps<'a> {
    let mut deps = h.deps();
    deps.audit_log = {
        let logs2 = logs.clone();
        Box::leak(Box::new(move |line: &str| {
            logs2.lock().unwrap().push(line.to_string());
        }))
    };
    deps
}

/// 零工具调用 + 执行完成态终稿：应打回一轮，模型改口如实说明后才终结回合。
#[tokio::test]
async fn core_execution_claim_without_any_tool_call_is_pushed_back() {
    let server = MockLlmServer::start();
    // 第 1 轮：事故原话——零工具调用却报执行完成
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，sleep 120 秒后工具真实返回，打印了 `done`。".into(),
    ));
    // 第 2 轮：收到系统提示后改口，如实说明做不到（同会话 684/686 的正确行为）
    server.push_behavior(MockBehavior::TextReply(
        "受白名单约束，run_python 无法执行，不能编造结果。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    // exec_never：整场不该有任何工具被执行；真被调会 panic
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("打回后应 Ok");
    assert_eq!(
        text.trim(),
        "受白名单约束，run_python 无法执行，不能编造结果。",
        "应续跑并以模型的如实说明终结，实际：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "守卫应打回一轮而不是直接终结");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但本次提问零凭证")),
        "应记执行类谎报审计：{lines:?}"
    );
}

/// 真的调过工具之后：同样的完成态话术不得被拦（跨轮转述工具结果是常态）。
#[tokio::test]
async fn core_execution_claim_passes_after_a_tool_actually_ran() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 工具跑过之后的收尾话术——带执行完成态，但确实动过手
    server.push_behavior(MockBehavior::TextReply("已执行完毕，任务清单如上。".into()));

    let h = CoreHarness::new();
    let deps = h.deps();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("工具跑过后应直接 Ok");
    assert_eq!(
        text.trim(),
        "已执行完毕，任务清单如上。",
        "动过手就不该拦：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "不应被守卫多打一轮");
}

/// 零工具调用但如实拒绝：一次请求就该终结。守卫若误拦，等于惩罚诚实——
/// 同会话 message 684/686/688 正是这个形态。
#[tokio::test]
async fn core_honest_refusal_without_tool_call_ends_immediately() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "受白名单约束，`node_modules` 路径不在白名单，工具拒绝真去扫。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("如实拒绝应直接 Ok");
    assert!(text.contains("不在白名单"), "应原样返回模型的拒绝说明");
    assert_eq!(server.request_count(), 1, "如实拒绝不该被守卫多打一轮");
}

/// 实机抓到的第二起漏网：模型零工具调用，却用「分步叙述」报出完整的两步执行、
/// 异常原文和 stdout。窄词表（「已执行」家族）全不命中，扩表后必须打回。
#[tokio::test]
async fn core_step_narrated_execution_fabrication_is_pushed_back() {
    let server = MockLlmServer::start();
    // 第 1 轮：trace 51 的原话
    server.push_behavior(MockBehavior::TextReply(
        "完成计划2 步：第一步 `import nosuchmodule_zzz` 触发 `ModuleNotFoundError: No module named 'nosuchmodule_zzz'`，\
         第二步 `print(\"done\")` 仍输出 `done`（异常已捕获，未中断程序）。"
            .into(),
    ));
    // 第 2 轮：收到「必须发起 tool_call」的返工指令后，这次真去调
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"import nosuchmodule_zzz"}"#.into(),
    }));
    // 第 3 轮：拿到真实失败返回后才汇报
    server.push_behavior(MockBehavior::TextReply(
        "已执行完毕，确实失败了：ModuleNotFoundError: No module named 'nosuchmodule_zzz'。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec = |_n: String, _a: String, _t: ToolCallTrace| async {
        ToolResult::ok("ModuleNotFoundError", Vec::new())
    };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("打回后应 Ok");
    assert!(
        text.contains("ModuleNotFoundError"),
        "应续跑到真实执行后的汇报，实际：{text:?}"
    );
    assert_eq!(
        server.request_count(),
        3,
        "应「编造→打回→真调工具→据实汇报」三段"
    );
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但本次提问零凭证")),
        "应记执行类谎报审计：{lines:?}"
    );
}

/// 工具发起过但全部失败：返工指令必须点明「调过了但没成功」，而不是笼统说
/// 「你从未调用工具」——说错会让模型以为自己漏调了一次、转而去重复同一调用。
#[tokio::test]
async fn core_all_failed_tools_report_distinct_remedy() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"1/0"}"#.into(),
    }));
    // 工具失败后模型仍报成功（终稿含执行完成态）
    server.push_behavior(MockBehavior::TextReply("已执行完毕，一切正常。".into()));
    // 第 3 轮改口
    server.push_behavior(MockBehavior::TextReply(
        "执行失败了：ZeroDivisionError。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let exec = |_n: String, _a: String, _t: ToolCallTrace| async {
        ToolResult::error("ZeroDivisionError: division by zero", Vec::new())
    };
    let (_text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("应 Ok");
    assert_eq!(
        server.request_count(),
        3,
        "工具失败后的成功汇报同样应打回一轮"
    );
}

/// 零工具调用的代码讲解轮：不得被扩表后的守卫误伤（讲「会抛出什么」是正当回答）。
#[tokio::test]
async fn core_code_explanation_without_tool_call_is_not_pushed_back() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "这段代码在 `import nosuchmodule_zzz` 处会抛出 ModuleNotFoundError，因为环境里没装这个模块。"
            .into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("讲解轮应直接 Ok");
    assert!(text.contains("ModuleNotFoundError"), "应原样返回讲解");
    assert_eq!(server.request_count(), 1, "代码讲解不该被守卫多打一轮");
}

// ────────────────────────────────────────────────────────────────────────────
// 凭证表判决：点名的工具逐个查本次提问的凭证池。
//
// 旧写法是一个布尔「本回合有没有任何工具成功」，同一次提问里不同工具的凭证能
// 互相担保——本轮成功调了 A，编造 B 的执行结果照样放行。下面两条把两侧都钉死。
// ────────────────────────────────────────────────────────────────────────────

/// 本轮成功调了 list_tasks，终稿却声称调了 run_python —— 必须拦。
/// 这是旧布尔写法漏掉的那一类：any_tool_succeeded 为真 → claim_execution 直接 false。
#[tokio::test]
async fn core_receipt_table_blocks_claiming_an_unrun_tool() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 编造：点名 run_python，但本次提问里从未调过它
    server.push_behavior(MockBehavior::TextReply(
        "已执行完毕，run_python 跑出来是 42。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply(
        "抱歉，我并没有调用过 run_python——本次只列了任务清单。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("打回后应 Ok");
    assert!(
        text.contains("并没有调用过 run_python"),
        "应改口承认没调过：{text:?}"
    );
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但无对应凭证: run_python")),
        "审计应点名缺失的工具：{lines:?}"
    );
}

/// 跨轮据实汇报：凭证在上一轮、汇报在下一轮，必须放行且不多问一轮。
///
/// 真实事故：trace 53 的最终汇报「执行完毕：模块不存在…最终输出 done」是真话，
/// 凭证落在上一轮。按轮清空凭证池会把它判成幻觉，再让模型重跑一个已完成的
/// 动作——对 delete_task / link_file_to_task 这类就是重复的破坏性操作。
#[tokio::test]
async fn core_receipt_table_survives_cross_round_truthful_report() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print('done')"}"#.into(),
    }));
    // 下一轮据实转述上一轮的凭证：点名 run_python，且凭证表里确有它
    server.push_behavior(MockBehavior::TextReply(
        "已执行完毕，run_python 输出 `done`。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("据实汇报应直接 Ok");
    assert_eq!(
        text.trim(),
        "已执行完毕，run_python 输出 `done`。",
        "跨轮据实转述不该被拦：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "凭证表跨轮累积，不得多打一轮");
}

/// 建议语气里提到工具名（零工具调用）不该被当成「声称调过它」。
#[tokio::test]
async fn core_advice_mentioning_tool_name_is_not_a_claim() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "这个问题你可以用 grep_files 搜，但我这边没有文件访问权限，搜不了。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("如实说明应直接 Ok");
    assert!(text.contains("没有文件访问权限"), "应原样返回：{text:?}");
    assert_eq!(server.request_count(), 1, "建议语气不得被拦");
}

/// 实机漏网一（trace 63，tools=0）：整句没有完成态词点，闸门压根没进判决。
///
/// 模型零工具调用却报出一次完整执行，而且连历史都是编的——那个会话只跑过
/// time.sleep(2)，从来没有 print 100。
#[tokio::test]
async fn core_first_person_execution_narration_without_receipt_is_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "我跑了一个 print 100 的小任务，返回了 100。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply(
        "我没有执行过任何脚本——刚才那句是错的。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("打回后应 Ok");
    assert!(text.contains("没有执行过任何脚本"), "应改口：{text:?}");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但本次提问零凭证")),
        "应记零凭证：{lines:?}"
    );
}

/// 实机漏网二（trace 65，tools=0）：声称派了子 agent 并报了个实测值。
///
/// 双层都得在位才拦得住——「实测约」把话术引进判决，别名表把「子 agent」
/// 映射成 spawn_subagent 让点名查询有键可查。缺任一层都是静默放行。
#[tokio::test]
async fn core_subagent_fabrication_is_blocked_via_alias() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "子 agent 跑 sleep(5) 实测约 5013 ms。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply(
        "我没有派过子 agent，那是编的。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("打回后应 Ok");
    assert!(text.contains("没有派过子 agent"), "应改口：{text:?}");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但无对应凭证: spawn_subagent")),
        "审计应点名 spawn_subagent（别名映射的证据）：{lines:?}"
    );
}

/// 子 agent 凭证归并的正路：主 agent 派过子 agent 并如实汇报它跑的活，不该被拦。
///
/// 子 agent 的 span 在自己的 trace 下，主池里没有 run_python——不做归并的话
/// 这条正路每次汇报都会被当成凭空伪造。
#[tokio::test]
async fn core_subagent_receipts_credit_lets_truthful_report_through() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "spawn_subagent".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply(
        "子 agent 已执行完毕，run_python 实测约 5013 ms。".into(),
    ));

    let h = CoreHarness::new();
    // 子 agent 真的用 run_python 跑出了那个数
    h.set_subagent_tools(&["run_python"]);
    let deps = h.deps();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("据实汇报应直接 Ok");
    assert!(
        text.contains("run_python 实测约 5013 ms"),
        "应原样返回：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "子 agent 归并后不得返工");
}

/// 返工之后仍编造：不再返工，终稿末尾挂一条如实更正。
///
/// 守的是「谎话不能单独到达用户」。此前第二次谎报整块守卫直接跳过（返工机会
/// 一次用完就作废），原文原样出到用户眼前——用户看到的是一句纯假的执行汇报。
/// 现在第二次改成不返工、只标注：再返工会让模型重跑已完成的动作（delete_task
/// 那类是重复的破坏性操作），而谎报已经拦过一次，再拦一次收益不抵风险。
#[tokio::test]
async fn core_second_fabrication_is_marked_not_silently_passed() {
    let server = MockLlmServer::start();
    // 第一轮：零工具调用却叙述执行结果 → 守卫返工
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，工具真实返回，打印了 done。".into(),
    ));
    // 第二轮：仍然编造 → 不再返工，改为在终稿标注
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，工具真实返回，打印了 done。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("第二次编造应 Ok");
    // 谎话本身仍在（不删原文——删掉只会让用户更信那句假的）
    assert!(text.contains("执行完毕"), "应保留原文：{text:?}");
    // 但必须带一条如实更正，让用户知道这句没有凭证
    assert!(
        text.contains("⚠️") && text.contains("没有实际发生"),
        "第二次编造应在终稿标注：{text:?}"
    );
    // 不再返工：只有两次请求（第二次是模型的作答，不是第三次返工）
    assert_eq!(server.request_count(), 2, "第二次编造不得再返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("hallucination_guard.repeat")),
        "应记 repeat 审计：{lines:?}"
    );
}

/// 正路不回归：第一次编造被返工、模型改口后终稿干净，不该挂更正标注。
#[tokio::test]
async fn core_truthful_rewrite_after_rework_carries_no_notice() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，工具真实返回，打印了 done。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply("我没有执行过任何脚本。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("改口后应 Ok");
    assert!(text.contains("没有执行过任何脚本"), "应改口：{text:?}");
    assert!(
        !text.contains("⚠️"),
        "改口后的干净终稿不该挂更正标注：{text:?}"
    );
}

/// 落盘凭证正路：模型真调了文档生成工具（带回凭证），终稿报同一个路径——
/// 存在性与凭证两道都站得住，不该被拦。
///
/// 这条是正路回归锁：凭证那道判据一旦接线错（凭证没进池），每一次正常的
/// 文档生成都会被当成凭空声称 → 守卫让模型重做 → 白跑一遍生成。
#[tokio::test]
async fn core_generated_doc_with_receipt_is_not_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("周报.pptx");
    std::fs::write(&doc, b"pptx").unwrap();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_ppt".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply(format!(
        "已生成 PPT 演示文稿：{}",
        doc.display()
    )));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let receipt = wmessage_lib::bot::FileChangeReceipt {
        path: doc.to_string_lossy().into_owned(),
        kind: "create",
        added: 0,
        deleted: 0,
        diff: None,
        truncated: false,
        before_ref: None,
        before_sha: None,
        after_sha: Some("0f0f".into()),
        replacements: 0,
    };
    let exec = move |_n: String, _a: String, _t: ToolCallTrace| {
        let receipt = receipt.clone();
        async move { ToolResult::ok("已生成 PPT 演示文稿", Vec::new()).with_file_change(receipt) }
    };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("有凭证的正路应 Ok");
    assert!(text.contains("已生成 PPT 演示文稿"), "应原样返回：{text:?}");
    assert!(!text.contains("⚠️"), "有凭证的正路不该被标注：{text:?}");
    assert_eq!(server.request_count(), 2, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "不该记守卫审计：{lines:?}"
    );
}

/// 落盘凭证反路：路径在磁盘上确实存在，但从来不是任何工具写出来的——
/// 模型在凭空声称「已导出」这个文件。
///
/// 用「已导出」而不是「已生成」是有意的：「已导出」不在变更类词表里
/// （claims_mutation 抓不到），所以这一条只有落盘凭证那道判据能拦——测的就是它。
#[tokio::test]
async fn core_claimed_doc_without_receipt_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("不是我生成的.pdf");
    std::fs::write(&doc, b"pdf").unwrap();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(format!(
        "已导出 PDF 文档：{}",
        doc.display()
    )));
    server.push_behavior(MockBehavior::TextReply("这个文件不是我导出的。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 这个路径查不到落盘凭证：它不是任何工具写出来的
    h.file_receipts_missing
        .lock()
        .unwrap()
        .push(doc.to_string_lossy().into_owned());
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("无凭证的声称应被打回后 Ok");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("声称已生成但无落盘凭证")),
        "应记无落盘凭证：{lines:?}"
    );
    assert_eq!(server.request_count(), 2, "应返工一轮");
    assert!(text.contains("不是我导出的"), "模型应被要求改口：{text:?}");
}

/// 不可证伪的写入通道按**本次提问**让开，不是整条会话。
///
/// 本次提问跑过 run_python（它能写盘、但走的是我们没有凭证的通道）→ 落盘凭证
/// 那道判据让开。早先按会话判：一次 run_python 会把这条会话后续所有提问的凭证
/// 闸全打死——那不是「无从确证」，那是把闸门焊死。
#[tokio::test]
async fn core_unverifiable_writer_exemption_is_per_question() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("不是我生成的.pdf");
    std::fs::write(&doc, b"pdf").unwrap();

    let server = MockLlmServer::start();
    // 本次提问先跑一次 run_python（自己写盘的通道，我们拿不到它的落盘凭证）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print('done')"}"#.into(),
    }));
    server.push_behavior(MockBehavior::TextReply(format!(
        "已导出 PDF 文档：{}",
        doc.display()
    )));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 这个路径查不到落盘凭证：run_python 写的东西走的是我们没有凭证的通道
    h.file_receipts_missing
        .lock()
        .unwrap()
        .push(doc.to_string_lossy().into_owned());
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("让开时应直接 Ok");
    assert_eq!(server.request_count(), 2, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "本次提问跑过不可证伪的写入通道，凭证闸应让开：{lines:?}"
    );
    assert!(text.contains("已导出 PDF 文档"), "应原样返回：{text:?}");
}
