//! SSE 流消费层：逐 chunk 读取、空闲超时与流内错误处理。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{
    accumulate_tool_call_delta, drain_sse_lines, feed_think, merge_stream_usage, parse_sse_chunk,
    unparsable_sse_payload, LlmHttp, ModelLoopDeps, TokenUsage, MAX_TOOL_CALL_INDEX,
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
    pub(crate) stream_error: Option<String>,
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
    /// 收尾原因（length / content_filter 等）
    pub(crate) last_finish_reason: Option<String>,
    /// 用户在流中叫停（正文已出的部分照常返回给用户）
    pub(crate) stopped: bool,
}

/// 流消费（读侧）：逐 chunk 收 SSE 行 → 解析成正文 / 工具调用 / 思考块。
///
/// 覆盖：SSE 行解析（两协议）· think 拆分 · 工具调用累积 · 流完整性标记
/// （[DONE] / finish_reason）· 坏块计数 · token 用量累计 · 流内错误载荷。
pub(crate) async fn read_sse_stream(
    http: &LlmHttp,
    resp: reqwest::Response,
    stop: &StopGuard,
    deps: &ModelLoopDeps<'_>,
    expecting_selfcheck: bool,
    expecting_final_selfcheck: bool,
) -> Result<SseStream, CommandError> {
    let emit = deps.emit;
    let audit = deps.audit;
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
            let item = match tokio::time::timeout(STREAM_CHUNK_IDLE_TIMEOUT, stream.next()).await {
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
    Ok(SseStream {
        final_text,
        tool_calls,
        thinking_blocks,
        last_finish_reason,
        stream_end_seen,
        think_buf,
        think_mode,
        usage_acc,
        bad_sse_chunks,
        bad_sse_sample,
        stream_error,
        stopped,
    })
}

/// 流消费（收尾侧）：坏块可见化 / token 用量审计 / 流内错误判定 /
/// 思考缓冲冲刷 / 流完整性（截断）防线 / 幽灵条目清理 / 空 id 合成。
pub(crate) fn stream_round(
    sse: SseStream,
    deps: &ModelLoopDeps<'_>,
    expecting_selfcheck: bool,
    expecting_final_selfcheck: bool,
) -> Result<RoundStream, CommandError> {
    let emit = deps.emit;
    let audit = deps.audit;
    let skill_finish = deps.skill_finish;
    let SseStream {
        mut final_text,
        mut tool_calls,
        thinking_blocks,
        last_finish_reason,
        stream_end_seen,
        mut think_buf,
        think_mode,
        usage_acc,
        bad_sse_chunks,
        mut bad_sse_sample,
        stream_error,
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
        return Ok(RoundStream {
            final_text,
            tool_calls,
            thinking_blocks,
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
    Ok(RoundStream {
        final_text,
        tool_calls,
        thinking_blocks,
        last_finish_reason,
        stopped: false,
    })
}
