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

use crate::bot_model_loop::{ParsedChunk, ToolCallDelta};

/// Anthropic API 版本头（messages API 稳定版本）
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

// ───────────────────────── 消息转换（出站） ─────────────────────────

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
                if let Some(tcs) = m.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        if name.is_empty() {
                            continue; // 防御：无名 tool_call 转出去必被 400，跳过
                        }
                        // arguments 是 JSON 字符串 → Anthropic input 必须是对象；
                        // 坏 JSON 兜底空对象（不阻断整轮对话）
                        let args_str = tc["function"]["arguments"].as_str().unwrap_or("");
                        let input = serde_json::from_str::<serde_json::Value>(args_str)
                            .ok()
                            .filter(|v| v.is_object())
                            .unwrap_or_else(|| serde_json::json!({}));
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc["id"].as_str().unwrap_or(""),
                            "name": name,
                            "input": input,
                        }));
                    }
                }
                push_or_merge(&mut out, "assistant", non_empty(blocks));
            }
            "tool" => {
                let text = tool_result_text(m.get("content"));
                let mut block = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": m.get("tool_call_id").and_then(|i| i.as_str()).unwrap_or(""),
                    "content": if text.trim().is_empty() { " " } else { text.as_str() },
                });
                // 失败口径复用全链路统一的 audit::tool_call_failed（同一判定）
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

// ───────────────────────── 请求体（出站） ─────────────────────────

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
    let (system, messages) = openai_msgs_to_anthropic(msgs)?;
    let mut tools = convert_tools(tools_openai);
    // prompt caching 断点 2/2：最后一个工具打 ephemeral 标记
    //（工具 schema 是仅次于 system 的静态前缀；断点 ≤4 个限制内，本项目用 2 个）
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = serde_json::json!({"type": "ephemeral"});
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

/// Anthropic messages URL 归一化：去尾斜杠后，已含 /v1 结尾直接拼 /messages，
/// 否则拼 /v1/messages（设置页 baseUrl 两种填法都可用）。
pub fn anthropic_messages_url(base_url: &str) -> String {
    let b = base_url.trim_end_matches('/');
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

// ───────────────────────── SSE 解析（入站） ─────────────────────────

/// 流内捞到的 token 用量（message_start 的 input_tokens / message_delta 的 output_tokens）
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AnthropicUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// 一个 Anthropic SSE 事件的解析结果：复用 OpenAI 形状的 ParsedChunk
///（主循环 handle_line 消费逻辑两协议共享），usage 单独带出
pub struct AnthropicEvent {
    pub chunk: ParsedChunk,
    pub usage: Option<AnthropicUsage>,
}

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
pub fn parse_anthropic_event(line: &str) -> Option<AnthropicEvent> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let ty = v.get("type").and_then(|t| t.as_str())?;
    let mut ev = AnthropicEvent {
        chunk: ParsedChunk::default(),
        usage: None,
    };
    match ty {
        "message_start" => {
            // 顺带捞 usage（input_tokens 只在 message_start 里）
            let u = &v["message"]["usage"];
            let (i, o) = (
                u["input_tokens"].as_u64().unwrap_or(0),
                u["output_tokens"].as_u64().unwrap_or(0),
            );
            if i > 0 || o > 0 {
                ev.usage = Some(AnthropicUsage {
                    input_tokens: i,
                    output_tokens: o,
                });
            }
            Some(ev)
        }
        "content_block_start" => {
            let cb = &v["content_block"];
            match cb["type"].as_str() {
                // tool_use 块开始：给出 id + name（arguments 等后续 input_json_delta）
                Some("tool_use") => {
                    ev.chunk.tool_calls.push(ToolCallDelta {
                        index: v["index"].as_u64().unwrap_or(0) as usize,
                        id: cb["id"].as_str().map(String::from),
                        name_chunk: cb["name"].as_str().map(String::from),
                        arguments_chunk: None,
                    });
                    Some(ev)
                }
                // text/thinking 块开始无内容，等 delta
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
                            ev.chunk.content = Some(t.to_string());
                        }
                    }
                    Some(ev)
                }
                Some("input_json_delta") => {
                    if let Some(p) = delta["partial_json"].as_str() {
                        if !p.is_empty() {
                            ev.chunk.tool_calls.push(ToolCallDelta {
                                index,
                                arguments_chunk: Some(p.to_string()),
                                ..Default::default()
                            });
                        }
                    }
                    Some(ev)
                }
                // thinking_delta 喂现有 bot-think-delta 出口（不需启用 thinking 也兼容）
                Some("thinking_delta") => {
                    if let Some(t) = delta["thinking"].as_str() {
                        if !t.is_empty() {
                            ev.chunk.reasoning = Some(t.to_string());
                        }
                    }
                    Some(ev)
                }
                _ => None, // signature_delta 等
            }
        }
        "message_delta" => {
            if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                ev.chunk.finish_reason = Some(map_stop_reason(sr).to_string());
            }
            // usage（output_tokens 在 message_delta 里）
            let o = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
            if o > 0 {
                ev.usage = Some(AnthropicUsage {
                    input_tokens: 0,
                    output_tokens: o,
                });
            }
            Some(ev)
        }
        "message_stop" => {
            ev.chunk.is_done = true;
            Some(ev)
        }
        // 流内 error 事件（200 流内错误，对齐 OpenAI 侧防线）
        "error" => {
            let msg = v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .unwrap_or("未知错误");
            ev.chunk.error = Some(msg.chars().take(200).collect());
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

// ────────────────────────────────────────────────────────────────────
// 测试：消息转换全矩阵 / 请求体 / URL 归一化 / SSE 解析 / 非流式响应
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    // ── ToolSlotMapper（真实环境 400 回归）──

    /// 模拟一轮 Anthropic 事件流经 parse + remap + 生产同一累积函数后的工具列表
    fn run_anthropic_stream(lines: &[&str]) -> Vec<(String, String, String)> {
        let mut mapper = ToolSlotMapper::new();
        let mut calls: Vec<(String, String, String)> = Vec::new();
        for line in lines {
            let Some(ev) = parse_anthropic_event(line) else {
                continue;
            };
            for mut d in ev.chunk.tool_calls {
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
    }

    // ── SSE 解析 ──

    fn parse(line: &str) -> Option<AnthropicEvent> {
        parse_anthropic_event(line)
    }

    #[test]
    fn sse_text_flow() {
        let ev = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#)
            .expect("text_delta 应解析");
        assert_eq!(ev.chunk.content.as_deref(), Some("你好"));
        assert!(ev.usage.is_none());
        // message_start 带 usage
        let ev = parse(r#"data: {"type":"message_start","message":{"id":"m1","usage":{"input_tokens":12,"output_tokens":1}}}"#)
            .expect("message_start 应解析（捞 usage）");
        assert_eq!(
            ev.usage,
            Some(AnthropicUsage {
                input_tokens: 12,
                output_tokens: 1
            })
        );
        assert!(ev.chunk.content.is_none());
        // message_delta 带 stop_reason + usage
        let ev = parse(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#)
            .expect("message_delta 应解析");
        assert_eq!(ev.chunk.finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            ev.usage,
            Some(AnthropicUsage {
                input_tokens: 0,
                output_tokens: 7
            })
        );
        // message_stop
        let ev = parse(r#"data: {"type":"message_stop"}"#).expect("message_stop 应解析");
        assert!(ev.chunk.is_done);
    }

    #[test]
    fn sse_tool_use_partial_json_across_blocks() {
        let ev = parse(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"create_task"}}"#)
            .expect("tool_use 开始");
        assert_eq!(ev.chunk.tool_calls.len(), 1);
        assert_eq!(ev.chunk.tool_calls[0].id.as_deref(), Some("toolu_1"));
        assert_eq!(
            ev.chunk.tool_calls[0].name_chunk.as_deref(),
            Some("create_task")
        );

        let d1 = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"title\":"}}"#)
            .expect("partial_json 1");
        let d2 = parse(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"买牛奶\"}"}}"#)
            .expect("partial_json 2");
        assert_eq!(
            d1.chunk.tool_calls[0].arguments_chunk.as_deref(),
            Some("{\"title\":")
        );
        assert_eq!(
            d2.chunk.tool_calls[0].arguments_chunk.as_deref(),
            Some("\"买牛奶\"}")
        );
    }

    #[test]
    fn sse_parallel_tool_uses_keep_their_index() {
        let a = parse(r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_a","name":"list_tasks"}}"#).unwrap();
        let b = parse(r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_b","name":"get_current_time"}}"#).unwrap();
        assert_eq!(a.chunk.tool_calls[0].index, 0);
        assert_eq!(b.chunk.tool_calls[0].index, 1);
        let d = parse(r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#).unwrap();
        assert_eq!(
            d.chunk.tool_calls[0].index, 1,
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
            let line =
                format!(r#"data: {{"type":"message_delta","delta":{{"stop_reason":"{sr}"}}}}"#);
            let ev = parse(&line).expect("message_delta 应解析");
            assert_eq!(
                ev.chunk.finish_reason.as_deref(),
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
        assert_eq!(ev.chunk.error.as_deref(), Some("Overloaded"));
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
        assert_eq!(ev.chunk.reasoning.as_deref(), Some("让我想想"));
        assert!(ev.chunk.content.is_none(), "thinking 不进正文");
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
}
