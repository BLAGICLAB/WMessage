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
{"subtasks":[{"title":"任务标题","note":"做什么/产出什么","acceptance":"一行可验证的完成标准","dependsOn":[]}]}
- subtasks：1 到 20 个元素，按执行顺序排列（超过 30 个会被拒绝入库）
- title：≤80 字，祈使句、动词开头，一张卡一个可独立交付的步骤
- note：≤500 字，写清楚做什么、产出什么；产出文件必须写具体文件名和格式（如「产出周报.docx」），下游任务按文件名引用上游产物，不写具体名字会导致下游拿不到上游结果
- acceptance：≤120 字，一行可验证的完成标准——这张卡做到什么程度算完成（产出什么文件、包含哪些关键内容点），执行时会据此自检
- dependsOn：数组下标引用，尽量只引用排在它前面的任务；无依赖为 []（顺序写反系统会自动纠正，但引用的任务必须存在）
- subtasks：可选，2~8 条子任务文本（每条 ≤60 字）；有前置材料的任务，把材料里对应的要点/数据拆进 subtasks
- 无依赖关系的任务会并行执行，有依赖的按图顺序执行
示例：
{"subtasks":[{"title":"收集素材","note":"产出素材清单.md","acceptance":"产出素材清单.md，含至少 5 条素材及其来源链接","dependsOn":[]},{"title":"写初稿","note":"引用素材清单.md 起草","acceptance":"产出初稿.docx，覆盖素材清单全部要点","dependsOn":[0]}]}"#;

/// 默认指引段（用户可在设置页编辑；与前端 `src/lib/workflowPrompt.ts` 副本保持一致）
pub const DEFAULT_DECOMPOSE_GUIDANCE: &str = r#"你是工作流拆解专家。把用户的目标拆解为一组可执行的任务卡。
拆解原则：
- 每张卡是一个明确的、可独立交付的步骤，粒度适中：不拆成太碎的分钟级动作，也不留"把所有事做完"的空泛大卡
- 卡的 note 写清楚：做什么、产出什么；产物文件写具体文件名和格式，下游卡按文件名引用上游产出
- 每张卡给一行可验证的验收标准（acceptance）：产出什么文件、包含什么关键内容点
- 有顺序或数据依赖的卡用 dependsOn 表达先后，无依赖的卡并行
- 日常目标通常 3~10 张卡即可覆盖"#;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DecomposeSubtask {
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    /// 一行可验证的完成标准（W-QA 卡即契约）；模型漏给不拒绝
    #[serde(default)]
    pub acceptance: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<usize>,
    /// 子任务文本清单（W8-ATTACH）：附件内容拆进卡片子任务
    #[serde(default)]
    pub subtasks: Option<Vec<String>>,
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
/// pub(crate)：task_autotag 的输出校验链复用同一契约（任务图谱设计 §2）
pub(crate) fn strip_fences(raw: &str) -> &str {
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
        // 验收标准校验（W-QA 卡即契约）：trim + ≤120 字；缺失容忍（模型漏给不拒整包）
        st.acceptance = st
            .acceptance
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        if let Some(acc) = &st.acceptance {
            if acc.chars().count() > crate::db::workflow::MAX_NODE_ACCEPTANCE {
                return Err(CommandError::InvalidArgument {
                    field: "subtasks".into(),
                    value: acc.clone(),
                    reason: format!(
                        "第 {} 个任务验收标准超过 {} 字上限",
                        i + 1,
                        crate::db::workflow::MAX_NODE_ACCEPTANCE
                    ),
                });
            }
        }
        // 子任务清单校验（W8-ATTACH）：≤8 条 × ≤60 字，trim，空白条剔除
        if let Some(list) = &mut st.subtasks {
            if list.len() > 8 {
                return Err(CommandError::InvalidArgument {
                    field: "subtasks".into(),
                    value: list.len().to_string(),
                    reason: format!("第 {} 个任务子任务超过 8 条上限", i + 1),
                });
            }
            for (si, t) in list.iter_mut().enumerate() {
                *t = t.trim().to_string();
                if t.chars().count() > 60 {
                    return Err(CommandError::InvalidArgument {
                        field: "subtasks".into(),
                        value: t.clone(),
                        reason: format!(
                            "第 {} 个任务的第 {} 条子任务超过 60 字上限",
                            i + 1,
                            si + 1
                        ),
                    });
                }
            }
            list.retain(|t| !t.is_empty());
            if list.is_empty() {
                st.subtasks = None;
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
                    // done 节点不在（Kahn 已序）不减；重复减由 done_flags 屏蔽
                    // （W7 r1 low：改为显式队列实现时须保留该守卫）
                    if !done_flags[d] {
                        indegree[d] -= 1;
                    }
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
                // value = 机器可读定位（环内任务名），reason = 人类解释（本函数约定）
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
                acceptance: st.acceptance.clone(),
                depends_on: st.depends_on.iter().map(|d| old_to_new[d]).collect(),
                subtasks: st.subtasks.clone(),
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

/// 附件路径边界校验（同步阻塞，调用方须放 spawn_blocking）：
/// canonicalize（存在性 + 软链解析到真实目标）→ 必须是常规文件 → 扩展名在
/// 抽取脚本支持集内（脚本对其余扩展名本就报错退出）。返回 canonical 路径。
fn validate_attach_path(path: &str) -> Result<std::path::PathBuf, String> {
    const SUPPORTED: [&str; 4] = ["docx", "xlsx", "pptx", "pdf"];
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("路径为空".into());
    }
    let canon = std::fs::canonicalize(trimmed).map_err(|e| format!("路径无法解析：{e}"))?;
    if !canon.is_file() {
        return Err("不是常规文件".into());
    }
    let ext = canon
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !SUPPORTED.contains(&ext.as_str()) {
        return Err("仅支持 docx/xlsx/pptx/pdf".into());
    }
    Ok(canon)
}

/// 一次性拆解调用（无会话、无工具、无流式；失败自动带错误反馈重试 1 次）
#[tauri::command]
pub async fn workflow_decompose(
    app: AppHandle,
    goal: String,
    guidance: Option<String>,
    attachments: Option<Vec<String>>,
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
    // 附件抽取（W8-ATTACH）：用户在对话框亲手选的文件 = 明确授权，
    // 直调 doc_extract 不走工具授权闸；抽取失败不炸整包（占位说明）。
    // 单文件 12k 字符、总 48k 字符封顶（防上下文撑爆）。
    let attachment_list = attachments.unwrap_or_default();
    if attachment_list.len() > 10 {
        return Err(CommandError::InvalidArgument {
            field: "attachments".into(),
            value: attachment_list.len().to_string(),
            reason: "附件最多 10 个".into(),
        });
    }
    let mut attach_blocks = String::new();
    let mut attached_ok = 0usize;
    {
        const PER_FILE_CAP: usize = 12_000;
        const TOTAL_CAP: usize = 48_000;
        let mut total_used = 0usize;
        for (i, path) in attachment_list.iter().enumerate() {
            if total_used >= TOTAL_CAP {
                attach_blocks.push_str(&format!(
                    "\n【附件 {}：{}】（超出总字符上限，未注入）",
                    i + 1,
                    path
                ));
                continue;
            }
            let file_name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            // 附件路径先过边界校验再进抽取：canonicalize 确认存在并解析软链到
            // 真实目标、必须是常规文件、扩展名落在抽取脚本支持集内。attachments
            // 直达自 invoke 参数，不能沿用「对话框亲手选 = 明确授权」假设；不合规
            // 条目走占位说明，与读取失败同口径，不炸整包。
            let probe = path.clone();
            let checked =
                crate::py::document::spawn_blocking_map(move || validate_attach_path(&probe)).await;
            if let Err(reason) = checked {
                attach_blocks.push_str(&format!(
                    "\n【附件 {}：{}】（校验未通过：{}）\n",
                    i + 1,
                    file_name,
                    reason
                ));
                continue;
            }
            match crate::bot_py::doc_extract(app.clone(), Some(path.clone())).await {
                Ok(res) => {
                    let mut text: String = res.text.chars().take(PER_FILE_CAP).collect();
                    total_used += text.chars().count();
                    if total_used > TOTAL_CAP {
                        let remain = TOTAL_CAP.saturating_sub(total_used - text.chars().count());
                        text = res.text.chars().take(remain).collect();
                    }
                    attach_blocks.push_str(&format!(
                        "\n【附件 {}：{}】\n{}\n",
                        i + 1,
                        file_name,
                        text
                    ));
                    attached_ok += 1;
                }
                Err(e) => {
                    attach_blocks.push_str(&format!(
                        "\n【附件 {}：{}】（读取失败：{}）\n",
                        i + 1,
                        file_name,
                        crate::bot::truncate_for_log(&e.message(), 120)
                    ));
                }
            }
        }
        if !attachment_list.is_empty() {
            let header = format!(
                "\n用户提供了 {} 个附件（{} 个读取成功），请把与各任务相关的内容拆进对应任务的 note 或 subtasks：",
                attachment_list.len(),
                attached_ok
            );
            attach_blocks.insert_str(0, &header);
        }
    }
    let mut user_content = format!("总目标：{goal_trimmed}{attach_blocks}");
    if !attachment_list.is_empty() {
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "workflow_decompose",
            &[
                ("attachments", attachment_list.len().to_string()),
                ("attachedOk", attached_ok.to_string()),
            ],
        );
    }
    let mut attempts: u8 = 0;
    let mut last_err = String::new();
    // 失败审计的收口（OCR r2：LLM 调用本身的失败经 `?` 直抛会绕过审计，
    // 统一走 outcome=failed 出口；错误值走 escape_for_log 管道）
    macro_rules! fail {
        ($err:expr) => {{
            // 表达式只求值一次：调用方传 format! 时避免拼两遍、只留一份
            let err = $err;
            last_err = err.to_string();
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
            return Err(err);
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
                // 重试：保留原始目标与附件段，仅追加校验错误让模型自修（设计 §6.1）；
                // 丢掉附件会让第二次尝试拿到的上下文比第一次更少，抽取成本白付
                user_content = format!(
                    "总目标：{goal_trimmed}{attach_blocks}\n\n你上一次的输出未通过校验：{last_err}\n请严格按照输出格式要求重新输出 JSON。"
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
    fn attach_path_rejects_missing_unsupported_and_accepts_doc() {
        let dir = std::env::temp_dir().join(format!("wfdecompose-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("a.docx");
        std::fs::write(&doc, b"placeholder").unwrap();
        let txt = dir.join("b.txt");
        std::fs::write(&txt, b"secret").unwrap();

        let ok = validate_attach_path(doc.to_str().unwrap()).unwrap();
        assert_eq!(ok, doc.canonicalize().unwrap());
        assert!(validate_attach_path(txt.to_str().unwrap())
            .unwrap_err()
            .contains("docx"));
        assert!(
            validate_attach_path(dir.join("gone.docx").to_str().unwrap())
                .unwrap_err()
                .contains("无法解析")
        );
        assert!(validate_attach_path("").unwrap_err().contains("路径为空"));
        assert!(validate_attach_path(dir.to_str().unwrap())
            .unwrap_err()
            .contains("常规文件"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn attach_path_resolves_symlink_to_real_target_extension() {
        let dir = std::env::temp_dir().join(format!("wfdecompose-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.docx");
        std::fs::write(&real, b"placeholder").unwrap();
        let link_txt = dir.join("link.txt");
        std::os::unix::fs::symlink(&real, &link_txt).unwrap();
        // 软链文件名是 .txt：按链接名校验会误拒、按真实目标校验应放行
        assert!(validate_attach_path(link_txt.to_str().unwrap()).is_ok());

        let secret = dir.join("secret");
        std::fs::write(&secret, b"secret").unwrap();
        let link_doc = dir.join("evil.docx");
        std::os::unix::fs::symlink(&secret, &link_doc).unwrap();
        // .docx 软链指向非文档文件：解析到真实目标后按扩展名拒绝
        assert!(validate_attach_path(link_doc.to_str().unwrap())
            .unwrap_err()
            .contains("docx"));

        std::fs::remove_dir_all(&dir).ok();
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
            acceptance: None,
            title: long_title,
            note: None,
            depends_on: vec![],
            subtasks: None,
        }])
        .unwrap_err();
        assert!(err.to_string().contains("上限"));
        // 空标题
        let err = validate_decompose(vec![DecomposeSubtask {
            acceptance: None,
            title: "   ".into(),
            note: None,
            depends_on: vec![],
            subtasks: None,
        }])
        .unwrap_err();
        assert!(err.to_string().contains("为空"));
        // trim 生效
        let ok = validate_decompose(vec![DecomposeSubtask {
            acceptance: None,
            title: "  收集  ".into(),
            note: None,
            depends_on: vec![],
            subtasks: None,
        }])
        .unwrap();
        assert_eq!(ok[0].title, "收集");
    }

    #[test]
    fn validate_suffixes_duplicate_titles() {
        let ok = validate_decompose(vec![
            DecomposeSubtask {
                acceptance: None,
                title: "审阅".into(),
                note: None,
                depends_on: vec![],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "审阅".into(),
                note: None,
                depends_on: vec![0],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "审阅".into(),
                note: None,
                depends_on: vec![1],
                subtasks: None,
            },
        ])
        .unwrap();
        assert_eq!(ok[0].title, "审阅");
        assert_eq!(ok[1].title, "审阅（2）");
        assert_eq!(ok[2].title, "审阅（3）");
    }

    #[test]
    fn validate_normalizes_forward_refs_and_strips_self_loops() {
        // W7-TOPO：前向引用接受并拓扑重排（被依赖的 A 排前）
        let ok = validate_decompose(vec![
            DecomposeSubtask {
                acceptance: None,
                title: "B".into(),
                note: None,
                depends_on: vec![1],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "A".into(),
                note: None,
                depends_on: vec![],
                subtasks: None,
            },
        ])
        .unwrap();
        assert_eq!(ok[0].title, "A");
        assert!(ok[0].depends_on.is_empty());
        assert_eq!(ok[1].title, "B");
        assert_eq!(ok[1].depends_on, vec![0]);
        // 自环剥离：B 依赖自己 → 无害剥离，合法依赖保留
        let ok = validate_decompose(vec![
            DecomposeSubtask {
                acceptance: None,
                title: "A".into(),
                note: None,
                depends_on: vec![],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "B".into(),
                note: None,
                depends_on: vec![0, 1],
                subtasks: None,
            },
        ])
        .unwrap();
        assert_eq!(ok[1].depends_on, vec![0]);
    }

    #[test]
    fn validate_rejects_real_cycles_with_names() {
        // 真环（非自环）：拒绝且报出环内任务名
        let err = validate_decompose(vec![
            DecomposeSubtask {
                acceptance: None,
                title: "甲".into(),
                note: None,
                depends_on: vec![1],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "乙".into(),
                note: None,
                depends_on: vec![0],
                subtasks: None,
            },
        ])
        .unwrap_err();
        assert!(err.to_string().contains("循环依赖"));
        assert!(err.to_string().contains("甲") && err.to_string().contains("乙"));
    }

    #[test]
    fn validate_caps_subtask_count() {
        let items: Vec<DecomposeSubtask> = (0..MAX_WORKFLOW_NODES + 1)
            .map(|i| DecomposeSubtask {
                acceptance: None,
                title: format!("T{i}"),
                note: None,
                depends_on: vec![],
                subtasks: None,
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
                acceptance: None,
                title: "审阅".into(),
                note: None,
                depends_on: vec![],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "审阅".into(),
                note: None,
                depends_on: vec![0],
                subtasks: None,
            },
            DecomposeSubtask {
                acceptance: None,
                title: "审阅（2）".into(),
                note: None,
                depends_on: vec![0],
                subtasks: None,
            },
        ])
        .unwrap();
        let mut names: Vec<&str> = ok.iter().map(|s| s.title.as_str()).collect();
        names.sort();
        let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "终名不得重复：{names:?}");
    }
}
