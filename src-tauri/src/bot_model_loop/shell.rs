//! 薄壳装配层：构造 LlmHttp 与依赖后进入核心循环。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{
    apply_output_cap, decide_selfcheck, run_model_loop_core, LlmHttp, LoopTrace, ModelLoopDeps,
    SelfcheckDecision, WallBudget, DEFAULT_TOOL_OUTPUT_CAP_BYTES, MAX_FUNCTION_CALLS_PER_REQUEST,
};
use crate::bot_chat::TaskRef;
use crate::bot_slash::StopGuard;
use crate::error::CommandError;
use tauri::AppHandle;
use tauri::Emitter;

/// 默认对话轮数（聊天 / 任务执行 / 逐步执行统一为 50）；
/// 多步 Skill 可在 SKILL.md frontmatter 自报 max_rounds 覆盖（见 resolve_max_rounds）；
/// 配置 `maxRounds` 亦可覆盖默认（Skill 仍最优先），常量单源 bot/params.rs。
pub(crate) const DEFAULT_MAX_ROUNDS: usize = crate::bot::params::DEFAULT_MAX_ROUNDS as usize;

/// 本轮工具循环的轮数上限：Skill 自报 max_rounds 最优先，其次配置 maxRounds，
/// 都没有 → DEFAULT_MAX_ROUNDS（单源 bot/params.rs）。
pub(crate) fn resolve_max_rounds(
    skill_max_rounds: Option<usize>,
    config_max_rounds: usize,
) -> usize {
    skill_max_rounds.unwrap_or(config_max_rounds)
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

/// 主循环推理参数选取：override 命中必须取覆盖条目的（resolve_model_override 已按
/// 条目值 > 全局 max_tokens 兜底算好）；未命中才按 active 条目解析（active 条目 >
/// 全局 > 内置默认，见 schema::effective_inference）。两路不可混用——覆盖端点打
/// active 条目的 temperature/top_p/system_prompt 是参数张冠李戴。
pub(crate) fn resolve_loop_inference(
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
        // 思考段回灌：默认关（维持现状），开了才把本轮 reasoning_content 随
        // 工具调用消息回灌历史（DeepSeek reasoner 缺了就 400，MiniMax M2/M3
        // 缺了思考链会断）。
        reasoning_echo: crate::bot::params::reasoning_echo_on(cfg.reasoning_echo_enabled),
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
