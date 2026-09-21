//! 五个核心指标计算（spec R1）
//!
//! 1. 任务成功率 task_success_rate = succeeded / total
//! 2. 工具调用效率 tool_call_efficiency = successful_tool_calls / total_tool_calls
//! 3. 行为偏差 behavior_deviation = 1 - fraction(预期行为命中)
//! 4. 回滚率 rollback_rate = rollbacks / applied
//! 5. 污染存活期 pollution_survival_days = avg(applied_at 未滑动的天数)
//!
//! 纯函数，不调 LLM；只读 evolution-applied.jsonl + mem_items 状态。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 指标计算结果（一轮评估输出）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MetricsReport {
    /// 评估 case 总数
    pub case_total: usize,
    /// 评估 case 成功数（基于 expected_behavior 命中）
    pub case_passed: usize,
    /// 任务成功率（0.0-1.0）
    pub task_success_rate: f64,
    /// 工具调用总次数（来自反馈 + applied 记录）
    pub tool_calls_total: u64,
    pub tool_calls_succeeded: u64,
    /// 工具调用效率
    pub tool_call_efficiency: f64,
    /// 行为偏差（0.0-1.0；越大越偏离）
    pub behavior_deviation: f64,
    /// 已应用提案数
    pub applied_total: u64,
    /// 回滚数
    pub rolled_back_total: u64,
    /// 回滚率
    pub rollback_rate: f64,
    /// 当前仍存活的 lesson 数（污染存活）
    pub live_lessons: u64,
    /// 污染存活期（天；仅对当前存活 lesson 求均值）
    pub pollution_survival_days: f64,
    /// 评估时刻（epoch ms）
    pub evaluated_at_ms: i64,
    /// 评估周期标签（手动 / "before" / "after"）
    pub period: String,
}

/// 单条 applied 记录（读 evolution-applied.jsonl）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppliedRecord {
    pub proposal_id: String,
    pub mem_key: String,
    pub applied_at_ms: i64,
    pub impact: String,
    pub summary: String,
}

/// 读 evolution-applied.jsonl；缺文件返回空 vec
pub fn read_applied(path: &std::path::Path) -> Result<Vec<AppliedRecord>, String> {
    use std::io::BufRead;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let f = std::fs::File::open(path).map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let reader = std::io::BufReader::new(f);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("读取第 {} 行失败：{e}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(&line).map_err(|e| format!("第 {} 行 JSON 错误：{e}", i + 1))?,
        );
    }
    Ok(out)
}

/// 计算回滚率（基于当前 mem_items 状态）
///
/// 已应用 = applied.jsonl 行数；
/// 仍存在 = 当前 mem_items 中 tags[0] 以 "evo:" 开头的条目数；
/// 已回滚 = 已应用 - 仍存在。
pub fn rollback_rate(applied: &[AppliedRecord], live_keys: &[String]) -> f64 {
    if applied.is_empty() {
        return 0.0;
    }
    let live: HashMap<&str, ()> = live_keys.iter().map(|k| (k.as_str(), ())).collect();
    let rolled_back = applied
        .iter()
        .filter(|r| !live.contains_key(r.mem_key.as_str()))
        .count();
    rolled_back as f64 / applied.len() as f64
}

/// 污染存活期（天；当前仍存活 lesson 的 (now - applied_at) 均值）
///
/// `applied_at_lookup`: mem_key → applied_at_ms（applied.jsonl 的索引）
pub fn pollution_survival_days(
    live_keys: &[String],
    applied: &[AppliedRecord],
    now_ms: i64,
) -> f64 {
    let lookup: HashMap<&str, i64> = applied
        .iter()
        .map(|r| (r.mem_key.as_str(), r.applied_at_ms))
        .collect();
    let mut total_days = 0.0;
    let mut n = 0u64;
    for k in live_keys {
        if let Some(at) = lookup.get(k.as_str()) {
            let days = (now_ms - *at) as f64 / 86_400_000.0;
            if days >= 0.0 {
                total_days += days;
                n += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        total_days / n as f64
    }
}

/// 行为偏差：对每个 case 的 expected_behavior 列表，统计命中率；
/// deviation = 1 - avg(hit_fraction)。
/// `actual_hits`: case_id → 实际命中的 expected_behavior 子集
pub fn behavior_deviation(case_total: usize, hit_fractions: &[(String, f64)]) -> f64 {
    if case_total == 0 {
        return 0.0;
    }
    let avg: f64 = if hit_fractions.is_empty() {
        1.0
    } else {
        hit_fractions.iter().map(|(_, f)| *f).sum::<f64>() / hit_fractions.len() as f64
    };
    (1.0 - avg).clamp(0.0, 1.0)
}

/// 主入口：聚合全部指标
pub fn compute(
    case_total: usize,
    case_passed: usize,
    tool_calls_total: u64,
    tool_calls_succeeded: u64,
    applied: &[AppliedRecord],
    live_keys: &[String],
    hit_fractions: &[(String, f64)],
    now_ms: i64,
    period: impl Into<String>,
) -> MetricsReport {
    let task_success_rate = if case_total == 0 {
        0.0
    } else {
        case_passed as f64 / case_total as f64
    };
    let tool_call_efficiency = if tool_calls_total == 0 {
        0.0
    } else {
        tool_calls_succeeded as f64 / tool_calls_total as f64
    };
    let dev = behavior_deviation(case_total, hit_fractions);
    let rb_rate = rollback_rate(applied, live_keys);
    let survival = pollution_survival_days(live_keys, applied, now_ms);
    MetricsReport {
        case_total,
        case_passed,
        task_success_rate,
        tool_calls_total,
        tool_calls_succeeded,
        tool_call_efficiency,
        behavior_deviation: dev,
        applied_total: applied.len() as u64,
        rolled_back_total: (applied.len() as f64 * rb_rate) as u64,
        rollback_rate: rb_rate,
        live_lessons: live_keys.len() as u64,
        pollution_survival_days: survival,
        evaluated_at_ms: now_ms,
        period: period.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_applied(id: &str, at: i64) -> AppliedRecord {
        AppliedRecord {
            proposal_id: id.into(),
            mem_key: format!("evo:{id}"),
            applied_at_ms: at,
            impact: "high".into(),
            summary: format!("s-{id}"),
        }
    }

    #[test]
    fn task_success_rate_basic() {
        let r = compute(10, 7, 0, 0, &[], &[], &[], 0, "test");
        assert!((r.task_success_rate - 0.7).abs() < 1e-9);
    }

    #[test]
    fn zero_cases_zero_rates() {
        let r = compute(0, 0, 0, 0, &[], &[], &[], 0, "empty");
        assert_eq!(r.task_success_rate, 0.0);
        assert_eq!(r.tool_call_efficiency, 0.0);
        assert_eq!(r.rollback_rate, 0.0);
        assert_eq!(r.pollution_survival_days, 0.0);
    }

    #[test]
    fn tool_efficiency_fraction() {
        let r = compute(0, 0, 10, 8, &[], &[], &[], 0, "t");
        assert!((r.tool_call_efficiency - 0.8).abs() < 1e-9);
    }

    #[test]
    fn rollback_rate_subtracts_live() {
        let applied = vec![
            mk_applied("a", 1000),
            mk_applied("b", 2000),
            mk_applied("c", 3000),
        ];
        let live = vec!["evo:a".into()]; // b, c 已回滚
        let r = rollback_rate(&applied, &live);
        assert!((r - 2.0 / 3.0).abs() < 1e-9, "回滚率应为 2/3：{r}");
    }

    #[test]
    fn rollback_rate_zero_when_all_live() {
        let applied = vec![mk_applied("a", 1000), mk_applied("b", 2000)];
        let live = vec!["evo:a".into(), "evo:b".into()];
        assert_eq!(rollback_rate(&applied, &live), 0.0);
    }

    #[test]
    fn pollution_survival_days_avg() {
        let day_ms: i64 = 86_400_000;
        let applied = vec![mk_applied("a", 0), mk_applied("b", 0), mk_applied("c", 0)];
        let live = vec!["evo:a".into(), "evo:b".into(), "evo:c".into()];
        // 当前时刻 5 天后
        let r = pollution_survival_days(&live, &applied, 5 * day_ms);
        assert!((r - 5.0).abs() < 1e-9);
    }

    #[test]
    fn pollution_skips_keys_without_applied_record() {
        let day_ms: i64 = 86_400_000;
        let applied = vec![mk_applied("a", 0)];
        let live = vec!["evo:a".into(), "evo:unknown".into()];
        // unknown 没有 applied 记录，跳过
        let r = pollution_survival_days(&live, &applied, 10 * day_ms);
        assert!((r - 10.0).abs() < 1e-9);
    }

    #[test]
    fn behavior_deviation_perfect_zero() {
        let hits = vec![("c1".into(), 1.0), ("c2".into(), 1.0)];
        assert_eq!(behavior_deviation(2, &hits), 0.0);
    }

    #[test]
    fn behavior_deviation_partial() {
        let hits = vec![("c1".into(), 0.5), ("c2".into(), 1.0)];
        let r = behavior_deviation(2, &hits);
        assert!((r - 0.25).abs() < 1e-9);
    }

    #[test]
    fn read_applied_missing_returns_empty() {
        let p = std::path::Path::new("/tmp/no-such-applied-xyz-98765.jsonl");
        let r = read_applied(p).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn read_applied_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "metrics-rt-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("applied.jsonl");
        let line = r#"{"proposal_id":"p1","mem_key":"evo:p1","applied_at_ms":1000,"impact":"high","summary":"x"}"#;
        std::fs::write(&p, line).unwrap();
        let r = read_applied(&p).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].proposal_id, "p1");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
