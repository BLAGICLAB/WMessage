//! Agent 运行参数注册表（P3-a，Agent 透明化设计 §3）：
//!
//! **单一真相源查表**（仿 bot/registry.rs TOOLS_TABLE 哲学）——每个影响 agent
//! 行为的参数一条 `ParamDef`，`bot_effective_params` 命令遍历出表，设置页参数卡
//! 渲染生效值/默认值/来源。以后加参数 = 表里加一条，展示自动跟随。
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

// ───────────────────────── 内置默认（单一事实源） ─────────────────────────

/// 模型循环默认轮数（原 bot_model_loop::DEFAULT_MAX_ROUNDS 同值；该处引用本常量）
pub const DEFAULT_MAX_ROUNDS: u32 = 50;
pub const MAX_ROUNDS_CAP: u32 = 200;
/// 会话历史字符预算默认（原 bot_chat::HISTORY_BUDGET_CHARS 同值）
pub const DEFAULT_HISTORY_BUDGET_CHARS: u32 = 100_000;
pub const HISTORY_BUDGET_MIN: u32 = 20_000;
pub const HISTORY_BUDGET_MAX: u32 = 500_000;
/// 子 agent 默认预算（原 db::subagents DEFAULT_* 同值）：
/// max_tool_calls 仅是缺省兜底——配置了 max_function_calls 时子 agent 同源全域上限
pub const DEFAULT_SUBAGENT_MAX_TURNS: u32 = 30;
pub const DEFAULT_SUBAGENT_MAX_TOOL_CALLS: u32 = 100;
pub const DEFAULT_SUBAGENT_MAX_WALL_SECS: u64 = 600;
/// web_search 默认结果条数（原 bot_web SEARCH_MAX_RESULTS 同值）
pub const DEFAULT_SEARCH_MAX_RESULTS: u32 = 8;

// ───────────────────────── 解析辅助（读取口唯一） ─────────────────────────

pub fn resolve_max_rounds_cfg(cfg: &BotConfig) -> u32 {
    cfg.max_rounds
        .unwrap_or(DEFAULT_MAX_ROUNDS)
        .clamp(5, MAX_ROUNDS_CAP)
}

pub fn resolve_history_budget_chars(cfg: &BotConfig) -> u32 {
    cfg.history_budget_chars
        .unwrap_or(DEFAULT_HISTORY_BUDGET_CHARS)
        .clamp(HISTORY_BUDGET_MIN, HISTORY_BUDGET_MAX)
}

pub fn resolve_subagent_budget(cfg: &BotConfig) -> crate::db::SubagentBudget {
    crate::db::SubagentBudget {
        max_turns: cfg
            .subagent_max_turns
            .unwrap_or(DEFAULT_SUBAGENT_MAX_TURNS)
            .clamp(1, crate::db::MAX_TURNS_HARD_CAP),
        // 工具调用上限全域统一（老板拍板合并两条设置）：子 agent 与主循环同源
        // max_function_calls（缺省 100，钳 1..=500）；LLM 显式给的预算仍可覆盖
        max_tool_calls: cfg
            .max_function_calls
            .unwrap_or(DEFAULT_SUBAGENT_MAX_TOOL_CALLS)
            .clamp(1, 500),
        max_wall_seconds: cfg
            .subagent_max_wall_secs
            .unwrap_or(DEFAULT_SUBAGENT_MAX_WALL_SECS)
            .clamp(30, 3600),
    }
}

pub fn resolve_search_max_results(cfg: &BotConfig) -> u32 {
    cfg.search_max_results
        .unwrap_or(DEFAULT_SEARCH_MAX_RESULTS)
        .clamp(1, 10)
}

use crate::bot::config::types::BotConfig;

// ───────────────────────── 参数表 ─────────────────────────

/// 参数定义（编译期常量表；value 的解析在 `bot_effective_params` 内查 cfg 字段）
pub struct ParamDef {
    /// 稳定 key（前端 React key + 未来深链定位），点分分层
    pub key: &'static str,
    /// 中文展示名
    pub label: &'static str,
    /// 分组（设置页参数卡按此分节）：loop / subagent / search / model / tools / schedule
    pub category: &'static str,
    /// 一句话说明（含调大的代价）
    pub hint: &'static str,
    /// 默认值展示串
    pub default: &'static str,
    /// true = bot-config.json 有对应字段可改；false = 硬编码只读
    pub editable: bool,
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
    pub default: &'static str,
    /// config = 配置覆盖生效 / default = 用内置默认 / hardcoded = 硬编码只读
    pub source: &'static str,
    pub editable: bool,
}

/// 全量参数表：可编辑区在前，硬编码只读区在后（前端按 category 分组渲染）
pub static PARAMS_TABLE: &[ParamDef] = &[
    // ── 可编辑：循环/历史 ──
    ParamDef {
        key: "loop.max_rounds",
        label: "模型循环最大轮数",
        category: "loop",
        hint: "单次对话/任务执行的 LLM 轮数上限；调大=更复杂任务但失控时更费 token。Skill 自报轮数优先于此值",
        default: "50",
        editable: true,
    },
    ParamDef {
        key: "loop.max_function_calls",
        label: "单次请求工具调用熔断",
        category: "loop",
        hint: "全域工具调用熔断上限（子 agent 预算同源此值）；工作流节点等长链任务可调大；软警告在 70% 处自动触发",
        default: "100",
        editable: true,
    },
    ParamDef {
        key: "loop.history_budget_chars",
        label: "会话历史字符预算",
        category: "loop",
        hint: "超出后最旧消息先丢并生成摘要；调大=长对话记忆更好但上下文成本更高",
        default: "100000",
        editable: true,
    },
    // ── 可编辑：子 agent ──
    ParamDef {
        key: "subagent.max_turns",
        label: "子 agent 轮数预算",
        category: "subagent",
        hint: "LLM 派发子 agent 未显式给预算时的默认轮数；硬顶 50",
        default: "30",
        editable: true,
    },
    ParamDef {
        key: "subagent.max_wall_secs",
        label: "子 agent 墙钟预算（秒）",
        category: "subagent",
        hint: "子 agent 从起跑计的最长时长；排队不计入",
        default: "600",
        editable: true,
    },
    // ── 可编辑：搜索 ──
    ParamDef {
        key: "search.max_results",
        label: "联网搜索默认条数",
        category: "search",
        hint: "web_search 未指定 count 时的结果条数；后端硬顶 10",
        default: "8",
        editable: true,
    },
    // ── 可编辑：模型（已有字段入表展示） ──
    ParamDef {
        key: "model.max_tokens",
        label: "全局 max_tokens",
        category: "model",
        hint: "仅 Anthropic 协议发送；OpenAI 兼容模式不发送。条目级设置优先于此全局值",
        default: "8192",
        editable: true,
    },
    ParamDef {
        key: "model.reasoning_effort",
        label: "推理强度默认档",
        category: "model",
        hint: "off/low/medium/high；按模型族映射到线上参数（聊天输入栏 ⚡ 可按会话覆盖）",
        default: "medium",
        editable: true,
    },
    ParamDef {
        key: "tools.python_timeout_secs",
        label: "Python 执行超时（秒）",
        category: "tools",
        hint: "run_python 默认超时；工具参数可单次覆盖，硬顶 300",
        default: "60",
        editable: true,
    },
    // ── 可编辑：工具输出截断（P4 落行为，本期仅入表） ──
    ParamDef {
        key: "tools.max_output_chars",
        label: "工具结果截断字符数",
        category: "tools",
        hint: "单条工具结果回填模型上下文的截断上限；默认不截断（超大输出可能挤占上下文，P4 接线生效）",
        default: "不截断",
        editable: true,
    },
    // ── 硬编码只读区（调坏易出问题，仅展示） ──
    ParamDef {
        key: "subagent.max_running",
        label: "子 agent 全局并发",
        category: "subagent",
        hint: "同时运行的子 agent 上限（每会话 2）；排队在 SubagentGate",
        default: "3",
        editable: false,
    },
    ParamDef {
        key: "subagent.turns_hard_cap",
        label: "子 agent 轮数硬顶",
        category: "subagent",
        hint: "任何配置（含 LLM 自报）都不能超过的轮数硬顶",
        default: "50",
        editable: false,
    },
    ParamDef {
        key: "search.count_cap",
        label: "搜索条数硬顶",
        category: "search",
        hint: "工具参数 count 的硬上限（防单次搜索拉爆上下文）",
        default: "10",
        editable: false,
    },
    ParamDef {
        key: "search.output_chars",
        label: "搜索输出字符上限",
        category: "search",
        hint: "搜索结果合并渲染后的截断长度",
        default: "6000",
        editable: false,
    },
    ParamDef {
        key: "llm.stream_idle_secs",
        label: "流式空闲超时（秒）",
        category: "model",
        hint: "两个流式 chunk 间隔超过此值判定连接死亡；总时长不设限（长生成合法）",
        default: "120",
        editable: false,
    },
    ParamDef {
        key: "llm.retry",
        label: "LLM 请求重试",
        category: "model",
        hint: "429/5xx 自动重试次数与间隔",
        default: "2次/1.5s",
        editable: false,
    },
    ParamDef {
        key: "schedule.scan_secs",
        label: "定时扫描间隔（秒）",
        category: "schedule",
        hint: "定时任务/工作流调度的扫描周期（执行最早延迟该时长）",
        default: "30",
        editable: false,
    },
    ParamDef {
        key: "trace.span_text_max",
        label: "痕迹单条文本上限（字节）",
        category: "tools",
        hint: "执行痕迹里单条工具入参/结果的落库钳制（超出留审计）",
        default: "16384",
        editable: false,
    },
    ParamDef {
        key: "trace.diff_max_lines",
        label: "文件 diff 最大行数",
        category: "tools",
        hint: "执行痕迹里 unified diff 的行数钳制（±行计数不随截断丢失）",
        default: "2000",
        editable: false,
    },
    ParamDef {
        key: "trace.retention_days",
        label: "执行痕迹保留期（天）",
        category: "tools",
        hint: "痕迹三表与回滚快照的保留期；数据管理可手动清理",
        default: "30",
        editable: false,
    },
];

/// 生效值解析：可编辑项查 cfg 字段（None → 默认 / Some → 值），只读项出硬编码常量
fn param_value(key: &str, cfg: &BotConfig) -> (String, &'static str) {
    // 可编辑区一律过 resolve_* 取生效值：面板展示的就是运行时真值，避免
    // 「看到 999、实际钳到 200」的脱节；来源仍按「是否配置了该字段」标注
    match key {
        "loop.max_rounds" => (
            resolve_max_rounds_cfg(cfg).to_string(),
            if cfg.max_rounds.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "loop.max_function_calls" => (
            resolve_subagent_budget(cfg).max_tool_calls.to_string(),
            if cfg.max_function_calls.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "loop.history_budget_chars" => (
            resolve_history_budget_chars(cfg).to_string(),
            if cfg.history_budget_chars.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "subagent.max_turns" => (
            resolve_subagent_budget(cfg).max_turns.to_string(),
            if cfg.subagent_max_turns.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "subagent.max_wall_secs" => (
            resolve_subagent_budget(cfg).max_wall_seconds.to_string(),
            if cfg.subagent_max_wall_secs.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "search.max_results" => (
            resolve_search_max_results(cfg).to_string(),
            if cfg.search_max_results.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "model.max_tokens" => match cfg.max_tokens {
            Some(v) => (v.to_string(), "config"),
            None => ("8192".into(), "default"),
        },
        // 推理强度无钳制但有静默回退：非法值下游一律按 medium 生效，展示同步取回退结果
        "model.reasoning_effort" => (
            crate::bot::reasoning::EffortLevel::from_cfg(cfg.reasoning_effort.as_deref())
                .as_str()
                .to_string(),
            if cfg.reasoning_effort.is_some() {
                "config"
            } else {
                "default"
            },
        ),
        "tools.python_timeout_secs" => match cfg.python_timeout_secs {
            Some(v) => (v.to_string(), "config"),
            None => ("60".into(), "default"),
        },
        "tools.max_output_chars" => match cfg.max_tool_output_chars {
            Some(v) if v > 0 => (v.to_string(), "config"),
            _ => ("不截断".into(), "default"),
        },
        // 只读硬编码区
        "subagent.max_running" => ("3".into(), "hardcoded"),
        "subagent.turns_hard_cap" => ("50".into(), "hardcoded"),
        "search.count_cap" => ("10".into(), "hardcoded"),
        "search.output_chars" => ("6000".into(), "hardcoded"),
        "llm.stream_idle_secs" => ("120".into(), "hardcoded"),
        "llm.retry" => ("2次/1.5s".into(), "hardcoded"),
        "schedule.scan_secs" => ("30".into(), "hardcoded"),
        "trace.span_text_max" => ("16384".into(), "hardcoded"),
        "trace.diff_max_lines" => ("2000".into(), "hardcoded"),
        "trace.retention_days" => ("30".into(), "hardcoded"),
        other => (format!("?{other}"), "hardcoded"),
    }
}

/// 遍历参数表产出生效值视图（命令体直接映射）
pub fn effective_params(cfg: &BotConfig) -> Vec<ParamView> {
    PARAMS_TABLE
        .iter()
        .map(|d| {
            let (value, source) = param_value(d.key, cfg);
            ParamView {
                key: d.key,
                label: d.label,
                category: d.category,
                hint: d.hint,
                value,
                default: d.default,
                source,
                editable: d.editable,
            }
        })
        .collect()
}

#[tauri::command]
pub fn bot_effective_params(app: tauri::AppHandle) -> CommandResult<Vec<ParamView>> {
    let cfg = crate::bot::config::io::load_config(&app);
    Ok(effective_params(&cfg))
}

use crate::error::CommandResult;

#[cfg(test)]
mod tests {
    use super::*;

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
            archive_after_days: None,
            models_by_provider: None,
            active_model_id: None,
            ui_font_size: None,
            memory_consolidation: None,
            memory_control: None,
            memory_tuning: None,
            mcp_servers: None,
            evolution: None,
            disabled_vendors: Vec::new(),
            verified_vendors: Vec::new(),
            schema_version: 1,
            max_rounds: None,
            history_budget_chars: None,
            tool_rules: None,
            subagent_max_turns: None,
            subagent_max_wall_secs: None,
            search_max_results: None,
            max_tool_output_chars: None,
        }
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
}
