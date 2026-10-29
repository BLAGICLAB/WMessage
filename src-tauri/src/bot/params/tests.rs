//! `bot::params`（缩表版）单测：三行、口径与钳制常量同源。

use super::*;
use crate::bot::config::types::BotConfig;

fn empty_cfg() -> BotConfig {
    BotConfig::default()
}

#[test]
fn table_has_exactly_the_three_rows_with_consumers() {
    let v = effective_params(&empty_cfg());
    assert_eq!(v.len(), 3);
    let keys: Vec<&str> = v.iter().map(|p| p.key).collect();
    assert!(keys.contains(&"tools.python_timeout_secs"));
    assert!(keys.contains(&"trace.retention_days"));
    assert!(keys.contains(&"memory.extract_min_interval_secs"));
}

#[test]
fn python_timeout_row_tracks_config_and_clamp() {
    let mut cfg = empty_cfg();
    // 未配置 → 默认来源，默认值 = constants::PYTHON_TIMEOUT_SECS
    let v = effective_params(&cfg);
    let row = v
        .iter()
        .find(|p| p.key == "tools.python_timeout_secs")
        .unwrap();
    assert_eq!(row.source, "default");
    assert_eq!(
        row.default,
        crate::bot::constants::PYTHON_TIMEOUT_SECS.to_string()
    );
    // 配置越界 → 展示值必须是钳过的生效值，不是用户填的原值
    cfg.python_timeout_secs = Some(u64::MAX);
    let v = effective_params(&cfg);
    let row = v
        .iter()
        .find(|p| p.key == "tools.python_timeout_secs")
        .unwrap();
    assert_eq!(row.source, "config");
    assert_eq!(
        row.value,
        crate::py::runtime::clamp_timeout_secs(u64::MAX).to_string()
    );
}

#[test]
fn retention_row_range_matches_trace_clamp() {
    let v = effective_params(&empty_cfg());
    let row = v.iter().find(|p| p.key == "trace.retention_days").unwrap();
    assert_eq!(
        (row.min, row.max),
        (
            Some(crate::db::trace::TRACE_RETENTION_MIN_DAYS as u64),
            Some(crate::db::trace::TRACE_RETENTION_MAX_DAYS as u64)
        ),
        "可填范围必须等于数据管理卡实际生效的钳制边界"
    );
}
