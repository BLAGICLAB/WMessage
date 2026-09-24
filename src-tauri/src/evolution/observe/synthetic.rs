//! R6 B 阶段 · 合成数据生成器
//!
//! 目的：验证 R6 4 个指标的计算逻辑（尺子准不准），不依赖真实 apply 路径。
//!
//! 生成内容（覆盖 30 天窗口）：
//! - ~100 ProposalEntry（分散在 30 天内）
//!   - Pooled: 20 条
//!   - Promoted: 60 条
//!   - Rejected: 15 条
//!   - Expired: 5 条
//! - ~60 ChangeRecord（对应 Promoted 的 proposal）
//!   - Active: 45
//!   - RolledBack: 12
//!   - Approved 但没到 Active: 3
//! - ~60 AppliedRecord（对应 Active 的 ChangeRecord，含 applied_at_ms）
//!
//! 调用方负责写 jsonl 文件；本模块只生成 in-memory 数据。

use crate::eval::metrics::AppliedRecord;
use crate::evolution::candidate::{ProposalEntry, ProposalStatus};
use crate::evolution::change::{ApprovalSource, ChangeRecord, ChangeStatus, EvolutionLayer};
use crate::evolution::proposal::{ImpactLevel, ProposalOrigin, ProposalTarget};

const MS_PER_DAY: i64 = 86_400_000;

/// 合成数据配置
#[derive(Debug, Clone)]
pub struct SyntheticConfig {
    /// 候选总数
    pub proposal_total: usize,
    /// Promoted 占比（0.0-1.0）
    pub promoted_ratio: f64,
    /// Rejected 占比
    pub rejected_ratio: f64,
    /// Expired 占比
    pub expired_ratio: f64,
    /// Active 占比（占 Promoted 的比例）
    pub active_ratio_of_promoted: f64,
    /// RolledBack 占比（占 Promoted 的比例）
    pub rolled_back_ratio_of_promoted: f64,
    /// 观察窗口天数
    pub window_days: i64,
    /// 随机种子（确定性生成）
    pub seed: u64,
}

impl Default for SyntheticConfig {
    fn default() -> Self {
        Self {
            proposal_total: 100,
            promoted_ratio: 0.60,
            rejected_ratio: 0.15,
            expired_ratio: 0.05,
            active_ratio_of_promoted: 0.75,      // 45/60
            rolled_back_ratio_of_promoted: 0.20, // 12/60
            window_days: 30,
            seed: 42,
        }
    }
}

impl SyntheticConfig {
    /// 生成前必过的合法性校验。不合法配置会静默歪斜分布
    /// （负数/NaN/总和>1 的余量概率全落 Pooled）或直接 panic
    /// （window_days=0 → modulo-by-zero；负数 → as u32 回绕），
    /// 打败「验证 R6 指标逻辑」的既定目的——fail-fast 带明确消息。
    pub fn validate(&self) -> Result<(), String> {
        let ratios = [
            ("promoted_ratio", self.promoted_ratio),
            ("rejected_ratio", self.rejected_ratio),
            ("expired_ratio", self.expired_ratio),
            ("active_ratio_of_promoted", self.active_ratio_of_promoted),
            (
                "rolled_back_ratio_of_promoted",
                self.rolled_back_ratio_of_promoted,
            ),
        ];
        for (name, r) in ratios {
            if !r.is_finite() || !(0.0..=1.0).contains(&r) {
                return Err(format!("{name} 必须是 [0,1] 有限值，实测 {r}"));
            }
        }
        let status_sum = self.promoted_ratio + self.rejected_ratio + self.expired_ratio;
        if status_sum > 1.0 {
            return Err(format!(
                "promoted+rejected+expired 占比总和必须 ≤1.0，实测 {status_sum}"
            ));
        }
        let active_sum = self.active_ratio_of_promoted + self.rolled_back_ratio_of_promoted;
        if active_sum > 1.0 {
            return Err(format!(
                "active+rolled_back（占 Promoted）总和必须 ≤1.0，实测 {active_sum}"
            ));
        }
        if self.window_days < 1 {
            return Err(format!(
                "window_days 必须 ≥1（0 会 modulo-by-zero panic，负数 as u32 回绕），实测 {}",
                self.window_days
            ));
        }
        // 上限：window_days * MS_PER_DAY 不得溢出 i64（release 下静默回绕
        // 会把 window_start_ms 算成正/负垃圾值）；as u32 用法也要求 ≤ u32::MAX
        let max_days = (i64::MAX / MS_PER_DAY).min(u32::MAX as i64);
        if self.window_days > max_days {
            return Err(format!(
                "window_days 必须 ≤{max_days}（i64 乘法溢出 / u32 转换上限），实测 {}",
                self.window_days
            ));
        }
        Ok(())
    }
}

/// 合成数据输出
pub struct SyntheticData {
    pub proposals: Vec<ProposalEntry>,
    pub changes: Vec<ChangeRecord>,
    pub applied: Vec<AppliedRecord>,
}

/// 生成合成数据
///
/// 入口先 `cfg.validate()`：不合法配置直接 panic（fail-fast，
/// 唯一生产调用方是 observe_run dev bin，与其中 expect 风格一致）。
pub fn generate(cfg: &SyntheticConfig, now_ms: i64) -> SyntheticData {
    cfg.validate()
        .unwrap_or_else(|e| panic!("SyntheticConfig 非法: {e}"));
    let mut rng = SimpleRng::new(cfg.seed);
    let window_start_ms = now_ms - cfg.window_days * MS_PER_DAY;

    // 1. 生成 ProposalEntry
    let mut proposals = Vec::with_capacity(cfg.proposal_total);
    let mut promoted_proposal_ids = Vec::new();
    for i in 0..cfg.proposal_total {
        let id = format!("synth-{:04}", i);
        let r: f64 = rng.next_f64();
        let status = if r < cfg.promoted_ratio {
            ProposalStatus::Promoted
        } else if r < cfg.promoted_ratio + cfg.rejected_ratio {
            ProposalStatus::Rejected
        } else if r < cfg.promoted_ratio + cfg.rejected_ratio + cfg.expired_ratio {
            ProposalStatus::Expired
        } else {
            ProposalStatus::Pooled
        };
        if status == ProposalStatus::Promoted {
            promoted_proposal_ids.push(id.clone());
        }
        // 均匀分布在窗口内
        let day_offset = rng.next_int(cfg.window_days as u32) as i64;
        let created_at_ms = window_start_ms + day_offset * MS_PER_DAY;
        proposals.push(ProposalEntry {
            proposal_id: id.clone(),
            change_id: format!("chg-{id}"),
            layer: pick_layer(&mut rng),
            impact: pick_impact(&mut rng),
            origin: ProposalOrigin::ConsolidationReflection,
            target: ProposalTarget::MemoryPolicy {
                policy: format!("p-{}", rng.next_int(5)),
            },
            suggestion_text: format!("text-{}", i),
            mem_key: format!("evo:{id}"),
            related_refs: vec![],
            summary: format!("summary-{}", i),
            occurrence_count: rng.next_int(10) + 1,
            window_hours: 24,
            created_at_ms,
            expires_at_ms: created_at_ms + MS_PER_DAY * 14,
            status,
        });
    }

    // 2. 为 Promoted 的 proposal 生成 ChangeRecord + AppliedRecord
    let mut changes = Vec::new();
    let mut applied = Vec::new();
    for id in &promoted_proposal_ids {
        let chg_id = format!("chg-{id}");
        // 随机分布状态
        let r: f64 = rng.next_f64();
        let status = if r < cfg.active_ratio_of_promoted {
            ChangeStatus::Active
        } else if r < cfg.active_ratio_of_promoted + cfg.rolled_back_ratio_of_promoted {
            ChangeStatus::RolledBack
        } else {
            ChangeStatus::Approved // 没到 Active
        };
        let created_at_ms = proposals
            .iter()
            .find(|p| &p.proposal_id == id)
            .map(|p| p.created_at_ms)
            .unwrap_or(now_ms - MS_PER_DAY);
        let change = ChangeRecord {
            change_id: chg_id.clone(),
            parent_id: None,
            schema_version: 1,
            layer: EvolutionLayer::Policy,
            origin: ProposalOrigin::ConsolidationReflection,
            proposal_id: id.clone(),
            target: ProposalTarget::MemoryPolicy {
                policy: "synth".into(),
            },
            suggestion_text: format!("text-{id}"),
            mem_key: format!("evo:{id}"),
            impact: ImpactLevel::Medium,
            eval_before: None,
            eval_after: None,
            status,
            hard_constraint_compliance: true,
            approval_source: ApprovalSource::AutoApplied,
            human_approver: None,
            created_at_ms,
            rolled_back_at: if status == ChangeStatus::RolledBack {
                Some(now_ms - rng.next_int(15) as i64 * MS_PER_DAY)
            } else {
                None
            },
            rollback_reason: None,
        };
        // 只为 Active 生成 AppliedRecord
        if status == ChangeStatus::Active {
            // applied_at 距 now 0-30 天随机
            let applied_days_ago = rng.next_int(cfg.window_days as u32) as i64;
            let applied_at_ms = now_ms - applied_days_ago * MS_PER_DAY;
            applied.push(AppliedRecord {
                proposal_id: id.clone(),
                mem_key: format!("evo:{id}"),
                applied_at_ms,
                impact: "medium".into(),
                summary: format!("s-{id}"),
            });
        }
        changes.push(change);
    }

    SyntheticData {
        proposals,
        changes,
        applied,
    }
}

/// 写合成数据到 jsonl 文件（模拟真实 R6 阶段）
pub fn write_to_files(
    data: &SyntheticData,
    proposals_path: &std::path::Path,
    changes_path: &std::path::Path,
    applied_path: &std::path::Path,
) -> Result<(), String> {
    use std::io::Write;
    let write_jsonl = |path: &std::path::Path, lines: Vec<String>| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(|e| format!("打开 {path:?} 失败：{e}"))?;
        for line in lines {
            writeln!(f, "{line}").map_err(|e| format!("写 {path:?} 失败：{e}"))?;
        }
        Ok(())
    };

    let proposals_lines: Vec<String> = data
        .proposals
        .iter()
        .map(|p| serde_json::to_string(p).map_err(|e| format!("序列化 proposal：{e}")))
        .collect::<Result<_, _>>()?;
    let changes_lines: Vec<String> = data
        .changes
        .iter()
        .map(|c| serde_json::to_string(c).map_err(|e| format!("序列化 change：{e}")))
        .collect::<Result<_, _>>()?;
    let applied_lines: Vec<String> = data
        .applied
        .iter()
        .map(|a| serde_json::to_string(a).map_err(|e| format!("序列化 applied：{e}")))
        .collect::<Result<_, _>>()?;

    write_jsonl(proposals_path, proposals_lines)?;
    write_jsonl(changes_path, changes_lines)?;
    write_jsonl(applied_path, applied_lines)?;
    Ok(())
}

// ─── 简单随机数生成器（确定性 LCG）───

struct SimpleRng(u64);

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        // LCG
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() % 100_000) as f64 / 100_000.0
    }
    fn next_int(&mut self, max_exclusive: u32) -> u32 {
        (self.next_u64() % max_exclusive as u64) as u32
    }
}

fn pick_layer(rng: &mut SimpleRng) -> EvolutionLayer {
    match rng.next_int(6) {
        0 => EvolutionLayer::Parameter,
        1 => EvolutionLayer::Policy,
        2 => EvolutionLayer::PromptHint,
        3 => EvolutionLayer::ToolSchema,
        4 => EvolutionLayer::Skill,
        _ => EvolutionLayer::Code,
    }
}

fn pick_impact(rng: &mut SimpleRng) -> ImpactLevel {
    match rng.next_int(3) {
        0 => ImpactLevel::Low,
        1 => ImpactLevel::Medium,
        _ => ImpactLevel::High,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_default() {
        assert!(SyntheticConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_ratios() {
        let bad = [
            SyntheticConfig {
                promoted_ratio: -0.1,
                ..Default::default()
            },
            SyntheticConfig {
                promoted_ratio: f64::NAN,
                ..Default::default()
            },
            SyntheticConfig {
                promoted_ratio: 0.8,
                rejected_ratio: 0.3,
                ..Default::default()
            }, // 三桶和 0.8+0.3+0.05 > 1
            SyntheticConfig {
                active_ratio_of_promoted: 0.8,
                rolled_back_ratio_of_promoted: 0.3,
                ..Default::default()
            }, // 占 Promoted 和 > 1
        ];
        for cfg in &bad {
            assert!(cfg.validate().is_err(), "应拒：{cfg:?}");
        }
    }

    #[test]
    fn validate_rejects_window_days_out_of_range() {
        // 契约锚在 validator 本体（不只 generate 的 panic 路径）
        for bad_days in [0, -1, i64::MAX] {
            let cfg = SyntheticConfig {
                window_days: bad_days,
                ..Default::default()
            };
            let msg = cfg.validate().expect_err("应拒 window_days 越界");
            assert!(
                msg.contains("window_days"),
                "错误消息应指名字段，实测：{msg}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "SyntheticConfig 非法")]
    fn generate_panics_on_zero_window_days() {
        // window_days=0 → next_int modulo-by-zero；validate fail-fast 拦在入口
        let cfg = SyntheticConfig {
            window_days: 0,
            ..Default::default()
        };
        let _ = generate(&cfg, 1_000);
    }

    #[test]
    fn default_config_generates_100_proposals() {
        let cfg = SyntheticConfig::default();
        let now = cfg.window_days * MS_PER_DAY;
        let data = generate(&cfg, now);
        assert_eq!(data.proposals.len(), 100);
    }

    #[test]
    fn promoted_count_matches_config_ratio() {
        // 60% 默认 promoted_ratio
        let cfg = SyntheticConfig::default();
        let now = cfg.window_days * MS_PER_DAY;
        let data = generate(&cfg, now);
        let promoted = data
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Promoted)
            .count();
        let expected = (cfg.proposal_total as f64 * cfg.promoted_ratio) as usize;
        assert!(
            (promoted as i64 - expected as i64).abs() <= 10,
            "Promoted 数 ≈ {expected}，实测 {promoted}（100 样本 × 60%，标准差 ≈ 5，±10 为 2σ）"
        );
    }

    #[test]
    fn changes_count_matches_promoted() {
        let cfg = SyntheticConfig::default();
        let now = cfg.window_days * MS_PER_DAY;
        let data = generate(&cfg, now);
        let promoted = data
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Promoted)
            .count();
        assert_eq!(data.changes.len(), promoted);
    }

    #[test]
    fn applied_count_matches_active() {
        let cfg = SyntheticConfig::default();
        let now = cfg.window_days * MS_PER_DAY;
        let data = generate(&cfg, now);
        let active = data
            .changes
            .iter()
            .filter(|c| c.status == ChangeStatus::Active)
            .count();
        assert_eq!(data.applied.len(), active);
    }

    #[test]
    fn deterministic_with_same_seed() {
        let cfg = SyntheticConfig::default();
        let now = cfg.window_days * MS_PER_DAY;
        let d1 = generate(&cfg, now);
        let d2 = generate(&cfg, now);
        // proposal_id 序列一致
        for (a, b) in d1.proposals.iter().zip(d2.proposals.iter()) {
            assert_eq!(a.proposal_id, b.proposal_id);
            assert_eq!(a.status, b.status);
        }
    }

    #[test]
    fn distribution_reasonable() {
        // 100 条：60 promoted, 15 rejected, 5 expired, 20 pooled
        let cfg = SyntheticConfig::default();
        let now = cfg.window_days * MS_PER_DAY;
        let data = generate(&cfg, now);
        let pooled = data
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Pooled)
            .count();
        let rejected = data
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Rejected)
            .count();
        let expired = data
            .proposals
            .iter()
            .filter(|p| p.status == ProposalStatus::Expired)
            .count();
        // 允许 ±5 误差（LCG 伪随机分布）
        assert!(
            (pooled as i64 - 20).abs() <= 5,
            "Pooled 数 ≈ 20，实测 {pooled}"
        );
        assert!(
            (rejected as i64 - 15).abs() <= 5,
            "Rejected 数 ≈ 15，实测 {rejected}"
        );
        assert!(
            (expired as i64 - 5).abs() <= 5,
            "Expired 数 ≈ 5，实测 {expired}"
        );
    }

    #[test]
    fn write_and_read_back_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "synth-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("evolution-proposals.jsonl");
        let c = dir.join("evolution-changes.jsonl");
        let a = dir.join("evolution-applied.jsonl");
        let cfg = SyntheticConfig::default();
        let data = generate(&cfg, cfg.window_days * MS_PER_DAY);
        write_to_files(&data, &p, &c, &a).unwrap();
        // 验证读回
        let proposals_text = std::fs::read_to_string(&p).unwrap();
        let changes_text = std::fs::read_to_string(&c).unwrap();
        let applied_text = std::fs::read_to_string(&a).unwrap();
        assert_eq!(proposals_text.lines().count(), 100);
        assert!(!changes_text.is_empty());
        assert!(!applied_text.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
