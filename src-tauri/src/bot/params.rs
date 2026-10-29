//! Agent 运行参数注册表（缩表版）：
//!
//! Agent 运行参数已全部常量化（轮数 / 工具数安全网、历史预算、max_tokens、
//! 回填上限、搜索条数、提问预算、回灌与滑动窗开关——见 `bot/constants.rs` 与
//! 各消费点），设置页不再有「Agent 运行参数」卡。本表只剩**仍有卡外消费者**
//! 的三行：
//! - `tools.python_timeout_secs`（「机器人设置」卡的输入框 + Python 开启确认
//!   框文案读它的生效值）；
//! - `trace.retention_days`（数据管理卡公示默认天数与可填范围）；
//! - `memory.extract_min_interval_secs`（记忆区的抽取限频文案）。
//!
//! 三行的口径不变：默认值由 resolver 在空配置下的输出派生，可填范围 = 钳制
//! 常量，生效值 = 钳过之后的数——「页面显示的与实际跑的是同一个数」。

use serde::Serialize;

use crate::bot::config::types::BotConfig;

/// 硬编码只读条目的解析器：来源恒为 hardcoded。
macro_rules! hardcoded_resolver {
    ($v:expr) => {{
        fn f(_cfg: &BotConfig) -> (String, &'static str) {
            ($v.to_string(), "hardcoded")
        }
        f as fn(&BotConfig) -> (String, &'static str)
    }};
}

// ── 仍有外部消费的常量与钳制（配置键已退役的一律收进 constants）──

/// 联网搜索条数的工具入参钳制（web_search 的 count 仍需上限，其余配置键已退役）。
pub fn clamp_search_max_results(n: u32) -> u32 {
    n.clamp(
        crate::bot::constants::SEARCH_MAX_RESULTS_MIN,
        crate::bot::constants::SEARCH_MAX_RESULTS_CAP,
    )
}

/// verifier 拒绝后的修复重试预算：未配置时的默认次数与上限。
/// `verifier_retry_max` 是保留的 bot-config.json 手改键（无 UI）。
pub const DEFAULT_VERIFIER_RETRY_MAX: u32 = 1;

pub fn clamp_verifier_retry_max(n: u32) -> u32 {
    n.clamp(0, crate::bot::constants::VERIFIER_RETRY_MAX_CAP)
}

pub fn resolve_verifier_retry_max(configured: Option<u32>) -> u32 {
    clamp_verifier_retry_max(configured.unwrap_or(DEFAULT_VERIFIER_RETRY_MAX))
}

// ── 参数表（三行）──

/// 参数定义（编译期常量表；value 的解析在 `bot_effective_params` 内查 cfg 字段）
pub struct ParamDef {
    /// 稳定 key（前端 React key），点分分层
    pub key: &'static str,
    /// 中文展示名
    pub label: &'static str,
    /// 分组
    pub category: &'static str,
    /// 一句话说明。**不写具体数字**：默认值由 resolver 派生、范围由 min/max 给出，
    /// 文案里再抄一遍数字就是第二份真相源。
    pub hint: &'static str,
    /// 可填范围下界（None = 不限或不适用）
    pub min: Option<u64>,
    /// 可填范围上界（None = 不限或不适用）
    pub max: Option<u64>,
    /// true = bot-config.json 有对应字段可改；false = 硬编码只读
    pub editable: bool,
    /// 生效值解析（值展示串, 来源）。
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
    /// 内置默认值（由 resolver 在空配置下的输出派生，不是手写字面量）
    pub default: String,
    /// config = 配置覆盖生效 / default = 用内置默认 / hardcoded = 硬编码只读
    pub source: &'static str,
    pub editable: bool,
    /// 可填范围下界（None = 不限或不适用）
    pub min: Option<u64>,
    /// 可填范围上界（None = 不限或不适用）
    pub max: Option<u64>,
}

fn plain_py_timeout(cfg: &BotConfig) -> String {
    crate::py::runtime::clamp_timeout_secs(
        cfg.python_timeout_secs
            .unwrap_or(crate::py::runtime::DEFAULT_TIMEOUT_SECS),
    )
    .to_string()
}

fn python_timeout_row(cfg: &BotConfig) -> (String, &'static str) {
    let src = if cfg.python_timeout_secs.is_some() {
        "config"
    } else {
        "default"
    };
    (plain_py_timeout(cfg), src)
}

/// 缩表后的参数表：只留仍有卡外消费者的三行（模块头注释）。
pub static PARAMS_TABLE: &[ParamDef] = &[
    ParamDef {
        key: "tools.python_timeout_secs",
        label: "Python 执行超时（秒）",
        category: "tools",
        hint: "run_python 默认超时；工具参数可单次覆盖",
        min: Some(1),
        max: Some(crate::py::runtime::MAX_TIMEOUT_SECS),
        editable: true,
        resolve: python_timeout_row,
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

/// 遍历参数表产出生效值视图（命令体直接映射）。
///
/// 默认值从 resolver 在空配置下的输出派生——派生与运行时同走一条路，不存在
/// 「页面默认 X、实际生效 Y」的分叉。
pub fn effective_params(cfg: &BotConfig) -> Vec<ParamView> {
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

/// 数据管理卡 / 记忆区 / Python 确认框的常量数据源：生效值 + 默认值 + 来源 + 范围。
#[tauri::command]
pub fn bot_effective_params(app: tauri::AppHandle) -> Vec<ParamView> {
    let cfg = crate::bot::config::load_config(&app);
    effective_params(&cfg)
}

#[cfg(test)]
mod tests;
