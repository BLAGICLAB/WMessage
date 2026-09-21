//! R3 L3 沙箱层 · jsonl 持久化（evolution-shadow.jsonl / evolution-ab.jsonl）
//!
//! spec R3：
//! - 4. evolution-shadow.jsonl / evolution-ab.jsonl 写入

use std::path::Path;

use super::shadow::ShadowOutcome;
use serde::{Deserialize, Serialize};

/// A/B 测试结果（一行一条）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AbRecord {
    pub change_id: String,
    pub session_id: String,
    /// A 或 B 分组
    pub group: AbGroup,
    pub change_applied: bool,                // 本组实际生效吗
    pub metrics_snapshot_id: Option<String>, // 关联 eval_results.jsonl 的某条
    pub recorded_at_ms: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AbGroup {
    A,
    B,
}

impl AbGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

// ───────────────────────── shadow jsonl ─────────────────────────

pub fn append_shadow(path: &Path, record: &ShadowOutcome) -> Result<(), String> {
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
        serde_json::to_string(record).map_err(|e| format!("序列化 ShadowOutcome 失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    Ok(())
}

pub fn read_shadow(path: &Path) -> Result<Vec<ShadowOutcome>, String> {
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

// ───────────────────────── A/B jsonl ─────────────────────────

pub fn append_ab(path: &Path, record: &AbRecord) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let line = serde_json::to_string(record).map_err(|e| format!("序列化 AbRecord 失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    Ok(())
}

pub fn read_ab(path: &Path) -> Result<Vec<AbRecord>, String> {
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

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::{ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
    use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

    fn mk_change(id: &str) -> ChangeRecord {
        ChangeRecord {
            change_id: id.into(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: id.trim_start_matches("chg-").into(),
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: "shadow test".into(),
            mem_key: format!("evo:{}", id.trim_start_matches("chg-")),
            impact: ImpactLevel::High,
            eval_before: None,
            eval_after: None,
            status: ChangeStatus::Shadowing,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::Pending,
            human_approver: None,
            created_at_ms: 1_700_000_000_000,
            rolled_back_at: None,
            rollback_reason: None,
        }
    }

    fn mk_shadow(change_id: &str) -> ShadowOutcome {
        ShadowOutcome {
            change_id: change_id.into(),
            session_id: "s1".into(),
            would_inject: true,
            hash_before: "aaa".into(),
            hash_after: "bbb".into(),
            decision: super::super::shadow::ShadowDecision::Pass,
            note: None,
            evaluated_at_ms: 1_700_000_000_000,
        }
    }

    fn mk_ab(change_id: &str, group: AbGroup) -> AbRecord {
        AbRecord {
            change_id: change_id.into(),
            session_id: "s1".into(),
            group,
            change_applied: true,
            metrics_snapshot_id: None,
            recorded_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn shadow_append_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "sb-sh-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-shadow.jsonl");
        let s1 = mk_shadow("chg-1");
        let s2 = mk_shadow("chg-2");
        append_shadow(&p, &s1).unwrap();
        append_shadow(&p, &s2).unwrap();
        let read = read_shadow(&p).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].change_id, "chg-1");
        assert_eq!(read[1].change_id, "chg-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ab_append_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "sb-ab-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-ab.jsonl");
        append_ab(&p, &mk_ab("chg-1", AbGroup::A)).unwrap();
        append_ab(&p, &mk_ab("chg-1", AbGroup::B)).unwrap();
        let read = read_ab(&p).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].group, AbGroup::A);
        assert_eq!(read[1].group, AbGroup::B);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_files_return_empty() {
        let p = std::path::Path::new("/tmp/definitely-no-shadow-xyz-12345.jsonl");
        assert!(read_shadow(p).unwrap().is_empty());
        let p2 = std::path::Path::new("/tmp/definitely-no-ab-xyz-67890.jsonl");
        assert!(read_ab(p2).unwrap().is_empty());
    }

    #[test]
    fn append_creates_parent_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "sb-pd-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let p = dir.join("nested").join("evolution-shadow.jsonl");
        append_shadow(&p, &mk_shadow("chg-x")).unwrap();
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ab_group_strings_locked() {
        assert_eq!(AbGroup::A.as_str(), "a");
        assert_eq!(AbGroup::B.as_str(), "b");
    }

    // 沉默未用变量
    #[allow(dead_code)]
    fn _unused_silence() {
        mk_change("chg-dummy");
    }
}
