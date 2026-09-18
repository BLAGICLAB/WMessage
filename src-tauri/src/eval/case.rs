//! 评估用例数据结构（R1 L4 评估层）
//!
//! jsonl 一行一条；schema 严格按 spec：
//! `{case_id, source_session_id, input, expected_behavior[], metrics[]}`
//!
//! 不动 evolution 模块的现有 JSON 字段；新字段独立落 jsonl（解决 VERIFICATION.md 冲突 #2）。

use serde::{Deserialize, Serialize};

/// 一条评估用例
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalCase {
    /// 唯一 id（如 "case-0001"、"test-apply_one_inserts_lesson_with_evolution_key"）
    pub case_id: String,
    /// 来源 session（从 bot_sessions 采样时填；测试派生时为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    /// 来源测试函数名（#[test] fn name；采样派生时为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_test: Option<String>,
    /// 输入/场景描述（自然语言；测试派生时是测试名）
    pub input: String,
    /// 期望行为列表（按字符串精确匹配；多行为表示同时满足）
    pub expected_behavior: Vec<String>,
    /// 适用指标（每个指标有权重；评估时聚合）
    pub metrics: Vec<MetricSpec>,
    /// 分类标签（如 "memory"、"tool"、"safety"）
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 指标规格
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricSpec {
    /// 指标名（与 metrics.rs 的指标枚举对齐）
    pub metric: String,
    /// 在聚合分中的权重（0.0-1.0）
    pub weight: f64,
}

/// 读 jsonl 为 EvalCase 列表；行格式错误抛 Err 并指出行号
pub fn read_jsonl(path: &std::path::Path) -> Result<Vec<EvalCase>, String> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let reader = std::io::BufReader::new(f);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("读取第 {} 行失败：{e}", i + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let c: EvalCase = serde_json::from_str(&line)
            .map_err(|e| format!("第 {} 行 JSON 错误：{e}", i + 1))?;
        out.push(c);
    }
    Ok(out)
}

/// 写 jsonl（追加模式）；保证父目录存在
pub fn append_jsonl(path: &std::path::Path, cases: &[EvalCase]) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    for c in cases {
        let line = serde_json::to_string(c).map_err(|e| format!("序列化 {case_id} 失败：{e}", case_id = c.case_id))?;
        writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_roundtrip_preserves_fields() {
        let c = EvalCase {
            case_id: "c1".into(),
            source_session_id: Some("s1".into()),
            source_test: None,
            input: "用户问天气".into(),
            expected_behavior: vec!["调用 web 工具".into(), "用中文回答".into()],
            metrics: vec![MetricSpec {
                metric: "task_success_rate".into(),
                weight: 1.0,
            }],
            tags: vec!["weather".into()],
        };
        let s = serde_json::to_string(&c).unwrap();
        let d: EvalCase = serde_json::from_str(&s).unwrap();
        assert_eq!(c, d);
    }

    #[test]
    fn case_omits_none_fields_in_serialize() {
        // source_session_id=None 不应出现在 JSON 里
        let c = EvalCase {
            case_id: "c1".into(),
            source_session_id: None,
            source_test: Some("t1".into()),
            input: "x".into(),
            expected_behavior: vec![],
            metrics: vec![],
            tags: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        assert!(v.get("source_session_id").is_none());
        assert_eq!(v["source_test"], "t1");
    }

    #[test]
    fn read_skips_empty_lines() {
        let dir = std::env::temp_dir().join(format!("eval-test-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("cases.jsonl");
        let content = "{\"case_id\":\"a\",\"input\":\"x\",\"expected_behavior\":[],\"metrics\":[]}\n\n{\"case_id\":\"b\",\"input\":\"y\",\"expected_behavior\":[],\"metrics\":[]}\n";
        std::fs::write(&p, content).unwrap();
        let cs = read_jsonl(&p).unwrap();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].case_id, "a");
        assert_eq!(cs[1].case_id, "b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_reports_bad_line_number() {
        let dir = std::env::temp_dir().join(format!("eval-bad-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.jsonl");
        std::fs::write(
            &p,
            "{\"case_id\":\"a\",\"input\":\"x\",\"expected_behavior\":[],\"metrics\":[]}\nNOT JSON\n",
        )
        .unwrap();
        let e = read_jsonl(&p).unwrap_err();
        assert!(e.contains("第 2 行"), "错误信息应包含行号：{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_creates_parent_dirs() {
        let dir = std::env::temp_dir().join(format!("eval-parent-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let p = dir.join("nested").join("cases.jsonl");
        let c = EvalCase {
            case_id: "c".into(),
            source_session_id: None,
            source_test: None,
            input: "x".into(),
            expected_behavior: vec![],
            metrics: vec![],
            tags: vec![],
        };
        append_jsonl(&p, &[c]).unwrap();
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}