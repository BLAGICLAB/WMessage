//! R3 L3 沙箱层（spec R3）
//!
//! 模块结构：
//! - `routing`     session_id 分流（FNV-1a + canary 5% + A/B 50/50）
//! - `kill_switch` kill_switch struct + bot-config.json 加载 + 优先级判断
//! - `shadow`      ShadowRunner + ShadowDecision（不重跑 LLM）
//! - `io`          evolution-shadow.jsonl / evolution-ab.jsonl 持久化
//!
//! 不调 LLM；不写新数据库表；持久化一律 jsonl。

pub mod io;
pub mod kill_switch;
pub mod routing;
pub mod shadow;

pub use io::{append_ab, append_shadow, read_ab, read_shadow, AbGroup, AbRecord};
pub use kill_switch::{
    default_off as default_kill_switch, load_from_file as load_kill_switch, KillSwitch,
};
pub use routing::{fnv1a, is_ab_a, is_canary};
pub use shadow::{run_shadow, ShadowDecision, ShadowInput, ShadowLesson, ShadowOutcome};
