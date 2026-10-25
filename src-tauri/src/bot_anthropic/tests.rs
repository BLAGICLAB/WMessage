use super::*;
use serde_json::json;

/// 工具附图序列 [assistant(tool_use), tool(文本), user(text+image_url)]
/// → Anthropic 侧必须合并成单条 user 消息 [tool_result, text, image]
///（官方 tool_result 附图形态 + 严格交替约束）。
#[test]
fn tool_result_and_image_user_merge_into_one_user() {
    let msgs = vec![
        json!({"role":"assistant","content":"截屏中","tool_calls":[
            {"id":"c1","type":"function","function":{"name":"screenshot","arguments":"{}"}}
        ]}),
        json!({"role":"tool","tool_call_id":"c1","content":"已截屏：/tmp/x.png"}),
        json!({"role":"user","content":[
            {"type":"text","text":"〔系统附图〕以上工具返回了截图，请直接用视觉能力读取图片内容。"},
            {"type":"image_url","image_url":{"url":"data:image/png;base64,aGVsbG8"}}
        ]}),
    ];
    let (_, out, skipped) = convert_all(&msgs);
    assert_eq!(skipped, 0, "data URL 图片应转换而非跳过");
    // out[0] = 首条 assistant 前自动补的 user 占位（「续前对话」）
    assert_eq!(
        out.len(),
        3,
        "占位 user + assistant(tool_use) + 合并后的单条 user：{out:?}"
    );
    assert_eq!(out[2]["role"], "user");
    let blocks = out[2]["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(blocks[0]["tool_use_id"], "c1");
    assert!(
        blocks.iter().any(|b| b["type"] == "image"),
        "tool_result 与 image 应在同一条 user 消息里：{blocks:?}"
    );
    assert!(blocks.iter().any(|b| b["type"] == "text"));
}

/// Anthropic 的 thinking 块必须能原样收集回来（文本 + 签名）。
///
/// 块丢了，下一轮带 tool_result 的请求被 400：
/// "'thinking' or 'redacted_thinking' blocks in the latest assistant message
/// cannot be modified"。
#[test]
fn thinking_block_collected_with_signature() {
    let mut c = ThinkingCollector::default();
    for line in [
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"让我想想"}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"SIG123"}}"#,
    ] {
        let ev = parse(line).expect("thinking 相关事件应解析");
        if let Some(d) = ev.thinking {
            c.push(&d);
        }
    }
    let blocks = c.blocks();
    assert_eq!(blocks.len(), 1, "应收集到一个 thinking 块");
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "让我想想");
    assert_eq!(blocks[0]["signature"], "SIG123");
}

/// 加密的 thinking 块（redacted_thinking）：密文在 start 里给全，原样回传。
#[test]
fn redacted_thinking_block_collected() {
    let mut c = ThinkingCollector::default();
    let ev = parse(
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"redacted_thinking","data":"ENC=="}}"#,
    )
    .expect("redacted_thinking 应解析");
    if let Some(d) = ev.thinking {
        c.push(&d);
    }
    let blocks = c.blocks();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "redacted_thinking");
    assert_eq!(blocks[0]["data"], "ENC==");
}

/// 缺签名的 thinking 块不能回传（服务端判签名无效）——宁可少带一个块。
#[test]
fn thinking_block_without_signature_is_not_replayed() {
    let mut c = ThinkingCollector::default();
    for line in [
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"想"}}"#,
    ] {
        let ev = parse(line).expect("应解析");
        if let Some(d) = ev.thinking {
            c.push(&d);
        }
    }
    assert!(
        c.blocks().is_empty(),
        "没有签名的块回传过去会被判签名无效，不能带"
    );
}

/// assistant 轮必须把 thinking 块原样回传，且排在内容数组最前。
///
/// Anthropic 要求 thinking 块在 assistant 轮开头；缺了它，带 tool_result
/// 的下一轮请求被 400（"cannot be modified"）。
#[test]
fn assistant_turn_replays_thinking_blocks_first() {
    let msgs = vec![
        user("查一下"),
        json!({
            "role": "assistant", "content": "我查一下",
            "anthropic_thinking": [
                {"type":"thinking","thinking":"先想","signature":"SIG"}
            ],
            "tool_calls": [{"id":"c1","type":"function","function":{"name":"query_tasks","arguments":"{}"}}]
        }),
        json!({"role":"tool","tool_call_id":"c1","content":"ok"}),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    let asst = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("应有 assistant 轮");
    let blocks = asst["content"].as_array().unwrap();
    assert_eq!(
        blocks[0]["type"], "thinking",
        "thinking 块必须排在 assistant 轮最前：{blocks:?}"
    );
    assert_eq!(blocks[0]["thinking"], "先想");
    assert_eq!(blocks[0]["signature"], "SIG");
    assert!(
        blocks.iter().any(|b| b["type"] == "tool_use"),
        "tool_use 块仍应在：{blocks:?}"
    );
}

/// 没有 thinking 块的 assistant 轮不受影响（普通 Anthropic 模型路径）。
#[test]
fn assistant_turn_without_thinking_is_unchanged() {
    let msgs = vec![user("x"), asst_text("答")];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    let asst = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("应有 assistant 轮");
    let blocks = asst["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["type"], "text");
}

#[test]
fn orphan_tool_result_of_skipped_tool_call_dropped() {
    // 无名 tool_call 被跳过后，配对的 tool_result 若照发会成为孤儿引用
    //（上游 400 "tool_use_id not found"）——必须一并丢弃
    let msgs = vec![
        user("x"),
        json!({
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "c9", "type": "function",
                "function": {"name": "", "arguments": "{}"}}]
        }),
        json!({"role": "tool", "tool_call_id": "c9", "content": "结果"}),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(
        messages.len(),
        2,
        "孤儿 tool_result 不产出消息：{messages:?}"
    );
    assert_eq!(messages[1]["role"], "assistant");
    assert!(!messages[1]["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["type"] == "tool_use"));
}

#[test]
fn tool_result_without_call_id_dropped() {
    // 缺 tool_call_id 的 tool 消息：发空 tool_use_id 必被上游 400，直接丢弃
    let msgs = vec![user("x"), json!({"role": "tool", "content": "结果"})];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(
        messages.len(),
        1,
        "无 id 的 tool_result 不产出：{messages:?}"
    );
    assert_eq!(messages[0]["role"], "user");
}

fn sys(text: &str) -> serde_json::Value {
    json!({"role": "system", "content": text})
}
fn user(text: &str) -> serde_json::Value {
    json!({"role": "user", "content": text})
}
fn asst_text(text: &str) -> serde_json::Value {
    json!({"role": "assistant", "content": text})
}

#[test]
fn multi_system_merged_with_cache_control_on_last() {
    let msgs = vec![
        sys("主提示词"),
        sys("[早前对话摘要] x"),
        sys("记忆块"),
        user("u"),
    ];
    let (system, _) = openai_msgs_to_anthropic(&msgs).unwrap();
    let blocks = system.as_array().unwrap();
    assert_eq!(blocks.len(), 3, "多条 system 应全部保留为块");
    assert_eq!(blocks[0]["text"], "主提示词");
    assert_eq!(blocks[2]["text"], "记忆块");
    assert!(
        blocks[0].get("cache_control").is_none() && blocks[1].get("cache_control").is_none(),
        "cache_control 只打最后一块"
    );
    assert_eq!(blocks[2]["cache_control"], json!({"type": "ephemeral"}));
}

#[test]
fn all_system_no_messages_errors() {
    let msgs = vec![sys("只有系统")];
    assert!(
        openai_msgs_to_anthropic(&msgs).is_err(),
        "无 messages 应报错"
    );
}

#[test]
fn system_and_user_basic_flow() {
    let msgs = vec![sys("s1"), sys("s2"), user("你好")];
    let (system, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(system.as_array().unwrap().len(), 2);
    assert_eq!(system[1]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(
        messages[0]["content"][0],
        json!({"type": "text", "text": "你好"})
    );
}

#[test]
fn assistant_tool_calls_to_tool_use() {
    let msgs = vec![
        user("查任务"),
        json!({
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "list_tasks", "arguments": "{}"}}]
        }),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    let blocks = messages[1]["content"].as_array().unwrap();
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(blocks.len(), 1, "无文本只有 tool_use 块");
    assert_eq!(
        blocks[0],
        json!({"type": "tool_use", "id": "call_1", "name": "list_tasks", "input": {}})
    );
}

#[test]
fn assistant_text_then_tool_use_order() {
    let msgs = vec![
        user("x"),
        json!({
            "role": "assistant", "content": "我先查一下",
            "tool_calls": [{"id": "c1", "type": "function",
                "function": {"name": "web_search", "arguments": "{\"query\":\"rust\"}"}}]
        }),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    let blocks = messages[1]["content"].as_array().unwrap();
    assert_eq!(
        blocks[0],
        json!({"type": "text", "text": "我先查一下"}),
        "文本块在前"
    );
    assert_eq!(blocks[1]["type"], "tool_use");
    assert_eq!(blocks[1]["input"], json!({"query": "rust"}));
}

#[test]
fn tool_use_bad_arguments_fallback_empty_object() {
    let msgs = vec![
        user("x"),
        json!({
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "c1", "type": "function",
                "function": {"name": "list_tasks", "arguments": "{坏JSON"}}]
        }),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(
        messages[1]["content"][0]["input"],
        json!({}),
        "坏 JSON arguments 兜底 input:{{}}"
    );
}

#[test]
fn consecutive_tool_results_merge_into_one_user_msg() {
    let msgs = vec![
        user("并行调两个"),
        json!({
            "role": "assistant", "content": null,
            "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "list_tasks", "arguments": "{}"}},
                {"id": "c2", "type": "function", "function": {"name": "get_current_time", "arguments": "{}"}}
            ]
        }),
        json!({"role": "tool", "tool_call_id": "c1", "content": "共 2 个任务"}),
        json!({"role": "tool", "tool_call_id": "c2", "content": "失败：超时"}),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(messages.len(), 3, "user / assistant / user 严格交替");
    assert_eq!(messages[2]["role"], "user", "tool 结果合并进 user 消息");
    let blocks = messages[2]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 2, "连续两条 tool 合并为一条 user 的两个块");
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(blocks[0]["tool_use_id"], "c1");
    assert!(blocks[0].get("is_error").is_none(), "成功结果不打 is_error");
    assert_eq!(blocks[1]["tool_use_id"], "c2");
    assert_eq!(blocks[1]["is_error"], json!(true), "失败结果打 is_error");
}

#[test]
fn tool_results_then_injected_user_msg_merged_and_alternating() {
    // 生产序列：assistant(tool_calls) → tool → tool → user(soft_warn 注入)
    // 期望：tool 结果与注入 user 合并成一条 user 消息（不破坏交替）
    let msgs = vec![
        user("x"),
        json!({
            "role": "assistant", "content": null,
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "list_tasks", "arguments": "{}"}}]
        }),
        json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
        user("【系统提示】请收尾"),
    ];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(messages.len(), 3);
    let blocks = messages[2]["content"].as_array().unwrap();
    assert_eq!(blocks[0]["type"], "tool_result");
    assert_eq!(
        blocks[1],
        json!({"type": "text", "text": "【系统提示】请收尾"})
    );
    // 交替校验
    let roles: Vec<&str> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user"]);
}

#[test]
fn multimodal_image_data_url_split() {
    let msgs = vec![json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "看这张图"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,QUJD"}}
        ]
    })];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    let blocks = messages[0]["content"].as_array().unwrap();
    assert_eq!(blocks[0], json!({"type": "text", "text": "看这张图"}));
    assert_eq!(
        blocks[1],
        json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}})
    );
}

#[test]
fn non_data_url_image_skipped_not_crash() {
    let msgs = vec![json!({
        "role": "user",
        "content": [
            {"type": "text", "text": "看图"},
            {"type": "image_url", "image_url": {"url": "https://example.com/a.png"}}
        ]
    })];
    let (system, messages, skipped) = convert_all(&msgs);
    let _ = system;
    assert_eq!(skipped, 1, "非 data URL 图片跳过并计数");
    let blocks = messages[0]["content"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "跳过的图片不产出块");
    assert_eq!(blocks[0]["type"], "text");
}

#[test]
fn empty_content_gets_placeholder() {
    let msgs = vec![user(""), user("   "), asst_text("")];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    // 前两条空 user 合并成一条（同角色合并），内容为占位
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0]["content"][0],
        json!({"type": "text", "text": " "})
    );
    assert_eq!(
        messages[1]["content"][0],
        json!({"type": "text", "text": " "})
    );
}

#[test]
fn assistant_first_prepends_user_placeholder() {
    let msgs = vec![asst_text("半截历史"), user("继续")];
    let (_, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(messages[0]["role"], "user", "assistant 开头应补 user");
    assert_eq!(messages[0]["content"][0]["text"], "（续前对话）");
    assert_eq!(messages[1]["role"], "assistant");
}

/// 只含 system 的请求发不出去——子任务汇报轮踩过的正是这一条。
///
/// 汇报轮由后端发起、没有用户消息，早期实现直接传空历史：请求里只剩系统提示词、
/// 轮次说明和子任务回报三段 system，全被抽进 system 之后 messages 归零，
/// 转换阶段硬失败。症状在用户端是「子任务跑完了，却没人告诉我结果」。
#[test]
fn system_only_input_is_rejected() {
    let msgs = vec![
        sys("主提示词"),
        sys("汇报轮说明：没有新的用户消息"),
        sys("子任务回报：已完成"),
    ];
    assert!(
        openai_msgs_to_anthropic(&msgs).is_err(),
        "纯 system 的请求转出来是空消息列表，必须拒绝"
    );
}

/// 汇报轮补上历史（或兜底那条 user 消息）后，同样的 system 段就能发出去了。
#[test]
fn report_turn_sends_once_it_carries_a_user_turn() {
    let msgs = vec![
        sys("主提示词"),
        sys("汇报轮说明：没有新的用户消息"),
        user("派子 agent 查一下东海水产研究所的最新进展"),
        asst_text("已派发"),
        sys("汇报轮说明：没有新的用户消息"),
        sys("子任务回报：已完成"),
    ];
    let (system, messages) = openai_msgs_to_anthropic(&msgs).unwrap();
    assert_eq!(messages.len(), 2, "历史两条都该发出去");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    // 四段 system 全进 system 段，不占 messages
    assert_eq!(system.as_array().unwrap().len(), 4);
}

// ── ToolSlotMapper（真实环境 400 回归）──

/// 模拟一轮 Anthropic 事件流经 parse + remap + 生产同一累积函数后的工具列表
fn run_anthropic_stream(lines: &[&str]) -> Vec<(String, String, String)> {
    let mut mapper = ToolSlotMapper::new();
    let mut calls: Vec<(String, String, String)> = Vec::new();
    for line in lines {
        let Some(ev) = parse_anthropic_event(line) else {
            continue;
        };
        for mut d in ev.tool_calls {
            mapper.remap(&mut d);
            assert!(crate::bot_model_loop::accumulate_tool_call_delta(
                &mut calls, &d
            ));
        }
    }
    calls
}

#[test]
fn slot_mapper_text_block_first_then_tool_use_no_ghost() {
    // 回归根因：text 块占 index 0，tool_use 块 index 1——重映射后工具列表
    // 必须只有 1 条且 id 是服务端真实签发的（无 ("","","") 幽灵占位）
    let calls = run_anthropic_stream(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"我先读文件"}}"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_real_1","name":"extract_document"}}"#,
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"a.docx\"}"}}"#,
        r#"data: {"type":"content_block_stop","index":1}"#,
    ]);
    assert_eq!(calls.len(), 1, "不应有幽灵占位条目：{calls:?}");
    assert_eq!(calls[0].0, "toolu_real_1", "id 必须是服务端签发的真实 id");
    assert_eq!(calls[0].1, "extract_document");
    assert_eq!(calls[0].2, "{\"path\":\"a.docx\"}");
}

#[test]
fn slot_mapper_pure_tool_no_text() {
    // 纯工具无文本：tool_use 在 index 0，重映射应恒等
    let calls = run_anthropic_stream(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"list_tasks"}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
    ]);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "toolu_a");
    assert_eq!(calls[0].1, "list_tasks");
}

#[test]
fn slot_mapper_parallel_tool_uses_dense_slots() {
    // 文本块 index 0 + 两个并行 tool_use（index 1、2）→ 稠密槽位 0、1
    let calls = run_anthropic_stream(&[
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"data: {"type":"content_block_stop","index":0}"#,
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_a","name":"list_tasks"}}"#,
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
        r#"data: {"type":"content_block_stop","index":1}"#,
        r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_b","name":"get_current_time"}}"#,
        r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
        r#"data: {"type":"content_block_stop","index":2}"#,
    ]);
    assert_eq!(calls.len(), 2, "两个并行工具各一条，无幽灵：{calls:?}");
    assert_eq!(calls[0].0, "toolu_a");
    assert_eq!(calls[1].0, "toolu_b");
    assert_eq!(calls[1].1, "get_current_time");
}

#[test]
fn build_body_tools_converted_and_cached() {
    let tools = json!([
        {"type": "function", "function": {"name": "a", "description": "da", "parameters": {"type": "object", "properties": {}}}},
        {"type": "function", "function": {"name": "b", "description": "db", "parameters": {"type": "object"}}}
    ]);
    let body =
        build_anthropic_body("claude-x", &[sys("s"), user("u")], &tools, 8192, true).unwrap();
    assert_eq!(body["model"], "claude-x");
    assert_eq!(body["max_tokens"], 8192);
    assert_eq!(body["stream"], true);
    let ts = body["tools"].as_array().unwrap();
    assert_eq!(ts[0]["name"], "a");
    assert_eq!(
        ts[0]["input_schema"],
        json!({"type": "object", "properties": {}})
    );
    assert!(
        ts[0].get("function").is_none(),
        "不应残留 OpenAI function 形态"
    );
    assert!(
        ts[0].get("cache_control").is_none(),
        "cache_control 只打最后一个工具"
    );
    assert_eq!(ts[1]["cache_control"], json!({"type": "ephemeral"}));
    assert!(body.get("system").is_some());
}

#[test]
fn build_body_no_tools_no_system_omits_fields() {
    let body = build_anthropic_body("m", &[user("u")], &json!([]), 4096, false).unwrap();
    assert!(body.get("tools").is_none(), "空 tools 不产出 tools 字段");
    assert!(body.get("system").is_none(), "空 system 不产出 system 字段");
    assert_eq!(body["stream"], false, "非流式变体 stream:false");
}

#[test]
fn url_normalization_four_forms() {
    assert_eq!(
        anthropic_messages_url("https://api.anthropic.com"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        anthropic_messages_url("https://api.anthropic.com/"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        anthropic_messages_url("https://api.anthropic.com/v1"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        anthropic_messages_url("https://api.anthropic.com/v1/"),
        "https://api.anthropic.com/v1/messages"
    );
    // 误把端点路径粘进 base_url：剥掉再拼，不得叠成 /v1/messages/v1/messages
    assert_eq!(
        anthropic_messages_url("https://api.anthropic.com/v1/messages"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        anthropic_messages_url("https://api.anthropic.com/v1/messages/"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        anthropic_messages_url("https://api.minimaxi.com/anthropic/messages"),
        "https://api.minimaxi.com/anthropic/v1/messages"
    );
}

// ── SSE 解析 ──

fn parse(line: &str) -> Option<AnthropicEvent> {
    parse_anthropic_event(line)
}

#[test]
fn sse_text_flow() {
    let ev = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#)
        .expect("text_delta 应解析");
    assert_eq!(ev.content.as_deref(), Some("你好"));
    assert!(ev.usage.is_none());
    // message_start 带 usage
    let ev = parse(r#"data: {"type":"message_start","message":{"id":"m1","usage":{"input_tokens":12,"output_tokens":1}}}"#)
        .expect("message_start 应解析（捞 usage）");
    assert_eq!(
        ev.usage,
        Some(TokenUsage {
            input_tokens: 12,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        })
    );
    // message_start 带 cache_*_tokens：解析四项而非仅 input/output
    let ev = parse(r#"data: {"type":"message_start","message":{"id":"m2","usage":{"input_tokens":50,"output_tokens":5,"cache_read_input_tokens":4000,"cache_creation_input_tokens":1500}}}"#)
        .expect("带缓存的 message_start 应解析");
    assert_eq!(
        ev.usage,
        Some(TokenUsage {
            input_tokens: 50,
            output_tokens: 5,
            cache_read_tokens: 4000,
            cache_creation_tokens: 1500,
        }),
        "cache 字段必须被捕获，否则命中率不可算"
    );
    assert!(ev.content.is_none());
    // message_delta 带 stop_reason + usage
    let ev = parse(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#)
        .expect("message_delta 应解析");
    assert_eq!(ev.finish_reason.as_deref(), Some("stop"));
    assert_eq!(
        ev.usage,
        Some(TokenUsage {
            input_tokens: 0,
            output_tokens: 7,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        })
    );
    // message_stop
    let ev = parse(r#"data: {"type":"message_stop"}"#).expect("message_stop 应解析");
    assert!(ev.is_done);
}

#[test]
fn sse_tool_use_partial_json_across_blocks() {
    let ev = parse(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"create_task"}}"#)
        .expect("tool_use 开始");
    assert_eq!(ev.tool_calls.len(), 1);
    assert_eq!(ev.tool_calls[0].id.as_deref(), Some("toolu_1"));
    assert_eq!(ev.tool_calls[0].name_chunk.as_deref(), Some("create_task"));

    let d1 = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"title\":"}}"#)
        .expect("partial_json 1");
    let d2 = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"买牛奶\"}"}}"#)
        .expect("partial_json 2");
    assert_eq!(
        d1.tool_calls[0].arguments_chunk.as_deref(),
        Some("{\"title\":")
    );
    assert_eq!(
        d2.tool_calls[0].arguments_chunk.as_deref(),
        Some("\"买牛奶\"}")
    );
}

#[test]
fn sse_parallel_tool_uses_keep_their_index() {
    let a = parse(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"list_tasks"}}"#).unwrap();
    let b = parse(r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_b","name":"get_current_time"}}"#).unwrap();
    assert_eq!(a.tool_calls[0].index, 0);
    assert_eq!(b.tool_calls[0].index, 1);
    let d = parse(r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#).unwrap();
    assert_eq!(
        d.tool_calls[0].index, 1,
        "delta 应按 content_block index 归位"
    );
}

#[test]
fn sse_stop_reason_mapping_four_states() {
    let cases = [
        ("end_turn", "stop"),
        ("tool_use", "tool_calls"),
        ("max_tokens", "length"),
        ("refusal", "content_filter"),
        ("stop_sequence", "stop"),
    ];
    for (sr, expect) in cases {
        let line = format!(r#"data: {{"type":"message_delta","delta":{{"stop_reason":"{sr}"}}}}"#);
        let ev = parse(&line).expect("message_delta 应解析");
        assert_eq!(
            ev.finish_reason.as_deref(),
            Some(expect),
            "stop_reason={sr}"
        );
    }
}

#[test]
fn sse_error_event_surfaced() {
    let ev = parse(
        r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
    )
    .expect("error 事件应解析");
    assert_eq!(ev.error.as_deref(), Some("Overloaded"));
}

#[test]
fn sse_ignores_event_lines_ping_and_garbage() {
    assert!(parse("event: message_start").is_none(), "event: 行忽略");
    assert!(parse(r#"data: {"type":"ping"}"#).is_none(), "ping 忽略");
    assert!(parse("").is_none());
    assert!(parse("data: ").is_none());
    assert!(parse("data: {坏JSON").is_none(), "坏 JSON 忽略不崩");
    assert!(
        parse(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#).is_none(),
        "text 块开始等 delta，本身忽略"
    );
    assert!(
        parse(r#"data: {"type":"content_block_stop","index":0}"#).is_none(),
        "content_block_stop 忽略"
    );
}

#[test]
fn sse_thinking_delta_to_reasoning() {
    let ev = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"让我想想"}}"#)
        .expect("thinking_delta 应解析");
    assert_eq!(ev.reasoning.as_deref(), Some("让我想想"));
    assert!(ev.content.is_none(), "thinking 不进正文");
}

#[test]
fn non_stream_response_text_blocks_joined() {
    let body = json!({
        "id": "msg_1", "type": "message", "role": "assistant",
        "content": [
            {"type": "text", "text": "第一段"},
            {"type": "thinking", "thinking": "忽略"},
            {"type": "text", "text": "第二段"}
        ],
        "stop_reason": "end_turn"
    });
    assert_eq!(parse_anthropic_response(&body), "第一段第二段");
    assert_eq!(
        parse_anthropic_response(&json!({})),
        "",
        "缺 content 兜底空串"
    );
}
