//! 自进化提案的数据结构。
//!
//! Phase 1 仅通过 audit 写出；不落盘、不发应用事件、不触发任何应用路径。
//!
//! `proposal_id` 是短 hash（8 字节 hex，16 字符），输入由三部分组成：
//!   1. `category`（如 `"memory_hint"`）
//!   2. `target` 的稳定 tag（如 `"tool_schema:run_python"`）
//!   3. `evidence.summary` 经归一化（小写 / 去标点 / 数字→N / 空白合并）
//!
//! 归一化目的：同一问题反复出现时 id 相同，audit 端可去重，
//! 不依赖 LLM 措辞的微小差异。

use serde::{Deserialize, Serialize};

/// 一条自进化提案（写 audit 用）。
///
/// `created_at_ms` 同样用 `i64` epoch ms（见 `trace.rs` 注释——chrono serde feature
/// 未启用，无法 derive `DateTime<Utc>` 的 Serialize/Deserialize）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionProposal {
    /// 短 hash id（见 `proposal_id` 函数）。
    pub proposal_id: String,
    pub created_at_ms: i64,
    pub origin: ProposalOrigin,
    pub category: ProposalCategory,
    pub target: ProposalTarget,
    pub impact: ImpactLevel,
    pub evidence: Evidence,
    pub suggestion: Suggestion,
}

/// 提案来源（Phase 1 仅 ConsolidationReflection 用得到；其余两种占位留 Phase 2 扩展）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOrigin {
    ConsolidationReflection,
    SchedulerAudit,
    UserTriggered,
}

/// 提案类别。Phase 1 三类 consolidate ops 全部映射到 `MemoryHint`（最安全分类）——
/// 见 `derive::derive_proposals` 注释。Phase 2 可根据一周数据决定是否拆分到
/// `ToolSchemaHint` / `PromptHint` 等。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalCategory {
    MemoryHint,
    PromptHint,
    ToolSchemaHint,
    SkillHint,
}

impl ProposalCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MemoryHint => "memory_hint",
            Self::PromptHint => "prompt_hint",
            Self::ToolSchemaHint => "tool_schema_hint",
            Self::SkillHint => "skill_hint",
        }
    }
}

/// 提案目标。`tag()` 返回稳定的 `"kind:name"` 短字符串，用于 proposal_id 归一化。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalTarget {
    PromptSection { name: String },
    ToolSchema { tool_name: String },
    SkillDsl { skill_name: String },
    MemoryPolicy { policy: String },
}

impl ProposalTarget {
    pub fn tag(&self) -> String {
        match self {
            Self::PromptSection { name } => format!("prompt_section:{name}"),
            Self::ToolSchema { tool_name } => format!("tool_schema:{tool_name}"),
            Self::SkillDsl { skill_name } => format!("skill_dsl:{skill_name}"),
            Self::MemoryPolicy { policy } => format!("memory_policy:{policy}"),
        }
    }
}

/// 影响等级（决定后续 Phase 2 评审优先级）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImpactLevel {
    Low,
    Medium,
    High,
}

impl ImpactLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// 证据：触发本次提案的事实摘要 + 计数 + 时间窗口 + 关联引用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    /// 一句话描述发现（自然语言，**不**存原始堆栈 / 文件路径 / 用户输入）。
    pub summary: String,
    /// 发生次数。Phase 1 语义：单次反思的 ops 内 ids 数（占位语义，非跨次累计）——
    /// 见 HANDOFF.md，避免一周后误读为「同类失败出现 N 次」。
    pub occurrence_count: u32,
    /// 时间窗口（小时）。Phase 1 固定 24h。
    pub window_hours: u32,
    /// 关联引用：trace_id / tool_name / memory id（**不**存原始路径 / 对话内容）。
    pub related_refs: Vec<String>,
}

/// 建议文本 + 可选结构化 patch（Phase 1 必为 None；结构化 diff 是 Phase 2 的事）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub structured_patch: Option<String>,
}

// ───────────────────────── proposal_id 归一化 + 短 hash ─────────────────────────

/// 归一化（用于 proposal_id 输入）：
/// - 大写 → 小写
/// - 所有数字 → `N`
/// - 非字母数字字符 → 空白
/// - 连续空白合并为单个空格
/// - 前后空白裁掉
///
/// 目的：让 LLM 措辞差异（"工具 A 失败" vs "工具A失败。" vs "tool A failed 3 times"）
/// 不影响 id 稳定性。
pub fn normalize_for_hash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        let mapped = if c.is_ascii_digit() {
            'N'
        } else if c.is_alphanumeric() {
            c.to_ascii_lowercase()
        } else {
            ' '
        };
        if mapped == ' ' {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(mapped);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// 计算 proposal_id。输入三段拼成 `"<category>|<target_tag>|<normalized_summary>"`。
pub fn proposal_id(category: ProposalCategory, target: &ProposalTarget, summary: &str) -> String {
    let normalized = normalize_for_hash(summary);
    let raw = format!("{}|{}|{}", category.as_str(), target.tag(), normalized);
    short_hash(&raw)
}

/// R7→A 简化（老板 21:10 拍板）：判断 proposal 是否可逆（pure，no IO）
///
/// 规则（MVP）：
/// - ToolSchemaHint：不可逆（改 schema 可能破坏现有 tool 调用）
/// - High impact：不可逆（高风险需人工确认）
/// - 其他：可逆
///
/// shadow 路径和 activation 路由都依赖它；放 proposal.rs 是其本体属性。
pub fn is_reversible(p: &EvolutionProposal) -> bool {
    if matches!(p.category, ProposalCategory::ToolSchemaHint) {
        return false;
    }
    if matches!(p.impact, ImpactLevel::High) {
        return false;
    }
    true
}

/// 8 字节（16 hex 字符）短 hash。沿用 `trace::compute_trace_id` 的实现模式。
pub(crate) fn short_hash(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    let bytes = h.finish().to_be_bytes();
    let mut out = String::with_capacity(16);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> ProposalCategory {
        ProposalCategory::MemoryHint
    }

    fn target_tool(tool: &str) -> ProposalTarget {
        ProposalTarget::ToolSchema {
            tool_name: tool.into(),
        }
    }

    // ─── normalize_for_hash ───

    #[test]
    fn normalize_lowercases_and_collapses_whitespace() {
        assert_eq!(
            normalize_for_hash("Tool A   Failed"),
            "tool a failed",
            "多余空白合并、大写转小写"
        );
    }

    #[test]
    fn normalize_replaces_digits_with_n() {
        // 数字一律变 'N'（uppercase 占位符，读起来与字母区分明显）
        assert_eq!(
            normalize_for_hash("error 502 / 3 times"),
            "error NNN N times"
        );
        assert_eq!(normalize_for_hash("a1b22c333"), "aNbNNcNNN");
    }

    #[test]
    fn normalize_strips_punctuation_only() {
        // 标点（中日英文都算）全部 strip；CJK 字母数字保留
        // 输入: 工 具 - A 空 失 败 ： 超 时 空 ( t i m e o u t ) !
        // 输出: 工 具 a 失 败 超 时 timeout
        // （-、：、(、)、!、空格等标点去掉；CJK 字母数字保留；连续空白合并）
        assert_eq!(
            normalize_for_hash("工具-A 失败：超时 (timeout)!"),
            "工具 a 失败 超时 timeout"
        );
    }

    #[test]
    fn normalize_strips_edge_whitespace() {
        assert_eq!(normalize_for_hash("   hello   "), "hello");
        assert_eq!(normalize_for_hash("\n\t  hi\r\n"), "hi");
    }

    #[test]
    fn normalize_unicode_letters_pass_through() {
        // 中文是字母数字字符（unicode alnum），保留——这意味着 proposal_id
        // 对中文摘要天然 idempotent（同中文表述 → 同归一化）
        assert_eq!(normalize_for_hash("工具调用失败"), "工具调用失败");
    }

    // ─── proposal_id 稳定性 ───

    #[test]
    fn proposal_id_is_deterministic() {
        let p1 = proposal_id(cat(), &target_tool("run_python"), "Tool A failed");
        let p2 = proposal_id(cat(), &target_tool("run_python"), "Tool A failed");
        assert_eq!(p1, p2);
        assert_eq!(p1.len(), 16, "proposal_id 必须是 16 hex 字符");
    }

    #[test]
    fn proposal_id_normalizes_evidence() {
        // 同一问题不同措辞 + 数字不同 → id 应相同（数字归一化为 N）
        let p1 = proposal_id(cat(), &target_tool("run_python"), "Tool A failed 3 times");
        let p2 = proposal_id(cat(), &target_tool("run_python"), "tool A failed 5 times");
        assert_eq!(p1, p2, "数字归一化后 id 应相同");
        // 标点差异不影响：同样含「failed N times」语义，仅标点/大小写不同
        let p3 = proposal_id(cat(), &target_tool("run_python"), "tool-A failed 7 times.");
        assert_eq!(p1, p3, "标点/大小写差异不影响 id（语义结构相同）");
    }

    #[test]
    fn proposal_id_differs_across_categories() {
        let p1 = proposal_id(ProposalCategory::MemoryHint, &target_tool("x"), "same");
        let p2 = proposal_id(ProposalCategory::ToolSchemaHint, &target_tool("x"), "same");
        assert_ne!(p1, p2);
    }

    #[test]
    fn proposal_id_differs_across_targets() {
        let p1 = proposal_id(cat(), &target_tool("a"), "same");
        let p2 = proposal_id(cat(), &target_tool("b"), "same");
        assert_ne!(p1, p2);
    }

    #[test]
    fn proposal_id_stable_across_unicode_normalization() {
        // 中文大小写无概念，所以两次相同输入 → 相同 id
        let p1 = proposal_id(cat(), &target_tool("run_python"), "工具调用失败");
        let p2 = proposal_id(cat(), &target_tool("run_python"), "工具调用失败");
        assert_eq!(p1, p2);
    }

    // ─── 字段集锁死 ───

    #[test]
    fn evolution_proposal_carries_minimal_fields() {
        // 锁死字段集——后续加字段会破坏 audit 解析，强制评审
        let p = EvolutionProposal {
            proposal_id: "abc".into(),
            created_at_ms: 1_700_000_000_000,
            origin: ProposalOrigin::ConsolidationReflection,
            category: ProposalCategory::MemoryHint,
            target: ProposalTarget::ToolSchema {
                tool_name: "x".into(),
            },
            impact: ImpactLevel::Medium,
            evidence: Evidence {
                summary: "s".into(),
                occurrence_count: 3,
                window_hours: 24,
                related_refs: vec!["r1".into()],
            },
            suggestion: Suggestion {
                text: "t".into(),
                structured_patch: None,
            },
        };
        let v = serde_json::to_value(&p).unwrap();
        for key in [
            "proposal_id",
            "created_at_ms",
            "origin",
            "category",
            "target",
            "impact",
            "evidence",
            "suggestion",
        ] {
            assert!(v.get(key).is_some(), "EvolutionProposal 缺字段 {key}");
        }
    }

    #[test]
    fn evidence_carries_minimal_fields() {
        let e = Evidence {
            summary: "x".into(),
            occurrence_count: 1,
            window_hours: 24,
            related_refs: vec![],
        };
        let v = serde_json::to_value(&e).unwrap();
        for key in [
            "summary",
            "occurrence_count",
            "window_hours",
            "related_refs",
        ] {
            assert!(v.get(key).is_some(), "Evidence 缺字段 {key}");
        }
    }

    #[test]
    fn target_tag_format_is_stable() {
        // 锁死 tag 形态——tag 是 proposal_id 输入的一部分，变了就破坏去重
        assert_eq!(
            ProposalTarget::PromptSection {
                name: "system".into()
            }
            .tag(),
            "prompt_section:system"
        );
        assert_eq!(
            ProposalTarget::ToolSchema {
                tool_name: "run_python".into()
            }
            .tag(),
            "tool_schema:run_python"
        );
        assert_eq!(
            ProposalTarget::SkillDsl {
                skill_name: "s".into()
            }
            .tag(),
            "skill_dsl:s"
        );
        assert_eq!(
            ProposalTarget::MemoryPolicy {
                policy: "dedup".into()
            }
            .tag(),
            "memory_policy:dedup"
        );
    }

    #[test]
    fn category_as_str_is_stable() {
        // 锁死：审计 grep 的稳定字符串
        assert_eq!(ProposalCategory::MemoryHint.as_str(), "memory_hint");
        assert_eq!(ProposalCategory::PromptHint.as_str(), "prompt_hint");
        assert_eq!(
            ProposalCategory::ToolSchemaHint.as_str(),
            "tool_schema_hint"
        );
        assert_eq!(ProposalCategory::SkillHint.as_str(), "skill_hint");
    }

    #[test]
    fn impact_as_str_is_stable() {
        assert_eq!(ImpactLevel::Low.as_str(), "low");
        assert_eq!(ImpactLevel::Medium.as_str(), "medium");
        assert_eq!(ImpactLevel::High.as_str(), "high");
    }
}
