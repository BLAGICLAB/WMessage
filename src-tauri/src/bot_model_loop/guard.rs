//! 防幻觉守卫层：终稿声称与实际动作对账，以及自检回复的判定。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{
    claim_mutation_fires, context_sources, finalize_exit, finalize_selfcheck, number_sourced,
    with_thinking, FinalizeOutcome, ModelLoopDeps, ToolEvidence,
};
use crate::bot::registry::{
    claimed_doc_paths, claimed_executed_tools, claims_execution_with, mentioned_tool_names,
    path_missing_on_disk,
};
use crate::bot_slash::StopGuard;
use crate::error::CommandError;

/// 幻觉守卫的证据（收齐后交给判决）。
struct GuardEvidence {
    /// 声称已生成但路径在磁盘上不存在
    fake_gen_path: Option<String>,
    /// 声称已生成但没有落盘凭证
    unreceipted_path: Option<String>,
    /// 终稿点名但本次提问与会话历史都没有成功凭证的工具
    unsupported: Vec<&'static str>,
    /// 执行类粗判命中（终稿不点名任何工具却在叙述一次执行）
    claim_execution: bool,
    /// 终稿报出的数在工具返回与上下文里都找不到出处
    unsourced_value: Option<String>,
    /// 结论无工具支撑（本次提问的调用全是回声）
    echo_backed: bool,
    /// 变更类声称命中（声称完成但本轮没动过手）
    claim_mutation: bool,
}

/// 无工具轮的收尾判决：自检回复 → 幻觉守卫 → 出口段 → 终稿自检。
///
/// 判决未定时返回 `Continue`（推回消息栈、续跑一轮）；定案后返回 `Proceed`，
/// 由调用方接着做终稿组装（答案定稿 → post_turn 钩子 → 返回）；用户叫停
/// 返回 `Stopped`（附「已停止」正文，由调用方直接返回）。
pub(crate) async fn finalize_without_tools(
    msgs: &mut Vec<serde_json::Value>,
    mutation_done: &mut bool,
    claim_retry_used: &mut bool,
    fabrication_notice: &mut Option<String>,
    receipts: &[String],
    evidence: &[ToolEvidence],
    written_paths: &[String],
    session_tools_cache: &mut Option<Option<crate::db::trace::SessionTools>>,
    function_calls_total: &mut usize,
    last_selfcheck_round: &mut Option<usize>,
    pending_selfcheck: &mut Option<(String, String)>,
    expecting_selfcheck: &mut bool,
    expecting_final_selfcheck: &mut bool,
    final_selfcheck_snapshot: &mut Option<String>,
    last_final_selfcheck_round: &mut Option<usize>,
    late_verdict_retries: &mut u8,
    length_continuations: &mut usize,
    length_prefix: &mut String,
    final_text: &mut String,
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
    round: usize,
    thinking_blocks: &[serde_json::Value],
    last_finish_reason: &Option<String>,
) -> Result<FinalizeOutcome, CommandError> {
    match finalize_selfcheck_reply(
        msgs,
        pending_selfcheck,
        expecting_selfcheck,
        final_text,
        deps,
        thinking_blocks,
    )? {
        FinalizeOutcome::Proceed => {}
        other => return Ok(other),
    }
    let ev = guard_collect(
        msgs,
        mutation_done,
        receipts,
        evidence,
        written_paths,
        session_tools_cache,
        final_text,
        deps,
        stop,
    )?;
    match guard_verdict(
        msgs,
        claim_retry_used,
        fabrication_notice,
        final_text,
        deps,
        ev,
        thinking_blocks,
    )? {
        FinalizeOutcome::Proceed => {}
        other => return Ok(other),
    }
    match finalize_exit(
        expecting_final_selfcheck,
        expecting_selfcheck,
        final_text,
        last_final_selfcheck_round,
        last_selfcheck_round,
        late_verdict_retries,
        length_continuations,
        length_prefix,
        msgs,
        deps,
        last_finish_reason,
        thinking_blocks,
    )? {
        FinalizeOutcome::Proceed => {}
        other => return Ok(other),
    }
    match finalize_selfcheck(
        expecting_final_selfcheck,
        expecting_selfcheck,
        final_selfcheck_snapshot,
        final_text,
        function_calls_total,
        last_final_selfcheck_round,
        msgs,
        deps,
        stop,
        round,
    )
    .await?
    {
        FinalizeOutcome::Proceed => {}
        other => return Ok(other),
    }
    Ok(FinalizeOutcome::Proceed)
}

fn finalize_selfcheck_reply(
    msgs: &mut Vec<serde_json::Value>,
    pending_selfcheck: &mut Option<(String, String)>,
    expecting_selfcheck: &mut bool,
    final_text: &mut str,
    deps: &ModelLoopDeps<'_>,
    thinking_blocks: &[serde_json::Value],
) -> Result<FinalizeOutcome, CommandError> {
    let emit = deps.emit;
    // 防幻觉汇报守卫（前置：每轮无工具调用的文本产出点即检查，当场打回，
    // 不等会话收尾）三个支柱：
    // a) claims_mutation——声称完成变更但本轮没动过手；
    // b) claimed_doc_paths + 存在性——声称已生成文档且给出路径，但路径在
    //    磁盘上不存在（编造的路径经不起存在性检查，实测窄词表漏检的话术
    //    靠它兜住）
    // c) claimed_doc_paths + 落盘凭证——路径在磁盘上，但它从来不是任何
    //    工具写出来的：模型在声称「已生成」一个跟自己无关的文件
    // 自检回复解析：上一轮注入了 SELFCHECK_PROMPT（pending 仍在场、
    // expecting_selfcheck 为真）。纯 Y/N 不是给用户的答案：
    // Y → ✅ 视觉提示后续跑一轮，让模型产出正常汇报（否则回合被自检
    //     终结，用户的请求得不到真实答复）；
    // N → ⚠️ 视觉提示 + 直接 push 换策略提示后续跑（PREVR 第 1 层同源
    //     文案；fail_hint_queued 的注入点在工具回填之后，自检回复轮
    //     没有工具调用到不了那里，只能就地 push）；
    // 附解释 → 自检外还有真实回答，按正常答案走主出口终结回合。
    if *expecting_selfcheck {
        let sc_name = pending_selfcheck
            .as_ref()
            .map(|(n, _)| n.clone())
            .unwrap_or_default();
        match crate::prompts::parse_selfcheck_response(final_text.trim()) {
            Some(true) => {
                emit(
                    "bot-chat-delta",
                    serde_json::json!({ "text": "✅ 自检通过\n\n" }),
                );
                msgs.push(with_thinking(
                    serde_json::json!({ "role": "assistant", "content": final_text }),
                    thinking_blocks,
                ));
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": "【系统提示】自检通过。请继续：完成剩余步骤，或直接向用户汇报结果。"
                }));
                *expecting_selfcheck = false;
                *pending_selfcheck = None;
                return Ok(FinalizeOutcome::Continue);
            }
            Some(false) => {
                emit(
                    "bot-chat-delta",
                    serde_json::json!({ "text": "⚠️ 自检未达预期,换策略中…\n\n" }),
                );
                msgs.push(with_thinking(
                    serde_json::json!({ "role": "assistant", "content": final_text }),
                    thinking_blocks,
                ));
                msgs.push(serde_json::json!({
                            "role": "user",
                            "content": format!(
                                "【系统提示】你对工具 {sc_name} 的自检回复了「未达预期」。不要重复同样的调用；分析原因后换策略：换参数、换工具、或把任务拆成更小的步骤再试一次。"
                            )
                        }));
                *expecting_selfcheck = false;
                *pending_selfcheck = None;
                return Ok(FinalizeOutcome::Continue);
            }
            None => {
                // 模型附了解释说明：emit 原 final_text（之前 SSE 已跳 emit）
                emit("bot-chat-delta", serde_json::json!({ "text": final_text }));
                *expecting_selfcheck = false;
                *pending_selfcheck = None;
            }
        }
    }
    Ok(FinalizeOutcome::Proceed)
}

fn guard_collect(
    msgs: &mut Vec<serde_json::Value>,
    mutation_done: &mut bool,
    receipts: &[String],
    evidence: &[ToolEvidence],
    written_paths: &[String],
    session_tools_cache: &mut Option<Option<crate::db::trace::SessionTools>>,
    final_text: &mut str,
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
) -> Result<GuardEvidence, CommandError> {
    // 声称产出的文档路径：两道校验。
    //
    // · 存在性——路径在磁盘上不在，就是凭空写的路径（原有判据）。
    // · 落盘凭证——路径在，但它从来不是任何工具写出来的：模型在声称
    //   「已生成」一个跟自己无关的文件。这道是存在性查不出来的——上周
    //   生成过、或者用户自己放在那儿的文件，存在性一律放行。
    //
    // 凭证来源两处：本提问写过的路径在进程内（不等痕迹落库），历史上
    // 写过的查库。两处都没有才是编造。
    let claimed_paths = claimed_doc_paths(final_text);
    let mut fake_gen_path: Option<String> = None;
    for p in &claimed_paths {
        if path_missing_on_disk(p) {
            fake_gen_path = Some(p.clone());
            break;
        }
    }
    // 本次提问跑过「自己能写盘、我们无从确证」的工具（run_python 无文件
    // 系统隔离、MCP 工具在外部服务里执行）——它们能写出文件却不经过
    // 我们的凭证，「没凭证」推不出「不是工具产物」，这两道闸都必须让开。
    //
    // 口径是**本次提问**，不是整个会话：早先按会话判，一次 run_python
    // 会把这条会话后续所有提问的凭证闸全打死。
    //
    // 派过子 agent 也让开：子 agent 的调用不落我们的痕迹表（它的
    // trace_id 是 'trc_xxx' 字符串，与 exec_spans 的整数 trace_id 类型
    // 都对不上，join 永远不成立），它写的文件在这道闸眼里永远没凭证。
    let unverifiable_ran = receipts
        .iter()
        .any(|n| n == "run_python" || n.starts_with("mcp_"))
        || receipts.iter().any(|n| n == "spawn_subagent");
    // 落盘凭证这道要在存在性过了之后才问：路径压根不存在时已经判编造，
    // 没必要再查库。
    let mut unreceipted_path: Option<String> = None;
    if fake_gen_path.is_none() && !claimed_paths.is_empty() && !unverifiable_ran {
        let pending: Vec<String> = claimed_paths
            .iter()
            .filter(|p| !written_paths.contains(&p.to_lowercase()))
            .cloned()
            .collect();
        if !pending.is_empty() {
            if let Some(fetch) = deps.file_receipts {
                unreceipted_path = fetch(stop.session_id(), &pending).into_iter().next();
            }
        }
    }
    // 「动过手」的凭据：变更类工具成功（mutation_done），**或**跑过
    // 不可核验但确实改状态的工具（run_python / MCP 工具 / 子 agent）。
    //
    // 后者的副作用核验不了凭证，但「没凭证」推不出「没动手」——少了
    // 这一条，「用 Python 写完文件并如实汇报」会被守卫当成凭空汇报：
    // 变更声称命中（write_file 的「已写入」等），而 mutation_done 是
    // 假的，守卫把终稿推回让模型重跑，副作用翻倍，末尾还会挂一条
    // 「这句没有对应凭证」的更正。口径与上面落盘凭证那道闸一致。
    let claim_mutation = claim_mutation_fires(*mutation_done, unverifiable_ran, final_text);
    // 执行类谎报：本回合零工具成功，终稿却在叙述执行结果。此前守卫只有
    // mutating 一档，run_python / fetch_url 这类非 mutating 工具可以被凭空
    // 叙述——实测两起：原话一「执行完毕，sleep 120 秒后工具真实返回，打印了
    // done」，原话二「完成计划2 步：第一步触发 ModuleNotFoundError，第二步仍
    // 输出 done」，两条对应 trace 都是 turns=0 tools=0、耗时 1.9s/1.8s。
    //
    // 判决只看凭证表，不看轮次：终稿点名的工具逐个查本次提问的凭证池，
    // 任何一个查不到就是伪造。旧的「本回合有没有任何工具成功」是个布尔，
    // 挡不住「本轮成功调了 A、却编造 B 的执行结果」——同一个提问里
    // 不同工具的凭证不能互相担保。
    let mentioned = mentioned_tool_names(final_text);
    let table_hit = claims_execution_with(final_text, deps.guard_claim_keywords);
    // 终稿里「点名了某次执行」的工具：执行形态词与工具名落在同一句内。
    //
    // 词表收的全是完成态 / 成功态的措辞（失败形态「调用失败」「调用被拒」
    // 刻意不收），所以「词贴在这个工具名旁边」= 模型在报这个工具跑成了。
    // 触发层按工具名逐个判，而不是全局判一次：全局判会把「工具调用结果：
    // - run_python：调用失败」这种如实汇报失败也拖下水——词在标题那一行，
    // 工具名在下一行，说的不是同一件事。
    let claimed = claimed_executed_tools(final_text, deps.guard_claim_keywords);
    // 本次提问派过子 agent → 点名判定让开。子 agent 的调用不落我们的痕迹表
    // （trace_id 类型都对不上），主 agent 如实汇报「子 agent 跑完了 run_python」
    // 在主池里查不到这个名字——查不到不等于没跑过。
    let subagent_spawned = receipts.iter().any(|n| n == "spawn_subagent");
    // 会话工具名单（本次提问之前本会话调用过 / 成功过的工具）：跨提问的
    // 重述是真话（「刚才 create_word 生成的那个」），排除集要覆盖本会话历史。
    // 惰性拉一次，且只在判据用得上时拉——正常路径零读库。
    let need_session_tools = !claimed.is_empty();
    let session_tools: Option<crate::db::trace::SessionTools> = if !need_session_tools {
        None
    } else {
        match &*session_tools_cache {
            Some(v) => v.clone(),
            None => {
                let v = deps.session_tools.and_then(|f| f(stop.session_id()));
                *session_tools_cache = Some(v.clone());
                v
            }
        }
    };
    let subagent_in_session = match &session_tools {
        // 查不到历史 → 让开（薄壳没这个注入点、或查库失败）
        None => true,
        Some(st) => st.called.iter().any(|n| n == "spawn_subagent"),
    };
    // 点名判据统一的开闸条件：派过子 agent、或查不到本会话历史时整档让开
    // （查不到证据不等于有罪）。
    let point_check_open = !subagent_spawned && !subagent_in_session;
    let session_succeeded: &[String] = session_tools
        .as_ref()
        .map(|st| st.succeeded.as_slice())
        .unwrap_or(&[]);
    // 点名判据（独立判据）：终稿点名某工具跑过，而那个工具**从没成功返回
    // 过**——编造的一次执行。
    //
    // 判决口径是「没成功过」而不是「没调用过」：调用过但失败、却报成功，
    // 正是这道要抓的；而如实汇报失败（词表不收失败形态）根本进不了触发
    // 层，不会被牵连。
    //
    // 排除集两处：本次提问的成功凭证与本会话历史里的成功记录（跨提问的
    // 重述是真话）。
    let unsupported: Vec<&str> = if !point_check_open {
        Vec::new()
    } else {
        claimed
            .iter()
            .filter(|t| {
                !receipts.iter().any(|r| r == *t) && !session_succeeded.iter().any(|n| n == *t)
            })
            .copied()
            .collect()
    };
    // 执行类粗判：终稿不点名任何工具（无从按名查）却在叙述一次执行，
    // 而本次提问一条成功凭证都没有——退回整条提问零凭证的粗判，这正是
    // 词表要顶住的那类（「执行完毕，工具真实返回…」）。
    let claim_execution = table_hit && mentioned.is_empty() && receipts.is_empty();
    // 判据一（结果无出处）：终稿把一个数当成执行产物报出来，而那个数在
    // 本次提问的工具返回与对话上下文里都找不到出处。
    //
    // 凭证表答不了这一问：它记的是「哪个工具成功过」，不是「这个数是哪
    // 来的」。同一个提问里调过工具，不代表报出来的数就是那个工具给的
    // ——「已跑完，输出 5050」而工具给的是别的数，就是编的。
    //
    // 让开两处：派过子 agent（它的返回不落我们的池子）、终稿如实说了
    // 执行没做成（那它给的是自己的推算，不是把执行结果编出来）。
    let claimed_numbers =
        crate::bot::registry::claimed_result_numbers(final_text, deps.guard_claim_keywords);
    let mut sources: Vec<String> = context_sources(&*msgs);
    sources.extend(evidence.iter().filter(|e| e.ok).map(|e| e.output.clone()));
    let unsourced_value: Option<String> = if claimed_numbers.is_empty()
        || subagent_spawned
        || crate::bot::registry::states_execution_not_done(final_text)
    {
        None
    } else {
        claimed_numbers
            .iter()
            .find(|n| !number_sourced(n, &sources))
            .cloned()
    };
    // 判据二（回声调用）：本次提问的成功调用全都没取回新信息——返回内容
    // 就是模型自己写进参数的那句。终稿再拿它当结论的依据，就是自证。
    //
    // 口径是「有没有别的调用取回过真东西」：同一提问里若还有一次读到现
    // 场的调用（真机 trace 106 的 run_python），结论就有依据，不该拦。
    let echo_only = evidence.iter().any(|e| e.ok)
        && evidence
            .iter()
            .filter(|e| e.ok)
            .all(|e| crate::bot::registry::is_echo_call(&e.args, &e.output));
    let echo_backed = echo_only
        && !subagent_spawned
        && (crate::bot::registry::states_a_result(final_text) || !mentioned.is_empty());
    Ok(GuardEvidence {
        fake_gen_path,
        unreceipted_path,
        unsupported,
        claim_execution,
        unsourced_value,
        echo_backed,
        claim_mutation,
    })
}

fn guard_verdict(
    msgs: &mut Vec<serde_json::Value>,
    claim_retry_used: &mut bool,
    fabrication_notice: &mut Option<String>,
    final_text: &mut String,
    deps: &ModelLoopDeps<'_>,
    ev: GuardEvidence,
    thinking_blocks: &[serde_json::Value],
) -> Result<FinalizeOutcome, CommandError> {
    let GuardEvidence {
        fake_gen_path,
        unreceipted_path,
        unsupported,
        claim_execution,
        unsourced_value,
        echo_backed,
        claim_mutation,
    } = ev;
    let audit = deps.audit;
    // 幻觉守卫判决：声称完成但无对应凭证 → 第一次返工，第二次如实标注。
    //
    // 两次是两种不同处置，不是同一种做两遍：
    // · 第一次（claim_retry_used=false）：把终稿推回消息栈、注入系统提示
    //   让模型真去调工具。此时谎报还只在消息栈里——前端收尾时用 bot_chat
    //   的返回值覆盖流式气泡，用户最终看到的是返工后的那一版。
    // · 第二次（claim_retry_used=true）：不再返工。返工会让模型重跑已完成
    //   的动作（delete_task / link_file_to_task 这类是重复的破坏性操作），
    //   而谎报已经拦过一次，再拦一次收益不抵风险。改为把「这句没有对应
    //   凭证」如实标在终稿末尾：让用户看见谎话 + 一条更正，好过让谎话
    //   冒充事实单独到达用户。
    if claim_mutation
        || claim_execution
        || !unsupported.is_empty()
        || fake_gen_path.is_some()
        || unreceipted_path.is_some()
        || unsourced_value.is_some()
        || echo_backed
    {
        // kind 的优先级与下面的 reason 一致：先说最硬的那条（路径压根
        // 不存在 / 路径不是工具产物），再说点名判据、执行类粗判，然后是
        // 结果无出处与回声调用，最后变更类。
        //
        // 粗判排在结果无出处前面：零凭证时「本次提问没有任何工具调用
        // 成功」是更完整的根因说法——那种情形下结果值必然也没出处，
        // 换个说法反而丢信息。两条新判据顶的是粗判够不着的地界：工具
        // 确实调过（有凭证），但报出来的数不是它给的。
        let kind = if fake_gen_path.is_some() {
            "声称已生成但路径不存在".to_string()
        } else if unreceipted_path.is_some() {
            "声称已生成但无落盘凭证".to_string()
        } else if !unsupported.is_empty() {
            format!("声称已执行但无对应凭证: {}", unsupported.join(","))
        } else if claim_execution {
            "声称已执行但本次提问零凭证".to_string()
        } else if let Some(v) = &unsourced_value {
            format!("声称的结果无出处: {v}")
        } else if echo_backed {
            "结论无工具支撑".to_string()
        } else {
            "声称变更但未调工具".to_string()
        };
        // 同一处判决的两种口吻：reason 说给模型听（第二人称、要它改正），
        // notice 说给用户听（第三人称、只陈述事实）。
        let (reason, notice) = match (
                    &fake_gen_path,
                    &unreceipted_path,
                    claim_execution,
                    unsupported.as_slice(),
                    unsourced_value.as_deref(),
                    echo_backed,
                ) {
                    (Some(p), _, _, _, _, _) => (
                        format!(
                            "你声称已生成文件 {p}，但该路径在磁盘上不存在——文件实际没有生成。"
                        ),
                        format!("上面提到的文件 {p} 实际没有生成——磁盘上不存在这个路径。"),
                    ),
                    (None, Some(p), _, _, _, _) => (
                        format!(
                            "你声称已生成文件 {p}，但这个路径没有落盘凭证——本次提问的落盘记录和历史痕迹里都没有它，这个文件不是本次生成的。"
                        ),
                        format!(
                            "上面提到的文件 {p} 不是本次生成的——这个路径没有落盘凭证。"
                        ),
                    ),
                    (None, None, _, [t, ..], _, _) => (
                        format!(
                            "你刚才说调了 {t} 并给出了结果，但本次提问和本会话历史里它都没有成功返回过——这个结果是编造的。"
                        ),
                        format!(
                            "上面关于 {t} 的执行结果是编造的——本次提问和本会话历史里它都没有成功返回过。"
                        ),
                    ),
                    (None, None, true, _, _, _) => (
                        "你刚才叙述了一次已完成的执行并给出了结果，但本次提问从头到尾没有任何工具调用成功——这个结果是编造的。".to_string(),
                        "上面叙述的这次执行没有实际发生——本次提问里没有任何工具调用成功。"
                            .to_string(),
                    ),
                    (None, None, _, _, Some(v), _) => (
                        format!(
                            "你刚报的结果 {v} 在本次提问的工具返回与对话内容里都找不到出处——这个数不是执行出来的。要么真的执行一次、拿到返回后再如实转述，要么明确写清这是你的推算值（不是执行结果），禁止把没跑出来的数说成跑出来的。"
                        ),
                        format!(
                            "上面报的 {v} 没有出处——本次提问的工具返回与对话内容里都没有这个数。"
                        ),
                    ),
                    (None, None, _, _, _, true) => (
                        "你这次的调用没有取回任何新信息——返回内容就是你写进参数里的那句话，工具没有替你核实任何东西，结论不该拿它当依据。请换用能真正读到现场的工具重新核实（列目录用 list_files、读文件用 read_text_file、查任务用 query_tasks、搜内容用 grep_files）；若确实无法核实，如实说明这次没有核实过，禁止把没核实过的结论说成核实过的。".to_string(),
                        "上面的结论没有工具支撑——本次提问的调用没有取回任何新信息。"
                            .to_string(),
                    ),
                    (None, None, false, _, _, _) => (
                        "你刚才声称完成了变更，但本轮没有任何变更类工具调用成功，数据实际没有变化。"
                            .to_string(),
                        "上面声称的变更没有实际发生——本轮没有变更类工具调用成功，数据实际没有变化。"
                            .to_string(),
                    ),
                };
        if !*claim_retry_used {
            *claim_retry_used = true;
            audit(
                crate::audit::AuditLevel::Warn,
                "hallucination_guard",
                vec![
                    ("kind", kind.to_string()),
                    ("detail", crate::bot::truncate_for_log(final_text, 100)),
                ],
            );
            // 执行类谎报的补救路径与变更类不同：变更类要点名具体工具，
            // 执行类（含 run_python / bash / fetch_url）点不全也不该点名——
            // 该做的是把工具真正调起来，拿不到结果就如实说拿不到。
            // 文件类谎报（路径不存在 / 路径不是工具产物）第三档：只说
            // 文档生成那几个工具，别把模型引到任务卡工具上去。
            // 点名判据（独立判据）走执行类那一档：它说的就是一次执行。
            // 结果无出处那道：要么真跑一次拿到返回，要么写清这是推算。
            // 回声调用那道：换能读到现场的工具重查，别拿自己写进参数的
            // 那句话当证据。
            let remedy = if echo_backed {
                "请换用能真正读到现场的工具重新核实（列目录用 list_files、读文件用 read_text_file、查任务用 query_tasks、搜内容用 grep_files），并只转述工具真实返回的内容；若确实无法核实，如实说明这次没有核实过，禁止把没核实过的结论说成核实过的。".to_string()
            } else if unsourced_value.is_some() {
                "请发起 tool_call 实际执行并拿到真实返回后再如实转述；若确实无法执行（白名单拦截 / 无权限 / 工具不可用），就明确写清这是你的推算值而不是执行结果，禁止把没跑出来的数说成跑出来的。".to_string()
            } else if (claim_execution || !unsupported.is_empty()) && fake_gen_path.is_none() {
                "请立即发起 tool_call 实际执行，禁止只改文字糊弄；拿到真实返回后再如实转述（失败就如实报失败）。若确实无法执行（白名单拦截 / 无权限 / 工具不可用），如实向用户说明原因，禁止再次编造执行结果。".to_string()
            } else if fake_gen_path.is_some() || unreceipted_path.is_some() {
                "请立即调用文档生成工具实际把这个文件写出来（Word 用 create_word 或 create_word_revisions、Excel 用 create_excel、PPT 用 create_ppt、PDF 用 create_pdf；纯文本文件用 write_file），禁止只改文字糊弄；工具返回的完整路径才是能报给用户的路径。若确实无法生成（内容不足 / 工具不可用 / 权限不足），如实向用户说明原因，禁止再次声称已生成。".to_string()
            } else {
                "请立即调用对应工具实际执行（删除用 delete_task、完成用 complete_task、编辑用 edit_task、子任务用 add_subtask/remove_subtask、绑定产物用 link_file_to_task（任务卡执行流程内有效）、生成文档用 create_word/create_excel/create_ppt/create_pdf；逐步执行模式下子任务勾选由系统完成，不要代调 toggle_subtask）；若确实无法执行（任务不存在/被安全闸门拦截/无权限等），如实向用户说明原因，禁止再次声称已完成。".to_string()
            };
            msgs.push(with_thinking(
                serde_json::json!({"role": "assistant", "content": final_text}),
                thinking_blocks,
            ));
            msgs.push(serde_json::json!({
                "role": "user",
                "content": format!("【系统提示】{reason}{remedy}")
            }));
            return Ok(FinalizeOutcome::Continue);
        }
        // 返工之后仍编造：不返工（见块首），只在终稿末尾挂一条如实更正。
        audit(
            crate::audit::AuditLevel::Warn,
            "hallucination_guard.repeat",
            vec![
                ("kind", kind.to_string()),
                ("detail", crate::bot::truncate_for_log(final_text, 100)),
            ],
        );
        *fabrication_notice = Some(notice);
    }
    Ok(FinalizeOutcome::Proceed)
}
