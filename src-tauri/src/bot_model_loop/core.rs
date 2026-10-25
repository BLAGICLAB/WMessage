//! 核心循环：轮次调度、工具分发与轮间状态维护。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::{
    check_fuses, compact_before_round, finalize_without_tools, fuse_message, inject_soft_warnings,
    mutation_succeeded, prepare_round, read_sse_stream, request_round, rounds_fuse_exit,
    should_fuse, side_selfcheck_verdict, stamp_last_assistant_reasoning, stream_round,
    user_intent_of, with_thinking, FinalizeOutcome, LlmHttp, ModelLoopDeps, RequestOutcome,
    RoundCtx, ToolEvidence, TurnStatus, EVIDENCE_OUTPUT_CHARS,
};
use crate::bot_chat::merge_task_refs_dedup;
use crate::bot_chat::TaskRef;
use crate::bot_skills::skill_exit_body;
use crate::bot_slash::StopGuard;
use crate::error::CommandError;

/// 流完整性收尾状态：替代旧 `saw_done_or_finish: bool`，区分具体收尾来源。
/// - Done：SSE 协议终结哨兵（`data: [DONE]`）
/// 模型工具循环核心：流式请求（思考拆分 + 工具折叠事件）、进程内执行工具。
/// 配置/Key/HTTP 客户端/副作用出口全部注入，不依赖 AppHandle。
/// msgs 需已含 system 消息；返回 (最终正文, 任务引用)。
/// 轮数上限由调用方传入：默认 DEFAULT_MAX_ROUNDS（50），多步 Skill 可自报 max_rounds 覆盖。
/// plan_state（PREVR 第 2 层）：复杂任务的动态计划；工具连续失败时触发
/// Replan（重规划剩余步骤，≤MAX_REPLANS 次）。None = 无计划自由循环。
//
// clippy 的 too_many_lines 只数函数体的非注释行（现 274 行，阈值 200），
// 总量已收进 400 行内但代码行数仍超阈值——豁免暂留，再拆就得改已落地的
// 各单元签名或把共享状态改成 struct 传递，两者都越过「纯机械搬移」的边界。
#[allow(clippy::too_many_lines)]
pub async fn run_model_loop_core<X, XP, R, RP>(
    http: &LlmHttp,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
    deps: &ModelLoopDeps<'_>,
    execute_tool: X,
    replan: R,
) -> Result<(String, Vec<TaskRef>), CommandError>
where
    X: Fn(String, String, crate::bot::ToolCallTrace) -> XP,
    XP: std::future::Future<Output = crate::bot::registry::ToolResult>,
    R: Fn(crate::bot_plan::PlanState, String) -> RP,
    RP: std::future::Future<Output = Option<Vec<String>>>,
{
    // 循环级共享量一次性备好（熔断上限与两个软警阈值 / 一次请求的固定件与
    // 跨轮状态）——见 prepare_round。
    let RoundCtx {
        fuse_cap,
        soft_warn_at,
        rounds_warn_at,
        mut plan,
    } = prepare_round(http, stop, deps, max_rounds);
    let mut msgs = msgs;
    // 侧调自检要用的「用户要什么」：在进循环前从初始消息栈取一次——
    // 循环里注入的提示块会混进消息栈，那时再取就分不清哪句是真人说的。
    let user_intent = user_intent_of(&msgs);
    // 条目级 system prompt：追加到消息栈末尾——OpenAI 直接多一条 system
    // 消息；Anthropic 分支 openai_msgs_to_anthropic 会把它归并进顶层 system 块。
    if let Some(sp) = &http.system_prompt {
        msgs.push(serde_json::json!({ "role": "system", "content": sp }));
    }
    let audit = deps.audit;
    let skill_finish = deps.skill_finish;
    let active_skill_run = deps.active_skill_run;
    let session_id: Option<&str> = stop.session_id();
    // 僵尸终态清理已上移到薄壳 `run_model_loop`（核心只读技能状态，
    // 写生命周期不在核心——原位置与现在等价，都在进入轮循环之前）
    // 轮内工具结果滑动窗：一次性开关（触发过就不再改写，前缀重归 append-only）
    let mut tool_window_applied = false;
    // 最多 max_rounds 轮（工具循环），每轮流式输出；收到 tool_calls 则执行后把结果续进对话
    let mut collected_refs: Vec<TaskRef> = Vec::new();
    let mut function_calls_total: usize = 0;
    let mut soft_warn_sent: bool = false;
    // 轮数软警告同款：只发一次，发完不再打扰后续轮次
    let mut rounds_warn_sent: bool = false;
    // 软墙钟提醒已注入（只一次）：进入收尾预留窗口时提醒一次，不重复刷屏
    let mut wall_warn_sent: bool = false;
    // 技能预算软警告已注入（只一次）：技能步数/墙钟快用完时提醒模型收尾
    let mut skill_warn_sent: bool = false;
    // 防幻觉汇报守卫：本次提问是否实际执行过变更类工具（提问级一次置位、不按轮
    // 重置——后续正常汇报轮靠它放行，按轮重置会把改完再汇报的正常轮误判为幻觉）；
    // 补一轮机会（claim_retry）同样每次提问只用一次
    let mut mutation_done: bool = false;
    let mut claim_retry_used: bool = false;
    // 幻觉守卫第二次拦下时写这里：终稿末尾追加一条如实更正（见判决处）。
    // None = 本次提问没有出现「返工后仍编造」，终稿照常。
    let mut fabrication_notice: Option<String> = None;
    // 执行类谎报守卫的凭证池：本次**用户提问**内所有工具调用的真实记录。
    //
    // 作用域是整条提问（= 一次 run_model_loop_core 调用 = exec_spans 的一条 trace_id），
    // 跨轮累积、跨轮不清零。两个边界都是被真实数据逼出来的：
    //
    // · 不跨轮清零 —— trace 53 的最终汇报（msg 720「执行完毕：模块不存在…」）是真话，
    //   但它的凭证在上一轮；按轮查会把真话判成幻觉，再让模型去重跑一个已完成的
    //   动作（delete_task / link_file_to_task 这类就是重复的破坏性操作）。
    // · 跨提问的成功记录另查 —— 下一条用户消息是新的 trace，这个池子自然清零；
    //   跨提问的重述（「刚才 create_word 生成的那个」）是真话，由会话级的成功
    //   记录兜住，见下面的 session_tools_cache。
    let mut receipts: Vec<String> = Vec::new();
    // 本提问里每次工具调用的「参数 + 返回」。
    //
    // 凭证池只记「哪个工具成功过」——那是存在性。要判「模型报出来的这个数是哪来的」
    // 「这次调用到底替模型核实了什么」，得留着返回内容本身：两者是不同的轴，
    // 凭证表答不了后者。
    let mut evidence: Vec<ToolEvidence> = Vec::new();
    // 本会话调用过的工具名（调用过 / 成功过两档）——点名判据的排除集，跨提问的
    // 重述是真话。惰性拉一次：判据要用它，但每轮拉一次库不值当。None = 还没拉。
    let mut session_tools_cache: Option<Option<crate::db::trace::SessionTools>> = None;
    // 本提问里被工具写过的路径（存原始大小写：这条路径要给人看，
    // 小写归一的版本只用于比对）。
    //
    // 用途有二：① 幻觉守卫的「落盘凭证」那道判据——模型声称「已生成
    // /path/x.docx」，而这个路径从来不是工具写出来的——那是凭空声称。
    // 凭证在**进程内**攒，不等痕迹落库（本轮刚写的文件此刻还只在 trace sink
    // 的队列里，查库查不到）。② 技能收尾兜底正文里列出已产出文件。
    let mut written_paths: Vec<String> = Vec::new();
    // soft_warn 待注入标志：本轮 tool 响应全部回填后才真正 push（见循环内注释）
    let mut soft_warn_queued: bool = false;
    // PREVR 第 1 层：工具失败检测——同工具连续失败计数，
    // 第 1 次失败注入「换策略」提示；连续 2 次失败：有计划则 Replan，无计划则要求如实告知
    let mut last_failed_tool: Option<String> = None;
    let mut last_fail_reason: Option<String> = None;
    let mut consec_failures: usize = 0;
    // Replan 预算耗尽审计只记一次（耗尽后每轮连续失败仍会发生，不刷屏）
    let mut replan_exhausted_logged: bool = false;
    let mut fail_hint_queued: Option<String> = None;
    // 自检节流：每 5 轮最多一次；本轮 pending 待本轮 tool 响应回填后 push
    let mut last_selfcheck_round: Option<usize> = None;
    let mut pending_selfcheck: Option<(String, String)> = None;
    // 自检视觉过滤：pending 推入 prompt 后置 true，本轮 SSE 流式 emit 不输出 Y/N，
    // 改在 final_text 解析时统一 emit 视觉提示（✅ / ⚠️）
    let mut expecting_selfcheck = false;
    // 滚动降密度：每条 tool 消息的 (入栈轮次, msgs 下标)。msgs 循环内只追加
    // 不删（压缩原位改写），下标恒稳定——唯一例外是上下文超限降级砍历史，
    // 那条路径必须调 reindex_after_shed 同步左移，否则下标越界。
    let mut tool_msg_ages: Vec<(usize, usize)> = Vec::new();
    // 终稿自检视觉过滤：本轮已被注入 FINAL_SELFCHECK_PROMPT、下一轮 SSE 流式
    // emit 跳过 Y/N 字样（用户已在主出口前看到 final_text 自检运行属于机器内部
    // 信号，不该再叠一层 Y/N 字样）；终态在主出口解析后还原 final_text。
    let mut expecting_final_selfcheck = false;
    // 截断续跑已用次数（stop_reason=max_tokens 时自动接一轮，全会话共享配额）
    let mut length_continuations: usize = 0;
    // 被截断轮次的正文前缀：续跑轮只产出后半段，主出口拼接后才是一份完整答复
    let mut length_prefix = String::new();
    // 终稿自检 snapshot：触发时拍下 final_text 本体，下一轮 LLM 响应解析后
    // 据 verdict 还原（Y/N/附解释均回滚到原文，N 路径在末尾追加 ⚠️）
    let mut final_selfcheck_snapshot: Option<String> = None;
    // 终稿自检节流：与 last_selfcheck_round 同模式但独立计数——SELFCHECK 节流
    // 算的是「工具变更类自检」、final_selfcheck 算的是「终稿整体自检」，
    // 两条路径不互相抑制。
    let mut last_final_selfcheck_round: Option<usize> = None;
    // 出口闸追问次数：模型隔了几轮才把自检判定（纯 Y/N）送回来时，出口追着它
    // 要真实结果。次数有硬顶（MAX_LATE_VERDICT_RETRIES），否则模型只回 Y/N
    // 就能把回合拖成死循环。
    let mut late_verdict_retries: u8 = 0;
    let mut plan_state = plan_state;
    // 上轮 streamed 文本快照：
    // AwaitConfirm/Finish/Fail/Terminate 跳出主循环时，返回 user 已看到的文本
    let mut last_streamed = String::new();
    // 回合状态统一收口（见 TurnStatus）。整段循环体跑在 `run_body` 这个 async
    // 块里，外层只负责「出口 → 状态」映射：
    //
    // 不这么做的理由——循环体内有 14 处 `return Ok(...)`（已停止 3 处、Skill
    // 状态机 5 处、单轮熔断 1 处、正文定稿 1 处、续写/自检若干），逐个补
    // 状态发射必漏一处，而漏掉的那处正好表现为「UI 提前显示完成」。
    // 内层跑完由外层统一判定出口类别，出口只有一处可漏。
    let mut body_outcome: Option<&'static str> = None;
    let body: Result<(String, Vec<TaskRef>), CommandError> = async {
        for round in 0..max_rounds {
            // 墙钟熔断（轮边界检查）：期限已到就在这里退出，**带上上一轮的正文**（见 check_fuses）。
            if let Some(msg) = check_fuses(deps, &last_streamed, round) {
                body_outcome = Some("fused");
                return Ok((msg, collected_refs));
            }
            // 轮前准备：预算硬压（msgs 字符分级）+ 轮龄降密度（老工具结果降档）。
            compact_before_round(
                &mut msgs,
                &tool_msg_ages,
                deps,
                stop,
                round,
                &mut tool_window_applied,
            );
            if stop.stopped() {
                let hint = skill_finish(false, "用户停止");
                return Ok((format!("⏹ 已停止{hint}"), collected_refs));
            }
            // 状态机推进决策：集中 Skill 推进逻辑
            // 未来横切关注点（审批/沙箱/上下文压缩）只动 advance_skill，主循环不重构
            if let Some(run) = active_skill_run(session_id) {
                use crate::bot_skills::{advance_skill, AdvanceAction};
                match advance_skill(&run, chrono::Utc::now().timestamp_millis()) {
                    AdvanceAction::NoActive | AdvanceAction::Continue => {} // 继续本轮
                    AdvanceAction::AwaitConfirm => {
                        // Skill 暂停等用户确认，跳出主循环等待 bot_confirm_response 唤起。
                        // 并发新消息在第 0 轮命中此分支时 last_streamed
                        // 是空串，用户会得到空白回复——空串时给一句可读提示
                        body_outcome = Some("await_confirm");
                        if last_streamed.is_empty() {
                            return Ok((
                                "⏸ 上一个操作正在等待你的确认——请先处理确认弹窗，再继续对话。"
                                    .into(),
                                collected_refs,
                            ));
                        }
                        return Ok((last_streamed.clone(), collected_refs));
                    }
                    AdvanceAction::Finish => {
                        let _hint = skill_finish(true, "");
                        body_outcome = Some("skill_finish");
                        return Ok((
                            skill_exit_body(
                                &last_streamed,
                                crate::bot_skills::SkillExitKind::Completed,
                                &run.name,
                                "",
                                run.step,
                                &written_paths,
                            ),
                            collected_refs,
                        ));
                    }
                    AdvanceAction::Fail(reason) => {
                        let _hint = skill_finish(false, &reason);
                        body_outcome = Some("skill_fail");
                        return Ok((
                            skill_exit_body(
                                &last_streamed,
                                crate::bot_skills::SkillExitKind::Failed,
                                &run.name,
                                &reason,
                                run.step,
                                &written_paths,
                            ),
                            collected_refs,
                        ));
                    }
                    AdvanceAction::Terminate(reason) => {
                        let _hint = skill_finish(false, &reason);
                        body_outcome = Some("skill_terminate");
                        return Ok((
                            skill_exit_body(
                                &last_streamed,
                                crate::bot_skills::SkillExitKind::Terminated,
                                &run.name,
                                &reason,
                                run.step,
                                &written_paths,
                            ),
                            collected_refs,
                        ));
                    }
                }
            }
            // 轮首活体信号：上一轮 bot-tool-done 之后到本轮首个 delta 之间是第二段
            // 空窗（LLM 首 token 延迟，无工具行可挂）。发一条 running 让前端知道
            // 「模型正在想」，配 turn 计数与已用时长的秒级刷新。
            if let Some(cb) = deps.turn_status {
                cb(
                    TurnStatus::Running,
                    &format!("正在思考 · 第 {} 轮", round + 1),
                );
            }
            // 请求段：装配 body → 发请求 → 失败重试/降级（见 request_round）。
            let resp =
                match request_round(http, &mut msgs, &mut plan, stop, deps, &mut tool_msg_ages)
                    .await?
                {
                    RequestOutcome::Response(r) => r,
                    RequestOutcome::Stopped => {
                        let hint = skill_finish(false, "用户停止");
                        return Ok((format!("⏹ 已停止{hint}"), collected_refs));
                    }
                };

            // 流消费：逐 chunk 解析 SSE → 正文 / 工具调用 / 思考块（见 stream_round）。
            let sse = read_sse_stream(
                http,
                resp,
                stop,
                deps,
                expecting_selfcheck,
                expecting_final_selfcheck,
            )
            .await?;
            let rs = stream_round(sse, deps, expecting_selfcheck, expecting_final_selfcheck)?;
            if rs.stopped {
                let hint = skill_finish(false, "用户停止");
                return Ok((
                    format!("{}\n\n⏹ 已停止{hint}", rs.final_text),
                    collected_refs,
                ));
            }
            let mut final_text = rs.final_text;
            let tool_calls = rs.tool_calls;
            let thinking_blocks = rs.thinking_blocks;
            let last_finish_reason = rs.last_finish_reason;
            let reasoning_text = rs.reasoning_text;
            // 思考段回灌：只对 OpenAI 兼容路径有意义（Anthropic 协议走 thinking 块
            // 原样回传，转换层会丢掉这个键），且只在模型真吐了思考段时才带——
            // 没思考段的模型挂了也是空字段，白占请求体。
            let echo_reasoning: Option<&str> = if deps.reasoning_echo
                && http.provider == crate::bot::ApiProvider::Openai
                && !reasoning_text.trim().is_empty()
            {
                Some(reasoning_text.as_str())
            } else {
                None
            };

            if tool_calls.is_empty() {
                match finalize_without_tools(
                    &mut msgs,
                    &mut mutation_done,
                    &mut claim_retry_used,
                    &mut fabrication_notice,
                    &receipts,
                    &evidence,
                    &written_paths,
                    &mut session_tools_cache,
                    &mut function_calls_total,
                    &mut last_selfcheck_round,
                    &mut pending_selfcheck,
                    &mut expecting_selfcheck,
                    &mut expecting_final_selfcheck,
                    &mut final_selfcheck_snapshot,
                    &mut last_final_selfcheck_round,
                    &mut late_verdict_retries,
                    &mut length_continuations,
                    &mut length_prefix,
                    &mut final_text,
                    deps,
                    stop,
                    round,
                    &thinking_blocks,
                    &last_finish_reason,
                )
                .await?
                {
                    FinalizeOutcome::Continue => {
                        // 返工/续跑路径推回的 assistant 消息也要带思考段：
                        // 这些路径都会再发一次请求，DeepSeek 这类模型缺了就 400。
                        if let Some(r) = echo_reasoning {
                            stamp_last_assistant_reasoning(&mut msgs, r);
                        }
                        continue;
                    }
                    FinalizeOutcome::Proceed => {}
                    FinalizeOutcome::Stopped(msg) => {
                        return Ok((msg, collected_refs));
                    }
                }
                let _ = skill_finish(true, "");
                collected_refs = merge_task_refs_dedup(collected_refs);
                let mut answer = if length_prefix.is_empty() {
                    final_text.clone()
                } else {
                    format!("{length_prefix}{final_text}")
                };
                // 幻觉守卫返工后仍编造：把更正挂在终稿末尾（不删原文——谎话已经
                // 流出去了，删掉它只会让用户更信那句假的）。前端收尾时用本返回值
                // 覆盖流式气泡，所以这行更正会落到用户眼前的那一份文本上。
                if let Some(notice) = &fabrication_notice {
                    answer.push_str("\n\n⚠️ ");
                    answer.push_str(notice);
                }
                // L6 post_turn 钩子层：回合收尾、答案定稿（终稿自检已还原、
                // 长度续跑已拼好）之后、回灌与 emit 之前。
                // 与 L5 同理：放这里意味着钩子看到并能改的，就是用户真正会看到的
                // 那一份文本。/stop 的早退出口（「⏹ 已停止」占位）不走本层——
                // 那是系统占位串不是模型回复，钩子对它做归档/沉淀没有意义。
                {
                    let run = crate::bot::hooks::registry().run_post_turn(
                        stop.session_id(),
                        &mut answer,
                        round,
                    );
                    for f in &run.failures {
                        let (kind, err) = match &f.kind {
                            crate::bot::hooks::HookFailureKind::Returned(e) => {
                                ("returned_err", e.as_str())
                            }
                            crate::bot::hooks::HookFailureKind::Panic(e) => ("panic", e.as_str()),
                        };
                        audit(
                            crate::audit::AuditLevel::Warn,
                            "post_turn.hook_failed",
                            vec![
                                ("hook", f.hook.to_string()),
                                ("kind", kind.to_string()),
                                ("err", err.to_string()),
                                ("round", round.to_string()),
                            ],
                        );
                    }
                }
                return Ok((answer, collected_refs));
            }

            match dispatch_round(
                &execute_tool,
                &replan,
                &mut msgs,
                &final_text,
                &tool_calls,
                &thinking_blocks,
                echo_reasoning,
                &mut collected_refs,
                &mut function_calls_total,
                &mut mutation_done,
                &mut receipts,
                &mut evidence,
                &mut written_paths,
                &mut expecting_selfcheck,
                &mut pending_selfcheck,
                &mut expecting_final_selfcheck,
                &mut final_selfcheck_snapshot,
                &mut last_selfcheck_round,
                &mut last_failed_tool,
                &mut last_fail_reason,
                &mut consec_failures,
                &mut fail_hint_queued,
                &mut replan_exhausted_logged,
                &mut soft_warn_sent,
                &mut soft_warn_queued,
                &mut tool_msg_ages,
                fuse_cap,
                soft_warn_at,
                &user_intent,
                plan_state.as_deref_mut(),
                deps,
                stop,
                round,
            )
            .await?
            {
                DispatchOutcome::Continue => {}
                DispatchOutcome::Stopped(msg) => {
                    return Ok((msg, collected_refs));
                }
                DispatchOutcome::Fused(msg) => {
                    body_outcome = Some("tool_fuse");
                    return Ok((msg, collected_refs));
                }
            }
            // 轮末软警告：轮数与墙钟各提醒一次（见 inject_soft_warnings）。
            inject_soft_warnings(
                deps,
                &mut msgs,
                &mut rounds_warn_sent,
                &mut wall_warn_sent,
                max_rounds,
                rounds_warn_at,
                round,
            );
            // 技能预算软警告：步数/墙钟快用完时提醒一次，让模型把已经做出来的
            // 东西整理成回复（产物路径 + 结论）。硬闸砍掉的那一轮模型往往刚做完
            // 关键动作、正准备总结——没有这句提醒，用户拿到的是空气泡。
            if !skill_warn_sent {
                if let Some(run) = active_skill_run(session_id) {
                    if let Some(warn) = crate::bot_skills::skill_budget_warning(
                        &run,
                        chrono::Utc::now().timestamp_millis(),
                    ) {
                        msgs.push(serde_json::json!({ "role": "user", "content": warn }));
                        skill_warn_sent = true;
                        audit(
                            crate::audit::AuditLevel::Info,
                            "skill_budget_soft_warn",
                            vec![
                                ("name", run.name.clone()),
                                ("step", run.step.to_string()),
                                ("max_steps", run.max_steps.to_string()),
                                ("timeout_secs", run.timeout_secs.to_string()),
                            ],
                        );
                    }
                }
            }
            // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
            last_streamed = final_text.clone();
            // 轮数自报：1-based 累计，「轮」的唯一口径来源。
            // 报在轮末，轮数熔断（自然落出循环）因此能报满 max_rounds；早返回出口
            // （已停止 / Skill 收尾 / 单轮熔断）在轮中 return，漏报它们所在那轮——
            // 那些都是「没跑完的一轮」，不上报反而更贴近真实完成量。
            (deps.on_round)(round + 1);
        }
        // 轮数熔断补审计（单轮工具熔断已有 fuse 日志，对称留痕）
        body_outcome = Some("fused");
        Ok((
            rounds_fuse_exit(deps, &last_streamed, max_rounds),
            collected_refs,
        ))
    }
    .await;
    // 回合状态收口：出口 → 状态。settling 在正文定稿后、收尾自检之前发，
    // 它是「说完了但还在动」这个场景唯一的存在方式——没有它，前端只能把
    // 「正文说完」当「本轮结束」，随后那段收尾（可静默 300s）就是一片静止。
    let final_status = match (&body, body_outcome, stop.stopped()) {
        (_, Some("fused"), _) => TurnStatus::Completed,
        (_, Some("interrupted"), _) => TurnStatus::Interrupted,
        (Err(_), _, _) => TurnStatus::Failed,
        (Ok(_), _, true) => TurnStatus::Interrupted,
        (Ok(_), _, false) => TurnStatus::Completed,
    };
    if let Some(cb) = deps.turn_status {
        cb(final_status, "");
    }
    body
}

/// 工具轮的执行出口。
enum DispatchOutcome {
    /// 本轮工具已执行完，继续下一轮
    Continue,
    /// 用户叫停：附「已停止」正文
    Stopped(String),
    /// 单轮 Function 调用熔断：附熔断正文
    Fused(String),
}

/// 工具轮：execute_tool 分发 / 结果回灌 / 凭证登记。
///
/// 出口：跑完一轮（Continue）/ 用户叫停（Stopped，附「已停止」正文）/
/// 单轮 Function 调用熔断（Fused，附熔断正文）。
//
// 本段顺序即不变量：assistant(tool_calls) 消息 → 逐个执行并回填 tool 响应 →
// Replan → 各类提示注入。拆细会把「tool_calls 后必须紧跟 tool 消息」的协议
// 约束切散到多处，反而更难看出哪一处破坏了它。
#[allow(clippy::too_many_lines)]
async fn dispatch_round<X, XP, R, RP>(
    execute_tool: &X,
    replan: &R,
    msgs: &mut Vec<serde_json::Value>,
    final_text: &str,
    tool_calls: &[(String, String, String)],
    thinking_blocks: &[serde_json::Value],
    echo_reasoning: Option<&str>,
    collected_refs: &mut Vec<TaskRef>,
    function_calls_total: &mut usize,
    mutation_done: &mut bool,
    receipts: &mut Vec<String>,
    evidence: &mut Vec<ToolEvidence>,
    written_paths: &mut Vec<String>,
    expecting_selfcheck: &mut bool,
    pending_selfcheck: &mut Option<(String, String)>,
    expecting_final_selfcheck: &mut bool,
    final_selfcheck_snapshot: &mut Option<String>,
    last_selfcheck_round: &mut Option<usize>,
    last_failed_tool: &mut Option<String>,
    last_fail_reason: &mut Option<String>,
    consec_failures: &mut usize,
    fail_hint_queued: &mut Option<String>,
    replan_exhausted_logged: &mut bool,
    soft_warn_sent: &mut bool,
    soft_warn_queued: &mut bool,
    tool_msg_ages: &mut Vec<(usize, usize)>,
    fuse_cap: usize,
    soft_warn_at: usize,
    user_intent: &str,
    plan_state: Option<&mut crate::bot_plan::PlanState>,
    deps: &ModelLoopDeps<'_>,
    stop: &StopGuard,
    round: usize,
) -> Result<DispatchOutcome, CommandError>
where
    X: Fn(String, String, crate::bot::ToolCallTrace) -> XP,
    XP: std::future::Future<Output = crate::bot::registry::ToolResult>,
    R: Fn(crate::bot_plan::PlanState, String) -> RP,
    RP: std::future::Future<Output = Option<Vec<String>>>,
{
    let emit = deps.emit;
    let audit = deps.audit;
    let skill_finish = deps.skill_finish;
    // 模型请求工具：进程内执行，结果回填后继续下一轮。
    // 模型以工具调用回应自检提示 = 以行动代回答：丢弃工具自检态，
    // 后续轮流式不再被抑制；终稿自检快照一并释放——最终回答以
    // 模型核验后的新产出为准，不能把旧快照顶回去。
    *expecting_selfcheck = false;
    *pending_selfcheck = None;
    *expecting_final_selfcheck = false;
    *final_selfcheck_snapshot = None;
    msgs.push(with_thinking(
        serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": args}
            })).collect::<Vec<_>>()
        }),
        thinking_blocks,
    ));
    // 思考段盖在这条 assistant 消息上（回灌开启时）。盖在消息栈尾部那条，
    // 更早的内容一个字节不动——隐式前缀缓存靠的就是那截不动。
    if let Some(r) = echo_reasoning {
        stamp_last_assistant_reasoning(msgs, r);
    }
    if stop.stopped() {
        let hint = skill_finish(false, "用户停止");
        return Ok(DispatchOutcome::Stopped(format!(
            "{final_text}\n\n⏹ 已停止{hint}"
        )));
    }
    for (id, name, args) in tool_calls {
        // 工具批中途可停——/stop 后剩余调用不执行，
        // 但必须回填占位 tool 响应（tool_calls → tool 消息协议完整性，
        // 缺响应会让下一轮请求被 API 拒为 400 invalid params）
        if stop.stopped() {
            msgs.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": "⏹ 已停止，该工具未执行"
            }));
            tool_msg_ages.push((round, msgs.len() - 1));
            continue;
        }
        *function_calls_total += 1;
        if should_fuse(*function_calls_total, fuse_cap) {
            let hint = skill_finish(false, "单轮 Function 调用超上限");
            audit(
                crate::audit::AuditLevel::Warn,
                "fuse",
                vec![("cap", fuse_cap.to_string())],
            );
            return Ok(DispatchOutcome::Fused(fuse_message(
                final_text, &hint, fuse_cap,
            )));
        }
        // 软警告（soft_warn_at，按会话熔断上限派生）：置标志，推迟到本轮 tool 响应全部回填后再注入——
        // 若在此直接 push user 消息，会插进 assistant(tool_calls) 与 tool 响应之间，
        // 破坏「tool_calls 后必须紧跟 tool 消息」的协议，下一轮请求被 API 拒为
        // 400 invalid params（实锤：两次 400 均紧跟 soft_warn 注入）
        if !*soft_warn_sent && *function_calls_total >= soft_warn_at {
            *soft_warn_sent = true;
            *soft_warn_queued = true;
            audit(
                crate::audit::AuditLevel::Info,
                "soft_warn",
                vec![
                    ("used", soft_warn_at.to_string()),
                    ("cap", fuse_cap.to_string()),
                ],
            );
        }
        // 把 /stop 守卫透传给 execute_tool，run_python 在途可中断；
        // turn + tool_call_id 一并下传，工具审计可按轮回放（见 bot::ToolCallTrace）
        let t_tool = std::time::Instant::now();
        // 工具执行段的活体信号：bot-tool 只说明「要调了」，bot-tool-done 要等
        // 工具返回——中间这段（run_python 可静默 300s、MCP 600s）此前零事件，
        // UI 只有一个静态「…」。补一条 running 事件让前端起计时与 spinner。
        emit(
            "bot-tool-running",
            serde_json::json!({
                "id": id, "name": name,
                "args_preview": args.chars().take(200).collect::<String>(),
            }),
        );
        let tool_outcome = execute_tool(
            name.clone(),
            args.clone(),
            crate::bot::ToolCallTrace {
                turn: Some(round),
                tool_call_id: Some(id.clone()),
            },
        )
        .await;
        let tool_ms = t_tool.elapsed().as_millis() as u64;
        let result = &tool_outcome.text;
        let refs = &tool_outcome.refs;
        // 按执行结果置位——被门禁拦截/用户拒绝/执行失败的
        // 变更工具不算「动过手」，幻觉守卫对后续虚假汇报保持拦截能力
        if mutation_succeeded(name, &tool_outcome) {
            *mutation_done = true;
            // 自检触发：变更类工具 + 非子 agent 路径 + 节流(5 轮 1 次)
            // 失败路径写 last_failed_tool/consec_failures/fail_hint_queued，
            // 复用 PREVR 第 1 层既有「换策略」注入
            if crate::bot::registry::mutating_tools()
                .iter()
                .any(|t| *t == name)
                && stop
                    .session_id()
                    .and_then(|sid| crate::tool_guard::subagent_ctx(Some(sid)))
                    .is_none()
                && last_selfcheck_round.is_none_or(|r| round.saturating_sub(r) >= 5)
            {
                let snippet: String = result.chars().take(80).collect();
                *pending_selfcheck = Some((name.clone(), snippet));
            }
        }
        // 成败口径与全链路统一（ToolStatus 是唯一真相源）——bot-tool-done 的 ok 同源
        let failed = !tool_outcome.status.is_success();
        // 被闸拒掉的调用（Denied：预算触顶 / 白名单外 / 用户拒绝）不计入熔断
        // 计数——计数守的是「实际执行」的调用量。子 agent 收尾窗里模型可能
        // 成批发读类调用试探，若被拒的也计数，一次并行批就能烧光收尾裕量，
        // 模型永远轮不到写产物（写类在收尾窗放行、占裕量，正是设计语义）。
        if tool_outcome.status == crate::bot::registry::ToolStatus::Denied {
            *function_calls_total = function_calls_total.saturating_sub(1);
        }
        // 证据池：参数与返回都留着。成功与失败都记——失败信息也是模型看得见的
        // 内容（模型会照着它说「跑不了」），出处比对时算不算得到依据就在这里。
        evidence.push(ToolEvidence {
            ok: !failed,
            args: args.clone(),
            output: result.chars().take(EVIDENCE_OUTPUT_CHARS).collect(),
        });
        // 只有成功返回的工具才算数：被门禁拦下 / 用户拒绝 / 执行失败的调用没产出
        // 任何可转述的东西，模型若照着它编结果，凭证表还得拦得住。
        if !failed {
            receipts.push(name.clone());
            // 落盘凭证同步入池：写盘类工具成功时带回来的 file_changes 就是
            // 「这个路径确实是工具产物」的凭据（文档生成器在 24a4ea6 之前
            // 不带凭证，那是这道判据的地基缺口）。
            for fc in &tool_outcome.file_changes {
                written_paths.push(fc.path.clone());
            }
        }
        emit(
            "bot-tool-done",
            serde_json::json!({
                "id": id, "name": name, "args": args,
                "result": result.chars().take(2000).collect::<String>(),
                "ms": tool_ms,
                "ok": !failed,
            }),
        );
        audit(
            crate::audit::AuditLevel::Info,
            "tool",
            vec![
                // 工具名是模型给的字符串：truncate_for_log = escape_for_log 别名（\n\r| 已转义），不可伪造日志行
                ("tool", crate::bot::truncate_for_log(name, 60)),
                ("args", crate::bot::truncate_for_log(args, 500)),
                ("result", crate::bot::truncate_for_log(result, 300)),
            ],
        );
        collected_refs.extend(refs.iter().cloned());
        // PREVR 第 1 层：工具失败检测。判定走全链路统一口径
        // （audit::tool_call_failed）——门禁拦截/熔断/暂停/拒绝都能识别；
        // 同工具连续失败才升级——单次失败先提示换策略。
        // 与 soft_warn 同理：提示推迟到本轮 tool 响应全部回填后注入（协议安全）
        if failed {
            if last_failed_tool.as_deref() == Some(name.as_str()) {
                *consec_failures += 1;
            } else {
                *consec_failures = 1;
                *last_failed_tool = Some(name.clone());
            }
            let reason = crate::bot::truncate_for_log(result, 200);
            *last_fail_reason = Some(reason.clone());
            *fail_hint_queued = Some(if *consec_failures >= 2 {
                format!(
                        "【系统提示】工具 {name} 已连续失败 {consec_failures} 次（最近原因：{reason}）。禁止再次以相同方式调用该工具；如果换参数/换路径仍无法完成，如实向用户说明失败原因与当前进度，由用户决定下一步。"
                    )
            } else {
                format!(
                        "【系统提示】上一步调用的工具 {name} 失败了（原因：{reason}）。不要重复同样的调用；分析原因后换策略：换参数、换工具、或把任务拆成更小的步骤再试一次。"
                    )
            });
        } else if !result.is_empty() {
            // 成功调用重置连续失败链（空结果不算成功也不算失败，不重置）
            *last_failed_tool = None;
            *last_fail_reason = None;
            *consec_failures = 0;
        }
        msgs.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": id,
            "content": result
        }));
        tool_msg_ages.push((round, msgs.len() - 1));
        // 工具随结果附图（screenshot）→ 图作为紧随 tool 消息的 user 消息注入。
        // OpenAI 协议 tool 消息只收文本，「工具后追加带图 user 消息」是官方视觉
        // 示例同款；Anthropic 转换器会把 [tool, user(图)] 合并成单条 user
        // [tool_result, image]（官方 tool_result 附图形态，满足严格交替）。
        // 图片消息在本轮 msgs 驻留后续轮次：单次截图场景 token 可控，不做
        // 历史裁剪（循环内 msgs 生命周期只有一次运行，与 bot_chat 附件跨轮
        // 「最后 3 条」裁剪不同源）。
        if !tool_outcome.images.is_empty() {
            let mut parts = vec![serde_json::json!({
                "type": "text",
                "text": "〔系统附图〕以上工具返回了截图，请直接用视觉能力读取图片内容。"
            })];
            // 单图至多 3MB（读文件 + base64）：与 bot_chat 附件链路同款包
            // spawn_blocking，防多图/并发会话把 async runtime 线程占死
            let images = tool_outcome.images.clone();
            let read = tauri::async_runtime::spawn_blocking(move || {
                images
                    .iter()
                    .map(|p| crate::bot_chat::image_part_from_file(std::path::Path::new(p)))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            let mut attached = 0usize;
            for (p, part) in tool_outcome.images.iter().zip(read) {
                match part {
                    Some(part) => {
                        parts.push(part);
                        attached += 1;
                    }
                    None => audit(
                        crate::audit::AuditLevel::Info,
                        "tool_image.skip",
                        vec![("detail", crate::bot::truncate_for_log(p, 200))],
                    ),
                }
            }
            if attached > 0 {
                msgs.push(serde_json::json!({ "role": "user", "content": parts }));
            }
        }
    }
    // PREVR 第 2 层：同工具连续失败 ≥2 且有计划 → Replan 一次
    // （重规划剩余步骤，替换计划文本；≤MAX_REPLANS 次硬上限，防重规划死循环）。
    // 预算不管成败都消耗——失败 replan 若不计数，
    // Planner 持续故障时每轮会白烧一次调用，硬上限名不副实；fail_reason 带真实错误
    // 文本（只传工具名的话 Planner 拿不到任何失败细节）。
    // Replan 是同步阻塞本轮的 LLM 调用：放在 tool 响应全部回填后、注入提示前，
    // 这样提示里带的就是新计划。
    if *consec_failures >= 2 {
        if let Some(plan) = plan_state {
            if plan.replans_used < crate::bot_plan::MAX_REPLANS {
                plan.replans_used += 1;
                let reason = format!(
                    "工具 {} 连续失败 {} 次，最近错误：{}",
                    last_failed_tool.as_deref().unwrap_or("?"),
                    consec_failures,
                    last_fail_reason.as_deref().unwrap_or("（无错误详情）")
                );
                // replan 走注入回调；PlanState 按值快照传入（回调签名
                // 不能借用本轮局部 &mut，见 ModelLoopDeps 注释）
                let snapshot = crate::bot_plan::PlanState {
                    task: plan.task.clone(),
                    steps: plan.steps.clone(),
                    replans_used: plan.replans_used,
                };
                // 用户已停止：不再重规划。planner 一次调用最多烧 60s，而
                // 它的产物只是给模型注入一段提示——用户已经叫停了。
                if stop.stopped() {
                    let hint = skill_finish(false, "用户停止");
                    return Ok(DispatchOutcome::Stopped(format!(
                        "{final_text}\n\n⏹ 已停止{hint}"
                    )));
                }
                if let Some(new_steps) = replan(snapshot, reason).await {
                    plan.steps = new_steps;
                    *fail_hint_queued = Some(format!(
                            "【系统提示】原计划执行受阻，已重新规划剩余步骤：\n{}\n请按新计划继续；若仍无法推进，如实向用户说明。",
                            plan.steps.join("\n")
                        ));
                }
            } else if !*replan_exhausted_logged {
                // 预算耗尽留痕（只记一次）——
                // 「连续失败持续发生但不再重规划」这件事不能零痕迹
                *replan_exhausted_logged = true;
                audit(
                    crate::audit::AuditLevel::Warn,
                    "plan.replan_budget_exhausted",
                    vec![("max", crate::bot_plan::MAX_REPLANS.to_string())],
                );
            }
        }
    }
    // 本轮 tool 响应已全部回填（tool_calls → tool×N 序列完整），此时注入提示才合法。
    // pending 保持 Some 直到回复轮消费——在这里 take() 提前清空，回复轮的解析
    // 就会永远扑空（N 换策略路径整段死代码的根因）；expecting 守卫防重复注入。
    if !*expecting_selfcheck {
        if let Some((sc_name, sc_snippet)) = pending_selfcheck.clone() {
            *last_selfcheck_round = Some(round);
            // 长历史 + 配了侧调模型 → 侧调判定，省掉主循环整段历史重付的那一轮；
            // 侧调没给出可判定的答案就回落 in-loop（这道闸不丢）。
            match side_selfcheck_verdict(deps, msgs, user_intent, &sc_name, &sc_snippet).await {
                Some(true) => {
                    emit(
                        "bot-chat-delta",
                        serde_json::json!({ "text": super::SELFCHECK_PASS_EMIT }),
                    );
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": super::SELFCHECK_PASS_HINT
                    }));
                    *pending_selfcheck = None;
                }
                Some(false) => {
                    emit(
                        "bot-chat-delta",
                        serde_json::json!({ "text": super::SELFCHECK_FAIL_EMIT }),
                    );
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": super::selfcheck_retry_hint(&sc_name)
                    }));
                    *pending_selfcheck = None;
                }
                None => {
                    *expecting_selfcheck = true;
                    msgs.push(serde_json::json!({
                        "role": "user",
                        "content": crate::prompts::SELFCHECK_PROMPT
                            .replace("{tool_name}", &sc_name)
                            .replace("{snippet}", &sc_snippet),
                    }));
                }
            }
        }
    }
    if let Some(hint) = fail_hint_queued.take() {
        msgs.push(serde_json::json!({
            "role": "user",
            "content": hint,
        }));
    }
    if *soft_warn_queued {
        *soft_warn_queued = false;
        msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】你已累计调用 {soft_warn_at} 个工具（全程累计），最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                    fuse_cap - soft_warn_at
                ),
            }));
    }
    Ok(DispatchOutcome::Continue)
}
