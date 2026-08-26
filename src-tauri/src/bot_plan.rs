//! PREVR 动态规划层（2026-08-26，老板拍板先做第 1+2 层）：
//! - 第 1 层（执行器内验证）在 bot_model_loop：工具失败 → 注入换策略提示；
//!   同工具连续失败 → 要求如实告知用户
//! - 第 2 层（本模块）：复杂任务进主循环前先生成动态计划（Planner），
//!   注入 system prompt；执行中同工具连续失败 → Replan（带失败原因重规划，≤2 次）
//!
//! 设计约束：
//! - 触发要保守：needs_plan 只在明显多步任务时命中（关键词/多附件），
//!   简单问答不多花一次 Planner 调用
//! - Planner 输出非法 / 调用失败 → 降级为原自由循环，不阻断聊天
//! - 计划只是提示词文本，执行仍走 execute_tool 全量安全网关，无绕过通道

use crate::error::CommandResult;

/// 计划状态（随 run_model_loop 走完一轮对话；不入库）
pub struct PlanState {
    /// 用户原始任务文本（Replan 时给 Planner 做上下文）
    pub task: String,
    /// 当前计划步骤
    pub steps: Vec<String>,
    /// 已用 Replan 次数（硬上限 MAX_REPLANS）
    pub replans_used: usize,
}

/// Replan 硬上限：防「失败 → 重规划 → 再失败」死循环烧 token
pub const MAX_REPLANS: usize = 2;

/// 计划步数上限：超出截断（防模型生成几十步的长计划稀释上下文）
const MAX_PLAN_STEPS: usize = 8;

/// 多步意图关键词：命中任一即认为可能需要规划
const MULTI_STEP_KEYWORDS: [&str; 10] = [
    "然后", "之后", "接着", "批量", "分别", "依次", "按步骤", "分步", "逐步", "计划",
];

/// 是否复杂多步任务（保守启发式，纯函数可单测）：
/// - 命中多步关键词，或
/// - 同时含「先」和「再」（先…再…句式），或
/// - 引用多个附件（[附件文件] 块 ≥2）
pub fn needs_plan(text: &str) -> bool {
    if MULTI_STEP_KEYWORDS.iter().any(|k| text.contains(k)) {
        return true;
    }
    if text.contains('先') && text.contains('再') {
        return true;
    }
    text.matches("[附件文件]").count() >= 2
}

/// 解析 Planner 输出为步骤列表：容忍模型在 JSON 外包裹解释文字
/// （找第一个 { 到最后一个 }；或第一个 [ 到最后一个 ]），解析失败/空计划 → None。
pub fn parse_plan(text: &str) -> Option<Vec<String>> {
    let json_str = extract_json(text)?;
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let arr = if let Some(arr) = v.as_array() {
        arr.clone()
    } else if let Some(arr) = v["steps"].as_array() {
        arr.clone()
    } else {
        return None;
    };
    let steps: Vec<String> = arr
        .iter()
        .filter_map(|s| s.as_str().map(|x| x.trim().to_string()))
        .filter(|s| !s.is_empty())
        .take(MAX_PLAN_STEPS)
        .collect();
    if steps.is_empty() {
        None
    } else {
        Some(steps)
    }
}

/// 从模型输出中截取 JSON 片段（对象或数组）
fn extract_json(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{' || b == b'[')?;
    let (open, close) = (bytes[start], if bytes[start] == b'{' { b'}' } else { b']' });
    let _ = open;
    let end = bytes.iter().rposition(|&b| b == close)?;
    if end <= start {
        return None;
    }
    Some(&text[start..=end])
}

/// 计划文本块（注入 system prompt）
pub fn format_plan_block(steps: &[String]) -> String {
    let list = steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n\n【执行计划】\n{list}\n按计划逐步执行；每步完成后对照计划确认产出。某步走不通时不要原样重试，分析原因后调整剩余步骤（换参数/换工具/拆小步骤），并向用户说明调整原因。"
    )
}

/// Planner 系统提示词
const PLANNER_PROMPT: &str = "\
你是任务规划器。把用户任务拆成有序的执行步骤，只输出 JSON：{\"steps\": [\"步骤1\", \"步骤2\", ...]}。\
要求：3-6 步为宜，最多 8 步；每步是一个可执行的具体动作（读文件/搜索/生成文档/操作任务卡等）；\
不要输出 JSON 以外的任何内容；不要嵌套子步骤。";

/// Replan 系统提示词（带失败上下文重规划）
const REPLANNER_PROMPT: &str = "\
你是任务规划器。原计划执行失败了，请基于失败原因修正剩余计划。\
只输出 JSON：{\"steps\": [...]}（只列剩余要做的步骤，已完成的不要重复）。\
要求：不要重复失败的调用方式；换参数/换工具/拆小步骤；最多 8 步；不要输出 JSON 以外的内容。";

/// 调 Planner（单次非流式，60s 超时）。失败 → Err，调用方降级。
async fn call_planner(
    app: &tauri::AppHandle,
    system: &str,
    user: &str,
) -> CommandResult<Vec<String>> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let api_key = crate::bot::read_api_key()?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "stream": false
    });
    let resp = client
        .post(&url)
        .bearer_auth(api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Planner 请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Planner API 错误：{}", resp.status()).into());
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Planner 响应解析失败：{e}"))?;
    let text = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c["message"]["content"].as_str())
        .unwrap_or("");
    parse_plan(text).ok_or_else(|| format!("Planner 输出无法解析为计划：{}", crate::bot::truncate_for_log(text, 200)).into())
}

/// 生成初始计划。失败/解析不出 → None（调用方降级为自由循环，不阻断聊天）。
pub async fn generate_plan(app: &tauri::AppHandle, task: &str) -> Option<Vec<String>> {
    match call_planner(app, PLANNER_PROMPT, task).await {
        Ok(steps) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "plan.generate",
                "steps" => steps.len(),
            );
            Some(steps)
        }
        Err(e) => {
            // 降级不阻断：记 Warn 审计，走原自由循环
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "plan.skip",
                "reason" => e.to_string(),
            );
            None
        }
    }
}

/// Replan：原计划 + 失败原因 → 修正的剩余计划。失败 → None（调用方按原提示词路径收尾）。
pub async fn replan(app: &tauri::AppHandle, plan: &PlanState, fail_reason: &str) -> Option<Vec<String>> {
    let done_note = if plan.steps.is_empty() {
        String::new()
    } else {
        format!("原计划：\n{}\n\n", plan.steps.join("\n"))
    };
    let user = format!(
        "用户任务：{}\n\n{done_note}失败原因：{fail_reason}",
        plan.task
    );
    match call_planner(app, REPLANNER_PROMPT, &user).await {
        Ok(steps) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "plan.replan",
                "steps" => steps.len(),
                "used" => plan.replans_used + 1,
            );
            Some(steps)
        }
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "plan.replan_failed",
                "reason" => e.to_string(),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_plan_hits_multi_step_keywords() {
        assert!(needs_plan("帮我把这三个文件分别润色"));
        assert!(needs_plan("先读文件再生成 Word"));
        assert!(needs_plan("批量处理这些任务"));
        assert!(needs_plan("[附件文件] a.docx [附件文件] b.docx 合并一下"));
    }

    #[test]
    fn needs_plan_passes_simple_requests() {
        assert!(!needs_plan("现在几点"));
        assert!(!needs_plan("帮我新建一个任务：买牛奶"));
        assert!(!needs_plan("[附件文件] a.docx 润色"));
    }

    #[test]
    fn parse_plan_accepts_plain_json_object() {
        let steps = parse_plan(r#"{"steps": ["读文件", "润色", "生成 Word"]}"#).unwrap();
        assert_eq!(steps, vec!["读文件", "润色", "生成 Word"]);
    }

    #[test]
    fn parse_plan_accepts_bare_array_and_wrapped_text() {
        let steps = parse_plan("好的，计划如下：\n[\"步骤一\", \"步骤二\"]\n请查收").unwrap();
        assert_eq!(steps, vec!["步骤一", "步骤二"]);
    }

    #[test]
    fn parse_plan_caps_at_max_steps() {
        let many: Vec<String> = (1..=12).map(|i| format!("步骤{i}")).collect();
        let json = serde_json::json!({"steps": many}).to_string();
        let steps = parse_plan(&json).unwrap();
        assert_eq!(steps.len(), MAX_PLAN_STEPS);
    }

    #[test]
    fn parse_plan_rejects_garbage() {
        assert!(parse_plan("我不会规划").is_none());
        assert!(parse_plan("").is_none());
        assert!(parse_plan(r#"{"steps": []}"#).is_none());
        assert!(parse_plan(r#"{"other": 1}"#).is_none());
    }

    #[test]
    fn format_plan_block_renders_numbered_steps() {
        let block = format_plan_block(&["第一步".into(), "第二步".into()]);
        assert!(block.contains("1. 第一步"));
        assert!(block.contains("2. 第二步"));
        assert!(block.contains("【执行计划】"));
    }
}
