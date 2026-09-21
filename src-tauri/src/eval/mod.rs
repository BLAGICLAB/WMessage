//! L4 评估层（R1）
//!
//! 提供：
//!   - `case`：EvalCase 数据结构 + jsonl 读写
//!   - `feedback`：FeedbackEntry 数据结构 + jsonl 读写
//!   - `metrics`：五个核心指标计算（纯函数）
//!   - `sampler`：从 bot_sessions 采样 / 从 #[test] 派生
//!   - `runner`：跑一轮评估、输出 MetricsReport
//!   - `config`：bot-config.json evolution.eval 块加载
//!
//! 不动 evolution 模块现有结构；不调 LLM；不写新数据库表；持久化一律 jsonl。
//!
//! 公开面：所有子模块（CLI binary 需要）。

pub mod case;
pub mod config;
pub mod feedback;
pub mod metrics;
pub mod runner;
pub mod sampler;

pub use case::{read_jsonl, EvalCase, MetricSpec};
pub use config::{load as load_config, EvolutionEvalConfig, RunFrequency};
pub use feedback::{
    append as append_feedback, read_all as read_feedback, FeedbackEntry, SignalType,
};
pub use metrics::{compute, read_applied, AppliedRecord, MetricsReport};
pub use runner::{append_result, run as run_eval};
pub use sampler::{
    list_sessions, sample_sessions, sessions_to_cases, test_name_to_case, SessionRow,
};
