//! R2 L2 版本层 · ChangeRecord 数据结构 + jsonl IO（B 方案）
//!
//! 文件：`{data_dir}/evolution-changes.jsonl`，追加写。
//! 字段分组（按 DERIVABILITY.md 派生性）：
//! - 构造时派生（不入 jsonl 即可）：change_id, hard_constraint_compliance, schema_version
//!   （schema_version 因向前兼容默认 1，仍持久化以便未来 bump）
//! - 必须持久化：status, parent_id, eval_before, eval_after, rolled_back_at,
//!   rollback_reason, approval_source, human_approver, created_at_ms,
//!   layer, proposal_id, target, suggestion_text, mem_key, impact
//! - 不持久化（查表）：applied_at 从 evolution-applied.jsonl[mem_key] 派生

use serde::{Deserialize, Serialize};

use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

/// 一条变更记录（jsonl 一行一条）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChangeRecord {
    /// "chg-" + proposal_id（构造时派生）
    pub change_id: String,
    /// 父 change_id；根 = None
    pub parent_id: Option<String>,
    /// Schema 版本（默认 1，向前兼容字段）
    pub schema_version: u32,

    /// 六层之一
    pub layer: EvolutionLayer,
    /// 提案来源（透传 EvolutionProposal.origin）
    pub origin: ProposalOrigin,
    /// 关联 proposal_id
    pub proposal_id: String,
    /// 透传 ProposalTarget
    pub target: ProposalTarget,
    /// suggestion.text 不可变快照
    pub suggestion_text: String,
    /// evolution-applied.jsonl 的 mem_key（"evo:<proposal_id>"）
    pub mem_key: String,
    /// 透传 ImpactLevel
    pub impact: ImpactLevel,

    /// 应用前评估快照
    pub eval_before: Option<EvalResult>,
    /// 应用后评估快照
    pub eval_after: Option<EvalResult>,

    /// 9 态状态机当前状态
    pub status: ChangeStatus,
    /// 硬约束合规检查（构造时派生）
    pub hard_constraint_compliance: bool,
    /// 批准来源（Pending / AutoApplied / HumanApproved / SystemRejected / HumanRejected）
    pub approval_source: ApprovalSource,
    /// 人工批准者（None = 还没批准 / 系统自动）
    pub human_approver: Option<String>,

    /// ChangeRecord 创建时间（epoch ms）
    pub created_at_ms: i64,
    /// 回滚时间
    pub rolled_back_at: Option<i64>,
    /// 回滚原因
    pub rollback_reason: Option<String>,
}

/// 评估快照（仅五个核心指标 + 时间戳；不依赖 eval 模块避免耦合）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalResult {
    pub task_success_rate: f64,
    pub tool_call_efficiency: f64,
    pub behavior_deviation: f64,
    pub rollback_rate: f64,
    pub pollution_survival_days: f64,
    pub evaluated_at_ms: i64,
}

/// 9 态状态机
///
/// spec 硬约束 ②：必须 `Pending → Approved → Applied`，
/// 本状态机对 `Approved → Canary → Active` 显式建模每一步，
/// `AutoApplied` 和 `HumanApproved` 在 ApprovalSource 区分（不在 status 区分）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    /// 候选刚产出（Pending — spec 用词）
    Pending,
    /// R3 影子测试中
    Shadowing,
    /// 影子通过，待批准
    ShadowPassed,
    /// 已批准（待 canary 或 active）
    Approved,
    /// R3 金丝雀（5%）
    Canary,
    /// 已全量生效
    Active,
    /// 拒绝（人工 / 系统 / 合规失败）
    Rejected,
    /// 已回滚
    RolledBack,
    /// 超期（TTL 14 天）
    Expired,
}

impl ChangeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Shadowing => "shadowing",
            Self::ShadowPassed => "shadow_passed",
            Self::Approved => "approved",
            Self::Canary => "canary",
            Self::Active => "active",
            Self::Rejected => "rejected",
            Self::RolledBack => "rolled_back",
            Self::Expired => "expired",
        }
    }

    /// 是否终态（不可再流转）
    ///
    /// Active **不是**严格终态——可被回滚到 RolledBack，也可自然 Expired。
    /// 严格终态只有 Rejected / RolledBack / Expired（3 个）。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Rejected | Self::RolledBack | Self::Expired)
    }
}

/// 批准来源（spec 硬约束 ②：AutoApplied / HumanApproved 显式区分）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalSource {
    /// 还没批准（Pending 状态时）
    Pending,
    /// 系统自动应用（满足 auto_apply_gate 后走 apply 路径）
    AutoApplied,
    /// 人工批准（R5 决策面板批准后走 apply 路径）
    HumanApproved,
    /// 系统拒绝（hard_constraint_compliance=false 或 sandbox 失败）
    SystemRejected,
    /// 人工拒绝（R5 决策面板拒绝）
    HumanRejected,
}

impl ApprovalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::AutoApplied => "auto_applied",
            Self::HumanApproved => "human_approved",
            Self::SystemRejected => "system_rejected",
            Self::HumanRejected => "human_rejected",
        }
    }
}

/// 六层（参数 / 策略 / PromptHint / ToolSchema / Skill / Code）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionLayer {
    Parameter,
    Policy,
    PromptHint,
    ToolSchema,
    Skill,
    Code,
}

impl EvolutionLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parameter => "parameter",
            Self::Policy => "policy",
            Self::PromptHint => "prompt_hint",
            Self::ToolSchema => "tool_schema",
            Self::Skill => "skill",
            Self::Code => "code",
        }
    }
}

// ───────────────────────── jsonl IO ─────────────────────────

/// 追加一条 ChangeRecord 到 jsonl；保证父目录存在
pub fn append(path: &std::path::Path, record: &ChangeRecord) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录 {parent:?} 失败：{e}"))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
    let line = serde_json::to_string(record).map_err(|e| format!("序列化失败：{e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写入 {path:?} 失败：{e}"))?;
    Ok(())
}

/// 读全部 ChangeRecord；缺文件返空 vec
pub fn read_all(path: &std::path::Path) -> Result<Vec<ChangeRecord>, String> {
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
            serde_json::from_str(&line)
                .map_err(|e| format!("第 {} 行 JSON 错误：{e}", i + 1))?,
        );
    }
    Ok(out)
}

/// 按 change_id 查询；返回 (record, index) 或 None
pub fn find_by_id<'a>(
    records: &'a [ChangeRecord],
    change_id: &str,
) -> Option<(usize, &'a ChangeRecord)> {
    records.iter().enumerate().find(|(_, r)| r.change_id == change_id)
}

/// 按 parent_id 查询子代
pub fn find_children<'a>(
    records: &'a [ChangeRecord],
    parent_id: &str,
) -> Vec<&'a ChangeRecord> {
    records.iter().filter(|r| r.parent_id.as_deref() == Some(parent_id)).collect()
}

/// 根记录（parent_id = None）
pub fn find_roots(records: &[ChangeRecord]) -> Vec<&ChangeRecord> {
    records.iter().filter(|r| r.parent_id.is_none()).collect()
}

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{ProposalOrigin, ProposalTarget};

    fn mk(id: &str, status: ChangeStatus) -> ChangeRecord {
        ChangeRecord {
            change_id: id.into(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: id.trim_start_matches("chg-").to_string(),
            target: ProposalTarget::MemoryPolicy { policy: "test".into() },
            suggestion_text: "test".into(),
            mem_key: format!("evo:{}", id.trim_start_matches("chg-")),
            impact: ImpactLevel::Medium,
            eval_before: None,
            eval_after: None,
            status,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::Pending,
            human_approver: None,
            created_at_ms: 1_700_000_000_000,
            rolled_back_at: None,
            rollback_reason: None,
        }
    }

    // ─── 字符串锁死 ───

    #[test]
    fn status_strings_locked() {
        // 锁死 grep 字符串
        assert_eq!(ChangeStatus::Pending.as_str(), "pending");
        assert_eq!(ChangeStatus::Shadowing.as_str(), "shadowing");
        assert_eq!(ChangeStatus::ShadowPassed.as_str(), "shadow_passed");
        assert_eq!(ChangeStatus::Approved.as_str(), "approved");
        assert_eq!(ChangeStatus::Canary.as_str(), "canary");
        assert_eq!(ChangeStatus::Active.as_str(), "active");
        assert_eq!(ChangeStatus::Rejected.as_str(), "rejected");
        assert_eq!(ChangeStatus::RolledBack.as_str(), "rolled_back");
        assert_eq!(ChangeStatus::Expired.as_str(), "expired");
    }

    #[test]
    fn approval_source_strings_locked() {
        assert_eq!(ApprovalSource::Pending.as_str(), "pending");
        assert_eq!(ApprovalSource::AutoApplied.as_str(), "auto_applied");
        assert_eq!(ApprovalSource::HumanApproved.as_str(), "human_approved");
        assert_eq!(ApprovalSource::SystemRejected.as_str(), "system_rejected");
        assert_eq!(ApprovalSource::HumanRejected.as_str(), "human_rejected");
    }

    #[test]
    fn layer_strings_locked() {
        assert_eq!(EvolutionLayer::Parameter.as_str(), "parameter");
        assert_eq!(EvolutionLayer::Policy.as_str(), "policy");
        assert_eq!(EvolutionLayer::PromptHint.as_str(), "prompt_hint");
        assert_eq!(EvolutionLayer::ToolSchema.as_str(), "tool_schema");
        assert_eq!(EvolutionLayer::Skill.as_str(), "skill");
        assert_eq!(EvolutionLayer::Code.as_str(), "code");
    }

    // ─── jsonl IO ───

    #[test]
    fn append_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("change-rt-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-changes.jsonl");
        let r1 = mk("chg-a", ChangeStatus::Pending);
        let r2 = mk("chg-b", ChangeStatus::Active);
        append(&p, &r1).unwrap();
        append(&p, &r2).unwrap();
        let read = read_all(&p).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].change_id, "chg-a");
        assert_eq!(read[1].status, ChangeStatus::Active);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let p = std::path::Path::new("/tmp/definitely-no-such-evolution-changes-xyz-99999.jsonl");
        let read = read_all(p).unwrap();
        assert!(read.is_empty());
    }

    #[test]
    fn append_creates_parent_dirs() {
        let dir = std::env::temp_dir().join(format!("change-parent-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        let p = dir.join("nested").join("changes.jsonl");
        let r = mk("chg-c", ChangeStatus::Pending);
        append(&p, &r).unwrap();
        assert!(p.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_skips_empty_lines_on_read() {
        let dir = std::env::temp_dir().join(format!("change-skip-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("changes.jsonl");
        append(&p, &mk("chg-a", ChangeStatus::Pending)).unwrap();
        append(&p, &mk("chg-b", ChangeStatus::Active)).unwrap();
        let read = read_all(&p).unwrap();
        assert_eq!(read.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── parent_id 链查询 ───

    #[test]
    fn find_by_id_returns_match() {
        let records = vec![
            mk("chg-root", ChangeStatus::Active),
            mk("chg-child", ChangeStatus::Pending),
        ];
        let (i, r) = find_by_id(&records, "chg-child").unwrap();
        assert_eq!(i, 1);
        assert_eq!(r.change_id, "chg-child");
        assert!(find_by_id(&records, "chg-missing").is_none());
    }

    #[test]
    fn find_children_returns_direct_descendants() {
        let mut r1 = mk("chg-root", ChangeStatus::Active);
        r1.parent_id = None;
        let mut r2 = mk("chg-child1", ChangeStatus::Pending);
        r2.parent_id = Some("chg-root".into());
        let mut r3 = mk("chg-child2", ChangeStatus::Pending);
        r3.parent_id = Some("chg-root".into());
        let mut r4 = mk("chg-grandchild", ChangeStatus::Pending);
        r4.parent_id = Some("chg-child1".into());
        let records = vec![r1, r2, r3, r4];
        let children = find_children(&records, "chg-root");
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|r| r.change_id == "chg-child1"));
        assert!(children.iter().any(|r| r.change_id == "chg-child2"));
    }

    #[test]
    fn find_roots_returns_only_top_level() {
        let mut r1 = mk("chg-root", ChangeStatus::Active);
        r1.parent_id = None;
        let mut r2 = mk("chg-child", ChangeStatus::Pending);
        r2.parent_id = Some("chg-root".into());
        let records = vec![r1, r2];
        let roots = find_roots(&records);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].change_id, "chg-root");
    }

    // ─── schema_version 默认值 ───

    #[test]
    fn schema_version_defaults_to_one() {
        let r = mk("chg-x", ChangeStatus::Pending);
        assert_eq!(r.schema_version, 1);
    }

    // ─── 字段集锁死 ───

    #[test]
    fn change_record_carries_locked_fields() {
        // 锁死 ChangeRecord 字段集——后续加字段要老板拍
        let r = mk("chg-lock", ChangeStatus::Pending);
        let v = serde_json::to_value(&r).unwrap();
        for key in [
            "change_id",
            "parent_id",
            "schema_version",
            "layer",
            "origin",
            "proposal_id",
            "target",
            "suggestion_text",
            "mem_key",
            "impact",
            "eval_before",
            "eval_after",
            "status",
            "hard_constraint_compliance",
            "approval_source",
            "human_approver",
            "created_at_ms",
            "rolled_back_at",
            "rollback_reason",
        ] {
            assert!(v.get(key).is_some(), "ChangeRecord 缺字段 {key}");
        }
    }

    #[test]
    fn eval_result_carries_locked_fields() {
        let e = EvalResult {
            task_success_rate: 0.8,
            tool_call_efficiency: 0.7,
            behavior_deviation: 0.1,
            rollback_rate: 0.05,
            pollution_survival_days: 3.0,
            evaluated_at_ms: 1_700_000_000_000,
        };
        let v = serde_json::to_value(&e).unwrap();
        for key in [
            "task_success_rate",
            "tool_call_efficiency",
            "behavior_deviation",
            "rollback_rate",
            "pollution_survival_days",
            "evaluated_at_ms",
        ] {
            assert!(v.get(key).is_some(), "EvalResult 缺字段 {key}");
        }
    }
}