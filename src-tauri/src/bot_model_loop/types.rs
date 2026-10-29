//! 共享类型：模型循环的依赖结构、请求状态机与流结构。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use crate::error::CommandError;

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

/// 200 流内错误载荷
///
/// 上游在 **HTTP 200 的流里** 报错（Anthropic 协议的 `{"type":"error",...}`、
/// OpenAI 兼容网关的 `{"error":{...}}`）。此前只留一句 message，
/// 丢掉了上游自己标的 `type`——而可重试性恰恰由 type 决定：
/// `overloaded_error` / `api_error` 是瞬时故障（重发有意义），
/// `authentication_error` / `invalid_request_error` 是确定性拒绝（重发无意义）。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StreamError {
    /// 上游自己说的那句（截断到 200 字）
    pub message: String,
    /// 上游标的错误类型（Anthropic: `error.type`；OpenAI: `error.type` / `error.code`）
    pub kind: Option<String>,
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
    pub error: Option<StreamError>,
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
    /// 思考段回灌开关：开启后 OpenAI 兼容路径的 assistant(tool_calls) 消息
    /// 带上本轮的 reasoning_content 原文。
    ///
    /// MiniMax M2/M3、DeepSeek reasoner、Moonshot K2 这类模型要求思考段随工具
    /// 调用消息一起回灌历史：DeepSeek 缺了直接 400（"reasoning_content in the
    /// thinking mode must be passed back to the API"），Moonshot 报"thinking is
    /// enabled but reasoning_content is missing in assistant tool call message"，
    /// MiniMax 官方口径是丢了思考链后续轮次会失去上下文。
    ///
    /// 默认关（维持现状）：回灌要多付一份思考段的 input token，而受益只落在
    /// 那些要求回灌的模型上——是否值得由设置页的开关决定，值由实机对拍定。
    pub reasoning_echo: bool,
    /// 轮内工具结果滑动窗：累计工具结果超阈值时，把「最近 K 条之外」的
    /// 工具结果换成桩子（原文仍在痕迹表里，桩子给出 lookup_receipt 入口）。
    ///
    /// 默认开：阈值定在实测 p90 附近（约 9% 的提问会触发），触发的那些提问
    /// 省下的是「早期工具结果在轮内被反复重发」的那部分 token；召回闭环由
    /// `lookup_receipt` 兜着（桩子指路 + 对账闸仍拦编造）。
    pub in_turn_context_edit: bool,
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

/// 循环级共享量：把原先散在 run_model_loop_core 里的局部变量聚成结构体，
/// 让各提取单元能显式传参。字段与原局部变量一一对应，不带行为。
pub(crate) struct RoundCtx {
    /// 单轮 Function 调用熔断上限
    pub(crate) fuse_cap: usize,
    /// 工具数软警告阈值（cap × 90%）
    pub(crate) soft_warn_at: usize,
    /// 收尾窗阈值（cap × 95%）：达到后只放行写产物类工具，且这些调用不占配额。
    pub(crate) wrap_up_at: usize,
    /// 轮数软警告阈值（max_rounds × 90%，与 soft_warn_at 同口径）
    pub(crate) rounds_warn_at: usize,
    /// 一次请求的固定件与跨轮状态
    pub(crate) plan: RequestPlan,
}

/// 一次请求要用的固定件 + 请求段跨轮共享的可变状态。
pub(crate) struct RequestPlan {
    /// 请求 URL（协议分支派生一次：OpenAI /chat/completions，Anthropic /v1/messages）
    pub(crate) url: String,
    /// 工具 schema（按会话选取：子 agent 会话给 profile 白名单）
    pub(crate) tools: serde_json::Value,
    /// 跨轮可变状态（随会话共享，不随轮清零）
    pub(crate) state: RequestState,
}

/// 请求段的跨轮可变状态：usage 探测降级 / thinking 补救 / 上下文砍史次数。
pub(crate) struct RequestState {
    /// 网关是否接受 `stream_options.include_usage`（OpenAI 流式 usage 的前置）。
    /// 初值 true；收到 400 时翻 false 并原样重发一次，此后整场对话不再带该字段
    /// ——探测成本只付一次，且失败只降级统计口径、不影响回复本身。
    pub(crate) openai_usage_supported: bool,
    /// Anthropic 的 thinking 块是否已被丢弃过一次（签名失效时的补救，见请求失败分支）。
    /// 只补救一次：重发后仍报同样的错就是别的问题，不该无限重试。
    pub(crate) anthropic_thinking_dropped: bool,
    /// 上下文超长降级次数（全循环共享）：超长时砍历史重发，至多两次
    pub(crate) context_sheds: u8,
}

/// 请求段的出口：拿到 2xx 响应 / 用户在发请求途中叫停。
pub(crate) enum RequestOutcome {
    /// 拿到 2xx 响应（流还没开始读）
    Response(reqwest::Response),
    /// 用户叫停（重试间隙）：调用方组装「已停止」正文
    Stopped,
}

/// 请求段的工作集：降级路径要同时改 body / 会话消息 / 工具消息轮龄表。
pub(crate) struct RequestWork<'a> {
    /// 当前请求 body（降级后重建时原地替换）
    pub(crate) body: serde_json::Value,
    /// 会话消息（thinking 补救 / 砍史会改它）
    pub(crate) msgs: &'a mut Vec<serde_json::Value>,
    /// 工具消息轮龄表（砍史后要重排下标）
    pub(crate) tool_msg_ages: &'a mut Vec<(usize, usize)>,
}

/// 非 2xx 响应体：状态码 + 正文（降级判定与报错文案共用同一份）。
pub(crate) struct ErrorResponse {
    /// HTTP 状态码
    pub(crate) status: u16,
    /// 响应正文
    pub(crate) text: String,
}

/// 非 2xx 响应的处置结果。
pub(crate) enum ErrorDisposition {
    /// 重发同一请求（已降级 / 已砍史 / 已退避）
    Retry,
    /// 用户叫停
    Stopped,
    /// 报错收场
    Fatal(CommandError),
}
