//! SSE 流消费层：逐 chunk 读取、空闲超时与流内错误处理。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{
    accumulate_tool_call_delta, drain_sse_lines, feed_think, merge_stream_usage, parse_sse_chunk,
    unparsable_sse_payload, LlmHttp, ModelLoopDeps, StreamError, TokenUsage, MAX_TOOL_CALL_INDEX,
};
use crate::bot_slash::StopGuard;
use crate::error::CommandError;
use futures_util::StreamExt;

/// 流式逐 chunk idle 超时：对端持续吐 delta，两 chunk 间隔超过此值即判定死连接
///（旧实现的 300s **总**超时会掐掉合法长生成——长正文/长工具参数总时长可超 5 分钟）
pub const STREAM_CHUNK_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// SSE 流读完后的原始产出（收尾处理前）。
pub(crate) struct SseStream {
    /// 本轮正文
    pub(crate) final_text: String,
    /// 本轮工具调用（id, name, arguments）
    pub(crate) tool_calls: Vec<(String, String, String)>,
    /// 本轮 Anthropic thinking 块（回传服务端用）
    pub(crate) thinking_blocks: Vec<serde_json::Value>,
    /// 本轮 reasoning_content 原文（M2/M3 回灌用；默认不回灌，只进前端展示出口）
    pub(crate) reasoning_text: String,
    /// 收尾原因（length / content_filter 等）
    pub(crate) last_finish_reason: Option<String>,
    /// 是否见到正常收尾标记（[DONE] / finish_reason）
    pub(crate) stream_end_seen: bool,
    /// 思考缓冲（未闭合标签碎片，冲刷用）
    pub(crate) think_buf: String,
    /// 当前是否在思考块内
    pub(crate) think_mode: bool,
    /// 流内 token 用量累计
    pub(crate) usage_acc: TokenUsage,
    /// 解析不掉的 data 载荷计数
    pub(crate) bad_sse_chunks: u32,
    /// 解析不掉的 data 载荷首条样本
    pub(crate) bad_sse_sample: Option<String>,
    /// 流内错误载荷
    pub(crate) stream_error: Option<StreamError>,
    /// 本轮是否已向前端吐过内容（正文 / 思考段 / 工具折叠行）。
    /// 收尾侧据此判断「这条失败能不能重发本轮」——重发＝用户看两遍。
    pub(crate) emitted_any: bool,
    /// 用户在流中叫停
    pub(crate) stopped: bool,
}

/// 一轮流消费后的产出。
pub(crate) struct RoundStream {
    /// 本轮正文
    pub(crate) final_text: String,
    /// 本轮工具调用（id, name, arguments）
    pub(crate) tool_calls: Vec<(String, String, String)>,
    /// 本轮 Anthropic thinking 块（回传服务端用）
    pub(crate) thinking_blocks: Vec<serde_json::Value>,
    /// 本轮 reasoning_content 原文（M2/M3 回灌用）
    pub(crate) reasoning_text: String,
    /// 收尾原因（length / content_filter 等）
    pub(crate) last_finish_reason: Option<String>,
    /// 用户在流中叫停（正文已出的部分照常返回给用户）
    pub(crate) stopped: bool,
}

/// 流读取失败：错误本体 + 失败前是否已向前端吐过内容 + 能不能重发本轮 +
/// 放弃重试时收尾技能用的原因。
///
/// 「有没有吐过内容」决定这条失败能不能重发本轮：正文、思考段、工具折叠行
/// 一旦显示过，重发一遍就是重复内容；什么都没显示时重发才是干净的。
///
/// 「能不能重发」决定值不值得重发：上游瞬时故障（5xx / 过载 / 限流）值得，
/// 确定性拒绝（鉴权 / 参数 / 配额）不值得——重发多少次结果都一样。
///
/// 技能收尾统一由主循环在这一处做——本模块不调 `skill_finish`：流内错误
/// 可能要重发本轮，提前收尾会把仍在跑的技能判成失败。
pub(crate) struct StreamFailure {
    pub(crate) err: CommandError,
    pub(crate) emitted_any: bool,
    /// 该失败是否值得重发本轮
    pub(crate) retryable: bool,
    /// 放弃重试时收尾技能用的原因（写进技能收尾文案）
    pub(crate) reason: &'static str,
}

/// 构造一条流失败（读侧与收尾侧同一形状）。
///
/// 五个失败出口都走这里——形状散着写，迟早有一处漏填 `retryable`/`reason`，
/// 而这两个字段直接决定「要不要重发本轮」与「技能收尾写什么」。
///
/// `err` 收 `CommandError` 而非字符串：字符串会被 blanket `From<String>`
/// 静默降级成 `Internal`（不可恢复），变体选择必须每处显式。
fn stream_failure(
    err: CommandError,
    emitted_any: bool,
    retryable: bool,
    reason: &'static str,
) -> StreamFailure {
    StreamFailure {
        err,
        emitted_any,
        retryable,
        reason,
    }
}

/// 流内错误载荷 → CommandError。
///
/// 分类与 HTTP 路径对齐：一律报 `LlmApiError`（recoverable=true，前端给
/// 「重试」入口 + 按状态码的人话指引），**不再降级成 Internal**——
/// Internal 恒为 recoverable=false，前端只会让用户「反馈日志」，而上游明说
/// 「Please retry your request」。
///
/// 状态码从上游 type / message 反推；推不出时用 0（= 上游未给状态码），
/// 按不可重试处理——不猜：误判成可重试会白烧一次请求，误判成不可重试
/// 只是少试一次。
fn stream_error_to_cmd_error(err: &StreamError) -> CommandError {
    let status = stream_error_status(err);
    let detail = if status != 0 {
        crate::error::llm_error_detail(status, &err.message, None)
    } else {
        err.message.clone()
    };
    CommandError::LlmApiError { status, detail }
}

/// 流内错误是否值得重发本轮：上游瞬时故障（5xx / 过载 / 限流）值得，
/// 确定性拒绝（鉴权 / 参数 / 配额）不值得——重发多少次结果都一样。
fn stream_error_is_retryable(err: &StreamError) -> bool {
    let status = stream_error_status(err);
    status != 0 && super::is_retryable_llm_status(status)
}

/// 流内错误 → 归一化 HTTP 状态码（0 = 上游未给可判定的状态码）。
///
/// 依据上游标的 type 与 message 里内嵌的状态码（MiniMax 兼容层把「(500)」
/// 塞在 message 尾部）。两者都取不到时返回 0——不猜。
///
/// type 名按各协议的**标准名**列，不收自造名：网关自造的标签（如
/// `stream_error`）语义各家不同，猜错会把确定性拒绝当瞬时故障重发。
/// - Anthropic 协议标准名：`overloaded_error` / `api_error` / `rate_limit_error`…
/// - OpenAI 兼容网关标准名：`authentication_error` / `server_error` /
///   `insufficient_quota`…
fn stream_error_status(err: &StreamError) -> u16 {
    if let Some(kind) = err.kind.as_deref() {
        let k = kind.trim().to_ascii_lowercase();
        let mapped = match k.as_str() {
            "overloaded_error" => 529,
            "api_error" | "server_error" | "internal_error" | "service_unavailable" => 500,
            "rate_limit_error" => 429,
            "authentication_error" | "auth_error" => 401,
            "permission_error" => 403,
            "invalid_request_error" | "invalid_request" => 400,
            "not_found_error" => 404,
            "insufficient_balance_error" | "insufficient_quota" => 402,
            _ => 0,
        };
        if mapped != 0 {
            return mapped;
        }
        // 兼容网关把数字状态码塞进 type
        if let Ok(code) = k.parse::<u16>() {
            return code;
        }
    }
    // message 尾部内嵌状态码：「… Please retry your request. (500)」
    if let Some(idx) = err.message.rfind('(') {
        let tail = &err.message[idx + 1..];
        let num: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(code) = num.parse::<u16>() {
            if (100..=599).contains(&code) {
                return code;
            }
        }
    }
    0
}

/// 读流过程中的累积状态（正文 / 工具调用 / 思考块 / 用量 / 坏块计数…）。
///
/// 解析一行要读写十几样东西，全靠闭包捕获会把所在函数顶过 clippy 的行数闸
/// ——收成结构体传，「解析一行」与「读流循环」各管各的。
struct StreamState {
    /// 协议（决定行解析走哪条分支）
    provider: crate::bot::ApiProvider,
    /// 自检回复整段隐藏：流式不 emit，final_text 解析后统一 emit 视觉提示
    expecting_selfcheck: bool,
    /// 终稿自检同样整段隐藏
    expecting_final_selfcheck: bool,
    /// 本轮正文
    final_text: String,
    /// 本轮工具调用（id, name, arguments）
    tool_calls: Vec<(String, String, String)>,
    /// reasoning_content 原文（M2/M3 回灌用；默认不回灌，只进前端展示出口）
    reasoning_text: String,
    /// 未闭合的 <think> 标签碎片（冲刷用）
    think_buf: String,
    /// 当前是否在思考块内
    think_mode: bool,
    /// 收尾原因（length / content_filter 等）
    last_finish_reason: Option<String>,
    /// 是否见到正常收尾标记（[DONE] / finish_reason）
    stream_end_seen: bool,
    /// 流内 token 用量累计（Anthropic：message_start 的 input /
    /// message_delta 的 output；OpenAI：末块 usage），回合结束写 llm.usage 审计。
    ///
    /// 合并口径见 merge_stream_usage：按字段取最后一次非零值，**不逐块累加**
    /// ——两协议的 usage 都是累计总量，逐块加会把同一份总量重复计入。
    /// 缓存命中/写入两档单独记（仅 Anthropic 协议会变；OpenAI 保持 0），
    /// 前端水位条按「读/写」单列展示，不折进输入输出，也不算命中率。
    usage_acc: TokenUsage,
    /// 解析不掉的 data 载荷计数（回合结束发一条审计，不逐块刷屏）
    bad_sse_chunks: u32,
    /// 解析不掉的 data 载荷首条样本
    bad_sse_sample: Option<String>,
    /// 工具槽位溢出只记一次
    tc_index_overflow_logged: bool,
    /// Anthropic 模式 content block index → 工具槽位重映射：
    /// Anthropic 的块序号连文本块一起数，直接当工具序号累积会留下
    /// 幽灵空条目，严格 API 会 400（详见 bot_anthropic::ToolSlotMapper 注释）
    tool_slot_mapper: crate::bot_anthropic::ToolSlotMapper,
    /// Anthropic 的 thinking 块收集器（仅 Anthropic 协议会收到增量）：
    /// 块要原样回传服务端，故随本轮响应收集、随 assistant 消息入历史。
    thinking_collector: crate::bot_anthropic::ThinkingCollector,
}

impl StreamState {
    /// 新建一份空状态。
    fn new(
        provider: crate::bot::ApiProvider,
        expecting_selfcheck: bool,
        expecting_final_selfcheck: bool,
    ) -> Self {
        Self {
            provider,
            expecting_selfcheck,
            expecting_final_selfcheck,
            final_text: String::new(),
            tool_calls: Vec::new(),
            reasoning_text: String::new(),
            think_buf: String::new(),
            think_mode: false,
            last_finish_reason: None,
            stream_end_seen: false,
            usage_acc: TokenUsage::default(),
            bad_sse_chunks: 0,
            bad_sse_sample: None,
            tc_index_overflow_logged: false,
            tool_slot_mapper: crate::bot_anthropic::ToolSlotMapper::new(),
            thinking_collector: crate::bot_anthropic::ThinkingCollector::default(),
        }
    }

    /// 解析一行 SSE → 消费成正文 / 工具调用 / 思考块。
    ///
    /// 返回 Some = 流内错误载荷，调用方收尾报错。
    fn handle_line(
        &mut self,
        line: &str,
        emit: &dyn Fn(&str, serde_json::Value),
        audit: &dyn Fn(crate::audit::AuditLevel, &'static str, Vec<(&'static str, String)>),
    ) -> Option<StreamError> {
        // 行解析按协议分支，解析出的
        // ParsedChunk 走同一消费逻辑（think 拆分 / 工具累积 / 流完整性 /
        // finish_reason 处理全部两协议共享）
        let parsed = match self.provider {
            crate::bot::ApiProvider::Openai => parse_sse_chunk(line),
            crate::bot::ApiProvider::Anthropic => {
                match crate::bot_anthropic::parse_anthropic_event(line) {
                    Some(mut ev) => {
                        // content block index → 稠密工具槽位（防幽灵条目）
                        for d in &mut ev.tool_calls {
                            self.tool_slot_mapper.remap(d);
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
            if let Some(payload) = unparsable_sse_payload(line, self.provider) {
                self.bad_sse_chunks += 1;
                if self.bad_sse_sample.is_none() {
                    self.bad_sse_sample = Some(payload.chars().take(120).collect::<String>());
                }
            }
            return None;
        };
        // token 记账两协议共用一处：Anthropic 分布在 message_start /
        // message_delta 两个事件，OpenAI 集中在末块 usage。
        // 合并口径统一走 merge_stream_usage（按字段取最后一次非零值）。
        if let Some(u) = parsed.usage.take() {
            merge_stream_usage(&mut self.usage_acc, u);
        }
        // Anthropic 的 thinking 块收进收集器：它要原样回传（丢了下一轮
        // 带 tool_result 的请求会被 400），不能只走前端展示出口。
        if let Some(d) = parsed.thinking.take() {
            self.thinking_collector.push(&d);
        }
        if parsed.is_done {
            self.stream_end_seen = true;
            return None;
        }
        if let Some(reason) = parsed.finish_reason {
            self.stream_end_seen = true;
            self.last_finish_reason = Some(reason);
        }
        // reasoning_content 与 <think> 同出口（不进 final_text、不进历史）
        // 另存一份原文：M2/M3 官方要求把 reasoning_content 随 tool_calls
        // 回灌历史（「otherwise subsequent conversation will lose context」），
        // 回灌与否由配置开关决定（默认关 = 只走前端展示出口）。
        if let Some(r) = parsed.reasoning {
            self.reasoning_text.push_str(&r);
            emit("bot-think-delta", serde_json::json!({ "text": r }));
        }
        if let Some(t) = parsed.content {
            let (normal, think) = feed_think(&mut self.think_mode, &mut self.think_buf, &t);
            if !think.is_empty() {
                // <think> 段并入 reasoning 原文：MiniMax 这类模型的 OpenAI
                // 兼容端点把思考段放正文 <think> 标签里（没有 reasoning_content
                // 字段），剥出后不留底，回灌开关就无从盖它——带 tools 的后续
                // 请求里这一轮永远缺思考段。
                self.reasoning_text.push_str(&think);
                emit("bot-think-delta", serde_json::json!({ "text": think }));
            }
            if !normal.is_empty() {
                self.final_text.push_str(&normal);
                // 自检回复（Y/N）整段隐藏：流式不 emit，final_text 解析后
                // 统一 emit ✅/⚠️ 视觉提示，避免用户先看到 Y 又被覆盖
                // 终稿自检响应（Y/N）同样整段隐藏：用户已在主出口前看到
                // final_text 全文，不该再叠一层 Y/N 字样
                if !self.expecting_selfcheck && !self.expecting_final_selfcheck {
                    emit("bot-chat-delta", serde_json::json!({ "text": normal }));
                }
            }
        }
        for tc_delta in parsed.tool_calls {
            let first_id = tc_delta.id.is_some()
                && self
                    .tool_calls
                    .get(tc_delta.index)
                    .map(|t| t.0.is_empty())
                    .unwrap_or(true);
            // 累积走 accumulate_tool_call_delta（与 tests/llm_integration 同一实现）
            if !accumulate_tool_call_delta(&mut self.tool_calls, &tc_delta) {
                if !self.tc_index_overflow_logged {
                    self.tc_index_overflow_logged = true;
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
            let t = &self.tool_calls[tc_delta.index];
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
    }
}

/// 流消费（读侧）：逐 chunk 收 SSE 行 → 解析成正文 / 工具调用 / 思考块。
///
/// 覆盖：SSE 行解析（两协议）· think 拆分 · 工具调用累积 · 流完整性标记
/// （[DONE] / finish_reason）· 坏块计数 · token 用量累计 · 流内错误载荷。
///
/// 失败一律返回 `StreamFailure`（带「失败前是否已吐过内容」）：调用方据此决定
/// 重发本轮还是收尾上抛。本函数**不**收尾技能——重试判定在调用方，收尾与重试
/// 只能有一个地方做。
pub(crate) async fn read_sse_stream(
    http: &LlmHttp,
    resp: reqwest::Response,
    stop: &StopGuard,
    deps: &ModelLoopDeps<'_>,
    expecting_selfcheck: bool,
    expecting_final_selfcheck: bool,
) -> Result<SseStream, StreamFailure> {
    let emit_raw = deps.emit;
    let audit = deps.audit;
    // 「本轮是否已向前端吐过内容」：正文、思考段、工具折叠行都算——
    // 这三样显示过就不能重发本轮（重发＝重复内容）。
    // 用原子量而非 Cell：读侧 future 要跨线程（Send），Cell 不满足 Sync
    let emitted_any = std::sync::atomic::AtomicBool::new(false);
    // 出口闭包就叫 emit：事件出口靠这个名字被一致性对拍认出来（前端 listen
    // 的每一件都得在 Rust 侧找得到 emit），改名会让门禁以为没人发这些事件
    let emit = |ev: &str, payload: serde_json::Value| {
        // 用户已叫停：不再往前端推流式增量。
        //
        // 为什么必须在这里闸：读流循环只在**每轮开头**查一次停止标志，用户
        // 按停止时往往正卡在 `stream.next()` 上——在途的那一块数据到了照样
        // 被解析、被推出。于是「停止之后还在冒字」，而且这段迟到的增量到达
        // 时回合往往已经收尾：前端据此新开一个没人收的「正在思考」气泡，
        // 活体指示永远转（真机 2026-10-26：点了停止仍显示「正在思考」）。
        //
        // 正文照旧累积进 final_text——闸只挡「推流」，不挡「记账」，最终
        // 答复里那半句字不会丢。
        if stop.stopped() {
            return;
        }
        emitted_any.store(true, std::sync::atomic::Ordering::Relaxed);
        emit_raw(ev, payload);
    };
    let mut stream = resp.bytes_stream();
    // 字节缓冲按行切——逐 chunk from_utf8_lossy 时
    // chunk 边界落在多字节字符中间会产生 U+FFFD（正文乱码尚可，
    // 嵌在工具 arguments 里则是「合法 JSON 但内容损坏」会被真实执行）
    let mut byte_buf: Vec<u8> = Vec::new();
    // SSE 单行长度上限（字节）：单条合法 data 行上界 = 工具参数累积上限
    // （MAX_TOOL_CALL_ARGUMENTS_BYTES，1MB）加 JSON 包装，2MB 是它的两倍——
    // 正常生成永远碰不到，畸形流则在这里被截停。
    const MAX_SSE_LINE_BYTES: usize = 2 * 1024 * 1024;
    let mut line_cap_reported = false;
    let mut data_cap_reported = false;
    let mut state = StreamState::new(
        http.provider,
        expecting_selfcheck,
        expecting_final_selfcheck,
    );
    let mut stream_error: Option<StreamError> = None;

    let mut stopped = false;
    // data 攒批：SSE 规范允许一条事件拆多条 `data:` 行（按 \n 拼接成一条
    // 载荷），逐行独立解析会把它当坏块跳过、丢内容。攒到事件边界（非 data
    // 行）或流尾再拼成一条交给解析。跨 chunk 保留——事件边界行可能与它的
    // data 行落在不同 chunk。
    let mut data_buf = String::new();
    {
        // 逐 chunk idle 超时——next() 单次等待超过 STREAM_CHUNK_IDLE_TIMEOUT
        // 即判定死连接（总时长不再设限，合法长生成不受 300s 总超时误杀）
        loop {
            if stop.stopped() {
                stopped = true;
                break;
            }
            // next() 包 idle 超时：Ok(Some(inner)) = 拿到 chunk/流错误，
            // Ok(None) = 对端干净 EOF，Err = Elapsed（idle 超时）
            let item = match tokio::time::timeout(STREAM_CHUNK_IDLE_TIMEOUT, stream.next()).await {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    audit(
                        crate::audit::AuditLevel::Error,
                        "llm.stream_idle_timeout",
                        vec![("idle_secs", STREAM_CHUNK_IDLE_TIMEOUT.as_secs().to_string())],
                    );
                    return Err(stream_failure(
                        CommandError::Internal(format!(
                            "流式读取超时：连续 {}s 未收到数据，判定连接死亡",
                            STREAM_CHUNK_IDLE_TIMEOUT.as_secs()
                        )),
                        emitted_any.load(std::sync::atomic::Ordering::Relaxed),
                        // 死连接重发有意义（换一条新连接往往就成了）
                        true,
                        "流式响应中断",
                    ));
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
                    return Err(stream_failure(
                        CommandError::Internal(format!("流式读取失败：{e}")),
                        emitted_any.load(std::sync::atomic::Ordering::Relaxed),
                        // 连接中断重发有意义（上游/网关瞬时断链）
                        true,
                        "流式响应中断",
                    ));
                }
            };
            byte_buf.extend_from_slice(&chunk);
            for line in drain_sse_lines(&mut byte_buf) {
                let line = line.trim();
                // 注释行（: 开头）按 SSE 规范忽略——它不终结正在攒的事件，
                // 也不产生输出；keep-alive 常见形态
                if line.starts_with(':') {
                    continue;
                }
                if let Some(rest) = line.strip_prefix("data:") {
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(rest.trim_start());
                    // 攒批缓冲与单行同口径上限：切出的完整 data 行不占
                    // byte_buf 的 2MB 配额，故障/恶意流持续发短 data 行即可
                    // 无限攒——超限截停并留痕一次（丢批保会话，同单行口径）
                    if data_buf.len() > MAX_SSE_LINE_BYTES {
                        if !data_cap_reported {
                            data_cap_reported = true;
                            audit(
                                crate::audit::AuditLevel::Warn,
                                "llm.sse_data_batch_truncated",
                                vec![("bytes", data_buf.len().to_string())],
                            );
                        }
                        data_buf.clear();
                    }
                    continue;
                }
                // 非 data 行 = 事件边界：把攒到的 data 行拼成一条再解析
                if !data_buf.is_empty() {
                    let joined = format!("data:{data_buf}");
                    data_buf.clear();
                    if let Some(e) = state.handle_line(joined.trim(), &emit, audit) {
                        stream_error = Some(e);
                        break;
                    }
                }
                if let Some(e) = state.handle_line(line, &emit, audit) {
                    stream_error = Some(e);
                    break;
                }
            }
            // SSE 单行长度上限：无换行的畸形/恶意流不能把 byte_buf 无限撑大。
            // drain 之后 buf 只剩未完结行，截掉它不影响已切出的完整行；残段
            // 交给 handle_line 按不可解析行跳过（与 MCP stdio 读侧同口径：
            // 丢行保会话），每条流只留一次审计。
            if byte_buf.len() > MAX_SSE_LINE_BYTES {
                if !line_cap_reported {
                    line_cap_reported = true;
                    audit(
                        crate::audit::AuditLevel::Warn,
                        "llm.sse_line_truncated",
                        vec![("bytes", byte_buf.len().to_string())],
                    );
                }
                byte_buf.clear();
            }
            if stream_error.is_some() {
                break;
            }
        }
        // 流尾残余行冲刷：非标准服务端最后一个 data 事件可能不带尾换行
        if !stopped && stream_error.is_none() && !byte_buf.is_empty() {
            byte_buf.push(b'\n');
            for line in drain_sse_lines(&mut byte_buf) {
                let line = line.trim();
                // 注释行忽略（同主循环：不终结攒批、不产生输出）
                if line.starts_with(':') {
                    continue;
                }
                if let Some(rest) = line.strip_prefix("data:") {
                    if !data_buf.is_empty() {
                        data_buf.push('\n');
                    }
                    data_buf.push_str(rest.trim_start());
                    continue;
                }
                if !data_buf.is_empty() {
                    let joined = format!("data:{data_buf}");
                    data_buf.clear();
                    if let Some(e) = state.handle_line(joined.trim(), &emit, audit) {
                        stream_error = Some(e);
                        break;
                    }
                }
                if let Some(e) = state.handle_line(line, &emit, audit) {
                    stream_error = Some(e);
                    break;
                }
            }
        }
        // 流尾 data 攒批的尾巴：最后一条事件可能没有空行收边
        if !stopped && stream_error.is_none() && !data_buf.is_empty() {
            let joined = format!("data:{data_buf}");
            data_buf.clear();
            if let Some(e) = state.handle_line(joined.trim(), &emit, audit) {
                stream_error = Some(e);
            }
        }
    }
    // 本轮响应里的 Anthropic thinking 块（空 = 非 Anthropic 协议，
    // 或模型没开思考）。随本轮的 assistant 消息入历史，回传时原样发回。
    let thinking_blocks = state.thinking_collector.blocks();
    let emitted_any = emitted_any.load(std::sync::atomic::Ordering::Relaxed);
    Ok(SseStream {
        final_text: state.final_text,
        tool_calls: state.tool_calls,
        thinking_blocks,
        reasoning_text: state.reasoning_text,
        last_finish_reason: state.last_finish_reason,
        stream_end_seen: state.stream_end_seen,
        think_buf: state.think_buf,
        think_mode: state.think_mode,
        usage_acc: state.usage_acc,
        bad_sse_chunks: state.bad_sse_chunks,
        bad_sse_sample: state.bad_sse_sample,
        stream_error,
        emitted_any,
        stopped,
    })
}

/// 流消费（收尾侧）：坏块可见化 / token 用量审计 / 流内错误判定 /
/// 思考缓冲冲刷 / 流完整性（截断）防线 / 幽灵条目清理 / 空 id 合成。
///
/// 失败一律返回 `StreamFailure`——与读侧（`read_sse_stream`）同一形状：
/// 错误本体 + 「失败前是否已向前端吐过内容」。主循环据此决定重发本轮还是收场。
pub(crate) fn stream_round(
    sse: SseStream,
    deps: &ModelLoopDeps<'_>,
    expecting_selfcheck: bool,
    expecting_final_selfcheck: bool,
) -> Result<RoundStream, StreamFailure> {
    let emit = deps.emit;
    let audit = deps.audit;
    let SseStream {
        mut final_text,
        mut tool_calls,
        thinking_blocks,
        reasoning_text,
        last_finish_reason,
        stream_end_seen,
        mut think_buf,
        think_mode,
        usage_acc,
        bad_sse_chunks,
        mut bad_sse_sample,
        stream_error,
        emitted_any,
        stopped,
    } = sse;

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
            kv.push(("cache_read_tokens", usage_acc.cache_read_tokens.to_string()));
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
    //
    // 上游在 200 流里报错，分两类：
    // - **瞬时故障**（overloaded_error / api_error / 5xx 语义）：上游自己都说
    //   「Please retry your request」，重发本轮往往就成了。此处**不直接报错**，
    //   交回主循环走既有的「没吐过内容 → 重发本轮」通道（core.rs 的
    //   MAX_STREAM_RETRIES），与流读取失败（idle 超时 / 连接中断）同一处置。
    // - **确定性拒绝**（鉴权 / 参数 / 配额）：重发多少次都一样，直接报错。
    //
    // 错误分类与 HTTP 路径对齐：报 `LlmApiError`（recoverable=true，前端给
    // 「重试」入口），不再一律降级成 Internal（recoverable=false，前端只让
    // 用户「反馈日志」，与上游自己说的「Please retry your request」相反）。
    if let Some(err) = stream_error {
        let e = stream_error_to_cmd_error(&err);
        audit(
            crate::audit::AuditLevel::Error,
            "llm.stream_error",
            crate::audit::error_kv(&e),
        );
        return Err(stream_failure(
            e,
            emitted_any,
            // 上游瞬时故障值得重发；确定性拒绝（鉴权/参数/配额）不值得
            stream_error_is_retryable(&err),
            "大模型流内错误",
        ));
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
        return Ok(RoundStream {
            final_text,
            tool_calls,
            thinking_blocks,
            reasoning_text,
            last_finish_reason,
            stopped: true,
        });
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
            let err = CommandError::Internal("大模型响应中断（流被截断），工具调用未执行".into());
            let mut kv = crate::audit::error_kv(&err);
            kv.extend(trunc_kv(tool_calls.len(), final_text.chars().count()));
            audit(crate::audit::AuditLevel::Warn, "llm.stream_truncated", kv);
            return Err(stream_failure(
                err,
                emitted_any,
                // 流被切 = 上游/代理断了，重发换一条连接有意义
                true,
                "流式响应中断",
            ));
        }
        if final_text.is_empty() {
            let err = CommandError::Internal("大模型响应中断：未收到完整回复".into());
            let mut kv = crate::audit::error_kv(&err);
            kv.extend(trunc_kv(tool_calls.len(), final_text.chars().count()));
            audit(crate::audit::AuditLevel::Warn, "llm.stream_truncated", kv);
            return Err(stream_failure(
                err,
                emitted_any,
                // 流被切 = 上游/代理断了，重发换一条连接有意义
                true,
                "流式响应中断",
            ));
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
    Ok(RoundStream {
        final_text,
        tool_calls,
        thinking_blocks,
        reasoning_text,
        last_finish_reason,
        stopped: false,
    })
}
