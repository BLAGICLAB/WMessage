//! Anthropic 协议面用例：/v1/messages 路由、思考块、长度截断续写。

use super::*;

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
async fn core_anthropic_context_overflow_resends_anthropic_shaped_body() {
    // Anthropic 协议下的上下文超长降级：砍完历史重发的请求体必须仍是
    // Anthropic 形状（content 是块数组、system 走顶层字段）。
    // 把 OpenAI 形状的 msgs 直接回填进 body 的话，role=system / role=tool
    // 都不被 Anthropic 接受 → 必 400；而那个 400 文案不再像超长，
    // 用户连「请 /compact」的指引都拿不到。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::HttpError(
        400,
        r#"{"error":{"message":"prompt is too long: 210000 tokens > 200000 maximum","type":"invalid_request_error"}}"#
            .to_string(),
    ));
    server.push_behavior(MockBehavior::AnthropicTextReply("降级后正常回复".into()));
    let h = CoreHarness::new();

    let msgs: Vec<serde_json::Value> = vec![
        serde_json::json!({"role": "system", "content": "系统提示词"}),
        serde_json::json!({"role": "user", "content": "旧问题一"}),
        serde_json::json!({"role": "assistant", "content": "旧回答一"}),
        serde_json::json!({"role": "user", "content": "旧问题二"}),
        serde_json::json!({"role": "assistant", "content": "旧回答二"}),
        serde_json::json!({"role": "user", "content": "本轮问题"}),
    ];

    let (text, _) = run_model_loop_core(
        &core_http_anthropic(&server),
        msgs,
        5,
        &h.stop,
        None,
        &h.deps(),
        exec_never,
        noop_replan,
    )
    .await
    .expect("Anthropic 超长降级后应成功返回");

    assert!(text.contains("降级后正常回复"), "got: {text}");
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2, "应重发一次：{bodies:?}");
    let second: serde_json::Value =
        serde_json::from_str(&bodies[1]).expect("第二次请求体应是 JSON");
    let arr = second["messages"]
        .as_array()
        .expect("messages 应是数组")
        .clone();
    assert!(!arr.is_empty(), "重发体里应还有消息");
    for m in &arr {
        assert!(
            m["content"].is_array(),
            "Anthropic 的 content 必须是块数组（OpenAI 形状的字符串 content 会被拒）：{m}"
        );
    }
    assert!(
        second.get("system").is_some(),
        "system 段在 Anthropic 走顶层字段，不进 messages：{second}"
    );
    assert!(
        !bodies[1].contains("旧回答二"),
        "旧历史应已砍掉：{}",
        bodies[1]
    );
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
    // Anthropic 协议流内 error 事件（200 流内错误）必须显式报错，对齐 OpenAI 侧 P1-3 防线  audit-ok
    //
    // mock 的 error.type 默认是 api_error（Anthropic 标准名 = 上游 5xx 语义）
    // → 属瞬时故障，会先重发本轮一次。故两次都排上错误：重发后仍失败才报错。
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::AnthropicStreamError("overloaded".into()));
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
        msg.contains("大模型 API 错误") && msg.contains("overloaded"),
        "错误应带流内错误详情：{msg}"
    );
    assert!(
        h.events().contains(&"llm.stream_error"),
        "应记 llm.stream_error 审计：{:?}",
        h.events()
    );
    assert_eq!(
        server.request_count(),
        2,
        "瞬时故障重发本轮一次（MAX_STREAM_RETRIES=1），两次都失败才报错"
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
