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

#[allow(unused_imports)]
pub use crate::bot::registry::claims_mutation;
// ── 拆分模块声明 ──
mod context;
mod evidence;
mod finalize;
mod fuse;
mod guard;
mod output;
mod sse;
mod stream;
mod types;

// 旧路径 1:1 保持：拆分前就是 pub 的项，拆分后仍从 crate::bot_model_loop:: 取。
// bot_model_loop 是私有模块，这些 pub 项对外不可达——「没人用」不等于写错，
// 故逐条豁免 unused_imports（属性只作用于紧邻的那条语句）。
#[allow(unused_imports)]
pub use self::sse::STREAM_CHUNK_IDLE_TIMEOUT;
#[allow(unused_imports)]
pub use self::stream::{
    accumulate_tool_call_delta, drain_sse_lines, merge_stream_usage, parse_openai_usage,
    parse_sse_chunk, MAX_TOOL_CALL_ARGUMENTS_BYTES, MAX_TOOL_CALL_INDEX,
};
#[allow(unused_imports)]
pub use self::types::{
    LlmHttp, LoopTrace, ModelLoopDeps, ParsedChunk, TokenUsage, ToolCallDelta, TurnStatus,
    WallBudget,
};

// 跨模块共用的私有项：门面转出，子模块之间零直接依赖。
pub(crate) use self::context::{
    is_context_overflow, is_retryable_llm_status, msg_chars, reindex_after_shed,
    shed_oldest_history,
};
pub(crate) use self::evidence::{
    context_sources, number_sourced, ToolEvidence, EVIDENCE_OUTPUT_CHARS,
};
pub(crate) use self::finalize::{
    decide_selfcheck, finalize_exit, finalize_selfcheck, FinalizeOutcome, SelfcheckDecision,
};
pub(crate) use self::fuse::{
    check_fuses, fuse_cap_for, fuse_message, inject_soft_warnings, rounds_fuse_exit, should_fuse,
    FUSE_MARKER, MAX_FUNCTION_CALLS_PER_REQUEST, ROUNDS_FUSE_MARKER, WALL_FUSE_MARKER,
};
pub(crate) use self::guard::finalize_without_tools;
pub(crate) use self::output::{apply_output_cap, DEFAULT_TOOL_OUTPUT_CAP_BYTES};
pub(crate) use self::sse::{read_sse_stream, stream_round};
pub(crate) use self::stream::{
    claim_mutation_fires, feed_think, mutation_succeeded, strip_thinking_key,
    unparsable_sse_payload, with_thinking,
};
pub(crate) use self::types::{
    ErrorDisposition, ErrorResponse, RequestOutcome, RequestPlan, RequestState, RequestWork,
    RoundCtx,
};

// 只有测试在用的私有项：按构建期切换——非测试构建里它们没人用，
// 转出会被 rustc 判 unused import。
#[cfg(test)]
pub(crate) use self::fuse::{fuse_cap_upper_bound, rounds_fuse_message, wall_fuse_message};
// ── 拆分模块声明 结束 ──

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

/// 从 ToolDef 注册表派生的变更声称检测。
/// 原「14 个动词 + 4 个整段」硬编码迁到 `bot::registry::ToolDef.claims_patterns`，
/// 加新 mutating 工具只需填自己的 pattern，检测逻辑零修改。
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
pub(crate) const LLM_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);

/// 上下文超长降级：保留的历史比例（砍掉一半再试）
const CONTEXT_SHED_KEEP_RATIO: f64 = 0.5;
/// 单次模型循环内的降级次数上限（防「砍一半仍超长 → 再砍 → 再超」打转）
const CONTEXT_SHED_MAX: u8 = 2;

/// 空 replan 出口：无计划/测试调用方直驱 run_model_loop_core 时传入，
/// 永不重规划。pub：bot_plan 模块未对集成测试公开，PlanState 在 tests/ 不可命名，
/// 以 fn item 形式传入绕开闭包参数类型标注问题。
pub async fn noop_replan(
    _plan: crate::bot_plan::PlanState,
    _reason: String,
) -> Option<Vec<String>> {
    None
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

/// send() 响应头超时：只覆盖连接+发请求+收到响应头，
/// 不含流式 body——服务端收下请求却挂起不回时兜底，防永久挂起
pub const LLM_HEADER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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
                serde_json::json!({
                    "status": status.as_str(),
                    "detail": detail,
                    // 本轮起点：前端回合秒表按真实起点算——回灌接回时用
                    // 「本地此刻」当起点，秒表会从零开始（回复其实已跑了一会儿）
                    "startedAt": session_id.and_then(crate::bot::stream_log::run_started_at),
                }),
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
            let text = apply_output_cap(&app, tool_output_cap, &name, out.text);
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
    // MCP 工具按需发现（放在拼装工具清单之前）：
    // 外部 MCP 服务器平时静默待命、不连着——但模型要看得见它的工具才能调它。
    // 快照缺失时在这里按需发现一次（连上→列工具→断开→落盘快照），此后不再连。
    // 子 agent 会话不参与：它们的工具面是静态白名单，不挂 MCP 工具（§5.1 边界）。
    let is_subagent = stop
        .session_id()
        .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
        .is_some();
    if !is_subagent {
        crate::bot::mcp::manager::shared()
            .ensure_tools_known(&app)
            .await;
    }
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
//
// clippy 的 too_many_lines 只数函数体的非注释行（现 274 行，阈值 200），
// 总量已收进 400 行内但代码行数仍超阈值——豁免暂留，再拆就得改已落地的
// 各单元签名或把共享状态改成 struct 传递，两者都越过「纯机械搬移」的边界。
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
    // 循环级共享量一次性备好（熔断上限与两个软警阈值 / 一次请求的固定件与
    // 跨轮状态）——见 prepare_round。
    let RoundCtx {
        fuse_cap,
        soft_warn_at,
        rounds_warn_at,
        mut plan,
    } = prepare_round(http, stop, deps, max_rounds);
    let mut msgs = msgs;
    // 条目级 system prompt：追加到消息栈末尾——OpenAI 直接多一条 system
    // 消息；Anthropic 分支 openai_msgs_to_anthropic 会把它归并进顶层 system 块。
    if let Some(sp) = &http.system_prompt {
        msgs.push(serde_json::json!({ "role": "system", "content": sp }));
    }
    let audit = deps.audit;
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
            // 墙钟熔断（轮边界检查）：期限已到就在这里退出，**带上上一轮的正文**（见 check_fuses）。
            if let Some(msg) = check_fuses(deps, &last_streamed, round) {
                body_outcome = Some("fused");
                return Ok((msg, collected_refs));
            }
            // 轮前准备：预算硬压（msgs 字符分级）+ 轮龄降密度（老工具结果降档）。
            compact_before_round(&mut msgs, &tool_msg_ages, deps, stop, round);
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
                                "⏸ 上一个操作正在等待你的确认——请先处理确认弹窗，再继续对话。"
                                    .into(),
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
                    &format!("正在思考 · 第 {} 轮", round + 1),
                );
            }
            // 请求段：装配 body → 发请求 → 失败重试/降级（见 request_round）。
            let resp =
                match request_round(http, &mut msgs, &mut plan, stop, deps, &mut tool_msg_ages)
                    .await?
                {
                    RequestOutcome::Response(r) => r,
                    RequestOutcome::Stopped => {
                        let hint = skill_finish(false, "用户停止");
                        return Ok((format!("⏹ 已停止{hint}"), collected_refs));
                    }
                };

            // 流消费：逐 chunk 解析 SSE → 正文 / 工具调用 / 思考块（见 stream_round）。
            let sse = read_sse_stream(
                http,
                resp,
                stop,
                deps,
                expecting_selfcheck,
                expecting_final_selfcheck,
            )
            .await?;
            let rs = stream_round(sse, deps, expecting_selfcheck, expecting_final_selfcheck)?;
            if rs.stopped {
                let hint = skill_finish(false, "用户停止");
                return Ok((
                    format!("{}\n\n⏹ 已停止{hint}", rs.final_text),
                    collected_refs,
                ));
            }
            let mut final_text = rs.final_text;
            let tool_calls = rs.tool_calls;
            let thinking_blocks = rs.thinking_blocks;
            let last_finish_reason = rs.last_finish_reason;

            if tool_calls.is_empty() {
                match finalize_without_tools(
                    &mut msgs,
                    &mut mutation_done,
                    &mut claim_retry_used,
                    &mut fabrication_notice,
                    &receipts,
                    &evidence,
                    &written_paths,
                    &mut session_tools_cache,
                    &mut function_calls_total,
                    &mut last_selfcheck_round,
                    &mut pending_selfcheck,
                    &mut expecting_selfcheck,
                    &mut expecting_final_selfcheck,
                    &mut final_selfcheck_snapshot,
                    &mut last_final_selfcheck_round,
                    &mut late_verdict_retries,
                    &mut length_continuations,
                    &mut length_prefix,
                    &mut final_text,
                    deps,
                    stop,
                    round,
                    &thinking_blocks,
                    &last_finish_reason,
                )
                .await?
                {
                    FinalizeOutcome::Continue => continue,
                    FinalizeOutcome::Proceed => {}
                    FinalizeOutcome::Stopped(msg) => {
                        return Ok((msg, collected_refs));
                    }
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

            match dispatch_round(
                &execute_tool,
                &replan,
                &mut msgs,
                &final_text,
                &tool_calls,
                &thinking_blocks,
                &mut collected_refs,
                &mut function_calls_total,
                &mut mutation_done,
                &mut receipts,
                &mut evidence,
                &mut written_paths,
                &mut expecting_selfcheck,
                &mut pending_selfcheck,
                &mut expecting_final_selfcheck,
                &mut final_selfcheck_snapshot,
                &mut last_selfcheck_round,
                &mut last_failed_tool,
                &mut last_fail_reason,
                &mut consec_failures,
                &mut fail_hint_queued,
                &mut replan_exhausted_logged,
                &mut soft_warn_sent,
                &mut soft_warn_queued,
                &mut tool_msg_ages,
                fuse_cap,
                soft_warn_at,
                plan_state.as_deref_mut(),
                deps,
                stop,
                round,
            )
            .await?
            {
                DispatchOutcome::Continue => {}
                DispatchOutcome::Stopped(msg) => {
                    return Ok((msg, collected_refs));
                }
                DispatchOutcome::Fused(msg) => {
                    body_outcome = Some("tool_fuse");
                    return Ok((msg, collected_refs));
                }
            }
            // 轮末软警告：轮数与墙钟各提醒一次（见 inject_soft_warnings）。
            inject_soft_warnings(
                deps,
                &mut msgs,
                &mut rounds_warn_sent,
                &mut wall_warn_sent,
                max_rounds,
                rounds_warn_at,
                round,
            );
            // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
            last_streamed = final_text.clone();
            // 轮数自报：1-based 累计，「轮」的唯一口径来源。
            // 报在轮末，轮数熔断（自然落出循环）因此能报满 max_rounds；早返回出口
            // （已停止 / Skill 收尾 / 单轮熔断）在轮中 return，漏报它们所在那轮——
            // 那些都是「没跑完的一轮」，不上报反而更贴近真实完成量。
            (deps.on_round)(round + 1);
        }
        // 轮数熔断补审计（单轮工具熔断已有 fuse 日志，对称留痕）
        body_outcome = Some("fused");
        Ok((
            rounds_fuse_exit(deps, &last_streamed, max_rounds),
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
/// 工具轮的执行出口。
enum DispatchOutcome {
    /// 本轮工具已执行完，继续下一轮
    Continue,
    /// 用户叫停：附「已停止」正文
    Stopped(String),
    /// 单轮 Function 调用熔断：附熔断正文
    Fused(String),
}

/// 工具轮：execute_tool 分发 / 结果回灌 / 凭证登记。
///
/// 出口：跑完一轮（Continue）/ 用户叫停（Stopped，附「已停止」正文）/
/// 单轮 Function 调用熔断（Fused，附熔断正文）。
//
// 本段顺序即不变量：assistant(tool_calls) 消息 → 逐个执行并回填 tool 响应 →
// Replan → 各类提示注入。拆细会把「tool_calls 后必须紧跟 tool 消息」的协议
// 约束切散到多处，反而更难看出哪一处破坏了它。
#[allow(clippy::too_many_lines)]
async fn dispatch_round<X, XP, R, RP>(
    execute_tool: &X,
    replan: &R,
    msgs: &mut Vec<serde_json::Value>,
    final_text: &str,
    tool_calls: &[(String, String, String)],
    thinking_blocks: &[serde_json::Value],
    collected_refs: &mut Vec<TaskRef>,
    function_calls_total: &mut usize,
    mutation_done: &mut bool,
    receipts: &mut Vec<String>,
    evidence: &mut Vec<ToolEvidence>,
    written_paths: &mut Vec<String>,
    expecting_selfcheck: &mut bool,
    pending_selfcheck: &mut Option<(String, String)>,
    expecting_final_selfcheck: &mut bool,
    final_selfcheck_snapshot: &mut Option<String>,
    last_selfcheck_round: &mut Option<usize>,
    last_failed_tool: &mut Option<String>,
    last_fail_reason: &mut Option<String>,
    consec_failures: &mut usize,
    fail_hint_queued: &mut Option<String>,
    replan_exhausted_logged: &mut bool,
    soft_warn_sent: &mut bool,
    soft_warn_queued: &mut bool,
    tool_msg_ages: &mut Vec<(usize, usize)>,
    fuse_cap: usize,
    soft_warn_at: usize,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
    round: usize,
) -> Result<DispatchOutcome, CommandError>
where
    X: Fn(String, String, crate::bot::ToolCallTrace) -> XP,
    XP: std::future::Future<Output = crate::bot::registry::ToolResult>,
    R: Fn(crate::bot_plan::PlanState, String) -> RP,
    RP: std::future::Future<Output = Option<Vec<String>>>,
{
    let emit = deps.emit;
    let audit = deps.audit;
    let skill_finish = deps.skill_finish;
    // 模型请求工具：进程内执行，结果回填后继续下一轮。
    // 模型以工具调用回应自检提示 = 以行动代回答：丢弃工具自检态，
    // 后续轮流式不再被抑制；终稿自检快照一并释放——最终回答以
    // 模型核验后的新产出为准，不能把旧快照顶回去。
    *expecting_selfcheck = false;
    *pending_selfcheck = None;
    *expecting_final_selfcheck = false;
    *final_selfcheck_snapshot = None;
    msgs.push(with_thinking(
        serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": args}
            })).collect::<Vec<_>>()
        }),
        thinking_blocks,
    ));
    if stop.stopped() {
        let hint = skill_finish(false, "用户停止");
        return Ok(DispatchOutcome::Stopped(format!(
            "{final_text}\n\n⏹ 已停止{hint}"
        )));
    }
    for (id, name, args) in tool_calls {
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
        *function_calls_total += 1;
        if should_fuse(*function_calls_total, fuse_cap) {
            let hint = skill_finish(false, "单轮 Function 调用超上限");
            audit(
                crate::audit::AuditLevel::Warn,
                "fuse",
                vec![("cap", fuse_cap.to_string())],
            );
            return Ok(DispatchOutcome::Fused(fuse_message(
                final_text, &hint, fuse_cap,
            )));
        }
        // 软警告（soft_warn_at，按会话熔断上限派生）：置标志，推迟到本轮 tool 响应全部回填后再注入——
        // 若在此直接 push user 消息，会插进 assistant(tool_calls) 与 tool 响应之间，
        // 破坏「tool_calls 后必须紧跟 tool 消息」的协议，下一轮请求被 API 拒为
        // 400 invalid params（实锤：两次 400 均紧跟 soft_warn 注入）
        if !*soft_warn_sent && *function_calls_total >= soft_warn_at {
            *soft_warn_sent = true;
            *soft_warn_queued = true;
            audit(
                crate::audit::AuditLevel::Info,
                "soft_warn",
                vec![
                    ("used", soft_warn_at.to_string()),
                    ("cap", fuse_cap.to_string()),
                ],
            );
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
            *mutation_done = true;
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
                *pending_selfcheck = Some((name.clone(), snippet));
            }
        }
        // 成败口径与全链路统一（ToolStatus 是唯一真相源）——bot-tool-done 的 ok 同源
        let failed = !tool_outcome.status.is_success();
        // 被闸拒掉的调用（Denied：预算触顶 / 白名单外 / 用户拒绝）不计入熔断
        // 计数——计数守的是「实际执行」的调用量。子 agent 收尾窗里模型可能
        // 成批发读类调用试探，若被拒的也计数，一次并行批就能烧光收尾裕量，
        // 模型永远轮不到写产物（写类在收尾窗放行、占裕量，正是设计语义）。
        if tool_outcome.status == crate::bot::registry::ToolStatus::Denied {
            *function_calls_total = function_calls_total.saturating_sub(1);
        }
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
        audit(
            crate::audit::AuditLevel::Info,
            "tool",
            vec![
                // 工具名是模型给的字符串：truncate_for_log = escape_for_log 别名（\n\r| 已转义），不可伪造日志行
                ("tool", crate::bot::truncate_for_log(name, 60)),
                ("args", crate::bot::truncate_for_log(args, 500)),
                ("result", crate::bot::truncate_for_log(result, 300)),
            ],
        );
        collected_refs.extend(refs.iter().cloned());
        // PREVR 第 1 层：工具失败检测。判定走全链路统一口径
        // （audit::tool_call_failed）——门禁拦截/熔断/暂停/拒绝都能识别；
        // 同工具连续失败才升级——单次失败先提示换策略。
        // 与 soft_warn 同理：提示推迟到本轮 tool 响应全部回填后注入（协议安全）
        if failed {
            if last_failed_tool.as_deref() == Some(name.as_str()) {
                *consec_failures += 1;
            } else {
                *consec_failures = 1;
                *last_failed_tool = Some(name.clone());
            }
            let reason = crate::bot::truncate_for_log(result, 200);
            *last_fail_reason = Some(reason.clone());
            *fail_hint_queued = Some(if *consec_failures >= 2 {
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
            *last_failed_tool = None;
            *last_fail_reason = None;
            *consec_failures = 0;
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
                    None => audit(
                        crate::audit::AuditLevel::Info,
                        "tool_image.skip",
                        vec![("detail", crate::bot::truncate_for_log(p, 200))],
                    ),
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
    if *consec_failures >= 2 {
        if let Some(plan) = plan_state {
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
                    return Ok(DispatchOutcome::Stopped(format!(
                        "{final_text}\n\n⏹ 已停止{hint}"
                    )));
                }
                if let Some(new_steps) = replan(snapshot, reason).await {
                    plan.steps = new_steps;
                    *fail_hint_queued = Some(format!(
                            "【系统提示】原计划执行受阻，已重新规划剩余步骤：\n{}\n请按新计划继续；若仍无法推进，如实向用户说明。",
                            plan.steps.join("\n")
                        ));
                }
            } else if !*replan_exhausted_logged {
                // 预算耗尽留痕（只记一次）——
                // 「连续失败持续发生但不再重规划」这件事不能零痕迹
                *replan_exhausted_logged = true;
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
        if !*expecting_selfcheck {
            *last_selfcheck_round = Some(round);
            *expecting_selfcheck = true;
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
    if *soft_warn_queued {
        *soft_warn_queued = false;
        msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】你已累计调用 {soft_warn_at} 个工具（全程累计），最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                    fuse_cap - soft_warn_at
                ),
            }));
    }
    Ok(DispatchOutcome::Continue)
}

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
fn prepare_round(
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

/// 轮前压缩：预算硬压（msgs 字符分级）+ 轮龄降密度（老工具结果随轮龄降档）。
fn compact_before_round(
    msgs: &mut [serde_json::Value],
    tool_msg_ages: &[(usize, usize)],
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
    round: usize,
) {
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
async fn request_round(
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
                    // 下面的报错文案与超长判定也要用同一份
                    let text = r.text().await.unwrap_or_default();
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
                let err: CommandError = format!("请求大模型失败：{e}{hint}").into();
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

#[cfg(test)]
mod tests;
