//! 激活状态：三态枚举 + 当前态读取 + S2 占位评估。
//!
//! 状态机语义：S0_observe（只写 shadow）/ S1_suggest（候选等确认）/
//! S2_active（走 evaluate_s2 裁决写主记忆）。当前恒处 S0。
//!
//! 转移表 / ActivationConfig 阈值 / shadow 路由 / save_state 持久化
//! 零生产调用已删除——现行路径只有「读状态 + 按 S0/S1 与 S2 分流」
//! （`observe::shadow::shadow_apply_for_batch_with_app`）。
//! 真要升档时从 git 历史找回骨架，第二个调用方出现再抽象。

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::evolution::proposal::EvolutionProposal;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationState {
    #[default]
    S0Observe,
    S1Suggest,
    S2Active,
}

impl ActivationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S0Observe => "s0_observe",
            Self::S1Suggest => "s1_suggest",
            Self::S2Active => "s2_active",
        }
    }
}

/// S2 evaluate 占位决策。
///
/// 骨架阶段永远返 Allow：is_reversible 已在路由前过滤（防御纵深），
/// 不可逆 proposal 到不了这里，Block 变体当前不可达——
/// 它是为真 policy 规则（高风险 block + 其他 allow）留的位置，
/// 兼职记录「未生效」防止误判「S2 已自治」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S2Decision {
    Allow { reason: String },
    Block { reason: String },
}

impl S2Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Allow { .. } => "allow",
            Self::Block { .. } => "block",
        }
    }
    pub fn reason(&self) -> &str {
        match self {
            Self::Allow { reason } | Self::Block { reason } => reason,
        }
    }
}

/// S2 evaluate 占位：恒 Allow。真 policy 填充后在此查表分流。
pub fn evaluate_s2(_p: &EvolutionProposal) -> S2Decision {
    S2Decision::Allow {
        reason: "占位：真 policy 未填，默认 allow".into(),
    }
}

/// 从 bot-config.json 读 activation_state（当前状态）。lenient：
/// 缺文件/缺 key/坏值 = 默认 S0 静默；IO 错与 parse 失败留痕。
pub fn load_state_from_file(path: &Path) -> ActivationState {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "[evolution_activation] 读 {path:?} 失败（{e}），activation_state 用默认值"
                );
            }
            return ActivationState::default();
        }
    };
    let v = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "[evolution_activation] {path:?} JSON 解析失败（{e}），activation_state 用默认值"
            );
            return ActivationState::default();
        }
    };
    let Some(state_val) = v.get("evolution").and_then(|e| e.get("activation_state")) else {
        return ActivationState::default(); // 缺 key = 未设置，合法静默
    };
    let Some(s) = state_val.as_str() else {
        eprintln!(
            "[evolution_activation] {path:?} activation_state 非字符串（{state_val}），用默认值"
        );
        return ActivationState::default();
    };
    match s {
        "s0_observe" => ActivationState::S0Observe,
        "s1_suggest" => ActivationState::S1Suggest,
        "s2_active" => ActivationState::S2Active,
        unknown => {
            eprintln!(
                "[evolution_activation] {path:?} 未知 activation_state \"{unknown}\"，用默认值"
            );
            ActivationState::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{
        Evidence, ImpactLevel, ProposalCategory, ProposalOrigin, ProposalTarget, Suggestion,
    };

    #[test]
    fn activation_state_strings_locked() {
        assert_eq!(ActivationState::S0Observe.as_str(), "s0_observe");
        assert_eq!(ActivationState::S1Suggest.as_str(), "s1_suggest");
        assert_eq!(ActivationState::S2Active.as_str(), "s2_active");
    }

    #[test]
    fn default_state_is_s0_observe() {
        assert_eq!(ActivationState::default(), ActivationState::S0Observe);
    }

    #[test]
    fn load_state_from_missing_file_returns_s0() {
        let s = load_state_from_file(std::path::Path::new(
            "/tmp/no-such-bot-config-xyz-98765.json",
        ));
        assert_eq!(s, ActivationState::S0Observe);
    }

    #[test]
    fn load_state_from_partial_json() {
        let dir = std::env::temp_dir().join(format!(
            "act-state-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"evolution":{"activation_state":"s2_active"}}"#).unwrap();
        assert_eq!(load_state_from_file(&p), ActivationState::S2Active);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn mk_proposal(id: &str, cat: ProposalCategory, impact: ImpactLevel) -> EvolutionProposal {
        EvolutionProposal {
            proposal_id: id.into(),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: cat,
            target: ProposalTarget::MemoryPolicy {
                policy: "test".into(),
            },
            impact,
            evidence: Evidence {
                summary: format!("s-{id}"),
                occurrence_count: 1,
                window_hours: 24,
                related_refs: vec![],
            },
            suggestion: Suggestion {
                text: format!("text-{id}"),
                structured_patch: None,
            },
        }
    }

    #[test]
    fn evaluate_s2_always_allows_in_skeleton_phase() {
        // 占位恒 Allow：可逆/不可逆/高风险同等待遇（is_reversible 在路由前已滤）
        for (cat, impact) in [
            (ProposalCategory::MemoryHint, ImpactLevel::Medium),
            (ProposalCategory::ToolSchemaHint, ImpactLevel::Low),
            (ProposalCategory::MemoryHint, ImpactLevel::High),
        ] {
            let p = mk_proposal("p1", cat, impact);
            let d = evaluate_s2(&p);
            assert!(matches!(d, S2Decision::Allow { .. }));
            assert_eq!(d.as_str(), "allow");
            assert!(d.reason().contains("占位"));
        }
    }
}
