//! 归档自动打标（任务图谱设计 §2）：任务进入归档且无标签时，一次性 LLM 生成
//! ≤3 个标签回写 tags。复用 workflow_decompose 的一次性推理样板——不开会话、
//! 不写 bot_messages、不进聊天记录；失败静默（审计留痕，前端不打扰）。
//!
//! 触发方：前端观测「已加载任务从未归档 → 归档」转变后 fire-and-forget 调用；
//! 首屏加载不触发（防对历史归档批量调用），守卫链保证幂等（已有 tags 直接跳过）。

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::error::CommandResult;

/// 标签硬上限（设计 §2）：≤3 个、单个 ≤12 字
pub const MAX_TAGS: usize = 3;
pub const MAX_TAG_CHARS: usize = 12;

/// 指引段：角色 + 取材范围 + 词表风格
const GUIDANCE: &str = r#"你是任务标签助手。根据任务卡的标题、备注和子任务清单，为它生成用于分类聚合的标签。
原则：
- 标签是稳定的工作类别词（如「周报」「部署」「评审」「数据分析」），不是动作短语或句子
- 优先沿用任务标题中出现过的词；不同任务卡对同类工作应打出相同标签
- 不生成「任务」「工作」「待办」这类无信息量的泛化词"#;

/// 输出契约段（代码硬拼，模型不可改）
const CONTRACT: &str = r#"

【输出格式硬性要求——必须遵守，优先级高于上文一切指引】
只输出一个 JSON 对象，不要输出任何解释、前后缀或 Markdown 代码围栏，形如：
{"tags":["标签一","标签二"]}
- tags：0 到 3 个字符串，每个 2~12 个字，去重
- 内容不足以归类时输出 {"tags":[]}"#;

/// 校验链（纯逻辑，单测锚点）：fences 剥离 → JSON → 数量/长度/去重/trim。
/// 打标是增强功能——超限**截断**不整包拒绝（宁缺毋炸），仅结构非法才报错。
pub(crate) fn parse_and_validate_tags(raw: &str) -> CommandResult<Vec<String>> {
    let text = crate::workflow_decompose::strip_fences(raw);
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| crate::error::CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: format!("不是有效 JSON：{e}"),
        })?;
    let Some(arr) = value.get("tags").and_then(|t| t.as_array()) else {
        return Err(crate::error::CommandError::InvalidArgument {
            field: "output".into(),
            value: raw.chars().take(120).collect(),
            reason: "缺少 tags 数组".into(),
        });
    };
    let mut out: Vec<String> = Vec::with_capacity(arr.len());
    for v in arr {
        let Some(s) = v.as_str() else { continue };
        let t = s.trim();
        if t.is_empty() || out.iter().any(|e| e == t) {
            continue;
        }
        out.push(t.chars().take(MAX_TAG_CHARS).collect());
        if out.len() >= MAX_TAGS {
            break;
        }
    }
    Ok(out)
}

/// 从任务行组装打标用户消息（截断防 token 失控：note 300 字、子任务合计 300 字）
pub(crate) fn build_user_content(
    title: &str,
    note: Option<&str>,
    subtasks: &[crate::db::Subtask],
) -> String {
    let mut c = format!("标题：{}", title.trim());
    if let Some(n) = note.map(str::trim).filter(|s| !s.is_empty()) {
        let truncated: String = n.chars().take(300).collect();
        c.push_str(&format!("\n备注：{truncated}"));
    }
    if !subtasks.is_empty() {
        let items: Vec<String> = subtasks
            .iter()
            .filter_map(|s| {
                let t = s.text.trim();
                (!t.is_empty()).then(|| t.chars().take(60).collect::<String>())
            })
            .take(10)
            .collect();
        if !items.is_empty() {
            let joined: String = items.join("；").chars().take(300).collect();
            c.push_str(&format!("\n子任务：{joined}"));
        }
    }
    c
}

/// 守卫链 + LLM + 回写。skip（不满足条件）返回 Ok(None)；
/// LLM/校验失败返回 Err（调用方——前端——静默吞掉，此处已写审计）。
#[tauri::command]
pub async fn task_autotag(app: AppHandle, id: String) -> CommandResult<Option<Vec<String>>> {
    use crate::db::TaskStatus;

    // 前置读（守卫链大部分与数据相关，先在锁外快速失败可省一次写锁）
    let app2 = app.clone();
    let id2 = id.clone();
    let prelude =
        tauri::async_runtime::spawn_blocking(move || -> Result<Option<crate::db::Task>, String> {
            let conn = crate::db::open_db(&app2)?;
            let Some(t) = crate::db::load_task(&conn, &id2)? else {
                return Ok(None);
            };
            let skip = t.column != TaskStatus::Done
            || t.archived != Some(true)
            || t.deleted_at.is_some()
            || t.owner_id.is_some() // 只给自己的卡打标（外来卡只读）
            || t.tags.as_ref().is_some_and(|v| !v.is_empty());
            Ok(if skip { None } else { Some(t) })
        })
        .await
        .map_err(|e| {
            crate::error::CommandError::from(format!("autotag 前置读线程 join 失败：{e}"))
        })?;

    let task = match prelude {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(None), // 不满足守卫：静默跳过（幂等，重复调用无害）
        Err(e) => return Err(crate::error::CommandError::from(e)),
    };

    let user_content = build_user_content(
        &task.title,
        task.note.as_deref(),
        task.subtasks.as_deref().unwrap_or(&[]),
    );
    let system_prompt = format!("{GUIDANCE}{CONTRACT}");

    let raw = match crate::bot_chat::summarize_messages(
        &app,
        &system_prompt,
        &[crate::bot_chat::ChatMsg {
            role: "user".into(),
            content: user_content,
        }],
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::audit::write_event(
                &app,
                crate::audit::AuditLevel::Warn,
                "task_autotag",
                &[
                    ("outcome", "failed".to_string()),
                    ("error", crate::audit::escape_for_log(&e.to_string(), 200)),
                ],
            );
            return Err(e.into());
        }
    };

    let tags = match parse_and_validate_tags(&raw) {
        Ok(t) => t,
        Err(e) => {
            crate::audit::write_event(
                &app,
                crate::audit::AuditLevel::Warn,
                "task_autotag",
                &[
                    ("outcome", "failed".to_string()),
                    ("error", crate::audit::escape_for_log(&e.to_string(), 200)),
                ],
            );
            return Err(e);
        }
    };
    if tags.is_empty() {
        // 模型判断无合适标签：留空即归档成功，不报错（下次不会重试——tags 仍为空，
        // 但守卫已放行过一次；重启后的归档转变才可能再试，可接受）
        crate::audit::write_event(
            &app,
            crate::audit::AuditLevel::Info,
            "task_autotag",
            &[("outcome", "empty".to_string())],
        );
        return Ok(Some(vec![]));
    }

    // 回写：与 task_patch 同一持久化纪律（锁内重读现值 → 白名单 patch → 打戳 → upsert → 广播）
    #[derive(Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct AutotagWritten {
        tags: Vec<String>,
    }
    let row = {
        let app3 = app.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<crate::db::Task, String> {
            let _g = crate::db::lock_db_write();
            let mut conn = crate::db::open_db(&app3)?;
            let Some(mut t) = crate::db::load_task(&conn, &id)? else {
                return Err("任务已不存在".into());
            };
            if t.tags.as_ref().is_some_and(|v| !v.is_empty()) {
                // 并发窗口内别人先写了标签：以先写者为准，不覆盖
                return Ok(t);
            }
            crate::db::apply_task_patch(&mut t, &serde_json::json!({ "tags": tags }))
                .map_err(|e| e.to_string())?;
            crate::db::prepare_for_upsert(&mut t);
            crate::db::upsert_tasks(&conn, std::slice::from_ref(&t))?;
            Ok(t)
        })
        .await
        .map_err(|e| crate::error::CommandError::from(format!("autotag 回写线程 join 失败：{e}")))?
        .map_err(crate::error::CommandError::from)?
    };

    // 广播：挂件 tasks-changed 重读收敛；主窗 tasks-updated 行级合并
    let _ = app.emit("tasks-changed", ());
    let _ = app.emit_to(
        "main",
        "tasks-updated",
        serde_json::json!({
            "source": crate::mutation::MutationOrigin::Main.as_str(),
            "upserts": [row],
            "deletes": []
        }),
    );
    crate::audit::write_event(
        &app,
        crate::audit::AuditLevel::Info,
        "task_autotag",
        &[("tags", row.tags.clone().unwrap_or_default().join(","))],
    );
    Ok(row.tags)
}

// ────────────── 单测 ──────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tolerates_fences_and_plain_array_items() {
        let raw = "```json\n{\"tags\":[\"周报\",\" 数据分析 \",\"周报\"]}\n```";
        let out = parse_and_validate_tags(raw).unwrap();
        assert_eq!(out, vec!["周报", "数据分析"], "trim + 去重");
    }

    #[test]
    fn parse_caps_at_three_tags_and_twelve_chars() {
        let raw = r#"{"tags":["aaaaaaaaaaaaaaaa","bbbbbbbbbbbbbbbb","cc","dd","ee"]}"#;
        let out = parse_and_validate_tags(raw).unwrap();
        assert_eq!(out.len(), MAX_TAGS);
        assert!(out[0].chars().count() <= MAX_TAG_CHARS);
        assert!(!out.contains(&"ee".to_string()), "超量截断");
    }

    #[test]
    fn parse_skips_non_string_and_keeps_empty_result_valid() {
        let out = parse_and_validate_tags(r#"{"tags":[1, null, ""]}"#).unwrap();
        assert!(out.is_empty(), "非字符串/空串跳过，空结果合法");
    }

    #[test]
    fn parse_rejects_non_json_and_missing_tags() {
        assert!(parse_and_validate_tags("抱歉，我无法完成").is_err());
        assert!(parse_and_validate_tags(r#"{"foo": 1}"#).is_err());
    }

    #[test]
    fn user_content_truncates_and_omits_empty_sections() {
        let long_note = "长".repeat(500);
        let c = build_user_content(
            "写周报",
            Some(&long_note),
            &[
                crate::db::Subtask {
                    id: "1".into(),
                    text: "收集数据".into(),
                    done: true,
                },
                crate::db::Subtask {
                    id: "2".into(),
                    text: "  ".into(),
                    done: false,
                },
            ],
        );
        assert!(c.starts_with("标题：写周报"));
        assert!(c.contains("子任务：收集数据"), "空子任务文本被剔除；{c}");
        let note_line = c.lines().find(|l| l.starts_with("备注：")).unwrap();
        assert_eq!(note_line.chars().count(), "备注：".chars().count() + 300);
        let bare = build_user_content("只有标题", None, &[]);
        assert_eq!(bare, "标题：只有标题");
    }
}
