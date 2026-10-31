//! Agent 计划状态（会话内 transient）：`update_plan` 工具写入，`bot-plan` 事件广播。
//!
//! 三条口径（对齐 ACP v1 Agent Plan）：
//! - **整表替换**：每次调用交出完整清单，客户端整体替换——不做增量、不给条目 ID。
//!   增量协议要求客户端与服务端对「哪几条变了」达成一致，模型漏报一条就永久错位；
//! - **三态**：`pending` / `in_progress` / `completed`，任一时刻至多一条 `in_progress`；
//! - **会话内 transient**：不入库，进程重启即失效（与 opencode 同取舍）。
//!
//! 状态存进程内会话表，变更经 `bot-plan` 事件广播（带 sessionId，客户端按会话过滤）。
//! 事件是「变化流」不是「状态源」：面板打开时用 `bot_plan_state` 命令取当前快照。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// 步骤数上限：面板一屏放不下更多条，超了模型多半是在列流水账而不是列步骤。
pub const MAX_PLAN_ENTRIES: usize = 20;
/// 单步文本上限（字符）：一步一句话，说太长就不是步骤了。
pub const MAX_STEP_CHARS: usize = 120;
/// 变更说明上限（字符）：只放「为什么改」，不放清单本身。
pub const MAX_EXPLANATION_CHARS: usize = 400;
/// 会话表容量上限：超了按 updated_at 淘汰最旧的。计划表是进程内临时态，
/// 不该随会话数无限增长。
const MAX_TRACKED_SESSIONS: usize = 64;

/// 计划条目状态（ACP `PlanEntryStatus` 口径）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

/// 计划条目：步骤文本 + 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub content: String,
    pub status: PlanStepStatus,
}

/// 计划快照（整表）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanSnapshot {
    pub steps: Vec<PlanStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub updated_at: i64,
}

/// 面板看到的计划视图：快照 + 本轮起点。
///
/// `turn_started_at` 用来判「计划是不是本轮更新的」：模型这一轮没调
/// `update_plan` 时，面板要如实标出状态可能已过期，而不是让上一轮的
/// `in_progress` 继续冒充实时状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub steps: Vec<PlanStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub updated_at: i64,
    pub turn_started_at: Option<i64>,
}

static PLANS: OnceLock<Mutex<HashMap<String, PlanSnapshot>>> = OnceLock::new();

fn plans() -> &'static Mutex<HashMap<String, PlanSnapshot>> {
    PLANS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 写入本会话的当前计划（整表替换）。
pub fn set_plan(session_id: &str, snapshot: PlanSnapshot) {
    let mut m = plans().lock().unwrap_or_else(|e| e.into_inner());
    if m.len() >= MAX_TRACKED_SESSIONS && !m.contains_key(session_id) {
        // 淘汰最旧的一条：计划是临时态，丢了只是面板少一段历史，
        // 不值得为它常驻内存。
        if let Some(oldest) = m
            .iter()
            .min_by_key(|(_, v)| v.updated_at)
            .map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
    }
    m.insert(session_id.to_string(), snapshot);
}

/// 取本会话的当前计划（无计划 = None）。
pub fn get_plan(session_id: &str) -> Option<PlanSnapshot> {
    let m = plans().lock().unwrap_or_else(|e| e.into_inner());
    m.get(session_id).cloned()
}

/// 丢弃本会话的计划。
pub fn clear_plan(session_id: &str) {
    let mut m = plans().lock().unwrap_or_else(|e| e.into_inner());
    m.remove(session_id);
}

/// 本轮起点（毫秒）：没有跑过就是 None。
fn turn_started_at(session_id: &str) -> Option<i64> {
    crate::bot::stream_log::run_started_at(session_id)
}

/// 组装面板视图：快照 + 本轮起点。
pub fn plan_view(session_id: &str) -> Option<PlanView> {
    get_plan(session_id).map(|s| PlanView {
        steps: s.steps,
        explanation: s.explanation,
        updated_at: s.updated_at,
        turn_started_at: turn_started_at(session_id),
    })
}

/// `bot-plan` 事件载荷。sessionId（信封）由统一出口 `emit_recorded` 盖章，
/// 调用方不带同名键——自带 sessionId 是信封纪律的破口。
pub fn plan_payload(view: &PlanView) -> serde_json::Value {
    serde_json::json!({
        "steps": view.steps,
        "explanation": view.explanation,
        "updatedAt": view.updated_at,
        "turnStartedAt": view.turn_started_at,
    })
}

/// 解析 `update_plan` 入参：整表 + 校验。
///
/// 校验从严：模型写错就当场报错并把可执行的修法说清楚，不猜字段、
/// 不静默丢条目——猜出来的状态会在面板上冒充真实进度。
pub fn parse_update_plan(args: &str) -> Result<(Vec<PlanStep>, Option<String>), String> {
    let v = crate::bot::dispatch::parse_args(args);
    let arr = v["plan"]
        .as_array()
        .ok_or_else(|| "缺少 plan 字段（应为步骤数组）。".to_string())?;
    if arr.is_empty() {
        return Err("plan 是空数组：没有步骤就别调本工具。".to_string());
    }
    if arr.len() > MAX_PLAN_ENTRIES {
        return Err(format!(
            "plan 有 {} 条，超过上限 {MAX_PLAN_ENTRIES} 条：合并同类步骤后再调。",
            arr.len()
        ));
    }
    let mut steps: Vec<PlanStep> = Vec::with_capacity(arr.len());
    let mut in_progress = 0usize;
    for (i, item) in arr.iter().enumerate() {
        let content = item["step"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "第 {} 条缺 step 文本（或不是字符串）：每条都要写清这一步要做什么。",
                    i + 1
                )
            })?;
        let status = match item["status"].as_str() {
            Some("pending") => PlanStepStatus::Pending,
            Some("in_progress") => {
                in_progress += 1;
                PlanStepStatus::InProgress
            }
            Some("completed") => PlanStepStatus::Completed,
            Some(other) => {
                return Err(format!(
                    "第 {} 条的 status 是「{other}」，只认 pending / in_progress / completed。",
                    i + 1
                ))
            }
            None => {
                return Err(format!(
                    "第 {} 条缺 status：每条都要写明 pending / in_progress / completed。",
                    i + 1
                ))
            }
        };
        steps.push(PlanStep {
            content: super::log_sanitize::truncate_chars(content, MAX_STEP_CHARS, true),
            status,
        });
    }
    if in_progress > 1 {
        return Err(format!(
            "有 {in_progress} 条标成了 in_progress：任一时刻只允许一条正在做，其余标 pending。"
        ));
    }
    let explanation = v["explanation"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| super::log_sanitize::truncate_chars(s, MAX_EXPLANATION_CHARS, true));
    Ok((steps, explanation))
}

/// 取本会话当前计划快照（面板打开时拉初值）。
///
/// 事件是变化流：面板晚挂载、或上一轮的事件已经滚出回放缓冲时，
/// 靠这条命令拿回当前状态。没计划就返回 null，不编一份空清单出来。
#[tauri::command]
pub fn bot_plan_state(session_id: String) -> Option<PlanView> {
    plan_view(&session_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_three_states() {
        let (steps, expl) = parse_update_plan(
            r#"{"explanation":"调整了第2步","plan":[
                {"step":"读文件","status":"completed"},
                {"step":"改代码","status":"in_progress"},
                {"step":"跑测试","status":"pending"}]}"#,
        )
        .expect("三态清单应解析成功");
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].status, PlanStepStatus::Completed);
        assert_eq!(steps[1].status, PlanStepStatus::InProgress);
        assert_eq!(steps[2].status, PlanStepStatus::Pending);
        assert_eq!(expl.as_deref(), Some("调整了第2步"));
    }

    #[test]
    fn parse_rejects_two_in_progress() {
        let err = parse_update_plan(
            r#"{"plan":[{"step":"a","status":"in_progress"},{"step":"b","status":"in_progress"}]}"#,
        )
        .expect_err("两条 in_progress 应被拒");
        assert!(err.contains("只允许一条"), "错误要说清怎么改：{err}");
    }

    #[test]
    fn parse_rejects_missing_status_and_step() {
        assert!(parse_update_plan(r#"{"plan":[{"step":"a"}]}"#).is_err());
        assert!(parse_update_plan(r#"{"plan":[{"status":"pending"}]}"#).is_err());
        assert!(parse_update_plan(r#"{"plan":[{"step":"  ","status":"pending"}]}"#).is_err());
    }

    #[test]
    fn parse_rejects_unknown_status_and_empty_plan() {
        assert!(parse_update_plan(r#"{"plan":[{"step":"a","status":"done"}]}"#).is_err());
        assert!(parse_update_plan(r#"{"plan":[]}"#).is_err());
        assert!(parse_update_plan(r#"{}"#).is_err());
    }

    #[test]
    fn parse_caps_step_count() {
        let plan: Vec<String> = (0..MAX_PLAN_ENTRIES + 1)
            .map(|i| format!(r#"{{"step":"s{i}","status":"pending"}}"#))
            .collect();
        let args = format!(r#"{{"plan":[{}]}}"#, plan.join(","));
        let err = parse_update_plan(&args).expect_err("超上限应被拒");
        assert!(err.contains("上限"), "错误要说清上限：{err}");
    }

    #[test]
    fn parse_truncates_long_step_text() {
        let long = "步".repeat(MAX_STEP_CHARS + 50);
        let args = format!(r#"{{"plan":[{{"step":"{long}","status":"pending"}}]}}"#);
        let (steps, _) = parse_update_plan(&args).expect("应解析成功");
        assert_eq!(steps[0].content.chars().count(), MAX_STEP_CHARS + 1);
    }

    #[test]
    fn set_get_clear_roundtrip() {
        let sid = "plan-state-test-session";
        clear_plan(sid);
        assert!(get_plan(sid).is_none());
        set_plan(
            sid,
            PlanSnapshot {
                steps: vec![PlanStep {
                    content: "做一件事".into(),
                    status: PlanStepStatus::InProgress,
                }],
                explanation: None,
                updated_at: 42,
            },
        );
        let got = get_plan(sid).expect("刚写的计划应能读回");
        assert_eq!(got.steps.len(), 1);
        assert_eq!(got.updated_at, 42);
        clear_plan(sid);
        assert!(get_plan(sid).is_none());
    }

    #[test]
    fn payload_carries_steps_and_timestamps() {
        let view = PlanView {
            steps: vec![PlanStep {
                content: "x".into(),
                status: PlanStepStatus::Pending,
            }],
            explanation: Some("e".into()),
            updated_at: 7,
            turn_started_at: Some(3),
        };
        let p = plan_payload(&view);
        assert_eq!(p["updatedAt"], serde_json::json!(7));
        assert_eq!(p["turnStartedAt"], serde_json::json!(3));
        assert_eq!(p["steps"][0]["status"], serde_json::json!("pending"));
        assert_eq!(p["steps"][0]["content"], serde_json::json!("x"));
    }
}
