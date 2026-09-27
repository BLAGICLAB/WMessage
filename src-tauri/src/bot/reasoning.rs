//! 推理强度（RE-1）：后台设置页全局默认 + 挂件聊天按会话覆盖（不回写）。
//!
//! 前端/配置只存**抽象档位**（off / low / medium / high，默认 medium），
//! 发请求前由本模块按**具体模型族**映射到各 provider 的线上参数——
//! 每个模型的档位粒度和参数名都不同（按各家 API 文档逐个设计，不是全局一套）：
//!
//! | 模型族（model 前缀匹配） | 关闭 | 低 | 中 | 高 |
//! |---|---|---|---|---|
//! | `glm-5.3+`（OpenAI 兼容） | 不发字段（5.3 官方已移除 thinking disabled） | `reasoning_effort:"low"` | `reasoning_effort:"high"`（无 medium 档，就近） | `reasoning_effort:"max"` |
//! | `glm-5.2`（OpenAI 兼容） | `thinking:{type:"disabled"}` | `reasoning_effort:"low"` | `reasoning_effort:"high"`（5.2 仅 low/high 两档） | `reasoning_effort:"high"` |
//! | `glm-4.5/4.6/5.0/5.1` 及其余 glm | `thinking:{type:"disabled"}` | `thinking:{type:"enabled"}` | 同低（二档模型） | 同低 |
//! | `gpt-5*`（OpenAI 兼容） | `reasoning_effort:"minimal"` | `"low"` | `"medium"` | `"high"` |
//! | `o1*/o3*/o4*`（OpenAI 兼容） | 不发字段（o 系无关闭档） | `"low"` | `"medium"` | `"high"` |
//! | Anthropic 分支（claude*） | 不发 thinking 块 | budget 2048 | 4096 | 6144 |
//! | 其余（deepseek/qwen/kimi/gpt-4o 等未知族） | 不发字段 | 不发字段（保守：未知模型不发未证实字段，防 400） | 同左 | 同左 |
//!
//! Anthropic budget 基准 2048/4096/6144（老板拍板），再按模型收敛：
//! budget 必须 ≥1024 且 < max_tokens（Anthropic 硬约束），且不超过该模型的
//! 思维链上限（`anthropic_budget_cap`，未知 claude 型号保守取 8192）。
//!
//! 文档来源：docs.bigmodel.cn（GLM thinking / reasoning_effort）、
//! OpenAI reasoning 文档、Anthropic extended thinking 文档。
//! 新接入一个模型 = 在 [`resolve`] 加一个 match 臂 + 补单测。

/// 抽象档位：配置文件与前端只认这四档（None/非法值 = Medium 默认）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortLevel {
    Off,
    Low,
    Medium,
    High,
}

impl EffortLevel {
    /// 配置字符串 → 档位。None / "medium" / 非法值一律回 Medium
    /// （与 ApiProvider::from_cfg 的防御回退同风格；默认「中」老板拍板）。
    pub fn from_cfg(v: Option<&str>) -> Self {
        match v.map(|s| s.trim()) {
            Some("off") => EffortLevel::Off,
            Some("low") => EffortLevel::Low,
            Some("high") => EffortLevel::High,
            _ => EffortLevel::Medium,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EffortLevel::Off => "off",
            EffortLevel::Low => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High => "high",
        }
    }
}

/// 档位解析结果：请求体注入的唯一依据（bot_model_loop 按变体注入对应字段）。
#[derive(Debug, Clone, PartialEq)]
pub enum ReasoningWire {
    /// 不发送任何推理字段（模型不支持 / 该档位无对应线上参数）
    None,
    /// OpenAI 兼容：`"reasoning_effort": "<effort>"`（GLM-5.2+ 的 low/high/max 也走这）
    OpenAiEffort(String),
    /// GLM 思考开关：`"thinking": {"type": "enabled"|"disabled"}`（4.5/4.6/5.0/5.1 二档模型）
    GlmThinking(bool),
    /// Anthropic：`"thinking": {"type": "enabled", "budget_tokens": <budget>}`
    AnthropicBudget(u32),
}

impl ReasoningWire {
    /// 审计日志用的人读描述（llm.request 事件的 reasoning kv）
    pub fn describe(&self) -> String {
        match self {
            ReasoningWire::None => "none".into(),
            ReasoningWire::OpenAiEffort(e) => format!("reasoning_effort={e}"),
            ReasoningWire::GlmThinking(true) => "thinking=enabled".into(),
            ReasoningWire::GlmThinking(false) => "thinking=disabled".into(),
            ReasoningWire::AnthropicBudget(b) => format!("thinking.budget_tokens={b}"),
        }
    }
}

/// GLM 版本号解析：`glm-5.3-flash` → (5, 3)；解析失败 → None。
/// 容忍 `-flash/-air/-plus` 等后缀与 `glm5.3` 无连字符写法。
fn glm_version(model_lower: &str) -> Option<(u32, u32)> {
    let rest = model_lower.strip_prefix("glm")?;
    let rest = rest.trim_start_matches(['-', 'v']);
    let (major, rest) = take_digits(rest);
    let major = major?;
    let minor = match rest.strip_prefix('.') {
        Some(r) => take_digits(r).0.unwrap_or(0),
        None => 0,
    };
    Some((major, minor))
}

/// 连续读一段十进制数字：返回 (数值, 剩余串)
fn take_digits(s: &str) -> (Option<u32>, &str) {
    let num: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    (num.parse().ok(), &s[num.len()..])
}

/// Anthropic 思维链 budget 上限：各型号文档值（token）；未知型号保守 8192。
/// （sonnet-4-5 64k / opus-4 系 32k / 3.7-sonnet 128k；实际还要被请求的
/// max_tokens-1024 再夹一次——budget < max_tokens 是 Anthropic 硬约束。）
fn anthropic_budget_cap(model_lower: &str) -> u32 {
    if model_lower.contains("sonnet-4-5") || model_lower.contains("sonnet-4.5") {
        64_000
    } else if model_lower.contains("opus-4") {
        32_000
    } else if model_lower.contains("3-7-sonnet") || model_lower.contains("3.7-sonnet") {
        128_000
    } else {
        8_192
    }
}

/// Anthropic budget 三档基准（老板拍板）：低 / 中 / 高。
const ANTHROPIC_BUDGET: [u32; 3] = [2_048, 4_096, 6_144];

/// 档位 → Anthropic budget：按基准档取值，再被「模型上限」与「max_tokens-1024」
/// 夹紧；夹完 <1024（max_tokens 太小放不下思维链）→ 放弃注入（None）。
fn anthropic_budget(level: EffortLevel, model: &str, max_tokens: u32) -> ReasoningWire {
    let base = match level {
        EffortLevel::Off => return ReasoningWire::None,
        EffortLevel::Low => ANTHROPIC_BUDGET[0],
        EffortLevel::Medium => ANTHROPIC_BUDGET[1],
        EffortLevel::High => ANTHROPIC_BUDGET[2],
    };
    let cap = anthropic_budget_cap(&model.to_lowercase());
    // Anthropic 硬约束：1024 ≤ budget_tokens < max_tokens
    let budget = base.min(cap).min(max_tokens.saturating_sub(1024));
    if budget < 1024 {
        ReasoningWire::None
    } else {
        ReasoningWire::AnthropicBudget(budget)
    }
}

/// 档位 + 模型 + 协议 → 线上参数。所有模型差异集中在这一个函数
/// （新模型 = 加 match 臂，见模块头映射表）。
pub fn resolve(
    provider: crate::bot::config::types::ApiProvider,
    model: &str,
    level: EffortLevel,
    max_tokens: u32,
) -> ReasoningWire {
    let m = model.trim().to_lowercase();
    match provider {
        crate::bot::config::types::ApiProvider::Anthropic => {
            anthropic_budget(level, &m, max_tokens)
        }
        crate::bot::config::types::ApiProvider::Openai => {
            if m.starts_with("glm") {
                match glm_version(&m) {
                    // GLM-5.3+：reasoning_effort low/high/max，无 medium，官方已移除 disabled
                    Some((major, minor)) if major > 5 || (major == 5 && minor >= 3) => {
                        match level {
                            EffortLevel::Off => ReasoningWire::None,
                            EffortLevel::Low => ReasoningWire::OpenAiEffort("low".into()),
                            EffortLevel::Medium => ReasoningWire::OpenAiEffort("high".into()),
                            EffortLevel::High => ReasoningWire::OpenAiEffort("max".into()),
                        }
                    }
                    // GLM-5.2：effort low/high + thinking disabled 共存（两者不能同时发 disabled+effort）
                    Some((5, 2)) => match level {
                        EffortLevel::Off => ReasoningWire::GlmThinking(false),
                        EffortLevel::Low => ReasoningWire::OpenAiEffort("low".into()),
                        EffortLevel::Medium | EffortLevel::High => {
                            ReasoningWire::OpenAiEffort("high".into())
                        }
                    },
                    // GLM-4.5/4.6/5.0/5.1 及版本号异常的 glm*：二档思考开关
                    _ => match level {
                        EffortLevel::Off => ReasoningWire::GlmThinking(false),
                        _ => ReasoningWire::GlmThinking(true),
                    },
                }
            } else if m.starts_with("gpt-5") {
                // gpt-5 系：有 minimal 档，关闭 ≈ minimal
                match level {
                    EffortLevel::Off => ReasoningWire::OpenAiEffort("minimal".into()),
                    EffortLevel::Low => ReasoningWire::OpenAiEffort("low".into()),
                    EffortLevel::Medium => ReasoningWire::OpenAiEffort("medium".into()),
                    EffortLevel::High => ReasoningWire::OpenAiEffort("high".into()),
                }
            } else if m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
                // o 系：无关闭档，不发 = 默认 medium
                match level {
                    EffortLevel::Off => ReasoningWire::None,
                    EffortLevel::Low => ReasoningWire::OpenAiEffort("low".into()),
                    EffortLevel::Medium => ReasoningWire::OpenAiEffort("medium".into()),
                    EffortLevel::High => ReasoningWire::OpenAiEffort("high".into()),
                }
            } else {
                // 未知模型族（deepseek / qwen / kimi / gpt-4o / 自建网关…）：
                // 保守不发未证实字段，防严格网关 400；审计里可见 none 便于发现漏配
                ReasoningWire::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::config::types::ApiProvider;

    const MT: u32 = 8192;

    #[test]
    fn from_cfg_defaults_to_medium() {
        assert_eq!(EffortLevel::from_cfg(None), EffortLevel::Medium);
        assert_eq!(EffortLevel::from_cfg(Some("medium")), EffortLevel::Medium);
        assert_eq!(EffortLevel::from_cfg(Some("垃圾值")), EffortLevel::Medium);
        assert_eq!(EffortLevel::from_cfg(Some(" off ")), EffortLevel::Off);
    }

    #[test]
    fn glm_5_3_uses_effort_low_high_max() {
        let p = ApiProvider::Openai;
        assert_eq!(
            resolve(p, "glm-5.3-flash", EffortLevel::Off, MT),
            ReasoningWire::None
        );
        assert_eq!(
            resolve(p, "glm-5.3-flash", EffortLevel::Low, MT),
            ReasoningWire::OpenAiEffort("low".into())
        );
        // 无 medium 档 → 就近 high
        assert_eq!(
            resolve(p, "glm-5.3-flash", EffortLevel::Medium, MT),
            ReasoningWire::OpenAiEffort("high".into())
        );
        assert_eq!(
            resolve(p, "glm-5.3", EffortLevel::High, MT),
            ReasoningWire::OpenAiEffort("max".into())
        );
    }

    #[test]
    fn glm_5_2_off_disables_thinking() {
        let p = ApiProvider::Openai;
        assert_eq!(
            resolve(p, "glm-5.2", EffortLevel::Off, MT),
            ReasoningWire::GlmThinking(false)
        );
        assert_eq!(
            resolve(p, "glm-5.2", EffortLevel::Low, MT),
            ReasoningWire::OpenAiEffort("low".into())
        );
    }

    #[test]
    fn glm_older_uses_thinking_switch() {
        let p = ApiProvider::Openai;
        assert_eq!(
            resolve(p, "glm-4.6", EffortLevel::Off, MT),
            ReasoningWire::GlmThinking(false)
        );
        assert_eq!(
            resolve(p, "glm-4.5-air", EffortLevel::High, MT),
            ReasoningWire::GlmThinking(true)
        );
    }

    #[test]
    fn openai_families() {
        let p = ApiProvider::Openai;
        // gpt-5：关闭 = minimal
        assert_eq!(
            resolve(p, "gpt-5", EffortLevel::Off, MT),
            ReasoningWire::OpenAiEffort("minimal".into())
        );
        assert_eq!(
            resolve(p, "gpt-5-mini", EffortLevel::Medium, MT),
            ReasoningWire::OpenAiEffort("medium".into())
        );
        // o 系：无关闭档
        assert_eq!(
            resolve(p, "o3-mini", EffortLevel::Off, MT),
            ReasoningWire::None
        );
        assert_eq!(
            resolve(p, "o4-mini", EffortLevel::High, MT),
            ReasoningWire::OpenAiEffort("high".into())
        );
    }

    #[test]
    fn unknown_model_never_sends_fields() {
        let p = ApiProvider::Openai;
        for lv in [
            EffortLevel::Off,
            EffortLevel::Low,
            EffortLevel::Medium,
            EffortLevel::High,
        ] {
            assert_eq!(resolve(p, "deepseek-reasoner", lv, MT), ReasoningWire::None);
            assert_eq!(resolve(p, "gpt-4o", lv, MT), ReasoningWire::None);
            assert_eq!(resolve(p, "mystery-model", lv, MT), ReasoningWire::None);
        }
    }

    #[test]
    fn anthropic_budget_levels_and_clamp() {
        let p = ApiProvider::Anthropic;
        assert_eq!(
            resolve(p, "claude-sonnet-4-5", EffortLevel::Off, MT),
            ReasoningWire::None
        );
        assert_eq!(
            resolve(p, "claude-sonnet-4-5", EffortLevel::Low, MT),
            ReasoningWire::AnthropicBudget(2_048)
        );
        assert_eq!(
            resolve(p, "claude-sonnet-4-5", EffortLevel::Medium, MT),
            ReasoningWire::AnthropicBudget(4_096)
        );
        assert_eq!(
            resolve(p, "claude-sonnet-4-5", EffortLevel::High, MT),
            ReasoningWire::AnthropicBudget(6_144)
        );
        // budget < max_tokens 硬约束：max_tokens 2048 → low 基准 2048 被夹到 1024
        assert_eq!(
            resolve(p, "claude-3-7-sonnet", EffortLevel::Low, 2_048),
            ReasoningWire::AnthropicBudget(1_024)
        );
        // max_tokens 连 1024 都放不下 → 放弃注入
        assert_eq!(
            resolve(p, "claude-opus-4", EffortLevel::High, 1_500),
            ReasoningWire::None
        );
        // 模型上限收敛：3.7-sonnet 上限 128k 不会超过基准；未知型号 cap 8192 不影响默认档
        assert_eq!(
            resolve(p, "claude-mystery", EffortLevel::High, 200_000),
            ReasoningWire::AnthropicBudget(6_144)
        );
    }

    #[test]
    fn describe_is_human_readable() {
        assert_eq!(ReasoningWire::None.describe(), "none");
        assert_eq!(
            ReasoningWire::OpenAiEffort("max".into()).describe(),
            "reasoning_effort=max"
        );
        assert_eq!(
            ReasoningWire::GlmThinking(false).describe(),
            "thinking=disabled"
        );
        assert_eq!(
            ReasoningWire::AnthropicBudget(4_096).describe(),
            "thinking.budget_tokens=4096"
        );
    }
}
