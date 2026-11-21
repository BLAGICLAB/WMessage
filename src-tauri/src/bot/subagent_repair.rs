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

/// 熔断出口恢复结果。
#[derive(Debug)]
pub(crate) enum FusedRecovery {
    /// 恢复出可作为收尾正文的文本（已可解析或交给下游补答）：调用方用它替换
    /// 熔断正文，下游 classify 照常走「模型自报终态 / verifier checks 判定」，
    /// 预算触顶但工作实际完成 → 交出收尾 JSON 判真实完成（收尾窗同款口径）。
    Recovered { text: String, via: &'static str },
    /// 恢复不了：调用方维持熔断留产（budget_exceeded），并把对话历史落 sidecar
    /// 供事后诊断与跨进程续跑。
    NotRecovered,
}

/// 熔断出口恢复的探测材料（调用方装配；本模块不持有 AppHandle）。
pub(crate) struct FusedRecoveryCtx<'a> {
    /// 需自评判据数（非机器判据条数）：> 0 才做盘上 verdict 推导
    pub required_checks: usize,
    pub artifact_dir: &'a std::path::Path,
    pub gen_dir: Option<&'a std::path::Path>,
    /// 本轮开跑时刻：盘上报告只认这之后新写的（跨会话旧文件不冒充）
    pub started: std::time::SystemTime,
}

/// 续跑小额度：熔断后原会话自动续跑一次的轮数。不重置满额（业界口径：
/// 重试消耗同一份预算），只给「把已完成的工作收尾」所需的量。
pub(crate) const RESUME_ROUNDS: usize = 10;

/// 剥掉熔断标记取回正文。两种标记（工具 / 轮数）共用 `⏹ 已熔断` 前缀，
/// 切短标记即覆盖两者。正文本身可能是合法收尾 JSON（模型交卷与熔断写标记
/// 同轮发生）——剥掉就能直接解析，零 LLM 成本。
fn strip_fuse_marker(text: &str) -> &str {
    match text.find(crate::bot_model_loop::FUSE_MARKER) {
        Some(i) => text[..i].trim_end(),
        None => text,
    }
}

/// 强制交卷轮：追加「只输出收尾 JSON」提示后发一轮单轮请求。轮内模型又调
/// 工具（循环带熔断标记返回）或解析不出收尾 JSON → None。
/// `msgs` 追加的提示留在历史里（成功与否都如实记录）。
async fn forced_submit_round<F, Fut>(
    msgs: &mut Vec<serde_json::Value>,
    run_rounds: &mut F,
) -> Option<String>
where
    F: FnMut(Vec<serde_json::Value>, usize) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    use crate::bot_model_loop::{FUSE_MARKER, ROUNDS_FUSE_MARKER};

    msgs.push(serde_json::json!({
        "role": "user",
        "content": format!(
            "【系统提示】预算已触顶（熔断），这是唯一的收尾机会。不要再调用任何工具、\
             不要展开任何新核对：把已经完成的工作与结论整理成收尾 JSON，**只输出 JSON 本身**：\n{}",
            crate::prompts::RESULT_SCHEMA_HINT
        ),
    }));
    let history = msgs.clone();
    let out = run_rounds(history, 1).await?;
    if out.contains(FUSE_MARKER) || out.contains(ROUNDS_FUSE_MARKER) {
        return None;
    }
    parse_final_json(&out)?;
    Some(out)
}

/// 盘上 verdict 推导：verifier 已把完整验收报告写进产物区（熔断只截走了交卷
/// 那一步）。报告就是模型自己写的，取回并标注来源，按 verifier 判定口径走。
fn disk_verdict_text(ctx: &FusedRecoveryCtx<'_>) -> Option<String> {
    if ctx.required_checks == 0 {
        return None;
    }
    let report = super::subagent_runner::find_fresh_deliverable_report(
        ctx.artifact_dir,
        ctx.gen_dir,
        ctx.required_checks,
        ctx.started,
    )?;
    let mut v = report.value;
    v["recovered_from"] = serde_json::json!(format!("artifact:{}", report.path.display()));
    Some(v.to_string())
}

/// 熔断出口恢复链（强按顺序，成本从零到高）：
///
/// 1. **剥标记**（零成本）：熔断正文剥掉标记后能解析 → 直接当收尾；
/// 2. **盘上 verdict**（零 LLM）：verifier 的验收报告已完整落盘 → 从制品推导
///    verdict（「交付件即真源」）；
/// 3. **强制交卷轮**（1 轮 LLM）：带着完整对话历史发一轮「预算已尽、只输出
///    收尾 JSON」——历史都在上下文里，verdict 一轮就能交出来；
/// 4. **原会话续跑一次**（≤10 轮 LLM）：以上都失败 → 追加小额度继续完成并交卷，
///    续跑后再走一遍廉价恢复（剥标记 / 盘上 verdict / 单轮强制交卷）。
///
/// 用户已停止 → 整条链让开（取消就是取消，不给机会烧钱）。
/// `msgs` 会被追加恢复用的 user 消息（成功与否都留在历史里，如实记录）；
/// run_rounds 是轮执行注入点（生产 = run_model_loop，测试 = 假轮次脚本）。
pub(crate) async fn recover_fused_exit<F, Fut>(
    stop: &crate::bot_slash::StopGuard,
    msgs: &mut Vec<serde_json::Value>,
    text: &str,
    ctx: &FusedRecoveryCtx<'_>,
    mut run_rounds: F,
) -> FusedRecovery
where
    F: FnMut(Vec<serde_json::Value>, usize) -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    use crate::bot_model_loop::{FUSE_MARKER, ROUNDS_FUSE_MARKER};

    let fused = text.contains(FUSE_MARKER) || text.contains(ROUNDS_FUSE_MARKER);
    if !fused || stop.stopped() {
        return FusedRecovery::NotRecovered;
    }

    // 1. 剥标记：正文已是收尾 JSON 的情形（交卷与熔断标记同轮）
    let stripped = strip_fuse_marker(text);
    if parse_final_json(stripped).is_some() {
        return FusedRecovery::Recovered {
            text: stripped.to_string(),
            via: "fuse_tail_json",
        };
    }

    // 2. 盘上 verdict
    if let Some(text) = disk_verdict_text(ctx) {
        return FusedRecovery::Recovered {
            text,
            via: "disk_verdict",
        };
    }

    // 3. 强制交卷轮
    if !stop.stopped() {
        if let Some(out) = forced_submit_round(msgs, &mut run_rounds).await {
            return FusedRecovery::Recovered {
                text: out,
                via: "forced_submit",
            };
        }
    }

    // 4. 原会话续跑一次：executor 型子 agent 的主恢复路径（verifier 走到这说明
    //    报告没写完，续跑让它补完再交卷）
    if stop.stopped() {
        return FusedRecovery::NotRecovered;
    }
    msgs.push(serde_json::json!({
        "role": "user",
        "content": format!(
            "【系统提示】你此前因预算触顶被中断，没有交出收尾 JSON。现追加 {RESUME_ROUNDS} 轮预算\
             让你完成任务：基于已有进展继续，尽快收尾并输出收尾 JSON（schema 见任务包装末尾）；\
             不要重新开始已完成的步骤，也不要展开与收尾无关的新探索。"
        ),
    }));
    let Some(text2) = run_rounds(msgs.clone(), RESUME_ROUNDS).await else {
        return FusedRecovery::NotRecovered;
    };
    if !text2.contains(FUSE_MARKER) && !text2.contains(ROUNDS_FUSE_MARKER) {
        // 续跑正文交给调用方既有补答链（不可解析时 repair_final_json_once 兜）
        return FusedRecovery::Recovered {
            text: text2,
            via: "resumed",
        };
    }
    let stripped2 = strip_fuse_marker(&text2);
    if parse_final_json(stripped2).is_some() {
        return FusedRecovery::Recovered {
            text: stripped2.to_string(),
            via: "resumed_fuse_tail",
        };
    }
    if let Some(text) = disk_verdict_text(ctx) {
        return FusedRecovery::Recovered {
            text,
            via: "resumed_disk_verdict",
        };
    }
    if !stop.stopped() {
        if let Some(out) = forced_submit_round(msgs, &mut run_rounds).await {
            return FusedRecovery::Recovered {
                text: out,
                via: "resumed_forced_submit",
            };
        }
    }
    FusedRecovery::NotRecovered
}
/// 收尾 JSON 解析失败时的补答（结构化输出三段修复的第 3 段）。
///
/// 业界口径：干净解析 → 本地修复 → 带修正提示重试 LLM。补答只做一次。
///
/// 只在「文本非空 + 不含熔断标记 + 本地修复后仍解析不出」时补答：
/// - 预算熔断（轮数 / 工具）不是格式问题，补答只是白烧一次 token；
/// - 本地修复能救的已经在 `parse_final_json` 里救掉了，不必再问模型。
///
/// 返回补答后的正文（可解析）；补答失败或仍不可解析 → None（走原失败路径）。
pub(crate) async fn repair_final_json_once(
    app: &AppHandle,
    stop: &crate::bot_slash::StopGuard,
    text: &str,
) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    for marker in [
        crate::bot_model_loop::ROUNDS_FUSE_MARKER,
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
    let out =
        crate::bot_model_loop::run_model_loop(app.clone(), msgs, 1, stop, None, None, None, None)
            .await
            .ok()?;
    let (reply, _refs, _final_msgs, _trace) = out;
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
    final_text: Option<&str>,
) -> (SubagentStatus, Option<String>, Option<serde_json::Value>) {
    let text = final_text.unwrap_or_default();
    if let Some(err) = run_err {
        return (
            SubagentStatus::Failed,
            Some(crate::bot::truncate_for_log(err, 300)),
            None,
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

#[cfg(test)]
mod fused_recovery_tests {
    use super::*;
    use crate::bot_slash::StopGuard;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    type BoxFut = std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>>>>;

    /// 假轮次执行器：按脚本逐次返回响应，记录每次请求的轮数额度。
    /// 脚本耗尽后返回 None（模拟 API 报错路径）。
    fn scripted_runner(
        responses: Vec<Option<&'static str>>,
        calls: Arc<Mutex<Vec<usize>>>,
    ) -> impl FnMut(Vec<serde_json::Value>, usize) -> BoxFut {
        let idx = Arc::new(Mutex::new(0usize));
        let responses: Vec<Option<String>> = responses
            .into_iter()
            .map(|r| r.map(|s| s.to_string()))
            .collect();
        move |history: Vec<serde_json::Value>, rounds: usize| -> BoxFut {
            calls.lock().unwrap().push(rounds);
            let _ = history;
            let i = {
                let mut g = idx.lock().unwrap();
                let i = *g;
                *g += 1;
                i
            };
            let resp = responses.get(i).cloned().flatten();
            Box::pin(async move { resp })
        }
    }

    fn ctx_for<'a>(
        dir: &'a std::path::Path,
        required: usize,
        started: SystemTime,
    ) -> FusedRecoveryCtx<'a> {
        FusedRecoveryCtx {
            required_checks: required,
            artifact_dir: dir,
            gen_dir: None,
            started,
        }
    }

    fn base_msgs() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"role": "system", "content": "sys"}),
            serde_json::json!({"role": "user", "content": "task"}),
        ]
    }

    const FUSED: &str = "已核对 40+ 个数字\n\n⏹ 已熔断（轮数上限）：对话轮数超过 200 轮上限（安全保护），已停止后续执行";
    const FUSED_TOOL: &str =
        "进度记录\n\n⏹ 已熔断：本轮 Function 调用超过 400 次上限（安全保护），过限执行已停";
    const SUBMIT_JSON: &str = r#"{"status":"succeeded","summary":"12 条验收 11 过 1 不过"}"#;

    #[tokio::test]
    async fn not_fused_or_stopped_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        // 非熔断正文：整链不进
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            "普通正文没有标记",
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(vec![Some(SUBMIT_JSON)], calls.clone()),
        )
        .await;
        assert!(matches!(out, FusedRecovery::NotRecovered));
        assert!(calls.lock().unwrap().is_empty(), "非熔断不烧恢复轮");
        // 已停止：熔断正文也不烧轮（取消就是取消，不给机会烧钱）
        let stopped = StopGuard::new_for_test();
        stopped.force_stop();
        let out = recover_fused_exit(
            &stopped,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(vec![Some(SUBMIT_JSON)], calls.clone()),
        )
        .await;
        assert!(matches!(out, FusedRecovery::NotRecovered));
        assert!(calls.lock().unwrap().is_empty(), "停止态不烧恢复轮");
    }

    #[tokio::test]
    async fn fuse_tail_json_recovers_without_llm() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        // 交卷与熔断标记同轮：标记前就是合法收尾 JSON
        let fused_with_json: &'static str =
            Box::leak(format!("{SUBMIT_JSON}\n\n{FUSED}").into_boxed_str());
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            fused_with_json,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(vec![Some("不该被调用")], calls.clone()),
        )
        .await;
        match out {
            FusedRecovery::Recovered { text, via } => {
                assert_eq!(via, "fuse_tail_json");
                assert!(text.contains("12 条验收"), "剥标记后正文保留：{text}");
            }
            _ => panic!("应恢复"),
        }
        assert!(calls.lock().unwrap().is_empty(), "零 LLM 成本");
    }

    #[tokio::test]
    async fn disk_verdict_recovers_without_llm() {
        let dir = tempfile::tempdir().unwrap();
        let started = SystemTime::now();
        // 本轮新写的完整验收报告（3 条 checks ≥ 判据数 3）
        let report = dir.path().join("verified.json");
        std::fs::write(
            &report,
            r#"{"checks":[{"criterion":"C1","met":true},{"criterion":"C2","met":true},{"criterion":"C3","met":false}],"summary":"混合"}"#,
        )
        .unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 3, started),
            scripted_runner(vec![Some("不该被调用")], calls.clone()),
        )
        .await;
        match out {
            FusedRecovery::Recovered { text, via } => {
                assert_eq!(via, "disk_verdict");
                assert!(text.contains("recovered_from"), "标注来源：{text}");
                assert!(text.contains("artifact:"), "来源带路径：{text}");
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(v["checks"].as_array().unwrap().len(), 3);
            }
            _ => panic!("应从盘上恢复"),
        }
        assert!(calls.lock().unwrap().is_empty(), "零 LLM 成本");
    }

    #[tokio::test]
    async fn forced_submit_round_runs_one_llm_call() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(vec![Some(SUBMIT_JSON)], calls.clone()),
        )
        .await;
        match out {
            FusedRecovery::Recovered { via, text } => {
                assert_eq!(via, "forced_submit");
                assert!(text.contains("succeeded"));
            }
            _ => panic!("应恢复"),
        }
        assert_eq!(*calls.lock().unwrap(), vec![1], "恰好一次单轮请求");
        assert_eq!(msgs.len(), 3, "交卷提示留进历史");
        assert!(msgs[2]["content"]
            .as_str()
            .unwrap()
            .contains("只输出 JSON 本身"));
    }

    #[tokio::test]
    async fn resume_runs_after_failed_submit_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        // 第 1 次（交卷轮）返回不可解析散文 → 进入续跑；第 2 次（续跑）返回正文
        //（不可解析——交给调用方既有补答链，恢复链按 resumed 交出）
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED_TOOL,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(
                vec![Some("还在核对中…"), Some("收尾：全部完成")],
                calls.clone(),
            ),
        )
        .await;
        match out {
            FusedRecovery::Recovered { via, text } => {
                assert_eq!(via, "resumed");
                assert!(text.contains("全部完成"));
            }
            _ => panic!("应经续跑恢复"),
        }
        assert_eq!(
            *calls.lock().unwrap(),
            vec![1, 10],
            "交卷 1 轮 + 续跑 10 轮"
        );
        assert_eq!(msgs.len(), 4, "交卷提示 + 续跑提示都留进历史");
        assert!(msgs[3]["content"].as_str().unwrap().contains("10 轮预算"));
    }

    #[tokio::test]
    async fn resume_fuse_tail_recovers_strip_first() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        // 交卷轮散文 → 续跑又熔断（标记前带 JSON）→ 剥标记恢复
        let resumed_fused: &'static str = Box::leak(
            format!(
                "{}\n\n{}",
                r#"{"status":"succeeded","summary":"done"}"#, FUSED
            )
            .into_boxed_str(),
        );
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(vec![Some("没交卷"), Some(resumed_fused)], calls.clone()),
        )
        .await;
        match out {
            FusedRecovery::Recovered { via, .. } => assert_eq!(via, "resumed_fuse_tail"),
            _ => panic!("应恢复"),
        }
    }

    #[tokio::test]
    async fn resume_then_disk_verdict_when_report_written_late() {
        let dir = tempfile::tempdir().unwrap();
        let started = SystemTime::now();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        // 报告在续跑轮期间才落盘（模型先写完报告后被熔断的实机形态）：
        // 交卷轮散文 → 续跑熔断、剥标记无 JSON → 盘上 verdict 命中
        let report = dir.path().join("verified.json");
        let writer = report.clone();
        let recorder = calls.clone();
        let run = move |history: Vec<serde_json::Value>, rounds: usize| -> BoxFut {
            recorder.lock().unwrap().push(rounds);
            let _ = history;
            let w = writer.clone();
            Box::pin(async move {
                let _ = std::fs::write(&w, r#"{"checks":[{},{},{}]}"#);
                Some(FUSED.to_string())
            })
        };
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 3, started),
            run,
        )
        .await;
        match out {
            FusedRecovery::Recovered { via, text } => {
                assert_eq!(via, "resumed_disk_verdict");
                assert!(text.contains("recovered_from"));
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(v["checks"].as_array().unwrap().len(), 3);
            }
            _ => panic!("应恢复"),
        }
        assert_eq!(*calls.lock().unwrap(), vec![1, 10]);
    }

    #[tokio::test]
    async fn total_miss_returns_not_recovered_and_history_kept() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut msgs = base_msgs();
        // 交卷轮熔断、续跑又熔断、二段恢复全空 → NotRecovered；msgs 保留全部追加
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            scripted_runner(
                vec![Some(FUSED), Some(FUSED_TOOL), Some(FUSED), None],
                calls.clone(),
            ),
        )
        .await;
        assert!(matches!(out, FusedRecovery::NotRecovered));
        // 交卷 1 + 续跑 10 + 续跑后交卷 1
        assert_eq!(*calls.lock().unwrap(), vec![1, 10, 1]);
        assert_eq!(msgs.len(), 5, "两条恢复提示 + 一次交卷提示追加");
    }

    #[test]
    fn strip_fuse_marker_cuts_both_markers() {
        assert_eq!(strip_fuse_marker(FUSED), "已核对 40+ 个数字");
        assert_eq!(strip_fuse_marker(FUSED_TOOL), "进度记录");
        assert_eq!(strip_fuse_marker("无标记正文"), "无标记正文");
    }

    /// 恢复链拿到的是**循环累积的完整历史**（assistant/tool 消息都在），
    /// 不是入口快照——交卷轮模型面前的上下文必须包含已完成的工作，
    /// 否则要么编造收尾 JSON，要么在空历史上重做任务。
    #[tokio::test]
    async fn forced_submit_receives_full_accumulated_history() {
        let dir = tempfile::tempdir().unwrap();
        let stop = StopGuard::new_for_test();
        let seen: Arc<Mutex<Vec<Vec<serde_json::Value>>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let history_recorder = move |history: Vec<serde_json::Value>, rounds: usize| -> BoxFut {
            calls.lock().unwrap().push(rounds);
            seen2.lock().unwrap().push(history.clone());
            Box::pin(async move { Some(SUBMIT_JSON.to_string()) })
        };
        let mut msgs = base_msgs();
        // 模拟主循环累积：assistant(tool_calls) + tool 结果（入口快照没有这两条）
        msgs.push(serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": [{"id": "call_a", "type": "function",
                "function": {"name": "read_text_file", "arguments": "{}"}}],
        }));
        msgs.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": "call_a",
            "content": "工具真跑过：历史值 42",
        }));
        let out = recover_fused_exit(
            &stop,
            &mut msgs,
            FUSED,
            &ctx_for(dir.path(), 0, SystemTime::now()),
            history_recorder,
        )
        .await;
        assert!(
            matches!(
                out,
                FusedRecovery::Recovered {
                    via: "forced_submit",
                    ..
                }
            ),
            "剥标记失败 → 走交卷轮：{out:?}"
        );
        let histories = seen.lock().unwrap().clone();
        assert!(!histories.is_empty(), "交卷轮应被调用");
        let joined = serde_json::to_string(&histories[0]).unwrap();
        assert!(
            joined.contains("工具真跑过：历史值 42"),
            "交卷轮收到的历史必须包含 tool 结果：{joined}"
        );
        assert!(joined.contains("call_a"), "交卷轮历史应含 tool_calls 身份");
    }
}
