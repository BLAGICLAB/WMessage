//! 收尾层：无工具收尾、自检侧调与出口闸。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{with_thinking, ModelLoopDeps, TurnStatus};
use crate::bot_slash::StopGuard;
use crate::error::CommandError;

/// 终稿自检 trigger 条件：final_text 字符数下限。trivial 回复不值得跑自检——
/// 调小（50）会让「好的/收到」这种短回复也触发，刷 token；调大（200）会漏
/// 短答复+长声明的混合回复。硬编码常量，不暴露设置。
pub(crate) const MIN_FINAL_SELFCHECK_CHARS: usize = 80;

/// 终稿自检 trigger 条件：本会话累计函数调用次数下限。无真实工作量的对话
/// 不该触发——调小（1）会让刚开聊两句就触发，调大（5）会漏掉小工作量对话。
/// 硬编码常量，不暴露设置。
pub(crate) const MIN_FINAL_SELFCHECK_FUNCTION_CALLS: usize = 3;

/// 终稿自检截断注入长度：截 final_text 开头 N 字注入 prompt，覆盖主要声明段
/// （"已完成/已生成"等口胡多数在段首）。N=500 与 SELFCHECK_PROMPT 的 snippet 同
/// 截断逻辑，对齐视觉口径。
pub(crate) const FINAL_SELFCHECK_SNIPPET_CHARS: usize = 500;

/// 终稿自检默认轮次间隔（设置项为 None 时启用）：8 轮 1 次。
const DEFAULT_FINAL_SELFCHECK_ROUND_GAP: u32 =
    crate::bot::constants::FINAL_SELFCHECK_ROUND_GAP_DEFAULT;

/// stop_reason=max_tokens 后的自动续跑配额。续跑是全新请求、带全新输出预算，
/// 一轮通常就够；留 2 是为「首轮续跑仍撞上限」再给一次机会，再多只白烧 token
/// ——截断的成因不会因为多试几次而消失。
const MAX_LENGTH_CONTINUATIONS: usize = 2;

/// 截断续跑的注入文案。正文与工具调用在响应里排在思维链之后，撞上限时被砍掉
/// 的总是它们，文案要同时覆盖「接着写」和「别只说要动手、直接调工具」两种断法。
const LENGTH_CONTINUE_PROMPT: &str = "\
【系统提示】上一条回复超出长度上限被截断。请从断处接着写，不要重复已写的内容。\
如果你刚才正准备调用工具，现在就直接调用它，不要只用文字描述下一步。";

/// 终稿自检 N 判定追加到原文尾部的警告（in-loop 与侧调两条路径共用）。
const FINAL_SELFCHECK_WARN: &str = "\n\n⚠️ 自检未达预期，请人工核对";

/// 出口闸追问「请把真实结果说出来」的上限：模型只回 Y/N 判定时不能无限追着问。
/// 2 次仍不给结果就落到兜底文案（别把裸 Y 交给用户）。追问是完整的一轮 LLM
/// 调用，次数要有硬顶。
const MAX_LATE_VERDICT_RETRIES: u8 = 2;

/// 终稿自检侧调条目的放行判定
///
/// 侧调是**真实带 key 的请求**：条目所属厂商没过连接测试时，主请求被 vendor 门
/// 拦下，侧调请求却照发——等于把 key 发往未验证的 base_url。所以侧调过同一道门。
/// 拦下时回落主路径内的自检（不阻断主流程）+ 审计。
pub(crate) enum SelfcheckDecision {
    /// 放行：条目解析成功且厂商已验证
    Use(crate::bot::config::ResolvedModel),
    /// 不放行：条目解析失败（不存在 / 已停用 / 拿不到 key）
    ResolveFailed(String),
    /// 不放行：厂商没过连接测试（回落主路径内的自检）
    VendorNotVerified { vendor: String, reason: String },
}

/// 侧调条目的放行判定（纯判定，可单测）
pub(crate) fn decide_selfcheck(
    resolved: Result<crate::bot::config::ResolvedModel, crate::error::CommandError>,
    verified_vendors: &[String],
) -> SelfcheckDecision {
    match resolved {
        Err(e) => SelfcheckDecision::ResolveFailed(e.to_string()),
        Ok(r) => match crate::bot::ensure_vendor_verified(r.vendor.as_deref(), verified_vendors) {
            Ok(()) => SelfcheckDecision::Use(r),
            Err(reason) => SelfcheckDecision::VendorNotVerified {
                vendor: r.vendor.clone().unwrap_or_default(),
                reason,
            },
        },
    }
}

/// 无工具轮收尾判决的出口。
pub(crate) enum FinalizeOutcome {
    /// 判决已定：继续走终稿组装（答案定稿 → post_turn 钩子 → 返回）
    Proceed,
    /// 推回消息栈、续跑一轮
    Continue,
    /// 用户叫停：附「已停止」正文（由调用方直接返回）
    Stopped(String),
}

pub(crate) fn finalize_exit(
    expecting_final_selfcheck: &mut bool,
    expecting_selfcheck: &mut bool,
    selfcheck_question_pending: &bool,
    final_text: &mut String,
    late_verdict_retries: &mut u8,
    length_continuations: &mut usize,
    length_prefix: &mut String,
    msgs: &mut Vec<serde_json::Value>,
    deps: &ModelLoopDeps<'_>,
    last_finish_reason: &Option<String>,
    thinking_blocks: &[serde_json::Value],
) -> Result<FinalizeOutcome, CommandError> {
    let emit = deps.emit;
    let audit = deps.audit;
    // stop_reason=max_tokens：思维链把输出预算吃光时，正文和排在最后的
    // tool_use 会整段被砍，用户只拿到半截甚至空回复，还得自己打「继续」。
    // 续跑一轮拿的是全新输出预算，比把半截文本丢回去划算。
    // 自检两态要排除：自检侧调用故意压极小 max_tokens，length 是它的正常
    // 终止形态，在这里续跑只会对着 Y/N 空转。
    if last_finish_reason.as_deref() == Some("length")
        && !*expecting_selfcheck
        && !*expecting_final_selfcheck
        && *length_continuations < MAX_LENGTH_CONTINUATIONS
    {
        *length_continuations += 1;
        length_prefix.push_str(final_text);
        audit(
            crate::audit::AuditLevel::Info,
            "llm.length_continued",
            vec![
                ("attempt", length_continuations.to_string()),
                ("chars", final_text.chars().count().to_string()),
            ],
        );
        msgs.push(with_thinking(
            serde_json::json!({"role": "assistant", "content": final_text}),
            thinking_blocks,
        ));
        msgs.push(serde_json::json!({
            "role": "user",
            "content": LENGTH_CONTINUE_PROMPT
        }));
        return Ok(FinalizeOutcome::Continue);
    }
    // finish_reason=length（token 截断）/content_filter 需要显式感知——
    // 否则半截回复会被当正常答案、空回复无解释
    match last_finish_reason.as_deref() {
        Some("length") => final_text.push_str("\n\n（回复因长度限制被截断，可以让我「继续」补完）"),
        Some("content_filter") if final_text.is_empty() => {
            *final_text = "（回复被服务商的内容过滤拦截，请换个方式提问）".into();
        }
        _ => {}
    }
    // 出口闸：纯 Y/N 单字不是给用户的答复，是自检判定（协议见
    // prompts/selfcheck.rs，in-loop 路径在「expecting_selfcheck」窗口内已
    // 解析过一次）。模型可能隔几轮才回答那句「请仅用一个字回答 Y/N」——
    // 中途它用行动代替了回答，自检态随「以行动代回答」释放了，那句问话
    // 却还留在上下文里，模型在后面某轮补了个 Y。那个 Y 落到这里就成了
    // 终稿：实测事故（真机 2026-10-23）用户只看到一个 Y，真正的报告
    // 压根没产出（模型正要写报告就被这一轮终结了）。
    //
    // 判据：终稿是纯 Y/N 单字 + **自检问句此刻仍悬着**（selfcheck_question_pending，
    // 问句注入置 true、被消费或侧调裁决置 false——「本提问问过自检」不等于
    // 「问句还悬着」，拿节流记忆当判据会把用户要的答复劫走）→ 续跑一轮，
    // 让模型把真实结果说出来。豁免：用户最近一条真实消息里明确要求 Y/N
    // 答复时，裸 Y/N 就是合法答复，闸不拦。
    // 窗口内的判定（expecting_final_selfcheck）不算迟到——它由下面的
    // 终稿自检 verdict 解析接管（Y 还原原文 / N 追加警告），闸不能抢。
    let asked_selfcheck =
        !*expecting_final_selfcheck && *selfcheck_question_pending && !user_demands_yn_answer(msgs);
    if asked_selfcheck && crate::prompts::parse_selfcheck_response(final_text.trim()).is_some() {
        let passed = crate::prompts::parse_selfcheck_response(final_text.trim()) == Some(true);
        if *late_verdict_retries < MAX_LATE_VERDICT_RETRIES {
            *late_verdict_retries += 1;
            let hint = if passed {
                "【系统提示】自检通过。请继续：完成剩余步骤，或直接向用户汇报结果。"
            } else {
                "【系统提示】自检未达预期：请检查本轮的实际结果是否真的达成预期，必要时换策略重做，然后向用户汇报。"
            };
            emit(
                "bot-chat-delta",
                serde_json::json!({
                    "text": if passed { "✅ 自检通过\n\n" } else { "⚠️ 自检未达预期\n\n" }
                }),
            );
            msgs.push(with_thinking(
                serde_json::json!({ "role": "assistant", "content": final_text.clone() }),
                thinking_blocks,
            ));
            msgs.push(serde_json::json!({
                "role": "user",
                "content": hint
            }));
            return Ok(FinalizeOutcome::Continue);
        }
        // 追问额度用尽：模型只肯给判定字样。给一句可读的说明，别把裸 Y
        // 当答复交给用户——那等于没回答。
        *final_text = format!(
            "（模型只回了自检判定「{}」，没有给出可读的结果）",
            final_text.trim()
        );
    }
    Ok(FinalizeOutcome::Proceed)
}

/// 用户显式要求 Y/N 答复的豁免判据：最近一条**真实用户消息**里明确说了
/// 「只回答 Y/N」一类口径。系统注入的 user 消息（自检 prompt【自检】/
/// 【终稿自检】、提示【系统提示】）自带锚点前缀，跳过它们才不会把
/// 「仅回答 Y=无幻觉」的终稿自检问句误当成用户的 Y/N 诉求。
fn user_demands_yn_answer(msgs: &[serde_json::Value]) -> bool {
    const INJECTION_ANCHORS: [&str; 3] = ["【自检】", "【终稿自检】", "【系统提示】"];
    let Some(content) = msgs.iter().rev().find_map(|m| {
        if m.get("role").and_then(|r| r.as_str()) != Some("user") {
            return None;
        }
        // 多模态数组（带图消息）不是 Y/N 文本诉求
        let c = m.get("content")?.as_str()?;
        (!INJECTION_ANCHORS.iter().any(|a| c.contains(a))).then_some(c)
    }) else {
        return false;
    };
    let lower = content.to_lowercase();
    lower.contains("y/n") || lower.contains("y 或 n")
}

pub(crate) async fn finalize_selfcheck(
    expecting_final_selfcheck: &mut bool,
    expecting_selfcheck: &mut bool,
    final_selfcheck_snapshot: &mut Option<String>,
    selfcheck_question_pending: &mut bool,
    final_text: &mut String,
    function_calls_total: &mut usize,
    last_final_selfcheck_round: &mut Option<usize>,
    msgs: &mut Vec<serde_json::Value>,
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
    round: usize,
) -> Result<FinalizeOutcome, CommandError> {
    let emit = deps.emit;
    let audit = deps.audit;
    let skill_finish = deps.skill_finish;
    // 终稿自检节流闸：final_text 不在工具自检态、不在 final 自检态、
    // 字数 ≥ 80、累计函数调用 ≥ 3、距上次 ≥ gap（默认 8 轮；0 = 关闭）→
    // 拍 final_text snapshot + 注入 FINAL_SELFCHECK_PROMPT + 续跑一轮
    // 让模型判 Y/N；本轮不走主出口，模型响应在下一轮迭代末尾解析后还原。
    let final_selfcheck_gap = deps
        .final_selfcheck_round_gap
        .unwrap_or(DEFAULT_FINAL_SELFCHECK_ROUND_GAP) as usize;
    if final_selfcheck_gap > 0
        && !*expecting_selfcheck
        && !*expecting_final_selfcheck
        && final_text.chars().count() >= MIN_FINAL_SELFCHECK_CHARS
        && *function_calls_total >= MIN_FINAL_SELFCHECK_FUNCTION_CALLS
        && last_final_selfcheck_round.is_none_or(|r| round.saturating_sub(r) >= final_selfcheck_gap)
    {
        // 用户已停止：不再做终稿自检。侧调一次最多烧 60s，期间 UI 停在
        // 「收尾中 · 校验答复」——用户看着像停止失灵。
        if stop.stopped() {
            let hint = skill_finish(false, "用户停止");
            return Ok(FinalizeOutcome::Stopped(format!(
                "{final_text}\n\n⏹ 已停止{hint}"
            )));
        }
        // settling：正文已经完整输出，用户「看完了」，但模型还要跑一次
        // 自检（可静默数十秒）。这一步不发生，用户会把正文结束误读成
        // 「本轮结束」——ZCode 的 settling 就是为这个场景存在的。
        if let Some(cb) = deps.turn_status {
            cb(TurnStatus::Settling, "收尾中 · 校验答复");
        }
        let snippet: String = final_text
            .chars()
            .take(FINAL_SELFCHECK_SNIPPET_CHARS)
            .collect();
        *last_final_selfcheck_round = Some(round);
        if let Some(sc_http) = deps.final_selfcheck_http {
            // 侧调路径：独立小请求只带 snippet（几百字符）——省主循环
            // 整段历史重付（agent 输入输出比 ~100:1，主循环多一轮 =
            // 全史重付一次）。判定失败/请求错误 → 视作通过原文照常
            // 返回（与 in-loop 附解释语义一致），只留审计。
            *final_selfcheck_snapshot = Some(final_text.clone());
            let prompt = crate::prompts::FINAL_SELFCHECK_PROMPT.replace("{snippet}", &snippet);
            let side_msgs = [crate::bot_chat::ChatMsg {
                role: "user".to_string(),
                content: prompt,
            }];
            let sidecall = crate::bot_chat::summarize_http(
                &sc_http.client,
                &sc_http.base_url,
                &sc_http.api_key,
                &sc_http.model,
                "",
                &side_msgs,
                sc_http.provider,
                // 判定 prompt 要求「仅回答 Y/N」——32 token 足够首字
                // 符判定，截断不影响宽松前缀解析
                32,
                None,
                None,
            )
            .await;
            match sidecall {
                Ok(v) => {
                    if crate::prompts::parse_selfcheck_reply_lenient(v.trim()) == Some(false) {
                        if let Some(snapshot) = final_selfcheck_snapshot.take() {
                            *final_text = format!("{snapshot}{FINAL_SELFCHECK_WARN}");
                            emit(
                                "bot-chat-delta",
                                serde_json::json!({ "text": FINAL_SELFCHECK_WARN }),
                            );
                            audit(
                                crate::audit::AuditLevel::Warn,
                                "final_selfcheck.fail",
                                vec![("detail", crate::bot::truncate_for_log(&snapshot, 200))],
                            );
                        }
                    }
                }
                Err(e) => (deps.audit)(
                    crate::audit::AuditLevel::Warn,
                    "final_selfcheck.sidecall_failed",
                    vec![("err", e.to_string())],
                ),
            }
            *final_selfcheck_snapshot = None;
            // 侧调已裁决（无论成败，本触发不再走 in-loop）：问句不悬着，
            // 出口闸的迟到判定不得劫走模型后续自己的 Y/N 答复
            *selfcheck_question_pending = false;
            // fall through：主出口照常返回，侧调不占主循环轮次
        } else {
            // in-loop 路径（未配置侧调模型）：注入 prompt 续跑一轮，
            // 模型响应在下一轮迭代末尾解析后还原
            *final_selfcheck_snapshot = Some(final_text.clone());
            msgs.push(serde_json::json!({
                "role": "user",
                "content": crate::prompts::FINAL_SELFCHECK_PROMPT
                    .replace("{snippet}", &snippet),
            }));
            *expecting_final_selfcheck = true;
            *selfcheck_question_pending = true;
            return Ok(FinalizeOutcome::Continue);
        }
    }
    // 终稿自检 verdict 解析：本轮是上一轮触发的 final 自检响应
    // （expecting_final_selfcheck=true），Y/N 字样已被流式跳 emit，
    // 据 verdict 决定还原 final_text：Y→原文；N→原文+⚠️+emit+audit。
    // 判定用宽松前缀版 parse_selfcheck_reply_lenient——模型答「N，…」
    // 带解释时整段严格匹配得 None，幻觉自白会被当通过静默丢弃。
    // Y 路径不写 audit_log（成功是常态、记了会刷屏）。
    if *expecting_final_selfcheck {
        match crate::prompts::parse_selfcheck_reply_lenient(final_text.trim()) {
            Some(false) => {
                if let Some(snapshot) = final_selfcheck_snapshot.take() {
                    *final_text = format!("{snapshot}{FINAL_SELFCHECK_WARN}");
                    emit(
                        "bot-chat-delta",
                        serde_json::json!({ "text": FINAL_SELFCHECK_WARN }),
                    );
                    audit(
                        crate::audit::AuditLevel::Warn,
                        "final_selfcheck.fail",
                        vec![("detail", crate::bot::truncate_for_log(&snapshot, 200))],
                    );
                }
            }
            Some(true) | None => {
                if let Some(snapshot) = final_selfcheck_snapshot.take() {
                    *final_text = snapshot;
                }
            }
        }
        // expecting_final_selfcheck 在此处置 false 之后立即 return，flag 不
        // 再被任何流式 emit 检查读取——下次循环触发由 trigger 分支重新置 true。
        *expecting_final_selfcheck = false;
        // 问句已被这句 Y/N 回答（或快照已还原）：悬着标志同步收掉
        *selfcheck_question_pending = false;
    }
    Ok(FinalizeOutcome::Proceed)
}
