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

// 阶段 2：从 crate::bot::registry 单源派生的 TOOLS / MUTATING_TOOLS。
// 原 const 字符串 / 数组现为函数（OnceLock 缓存）——保持向后兼容路径。
pub use crate::bot::registry::mutating_tools as MUTATING_TOOLS;
pub use crate::bot::registry::tools_json as TOOLS;

// ───────────────────────── 防幻觉汇报守卫 ─────────────────────────
// 实锤事故：MiniMax-M3 多次不调任何工具就回复「已添加子任务」「已移至回收站」，
// 数据实际没变，用户以为操作成功。提示词约束（SYSTEM_PROMPT 规则 8）不够，
// 这里在循环出口做确定性拦截：最终文本声称完成变更、但本轮 0 次变更类工具调用 →
// 注入系统提醒并补一轮（每次对话最多补一次），让模型实际调工具或如实说明。

/// 会改动任务卡/文件系统的工具（判定「本轮是否真的动手了」）

/// 变更工具是否真的成功落库/落盘（幻觉守卫 mutation_done 的判定依据）。
/// 按执行结果判定而非调用前按名字置位：被门禁拦截（⚠️）、用户拒绝、执行失败的
/// 调用都不算「动过手」，否则之后的幻觉汇报就不再被拦，守卫被架空。
/// 失败口径走全链路统一的 `audit::tool_call_failed`。
fn mutation_succeeded(name: &str, result: &str) -> bool {
    MUTATING_TOOLS().iter().any(|t| t == &name) && !crate::audit::tool_call_failed(name, result)
}

/// 最终文本是否含「变更已完成」表述（任务卡/文件类；纯查询汇报不命中）。
/// 枚举完整话术是打地鼠（实锤漏网：「已彻底删除」不含「已删除」字面），
/// 改成模式匹配：完成态标记「已」+ 其后 8 字窗口内含变更动词（覆盖 已彻底删除/已经把…移除 等变体），
/// 另加若干无「已」的高频话术兜底。
/// 动词表刻意不含「完成」（「已完成搜索/分析」这类只读汇报会被误拦）；
/// 「保存/记住」覆盖「已保存到 AI_Gen_Files」「已记住偏好」话术；
/// 「已完成任务」走 PLAIN 整段匹配保住任务完成话术。
fn claims_mutation(text: &str) -> bool {
    const VERBS: [&str; 14] = [
        "添加", "删除", "移除", "修改", "更新", "绑定", "清空", "恢复", "勾选", "创建", "生成",
        "移至", "保存", "记住",
    ];
    const PLAIN: [&str; 4] = ["移至回收站", "标记为完成", "添加子任务", "已完成任务"];
    if PLAIN.iter().any(|p| text.contains(p)) {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '已' {
            let window: String = chars[i + 1..].iter().take(8).collect();
            if VERBS.iter().any(|v| window.contains(v)) {
                return true;
            }
        }
    }
    false
}

// ───────────────────────── TOOLS schema（编译期字符串，运行期 JSON 解析） ─────────────────────────

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

// ────────────────────────────────────────────────────────────────────
// SSE chunk 解析
//
// 动机：消除 tests/llm_integration.rs 与 run_model_loop 的解析逻辑重复。
// 提取后：测试调 wmessage_lib::bot::parse_sse_chunk，生产代码同样调之，
//        OpenAI SSE 格式演化只改这一处。
// ────────────────────────────────────────────────────────────────────

/// 一次 SSE chunk 解析结果（content / reasoning / tool_calls / finish_reason / 流内错误 / [DONE]）
#[derive(Debug, Default, Clone)]
pub struct ParsedChunk {
    /// delta.content（仅在非空字符串时 Some）
    pub content: Option<String>,
    /// delta.reasoning_content（DeepSeek-reasoner 等模型的独立推理字段；
    /// 与 <think> 标签同走 bot-think-delta 出口）
    pub reasoning: Option<String>,
    /// delta.tool_calls 增量（多 chunk 拼成一个完整 tool_call）
    pub tool_calls: Vec<ToolCallDelta>,
    /// choices[0].finish_reason（最后一 chunk 通常为 "stop" / "tool_calls"）
    pub finish_reason: Option<String>,
    /// 200 流内错误载荷：部分 OpenAI 兼容网关在 200 流内发 `{"error":{...}}`，
    /// 静默丢弃会导致用户拿到空白回复——显式冒出
    pub error: Option<String>,
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
///
/// 注意：纯函数，不产生任何 side effect（无 widget emit、无 think-block 处理、无 final_text push）。
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
        return None;
    };
    let delta = &choice0["delta"];

    let mut chunk = ParsedChunk::default();

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
    let mut lines = Vec::new();
    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=pos).collect();
        lines.push(String::from_utf8_lossy(&line).into_owned());
    }
    lines
}

/// 单条响应内 tool_calls 的 index 上限：
/// index 来自服务端，畸形/恶意 index（如 10000000）会让累积循环无脑 push 撑爆内存
pub const MAX_TOOL_CALL_INDEX: usize = 64;

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
        t.2.push_str(args);
    }
    true
}

/// 默认对话轮数（聊天 / 任务执行 / 逐步执行统一为 50）；
/// 多步 Skill 可在 SKILL.md frontmatter 自报 max_rounds 覆盖（见 resolve_max_rounds）。
pub(crate) const DEFAULT_MAX_ROUNDS: usize = 50;

/// 本轮工具循环的轮数上限：Skill 自报 max_rounds 优先，未声明 → DEFAULT_MAX_ROUNDS。
pub(crate) fn resolve_max_rounds(skill_max_rounds: Option<usize>) -> usize {
    skill_max_rounds.unwrap_or(DEFAULT_MAX_ROUNDS)
}

// Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
// max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
//
// 上限 50 与对话轮数上限拉齐：复杂多步任务 10 次不够用，而更高会掩护
// LLM 死循环 / 幻觉调工具。失控防护靠幻觉守卫（claims_mutation）+ 软警告 + /stop，
// 不靠压低上限。软警告阈值 35 保持 ~30% buffer（50-15=35）。
const MAX_FUNCTION_CALLS_PER_TURN: usize = 50;
const SOFT_WARN_AT: usize = 35;

/// LLM 请求重试：429/5xx/网络错误重试一次（1.5s 退避）。
/// 只在流式产出开始前重试——响应已开始流式产出后不重试（无重放风险：
/// 重试发的是同一轮请求，已执行的工具在 msgs 里，不会因重发而重放）。
const MAX_LLM_ATTEMPTS: usize = 2;
const LLM_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// 可重试的 HTTP 状态：429 限流 + 5xx 服务端瞬时错误；401/400 等重试无意义
fn is_retryable_llm_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// 熔断判定：第 n 次（1-based 累计）Function 调用是否超上限
fn should_fuse(calls_so_far: usize) -> bool {
    calls_so_far > MAX_FUNCTION_CALLS_PER_TURN
}

/// 熔断返回消息（与主循环文案同源，单测直接断言）
fn fuse_message(final_text: &str, hint: &str) -> String {
    format!(
        "{final_text}\n\n⏹ 已熔断：本轮 Function 调用超过 {} 次上限（安全保护），已停止后续执行{hint}",
        MAX_FUNCTION_CALLS_PER_TURN
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
    /// 多数兼容网关不认识该字段）。装配时已 resolve_max_tokens 钳制过。
    pub max_tokens: u32,
}

/// run_model_loop_core 的同步副作用出口：
/// widget 流式事件 / 结构化审计 / bot.log / Skill 收尾 4 个出口抽成注入回调，
/// 使核心循环不依赖 AppHandle；异步出口（execute_tool / replan）因 Rust 闭包生命周期
/// 限制走泛型参数。生产薄壳 run_model_loop 传入 AppHandle 实现，测试传 stub。
pub struct ModelLoopDeps<'a> {
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
}

/// 模型工具循环薄壳：只做 AppHandle 依赖装配——
/// 读配置/Key、构 HTTP 客户端、把 widget emit / 审计 / skill_finish / execute_tool /
/// replan 包成回调，实际循环逻辑全在 run_model_loop_core（可注入 mock server 集成测试）。
pub async fn run_model_loop(
    app: AppHandle,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
) -> Result<(String, Vec<TaskRef>), CommandError> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let api_key = crate::bot::read_api_key()?;
    if api_key.trim().is_empty() {
        return Err(CommandError::ApiKeyMissing);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let http = LlmHttp {
        client,
        base_url: cfg.base_url,
        api_key,
        model: cfg.model,
        // None/非法值 → Openai（旧行为零影响）
        provider: crate::bot::ApiProvider::from_cfg(cfg.api_provider.as_deref()),
        max_tokens: crate::bot::resolve_max_tokens(cfg.max_tokens),
    };
    // 会话隔离：流式事件（bot-chat-delta 等）只由交互实例广播；
    // 后台定时任务（interactive=false）不向挂件推流——否则后台执行的输出会
    // 串进用户当前会话的 streaming 气泡。Skill 归属同理按 session 过滤。
    let stream_to_widget = stop.is_interactive();
    let session_id: Option<&str> = stop.session_id();
    // 流式事件出口统一收口（会话隔离）：非交互实例（后台定时任务）
    // 不向挂件发任何流式增量，防后台执行输出串进用户当前会话的 streaming 气泡。
    // payload 统一注入 sessionId，前端按当前会话过滤——
    // 事件不带会话标记时，两个会话并行跑 A 的流式增量会串进 B 正在显示的气泡。
    let emit = |event: &str, mut payload: serde_json::Value| {
        if stream_to_widget {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("sessionId".into(), serde_json::json!(session_id));
            }
            let _ = app.emit_to("widget", event, payload);
        }
    };
    let deps = ModelLoopDeps {
        emit: &emit,
        audit: &|level, event, kv| crate::audit::write_event(&app, level, event, &kv),
        audit_log: &|line| crate::bot::audit_log(&app, line),
        skill_finish: &|ok, reason| crate::bot_skills::skill_finish(&app, ok, reason, session_id),
        // 阶段 3.3（口径：只读走注入）：核心每轮要读「本会话活动技能快照」决定是否短路，
        // 但**不给它表**——写生命周期（收尾/暂停/清理）一律留在核心之外或既有回调里。
        active_skill_run: &|session_id| crate::bot_skills::active_skill_run_for(&app, session_id),
    };
    // 僵尸终态清理上移到薄壳（口径：写操作不该在模型循环核心）：
    // 上轮遗留的 Completed/Failed/Terminated run 会在第 0 轮被 advance 短路（agent 假死根因）。
    // 位置与原核心内调用等价——都发生在进入轮循环之前。
    crate::bot_skills::clear_terminal_skill_runs(&app);
    let execute_tool = |name: String, args: String, trace: crate::bot::ToolCallTrace| {
        let app = app.clone();
        async move { crate::bot::execute_tool_traced(&app, &name, &args, Some(stop), &trace).await }
    };
    let replan = |plan: crate::bot_plan::PlanState, reason: String| {
        let app = app.clone();
        async move { crate::bot_plan::replan(&app, &plan, &reason).await }
    };
    run_model_loop_core(
        &http,
        msgs,
        max_rounds,
        stop,
        plan_state,
        &deps,
        execute_tool,
        replan,
    )
    .await
}

/// 模型工具循环核心：流式请求（思考拆分 + 工具折叠事件）、进程内执行工具。
/// 配置/Key/HTTP 客户端/副作用出口全部注入，不依赖 AppHandle。
/// msgs 需已含 system 消息；返回 (最终正文, 任务引用)。
/// 轮数上限由调用方传入：默认 DEFAULT_MAX_ROUNDS（50），多步 Skill 可自报 max_rounds 覆盖。
/// plan_state（PREVR 第 2 层）：复杂任务的动态计划；工具连续失败时触发
/// Replan（重规划剩余步骤，≤MAX_REPLANS 次）。None = 无计划自由循环。
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
    XP: std::future::Future<Output = (String, Vec<TaskRef>)>,
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
    let tools: serde_json::Value = serde_json::from_str(TOOLS()).unwrap();

    let mut msgs = msgs;
    let emit = deps.emit;
    let audit = deps.audit;
    let audit_log = deps.audit_log;
    let skill_finish = deps.skill_finish;
    let active_skill_run = deps.active_skill_run;
    let session_id: Option<&str> = stop.session_id();
    // 僵尸终态清理已上移到薄壳 `run_model_loop`（阶段 3.3：核心只读技能状态，
    // 写生命周期不在核心——原位置与现在等价，都在进入轮循环之前）
    // 最多 max_rounds 轮（工具循环），每轮流式输出；收到 tool_calls 则执行后把结果续进对话
    let mut collected_refs: Vec<TaskRef> = Vec::new();
    let mut function_calls_total: usize = 0;
    let mut soft_warn_sent: bool = false;
    // 防幻觉汇报守卫：本轮是否实际执行过变更类工具；补一轮机会每次对话只用一次
    let mut mutation_done: bool = false;
    let mut claim_retry_used: bool = false;
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
    let mut plan_state = plan_state;
    // 上轮 streamed 文本快照：
    // AwaitConfirm/Finish/Fail/Terminate 跳出主循环时，返回 user 已看到的文本
    let mut last_streamed = String::new();
    for round in 0..max_rounds {
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
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Fail(reason) => {
                    let _hint = skill_finish(false, &reason);
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Terminate(reason) => {
                    let _hint = skill_finish(false, &reason);
                    return Ok((last_streamed.clone(), collected_refs));
                }
            }
        }
        // body 按协议分支——内部消息流保持 OpenAI
        // 形状不动，只在发请求前这一边界转换（bot_anthropic::build_anthropic_body）。
        // 转换失败（理论不可达，msgs 必含 user 消息）记审计并报错，不静默发出畸形请求。
        let body = match http.provider {
            crate::bot::ApiProvider::Openai => serde_json::json!({
                "model": http.model,
                "messages": msgs,
                "tools": tools,
                "stream": true
            }),
            crate::bot::ApiProvider::Anthropic => {
                match crate::bot_anthropic::build_anthropic_body(
                    &http.model,
                    &msgs,
                    &tools,
                    http.max_tokens,
                    true,
                ) {
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
                }
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
            match req.send().await {
                Ok(r) => {
                    if !r.status().is_success()
                        && is_retryable_llm_status(r.status().as_u16())
                        && attempt < MAX_LLM_ATTEMPTS
                    {
                        audit(
                            crate::audit::AuditLevel::Warn,
                            "llm.retry",
                            vec![
                                ("status", r.status().as_u16().to_string()),
                                ("attempt", attempt.to_string()),
                            ],
                        );
                        drop(r);
                        tokio::time::sleep(LLM_RETRY_DELAY).await;
                        continue;
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
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let hint = skill_finish(false, "大模型 API 错误");
            let err = CommandError::LlmApiError {
                status: status.as_u16(),
                body_preview: format!("{}{hint}", text.chars().take(300).collect::<String>()),
            };
            // 带 code 的失败审计：`grep 'code=LLM_API_ERROR'` 即可统计网关侧失败
            let mut kv = crate::audit::error_kv(&err);
            kv.push(("status", status.as_u16().to_string()));
            audit(crate::audit::AuditLevel::Warn, "llm.response", kv);
            return Err(err);
        }
        audit(
            crate::audit::AuditLevel::Info,
            "llm.response",
            vec![("status", status.as_u16().to_string())],
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
        let mut saw_done_or_finish = false;
        let mut last_finish_reason: Option<String> = None;
        let mut stream_error: Option<String> = None;
        let mut tc_index_overflow_logged = false;
        // Anthropic 模式 token 用量累积（message_start 的 input / message_delta 的
        // output，parse_anthropic_event 顺带捞出），回合结束写 llm.usage 审计
        let mut usage_input: u64 = 0;
        let mut usage_output: u64 = 0;
        // Anthropic 模式 content block index → 工具槽位重映射：
        // Anthropic 的块序号连文本块一起数，直接当工具序号累积会留下
        // 幽灵空条目，严格 API 会 400（详见 bot_anthropic::ToolSlotMapper 注释）
        let mut tool_slot_mapper = crate::bot_anthropic::ToolSlotMapper::new();

        let mut stopped = false;
        {
            // 单行 SSE 处理（主循环与流尾残余行冲刷共用）；
            // 返回 Some = 流内错误载荷，调用方收尾报错
            let mut handle_line = |line: &str| -> Option<String> {
                // 行解析按协议分支，解析出的
                // ParsedChunk 走同一消费逻辑（think 拆分 / 工具累积 / 流完整性 /
                // finish_reason 处理全部两协议共享）
                let parsed = match http.provider {
                    crate::bot::ApiProvider::Openai => parse_sse_chunk(line)?,
                    crate::bot::ApiProvider::Anthropic => {
                        match crate::bot_anthropic::parse_anthropic_event(line) {
                            Some(mut ev) => {
                                if let Some(u) = ev.usage {
                                    usage_input += u.input_tokens;
                                    usage_output += u.output_tokens;
                                }
                                // content block index → 稠密工具槽位（防幽灵条目）
                                for d in &mut ev.chunk.tool_calls {
                                    tool_slot_mapper.remap(d);
                                }
                                ev.chunk
                            }
                            None => return None,
                        }
                    }
                };
                if parsed.is_done {
                    saw_done_or_finish = true;
                    return None;
                }
                if let Some(reason) = parsed.finish_reason {
                    saw_done_or_finish = true;
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
                        emit("bot-chat-delta", serde_json::json!({ "text": normal }));
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
                        // 新工具调用开始：推折叠行给挂件
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
            while let Some(chunk) = stream.next().await {
                if stop.stopped() {
                    stopped = true;
                    break;
                }
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
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

        // Anthropic 模式：回合结束把流内捞到的 token 用量写审计
        if http.provider == crate::bot::ApiProvider::Anthropic
            && (usage_input > 0 || usage_output > 0)
        {
            audit(
                crate::audit::AuditLevel::Info,
                "llm.usage",
                vec![
                    ("input_tokens", usage_input.to_string()),
                    ("output_tokens", usage_output.to_string()),
                ],
            );
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
                emit("bot-chat-delta", serde_json::json!({ "text": tail }));
            }
        }

        if stopped {
            let hint = skill_finish(false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }

        // 流完整性检查：未见 [DONE]/finish_reason 的
        // 干净 EOF = 流被截断（中间代理 idle cut 等）。残缺 tool_calls 不得执行
        // （不能靠 parse_args 失败落 Null 侥幸兜底，要有显式防线）。
        if !saw_done_or_finish {
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
            // 防幻觉汇报守卫：声称完成变更但本轮没动过手 →
            // 注入系统提醒补一轮，逼模型实际调工具或如实说明（最多补一次）
            if !mutation_done && !claim_retry_used && claims_mutation(&final_text) {
                claim_retry_used = true;
                audit_log(&format!(
                    "hallucination_guard | 声称变更但未调工具，补一轮: {}",
                    crate::bot::truncate_for_log(&final_text, 100)
                ));
                msgs.push(serde_json::json!({"role": "assistant", "content": final_text}));
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "【系统提示】你刚才声称完成了变更，但本轮没有任何变更类工具调用成功，数据实际没有变化。请立即调用对应工具实际执行（删除用 delete_task、完成用 complete_task、编辑用 edit_task、子任务用 add_subtask/remove_subtask、绑定产物用 link_file_to_task（任务卡执行流程内有效）、生成文档用 create_word/create_excel/create_ppt/create_pdf；逐步执行模式下子任务勾选由系统完成，不要代调 toggle_subtask）；若确实无法执行（任务不存在/被安全闸门拦截/无权限等），如实向用户说明原因，禁止再次声称已完成。"
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
            let _ = skill_finish(true, "");
            collected_refs = merge_task_refs_dedup(collected_refs);
            return Ok((final_text.clone(), collected_refs));
        }

        // 模型请求工具：进程内执行，结果回填后继续下一轮
        msgs.push(serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": args}
            })).collect::<Vec<_>>()
        }));
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
                continue;
            }
            function_calls_total += 1;
            if should_fuse(function_calls_total) {
                let hint = skill_finish(false, "单轮 Function 调用超上限");
                audit_log(&format!(
                    "fuse | 单轮 Function 调用超过 {} 次，已熔断",
                    MAX_FUNCTION_CALLS_PER_TURN
                ));
                return Ok((fuse_message(&final_text, &hint), collected_refs));
            }
            // 软警告（SOFT_WARN_AT）：置标志，推迟到本轮 tool 响应全部回填后再注入——
            // 若在此直接 push user 消息，会插进 assistant(tool_calls) 与 tool 响应之间，
            // 破坏「tool_calls 后必须紧跟 tool 消息」的协议，下一轮请求被 API 拒为
            // 400 invalid params（实锤：两次 400 均紧跟 soft_warn 注入）
            if !soft_warn_sent && function_calls_total >= SOFT_WARN_AT {
                soft_warn_sent = true;
                soft_warn_queued = true;
                audit_log(&format!(
                    "soft_warn | Function 调用达 {} 次（上限 {}），追加收尾提醒",
                    SOFT_WARN_AT, MAX_FUNCTION_CALLS_PER_TURN
                ));
            }
            // 把 /stop 守卫透传给 execute_tool，run_python 在途可中断；
            // turn + tool_call_id 一并下传，工具审计可按轮回放（见 bot::ToolCallTrace）
            let (result, refs) = execute_tool(
                name.clone(),
                args.clone(),
                crate::bot::ToolCallTrace {
                    turn: Some(round),
                    tool_call_id: Some(id.clone()),
                },
            )
            .await;
            // 按执行结果置位——被门禁拦截/用户拒绝/执行失败的
            // 变更工具不算「动过手」，幻觉守卫对后续虚假汇报保持拦截能力
            if mutation_succeeded(name, &result) {
                mutation_done = true;
            }
            emit(
                "bot-tool-done",
                serde_json::json!({ "id": id, "name": name, "args": args }),
            );
            audit_log(&format!(
                "tool: {} | args: {} | result: {}",
                // 工具名是模型给的字符串，直插可伪造日志行
                crate::bot::truncate_for_log(name, 60),
                crate::bot::truncate_for_log(args, 500),
                crate::bot::truncate_for_log(&result, 300)
            ));
            collected_refs.extend(refs);
            // PREVR 第 1 层：工具失败检测。判定走全链路统一口径
            // （audit::tool_call_failed）——门禁拦截/熔断/暂停/拒绝都能识别；
            // 同工具连续失败才升级——单次失败先提示换策略。
            // 与 soft_warn 同理：提示推迟到本轮 tool 响应全部回填后注入（协议安全）
            let failed = crate::audit::tool_call_failed(name, &result);
            if failed {
                if last_failed_tool.as_deref() == Some(name.as_str()) {
                    consec_failures += 1;
                } else {
                    consec_failures = 1;
                    last_failed_tool = Some(name.clone());
                }
                let reason = crate::bot::truncate_for_log(&result, 200);
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
        // 本轮 tool 响应已全部回填（tool_calls → tool×N 序列完整），此时注入提示才合法
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
                    "【系统提示】你已累计调用 {SOFT_WARN_AT} 个工具（全程累计），最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                    MAX_FUNCTION_CALLS_PER_TURN - SOFT_WARN_AT
                ),
            }));
        }
        // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
        last_streamed = final_text.clone();
    }
    // 轮数熔断补审计（单轮工具熔断已有 fuse 日志，对称留痕）
    let hint = skill_finish(false, "对话轮数超限");
    let err = CommandError::Internal(format!("对话轮数超限{hint}"));
    let mut kv = crate::audit::error_kv(&err);
    kv.push(("max_rounds", max_rounds.to_string()));
    audit(crate::audit::AuditLevel::Warn, "fuse_rounds", kv);
    Err(err)
}

// ────────────────────────────────────────────────────────────────────
// 测试：feed_think / parse_sse_chunk / TOOLS schema
// ────────────────────────────────────────────────────────────────────

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

    #[test]
    fn mutation_succeeded_true_on_real_success() {
        assert!(mutation_succeeded("complete_task", "已完成任务「买菜」"));
        assert!(mutation_succeeded(
            "link_file_to_task",
            "已绑定文件到任务卡"
        ));
        assert!(mutation_succeeded(
            "create_word",
            "已生成 Word 文档：/tmp/x.docx"
        ));
    }

    #[test]
    fn mutation_succeeded_false_on_gate_block() {
        // 原子工具被 AtomicGuard 拦截：⚠️ 开头 → 不算动过手，幻觉守卫保持拦截能力
        assert!(!mutation_succeeded(
            "link_file_to_task",
            "⚠️ link_file_to_task 是 Skill 末尾绑产物的内部原子，不允许裸调。"
        ));
    }

    #[test]
    fn mutation_succeeded_false_on_user_reject_and_failure() {
        assert!(!mutation_succeeded(
            "delete_task",
            "用户拒绝了删除，任务未删除"
        ));
        assert!(!mutation_succeeded("create_task", "新建任务失败：磁盘只读"));
        assert!(!mutation_succeeded(
            "complete_task",
            "未知工具：complete_task"
        ));
    }

    #[test]
    fn mutation_succeeded_false_for_readonly_tools() {
        // 只读工具即使返回成功文本也不算变更
        assert!(!mutation_succeeded("list_tasks", "已完成任务「买菜」"));
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
        // 网关异常：200 OK + choices:[] → 跳过
        let line = r#"data: {"choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
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
mod tools_schema_tests {
    use super::*;

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
            "list_tasks",
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
// ────────────────────────────────────────────────────────────────────
// 测试：max_rounds 解析 / fallback + 单轮 Function 调用熔断
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod rounds_fuse_tests {
    use super::*;

    #[test]
    fn skill_frontmatter_max_rounds_overrides_default() {
        // Skill frontmatter 声明 max_rounds: 25 → 生效，run_model_loop 收到 25
        let meta = crate::bot_skills::parse_meta(
            "---\nname: minimax-docx\nmax_rounds: 25\n---\n# body\n",
            "minimax-docx",
        );
        assert_eq!(meta.max_rounds, Some(25));
        assert_eq!(resolve_max_rounds(meta.max_rounds), 25);
    }

    #[test]
    fn skill_without_max_rounds_falls_back_to_default_50() {
        // 现有 Skill 未声明 max_rounds → None → fallback 默认 50（兼容不崩）
        let meta = crate::bot_skills::parse_meta("---\nname: x\ndescription: d\n---\nbody\n", "x");
        assert_eq!(meta.max_rounds, None);
        assert_eq!(resolve_max_rounds(meta.max_rounds), DEFAULT_MAX_ROUNDS);
        assert_eq!(DEFAULT_MAX_ROUNDS, 50);
    }

    #[test]
    fn fuse_trips_on_51st_function_call() {
        // MAX_FUNCTION_CALLS_PER_TURN = 50 实际生效：模拟主循环计数，
        // 构造 51 个 tool_calls → 第 51 次触发熔断并返回「⏹ 已熔断」消息
        assert_eq!(MAX_FUNCTION_CALLS_PER_TURN, 50);
        assert_eq!(SOFT_WARN_AT, 35);
        let mut calls = 0usize;
        let mut fused_msg: Option<String> = None;
        for _ in 0..51 {
            calls += 1;
            if should_fuse(calls) {
                fused_msg = Some(fuse_message("前文", ""));
                break;
            }
        }
        let msg = fused_msg.expect("51 次 Function 调用内必须触发熔断");
        assert!(msg.contains("⏹ 已熔断"), "熔断消息应含「⏹ 已熔断」：{msg}");
        assert!(msg.contains("50 次上限"), "熔断消息应带上限值：{msg}");
        // 边界：第 50 次放行，第 51 次熔断
        assert!(!should_fuse(50));
        assert!(should_fuse(51));
    }
}
