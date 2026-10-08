//! 观察层：四指标纯函数 + 影子落库。
//!
//! observe CLI（指标/停止条件/合成数据生成器）零生产调用已删除——
//! 决策板经 `panel::commands::evolution_metrics` 在线取数；
//! 停止条件阈值思想移入决策板回滚预警。历史实现见 git log。
//! 不调 LLM；不写新数据库表；只读 evolution-proposals/changes/applied.jsonl。

pub mod metrics;
pub mod shadow;

pub use metrics::{compute as compute_metrics, ObserveMetrics};
