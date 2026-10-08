//! 评估指标（五指标纯函数 + applied 台账读取）。
//!
//! 离线评估 harness（case/sampler/feedback/runner/config + eval-run CLI）
//! 零生产调用，已删除；历史实现见 git log。
//! 现存消费方：决策板 `evolution_metrics` 命令、`tests/evolution_gov.rs`。

pub mod metrics;

pub use metrics::{compute, read_applied, AppliedRecord, MetricsReport};
