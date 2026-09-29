//! EvalRunner —— 跑一轮评估、产出 MetricsReport
//!
//! 流程：
//!   1. 加载 eval_set.jsonl → EvalCase 列表
//!   2. 加载 evolution-applied.jsonl → AppliedRecord 列表
//!   3. 打开 dev DB（rusqlite 直连，无 AppHandle）→ 列出当前存活 lesson 的 mem_key
//!   4. 加载 feedback.jsonl → 计算 tool_calls_total / succeeded / hit_fractions
//!   5. compute() → MetricsReport
//!   6. 追加写 evolution-eval-results.jsonl + 打印到 stdout
//!
//! 不调 LLM；不动 prompt / TOOLS / 命令名 / 事件名 / JSON 字段 / 错误码。

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::case::{read_jsonl, EvalCase};
use super::config::EvolutionEvalConfig;
use super::feedback::{read_all as read_feedback, FeedbackEntry, SignalType};
use super::metrics::{compute, read_applied, MetricsReport};

/// 跑一轮评估，返回 MetricsReport
///
/// `period` 标签："manual" / "before" / "after" / 自定义
/// `db_path`：dev SQLite 文件路径（None 时跳过 mem_items 状态读取，回滚率/存活期=0）
/// `applied_path`：B4-1 覆盖（CLI `--applied`）；None 按优先级推导
///（db 同目录 > eval_set 同目录兜底）
pub fn run(
    cfg: &EvolutionEvalConfig,
    db_path: Option<&Path>,
    applied_override: Option<&Path>,
    period: &str,
) -> Result<MetricsReport, String> {
    // 1. eval set
    let cases: Vec<EvalCase> = read_jsonl(&cfg.eval_set_path)?;
    // 2. applied（B4-1：路径推导修正）
    let applied_path = resolve_applied_path(cfg, db_path, applied_override);
    let applied = read_applied(&applied_path)?;
    // 3. live keys
    let live_keys: Vec<String> = if let Some(p) = db_path {
        list_live_lesson_keys(p)?
    } else {
        Vec::new()
    };
    // 4. feedback
    let feedback = read_feedback(&cfg.feedback_path)?;
    let (tool_total, tool_ok, hit_fractions) = aggregate_feedback(&cases, &feedback);
    // 5. compute
    let now_ms = chrono::Utc::now().timestamp_millis();
    let report = compute(
        cases.len(),
        cases.len(), // B4-1 标注：无 case 级判定，恒 100% 占位（MetricsReport.case_passed_placeholder=true）；R2 接入 ChangeRecord 后实判
        tool_total,
        tool_ok,
        &applied,
        &live_keys,
        &hit_fractions,
        now_ms,
        period,
    );
    Ok(report)
}

/// 把 eval 结果追加到 evolution-eval-results.jsonl
pub fn append_result(path: &Path, report: &MetricsReport) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let line =
        serde_json::to_string(report).map_err(|e| format!("序列化 MetricsReport 失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    Ok(())
}

/// 列出当前 mem_items 里 tags[0] 以 "evo:" 开头的 mem_key
///
/// 只读；缺表返回空 vec（dev DB 没建表时不 panic）
fn list_live_lesson_keys(db_path: &Path) -> Result<Vec<String>, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("打开 DB {db_path:?} 失败：{e}"))?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='mem_items'",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("查 mem_items 表失败：{e}"))?;
    if exists == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT tags FROM mem_items WHERE kind='lesson'")
        .map_err(|e| format!("prepare mem_items 失败：{e}"))?;
    let rows = stmt
        .query_map([], |r| {
            let tags: String = r.get(0)?;
            Ok(tags)
        })
        .map_err(|e| format!("query mem_items 失败：{e}"))?;
    let mut keys = Vec::new();
    for row in rows {
        let tags = row.map_err(|e| format!("read row 失败：{e}"))?;
        // tags 是逗号分隔；取第一段
        if let Some(first) = tags.split(',').next() {
            let first = first.trim();
            if first.starts_with("evo:") {
                keys.push(first.to_string());
            }
        }
    }
    Ok(keys)
}

/// 把 feedback 按 session / case 聚合
fn aggregate_feedback(
    cases: &[EvalCase],
    feedback: &[FeedbackEntry],
) -> (u64, u64, Vec<(String, f64)>) {
    // tool_calls: 全部 ToolError 信号视为失败调用，task_complete 视为成功调用
    let mut total: u64 = 0;
    let mut succeeded: u64 = 0;
    for f in feedback {
        match f.signal_type {
            SignalType::ToolError => {
                total += 1;
                // value 0.0/1.0 = 失败/不一定（按 0 算）
            }
            SignalType::TaskComplete => {
                total += 1;
                succeeded += 1;
            }
            SignalType::TaskFailed => {
                total += 1;
            }
            _ => {}
        }
    }
    // hit_fractions: 对每个 case 检查 feedback 里 thumbs_up / thumbs_down 比例
    let mut hit_fractions: Vec<(String, f64)> = Vec::new();
    for c in cases {
        let related: Vec<&FeedbackEntry> = feedback
            .iter()
            .filter(|f| {
                f.case_id.as_deref() == Some(c.case_id.as_str()) || f.session_id == c.case_id
            })
            .collect();
        if related.is_empty() {
            hit_fractions.push((c.case_id.clone(), 1.0));
            continue;
        }
        let pos = related
            .iter()
            .filter(|f| matches!(f.signal_type, SignalType::ThumbsUp))
            .count() as f64;
        let neg = related
            .iter()
            .filter(|f| matches!(f.signal_type, SignalType::ThumbsDown))
            .count() as f64;
        let frac = if pos + neg > 0.0 {
            pos / (pos + neg)
        } else {
            1.0
        };
        hit_fractions.push((c.case_id.clone(), frac));
    }
    (total, succeeded, hit_fractions)
}

// EvolutionEvalConfig 扩展：applied 路径兜底推导。B4-1（P2-EV8）：真实路径
// 优先级 = 显式 --applied > db_path 同目录（applied.jsonl 与 DB 同在 data_dir）
// > 此兜底；此前直接用本兜底会读到 `<eval_set>.applied.jsonl`（不存在 → 空
// applied → 回滚率/污染存活期恒 0）
trait ConfigExt {
    fn eval_set_path_with_applied(&self) -> std::path::PathBuf;
}
impl ConfigExt for EvolutionEvalConfig {
    fn eval_set_path_with_applied(&self) -> std::path::PathBuf {
        let mut p = self.eval_set_path.clone();
        p.set_extension("applied.jsonl");
        p
    }
}

/// B4-1：applied.jsonl 路径解析（--applied 覆盖 > db 同目录 > 兜底）
pub(crate) fn resolve_applied_path(
    cfg: &EvolutionEvalConfig,
    db_path: Option<&Path>,
    applied_override: Option<&Path>,
) -> std::path::PathBuf {
    applied_override
        .map(Path::to_path_buf)
        .or_else(|| db_path.and_then(|p| p.parent().map(|d| d.join("evolution-applied.jsonl"))))
        .unwrap_or_else(|| cfg.eval_set_path_with_applied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::metrics::AppliedRecord;
    use std::collections::HashMap;

    fn mk_fb(signal: SignalType, value: f64, session: &str, case: Option<&str>) -> FeedbackEntry {
        FeedbackEntry {
            session_id: session.into(),
            case_id: case.map(String::from),
            signal_type: signal,
            value,
            timestamp_ms: 1_700_000_000_000,
            note: None,
        }
    }

    #[test]
    fn aggregate_feedback_counts_tools() {
        let cases: Vec<EvalCase> = vec![];
        let fb = vec![
            mk_fb(SignalType::TaskComplete, 1.0, "s1", None),
            mk_fb(SignalType::TaskComplete, 1.0, "s2", None),
            mk_fb(SignalType::ToolError, 1.0, "s3", None),
            mk_fb(SignalType::TaskFailed, 1.0, "s4", None),
            mk_fb(SignalType::ThumbsUp, 1.0, "s5", None), // 不计数
        ];
        let (t, ok, _) = aggregate_feedback(&cases, &fb);
        assert_eq!(t, 4);
        assert_eq!(ok, 2);
    }

    // ── B4-1（P2-EV8）：applied 路径三分支优先级 ──

    fn mk_cfg() -> EvolutionEvalConfig {
        EvolutionEvalConfig {
            eval_set_path: PathBuf::from("/cfg/eval-set.jsonl"),
            feedback_path: PathBuf::from("/cfg/feedback.jsonl"),
            run_frequency: crate::eval::config::RunFrequency::Manual,
        }
    }

    #[test]
    fn applied_path_override_wins() {
        let p = super::resolve_applied_path(
            &mk_cfg(),
            Some(Path::new("/data/wmessage.db")),
            Some(Path::new("/explicit/applied.jsonl")),
        );
        assert_eq!(
            p,
            PathBuf::from("/explicit/applied.jsonl"),
            "--applied 最优先"
        );
    }

    #[test]
    fn applied_path_defaults_to_db_sibling() {
        // 修复的回归点：默认应取 db 同目录（= data_dir）的 evolution-applied.jsonl，
        // 而非旧的 eval_set 同目录兜底（读不到 → 恒 0 指标）
        let p = super::resolve_applied_path(&mk_cfg(), Some(Path::new("/data/wmessage.db")), None);
        assert_eq!(
            p,
            PathBuf::from("/data/evolution-applied.jsonl"),
            "db 同目录优先于兜底"
        );
    }

    #[test]
    fn applied_path_falls_back_to_eval_set_sibling() {
        let p = super::resolve_applied_path(&mk_cfg(), None, None);
        assert_eq!(
            p,
            PathBuf::from("/cfg/eval-set.applied.jsonl"),
            "无 db 时走兜底"
        );
    }

    #[test]
    fn aggregate_feedback_hit_fractions_match_case() {
        let cases = vec![EvalCase {
            case_id: "case-A".into(),
            source_session_id: None,
            source_test: None,
            input: "x".into(),
            expected_behavior: vec![],
            metrics: vec![],
            tags: vec![],
        }];
        let fb = vec![
            mk_fb(SignalType::ThumbsUp, 1.0, "s", Some("case-A")),
            mk_fb(SignalType::ThumbsUp, 1.0, "s", Some("case-A")),
            mk_fb(SignalType::ThumbsDown, 1.0, "s", Some("case-A")),
        ];
        let (_, _, hits) = aggregate_feedback(&cases, &fb);
        assert_eq!(hits.len(), 1);
        assert!((hits[0].1 - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn append_result_writes_jsonl() {
        let dir = std::env::temp_dir().join(format!(
            "runner-res-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let p = dir.join("nested").join("results.jsonl");
        let report = MetricsReport {
            case_total: 10,
            case_passed: 8,
            case_passed_placeholder: false,
            task_success_rate: 0.8,
            tool_calls_total: 5,
            tool_calls_succeeded: 4,
            tool_call_efficiency: 0.8,
            behavior_deviation: 0.1,
            applied_total: 3,
            rolled_back_total: 1,
            rollback_rate: 1.0 / 3.0,
            live_lessons: 2,
            pollution_survival_days: 5.0,
            evaluated_at_ms: 1_700_000_000_000,
            period: "test".into(),
        };
        append_result(&p, &report).unwrap();
        let read = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(read.lines().next().unwrap()).unwrap();
        assert_eq!(v["case_total"], 10);
        assert_eq!(v["period"], "test");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_live_lesson_keys_empty_when_table_missing() {
        let dir = std::env::temp_dir().join(format!(
            "runner-db-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("empty.db");
        // 建一个不含 mem_items 的空 DB
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch("CREATE TABLE dummy(id INTEGER);")
            .unwrap();
        drop(conn);
        let keys = list_live_lesson_keys(&p).unwrap();
        assert!(keys.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_live_lesson_keys_returns_evo_prefixed() {
        let dir = std::env::temp_dir().join(format!(
            "runner-keys-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.db");
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch(
            "CREATE TABLE mem_items(id TEXT, kind TEXT, tags TEXT);
             INSERT INTO mem_items VALUES ('1', 'lesson', 'evo:p1,evolution');
             INSERT INTO mem_items VALUES ('2', 'lesson', 'evo:p2,evolution');
             INSERT INTO mem_items VALUES ('3', 'fact', 'profile:foo');",
        )
        .unwrap();
        drop(conn);
        let keys = list_live_lesson_keys(&p).unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"evo:p1".to_string()));
        assert!(keys.contains(&"evo:p2".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
