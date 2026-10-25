//! Agent 运行参数注册表（Agent 透明化设计 §3）：
//!
//! **单一真相源查表**（仿 bot/registry.rs TOOLS_TABLE 哲学）——每个影响 agent
//! 行为的参数一条 `ParamDef`，元数据与生效值解析都在这一条里，
//! `bot_effective_params` 命令遍历出表，设置页参数卡渲染生效值/默认值/来源。
//! 以后加参数 = 表里加一条，展示自动跟随；解析函数也随条目一起给，
//! 不存在「加了 key 忘了加解析分支」这种静默漏解析。
//!
//! 两区：
//! - **可编辑区**（`editable=true`，config 驱动）：值 = config 字段解析（None →
//!   内置默认，source=default / Some → source=config）；
//! - **硬编码只读区**（`editable=false`）：调坏容易出问题的运行时常量
//!   （LLM 超时/重试、调度间隔、硬顶/硬上限），只展示不开放编辑（设计 §3.2）。
//!
//! 解析辅助 `resolve_*` 是唯一读取口：读取点不得各写一份 `unwrap_or(默认)`——
//! 否则「展示的默认值」与「实际生效值」会漂。

use serde::Serialize;

/// 硬编码只读条目的解析器：来源恒为 hardcoded。
macro_rules! hardcoded_resolver {
    ($v:expr) => {{
        fn f(_cfg: &BotConfig) -> (String, &'static str) {
            ($v.to_string(), "hardcoded")
        }
        f as fn(&BotConfig) -> (String, &'static str)
    }};
}

/// 可编辑条目的解析器：值走 resolve_* 取运行时真值（避免「看到 999、实际
/// 钳到 200」的脱节），来源按「config 里有没有这个字段」标注。
macro_rules! cfg_resolver {
    ($resolve:path, $field:ident) => {{
        fn f(cfg: &BotConfig) -> (String, &'static str) {
            let src = if cfg.$field.is_some() {
                "config"
            } else {
                "default"
            };
            ($resolve(cfg).to_string(), src)
        }
        f as fn(&BotConfig) -> (String, &'static str)
    }};
}

// 内置默认（单一事实源）

/// 模型循环默认轮数（原 bot_model_loop::DEFAULT_MAX_ROUNDS 同值；该处引用本常量）
pub const DEFAULT_MAX_ROUNDS: u32 = 50;
pub const MAX_ROUNDS_CAP: u32 = 200;
/// 轮数下界（少于 5 轮的循环连一次工具往返都跑不完）——钳制与设置页展示的
/// 可填范围共用这一个数。
pub const MAX_ROUNDS_MIN: u32 = 5;
/// 会话历史字符预算默认（原 bot_chat::HISTORY_BUDGET_CHARS 同值）
pub const DEFAULT_HISTORY_BUDGET_CHARS: u32 = 100_000;
pub const HISTORY_BUDGET_MIN: u32 = 20_000;
pub const HISTORY_BUDGET_MAX: u32 = 500_000;
/// 历史截断滞回目标比例默认（百分比）：超预算时一次裁到预算的 70%。
/// 贴边裁会让长会话几乎每轮都改一次缓存前缀，留 30% 余量能稳住好几轮。
pub const DEFAULT_HISTORY_TRUNCATE_TARGET_PCT: u32 = 70;
/// 滞回比例下界（50%）：再低就是历史断崖式变短，摘要都救不回来。
pub const HISTORY_TRUNCATE_TARGET_PCT_MIN: u32 = 50;
/// 滞回比例上界（90%）：再高等于没有余量，贴边裁的问题原样回来。
pub const HISTORY_TRUNCATE_TARGET_PCT_MAX: u32 = 90;
/// 思考段回灌开关默认（0 = 关）：维持现状，不为「可能受益」的模型族预付
/// 思考段的 input token；受益只落在明确要求回灌的模型上，由用户自己开。
pub const DEFAULT_REASONING_ECHO_ENABLED: u32 = 0;
/// 思考段回灌开关上界（1 = 开）：只有两态，中间值没有语义。
pub const REASONING_ECHO_ENABLED_MAX: u32 = 1;
/// 轮内工具结果滑动窗开关默认值（1 = 开）：触发条件是实测 p90 附近的阈值，
/// 只影响约 9% 的提问，且召回闭环由 lookup_receipt 兜着。
pub const DEFAULT_IN_TURN_CONTEXT_EDIT: u32 = 1;
/// 轮内工具结果滑动窗开关上界（1 = 开）：只有两态。
pub const IN_TURN_CONTEXT_EDIT_MAX: u32 = 1;
/// 子 agent 默认预算（行上预算缺省时的兜底；db::subagents 的 DEFAULT_* 引用此处）：
/// max_tool_calls 仅是缺省兜底——配置了 max_function_calls 时子 agent 同源全域上限
pub const DEFAULT_SUBAGENT_MAX_TURNS: u32 = 30;
pub const DEFAULT_SUBAGENT_MAX_TOOL_CALLS: u32 = 100;
pub const DEFAULT_SUBAGENT_MAX_WALL_SECS: u64 = 600;
/// 子 agent 墙钟下界（秒）——钳制与设置页展示的可填范围共用这一个数。
pub const SUBAGENT_MAX_WALL_SECS_MIN: u64 = 30;
/// 子 agent 墙钟上界（秒）
pub const SUBAGENT_MAX_WALL_SECS_MAX: u64 = 3600;
/// web_search 默认结果条数（原 bot_web SEARCH_MAX_RESULTS 同值）
pub const DEFAULT_SEARCH_MAX_RESULTS: u32 = 8;

// 钳制单源：写侧落盘（bot_set_config）与读侧生效（下方 resolve_*）共用同一组
// 边界——字面量只落在这里，防两边各自改数后静默漂移。子 agent 轮数的边界是
// subagents 域不变量，单源在 db::subagents::clamp_subagent_max_turns。

/// 模型循环轮数钳 5..=200
pub fn clamp_max_rounds(n: u32) -> u32 {
    n.clamp(MAX_ROUNDS_MIN, MAX_ROUNDS_CAP)
}

/// 会话历史字符预算钳 20K..=500K
pub fn clamp_history_budget_chars(n: u32) -> u32 {
    n.clamp(HISTORY_BUDGET_MIN, HISTORY_BUDGET_MAX)
}

/// 历史截断滞回比例钳 50..=90（百分比）
pub fn clamp_history_truncate_target_pct(n: u32) -> u32 {
    n.clamp(
        HISTORY_TRUNCATE_TARGET_PCT_MIN,
        HISTORY_TRUNCATE_TARGET_PCT_MAX,
    )
}

/// 思考段回灌开关钳 0..=1（0 = 关，1 = 开）
pub fn clamp_reasoning_echo_enabled(n: u32) -> u32 {
    n.min(REASONING_ECHO_ENABLED_MAX)
}

/// 轮内工具结果滑动窗开关钳 0..=1（0 = 关，1 = 开）
pub fn clamp_in_turn_context_edit(n: u32) -> u32 {
    n.min(IN_TURN_CONTEXT_EDIT_MAX)
}

/// 子 agent 墙钟秒数钳 30..=3600
pub fn clamp_subagent_max_wall_secs(n: u64) -> u64 {
    n.clamp(SUBAGENT_MAX_WALL_SECS_MIN, SUBAGENT_MAX_WALL_SECS_MAX)
}

/// 软墙钟的**收尾预留窗口**（秒）：墙钟预算的 10%，下限 30s。
///
/// 业界口径是「软顶在边界检查 + 为收尾预留一段预算」：最后这段预算专门留给
/// 「模型把已经拿到的结果整理成收尾 JSON」。不留这段，跑到临近上限的子
/// agent 会被外层墙钟硬杀——正在跑的工具、已经说出口的正文一起没了。
///
/// 与硬墙钟的分工：软墙钟只**拒绝新调用**（不打断在飞工具），硬墙钟是绝对
/// 兜底。预留窗口是软的，因此这里给的是下限而非精确比例。
pub fn subagent_wall_reserve_secs(budget_secs: u64) -> u64 {
    (budget_secs / 10).max(SUBAGENT_WALL_RESERVE_MIN_SECS)
}

/// 收尾预留窗口下限（秒）：预算很小时 10% 不够模型跑完最后一轮
pub const SUBAGENT_WALL_RESERVE_MIN_SECS: u64 = 30;

/// 联网搜索结果条数硬顶（clamp 下界恒为 1：count=0 无意义）
pub const SEARCH_MAX_RESULTS_CAP: u32 = 10;
/// 联网搜索结果条数下界（0 条搜索无意义）
pub const SEARCH_MAX_RESULTS_MIN: u32 = 1;

/// 联网搜索结果条数钳 1..=SEARCH_MAX_RESULTS_CAP
pub fn clamp_search_max_results(n: u32) -> u32 {
    n.clamp(SEARCH_MAX_RESULTS_MIN, SEARCH_MAX_RESULTS_CAP)
}

/// 单次请求工具调用熔断上限硬顶：写侧（子 agent 预算解析）与读侧（模型循环
/// 派生链末端的 `.clamp`）共用这一个数，防「预算按 500 记账、循环按 999 放行」。
pub const MAX_FUNCTION_CALLS_CAP: u32 = 500;
/// 工具调用熔断下界（0 次调用无意义）
pub const MAX_FUNCTION_CALLS_MIN: u32 = 1;

pub fn clamp_max_function_calls(n: u32) -> u32 {
    n.clamp(MAX_FUNCTION_CALLS_MIN, MAX_FUNCTION_CALLS_CAP)
}

/// verifier 拒绝后的修复重试预算：未配置时的默认次数。
pub const DEFAULT_VERIFIER_RETRY_MAX: u32 = 1;
/// verifier 重试预算上限（超过 3 次还失败的任务属硬阻塞，如实汇报比继续烧
/// token 更省；与配置落盘前的写侧钳制共用）。
pub const VERIFIER_RETRY_MAX_CAP: u32 = 3;

pub fn clamp_verifier_retry_max(n: u32) -> u32 {
    n.clamp(0, VERIFIER_RETRY_MAX_CAP)
}

/// verifier 重试预算生效值：读取点唯一入口。
///
/// 入参是「配置里有没有这一项」而不是整份 config——调用方
/// （bot_chat 的提示词尾段）手上是 View，View 与 BotConfig 在这个可选字段上
/// 同形，传 Option 才能让两边共用同一份「默认 + 钳制」。
pub fn resolve_verifier_retry_max(configured: Option<u32>) -> u32 {
    clamp_verifier_retry_max(configured.unwrap_or(DEFAULT_VERIFIER_RETRY_MAX))
}

/// 单条工具结果回填上限钳（上限 200K 字节，UTF-8）。
///
/// 这个上限原先只落在模型循环的生效点（`.min(200_000)`），而读侧 resolver 原样
/// 把配置值透给设置页——用户设 500000，设置页显示 500000，运行时按 200000 走，
/// 两边都不知道发生了什么。写侧钳制 + 读侧 + 生效点三处共用这一个边界。
pub const MAX_TOOL_OUTPUT_BYTES_CAP: u32 = 200_000;

pub fn clamp_max_tool_output_bytes(n: u32) -> u32 {
    n.min(MAX_TOOL_OUTPUT_BYTES_CAP)
}

/// 任务执行提问预算（ask_user 每次执行可问次数）钳 1..=5
pub const ASK_BUDGET_MIN: u8 = 1;
pub const ASK_BUDGET_MAX: u8 = 5;

/// 任务执行提问预算（ask_user 每次执行可问次数）钳 1..=5
pub fn clamp_ask_budget(n: u8) -> u8 {
    n.clamp(ASK_BUDGET_MIN, ASK_BUDGET_MAX)
}

// 解析辅助（读取口唯一）

pub fn resolve_max_rounds_cfg(cfg: &BotConfig) -> u32 {
    clamp_max_rounds(cfg.max_rounds.unwrap_or(DEFAULT_MAX_ROUNDS))
}

pub fn resolve_history_budget_chars(cfg: &BotConfig) -> u32 {
    clamp_history_budget_chars(
        cfg.history_budget_chars
            .unwrap_or(DEFAULT_HISTORY_BUDGET_CHARS),
    )
}

/// 历史截断滞回比例（百分比，50..=90）：参数表展示与写盘都用这个整数口径。
pub fn resolve_history_truncate_target_pct(cfg: &BotConfig) -> u32 {
    clamp_history_truncate_target_pct(
        cfg.history_truncate_target_pct
            .unwrap_or(DEFAULT_HISTORY_TRUNCATE_TARGET_PCT),
    )
}

/// 历史截断滞回比例（0.5..=0.9）：超预算时一次裁到预算的这个比例。
/// 比例越低裁得越狠（前缀越稳、丢的历史越多）。
pub fn resolve_history_truncate_target_ratio(cfg: &BotConfig) -> f64 {
    resolve_history_truncate_target_pct(cfg) as f64 / 100.0
}

/// 思考段回灌开关取值（0 = 关 / 1 = 开）。
///
/// 入口收原始 `Option<u32>` 而不是配置结构：配置本体与配置视图是两个类型，
/// 但开关只有一个判定口径——写成两套 resolver 迟早漂。
pub fn reasoning_echo_enabled_value(raw: Option<u32>) -> u32 {
    clamp_reasoning_echo_enabled(raw.unwrap_or(DEFAULT_REASONING_ECHO_ENABLED))
}

/// 思考段回灌是否开启（读点用这个布尔口径）。
pub fn reasoning_echo_on(raw: Option<u32>) -> bool {
    reasoning_echo_enabled_value(raw) == 1
}

/// 思考段回灌开关（0 = 关 / 1 = 开）——参数表解析用。
pub fn resolve_reasoning_echo_enabled(cfg: &BotConfig) -> u32 {
    reasoning_echo_enabled_value(cfg.reasoning_echo_enabled)
}

/// 轮内工具结果滑动窗开关取值（0 = 关 / 1 = 开）。
pub fn in_turn_context_edit_value(raw: Option<u32>) -> u32 {
    clamp_in_turn_context_edit(raw.unwrap_or(DEFAULT_IN_TURN_CONTEXT_EDIT))
}

/// 轮内工具结果滑动窗是否开启（读点用这个布尔口径）。
pub fn in_turn_context_edit_on(raw: Option<u32>) -> bool {
    in_turn_context_edit_value(raw) == 1
}

/// 轮内工具结果滑动窗开关（0 = 关 / 1 = 开）——参数表解析用。
pub fn resolve_in_turn_context_edit(cfg: &BotConfig) -> u32 {
    in_turn_context_edit_value(cfg.in_turn_context_edit)
}

pub fn resolve_subagent_budget(cfg: &BotConfig) -> crate::db::SubagentBudget {
    crate::db::SubagentBudget {
        max_turns: crate::db::clamp_subagent_max_turns(
            cfg.subagent_max_turns.unwrap_or(DEFAULT_SUBAGENT_MAX_TURNS),
        ),
        // 工具调用上限全域统一（合并两条设置）：子 agent 与主循环同源
        // max_function_calls（缺省 100，钳 1..=MAX_FUNCTION_CALLS_CAP）；LLM 显式
        // 给的预算仍可覆盖
        max_tool_calls: clamp_max_function_calls(
            cfg.max_function_calls
                .unwrap_or(DEFAULT_SUBAGENT_MAX_TOOL_CALLS),
        ),
        max_wall_seconds: clamp_subagent_max_wall_secs(
            cfg.subagent_max_wall_secs
                .unwrap_or(DEFAULT_SUBAGENT_MAX_WALL_SECS),
        ),
    }
}

pub fn resolve_search_max_results(cfg: &BotConfig) -> u32 {
    clamp_search_max_results(cfg.search_max_results.unwrap_or(DEFAULT_SEARCH_MAX_RESULTS))
}

/// 任务执行提问预算（原 workflow_settings.ask_budget，随工作流退役迁入 bot 参数）
pub fn resolve_ask_budget_cfg(cfg: &BotConfig) -> u8 {
    clamp_ask_budget(cfg.ask_budget.unwrap_or(3))
}

use crate::bot::config::types::BotConfig;

// 参数表

/// 参数定义（编译期常量表；value 的解析在 `bot_effective_params` 内查 cfg 字段）
pub struct ParamDef {
    /// 稳定 key（前端 React key + 未来深链定位），点分分层
    pub key: &'static str,
    /// 中文展示名
    pub label: &'static str,
    /// 分组（设置页参数卡按此分节）：loop / subagent / search / model / tools / schedule
    pub category: &'static str,
    /// 一句话说明（含调大的代价）。
    ///
    /// **不写具体数字**：默认值由 resolver 在空配置下的输出派生（见
    /// `effective_params`），可填范围由 `min`/`max`（钳制常量）给出——文案里
    /// 再抄一遍数字就是第二份真相源，改一边漏另一边不会编译失败也不会测试失败。
    pub hint: &'static str,
    /// 可填范围下界（None = 不限或不适用）
    pub min: Option<u64>,
    /// 可填范围上界（None = 不限或不适用）
    pub max: Option<u64>,
    /// true = bot-config.json 有对应字段可改；false = 硬编码只读
    pub editable: bool,
    /// 生效值解析（值展示串, 来源）。
    ///
    /// 解析随表走，不另开一个 `match key` —— 那等于给「加参数」留了一条能静默
    /// 漏掉的旁路：表里加了一条 key 而忘了加 match 臂，不编译失败、不测试失败，
    /// 只在设置页显示字面量 `?subagent.foo` 并把来源标成 hardcoded。
    resolve: fn(&BotConfig) -> (String, &'static str),
}

/// 生效值视图（serde camelCase 直出前端）
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ParamView {
    pub key: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub hint: &'static str,
    /// 生效值展示串（config 解析或硬编码常量）
    pub value: String,
    /// 内置默认值（**由 resolver 在空配置下的输出派生**，不是手写字面量——
    /// 手写的那份曾与真正的内置默认各自漂移，且没有任何测试把两者绑在一起）
    pub default: String,
    /// config = 配置覆盖生效 / default = 用内置默认 / hardcoded = 硬编码只读
    pub source: &'static str,
    pub editable: bool,
    /// 可填范围下界（None = 不限或不适用）
    pub min: Option<u64>,
    /// 可填范围上界（None = 不限或不适用）
    pub max: Option<u64>,
}

/// 全量参数表：可编辑区在前，硬编码只读区在后（前端按 category 分组渲染）
pub static PARAMS_TABLE: &[ParamDef] = &[
    // ── 可编辑：循环/历史 ──
    ParamDef {
        key: "loop.max_rounds",
        label: "模型循环最大轮数",
        category: "loop",
        hint: "单次对话/任务执行的 LLM 轮数上限；调大=更复杂任务但失控时更费 token。Skill 自报轮数优先于此值",
        min: Some(MAX_ROUNDS_MIN as u64),
        max: Some(MAX_ROUNDS_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_max_rounds_cfg, max_rounds),
    },
    ParamDef {
        key: "loop.max_function_calls",
        label: "单次请求工具调用熔断",
        category: "loop",
        hint: "全域工具调用熔断上限（子 agent 预算同源此值）；工作流节点等长链任务可调大；软警告在 70% 处自动触发",
        min: Some(MAX_FUNCTION_CALLS_MIN as u64),
        max: Some(MAX_FUNCTION_CALLS_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_budget_tool_calls, max_function_calls),
    },
    ParamDef {
        key: "loop.history_budget_chars",
        label: "会话历史字符预算",
        category: "loop",
        hint: "超出后最旧消息先丢并生成摘要；调大=长对话记忆更好但上下文成本更高",
        min: Some(HISTORY_BUDGET_MIN as u64),
        max: Some(HISTORY_BUDGET_MAX as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_history_budget_chars, history_budget_chars),
    },
    ParamDef {
        key: "loop.history_truncate_target_pct",
        label: "历史截断滞回比例（%）",
        category: "loop",
        hint: "超出预算时一次裁到预算的这个比例，留余量稳住缓存前缀；调小=前缀更稳但丢的历史更多",
        min: Some(HISTORY_TRUNCATE_TARGET_PCT_MIN as u64),
        max: Some(HISTORY_TRUNCATE_TARGET_PCT_MAX as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_history_truncate_target_pct, history_truncate_target_pct),
    },
    ParamDef {
        key: "loop.reasoning_echo_enabled",
        label: "思考段回灌",
        category: "loop",
        hint: "1 = 把本轮思考段随工具调用消息回灌历史（MiniMax M2/M3、DeepSeek reasoner、Moonshot K2 要求回灌，缺了会 400）；0 = 不回灌（省一份思考段的 input token）",
        min: Some(0),
        max: Some(REASONING_ECHO_ENABLED_MAX as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_reasoning_echo_enabled, reasoning_echo_enabled),
    },
    ParamDef {
        key: "loop.in_turn_context_edit",
        label: "轮内工具结果滑动窗",
        category: "loop",
        hint: "1 = 轮内工具结果累计超阈值时，把最近几条之外的结果换成可召回的桩子（省重复重发的 token）；0 = 不裁（工具结果全量留在上下文里）",
        min: Some(0),
        max: Some(IN_TURN_CONTEXT_EDIT_MAX as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_in_turn_context_edit, in_turn_context_edit),
    },
    // ── 可编辑：子 agent ──
    ParamDef {
        key: "subagent.max_turns",
        label: "子 agent 轮数预算",
        category: "subagent",
        hint: "LLM 派发子 agent 未显式给预算时的默认轮数；任何配置（含 LLM 自报）都不能越过硬顶",
        min: Some(crate::db::MIN_SUBAGENT_TURNS as u64),
        max: Some(crate::db::MAX_TURNS_HARD_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_budget_turns, subagent_max_turns),
    },
    ParamDef {
        key: "subagent.max_wall_secs",
        label: "子 agent 墙钟预算（秒）",
        category: "subagent",
        hint: "子 agent 从起跑计的最长时长；排队不计入",
        min: Some(SUBAGENT_MAX_WALL_SECS_MIN),
        max: Some(SUBAGENT_MAX_WALL_SECS_MAX),
        editable: true,
        resolve: cfg_resolver!(resolve_budget_wall, subagent_max_wall_secs),
    },
    // ── 可编辑：搜索 ──
    ParamDef {
        key: "search.max_results",
        label: "联网搜索默认条数",
        category: "search",
        hint: "web_search 未指定 count 时的结果条数；后端有硬顶",
        min: Some(SEARCH_MAX_RESULTS_MIN as u64),
        max: Some(SEARCH_MAX_RESULTS_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_search_max_results, search_max_results),
    },
    // ── 可编辑：模型（已有字段入表展示） ──
    ParamDef {
        key: "model.max_tokens",
        label: "全局 max_tokens",
        category: "model",
        hint: "仅 Anthropic 协议发送；OpenAI 兼容模式不发送。条目级设置优先于此全局值",
        min: Some(crate::bot::config::schema::MIN_MAX_TOKENS as u64),
        max: Some(crate::bot::config::schema::MAX_MAX_TOKENS as u64),
        editable: true,
        resolve: cfg_resolver!(plain_max_tokens, max_tokens),
    },
    ParamDef {
        key: "model.reasoning_effort",
        label: "推理强度默认档",
        category: "model",
        hint: "off/low/medium/high；按模型族映射到线上参数（聊天输入栏 ⚡ 可按会话覆盖）",
        min: None,
        max: None,
        editable: true,
        resolve: cfg_resolver!(plain_reasoning, reasoning_effort),
    },
    ParamDef {
        key: "tools.python_timeout_secs",
        label: "Python 执行超时（秒）",
        category: "tools",
        hint: "run_python 默认超时；工具参数可单次覆盖",
        min: Some(1),
        max: Some(crate::py::runtime::MAX_TIMEOUT_SECS),
        editable: true,
        resolve: cfg_resolver!(plain_py_timeout, python_timeout_secs),
    },
    // ── 可编辑：工具输出截断（字节口径） ──
    ParamDef {
        key: "tools.max_output_bytes",
        label: "工具结果截断字节数",
        category: "tools",
        hint: "单条工具结果回填模型上下文的截断上限（字节，UTF-8）；超大输出会挤占上下文，设 0 = 显式关闭截断",
        min: Some(0),
        max: Some(MAX_TOOL_OUTPUT_BYTES_CAP as u64),
        editable: true,
        resolve: resolve_max_output_bytes,
    },
    ParamDef {
        key: "tools.ask_budget",
        label: "任务提问预算（次）",
        category: "tools",
        hint: "ask_user 每次任务执行最多提问次数，超限按 AI 假设继续",
        min: Some(ASK_BUDGET_MIN as u64),
        max: Some(ASK_BUDGET_MAX as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_ask_budget_cfg, ask_budget),
    },
    // ── 硬编码只读区（调坏易出问题，仅展示） ──
    ParamDef {
        key: "loop.tool_window_trigger_chars",
        label: "工具结果滑动窗触发阈值（字符）",
        category: "loop",
        hint: "轮内工具结果累计超过这个量就裁；实测单次提问的工具结果 p90 落在两万字出头，这个值取在 p90 与 p95 之间",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot_model_loop::TOOL_WINDOW_TRIGGER_CHARS),
    },
    ParamDef {
        key: "loop.tool_window_keep_recent",
        label: "工具结果滑动窗保留条数",
        category: "loop",
        hint: "最近这几条工具结果永不裁；少一条模型就可能忘了上一轮做过什么（重跑工具 = 重跑副作用）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot_model_loop::TOOL_WINDOW_KEEP_RECENT),
    },
    ParamDef {
        key: "subagent.max_running",
        label: "子 agent 全局并发",
        category: "subagent",
        hint: "全局同时运行的子 agent 上限；排队在 SubagentGate（单会话上限见「每会话并发」）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot_orchestrator::MAX_GLOBAL_RUNNING),
    },
    ParamDef {
        key: "subagent.per_session_running",
        label: "子 agent 每会话并发",
        category: "subagent",
        hint: "同一个主会话同时运行的子 agent 上限；与全局上限取更紧者（全局见「全局并发」）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot_orchestrator::MAX_PER_SESSION_RUNNING),
    },
    ParamDef {
        key: "subagent.turns_hard_cap",
        label: "子 agent 轮数硬顶",
        category: "subagent",
        hint: "任何配置（含 LLM 自报）都不能超过的轮数硬顶",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::db::MAX_TURNS_HARD_CAP),
    },
    ParamDef {
        key: "search.count_cap",
        label: "搜索条数硬顶",
        category: "search",
        hint: "工具参数 count 的硬上限（防单次搜索拉爆上下文）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(SEARCH_MAX_RESULTS_CAP),
    },
    ParamDef {
        key: "search.output_bytes",
        label: "搜索输出字节上限",
        category: "search",
        hint: "搜索结果合并渲染后的截断长度（字节，UTF-8）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot_web::SEARCH_OUTPUT_CAP_BYTES),
    },
    ParamDef {
        key: "llm.stream_idle_secs",
        label: "流式空闲超时（秒）",
        category: "model",
        hint: "两个流式 chunk 间隔超过此值判定连接死亡；总时长不设限（长生成合法）",
        min: None,
        max: None,
        editable: false,
        resolve: resolve_stream_idle_secs,
    },
    ParamDef {
        key: "llm.retry",
        label: "LLM 请求重试",
        category: "model",
        hint: "429/5xx 自动重试次数与间隔",
        min: None,
        max: None,
        editable: false,
        resolve: resolve_llm_retry,
    },
    ParamDef {
        key: "schedule.scan_secs",
        label: "定时扫描间隔（秒）",
        category: "schedule",
        hint: "定时任务/工作流调度的扫描周期（执行最早延迟该时长）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot_scheduler::SCHED_TICK_SECS),
    },
    ParamDef {
        key: "trace.span_text_max",
        label: "痕迹单条文本上限（字节）",
        category: "tools",
        hint: "执行痕迹里单条工具入参/结果的落库钳制（超出留审计）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::db::trace::SPAN_TEXT_MAX),
    },
    ParamDef {
        key: "trace.diff_max_lines",
        label: "文件 diff 最大行数",
        category: "tools",
        hint: "执行痕迹里 unified diff 的行数钳制（±行计数不随截断丢失）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::db::trace::MAX_DIFF_LINES),
    },
    ParamDef {
        key: "trace.retention_days",
        label: "执行痕迹保留期（天）",
        category: "tools",
        hint: "痕迹三表与回滚快照的保留期：启动即清理一次，此后每天一次；数据管理可手动清理",
        min: Some(crate::db::trace::TRACE_RETENTION_MIN_DAYS as u64),
        max: Some(crate::db::trace::TRACE_RETENTION_MAX_DAYS as u64),
        editable: false,
        resolve: hardcoded_resolver!(crate::db::trace::TRACE_RETENTION_DAYS),
    },
    ParamDef {
        key: "memory.extract_min_interval_secs",
        label: "记忆抽取最小间隔（秒）",
        category: "memory",
        hint: "同一会话两次自动抽取的最小间隔（节流是抽取成本的第一道闸）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::memory::extract::EXTRACT_MIN_INTERVAL_SECS),
    },
];

/// 生效值解析：可编辑项查 cfg 字段（None → 默认 / Some → 值），只读项出硬编码常量。
/// 子 agent 预算三个键共用同一份 `resolve_subagent_budget`，这里按需取字段。
fn resolve_budget_tool_calls(cfg: &BotConfig) -> String {
    resolve_subagent_budget(cfg).max_tool_calls.to_string()
}

fn resolve_budget_turns(cfg: &BotConfig) -> String {
    resolve_subagent_budget(cfg).max_turns.to_string()
}

fn resolve_budget_wall(cfg: &BotConfig) -> String {
    resolve_subagent_budget(cfg).max_wall_seconds.to_string()
}

fn plain_max_tokens(cfg: &BotConfig) -> String {
    // 默认值取后端常量（32768）——此前这里手写 8192，而后端早已把默认提到 32K
    // （截断自愈那条），两处就此各说各话：表里写 8192、运行时按 32768 走。
    cfg.max_tokens
        .unwrap_or(crate::bot::config::schema::DEFAULT_MAX_TOKENS)
        .to_string()
}

fn plain_py_timeout(cfg: &BotConfig) -> String {
    cfg.python_timeout_secs
        .unwrap_or(crate::py::runtime::DEFAULT_TIMEOUT_SECS)
        .to_string()
}

/// 推理强度无钳制但有静默回退：非法值下游一律按 medium 生效，展示同步取回退结果
fn plain_reasoning(cfg: &BotConfig) -> String {
    crate::bot::reasoning::EffortLevel::from_cfg(cfg.reasoning_effort.as_deref())
        .as_str()
        .to_string()
}

/// max_output_bytes 有第三态：Some(0) = 显式关闭截断，展示串不是数字
///
/// 展示值必须等于生效值：这里显示的就是模型循环实际会用的那个数（同一个
/// `clamp_max_tool_output_bytes`），配置手改越过上限时设置页也能看出被钳到哪。
fn resolve_max_output_bytes(cfg: &BotConfig) -> (String, &'static str) {
    match cfg.max_tool_output_bytes {
        Some(v) if v > 0 => (clamp_max_tool_output_bytes(v).to_string(), "config"),
        Some(_) => ("不截断（显式关闭）".into(), "config"),
        None => (
            crate::bot_model_loop::DEFAULT_TOOL_OUTPUT_CAP_BYTES.to_string(),
            "default",
        ),
    }
}

/// 流式空闲超时是 `Duration`：展示串取秒（该超时只按秒粒度配置与告警）
fn resolve_stream_idle_secs(_cfg: &BotConfig) -> (String, &'static str) {
    (
        crate::bot_model_loop::STREAM_CHUNK_IDLE_TIMEOUT
            .as_secs()
            .to_string(),
        "hardcoded",
    )
}

/// 重试策略是两个独立常量（次数 / 间隔），拼成一个展示串——任一常量改动都
/// 自动带进设置页，不留「显示 2 次实际 3 次」的缝
fn resolve_llm_retry(_cfg: &BotConfig) -> (String, &'static str) {
    let delay_ms = crate::bot_model_loop::LLM_RETRY_DELAY.as_millis() as f64;
    (
        format!(
            "{}次/{}s",
            crate::bot_model_loop::MAX_LLM_ATTEMPTS,
            delay_ms / 1000.0
        ),
        "hardcoded",
    )
}

/// 遍历参数表产出生效值视图（命令体直接映射）
///
/// **默认值从 resolver 派生**：同一份 resolver 在空配置下的输出就是内置默认，
/// 不再另抄一份字面量。此前 `ParamDef.default` 是手写 `&'static str`，
/// 与 resolver 各自漂移且没有测试把两者绑在一起——后端改了默认、设置页仍
/// 写着旧数，页面上「默认 X」与实际生效的 Y 就是两回事。
pub fn effective_params(cfg: &BotConfig) -> Vec<ParamView> {
    // 空配置 = 各 resolver 的内置默认分支（BotConfig::default 里 Agent 运行
    // 参数全为 None，正是「未配置」这一态）
    let empty = BotConfig::default();
    PARAMS_TABLE
        .iter()
        .map(|d| {
            let (value, source) = (d.resolve)(cfg);
            let default = (d.resolve)(&empty).0;
            ParamView {
                key: d.key,
                label: d.label,
                category: d.category,
                hint: d.hint,
                value,
                default,
                source,
                editable: d.editable,
                min: d.min,
                max: d.max,
            }
        })
        .collect()
}

/// 设置页「Agent 运行参数」卡的数据源：生效值 + 默认值 + 来源 + 可填范围。
///
/// 命令而不是函数：设置页要的是**当前这份配置**下的生效值（用户改过什么、
/// 哪些被钳过、哪些还在用内置默认），core 里的表只回答「有哪些参数」。
#[tauri::command]
pub fn bot_effective_params(app: tauri::AppHandle) -> Vec<ParamView> {
    let cfg = crate::bot::config::load_config(&app);
    effective_params(&cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每条参数都必须能被表里自带的 resolver 解析出真值。
    ///
    /// 这条测试替代原来的兜底臂：解析曾经是一个独立的 `match key`，兜底是
    /// `other => (format!("?{other}"), "hardcoded")` —— 表里加了一条 key 而忘了
    /// 加 match 臂时，不编译失败、不测试失败，只在设置页显示字面量
    /// `?subagent.foo`。解析收进表之后，漏解析根本构造不出来。
    #[test]
    fn every_param_resolves_to_a_real_value() {
        for d in PARAMS_TABLE {
            let (value, source) = (d.resolve)(&empty_cfg());
            assert!(!value.is_empty(), "{} 解析出空值", d.key);
            assert!(
                !value.starts_with('?'),
                "{} 落进了兜底（key 与 resolver 对不上）",
                d.key
            );
            assert!(
                matches!(source, "config" | "default" | "hardcoded"),
                "{} 来源非法：{source}",
                d.key
            );
            assert_eq!(
                d.editable,
                source != "hardcoded",
                "{}：可编辑性与来源标注不一致（editable={} source={source}）",
                d.key,
                d.editable
            );
        }
    }

    /// key 不得重复：设置页按 key 定位，重复会静默遮蔽前一条。
    #[test]
    fn param_keys_are_unique() {
        let mut keys: Vec<&str> = PARAMS_TABLE.iter().map(|d| d.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "参数表里有重复 key");
    }

    fn empty_cfg() -> BotConfig {
        BotConfig {
            base_url: String::new(),
            model: String::new(),
            api_key: None,
            bypass_llm_on_pre_step_hit: true,
            allowed_dirs: Vec::new(),
            tavily_key: None,
            tavily_enabled: None,
            brave_key: None,
            brave_enabled: None,
            python_timeout_secs: None,
            perm_mode: None,
            api_provider: None,
            max_tokens: None,
            reasoning_effort: None,
            max_function_calls: None,
            final_selfcheck_round_gap: None,
            selfcheck_model_id: None,
            guard_claim_keywords: None,
            verifier_retry_max: None,
            archive_after_days: None,
            models_by_provider: None,
            active_model_id: None,
            ui_font_size: None,
            memory_consolidation: None,
            memory_control: None,
            memory_tuning: None,
            mcp_servers: None,
            disabled_vendors: Vec::new(),
            verified_vendors: Vec::new(),
            schema_version: 1,
            max_rounds: None,
            history_truncate_target_pct: None,
            reasoning_echo_enabled: None,
            in_turn_context_edit: None,
            history_budget_chars: None,
            tool_rules: None,
            subagent_max_turns: None,
            subagent_max_wall_secs: None,
            search_max_results: None,
            max_tool_output_bytes: None,
            ask_budget: None,
        }
    }

    /// 硬编码只读区每行的展示值必须等于代码里真正生效的那个常量。
    ///
    /// 这些行的 resolve 取的是真常量（MAX_GLOBAL_RUNNING / MAX_TURNS_HARD_CAP /
    /// clamp 上界 / SEARCH_OUTPUT_CAP_BYTES / 流式超时 / 重试两常量），而 `default`
    /// 现在是**从 resolver 派生**的（空配置下的输出）——只读区两者恒等。
    /// 这里逐条把派生值与常量双向钉死：任一侧单独改动而不改另一侧即红。
    #[test]
    fn hardcoded_rows_track_runtime_constants() {
        let views = effective_params(&empty_cfg());
        // 取一行的展示值，并断言生效值与 default 同源（只读区两者恒等）
        let shown = |k: &str| -> String {
            let v = views.iter().find(|v| v.key == k).unwrap();
            assert_eq!(v.source, "hardcoded", "{k} 应属硬编码只读区");
            assert_eq!(v.value, v.default, "{k}: 生效值与 default 不同源");
            v.default.clone()
        };
        assert_eq!(
            shown("subagent.max_running"),
            crate::bot_orchestrator::MAX_GLOBAL_RUNNING.to_string()
        );
        assert_eq!(
            shown("subagent.per_session_running"),
            crate::bot_orchestrator::MAX_PER_SESSION_RUNNING.to_string()
        );
        assert_eq!(
            shown("subagent.turns_hard_cap"),
            crate::db::MAX_TURNS_HARD_CAP.to_string()
        );
        assert_eq!(
            shown("search.count_cap"),
            SEARCH_MAX_RESULTS_CAP.to_string(),
            "搜索条数硬顶须与 clamp_search_max_results 上界同源"
        );
        assert_eq!(
            shown("search.output_bytes"),
            crate::bot_web::SEARCH_OUTPUT_CAP_BYTES.to_string()
        );
        assert_eq!(
            shown("llm.stream_idle_secs"),
            crate::bot_model_loop::STREAM_CHUNK_IDLE_TIMEOUT
                .as_secs()
                .to_string()
        );
        assert_eq!(
            shown("llm.retry"),
            format!(
                "{}次/{}s",
                crate::bot_model_loop::MAX_LLM_ATTEMPTS,
                crate::bot_model_loop::LLM_RETRY_DELAY.as_millis() as f64 / 1000.0
            )
        );
        // 扫描间隔 / 痕迹三表的两个钳制 + 保留期：这四条原先是表里的裸字面量
        // （"30" / "16384" / "2000" / "30"），与真常量恰好相等纯属手写对了——
        // 真常量一改，表里的旧数会静默留着（没有任何测试把两者绑住）。
        // 现在 resolve 直接取常量，测试再钉一层。
        assert_eq!(
            shown("schedule.scan_secs"),
            crate::bot_scheduler::SCHED_TICK_SECS.to_string()
        );
        assert_eq!(
            shown("trace.span_text_max"),
            crate::db::trace::SPAN_TEXT_MAX.to_string()
        );
        assert_eq!(
            shown("trace.diff_max_lines"),
            crate::db::trace::MAX_DIFF_LINES.to_string()
        );
        assert_eq!(
            shown("trace.retention_days"),
            crate::db::trace::TRACE_RETENTION_DAYS.to_string()
        );
        // 记忆抽取限频：设置页文案里的「≥N 分钟一次」引用的就是这个常量
        assert_eq!(
            shown("memory.extract_min_interval_secs"),
            crate::memory::extract::EXTRACT_MIN_INTERVAL_SECS.to_string()
        );
        // 「每会话并发」此前只在 max_running 的 hint 里以文字出现，表里查不到；
        // 现在它有自己的行，且展示值即派发闸真正认的那个数
        assert!(PARAMS_TABLE
            .iter()
            .any(|d| d.key == "subagent.per_session_running"));
    }

    /// 工具调用熔断上界单源：子 agent 预算解析与模型循环派生链末端同一个数。
    ///
    /// 两处曾各写一份 `clamp(1, 500)`，其中薄壳那份还会被绕过（测试直驱 core
    /// 传 deps.fuse_cap_override 时不经薄壳）——同一条边界抄两份，改一处漏一处
    /// 不编译失败也不测试失败。
    #[test]
    fn max_function_calls_cap_is_single_sourced() {
        let mut cfg = empty_cfg();
        cfg.max_function_calls = Some(u32::MAX);
        assert_eq!(
            resolve_subagent_budget(&cfg).max_tool_calls,
            MAX_FUNCTION_CALLS_CAP,
            "预算侧须钳到 MAX_FUNCTION_CALLS_CAP"
        );
        assert_eq!(clamp_max_function_calls(0), 1, "0 次调用无意义，钳到 1");
        assert_eq!(clamp_max_function_calls(30), 30, "区间内原样放行");
        // 模型循环消费端（bot_model_loop::fuse_cap_for）用的是同一个常量的
        // usize 投影——两边改数必须同时，否则一个按 500 记账一个按 999 放行
        assert_eq!(
            MAX_FUNCTION_CALLS_CAP as usize,
            crate::bot_model_loop::fuse_cap_upper_bound()
        );
    }

    /// verifier 重试预算：默认值与钳制边界单源，写侧落盘与读侧提示词同口径。
    #[test]
    fn resolve_verifier_retry_max_single_source() {
        assert_eq!(resolve_verifier_retry_max(None), 1, "未配置 = 默认 1");
        assert_eq!(resolve_verifier_retry_max(Some(0)), 0, "0 = 不重试，合法");
        assert_eq!(
            resolve_verifier_retry_max(Some(99)),
            VERIFIER_RETRY_MAX_CAP,
            "越界钳到上限"
        );
        assert_eq!(
            clamp_verifier_retry_max(u32::MAX),
            VERIFIER_RETRY_MAX_CAP,
            "写侧与读侧共用同一钳制"
        );
    }

    /// 表完整性：key 唯一 + 每项 category/hint 非空 + 行数下限（防漏登记）
    #[test]
    fn params_table_keys_unique_and_populated() {
        let mut seen = std::collections::HashSet::new();
        for d in PARAMS_TABLE {
            assert!(seen.insert(d.key), "重复 key：{}", d.key);
            assert!(!d.label.is_empty() && !d.hint.is_empty() && !d.category.is_empty());
        }
        assert!(
            PARAMS_TABLE.len() >= 20,
            "参数表 {} 项，疑似漏登记",
            PARAMS_TABLE.len()
        );
    }

    /// 参数表里的默认展示值必须等于代码里真正生效的那个数。
    ///
    /// `tools.max_output_bytes` 的 `default` 曾写「不截断」，而空配置下 resolver
    /// 返 30000、模型循环按 `DEFAULT_TOOL_OUTPUT_CAP_BYTES` 走——表里自相矛盾，且
    /// 没有任何测试把这两个数字绑在一起。这条把该绑的都绑上：改任一侧不同步即红。
    ///
    /// 现在 `default` 由 resolver 派生（不再是手写字面量），但「派生的结果等于
    /// 运行时常量」这件事仍要钉住——派生走的是 resolver 路径，运行时走的是
    /// 模型循环里的常量，两条路必须落在同一个数上。
    #[test]
    fn params_table_defaults_match_runtime_constants() {
        let views = effective_params(&empty_cfg());
        let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
        assert_eq!(
            by_key("tools.max_output_bytes").default,
            crate::bot_model_loop::DEFAULT_TOOL_OUTPUT_CAP_BYTES.to_string(),
            "参数表展示的默认截断字节数与模型循环实际生效的默认值不符"
        );
        // 其余可编辑项：展示的默认 = 运行时内置默认常量
        assert_eq!(
            by_key("loop.max_rounds").default,
            DEFAULT_MAX_ROUNDS.to_string()
        );
        assert_eq!(
            by_key("loop.max_function_calls").default,
            DEFAULT_SUBAGENT_MAX_TOOL_CALLS.to_string()
        );
        assert_eq!(
            by_key("loop.history_budget_chars").default,
            DEFAULT_HISTORY_BUDGET_CHARS.to_string()
        );
        assert_eq!(
            by_key("subagent.max_turns").default,
            DEFAULT_SUBAGENT_MAX_TURNS.to_string()
        );
        assert_eq!(
            by_key("subagent.max_wall_secs").default,
            DEFAULT_SUBAGENT_MAX_WALL_SECS.to_string()
        );
        assert_eq!(
            by_key("search.max_results").default,
            DEFAULT_SEARCH_MAX_RESULTS.to_string()
        );
        // 全局 max_tokens：默认 32768（后端早已从 8192 提到 32K），表里若还写
        // 8192 就是页面与运行时各说各话
        assert_eq!(
            by_key("model.max_tokens").default,
            crate::bot::config::schema::DEFAULT_MAX_TOKENS.to_string()
        );
        // 越界配置：展示值必须已经是钳过的生效值，不能是用户填的原值
        let mut cfg = empty_cfg();
        cfg.max_tool_output_bytes = Some(MAX_TOOL_OUTPUT_BYTES_CAP + 100_000);
        let views = effective_params(&cfg);
        let v = views
            .iter()
            .find(|v| v.key == "tools.max_output_bytes")
            .unwrap();
        assert_eq!(v.value, MAX_TOOL_OUTPUT_BYTES_CAP.to_string());
    }

    /// 可填范围（min/max）必须等于实际生效的钳制边界。
    ///
    /// 设置页把这两个数直接显示成「可填 X–Y」——它跟真正生效的钳制不是同一个
    /// 数的话，页面就在骗人（用户填 999，运行时按 200 走，页面却说可填到 999）。
    #[test]
    fn editable_rows_ranges_match_clamp_bounds() {
        let views = effective_params(&empty_cfg());
        let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
        let range = |k: &str| -> (Option<u64>, Option<u64>) {
            let v = by_key(k);
            (v.min, v.max)
        };
        assert_eq!(
            range("loop.max_rounds"),
            (Some(MAX_ROUNDS_MIN as u64), Some(MAX_ROUNDS_CAP as u64))
        );
        assert_eq!(
            range("loop.max_function_calls"),
            (
                Some(MAX_FUNCTION_CALLS_MIN as u64),
                Some(MAX_FUNCTION_CALLS_CAP as u64)
            )
        );
        assert_eq!(
            range("loop.history_budget_chars"),
            (
                Some(HISTORY_BUDGET_MIN as u64),
                Some(HISTORY_BUDGET_MAX as u64)
            )
        );
        assert_eq!(
            range("subagent.max_turns"),
            (
                Some(crate::db::MIN_SUBAGENT_TURNS as u64),
                Some(crate::db::MAX_TURNS_HARD_CAP as u64)
            )
        );
        assert_eq!(
            range("subagent.max_wall_secs"),
            (
                Some(SUBAGENT_MAX_WALL_SECS_MIN),
                Some(SUBAGENT_MAX_WALL_SECS_MAX)
            )
        );
        assert_eq!(
            range("search.max_results"),
            (
                Some(SEARCH_MAX_RESULTS_MIN as u64),
                Some(SEARCH_MAX_RESULTS_CAP as u64)
            )
        );
        assert_eq!(
            range("tools.ask_budget"),
            (Some(ASK_BUDGET_MIN as u64), Some(ASK_BUDGET_MAX as u64))
        );
        // 全局 max_tokens：页面显示的「可填 X–Y」要与写侧/读侧同一个边界
        assert_eq!(
            range("model.max_tokens"),
            (
                Some(crate::bot::config::schema::MIN_MAX_TOKENS as u64),
                Some(crate::bot::config::schema::MAX_MAX_TOKENS as u64)
            )
        );
        // 痕迹保留期：手动清理的输入框公示「1–365」，与实际删的范围同源
        assert_eq!(
            range("trace.retention_days"),
            (
                Some(crate::db::trace::TRACE_RETENTION_MIN_DAYS as u64),
                Some(crate::db::trace::TRACE_RETENTION_MAX_DAYS as u64)
            )
        );
        // 工具输出截断：0 是合法值（显式关闭截断），不是下界 1
        assert_eq!(
            range("tools.max_output_bytes"),
            (Some(0), Some(MAX_TOOL_OUTPUT_BYTES_CAP as u64))
        );
        // 范围两端都要真的被钳制函数认（否则展示与生效又是一回事）
        assert_eq!(clamp_max_rounds(MAX_ROUNDS_MIN), MAX_ROUNDS_MIN);
        assert_eq!(clamp_max_rounds(MAX_ROUNDS_CAP), MAX_ROUNDS_CAP);
        assert_eq!(
            clamp_max_function_calls(MAX_FUNCTION_CALLS_CAP),
            MAX_FUNCTION_CALLS_CAP
        );
        assert_eq!(clamp_ask_budget(ASK_BUDGET_MAX), ASK_BUDGET_MAX);
        assert_eq!(
            clamp_search_max_results(SEARCH_MAX_RESULTS_CAP),
            SEARCH_MAX_RESULTS_CAP
        );
    }

    /// 空配置 → 全部默认来源（可编辑区）；只读区恒 hardcoded
    #[test]
    fn effective_params_defaults_and_hardcoded() {
        let views = effective_params(&empty_cfg());
        let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
        assert_eq!(by_key("loop.max_rounds").value, "50");
        assert_eq!(by_key("loop.max_rounds").source, "default");
        assert_eq!(by_key("subagent.max_wall_secs").value, "600");
        assert_eq!(by_key("schedule.scan_secs").source, "hardcoded");
        assert!(!by_key("schedule.scan_secs").editable);
        assert_eq!(views.len(), PARAMS_TABLE.len());
    }

    /// 配置覆盖 → source=config 且值跟随；钳制在 resolve_* 生效
    #[test]
    fn effective_params_reflect_config_and_clamp() {
        let mut cfg = empty_cfg();
        cfg.max_rounds = Some(999);
        cfg.subagent_max_turns = Some(1);
        cfg.search_max_results = Some(99);
        let views = effective_params(&cfg);
        let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
        assert_eq!(
            by_key("loop.max_rounds").value,
            "200",
            "表展示生效值：999 已被 resolve 钳到硬顶 200"
        );
        assert_eq!(by_key("loop.max_rounds").source, "config");
        // resolve_* 才是行为入口：钳 5..=200
        assert_eq!(resolve_max_rounds_cfg(&cfg), 200);
        assert_eq!(resolve_subagent_budget(&cfg).max_turns, 1);
        assert_eq!(
            by_key("search.max_results").value,
            "10",
            "表展示生效值：99 已钳到硬顶 10"
        );
        assert_eq!(resolve_search_max_results(&cfg), 10, "钳到硬顶 10");
        // 非法推理强度：下游静默回退 medium，展示同步
        cfg.reasoning_effort = Some("foobar".into());
        let views = effective_params(&cfg);
        let by_key = |k: &str| views.iter().find(|v| v.key == k).unwrap();
        assert_eq!(
            by_key("model.reasoning_effort").value,
            "medium",
            "非法档位展示回退后的生效值"
        );
    }

    /// resolve 链：子 agent 预算三项生效 + 各自钳制；工具调用全域同源 max_function_calls
    #[test]
    fn resolve_subagent_budget_independent_fields() {
        let mut cfg = empty_cfg();
        cfg.max_function_calls = Some(9999);
        cfg.subagent_max_wall_secs = Some(10);
        let b = resolve_subagent_budget(&cfg);
        assert_eq!(b.max_turns, DEFAULT_SUBAGENT_MAX_TURNS);
        assert_eq!(b.max_tool_calls, 500, "全域同源，钳 1..=500");
        assert_eq!(b.max_wall_seconds, 30, "钳 30..=3600");
    }

    /// resolve 链：未配置 max_function_calls 时子 agent 工具调用落内置默认 100
    #[test]
    fn resolve_subagent_budget_tool_calls_default() {
        let b = resolve_subagent_budget(&empty_cfg());
        assert_eq!(b.max_tool_calls, DEFAULT_SUBAGENT_MAX_TOOL_CALLS);
    }

    /// resolve 链：历史预算钳 20K..=500K
    #[test]
    fn resolve_history_budget_clamps() {
        let mut cfg = empty_cfg();
        cfg.history_budget_chars = Some(100);
        assert_eq!(resolve_history_budget_chars(&cfg), HISTORY_BUDGET_MIN);
        cfg.history_budget_chars = Some(999_999_999);
        assert_eq!(resolve_history_budget_chars(&cfg), HISTORY_BUDGET_MAX);
        cfg.history_budget_chars = None;
        assert_eq!(
            resolve_history_budget_chars(&cfg),
            DEFAULT_HISTORY_BUDGET_CHARS
        );
    }

    /// 思考段回灌开关：只有 0/1 两态，别的值一律夹到 1；未配置 = 关。
    #[test]
    fn reasoning_echo_is_two_state_and_defaults_off() {
        assert_eq!(reasoning_echo_enabled_value(None), 0);
        assert!(!reasoning_echo_on(None));
        assert_eq!(reasoning_echo_enabled_value(Some(0)), 0);
        assert!(!reasoning_echo_on(Some(0)));
        assert_eq!(reasoning_echo_enabled_value(Some(1)), 1);
        assert!(reasoning_echo_on(Some(1)));
        // 越界值夹到上界（当作开），不是夹成关——写侧已钳过，这里是读侧兜底
        assert_eq!(reasoning_echo_enabled_value(Some(9)), 1);
    }

    /// 思考段回灌行的可填范围是 0–1，与钳制边界同源。
    #[test]
    fn reasoning_echo_param_row_range_matches_clamp() {
        let views = effective_params(&empty_cfg());
        let v = views
            .iter()
            .find(|v| v.key == "loop.reasoning_echo_enabled")
            .expect("参数表应有思考段回灌这一条");
        assert_eq!(v.min, Some(0));
        assert_eq!(v.max, Some(REASONING_ECHO_ENABLED_MAX as u64));
        assert!(v.editable);
        assert_eq!(v.default, "0", "内置默认是关");
    }
}
