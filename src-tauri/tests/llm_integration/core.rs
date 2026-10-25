//! 主循环核心用例：文本回复 / 推理注入 / HTTP 错误分档 / 上下文溢出降级。

use super::*;

/// **一轮里多个 tool_calls** 必须只花一轮：引擎逐个执行本轮的全部调用、
/// 逐个回填 tool 消息，然后才发下一次请求。
///
/// 这条是「并行调用引导」（提示词让模型把互不依赖的调用合并到同一轮）的
/// 落地前提：提示词说得再好，引擎若一轮只认一个调用，省下的轮次就不存在。
/// 回归钉两件事：①两个调用都被执行；②两次请求之间没有多发一轮
/// （第二轮请求体里应同时带着两条 tool 结果）。
#[tokio::test]
async fn core_multiple_tool_calls_in_one_round_costs_one_round() {
    let server = MockLlmServer::start();
    // 一条 assistant 消息里带两个 tool_calls（index 0 / 1）
    let two_calls = r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-chat","choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"read_text_file","arguments":"{}"}},{"index":1,"id":"call_b","type":"function","function":{"name":"grep_files","arguments":"{}"}}]},"index":0,"finish_reason":null}]}

data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-chat","choices":[{"delta":{},"index":0,"finish_reason":"tool_calls"}]}

data: [DONE]

"#;
    server.push_behavior(MockBehavior::RawSse(two_calls.to_string()));
    server.push_behavior(MockBehavior::TextReply("都跑完了".into()));

    let h = CoreHarness::new();
    let called: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let called2 = called.clone();
    let exec = move |name: String, _args: String, _trace: ToolCallTrace| {
        let called = called2.clone();
        async move {
            called.lock().unwrap().push(name);
            ToolResult::ok("ok", Vec::new())
        }
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
    .expect("两调用一轮应 Ok");

    assert_eq!(text, "都跑完了");
    let names = called.lock().unwrap().clone();
    assert_eq!(
        names,
        vec!["read_text_file".to_string(), "grep_files".to_string()],
        "本轮的两个调用都应被执行"
    );
    // 只发了两轮请求：工具轮 + 收尾轮。中间若多发一轮，说明引擎一轮只认一个调用。
    assert_eq!(
        server.request_count(),
        2,
        "两个调用应挤在同一轮里（工具轮 + 收尾轮 = 2 次请求）"
    );
    let bodies = server.request_bodies();
    let second = &bodies[1];
    assert!(
        second.contains("call_a") && second.contains("call_b"),
        "第二轮请求体应同时带两条 tool 响应：{second}"
    );
}

/// 思考段回灌开关的两态消息形状：开 → assistant(tool_calls) 消息带
/// reasoning_content 原文；关 → 不带。
///
/// 这不是可有可无的优化：DeepSeek reasoner 缺了直接 400（"reasoning_content in
/// the thinking mode must be passed back to the API"），Moonshot K2 报"thinking is
/// enabled but reasoning_content is missing in assistant tool call message"。
/// 但回灌要实打实多付一份思考段的 input token，受益只落在要求回灌的模型上，
/// 故做成默认关的开关，而不是一律回灌。
///
/// 断言打在**第二轮**请求体上：第一轮还没发过 assistant(tool_calls) 消息。
#[tokio::test]
async fn core_reasoning_echo_switch_controls_message_shape() {
    let off = reasoning_echo_second_request_body(false).await;
    assert!(
        !off.contains("reasoning_content"),
        "关态不应带 reasoning_content：{off}"
    );
    assert!(off.contains("call_echo"), "两态都应带上工具调用本身：{off}");

    let on = reasoning_echo_second_request_body(true).await;
    assert!(
        on.contains(r#""reasoning_content":"先想一下""#),
        "开态应带本轮 reasoning_content 原文：{on}"
    );
}

/// 跑一轮「思考段 + 工具调用」对话，返回第二轮（工具轮之后那一轮）的请求体。
///
/// 两态共用同一份 SSE：思考段与工具调用同在一个 delta 里，与 DeepSeek
/// reasoner 的真实形状一致。
async fn reasoning_echo_second_request_body(echo: bool) -> String {
    let server = MockLlmServer::start();
    let tool_round = r#"data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-reasoner","choices":[{"delta":{"role":"assistant","reasoning_content":"先想一下","content":null,"tool_calls":[{"index":0,"id":"call_echo","type":"function","function":{"name":"read_text_file","arguments":"{}"}}]},"index":0,"finish_reason":null}]}

data: {"id":"chatcmpl-mock","object":"chat.completion.chunk","created":1234567890,"model":"deepseek-reasoner","choices":[{"delta":{},"index":0,"finish_reason":"tool_calls"}]}

data: [DONE]

"#;
    server.push_behavior(MockBehavior::RawSse(tool_round.to_string()));
    server.push_behavior(MockBehavior::TextReply("查完了".into()));

    let h = if echo {
        CoreHarness::new().with_reasoning_echo()
    } else {
        CoreHarness::new()
    };
    let exec = |_name: String, _args: String, _trace: ToolCallTrace| async {
        ToolResult::ok("内容", Vec::new())
    };

    run_model_loop_core(
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
    .expect("思考段回灌用例应 Ok");

    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2, "工具轮 + 收尾轮 = 2 次请求");
    bodies[1].clone()
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
        wall_budget: None,
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
        wall_budget: None,
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

/// 子 agent 熔断裕量语义：被闸拒（Denied）的调用不得计入熔断计数。
/// 预算 2 → 熔断上限 = 2 + 收尾裕量 5 = 7。模型连发 9 轮被拒的读类调用：
/// 若被拒的也计数，第 8 次就熔断（正文带 ⏹ 标记）；只有「实际执行的才算数」
/// 才能让模型在收尾窗里继续试探并交卷。exec 注入模拟 dispatch 的拒绝回包。
#[tokio::test]
async fn core_denied_tool_calls_do_not_burn_fuse_margin() {
    use std::sync::atomic::AtomicUsize;

    let sid = "sess-fuse-margin-e2e";
    let used = Arc::new(AtomicUsize::new(0));
    let wrap_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    wmessage_lib::tool_guard::register_subagent_session(
        sid,
        wmessage_lib::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_fuse_margin".into(),
            task_id: None,
            profile: wmessage_lib::db::SubagentProfile::Research,
            tools: Arc::new(vec!["read_text_file", "write_artifact_file"]),
            budget: wmessage_lib::db::SubagentBudget {
                max_turns: 50,
                max_tool_calls: 2,
                max_wall_seconds: 600,
            },
            artifact_dir: std::env::temp_dir().join("wmsg-fuse-margin-e2e"),
            used_tool_calls: used.clone(),
            used_rounds: Arc::new(AtomicUsize::new(0)),
            wrap_up: wrap_flag.clone(),
            wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
        },
    );

    let server = MockLlmServer::start();
    // 9 轮读类调用（exec 全部回 Denied，模拟 dispatch 预算闸的拒绝），然后交卷
    for _ in 0..9 {
        server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "read_text_file".into(),
            arguments: r#"{"path":"/x.txt"}"#.into(),
        }));
    }
    server.push_behavior(MockBehavior::TextReply(
        r#"{"status":"succeeded","summary":"交卷"}"#.into(),
    ));

    let h = CoreHarness::with_session(Some(sid));
    let exec = |name: String, _args: String, _trace: ToolCallTrace| async move {
        assert_eq!(name, "read_text_file");
        ToolResult::denied("⚠️ 工具调用预算已触顶：收尾窗内只放行写产物工具。")
    };

    let (text, _refs) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        20,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("被拒不计数时不应熔断，应跑到交卷轮");

    wmessage_lib::tool_guard::unregister_subagent_session(sid);

    assert!(
        text.contains("succeeded") && !text.contains("⏹ 已熔断"),
        "9 次被拒调用不得烧掉 7 的熔断上限（预算 2 + 裕量 5）：{text}"
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
        CommandError::LlmApiError { status, detail } => {
            assert_eq!(status, 401);
            assert!(
                detail.contains("Invalid API key"),
                "detail 应含服务端错误详情：{detail}"
            );
            assert!(
                detail.contains("API Key"),
                "401 应带可操作指引（检查密钥）：{detail}"
            );
        }
        other => panic!("401 应包装为 LlmApiError，got {other:?}"),
    }
    // mock server 在写回响应前已计数，此处无需 sleep 等待即可断言
    assert_eq!(server.request_count(), 1, "401 不可重试，只发 1 次请求");
}

#[tokio::test]
async fn core_http_402_gives_actionable_balance_message() {
    // 2026-10-24 实机：402 余额不足原样把上游 JSON 甩进气泡
    // （`{"type":"error","error":{"type":"insufficient_balance_error",...}}`），
    // 用户看不出该充值还是换模型。这里锁「说人话 + 不外泄原始 JSON」。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        402,
        r#"{"type":"error","error":{"type":"insufficient_balance_error","message":"insufficient balance (1008)"},"request_id":"072bb75eb78196241b5ab6e12744a166"}"#.into(),
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
        Ok(_) => panic!("402 应报错"),
    };

    match err {
        CommandError::LlmApiError { status, detail } => {
            assert_eq!(status, 402);
            assert!(detail.contains("余额"), "应说清是余额问题：{detail}");
            assert!(detail.contains("充值"), "应给出下一步：{detail}");
            assert!(
                detail.contains("insufficient balance (1008)"),
                "上游自己说的那句要留着：{detail}"
            );
            assert!(
                !detail.contains("request_id"),
                "上游原始 JSON 不该外泄：{detail}"
            );
        }
        other => panic!("402 应包装为 LlmApiError，got {other:?}"),
    }
    // 402 不在重试白名单：余额不会因为重试而回来
    assert_eq!(server.request_count(), 1, "402 不可重试，只发 1 次请求");
}

#[tokio::test]
async fn core_http_429_retried_once_then_success() {
    // 重试退避（MAX_LLM_ATTEMPTS=2，P1-4）：429 第一次 → 记 llm.retry → 第二次成功  audit-ok
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
    // P1-3：200 流内错误载荷（OneAPI 类网关）必须显式报错，不再返回空白回复  audit-ok
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
    // P1-1 防线：干净 EOF（无 [DONE] / 无 finish_reason）+ 残缺 tool_calls →  audit-ok
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
    // P1-1 防线另一支：干净 EOF 无 tool_calls 但有正文 → 不报错，  audit-ok
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
