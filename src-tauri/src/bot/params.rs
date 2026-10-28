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

/// Agent 运行参数的**读取面**：配置本体（`BotConfig`）与对外视图
/// （`BotConfigView`）字段同名，但类型不同。
///
/// 解析器写两份的话，「配置本体按 A 解析、视图按 B 解析」迟早分叉——设置页
/// 显示一个值、运行时走另一个值，且不编译失败也不测试失败。trait 把两者的
/// 字段名收敛到一处：新增参数只改这一处，两个类型都会被编译器盯着补实现。
pub trait AgentParams {
    /// 任务模式档位。
    fn task_mode(&self) -> Option<&str>;
    /// 主会话轮数上限（高级覆盖）。
    fn max_rounds(&self) -> Option<u32>;
    /// 主会话工具调用上限（高级覆盖）。
    fn max_function_calls(&self) -> Option<u32>;
    /// 会话历史字符预算（高级覆盖）。
    fn history_budget_chars(&self) -> Option<u32>;
    /// 历史截断滞回比例（高级覆盖）。
    fn history_truncate_target_pct(&self) -> Option<u32>;
    /// 思考段回灌开关（高级覆盖）。
    fn reasoning_echo_enabled(&self) -> Option<u32>;
    /// 轮内工具结果滑动窗开关（高级覆盖）。
    fn in_turn_context_edit(&self) -> Option<u32>;
    /// 子 agent 轮数预算（高级覆盖）。
    fn subagent_max_turns(&self) -> Option<u32>;
    /// 子 agent 工具调用上限（高级覆盖）。
    fn subagent_max_tool_calls(&self) -> Option<u32>;
    /// 子 agent 墙钟预算（高级覆盖）。
    fn subagent_max_wall_secs(&self) -> Option<u64>;
    /// 联网搜索默认条数（高级覆盖）。
    fn search_max_results(&self) -> Option<u32>;
    /// 工具结果回填上限（高级覆盖）。
    fn max_tool_output_bytes(&self) -> Option<u32>;
    /// 任务提问预算（高级覆盖）。
    fn ask_budget(&self) -> Option<u8>;
    /// 单轮输出 max_tokens（高级覆盖）。
    fn max_tokens(&self) -> Option<u32>;
}

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

// ── 三档任务模式（用户预设）──
//
// 用户侧只选一档，不暴露零散滑条；每档把「回合池 / 工具调用 / 墙钟 / 历史预算 /
// 输出上限 / 回填上限」六项一次性定好。三档的取舍是「任务多重、失控代价多大」：
// 轻量给问答与单文件小改，均衡给日常需求实现，深度给大重构与全仓库阅读。
//
// 档位不是三套独立的常量表——三档共用同一组**全局硬顶**，档位只是硬顶下的
// 预设取值。三张表各写各的，迟早出现「某档超过硬顶」或「两档数值打架」。

/// 任务模式档位。
///
/// 取值三态：`light` / `balanced` / `deep`。未配置 = `balanced`（默认档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskMode {
    /// 轻量模式：小修改、问答、单文件调试。
    Light,
    /// 均衡模式（默认）：日常开发、需求实现。
    Balanced,
    /// 深度模式：大重构、全仓库阅读、长文档分析。
    Deep,
}

impl TaskMode {
    /// 档位 key（落盘用；前端也用这三个字面量）。
    pub fn as_str(self) -> &'static str {
        match self {
            TaskMode::Light => "light",
            TaskMode::Balanced => "balanced",
            TaskMode::Deep => "deep",
        }
    }

    /// 从配置字面量解析；非法值回落默认档（`balanced`）。
    ///
    /// 大小写不敏感：档位名是人手输的字面量（前端下拉写死，但配置文件能手改），
    /// `LIGHT` 与 `light` 指同一档——把大小写当成两档会让手改的配置静默变成
    /// 默认档，用户以为在用轻量档。
    pub fn from_cfg(raw: Option<&str>) -> TaskMode {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("light") => TaskMode::Light,
            Some("deep") => TaskMode::Deep,
            _ => TaskMode::Balanced,
        }
    }
}

/// 单档的预算画像：六项一次性定档。
///
/// 字段顺序与设置页参数卡的分组一致，读的时候能对着看。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModePreset {
    /// 全局总回合（turn）：主会话 + 子 agent + 技能共用同一个池。
    pub global_turns: u32,
    /// 全局总工具调用次数：写文件收尾调用不计入配额。
    pub global_tool_calls: u32,
    /// 全局墙钟时长（秒）：倒计时全任务共享。
    pub global_wall_secs: u64,
    /// 会话历史字符预算。
    pub history_budget_chars: u32,
    /// 单轮输出 max_tokens。
    pub max_tokens: u32,
    /// 单条工具回填 payload 上限（字节）。
    pub tool_output_bytes: u32,
}

/// 轻量模式画像。
pub const LIGHT_PRESET: ModePreset = ModePreset {
    global_turns: 60,
    global_tool_calls: 150,
    global_wall_secs: 300,
    history_budget_chars: 80_000,
    max_tokens: 8_192,
    tool_output_bytes: 64 * 1024,
};

/// 均衡模式画像（默认档）。
pub const BALANCED_PRESET: ModePreset = ModePreset {
    global_turns: 100,
    global_tool_calls: 250,
    global_wall_secs: 900,
    history_budget_chars: 100_000,
    max_tokens: 16_384,
    tool_output_bytes: 128 * 1024,
};

/// 深度模式画像。
pub const DEEP_PRESET: ModePreset = ModePreset {
    global_turns: 250,
    global_tool_calls: 700,
    global_wall_secs: 2_400,
    history_budget_chars: 300_000,
    max_tokens: 32_768,
    tool_output_bytes: 200 * 1024,
};

/// 取指定档位的画像。
pub fn preset_for(mode: TaskMode) -> ModePreset {
    match mode {
        TaskMode::Light => LIGHT_PRESET,
        TaskMode::Balanced => BALANCED_PRESET,
        TaskMode::Deep => DEEP_PRESET,
    }
}

// ── 全局硬顶（三档共用）──
//
// 硬顶是「用户能调到的最大值」，档位预设都在硬顶之下。分档的用处是给日常
// 选一个安全值；硬顶的用处是让「高级覆盖」有明确的天花板。

/// 全局总回合硬顶。
pub const GLOBAL_TURNS_CAP: u32 = 500;
/// 全局总工具调用次数硬顶。
pub const GLOBAL_TOOL_CALLS_CAP: u32 = 1_200;
/// 全局墙钟硬顶（秒）。
pub const GLOBAL_WALL_SECS_CAP: u64 = 3_600;
/// 会话历史字符预算硬顶。
///
/// 不设 500K 档：历史预算与上下文窗口耦合，500K 字符已超多数模型的可用
/// 输入量，高风险档位不开放。
pub const HISTORY_BUDGET_MAX: u32 = 300_000;
/// 单轮输出 max_tokens 硬顶。
pub const MAX_TOKENS_CAP: u32 = 32_768;
/// 单条工具回填 payload 硬顶（字节）。
pub const GLOBAL_TOOL_OUTPUT_BYTES_CAP: u32 = 200 * 1024;

// ── 预算分配比例（内置，不可配）──
//
// 同一个全局池按比例切给三个消费者：主会话、子 agent、技能。三者**各自
// 独立计数**（不共用一个递减计数器）——业界口径是「子 agent 跑在各自隔离的
// context / 独立预算里」：Claude Code 的子 agent 用隔离上下文正是为了不撑爆
// 父窗口；AutoGen 系的父子约束是 `child_budget <= parent_available_budget`
// 这类**上限**约束，不是共用一个计数器。共享递减池会让「主会话多跑两轮 →
// 子 agent 少两轮额度」这种耦合传进每一次派发，调试时看不出是谁吃掉了谁。
//
// 比例含义：主会话 60%，单个子 agent 最多取用剩余 20%，单个技能最多取用
// 剩余 15%。三者相加 95%，留 5% 余量。

/// 主会话占用总预算的比例（百分比）。
pub const MAIN_SESSION_BUDGET_PCT: usize = 60;
/// 单个子 agent 最多取用剩余预算的比例（百分比）。
pub const SUBAGENT_BUDGET_PCT: usize = 20;
/// 单个技能最多取用剩余预算的比例（百分比）。
pub const SKILL_BUDGET_PCT: usize = 15;

/// 按比例取预算：`(总量 × 百分比) / 100`，向下取整，至少 1。
///
/// 至少 1 而不是允许 0：比例为 0 的话这个消费者等于被禁用，但「禁用」
/// 不是本处的语义（要禁用有专门的开关），0 预算会让除法下游拿到 0 并炸掉。
pub const fn budget_share(total: u32, pct: usize) -> u32 {
    let raw = (total.saturating_mul(pct as u32)) / 100;
    if raw == 0 {
        1
    } else {
        raw
    }
}

/// 墙钟按比例取预算（秒）：`(总量 × 百分比) / 100`，至少 1 秒。
pub const fn wall_share(total: u64, pct: usize) -> u64 {
    let raw = (total.saturating_mul(pct as u64)) / 100;
    if raw == 0 {
        1
    } else {
        raw
    }
}

// ── 内置默认（单一事实源）──

/// 模型循环默认轮数：均衡档全局池的 60%（分配规则给主会话的那一份）。
pub const DEFAULT_MAX_ROUNDS: u32 =
    budget_share(BALANCED_PRESET.global_turns, MAIN_SESSION_BUDGET_PCT);
/// 轮数硬顶：全局回合池硬顶。
pub const MAX_ROUNDS_CAP: u32 = GLOBAL_TURNS_CAP;
/// 轮数下界（少于 5 轮的循环连一次工具往返都跑不完）——钳制与设置页展示的
/// 可填范围共用这一个数。
pub const MAX_ROUNDS_MIN: u32 = 5;
/// 会话历史字符预算默认：均衡档画像。
pub const DEFAULT_HISTORY_BUDGET_CHARS: u32 = BALANCED_PRESET.history_budget_chars;
pub const HISTORY_BUDGET_MIN: u32 = 20_000;
/// 历史截断滞回目标比例默认（百分比）：超预算时一次裁到预算的 70%。
/// 贴边裁会让长会话几乎每轮都改一次缓存前缀，留 30% 余量能稳住好几轮。
pub const DEFAULT_HISTORY_TRUNCATE_TARGET_PCT: u32 = 70;
/// 滞回比例可取区间：钳制与设置页展示的可填范围共用，单源在 constants。
pub const HISTORY_TRUNCATE_TARGET_PCT_MIN: u32 =
    crate::bot::constants::HISTORY_TRUNCATE_TARGET_PCT_MIN;
pub const HISTORY_TRUNCATE_TARGET_PCT_MAX: u32 =
    crate::bot::constants::HISTORY_TRUNCATE_TARGET_PCT_MAX;
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
/// 子 agent 默认预算（行上预算缺省时的兜底；db::subagents 的 DEFAULT_* 引用此处）。
///
/// 轮数与工具数取均衡档全局池的 20%（分配规则给单个子 agent 的那一份），
/// 墙钟同理；不是拍脑袋的独立默认值。工具数与主会话的 max_function_calls
/// 是两个独立键（见 resolve_subagent_budget）。
pub const DEFAULT_SUBAGENT_MAX_TURNS: u32 =
    budget_share(BALANCED_PRESET.global_turns, SUBAGENT_BUDGET_PCT);
pub const DEFAULT_SUBAGENT_MAX_TOOL_CALLS: u32 =
    budget_share(BALANCED_PRESET.global_tool_calls, SUBAGENT_BUDGET_PCT);
pub const DEFAULT_SUBAGENT_MAX_WALL_SECS: u64 =
    wall_share(BALANCED_PRESET.global_wall_secs, SUBAGENT_BUDGET_PCT);
/// 子 agent 墙钟下界（秒）——钳制与设置页展示的可填范围共用这一个数。
pub const SUBAGENT_MAX_WALL_SECS_MIN: u64 = 30;
/// 子 agent 墙钟上界（秒）：全局墙钟硬顶。
pub const SUBAGENT_MAX_WALL_SECS_MAX: u64 = GLOBAL_WALL_SECS_CAP;
/// 联网搜索默认结果条数（Tavily 主流推荐：6 条）。
pub const DEFAULT_SEARCH_MAX_RESULTS: u32 = 6;

// 钳制单源：写侧落盘（bot_set_config）与读侧生效（下方 resolve_*）共用同一组
// 边界——字面量只落在这里，防两边各自改数后静默漂移。子 agent 轮数的边界是
// subagents 域不变量，单源在 db::subagents::clamp_subagent_max_turns。

/// 模型循环轮数钳 5..=全局回合硬顶
pub fn clamp_max_rounds(n: u32) -> u32 {
    n.clamp(MAX_ROUNDS_MIN, MAX_ROUNDS_CAP)
}

/// 会话历史字符预算钳 20K..=300K
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
pub const SEARCH_MAX_RESULTS_CAP: u32 = crate::bot::constants::SEARCH_MAX_RESULTS_CAP;
/// 联网搜索结果条数下界（0 条搜索无意义）
pub const SEARCH_MAX_RESULTS_MIN: u32 = crate::bot::constants::SEARCH_MAX_RESULTS_MIN;

/// 联网搜索结果条数钳 1..=SEARCH_MAX_RESULTS_CAP
pub fn clamp_search_max_results(n: u32) -> u32 {
    n.clamp(SEARCH_MAX_RESULTS_MIN, SEARCH_MAX_RESULTS_CAP)
}

/// 单次请求工具调用熔断上限硬顶：写侧（子 agent 预算解析）与读侧（模型循环
/// 派生链末端的 `.clamp`）共用这一个数，防「预算按 500 记账、循环按 999 放行」。
pub const MAX_FUNCTION_CALLS_CAP: u32 = GLOBAL_TOOL_CALLS_CAP;
/// 工具调用熔断下界（0 次调用无意义）
pub const MAX_FUNCTION_CALLS_MIN: u32 = 1;

pub fn clamp_max_function_calls(n: u32) -> u32 {
    n.clamp(MAX_FUNCTION_CALLS_MIN, MAX_FUNCTION_CALLS_CAP)
}

/// verifier 拒绝后的修复重试预算：未配置时的默认次数。
pub const DEFAULT_VERIFIER_RETRY_MAX: u32 = 1;
/// verifier 重试预算上限（超过 2 次还失败的任务属硬阻塞，如实汇报比继续烧
/// token 更省；与配置落盘前的写侧钳制共用）。
pub const VERIFIER_RETRY_MAX_CAP: u32 = crate::bot::constants::VERIFIER_RETRY_MAX_CAP;

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

/// 单条工具结果回填上限钳（上限 200KB 字节，UTF-8）。
///
/// 这个上限原先只落在模型循环的生效点（`.min(200_000)`），而读侧 resolver 原样
/// 把配置值透给设置页——用户设 500000，设置页显示 500000，运行时按 200000 走，
/// 两边都不知道发生了什么。写侧钳制 + 读侧 + 生效点三处共用这一个边界。
pub const MAX_TOOL_OUTPUT_BYTES_CAP: u32 = GLOBAL_TOOL_OUTPUT_BYTES_CAP;

pub fn clamp_max_tool_output_bytes(n: u32) -> u32 {
    n.min(MAX_TOOL_OUTPUT_BYTES_CAP)
}

/// 任务执行提问预算（ask_user 每次执行可问次数）钳 1..=5
pub const ASK_BUDGET_MIN: u8 = crate::bot::constants::ASK_BUDGET_MIN;
pub const ASK_BUDGET_MAX: u8 = crate::bot::constants::ASK_BUDGET_MAX;

/// 任务执行提问预算（ask_user 每次执行可问次数）钳 1..=5
pub fn clamp_ask_budget(n: u8) -> u8 {
    n.clamp(ASK_BUDGET_MIN, ASK_BUDGET_MAX)
}

// 解析辅助（读取口唯一）
//
// 两段式：**档位预设**给默认，**高级覆盖**给单项手填。旧配置里已有的
// maxRounds / maxFunctionCalls 等字段即高级覆盖位——老用户改过的值不会被
// 档位悄悄顶掉，新用户不填任何东西时按档位走。两段都要过同一组 clamp，
// 「显示的值 = 生效的值」这条不变量不许破。

/// 当前生效的任务档位。
pub fn resolve_task_mode(cfg: &impl AgentParams) -> TaskMode {
    TaskMode::from_cfg(cfg.task_mode())
}

/// 当前档位的预算画像（未经单项覆盖）。
pub fn resolve_preset(cfg: &impl AgentParams) -> ModePreset {
    preset_for(resolve_task_mode(cfg))
}

/// 全局回合池（档位预设值；单项链制到全局硬顶）。
pub fn resolve_global_turns(cfg: &impl AgentParams) -> u32 {
    resolve_preset(cfg).global_turns.min(GLOBAL_TURNS_CAP)
}

/// 全局工具调用池（档位预设值）。
pub fn resolve_global_tool_calls(cfg: &impl AgentParams) -> u32 {
    resolve_preset(cfg)
        .global_tool_calls
        .min(GLOBAL_TOOL_CALLS_CAP)
}

/// 全局墙钟（秒，档位预设值）。
pub fn resolve_global_wall_secs(cfg: &impl AgentParams) -> u64 {
    resolve_preset(cfg)
        .global_wall_secs
        .min(GLOBAL_WALL_SECS_CAP)
}

/// 主会话回合上限：档位全局池的 60%，可被高级覆盖 `max_rounds` 顶掉。
///
/// 覆盖优先于分配比例——用户手填的值是他明确要的，档位只是默认值。
pub fn resolve_max_rounds_cfg(cfg: &impl AgentParams) -> u32 {
    match cfg.max_rounds() {
        Some(n) => clamp_max_rounds(n),
        None => clamp_max_rounds(budget_share(
            resolve_global_turns(cfg),
            MAIN_SESSION_BUDGET_PCT,
        )),
    }
}

/// 主会话工具调用上限：档位全局池的 60%，可被高级覆盖 `max_function_calls` 顶掉。
pub fn resolve_main_tool_calls(cfg: &impl AgentParams) -> u32 {
    match cfg.max_function_calls() {
        Some(n) => clamp_max_function_calls(n),
        None => clamp_max_function_calls(budget_share(
            resolve_global_tool_calls(cfg),
            MAIN_SESSION_BUDGET_PCT,
        )),
    }
}

/// 主会话墙钟预算（秒）：档位全局墙钟的 60%，可被高级覆盖顶掉。
///
/// 之前主会话**没有墙钟**（只有子 agent 有），长任务能被无限拖；档位给了
/// 全局时间预算后，主会话这一路也必须有一份，否则「全局墙钟」名不副实。
pub fn resolve_main_wall_secs(cfg: &impl AgentParams) -> u64 {
    let shared = resolve_global_wall_secs(cfg) * MAIN_SESSION_BUDGET_PCT as u64 / 100;
    clamp_subagent_max_wall_secs(shared.max(SUBAGENT_MAX_WALL_SECS_MIN))
}

/// 会话历史字符预算：档位预设值，可被高级覆盖顶掉。
pub fn resolve_history_budget_chars(cfg: &impl AgentParams) -> u32 {
    match cfg.history_budget_chars() {
        Some(n) => clamp_history_budget_chars(n),
        None => clamp_history_budget_chars(resolve_preset(cfg).history_budget_chars),
    }
}

/// 历史截断滞回比例（百分比，50..=90）：参数表展示与写盘都用这个整数口径。
pub fn resolve_history_truncate_target_pct(cfg: &impl AgentParams) -> u32 {
    clamp_history_truncate_target_pct(
        cfg.history_truncate_target_pct()
            .unwrap_or(DEFAULT_HISTORY_TRUNCATE_TARGET_PCT),
    )
}

/// 历史截断滞回比例（0.5..=0.9）：超预算时一次裁到预算的这个比例。
/// 比例越低裁得越狠（前缀越稳、丢的历史越多）。
pub fn resolve_history_truncate_target_ratio(cfg: &impl AgentParams) -> f64 {
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
pub fn resolve_reasoning_echo_enabled(cfg: &impl AgentParams) -> u32 {
    reasoning_echo_enabled_value(cfg.reasoning_echo_enabled())
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
pub fn resolve_in_turn_context_edit(cfg: &impl AgentParams) -> u32 {
    in_turn_context_edit_value(cfg.in_turn_context_edit())
}

/// 子 agent 预算：档位全局池的 20%（分配规则给单个子 agent 的那一份），
/// 三项各自可被高级覆盖顶掉。
///
/// 工具调用这一项**不与主会话同源**：主会话拿 60%、子 agent 拿 20%，是同一
/// 个全局池的两份配额。此前两条共用一个 `max_function_calls`，等于子 agent
/// 拿到了主会话同等的工具额度，分配规则无从谈起。
pub fn resolve_subagent_budget(cfg: &impl AgentParams) -> crate::db::SubagentBudget {
    crate::db::SubagentBudget {
        max_turns: match cfg.subagent_max_turns() {
            Some(n) => crate::db::clamp_subagent_max_turns(n),
            None => crate::db::clamp_subagent_max_turns(budget_share(
                resolve_global_turns(cfg),
                SUBAGENT_BUDGET_PCT,
            )),
        },
        max_tool_calls: match cfg.subagent_max_tool_calls() {
            Some(n) => clamp_max_function_calls(n),
            None => clamp_max_function_calls(budget_share(
                resolve_global_tool_calls(cfg),
                SUBAGENT_BUDGET_PCT,
            )),
        },
        max_wall_seconds: match cfg.subagent_max_wall_secs() {
            Some(n) => clamp_subagent_max_wall_secs(n),
            None => clamp_subagent_max_wall_secs(wall_share(
                resolve_global_wall_secs(cfg),
                SUBAGENT_BUDGET_PCT,
            )),
        },
    }
}

pub fn resolve_search_max_results(cfg: &impl AgentParams) -> u32 {
    clamp_search_max_results(
        cfg.search_max_results()
            .unwrap_or(DEFAULT_SEARCH_MAX_RESULTS),
    )
}

/// 任务执行提问预算（原 workflow_settings.ask_budget，随工作流退役迁入 bot 参数）
pub fn resolve_ask_budget_cfg(cfg: &impl AgentParams) -> u8 {
    clamp_ask_budget(cfg.ask_budget().unwrap_or(3))
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
    // ── 可编辑：任务档位 ──
    ParamDef {
        key: "mode.task_mode",
        label: "任务模式",
        category: "mode",
        hint: "一次性定下全局回合池 / 工具调用 / 墙钟 / 历史预算 / 输出上限 / 回填上限六项；轻量=问答与小改，均衡=日常开发，深度=大重构",
        min: None,
        max: None,
        editable: true,
        resolve: cfg_resolver!(resolve_task_mode_label, task_mode),
    },
    // ── 可编辑：循环/历史（档位预设 + 高级覆盖）──
    ParamDef {
        key: "loop.max_rounds",
        label: "主会话轮数",
        category: "loop",
        hint: "主会话单次对话/任务执行的 LLM 轮数上限；默认取档位全局池的 60%；调大=更复杂任务但失控时更费 token。Skill 自报轮数优先于此值",
        min: Some(MAX_ROUNDS_MIN as u64),
        max: Some(MAX_ROUNDS_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_max_rounds_cfg, max_rounds),
    },
    ParamDef {
        key: "loop.max_function_calls",
        label: "主会话工具调用上限",
        category: "loop",
        hint: "主会话的工具调用熔断上限；默认取档位全局池的 60%；软警告在 90% 处触发，95% 进收尾窗，100% 硬熔断",
        min: Some(MAX_FUNCTION_CALLS_MIN as u64),
        max: Some(MAX_FUNCTION_CALLS_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_main_tool_calls, max_function_calls),
    },
    ParamDef {
        key: "loop.history_budget_chars",
        label: "会话历史字符预算",
        category: "loop",
        hint: "超出后最旧消息先丢并生成摘要；默认取档位预设；调大=长对话记忆更好但上下文成本更高",
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
        hint: "单个子 agent 的轮数上限；默认取档位全局池的 20%；任何配置（含 LLM 自报）都不能越过硬顶",
        min: Some(crate::db::MIN_SUBAGENT_TURNS as u64),
        max: Some(crate::db::MAX_TURNS_HARD_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_budget_turns, subagent_max_turns),
    },
    ParamDef {
        key: "subagent.max_tool_calls",
        label: "子 agent 工具调用上限",
        category: "subagent",
        hint: "单个子 agent 的工具调用上限；默认取档位全局工具池的 20%；与主会话是同一池的两份配额（主 60% / 子 20%）",
        min: Some(MAX_FUNCTION_CALLS_MIN as u64),
        max: Some(MAX_FUNCTION_CALLS_CAP as u64),
        editable: true,
        resolve: cfg_resolver!(resolve_budget_tool_calls, subagent_max_tool_calls),
    },
    ParamDef {
        key: "subagent.max_wall_secs",
        label: "子 agent 墙钟预算（秒）",
        category: "subagent",
        hint: "单个子 agent 从起跑计的最长时长；默认取档位全局墙钟的 20%；排队不计入",
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
        hint: "仅 Anthropic 协议发送；OpenAI 兼容模式不发送。默认取档位预设；条目级设置优先于此全局值",
        min: Some(crate::bot::config::schema::MIN_MAX_TOKENS as u64),
        max: Some(MAX_TOKENS_CAP as u64),
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
        resolve: hardcoded_resolver!(crate::bot::constants::TOOL_WINDOW_TRIGGER_CHARS),
    },
    ParamDef {
        key: "loop.tool_window_keep_recent",
        label: "工具结果滑动窗保留条数",
        category: "loop",
        hint: "最近这几条工具结果永不裁；少一条模型就可能忘了上一轮做过什么（重跑工具 = 重跑副作用）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot::constants::TOOL_WINDOW_KEEP_RECENT),
    },
    ParamDef {
        key: "subagent.max_running",
        label: "子 agent 全局并发",
        category: "subagent",
        hint: "全局同时运行的子 agent 上限；排队在 SubagentGate（单会话上限见「每会话并发」）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot::constants::SUBAGENT_MAX_GLOBAL_RUNNING),
    },
    ParamDef {
        key: "subagent.per_session_running",
        label: "子 agent 每会话并发",
        category: "subagent",
        hint: "同一个主会话同时运行的子 agent 上限；与全局上限取更紧者（全局见「全局并发」）",
        min: None,
        max: None,
        editable: false,
        resolve: hardcoded_resolver!(crate::bot::constants::SUBAGENT_MAX_PER_SESSION_RUNNING),
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

/// 生效值解析：可编辑项查 cfg 字段（None → 档位预设 / Some → 值），只读项出硬编码常量。
/// 子 agent 预算三个键共用同一份 `resolve_subagent_budget`，这里按需取字段。
fn resolve_task_mode_label(cfg: &impl AgentParams) -> String {
    resolve_task_mode(cfg).as_str().to_string()
}

fn resolve_budget_tool_calls(cfg: &impl AgentParams) -> String {
    resolve_subagent_budget(cfg).max_tool_calls.to_string()
}

fn resolve_budget_turns(cfg: &impl AgentParams) -> String {
    resolve_subagent_budget(cfg).max_turns.to_string()
}

fn resolve_budget_wall(cfg: &impl AgentParams) -> String {
    resolve_subagent_budget(cfg).max_wall_seconds.to_string()
}

fn plain_max_tokens(cfg: &impl AgentParams) -> String {
    // 档位预设（轻量 8K / 均衡 16K / 深度 32K），高级覆盖走 config.max_tokens；
    // 条目级覆盖仍优先于此全局值（见 bot_model_loop 的请求装配）。
    resolve_max_tokens(cfg).to_string()
}

/// 全局 max_tokens 生效值：档位预设 → 高级覆盖 → 钳到硬顶。
///
/// 硬顶 32768：单轮输出再大也不该超过这个量级——超长的单轮输出意味着模型在
/// 一次回复里塞了太多东西，既烧钱又挤占后续轮次的输入预算。
pub fn resolve_max_tokens(cfg: &impl AgentParams) -> u32 {
    let preset = resolve_preset(cfg).max_tokens.min(MAX_TOKENS_CAP);
    match cfg.max_tokens() {
        Some(n) => n.clamp(crate::bot::config::schema::MIN_MAX_TOKENS, MAX_TOKENS_CAP),
        None => preset,
    }
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

/// 工具结果回填上限：档位预设（64KB / 128KB / 200KB），高级覆盖走
/// `max_tool_output_bytes`。
///
/// 有第三态：Some(0) = 显式关闭截断，展示串不是数字。
///
/// 展示值必须等于生效值：这里显示的就是模型循环实际会用的那个数（同一个
/// `clamp_max_tool_output_bytes`），配置手改越过上限时设置页也能看出被钳到哪。
fn resolve_max_output_bytes(cfg: &impl AgentParams) -> (String, &'static str) {
    match cfg.max_tool_output_bytes() {
        Some(v) if v > 0 => (clamp_max_tool_output_bytes(v).to_string(), "config"),
        Some(_) => ("不截断（显式关闭）".into(), "config"),
        None => (
            resolve_preset(cfg)
                .tool_output_bytes
                .min(MAX_TOOL_OUTPUT_BYTES_CAP)
                .to_string(),
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
mod tests;
