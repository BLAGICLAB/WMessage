//! Anthropic 兼容模式适配层（协议下拉切换 / max_tokens 默认 8192 /
//! prompt caching 与主体一起做）。
//!
//! 设计：边界适配器——内部消息流全程保持 OpenAI 形状不动（run_model_loop_core 主循环
//! 零改动），只在「发请求前」和「解析响应时」两个边界做协议转换：
//! - 出站：openai_msgs_to_anthropic（消息）+ convert_tools（工具 schema）+
//!   build_anthropic_body（请求体）+ anthropic_messages_url（URL 归一化）+
//!   apply_anthropic_auth（x-api-key + anthropic-version 头）
//! - 入站：parse_anthropic_event（SSE 事件 → 复用 bot_model_loop 的 ParsedChunk，
//!   主循环 handle_line 消费逻辑两协议共享）+ parse_anthropic_response（非流式）
//!
//! 本模块全部纯函数（唯一例外 apply_anthropic_auth 只给 RequestBuilder 加头），
//! 无 IO、无 AppHandle 依赖，模块内单测覆盖全矩阵。

use crate::bot_model_loop::{ParsedChunk, TokenUsage, ToolCallDelta};

/// Anthropic API 版本头（messages API 稳定版本）
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

// 消息转换（出站）

/// OpenAI 消息数组 → Anthropic (system 文本块数组, messages)。
///
/// 规则（ Anthropic /v1/messages 协议约束驱动）：
/// - 所有 role=system 抽出合并为顶层 system 文本块数组（Anthropic 的 system 是顶层字段，
///   不在 messages 里）；非空时给最后一块打 cache_control ephemeral（prompt caching
///   断点 1/2：SYSTEM_PROMPT+技能清单+记忆块是最大静态前缀）
/// - assistant 带 tool_calls → content 数组：文本块在前，每个 tool_call 转
///   {"type":"tool_use","id","name","input"}（arguments 字符串 parse 成 JSON 对象，
///   坏 JSON 兜底 input:{}——Anthropic 要求 input 必须是对象）
/// - role=tool → {"type":"tool_result","tool_use_id","content"} 块，连续 tool 消息
///   合并进一条 user 消息（Anthropic 要求 user/assistant 严格交替，且 tool_result
///   必须出现在 user 消息里）；工具结果按全链路统一口径 audit::tool_call_failed
///   判失败的块加 "is_error": true
/// - 字符串 content → [{"type":"text","text"}]；数组 content（多模态）逐项转换：
///   text 原样；image_url 的 data:{mime};base64,{data} 拆成 Anthropic base64 image 块；
///   非 data URL 的图片跳过并计数（不崩——Anthropic 只接受 base64 内联图片）
/// - 防御（Anthropic 400 红线）：空文本块不产出；整条消息无内容块塞 " " 占位；
///   连续同角色消息合并 content（软警告/失败提示等注入的连续 user 消息）；
///   messages 首条必须 user——assistant 开头时最前补一条占位 user
pub fn openai_msgs_to_anthropic(
    msgs: &[serde_json::Value],
) -> Result<(serde_json::Value, Vec<serde_json::Value>), String> {
    let (system_blocks, messages, _skipped_images) = convert_all(msgs);
    if messages.is_empty() {
        return Err("没有可发送的消息（转换后 messages 为空）".into());
    }
    Ok((serde_json::Value::Array(system_blocks), messages))
}

/// 转换内核：比公开出口多带回「跳过的非 data URL 图片数」（计数供测试断言，
/// 调用方不需要就不透传，保持 plan 签名）。
fn convert_all(
    msgs: &[serde_json::Value],
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>, usize) {
    let mut system_blocks: Vec<serde_json::Value> = Vec::new();
    let mut out: Vec<serde_json::Value> = Vec::new();
    // 待回填的 tool_result 块：连续 role=tool 消息攒在一起，遇到下一条非 tool
    // 消息时合并进一条 user 消息（严格交替约束）
    let mut pending_tools: Vec<serde_json::Value> = Vec::new();
    // 已发出的 tool_use id：无名/无 id 的 tool_call 被跳过后，配对的 tool_result
    // 若照发会成为孤儿引用（上游 400 "tool_use_id not found"），必须一并丢弃
    let mut emitted_tool_use_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut skipped_images = 0usize;

    for m in msgs {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        match role {
            "system" => {
                for t in extract_texts(m.get("content")) {
                    system_blocks.push(serde_json::json!({"type": "text", "text": t}));
                }
            }
            "assistant" => {
                flush_tool_results(&mut out, &mut pending_tools);
                let (mut blocks, skipped) = convert_content_blocks(m.get("content"));
                skipped_images += skipped;
                // thinking 块必须原样回传，且排在 assistant 轮最前
                // （Anthropic 要求 thinking 块在内容数组开头）。
                // 缺了这一步，带 tool_result 的下一轮请求被 400：
                // "'thinking' or 'redacted_thinking' blocks in the latest
                // assistant message cannot be modified"。
                let thinking = thinking_blocks_of(m);
                blocks.splice(0..0, thinking);
                if let Some(tcs) = m.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let id = tc["id"].as_str().unwrap_or("");
                        // 防御：无名/无 id 的 tool_call 转出去必被 400，跳过；
                        // 配对的 tool_result 由 emitted 集合一并跳过，不产生孤儿引用
                        if name.is_empty() || id.is_empty() {
                            continue;
                        }
                        // arguments 是 JSON 字符串 → Anthropic input 必须是对象；
                        // 坏 JSON 兜底空对象（不阻断整轮对话）
                        let args_str = tc["function"]["arguments"].as_str().unwrap_or("");
                        let input = serde_json::from_str::<serde_json::Value>(args_str)
                            .ok()
                            .filter(|v| v.is_object())
                            .unwrap_or_else(|| serde_json::json!({}));
                        emitted_tool_use_ids.insert(id.to_string());
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }));
                    }
                }
                push_or_merge(&mut out, "assistant", non_empty(blocks));
            }
            "tool" => {
                let text = tool_result_text(m.get("content"));
                let tool_use_id = m.get("tool_call_id").and_then(|i| i.as_str()).unwrap_or("");
                // 防御：无 id 或引用的 tool_use 未发出（配对被跳过/历史截断）的
                // tool_result 发出去必被上游 400，直接丢弃不产出该块
                if tool_use_id.is_empty() || !emitted_tool_use_ids.contains(tool_use_id) {
                    continue;
                }
                let mut block = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": if text.trim().is_empty() { " " } else { text.as_str() },
                });
                // 本层拿不到 ToolStatus：转换的是**历史消息**（status 在上游组装
                // msgs 时就折叠成文本了），所以只能走文本分类。这也要求门禁拒绝的
                // 文案必须自带可识别前缀（⚠️ / 用户拒绝 / 已强制终止）——
                // ToolStatus::denied 的调用点都守住了这条。
                if crate::audit::tool_call_failed("", &text) {
                    block["is_error"] = serde_json::json!(true);
                }
                pending_tools.push(block);
            }
            // user 及任何未知角色（防御按 user，与 bot_chat 的 role 白名单同精神）
            _ => {
                flush_tool_results(&mut out, &mut pending_tools);
                let (blocks, skipped) = convert_content_blocks(m.get("content"));
                skipped_images += skipped;
                push_or_merge(&mut out, "user", non_empty(blocks));
            }
        }
    }
    flush_tool_results(&mut out, &mut pending_tools);

    // messages 首条必须 user：assistant 开头（历史被截断到半截等）时最前补占位 user
    if out
        .first()
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        == Some("assistant")
    {
        out.insert(
            0,
            serde_json::json!({"role": "user", "content": [{"type": "text", "text": "（续前对话）"}]}),
        );
    }

    // prompt caching 断点 1/2：system 数组非空时最后一块打 ephemeral 标记
    if let Some(last) = system_blocks.last_mut() {
        last["cache_control"] = serde_json::json!({"type": "ephemeral"});
    }
    (system_blocks, out, skipped_images)
}

/// 从内部消息里取回该轮保存的 Anthropic thinking 块（内部消息用 provider
/// 专属键承载；OpenAI 形状的普通消息没有这个键）。
fn thinking_blocks_of(m: &serde_json::Value) -> Vec<serde_json::Value> {
    m.get(THINKING_KEY)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// 攒着的 tool_result 块冲刷成（或合并进末尾）一条 user 消息
fn flush_tool_results(out: &mut Vec<serde_json::Value>, pending: &mut Vec<serde_json::Value>) {
    if pending.is_empty() {
        return;
    }
    let blocks = std::mem::take(pending);
    push_or_merge(out, "user", blocks);
}

/// 整条消息无内容块 → 塞 " " 占位（Anthropic 400: content 不能为空/空文本块）
fn non_empty(blocks: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    if blocks.is_empty() {
        vec![serde_json::json!({"type": "text", "text": " "})]
    } else {
        blocks
    }
}

/// 追加消息：与末尾同角色则合并 content 数组（严格交替约束——
/// 注入的 soft_warn/失败提示 user 消息可能紧跟 tool_result 的 user 消息）
fn push_or_merge(out: &mut Vec<serde_json::Value>, role: &str, blocks: Vec<serde_json::Value>) {
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some(role) {
            if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                arr.extend(blocks);
                return;
            }
        }
    }
    out.push(serde_json::json!({"role": role, "content": blocks}));
}

/// 提取 content 里的全部非空文本（字符串 content 或数组 content 的 text 项通用）
fn extract_texts(content: Option<&serde_json::Value>) -> Vec<String> {
    let mut texts = Vec::new();
    match content {
        Some(serde_json::Value::String(s)) => {
            if !s.trim().is_empty() {
                texts.push(s.clone());
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            for item in arr {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        if !t.trim().is_empty() {
                            texts.push(t.to_string());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    texts
}

/// user/assistant 普通消息 content → Anthropic 内容块数组。
/// 返回 (块数组, 跳过的非 data URL 图片数)。
fn convert_content_blocks(content: Option<&serde_json::Value>) -> (Vec<serde_json::Value>, usize) {
    let mut skipped = 0usize;
    let mut blocks: Vec<serde_json::Value> = Vec::new();
    match content {
        Some(serde_json::Value::String(s)) => {
            if !s.trim().is_empty() {
                blocks.push(serde_json::json!({"type": "text", "text": s}));
            }
        }
        Some(serde_json::Value::Array(arr)) => {
            for item in arr {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            if !t.trim().is_empty() {
                                blocks.push(serde_json::json!({"type": "text", "text": t}));
                            }
                        }
                    }
                    Some("image_url") => {
                        let url = item["image_url"]["url"].as_str().unwrap_or("");
                        match parse_data_url(url) {
                            Some((mime, data)) => blocks.push(serde_json::json!({
                                "type": "image",
                                "source": {"type": "base64", "media_type": mime, "data": data},
                            })),
                            // Anthropic 只收 base64 内联图片：http(s) URL 图片跳过并计数，不崩
                            None => skipped += 1,
                        }
                    }
                    _ => {} // 未知块类型跳过（防御）
                }
            }
        }
        _ => {} // null / 其他形态 → 无块（调用方塞占位）
    }
    (blocks, skipped)
}

/// 解析 data URL：`data:{mime};base64,{data}` → (mime, data)；非此形态 → None
fn parse_data_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(";base64,")?;
    let mime = meta.trim();
    if mime.is_empty() || data.is_empty() {
        return None;
    }
    Some((mime.to_string(), data.to_string()))
}

/// tool 消息 content → 纯文本（生产回填的恒为字符串；防御数组形态取 text 项拼接）
fn tool_result_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(_)) => extract_texts(content).join("\n"),
        _ => String::new(),
    }
}

// 请求体（出站）

/// OpenAI tools schema → Anthropic tools：
/// {"type":"function","function":{name,description,parameters}} →
/// {"name","description","input_schema":parameters}（parameters 缺失/非对象兜底空 object）
fn convert_tools(tools_openai: &serde_json::Value) -> Vec<serde_json::Value> {
    tools_openai
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = &t["function"];
                    let name = f["name"].as_str()?;
                    let params = &f["parameters"];
                    Some(serde_json::json!({
                        "name": name,
                        "description": f["description"].as_str().unwrap_or(""),
                        "input_schema": if params.is_object() {
                            params.clone()
                        } else {
                            serde_json::json!({"type": "object", "properties": {}})
                        },
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 组装 Anthropic /v1/messages 请求体。
/// stream：主流式循环传 true；Planner/摘要等非流式调用传 false。
/// tools_openai 传空数组（json!([])）时不产出 tools 字段（Anthropic 不接受空 tools）。
pub fn build_anthropic_body(
    model: &str,
    msgs: &[serde_json::Value],
    tools_openai: &serde_json::Value,
    max_tokens: u32,
    stream: bool,
) -> Result<serde_json::Value, String> {
    let (system, mut messages) = openai_msgs_to_anthropic(msgs)?;
    let mut tools = convert_tools(tools_openai);
    // prompt caching 断点 2/2：最后一个工具打 ephemeral 标记
    //（工具 schema 是仅次于 system 的静态前缀；断点 ≤4 个限制内，本项目用 3 个）
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = serde_json::json!({"type": "ephemeral"});
    }
    // prompt caching 断点 3/3：最后一个 user 消息打 ephemeral——把增量对话
    // 也纳入缓存（SWE-agent 同款：断点打在最后几条消息上）。多轮会话每轮
    // 命中前缀、只重付最后一条；缓存写 1.25×、读 0.1×，长会话净赚。
    // content 是字符串 → 包成单 text 块打标；已是块数组（tool_result/图）→
    // 只对末尾的 text/tool_result 块打标，其余结构不动。
    if let Some(last_msg) = messages.last_mut() {
        if last_msg["role"] == "user" {
            match last_msg.get_mut("content") {
                Some(serde_json::Value::String(s)) => {
                    let text = std::mem::take(s);
                    last_msg["content"] = serde_json::json!([{
                        "type": "text",
                        "text": text,
                        "cache_control": {"type": "ephemeral"},
                    }]);
                }
                Some(serde_json::Value::Array(blocks)) => {
                    if let Some(last_block) = blocks.last_mut() {
                        let ty = last_block.get("type").and_then(|t| t.as_str());
                        if matches!(ty, Some("text") | Some("tool_result")) {
                            last_block["cache_control"] = serde_json::json!({"type": "ephemeral"});
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": stream,
    });
    if system.as_array().is_some_and(|a| !a.is_empty()) {
        body["system"] = system;
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    Ok(body)
}

/// 注入 extended thinking 块（RE-1 推理强度）：
/// `thinking: {type: "enabled", budget_tokens: budget}`。
/// budget 已在 reasoning::resolve 阶段夹紧（≥1024 且 < max_tokens），这里只管注入；
/// None 档（关闭）不调用本函数，body 保持无 thinking 字段 = 非思考模式。
pub fn apply_anthropic_thinking(body: &mut serde_json::Value, budget: u32) {
    body["thinking"] = serde_json::json!({
        "type": "enabled",
        "budget_tokens": budget,
    });
}

/// Anthropic messages URL 归一化：去尾斜杠后，已含 /v1 结尾直接拼 /messages，
/// 否则拼 /v1/messages（设置页 baseUrl 两种填法都可用）。
/// 误把端点路径粘进 base_url 的填法（…/v1/messages、…/messages）先剥掉再拼，
/// 否则请求路径会叠成 /v1/messages/v1/messages 打不通。
pub fn anthropic_messages_url(base_url: &str) -> String {
    let b = base_url.trim_end_matches('/');
    let b = b
        .strip_suffix("/v1/messages")
        .or_else(|| b.strip_suffix("/messages"))
        .unwrap_or(b);
    if b.ends_with("/v1") {
        format!("{b}/messages")
    } else {
        format!("{b}/v1/messages")
    }
}

/// Anthropic 鉴权头：x-api-key + anthropic-version（替代 OpenAI 的 Bearer）
pub fn apply_anthropic_auth(
    req: reqwest::RequestBuilder,
    api_key: &str,
) -> reqwest::RequestBuilder {
    req.header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
}

// SSE 解析（入站）

/// 一个 Anthropic SSE 事件的解析结果：直接复用 OpenAI 形状的 ParsedChunk
///（主循环 handle_line 消费逻辑两协议共享）。
/// token 用量挂在 `chunk.usage` 上（message_start 带 input + cache_*，
/// message_delta 带 output），与 OpenAI 末块 usage 走同一条累计路径——
/// 两协议的上报时机不同（前者分散在两个事件、后者集中在末块），
/// 但都是「出现就累加」，故不必在协议分支里各写一份记账。
pub type AnthropicEvent = ParsedChunk;

/// Anthropic stop_reason → OpenAI finish_reason 映射
///（下游 finish_reason 处理——length 截断提示 / content_filter 拦截提示——两协议共享）
pub fn map_stop_reason(sr: &str) -> &'static str {
    match sr {
        "max_tokens" => "length",
        "refusal" => "content_filter",
        "tool_use" => "tool_calls",
        // end_turn / stop_sequence / 未知值一律 stop
        _ => "stop",
    }
}

/// Anthropic content block index → 工具调用槽位（0 起稠密序号）重映射器
///（不做映射时 MiniMax Anthropic 端点会报 400：
/// "tool result's tool id(call_synth_0) not found"）。
///
/// 根因：Anthropic 流里 content_block_start/delta 的 index 是**所有内容块**的序号
///（text/thinking 块也占位），不是工具调用序号。模型先输出一段文本（block 0）再调
/// 工具（block 1）时，ToolCallDelta.index=1 直接进 accumulate_tool_call_delta，
/// vec 补长到 index 1、index 0 留下 ("","","") 幽灵条目——主循环给它合成
/// call_synth_0、当真实 tool_call 执行（未知工具）并回填历史；下一轮请求里
/// tool_result 引用 call_synth_0 但 assistant 消息里没有配对的 tool_use
///（转换层跳过无名 tool_call），严格 API 400。
///
/// 用法：每轮响应流开始时新建一个，该轮所有 Anthropic 事件的 ToolCallDelta
/// 在进累积层之前先 remap（把 content block index 换成首次出现顺序的稠密槽位）。
/// OpenAI 的 tool_calls[*].index 本来就是工具序号空间，不需要此映射。
#[derive(Default)]
pub struct ToolSlotMapper {
    /// content block index → 工具槽位
    map: std::collections::HashMap<usize, usize>,
    next: usize,
}

impl ToolSlotMapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// 原地重映射一个 delta 的 index。同一 block index 的后续 delta
    ///（input_json_delta 续块）映射到同一槽位；未知 block（防御：缺
    /// content_block_start 直接来 delta）按首次出现分配新槽位。
    pub fn remap(&mut self, delta: &mut ToolCallDelta) {
        let slot = *self.map.entry(delta.index).or_insert_with(|| {
            let s = self.next;
            self.next += 1;
            s
        });
        delta.index = slot;
    }
}

/// 解析一行 Anthropic SSE（`data: <json>`，按 JSON type 字段分发）。
/// 返回 None = 非 data 行（含 `event:` 行）/ 空行 / ping / 无需消费的事件 /
/// JSON 解析失败（与 parse_sse_chunk 的 None 语义一致：跳过该行）。
/// Anthropic 的 thinking / redacted_thinking 内容块。
///
/// **必须原样回传**——Anthropic 文档明令：「多轮对话和工具使用对话中，助手
/// 消息（含其 thinking 与 redacted_thinking 块）要原样发回 API，API 会验证
/// 它们未经修改地到达」。丢掉或重建助手消息时，带 tool_result 的下一轮请求
/// 被拒：
/// `400 invalid_request_error: 'thinking' or 'redacted_thinking' blocks in the
/// latest assistant message cannot be modified`。
#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingBlock {
    /// 普通 thinking 块：思考文本 + 签名（签名由 signature_delta 补齐）。
    /// **没有签名的块不能回传**（服务端会判签名无效）。
    Thinking { text: String, signature: String },
    /// 加密的 thinking 块：密文原样回传。
    Redacted { data: String },
}

/// Anthropic thinking 块的流式增量（一个响应里可能有多块，按 content_block
/// 的 index 归位）。
#[derive(Debug, Clone, PartialEq)]
pub enum ThinkingDelta {
    /// content_block_start：thinking（或 redacted_thinking）块开始。
    Start {
        index: usize,
        redacted: bool,
        data: String,
    },
    /// content_block_delta / thinking_delta：思考文本增量。
    Text { index: usize, text: String },
    /// content_block_delta / signature_delta：签名（整块一次给全）。
    Signature { index: usize, signature: String },
}

/// 一个响应里的 thinking 块收集器（按 content_block 的 index 归位）。
#[derive(Debug, Default, Clone)]
pub struct ThinkingCollector {
    blocks: Vec<(usize, ThinkingBlock)>,
}

impl ThinkingCollector {
    /// 吃进一条 thinking 增量。未知 index 的文本/签名增量直接忽略——
    /// 没收到过 Start 说明这不是 thinking 块（正常流不会这样）。
    pub fn push(&mut self, d: &ThinkingDelta) {
        match d {
            ThinkingDelta::Start {
                index,
                redacted,
                data,
            } => {
                let block = if *redacted {
                    ThinkingBlock::Redacted { data: data.clone() }
                } else {
                    ThinkingBlock::Thinking {
                        text: String::new(),
                        signature: String::new(),
                    }
                };
                // 同一 index 重复 Start（脏网关）以最后一次为准
                self.blocks.retain(|(i, _)| i != index);
                self.blocks.push((*index, block));
            }
            ThinkingDelta::Text { index, text } => {
                if let Some((_, ThinkingBlock::Thinking { text: t, .. })) =
                    self.blocks.iter_mut().find(|(i, _)| i == index)
                {
                    t.push_str(text);
                }
            }
            ThinkingDelta::Signature { index, signature } => {
                if let Some((_, ThinkingBlock::Thinking { signature: s, .. })) =
                    self.blocks.iter_mut().find(|(i, _)| i == index)
                {
                    *s = signature.clone();
                }
            }
        }
    }

    /// 回传用的 Anthropic 块数组（按 index 升序）。
    ///
    /// **缺签名的 thinking 块被丢弃**：没有签名回传等于伪造，服务端会判
    /// 签名无效（400）。那种情况宁可少带一个块，也不能带一个坏的。
    pub fn blocks(&self) -> Vec<serde_json::Value> {
        let mut sorted: Vec<&(usize, ThinkingBlock)> = self.blocks.iter().collect();
        sorted.sort_by_key(|(i, _)| *i);
        sorted
            .into_iter()
            .filter_map(|(_, b)| match b {
                ThinkingBlock::Thinking { text, signature } if !signature.is_empty() => {
                    Some(serde_json::json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature,
                    }))
                }
                ThinkingBlock::Thinking { .. } => None,
                ThinkingBlock::Redacted { data } => Some(serde_json::json!({
                    "type": "redacted_thinking",
                    "data": data,
                })),
            })
            .collect()
    }
}

/// 内部消息里承载 Anthropic thinking 块的键（provider 专属）。
///
/// 内部消息流是 OpenAI 形状、同一份 msgs 会直接发给 OpenAI 端点，故这个键
/// 只在 Anthropic 分支挂载，发 OpenAI 前会被剥掉（见 build_request_body）。
pub const THINKING_KEY: &str = "anthropic_thinking";

/// Anthropic SSE 事件解析入口。
pub fn parse_anthropic_event(line: &str) -> Option<AnthropicEvent> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let ty = v.get("type").and_then(|t| t.as_str())?;
    let mut ev = ParsedChunk::default();
    match ty {
        "message_start" => {
            // 顺带捞 usage（input_tokens / cache_*_tokens 只在 message_start 里）
            let u = &v["message"]["usage"];
            let input = u["input_tokens"].as_u64().unwrap_or(0);
            let cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let cache_creation = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            // output_tokens 此时通常为 0；保持现有「非零才报」以免空 usage 噪音
            let output = u["output_tokens"].as_u64().unwrap_or(0);
            if input > 0 || output > 0 || cache_read > 0 || cache_creation > 0 {
                ev.usage = Some(TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_read_tokens: cache_read,
                    cache_creation_tokens: cache_creation,
                });
            }
            Some(ev)
        }
        "content_block_start" => {
            let cb = &v["content_block"];
            match cb["type"].as_str() {
                // tool_use 块开始：给出 id + name（arguments 等后续 input_json_delta）
                Some("tool_use") => {
                    ev.tool_calls.push(ToolCallDelta {
                        index: v["index"].as_u64().unwrap_or(0) as usize,
                        id: cb["id"].as_str().map(String::from),
                        name_chunk: cb["name"].as_str().map(String::from),
                        arguments_chunk: None,
                    });
                    Some(ev)
                }
                // text/thinking 块开始无内容，等 delta
                Some("thinking") => {
                    ev.thinking = Some(ThinkingDelta::Start {
                        index: v["index"].as_u64().unwrap_or(0) as usize,
                        redacted: false,
                        data: String::new(),
                    });
                    Some(ev)
                }
                // 加密的 thinking 块：密文在 start 里给全，后续无 delta
                Some("redacted_thinking") => {
                    ev.thinking = Some(ThinkingDelta::Start {
                        index: v["index"].as_u64().unwrap_or(0) as usize,
                        redacted: true,
                        data: cb["data"].as_str().unwrap_or("").to_string(),
                    });
                    Some(ev)
                }
                _ => None,
            }
        }
        "content_block_delta" => {
            let delta = &v["delta"];
            let index = v["index"].as_u64().unwrap_or(0) as usize;
            match delta["type"].as_str() {
                Some("text_delta") => {
                    if let Some(t) = delta["text"].as_str() {
                        if !t.is_empty() {
                            ev.content = Some(t.to_string());
                        }
                    }
                    Some(ev)
                }
                Some("input_json_delta") => {
                    if let Some(p) = delta["partial_json"].as_str() {
                        if !p.is_empty() {
                            ev.tool_calls.push(ToolCallDelta {
                                index,
                                arguments_chunk: Some(p.to_string()),
                                ..Default::default()
                            });
                        }
                    }
                    Some(ev)
                }
                // thinking_delta 喂现有 bot-think-delta 出口（不需启用 thinking 也兼容）
                // 同时把增量喂给 thinking 收集器——那块要原样回传（见 ThinkingBlock）
                Some("thinking_delta") => {
                    if let Some(t) = delta["thinking"].as_str() {
                        if !t.is_empty() {
                            ev.reasoning = Some(t.to_string());
                            ev.thinking = Some(ThinkingDelta::Text {
                                index,
                                text: t.to_string(),
                            });
                        }
                    }
                    Some(ev)
                }
                // signature_delta：thinking 块的签名（整块一次给全）。
                // 缺它那块就回传不了（服务端判签名无效），故必须收。
                Some("signature_delta") => {
                    if let Some(s) = delta["signature"].as_str() {
                        if !s.is_empty() {
                            ev.thinking = Some(ThinkingDelta::Signature {
                                index,
                                signature: s.to_string(),
                            });
                        }
                    }
                    Some(ev)
                }
                _ => None,
            }
        }
        "message_delta" => {
            if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                ev.finish_reason = Some(map_stop_reason(sr).to_string());
            }
            // output_tokens 在 message_delta 里；cache_*_tokens 某些版本也在
            // 此处补登一次（Anthropic 当前仅 message_start 带 cache 字段，保留
            // 兜底便于未来扩展）
            let u = &v["usage"];
            let o = u["output_tokens"].as_u64().unwrap_or(0);
            let cr = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let cc = u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            if o > 0 || cr > 0 || cc > 0 {
                ev.usage = Some(TokenUsage {
                    input_tokens: 0,
                    output_tokens: o,
                    cache_read_tokens: cr,
                    cache_creation_tokens: cc,
                });
            }
            Some(ev)
        }
        "message_stop" => {
            ev.is_done = true;
            Some(ev)
        }
        // 流内 error 事件（200 流内错误，对齐 OpenAI 侧防线）
        "error" => {
            let msg = v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .unwrap_or("未知错误");
            ev.error = Some(msg.chars().take(200).collect());
            Some(ev)
        }
        // ping / content_block_stop / 未知 type → 跳过
        _ => None,
    }
}

/// 非流式响应解析（Planner/摘要）：content 数组里所有 text 块拼接
pub fn parse_anthropic_response(body: &serde_json::Value) -> String {
    body.get("content")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

// 测试：消息转换全矩阵 / 请求体 / URL 归一化 / SSE 解析 / 非流式响应

#[cfg(test)]
mod tests;
