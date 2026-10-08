//! 沙箱层存留部分：kill_switch（apply 入口在用）。
//!
//! canary/A-B 分流、ShadowRunner 影子判定、shadow/ab jsonl 持久化
//! 零生产调用已删除（现行影子观察走 `observe::shadow` 全量版）；
//! FNV-1a 稳定哈希移入 `proposal::short_hash` 唯一消费者处。历史实现见 git log。

pub mod kill_switch;

pub use kill_switch::{
    default_off as default_kill_switch, load_from_file as load_kill_switch, KillSwitch,
};
