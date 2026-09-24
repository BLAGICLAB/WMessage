//! R7→B 骨架：三态激活机制 + 4 态 audit（老板 21:38 拍板）
//!
//! 状态机：
//! - S0_observe：policy 空，不调 evaluate，只写 shadow jsonl
//! - S1_suggest：生成候选，等用户确认，不写主记忆
//! - S2_active：走 policy（骨架阶段 placeholder Allow），写主记忆
//!
//! 转移：
//! - S0 → S1：触发阈值命中（calibrating 占位，不自动）
//! - S1 → S2：用户确认
//! - S1 → S0：用户否决 / 候选置信不足
//! - S2 → S0：用户回退 / 误 block 累积 / 漂移
//!
//! 4 态 audit：
//! - no_policy_applied（S0/S1）
//! - Allow（S2）
//! - Block（S2）
//! - Expire（policy 到期）
//!
//! 当前阶段：calibrating（不自动触发 S1）

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::evolution::proposal::{is_reversible, EvolutionProposal, ImpactLevel, ProposalCategory};

// ───────────────────────── 三态 ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationState {
    S0Observe,
    S1Suggest,
    S2Active,
}

impl Default for ActivationState {
    fn default() -> Self {
        Self::S0Observe
    }
}

impl ActivationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S0Observe => "s0_observe",
            Self::S1Suggest => "s1_suggest",
            Self::S2Active => "s2_active",
        }
    }

    /// 该状态下 shadow 是否应写 jsonl（骨架阶段：S0/S1/S2 都写）
    pub fn writes_shadow(self) -> bool {
        true
    }
}

// ───────────────────────── 转移校验 ─────────────────────────

/// 校验转移是否合法
pub fn can_transition(from: ActivationState, to: ActivationState) -> bool {
    matches!(
        (from, to),
        (ActivationState::S0Observe, ActivationState::S1Suggest)
            | (ActivationState::S1Suggest, ActivationState::S2Active)
            | (ActivationState::S1Suggest, ActivationState::S0Observe)
            | (ActivationState::S2Active, ActivationState::S0Observe)
    )
}

// ───────────────────────── 4 态 audit ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditDecision {
    NoPolicyApplied,
    Allow,
    Block,
    Expire,
}

impl AuditDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoPolicyApplied => "no_policy_applied",
            Self::Allow => "allow",
            Self::Block => "block",
            Self::Expire => "expire",
        }
    }
}

// ───────────────────────── 配置（占位）─────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct ActivationConfig {
    /// calibrating | observe | suggest | active
    /// calibrating: 阈值未填，不自动触发 S1（骨架阶段）
    pub mode: String,
    /// 同偏好重复次数（主判据），null = 未校准
    pub min_occurrences: Option<u32>,
    /// 全局兜底 proposal 数，null = 未校准
    pub min_proposals: Option<u32>,
}

impl ActivationConfig {
    pub fn is_calibrating(&self) -> bool {
        self.mode == "calibrating"
    }

    /// 是否应该自动触发 S1→S1（占位阶段总返 false）
    pub fn should_auto_trigger(&self, _current_count: u64) -> bool {
        // 骨架阶段：calibrating 下永不自动触发（老板 21:38 拍板）
        // 其他模式：必须有阈值才能触发（防误 block）
        if self.mode == "calibrating" {
            return false;
        }
        self.mode == "active" && self.min_occurrences.is_some() && self.min_proposals.is_some()
    }
}

// ───────────────────────── bot-config.json loader ──────────────────────────

/// 从 bot-config.json 读 activation 块（lenient）。
/// 缺文件/缺块 = 合法默认模式静默；**存在但坏**（IO 错 / parse 失败 / schema 不匹配）留痕。
pub fn load_config_from_file(path: &Path) -> ActivationConfig {
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("[evolution_activation] 读 {path:?} 失败（{e}），activation 用默认值");
            }
            return ActivationConfig::default();
        }
    };
    let v = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[evolution_activation] {path:?} JSON 解析失败（{e}），activation 用默认值");
            return ActivationConfig::default();
        }
    };
    let Some(activation) = v.get("evolution").and_then(|e| e.get("activation")) else {
        return ActivationConfig::default(); // 缺块 = 未配置，合法静默
    };
    match serde_json::from_value(activation.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "[evolution_activation] {path:?} activation 块 schema 不匹配（{e}），用默认值"
            );
            ActivationConfig::default()
        }
    }
}

/// 从 bot-config.json 读 activation_state（当前状态）。留痕策略同上。
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

// ───────────────────────── 路由决策（pure，可测）─────────────────────────

/// shadow 路由决策（pure function，便于测试）
///
/// 骨架阶段行为：
/// - 不可逆 → Skip
/// - 可逆 + 任何状态 → WriteShadow（4 态 audit 在 wrapper 区分）
pub fn shadow_route(state: ActivationState, p: &EvolutionProposal) -> RouteDecision {
    if !is_reversible(p) {
        return RouteDecision::Skip;
    }
    let _ = state; // 骨架阶段：S0/S1/S2 行为相同（都写 shadow jsonl）
                   // 未来 B 校准：S2 同时写主记忆
    RouteDecision::WriteShadow
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    WriteShadow,
    Skip,
}

// ───────────────────────── S2 评估（v4.1 §5 + §8）─────────────────────────

/// S2 evaluate 占位决策（老板 22:02 spec v4.1）
///
/// 骨架阶段：S2 评估 = is_reversible 反向
/// - 可逆 → Allow（写 shadow + audit allowed）
/// - 不可逆 → Block（跳过 + audit blocked）
///
/// 未来 B 校准阶段：替换为真 policy 规则（保守默认 → 高风险 block + 其他 allow）。
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

/// S2 evaluate 占位（老板 22:10 拍板，选 A 方案）
///
/// 重要：骨架阶段永远返 Allow。is_reversible 已在路由前过（防御纵深），
/// 不可逆 proposal 根本不到这里。所以 evaluate_s2 不会看到「高风险」，Block 不可达。
///
/// Block 变体不是死代码——是为 B 校准阶段真 policy 准备的位置。
/// 当真 policy 加载后（填了 min_occurrences / min_proposals / 高风险规则），
/// evaluate_s2 会查表匹配，返回 Block。
///
/// 当前占位原因：记录「未生效」+「真 policy 未填」，防止误判「S2 已生效」。
pub fn evaluate_s2(_p: &EvolutionProposal) -> S2Decision {
    S2Decision::Allow {
        reason: "占位：真 policy 未填，默认 allow".into(),
    }
}

// ───────────────────────── 状态持久化（v4.1 §12.7）─────────────────────────

/// 保存当前状态到 bot-config.json 的 evolution.activation_state 字段
///
/// 老板 22:02 spec v4.1 §12.7：“运行时切态必须写回，不能只手动改文件”。
/// 写后原文件其它字段保留。
pub fn save_state(state: ActivationState, config_path: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("读 {config_path:?} 失败：{e}"))?;
    let mut v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("解析 {config_path:?} 失败：{e}"))?;

    let evo = v
        .as_object_mut()
        .ok_or_else(|| format!("{config_path:?} 顶层非 object"))?
        .entry("evolution".to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| "evolution 块非 object".to_string())?;

    evo.insert(
        "activation_state".to_string(),
        serde_json::Value::String(state.as_str().to_string()),
    );

    std::fs::write(
        config_path,
        serde_json::to_string_pretty(&v).map_err(|e| format!("序列化：{e}"))?,
    )
    .map_err(|e| format!("写 {config_path:?} 失败：{e}"))?;
    Ok(())
}

// ───────────────────────── 单元测试 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evolution::proposal::{
        Evidence, ImpactLevel, ProposalCategory, ProposalOrigin, ProposalTarget, Suggestion,
    };

    // ─── 状态字符串锁死 ───

    #[test]
    fn activation_state_strings_locked() {
        assert_eq!(ActivationState::S0Observe.as_str(), "s0_observe");
        assert_eq!(ActivationState::S1Suggest.as_str(), "s1_suggest");
        assert_eq!(ActivationState::S2Active.as_str(), "s2_active");
    }

    #[test]
    fn audit_decision_strings_locked() {
        assert_eq!(AuditDecision::NoPolicyApplied.as_str(), "no_policy_applied");
        assert_eq!(AuditDecision::Allow.as_str(), "allow");
        assert_eq!(AuditDecision::Block.as_str(), "block");
        assert_eq!(AuditDecision::Expire.as_str(), "expire");
    }

    // ─── 默认值 ───

    #[test]
    fn default_state_is_s0_observe() {
        assert_eq!(ActivationState::default(), ActivationState::S0Observe);
    }

    #[test]
    fn default_config_calibrating_with_null_thresholds() {
        let cfg = ActivationConfig::default();
        assert_eq!(cfg.mode, "");
        assert_eq!(cfg.min_occurrences, None);
        assert_eq!(cfg.min_proposals, None);
    }

    // ─── 转移校验 ───

    #[test]
    fn transitions_valid() {
        // 老板 21:38 拍板的 4 个转移路径
        assert!(can_transition(
            ActivationState::S0Observe,
            ActivationState::S1Suggest
        ));
        assert!(can_transition(
            ActivationState::S1Suggest,
            ActivationState::S2Active
        ));
        assert!(can_transition(
            ActivationState::S1Suggest,
            ActivationState::S0Observe
        ));
        assert!(can_transition(
            ActivationState::S2Active,
            ActivationState::S0Observe
        ));
    }

    #[test]
    fn invalid_transitions_blocked() {
        // S0 不能直跳 S2（必经 S1）
        assert!(!can_transition(
            ActivationState::S0Observe,
            ActivationState::S2Active
        ));
        // S2 不能跳 S1（必经 S0）
        assert!(!can_transition(
            ActivationState::S2Active,
            ActivationState::S1Suggest
        ));
        // 同状态转移不允许
        assert!(!can_transition(
            ActivationState::S0Observe,
            ActivationState::S0Observe
        ));
    }

    // ─── 占位阶段不自动触发 ───

    #[test]
    fn calibrating_mode_never_auto_triggers() {
        // 老板 21:38：calibrating 下不自动触发 S1
        let cfg = ActivationConfig {
            mode: "calibrating".into(),
            min_occurrences: None,
            min_proposals: None,
        };
        assert!(!cfg.should_auto_trigger(0));
        assert!(!cfg.should_auto_trigger(100));
        assert!(!cfg.should_auto_trigger(10000));
    }

    #[test]
    fn active_mode_requires_thresholds() {
        // 占位校验：即使 mode=active，阈值空也不触发（防止误 block）
        let cfg = ActivationConfig {
            mode: "active".into(),
            min_occurrences: None,
            min_proposals: None,
        };
        // 骨架阶段 should_auto_trigger 返 false（阈值未填）
        assert!(!cfg.should_auto_trigger(0));
    }

    // ─── Loader ───

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

    #[test]
    fn load_config_from_missing_file_returns_default() {
        let cfg = load_config_from_file(std::path::Path::new(
            "/tmp/no-such-bot-config-xyz-98765.json",
        ));
        assert!(cfg.is_calibrating() == false); // 空 mode 不算 calibrating
        assert_eq!(cfg.min_occurrences, None);
    }

    // ─── shadow_route ───

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
    fn route_skip_when_irreversible() {
        let p = mk_proposal("p1", ProposalCategory::ToolSchemaHint, ImpactLevel::Low);
        for state in [
            ActivationState::S0Observe,
            ActivationState::S1Suggest,
            ActivationState::S2Active,
        ] {
            assert_eq!(
                shadow_route(state, &p),
                RouteDecision::Skip,
                "state={}",
                state.as_str()
            );
        }
    }

    #[test]
    fn route_skip_when_high_impact() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        for state in [
            ActivationState::S0Observe,
            ActivationState::S1Suggest,
            ActivationState::S2Active,
        ] {
            assert_eq!(
                shadow_route(state, &p),
                RouteDecision::Skip,
                "state={}",
                state.as_str()
            );
        }
    }

    #[test]
    fn route_write_shadow_for_all_states_when_reversible() {
        // 骨架阶段：3 态都写 shadow jsonl（差异在 audit）
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Medium);
        for state in [
            ActivationState::S0Observe,
            ActivationState::S1Suggest,
            ActivationState::S2Active,
        ] {
            assert_eq!(
                shadow_route(state, &p),
                RouteDecision::WriteShadow,
                "state={}",
                state.as_str()
            );
        }
    }

    // ─── S2 evaluate（老板 22:02 v4.1 §5 + §8）───

    #[test]
    fn evaluate_s2_reversible_returns_allow() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::Medium);
        let d = evaluate_s2(&p);
        assert!(matches!(d, S2Decision::Allow { .. }));
        assert_eq!(d.as_str(), "allow");
        assert!(d.reason().contains("占位"));
    }

    #[test]
    fn evaluate_s2_irreversible_still_returns_allow() {
        // 老板 22:10 选 A：is_reversible 防御纵深，evaluate_s2 占位全 Allow。
        // 即便不可逆 proposal 绕过 is_reversible 到 S2（不可能，但场景下），
        // evaluate_s2 仍返 Allow。Block 留给真 policy。
        let p = mk_proposal("p1", ProposalCategory::ToolSchemaHint, ImpactLevel::Low);
        let d = evaluate_s2(&p);
        assert!(matches!(d, S2Decision::Allow { .. }));
        assert!(d.reason().contains("占位"));
    }

    #[test]
    fn evaluate_s2_high_impact_still_returns_allow() {
        let p = mk_proposal("p1", ProposalCategory::MemoryHint, ImpactLevel::High);
        let d = evaluate_s2(&p);
        assert!(matches!(d, S2Decision::Allow { .. }));
    }

    // ─── 状态持久化（老板 22:02 v4.1 §12.7）───

    #[test]
    fn save_state_preserves_other_fields() {
        let dir = std::env::temp_dir().join(format!(
            "save-state-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        // 初始内容：含 evolution.shadow + evolution.activation + 其它字段
        std::fs::write(
            &p,
            r#"{
  "evolution": {
    "shadow": {"enabled": true},
    "activation": {"mode": "calibrating", "min_occurrences": null, "min_proposals": null},
    "activation_state": "s0_observe"
  },
  "other_top_level_field": "preserved"
}"#,
        )
        .unwrap();
        save_state(ActivationState::S2Active, &p).unwrap();
        // 读回验证
        let raw = std::fs::read_to_string(&p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            v["evolution"]["activation_state"], "s2_active",
            "activation_state 已写为 s2_active"
        );
        assert_eq!(
            v["evolution"]["shadow"]["enabled"], true,
            "shadow.enabled 保留"
        );
        assert_eq!(
            v["evolution"]["activation"]["mode"], "calibrating",
            "activation.mode 保留"
        );
        assert_eq!(v["other_top_level_field"], "preserved", "顶层其它字段保留");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_state_round_trip_through_load() {
        let dir = std::env::temp_dir().join(format!(
            "save-rt-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bot-config.json");
        std::fs::write(&p, r#"{"evolution":{"shadow":{"enabled":true}}}"#).unwrap();
        // 写 S1
        save_state(ActivationState::S1Suggest, &p).unwrap();
        // 读回
        assert_eq!(load_state_from_file(&p), ActivationState::S1Suggest);
        // 再写 S2
        save_state(ActivationState::S2Active, &p).unwrap();
        assert_eq!(load_state_from_file(&p), ActivationState::S2Active);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_state_errors_on_missing_file() {
        let p = std::path::Path::new("/tmp/no-such-bot-config-xyz-save-state-test.json");
        let r = save_state(ActivationState::S1Suggest, p);
        assert!(r.is_err(), "文件不存在应返 Err");
    }
}
