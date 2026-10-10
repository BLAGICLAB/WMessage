//! 工作流拆解前澄清（设计 docs/WORKFLOW-CLARIFY-AUDIT-DESIGN-2026-10-07.md §3.4）： audit-ok
//! goal + 附件 → 一次性 LLM → ≤3 个结构化问题（空 = 信息足够直接拆）。
//!
//! 红线：
//! - **每个问题必须自带假设（default）**——"跳过/未答"恒有合理默认，与执行提问同一原则
//! - 澄清是增强不是闸门：调用失败/校验连续不过 → 返回空 questions（outcome=degraded 审计），
//!   前端无缝直拆，绝不因澄清环节挡住拆解
//! - 只问一轮，不追问；不问 goal 已写明的事
//! - 校验超限**截断**而非整体失败（结构坏才失败）——宁可降级，不炸主流程

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

use crate::db::workflow::MAX_WORKFLOW_GOAL;

/// 最多 3 问（问多了就是打扰——用户答不完等于没问）
pub const MAX_CLARIFY_QUESTIONS: usize = 3;
pub const MAX_CLARIFY_QUESTION_CHARS: usize = 100;
pub const MAX_CLARIFY_WHY_CHARS: usize = 60;
pub const MAX_CLARIFY_OPTIONS: usize = 4;
pub const MAX_CLARIFY_OPTION_CHARS: usize = 20;
pub const MAX_CLARIFY_DEFAULT_CHARS: usize = 100;

/// 输出契约段（代码硬拼，模型可见、用户不可改；同 decompose §6.2 两段式）
const CLARIFY_CONTRACT: &str = r#"

【输出格式硬性要求——必须遵守，优先级高于上文一切指引】
只输出一个 JSON 对象，不要输出任何解释、前后缀或 Markdown 代码围栏，形如：
{"questions":[{"id":"q1","question":"问题文本","why":"为什么问这句","options":["选项A","选项B"],"default":"选项A"}]}
- questions：信息足够时输出 []（直接拆解），不要为了问而问；最多 3 个问题
- **每个问题必须给 default（你的推荐假设）**——用户不回答时按假设继续，给不出假设的问题不许问
- question ≤100 字；why ≤60 字、一句话说明为什么要问；options ≤4 个、每个 ≤20 字（开放问题 options 给 []）
- 只问影响拆解结构或产出质量、且从目标与附件里推断不出的信息；目标里已写明的事不许问
- 只问这一轮，拆解阶段不会再次追问
示例：
{"questions":[{"id":"q1","question":"文案投放哪个平台？","why":"各平台字数与排版规范差异大","options":["微信公众号","知乎","两者都"],"default":"微信公众号"}]}"#;

/// 默认澄清指引（角色设定；契约段独立硬拼，本段被掏空不影响契约）
const DEFAULT_CLARIFY_GUIDANCE: &str = r#"你是工作流拆解前的澄清助手。判断用户的目标在拆解前是否缺关键信息。
原则：宁可少问；只问影响任务卡怎么拆、产出长什么样的关键决策；信息足够就输出空数组。"#;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClarifyQuestion {
    #[serde(default)]
    pub id: String,
    pub question: String,
    #[serde(default)]
    pub why: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    /// 推荐假设（红线：缺失时由校验链兜底，兜不住则丢弃该问）
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ClarifyResult {
    pub questions: Vec<ClarifyQuestion>,
    /// 实际模型调用次数（1 = 一次成功；2 = 重试后成功）
    pub attempts: u8,
}

/// 解析 + 校验（纯逻辑，单测锚点）：fences 剥离 → JSON → 超限截断 → default 兜底 → 重新编号。
pub(crate) fn parse_and_validate_clarify(raw: &str) -> CommandResult<Vec<ClarifyQuestion>> {
    let text = crate::workflow_decompose::strip_fences(raw);
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: format!("不是有效 JSON：{e}"),
        })?;
    let Some(arr) = value.get("questions").and_then(|v| v.as_array()) else {
        return Err(CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: "缺少 questions 数组".into(),
        });
    };
    let questions: Vec<ClarifyQuestion> =
        serde_json::from_value(arr.clone().into()).map_err(|e| CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: format!("questions 结构不符：{e}"),
        })?;
    Ok(validate_clarify(questions))
}

/// 校验链（纯逻辑，单测锚点）：超限截断、空白问题丢弃、default 缺失取 options[0]、
/// default 与 options 双缺丢弃该问（无法保证"未答按假设继续"红线）、超 3 问裁到前 3、
/// 丢弃后按剩余顺序重编 id（q1..qn——前端按键值定位不依赖模型给的 id）。
pub(crate) fn validate_clarify(mut questions: Vec<ClarifyQuestion>) -> Vec<ClarifyQuestion> {
    questions.truncate(MAX_CLARIFY_QUESTIONS);
    let mut kept: Vec<ClarifyQuestion> = Vec::with_capacity(questions.len());
    for mut q in questions {
        let question = q.question.trim().to_string();
        if question.is_empty() {
            continue;
        }
        q.question = chars_truncate(&question, MAX_CLARIFY_QUESTION_CHARS);
        q.why = q
            .why
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| chars_truncate(s, MAX_CLARIFY_WHY_CHARS));
        q.options = q
            .options
            .iter()
            .map(|o| chars_truncate(o.trim(), MAX_CLARIFY_OPTION_CHARS))
            .filter(|o| !o.is_empty())
            .take(MAX_CLARIFY_OPTIONS)
            .collect();
        q.default = q
            .default
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| chars_truncate(s, MAX_CLARIFY_DEFAULT_CHARS))
            .or_else(|| q.options.first().cloned());
        // 双缺：这条问没有假设兜底，未答时没法继续——直接丢弃
        if q.default.is_none() {
            continue;
        }
        // 同文本去重前端答案按问题文本键控，重复文本互相覆盖）
        if kept.iter().any(|k| k.question == q.question) {
            continue;
        }
        kept.push(q);
    }
    for (i, q) in kept.iter_mut().enumerate() {
        q.id = format!("q{}", i + 1);
    }
    kept
}

fn chars_truncate(s: &str, cap: usize) -> String {
    s.chars().take(cap).collect()
}

/// 澄清调用（无会话、无工具、无流式；失败重试 1 次，再失败降级空 questions——增强非闸门）
#[tauri::command]
pub async fn workflow_clarify(
    app: AppHandle,
    goal: String,
    attachments: Option<Vec<String>>,
) -> CommandResult<ClarifyResult> {
    let goal_trimmed = goal.trim().to_string();
    if goal_trimmed.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "goal".into(),
            value: goal,
            reason: "目标不能为空".into(),
        });
    }
    if goal_trimmed.chars().count() > MAX_WORKFLOW_GOAL {
        return Err(CommandError::InvalidArgument {
            field: "goal".into(),
            value: goal_trimmed,
            reason: format!("目标超过 {MAX_WORKFLOW_GOAL} 字上限"),
        });
    }
    let attachment_list = attachments.unwrap_or_default();
    if attachment_list.len() > 10 {
        return Err(CommandError::InvalidArgument {
            field: "attachments".into(),
            value: attachment_list.len().to_string(),
            reason: "附件最多 10 个".into(),
        });
    }
    let (attach_blocks, _attached_ok) =
        crate::workflow_decompose::build_attachment_blocks(&app, &attachment_list).await;
    let system_prompt = format!("{DEFAULT_CLARIFY_GUIDANCE}{CLARIFY_CONTRACT}");
    let mut user_content = format!("总目标：{goal_trimmed}{attach_blocks}");
    let mut attempts: u8 = 0;
    let mut last_err; // 循环内两条退出路径均先赋值再读，无需占位初值
    loop {
        attempts += 1;
        // 轻量评审模型覆盖（设置键 review_model；条目缺失运行期降级跟随全局）
        let review_model = crate::db::workflow_settings::load_review_model(&app).await;
        let raw = match crate::bot_chat::summarize_messages_with_model(
            &app,
            &system_prompt,
            &[crate::bot_chat::ChatMsg {
                role: "user".into(),
                content: user_content.clone(),
            }],
            review_model.as_deref(),
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = e.to_string();
                break;
            }
        };
        match parse_and_validate_clarify(&raw) {
            Ok(questions) => {
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "workflow_clarify",
                    &[
                        ("outcome", "ok".into()),
                        ("questions", questions.len().to_string()),
                        ("attempts", attempts.to_string()),
                    ],
                );
                return Ok(ClarifyResult {
                    questions,
                    attempts,
                });
            }
            Err(e) => {
                last_err = e.to_string();
                if attempts >= 2 {
                    break;
                }
                user_content = format!(
                    "总目标：{goal_trimmed}{attach_blocks}\n\n你上一次的输出未通过校验：{last_err}\n请严格按照输出格式要求重新输出 JSON。"
                );
            }
        }
    }
    // 降级：空 questions = 前端无缝直拆（：澄清是增强不是闸门）
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Warn,
        "workflow_clarify",
        &[
            ("outcome", "degraded".into()),
            ("attempts", attempts.to_string()),
            ("error", crate::audit::escape_for_log(&last_err, 200)),
        ],
    );
    Ok(ClarifyResult {
        questions: Vec::new(),
        attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_dedupes_same_text_questions() {
        // 同文本两问：保留第一条（前端答案按问题文本键控，重复会互相覆盖）
        let out = validate_clarify(vec![
            ClarifyQuestion {
                id: "q1".into(),
                question: "平台？".into(),
                why: None,
                options: vec!["知乎".into()],
                default: Some("知乎".into()),
            },
            ClarifyQuestion {
                id: "q2".into(),
                question: "平台？".into(),
                why: None,
                options: vec!["公众号".into()],
                default: Some("公众号".into()),
            },
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].default.as_deref(), Some("知乎"));
    }

    #[test]
    fn parse_tolerates_fences_and_empty_questions() {
        let raw = "```json\n{\"questions\":[]}\n```";
        assert!(parse_and_validate_clarify(raw).unwrap().is_empty());
        let raw = r#"{"questions":[{"id":"q1","question":"投放平台？","options":["公众号","知乎"],"default":"公众号"}]}"#;
        let out = parse_and_validate_clarify(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "q1");
    }

    #[test]
    fn parse_rejects_bad_json_and_missing_questions() {
        assert!(parse_and_validate_clarify("抱歉，无法回答").is_err());
        assert!(parse_and_validate_clarify(r#"{"foo": 1}"#).is_err());
        assert!(parse_and_validate_clarify(r#"{"questions": "x"}"#).is_err());
    }

    #[test]
    fn validate_truncates_oversized_fields_instead_of_failing() {
        let out = validate_clarify(vec![ClarifyQuestion {
            id: "q1".into(),
            question: "问".repeat(MAX_CLARIFY_QUESTION_CHARS + 10),
            why: Some("因为".repeat(MAX_CLARIFY_WHY_CHARS + 5)),
            options: vec![
                "选".repeat(MAX_CLARIFY_OPTION_CHARS + 3),
                "  ".into(),
                "B".into(),
            ],
            default: Some("默".repeat(MAX_CLARIFY_DEFAULT_CHARS + 5)),
        }]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].question.chars().count(), MAX_CLARIFY_QUESTION_CHARS);
        assert_eq!(
            out[0].why.as_ref().unwrap().chars().count(),
            MAX_CLARIFY_WHY_CHARS
        );
        assert_eq!(out[0].options.len(), 2); // 空白项剔除，无上限截断
        assert_eq!(out[0].options[0].chars().count(), MAX_CLARIFY_OPTION_CHARS);
        assert_eq!(
            out[0].default.as_ref().unwrap().chars().count(),
            MAX_CLARIFY_DEFAULT_CHARS
        );
    }

    #[test]
    fn validate_caps_question_count() {
        let qs: Vec<ClarifyQuestion> = (0..5)
            .map(|i| ClarifyQuestion {
                id: format!("q{i}"),
                question: format!("问题{i}"),
                why: None,
                options: vec![],
                default: Some("假设".into()),
            })
            .collect();
        let out = validate_clarify(qs);
        assert_eq!(out.len(), MAX_CLARIFY_QUESTIONS);
        // 重新编号：丢弃/裁剪后 id 连续
        for (i, q) in out.iter().enumerate() {
            assert_eq!(q.id, format!("q{}", i + 1));
        }
    }

    #[test]
    fn validate_default_fallback_then_drop_when_unbackable() {
        // default 缺失 → 取 options[0] 兜底
        let out = validate_clarify(vec![ClarifyQuestion {
            id: "q1".into(),
            question: "平台？".into(),
            why: None,
            options: vec!["知乎".into(), "公众号".into()],
            default: None,
        }]);
        assert_eq!(out[0].default.as_deref(), Some("知乎"));
        // default 与 options 双缺 → 丢弃（红线：未答没法按假设继续）
        let out = validate_clarify(vec![ClarifyQuestion {
            id: "q1".into(),
            question: "开放问题？".into(),
            why: None,
            options: vec![],
            default: Some("  ".into()),
        }]);
        assert!(out.is_empty());
        // 空白问题 → 丢弃
        let out = validate_clarify(vec![ClarifyQuestion {
            id: "q1".into(),
            question: "   ".into(),
            why: None,
            options: vec![],
            default: Some("假设".into()),
        }]);
        assert!(out.is_empty());
    }
}
