//! 收尾 JSON 的本地修复与结果判定。
//!
//! 结构化输出三段修复：干净解析 → 本地修复（尾逗号 / 截断闭合）→
//! 带修正提示重试 LLM。本模块只做第二段（零 token、零延迟）。

use crate::bot_orchestrator::{clip, UNPARSEABLE_MARKER};
use crate::db::SubagentStatus;
use tauri::AppHandle;

/// 从文本提取**最后一个**顶层 JSON 对象（设计 §7 解析策略）。
/// 字符串感知的花括号扫描：收集深度 1 的 {...} span，从末往前试 serde 解析，
/// 首个成功且为 object 的返回。找不到/全失败 → None。
/// 文本里所有**花括号配平**的顶层对象片段（按出现顺序）。
///
/// 收尾解析从末往前逐个试这些片段（见 [parse_final_json]）：最后一个对象坏了
/// 就试前一个——与「取最后一个能直接解析的对象」的老语义一致。
pub(crate) fn json_object_spans(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *c == '\\' {
                escaped = true;
            } else if *c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    spans.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    spans
        .into_iter()
        .map(|(a, b)| chars[a..b].iter().collect())
        .collect()
}
/// 被截断的对象片段：最后一个 `{` 到文末。
///
/// 括号配平的对象找不着时才会走到这里——收尾 JSON 写到一半被 max_tokens
/// 截断，就是这种形状。
fn truncated_object_tail(text: &str) -> Option<String> {
    let start = text.rfind('{')?;
    let tail = text[start..].trim();
    if tail.is_empty() {
        None
    } else {
        Some(tail.to_string())
    }
}
/// 收尾 JSON 的**本地修复**（结构化输出三段修复的第 2 段）。
///
/// 业界口径：干净解析 → 本地修复 → 带修正提示重试 LLM。本地修复零 token、
/// 零延迟，必须排在补答之前——补答要再走一次 LLM 轮次。
///
/// 只做两种**不会改坏内容**的修复：
/// - 尾逗号（`{"a":1,}` → `{"a":1}`）
/// - 被截断的闭合（`{"a":1` → `{"a":1}`）
///
/// **不做** `None`/`True` → `null`/`true` 这类字面量替换——字符串内容里出现
/// 这些词是合法的，替换会把模型写的正文字段改坏，比解析失败更糟。
fn repair_json_candidate(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 1) 尾逗号
    let no_commas = strip_trailing_commas(trimmed);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&no_commas) {
        if v.is_object() {
            return Some(no_commas);
        }
    }
    // 2) 截断闭合（在尾逗号修复之后再做一次——两者可以叠加）
    let closed = close_truncated_json(&no_commas)?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&closed) {
        if v.is_object() {
            return Some(closed);
        }
    }
    None
}
/// 去掉结构性的尾逗号（string-aware）：`,` 之后只跟空白再接 `}` / `]` 时删它。
fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut drop = vec![false; chars.len()];
    let mut in_string = false;
    let mut escaped = false;
    for i in 0..chars.len() {
        let c = chars[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            ',' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                    drop[i] = true;
                }
            }
            _ => {}
        }
    }
    if !drop.iter().any(|d| *d) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, c) in chars.iter().enumerate() {
        if !drop[i] {
            out.push(*c);
        }
    }
    out
}
/// 补上被截断的闭合括号（string-aware）。
///
/// 字符串没闭合时**补一个引号再补括号**：模型写到一半被 max_tokens 截断，
/// 常见形态就是截在字符串中间（`{"status":"succeeded","summary":"调研完成`）。
/// 不补就整份报废——status 这类关键字段明明已经写出来了，却跟着一起丢。
/// 补出来的字符串是**残句**，但结构完整、关键字段在，比什么都拿不到强。
///
/// 只在片段末尾才会出现没闭合的字符串（片段本身就是尾巴），所以补引号不会
/// 把后面正常的内容吞进字符串。
fn close_truncated_json(s: &str) -> Option<String> {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in s.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }
    let mut out = String::with_capacity(s.len() + 8);
    out.push_str(s);
    if in_string {
        out.push('"');
    }
    for c in stack.iter().rev() {
        out.push(*c);
    }
    Some(out)
}
/// 单个候选片段的解析（干净 → 本地修复）。
///
/// `require_semantic`：额外要求对象里有 `status`（收尾语义）或 `checks`
/// （verifier 的逐条验收）。只对截断尾段启用——见 [parse_final_json]。
fn parse_json_candidate(candidate: &str, require_semantic: bool) -> Option<serde_json::Value> {
    let v = serde_json::from_str::<serde_json::Value>(candidate)
        .ok()
        .or_else(|| {
            repair_json_candidate(candidate)
                .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
        })?;
    if !v.is_object() {
        return None;
    }
    if require_semantic && v.get("status").is_none() && v.get("checks").is_none() {
        return None;
    }
    Some(v)
}
/// 收尾 JSON 解析（干净解析 + 本地修复）。
///
/// 逐个候选对象从后往前试：最后一个对象坏了就试前一个——与「取最后一个能
/// 直接解析的对象」的老语义一致。最后再试一次**被截断的对象**（最后一个
/// `{` 到文末）：括号配平不上时，截断的 JSON 正是靠这条救。
pub(crate) fn parse_final_json(text: &str) -> Option<serde_json::Value> {
    for candidate in json_object_spans(text).into_iter().rev() {
        if let Some(v) = parse_json_candidate(&candidate, false) {
            return Some(v);
        }
    }
    // 截断的尾段：只在修出来的对象**认得出收尾语义**时才认（有 status 或
    // checks）。半截 JSON 往往连 status 都还没写就断了，修出来是个空壳——
    // 而判定里「没有 status」默认按 succeeded 算，把「没说完」说成「说完了」。
    if let Some(tail) = truncated_object_tail(text) {
        if let Some(v) = parse_json_candidate(&tail, true) {
            return Some(v);
        }
    }
    None
}
/// 工具预算触顶后的强制改判条件（收尾窗语义，纯函数）：
/// 触顶 + 终态还不是 budget_exceeded + 模型没交出可解析的收尾 JSON，
/// 三者同时成立才把终态改判 budget_exceeded(max_tool_calls)。
/// 交了卷的触顶是真实完成——dispatch 收尾窗放行过写产物，改判只会把
/// 父 agent 的「带残留重试」逻辑错误触发。
pub(crate) fn budget_force_needed(
    tool_calls_used: usize,
    max_tool_calls: u32,
    status: SubagentStatus,
    wrapped_up_cleanly: bool,
) -> bool {
    tool_calls_used >= max_tool_calls as usize
        && status != SubagentStatus::BudgetExceeded
        && !wrapped_up_cleanly
}
/// 收尾 JSON 解析失败时的补答（结构化输出三段修复的第 3 段）。
///
/// 业界口径：干净解析 → 本地修复 → 带修正提示重试 LLM。补答只做一次，
/// 且**消耗同一份墙钟预算**（重试不重置时钟）。
///
/// 只在「文本非空 + 不含熔断标记 + 本地修复后仍解析不出」时补答：
/// - 预算熔断（轮数 / 工具 / 墙钟）不是格式问题，补答只是白烧一次 token；
/// - 本地修复能救的已经在 `parse_final_json` 里救掉了，不必再问模型。
///
/// 返回补答后的正文（可解析）；补答失败或仍不可解析 → None（走原失败路径）。
pub(crate) async fn repair_final_json_once(
    app: &AppHandle,
    stop: &crate::bot_slash::StopGuard,
    text: &str,
    wall_budget: Option<crate::bot_model_loop::WallBudget>,
) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    for marker in [
        crate::bot_model_loop::ROUNDS_FUSE_MARKER,
        crate::bot_model_loop::WALL_FUSE_MARKER,
        crate::bot_model_loop::FUSE_MARKER,
    ] {
        if text.contains(marker) {
            return None;
        }
    }
    // 本地修复已经救回来的不算「解析失败」——别为一份能用的结果再烧一次 LLM
    if parse_final_json(text).is_some() {
        return None;
    }
    let prompt = format!(
        "你上一步的收尾输出无法被解析为收尾 JSON。\n\
         请把已经完成的工作整理成下面这个结构。**只输出 JSON 本身**，不要解释、不要调用任何工具：\n\
         {}\n\n\
         你上一步的原文（仅供参考，可能不完整或格式有误）：\n{}",
        crate::prompts::RESULT_SCHEMA_HINT,
        clip(text, 1500),
    );
    let msgs = vec![
        serde_json::json!({ "role": "system", "content": crate::prompts::SUBAGENT_BASE }),
        serde_json::json!({ "role": "user", "content": prompt }),
    ];
    // 一轮就够：补答只需要「把结果整理成 JSON」，不需要再探索。
    let out = crate::bot_model_loop::run_model_loop(
        app.clone(),
        msgs,
        1,
        stop,
        None,
        None,
        None,
        None,
        None,
        wall_budget,
    )
    .await
    .ok()?;
    let (reply, _refs, _trace) = out;
    if parse_final_json(&reply).is_some() {
        crate::bot::audit_log(
            app,
            &format!(
                "subagent_final_json_repaired | 收尾 JSON 补答成功（{} 字符）",
                reply.chars().count()
            ),
        );
        Some(reply)
    } else {
        crate::bot::audit_log(app, "subagent_final_json_repair_failed | 补答后仍不可解析");
        None
    }
}
/// 收尾结果判定（纯函数，设计 §6/§7）：
/// 墙钟熔断 → budget_exceeded(max_wall_seconds) + 正文留产；
/// 轮数熔断 → budget_exceeded(max_turns) + 正文留产；
/// 其他循环错误 → failed；正文解析成功 → 模型自报 succeeded/failed（自报
/// cancelled/budget_exceeded 视为 failed + error 说明）；解析失败 →
/// failed + result_json_unparseable + 原文存 summary（部分结果不丢弃）。
///
/// 这里只看得到子 agent 自己说的话，判不了产物；机器验收在调用方之后才跑，
/// 它的结论由 `reconcile_machine_override` 在落库前回头修正终态——两步分开
/// 是为了让「模型说了什么」与「服务端验到了什么」各自独立可测。
pub(crate) fn classify_outcome(
    run_err: Option<&str>,
    wall_exceeded: bool,
    final_text: Option<&str>,
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    let text = final_text.unwrap_or_default();
    if wall_exceeded {
        // 外层墙钟兜底（future 被 timeout 掐掉）：正文随 future 一起没了，
        // 没有可留的产出。常态路径是轮边界的墙钟熔断（下一条）——那边带正文。
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_wall_seconds".into()),
            None,
        );
    }
    if let Some(err) = run_err {
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(err, 300)),
            None,
        );
    }
    // 墙钟熔断（模型循环在轮边界退出，正文带着）：与轮数熔断同款留产。
    //
    // 必须判在 FUSE_MARKER 之前——墙钟标记是「⏹ 已熔断（墙钟上限）」，
    // 而通用熔断标记是「⏹ 已熔断」，前者**包含**后者；判反了会被认成
    // 工具调用熔断（max_tool_calls），真实原因「墙钟触顶」又被丢掉。
    if text.contains(crate::bot_model_loop::WALL_FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_wall_seconds".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "墙钟预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    // 轮数熔断：熔断出口返回 Ok，正文带 ROUNDS_FUSE_MARKER（见 bot_model_loop）。
    // 终态仍是 budget_exceeded，但正文不是丢弃而是存进 result_json.summary——
    // 跑完的那十几轮里查到的东西对父 agent 仍有价值（判断该不该原目标重试、
    // 还是换 profile / 放宽验收），写 NULL 等于逼重试从零再来。
    if text.contains(crate::bot_model_loop::ROUNDS_FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_turns".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "轮数预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    // 工具熔断：正文带 FUSE_MARKER。轮数标记是它的前缀，所以必须判在轮数之后。
    //
    // 此前这条不认 → 落进 JSON 解析分支：模型照办输出 {"status":"succeeded"}
    // 就报成功（父 agent 因此不重试，产出残缺而无人知晓）；没有收尾 JSON 则
    // 报 failed + result_json_unparseable——真实原因「预算触顶」被丢掉。
    if text.contains(crate::bot_model_loop::FUSE_MARKER) {
        return (
            SubagentStatus::BudgetExceeded,
            Some("max_tool_calls".into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "工具调用预算熔断，未产出收尾 JSON；以上为熔断前保留的最后一段正文"
            })),
        );
    }
    // 干净解析 + 本地修复（尾逗号 / 截断闭合）——结构化输出三段修复的第 1、2 段。
    // 本地修复零 token 零延迟，排在补答（要再走一次 LLM）之前。
    let parsed = parse_final_json(text);
    match parsed {
        Some(v) => {
            // verifier 子 agent 输出独有 schema：含 checks 数组的 verified.json
            // ——只看 status 会漏掉「逐条 acceptance_criteria 验收」的真实语义。
            // 此处按结构识别走独立分支判定，不依赖 stage 字段（verifier 输出已经
            // 结构化，从结构出发比从 stage 出发的耦合面小）。
            if let Some(checks) = v.get("checks").and_then(|c| c.as_array()) {
                return classify_verifier_outcome(&v, checks);
            }
            let claimed = v
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("succeeded");
            match claimed {
                "succeeded" => (SubagentStatus::Succeeded, None, Some(v)),
                "failed" => (
                    SubagentStatus::Failed,
                    Some("model_reported_failure".into()),
                    Some(v),
                ),
                "cancelled" => (
                    SubagentStatus::Failed,
                    Some("model_reported_cancelled".into()),
                    Some(v),
                ),
                "budget_exceeded" => (
                    SubagentStatus::Failed,
                    Some("model_reported_budget_exceeded".into()),
                    Some(v),
                ),
                _ => (SubagentStatus::Succeeded, None, Some(v)),
            }
        }
        None => (
            SubagentStatus::Failed,
            Some(UNPARSEABLE_MARKER.into()),
            Some(serde_json::json!({
                "summary": crate::bot::truncate_for_log(text, 800),
                "note": "收尾 JSON 解析失败，原文截断存此防丢（设计 §7）"
            })),
        ),
    }
}
/// verifier 路径判定：按 `checks[*].met` 综合决定子 agent 终态。
/// - checks 空：summary 含明确否决词（未通过/fail/reject/failed/否决）→ Failed；
///   否则视为 N/A 走 Succeeded（verifier 没排到检查项极少，fail-safe 默认放行）
/// - 全部 met=true → Succeeded
/// - 全部 met=false → Failed + 错误 reason 拼未通过条目的 criterion 列表（≤3 项）
///   给主 agent 看到「到底哪里没通过」的最小信号
/// - 部分 met=true → Succeeded（主 agent 看 result_json 自己决策下一步）
fn classify_verifier_outcome(
    v: &serde_json::Value,
    checks: &[serde_json::Value],
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    let total = checks.len();
    let passed = checks
        .iter()
        .filter(|c| c.get("met").and_then(|m| m.as_bool()) == Some(true))
        .count();
    let summary = v.get("summary").and_then(|s| s.as_str()).unwrap_or("");
    if total == 0 {
        // checks 为空：按 summary 显式否决词判定
        let reject_keywords = ["未通过", "fail", "failed", "reject", "否决", "驳回"];
        let explicit_reject = reject_keywords
            .iter()
            .any(|kw| summary.to_lowercase().contains(&kw.to_lowercase()));
        if explicit_reject {
            return (
                SubagentStatus::Failed,
                Some(format!(
                    "verifier_summary_rejected: {}",
                    crate::bot::truncate_for_log(summary, 80)
                )),
                Some(v.clone()),
            );
        }
        return (SubagentStatus::Succeeded, None, Some(v.clone()));
    }
    if passed == total {
        return (SubagentStatus::Succeeded, None, Some(v.clone()));
    }
    if passed == 0 {
        // 全部未通过：把失败 criterion 列表（前 3 项）拼进 error reason 给主 agent 看
        let failed_list: Vec<String> = checks
            .iter()
            .take(3)
            .filter_map(|c| {
                let criterion = c.get("criterion").and_then(|s| s.as_str())?;
                Some(format!("{criterion}=false"))
            })
            .collect();
        let reason = if failed_list.is_empty() {
            "verifier_all_checks_failed".to_string()
        } else {
            format!("verifier_all_checks_failed: {}", failed_list.join("; "))
        };
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(&reason, 280)),
            Some(v.clone()),
        );
    }
    // 部分通过：判定 Succeeded（主 agent 看 result_json.checks 自行决策下一步）；
    // 这里保持纯函数不写审计——非关键信号，让 caller 在 run_subagent 收尾路径里
    // 决定是否打点（partical-pass 不是错误，重复审计反而拖累日志）。
    (SubagentStatus::Succeeded, None, Some(v.clone()))
}
