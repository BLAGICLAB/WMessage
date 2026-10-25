//! SSE 流解析层：chunk 解析、思考块拆分、工具调用增量累积。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{ParsedChunk, TokenUsage, ToolCallDelta};
use crate::bot::registry::claims_mutation;
use crate::bot::registry::mutating_tools as MUTATING_TOOLS;

/// 会改动任务卡/文件系统的工具（判定「本轮是否真的动手了」）
///
/// 变更工具是否真的成功落库/落盘（幻觉守卫 mutation_done 的判定依据）。
/// 按执行结果判定而非调用前按名字置位：被门禁拦截（status=Denied）、用户拒绝、
/// 执行失败的调用都不算「动过手」，否则之后的幻觉汇报就不再被拦，守卫被架空。
/// 成败口径走 `ToolStatus::is_success()`——status 是唯一真相源，不再对文本做
/// 失败前缀匹配（此前 4 类「没执行」的文案匹配不上，会被当成功放行）。
pub(crate) fn mutation_succeeded(name: &str, result: &crate::bot::registry::ToolResult) -> bool {
    MUTATING_TOOLS().iter().any(|t| t == &name) && result.status.is_success()
}

/// 变更声称守卫的判定：终稿声称改了东西，而本轮**没有动过手**。
///
/// 「动过手」有两类凭据：
/// - `mutation_done`：变更类工具（写盘 / 建任务卡…）调用成功；
/// - `unverifiable_ran`：跑过**能改状态但核验不了凭证**的工具
///   （run_python 自己写盘、MCP 工具在外部服务里执行、子 agent 代劳）。
///   这类的副作用不进我们的痕迹表，「没凭证」推不出「没动手」——少了这条，
///   「用 Python 写完文件并如实汇报」会被当成凭空汇报，模型被迫重跑
///   （副作用翻倍），终稿末尾还会挂一条「这句没有对应凭证」的更正。
///
/// 口径与落盘凭证那道闸一致（同一份 unverifiable_ran）。
pub(crate) fn claim_mutation_fires(
    mutation_done: bool,
    unverifiable_ran: bool,
    text: &str,
) -> bool {
    !mutation_done && !unverifiable_ran && claims_mutation(text)
}

/// <think> 标签拆分：喂入流式文本，返回 (正文, 思考)。标签跨流式块时用 think_buf 缓冲。
pub(crate) fn feed_think(in_think: &mut bool, buf: &mut String, text: &str) -> (String, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    buf.push_str(text);
    let mut normal = String::new();
    let mut think = String::new();
    loop {
        if !*in_think {
            if let Some(pos) = buf.find(OPEN) {
                normal.push_str(&buf[..pos]);
                buf.drain(..pos + OPEN.len());
                *in_think = true;
            } else {
                // 结尾可能是不完整的 <think> 前缀，留着等下一块
                let keep = tail_prefix_len(buf, OPEN);
                if keep > 0 {
                    let cut = buf.len() - keep; // keep 为 ASCII 前缀，字节数=字符数
                    normal.push_str(&buf[..cut]);
                    buf.drain(..cut);
                } else {
                    normal.push_str(buf);
                    buf.clear();
                }
                break;
            }
        } else if let Some(pos) = buf.find(CLOSE) {
            think.push_str(&buf[..pos]);
            buf.drain(..pos + CLOSE.len());
            *in_think = false;
        } else {
            let keep = tail_prefix_len(buf, CLOSE);
            if keep > 0 {
                let cut = buf.len() - keep;
                think.push_str(&buf[..cut]);
                buf.drain(..cut);
            } else {
                think.push_str(buf);
                buf.clear();
            }
            break;
        }
    }
    (normal, think)
}

/// s 结尾与 tag 开头重合的长度（如 s 尾是 "<thi"、tag "<think>" → 4）。匹配部分必是 ASCII，字节数=字符数。
fn tail_prefix_len(s: &str, tag: &str) -> usize {
    let mut k = tag.len().min(s.len());
    while k > 0 {
        if s.is_char_boundary(s.len() - k) && tag.starts_with(&s[s.len() - k..]) {
            break;
        }
        k -= 1;
    }
    k
}

// SSE chunk 解析
//
// 动机：消除 tests/llm_integration.rs 与 run_model_loop 的解析逻辑重复。
// 提取后：测试调 wmessage_lib::bot::parse_sse_chunk，生产代码同样调之，
//        OpenAI SSE 格式演化只改这一处。

/// 解析一行 OpenAI 兼容 SSE（`data: <json>` 或 `data: [DONE]`）。
/// 返回 None = 非 data 行 / JSON 解析失败 / choices 为空。
/// JSON 解析失败这一支由调用方用 `unparsable_sse_payload` 区分「协议正常成分」
/// 与「真坏块」并留痕——解析不了本身仍按跳过处理，不中断流。
///
/// 注意：纯函数，不产生任何 side effect（无事件 emit、无 think-block 处理、无 final_text push）。
/// 调用方（run_model_loop）负责 feed_think + emit + 累积。
pub fn parse_sse_chunk(line: &str) -> Option<ParsedChunk> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(ParsedChunk {
            is_done: true,
            ..Default::default()
        });
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    // usage 必须在 choices 判定之前取：OpenAI 带 include_usage 时末块是
    // `{"choices":[], "usage":{...}}`，choices 空数组会在下面的分支提前 return。
    let usage = parse_openai_usage(&v);
    let choice0 = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());
    let Some(choice0) = choice0 else {
        // 200 流内错误载荷（部分兼容网关发 `{"error":{...}}`）不得走 None
        // 静默丢弃，否则用户拿到无错误提示的空白回复
        if let Some(msg) = extract_stream_error(&v) {
            return Some(ParsedChunk {
                error: Some(msg),
                ..Default::default()
            });
        }
        // 纯 usage 末块（choices 空但带 usage）：不是错误，照常返回让调用方记账
        return usage.map(|u| ParsedChunk {
            usage: Some(u),
            ..Default::default()
        });
    };
    let delta = &choice0["delta"];

    let mut chunk = ParsedChunk {
        usage,
        ..Default::default()
    };

    if let Some(t) = delta["content"].as_str() {
        if !t.is_empty() {
            chunk.content = Some(t.to_string());
        }
    }

    if let Some(t) = delta["reasoning_content"].as_str() {
        if !t.is_empty() {
            chunk.reasoning = Some(t.to_string());
        }
    }

    if let Some(tcs) = delta["tool_calls"].as_array() {
        for tc in tcs {
            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
            let mut delta_tc = ToolCallDelta {
                index: idx,
                ..Default::default()
            };
            if let Some(id) = tc["id"].as_str() {
                delta_tc.id = Some(id.to_string());
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                delta_tc.name_chunk = Some(name.to_string());
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                delta_tc.arguments_chunk = Some(args.to_string());
            }
            chunk.tool_calls.push(delta_tc);
        }
    }

    if let Some(reason) = choice0["finish_reason"].as_str() {
        chunk.finish_reason = Some(reason.to_string());
    }

    Some(chunk)
}

/// 从 OpenAI 形状的 chunk 抽 token 用量（纯函数，无副作用）。
///
/// 两处口径差异必须在此归一，否则同一个列会混进两种含义：
/// - **缓存归口不同**：OpenAI 的 `prompt_tokens` 已含缓存部分（缓存量在
///   `prompt_tokens_details.cached_tokens`），而落库契约要求 `input_tokens`
///   是「普通 input 量」（与 Anthropic 的 `input_tokens` 同义）。故减去，
///   `saturating_sub` 防脏网关给出 cached > prompt 时下溢成天文数字。
/// - **无写入口**：OpenAI 兼容形状不报 cache write，保持 0（不是解析失败）。
///
/// 无 usage 字段（含未请求 include_usage 的网关）返回 None，调用方按「本块
/// 无用量」处理——这与 Anthropic 各事件各自带 usage 的形状不同，
/// 故调用方**只认末块**，不能逐块累加（逐块累加会把末块的总量重复计入）。
pub fn parse_openai_usage(v: &serde_json::Value) -> Option<TokenUsage> {
    let u = v.get("usage")?;
    if !u.is_object() {
        return None;
    }
    let prompt_total = u["prompt_tokens"].as_u64()?;
    let cached = u["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    Some(TokenUsage {
        input_tokens: prompt_total.saturating_sub(cached),
        output_tokens: u["completion_tokens"].as_u64().unwrap_or(0),
        cache_read_tokens: cached,
        cache_creation_tokens: 0,
    })
}

/// token 用量的合并口径：**按字段取最后一次非零值**，不逐块累加。
///
/// 两协议的 usage 都是「本请求截至此刻的**总量**」，不是增量：
/// - OpenAI：`stream_options.include_usage` 让末块带 usage（每块都带的网关
///   并不少见），每块带的是同一份总量；
/// - Anthropic：message_delta 的 `usage.output_tokens` 是累计输出量。
///
/// 逐块累加会把同一份总量重复计入（每块都带 usage 的网关按块数 ×N），
/// 落库后 token 数与缓存命中率全部虚高，前端水位条也跟着虚高。
///
/// 但两协议的字段**分散在不同事件**里（Anthropic 的 input 在 message_start、
/// output 在 message_delta；OpenAI 两者同块），故按字段取最后一次非零值，
/// 而不是整块替换——整块替换会把 Anthropic 的 input 抹成 0。
pub fn merge_stream_usage(acc: &mut TokenUsage, u: TokenUsage) {
    if u.input_tokens > 0 {
        acc.input_tokens = u.input_tokens;
    }
    if u.output_tokens > 0 {
        acc.output_tokens = u.output_tokens;
    }
    if u.cache_read_tokens > 0 {
        acc.cache_read_tokens = u.cache_read_tokens;
    }
    if u.cache_creation_tokens > 0 {
        acc.cache_creation_tokens = u.cache_creation_tokens;
    }
}

/// 给 assistant 消息挂上本轮的 Anthropic thinking 块。
///
/// 内部消息流是 OpenAI 形状，同一份 msgs 也会直接发给 OpenAI 端点，故这个
/// provider 专属键发 OpenAI 前会被剥掉（见 build_request_body）。
pub(crate) fn with_thinking(
    mut msg: serde_json::Value,
    thinking: &[serde_json::Value],
) -> serde_json::Value {
    if !thinking.is_empty() {
        if let Some(obj) = msg.as_object_mut() {
            obj.insert(
                crate::bot_anthropic::THINKING_KEY.to_string(),
                serde_json::Value::Array(thinking.to_vec()),
            );
        }
    }
    msg
}

/// 给消息栈里最后一条 assistant 消息补上本轮思考段（已有则不动）。
///
/// 覆盖面必须比「工具轮那一条」更宽：凡是思考模式产出的 assistant 消息，
/// 后续请求都得带上它的 reasoning_content——DeepSeek 官方口径是「携带 tools
/// 的请求必须完整回传 reasoning_content，否则 API 返回 400」，且明确「即使
/// 该轮模型未实际进行工具调用」也要回传。自检返工、幻觉守卫返工、长度续跑
/// 这三条路径都会再发一次请求，它们推回的 assistant 消息同样要补——只补工具
/// 轮那一条，这三条路径照样 400。
///
/// 空思考段不补：模型没思考就没有可回灌的东西。
///
/// 改的是消息栈**尾部**那条（它后面只跟着一条注入的 user 提示），不动更早
/// 的内容——隐式前缀缓存靠的正是更早那截不动。
pub(crate) fn stamp_last_assistant_reasoning(msgs: &mut [serde_json::Value], reasoning: &str) {
    if reasoning.trim().is_empty() {
        return;
    }
    let Some(idx) = msgs
        .iter()
        .rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
    else {
        return;
    };
    if msgs[idx].get("reasoning_content").is_some() {
        return;
    }
    if let Some(obj) = msgs[idx].as_object_mut() {
        obj.insert(
            "reasoning_content".to_string(),
            serde_json::Value::String(reasoning.to_string()),
        );
    }
}

/// 从内部消息里剥掉 provider 专属键（发 OpenAI 端点前用）。
///
/// 只有 Anthropic 分支会挂 thinking 块载体，平时不命中——不命中时直接
/// 原样返回，不重建数组。
pub(crate) fn strip_thinking_key(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let key = crate::bot_anthropic::THINKING_KEY;
    if !msgs.iter().any(|m| m.get(key).is_some()) {
        return msgs.to_vec();
    }
    msgs.iter()
        .map(|m| {
            let mut c = m.clone();
            if let Some(obj) = c.as_object_mut() {
                obj.remove(key);
            }
            c
        })
        .collect()
}

/// 协议解析失败时的坏块判定（纯函数，供调用方计数留痕）——坏块曾经无声
/// 消失：网关吐半截 JSON 或非本协议形状的载荷，回复凭空少一段，日志里什么
/// 都不留。两协议各有一条同样形态的 `.ok()?`（OpenAI parse_sse_chunk /
/// Anthropic parse_anthropic_event），故按 provider 分派形状判定。
///
/// 返回 Some(载荷) = data 行 + 非空 + 非 `[DONE]` 且
/// - JSON 解析失败，或
/// - OpenAI 形状：JSON 合法却没有 choices 数组（中转网关的错误信封
///   `{"code":401,...}`、外来载荷等——「解析成功但形状不对」同样会丢内容）；
///
/// 返回 None = 非 data 行（`event:` / 注释 / 空行）、空心跳、`[DONE]`、
/// 标准形状（含 `{"choices":[]}` 空心跳），以及一切合法的 Anthropic 事件
/// （Anthropic 载荷本就没有 choices 键，故该臂只对 Openai 生效）。
pub(crate) fn unparsable_sse_payload(
    line: &str,
    provider: crate::bot::ApiProvider,
) -> Option<&str> {
    let data = line.strip_prefix("data:")?.trim();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    let v = match serde_json::from_str::<serde_json::Value>(data) {
        Ok(v) => v,
        Err(_) => return Some(data),
    };
    if provider == crate::bot::ApiProvider::Openai
        && !v.get("choices").is_some_and(|c| c.is_array())
    {
        return Some(data);
    }
    None
}

/// 从流内 JSON 载荷提取错误消息（`{"error":{"message":...}}` 或 `{"error":"..."}`，截 200 字）
fn extract_stream_error(v: &serde_json::Value) -> Option<String> {
    let e = v.get("error")?;
    if let Some(m) = e.get("message").and_then(|m| m.as_str()) {
        return Some(m.chars().take(200).collect());
    }
    e.as_str().map(|s| s.chars().take(200).collect())
}

/// 从字节缓冲切出完整的 SSE 行（按 `b'\n'` 切，残余不完整行留在 buf）。
/// 按字节切行的原因：TCP chunk 边界可能落在多字节
/// UTF-8 字符中间，逐 chunk `from_utf8_lossy` 会产生 U+FFFD 替换字符——正文里只是乱码，
/// 工具 arguments 里则是「合法 JSON 但内容损坏」会被真实执行。
/// `b'\n'`（0x0A）不会出现在多字节 UTF-8 序列内，整行 decode 安全。
/// pub：tests/llm_integration.rs 的分片用例复用（与生产同一切行逻辑，防双份实现漂移）。
pub fn drain_sse_lines(buf: &mut Vec<u8>) -> Vec<String> {
    // 一次性扫描切出所有完整行（O(N)，原 O(N×M) 每行独立 drain 重排 buf）
    // 只在最后一个完整行之后做一次 buf.drain(..last_end)，避免反复 shift 元素
    let mut lines = Vec::new();
    let mut last_end = 0;
    for i in 0..buf.len() {
        if buf[i] == b'\n' {
            let line = buf[last_end..=i].to_vec();
            lines.push(String::from_utf8_lossy(&line).into_owned());
            last_end = i + 1;
        }
    }
    if last_end > 0 {
        buf.drain(..last_end);
    }
    lines
}

/// 单条响应内 tool_calls 的 index 上限：
/// index 来自服务端，畸形/恶意 index（如 10000000）会让累积循环无脑 push 撑爆内存
pub const MAX_TOOL_CALL_INDEX: usize = 64;

/// 单条 tool_call arguments 的累积字节上限：index 上限只管条数，管不住单条大小——
/// 畸形/恶意上游可流式发送任意大的 arguments 增量把单条撑到 OOM。
/// 超限的增量一律丢弃（返回 false，与 index 超限同语义，调用方走同一审计）。
pub const MAX_TOOL_CALL_ARGUMENTS_BYTES: usize = 1024 * 1024;

/// 把一个 tool_call delta 按 index 归位累积进 (id, name, arguments) 列表：
/// id 只置首次（迟到的 id 能补上）、name/arguments 跨 delta 追加。
/// 返回 false = index 超上限，该 delta 被丢弃（调用方记审计）。
/// pub：tests/llm_integration.rs 复用同一累积逻辑（防测试自写平铺式累积
/// 与生产 index 归并语义漂移）。
pub fn accumulate_tool_call_delta(
    calls: &mut Vec<(String, String, String)>,
    delta: &ToolCallDelta,
) -> bool {
    if delta.index > MAX_TOOL_CALL_INDEX {
        return false;
    }
    while calls.len() <= delta.index {
        calls.push((String::new(), String::new(), String::new()));
    }
    let t = &mut calls[delta.index];
    if let Some(id) = &delta.id {
        if t.0.is_empty() {
            t.0 = id.clone();
        }
    }
    if let Some(name) = &delta.name_chunk {
        if !name.is_empty() {
            t.1.push_str(name);
        }
    }
    if let Some(args) = &delta.arguments_chunk {
        if t.2.len() + args.len() > MAX_TOOL_CALL_ARGUMENTS_BYTES {
            return false;
        }
        t.2.push_str(args);
    }
    true
}
