//! R6 L4 观察层（spec R6）
//!
//! 模块结构：
//! - `metrics`    4 个核心指标纯函数
//! - `synthetic`  合成数据生成器（B 阶段验证用）
//!
//! 不调 LLM；不写新数据库表；只读 evolution-proposals/changes/applied.jsonl。

pub mod metrics;
pub mod shadow;
pub mod stop;
pub mod synthetic;

pub use metrics::{compute as compute_metrics, ObserveMetrics};
pub use stop::{check_stop_condition, StopConditionStatus, StopReason};
pub use synthetic::{
    generate as generate_synthetic, write_to_files, SyntheticConfig, SyntheticData,
};
