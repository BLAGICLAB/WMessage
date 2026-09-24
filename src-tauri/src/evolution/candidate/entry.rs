//! R4 L1 候选层 · ProposalEntry 数据结构 + jsonl IO
//!
//! 文件：`{data_dir}/evolution-proposals.jsonl`，追加写。
//!
//! 关键设计（避开硬约束 #3）：
//! - 不动 EvolutionProposal struct（emit.rs:107 锁死 audit schema 字段）
//! - 扩展字段（完整 6 层 layer / change_id / 相关引用 / TTL）落独立 jsonl
//! - 通过 `proposal_id` 关联到 EvolutionProposal
//!
//! ProposalEntry 是 superset：包含足够信息独立派生 ChangeRecord，
//! 不需要回查 EvolutionProposal。

use serde::{Deserialize, Serialize};

use crate::evolution::change::EvolutionLayer;
use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

/// 候选池条目（evolution-proposals.jsonl 一行一条）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProposalEntry {
    /// 关联 EvolutionProposal.proposal_id
    pub proposal_id: String,
    /// 派生："chg-" + proposal_id
    pub change_id: String,
    /// 完整 6 层（区别于 ProposalCategory 4 类）
    pub layer: EvolutionLayer,

    pub impact: ImpactLevel,
    pub origin: ProposalOrigin,
    pub target: ProposalTarget,

    /// EvolutionProposal.suggestion.text 透传
    pub suggestion_text: String,
    /// 派生："evo:" + proposal_id
    pub mem_key: String,

    /// EvolutionProposal.evidence 子字段（透传）
    pub related_refs: Vec<String>,
    pub summary: String,
    pub occurrence_count: u32,
    pub window_hours: u32,

    pub created_at_ms: i64,
    /// TTL = created_at + 14 天
    pub expires_at_ms: i64,

    /// 候选池生命周期
    pub status: ProposalStatus,
}

/// 候选池生命周期（区别于 ChangeStatus 9 态）
///
/// 候选池有自己的状态机（简单 4 态），晋升为 ChangeRecord 后由 ChangeStatus 接管。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    /// 在候选池（默认）
    Pooled,
    /// 已晋升（已创建 ChangeRecord）
    Promoted,
    /// TTL 过期
    Expired,
    /// 被拒绝（人工/系统）
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pooled => "pooled",
            Self::Promoted => "promoted",
            Self::Expired => "expired",
            Self::Rejected => "rejected",
        }
    }
}

// ───────────────────────── jsonl IO ─────────────────────────

/// 追加一条 ProposalEntry。
/// 整行（含显式 `\n`）拼成单个 buffer 后**一次 write_all**——POSIX O_APPEND
/// 对常规文件的单次 write() 原子，配合调用方持 evolution store 锁
///（evolution::lock_evolution_store，本函数内部不加锁）防进程内交错。
/// 残余：跨进程（如 observe_run bin 与主进程同写）无 flock，见批次 spec。
pub fn append(path: &std::path::Path, entry: &ProposalEntry) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let mut line = serde_json::to_string(entry).map_err(|e| format!("序列化失败：{e}"))?;
    line.push('\n');
    f.write_all(line.as_bytes())
        .map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    Ok(())
}

/// 读全部 ProposalEntry
pub fn read_all(path: &std::path::Path) -> Result<Vec<ProposalEntry>, String> {
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

/// 按 proposal_id 查询
pub fn find_by_id<'a>(
    entries: &'a [ProposalEntry],
    proposal_id: &str,
) -> Option<&'a ProposalEntry> {
    entries.iter().find(|e| e.proposal_id == proposal_id)
}

/// 按 status 过滤
pub fn filter_by_status(entries: &[ProposalEntry], status: ProposalStatus) -> Vec<&ProposalEntry> {
    entries.iter().filter(|e| e.status == status).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::change::EvolutionLayer;

    fn mk(id: &str, status: ProposalStatus) -> ProposalEntry {
        ProposalEntry {
            proposal_id: id.into(),
            change_id: format!("chg-{id}"),
            layer: EvolutionLayer::Policy,
            impact: ImpactLevel::Medium,
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("s-{id}"),
            occurrence_count: 1,
            window_hours: 24,
            created_at_ms: 1_700_000_000_000,
            expires_at_ms: 1_700_000_000_000 + 14 * 86_400_000,
            status,
        }
    }

    #[test]
    fn status_strings_locked() {
        assert_eq!(ProposalStatus::Pooled.as_str(), "pooled");
        assert_eq!(ProposalStatus::Promoted.as_str(), "promoted");
        assert_eq!(ProposalStatus::Expired.as_str(), "expired");
        assert_eq!(ProposalStatus::Rejected.as_str(), "rejected");
    }

    #[test]
    fn append_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "pe-rt-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-proposals.jsonl");
        append(&p, &mk("a", ProposalStatus::Pooled)).unwrap();
        append(&p, &mk("b", ProposalStatus::Promoted)).unwrap();
        let read = read_all(&p).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].proposal_id, "a");
        assert_eq!(read[1].status, ProposalStatus::Promoted);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let p = std::path::Path::new("/tmp/definitely-no-proposals-xyz-12345.jsonl");
        assert!(read_all(p).unwrap().is_empty());
    }

    #[test]
    fn append_creates_parent_dirs() {
        let dir = std::env::temp_dir().join(format!(
            "pe-pd-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let p = dir.join("nested").join("evolution-proposals.jsonl");
        append(&p, &mk("x", ProposalStatus::Pooled)).unwrap();
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_by_id_returns_match() {
        let entries = vec![
            mk("a", ProposalStatus::Pooled),
            mk("b", ProposalStatus::Promoted),
        ];
        let found = find_by_id(&entries, "b").unwrap();
        assert_eq!(found.proposal_id, "b");
        assert!(find_by_id(&entries, "missing").is_none());
    }

    #[test]
    fn filter_by_status_works() {
        let entries = vec![
            mk("a", ProposalStatus::Pooled),
            mk("b", ProposalStatus::Promoted),
            mk("c", ProposalStatus::Pooled),
        ];
        let pooled = filter_by_status(&entries, ProposalStatus::Pooled);
        assert_eq!(pooled.len(), 2);
        assert!(pooled.iter().all(|e| e.status == ProposalStatus::Pooled));
    }

    #[test]
    fn entry_carries_locked_fields() {
        // 锁死字段集
        let e = mk("lock", ProposalStatus::Pooled);
        let v = serde_json::to_value(&e).unwrap();
        for key in [
            "proposal_id",
            "change_id",
            "layer",
            "impact",
            "origin",
            "target",
            "suggestion_text",
            "mem_key",
            "related_refs",
            "summary",
            "occurrence_count",
            "window_hours",
            "created_at_ms",
            "expires_at_ms",
            "status",
        ] {
            assert!(v.get(key).is_some(), "ProposalEntry 缺字段 {key}");
        }
    }
}
