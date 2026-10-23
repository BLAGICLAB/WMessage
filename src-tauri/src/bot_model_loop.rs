//! 模型流式调用 + 工具循环 核心：
//!
//! 经典 Agent 框架（LangChain AgentExecutor / Claude Agent SDK / AutoGen）
//! 的「决策/调用」与「执行/工具循环」层职责：
//! - 流式 SSE 解析（parse_sse_chunk）
//! - 思考块拆分（feed_think + tail_prefix_len）
//! - 工具循环（run_model_loop）：单轮 Function 调用熔断 + 软警告 +
//!   Skill 状态机推进 + 进程内执行工具 + 续聊
//! - TOOLS schema 编译期保证合法（tests/llm_integration.rs 接入）
//!
//! 本模块与 bot_chat 的边界：bot_chat.rs 调 run_model_loop 拿到最终文本；
//! 本模块只关心「怎么流式拿到最终文本 + 工具执行结果」，不关心输入侧组装。

use crate::bot_chat::{merge_task_refs_dedup, TaskRef};
use crate::bot_slash::StopGuard;
use crate::error::CommandError;

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};

// 从 crate::bot::registry 单源派生的 TOOLS / MUTATING_TOOLS。
// 原 const 字符串 / 数组现为函数（OnceLock 缓存）——保持向后兼容路径。
pub use crate::bot::registry::mutating_tools as MUTATING_TOOLS;

// 防幻觉汇报守卫
// 实锤事故：MiniMax-M3 多次不调任何工具就回复「已添加子任务」「已移至回收站」，
// 数据实际没变，用户以为操作成功。提示词约束（SYSTEM_PROMPT 规则 8）不够，
// 这里在循环出口做确定性拦截：最终文本声称完成变更、但本轮 0 次变更类工具调用 →
// 注入系统提醒并补一轮（每次对话最多补一次），让模型实际调工具或如实说明。

/// 会改动任务卡/文件系统的工具（判定「本轮是否真的动手了」）
///
/// 变更工具是否真的成功落库/落盘（幻觉守卫 mutation_done 的判定依据）。
/// 按执行结果判定而非调用前按名字置位：被门禁拦截（status=Denied）、用户拒绝、
/// 执行失败的调用都不算「动过手」，否则之后的幻觉汇报就不再被拦，守卫被架空。
/// 成败口径走 `ToolStatus::is_success()`——status 是唯一真相源，不再对文本做
/// 失败前缀匹配（此前 4 类「没执行」的文案匹配不上，会被当成功放行）。
fn mutation_succeeded(name: &str, result: &crate::bot::registry::ToolResult) -> bool {
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
fn claim_mutation_fires(mutation_done: bool, unverifiable_ran: bool, text: &str) -> bool {
    !mutation_done && !unverifiable_ran && claims_mutation(text)
}

/// 从 ToolDef 注册表派生的变更声称检测。
/// 原「14 个动词 + 4 个整段」硬编码迁到 `bot::registry::ToolDef.claims_patterns`，
/// 加新 mutating 工具只需填自己的 pattern，检测逻辑零修改。
pub use crate::bot::registry::claims_mutation;
use crate::bot::registry::{
    claimed_doc_paths, claimed_executed_tools, claims_execution_with, mentioned_tool_names,
    path_missing_on_disk,
};

// TOOLS schema（编译期字符串，运行期 JSON 解析）

// TOOLS / MUTATING_TOOLS 已在阶段 2 拆出到 crate::bot::registry：
// - `crate::bot::registry::tools_json()` → 原 TOOLS JSON（OnceLock 缓存）
// - `crate::bot::registry::mutating_tools()` → 原 MUTATING_TOOLS（15 个 mutating 工具名）
// 公开路径仍走 `pub use crate::bot::registry::tools_json as TOOLS;` / `as MUTATING_TOOLS;`
// 但因 const → fn 类型变化（保持向后兼容需按函数调用），下游消费者已同步更新。

/// <think> 标签拆分：喂入流式文本，返回 (正文, 思考)。标签跨流式块时用 think_buf 缓冲。
fn feed_think(in_think: &mut bool, buf: &mut String, text: &str) -> (String, String) {
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

/// 一次 SSE chunk 内捞到的 token 用量（两协议归一后的口径）
///
/// 落库契约（见 SPEC 词元统计四档计价）：`input_tokens` 是**普通 input 量**，
/// 不含任何缓存档。两协议的上报形状不同，落库前各自归一——
/// - Anthropic：`input_tokens` 天然不含缓存，另有 `cache_read_input_tokens`
///   / `cache_creation_input_tokens`
/// - OpenAI：`prompt_tokens` **含** `cached_tokens`（在
///   `prompt_tokens_details`），故解析时减去缓存档再当普通 input
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// 一次 SSE chunk 解析结果（content / reasoning / tool_calls / finish_reason / 流内错误 / [DONE]）
#[derive(Debug, Default, Clone)]
pub struct ParsedChunk {
    /// delta.content（仅在非空字符串时 Some）
    pub content: Option<String>,
    /// delta.reasoning_content（DeepSeek-reasoner 等模型的独立推理字段；
    /// 与 <think> 标签同走 bot-think-delta 出口）
    pub reasoning: Option<String>,
    /// Anthropic 的 thinking 块增量（仅 Anthropic 协议会产出）。
    ///
    /// 与 `reasoning` 分开：reasoning 只是给前端看的思考文本，thinking 块
    /// 要**原样回传**给服务端（丢了下一轮带 tool_result 的请求会被 400），
    /// 故走独立的增量通道，由 bot_anthropic::ThinkingCollector 归位成块。
    pub thinking: Option<crate::bot_anthropic::ThinkingDelta>,
    /// delta.tool_calls 增量（多 chunk 拼成一个完整 tool_call）
    pub tool_calls: Vec<ToolCallDelta>,
    /// choices[0].finish_reason（最后一 chunk 通常为 "stop" / "tool_calls"）
    pub finish_reason: Option<String>,
    /// 200 流内错误载荷：部分 OpenAI 兼容网关在 200 流内发 `{"error":{...}}`，
    /// 静默丢弃会导致用户拿到空白回复——显式冒出
    pub error: Option<String>,
    /// 本 chunk 携带的 token 用量（OpenAI 需请求带
    /// `stream_options.include_usage`，末块 usage 的 choices 为空数组）
    pub usage: Option<TokenUsage>,
    /// `data: [DONE]` 标记
    pub is_done: bool,
}

/// 单个 tool_call 增量字段
#[derive(Debug, Default, Clone)]
pub struct ToolCallDelta {
    /// tool_calls[*].index（默认 0）
    pub index: usize,
    /// tool_calls[*].id（Some = 原 JSON 含此字段，值可能为空串）
    pub id: Option<String>,
    /// tool_calls[*].function.name 追加块（多 chunk 拼接）
    pub name_chunk: Option<String>,
    /// tool_calls[*].function.arguments 追加块（多 chunk 拼接成完整 JSON）
    pub arguments_chunk: Option<String>,
}

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
fn with_thinking(mut msg: serde_json::Value, thinking: &[serde_json::Value]) -> serde_json::Value {
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

/// 从内部消息里剥掉 provider 专属键（发 OpenAI 端点前用）。
///
/// 只有 Anthropic 分支会挂 thinking 块载体，平时不命中——不命中时直接
/// 原样返回，不重建数组。
fn strip_thinking_key(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
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
fn unparsable_sse_payload(line: &str, provider: crate::bot::ApiProvider) -> Option<&str> {
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

/// 默认对话轮数（聊天 / 任务执行 / 逐步执行统一为 50）；
/// 多步 Skill 可在 SKILL.md frontmatter 自报 max_rounds 覆盖（见 resolve_max_rounds）；
/// 配置 `maxRounds` 亦可覆盖默认（Skill 仍最优先），常量单源 bot/params.rs。
pub(crate) const DEFAULT_MAX_ROUNDS: usize = crate::bot::params::DEFAULT_MAX_ROUNDS as usize;

/// 单轮 msgs 总字符软上限（仅 audit，不截断）。
/// 默认 200K 字符（中文 ≈ 1 token/字符 ≈ 200K token 上下文）。
/// 超出走 `loop.msgs.over_budget` 事件供事后分析；不主动压缩、不截断、不触发摘要。
pub(crate) const MSGS_BUDGET_CHARS: usize = 200_000;

/// 本轮工具循环的轮数上限：Skill 自报 max_rounds 最优先，其次配置 maxRounds，
/// 都没有 → DEFAULT_MAX_ROUNDS（单源 bot/params.rs）。
pub(crate) fn resolve_max_rounds(
    skill_max_rounds: Option<usize>,
    config_max_rounds: usize,
) -> usize {
    skill_max_rounds.unwrap_or(config_max_rounds)
}

// Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
// max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
//
// 上限默认 100：工作流节点等长链任务
// 50 次不够用；更高上限的失控风险由幻觉守卫（claims_mutation）+ 软警告 +
// /stop 兜底。可用 bot-config.json `maxFunctionCalls` 覆盖（None = 100）；
// 子 agent 默认预算同源此值（resolve_subagent_budget），LLM 显式给的预算仍可覆盖。
// 软警告阈值动态 = cap*7/10（保持 ~30% buffer），钳 [1, cap-1]。
const MAX_FUNCTION_CALLS_PER_REQUEST: usize = 100;

/// 终稿自检 trigger 条件：final_text 字符数下限。trivial 回复不值得跑自检——
/// 调小（50）会让「好的/收到」这种短回复也触发，刷 token；调大（200）会漏
/// 短答复+长声明的混合回复。硬编码常量，不暴露设置。
pub(crate) const MIN_FINAL_SELFCHECK_CHARS: usize = 80;
/// 终稿自检 trigger 条件：本会话累计函数调用次数下限。无真实工作量的对话
/// 不该触发——调小（1）会让刚开聊两句就触发，调大（5）会漏掉小工作量对话。
/// 硬编码常量，不暴露设置。
pub(crate) const MIN_FINAL_SELFCHECK_FUNCTION_CALLS: usize = 3;
/// 终稿自检截断注入长度：截 final_text 开头 N 字注入 prompt，覆盖主要声明段
/// （"已完成/已生成"等口胡多数在段首）。N=500 与 SELFCHECK_PROMPT 的 snippet 同
/// 截断逻辑，对齐视觉口径。
pub(crate) const FINAL_SELFCHECK_SNIPPET_CHARS: usize = 500;
/// 终稿自检默认轮次间隔（设置项为 None 时启用）：8 轮 1 次。
const DEFAULT_FINAL_SELFCHECK_ROUND_GAP: u32 = 8;

/// stop_reason=max_tokens 后的自动续跑配额。续跑是全新请求、带全新输出预算，
/// 一轮通常就够；留 2 是为「首轮续跑仍撞上限」再给一次机会，再多只白烧 token
/// ——截断的成因不会因为多试几次而消失。
const MAX_LENGTH_CONTINUATIONS: usize = 2;

/// 截断续跑的注入文案。正文与工具调用在响应里排在思维链之后，撞上限时被砍掉
/// 的总是它们，文案要同时覆盖「接着写」和「别只说要动手、直接调工具」两种断法。
const LENGTH_CONTINUE_PROMPT: &str = "\
【系统提示】上一条回复超出长度上限被截断。请从断处接着写，不要重复已写的内容。\
如果你刚才正准备调用工具，现在就直接调用它，不要只用文字描述下一步。";

/// 滚动降密度：轮龄 ≥ 此值的 tool 结果软压（头 200 + 尾 100 + sidecar 指针）。
/// N=5 对齐业界——SWE-agent 消融只留最近 5 条观察比全历史高 3 分，
/// Claude Code microcompact 亦为最近 5 条工具结果。
const AGE_SOFT_ROUNDS: usize = 5;
/// 轮龄 ≥ 此值再降硬档（30+30+指针）。保守默认，config 化挂账
/// （见 docs/AGENT-TOKEN-EFFICIENCY-PLAN.md 滚动降密度一节）。
const AGE_HARD_ROUNDS: usize = 15;

/// 终稿自检 N 判定追加到原文尾部的警告（in-loop 与侧调两条路径共用）。
const FINAL_SELFCHECK_WARN: &str = "\n\n⚠️ 自检未达预期，请人工核对";

/// 出口闸追问「请把真实结果说出来」的上限：模型只回 Y/N 判定时不能无限追着问。
/// 2 次仍不给结果就落到兜底文案（别把裸 Y 交给用户）。追问是完整的一轮 LLM
/// 调用，次数要有硬顶。
const MAX_LATE_VERDICT_RETRIES: u8 = 2;

/// 单条工具结果回灌默认上限（字节，UTF-8）。业界对齐——Claude Code 文件读 25K token、
/// Bash 30K 输出、Gemini CLI 40K 输出。配置 Some(0) = 显式关闭截断。
///
/// 为什么按字节而不是字符：这条上限的职责是「别把模型上下文撑爆」，而撑爆的是
/// token，不是字符数。字节到 token 的换算跨语种稳定得多（中文约 3 字节/token、
/// ASCII 约 4 字节/token），字符数则随语种漂三倍（同样 30000，中文占 3 倍字节、
/// 只占 1/4 的 token）。且同一漏斗里 trace 侧的 `db::SPAN_TEXT_MAX` 与溢出指针
/// 文案本来就按字节报，字符口径会让同一次溢出在两个地方显示成两个数。
pub(crate) const DEFAULT_TOOL_OUTPUT_CAP_BYTES: usize = 30_000;
/// 截断时尾部保留字节数：错误堆栈/汇总性内容常在尾部，只留头部会丢关键信息。
const TOOL_OUTPUT_TAIL_KEEP_BYTES: usize = 6_000;

/// 本次工具结果回灌的实际天花板（字节）：per-tool 预算与全局配置 cap 取小。
///
/// 两个都要，不能只要一个：
/// - per-tool 预算（`ToolDef.max_output_bytes`）管「这个工具天生输出就大」——
///   fetch_url / read_text_file / list_files 可以给到 65536，其余默认 8192。
/// - 全局 cap 管用户在设置页调的总量，也是唯一兜底——per-tool 预算被调到比它大时
///   不能放任，否则设置页那个旋钮就成了空设置。
fn effective_output_cap(tool: &str, global_cap_bytes: usize) -> usize {
    crate::bot::registry::tools_index()
        .get(tool)
        .map(|t| t.max_output_bytes)
        .unwrap_or(global_cap_bytes)
        .min(global_cap_bytes)
}

/// 超预算时的渲染：优先转存溢出文件 + 指针；转存失败退回纯头尾截断。
///
/// 退回路径必须保留：落盘失败时若直接放行原文本，模型就白挨这一轮大上下文；
/// 若返回空，结果凭空消失更糟。`cap_tool_output` 的四一分在无第二份拷贝时是对的，
/// 这正是它作为兜底而非主路径的原因。
fn spill_or_cap(app: &tauri::AppHandle, tool: &str, text: String, cap_bytes: usize) -> String {
    match crate::overflow::spill(app, &text) {
        Ok(path) => {
            let notice = crate::overflow::pointer_text(tool, &path, &text);
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Info,
                "tool.output.spilled",
                &[
                    ("tool", tool.to_string()),
                    ("original", text.len().to_string()),
                    ("cap", cap_bytes.to_string()),
                    ("path", path.display().to_string()),
                ],
            );
            preview_with_pointer(&text, &notice, cap_bytes)
        }
        Err(e) => {
            crate::audit::write_event(
                app,
                crate::audit::AuditLevel::Warn,
                "tool.output.spill_failed",
                &[
                    ("tool", tool.to_string()),
                    ("cap", cap_bytes.to_string()),
                    ("err", e),
                ],
            );
            cap_tool_output(&text, cap_bytes)
        }
    }
}

/// 节选预览：头一半 + 指针 + 尾一半（字节口径）；指针自身长度先从 cap 里扣。
///
/// 为什么五五分而 `cap_tool_output` 是四一：那份是「没有第二份拷贝」时的最后手段，
/// 头部承载更多价值；这里完整内容已落盘、模型能分页读回，节选的职责只是让模型
/// 判断「要不要读、读哪一段」，不是复述正文，头尾对半才不至于把结论藏在另一半。
/// 纯函数便于单测。
fn preview_with_pointer(text: &str, notice: &str, cap_bytes: usize) -> String {
    // 指针两侧各一段 \n\n；不扣掉它们，节选会凭空超出 cap 这 4 个字节
    const SEPS: usize = 4;
    let half = cap_bytes.saturating_sub(notice.len() + SEPS) / 2;
    let head_end = crate::bot::format::byte_head_end(text, half);
    // 头尾不得重叠：正文短于两份预算时宁可不重复也别把同一段读两遍
    let tail_start = crate::bot::format::byte_tail_start(text, half).max(head_end);
    format!(
        "{}\n\n{}\n\n{}",
        &text[..head_end],
        notice,
        &text[tail_start..]
    )
}

/// 截断到 cap 字节：头 (cap-tail) + 截断说明 + 尾 tail。不超 cap 原样返回。
/// 纯函数便于单测；调用方（薄壳 execute_tool）负责审计留痕。
pub(crate) fn cap_tool_output(text: &str, cap_bytes: usize) -> String {
    let total = text.len();
    if total <= cap_bytes {
        return text.to_string();
    }
    // 小 cap 时尾部保留退化为 cap 的 1/5，保证头部至少 4/5
    let tail_keep = TOOL_OUTPUT_TAIL_KEEP_BYTES
        .min(cap_bytes / 5)
        .min(total / 2);
    let head_keep = cap_bytes - tail_keep;
    let head_end = crate::bot::format::byte_head_end(text, head_keep);
    let tail_start = crate::bot::format::byte_tail_start(text, tail_keep).max(head_end);
    let head_bytes = head_end;
    let tail_bytes = total - tail_start;
    format!(
        "{}\n…（已按 maxToolOutputBytes={cap_bytes} 截断，原文 {total} 字节，头 {head_bytes} + 尾 {tail_bytes} 保留）…\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

/// 熔断消息标记（runner 端识别熔断结果用同一常量，勿散落字符串字面量——W5 r1）
pub(crate) const FUSE_MARKER: &str = "⏹ 已熔断";

/// 轮数熔断标记。与 `FUSE_MARKER` 分开是因为两类熔断的下游语义不同：
/// Function 熔断正文仍可被当普通回复展示，轮数熔断则要被子任务侧认出来
/// 还原 `budget_exceeded`（见 bot_orchestrator::classify_outcome）。
pub(crate) const ROUNDS_FUSE_MARKER: &str = "⏹ 已熔断（轮数上限）";

/// 墙钟熔断标记。子 agent 侧据此还原 `budget_exceeded(max_wall_seconds)`
/// 并**保留熔断前的正文**（与轮数熔断同款：同一个「预算触顶」，一个留产
/// 一个不留，没有理由）。
pub(crate) const WALL_FUSE_MARKER: &str = "⏹ 已熔断（墙钟上限）";

/// 子 agent 墙钟预算（传进模型循环，让循环能在**轮边界**上做两件事）。
///
/// **软墙钟**：剩余进入收尾预留窗口就提醒模型停止展开新步骤、赶紧输出收尾
/// JSON——模型还有一次完整轮次产出结构化结果。
///
/// **墙钟熔断**：期限到了就在轮边界带着最后一轮正文退出，而不是让外层
/// `tokio::time::timeout` 把整条 future 掐掉——掐掉时正在跑的工具、已经说
/// 出口的正文一起没了（业界口径：kills are last resorts，预算停止是设计好的
/// 落地而不是失败）。
///
/// 用**绝对期限**而不是「每轮重算剩余秒数」：每层各算一份，起点漂移就等于
/// 没有统一口径（gRPC deadline 纪律）。
#[derive(Debug, Clone, Copy)]
pub struct WallBudget {
    /// 绝对期限（子 agent 进入 running 起算，排队时间不计）
    pub deadline: std::time::Instant,
    /// 收尾预留窗口：最后这段预算专门留给「把已有产出整理成收尾 JSON」
    pub reserve: std::time::Duration,
    /// 墙钟上限（秒）——只用于文案与审计留痕
    pub max_wall_seconds: u64,
}

impl WallBudget {
    /// 剩余时间（已过期的返回 0）
    pub fn remaining(&self) -> std::time::Duration {
        self.deadline
            .saturating_duration_since(std::time::Instant::now())
    }
    /// 是否进入收尾预留窗口
    pub fn in_reserve(&self) -> bool {
        self.remaining() <= self.reserve
    }
    /// 是否已过期限
    pub fn expired(&self) -> bool {
        std::time::Instant::now() >= self.deadline
    }
}

/// LLM 请求重试：429/5xx/网络错误重试一次（1.5s 退避）。
/// 只在流式产出开始前重试——响应已开始流式产出后不重试（无重放风险：
/// 重试发的是同一轮请求，已执行的工具在 msgs 里，不会因重发而重放）。
pub(crate) const MAX_LLM_ATTEMPTS: usize = 2;
pub(crate) const LLM_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// 可重试的 HTTP 状态：429 限流 + 5xx 服务端瞬时错误；401/400 等重试无意义
fn is_retryable_llm_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// 上下文超长降级：保留的历史比例（砍掉一半再试）
const CONTEXT_SHED_KEEP_RATIO: f64 = 0.5;
/// 单次模型循环内的降级次数上限（防「砍一半仍超长 → 再砍 → 再超」打转）
const CONTEXT_SHED_MAX: u8 = 2;

/// 上下文超长判定：400/413 + 载荷特征。各家文案不一（OpenAI
/// `context_length_exceeded`、Anthropic `prompt is too long`、中转网关
/// `maximum context length`、国产网关中文「上下文超长」），故 code 与关键词
/// 两类形态都认；只看状态码会把 400 的其它形态（鉴权、schema 错）误伤。
fn is_context_overflow(status: u16, body: &str) -> bool {
    if status != 400 && status != 413 {
        return false;
    }
    let b = body.to_lowercase();
    const MARKS: &[&str] = &[
        "context_length_exceeded",
        "maximum context length",
        "context window",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "reduce the length",
        "上下文超长",
        "超出上下文",
        "上下文长度",
    ];
    MARKS.iter().any(|m| b.contains(m))
}

/// 单条消息的字符数（序列化后计——比对的是请求体实际大小）
fn msg_chars(m: &serde_json::Value) -> usize {
    serde_json::to_string(m)
        .map(|s| s.chars().count())
        .unwrap_or(0)
}

/// 降级动作：砍掉最旧的会话消息，直到总量降到目标比例（纯函数，单测锚点）。
///
/// 只动 system 段之后、最后一条之前的消息：system 段是提示词/摘要/记忆块本体，
/// 最后一条是本轮用户输入；本轮已执行的 tool 消息追加在本轮请求构造之后，
/// 不落在可砍区间——砍它会破坏 tool_call/tool_result 配对，对端直接 400。
///
/// 返回 `(切口下标, 砍掉条数)`：调用方必须拿它修正按 msgs 下标记账的旁路表
/// （`tool_msg_ages`），否则砍完 msgs 变短、旧下标越界。0 条 = 没得砍（调用方走报错出口）。
fn shed_oldest_history(msgs: &mut Vec<serde_json::Value>, keep_ratio: f64) -> (usize, usize) {
    fn role_of(m: &serde_json::Value) -> &str {
        m.get("role").and_then(|r| r.as_str()).unwrap_or_default()
    }
    let first = msgs
        .iter()
        .position(|m| role_of(m) != "system")
        .unwrap_or(msgs.len());
    let mut used: usize = msgs.iter().map(msg_chars).sum();
    let target = (used as f64 * keep_ratio) as usize;
    let mut removable = msgs.len().saturating_sub(first).saturating_sub(1); // 最后一条本轮输入不可砍
    let mut removed = 0usize;
    while used > target && removable > 0 {
        used = used.saturating_sub(msg_chars(&msgs[first + removed]));
        removed += 1;
        removable -= 1;
    }
    if removed == 0 {
        return (first, 0);
    }
    // 切口不能停在 tool 分组中间：被砍走的 assistant(tool_calls) 会让紧邻的
    // tool 消息变成孤儿（对端 400：tool 必须紧跟带 tool_calls 的 assistant）。
    // 边界消息是 tool 就连带砍掉——它的父 assistant 已落在可砍区间里。
    // 最后一条是本轮输入不动，故上界停在 len-1（此时其父 assistant 仍被保留）。
    while first + removed < msgs.len().saturating_sub(1)
        && role_of(&msgs[first + removed]) == "tool"
    {
        removed += 1;
    }
    msgs.drain(first..first + removed);
    msgs.insert(
        first,
        serde_json::json!({
            "role": "system",
            "content": "[上下文超限] 早前对话已因超出模型上下文窗口被裁剪，上文从本条之后开始。"
        }),
    );
    (first, removed)
}

/// 降级砍历史后修正按 msgs 下标记账的旁路表：落在切口内的条目作废（消息已不存在），
/// 其后条目左移——净变化是 `-(removed-1)`（砍掉 removed 条、补回 1 条裁剪占位）。
fn reindex_after_shed(ages: &mut Vec<(usize, usize)>, first: usize, removed: usize) {
    if removed == 0 {
        return;
    }
    let end = first + removed;
    let shift = removed - 1;
    ages.retain_mut(|(_, idx)| {
        if *idx < first {
            true
        } else if *idx < end {
            false
        } else {
            *idx -= shift;
            true
        }
    });
}

/// 熔断上界的 usize 投影：与子 agent 预算解析（params::MAX_FUNCTION_CALLS_CAP）
/// 是同一个数，这里只做类型转换——两处各写一份字面量就会在「预算按 500 记账、
/// 循环按 999 放行」这种偏差上分叉，且分叉到熔断才暴露。
pub(crate) fn fuse_cap_upper_bound() -> usize {
    crate::bot::params::MAX_FUNCTION_CALLS_CAP as usize
}

/// 熔断上限派生链的唯一入口：子 agent 行上预算 > bot-config 覆盖 > 内置默认，
/// 末端钳到 [1, 上界]。
///
/// 上游（薄壳组装 deps）刻意不钳：`deps.fuse_cap_override` 也被单测直驱 core
/// 注入，绕过薄壳，钳在薄壳等于给「实际生效的那条边界」留了个不生效的副本。
fn fuse_cap_for(subagent_budget: Option<u32>, cfg_override: Option<usize>) -> usize {
    subagent_budget
        .map(|n| n as usize)
        .or(cfg_override)
        .unwrap_or(MAX_FUNCTION_CALLS_PER_REQUEST)
        .clamp(1, fuse_cap_upper_bound())
}

/// 熔断判定：第 n 次（1-based 累计）Function 调用是否超上限。
/// cap 由会话派生：子 agent = 预算 max_tool_calls，其余 = 默认 100
/// bot-config `maxFunctionCalls` 可覆盖，钳 1..=500）。
fn should_fuse(calls_so_far: usize, cap: usize) -> bool {
    calls_so_far > cap
}

/// 熔断返回消息（与主循环文案同源，单测直接断言）。
///
/// `pub(crate)`：子 agent 收尾判定要认这条熔断（工具预算触达 → budget_exceeded），
/// 测试也要用同一份文案——两处各拼一遍字符串会漂。
pub(crate) fn fuse_message(final_text: &str, hint: &str, cap: usize) -> String {
    format!(
        "{final_text}\n\n{FUSE_MARKER}：本轮 Function 调用超过 {cap} 次上限（安全保护），已停止后续执行{hint}"
    )
}

/// 轮数熔断返回消息：与 `fuse_message` 同源（都带上熔断前的正文），只是上限口径不同。
/// `partial` 传 `last_streamed`（上一轮 streamed 快照）——循环体退出时本轮的
/// final_text 尚未快照，能拿到的最新正文就是它。
pub(crate) fn rounds_fuse_message(partial: &str, hint: &str, max_rounds: usize) -> String {
    format!(
        "{partial}\n\n{ROUNDS_FUSE_MARKER}：对话轮数超过 {max_rounds} 轮上限（安全保护），已停止后续执行{hint}"
    )
}

/// 墙钟熔断返回消息：与 `rounds_fuse_message` 同源（都带上熔断前的正文），
/// 只是口径不同——轮数熔断是「轮」，墙钟熔断是「秒」。
///
/// 带正文是关键：墙钟触达时子 agent 可能已经跑了几十分钟、产出齐了，
/// 只留一句「墙钟预算触达」等于把这段时间的产出全丢掉，父 agent 只能
/// 从零重烧。
pub(crate) fn wall_fuse_message(partial: &str, hint: &str, max_wall_seconds: u64) -> String {
    format!(
        "{partial}\n\n{WALL_FUSE_MARKER}：墙钟预算超过 {max_wall_seconds} 秒上限（安全保护），已停止后续执行{hint}"
    )
}

/// 空 replan 出口：无计划/测试调用方直驱 run_model_loop_core 时传入，
/// 永不重规划。pub：bot_plan 模块未对集成测试公开，PlanState 在 tests/ 不可命名，
/// 以 fn item 形式传入绕开闭包参数类型标注问题。
pub async fn noop_replan(
    _plan: crate::bot_plan::PlanState,
    _reason: String,
) -> Option<Vec<String>> {
    None
}

/// 可注入的 LLM HTTP 连接参数：
/// base_url/client/api_key/model 由调用方注入，run_model_loop_core 不在函数体内
/// 经 AppHandle 取配置——集成测试可指向 tests/mock_llm.rs 的 mock server 跑真路径。
pub struct LlmHttp {
    pub client: reqwest::Client,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// API 协议：Openai = /chat/completions + Bearer；
    /// Anthropic = /v1/messages + x-api-key + anthropic-version（转换在 bot_anthropic）
    pub provider: crate::bot::ApiProvider,
    /// max_tokens（仅 Anthropic 模式发送——Anthropic 必填；OpenAI 兼容模式不发，
    /// 多数兼容网关不认识该字段）。装配时已 resolve_max_tokens 钳制过；
    /// 条目级覆盖也在此值里生效，thinking budget 夹紧用同一个值。
    pub max_tokens: u32,
    /// 推理强度线上参数（RE-1）：按模型族映射好的注入载荷
    /// （bot/reasoning::resolve），None = 不发任何推理字段。
    pub reasoning: crate::bot::reasoning::ReasoningWire,
    /// 条目级采样参数：active 模型条目的 temperature/top_p 覆盖，
    /// OpenAI 与 Anthropic 请求体都注入；None = 不写字段（用模型服务端默认）。
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    /// 条目级 system prompt：Some = 追加到消息栈末尾的额外 system 消息
    ///（内部消息流保持 OpenAI 形状；Anthropic 分支转换时归并进顶层 system）。
    pub system_prompt: Option<String>,
}

/// 条目级采样参数注入：temperature/top_p 有值才写请求体（有值才写 =
/// 不打扰不认识这些字段或用服务端默认的模型）。OpenAI /chat/completions 与
/// Anthropic /v1/messages 的顶层字段同名，一处实现两协议共用；
/// 摘要（summarize_http）与 Planner（call_planner）也走这里。
pub fn apply_inference_params(
    body: &mut serde_json::Value,
    temperature: Option<f64>,
    top_p: Option<f64>,
) {
    if let Some(t) = temperature {
        body["temperature"] = serde_json::json!(t);
    }
    if let Some(p) = top_p {
        body["top_p"] = serde_json::json!(p);
    }
}

/// 按协议构造请求体。
///
/// 内部消息流恒为 OpenAI 形状；Anthropic 分支在这里转成块结构（system 段、
/// tool_use / tool_result 块都在这一步成形）。**降级重发必须整体重建**：
/// 把 OpenAI 形状的 msgs 直接回填进 Anthropic 的 body，`role=system` /
/// `role=tool` 都不被 Anthropic 接受 → 必 400；而那个 400 文案不再像超长，
/// 用户连「请 /compact」的指引都拿不到。
fn build_request_body(
    http: &LlmHttp,
    msgs: &[serde_json::Value],
    tools: &serde_json::Value,
    openai_usage_supported: bool,
) -> Result<serde_json::Value, String> {
    let mut body = match http.provider {
        crate::bot::ApiProvider::Openai => {
            let mut b = serde_json::json!({
                "model": http.model,
                // Anthropic 的 thinking 块载体是 provider 专属键，OpenAI 端点
                // 不认（部分网关对消息里的未知字段直接 400）→ 发前剥掉
                "messages": strip_thinking_key(msgs),
                "tools": tools,
                "stream": true
            });
            // 流式 usage 需显式索取：末块才带 `usage`（含
            // prompt_tokens_details.cached_tokens），否则 OpenAI 协议下
            // 缓存命中永远记不到。部分中转网关不认这个字段会 400，
            // 届时 usage_supported 翻 false 并原样重发一次（见非成功分支）。
            if openai_usage_supported {
                b["stream_options"] = serde_json::json!({"include_usage": true});
            }
            b
        }
        crate::bot::ApiProvider::Anthropic => crate::bot_anthropic::build_anthropic_body(
            &http.model,
            msgs,
            tools,
            http.max_tokens,
            true,
        )?,
    };
    // 条目级采样参数注入：temperature/top_p 有值才写（两协议顶层字段同名）
    apply_inference_params(&mut body, http.temperature, http.top_p);
    // 推理强度注入（RE-1）：OpenAI 兼容分支直接加字段；
    // Anthropic 分支加 thinking 块（budget < max_tokens 约束已在 resolve 时夹紧）。
    match &http.reasoning {
        crate::bot::reasoning::ReasoningWire::OpenAiEffort(effort) => {
            body["reasoning_effort"] = serde_json::json!(effort);
        }
        crate::bot::reasoning::ReasoningWire::GlmThinking(on) => {
            body["thinking"] = serde_json::json!({
                "type": if *on { "enabled" } else { "disabled" }
            });
        }
        crate::bot::reasoning::ReasoningWire::AnthropicBudget(budget) => {
            if http.provider == crate::bot::ApiProvider::Anthropic {
                crate::bot_anthropic::apply_anthropic_thinking(&mut body, *budget);
            }
        }
        crate::bot::reasoning::ReasoningWire::None => {}
    }
    Ok(body)
}

/// run_model_loop_core 的同步副作用出口：
/// 流式事件 / 结构化审计 / bot.log / Skill 收尾 4 个出口抽成注入回调，
/// 使核心循环不依赖 AppHandle；异步出口（execute_tool / replan）因 Rust 闭包生命周期
/// 限制走泛型参数。生产薄壳 run_model_loop 传入 AppHandle 实现，测试传 stub。
pub struct ModelLoopDeps<'a> {
    /// 单次请求 Function 调用熔断上限覆盖：薄壳从 bot-config
    /// `maxFunctionCalls` 读入；None = 默认 100。子 agent 预算优先级更高
    /// （core 派生链第一级），测试直驱 core 传 Some(n) 可锚定任意 cap。
    pub fuse_cap_override: Option<usize>,
    /// 流式事件出口（bot-chat-delta / bot-think-delta / bot-tool / bot-tool-name / bot-tool-done）
    pub emit: &'a (dyn Fn(&str, serde_json::Value) + Send + Sync),
    /// 结构化审计事件（audit_event! 等价物：level + event + kv 列表）
    pub audit: &'a (dyn Fn(crate::audit::AuditLevel, &'static str, Vec<(&'static str, String)>)
             + Send
             + Sync),
    /// bot.log 文本行审计（bot::audit_log 等价物）
    pub audit_log: &'a (dyn Fn(&str) + Send + Sync),
    /// Skill 收尾（bot_skills::skill_finish 等价物；返回附加提示文本）
    pub skill_finish: &'a (dyn Fn(bool, &str) -> String + Send + Sync),
    /// 读「本会话活动技能快照」（`bot_skills::active_skill_run_for` 等价物）。
    ///
    /// 阶段 3.3 定的口径：核心**只读**技能状态（判断是否短路），拿到的是 clone 快照，
    /// 结构上无法改生命周期；写操作（收尾 / 暂停 / 入口清理）留在核心之外或已有回调里。
    pub active_skill_run:
        &'a (dyn Fn(Option<&str>) -> Option<crate::bot_skills::SkillRun> + Send + Sync),
    /// 上下文压缩 sidecar 落盘目录（AI_Gen_Files/conv-history）。
    /// 薄壳从 db::gen_dir(app) 派生；None = 不写盘（压缩文本留"原文未保留"标记）。
    /// 核心不依赖 AppHandle，路径借用由薄壳层保证生命周期。
    pub conv_sidecar_dir: Option<&'a std::path::Path>,
    /// 终稿自检轮次间隔（可选）：None = 默认 8 轮；0 = 关闭 final 自检。
    /// 薄壳从 bot-config `finalSelfcheckRoundGap` 读入。core 据此判断是否在主
    /// 出口前置触发 LLM 自检（FINAL_SELFCHECK_PROMPT）。
    pub final_selfcheck_round_gap: Option<u32>,
    /// 终稿自检侧调模型连接（可选）：Some = 自检走独立小请求（只带 snippet，
    /// 省主循环整段历史重付），core 在触发点直调并在主出口前拿到 verdict；
    /// None = 现状（注入 prompt 主循环内多跑一轮）。薄壳从 bot-config
    /// `selfcheckModelId` 解析模型库条目装配，解析失败回落 None 并审计。
    pub final_selfcheck_http: Option<&'a LlmHttp>,
    /// 轮数自报出口：core 每跑完一轮调一次，回传 1-based 已完成轮数。
    ///
    /// 存在的理由是「轮」必须由循环自己数，不能由薄壳数 `llm.request` 事件——
    /// 一轮里可能重试、可能撞 max_tokens 续写、可能补终稿自检，数事件必然偏大。
    pub on_round: &'a (dyn Fn(usize) + Send + Sync),
    /// 回合状态变化出口（见 emit_turn_status）。传 None = 不发（测试直驱 core）。
    pub turn_status: Option<&'a (dyn Fn(TurnStatus, &str) + Send + Sync)>,
    /// 子 agent 墙钟预算（软墙钟 + 墙钟熔断留产）。None = 不带墙钟
    /// （主聊天、任务执行等链路不受墙钟治理）。
    pub wall_budget: Option<WallBudget>,
    /// 本会话调用过的工具名（分「调用过」与「成功过」两档）——幻觉守卫「点名
    /// 判据」的排除集。
    ///
    /// 入参是当前会话 id。跨提问的重述是真话（「刚才 create_word 生成的那个」），
    /// 所以排除集要覆盖本会话历史，不止本次提问。
    ///
    /// 返回 None = 查不到（薄壳没这个注入点 / 查库失败）→ 点名相关的判据整档
    /// 让开（查不到证据不等于有罪）。Some(空) = 查到了，这个会话确实没调过
    /// 任何工具。
    pub session_tools:
        Option<&'a (dyn Fn(Option<&str>) -> Option<crate::db::trace::SessionTools> + Send + Sync)>,
    /// 幻觉守卫「执行类谎报」触发词表（设置页「工具调用触发器关键词」可编辑）。
    ///
    /// 薄壳已把「没配过」回落成内置默认表再传进来，core 不再关心来源——
    /// 空切片 = 用户显式清空，该档触发器失效（变更类与路径存在性两道闸不受影响）。
    pub guard_claim_keywords: &'a [String],
    /// 落盘凭证查询（幻觉守卫的「路径是工具产物吗」那道判据）。
    ///
    /// 入参是（会话 id，候选路径），返回「查不到落盘凭证」的那部分。闭包在薄壳
    /// 侧装配——那里有 AppHandle，能开库；core 保持可注入 mock（测试直驱 core
    /// 时传 None = 不查凭证，路径存在性那道闸照旧生效）。
    pub file_receipts: Option<&'a (dyn Fn(Option<&str>, &[String]) -> Vec<String> + Send + Sync)>,
}

/// 回合状态机。借鉴 ZCode 的 turn view model（`running` / `settling` /
/// `completed` / `interrupted` / `failed`），核心是 `settling`：
/// **正文已输出完、但收尾工作（终稿自检 / 熔断收尾）还没结束**。
/// 没有这个态，前端只能把「正文说完」当「本轮结束」，于是最后一段工具执行
/// （可静默 300s+）在 UI 上等同于静止。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    /// 本轮开始（含后续每轮开始）
    Running,
    /// 正文输出完、收尾中：仍可停止，UI 必须继续显示活动指示
    Settling,
    /// 正常结束
    Completed,
    /// 用户 /stop 中断
    Interrupted,
    /// 异常结束（模型循环报错）
    Failed,
}

impl TurnStatus {
    /// 机器读标识（前端 switch 用，不应改文案）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Settling => "settling",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

/// 模型工具循环薄壳：只做 AppHandle 依赖装配——
/// 读配置/Key、构 HTTP 客户端、把事件 emit / 审计 / skill_finish / execute_tool /
/// replan 包成回调，实际循环逻辑全在 run_model_loop_core（可注入 mock server 集成测试）。
/// 全局共享 LLM HTTP 客户端（OnceLock 连接池复用）——每轮模型循环新建
/// Client 会各建连接池/TLS 握手，且旧 Client drop 后连接粗暴关闭。**无默认总
/// 超时**：流式路径逐 chunk idle 超时（stream_chunk_idle_timeout），非流式调用方
/// （摘要/Planner）在 RequestBuilder 上挂 per-request timeout。
pub fn shared_llm_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("共享 LLM 客户端构建失败（builder 配置静态合法，失败即 bug）")
    })
}

/// 流式逐 chunk idle 超时：对端持续吐 delta，两 chunk 间隔超过此值即判定死连接
///（旧实现的 300s **总**超时会掐掉合法长生成——长正文/长工具参数总时长可超 5 分钟）
pub const STREAM_CHUNK_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// send() 响应头超时：只覆盖连接+发请求+收到响应头，
/// 不含流式 body——服务端收下请求却挂起不回时兜底，防永久挂起
pub const LLM_HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 模型循环执行明细（trace 管道接线，trace.rs:110 既定计划的落地）。
/// 轮数由循环按真实轮次自报（`ModelLoopDeps::on_round`），不是数 `llm.request`
/// 事件——一轮里可能发多次请求（网络重试 / 撞 max_tokens 自动续写 / 终稿自检），
/// 数事件会把「轮」算大，让上报值与真正约束循环的 max_rounds 脱钩。工具摘要由薄壳的
/// execute_tool 包装器采集——真实分发路径零改动。
#[derive(Debug, Clone, Default)]
pub struct LoopTrace {
    /// 真实循环轮数（run_model_loop_core 自报，含熔断那轮）
    pub turn_count: u32,
    /// 本轮工具调用次数（exec_traces.tool_calls 列；逐次明细在 trace_steps 表，
    /// 由 execute_tool_traced 的 span 写入，不在此重复采集）
    pub tool_calls: u32,
    /// llm.usage 审计事件累计（Anthropic 协议现发；OpenAI usage 解析留 ）
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// 缓存读取 / 写入 token（Anthropic 四档计价之二）。
    ///
    /// 独立于 prompt_tokens 落库：cache read 是 0.1x、cache write 是 1.25x，
    /// 混进 input 等于把长会话的缓存收益算没——一次长对话里 cache_read 常数倍
    /// 于新增 input，漏记会让成本统计明显偏低。OpenAI 协议恒为 0。
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// 本轮实际使用的模型名（词元统计按模型聚合用；未发请求即失败时为 None）
    pub model: Option<String>,
    /// 本轮发出的 LLM 请求次数（含重试与续写）。
    /// 与 `turn_count` 分开存：两者不等是常态，请求数才是成本口径。
    pub request_count: u32,
}

/// 证据池里单次调用保留的返回文本上限（字符）。
///
/// 工具返回本身已被回灌上限钳过（默认 30K 字节），这里再钳一层是防极端值把
/// 内存吃光。截掉的部分模型多半也没看见——真截在中间的数判成「无出处」是
/// 误伤方向，宁可放过。
const EVIDENCE_OUTPUT_CHARS: usize = 64 * 1024;

/// 一次工具调用的证据：参数与返回。
///
/// 凭证池记的是「哪个工具成功过」——存在性。要判「模型报出来的这个数是哪来的」
/// 「这次调用到底替模型核实了什么」，得留着返回内容与参数本身：那是另一根轴。
#[derive(Debug, Clone)]
struct ToolEvidence {
    /// 这次调用成功没有（失败与成功的返回都是模型看得见的内容，出处比对都要算）
    ok: bool,
    /// 调用的参数（原始 JSON 串）
    args: String,
    /// 工具返回的文本（模型看到的那一份）
    output: String,
}

/// 消息里的文本内容（字符串，或多模态数组里的 text 段）。
fn msg_text(msg: &serde_json::Value) -> Option<String> {
    match msg.get("content") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Array(parts)) => {
            let mut buf = String::new();
            for p in parts {
                if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(t);
                    buf.push('\n');
                }
            }
            if buf.is_empty() {
                None
            } else {
                Some(buf)
            }
        }
        _ => None,
    }
}

/// 对话里模型见过的「非模型自产」内容：用户消息 + 工具消息。
///
/// assistant 消息不算出处——模型自己上一轮说过的数字不是证据，它说的话不能
/// 给自己作证。跨提问的旧工具消息算：那是上一轮真的跑出来的。
fn context_sources(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter(|m| {
            matches!(
                m.get("role").and_then(|r| r.as_str()),
                Some("user") | Some("tool")
            )
        })
        .filter_map(msg_text)
        .collect()
}

/// 数值在出处里找得到吗。
///
/// 三条路算「找得到」：字面命中（去千分位逗号后子串命中）、四舍五入等价
/// （工具给 1234.56、模型报 1235）、以及出处里本来就有这个数。
/// 四舍五入那条例外是给「工具给的是原值、模型报的是它自己抄下来的近似值」
/// 留的——那种是如实转述，判成编造就是误伤。
fn number_sourced(value: &str, sources: &[String]) -> bool {
    let needle: String = value.chars().filter(|c| *c != ',').collect();
    if needle.is_empty() {
        return true;
    }
    let decimals = needle.split_once('.').map(|(_, d)| d.len()).unwrap_or(0);
    // 解析不过的不是数值，按「比不出来」处理（当前只有数字会走到这儿，
    // 但别让以后的形态悄悄进来当成有出处）。
    if needle.parse::<f64>().is_err() {
        return false;
    }
    for s in sources {
        if s.replace(',', "").contains(&needle) {
            return true;
        }
    }
    for s in sources {
        for n in crate::bot::registry::numbers_in_text(s) {
            if format!("{n:.decimals$}") == needle {
                return true;
            }
        }
    }
    false
}

/// 主循环推理参数选取：override 命中必须取覆盖条目的（resolve_model_override 已按
/// 条目值 > 全局 max_tokens 兜底算好）；未命中才按 active 条目解析（active 条目 >
/// 全局 > 内置默认，见 schema::effective_inference）。两路不可混用——覆盖端点打
/// active 条目的 temperature/top_p/system_prompt 是参数张冠李戴。
fn resolve_loop_inference(
    resolved: Option<&crate::bot::config::ResolvedModel>,
    api_provider: Option<&str>,
    max_tokens: Option<u32>,
    active_model_id: Option<&crate::bot::ActiveModelId>,
    models_by_provider: Option<&crate::bot::ModelsByProvider>,
) -> crate::bot::EffectiveInference {
    resolved.map(|r| r.inference.clone()).unwrap_or_else(|| {
        crate::bot::effective_inference(
            api_provider,
            max_tokens,
            active_model_id,
            models_by_provider,
        )
    })
}

/// 采样温度的优先级：**配置档 > 模型条目**。
///
/// 与「权限只收紧、运行时优先」那条相反且是刻意的——采样温度不是权限，
/// 放宽收紧都不构成安全问题；用户在设置页给模型配了 temperature=0.9 又写了
/// 一份 `temperature: 0` 的配置档，他要的是后者（更具体的意图）。
///
/// 独立成函数而不是内联 `.or()`：内联的话优先级方向只能靠读代码确认，
/// 测试只能去扫源码——而**测试就住在同一个文件里**，裸写字面量会自匹配，
/// 断言会在实现被改反时照样通过（本守卫第一版就是这么假绿的）。
pub fn resolve_temperature(config: Option<f64>, entry: Option<f64>) -> Option<f64> {
    config.or(entry)
}

/// 终稿自检侧调条目的放行判定
///
/// 侧调是**真实带 key 的请求**：条目所属厂商没过连接测试时，主请求被 vendor 门
/// 拦下，侧调请求却照发——等于把 key 发往未验证的 base_url。所以侧调过同一道门。
/// 拦下时回落主路径内的自检（不阻断主流程）+ 审计。
enum SelfcheckDecision {
    /// 放行：条目解析成功且厂商已验证
    Use(crate::bot::config::ResolvedModel),
    /// 不放行：条目解析失败（不存在 / 已停用 / 拿不到 key）
    ResolveFailed(String),
    /// 不放行：厂商没过连接测试（回落主路径内的自检）
    VendorNotVerified { vendor: String, reason: String },
}

/// 侧调条目的放行判定（纯判定，可单测）
fn decide_selfcheck(
    resolved: Result<crate::bot::config::ResolvedModel, crate::error::CommandError>,
    verified_vendors: &[String],
) -> SelfcheckDecision {
    match resolved {
        Err(e) => SelfcheckDecision::ResolveFailed(e.to_string()),
        Ok(r) => match crate::bot::ensure_vendor_verified(r.vendor.as_deref(), verified_vendors) {
            Ok(()) => SelfcheckDecision::Use(r),
            Err(reason) => SelfcheckDecision::VendorNotVerified {
                vendor: r.vendor.clone().unwrap_or_default(),
                reason,
            },
        },
    }
}

// 主循环本体：入口装配（deps / 出口分流）+ 轮次循环 + 每轮七类钩子接线。
// 长度本身就是这些接线堆在一起的结果，拆函数会把「钩子相对答案定稿的位置」
// 这条不变量摊散到多个文件里——而那正是上面 post_turn_layer_tests 要守的。
#[allow(clippy::too_many_lines)]
pub async fn run_model_loop(
    app: AppHandle,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
    // 推理强度单次覆盖（RE-1）：聊天 bot_chat 传入具体档位（off/low/medium/high）；
    // None = 任务执行/定时等后台链路，回落 bot-config.json 的全局默认。
    // 抽象档位 → 线上参数的按模型映射在 bot/reasoning.rs。
    reasoning_override: Option<String>,
    // 每卡模型覆盖：模型库条目 id；None = 跟随全局 active 模型。
    // 解析失败（条目不存在/禁用/key 缺失）→ 响亮报错，不静默回落
    model_override: Option<String>,
    // per-onion 配置档的采样温度（子 agent 用；None = 用模型条目的 temperature）。
    //
    // **配置档压过条目级**，理由见 `resolve_temperature`。刻意只传 temperature
    // 不传 top_p：yaml 没这个字段，传了也只会多一条没有来源的覆盖路径。
    temperature_override: Option<f64>,
    // 失败路径的执行明细出口（可选）：`Err` 时把已跑出的轮数/工具数/tokens
    // 交回调用方。返回值里的 LoopTrace 只在 Ok 时才有，失败轨迹（熔断历史路径、
    // API 报错、流式中断）会整段丢掉 stats，trace 于是记成 turns=0——
    // 实测一次跑满 50 轮撞熔断的 255 秒执行，trace 里 turns=0/tools=0/tokens=0。
    // 想要失败轨迹也落库的调用方（trace 管道）传 Some 即可。
    failed_trace_out: Option<&std::sync::Arc<std::sync::Mutex<Option<LoopTrace>>>>,
    // 子 agent 墙钟预算（软墙钟 + 墙钟熔断留产）。None = 不带墙钟治理
    // （主聊天、任务执行等链路不参与子 agent 预算）。
    wall_budget: Option<WallBudget>,
) -> Result<(String, Vec<TaskRef>, LoopTrace), CommandError> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    // 每卡模型覆盖优先：命中模型库条目则整组替换 base_url/model/协议/key
    let resolved = match model_override.as_deref() {
        Some(id) => Some(crate::bot::resolve_model_override(
            cfg.models_by_provider.as_ref(),
            id,
            cfg.max_tokens,
        )?),
        None => None,
    };
    // 请求装配前厂商可用性守卫：实际命中条目（override 命中条目 / 全局 active
    // 条目）带厂商且未通过连接测试 → 响亮拦截，不带未验证 key 出门换 401。
    // 「设为当前使用」入口已挡未验证厂商，这里兜存量坏指针 / 测试失败撤绿 /
    // Base URL 改动失效三种来源；聊天与任务执行（🤖/⏰/📦/逐步）共用本循环，
    // 一处拦截全链路生效
    let serving_vendor = match &resolved {
        Some(r) => r.vendor.clone(),
        None => crate::bot::active_model_entry(
            cfg.api_provider.as_deref(),
            cfg.active_model_id.as_ref(),
            cfg.models_by_provider.as_ref(),
        )
        .and_then(|e| e.vendor.clone()),
    };
    crate::bot::ensure_vendor_verified(serving_vendor.as_deref(), &cfg.verified_vendors)?;
    let api_key = match &resolved {
        Some(r) => r.api_key.clone(),
        None => crate::bot::read_llm_key(
            cfg.api_provider.as_deref(),
            cfg.active_model_id.as_ref(),
            cfg.models_by_provider.as_ref(),
        )?,
    };
    if api_key.trim().is_empty() {
        return Err(CommandError::ApiKeyMissing);
    }
    // 共享客户端（连接池复用）；总超时取消，流式路径逐 chunk idle 超时兜底
    let client = shared_llm_client().clone();
    // 推理参数族映射按实际请求端点：override 整组替换后走条目的 model/provider，
    // 仍按全局 cfg 解析会把 OpenAI 风格 reasoning_effort 发去 Anthropic 端点（或反之）
    let model_for_reasoning = resolved
        .as_ref()
        .map(|r| r.model.clone())
        .unwrap_or_else(|| cfg.model.clone());
    // None/非法值 → Openai（旧行为零影响）
    let provider = crate::bot::ApiProvider::from_cfg(cfg.api_provider.as_deref());
    let provider_for_reasoning = resolved
        .as_ref()
        .map(|r| crate::bot::ApiProvider::from_cfg(Some(r.api_provider.as_str())))
        .unwrap_or(provider);
    // 条目级推理参数：override 命中取覆盖条目的，未命中按 active 条目解析
    // （cfg 是对外视图 BotConfigView，拆参传入与 keyring::read_llm_key 同风格）
    let inference = resolve_loop_inference(
        resolved.as_ref(),
        cfg.api_provider.as_deref(),
        cfg.max_tokens,
        cfg.active_model_id.as_ref(),
        cfg.models_by_provider.as_ref(),
    );
    // 终稿自检侧调模型槽位：cfg 给了模型库条目 id 就整组解析（base_url/
    // model/协议/key）；解析失败 → 响亮审计 + 回落主循环内自检，不阻断。
    // 必须在主 http 构造前完成——那里会 move cfg 的字段。
    let selfcheck_http = cfg.selfcheck_model_id.as_ref().and_then(|entry_id| {
        let resolved = crate::bot::resolve_model_override(
            cfg.models_by_provider.as_ref(),
            entry_id,
            cfg.max_tokens,
        );
        match decide_selfcheck(resolved, &cfg.verified_vendors) {
            SelfcheckDecision::Use(r) => Some(LlmHttp {
                client: shared_llm_client().clone(),
                base_url: r.base_url,
                api_key: r.api_key,
                model: r.model,
                provider: crate::bot::ApiProvider::from_cfg(Some(r.api_provider.as_str())),
                // 判定 prompt 要求「仅回答 Y/N」——32 token 足够首字符判定
                max_tokens: 32,
                reasoning: crate::bot::reasoning::ReasoningWire::None,
                temperature: None,
                top_p: None,
                system_prompt: None,
            }),
            SelfcheckDecision::ResolveFailed(e) => {
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Warn,
                    "final_selfcheck.model_resolve_failed",
                    &[("entry", entry_id.clone()), ("err", e)],
                );
                None
            }
            SelfcheckDecision::VendorNotVerified { vendor, reason } => {
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Warn,
                    "final_selfcheck.vendor_not_verified",
                    &[
                        ("entry", entry_id.clone()),
                        ("vendor", vendor),
                        ("err", reason),
                    ],
                );
                None
            }
        }
    });
    let http = LlmHttp {
        client,
        base_url: resolved
            .as_ref()
            .map(|r| r.base_url.clone())
            .unwrap_or(cfg.base_url),
        api_key,
        model: resolved
            .as_ref()
            .map(|r| r.model.clone())
            .unwrap_or(cfg.model),
        provider: provider_for_reasoning,
        max_tokens: inference.max_tokens,
        temperature: resolve_temperature(temperature_override, inference.temperature),
        top_p: inference.top_p,
        system_prompt: inference.system_prompt,
        // 推理强度：覆盖优先于后台默认；抽象档位按模型族映射到线上参数。
        // budget < max_tokens 夹紧用的就是条目覆盖后的 max_tokens
        reasoning: crate::bot::reasoning::resolve(
            provider_for_reasoning,
            &model_for_reasoning,
            crate::bot::reasoning::EffortLevel::from_cfg(
                reasoning_override
                    .as_deref()
                    .or(cfg.reasoning_effort.as_deref()),
            ),
            inference.max_tokens,
        ),
    };
    // 会话隔离：流式事件（bot-chat-delta 等）只由交互实例广播；
    // 后台定时任务（interactive=false）不发流——否则后台执行的输出会
    // 串进用户当前会话的 streaming 气泡。Skill 归属同理按 session 过滤。
    let stream_to_main = stop.is_interactive();
    let session_id: Option<&str> = stop.session_id();
    // 在途登记：这一轮开始。客户端重挂载后靠它知道「有回合正在跑」——
    // 没有登记，客户端根本不会去问回放，在途回复就只剩最终正文。
    // 位置约束：这段之后到 core 返回之间没有早退路径（配置 / key 校验都在更早），
    // 所以收尾只挂在 core 返回那一处；往后若在这段里加早退，必须一并收尾。
    if stream_to_main {
        if let Some(sid) = session_id {
            crate::bot::stream_log::begin(sid);
        }
    }
    // 流式事件出口统一收口（会话隔离）：非交互实例（后台定时任务）
    // 不发任何流式增量，防后台执行输出串进用户当前会话的 streaming 气泡。
    // payload 统一注入 sessionId，前端按当前会话过滤——
    // 事件不带会话标记时，两个会话并行跑 A 的流式增量会串进 B 正在显示的气泡。
    let emit = |event: &str, mut payload: serde_json::Value| {
        if stream_to_main {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("sessionId".into(), serde_json::json!(session_id));
            }
            // 走统一出口：分配序号 + 写会话缓冲 + 广播。
            // 序号让客户端能按 seq 补齐（补齐与实时可能重叠，靠序号去重）。
            crate::bot::stream_log::emit_recorded(&app, session_id, event, payload);
        }
    };
    // 执行明细采集（薄壳层，真实分发路径零改动）——
    // 轮数：循环自报真实轮次（rounds_used，每轮末置位）；请求数：audit 闭包数
    // llm.request 事件。两者口径不同，必须分开存——见 LoopTrace 字段注释。
    // （声明在 deps 之前：deps 的 audit 闭包要引用请求数计数器）
    let rounds_used = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let request_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let request_for_audit = request_count.clone();
    // 缓存命中/写入累计（仅 Anthropic 协议会变；OpenAI 协议保持 0）。
    // 前端水位条按「读/写」两档单列展示，不折进输入输出，也不算命中率
    let prompt_tokens = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let completion_tokens = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let cache_read_tokens = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let cache_creation_tokens = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let usage_pt = prompt_tokens.clone();
    let usage_ct = completion_tokens.clone();
    let usage_cr = cache_read_tokens.clone();
    let usage_cc = cache_creation_tokens.clone();
    let tool_trace: std::sync::Arc<std::sync::Mutex<u32>> =
        std::sync::Arc::new(std::sync::Mutex::new(0));
    let tool_trace_for_exec = tool_trace.clone();
    // 上下文压缩 sidecar 目录（AI_Gen_Files/conv-history）；gen_dir 失败时降级 None
    let conv_sidecar_path = crate::db::gen_dir(&app)
        .ok()
        .map(|d| d.join("conv-history"));
    // 触发词表：cfg 是 View，「没配过 → 内置默认」的回落已在 bot_get_config
    // 组装时做完，这里拿到的就是当前生效表。剔空白项——盘上留个空串会让人
    // 以为它还在起作用，其实判定时永远不命中。
    let guard_claim_keywords: Vec<String> = cfg
        .guard_claim_keywords
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let deps = ModelLoopDeps {
        // 全域熔断上限：config 有值用之，None = 默认 100。这里**不钳**——
        // 钳位单点在下方派生链末端（fuse_cap 的 clamp），此处再钳一次等于把
        // 同一条边界抄成两份，改一处漏一处不会编译失败也不会测试失败。
        fuse_cap_override: Some(
            cfg.max_function_calls
                .map(|v| v as usize)
                .unwrap_or(MAX_FUNCTION_CALLS_PER_REQUEST),
        ),
        conv_sidecar_dir: conv_sidecar_path.as_deref(),
        // 终稿自检轮次间隔：cfg 有值用之（已落盘前钳 0..=100），None = 默认 8。
        // 0 = 关闭 final 自检的 kill switch 直接透传。
        final_selfcheck_round_gap: cfg.final_selfcheck_round_gap,
        final_selfcheck_http: selfcheck_http.as_ref(),
        on_round: &|rounds| {
            rounds_used.store(rounds, std::sync::atomic::Ordering::Relaxed);
            // 子 agent 会话：同步写进 tool_guard 的轮数仪表，供 check_subagent 的
            // progress 报「已跑 N / 预算 M 轮」。主 agent 此前只能靠轮询猜
            // 子 agent 有没有在推进——一个跑满预算的子任务在 UI 上和刚启动的长得一样。
            if let Some(ctx) = crate::tool_guard::subagent_ctx(stop.session_id()) {
                ctx.used_rounds
                    .store(rounds, std::sync::atomic::Ordering::Relaxed);
            }
        },
        turn_status: Some(&|status, detail| {
            emit(
                "bot-turn-status",
                serde_json::json!({ "status": status.as_str(), "detail": detail }),
            );
        }),
        wall_budget,
        guard_claim_keywords: &guard_claim_keywords,
        // 本会话调用过的工具名（点名判据的排除集）：查库失败返 None
        // （= 查不到 → core 侧整档让开，查不到证据不等于有罪）。
        session_tools: Some(&|sid: Option<&str>| {
            let sid = sid?;
            let conn = match crate::db::open_db(&app) {
                Ok(c) => c,
                Err(e) => {
                    crate::audit::write_event(
                        &app,
                        crate::audit::AuditLevel::Warn,
                        "bot.session_tools_open_fail",
                        &[("error", e)],
                    );
                    return None;
                }
            };
            match crate::db::trace::session_tool_names(&conn, sid) {
                Ok(names) => Some(names),
                Err(e) => {
                    crate::audit::write_event(
                        &app,
                        crate::audit::AuditLevel::Warn,
                        "bot.session_tools_query_fail",
                        &[("error", e)],
                    );
                    None
                }
            }
        }),
        // 落盘凭证查询：返回「查不到凭证」的候选路径。查库失败一律返回空
        // （= 全部放行）——查不到证据不等于有罪，宁可漏判不可误伤。
        file_receipts: Some(&|sid: Option<&str>, paths: &[String]| {
            // 没有会话 id 就不查：查不到证据不等于有罪，宁可漏判不可误伤。
            if sid.is_none() || paths.is_empty() {
                return Vec::new();
            }
            let conn = match crate::db::open_db(&app) {
                Ok(c) => c,
                Err(e) => {
                    crate::audit::write_event(
                        &app,
                        crate::audit::AuditLevel::Warn,
                        "bot.file_receipts_open_fail",
                        &[("error", e)],
                    );
                    return Vec::new();
                }
            };
            // 「本次提问跑过自己就能写盘的工具」这道让开在 core 侧判（提问级，
            // 不再是会话级）：一次 run_python 只让开那一轮，不是整个会话。
            let written = match crate::db::trace::file_paths_with_receipt(&conn, paths) {
                Ok(v) => v,
                Err(e) => {
                    crate::audit::write_event(
                        &app,
                        crate::audit::AuditLevel::Warn,
                        "bot.file_receipts_query_fail",
                        &[("error", e)],
                    );
                    return Vec::new();
                }
            };
            paths
                .iter()
                .filter(|p| !written.iter().any(|w| w.eq_ignore_ascii_case(p)))
                .cloned()
                .collect()
        }),
        emit: &emit,
        audit: &|level, event, kv| {
            // llm.request 事件计数 = 请求数（trace request_count 的数据源）。
            // **不是轮数**：一轮里可能重试、可能撞 max_tokens 续写、可能有终稿自检。
            if event == "llm.request" {
                request_for_audit.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            // llm.usage 累计（trace 汇总的 tokens 数据源）+  水位条事件
            //（仅交互实例推前端；前端按会话累计配合 contextK 显示水位。
            // 两协议都到这里：Anthropic 由 message_start/message_delta 带上，
            // OpenAI 由末块 usage 带上（请求侧带 stream_options.include_usage，
            // 网关不认时降级不统计，见 stream_options_unsupported 审计事件））
            if event == "llm.usage" {
                let mut input_opt: Option<u64> = None;
                let mut output_opt: Option<u64> = None;
                let mut cache_read_opt: Option<u64> = None;
                let mut cache_creation_opt: Option<u64> = None;
                for (k, v) in &kv {
                    match *k {
                        "input_tokens" => input_opt = v.parse::<u64>().ok(),
                        "output_tokens" => output_opt = v.parse::<u64>().ok(),
                        "cache_read_tokens" => cache_read_opt = v.parse::<u64>().ok(),
                        "cache_creation_tokens" => cache_creation_opt = v.parse::<u64>().ok(),
                        _ => {}
                    }
                }
                if let (Some(i), Some(o)) = (input_opt, output_opt) {
                    usage_pt.fetch_add(i, std::sync::atomic::Ordering::Relaxed);
                    usage_ct.fetch_add(o, std::sync::atomic::Ordering::Relaxed);
                    let cr = cache_read_opt.unwrap_or(0);
                    let cc = cache_creation_opt.unwrap_or(0);
                    if cr > 0 {
                        usage_cr.fetch_add(cr, std::sync::atomic::Ordering::Relaxed);
                    }
                    if cc > 0 {
                        usage_cc.fetch_add(cc, std::sync::atomic::Ordering::Relaxed);
                    }
                    if stream_to_main {
                        let _ = app.emit_to(
                            "main",
                            "bot-usage-delta",
                            serde_json::json!({
                                "sessionId": session_id,
                                "inputTokens": i,
                                "outputTokens": o,
                                "cacheReadTokens": cr,
                                "cacheCreationTokens": cc,
                            }),
                        );
                    }
                }
            }
            crate::audit::write_event(&app, level, event, &kv)
        },
        audit_log: &|line| crate::bot::audit_log(&app, line),
        skill_finish: &|ok, reason| crate::bot_skills::skill_finish(&app, ok, reason, session_id),
        // 阶段 3.3（口径：只读走注入）：核心每轮要读「本会话活动技能快照」决定是否短路，
        // 但**不给它表**——写生命周期（收尾/暂停/清理）一律留在核心之外或既有回调里。
        active_skill_run: &|session_id| crate::bot_skills::active_skill_run_for(&app, session_id),
    };
    // 僵尸终态清理上移到薄壳（口径：写操作不该在模型循环核心）：
    // 上轮遗留的 Completed/Failed/Terminated run 会在第 0 轮被 advance 短路（agent 假死根因）。
    // 位置与原核心内调用等价——都发生在进入轮循环之前。
    crate::bot_skills::clear_terminal_skill_runs(&app, session_id);
    // 配置化工具结果上限（设计 §3.2 maxToolOutputBytes，字节口径）——None = 显式关闭。
    // 与 per-tool 预算取小后才是本次回灌的实际天花板，见 effective_output_cap。
    let tool_output_cap: Option<usize> = match cfg.max_tool_output_bytes {
        Some(0) => None,
        // 上限单源在 bot::params（与写侧钳制、设置页展示同一个数）
        Some(n) => Some(crate::bot::params::clamp_max_tool_output_bytes(n) as usize),
        None => Some(DEFAULT_TOOL_OUTPUT_CAP_BYTES),
    };
    let execute_tool = |name: String, args: String, trace: crate::bot::ToolCallTrace| {
        let app = app.clone();
        let sink = tool_trace_for_exec.clone();
        async move {
            let out = crate::bot::execute_tool_traced(&app, &name, &args, Some(stop), &trace).await;
            // 三层漏斗（owner 各不相同，缺一不可）：
            //   L1 per-tool 预算（ToolDef.max_output_bytes，最紧）——超了就把完整结果
            //      转存成溢出文件，回灌节选 + 指针；模型按需 read_text_file 分页读回。
            //   L2 本配置 cap（maxToolOutputBytes，默认 30K 字节）——兜住 per-tool 预算
            //      被调得比它大的那三个长输出工具。
            //   L3 db::SPAN_TEXT_MAX——在 trace 侧，只管存多少进 SQLite，不管这里。
            // trace span 仍存原文（钳在 L3），整轮回放的证据不因回灌节选而丢失。
            let mut text = out.text;
            if let Some(global_cap) = tool_output_cap {
                let cap = effective_output_cap(&name, global_cap);
                if text.len() > cap {
                    // 「超预算」审计事件在**截断发生的地方**发：阈值就是这次
                    // 真正用的 cap（同一个数）。此前这条事件在 dispatch 侧按
                    // per-tool 预算报，而截断用 min(per-tool, 全局 cap)——
                    // 全局 cap 更紧时（默认 30000 vs 长输出工具 65536）文本被
                    // 截了而审计不报，运维据此查超预算会漏掉全部由全局 cap
                    // 触发的截断；反过来审计报了也可能根本没截。
                    crate::audit::write_event(
                        &app,
                        crate::audit::AuditLevel::Info,
                        "tool.output.over_budget",
                        &[
                            ("tool", name.clone()),
                            ("output_bytes", text.len().to_string()),
                            ("max_bytes", cap.to_string()),
                        ],
                    );
                    text = spill_or_cap(&app, &name, text, cap);
                }
            }
            if let Ok(mut n) = sink.lock() {
                *n += 1;
            }
            // 截断只换 text，其余字段（refs/status/images/file_changes）原样透传
            crate::bot::registry::ToolResult {
                text,
                refs: out.refs,
                status: out.status,
                images: out.images,
                file_changes: out.file_changes,
            }
        }
    };
    let replan = |plan: crate::bot_plan::PlanState, reason: String| {
        let app = app.clone();
        async move { crate::bot_plan::replan(&app, &plan, &reason).await }
    };
    let out = run_model_loop_core(
        &http,
        msgs,
        max_rounds,
        stop,
        plan_state,
        &deps,
        execute_tool,
        replan,
    )
    .await;
    // 在途收尾：core 返回即回合结束（正常完成 / 熔断 / 停止 / API 报错都在这归一）。
    // 缓冲不立刻丢——刚跑完就重连的客户端还要补齐最后一段（保留期到了才清扫）。
    if stream_to_main {
        if let Some(sid) = session_id {
            crate::bot::stream_log::finish(sid);
        }
    }
    // 执行明细无论成败都要组装：熔断走 Ok 但 API 失败/流式中断走 Err，
    // 早先用 `.map()` 只在成功时填 LoopTrace，于是跑满 50 轮被熔断的那次
    // trace 落库 turns=0、tools=0、tokens=0（实测 255 秒的执行记成 0 轮）。
    // 计数器在闭包里一路累加，出错不丢——这里只是把它们读出来。
    let loop_trace = LoopTrace {
        turn_count: rounds_used.load(std::sync::atomic::Ordering::Relaxed) as u32,
        request_count: request_count.load(std::sync::atomic::Ordering::Relaxed) as u32,
        tool_calls: tool_trace.lock().map_or(0, |n| *n),
        prompt_tokens: prompt_tokens.load(std::sync::atomic::Ordering::Relaxed),
        completion_tokens: completion_tokens.load(std::sync::atomic::Ordering::Relaxed),
        cache_read_tokens: cache_read_tokens.load(std::sync::atomic::Ordering::Relaxed),
        cache_write_tokens: cache_creation_tokens.load(std::sync::atomic::Ordering::Relaxed),
        model: Some(http.model.clone()),
    };
    match out {
        Ok(v) => Ok((v.0, v.1, loop_trace)),
        Err(e) => {
            if let Some(slot) = failed_trace_out {
                if let Ok(mut g) = slot.lock() {
                    *g = Some(loop_trace);
                }
            }
            Err(e)
        }
    }
}

/// 流完整性收尾状态：替代旧 `saw_done_or_finish: bool`，区分具体收尾来源。
/// - Done：SSE 协议终结哨兵（`data: [DONE]`）
/// 模型工具循环核心：流式请求（思考拆分 + 工具折叠事件）、进程内执行工具。
/// 配置/Key/HTTP 客户端/副作用出口全部注入，不依赖 AppHandle。
/// msgs 需已含 system 消息；返回 (最终正文, 任务引用)。
/// 轮数上限由调用方传入：默认 DEFAULT_MAX_ROUNDS（50），多步 Skill 可自报 max_rounds 覆盖。
/// plan_state（PREVR 第 2 层）：复杂任务的动态计划；工具连续失败时触发
/// Replan（重规划剩余步骤，≤MAX_REPLANS 次）。None = 无计划自由循环。
#[allow(clippy::too_many_lines)]
pub async fn run_model_loop_core<X, XP, R, RP>(
    http: &LlmHttp,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
    deps: &ModelLoopDeps<'_>,
    execute_tool: X,
    replan: R,
) -> Result<(String, Vec<TaskRef>), CommandError>
where
    X: Fn(String, String, crate::bot::ToolCallTrace) -> XP,
    XP: std::future::Future<Output = crate::bot::registry::ToolResult>,
    R: Fn(crate::bot_plan::PlanState, String) -> RP,
    RP: std::future::Future<Output = Option<Vec<String>>>,
{
    // URL 按协议分支（OpenAI 走 /chat/completions，
    // Anthropic 走 /v1/messages，base_url 两种填法都归一化）
    let url = match http.provider {
        crate::bot::ApiProvider::Openai => {
            format!("{}/chat/completions", http.base_url.trim_end_matches('/'))
        }
        crate::bot::ApiProvider::Anthropic => {
            crate::bot_anthropic::anthropic_messages_url(&http.base_url)
        }
    };
    // 工具 schema 按会话选取——子 agent 会话给 profile 白名单
    //（递归双保险①：清单里无 spawn 等），其他会话默认全量；
    // 阶段 3：主 agent 清单尾部追加外部 MCP 工具（tools_json_with_mcp，
    // 子 agent 分支短路不受影响）。
    // 拼装结果含外部 MCP description（外部
    // 数据引入面），此处兜底改 fail-soft——解析失败记审计并回退「无 MCP 的
    // 静态清单」（内置工具清单不陪 MCP 动态段陪葬；静态 base 恒合法，
    // 末层空表只为防御到底），不再 panic 打断模型循环。
    let tools: serde_json::Value = {
        let assembled = crate::bot::registry::tools_json_with_mcp(stop.session_id());
        match serde_json::from_str(assembled.as_ref()) {
            Ok(v) => v,
            Err(e) => {
                (deps.audit)(
                    crate::audit::AuditLevel::Error,
                    "bot.tools_json_parse_fail",
                    vec![("err", e.to_string())],
                );
                serde_json::from_str(
                    crate::bot::registry::tools_json_for(stop.session_id()).as_ref(),
                )
                .unwrap_or_else(|_| serde_json::json!([]))
            }
        }
    };

    // 熔断上限按会话派生——子 agent 预算 max_tool_calls（子会话注册表），
    // 其余 = bot-config maxFunctionCalls（缺省 100）。软警阈值 = cap*7/10。
    // 钳位只在 fuse_cap_for 末端做一次（防巨值实质关闭熔断），上界取 params 的
    // MAX_FUNCTION_CALLS_CAP，与子 agent 预算解析同一个数。
    let fuse_cap = fuse_cap_for(
        stop.session_id()
            .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
            .map(|c| c.budget.max_tool_calls),
        deps.fuse_cap_override,
    );
    // 小 cap 收敛：cap≤9 时 7/10 会塌到 0（提示「已调用 0 个」失真）——钳到
    // [1, cap-1]；cap=1 时软警本就无意义（下一次调用即熔断），钳到 1 即可
    let soft_warn_at = (fuse_cap * 7 / 10).clamp(1, fuse_cap.saturating_sub(1).max(1));
    // 轮数软警告阈值：与 soft_warn_at 同口径（7/10，留 30% 轮次收尾）。
    // 轮数预算原先只有熔断、没有任何预警——子 agent 撞满 20 轮时往往一句收尾
    // 都没说（全程只发工具调用），产出在 Err 里全丢。给足提前量后模型还有
    // 几轮把已查到的东西写成收尾 JSON。钳位同 soft_warn_at：max_rounds 小到
    // 7/10 塌成 0 时不注入失真的「已用 0 轮」。
    let rounds_warn_at = (max_rounds * 7 / 10).clamp(1, max_rounds.saturating_sub(1).max(1));
    let mut msgs = msgs;
    // 条目级 system prompt：追加到消息栈末尾——OpenAI 直接多一条 system
    // 消息；Anthropic 分支 openai_msgs_to_anthropic 会把它归并进顶层 system 块。
    if let Some(sp) = &http.system_prompt {
        msgs.push(serde_json::json!({ "role": "system", "content": sp }));
    }
    let emit = deps.emit;
    let audit = deps.audit;
    let audit_log = deps.audit_log;
    let skill_finish = deps.skill_finish;
    let active_skill_run = deps.active_skill_run;
    let session_id: Option<&str> = stop.session_id();
    // 僵尸终态清理已上移到薄壳 `run_model_loop`（核心只读技能状态，
    // 写生命周期不在核心——原位置与现在等价，都在进入轮循环之前）
    // 最多 max_rounds 轮（工具循环），每轮流式输出；收到 tool_calls 则执行后把结果续进对话
    let mut collected_refs: Vec<TaskRef> = Vec::new();
    let mut function_calls_total: usize = 0;
    let mut soft_warn_sent: bool = false;
    // 轮数软警告同款：只发一次，发完不再打扰后续轮次
    let mut rounds_warn_sent: bool = false;
    // 软墙钟提醒已注入（只一次）：进入收尾预留窗口时提醒一次，不重复刷屏
    let mut wall_warn_sent: bool = false;
    // 防幻觉汇报守卫：本次提问是否实际执行过变更类工具（提问级一次置位、不按轮
    // 重置——后续正常汇报轮靠它放行，按轮重置会把改完再汇报的正常轮误判为幻觉）；
    // 补一轮机会（claim_retry）同样每次提问只用一次
    let mut mutation_done: bool = false;
    let mut claim_retry_used: bool = false;
    // 幻觉守卫第二次拦下时写这里：终稿末尾追加一条如实更正（见判决处）。
    // None = 本次提问没有出现「返工后仍编造」，终稿照常。
    let mut fabrication_notice: Option<String> = None;
    // 网关是否接受 `stream_options.include_usage`（OpenAI 流式 usage 的前置）。
    // 初值 true；收到 400 时翻 false 并原样重发一次，此后整场对话不再带该字段
    // ——探测成本只付一次，且失败只降级统计口径、不影响回复本身。
    let mut openai_usage_supported: bool = true;
    // Anthropic 的 thinking 块是否已被丢弃过一次（签名失效时的补救，见请求失败分支）。
    // 只补救一次：重发后仍报同样的错就是别的问题，不该无限重试。
    let mut anthropic_thinking_dropped: bool = false;
    // 执行类谎报守卫的凭证池：本次**用户提问**内所有工具调用的真实记录。
    //
    // 作用域是整条提问（= 一次 run_model_loop_core 调用 = exec_spans 的一条 trace_id），
    // 跨轮累积、跨轮不清零。两个边界都是被真实数据逼出来的：
    //
    // · 不跨轮清零 —— trace 53 的最终汇报（msg 720「执行完毕：模块不存在…」）是真话，
    //   但它的凭证在上一轮；按轮查会把真话判成幻觉，再让模型去重跑一个已完成的
    //   动作（delete_task / link_file_to_task 这类就是重复的破坏性操作）。
    // · 跨提问的成功记录另查 —— 下一条用户消息是新的 trace，这个池子自然清零；
    //   跨提问的重述（「刚才 create_word 生成的那个」）是真话，由会话级的成功
    //   记录兜住，见下面的 session_tools_cache。
    let mut receipts: Vec<String> = Vec::new();
    // 本提问里每次工具调用的「参数 + 返回」。
    //
    // 凭证池只记「哪个工具成功过」——那是存在性。要判「模型报出来的这个数是哪来的」
    // 「这次调用到底替模型核实了什么」，得留着返回内容本身：两者是不同的轴，
    // 凭证表答不了后者。
    let mut evidence: Vec<ToolEvidence> = Vec::new();
    // 本会话调用过的工具名（调用过 / 成功过两档）——点名判据的排除集，跨提问的
    // 重述是真话。惰性拉一次：判据要用它，但每轮拉一次库不值当。None = 还没拉。
    let mut session_tools_cache: Option<Option<crate::db::trace::SessionTools>> = None;
    // 本提问里被工具写过的路径（小写归一，与仓库既有的路径同一性口径一致）。
    //
    // 用途是幻觉守卫的「落盘凭证」那道判据：模型声称「已生成 /path/x.docx」，
    // 而这个路径从来不是工具写出来的——那是凭空声称。凭证在**进程内**攒，
    // 不等痕迹落库（本轮刚写的文件此刻还只在 trace sink 的队列里，查库查不到）。
    let mut written_paths: Vec<String> = Vec::new();
    // soft_warn 待注入标志：本轮 tool 响应全部回填后才真正 push（见循环内注释）
    let mut soft_warn_queued: bool = false;
    // PREVR 第 1 层：工具失败检测——同工具连续失败计数，
    // 第 1 次失败注入「换策略」提示；连续 2 次失败：有计划则 Replan，无计划则要求如实告知
    let mut last_failed_tool: Option<String> = None;
    let mut last_fail_reason: Option<String> = None;
    let mut consec_failures: usize = 0;
    // Replan 预算耗尽审计只记一次（耗尽后每轮连续失败仍会发生，不刷屏）
    let mut replan_exhausted_logged: bool = false;
    let mut fail_hint_queued: Option<String> = None;
    // 自检节流：每 5 轮最多一次；本轮 pending 待本轮 tool 响应回填后 push
    let mut last_selfcheck_round: Option<usize> = None;
    let mut pending_selfcheck: Option<(String, String)> = None;
    // 自检视觉过滤：pending 推入 prompt 后置 true，本轮 SSE 流式 emit 不输出 Y/N，
    // 改在 final_text 解析时统一 emit 视觉提示（✅ / ⚠️）
    let mut expecting_selfcheck = false;
    // 滚动降密度：每条 tool 消息的 (入栈轮次, msgs 下标)。msgs 循环内只追加
    // 不删（压缩原位改写），下标恒稳定——唯一例外是上下文超限降级砍历史，
    // 那条路径必须调 reindex_after_shed 同步左移，否则下标越界。
    let mut tool_msg_ages: Vec<(usize, usize)> = Vec::new();
    // 终稿自检视觉过滤：本轮已被注入 FINAL_SELFCHECK_PROMPT、下一轮 SSE 流式
    // emit 跳过 Y/N 字样（用户已在主出口前看到 final_text 自检运行属于机器内部
    // 信号，不该再叠一层 Y/N 字样）；终态在主出口解析后还原 final_text。
    let mut expecting_final_selfcheck = false;
    // 截断续跑已用次数（stop_reason=max_tokens 时自动接一轮，全会话共享配额）
    let mut length_continuations: usize = 0;
    // 被截断轮次的正文前缀：续跑轮只产出后半段，主出口拼接后才是一份完整答复
    let mut length_prefix = String::new();
    // 终稿自检 snapshot：触发时拍下 final_text 本体，下一轮 LLM 响应解析后
    // 据 verdict 还原（Y/N/附解释均回滚到原文，N 路径在末尾追加 ⚠️）
    let mut final_selfcheck_snapshot: Option<String> = None;
    // 终稿自检节流：与 last_selfcheck_round 同模式但独立计数——SELFCHECK 节流
    // 算的是「工具变更类自检」、final_selfcheck 算的是「终稿整体自检」，
    // 两条路径不互相抑制。
    let mut last_final_selfcheck_round: Option<usize> = None;
    // 出口闸追问次数：模型隔了几轮才把自检判定（纯 Y/N）送回来时，出口追着它
    // 要真实结果。次数有硬顶（MAX_LATE_VERDICT_RETRIES），否则模型只回 Y/N
    // 就能把回合拖成死循环。
    let mut late_verdict_retries: u8 = 0;
    let mut plan_state = plan_state;
    // 上轮 streamed 文本快照：
    // AwaitConfirm/Finish/Fail/Terminate 跳出主循环时，返回 user 已看到的文本
    let mut last_streamed = String::new();
    // 上下文超长降级次数（全循环共享）：超长时砍历史重发，至多两次
    let mut context_sheds: u8 = 0;
    // 回合状态统一收口（见 TurnStatus）。整段循环体跑在 `run_body` 这个 async
    // 块里，外层只负责「出口 → 状态」映射：
    //
    // 不这么做的理由——循环体内有 14 处 `return Ok(...)`（已停止 3 处、Skill
    // 状态机 5 处、单轮熔断 1 处、正文定稿 1 处、续写/自检若干），逐个补
    // 状态发射必漏一处，而漏掉的那处正好表现为「UI 提前显示完成」。
    // 内层跑完由外层统一判定出口类别，出口只有一处可漏。
    let mut body_outcome: Option<&'static str> = None;
    let body: Result<(String, Vec<TaskRef>), CommandError> = async {
    for round in 0..max_rounds {
        // 墙钟熔断（轮边界检查）：期限已到就在这里退出，**带上上一轮的正文**。
        //
        // 为什么不交给外层的 tokio::timeout：timeout 掐掉整条 future 时，
        // 正在跑的工具、已经说出口的正文、已落盘的产物全悬空——子 agent 跑
        // 了几十分钟的产出归零，父 agent 只能从零重烧。轮边界退出与轮数熔断
        // 同款：熔断是一种「设计好的落地」，不是失败。
        if let Some(wb) = deps.wall_budget {
            if wb.expired() {
                let hint = skill_finish(false, "墙钟预算超限");
                let kv: Vec<(&str, String)> = vec![
                    ("max_wall_seconds", wb.max_wall_seconds.to_string()),
                    ("round", round.to_string()),
                    ("partial_chars", last_streamed.chars().count().to_string()),
                ];
                audit(crate::audit::AuditLevel::Warn, "fuse_wall", kv);
                body_outcome = Some("fused");
                return Ok((
                    wall_fuse_message(&last_streamed, &hint, wb.max_wall_seconds),
                    collected_refs,
                ));
            }
        }
        // 保守 token 预算——仅 audit，不压缩。超阈值走 `loop.msgs.over_budget` 事件。
        // 默认阈 200K 字符（中文 ~1 token/字符；上限 ≈ 200K token 上下文）。
        // 本轮 msgs 总字符超阈值时记个 audit，不截断——便于后续评估是否要加主动压缩。
        let msgs_chars: usize = msgs
            .iter()
            .filter_map(|m| m["content"].as_str())
            .map(|s| s.chars().count())
            .sum();
        match crate::bot::compaction::classify_level(msgs_chars, MSGS_BUDGET_CHARS) {
            crate::bot::compaction::CompactionLevel::None => {}
            level @ (crate::bot::compaction::CompactionLevel::Soft
            | crate::bot::compaction::CompactionLevel::Hard) => {
                // 70%/85% 软/硬压缩：tool 消息按比例降密度，原文落 sidecar
                //（AI_Gen_Files/conv-history，见 deps 构造处）；目录不可得时
                // 压缩文本留「原文未保留」标记，那一条是真的丢原文
                let session_id_owned = stop.session_id().map(|s| s.to_string());
                let compact_fn = match level {
                    crate::bot::compaction::CompactionLevel::Soft => {
                        crate::bot::compaction::soft_compact
                    }
                    crate::bot::compaction::CompactionLevel::Hard => {
                        crate::bot::compaction::hard_compact
                    }
                    _ => unreachable!(),
                };
                match compact_fn(
                    &mut msgs,
                    deps.conv_sidecar_dir,
                    session_id_owned.as_deref(),
                ) {
                    Ok(report) => (deps.audit)(
                        crate::audit::AuditLevel::Info,
                        "loop.msgs.compacted",
                        vec![
                            (
                                "level",
                                match level {
                                    crate::bot::compaction::CompactionLevel::Soft => "soft",
                                    _ => "hard",
                                }
                                .to_string(),
                            ),
                            ("before", report.before_chars.to_string()),
                            ("after", report.after_chars.to_string()),
                            ("tools", report.tool_msgs_compacted.to_string()),
                        ],
                    ),
                    Err(e) => (deps.audit)(
                        crate::audit::AuditLevel::Warn,
                        "loop.msgs.compact_failed",
                        vec![("err", e.clone())],
                    ),
                }
            }
            crate::bot::compaction::CompactionLevel::Fuse => {
                // 100% 熔断：仅 audit，soft_warn_queued 由 70% soft_warn_at 路径另起一行
                (deps.audit)(
                    crate::audit::AuditLevel::Info,
                    "loop.msgs.over_budget",
                    vec![
                        ("chars", msgs_chars.to_string()),
                        ("max", MSGS_BUDGET_CHARS.to_string()),
                    ],
                );
            }
        }
        // 滚动降密度：老工具结果随轮龄降档。业界实证旧细节是噪音——SWE-agent
        // 只留最近 5 条观察比全历史高 3 分；Anthropic context editing 实测
        // token -84%、表现 +29%。70/85% 阈值压缩保留作预算兜底；⟦c:⟧ 标记
        // 防软压重复、硬压沿用指针。无消息到龄时零开销（只遍历下标表）。
        let session_id_owned = stop.session_id().map(|s| s.to_string());
        let mut age_soft = 0usize;
        let mut age_hard = 0usize;
        for (born_round, idx) in &tool_msg_ages {
            let age = round.saturating_sub(*born_round);
            if age < AGE_SOFT_ROUNDS {
                continue;
            }
            let hard = age >= AGE_HARD_ROUNDS;
            match crate::bot::compaction::compact_one(
                &mut msgs[*idx],
                hard,
                deps.conv_sidecar_dir,
                session_id_owned.as_deref(),
            ) {
                Ok((true, _)) => {
                    if hard {
                        age_hard += 1;
                    } else {
                        age_soft += 1;
                    }
                }
                Ok((false, _)) => {}
                Err(e) => (deps.audit)(
                    crate::audit::AuditLevel::Warn,
                    "loop.msgs.compact_failed",
                    vec![("err", e)],
                ),
            }
        }
        if age_soft + age_hard > 0 {
            (deps.audit)(
                crate::audit::AuditLevel::Info,
                "loop.msgs.age_compacted",
                vec![
                    ("round", round.to_string()),
                    ("soft", age_soft.to_string()),
                    ("hard", age_hard.to_string()),
                ],
            );
        }
        if stop.stopped() {
            let hint = skill_finish(false, "用户停止");
            return Ok((format!("⏹ 已停止{hint}"), collected_refs));
        }
        // 状态机推进决策：集中 Skill 推进逻辑
        // 未来横切关注点（审批/沙箱/上下文压缩）只动 advance_skill，主循环不重构
        if let Some(run) = active_skill_run(session_id) {
            use crate::bot_skills::{advance_skill, AdvanceAction};
            match advance_skill(&run, chrono::Utc::now().timestamp_millis()) {
                AdvanceAction::NoActive | AdvanceAction::Continue => {} // 继续本轮
                AdvanceAction::AwaitConfirm => {
                    // Skill 暂停等用户确认，跳出主循环等待 bot_confirm_response 唤起。
                    // 并发新消息在第 0 轮命中此分支时 last_streamed
                    // 是空串，用户会得到空白回复——空串时给一句可读提示
                    body_outcome = Some("await_confirm");
                    if last_streamed.is_empty() {
                        return Ok((
                            "⏸ 上一个操作正在等待你的确认——请先处理确认弹窗，再继续对话。".into(),
                            collected_refs,
                        ));
                    }
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Finish => {
                    let _hint = skill_finish(true, "");
                    body_outcome = Some("skill_finish");
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Fail(reason) => {
                    let _hint = skill_finish(false, &reason);
                    body_outcome = Some("skill_fail");
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Terminate(reason) => {
                    let _hint = skill_finish(false, &reason);
                    body_outcome = Some("skill_terminate");
                    return Ok((last_streamed.clone(), collected_refs));
                }
            }
        }
        // 轮首活体信号：上一轮 bot-tool-done 之后到本轮首个 delta 之间是第二段
        // 空窗（LLM 首 token 延迟，无工具行可挂）。发一条 running 让前端知道
        // 「模型正在想」，配 turn 计数与已用时长的秒级刷新。
        if let Some(cb) = deps.turn_status {
            cb(
                TurnStatus::Running,
                &format!(
                    "正在思考 · 第 {} 轮",
                    round + 1
                ),
            );
        }
        // body 按协议分支——内部消息流保持 OpenAI
        // 形状不动，只在发请求前这一边界转换（bot_anthropic::build_anthropic_body）。
        // 转换失败（理论不可达，msgs 必含 user 消息）记审计并报错，不静默发出畸形请求。
        let mut body = match build_request_body(http, &msgs, &tools, openai_usage_supported) {
            Ok(b) => b,
            Err(e) => {
                let hint = skill_finish(false, "消息转换失败");
                let err: CommandError = format!("Anthropic 消息转换失败：{e}{hint}").into();
                audit(
                    crate::audit::AuditLevel::Error,
                    "llm.request_failed",
                    crate::audit::error_kv(&err),
                );
                return Err(err);
            }
        };

        // LLM 请求前记录
        audit(
            crate::audit::AuditLevel::Info,
            "llm.request",
            vec![
                ("model", http.model.clone()),
                ("msgs_count", msgs.len().to_string()),
                ("provider", http.provider.as_str().to_string()),
                ("reasoning", http.reasoning.describe()),
            ],
        );

        // 429/5xx/发送失败重试一次——瞬时抖动不应直接作废整轮工具循环。
        // 只在流式产出开始前重试，无部分内容重复/重放问题。
        let mut attempt = 0usize;
        let resp = loop {
            attempt += 1;
            // 鉴权头按协议分支
            //（Anthropic 用 x-api-key + anthropic-version，OpenAI 用 Bearer）
            let req = http.client.post(&url).json(&body);
            let req = match http.provider {
                crate::bot::ApiProvider::Openai => req.bearer_auth(http.api_key.trim()),
                crate::bot::ApiProvider::Anthropic => {
                    crate::bot_anthropic::apply_anthropic_auth(req, http.api_key.trim())
                }
            };
            // send() 单独挂响应头超时——共享客户端无默认
            // 总超时，等头阶段（服务端收下请求但不回）会永久挂起；per-request
            // total timeout 会连流式 body 一起算，不能用。60s 只覆盖
            // 连接+发请求+收到响应头，body 由下方逐 chunk idle 超时接管
            let send_result = match tokio::time::timeout(LLM_HEADER_TIMEOUT, req.send()).await {
                Ok(r) => r.map_err(|e| e.to_string()),
                Err(_) => Err(format!(
                    "响应头超时（{}s 无响应，判定服务端挂起）",
                    LLM_HEADER_TIMEOUT.as_secs()
                )),
            };
            match send_result {
                Ok(r) => {
                    if !r.status().is_success() {
                        let st = r.status().as_u16();
                        // 错误响应体统一先读出来：stream_options 探测要用它，
                        // 下面的报错文案与超长判定也要用同一份
                        let text = r.text().await.unwrap_or_default();
                        // 网关不认 stream_options 会 400。**只在错误体点名该字段
                        // 时降级**——普通 400（鉴权错/参数错/上下文超长）不是它的
                        // 问题，若一律重发，真错会被重试掩盖并变成一次多余请求。
                        if st == 400
                            && http.provider == crate::bot::ApiProvider::Openai
                            && openai_usage_supported
                            && body.get("stream_options").is_some()
                            && text.contains("stream_options")
                        {
                            openai_usage_supported = false;
                            // body 是轮外构造、轮内复用的同一份（重试不发新
                            // body），故必须就地摘掉字段，否则重发还是带它
                            if let Some(obj) = body.as_object_mut() {
                                obj.remove("stream_options");
                            }
                            audit(
                                crate::audit::AuditLevel::Warn,
                                "llm.stream_options_unsupported",
                                vec![("status", st.to_string())],
                            );
                            continue;
                        }
                        // Anthropic 的 thinking 块签名绑在会话前缀上：前缀变了
                        // （换模型 / 改系统提示 / 砍历史）签名即失效，服务端回
                        // 400 "Invalid `signature` in `thinking` block"。
                        //
                        // 官方给的补救是「删掉 thinking 与 redacted_thinking 块
                        // 后重试一次」（block_binding 的 drop_block 只在
                        // adaptive 模式可用，本项目走 budget_tokens 模式，只能手动删）。
                        // 只在响应体点名 thinking + signature 时才走这条路——
                        // 普通 400 不是它的问题，一律重发只会把真错掩盖成一次多余请求。
                        if st == 400
                            && http.provider == crate::bot::ApiProvider::Anthropic
                            && !anthropic_thinking_dropped
                            && text.contains("thinking")
                            && text.contains("signature")
                        {
                            let key = crate::bot_anthropic::THINKING_KEY;
                            let had = msgs.iter().any(|m| m.get(key).is_some());
                            if had {
                                anthropic_thinking_dropped = true;
                                for m in msgs.iter_mut() {
                                    if let Some(obj) = m.as_object_mut() {
                                        obj.remove(key);
                                    }
                                }
                                // body 已序列化：删完必须重建，否则重发还是带旧块
                                body = match build_request_body(
                                    http,
                                    &msgs,
                                    &tools,
                                    openai_usage_supported,
                                ) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        let hint = skill_finish(false, "消息转换失败");
                                        let err: CommandError =
                                            format!("Anthropic 消息转换失败：{e}{hint}").into();
                                        audit(
                                            crate::audit::AuditLevel::Error,
                                            "llm.request_failed",
                                            crate::audit::error_kv(&err),
                                        );
                                        return Err(err);
                                    }
                                };
                                audit(
                                    crate::audit::AuditLevel::Warn,
                                    "llm.anthropic_thinking_dropped",
                                    vec![("status", st.to_string())],
                                );
                                continue;
                            }
                        }
                        // 429/5xx 瞬时抖动：退避后重发同一请求
                        if is_retryable_llm_status(st) && attempt < MAX_LLM_ATTEMPTS {
                            audit(
                                crate::audit::AuditLevel::Warn,
                                "llm.retry",
                                vec![
                                    ("status", st.to_string()),
                                    ("attempt", attempt.to_string()),
                                ],
                            );
                            // 重试等待期响应 /stop——不再发出下一次请求（与主循环停止口径一致）
                            if stop.stopped() {
                                let hint = skill_finish(false, "用户停止");
                                return Ok((format!("⏹ 已停止{hint}"), collected_refs));
                            }
                            tokio::time::sleep(LLM_RETRY_DELAY).await;
                            continue;
                        }
                        let overflow = is_context_overflow(st, &text);
                        // 上下文超长：同一份 msgs 重发必然同样 400——砍掉最旧的
                        // 会话消息后重发（请求尚未开始流式产出，本轮无副作用可重放）
                        if overflow && context_sheds < CONTEXT_SHED_MAX {
                            let before = msgs.iter().map(msg_chars).sum::<usize>();
                            let (cut, dropped) =
                                shed_oldest_history(&mut msgs, CONTEXT_SHED_KEEP_RATIO);
                            if dropped > 0 {
                                context_sheds += 1;
                                // msgs 变短了：记账表里落在切口内的条目作废、
                                // 其后左移，不同步就是下一轮按旧下标压缩时越界
                                reindex_after_shed(&mut tool_msg_ages, cut, dropped);
                                // body 在进循环前已序列化——砍完必须重建整个 body，
                                // 否则重发的是同一份旧 body，降级等于没做。
                                // 重建要走协议构造：OpenAI 形状的 msgs 直接回填进
                                // Anthropic 的 body 会被拒（role=system / role=tool
                                // 都不认）→ 那个 400 还不再像超长，用户拿不到
                                // 「请 /compact」的指引。
                                body = match build_request_body(
                                    http,
                                    &msgs,
                                    &tools,
                                    openai_usage_supported,
                                ) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        let hint = skill_finish(false, "消息转换失败");
                                        let err: CommandError =
                                            format!("Anthropic 消息转换失败：{e}{hint}").into();
                                        audit(
                                            crate::audit::AuditLevel::Error,
                                            "llm.request_failed",
                                            crate::audit::error_kv(&err),
                                        );
                                        return Err(err);
                                    }
                                };
                                let after = msgs.iter().map(msg_chars).sum::<usize>();
                                audit(
                                    crate::audit::AuditLevel::Warn,
                                    "llm.context_overflow",
                                    vec![
                                        ("status", st.to_string()),
                                        ("dropped", dropped.to_string()),
                                        ("from_chars", before.to_string()),
                                        ("to_chars", after.to_string()),
                                        ("shed", context_sheds.to_string()),
                                    ],
                                );
                                if stop.stopped() {
                                    let hint = skill_finish(false, "用户停止");
                                    return Ok((format!("⏹ 已停止{hint}"), collected_refs));
                                }
                                continue;
                            }
                        }
                        // 没得砍（或本就不是超长）：回错误。超长时给出可操作指引，
                        // 别把裸 400 抛给用户——它看不出该 /compact 还是换模型
                        let hint = skill_finish(
                            false,
                            if overflow {
                                "上下文超长：请 /compact 压缩历史或新建会话，也可在设置里调小历史预算"
                            } else {
                                "大模型 API 错误"
                            },
                        );
                        let err = CommandError::LlmApiError {
                            status: st,
                            body_preview: format!(
                                "{}{hint}",
                                text.chars().take(300).collect::<String>()
                            ),
                        };
                        let mut kv = crate::audit::error_kv(&err);
                        kv.push(("status", st.to_string()));
                        audit(crate::audit::AuditLevel::Warn, "llm.response", kv);
                        return Err(err);
                    }
                    break r;
                }
                Err(e) => {
                    if attempt < MAX_LLM_ATTEMPTS {
                        audit(
                            crate::audit::AuditLevel::Warn,
                            "llm.retry",
                            vec![("err", e.to_string()), ("attempt", attempt.to_string())],
                        );
                        if stop.stopped() {
                            let hint = skill_finish(false, "用户停止");
                            return Ok((format!("⏹ 已停止{hint}"), collected_refs));
                        }
                        tokio::time::sleep(LLM_RETRY_DELAY).await;
                        continue;
                    }
                    let hint = skill_finish(false, "大模型请求失败");
                    let err: CommandError = format!("请求大模型失败：{e}{hint}").into();
                    let mut kv = crate::audit::error_kv(&err);
                    kv.push(("attempt", attempt.to_string()));
                    audit(crate::audit::AuditLevel::Error, "llm.request_failed", kv);
                    return Err(err);
                }
            }
        };
        // 非成功响应已在上面的请求循环内统一转 Err（429/5xx 退避重试、
        // 上下文超长降级重发、其余直接带指引报错），走到这里必是 2xx
        audit(
            crate::audit::AuditLevel::Info,
            "llm.response",
            vec![("status", resp.status().as_u16().to_string())],
        );

        let mut stream = resp.bytes_stream();
        // 字节缓冲按行切——逐 chunk from_utf8_lossy 时
        // chunk 边界落在多字节字符中间会产生 U+FFFD（正文乱码尚可，
        // 嵌在工具 arguments 里则是「合法 JSON 但内容损坏」会被真实执行）
        let mut byte_buf: Vec<u8> = Vec::new();
        let mut final_text = String::new();
        let mut tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments)
                                                                        // <think> 思考块拆分：思考走 bot-think-delta，正文走 bot-chat-delta
        let mut think_mode = false;
        let mut think_buf = String::new();
        // 流完整性：对端干净 EOF（无报错、无 [DONE]、无 finish_reason）时
        // 残缺 tool_calls 不得当完整回复执行——跟踪是否见到正常收尾标记
        let mut stream_end_seen = false;
        let mut last_finish_reason: Option<String> = None;
        let mut stream_error: Option<String> = None;
        let mut tc_index_overflow_logged = false;
        // 解析不掉的 data 载荷计数 + 首条样本：回合结束发一条审计，不逐块刷屏
        let mut bad_sse_chunks: u32 = 0;
        let mut bad_sse_sample: Option<String> = None;
        // 流内 token 用量累计（Anthropic：message_start 的 input /
        // message_delta 的 output；OpenAI：末块 usage），回合结束写 llm.usage 审计。
        //
        // 合并口径见 merge_stream_usage：按字段取最后一次非零值，**不逐块累加**
        // ——两协议的 usage 都是累计总量，逐块加会把同一份总量重复计入。
        // 缓存命中/写入两档单独记（仅 Anthropic 协议会变；OpenAI 保持 0），
        // 前端水位条按「读/写」单列展示，不折进输入输出，也不算命中率。
        let mut usage_acc = TokenUsage::default();
        // Anthropic 模式 content block index → 工具槽位重映射：
        // Anthropic 的块序号连文本块一起数，直接当工具序号累积会留下
        // 幽灵空条目，严格 API 会 400（详见 bot_anthropic::ToolSlotMapper 注释）
        let mut tool_slot_mapper = crate::bot_anthropic::ToolSlotMapper::new();
        // Anthropic 的 thinking 块收集器（仅 Anthropic 协议会收到增量）：
        // 块要原样回传服务端，故随本轮响应收集、随 assistant 消息入历史。
        let mut thinking_collector = crate::bot_anthropic::ThinkingCollector::default();

        let mut stopped = false;
        {
            // 单行 SSE 处理（主循环与流尾残余行冲刷共用）；
            // 返回 Some = 流内错误载荷，调用方收尾报错
            let mut handle_line = |line: &str| -> Option<String> {
                // 行解析按协议分支，解析出的
                // ParsedChunk 走同一消费逻辑（think 拆分 / 工具累积 / 流完整性 /
                // finish_reason 处理全部两协议共享）
                let parsed = match http.provider {
                    crate::bot::ApiProvider::Openai => parse_sse_chunk(line),
                    crate::bot::ApiProvider::Anthropic => {
                        match crate::bot_anthropic::parse_anthropic_event(line) {
                            Some(mut ev) => {
                                // content block index → 稠密工具槽位（防幽灵条目）
                                for d in &mut ev.tool_calls {
                                    tool_slot_mapper.remap(d);
                                }
                                Some(ev)
                            }
                            None => None,
                        }
                    }
                };
                // 解析不出东西时的留痕（两协议共用一处）：跳过是原行为且是对的
                // ——脏网关偶发半截 JSON 时其余 chunk 完好，中断会把好回复一起
                // 丢掉；补的只是「静默」，回合结束发一条计数 + 首条样本。
                let Some(mut parsed) = parsed else {
                    if let Some(payload) = unparsable_sse_payload(line, http.provider) {
                        bad_sse_chunks += 1;
                        if bad_sse_sample.is_none() {
                            bad_sse_sample = Some(payload.chars().take(120).collect::<String>());
                        }
                    }
                    return None;
                };
                // token 记账两协议共用一处：Anthropic 分布在 message_start /
                // message_delta 两个事件，OpenAI 集中在末块 usage。
                // 合并口径统一走 merge_stream_usage（按字段取最后一次非零值）。
                if let Some(u) = parsed.usage.take() {
                    merge_stream_usage(&mut usage_acc, u);
                }
                // Anthropic 的 thinking 块收进收集器：它要原样回传（丢了下一轮
                // 带 tool_result 的请求会被 400），不能只走前端展示出口。
                if let Some(d) = parsed.thinking.take() {
                    thinking_collector.push(&d);
                }
                if parsed.is_done {
                    stream_end_seen = true;
                    return None;
                }
                if let Some(reason) = parsed.finish_reason {
                    stream_end_seen = true;
                    last_finish_reason = Some(reason);
                }
                // reasoning_content 与 <think> 同出口（不进 final_text、不进历史）
                if let Some(r) = parsed.reasoning {
                    emit("bot-think-delta", serde_json::json!({ "text": r }));
                }
                if let Some(t) = parsed.content {
                    let (normal, think) = feed_think(&mut think_mode, &mut think_buf, &t);
                    if !think.is_empty() {
                        emit("bot-think-delta", serde_json::json!({ "text": think }));
                    }
                    if !normal.is_empty() {
                        final_text.push_str(&normal);
                        // 自检回复（Y/N）整段隐藏：流式不 emit，final_text 解析后
                        // 统一 emit ✅/⚠️ 视觉提示，避免用户先看到 Y 又被覆盖
                        // 终稿自检响应（Y/N）同样整段隐藏：用户已在主出口前看到
                        // final_text 全文，不该再叠一层 Y/N 字样
                        if !expecting_selfcheck && !expecting_final_selfcheck {
                            emit("bot-chat-delta", serde_json::json!({ "text": normal }));
                        }
                    }
                }
                for tc_delta in parsed.tool_calls {
                    let first_id = tc_delta.id.is_some()
                        && tool_calls
                            .get(tc_delta.index)
                            .map(|t| t.0.is_empty())
                            .unwrap_or(true);
                    // 累积走 accumulate_tool_call_delta（与 tests/llm_integration 同一实现）
                    if !accumulate_tool_call_delta(&mut tool_calls, &tc_delta) {
                        if !tc_index_overflow_logged {
                            tc_index_overflow_logged = true;
                            audit(
                                crate::audit::AuditLevel::Warn,
                                "llm.tool_call_index_overflow",
                                vec![
                                    ("index", tc_delta.index.to_string()),
                                    ("max", MAX_TOOL_CALL_INDEX.to_string()),
                                ],
                            );
                        }
                        continue;
                    }
                    let t = &tool_calls[tc_delta.index];
                    if first_id && !t.0.is_empty() {
                        // 新工具调用开始：推折叠行给前端
                        emit("bot-tool", serde_json::json!({ "id": t.0, "name": t.1 }));
                    }
                    if tc_delta
                        .name_chunk
                        .as_deref()
                        .is_some_and(|n| !n.is_empty())
                        && !t.0.is_empty()
                    {
                        emit(
                            "bot-tool-name",
                            serde_json::json!({ "id": t.0, "name": t.1 }),
                        );
                    }
                }
                parsed.error
            };
            // 逐 chunk idle 超时——next() 单次等待超过 STREAM_CHUNK_IDLE_TIMEOUT
            // 即判定死连接（总时长不再设限，合法长生成不受 300s 总超时误杀）
            loop {
                if stop.stopped() {
                    stopped = true;
                    break;
                }
                // next() 包 idle 超时：Ok(Some(inner)) = 拿到 chunk/流错误，
                // Ok(None) = 对端干净 EOF，Err = Elapsed（idle 超时）
                let item = match tokio::time::timeout(STREAM_CHUNK_IDLE_TIMEOUT, stream.next())
                    .await
                {
                    Ok(inner) => inner,
                    Err(_elapsed) => {
                        audit(
                            crate::audit::AuditLevel::Error,
                            "llm.stream_idle_timeout",
                            vec![("idle_secs", STREAM_CHUNK_IDLE_TIMEOUT.as_secs().to_string())],
                        );
                        return Err(format!(
                            "流式读取超时：连续 {}s 未收到数据，判定连接死亡",
                            STREAM_CHUNK_IDLE_TIMEOUT.as_secs()
                        )
                        .into());
                    }
                };
                let chunk = match item {
                    Some(Ok(c)) => c,
                    None => break, // 对端干净 EOF
                    Some(Err(e)) => {
                        audit(
                            crate::audit::AuditLevel::Error,
                            "llm.stream_failed",
                            vec![("err", e.to_string())],
                        );
                        return Err(format!("流式读取失败：{e}").into());
                    }
                };
                byte_buf.extend_from_slice(&chunk);
                for line in drain_sse_lines(&mut byte_buf) {
                    if let Some(e) = handle_line(line.trim()) {
                        stream_error = Some(e);
                        break;
                    }
                }
                if stream_error.is_some() {
                    break;
                }
            }
            // 流尾残余行冲刷：非标准服务端最后一个 data 事件可能不带尾换行
            if !stopped && stream_error.is_none() && !byte_buf.is_empty() {
                byte_buf.push(b'\n');
                for line in drain_sse_lines(&mut byte_buf) {
                    if let Some(e) = handle_line(line.trim()) {
                        stream_error = Some(e);
                        break;
                    }
                }
            }
        }

        // 本轮响应里的 Anthropic thinking 块（空 = 非 Anthropic 协议，
        // 或模型没开思考）。随本轮的 assistant 消息入历史，回传时原样发回。
        let thinking_blocks = thinking_collector.blocks();

        // 坏块可见化：整条流一条（逐块发会被脏网关刷屏），带首条样本供对拍
        if bad_sse_chunks > 0 {
            audit(
                crate::audit::AuditLevel::Warn,
                "llm.sse_chunk_unparsable",
                vec![
                    ("chunks", bad_sse_chunks.to_string()),
                    ("sample", bad_sse_sample.take().unwrap_or_default()),
                ],
            );
        }

        // 回合结束把流内捞到的 token 用量 + 缓存命中写审计（两协议）。
        // 缓存字段（cache_read / cache_creation）拼到 llm.usage 事件里——
        // bot.log 落盘可做命中率回看（业界把命中率列为生产 agent 第一指标）。
        // OpenAI 无 cache write 字段，该档恒 0（不是解析失败）。
        if usage_acc.input_tokens > 0 || usage_acc.output_tokens > 0 {
            let mut kv = vec![
                ("input_tokens", usage_acc.input_tokens.to_string()),
                ("output_tokens", usage_acc.output_tokens.to_string()),
            ];
            if usage_acc.cache_read_tokens > 0 || usage_acc.cache_creation_tokens > 0 {
                kv.push((
                    "cache_read_tokens",
                    usage_acc.cache_read_tokens.to_string(),
                ));
                kv.push((
                    "cache_creation_tokens",
                    usage_acc.cache_creation_tokens.to_string(),
                ));
                // 命中率 = read / (read + creation + input)，缓存写入一次占新前缀
                // 后续轮都是 read；creation 仅在「上一轮窗口写满 + 重新构造前缀」
                // 时出现。计算时 input 含未被缓存的尾部 token，命中看 read 占比
                let total = usage_acc.cache_read_tokens
                    + usage_acc.cache_creation_tokens
                    + usage_acc.input_tokens;
                // checked_div：total=0（无任何 token）时整个键都不落，与旧守卫语义一致
                if let Some(pct) = (usage_acc.cache_read_tokens * 100).checked_div(total) {
                    kv.push(("cache_hit_pct", pct.to_string()));
                }
            }
            audit(crate::audit::AuditLevel::Info, "llm.usage", kv);
        }

        // 200 流内错误载荷——显式报错 + 审计，不返回空白回复
        if let Some(err) = stream_error {
            let hint = skill_finish(false, "大模型流内错误");
            let e: CommandError = format!("大模型返回错误：{err}{hint}").into();
            audit(
                crate::audit::AuditLevel::Error,
                "llm.stream_error",
                crate::audit::error_kv(&e),
            );
            return Err(e);
        }

        // 回合结束：冲刷思考缓冲（丢弃未闭合标签碎片）
        let tail = std::mem::take(&mut think_buf)
            .replace("<think>", "")
            .replace("</think>", "");
        if !tail.is_empty() {
            if think_mode {
                emit("bot-think-delta", serde_json::json!({ "text": tail }));
            } else {
                final_text.push_str(&tail);
                if !expecting_selfcheck && !expecting_final_selfcheck {
                    emit("bot-chat-delta", serde_json::json!({ "text": tail }));
                }
            }
        }

        if stopped {
            let hint = skill_finish(false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }

        // 流完整性检查：未见 [DONE]/finish_reason 的
        // 干净 EOF = 流被截断（中间代理 idle cut 等）。残缺 tool_calls 不得执行
        // （不能靠 parse_args 失败落 Null 侥幸兜底，要有显式防线）。
        if !stream_end_seen {
            let trunc_kv = |tc: usize, text_len: usize| {
                vec![
                    ("tool_calls", tc.to_string()),
                    ("text_len", text_len.to_string()),
                ]
            };
            if !tool_calls.is_empty() {
                let hint = skill_finish(false, "流式响应中断");
                let err: CommandError =
                    format!("大模型响应中断（流被截断），工具调用未执行{hint}").into();
                let mut kv = crate::audit::error_kv(&err);
                kv.extend(trunc_kv(tool_calls.len(), final_text.chars().count()));
                audit(crate::audit::AuditLevel::Warn, "llm.stream_truncated", kv);
                return Err(err);
            }
            if final_text.is_empty() {
                let hint = skill_finish(false, "流式响应中断");
                let err: CommandError = format!("大模型响应中断：未收到完整回复{hint}").into();
                let mut kv = crate::audit::error_kv(&err);
                kv.extend(trunc_kv(tool_calls.len(), final_text.chars().count()));
                audit(crate::audit::AuditLevel::Warn, "llm.stream_truncated", kv);
                return Err(err);
            }
            audit(
                crate::audit::AuditLevel::Warn,
                "llm.stream_truncated",
                trunc_kv(tool_calls.len(), final_text.chars().count()),
            );
            final_text.push_str("\n\n⚠️ 响应可能被截断（连接提前结束），以上内容可能不完整。");
        }

        // 幽灵条目兜底防线：name 为空的条目从未收到
        // function.name，不可能是真实工具调用（Anthropic 侧已被 ToolSlotMapper 重映射
        // 防住，这里双保险覆盖 OpenAI 兼容网关的畸形流）。不丢弃的话：空 id 会被合成
        // call_synth_* 并以「未知工具」执行回填，下一轮请求里 tool_result 引用一个
        // 服务端从未签发的 id，严格 API 400（"tool result's tool id not found"）。
        let ghost_count = tool_calls.iter().filter(|t| t.1.is_empty()).count();
        if ghost_count > 0 {
            tool_calls.retain(|t| !t.1.is_empty());
            audit(
                crate::audit::AuditLevel::Warn,
                "llm.ghost_tool_call_dropped",
                vec![("count", ghost_count.to_string())],
            );
        }

        // 兼容省略 tool_call id 的供应商——空 id 进历史
        // 下一轮会被严格 API 拒为 400 invalid params；本地合成占位 id
        for (i, t) in tool_calls.iter_mut().enumerate() {
            if t.0.is_empty() {
                t.0 = format!("call_synth_{i}");
            }
        }

        if tool_calls.is_empty() {
            // 防幻觉汇报守卫（前置：每轮无工具调用的文本产出点即检查，当场打回，
            // 不等会话收尾）三个支柱：
            // a) claims_mutation——声称完成变更但本轮没动过手；
            // b) claimed_doc_paths + 存在性——声称已生成文档且给出路径，但路径在
            //    磁盘上不存在（编造的路径经不起存在性检查，实测窄词表漏检的话术
            //    靠它兜住）
            // c) claimed_doc_paths + 落盘凭证——路径在磁盘上，但它从来不是任何
            //    工具写出来的：模型在声称「已生成」一个跟自己无关的文件
            // 自检回复解析：上一轮注入了 SELFCHECK_PROMPT（pending 仍在场、
            // expecting_selfcheck 为真）。纯 Y/N 不是给用户的答案：
            // Y → ✅ 视觉提示后续跑一轮，让模型产出正常汇报（否则回合被自检
            //     终结，用户的请求得不到真实答复）；
            // N → ⚠️ 视觉提示 + 直接 push 换策略提示后续跑（PREVR 第 1 层同源
            //     文案；fail_hint_queued 的注入点在工具回填之后，自检回复轮
            //     没有工具调用到不了那里，只能就地 push）；
            // 附解释 → 自检外还有真实回答，按正常答案走主出口终结回合。
            if expecting_selfcheck {
                let sc_name = pending_selfcheck
                    .as_ref()
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                match crate::prompts::parse_selfcheck_response(final_text.trim()) {
                    Some(true) => {
                        emit(
                            "bot-chat-delta",
                            serde_json::json!({ "text": "✅ 自检通过\n\n" }),
                        );
                        msgs.push(with_thinking(
                            serde_json::json!({ "role": "assistant", "content": final_text }),
                            &thinking_blocks,
                        ));
                        msgs.push(serde_json::json!({
                            "role": "user",
                            "content": "【系统提示】自检通过。请继续：完成剩余步骤，或直接向用户汇报结果。"
                        }));
                        expecting_selfcheck = false;
                        pending_selfcheck = None;
                        continue;
                    }
                    Some(false) => {
                        emit(
                            "bot-chat-delta",
                            serde_json::json!({ "text": "⚠️ 自检未达预期,换策略中…\n\n" }),
                        );
                        msgs.push(with_thinking(
                            serde_json::json!({ "role": "assistant", "content": final_text }),
                            &thinking_blocks,
                        ));
                        msgs.push(serde_json::json!({
                            "role": "user",
                            "content": format!(
                                "【系统提示】你对工具 {sc_name} 的自检回复了「未达预期」。不要重复同样的调用；分析原因后换策略：换参数、换工具、或把任务拆成更小的步骤再试一次。"
                            )
                        }));
                        expecting_selfcheck = false;
                        pending_selfcheck = None;
                        continue;
                    }
                    None => {
                        // 模型附了解释说明：emit 原 final_text（之前 SSE 已跳 emit）
                        emit("bot-chat-delta", serde_json::json!({ "text": final_text }));
                        expecting_selfcheck = false;
                        pending_selfcheck = None;
                    }
                }
            }
            // 声称产出的文档路径：两道校验。
            //
            // · 存在性——路径在磁盘上不在，就是凭空写的路径（原有判据）。
            // · 落盘凭证——路径在，但它从来不是任何工具写出来的：模型在声称
            //   「已生成」一个跟自己无关的文件。这道是存在性查不出来的——上周
            //   生成过、或者用户自己放在那儿的文件，存在性一律放行。
            //
            // 凭证来源两处：本提问写过的路径在进程内（不等痕迹落库），历史上
            // 写过的查库。两处都没有才是编造。
            let claimed_paths = claimed_doc_paths(&final_text);
            let mut fake_gen_path: Option<String> = None;
            for p in &claimed_paths {
                if path_missing_on_disk(p) {
                    fake_gen_path = Some(p.clone());
                    break;
                }
            }
            // 本次提问跑过「自己能写盘、我们无从确证」的工具（run_python 无文件
            // 系统隔离、MCP 工具在外部服务里执行）——它们能写出文件却不经过
            // 我们的凭证，「没凭证」推不出「不是工具产物」，这两道闸都必须让开。
            //
            // 口径是**本次提问**，不是整个会话：早先按会话判，一次 run_python
            // 会把这条会话后续所有提问的凭证闸全打死。
            //
            // 派过子 agent 也让开：子 agent 的调用不落我们的痕迹表（它的
            // trace_id 是 'trc_xxx' 字符串，与 exec_spans 的整数 trace_id 类型
            // 都对不上，join 永远不成立），它写的文件在这道闸眼里永远没凭证。
            let unverifiable_ran = receipts
                .iter()
                .any(|n| n == "run_python" || n.starts_with("mcp_"))
                || receipts.iter().any(|n| n == "spawn_subagent");
            // 落盘凭证这道要在存在性过了之后才问：路径压根不存在时已经判编造，
            // 没必要再查库。
            let mut unreceipted_path: Option<String> = None;
            if fake_gen_path.is_none() && !claimed_paths.is_empty() && !unverifiable_ran {
                let pending: Vec<String> = claimed_paths
                    .iter()
                    .filter(|p| !written_paths.contains(&p.to_lowercase()))
                    .cloned()
                    .collect();
                if !pending.is_empty() {
                    if let Some(fetch) = deps.file_receipts {
                        unreceipted_path =
                            fetch(stop.session_id(), &pending).into_iter().next();
                    }
                }
            }
            // 「动过手」的凭据：变更类工具成功（mutation_done），**或**跑过
            // 不可核验但确实改状态的工具（run_python / MCP 工具 / 子 agent）。
            //
            // 后者的副作用核验不了凭证，但「没凭证」推不出「没动手」——少了
            // 这一条，「用 Python 写完文件并如实汇报」会被守卫当成凭空汇报：
            // 变更声称命中（write_file 的「已写入」等），而 mutation_done 是
            // 假的，守卫把终稿推回让模型重跑，副作用翻倍，末尾还会挂一条
            // 「这句没有对应凭证」的更正。口径与上面落盘凭证那道闸一致。
            let claim_mutation =
                claim_mutation_fires(mutation_done, unverifiable_ran, &final_text);
            // 执行类谎报：本回合零工具成功，终稿却在叙述执行结果。此前守卫只有
            // mutating 一档，run_python / fetch_url 这类非 mutating 工具可以被凭空
            // 叙述——实测两起：原话一「执行完毕，sleep 120 秒后工具真实返回，打印了
            // done」，原话二「完成计划2 步：第一步触发 ModuleNotFoundError，第二步仍
            // 输出 done」，两条对应 trace 都是 turns=0 tools=0、耗时 1.9s/1.8s。
            //
            // 判决只看凭证表，不看轮次：终稿点名的工具逐个查本次提问的凭证池，
            // 任何一个查不到就是伪造。旧的「本回合有没有任何工具成功」是个布尔，
            // 挡不住「本轮成功调了 A、却编造 B 的执行结果」——同一个提问里
            // 不同工具的凭证不能互相担保。
            let mentioned = mentioned_tool_names(&final_text);
            let table_hit = claims_execution_with(&final_text, deps.guard_claim_keywords);
            // 终稿里「点名了某次执行」的工具：执行形态词与工具名落在同一句内。
            //
            // 词表收的全是完成态 / 成功态的措辞（失败形态「调用失败」「调用被拒」
            // 刻意不收），所以「词贴在这个工具名旁边」= 模型在报这个工具跑成了。
            // 触发层按工具名逐个判，而不是全局判一次：全局判会把「工具调用结果：
            // - run_python：调用失败」这种如实汇报失败也拖下水——词在标题那一行，
            // 工具名在下一行，说的不是同一件事。
            let claimed = claimed_executed_tools(&final_text, deps.guard_claim_keywords);
            // 本次提问派过子 agent → 点名判定让开。子 agent 的调用不落我们的痕迹表
            // （trace_id 类型都对不上），主 agent 如实汇报「子 agent 跑完了 run_python」
            // 在主池里查不到这个名字——查不到不等于没跑过。
            let subagent_spawned = receipts.iter().any(|n| n == "spawn_subagent");
            // 会话工具名单（本次提问之前本会话调用过 / 成功过的工具）：跨提问的
            // 重述是真话（「刚才 create_word 生成的那个」），排除集要覆盖本会话历史。
            // 惰性拉一次，且只在判据用得上时拉——正常路径零读库。
            let need_session_tools = !claimed.is_empty();
            let session_tools: Option<crate::db::trace::SessionTools> = if !need_session_tools {
                None
            } else {
                match &session_tools_cache {
                    Some(v) => v.clone(),
                    None => {
                        let v = deps.session_tools.and_then(|f| f(stop.session_id()));
                        session_tools_cache = Some(v.clone());
                        v
                    }
                }
            };
            let subagent_in_session = match &session_tools {
                // 查不到历史 → 让开（薄壳没这个注入点、或查库失败）
                None => true,
                Some(st) => st.called.iter().any(|n| n == "spawn_subagent"),
            };
            // 点名判据统一的开闸条件：派过子 agent、或查不到本会话历史时整档让开
            // （查不到证据不等于有罪）。
            let point_check_open = !subagent_spawned && !subagent_in_session;
            let session_succeeded: &[String] = session_tools
                .as_ref()
                .map(|st| st.succeeded.as_slice())
                .unwrap_or(&[]);
            // 点名判据（独立判据）：终稿点名某工具跑过，而那个工具**从没成功返回
            // 过**——编造的一次执行。
            //
            // 判决口径是「没成功过」而不是「没调用过」：调用过但失败、却报成功，
            // 正是这道要抓的；而如实汇报失败（词表不收失败形态）根本进不了触发
            // 层，不会被牵连。
            //
            // 排除集两处：本次提问的成功凭证与本会话历史里的成功记录（跨提问的
            // 重述是真话）。
            let unsupported: Vec<&str> = if !point_check_open {
                Vec::new()
            } else {
                claimed
                    .iter()
                    .filter(|t| {
                        !receipts.iter().any(|r| r == *t)
                            && !session_succeeded.iter().any(|n| n == *t)
                    })
                    .copied()
                    .collect()
            };
            // 执行类粗判：终稿不点名任何工具（无从按名查）却在叙述一次执行，
            // 而本次提问一条成功凭证都没有——退回整条提问零凭证的粗判，这正是
            // 词表要顶住的那类（「执行完毕，工具真实返回…」）。
            let claim_execution = table_hit && mentioned.is_empty() && receipts.is_empty();
            // 判据一（结果无出处）：终稿把一个数当成执行产物报出来，而那个数在
            // 本次提问的工具返回与对话上下文里都找不到出处。
            //
            // 凭证表答不了这一问：它记的是「哪个工具成功过」，不是「这个数是哪
            // 来的」。同一个提问里调过工具，不代表报出来的数就是那个工具给的
            // ——「已跑完，输出 5050」而工具给的是别的数，就是编的。
            //
            // 让开两处：派过子 agent（它的返回不落我们的池子）、终稿如实说了
            // 执行没做成（那它给的是自己的推算，不是把执行结果编出来）。
            let claimed_numbers = crate::bot::registry::claimed_result_numbers(
                &final_text,
                deps.guard_claim_keywords,
            );
            let mut sources: Vec<String> = context_sources(&msgs);
            sources.extend(evidence.iter().filter(|e| e.ok).map(|e| e.output.clone()));
            let unsourced_value: Option<String> = if claimed_numbers.is_empty()
                || subagent_spawned
                || crate::bot::registry::states_execution_not_done(&final_text)
            {
                None
            } else {
                claimed_numbers
                    .iter()
                    .find(|n| !number_sourced(n, &sources))
                    .cloned()
            };
            // 判据二（回声调用）：本次提问的成功调用全都没取回新信息——返回内容
            // 就是模型自己写进参数的那句。终稿再拿它当结论的依据，就是自证。
            //
            // 口径是「有没有别的调用取回过真东西」：同一提问里若还有一次读到现
            // 场的调用（真机 trace 106 的 run_python），结论就有依据，不该拦。
            let echo_only = evidence.iter().any(|e| e.ok)
                && evidence
                    .iter()
                    .filter(|e| e.ok)
                    .all(|e| crate::bot::registry::is_echo_call(&e.args, &e.output));
            let echo_backed = echo_only
                && !subagent_spawned
                && (crate::bot::registry::states_a_result(&final_text) || !mentioned.is_empty());
            // 幻觉守卫判决：声称完成但无对应凭证 → 第一次返工，第二次如实标注。
            //
            // 两次是两种不同处置，不是同一种做两遍：
            // · 第一次（claim_retry_used=false）：把终稿推回消息栈、注入系统提示
            //   让模型真去调工具。此时谎报还只在消息栈里——前端收尾时用 bot_chat
            //   的返回值覆盖流式气泡，用户最终看到的是返工后的那一版。
            // · 第二次（claim_retry_used=true）：不再返工。返工会让模型重跑已完成
            //   的动作（delete_task / link_file_to_task 这类是重复的破坏性操作），
            //   而谎报已经拦过一次，再拦一次收益不抵风险。改为把「这句没有对应
            //   凭证」如实标在终稿末尾：让用户看见谎话 + 一条更正，好过让谎话
            //   冒充事实单独到达用户。
            if claim_mutation
                || claim_execution
                || !unsupported.is_empty()
                || fake_gen_path.is_some()
                || unreceipted_path.is_some()
                || unsourced_value.is_some()
                || echo_backed
            {
                // kind 的优先级与下面的 reason 一致：先说最硬的那条（路径压根
                // 不存在 / 路径不是工具产物），再说点名判据、执行类粗判，然后是
                // 结果无出处与回声调用，最后变更类。
                //
                // 粗判排在结果无出处前面：零凭证时「本次提问没有任何工具调用
                // 成功」是更完整的根因说法——那种情形下结果值必然也没出处，
                // 换个说法反而丢信息。两条新判据顶的是粗判够不着的地界：工具
                // 确实调过（有凭证），但报出来的数不是它给的。
                let kind = if fake_gen_path.is_some() {
                    "声称已生成但路径不存在".to_string()
                } else if unreceipted_path.is_some() {
                    "声称已生成但无落盘凭证".to_string()
                } else if !unsupported.is_empty() {
                    format!("声称已执行但无对应凭证: {}", unsupported.join(","))
                } else if claim_execution {
                    "声称已执行但本次提问零凭证".to_string()
                } else if let Some(v) = &unsourced_value {
                    format!("声称的结果无出处: {v}")
                } else if echo_backed {
                    "结论无工具支撑".to_string()
                } else {
                    "声称变更但未调工具".to_string()
                };
                // 同一处判决的两种口吻：reason 说给模型听（第二人称、要它改正），
                // notice 说给用户听（第三人称、只陈述事实）。
                let (reason, notice) = match (
                    &fake_gen_path,
                    &unreceipted_path,
                    claim_execution,
                    unsupported.as_slice(),
                    unsourced_value.as_deref(),
                    echo_backed,
                ) {
                    (Some(p), _, _, _, _, _) => (
                        format!(
                            "你声称已生成文件 {p}，但该路径在磁盘上不存在——文件实际没有生成。"
                        ),
                        format!("上面提到的文件 {p} 实际没有生成——磁盘上不存在这个路径。"),
                    ),
                    (None, Some(p), _, _, _, _) => (
                        format!(
                            "你声称已生成文件 {p}，但这个路径没有落盘凭证——本次提问的落盘记录和历史痕迹里都没有它，这个文件不是本次生成的。"
                        ),
                        format!(
                            "上面提到的文件 {p} 不是本次生成的——这个路径没有落盘凭证。"
                        ),
                    ),
                    (None, None, _, [t, ..], _, _) => (
                        format!(
                            "你刚才说调了 {t} 并给出了结果，但本次提问和本会话历史里它都没有成功返回过——这个结果是编造的。"
                        ),
                        format!(
                            "上面关于 {t} 的执行结果是编造的——本次提问和本会话历史里它都没有成功返回过。"
                        ),
                    ),
                    (None, None, true, _, _, _) => (
                        "你刚才叙述了一次已完成的执行并给出了结果，但本次提问从头到尾没有任何工具调用成功——这个结果是编造的。".to_string(),
                        "上面叙述的这次执行没有实际发生——本次提问里没有任何工具调用成功。"
                            .to_string(),
                    ),
                    (None, None, _, _, Some(v), _) => (
                        format!(
                            "你刚报的结果 {v} 在本次提问的工具返回与对话内容里都找不到出处——这个数不是执行出来的。要么真的执行一次、拿到返回后再如实转述，要么明确写清这是你的推算值（不是执行结果），禁止把没跑出来的数说成跑出来的。"
                        ),
                        format!(
                            "上面报的 {v} 没有出处——本次提问的工具返回与对话内容里都没有这个数。"
                        ),
                    ),
                    (None, None, _, _, _, true) => (
                        "你这次的调用没有取回任何新信息——返回内容就是你写进参数里的那句话，工具没有替你核实任何东西，结论不该拿它当依据。请换用能真正读到现场的工具重新核实（列目录用 list_files、读文件用 read_text_file、查任务用 query_tasks、搜内容用 grep_files）；若确实无法核实，如实说明这次没有核实过，禁止把没核实过的结论说成核实过的。".to_string(),
                        "上面的结论没有工具支撑——本次提问的调用没有取回任何新信息。"
                            .to_string(),
                    ),
                    (None, None, false, _, _, _) => (
                        "你刚才声称完成了变更，但本轮没有任何变更类工具调用成功，数据实际没有变化。"
                            .to_string(),
                        "上面声称的变更没有实际发生——本轮没有变更类工具调用成功，数据实际没有变化。"
                            .to_string(),
                    ),
                };
                if !claim_retry_used {
                    claim_retry_used = true;
                    audit_log(&format!(
                        "hallucination_guard | {kind}: {}",
                        crate::bot::truncate_for_log(&final_text, 100)
                    ));
                    // 执行类谎报的补救路径与变更类不同：变更类要点名具体工具，
                    // 执行类（含 run_python / bash / fetch_url）点不全也不该点名——
                    // 该做的是把工具真正调起来，拿不到结果就如实说拿不到。
                    // 文件类谎报（路径不存在 / 路径不是工具产物）第三档：只说
                    // 文档生成那几个工具，别把模型引到任务卡工具上去。
                    // 点名判据（独立判据）走执行类那一档：它说的就是一次执行。
                    // 结果无出处那道：要么真跑一次拿到返回，要么写清这是推算。
                    // 回声调用那道：换能读到现场的工具重查，别拿自己写进参数的
                    // 那句话当证据。
                    let remedy = if echo_backed {
                        "请换用能真正读到现场的工具重新核实（列目录用 list_files、读文件用 read_text_file、查任务用 query_tasks、搜内容用 grep_files），并只转述工具真实返回的内容；若确实无法核实，如实说明这次没有核实过，禁止把没核实过的结论说成核实过的。".to_string()
                    } else if unsourced_value.is_some() {
                        "请发起 tool_call 实际执行并拿到真实返回后再如实转述；若确实无法执行（白名单拦截 / 无权限 / 工具不可用），就明确写清这是你的推算值而不是执行结果，禁止把没跑出来的数说成跑出来的。".to_string()
                    } else if (claim_execution || !unsupported.is_empty())
                        && fake_gen_path.is_none()
                    {
                        "请立即发起 tool_call 实际执行，禁止只改文字糊弄；拿到真实返回后再如实转述（失败就如实报失败）。若确实无法执行（白名单拦截 / 无权限 / 工具不可用），如实向用户说明原因，禁止再次编造执行结果。".to_string()
                    } else if fake_gen_path.is_some() || unreceipted_path.is_some() {
                        "请立即调用文档生成工具实际把这个文件写出来（Word 用 create_word 或 create_word_revisions、Excel 用 create_excel、PPT 用 create_ppt、PDF 用 create_pdf；纯文本文件用 write_file），禁止只改文字糊弄；工具返回的完整路径才是能报给用户的路径。若确实无法生成（内容不足 / 工具不可用 / 权限不足），如实向用户说明原因，禁止再次声称已生成。".to_string()
                    } else {
                        "请立即调用对应工具实际执行（删除用 delete_task、完成用 complete_task、编辑用 edit_task、子任务用 add_subtask/remove_subtask、绑定产物用 link_file_to_task（任务卡执行流程内有效）、生成文档用 create_word/create_excel/create_ppt/create_pdf；逐步执行模式下子任务勾选由系统完成，不要代调 toggle_subtask）；若确实无法执行（任务不存在/被安全闸门拦截/无权限等），如实向用户说明原因，禁止再次声称已完成。".to_string()
                    };
                    msgs.push(with_thinking(
                        serde_json::json!({"role": "assistant", "content": final_text}),
                        &thinking_blocks,
                    ));
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": format!("【系统提示】{reason}{remedy}")
                    }));
                    continue;
                }
                // 返工之后仍编造：不返工（见块首），只在终稿末尾挂一条如实更正。
                audit_log(&format!(
                    "hallucination_guard.repeat | {kind}: {}",
                    crate::bot::truncate_for_log(&final_text, 100)
                ));
                fabrication_notice = Some(notice);
            }
            // stop_reason=max_tokens：思维链把输出预算吃光时，正文和排在最后的
            // tool_use 会整段被砍，用户只拿到半截甚至空回复，还得自己打「继续」。
            // 续跑一轮拿的是全新输出预算，比把半截文本丢回去划算。
            // 自检两态要排除：自检侧调用故意压极小 max_tokens，length 是它的正常
            // 终止形态，在这里续跑只会对着 Y/N 空转。
            if last_finish_reason.as_deref() == Some("length")
                && !expecting_selfcheck
                && !expecting_final_selfcheck
                && length_continuations < MAX_LENGTH_CONTINUATIONS
            {
                length_continuations += 1;
                length_prefix.push_str(&final_text);
                audit(
                    crate::audit::AuditLevel::Info,
                    "llm.length_continued",
                    vec![
                        ("attempt", length_continuations.to_string()),
                        ("chars", final_text.chars().count().to_string()),
                    ],
                );
                msgs.push(with_thinking(
                    serde_json::json!({"role": "assistant", "content": final_text}),
                    &thinking_blocks,
                ));
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": LENGTH_CONTINUE_PROMPT
                }));
                continue;
            }
            // finish_reason=length（token 截断）/content_filter 需要显式感知——
            // 否则半截回复会被当正常答案、空回复无解释
            match last_finish_reason.as_deref() {
                Some("length") => {
                    final_text.push_str("\n\n（回复因长度限制被截断，可以让我「继续」补完）")
                }
                Some("content_filter") if final_text.is_empty() => {
                    final_text = "（回复被服务商的内容过滤拦截，请换个方式提问）".into();
                }
                _ => {}
            }
            // 出口闸：纯 Y/N 单字不是给用户的答复，是自检判定（协议见
            // prompts/selfcheck.rs，in-loop 路径在「expecting_selfcheck」窗口内已
            // 解析过一次）。模型可能隔几轮才回答那句「请仅用一个字回答 Y/N」——
            // 中途它用行动代替了回答，自检态随「以行动代回答」释放了，那句问话
            // 却还留在上下文里，模型在后面某轮补了个 Y。那个 Y 落到这里就成了
            // 终稿：实测事故（真机 2026-10-23）用户只看到一个 Y，真正的报告
            // 压根没产出（模型正要写报告就被这一轮终结了）。
            //
            // 判据：终稿是纯 Y/N 单字 + 本轮问过自检（两个 last_*_round 有值
            // 即本轮问过）→ 续跑一轮，让模型把真实结果说出来。问过自检这一条
            // 不能省：用户明说「只回答 Y 或 N」时，一个 Y 就是合法答复。
            // 窗口内的判定（expecting_final_selfcheck）不算迟到——它由下面的
            // 终稿自检 verdict 解析接管（Y 还原原文 / N 追加警告），闸不能抢。
            let asked_selfcheck = !expecting_final_selfcheck
                && (last_selfcheck_round.is_some() || last_final_selfcheck_round.is_some());
            if asked_selfcheck
                && crate::prompts::parse_selfcheck_response(final_text.trim()).is_some()
            {
                let passed = crate::prompts::parse_selfcheck_response(final_text.trim()) == Some(true);
                if late_verdict_retries < MAX_LATE_VERDICT_RETRIES {
                    late_verdict_retries += 1;
                    let hint = if passed {
                        "【系统提示】自检通过。请继续：完成剩余步骤，或直接向用户汇报结果。"
                    } else {
                        "【系统提示】自检未达预期：请检查本轮的实际结果是否真的达成预期，必要时换策略重做，然后向用户汇报。"
                    };
                    emit(
                        "bot-chat-delta",
                        serde_json::json!({
                            "text": if passed { "✅ 自检通过\n\n" } else { "⚠️ 自检未达预期\n\n" }
                        }),
                    );
                    msgs.push(with_thinking(
                        serde_json::json!({ "role": "assistant", "content": final_text.clone() }),
                        &thinking_blocks,
                    ));
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": hint
                    }));
                    continue;
                }
                // 追问额度用尽：模型只肯给判定字样。给一句可读的说明，别把裸 Y
                // 当答复交给用户——那等于没回答。
                final_text = format!(
                    "（模型只回了自检判定「{}」，没有给出可读的结果）",
                    final_text.trim()
                );
            }
            // 终稿自检节流闸：final_text 不在工具自检态、不在 final 自检态、
            // 字数 ≥ 80、累计函数调用 ≥ 3、距上次 ≥ gap（默认 8 轮；0 = 关闭）→
            // 拍 final_text snapshot + 注入 FINAL_SELFCHECK_PROMPT + 续跑一轮
            // 让模型判 Y/N；本轮不走主出口，模型响应在下一轮迭代末尾解析后还原。
            let final_selfcheck_gap =
                deps.final_selfcheck_round_gap
                    .unwrap_or(DEFAULT_FINAL_SELFCHECK_ROUND_GAP) as usize;
            if final_selfcheck_gap > 0
                && !expecting_selfcheck
                && !expecting_final_selfcheck
                && final_text.chars().count() >= MIN_FINAL_SELFCHECK_CHARS
                && function_calls_total >= MIN_FINAL_SELFCHECK_FUNCTION_CALLS
                && last_final_selfcheck_round
                    .is_none_or(|r| round.saturating_sub(r) >= final_selfcheck_gap)
            {
                // 用户已停止：不再做终稿自检。侧调一次最多烧 60s，期间 UI 停在
                // 「收尾中 · 校验答复」——用户看着像停止失灵。
                if stop.stopped() {
                    let hint = skill_finish(false, "用户停止");
                    return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
                }
                // settling：正文已经完整输出，用户「看完了」，但模型还要跑一次
                // 自检（可静默数十秒）。这一步不发生，用户会把正文结束误读成
                // 「本轮结束」——ZCode 的 settling 就是为这个场景存在的。
                if let Some(cb) = deps.turn_status {
                    cb(TurnStatus::Settling, "收尾中 · 校验答复");
                }
                let snippet: String = final_text
                    .chars()
                    .take(FINAL_SELFCHECK_SNIPPET_CHARS)
                    .collect();
                last_final_selfcheck_round = Some(round);
                if let Some(sc_http) = deps.final_selfcheck_http {
                    // 侧调路径：独立小请求只带 snippet（几百字符）——省主循环
                    // 整段历史重付（agent 输入输出比 ~100:1，主循环多一轮 =
                    // 全史重付一次）。判定失败/请求错误 → 视作通过原文照常
                    // 返回（与 in-loop 附解释语义一致），只留审计。
                    final_selfcheck_snapshot = Some(final_text.clone());
                    let prompt =
                        crate::prompts::FINAL_SELFCHECK_PROMPT.replace("{snippet}", &snippet);
                    let side_msgs = [crate::bot_chat::ChatMsg {
                        role: "user".to_string(),
                        content: prompt,
                    }];
                    let sidecall = crate::bot_chat::summarize_http(
                        &sc_http.client,
                        &sc_http.base_url,
                        &sc_http.api_key,
                        &sc_http.model,
                        "",
                        &side_msgs,
                        sc_http.provider,
                        // 判定 prompt 要求「仅回答 Y/N」——32 token 足够首字
                        // 符判定，截断不影响宽松前缀解析
                        32,
                        None,
                        None,
                    )
                    .await;
                    match sidecall {
                        Ok(v) => {
                            if crate::prompts::parse_selfcheck_reply_lenient(v.trim())
                                == Some(false)
                            {
                                if let Some(snapshot) = final_selfcheck_snapshot.take() {
                                    final_text = format!("{snapshot}{FINAL_SELFCHECK_WARN}");
                                    emit(
                                        "bot-chat-delta",
                                        serde_json::json!({ "text": FINAL_SELFCHECK_WARN }),
                                    );
                                    audit_log(&format!(
                                        "final_selfcheck.fail | {}",
                                        crate::bot::truncate_for_log(&snapshot, 200)
                                    ));
                                }
                            }
                        }
                        Err(e) => (deps.audit)(
                            crate::audit::AuditLevel::Warn,
                            "final_selfcheck.sidecall_failed",
                            vec![("err", e.to_string())],
                        ),
                    }
                    final_selfcheck_snapshot = None;
                    // fall through：主出口照常返回，侧调不占主循环轮次
                } else {
                    // in-loop 路径（未配置侧调模型）：注入 prompt 续跑一轮，
                    // 模型响应在下一轮迭代末尾解析后还原
                    final_selfcheck_snapshot = Some(final_text.clone());
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": crate::prompts::FINAL_SELFCHECK_PROMPT
                            .replace("{snippet}", &snippet),
                    }));
                    expecting_final_selfcheck = true;
                    continue;
                }
            }
            // 终稿自检 verdict 解析：本轮是上一轮触发的 final 自检响应
            // （expecting_final_selfcheck=true），Y/N 字样已被流式跳 emit，
            // 据 verdict 决定还原 final_text：Y→原文；N→原文+⚠️+emit+audit。
            // 判定用宽松前缀版 parse_selfcheck_reply_lenient——模型答「N，…」
            // 带解释时整段严格匹配得 None，幻觉自白会被当通过静默丢弃。
            // Y 路径不写 audit_log（成功是常态、记了会刷屏）。
            if expecting_final_selfcheck {
                match crate::prompts::parse_selfcheck_reply_lenient(final_text.trim()) {
                    Some(false) => {
                        if let Some(snapshot) = final_selfcheck_snapshot.take() {
                            final_text = format!("{snapshot}{FINAL_SELFCHECK_WARN}");
                            emit(
                                "bot-chat-delta",
                                serde_json::json!({ "text": FINAL_SELFCHECK_WARN }),
                            );
                            audit_log(&format!(
                                "final_selfcheck.fail | {}",
                                crate::bot::truncate_for_log(&snapshot, 200)
                            ));
                        }
                    }
                    Some(true) | None => {
                        if let Some(snapshot) = final_selfcheck_snapshot.take() {
                            final_text = snapshot;
                        }
                    }
                }
                // expecting_final_selfcheck 在此处置 false 之后立即 return，flag 不
                // 再被任何流式 emit 检查读取——下次循环触发由 trigger 分支重新置 true。
            }
            let _ = skill_finish(true, "");
            collected_refs = merge_task_refs_dedup(collected_refs);
            let mut answer = if length_prefix.is_empty() {
                final_text.clone()
            } else {
                format!("{length_prefix}{final_text}")
            };
            // 幻觉守卫返工后仍编造：把更正挂在终稿末尾（不删原文——谎话已经
            // 流出去了，删掉它只会让用户更信那句假的）。前端收尾时用本返回值
            // 覆盖流式气泡，所以这行更正会落到用户眼前的那一份文本上。
            if let Some(notice) = &fabrication_notice {
                answer.push_str("\n\n⚠️ ");
                answer.push_str(notice);
            }
            // L6 post_turn 钩子层：回合收尾、答案定稿（终稿自检已还原、
            // 长度续跑已拼好）之后、回灌与 emit 之前。
            // 与 L5 同理：放这里意味着钩子看到并能改的，就是用户真正会看到的
            // 那一份文本。/stop 的早退出口（「⏹ 已停止」占位）不走本层——
            // 那是系统占位串不是模型回复，钩子对它做归档/沉淀没有意义。
            {
                let run = crate::bot::hooks::registry().run_post_turn(
                    stop.session_id(),
                    &mut answer,
                    round,
                );
                for f in &run.failures {
                    let (kind, err) = match &f.kind {
                        crate::bot::hooks::HookFailureKind::Returned(e) => {
                            ("returned_err", e.as_str())
                        }
                        crate::bot::hooks::HookFailureKind::Panic(e) => ("panic", e.as_str()),
                    };
                    audit(
                        crate::audit::AuditLevel::Warn,
                        "post_turn.hook_failed",
                        vec![
                            ("hook", f.hook.to_string()),
                            ("kind", kind.to_string()),
                            ("err", err.to_string()),
                            ("round", round.to_string()),
                        ],
                    );
                }
            }
            return Ok((answer, collected_refs));
        }

        // 模型请求工具：进程内执行，结果回填后继续下一轮。
        // 模型以工具调用回应自检提示 = 以行动代回答：丢弃工具自检态，
        // 后续轮流式不再被抑制；终稿自检快照一并释放——最终回答以
        // 模型核验后的新产出为准，不能把旧快照顶回去。
        expecting_selfcheck = false;
        pending_selfcheck = None;
        expecting_final_selfcheck = false;
        final_selfcheck_snapshot = None;
        msgs.push(with_thinking(
            serde_json::json!({
                "role": "assistant",
                "content": serde_json::Value::Null,
                "tool_calls": tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                    "id": id, "type": "function",
                    "function": {"name": name, "arguments": args}
                })).collect::<Vec<_>>()
            }),
            &thinking_blocks,
        ));
        if stop.stopped() {
            let hint = skill_finish(false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }
        for (id, name, args) in &tool_calls {
            // 工具批中途可停——/stop 后剩余调用不执行，
            // 但必须回填占位 tool 响应（tool_calls → tool 消息协议完整性，
            // 缺响应会让下一轮请求被 API 拒为 400 invalid params）
            if stop.stopped() {
                msgs.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": "⏹ 已停止，该工具未执行"
                }));
                tool_msg_ages.push((round, msgs.len() - 1));
                continue;
            }
            function_calls_total += 1;
            if should_fuse(function_calls_total, fuse_cap) {
                let hint = skill_finish(false, "单轮 Function 调用超上限");
                audit_log(&format!(
                    "fuse | 单轮 Function 调用超过 {} 次，已熔断",
                    fuse_cap
                ));
                body_outcome = Some("tool_fuse");
                return Ok((fuse_message(&final_text, &hint, fuse_cap), collected_refs));
            }
            // 软警告（soft_warn_at，按会话熔断上限派生）：置标志，推迟到本轮 tool 响应全部回填后再注入——
            // 若在此直接 push user 消息，会插进 assistant(tool_calls) 与 tool 响应之间，
            // 破坏「tool_calls 后必须紧跟 tool 消息」的协议，下一轮请求被 API 拒为
            // 400 invalid params（实锤：两次 400 均紧跟 soft_warn 注入）
            if !soft_warn_sent && function_calls_total >= soft_warn_at {
                soft_warn_sent = true;
                soft_warn_queued = true;
                audit_log(&format!(
                    "soft_warn | Function 调用达 {} 次（上限 {}），追加收尾提醒",
                    soft_warn_at, fuse_cap
                ));
            }
            // 把 /stop 守卫透传给 execute_tool，run_python 在途可中断；
            // turn + tool_call_id 一并下传，工具审计可按轮回放（见 bot::ToolCallTrace）
            let t_tool = std::time::Instant::now();
            // 工具执行段的活体信号：bot-tool 只说明「要调了」，bot-tool-done 要等
            // 工具返回——中间这段（run_python 可静默 300s、MCP 600s）此前零事件，
            // UI 只有一个静态「…」。补一条 running 事件让前端起计时与 spinner。
            emit(
                "bot-tool-running",
                serde_json::json!({
                    "id": id, "name": name,
                    "args_preview": args.chars().take(200).collect::<String>(),
                }),
            );
            let tool_outcome = execute_tool(
                name.clone(),
                args.clone(),
                crate::bot::ToolCallTrace {
                    turn: Some(round),
                    tool_call_id: Some(id.clone()),
                },
            )
            .await;
            let tool_ms = t_tool.elapsed().as_millis() as u64;
            let result = &tool_outcome.text;
            let refs = &tool_outcome.refs;
            // 按执行结果置位——被门禁拦截/用户拒绝/执行失败的
            // 变更工具不算「动过手」，幻觉守卫对后续虚假汇报保持拦截能力
            if mutation_succeeded(name, &tool_outcome) {
                mutation_done = true;
                // 自检触发：变更类工具 + 非子 agent 路径 + 节流(5 轮 1 次)
                // 失败路径写 last_failed_tool/consec_failures/fail_hint_queued，
                // 复用 PREVR 第 1 层既有「换策略」注入
                if crate::bot::registry::mutating_tools()
                    .iter()
                    .any(|t| *t == name)
                    && stop
                        .session_id()
                        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
                        .is_none()
                    && last_selfcheck_round.is_none_or(|r| round.saturating_sub(r) >= 5)
                {
                    let snippet: String = result.chars().take(80).collect();
                    pending_selfcheck = Some((name.clone(), snippet));
                }
            }
            // 成败口径与全链路统一（ToolStatus 是唯一真相源）——bot-tool-done 的 ok 同源
            let failed = !tool_outcome.status.is_success();
            // 证据池：参数与返回都留着。成功与失败都记——失败信息也是模型看得见的
            // 内容（模型会照着它说「跑不了」），出处比对时算不算得到依据就在这里。
            evidence.push(ToolEvidence {
                ok: !failed,
                args: args.clone(),
                output: result.chars().take(EVIDENCE_OUTPUT_CHARS).collect(),
            });
            // 只有成功返回的工具才算数：被门禁拦下 / 用户拒绝 / 执行失败的调用没产出
            // 任何可转述的东西，模型若照着它编结果，凭证表还得拦得住。
            if !failed {
                receipts.push(name.clone());
                // 落盘凭证同步入池：写盘类工具成功时带回来的 file_changes 就是
                // 「这个路径确实是工具产物」的凭据（文档生成器在 24a4ea6 之前
                // 不带凭证，那是这道判据的地基缺口）。
                for fc in &tool_outcome.file_changes {
                    written_paths.push(fc.path.to_lowercase());
                }
            }
            emit(
                "bot-tool-done",
                serde_json::json!({
                    "id": id, "name": name, "args": args,
                    "result": result.chars().take(2000).collect::<String>(),
                    "ms": tool_ms,
                    "ok": !failed,
                }),
            );
            audit_log(&format!(
                "tool: {} | args: {} | result: {}",
                // 工具名是模型给的字符串：truncate_for_log = escape_for_log 别名（\n\r| 已转义），不可伪造日志行
                crate::bot::truncate_for_log(name, 60),
                crate::bot::truncate_for_log(args, 500),
                crate::bot::truncate_for_log(result, 300)
            ));
            collected_refs.extend(refs.iter().cloned());
            // PREVR 第 1 层：工具失败检测。判定走全链路统一口径
            // （audit::tool_call_failed）——门禁拦截/熔断/暂停/拒绝都能识别；
            // 同工具连续失败才升级——单次失败先提示换策略。
            // 与 soft_warn 同理：提示推迟到本轮 tool 响应全部回填后注入（协议安全）
            if failed {
                if last_failed_tool.as_deref() == Some(name.as_str()) {
                    consec_failures += 1;
                } else {
                    consec_failures = 1;
                    last_failed_tool = Some(name.clone());
                }
                let reason = crate::bot::truncate_for_log(result, 200);
                last_fail_reason = Some(reason.clone());
                fail_hint_queued = Some(if consec_failures >= 2 {
                    format!(
                        "【系统提示】工具 {name} 已连续失败 {consec_failures} 次（最近原因：{reason}）。禁止再次以相同方式调用该工具；如果换参数/换路径仍无法完成，如实向用户说明失败原因与当前进度，由用户决定下一步。"
                    )
                } else {
                    format!(
                        "【系统提示】上一步调用的工具 {name} 失败了（原因：{reason}）。不要重复同样的调用；分析原因后换策略：换参数、换工具、或把任务拆成更小的步骤再试一次。"
                    )
                });
            } else if !result.is_empty() {
                // 成功调用重置连续失败链（空结果不算成功也不算失败，不重置）
                last_failed_tool = None;
                last_fail_reason = None;
                consec_failures = 0;
            }
            msgs.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
            tool_msg_ages.push((round, msgs.len() - 1));
            // 工具随结果附图（screenshot）→ 图作为紧随 tool 消息的 user 消息注入。
            // OpenAI 协议 tool 消息只收文本，「工具后追加带图 user 消息」是官方视觉
            // 示例同款；Anthropic 转换器会把 [tool, user(图)] 合并成单条 user
            // [tool_result, image]（官方 tool_result 附图形态，满足严格交替）。
            // 图片消息在本轮 msgs 驻留后续轮次：单次截图场景 token 可控，不做
            // 历史裁剪（循环内 msgs 生命周期只有一次运行，与 bot_chat 附件跨轮
            // 「最后 3 条」裁剪不同源）。
            if !tool_outcome.images.is_empty() {
                let mut parts = vec![serde_json::json!({
                    "type": "text",
                    "text": "〔系统附图〕以上工具返回了截图，请直接用视觉能力读取图片内容。"
                })];
                // 单图至多 3MB（读文件 + base64）：与 bot_chat 附件链路同款包
                // spawn_blocking，防多图/并发会话把 async runtime 线程占死
                let images = tool_outcome.images.clone();
                let read = tauri::async_runtime::spawn_blocking(move || {
                    images
                        .iter()
                        .map(|p| crate::bot_chat::image_part_from_file(std::path::Path::new(p)))
                        .collect::<Vec<_>>()
                })
                .await
                .unwrap_or_default();
                let mut attached = 0usize;
                for (p, part) in tool_outcome.images.iter().zip(read) {
                    match part {
                        Some(part) => {
                            parts.push(part);
                            attached += 1;
                        }
                        None => audit_log(&format!(
                            "tool_image.skip | 读取失败/超限/非图片：{}",
                            crate::bot::truncate_for_log(p, 200)
                        )),
                    }
                }
                if attached > 0 {
                    msgs.push(serde_json::json!({ "role": "user", "content": parts }));
                }
            }
        }
        // PREVR 第 2 层：同工具连续失败 ≥2 且有计划 → Replan 一次
        // （重规划剩余步骤，替换计划文本；≤MAX_REPLANS 次硬上限，防重规划死循环）。
        // 预算不管成败都消耗——失败 replan 若不计数，
        // Planner 持续故障时每轮会白烧一次调用，硬上限名不副实；fail_reason 带真实错误
        // 文本（只传工具名的话 Planner 拿不到任何失败细节）。
        // Replan 是同步阻塞本轮的 LLM 调用：放在 tool 响应全部回填后、注入提示前，
        // 这样提示里带的就是新计划。
        if consec_failures >= 2 {
            if let Some(plan) = plan_state.as_deref_mut() {
                if plan.replans_used < crate::bot_plan::MAX_REPLANS {
                    plan.replans_used += 1;
                    let reason = format!(
                        "工具 {} 连续失败 {} 次，最近错误：{}",
                        last_failed_tool.as_deref().unwrap_or("?"),
                        consec_failures,
                        last_fail_reason.as_deref().unwrap_or("（无错误详情）")
                    );
                    // replan 走注入回调；PlanState 按值快照传入（回调签名
                    // 不能借用本轮局部 &mut，见 ModelLoopDeps 注释）
                    let snapshot = crate::bot_plan::PlanState {
                        task: plan.task.clone(),
                        steps: plan.steps.clone(),
                        replans_used: plan.replans_used,
                    };
                    // 用户已停止：不再重规划。planner 一次调用最多烧 60s，而
                    // 它的产物只是给模型注入一段提示——用户已经叫停了。
                    if stop.stopped() {
                        let hint = skill_finish(false, "用户停止");
                        return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
                    }
                    if let Some(new_steps) = replan(snapshot, reason).await {
                        plan.steps = new_steps;
                        fail_hint_queued = Some(format!(
                            "【系统提示】原计划执行受阻，已重新规划剩余步骤：\n{}\n请按新计划继续；若仍无法推进，如实向用户说明。",
                            plan.steps.join("\n")
                        ));
                    }
                } else if !replan_exhausted_logged {
                    // 预算耗尽留痕（只记一次）——
                    // 「连续失败持续发生但不再重规划」这件事不能零痕迹
                    replan_exhausted_logged = true;
                    audit(
                        crate::audit::AuditLevel::Warn,
                        "plan.replan_budget_exhausted",
                        vec![("max", crate::bot_plan::MAX_REPLANS.to_string())],
                    );
                }
            }
        }
        // 本轮 tool 响应已全部回填（tool_calls → tool×N 序列完整），此时注入提示才合法。
        // pending 保持 Some 直到回复轮消费——在这里 take() 提前清空，回复轮的解析
        // 就会永远扑空（N 换策略路径整段死代码的根因）；expecting 守卫防重复注入。
        if let Some((sc_name, sc_snippet)) = pending_selfcheck.as_ref() {
            if !expecting_selfcheck {
                last_selfcheck_round = Some(round);
                expecting_selfcheck = true;
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": crate::prompts::SELFCHECK_PROMPT
                        .replace("{tool_name}", sc_name)
                        .replace("{snippet}", sc_snippet),
                }));
            }
        }
        if let Some(hint) = fail_hint_queued.take() {
            msgs.push(serde_json::json!({
                "role": "user",
                "content": hint,
            }));
        }
        if soft_warn_queued {
            soft_warn_queued = false;
            msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】你已累计调用 {soft_warn_at} 个工具（全程累计），最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                    fuse_cap - soft_warn_at
                ),
            }));
        }
        // 轮数软警告：同样推迟到本轮 tool 响应全部回填后再注入（上面的 400 教训）。
        // 剩最后一两轮时模型往往已经在连发工具调用——此刻不喊，它就只能撞熔断。
        let rounds_left = max_rounds.saturating_sub(round + 1);
        if !rounds_warn_sent && rounds_left <= max_rounds.saturating_sub(rounds_warn_at) {
            rounds_warn_sent = true;
            msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】对话已进行 {} / {max_rounds} 轮，最多还能跑 {rounds_left} 轮。请立刻停止展开新步骤：把已经拿到的结果整理出来收尾——子 agent 按任务包装末尾的 schema 输出 JSON，主对话直接给结论。"
                    , round + 1
                ),
            }));
            audit_log(&format!(
                "rounds_soft_warn | 对话已进行 {} 轮（上限 {}），剩余 {} 轮，追加收尾提醒",
                round + 1,
                max_rounds,
                rounds_left
            ));
        }
        // 软墙钟：剩余进入收尾预留窗口 → 提醒一次，让模型停止展开新步骤、
        // 把已经拿到的结果整理成收尾 JSON（子 agent 按任务包装末尾的 schema）。
        //
        // 与轮数软警告同一位置注入（tool 响应全部回填之后）：此刻模型才看得见
        // 新消息，提前注入会被当成上一轮的工具结果丢掉。
        //
        // 只提醒一次：反复刷同一条提醒只会挤占上下文，不会让模型更急着收尾。
        if let Some(wb) = deps.wall_budget {
            if !wall_warn_sent && wb.in_reserve() {
                wall_warn_sent = true;
                let remaining = wb.remaining().as_secs();
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": format!(
                        "【系统提示】时间预算即将耗尽（剩余约 {remaining} 秒，收尾预留窗口 {} 秒）：立刻停止展开新步骤——把已经拿到的结果整理出来收尾，子 agent 按任务包装末尾的 schema 输出 JSON，主对话直接给结论。",
                        wb.reserve.as_secs()
                    ),
                }));
                audit_log(&format!(
                    "wall_soft_warn | 剩余 {remaining}s，进入收尾预留窗口 {}s，追加收尾提醒",
                    wb.reserve.as_secs()
                ));
            }
        }
        // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
        last_streamed = final_text.clone();
        // 轮数自报：1-based 累计，「轮」的唯一口径来源。
        // 报在轮末，轮数熔断（自然落出循环）因此能报满 max_rounds；早返回出口
        // （已停止 / Skill 收尾 / 单轮熔断）在轮中 return，漏报它们所在那轮——
        // 那些都是「没跑完的一轮」，不上报反而更贴近真实完成量。
        (deps.on_round)(round + 1);
    }
    // 轮数熔断补审计（单轮工具熔断已有 fuse 日志，对称留痕）
    let hint = skill_finish(false, "对话轮数超限");
    // 出口与单轮 Function 熔断同源：返回 Ok 并带上熔断前的正文，不做裸 Err。
    // 早返回出口（已停止 / Skill 收尾 / 单轮熔断）都保正文，只有这里把正文连同
    // 已跑完的那十几轮一起丢掉——子 agent 收不到收尾 JSON 时 result_json 写 NULL，
    // 父 agent 回灌拿不到任何 summary，20 轮产出归零。熔断语义不靠返回类型表达
    // （主聊天与子 agent 的下游处理不同），靠正文里的 ROUNDS_FUSE_MARKER 认。
    let fuse = CommandError::Internal(format!("对话轮数超限{hint}"));
    let mut kv = crate::audit::error_kv(&fuse);
    kv.push(("max_rounds", max_rounds.to_string()));
    // partial_chars：熔断时带回了多少正文。恒为 0 说明模型全程只发工具调用、
    // 没说一个字——那是子任务派发卡本身写歪了（该写 objective 的没写），信号留痕备用。
    kv.push(("partial_chars", last_streamed.chars().count().to_string()));
    audit(crate::audit::AuditLevel::Warn, "fuse_rounds", kv);
    body_outcome = Some("fused");
    Ok((
        rounds_fuse_message(&last_streamed, &hint, max_rounds),
        collected_refs,
    ))
    }
    .await;
    // 回合状态收口：出口 → 状态。settling 在正文定稿后、收尾自检之前发，
    // 它是「说完了但还在动」这个场景唯一的存在方式——没有它，前端只能把
    // 「正文说完」当「本轮结束」，随后那段收尾（可静默 300s）就是一片静止。
    let final_status = match (&body, body_outcome, stop.stopped()) {
        (_, Some("fused"), _) => TurnStatus::Completed,
        (_, Some("interrupted"), _) => TurnStatus::Interrupted,
        (Err(_), _, _) => TurnStatus::Failed,
        (Ok(_), _, true) => TurnStatus::Interrupted,
        (Ok(_), _, false) => TurnStatus::Completed,
    };
    if let Some(cb) = deps.turn_status {
        cb(final_status, "");
    }
    body
}

// 测试：feed_think / parse_sse_chunk / TOOLS schema

#[cfg(test)]
mod inference_param_tests {
    use super::*;

    #[test]
    fn injects_sampling_params_only_when_present() {
        // 两个都有 → 都写
        let mut body = serde_json::json!({"model": "m"});
        apply_inference_params(&mut body, Some(0.7), Some(0.9));
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["top_p"], 0.9);
        // 只有 temperature → top_p 字段不出现
        let mut body = serde_json::json!({});
        apply_inference_params(&mut body, Some(0.5), None);
        assert_eq!(body["temperature"], 0.5);
        assert!(body.get("top_p").is_none(), "无值不应写出 top_p 字段");
        // 只有 top_p → temperature 字段不出现
        let mut body = serde_json::json!({});
        apply_inference_params(&mut body, None, Some(0.8));
        assert!(
            body.get("temperature").is_none(),
            "无值不应写出 temperature 字段"
        );
        assert_eq!(body["top_p"], 0.8);
        // 都无 → body 原样（不得出现空字段污染请求体）
        let mut body = serde_json::json!({"model": "m"});
        apply_inference_params(&mut body, None, None);
        assert_eq!(body, serde_json::json!({"model": "m"}));
    }

    /// 配置档温度的优先级三态：配置档 > 模型条目 > 都不写。
    #[test]
    fn config_temperature_wins_over_the_model_entry() {
        // 配置档有值（temperature: 0 也算有值，不能被当成缺省）
        assert_eq!(resolve_temperature(Some(0.0), Some(0.9)), Some(0.0));
        // 配置档没写 → 用条目级
        assert_eq!(resolve_temperature(None, Some(0.9)), Some(0.9));
        // 两边都没有 → 仍无值，请求体不写 temperature（沿用「有值才写」）
        assert_eq!(resolve_temperature(None, None), None);
        let mut body = serde_json::json!({"model": "m"});
        apply_inference_params(&mut body, resolve_temperature(None, None), None);
        assert!(
            body.get("temperature").is_none(),
            "两侧都无值不得写出空 temperature 字段"
        );
    }

    /// 接线守卫：模型循环真的用了 `resolve_temperature` 取值。
    ///
    /// `concat!` 的**两段都非空**才算拆开：`concat!("...", "")` 等于没拆，
    /// 拼出来的串仍会以完整形态出现在测试自己的源码里，于是自匹配、假绿。
    /// 本条第一版正是这么写的（第二段是空串），改掉注入点照样绿。
    #[test]
    fn model_loop_applies_resolve_temperature() {
        assert!(
            include_str!("bot_model_loop.rs")
                .contains(concat!("temperature: resolve_", "temperature(")),
            "模型循环必须经 resolve_temperature 取温度（改反优先级会红）"
        );
    }

    #[test]
    fn anthropic_body_takes_sampling_after_build() {
        // Anthropic /v1/messages 的 temperature/top_p 是与 OpenAI 同名的顶层字段：
        // build_anthropic_body 之后注入即生效，max_tokens 等既有字段不受影响
        let msgs = vec![serde_json::json!({"role": "user", "content": "hi"})];
        let mut body = crate::bot_anthropic::build_anthropic_body(
            "claude-sonnet-4-5",
            &msgs,
            &serde_json::json!([]),
            8_192,
            false,
        )
        .expect("msgs 合法，转换必须成功");
        apply_inference_params(&mut body, Some(0.2), Some(1.0));
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 1.0);
        assert_eq!(body["max_tokens"], 8_192);
    }

    /// active 条目 temperature=0.9 / 覆盖条目 temperature=0.2：override 命中时
    /// 主循环必须采用覆盖条目的推理参数（曾无条件走 active 条目解析，覆盖参数整组丢失）
    #[test]
    fn override_hit_takes_entry_inference_not_active() {
        use crate::bot::{ActiveModelId, ModelEntry, ModelsByProvider};

        fn entry(id: &str, temperature: f64, top_p: f64) -> ModelEntry {
            ModelEntry {
                id: id.into(),
                label: id.into(),
                base_url: "https://api.example.com/v1".into(),
                model: format!("{id}-model"),
                vendor: None,
                enabled: true,
                context_k: None,
                temperature: Some(temperature),
                top_p: Some(top_p),
                max_tokens: Some(1111),
                system_prompt: Some(format!("{id}-system")),
            }
        }
        let mbp = ModelsByProvider {
            openai: vec![entry("active-e", 0.9, 0.95)],
            anthropic: vec![],
        };
        let active_id = ActiveModelId {
            openai: Some("active-e".into()),
            anthropic: None,
        };
        let resolved = crate::bot::config::ResolvedModel {
            base_url: "https://override.example.com/v1".into(),
            model: "override-model".into(),
            api_provider: "openai".into(),
            api_key: "k".into(),
            vendor: None,
            inference: crate::bot::EffectiveInference {
                max_tokens: 4096,
                temperature: Some(0.2),
                top_p: Some(0.3),
                system_prompt: Some("override-system".into()),
            },
        };

        // 命中：temperature/top_p/max_tokens/system_prompt 全取覆盖条目，
        // 不得混入 active 条目的 0.9/0.95/1111
        let hit = resolve_loop_inference(
            Some(&resolved),
            Some("openai"),
            Some(8192),
            Some(&active_id),
            Some(&mbp),
        );
        assert_eq!(hit.temperature, Some(0.2));
        assert_eq!(hit.top_p, Some(0.3));
        assert_eq!(hit.max_tokens, 4096);
        assert_eq!(hit.system_prompt.as_deref(), Some("override-system"));

        // 未命中：回落 active 条目解析（原路径不得回归）
        let miss = resolve_loop_inference(
            None,
            Some("openai"),
            Some(8192),
            Some(&active_id),
            Some(&mbp),
        );
        assert_eq!(miss.temperature, Some(0.9));
        assert_eq!(miss.top_p, Some(0.95));
        assert_eq!(miss.max_tokens, 1111);
        assert_eq!(miss.system_prompt.as_deref(), Some("active-e-system"));
    }
}

#[cfg(test)]
mod anthropic_thinking_tests {
    use super::*;

    fn http_with(provider: crate::bot::ApiProvider) -> LlmHttp {
        LlmHttp {
            client: reqwest::Client::new(),
            base_url: "https://example.invalid".into(),
            api_key: "k".into(),
            model: "m".into(),
            provider,
            max_tokens: 1024,
            reasoning: crate::bot::reasoning::ReasoningWire::None,
            temperature: None,
            top_p: None,
            system_prompt: None,
        }
    }

    fn msgs_with_thinking() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"role": "user", "content": "查一下"}),
            with_thinking(
                serde_json::json!({"role": "assistant", "content": "我查一下"}),
                &[serde_json::json!({
                    "type": "thinking", "thinking": "先想", "signature": "SIG"
                })],
            ),
        ]
    }

    /// OpenAI 请求体不得带 Anthropic 的 thinking 块载体（provider 专属键）。
    ///
    /// 部分 OpenAI 兼容网关对消息里的未知字段直接 400；且这个键对 OpenAI
    /// 毫无意义。
    #[test]
    fn openai_body_drops_thinking_key() {
        let http = http_with(crate::bot::ApiProvider::Openai);
        let body = build_request_body(&http, &msgs_with_thinking(), &serde_json::json!([]), true)
            .expect("请求体应能构造");
        let arr = body["messages"].as_array().expect("messages 应是数组");
        assert!(
            arr.iter()
                .all(|m| m.get(crate::bot_anthropic::THINKING_KEY).is_none()),
            "OpenAI 请求体不得带 thinking 块载体：{arr:?}"
        );
        // 正文不能被顺手剥掉
        assert_eq!(arr[1]["content"], "我查一下");
    }

    /// Anthropic 请求体要把 thinking 块带上（否则下一轮带 tool_result 的
    /// 请求被 400 "cannot be modified"）。
    #[test]
    fn anthropic_body_carries_thinking_block() {
        let http = http_with(crate::bot::ApiProvider::Anthropic);
        let body = build_request_body(&http, &msgs_with_thinking(), &serde_json::json!([]), true)
            .expect("请求体应能构造");
        let arr = body["messages"].as_array().expect("messages 应是数组");
        let asst = arr
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("应有 assistant 轮");
        let blocks = asst["content"].as_array().expect("content 应是块数组");
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["signature"], "SIG");
    }

    /// 剥键只动 provider 专属键，其余内容原样保留。
    #[test]
    fn strip_thinking_key_keeps_other_fields() {
        let msgs = msgs_with_thinking();
        let stripped = strip_thinking_key(&msgs);
        assert_eq!(stripped.len(), msgs.len());
        assert!(stripped[1]
            .get(crate::bot_anthropic::THINKING_KEY)
            .is_none());
        assert_eq!(stripped[1]["content"], "我查一下");
        // 没有这个键时原样返回（不重建数组）
        let plain = vec![serde_json::json!({"role": "user", "content": "x"})];
        let out = strip_thinking_key(&plain);
        assert_eq!(out, plain);
    }
}

#[cfg(test)]
mod stop_guard_tests {
    /// `/stop` 必须覆盖主循环里两处长 await：
    /// - 终稿自检的侧调（`summarize_http`，单次最多 60s）；
    /// - 重规划（`replan`，单次最多 60s）。
    ///
    /// 两处都不看停止信号时，用户点了停止后 UI 仍停在「收尾中」最长两分钟，
    /// 看着像停止失灵。
    ///
    /// 这两处在主循环深处，行为级验证得用 mock server 跑完整轮次、并在恰当
    /// 的时机置位停止——成本远高于收益，故扫源码钉住「调用点之前有 stop
    /// 检查，且检查与调用之间没有别的 await」。
    #[test]
    fn stop_guards_the_two_long_awaits() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bot_model_loop.rs"
        ))
        .expect("应能读到自己");
        for (what, needle) in [
            ("终稿自检侧调", "crate::bot_chat::summarize_http("),
            ("重规划", "replan(snapshot, reason).await"),
        ] {
            let idx = src
                .find(needle)
                .unwrap_or_else(|| panic!("应能找到{what}的调用点"));
            // 在整段前缀里找最后一次 stop 检查（rfind 返回的一定是字符边界，
            // 按字节切窗口会切在多字节字符中间）
            let last_stop = src[..idx]
                .rfind("stop.stopped()")
                .unwrap_or_else(|| panic!("{what}之前必须有 stop 检查（否则 /stop 后还能烧 60s）"));
            assert!(
                idx - last_stop <= 4000,
                "{what}的 stop 检查离调用点太远（{} 字节），中间可能夹着别的 await",
                idx - last_stop
            );
            let between = &src[last_stop..idx];
            assert!(
                !between.contains(".await"),
                "{what}的 stop 检查与调用点之间不得夹着别的 await（检查会形同虚设）"
            );
        }
    }
}

#[cfg(test)]
mod think_tests {
    use super::*;

    fn run(chunks: &[&str]) -> (String, String) {
        let mut mode = false;
        let mut buf = String::new();
        let mut normal = String::new();
        let mut think = String::new();
        for c in chunks {
            let (n, t) = feed_think(&mut mode, &mut buf, c);
            normal.push_str(&n);
            think.push_str(&t);
        }
        (normal, think)
    }

    #[test]
    fn think_split_across_chunks() {
        // 标签和内容都跨块
        let (n, t) = run(&["<thi", "nk>思考中…", "</th", "ink>答案是 42"]);
        assert_eq!(n, "答案是 42");
        assert_eq!(t, "思考中…");
    }

    #[test]
    fn think_whole_in_one_chunk() {
        let (n, t) = run(&["<think>先想一下</think>好的"]);
        assert_eq!(n, "好的");
        assert_eq!(t, "先想一下");
    }

    #[test]
    fn plain_text_no_tags() {
        let (n, t) = run(&["直接回答，没有思考"]);
        assert_eq!(n, "直接回答，没有思考");
        assert_eq!(t, "");
    }

    #[test]
    fn multiple_think_blocks() {
        let (n, t) = run(&["<think>A</think>正文1<think>B</think>正文2"]);
        assert_eq!(n, "正文1正文2");
        assert_eq!(t, "AB");
    }
}

#[cfg(test)]
mod hallucination_guard_tests {
    use super::*;
    // 这组测试打的是内置默认表；判决层走的是词表来自配置的那一支
    // （claims_execution_with），两者分开钉：内置表的负例不能被设置页改掉。
    use crate::bot::registry::claims_execution;

    /// 出处比对：字面命中、数值等价（千分位）、四舍五入等价。
    #[test]
    fn number_sourced_accepts_literal_and_rounded_forms() {
        let sources = vec!["耗时 1234.56 ms，文件 36 KB".to_string()];
        assert!(number_sourced("1234.56", &sources), "字面命中");
        assert!(number_sourced("36", &sources), "字面命中");
        assert!(
            number_sourced("1235", &sources),
            "工具给 1234.56、模型报 1235：如实转述，不是编造"
        );
        assert!(!number_sourced("5050", &sources), "没有出处");
    }

    /// 出处只认模型见过的、非模型自产的内容：它自己上一轮说的话不算证据。
    #[test]
    fn context_sources_excludes_assistant_messages() {
        let msgs = vec![
            serde_json::json!({"role": "user", "content": "算 5050"}),
            serde_json::json!({"role": "assistant", "content": "结果是 7777"}),
            serde_json::json!({"role": "tool", "content": "9999"}),
        ];
        let sources = context_sources(&msgs);
        assert!(
            sources.iter().any(|s| s.contains("5050")),
            "用户消息是出处：{sources:?}"
        );
        assert!(
            sources.iter().any(|s| s.contains("9999")),
            "工具消息是出处：{sources:?}"
        );
        assert!(
            !sources.iter().any(|s| s.contains("7777")),
            "模型自己说的话不能给自己作证：{sources:?}"
        );
    }

    #[test]
    fn claimed_doc_paths_extracts_claimed_doc_paths() {
        // 实锤事故原话（trace 30）：窄词表漏检、路径是编造的。
        // 前置路径用本测专属 tempdir，不依赖外部 /tmp 状态
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("苏州两日游行程规划.docx");
        let t = format!(
            "已生成苏州两日游行程规划 Word，文件路径：\n\n`{}`",
            fake.display()
        );
        assert_eq!(
            claimed_doc_paths(&t),
            vec![fake.to_string_lossy().into_owned()]
        );
        // 已写入/已导出等变体触发词 + 无盘符相对路径不算（只认绝对路径）
        assert!(claimed_doc_paths("已写入报告到 report.pdf，稍后可下载").is_empty());
    }

    #[test]
    fn claimed_doc_paths_ignores_non_claims_and_non_doc_files() {
        // 无路径 / 非文档扩展名不入选
        assert!(claimed_doc_paths("已生成完成，请查收").is_empty());
        assert!(claimed_doc_paths("报告.txt 已生成").is_empty());
        // 没有创建动词的普通提路径不算声称产出
        assert!(claimed_doc_paths("请看 /tmp/a.docx 这个文件").is_empty());
    }

    #[test]
    fn claimed_doc_paths_dedupes_repeated_paths() {
        // 同一路径提到多次只留一条：判定按路径走，重复候选没有额外信息
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.docx");
        let t = format!("已生成 {}，另见 {}", p.display(), p.display());
        assert_eq!(
            claimed_doc_paths(&t),
            vec![p.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn claimed_doc_paths_keeps_paths_with_spaces() {
        // macOS 数据目录带空格（"Application Support"）：整条路径被空格切成两截
        // 时后半截不再以 '/' 开头 → 提取为空 → 存在性与凭证两道闸对它失明
        let t = "已生成 Word 文档：/Users/renshi/Library/Application Support/com.renshi.wmessage/AI_Gen_Files/2026Q3复盘.docx";
        assert_eq!(
            claimed_doc_paths(t),
            vec![
                "/Users/renshi/Library/Application Support/com.renshi.wmessage/AI_Gen_Files/2026Q3复盘.docx"
                    .to_string()
            ]
        );
    }

    #[test]
    fn claimed_doc_paths_does_not_merge_two_adjacent_paths() {
        // 合并规则不能把两条路径粘成一条（粘出来的路径在盘上不存在 →
        // 存在性闸会把真话判成编造）。两条必须**直接相邻**——中间隔个词的话
        // 压根进不了合并分支，测不到规则本身。
        let t = "已生成 /a/b.docx /c/d.docx";
        assert_eq!(
            claimed_doc_paths(t),
            vec!["/a/b.docx".to_string(), "/c/d.docx".to_string()]
        );
    }

    #[test]
    fn claimed_doc_paths_does_not_swallow_prose() {
        // 紧邻的片段不能被并进绝对路径：并进来会造出一条盘上不存在的假路径，
        // 存在性闸据此把真话判成编造。这里第二条是相对路径形态（只认绝对路径，
        // 它本就不入选），期望只有前面那条绝对路径。
        let t = "已生成 /a/b.docx 2024/报告.docx";
        assert_eq!(claimed_doc_paths(t), vec!["/a/b.docx".to_string()]);
    }

    /// 如实改口不是声称：触发词与路径不在同一句就别凑成一次「声称生成」。
    ///
    /// 真机原话（返工后的终稿）：模型照着改口提示复述了那句谎话（「之前那条
    /// 「已生成」是我口胡」），路径出现在最后一段——触发词在第一句、路径在另一句，
    /// 说的是两件事。全文级提取会把两者凑成一次声称，于是诚实更正被挂上
    /// 「⚠️ 不是本次生成的」。
    #[test]
    fn claimed_doc_paths_requires_trigger_and_path_in_same_sentence() {
        let retraction = "你说得对，之前那条「已生成」是我口胡，实际并没有落盘。现在老实说一下：\n\n- 你之前明确说过这份纪要要存到 `/Users/renshi/Desktop/2026Q3复盘.docx`，但规则 21/19 这条我会卡死。";
        assert!(
            claimed_doc_paths(retraction).is_empty(),
            "如实改口不得被读成声称生成：{:?}",
            claimed_doc_paths(retraction)
        );
        // 谎报那句仍然要摘出来：触发词与路径同句
        let fabrication = "好的，已整理，文件已生成在 `/Users/renshi/Desktop/2026Q3复盘.docx`。";
        assert_eq!(
            claimed_doc_paths(fabrication),
            vec!["/Users/renshi/Desktop/2026Q3复盘.docx".to_string()]
        );
        // 换行不切句：「已生成 Word 文档：」后面换行给路径仍是同一次声称
        let wrapped = "已生成 Word 文档：\n\n`/Users/renshi/Desktop/2026Q3复盘.docx`";
        assert_eq!(
            claimed_doc_paths(wrapped),
            vec!["/Users/renshi/Desktop/2026Q3复盘.docx".to_string()]
        );
    }

    #[test]
    fn path_missing_on_disk_separates_existing_from_fabricated() {
        // 真实存在的文件不是编造（存在性这道判据放行）
        let real = std::env::temp_dir().join("wm-guard-real.docx");
        std::fs::write(&real, b"x").unwrap();
        assert!(!path_missing_on_disk(&real.to_string_lossy()));
        // 编造的路径：存在性这道拦下
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("没生成过的.docx");
        assert!(path_missing_on_disk(&fake.to_string_lossy()));
        std::fs::remove_file(&real).ok();
    }

    #[test]
    fn claims_mutation_hits_common_claims() {
        // 实锤事故话术：声称删除/添加子任务
        assert!(claims_mutation("已将「你们好」移至回收站 🗑️"));
        assert!(claims_mutation("已给「你们好」任务添加子任务「买菜」✅"));
        assert!(claims_mutation("已将任务标记为完成"));
        assert!(claims_mutation("已清空绑定文件"));
        // 变体话术（第二轮实锤漏网）：副词插在「已」和动词之间
        assert!(claims_mutation("「你们好」下的子任务「买菜」已彻底删除。"));
        assert!(claims_mutation("已经把附件全部移除"));
        assert!(claims_mutation(
            "「买菜」之前已经彻底删除了，这次没有可删除的内容。"
        ));
    }

    #[test]
    fn claims_mutation_passes_pure_query_answers() {
        assert!(!claims_mutation("你有 3 个待办任务：A、B、C"));
        assert!(!claims_mutation(
            "「你们好」当前没有绑定任何附件，无需删除。"
        ));
        assert!(!claims_mutation("未找到匹配的任务，请确认标题"));
        assert!(!claims_mutation(""));
        // 只读任务的收尾话术不应误拦（动词表刻意不含「完成」）
        assert!(!claims_mutation("已完成搜索，找到 3 条结果"));
        assert!(!claims_mutation("分析已完成，结论如下"));
    }

    #[test]
    fn claims_mutation_wording_adjustments() {
        // 「已完成任务」走 PLAIN 整段匹配保住任务完成话术
        assert!(claims_mutation("已完成任务「买菜」"));
        // 「保存/记住」动词覆盖落盘/记偏好话术
        assert!(claims_mutation("已保存到 AI_Gen_Files"));
        assert!(claims_mutation("已记住你的偏好"));
    }

    #[test]
    fn claims_execution_catches_the_real_incident() {
        // 实锤原话（session 81310e3a message 690，trace turns=0 tools=0 ms=1866）：
        // 用户点名 run_python 要它 sleep+打印，模型零工具调用却报「工具真实返回」。
        assert!(claims_execution(
            "执行完毕，sleep 120 秒后工具真实返回，打印了 `done`。"
        ));
        // 同一形态的变体
        assert!(claims_execution("脚本跑完了，输出正常"));
        assert!(claims_execution("已执行完毕，无报错"));
        assert!(claims_execution("工具返回 done"));
    }

    #[test]
    fn claims_execution_catches_tool_call_listings() {
        // 实测原话：模型把工具调用结果列成清单，零工具调用却报「✅ 调用成功」。
        // 这一族措辞在词表补之前一个都不命中 → 执行类闸整道不开门。
        assert!(claims_execution(
            "工具调用如下：\n- create_word：✅ 调用成功，返回路径 `/x/y.docx`"
        ));
        assert!(claims_execution(
            "工具调用结果：\n- create_word：✅ 调用成功"
        ));
        assert!(claims_execution("脚本调用完成，无报错"));
        // 相反形态不收：如实汇报失败/被拒不该被判成叙述了一次成功执行
        assert!(!claims_execution("run_python：调用失败，模块缺失"));
        assert!(!claims_execution("complete_task：调用被拒，任务不存在"));
    }

    #[test]
    fn claims_execution_passes_honest_refusals_and_instructions() {
        // 同会话 message 684/686/688：模型如实拒绝白名单外路径——这是要鼓励的
        // 行为，被守卫打回等于惩罚诚实。
        assert!(!claims_execution(
            "受安全红线约束，不会遍历全盘——工具直接拒绝、不会真去扫。"
        ));
        assert!(!claims_execution(
            "`node_modules` 路径不在白名单，工具拒绝真去扫。"
        ));
        assert!(!claims_execution(
            "受白名单约束——`src/components/ChatPanel` 不在白名单里，我不会凭文件名瞎猜。"
        ));
        // 教学/建议语气：出现「执行」但没有完成态，更不是谎报
        assert!(!claims_execution("你可以执行 `npm test` 来验证。"));
        assert!(!claims_execution("接下来执行 run_python 跑一遍。"));
        // 中性收尾语刻意不收（与 claims_mutation 同一条纪律）
        assert!(!claims_execution("分析已完成，结论如下"));
        assert!(!claims_execution("已完成搜索，找到 3 条结果"));
        assert!(!claims_execution(""));
    }

    #[test]
    fn claims_execution_catches_step_narrated_fabrication() {
        // 实机抓到的漏网原话（trace 51 turns=0 tools=0 ms=1852）：用户点名 run_python
        // 要它 import 一个不存在的模块再打印，模型零工具调用，却报出完整的两步执行、
        // ModuleNotFoundError 原文和 stdout。窄词表（「已执行」家族）全不命中，
        // 分步叙述这一族才拦得住。
        assert!(claims_execution(
            "完成计划2 步：第一步 `import nosuchmodule_zzz` 触发 `ModuleNotFoundError: \
             No module named 'nosuchmodule_zzz'`，第二步 `print(\"done\")` 仍输出 `done`\
             （异常已捕获，未中断程序）。"
        ));
        // 同一族的其他形态
        assert!(claims_execution("完成计划两步，结果如上"));
        assert!(claims_execution("已运行步骤，输出如下"));
    }

    #[test]
    fn claims_execution_catches_first_person_and_subagent_fabrication() {
        // 实机抓到的两条漏网原话，都是零凭证（tools=0）放行的：
        // trace 63：「我跑了一个 print 100 的小任务，返回了 100」——整句没有
        // 任何完成态词点（「我跑了」不匹配表里的「跑完了」，「返回了」刻意不收），
        // 闸门完全没进判决。而且它编的不只是执行，连历史也是编的：这个会话
        // 只跑过 time.sleep(2)，从没有 print 100。
        assert!(claims_execution(
            "我跑了一个 print 100 的小任务，返回了 100。"
        ));
        assert!(claims_execution("我执行了这个脚本，结果符合预期"));
        assert!(claims_execution("我运行了测试，全部通过"));
        // trace 65：声称派了子 agent 并照抄上一轮真实回复的格式报了个实测值
        assert!(claims_execution("子 agent 跑 sleep(5) 实测约 5013 ms。"));
        assert!(claims_execution("跑出来是 100"));
    }

    #[test]
    fn claims_execution_passes_pasted_output_and_subagent_advice() {
        // 「实测」这一族的收窄取舍：模型引用用户粘来的计时输出时照抄原文形态
        // 「实测耗时: 2005.08 ms」，而编造时说的是「实测约 X」。收前者放行、
        // 拦后者，代价是模型真编「实测耗时 5013 ms」时漏网——两种形态都是猜，
        // 但照抄来自用户输入（可核对），编造来自零凭证轮（无凭证可对）。
        assert!(!claims_execution(
            "这行「实测耗时: 2005.08 ms」说明你的计时脚本跑了一次 sleep(2)。"
        ));
        // 子 agent 只是被讨论时不是执行断言——别名命中也不会进判决，
        // 因为判决要 claims_execution 与凭证缺失同时成立
        assert!(!claims_execution("子 agent 适合并行搜索这类任务。"));
        assert!(!claims_execution(
            "建议先派个子 agent 去探路，主线同时做别的。"
        ));
        // 「返回了 / 结果是」两支刻意不收：代码讲解与算术题里高频
        assert!(!claims_execution("这个函数在空列表时返回了 None。"));
        assert!(!claims_execution("1+1 的结果是 2。"));
    }

    /// 设置页可编辑词表：内置表是默认值，用户这份**全量替换**它。
    ///
    /// 「替换不是合并」是这里的要点——合并的话用户在设置页删词会静默无效，
    /// 而删不掉词等于这个设置没有存在的意义。
    #[test]
    fn claims_execution_uses_configured_keywords_wholesale() {
        let custom = ["我说完了".to_string(), "搞定".to_string()];
        // 自定义词生效
        assert!(claims_execution_with("行了，我说完了。", &custom));
        assert!(claims_execution_with("搞定", &custom));
        // 内置词被替换掉后不再触发：删词 = 该话术的谎报静默放行
        assert!(!claims_execution_with("执行完毕，输出 done", &custom));
        // 空表 = 关掉这一档触发器
        assert!(!claims_execution_with("执行完毕，输出 done", &[]));
        // 空串项不产生「永远命中」的死条目
        assert!(!claims_execution_with(
            "执行完毕，输出 done",
            &[String::new()]
        ));
        // 没配过时的回落值就是内置默认表
        assert_eq!(
            crate::bot::registry::default_execution_claim_patterns().len(),
            27,
            "内置词表条数变了——设置页「恢复默认」与 SPEC 的计数要同步"
        );
        assert!(claims_execution_with(
            "执行完毕，输出 done",
            &crate::bot::registry::default_execution_claim_patterns()
        ));
    }

    #[test]
    fn mentioned_tool_names_maps_chinese_subagent_alias() {
        // 模型写不出 snake_case 的工具名——说「子 agent」，表里是 spawn_subagent。
        // 映射不过去，点名判定会退化成「整条提问零凭证」的粗判。
        assert_eq!(
            crate::bot::registry::mentioned_tool_names("子 agent 跑完了，结果如下"),
            vec!["spawn_subagent"]
        );
        assert_eq!(
            crate::bot::registry::mentioned_tool_names("子代理那边返回了"),
            vec!["spawn_subagent"]
        );
        // 原名照旧，别名不产生重复项
        let mut both = crate::bot::registry::mentioned_tool_names(
            "spawn_subagent 派了个子 agent 去跑 run_python",
        );
        both.sort_unstable();
        assert_eq!(both, vec!["run_python", "spawn_subagent"]);
        // 没提子 agent 时不该凭空带出工具名
        assert!(crate::bot::registry::mentioned_tool_names("都搞定了").is_empty());
    }

    #[test]
    fn claims_execution_passes_code_explanation_without_tool_call() {
        // 零工具调用轮里最常见的正当内容：讲代码 / 讲步骤 / 讲将来时。这批是
        // 扩词表后最可能被误伤的一类，逐条钉住——其中「运行抛出异常时…」是
        // 扩词表当场被测试打回来才砍掉的（见 registry 词表末尾的不收清单）。
        assert!(!claims_execution(
            "这段代码在 `import nosuchmodule_zzz` 处会抛出 ModuleNotFoundError，\
             因为环境里没装这个模块。"
        ));
        assert!(!claims_execution(
            "可以分两步做：第一步安装依赖，第二步运行脚本即可。"
        ));
        assert!(!claims_execution(
            "如果你在这里抛出异常，外层会捕获并继续。"
        ));
        assert!(!claims_execution("运行抛出异常时该脚本会非零退出。"));
        assert!(!claims_execution("任务执行完成后会发通知给你。"));
        assert!(!claims_execution("等运行完成后再刷新页面。"));
        assert!(!claims_execution("每步骤执行完成后记录一条日志。"));
        assert!(!claims_execution("计划分三步：需求分析、开发、测试。"));
        assert!(!claims_execution("按这个计划完成后续两项工作。"));
        assert!(!claims_execution(
            "这段代码会打印 done 吗？不会，它只在导入失败时退出。"
        ));
    }

    #[test]
    fn cap_tool_output_under_cap_unchanged() {
        assert_eq!(cap_tool_output("hello", 30_000), "hello");
        // 恰好等于 cap 也不截
        assert_eq!(cap_tool_output(&"x".repeat(100), 100), "x".repeat(100));
    }

    #[test]
    fn cap_tool_output_keeps_head_and_tail() {
        let text = format!(
            "{}{}{}",
            "H".repeat(20_000),
            "M".repeat(20_000),
            "T".repeat(10_000)
        );
        let out = cap_tool_output(&text, 30_000);
        assert!(out.contains("已按 maxToolOutputBytes=30000 截断，原文 50000 字节"));
        // 头部保留 cap - 6K = 24K（原文头 20K 全 H + 中段 4K M）；尾部保留 6K
        assert!(out.starts_with(&format!("{}{}", "H".repeat(20_000), "M".repeat(4_000))));
        assert!(out.ends_with(&"T".repeat(6_000)));
        // 中段被替身说明取代（头部只含 4K 个 M，连续 5K M 必来自被替身的中段）
        assert!(!out.contains(&"M".repeat(5_000)));
    }

    #[test]
    fn cap_tool_output_small_cap_degrades_tail() {
        // cap < 5×6K：尾部保留退化为 cap/5，头部仍占大头
        let text = "A".repeat(1_000);
        let out = cap_tool_output(&text, 100);
        assert!(
            out.starts_with(&"A".repeat(80)),
            "头应保留 80：{}",
            &out[..90]
        );
        assert!(out.ends_with(&"A".repeat(20)));
        assert!(out.contains("已按 maxToolOutputBytes=100 截断"));
    }

    #[test]
    fn cap_tool_output_counts_bytes_not_chars() {
        // 中文 3 字节/字：50 个汉字 = 150 字节，cap 30 字节只够 8 个字
        let text = "中".repeat(50);
        let out = cap_tool_output(&text, 30);
        assert!(out.contains("原文 150 字节"));
        assert!(out.starts_with(&"中".repeat(8)), "头部应保留 8 个汉字");
        assert!(out.ends_with(&"中".repeat(2)), "尾部应保留 2 个汉字");
    }

    #[test]
    fn cap_tool_output_never_splits_a_multibyte_char() {
        // 切点落在汉字中间时必须回退到字符边界——String 切片切出半个 UTF-8 字符
        // 不是「乱码」，是 panic：整条工具调用直接崩。所以每个 cap 都要试一遍。
        let text = "中".repeat(50);
        for cap in 1..=150 {
            let out = cap_tool_output(&text, cap); // 不 panic 即为一半的通过条件
            let head: String = out.chars().take_while(|c| *c == '中').collect();
            assert_eq!(
                head.len() % 3,
                0,
                "cap={cap} 头部字节数不是 3 的倍数，说明切进了半个汉字"
            );
        }
    }

    #[test]
    fn effective_cap_is_per_tool_budget_when_tighter() {
        // run_python 的 per-tool 预算 8192 比全局 30_000 紧 → 取 8192
        assert_eq!(effective_output_cap("run_python", 30_000), 8_192);
    }

    #[test]
    fn effective_cap_falls_back_to_global_when_budget_is_looser() {
        // read_text_file 的 per-tool 预算 65536 比全局大 → 全局兜底，否则设置页那个
        // 旋钮在长输出工具上就成空设置
        assert_eq!(effective_output_cap("read_text_file", 30_000), 30_000);
        // 不在注册表里的名字（MCP 挂载工具）也走全局
        assert_eq!(effective_output_cap("mcp_unknown", 30_000), 30_000);
    }

    #[test]
    fn preview_keeps_head_tail_and_notice_within_cap() {
        let text: String = "A".repeat(20_000);
        let notice = "指针";
        let out = preview_with_pointer(&text, notice, 8_192);
        // 指针 6 字节 + 分隔 4 字节，剩余 8182 字节对半分 = 4091
        assert!(out.starts_with(&"A".repeat(4_091)), "头部应保留约一半预算");
        assert!(out.ends_with(&"A".repeat(4_091)), "尾部应保留约一半预算");
        assert!(out.contains("指针"), "指针必须在场");
        // 头尾对半分掉剩下的预算；真不变量是「不超过」
        assert!(out.len() <= 8_192, "节选总长 {} 超出 cap", out.len());
    }

    #[test]
    fn preview_survives_notice_longer_than_cap() {
        // 路径长到吃掉整个 cap 时不得 panic，也不得把正文整个挤没成空
        let text = "A".repeat(5_000);
        let notice = "N".repeat(20_000);
        let out = preview_with_pointer(&text, &notice, 1_000);
        assert!(out.contains(&"N".repeat(100)), "指针仍应完整在场");
    }

    #[test]
    fn preview_never_splits_a_multibyte_char() {
        // 节选路径同样按字节切：切点落在汉字中间会 panic，不是乱码
        let text = "中".repeat(5_000);
        let notice = "指针";
        for cap in 1..=3_000 {
            let out = preview_with_pointer(&text, notice, cap); // 不 panic 即通过
            let head: String = out.chars().take_while(|c| *c == '中').collect();
            assert_eq!(head.len() % 3, 0, "cap={cap} 头部切进了半个汉字");
        }
    }

    #[test]
    fn mutating_tools_cover_task_and_file_writes() {
        // 守卫白名单与工具分发保持一致的关键几个
        for t in [
            "delete_task",
            "add_subtask",
            "toggle_subtask",
            "complete_task",
            "edit_task",
            "create_task",
            "link_file_to_task",
        ] {
            assert!(MUTATING_TOOLS().contains(&t), "{t} 应算变更类工具");
        }
        // 纯查询工具不算变更
        for t in [
            "list_tasks",
            "search_tasks",
            "query_single_task",
            "web_search",
            "fetch_url",
        ] {
            assert!(!MUTATING_TOOLS().contains(&t), "{t} 不应算变更类工具");
        }
    }

    // ── mutation_done 按执行结果置位 ──

    /// 构造一个只带 status 的 ToolResult（mutation_succeeded 不看文本了）
    fn tr(status: crate::bot::registry::ToolStatus) -> crate::bot::registry::ToolResult {
        crate::bot::registry::ToolResult {
            text: String::new(),
            refs: Vec::new(),
            status,
            images: Vec::new(),
            file_changes: Vec::new(),
        }
    }

    #[test]
    fn mutation_succeeded_true_on_real_success() {
        use crate::bot::registry::ToolStatus;
        for name in ["complete_task", "link_file_to_task", "create_word"] {
            assert!(
                mutation_succeeded(name, &tr(ToolStatus::Ok)),
                "{name} 成功落库应算动过手"
            );
            // 带告警的成功也算动过手（做成了，只是有提示）
            assert!(
                mutation_succeeded(name, &tr(ToolStatus::Warn)),
                "{name} 带告警的成功仍算动过手"
            );
        }
    }

    #[test]
    fn mutation_succeeded_false_on_denied_and_error() {
        use crate::bot::registry::ToolStatus;
        // 门禁拦截 / 用户拒绝 / 预算触顶 / 白名单外 —— 工具根本没执行
        assert!(!mutation_succeeded(
            "link_file_to_task",
            &tr(ToolStatus::Denied)
        ));
        // 执行报错（落库失败、磁盘只读等）
        assert!(!mutation_succeeded("create_task", &tr(ToolStatus::Error)));
    }

    #[test]
    fn mutation_succeeded_false_for_readonly_tools() {
        use crate::bot::registry::ToolStatus;
        // 只读工具即使成功也不算变更
        assert!(!mutation_succeeded("list_tasks", &tr(ToolStatus::Ok)));
    }

    /// 变更声称守卫：跑过「能改状态但核验不了凭证」的工具后，如实汇报
    /// 不该被判成凭空汇报。
    ///
    /// 典型误伤：模型用 run_python 写完文件，如实说「已写入 /tmp/a.csv」
    /// ——「已写入」命中 write_file 的声称话术，但 run_python 不在变更类
    /// 工具里，mutation_done 是假的 → 守卫把终稿推回让模型重跑，副作用
    /// 翻倍，末尾还挂一条「这句没有对应凭证」的更正。
    #[test]
    fn claim_mutation_does_not_fire_after_unverifiable_tool_ran() {
        let text = "已写入 /tmp/a.csv";
        assert!(
            claims_mutation(text),
            "「已写入」确实是变更声称话术（对照组）"
        );
        assert!(
            claim_mutation_fires(false, false, text),
            "什么都没动过却声称写入 → 该拦"
        );
        assert!(
            !claim_mutation_fires(false, true, text),
            "run_python 写过盘 → 「没凭证」推不出「没动手」，不该拦"
        );
        // 变更类工具成功过 → 同样不该拦
        assert!(!claim_mutation_fires(true, false, text));
    }

    /// 守卫仍要拦得住真正的凭空汇报：不可核验工具没跑过时照拦。
    #[test]
    fn claim_mutation_still_fires_without_any_receipt() {
        assert!(claim_mutation_fires(false, false, "已把任务移至回收站"));
        // 没有变更声称 → 不拦
        assert!(!claim_mutation_fires(false, false, "今天天气不错"));
    }
}

#[cfg(test)]
mod parse_sse_chunk_tests {
    use super::*;

    #[test]
    fn parses_text_content_chunk() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(!parsed.is_done);
        assert_eq!(parsed.content.as_deref(), Some("hello"));
        assert!(parsed.tool_calls.is_empty());
    }

    #[test]
    fn parses_done_marker() {
        let parsed = parse_sse_chunk("data: [DONE]").unwrap();
        assert!(parsed.is_done);
        assert!(parsed.content.is_none());
        assert!(parsed.tool_calls.is_empty());
        assert!(parsed.finish_reason.is_none());
    }

    #[test]
    fn parses_tool_call_delta_with_index() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"list_tasks","arguments":"{}"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        let tc = &parsed.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        assert_eq!(tc.name_chunk.as_deref(), Some("list_tasks"));
        assert_eq!(tc.arguments_chunk.as_deref(), Some("{}"));
    }

    #[test]
    fn parses_multiple_tool_calls_with_distinct_indices() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"a"}},{"index":1,"function":{"name":"b"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].name_chunk.as_deref(), Some("a"));
        assert_eq!(parsed.tool_calls[1].name_chunk.as_deref(), Some("b"));
    }

    #[test]
    fn parses_finish_reason_stop() {
        let line = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parses_finish_reason_tool_calls() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"y","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("tool_calls"));
        assert!(!parsed.tool_calls.is_empty());
    }

    #[test]
    fn empty_content_string_treated_as_absent() {
        // SSE 中 content="" 时应等同 None（不触发内容推送）
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(
            parsed.content.is_none(),
            "空 content 应等同 None（不触发推送）"
        );
    }

    #[test]
    fn empty_name_treated_as_append_noop() {
        // name="" 时 chunk 仍为 Some("")；是否跳过 append 由 caller 决定（生产代码只追加非空 name）
        let line =
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":""}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(
            parsed.tool_calls[0].name_chunk.as_deref(),
            Some(""),
            "name 字段存在但为空 — caller 决定是否跳过 append"
        );
    }

    #[test]
    fn returns_none_for_non_data_line() {
        assert!(parse_sse_chunk("event: ping").is_none());
        assert!(parse_sse_chunk("").is_none());
        assert!(parse_sse_chunk("data: not-json{").is_none());
        assert!(parse_sse_chunk("data:").is_none());
        assert!(parse_sse_chunk("data:    ").is_none());
    }

    #[test]
    fn returns_none_for_empty_choices() {
        // 网关异常：200 OK + choices:[] 且无 usage → 跳过
        let line = r#"data: {"choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    /// OpenAI 流式 usage：末块 `{"choices":[],"usage":{...}}` 必须被取到
    /// （choices 为空数组，早于本改动会在空判定处直接 return 丢掉）。
    #[test]
    fn openai_usage_chunk_with_empty_choices_is_captured() {
        let line = r#"data: {"id":"x","choices":[],"usage":{"prompt_tokens":5000,"completion_tokens":120,"total_tokens":5120,"prompt_tokens_details":{"cached_tokens":4000}}}"#;
        let c = parse_sse_chunk(line).expect("usage 末块不得被丢弃");
        assert!(c.content.is_none() && c.tool_calls.is_empty());
        let u = c.usage.expect("usage 末块必须带出用量");
        // 关键口径：prompt_tokens 含 cached，落库的普通 input 要减掉
        assert_eq!(u.input_tokens, 1000);
        assert_eq!(u.cache_read_tokens, 4000);
        assert_eq!(u.output_tokens, 120);
        assert_eq!(u.cache_creation_tokens, 0, "OpenAI 无写入口");
    }

    /// 无 cached 细节（网关不上报或未命中）时 input 原样透传
    #[test]
    fn openai_usage_without_cache_details_passes_input_through() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":800,"completion_tokens":30}}"#;
        let u = parse_sse_chunk(line).unwrap().usage.unwrap();
        assert_eq!((u.input_tokens, u.cache_read_tokens), (800, 0));
        assert_eq!(u.output_tokens, 30);
    }

    /// 脏网关给出 cached > prompt（口径反了/重复计入）时不得下溢
    #[test]
    fn openai_usage_saturates_when_cached_exceeds_prompt() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":9999}}}"#;
        let u = parse_sse_chunk(line).unwrap().usage.unwrap();
        assert_eq!(u.input_tokens, 0, "减法必须 saturating，不能绕成天文数字");
        assert_eq!(u.cache_read_tokens, 9999);
    }

    /// 普通内容块不带 usage → None（调用方据此只认真正带用量的块）
    #[test]
    fn openai_content_chunk_has_no_usage() {
        let line = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        assert!(parse_sse_chunk(line).unwrap().usage.is_none());
    }

    /// OpenAI 网关**每块都带 usage** 时，token 数不得按块数翻倍。
    ///
    /// usage 是本请求的**总量**（`stream_options.include_usage` 约定），逐块
    /// 累加会把同一份总量重复计入——落库 token、缓存命中率、前端水位条全部
    /// 虚高。合并口径：按字段取最后一次非零值。
    #[test]
    fn stream_usage_is_not_multiplied_when_every_chunk_carries_it() {
        let chunk =
            r#"data: {"choices":[],"usage":{"prompt_tokens":5000,"completion_tokens":120}}"#;
        let mut acc = TokenUsage::default();
        for _ in 0..5 {
            let u = parse_sse_chunk(chunk).unwrap().usage.unwrap();
            merge_stream_usage(&mut acc, u);
        }
        assert_eq!(acc.input_tokens, 5000, "不得累加成 25000");
        assert_eq!(acc.output_tokens, 120, "不得累加成 600");
    }

    /// Anthropic 的字段分散在两个事件里（input 在 message_start、output 在
    /// message_delta），按字段取最后一次非零值才两样都在——整块替换会把
    /// Anthropic 的 input 抹成 0。
    #[test]
    fn anthropic_usage_keeps_input_from_start_and_output_from_delta() {
        let start = crate::bot_anthropic::parse_anthropic_event(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":50,"cache_read_input_tokens":4000,"cache_creation_input_tokens":1500}}}"#,
        )
        .unwrap()
        .usage
        .unwrap();
        let delta = crate::bot_anthropic::parse_anthropic_event(
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        )
        .unwrap()
        .usage
        .unwrap();
        let mut acc = TokenUsage::default();
        merge_stream_usage(&mut acc, start);
        merge_stream_usage(&mut acc, delta);
        assert_eq!(
            acc.input_tokens, 50,
            "message_start 的 input 不能被后到的 delta 抹掉"
        );
        assert_eq!(acc.output_tokens, 7);
        assert_eq!(acc.cache_read_tokens, 4000);
        assert_eq!(acc.cache_creation_tokens, 1500);
    }

    /// Anthropic 的 message_delta.output_tokens 是**累计**输出量：再来一个更大的
    /// delta 时应覆盖，不是相加（相加会把 5+7 算成 12）。
    #[test]
    fn anthropic_cumulative_output_replaces_rather_than_adds() {
        let mut acc = TokenUsage::default();
        for o in [5u64, 7] {
            let u = crate::bot_anthropic::parse_anthropic_event(&format!(
                r#"data: {{"type":"message_delta","usage":{{"output_tokens":{o}}}}}"#
            ))
            .unwrap()
            .usage
            .unwrap();
            merge_stream_usage(&mut acc, u);
        }
        assert_eq!(acc.output_tokens, 7, "累计值应覆盖，不得相加");
    }

    /// 非对象 usage（脏网关）不得当成有效用量
    #[test]
    fn openai_non_object_usage_is_ignored() {
        let line = r#"data: {"choices":[],"usage":"n/a"}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    /// 流内错误信封的优先级高于 usage（错误必须冒泡，不能被当末块静默吞掉）
    #[test]
    fn openai_error_envelope_beats_usage_chunk() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":5},"error":{"message":"rate limited"}}"#;
        let c = parse_sse_chunk(line).expect("错误信封不该被当成纯 usage 末块");
        assert_eq!(c.error.as_deref(), Some("rate limited"));
    }

    /// OpenAI 形状的 usage 末块不得被算成「协议正常成分之外」的坏块
    #[test]
    fn openai_usage_chunk_is_not_counted_as_bad_sse() {
        use super::unparsable_sse_payload;
        use crate::bot::ApiProvider::Openai;
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":5}}"#;
        assert!(unparsable_sse_payload(line, Openai).is_none());
    }

    #[test]
    fn unparsable_payload_flags_only_bad_data_lines() {
        use super::unparsable_sse_payload;
        use crate::bot::ApiProvider::{Anthropic, Openai};
        // 坏块：data 行 + 非空 + 解析不出来 → 计入降级（两协议同一判）
        for p in [Openai, Anthropic] {
            assert_eq!(
                unparsable_sse_payload("data: not-json{", p),
                Some("not-json{")
            );
            assert_eq!(unparsable_sse_payload("data: {半截", p), Some("{半截"));
        }
        // 协议正常成分：一律不算坏块（否则正常流每次都记降级）
        for p in [Openai, Anthropic] {
            assert_eq!(unparsable_sse_payload("event: ping", p), None);
            assert_eq!(unparsable_sse_payload("", p), None);
            assert_eq!(unparsable_sse_payload(": keep-alive", p), None);
            assert_eq!(unparsable_sse_payload("data:", p), None);
            assert_eq!(unparsable_sse_payload("data:    ", p), None);
            assert_eq!(unparsable_sse_payload("data: [DONE]", p), None);
            assert_eq!(unparsable_sse_payload("data:    [DONE]", p), None);
            // 标准形状（含空心跳）
            assert_eq!(unparsable_sse_payload(r#"data: {"choices":[]}"#, p), None);
            assert_eq!(
                unparsable_sse_payload(r#"data: {"choices":[{"delta":{}}]}"#, p),
                None
            );
        }
        // 合法 Anthropic 事件没有 choices 键，不得被 OpenAI 那条形状臂误伤
        assert_eq!(
            unparsable_sse_payload(r#"data: {"type":"message_start","message":{}}"#, Anthropic),
            None
        );
        // OpenAI 形状缺失：错误信封 / 外来载荷照样丢内容，必须计入
        assert_eq!(
            unparsable_sse_payload(r#"data: {"code":401,"msg":"invalid api key"}"#, Openai),
            Some(r#"{"code":401,"msg":"invalid api key"}"#)
        );
        assert_eq!(
            unparsable_sse_payload(r#"data: {"id":"x","object":"unknown"}"#, Openai),
            Some(r#"{"id":"x","object":"unknown"}"#)
        );
    }

    #[test]
    fn stream_error_payload_surfaced() {
        // 200 流内错误载荷不得静默忽略（否则用户拿到空白回复）；
        // 必须显式冒出（主循环据此报错 + 审计）
        let line = r#"data: {"id":"x","error":{"message":"auth_failed","type":"auth_error"}}"#;
        let parsed = parse_sse_chunk(line).expect("error 载荷必须冒出，不得静默丢弃");
        assert_eq!(parsed.error.as_deref(), Some("auth_failed"));

        // error 为裸字符串的兼容形态
        let line2 = r#"data: {"error":"rate limited"}"#;
        let parsed2 = parse_sse_chunk(line2).unwrap();
        assert_eq!(parsed2.error.as_deref(), Some("rate limited"));
    }

    #[test]
    fn returns_none_for_empty_choices_without_error() {
        // 空 choices 且无 error 字段 → 仍静默跳过（与原语义一致）
        let line = r#"data: {"id":"x","choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    #[test]
    fn parses_reasoning_content() {
        // DeepSeek-reasoner 类模型的独立推理字段
        let line =
            r#"data: {"choices":[{"delta":{"reasoning_content":"先想一下","content":null}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.reasoning.as_deref(), Some("先想一下"));
        assert!(parsed.content.is_none());
    }

    #[test]
    fn parses_chinese_content_utf8() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好世界"}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.content.as_deref(), Some("你好世界"));
    }

    #[test]
    fn parses_arguments_split_across_chunks() {
        // 模拟 arguments 跨多个 SSE chunk（流式追加）
        let chunk1 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"ti"}}]}}]}"#;
        let chunk2 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"tle\":\"x\""}}]}}]}"#;
        let p1 = parse_sse_chunk(chunk1).unwrap();
        let p2 = parse_sse_chunk(chunk2).unwrap();
        assert_eq!(p1.tool_calls[0].arguments_chunk.as_deref(), Some(r#"{"ti"#));
        assert_eq!(
            p2.tool_calls[0].arguments_chunk.as_deref(),
            Some(r#"tle":"x""#)
        );
        // 生产代码会按顺序 push 拼成完整 JSON
    }

    #[test]
    fn done_marker_data_is_correctly_parsed() {
        // data:    [DONE]（中间多空格）也应正确识别
        let parsed = parse_sse_chunk("data:    [DONE]").unwrap();
        assert!(parsed.is_done);
    }

    #[test]
    fn defaults_index_to_zero_when_missing() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"a"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls[0].index, 0, "缺 index 时默认 0");
    }
}

#[cfg(test)]
mod stream_accumulate_tests {
    use super::*;

    /// 多字节 UTF-8 字符跨 chunk 切断时不得产生 U+FFFD 替换字符
    #[test]
    fn drain_sse_lines_no_replacement_char_across_chunks() {
        let text = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n";
        let bytes = text.as_bytes();
        // 「你」是 3 字节字符，切在它的第 2 个字节后
        let pos = bytes
            .windows(3)
            .position(|w| w == "你".as_bytes())
            .expect("应含「你」");
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&bytes[..pos + 1]);
        assert!(
            drain_sse_lines(&mut buf).is_empty(),
            "行未完成时不应产出任何行"
        );
        buf.extend_from_slice(&bytes[pos + 1..]);
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines.len(), 1);
        assert!(
            !lines[0].contains('\u{FFFD}'),
            "多字节字符跨 chunk 不得产生替换字符：{:?}",
            lines[0]
        );
        assert!(lines[0].contains("你好"));
        assert!(buf.is_empty());
    }

    #[test]
    fn drain_sse_lines_keeps_incomplete_tail() {
        let mut buf = b"data: a\ndata: partial".to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines, vec!["data: a\n".to_string()]);
        assert_eq!(buf, b"data: partial".to_vec(), "残余不完整行留在 buf");
    }

    #[test]
    fn drain_sse_lines_empty_buf() {
        // 空 buf → 空 vec，buf 仍空
        let mut buf: Vec<u8> = Vec::new();
        let lines = drain_sse_lines(&mut buf);
        assert!(lines.is_empty(), "空 buf 应产出空 vec");
        assert!(buf.is_empty(), "buf 应保持空");
    }

    #[test]
    fn drain_sse_lines_no_newline() {
        // 无 \n → 空 vec，buf 不变（不完整行不走 drain）
        let mut buf = b"data: incomplete".to_vec();
        let original = buf.clone();
        let lines = drain_sse_lines(&mut buf);
        assert!(lines.is_empty(), "无 \\n 不应产出行");
        assert_eq!(buf, original, "buf 应保持原样");
    }

    #[test]
    fn drain_sse_lines_single_line() {
        // b"a\n" → ["a\n"]，buf 清空
        let mut buf = b"a\n".to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines, vec!["a\n".to_string()]);
        assert!(buf.is_empty(), "完整行后 buf 应清空");
    }

    #[test]
    fn drain_sse_lines_multiple_lines() {
        // b"a\nb\nc\n" → ["a\n", "b\n", "c\n"]，buf 清空
        let mut buf = b"a\nb\nc\n".to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(
            lines,
            vec!["a\n".to_string(), "b\n".to_string(), "c\n".to_string()]
        );
        assert!(buf.is_empty(), "多行完整后 buf 应清空");
    }

    #[test]
    fn drain_sse_lines_multibyte_utf8() {
        // 完整多字节 UTF-8 字符串中含 \n → 正确切分且无 U+FFFD
        let text = "你好\n世界\n";
        let mut buf = text.as_bytes().to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("你好") && lines[0].ends_with('\n'));
        assert!(lines[1].contains("世界") && lines[1].ends_with('\n'));
        assert!(!lines.iter().any(|l| l.contains('\u{FFFD}')));
        assert!(buf.is_empty());
    }

    #[test]
    fn accumulate_merges_shards_by_index() {
        let mut calls: Vec<(String, String, String)> = Vec::new();
        // 两个并行 tool_call 交错到达；id 只在首帧；arguments 分片
        let deltas = [
            ToolCallDelta {
                index: 0,
                id: Some("call_a".into()),
                name_chunk: Some("list".into()),
                arguments_chunk: Some("{\"ti".into()),
            },
            ToolCallDelta {
                index: 1,
                id: Some("call_b".into()),
                name_chunk: Some("web".into()),
                arguments_chunk: Some("{\"q".into()),
            },
            ToolCallDelta {
                index: 0,
                id: None,
                name_chunk: Some("_tasks".into()),
                arguments_chunk: Some("tle\":\"x\"}".into()),
            },
            ToolCallDelta {
                index: 1,
                id: None,
                name_chunk: Some("_search".into()),
                arguments_chunk: Some("\":\"y\"}".into()),
            },
        ];
        for d in &deltas {
            assert!(accumulate_tool_call_delta(&mut calls, d));
        }
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            (
                "call_a".into(),
                "list_tasks".into(),
                "{\"title\":\"x\"}".into()
            )
        );
        assert_eq!(
            calls[1],
            ("call_b".into(), "web_search".into(), "{\"q\":\"y\"}".into())
        );
    }

    #[test]
    fn accumulate_late_id_fills_empty_slot() {
        let mut calls: Vec<(String, String, String)> = Vec::new();
        let d1 = ToolCallDelta {
            index: 0,
            id: None,
            name_chunk: Some("list_tasks".into()),
            arguments_chunk: None,
        };
        assert!(accumulate_tool_call_delta(&mut calls, &d1));
        let d2 = ToolCallDelta {
            index: 0,
            id: Some("call_late".into()),
            name_chunk: None,
            arguments_chunk: None,
        };
        assert!(accumulate_tool_call_delta(&mut calls, &d2));
        assert_eq!(calls[0].0, "call_late", "迟到的 id 应补上");
    }

    /// 畸形/恶意 index 不得撑爆内存
    #[test]
    fn accumulate_rejects_insane_index() {
        let mut calls: Vec<(String, String, String)> = Vec::new();
        let d = ToolCallDelta {
            index: MAX_TOOL_CALL_INDEX + 1,
            id: Some("evil".into()),
            name_chunk: Some("x".into()),
            arguments_chunk: None,
        };
        assert!(!accumulate_tool_call_delta(&mut calls, &d));
        assert!(calls.is_empty(), "超限 index 不得伸长列表");
        // 边界：恰好上限放行
        let ok = ToolCallDelta {
            index: MAX_TOOL_CALL_INDEX,
            id: None,
            name_chunk: Some("x".into()),
            arguments_chunk: None,
        };
        assert!(accumulate_tool_call_delta(&mut calls, &ok));
        assert_eq!(calls.len(), MAX_TOOL_CALL_INDEX + 1);
    }

    /// 重试状态白名单
    #[test]
    fn retryable_status_429_and_5xx_only() {
        assert!(is_retryable_llm_status(429));
        assert!(is_retryable_llm_status(500));
        assert!(is_retryable_llm_status(503));
        assert!(!is_retryable_llm_status(400));
        assert!(!is_retryable_llm_status(401));
        assert!(!is_retryable_llm_status(404));
        assert!(!is_retryable_llm_status(200));
    }
}

#[cfg(test)]
mod context_overflow_tests {
    use super::*;

    #[test]
    fn overflow_detected_across_vendor_payload_shapes() {
        // OpenAI 形态
        assert!(is_context_overflow(
            400,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens","code":"context_length_exceeded"}}"#
        ));
        // Anthropic 形态
        assert!(is_context_overflow(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#
        ));
        // 国产网关中文形态 + 大小写不敏感
        assert!(is_context_overflow(400, "错误：上下文超长，请缩短对话"));
        assert!(is_context_overflow(
            413,
            "payload too large: context window"
        ));
        // 短 body 也认（关键词在）
        assert!(is_context_overflow(400, "too many tokens"));
    }

    #[test]
    fn other_400s_are_not_treated_as_overflow() {
        // 400 的其它形态不得触发降级：砍历史治不好鉴权/参数错，白砍一轮还多花一次请求
        assert!(!is_context_overflow(
            400,
            r#"{"error":{"message":"Invalid API key","code":"invalid_api_key"}}"#
        ));
        assert!(!is_context_overflow(
            400,
            r#"{"error":{"message":"tools[0].function.parameters: invalid schema"}}"#
        ));
        // 关键词出现在非超长语境也不误伤：只看 400/413 + 特征词，不做语义推理
        assert!(!is_context_overflow(401, "invalid api key"));
        assert!(!is_context_overflow(500, "internal error"));
        assert!(!is_context_overflow(200, r#"{"choices":[]}"#));
    }

    fn m(role: &str, n: usize) -> serde_json::Value {
        serde_json::json!({"role": role, "content": "x".repeat(n)})
    }

    fn assistant_with_tool_call(id: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": [{"id": id, "type": "function",
                            "function": {"name": "noop", "arguments": "{}"}}]
        })
    }

    fn tool_msg(id: &str, n: usize) -> serde_json::Value {
        serde_json::json!({"role": "tool", "tool_call_id": id, "content": "y".repeat(n)})
    }

    /// 孤儿 tool：前面最近一条非 system 消息不是「带 tool_calls 的 assistant」。
    /// 这种结构发出去对端直接 400，降级等于白降。
    fn orphan_tool_positions(msgs: &[serde_json::Value]) -> Vec<usize> {
        msgs.iter()
            .enumerate()
            .filter(|(_, m)| m["role"] == "tool")
            .filter(|(i, _)| {
                msgs[..*i]
                    .iter()
                    .rev()
                    .find(|p| p["role"] != "system")
                    .is_none_or(|p| p["role"] != "assistant" || p.get("tool_calls").is_none())
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// 复现：降级砍历史会让「msgs 下标」记账的 tool 消息表整体失效。
    /// 记账值是入栈时的旧下标，砍完 msgs 变短却不修正——下一轮按它压缩就
    /// 越界 panic，或错指到别的消息。
    #[test]
    fn shed_keeps_recorded_tool_indices_addressable() {
        let mut msgs = vec![
            m("system", 10),
            m("user", 400),
            m("assistant", 400),
            m("user", 400),
            m("assistant", 400),
            assistant_with_tool_call("c1"),
            tool_msg("c1", 300),
            m("user", 10),
        ];
        // 与循环内一致：tool 消息入栈时记下 (轮次, msgs 下标)
        let mut recorded: Vec<(usize, usize)> = msgs
            .iter()
            .enumerate()
            .filter(|(_, x)| x["role"] == "tool")
            .map(|(i, _)| (0usize, i))
            .collect();
        assert_eq!(recorded.len(), 1, "本用例要有 tool 消息可记账");

        let (cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0, "超预算必须真砍：{dropped}");
        reindex_after_shed(&mut recorded, cut, dropped);

        for (_born, idx) in recorded {
            assert!(
                idx < msgs.len(),
                "降级后记账下标 {idx} 越界：msgs 只剩 {} 条（下标须随切口左移）",
                msgs.len()
            );
            assert_eq!(
                msgs[idx]["role"], "tool",
                "记账下标 {idx} 已错指到非 tool 消息：{:?}",
                msgs[idx]
            );
        }
    }

    /// 落在切口内的记账必须作废：那条 tool 消息已被砍掉，下标不再指向任何消息，
    /// 留着就是一颗定时炸弹（后续轮按它压缩必越界）。
    #[test]
    fn shed_drops_bookkeeping_for_removed_tool_messages() {
        let mut msgs = vec![
            m("system", 10),
            m("user", 400),
            assistant_with_tool_call("c1"),
            tool_msg("c1", 300),
            tool_msg("c2", 300),
            m("user", 10),
        ];
        let mut recorded: Vec<(usize, usize)> = msgs
            .iter()
            .enumerate()
            .filter(|(_, x)| x["role"] == "tool")
            .map(|(i, _)| (0usize, i))
            .collect();
        assert_eq!(recorded.len(), 2);

        let (cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0);
        reindex_after_shed(&mut recorded, cut, dropped);

        for (_born, idx) in &recorded {
            assert!(
                *idx < msgs.len(),
                "残留记账 {idx} 指向已被砍掉的位置（msgs 只剩 {} 条）：{recorded:?}",
                msgs.len()
            );
            assert_eq!(msgs[*idx]["role"], "tool");
        }
    }

    /// 切口不能停在 tool 分组中间：砍掉带 tool_calls 的 assistant 却留下它的
    /// tool 消息，对端 400（tool 必须紧跟带 tool_calls 的 assistant）。
    #[test]
    fn shed_never_leaves_orphan_tool_message() {
        let mut msgs = vec![
            m("system", 10),
            m("user", 400),
            assistant_with_tool_call("c1"),
            tool_msg("c1", 300),
            tool_msg("c2", 300),
            m("user", 10),
        ];
        let (_cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0, "超预算必须真砍：{dropped}");
        let orphans = orphan_tool_positions(&msgs);
        assert!(
            orphans.is_empty(),
            "降级后留下孤儿 tool 消息（下标 {orphans:?}），发出去必 400：{msgs:?}"
        );
    }

    #[test]
    fn shed_keeps_system_block_and_current_turn() {
        let mut msgs = vec![
            m("system", 10),     // 系统提示词本体：不可砍
            m("system", 5),      // 摘要/记忆块：不可砍
            m("user", 400),      // 旧历史：可砍
            m("assistant", 400), // 旧历史：可砍
            m("user", 10),       // 本轮输入：不可砍
        ];
        let (_cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0, "超预算必须真砍：{dropped}");
        let roles: Vec<&str> = msgs.iter().filter_map(|x| x["role"].as_str()).collect();
        assert_eq!(
            roles.iter().filter(|r| **r == "system").count(),
            3,
            "两个原 system + 一条裁剪说明都要在：{roles:?}"
        );
        assert_eq!(
            msgs.last().unwrap()["content"],
            serde_json::json!("x".repeat(10)),
            "最后一条（本轮用户输入）必须保留"
        );
        assert!(
            !msgs
                .iter()
                .any(|x| x["content"] == serde_json::json!("x".repeat(400))),
            "旧历史应被砍掉：{roles:?}"
        );
    }

    #[test]
    fn shed_is_noop_when_nothing_left_to_drop() {
        // 只有 system + 本轮输入：没得砍，调用方据此走报错出口而非空转重发
        let mut msgs = vec![m("system", 10), m("user", 10)];
        assert_eq!(shed_oldest_history(&mut msgs, 0.5).1, 0);
        assert_eq!(msgs.len(), 2, "空转不得插入裁剪说明");

        let mut empty: Vec<serde_json::Value> = Vec::new();
        assert_eq!(shed_oldest_history(&mut empty, 0.5).1, 0);
    }

    #[test]
    fn shed_never_touches_messages_after_first_non_system() {
        // 本轮 tool 消息（role=tool）必须原样保留：砍掉会破坏
        // tool_call/tool_result 配对，对端直接 400（那就白降级了）
        let mut msgs = vec![
            m("system", 10),
            m("user", 500),
            serde_json::json!({"role": "tool", "tool_call_id": "c1", "content": "y".repeat(300)}),
        ];
        let (_cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert_eq!(dropped, 1, "只砍 user 那条：{dropped}");
        assert!(
            msgs.iter().any(|x| x.get("tool_call_id").is_some()),
            "tool 消息不得被砍"
        );
    }
}

#[cfg(test)]
mod tools_schema_tests {
    use crate::bot::registry::tools_json as TOOLS;

    /// TOOLS 是编译期字符串、运行期解析：语法坏会 panic 杀死聊天（历史 bug）。
    /// 此测试守住：加/改工具后必须合法且字段完整。
    #[test]
    fn tools_schema_parses() {
        let v: serde_json::Value = serde_json::from_str(TOOLS()).expect("TOOLS 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        assert!(!arr.is_empty(), "TOOLS 不能为空");
        for t in arr {
            assert_eq!(
                t["type"].as_str(),
                Some("function"),
                "每项 type 必须是 function"
            );
            let name = t["function"]["name"]
                .as_str()
                .expect("每项必须有 function.name");
            assert!(!name.is_empty(), "工具名不能为空");
            assert!(
                t["function"]["description"].as_str().is_some(),
                "{name} 缺 description"
            );
        }
        // 关键工具必须存在（与 execute_tool match 对齐，改名会在此暴露）
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        for required in [
            "query_tasks",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "run_python",
            "web_search",
            "fetch_url",
        ] {
            assert!(names.contains(&required), "缺少工具 {required}");
        }
    }
}
// 测试：max_rounds 解析 / fallback + 单轮 Function 调用熔断

#[cfg(test)]
mod rounds_fuse_tests {
    use super::*;

    /// 轮数熔断的出口必须带回熔断前的正文，且标记可被子任务侧识别。
    ///
    /// 回归背景：子任务撞满 20 轮时熔断走裸 Err，连同最后一轮 streamed 正文一起
    /// 丢弃，子任务侧收不到收尾 JSON → result_json 写 NULL → 父 agent 回灌拿不到
    /// summary，只能从零重跑（设计 §6「部分结果不丢弃」当时没兑现）。
    #[test]
    fn rounds_fuse_message_keeps_partial_text() {
        let msg = rounds_fuse_message("已查到 det.onnx / rec.onnx 在 pp-ocr-v6/ 下", "", 20);
        assert!(
            msg.contains("已查到 det.onnx"),
            "熔断必须保住熔断前正文：{msg}"
        );
        assert!(
            msg.contains(ROUNDS_FUSE_MARKER),
            "熔断正文须带子任务侧识别的标记：{msg}"
        );
        assert!(msg.contains("20 轮上限"), "熔断消息应带上限值：{msg}");
        // 子任务侧按 rounds 标记判 max_turns，所以单轮 Function 熔断的正文绝不能
        // 带这个标记——否则「工具调用超限」会被误报成「轮数超限」。
        assert!(
            !fuse_message("x", "", 100).contains(ROUNDS_FUSE_MARKER),
            "单轮 Function 熔断不得带轮数标记"
        );
    }

    /// 轮数软警告阈值与单轮软警告同口径：小 cap 不塌成 0，且留得出收尾轮次。
    #[test]
    fn rounds_warn_threshold_leaves_room_to_finish() {
        let at =
            |max_rounds: usize| (max_rounds * 7 / 10).clamp(1, max_rounds.saturating_sub(1).max(1));
        // 子 agent 典型预算 20 → 第 14 轮起提醒，剩 6 轮够写收尾 JSON
        assert_eq!(at(20), 14);
        assert!(at(20) < 20, "阈值必须早于熔断，否则预警没有意义");
        // 小 cap 整除塌到 0 时钳回 1（否则提示变成「已进行 0 轮」）；max_rounds=3
        // 的 7/10 是 2，留 1 轮收尾——阈值贴着熔断正是小预算的固有代价
        assert_eq!(at(3), 2);
        assert_eq!(at(1), 1);
        // 主聊天默认 50 同样留 15 轮
        assert_eq!(at(50), 35);
    }

    /// 「轮」的口径必须由循环自报，不能数 `llm.request` 事件。
    ///
    /// 回归背景：一轮里可能网络重试、可能撞 max_tokens 自动续写、可能补终稿自检，
    /// 数事件必然把上报轮数算大——上报值与真正约束循环的 max_rounds 对不上，
    /// trace 里的 turns 就成了「请求数」而非「轮数」。
    #[test]
    fn turn_count_is_rounds_not_llm_requests() {
        // 口径契约：LoopTrace 同时带轮数与请求数两个字段，二者不可复用同一来源。
        let t = LoopTrace::default();
        assert_eq!(t.turn_count, 0);
        assert_eq!(t.request_count, 0);
        // 审计闭包只给请求数加一（数据源是 llm.request 事件）；
        // 轮数由 core 的 on_round 单独写。测试锚定这条分工：请求数归请求数、
        // 轮数归轮数，谁把两者混起来就是回归。
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bot_model_loop.rs"
        ))
        .expect("读自身源码");
        assert!(
            src.contains("if event == \"llm.request\"")
                && src.contains("request_for_audit.fetch_add"),
            "请求数仍应数 llm.request 事件"
        );
        assert!(
            src.contains("turn_count: rounds_used.load"),
            "turn_count 必须来自 rounds_used（循环自报轮数），不得回到数请求事件"
        );
        assert!(
            src.contains("request_count: request_count.load"),
            "请求数应单独落在 request_count 字段"
        );
    }

    #[test]
    fn skill_frontmatter_max_rounds_overrides_default() {
        // Skill frontmatter 声明 max_rounds: 25 → 生效，run_model_loop 收到 25
        let meta = crate::bot_skills::parse_meta(
            "---\nname: minimax-docx\nmax_rounds: 25\n---\n# body\n",
            "minimax-docx",
        );
        assert_eq!(meta.max_rounds, Some(25));
        assert_eq!(resolve_max_rounds(meta.max_rounds, 50), 25);
    }

    #[test]
    fn skill_without_max_rounds_falls_back_to_default_50() {
        // 现有 Skill 未声明 max_rounds → None → fallback（：fallback = 配置解析值，测试传默认 50）
        let meta = crate::bot_skills::parse_meta("---\nname: x\ndescription: d\n---\nbody\n", "x");
        assert_eq!(meta.max_rounds, None);
        assert_eq!(
            resolve_max_rounds(meta.max_rounds, DEFAULT_MAX_ROUNDS),
            DEFAULT_MAX_ROUNDS
        );
        assert_eq!(DEFAULT_MAX_ROUNDS, 50);
    }

    #[test]
    fn fuse_trips_at_cap_boundary() {
        // MAX_FUNCTION_CALLS_PER_REQUEST = 50 实际生效：模拟主循环计数，
        // 构造 101 个 tool_calls → 第 101 次触发熔断默认上限 50→100）
        assert_eq!(MAX_FUNCTION_CALLS_PER_REQUEST, 100);
        let mut calls = 0usize;
        let mut fused_msg: Option<String> = None;
        for _ in 0..101 {
            calls += 1;
            if should_fuse(calls, MAX_FUNCTION_CALLS_PER_REQUEST) {
                fused_msg = Some(fuse_message("前文", "", MAX_FUNCTION_CALLS_PER_REQUEST));
                break;
            }
        }
        let msg = fused_msg.expect("101 次 Function 调用内必须触发熔断");
        assert!(msg.contains("⏹ 已熔断"), "熔断消息应含「⏹ 已熔断」：{msg}");
        assert!(msg.contains("100 次上限"), "熔断消息应带上限值：{msg}");
        // 配置覆盖：maxFunctionCalls=30 → 第 31 次熔断（派生链第二级）
        // 边界：默认 100 时第 100 次放行、第 101 次熔断
        assert!(!should_fuse(100, MAX_FUNCTION_CALLS_PER_REQUEST));
        assert!(should_fuse(101, MAX_FUNCTION_CALLS_PER_REQUEST));
        let fused = (1..=31).any(|n| should_fuse(n, 30));
        assert!(fused, "cap=30 时第 31 次必须熔断");
        // 边界：第 50 次放行，第 51 次熔断
        assert!(!should_fuse(50, 50));
        assert!(should_fuse(51, 50));
    }

    /// 熔断上限只在一处钳：上游传进来的越界值必须由派生链末端收口。
    ///
    /// 钳位曾同时存在于薄壳（组装 deps 时）与 core 派生链末端，两份边界改一处
    /// 漏一处不会有任何编译或测试信号。抽成 fuse_cap_for 后，薄壳原样透传
    /// （9999 进来，末端才钳），本测试钉住这条契约。
    #[test]
    fn fuse_cap_clamps_at_the_only_consumer() {
        let cap = fuse_cap_upper_bound();
        assert_eq!(fuse_cap_for(None, None), MAX_FUNCTION_CALLS_PER_REQUEST);
        assert_eq!(
            fuse_cap_for(None, Some(9_999)),
            cap,
            "上游未钳时，末端必须收口到上界"
        );
        assert_eq!(fuse_cap_for(None, Some(0)), 1, "0 次调用无意义，钳到 1");
        assert_eq!(fuse_cap_for(None, Some(30)), 30, "区间内原样放行");
        // 子 agent 行上预算优先于全局覆盖，且同样受末端钳制约束
        assert_eq!(fuse_cap_for(Some(7), Some(9_999)), 7);
        assert_eq!(fuse_cap_for(Some(u32::MAX), None), cap);
        // 漂移守卫：同一条边界在本文件里只允许有一份。薄壳那份钳位被删掉后，
        // 生产代码里不应再出现 `clamp(1, 500)` 字面量——它是当年人手抄的副本，
        // 被绕过也不报错（单测直驱 core 时根本不经薄壳）。
        let src = include_str!("bot_model_loop.rs");
        let production = src
            .split_once("#[cfg(test)]")
            .expect("bot_model_loop.rs 应有测试模块")
            .0;
        assert!(
            !production.contains("clamp(1, 500)"),
            "熔断上界不得在本文件内再存一份字面量副本（边界单源在 params）"
        );
        assert!(
            production.matches("fuse_cap_upper_bound()").count() >= 2,
            "熔断钳位必须走上界投影（定义 + 至少一处调用），不得另写数字"
        );
    }
}

/// L6 接线守卫：`post_turn` 钩子层必须真被模型循环调到，且排在
/// 「答案定稿」之后、「回灌与返回」之前。
///
/// 与 dispatch 里 L5 那条同款理由：注册表的行为测试验不到调用点是否存在。
/// 变异实测把模型循环里那段调用删掉（最终回复再也不过钩子）时，全仓测试
/// 照样全绿——所以这里钉「语句存在 + 两点相对位置」。
///
/// 先切掉 `#[cfg(test)]` 尾段再扫是这条守卫成立的前提：`include_str!` 把本
/// 测试模块自己也读进来了，不切的话，生产调用被删掉之后 `find` 会命中测试
/// 模块里那个字符串字面量，测试依旧绿——那才是真的假保证。
#[cfg(test)]
mod post_turn_layer_tests {
    #[test]
    fn post_turn_layer_is_wired_after_answer_is_final() {
        let src = include_str!("bot_model_loop.rs");
        let production = src
            .split_once("#[cfg(test)]")
            .expect("bot_model_loop.rs 应有测试模块")
            .0;
        assert_eq!(
            production.matches("run_post_turn(").count(),
            1,
            "生产代码里 post_turn 调用应恰好 1 处（多处说明接线分叉了）"
        );
        let hook = production
            .find("run_post_turn(")
            .expect("模型循环必须调用 post_turn 钩子层");
        let answer = production
            .find("let mut answer = if length_prefix.is_empty()")
            .expect("找不到答案定稿点");
        let ret = production
            .find("return Ok((answer, collected_refs));")
            .expect("找不到主出口");
        assert!(
            answer < hook,
            "钩子必须在答案定稿之后（改写的要是用户真会看到的那一份）"
        );
        assert!(hook < ret, "钩子必须在返回之前：否则改写不会到达用户");
        // 返回值必须被消费：run_post_turn 跑了却把结果丢掉，等于钩子改写
        // answer 之后的失败既没人看见也没人记，只钉调用存在是不够的
        let tail = &production[hook..];
        let consumed = tail
            .find("run.failures")
            .expect("run_post_turn 的返回值必须被读（至少 failures 列表）");
        let ret_after_hook = tail
            .find("return Ok((answer, collected_refs));")
            .expect("找不到主出口");
        assert!(
            consumed < ret_after_hook,
            "钩子失败的消费必须在返回之前，否则失败静默丢失"
        );
    }
}

#[cfg(test)]
mod selfcheck_vendor_gate_tests {
    use super::*;
    use crate::bot::config::ResolvedModel;

    fn resolved(vendor: Option<&str>) -> Result<ResolvedModel, crate::error::CommandError> {
        Ok(ResolvedModel {
            base_url: "https://third-party.example/v1".into(),
            model: "m".into(),
            api_provider: "openai".into(),
            api_key: "sk-side".into(),
            vendor: vendor.map(|v| v.to_string()),
            inference: crate::bot::config::EffectiveInference {
                max_tokens: 32,
                temperature: None,
                top_p: None,
                system_prompt: None,
            },
        })
    }

    /// 侧调条目指向没过连接测试的厂商 → 侧调不发（回落主路径内的自检）
    ///
    /// 回归背景：主请求被 vendor 门拦下，侧调请求却照发——真实带 key 的请求
    /// 打往未验证的 base_url。
    #[test]
    fn selfcheck_must_not_use_a_vendor_that_failed_the_connection_test() {
        let d = decide_selfcheck(resolved(Some("未验证厂商")), &[]);
        assert!(
            matches!(d, SelfcheckDecision::VendorNotVerified { .. }),
            "未验证厂商的侧调条目必须被拦下（侧调请求是真实带 key 的请求）"
        );
        let d = decide_selfcheck(resolved(Some("未验证厂商")), &["别的厂商".into()]);
        assert!(
            matches!(d, SelfcheckDecision::VendorNotVerified { .. }),
            "已验证名单里没有它 → 仍须拦下"
        );
    }

    /// 已验证厂商 → 放行；无 vendor 的老条目 → 放行（与主路径同口径）
    #[test]
    fn selfcheck_allows_verified_vendors_and_legacy_entries() {
        assert!(
            matches!(
                decide_selfcheck(resolved(Some("已验证厂商")), &["已验证厂商".into()]),
                SelfcheckDecision::Use(_)
            ),
            "已验证厂商应当放行"
        );
        assert!(
            matches!(
                decide_selfcheck(resolved(None), &[]),
                SelfcheckDecision::Use(_)
            ),
            "无 vendor 的老条目没有验证概念，应当放行"
        );
    }

    /// 条目解析失败 → 回落（不发侧调），错误可行动
    #[test]
    fn selfcheck_falls_back_when_the_entry_cannot_be_resolved() {
        let d = decide_selfcheck(
            Err(crate::error::CommandError::ApiKeyMissing),
            &["已验证厂商".into()],
        );
        assert!(
            matches!(d, SelfcheckDecision::ResolveFailed(_)),
            "解析失败必须回落（不发侧调）"
        );
    }
}
