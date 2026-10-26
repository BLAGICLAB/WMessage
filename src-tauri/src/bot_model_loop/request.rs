//! 请求装配层：请求体构造、429/超限重试与轮前准备。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{
    apply_tool_window, fuse_cap_for, is_context_overflow, is_retryable_llm_status, msg_chars,
    reindex_after_shed, shed_oldest_history, strip_thinking_key, tool_result_chars,
    ErrorDisposition, ErrorResponse, LlmHttp, ModelLoopDeps, RequestOutcome, RequestPlan,
    RequestState, RequestWork, RoundCtx, TOOL_WINDOW_KEEP_RECENT, TOOL_WINDOW_TRIGGER_CHARS,
};
use crate::bot_slash::StopGuard;
use crate::error::CommandError;

/// 单轮 msgs 总字符软上限（仅 audit，不截断）。
/// 默认 200K 字符（中文 ≈ 1 token/字符 ≈ 200K token 上下文）。
/// 超出走 `loop.msgs.over_budget` 事件供事后分析；不主动压缩、不截断、不触发摘要。
pub(crate) const MSGS_BUDGET_CHARS: usize = 200_000;

/// 滚动降密度：轮龄 ≥ 此值的 tool 结果软压（头 200 + 尾 100 + sidecar 指针）。
/// N=5 对齐业界——SWE-agent 消融只留最近 5 条观察比全历史高 3 分，
/// Claude Code microcompact 亦为最近 5 条工具结果。
const AGE_SOFT_ROUNDS: usize = 5;

/// 轮龄 ≥ 此值再降硬档（30+30+指针）。保守默认，config 化挂账
/// （见 docs/AGENT-TOKEN-EFFICIENCY-PLAN.md 滚动降密度一节）。
const AGE_HARD_ROUNDS: usize = 15;

/// LLM 请求重试：429/5xx/网络错误重试一次（1.5s 退避）。
/// 只在流式产出开始前重试——响应已开始流式产出后不重试（无重放风险：
/// 重试发的是同一轮请求，已执行的工具在 msgs 里，不会因重发而重放）。
pub(crate) const MAX_LLM_ATTEMPTS: usize = 2;

/// 流读取失败后的重发上限（本轮重发，不是重发整个回合）。
///
/// 只在「还没向前端吐过内容」时用得上——重发已显示过的内容等于给用户看两遍。
/// 1 次够：死连接多半在响应头之后立刻断，重发一次仍断就是上游/网络真有问题，
/// 再重发只是白烧一次请求。
pub(crate) const MAX_STREAM_RETRIES: usize = 1;

pub(crate) const LLM_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// 上下文超长降级：保留的历史比例（砍掉一半再试）
const CONTEXT_SHED_KEEP_RATIO: f64 = 0.5;

/// 单次模型循环内的降级次数上限（防「砍一半仍超长 → 再砍 → 再超」打转）
const CONTEXT_SHED_MAX: u8 = 2;

/// 非 2xx 错误响应体的读取上限（字节）：报错文案与 stream_options 探测只
/// 需要开头一小段，无上限读会把异常/恶意服务端的超大错误体整包进内存。
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// 错误响应体截断读取：拿到上限内的开头部分即可。读到一半连接断了就保留
/// 已读部分——错误体的价值在开头（status/message 字段），残缺好过丢弃。
async fn read_body_capped(mut r: reqwest::Response, cap: usize) -> String {
    let mut out: Vec<u8> = Vec::new();
    while out.len() < cap {
        match r.chunk().await {
            Ok(Some(c)) => {
                let remain = cap - out.len();
                out.extend_from_slice(&c[..c.len().min(remain)]);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
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
pub(crate) fn build_request_body(
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

/// send() 响应头超时：只覆盖连接+发请求+收到响应头，
/// 不含流式 body——服务端收下请求却挂起不回时兜底，防永久挂起
pub const LLM_HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 非 2xx 响应的处置：按状态码与正文决定重发、叫停还是报错收场。
///
/// 降级顺序与原内联块一致：stream_options 探测降级 → Anthropic thinking
/// 签名失效补救 → 429/5xx 退避重发 → 上下文超长砍史重发 → 报错。
async fn handle_error_response(
    http: &LlmHttp,
    plan: &mut RequestPlan,
    stop: &StopGuard,
    deps: &ModelLoopDeps<'_>,
    work: &mut RequestWork<'_>,
    resp: ErrorResponse,
    attempt: usize,
) -> ErrorDisposition {
    let st = resp.status;
    let text = resp.text;
    // 网关不认 stream_options 会 400。**只在错误体点名该字段
    // 时降级**——普通 400（鉴权错/参数错/上下文超长）不是它的
    // 问题，若一律重发，真错会被重试掩盖并变成一次多余请求。
    if st == 400
        && http.provider == crate::bot::ApiProvider::Openai
        && plan.state.openai_usage_supported
        && work.body.get("stream_options").is_some()
        && text.contains("stream_options")
    {
        plan.state.openai_usage_supported = false;
        // body 是轮外构造、轮内复用的同一份（重试不发新
        // body），故必须就地摘掉字段，否则重发还是带它
        if let Some(obj) = work.body.as_object_mut() {
            obj.remove("stream_options");
        }
        (deps.audit)(
            crate::audit::AuditLevel::Warn,
            "llm.stream_options_unsupported",
            vec![("status", st.to_string())],
        );
        return ErrorDisposition::Retry;
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
        && !plan.state.anthropic_thinking_dropped
        && text.contains("thinking")
        && text.contains("signature")
    {
        let key = crate::bot_anthropic::THINKING_KEY;
        let had = work.msgs.iter().any(|m| m.get(key).is_some());
        if had {
            plan.state.anthropic_thinking_dropped = true;
            for m in work.msgs.iter_mut() {
                if let Some(obj) = m.as_object_mut() {
                    obj.remove(key);
                }
            }
            // body 已序列化：删完必须重建，否则重发还是带旧块
            work.body = match build_request_body(
                http,
                &*work.msgs,
                &plan.tools,
                plan.state.openai_usage_supported,
            ) {
                Ok(b) => b,
                Err(e) => {
                    let hint = (deps.skill_finish)(false, "消息转换失败");
                    let err: CommandError = format!("Anthropic 消息转换失败：{e}{hint}").into();
                    (deps.audit)(
                        crate::audit::AuditLevel::Error,
                        "llm.request_failed",
                        crate::audit::error_kv(&err),
                    );
                    return ErrorDisposition::Fatal(err);
                }
            };
            (deps.audit)(
                crate::audit::AuditLevel::Warn,
                "llm.anthropic_thinking_dropped",
                vec![("status", st.to_string())],
            );
            return ErrorDisposition::Retry;
        }
    }
    // 429/5xx 瞬时抖动：退避后重发同一请求
    if is_retryable_llm_status(st) && attempt < MAX_LLM_ATTEMPTS {
        (deps.audit)(
            crate::audit::AuditLevel::Warn,
            "llm.retry",
            vec![("status", st.to_string()), ("attempt", attempt.to_string())],
        );
        // 重试等待期响应 /stop——不再发出下一次请求（与主循环停止口径一致）
        if stop.stopped() {
            return ErrorDisposition::Stopped;
        }
        tokio::time::sleep(LLM_RETRY_DELAY).await;
        return ErrorDisposition::Retry;
    }
    let overflow = is_context_overflow(st, &text);
    // 上下文超长：同一份 msgs 重发必然同样 400——砍掉最旧的
    // 会话消息后重发（请求尚未开始流式产出，本轮无副作用可重放）
    if overflow && plan.state.context_sheds < CONTEXT_SHED_MAX {
        let before = work.msgs.iter().map(msg_chars).sum::<usize>();
        let (cut, dropped) = shed_oldest_history(&mut *work.msgs, CONTEXT_SHED_KEEP_RATIO);
        if dropped > 0 {
            plan.state.context_sheds += 1;
            // msgs 变短了：记账表里落在切口内的条目作废、
            // 其后左移，不同步就是下一轮按旧下标压缩时越界
            reindex_after_shed(&mut *work.tool_msg_ages, cut, dropped);
            // body 在进循环前已序列化——砍完必须重建整个 body，
            // 否则重发的是同一份旧 body，降级等于没做。
            // 重建要走协议构造：OpenAI 形状的 msgs 直接回填进
            // Anthropic 的 body 会被拒（role=system / role=tool
            // 都不认）→ 那个 400 还不再像超长，用户拿不到
            // 「请 /compact」的指引。
            work.body = match build_request_body(
                http,
                &*work.msgs,
                &plan.tools,
                plan.state.openai_usage_supported,
            ) {
                Ok(b) => b,
                Err(e) => {
                    let hint = (deps.skill_finish)(false, "消息转换失败");
                    let err: CommandError = format!("Anthropic 消息转换失败：{e}{hint}").into();
                    (deps.audit)(
                        crate::audit::AuditLevel::Error,
                        "llm.request_failed",
                        crate::audit::error_kv(&err),
                    );
                    return ErrorDisposition::Fatal(err);
                }
            };
            let after = work.msgs.iter().map(msg_chars).sum::<usize>();
            (deps.audit)(
                crate::audit::AuditLevel::Warn,
                "llm.context_overflow",
                vec![
                    ("status", st.to_string()),
                    ("dropped", dropped.to_string()),
                    ("from_chars", before.to_string()),
                    ("to_chars", after.to_string()),
                    ("shed", plan.state.context_sheds.to_string()),
                ],
            );
            if stop.stopped() {
                return ErrorDisposition::Stopped;
            }
            return ErrorDisposition::Retry;
        }
    }
    // 没得砍（或本就不是超长）：回错误。超长时给出可操作指引，
    // 别把裸 400 抛给用户——它看不出该 /compact 还是换模型；
    // 状态码定性的（402 余额不足等）同样说人话，别甩上游 JSON
    let advice = if overflow {
        Some("上下文超长：请 /compact 压缩历史或新建会话，也可在设置里调小历史预算")
    } else {
        crate::error::llm_status_advice(st)
    };
    let hint = (deps.skill_finish)(false, advice.unwrap_or("大模型 API 错误"));
    let err = CommandError::LlmApiError {
        status: st,
        detail: format!(
            "{}{hint}",
            crate::error::llm_error_detail(st, &text, advice)
        ),
    };
    let mut kv = crate::audit::error_kv(&err);
    kv.push(("status", st.to_string()));
    (deps.audit)(crate::audit::AuditLevel::Warn, "llm.response", kv);
    ErrorDisposition::Fatal(err)
}

/// 轮次准备：请求 URL / 工具 schema / 熔断上限派生 + 跨轮标志初值。
///
/// 这几项都是**循环不变量**（与轮次无关），故只在进入轮循环前算一次；
/// 每轮都要跑的压缩在 compact_before_round 里。
pub(crate) fn prepare_round(
    http: &LlmHttp,
    stop: &StopGuard,
    deps: &ModelLoopDeps<'_>,
    max_rounds: usize,
) -> RoundCtx {
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
    RoundCtx {
        fuse_cap,
        soft_warn_at,
        rounds_warn_at,
        plan: RequestPlan {
            url,
            tools,
            state: RequestState {
                // 网关是否接受 `stream_options.include_usage`（OpenAI 流式 usage 的前置）。
                // 初值 true；收到 400 时翻 false 并原样重发一次，此后整场对话不再带该字段
                // ——探测成本只付一次，且失败只降级统计口径、不影响回复本身。
                openai_usage_supported: true,
                // Anthropic 的 thinking 块是否已被丢弃过一次（签名失效时的补救，见请求失败分支）。
                // 只补救一次：重发后仍报同样的错就是别的问题，不该无限重试。
                anthropic_thinking_dropped: false,
                // 上下文超长降级次数（全循环共享）：超长时砍历史重发，至多两次
                context_sheds: 0,
            },
        },
    }
}

/// 轮前压缩：预算硬压（msgs 字符分级）+ 轮内工具结果滑动窗 + 轮龄降密度。
pub(crate) fn compact_before_round(
    msgs: &mut [serde_json::Value],
    tool_msg_ages: &[(usize, usize)],
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
    round: usize,
    tool_window_applied: &mut bool,
) {
    // 轮内工具结果滑动窗：累计超阈值时，**一次性**把「最近 K 条之外」的
    // 工具结果换成桩子（原文仍在痕迹表里，桩子给出 lookup_receipt 的入口）。
    //
    // 一次性是硬要求：改写会改掉请求数组的缓存前缀，反复改写等于把前缀打碎，
    // 省下的 token 又被缓存失效吃回去。触发之后前缀重新变成 append-only。
    //
    // 先于预算压缩与轮龄降密度判定：那个阈值量的是「本轮真正要发的工具结果
    // 体量」，口径稳定，事后能对着痕迹库复算。
    if deps.in_turn_context_edit && !*tool_window_applied {
        let chars = tool_result_chars(msgs);
        if chars > TOOL_WINDOW_TRIGGER_CHARS {
            let trimmed = apply_tool_window(msgs, TOOL_WINDOW_KEEP_RECENT);
            // flag 只在真裁剪时落位：跨阈值但候选 ≤ keep_recent 的轮什么都没改写，
            // 缓存前缀未动——此处若烧掉唯一一次机会，同 trace 里后续更大的批次
            // 将永远失去裁剪
            if trimmed > 0 {
                *tool_window_applied = true;
                (deps.audit)(
                    crate::audit::AuditLevel::Info,
                    "loop.tool_window.applied",
                    vec![
                        ("chars", chars.to_string()),
                        ("trimmed", trimmed.to_string()),
                        ("keep_recent", TOOL_WINDOW_KEEP_RECENT.to_string()),
                    ],
                );
            }
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
            match compact_fn(msgs, deps.conv_sidecar_dir, session_id_owned.as_deref()) {
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
    for (born_round, idx) in tool_msg_ages {
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
}

/// 请求段：装配 body → 发请求 → 失败重试/降级。
///
/// 覆盖的降级路径：429/5xx 退避重发、stream_options 探测失败降级、
/// Anthropic thinking 签名失效重建、上下文超长砍史重发。
///
/// 用户叫停时返回 `RequestOutcome::Stopped`——「已停止」正文由调用方组装
/// （skill_finish 只在真正收尾的那一处调一次）。
/// 请求层失败（网络 / 超时 / 连接中断）→ CommandError。
///
/// 报 `LlmRequestFailed`（recoverable=true，前端给「重试」入口）——
/// 用户换个网络环境就能重试；降级成 blanket 的 Internal 会让前端只提示
/// 「不可自动重试；请反馈日志」，与实际处境相反。
fn request_failure_error(detail: &str, hint: &str) -> CommandError {
    CommandError::LlmRequestFailed(format!("{detail}{hint}"))
}

pub(crate) async fn request_round(
    http: &LlmHttp,
    msgs: &mut Vec<serde_json::Value>,
    plan: &mut RequestPlan,
    stop: &StopGuard,
    deps: &ModelLoopDeps<'_>,
    tool_msg_ages: &mut Vec<(usize, usize)>,
) -> Result<RequestOutcome, CommandError> {
    // body 按协议分支——内部消息流保持 OpenAI
    // 形状不动，只在发请求前这一边界转换（bot_anthropic::build_anthropic_body）。
    // 转换失败（理论不可达，msgs 必含 user 消息）记审计并报错，不静默发出畸形请求。
    let body = match build_request_body(http, msgs, &plan.tools, plan.state.openai_usage_supported)
    {
        Ok(b) => b,
        Err(e) => {
            let hint = (deps.skill_finish)(false, "消息转换失败");
            let err: CommandError = format!("Anthropic 消息转换失败：{e}{hint}").into();
            (deps.audit)(
                crate::audit::AuditLevel::Error,
                "llm.request_failed",
                crate::audit::error_kv(&err),
            );
            return Err(err);
        }
    };

    // LLM 请求前记录
    (deps.audit)(
        crate::audit::AuditLevel::Info,
        "llm.request",
        vec![
            ("model", http.model.clone()),
            ("msgs_count", msgs.len().to_string()),
            ("provider", http.provider.as_str().to_string()),
            ("reasoning", http.reasoning.describe()),
        ],
    );

    // 降级路径要同时改 body / 会话消息 / 轮龄表——收成一份工作集传下去
    let mut work = RequestWork {
        body,
        msgs,
        tool_msg_ages,
    };

    // 429/5xx/发送失败重试一次——瞬时抖动不应直接作废整轮工具循环。
    // 只在流式产出开始前重试，无部分内容重复/重放问题。
    let mut attempt = 0usize;
    let resp = loop {
        attempt += 1;
        // 鉴权头按协议分支
        //（Anthropic 用 x-api-key + anthropic-version，OpenAI 用 Bearer）
        let req = http.client.post(&plan.url).json(&work.body);
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
                    // 下面的报错文案与超长判定也要用同一份。带上限——报错
                    // 文案只需要开头一小段，无上限读会把异常服务端的超大
                    // 错误体整包进内存。
                    let text = read_body_capped(r, MAX_ERROR_BODY_BYTES).await;
                    match handle_error_response(
                        http,
                        plan,
                        stop,
                        deps,
                        &mut work,
                        ErrorResponse { status: st, text },
                        attempt,
                    )
                    .await
                    {
                        ErrorDisposition::Retry => continue,
                        ErrorDisposition::Stopped => return Ok(RequestOutcome::Stopped),
                        ErrorDisposition::Fatal(err) => return Err(err),
                    }
                }
                break r;
            }
            Err(e) => {
                if attempt < MAX_LLM_ATTEMPTS {
                    (deps.audit)(
                        crate::audit::AuditLevel::Warn,
                        "llm.retry",
                        vec![("err", e.to_string()), ("attempt", attempt.to_string())],
                    );
                    if stop.stopped() {
                        return Ok(RequestOutcome::Stopped);
                    }
                    tokio::time::sleep(LLM_RETRY_DELAY).await;
                    continue;
                }
                let hint = (deps.skill_finish)(false, "大模型请求失败");
                // 请求没打通（网络 / 超时 / 连接失败）——用户可以重试，
                // 报 LlmRequestFailed（recoverable=true，前端给「重试」入口）；
                // 降级成 blanket 的 Internal 会让前端只提示「反馈日志」。
                let err = request_failure_error(&e.to_string(), &hint);
                let mut kv = crate::audit::error_kv(&err);
                kv.push(("attempt", attempt.to_string()));
                (deps.audit)(crate::audit::AuditLevel::Error, "llm.request_failed", kv);
                return Err(err);
            }
        }
    };
    // 非成功响应已在上面的请求循环内统一转 Err（429/5xx 退避重试、
    // 上下文超长降级重发、其余直接带指引报错），走到这里必是 2xx
    (deps.audit)(
        crate::audit::AuditLevel::Info,
        "llm.response",
        vec![("status", resp.status().as_u16().to_string())],
    );
    Ok(RequestOutcome::Response(resp))
}

// 测试：feed_think / parse_sse_chunk / TOOLS schema
