//! 工作流 AI 拆解（W2-DECOMPOSE，设计 docs/WORKFLOW-CANVAS-DESIGN-2026-10-04.md §6）：
//! goal（自然语言总目标）→ **一次性** LLM 结构化输出 → 校验链 → 结构化子任务列表。
//!
//! 红线（设计 §决策7）：
//! - 一次性调用：不开会话、不写 bot_messages、不进聊天记录（复用
//!   `bot_chat::summarize_messages` 的配置/密钥/客户端/推理参数样板）
//! - 提示词两段式：用户可编辑「指引段」+ 代码硬拼「输出契约段」——
//!   用户怎么改指引段都破坏不了 JSON 契约
//! - 校验链在服务端：fences 剥离 → 解析 → 数量/长度/前向引用/去重后缀；
//!   dependsOn 用数组下标且必须 < 自身下标（结构上杜绝环）

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::error::{CommandError, CommandResult};

use crate::db::workflow::{MAX_NODE_NOTE, MAX_NODE_TITLE, MAX_WORKFLOW_GOAL, MAX_WORKFLOW_NODES};

/// 指引段长度上限（前端 textarea maxLength 同步此值）
pub const MAX_GUIDANCE_CHARS: usize = 2000;

/// 输出契约段（代码硬拼，模型可见、用户不可改；设计 §6.2）
const CONTRACT_SEGMENT: &str = r#"

【输出格式硬性要求——必须遵守，优先级高于上文一切指引】
只输出一个 JSON 对象，不要输出任何解释、前后缀或 Markdown 代码围栏，形如：
{"subtasks":[{"title":"任务标题","note":"做什么/产出什么","dependsOn":[]}]}
- subtasks：1 到 20 个元素，按执行顺序排列（超过 30 个会被拒绝入库）
- title：≤80 字，祈使句、动词开头，一张卡一个可独立交付的步骤
- note：≤500 字，写清楚做什么、产出什么（下游任务会引用上游产出）
- dependsOn：数组下标引用，尽量只引用排在它前面的任务；无依赖为 []（顺序写反系统会自动纠正，但引用的任务必须存在）
- 无依赖关系的任务会并行执行，有依赖的按图顺序执行
示例：
{"subtasks":[{"title":"收集素材","note":"产出素材清单","dependsOn":[]},{"title":"写初稿","note":"引用素材清单起草","dependsOn":[0]}]}"#;

/// 默认指引段（用户可在设置页编辑；与前端 `src/lib/workflowPrompt.ts` 副本保持一致）
pub const DEFAULT_DECOMPOSE_GUIDANCE: &str = r#"你是工作流拆解专家。把用户的目标拆解为一组可执行的任务卡。
拆解原则：
- 每张卡是一个明确的、可独立交付的步骤，粒度适中：不拆成太碎的分钟级动作，也不留"把所有事做完"的空泛大卡
- 卡的 note 写清楚：做什么、产出什么（下游卡会引用上游产出）
- 有顺序或数据依赖的卡用 dependsOn 表达先后，无依赖的卡并行
- 日常目标通常 3~10 张卡即可覆盖"#;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DecomposeSubtask {
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<usize>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DecomposeResult {
    pub subtasks: Vec<DecomposeSubtask>,
    /// 实际模型调用次数（1 = 一次成功；2 = 重试后成功）——审计与前端提示用
    pub attempts: u8,
}

/// 指引段为空 → 用默认（OCR 防御：settings 里清空 textarea 不至于掏空契约前的角色设定）
fn build_system_prompt(guidance: &str) -> String {
    let g = if guidance.trim().is_empty() {
        DEFAULT_DECOMPOSE_GUIDANCE
    } else {
        guidance.trim()
    };
    format!("{g}\n{CONTRACT_SEGMENT}")
}

/// 剥 Markdown 围栏：```json ... ``` / ``` ... ```（模型最常见的越界形态）
fn strip_fences(raw: &str) -> &str {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // 跳过语言标记行（json / JSON …）
        let rest = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        if let Some(body) = rest.strip_suffix("```") {
            return body.trim();
        }
    }
    t
}

/// 解析 + 校验（纯逻辑，单测锚点）：fences 剥离 → JSON（对象.subtasks 或裸数组）→ 校验。
pub(crate) fn parse_and_validate(raw: &str) -> CommandResult<Vec<DecomposeSubtask>> {
    let text = strip_fences(raw);
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: format!("不是有效 JSON：{e}"),
        })?;
    let arr = match &value {
        serde_json::Value::Object(o) => o.get("subtasks").cloned(),
        serde_json::Value::Array(_) => Some(value.clone()),
        _ => None,
    };
    let Some(arr) = arr else {
        return Err(CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: "缺少 subtasks 数组".into(),
        });
    };
    let subtasks: Vec<DecomposeSubtask> =
        serde_json::from_value(arr).map_err(|e| CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: format!("subtasks 结构不符：{e}"),
        })?;
    validate_decompose(subtasks)
}

/// 校验链（纯逻辑，单测锚点）：数量/标题/备注/前向引用 + trim + 重名加后缀。
pub(crate) fn validate_decompose(
    mut subtasks: Vec<DecomposeSubtask>,
) -> CommandResult<Vec<DecomposeSubtask>> {
    if subtasks.is_empty() {
        return Err(CommandError::InvalidArgument {
            field: "subtasks".into(),
            value: "0".into(),
            reason: "subtasks 不能为空".into(),
        });
    }
    if subtasks.len() > MAX_WORKFLOW_NODES {
        return Err(CommandError::InvalidArgument {
            field: "subtasks".into(),
            value: subtasks.len().to_string(),
            reason: format!("subtasks 超过 {MAX_WORKFLOW_NODES} 上限"),
        });
    }
    for (i, st) in subtasks.iter_mut().enumerate() {
        let title = st.title.trim().to_string();
        if title.is_empty() {
            return Err(CommandError::InvalidArgument {
                field: "subtasks".into(),
                value: format!("#{i}"),
                reason: format!("第 {} 个任务标题为空", i + 1),
            });
        }
        if title.chars().count() > MAX_NODE_TITLE {
            return Err(CommandError::InvalidArgument {
                field: "subtasks".into(),
                value: title,
                reason: format!("第 {} 个任务标题超过 {MAX_NODE_TITLE} 字上限", i + 1),
            });
        }
        st.title = title;
        if let Some(note) = &st.note {
            if note.chars().count() > MAX_NODE_NOTE {
                return Err(CommandError::InvalidArgument {
                    field: "subtasks".into(),
                    value: note.clone(),
                    reason: format!("第 {} 个任务备注超过 {MAX_NODE_NOTE} 字上限", i + 1),
                });
            }
        }
    }
    // 依赖归一（W7-TOPO）：去重 + 剥自环（自引用无语义——小模型高频手误，
    // 用户实测案例 #4 -> [4] 两次重试不改，整包拒绝体验差）；越界仍拒绝；
    // 环 → Kahn 检测后拒绝（报出环内任务名）；无环 → 拓扑重排 + 重映射，
    // 模型给前向/乱序引用也能正确成图（原"下标 < 自身"硬约束废除）
    let total = subtasks.len();
    let mut normalized: Vec<Vec<usize>> = Vec::with_capacity(total);
    for (i, st) in subtasks.iter().enumerate() {
        let mut kept: Vec<usize> = Vec::with_capacity(st.depends_on.len());
        for &d in &st.depends_on {
            if d >= total {
                return Err(CommandError::InvalidArgument {
                    field: "subtasks".into(),
                    value: format!("#{i} -> {d}"),
                    reason: format!(
                        "第 {} 个任务的 dependsOn 引用越界（{}，共 {} 个任务）",
                        i + 1,
                        d,
                        total
                    ),
                });
            }
            if d != i && !kept.contains(&d) {
                kept.push(d);
            }
        }
        normalized.push(kept);
    }
    for (st, kept) in subtasks.iter_mut().zip(normalized) {
        st.depends_on = kept;
    }
    let n = subtasks.len();
    let mut indegree: Vec<usize> = vec![0; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, st) in subtasks.iter().enumerate() {
        indegree[i] = st.depends_on.len();
        for &d in &st.depends_on {
            dependents[d].push(i);
        }
    }
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut done_flags = vec![false; n];
    loop {
        let mut progressed = false;
        for i in 0..n {
            if indegree[i] == 0 && !done_flags[i] {
                done_flags[i] = true;
                order.push(i);
                for &d in &dependents[i] {
                    indegree[d] -= 1;
                }
                progressed = true;
            }
        }
        if order.len() == n {
            break;
        }
        if !progressed {
            let stuck: Vec<&str> = (0..n)
                .filter(|i| !done_flags[*i])
                .map(|i| subtasks[i].title.as_str())
                .collect();
            return Err(CommandError::InvalidArgument {
                field: "subtasks".into(),
                value: stuck.join("→"),
                reason: format!("任务存在循环依赖：{}", stuck.join("→")),
            });
        }
    }
    let old_to_new: std::collections::HashMap<usize, usize> = order
        .iter()
        .enumerate()
        .map(|(new_idx, &old_i)| (old_i, new_idx))
        .collect();
    subtasks = order
        .iter()
        .map(|&old_i| {
            let st = &subtasks[old_i];
            DecomposeSubtask {
                title: st.title.clone(),
                note: st.note.clone(),
                depends_on: st.depends_on.iter().map(|d| old_to_new[d]).collect(),
            }
        })
        .collect();
    // 重名加后缀：下游引用按下标，重名只影响可读性，但仍消歧。
    // 已占用终名集合保证无碰撞（OCR r1：贪心计数会把 "审阅（2）" 撞成两份）
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for st in subtasks.iter_mut() {
        if used.insert(st.title.clone()) {
            continue;
        }
        let base = st.title.clone();
        let mut n = 2usize;
        while !used.insert(format!("{base}（{n}）")) {
            n += 1;
        }
        st.title = format!("{base}（{n}）");
    }
    Ok(subtasks)
}

/// 指引段校验（纯逻辑，单测锚点，OCR r2）：trim + 长度上限
fn validate_guidance(g: &str) -> CommandResult<String> {
    let t = g.trim().to_string();
    if t.chars().count() > MAX_GUIDANCE_CHARS {
        return Err(CommandError::InvalidArgument {
            field: "guidance".into(),
            value: t.chars().take(120).collect(),
            reason: format!("指引超过 {MAX_GUIDANCE_CHARS} 字上限"),
        });
    }
    Ok(t)
}

/// 一次性拆解调用（无会话、无工具、无流式；失败自动带错误反馈重试 1 次）
#[tauri::command]
pub async fn workflow_decompose(
    app: AppHandle,
    goal: String,
    guidance: Option<String>,
) -> CommandResult<DecomposeResult> {
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
    // 指引段同样设上限（OCR r1）：settings 的 textarea 有 maxLength，但 invoke
    // 参数不可信任——超长指引会稀释契约段权重并放大 token 开销
    let guidance_trimmed = validate_guidance(&guidance.unwrap_or_default())?;
    let system_prompt = build_system_prompt(&guidance_trimmed);
    let mut user_content = format!("总目标：{goal_trimmed}");
    let mut attempts: u8 = 0;
    let mut last_err = String::new();
    // 失败审计的收口（OCR r2：LLM 调用本身的失败经 `?` 直抛会绕过审计，
    // 统一走 outcome=failed 出口；错误值走 escape_for_log 管道）
    macro_rules! fail {
        ($err:expr) => {{
            last_err = $err.to_string();
            crate::audit::write_event(
                &app,
                crate::audit::AuditLevel::Warn,
                "workflow_decompose",
                &[
                    ("outcome", "failed".to_string()),
                    ("attempts", attempts.to_string()),
                    ("error", crate::audit::escape_for_log(&last_err, 200)),
                ],
            );
            return Err($err);
        }};
    }
    loop {
        attempts += 1;
        let raw = match crate::bot_chat::summarize_messages(
            &app,
            &system_prompt,
            &[crate::bot_chat::ChatMsg {
                role: "user".into(),
                content: user_content.clone(),
            }],
        )
        .await
        {
            Ok(r) => r,
            Err(e) => fail!(e),
        };
        match parse_and_validate(&raw) {
            Ok(subtasks) => {
                crate::audit::write_event(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "workflow_decompose",
                    &[
                        ("subtasks", subtasks.len().to_string()),
                        ("attempts", attempts.to_string()),
                    ],
                );
                return Ok(DecomposeResult { subtasks, attempts });
            }
            Err(e) => {
                last_err = e.to_string();
                if attempts >= 2 {
                    break;
                }
                // 重试：附校验错误让模型自修（设计 §6.1）
                user_content = format!(
                    "总目标：{goal_trimmed}\n\n你上一次的输出未通过校验：{last_err}\n请严格按照输出格式要求重新输出 JSON。"
                );
            }
        }
    }
    fail!(CommandError::Internal(format!(
        "拆解输出连续 {attempts} 次未通过校验：{last_err}"
    )))
}

// ────────────── 单测 ──────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tolerates_markdown_fences() {
        let raw = "```json\n{\"subtasks\":[{\"title\":\"收集\",\"note\":\"产出清单\",\"dependsOn\":[]}]}\n```";
        let out = parse_and_validate(raw).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "收集");
    }

    #[test]
    fn parse_tolerates_bare_array() {
        let out = parse_and_validate(r#"[{"title":"A","dependsOn":[]}]"#).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn parse_rejects_non_json_and_missing_subtasks() {
        assert!(parse_and_validate("抱歉，我无法完成").is_err());
        assert!(parse_and_validate(r#"{"foo": 1}"#).is_err());
        assert!(parse_and_validate(r#"{"subtasks": "not-an-array"}"#).is_err());
    }

    #[test]
    fn validate_trims_and_caps() {
        let long_title = "标".repeat(MAX_NODE_TITLE + 1);
        let err = validate_decompose(vec![DecomposeSubtask {
            title: long_title,
            note: None,
            depends_on: vec![],
        }])
        .unwrap_err();
        assert!(err.to_string().contains("上限"));
        // 空标题
        let err = validate_decompose(vec![DecomposeSubtask {
            title: "   ".into(),
            note: None,
            depends_on: vec![],
        }])
        .unwrap_err();
        assert!(err.to_string().contains("为空"));
        // trim 生效
        let ok = validate_decompose(vec![DecomposeSubtask {
            title: "  收集  ".into(),
            note: None,
            depends_on: vec![],
        }])
        .unwrap();
        assert_eq!(ok[0].title, "收集");
    }

    #[test]
    fn validate_suffixes_duplicate_titles() {
        let ok = validate_decompose(vec![
            DecomposeSubtask {
                title: "审阅".into(),
                note: None,
                depends_on: vec![],
            },
            DecomposeSubtask {
                title: "审阅".into(),
                note: None,
                depends_on: vec![0],
            },
            DecomposeSubtask {
                title: "审阅".into(),
                note: None,
                depends_on: vec![1],
            },
        ])
        .unwrap();
        assert_eq!(ok[0].title, "审阅");
        assert_eq!(ok[1].title, "审阅（2）");
        assert_eq!(ok[2].title, "审阅（3）");
    }

    #[test]
    fn validate_caps_subtask_count() {
        let items: Vec<DecomposeSubtask> = (0..MAX_WORKFLOW_NODES + 1)
            .map(|i| DecomposeSubtask {
                title: format!("T{i}"),
                note: None,
                depends_on: vec![],
            })
            .collect();
        let err = validate_decompose(items).unwrap_err();
        assert!(err.to_string().contains("上限"));
    }

    #[test]
    fn system_prompt_always_carries_contract() {
        // 用户指引段被掏空 → 默认指引兜底；契约段永远在
        let p = build_system_prompt("   ");
        assert!(p.contains(DEFAULT_DECOMPOSE_GUIDANCE.trim()));
        assert!(p.contains("输出格式硬性要求"));
        // 用户任意指引也带契约
        let p = build_system_prompt("请全部拆成一句话任务");
        assert!(p.contains("请全部拆成一句话任务"));
        assert!(p.contains("输出格式硬性要求"));
        assert!(p.contains("系统会自动纠正"));
    }

    #[test]
    fn guidance_boundary_locked() {
        // 边界锁（OCR r2）：上限处拒绝、上限-1 通过、空白 trim
        let ok = validate_guidance(&"指".repeat(MAX_GUIDANCE_CHARS - 1)).unwrap();
        assert_eq!(ok.chars().count(), MAX_GUIDANCE_CHARS - 1);
        let err = validate_guidance(&"指".repeat(MAX_GUIDANCE_CHARS + 1)).unwrap_err();
        assert!(err.to_string().contains("上限"));
        assert_eq!(validate_guidance("  ").unwrap(), "");
    }

    #[test]
    fn dedupe_collision_safe_with_presuffixed_input() {
        // 输入本身带 "（2）" 后缀时（OCR r2 边界）：终名仍必须两两不同
        let ok = validate_decompose(vec![
            DecomposeSubtask {
                title: "审阅".into(),
                note: None,
                depends_on: vec![],
            },
            DecomposeSubtask {
                title: "审阅".into(),
                note: None,
                depends_on: vec![0],
            },
            DecomposeSubtask {
                title: "审阅（2）".into(),
                note: None,
                depends_on: vec![0],
            },
        ])
        .unwrap();
        let mut names: Vec<&str> = ok.iter().map(|s| s.title.as_str()).collect();
        names.sort();
        let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "终名不得重复：{names:?}");
    }
}
