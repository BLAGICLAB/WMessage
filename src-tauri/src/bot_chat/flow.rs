//! 聊天主流程：入口命令的内层实现、技能路由、并发闸。

use super::*;

/// 机器人开关关闭 → BotDisabled（recoverable=true，引导去设置页开启）。
/// 抽成纯函数便于单测（tauri command 绑定 Wry AppHandle，mock_app 无法直接调用）。
pub(super) fn require_bot_enabled(enabled: bool) -> CommandResult<()> {
    if !enabled {
        return Err(CommandError::BotDisabled);
    }
    Ok(())
}

impl ChatGuard {
    /// Ok = 允许进入（无会话 id 不加锁）；Err = 本会话已有执行实例在跑
    pub(crate) fn acquire<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Option<&str>,
    ) -> Result<Self, ()> {
        let running = chat_running(app);
        let Some(sid) = session_id else {
            return Ok(Self {
                session_id: None,
                running,
            });
        };
        {
            let mut set = crate::db::lock::lock_or_recover(&running, "app_state::chat_running");
            if !set.insert(sid.to_string()) {
                return Err(());
            }
        }
        Ok(Self {
            session_id: Some(sid.to_string()),
            running,
        })
    }
}

impl Drop for ChatGuard {
    fn drop(&mut self) {
        if let Some(sid) = &self.session_id {
            crate::db::lock::lock_or_recover(&self.running, "bot_chat::ChatGuard::running")
                .remove(sid);
        }
    }
}

/// 会话锁是否被持有：tests/task_chat_exec.rs 断言
/// run_task_in_chat 执行期持有 ChatGuard——bot_execute_task 纳入会话锁的回归证据。
/// 生产代码不调用。表随 `AppState` 走，故取注入实例（缺失时兜底实例，
/// 与生产路径用的是同一个——集成测试的 mock handle 没注入时两边都落到兜底）。
#[cfg(debug_assertions)]
pub fn chat_guard_is_held<R: tauri::Runtime>(app: &AppHandle<R>, session_id: &str) -> bool {
    crate::db::lock::lock_or_recover(&chat_running(app), "bot_chat::chat_running")
        .contains(session_id)
}

///  辅助：start_skill + 审计。成功 log `pre_step.route_skill`；失败 log `pre_step.route_failed`。
/// 返回 `Option<(SkillMeta, String)>`：None 表示路由失败 → 放行 LLM（不阻断聊天）。
pub(super) fn start_skill_with_audit(
    app: &tauri::AppHandle,
    skill_name: String,
    session_id: Option<&str>,
) -> Option<(SkillMeta, String)> {
    match crate::bot_skills::start_skill(app, &skill_name, serde_json::Value::Null, session_id) {
        Ok((meta, body)) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "pre_step.route_skill",
                "skill" => skill_name,
                "mode" => meta.mode.clone(),
            );
            Some((meta, body))
        }
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "pre_step.route_failed",
                "skill" => skill_name,
                "error" => e,
            );
            None
        }
    }
}

///  抽出的「步骤 4」函数：处理 pre_step 命中 Skill 路由后的全部副作用。
///
/// 返回值契约：
/// - `Done(BotChatResult)`：auto-mode 调度器返回终态（Done / AwaitUser），调用方应直接 return 该结果
/// - `FallThrough { recovery_hint }`：需要落入「步骤 5」继续走 run_model_loop：
///   - interactive 模式 → recovery_hint 为 None（body 由 caller 拼入 system prompt）
///   - auto-mode 调度器返回 FailedButRecoverable → recovery_hint 为 Some(...)
/// - 调度器返回 Terminated → 走 Err(CommandError::Internal)，不进 Outcome
///
/// 调用方负责：
/// - 在调用本函数前完成 pre_step 路由 + bypass switch（pre_routed_skill 已通过 bypass）
/// - 在收到 FallThrough 时把 (meta, body) 装回 pre_routed_skill 并把 recovery_hint 写到主流程变量
pub(super) async fn apply_skill_route(
    app: &tauri::AppHandle,
    meta: SkillMeta,
    _body: String,
    stop: &StopGuard,
    _session_id: Option<&str>,
) -> CommandResult<SkillRouteOutcome> {
    if meta.mode != "auto" {
        // interactive（以及未来其他非 auto 模式）：body 由 caller 拼入 system prompt
        return Ok(SkillRouteOutcome::FallThrough {
            recovery_hint: None,
        });
    }
    // auto-mode：调用 Skill 调度器，四种 DslOutcome 映射
    match crate::bot_skills::run_skill_scheduler(app, &meta.name, stop.session_id(), Some(stop))
        .await
    {
        Ok(crate::bot_skills::DslOutcome::Done(text)) => {
            Ok(SkillRouteOutcome::Done(BotChatResult {
                text,
                task_refs: Vec::new(),
                busy: false,
                trace_id: None,
            }))
        }
        Ok(crate::bot_skills::DslOutcome::AwaitUser) => {
            // 不把内部哨兵 "__await_user__" 当回复文本直出给前端（前端无该哨兵的
            // 处理逻辑，用户会看到原始字符串），改出可读提示
            crate::bot::audit_log(
                app,
                &format!(
                    "skill_await_user | name: {} | 已暂停等待用户确认",
                    crate::bot::truncate_for_log(&meta.name, 60)
                ),
            );
            Ok(SkillRouteOutcome::Done(BotChatResult {
                text: format!(
                    "⏸ 技能「{}」已暂停，正在等待你的确认——请在确认弹窗里选择后继续。",
                    meta.name
                ),
                task_refs: Vec::new(),
                busy: false,
                trace_id: None,
            }))
        }
        Ok(crate::bot_skills::DslOutcome::FailedButRecoverable {
            reason,
            completed_summary,
            rollback_attempted,
        }) => {
            // SSE 推 Skill 失败给前端（让用户看到半成品 + rollback 状态）
            // 会话号由 emit_recorded 统一盖信封，前端按会话过滤，防串会话弹失败卡
            crate::bot::stream_log::emit_recorded(
                app,
                stop.session_id(),
                "bot-skill-failed",
                serde_json::json!({
                    "skillName": meta.name,
                    "reason": reason,
                    "completedSummary": completed_summary,
                    "rollbackAttempted": rollback_attempted,
                }),
            );
            // LLM 兜底：把「失败原因 + 已完成产物 + 回滚状态」拼进 system prompt 决策
            Ok(SkillRouteOutcome::FallThrough {
                recovery_hint: Some(format_recovery_hint(
                    &reason,
                    &completed_summary,
                    rollback_attempted,
                )),
            })
        }
        Err(crate::bot_skills::DslFailure::Terminated { reason }) => {
            Err(skill_terminated_error(reason))
        }
    }
}

/// 技能调度器终止（无可执行步骤 / DSL 解析失败等内容形态问题）的错误分类。
/// 这是可诊断的预期失败，不是内部故障——与运行时步骤熔断同族，走
/// DomainRule(skill)，不降级 INTERNAL（「内部错误」会把用户引向报障而非
/// 修技能内容）。
fn skill_terminated_error(reason: String) -> CommandError {
    CommandError::DomainRule {
        domain: "skill".to_string(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 技能终止的用户可见文案不得再挂「内部错误」前缀——内容形态问题是
    /// 域规则失败，用户该做的是改技能正文，不是反馈日志。
    #[test]
    fn skill_termination_is_not_classified_internal() {
        let err = skill_terminated_error("技能「t」无可执行步骤（DSL 解析为空）".to_string());
        let msg = err.to_string();
        assert!(!msg.contains("内部错误"), "不得标成内部故障：{msg}");
        assert!(msg.contains("[skill]"), "应走 skill 域规则分类：{msg}");
    }
}

/// 会话锁被占时的拒答结果。抽成纯函数便于单测（构造点内联在 Tauri 命令里，
/// mock_app 跑不到那一层）。
pub(super) fn busy_chat_result() -> BotChatResult {
    BotChatResult {
        text: CHAT_BUSY_REPLY.into(),
        task_refs: vec![],
        busy: true,
        trace_id: None,
    }
}

/// 本轮的发起方。
///
/// 汇报轮由后端主动发起（子任务收尾转述，见 `report_subagent_result`），它带着库里的
/// 对话历史却没有用户输入。历史上「有没有用户消息」等价于「messages 是否为空」，
/// 带上历史之后这个等价就断了，所以发起方必须做成显式入参：靠 `messages.is_empty()`
/// 猜出来的口径，会让汇报轮把用户的旧消息当成本轮输入，跑挂起恢复、技能路由和计划生成。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TurnOrigin {
    /// 前端发起的用户回合
    User,
    /// 后端发起的汇报轮
    Report,
}

/// `bot_chat_inner` 的产出：模型答复 + 本轮待记「已吸收」的子 agent id。
///
/// 「已吸收」不由这一层记：谁送达、谁记。交互聊天（前端驱动）在命令返回前
/// 记——模型确实看到了那段回报；汇报轮（后端驱动）要等回复落库成功才记——
/// 它的历史下一轮从库里读，没落库的回复等于没说出去，落库失败还记着就等于
/// 「已经汇报过了」，重试时那段回报不在 digest 里，子任务结果就此永久消失。
pub(super) struct ChatOutcome {
    pub(super) result: BotChatResult,
    /// 本轮 digest 段里被吸收的子 agent id
    pub(super) absorb: Vec<String>,
}

#[allow(clippy::too_many_lines)]
pub(super) async fn bot_chat_inner(
    app: AppHandle,
    messages: Vec<ChatMsg>,
    session_id: Option<String>,
    reasoning_effort: Option<String>,
    origin: TurnOrigin,
) -> CommandResult<ChatOutcome> {
    require_bot_enabled(bot_get_enabled(app.clone()))?;
    // 会话级防重入：同会话并发消息直接拒绝，防技能路由/start_skill 竞争
    let _chat_guard = match ChatGuard::acquire(&app, session_id.as_deref()) {
        Ok(g) => g,
        Err(()) => {
            crate::bot::audit_log(
                &app,
                &format!(
                    "bot_chat_rejected | session: {} | 已有执行实例在跑（防重入拦截）",
                    crate::bot::truncate_for_log(session_id.as_deref().unwrap_or("<none>"), 60)
                ),
            );
            return Ok(ChatOutcome {
                result: busy_chat_result(),
                absorb: Vec::new(),
            });
        }
    };
    // L0 pre_ingest（设计 §二）：输入侧唯一改写点，在**所有**输入判定之前。
    // 位置是刻意的——挂起恢复、技能路由、纠正检测、批量执行触发四处都扫
    // 同一条原文，只要有一处读到未清洗的原文，规范化就只做了一半
    // （「做\u{FEFF}PPT」照旧匹配不上技能）。
    let mut messages = messages;
    // 汇报轮没有用户输入动作，L0 不改写库里的历史消息（改的只是内存副本，
    // 却会留下一条 user.input_normalized 审计 + 让模型看到的末条与库里原文
    // 不一致）；L0 的价值在清洗用户刚发的内容，只在用户轮做。
    if origin == TurnOrigin::User {
        if let Some(n) = normalize_user_input(&mut messages) {
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Info,
                "user.input_normalized",
                "stripped" => n.stripped.to_string(),
            );
        }
    }
    // 本轮的用户输入：汇报轮恒为 None（它没有用户发言，只是把库里的历史摆回模型面前）。
    // 钉成一个变量而不是让各处各读一次 messages.last()：门控收在这里一处，将来
    // 新增输入侧判定照旧读 user_input，就不会漏掉汇报轮这道门。
    // 位置在 L0 之后——判定要读清洗过的文本；在 truncate 之前——truncate 会消耗掉
    // messages（「最后一条永远保留」目前成立，但把门控绑在一段会被重建的 vec 上，
    // 下一个改截断的人就会静默拿到错的值）。
    let user_input: Option<ChatMsg> = match origin {
        TurnOrigin::User => messages.last().cloned(),
        TurnOrigin::Report => None,
    };
    // 步骤 1：逐步执行挂起恢复：有子任务待确认时，本条消息是对执行流程的应答
    // （继续/重做/停），优先于一切聊天路由。/stop 走独立命令（bot_stop 内清挂起）。
    // 汇报轮 user_input=None，整体跳过：它不是用户的应答，不该劫持挂起中的流程。
    if let Some(last) = user_input.as_ref() {
        if crate::exec_steps::has_pending_for(&app, session_id.as_deref()) {
            let result =
                crate::exec_steps::resume(&app, &last.content, session_id.as_deref()).await?;
            return Ok(ChatOutcome {
                result,
                absorb: Vec::new(),
            });
        }
    }
    let stop = StopGuard::new(&app, true, session_id.clone());
    // 步骤 2：bypass_llm_on_pre_step_hit 开关读取：
    // true = 新行为（pre-step 路由生效），false = LEGACY 旧链路（路由命中一律丢弃，LLM 自由决策）。
    // 必须在任何路由判定之前读取——主流程禁止任何步骤抢跑。
    let bypass_llm_on_pre_step_hit = crate::bot::read_bypass_llm_switch(&app);
    // 审计：记录本轮用户最新指令（截断防刷日志）
    if let Some(last) = user_input.as_ref() {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "user.message",
            // write_event 已统一转义 kv 值，这里只做长度截断，避免二次转义
            "content" => last.content.chars().take(300).collect::<String>(),
        );
        // 用户纠正信号检测（Round 4 学习闭环）：push-back 模式命中 →
        // (a) 写 user.correction 事件 + 持久化为记忆 item（fire-and-forget）；
        // (b) 即时触发一次聚类判定：簇 ≥3 → record_lesson（同样 fire-and-forget）。
        //     二者均不 await，主流程零阻塞。
        if let Some(pat) = crate::bot::corrections::detect_correction(&last.content) {
            crate::bot::corrections::record_correction(
                &app,
                session_id.as_deref(),
                &last.content,
                pat,
            );
            let app3 = app.clone();
            tauri::async_runtime::spawn(async move {
                // 即时路径：刚写一条 correction，立刻尝试聚类不能漏历史同源 → since=0 走全表
                crate::bot::corrections::try_cluster_correction_lessons(&app3, 0).await;
            });
        }
    }
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    // 技能清单动态注入：系统提示词 + 已安装技能的「名称+描述」（progressive disclosure 第一层；
    // 全文由 use_skill 工具按需读取，省 token）
    //  改造：用 SystemPromptBuilder 按 PromptSlot 声明顺序排序拼接，替代脆弱的
    // 顺序 format!("{}{}", a, b) 链。Base/GenDir/SkillCatalog 是 system_base 的 3 段，
    // 段间用 \n\n 分隔；后续 SkillBody/Recovery/Plan 拼在尾部，无分隔符。
    let mut prompt = SystemPromptBuilder::new();
    prompt.push(PromptSlot::Base, SYSTEM_PROMPT);
    // 主 agent 派发子 agent 的职责段——同 Base 槽位按 push
    // 顺序追加在 SYSTEM_PROMPT 之后（同槽多次 push 按顺序拼接，无新 slot）。
    prompt.push(
        PromptSlot::Base,
        format!("\n\n{}", crate::prompts::MAIN_AGENT_ADDENDUM.trim_start()),
    );
    // 动态设置项放末尾的 VerifierRetryTail 槽位：保持 Base/GenDir/SkillCatalog
    // 这些「静态前缀」字字节节稳定，让 Anthropic prompt caching 断点 1（system
    // 末尾）能跨会话命中——若把 verifier_retry_max 写进 Base 段，每会话变动
    // 会让 GenDir/SkillCatalog/工具 schema 缓存全部作废（miss → 1.25× 写价）。
    let verifier_retry_max_cfg = crate::bot::params::resolve_verifier_retry_max(
        crate::bot::bot_get_config(app.clone())
            .ok()
            .and_then(|c| c.verifier_retry_max),
    );
    prompt.push(
        PromptSlot::VerifierRetryTail,
        format!(
            "\n\n## 当前设置\nverifier 拒绝重试预算上限 = {verifier_retry_max_cfg}\
             （0 = 不重试；循环同一任务 verifier → executor 修复 → 再 verifier 最多此数）"
        ),
    );
    prompt.push(PromptSlot::GenDir, format!("\n\n{}", gen_dir_rule(&app)));
    prompt.push(
        PromptSlot::SkillCatalog,
        format!(
            "\n\n{}",
            // 按用户末条消息做语义化技能推荐排序（embed 不可用降级全清单）
            crate::bot_skills::build_skill_block_for(
                &app,
                user_input.as_ref().map(|m| m.content.as_str()),
            )
            .await
        ),
    );

    // 步骤 3：pre-step 路由（单一出口：route_user_input_for 内部先判批量执行、再查意图路由表）：
    // - RouteAction::ExecuteTasks（「完成/执行」命令位关键词 + [已选任务] 引用块）→ 批量执行选中任务卡
    // - RouteAction::Skill 关键词 L1 硬锁命中复合业务 → start_skill
    // - PassThrough → 放行进 LLM
    // 仅处理用户最新一条消息（后续轮次走原 LLM 路径）。
    // 路由命中的处理全部发生在本步骤之后，任何步骤不得抢跑、不得前置 return。
    let pre_routed_skill: Option<PreStepRoute> = if let Some(last) = user_input.as_ref() {
        match crate::intent_router::route_user_input_for(&last.content, stop.session_id()) {
            RouteAction::Skill(skill_name) => {
                start_skill_with_audit(&app, skill_name, stop.session_id())
                    .map(|(meta, body)| PreStepRoute::Skill(meta, body))
            }
            RouteAction::ExecuteTasks(task_ids) => Some(PreStepRoute::ExecuteTasks(task_ids)),
            RouteAction::PassThrough => None,
        }
    } else {
        None
    };
    //  开关：bypass=false → LEGACY 旧链路（强制 pre_routed_skill = None，让 LLM 自由决策）。
    // 新行为（bypass=true）→ 保留路由结果让 pre-step 路由继续工作。
    let pre_routed_skill = if bypass_llm_on_pre_step_hit {
        pre_routed_skill
    } else {
        // LEGACY：audit 记录「pre-step 命中但 bypass 关闭 → 丢弃匹配」（Skill 与批量执行一视同仁）
        match &pre_routed_skill {
            Some(PreStepRoute::Skill(meta, _)) => {
                crate::audit_event!(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "pre_step.bypass_off",
                    "route" => "skill",
                    "skill" => meta.name.clone(),
                    "mode" => meta.mode.clone(),
                );
            }
            Some(PreStepRoute::ExecuteTasks(task_ids)) => {
                crate::audit_event!(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "pre_step.bypass_off",
                    "route" => "chat_execute",
                    "task_count" => task_ids.len(),
                );
            }
            None => {}
        }
        None
    };
    let (pre_routed_skill, batch_execute_tasks) = match pre_routed_skill {
        Some(PreStepRoute::Skill(meta, body)) => (Some((meta, body)), None),
        Some(PreStepRoute::ExecuteTasks(task_ids)) => (None, Some(task_ids)),
        None => (None, None),
    };
    // 选择任务卡批量执行（路由终态，不是前置短路：步骤 1-3 已按序完成）：
    // B 方案（1=宽松 / 2=继续 / 3=共用 stop）：每张卡复用
    // run_task_in_chat（EXECUTE_SYSTEM_PROMPT + 50 轮工具循环）；外层 stop 在
    // 卡片之间判定：聊天里 /stop 一次能阻止后续所有卡启动（正在执行的卡有
    // 自己新会话的 StopGuard，不受影响，跑到本卡结束）。定时任务模式
    // （bot_scheduler，interactive=false）同样只走 run_task_in_chat，不经本聊天流程，互不干扰。
    if let Some(task_ids) = batch_execute_tasks {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "chat_execute.routed",
            "task_count" => task_ids.len(),
        );
        let result = chat_execute_tasks(&app, task_ids, stop, BatchPolicy::default()).await?;
        return Ok(ChatOutcome {
            result,
            absorb: Vec::new(),
        });
    }
    // auto-mode Skill → 调用 apply_skill_route（步骤 4 抽出）；interactive 模式走 FallThrough。
    // LLM 兜底路径：FailedButRecoverable 不再 return，把 recovery_hint 拼进 system_content，
    // 继续走 run_model_loop 让 LLM 决策下一步。
    let mut recovery_hint: Option<String> = None;
    if let Some((meta, body)) = pre_routed_skill.as_ref() {
        match apply_skill_route(
            &app,
            meta.clone(),
            body.clone(),
            &stop,
            session_id.as_deref(),
        )
        .await?
        {
            SkillRouteOutcome::Done(result) => {
                return Ok(ChatOutcome {
                    result,
                    absorb: Vec::new(),
                })
            }
            SkillRouteOutcome::FallThrough {
                recovery_hint: hint,
            } => {
                recovery_hint = hint;
            }
        }
    }
    // 轮数三层优先级：Skill frontmatter 自报 > 配置 maxRounds > 默认 50
    let cfg_for_rounds = crate::bot::load_config(&app);
    let max_rounds = crate::bot_model_loop::resolve_max_rounds(
        pre_routed_skill
            .as_ref()
            .and_then(|(meta, _)| meta.max_rounds),
        crate::bot::params::resolve_max_rounds_cfg(&cfg_for_rounds) as usize,
    );
    let pre_routed_active_skill = pre_routed_skill.map(|(_, body)| body);
    // interactive 模式的技能正文拼入 system prompt（无分隔符接 SkillCatalog）
    if let Some(active_skill) = pre_routed_active_skill {
        prompt.push(PromptSlot::SkillBody, active_skill);
    }
    // auto-mode 失败兜底：recovery_hint 拼入让 LLM 决策下一步
    if let Some(hint) = recovery_hint {
        prompt.push(PromptSlot::Recovery, hint);
    }
    // PREVR 第 2 层：复杂多步任务先生成动态计划再执行。
    // 触发保守：needs_plan 启发式命中才多花一次 Planner 调用；
    // Planner 失败/输出非法 → None → 原自由循环，不阻断聊天。
    // 仅聊天主路径启用：任务卡执行（run_task_in_chat）/ 逐步执行（exec_steps）
    // 目标单一明确，不需要规划。汇报轮同理——它只该转述结果，不该顺手生计划。
    let last_user_text = user_input
        .as_ref()
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let mut plan_state: Option<crate::bot_plan::PlanState> =
        if crate::bot_plan::needs_plan(&last_user_text) {
            crate::bot_plan::generate_plan(&app, &last_user_text)
                .await
                .map(|steps| crate::bot_plan::PlanState {
                    task: last_user_text.clone(),
                    steps,
                    replans_used: 0,
                })
        } else {
            None
        };
    if let Some(plan) = &plan_state {
        prompt.push(
            PromptSlot::Plan,
            crate::bot_plan::format_plan_block(&plan.steps),
        );
    }
    msgs.push(serde_json::json!({"role": "system", "content": prompt.build()}));
    // 历史字符预算——长会话最旧的先丢，
    // 最后一条（本轮用户消息）永远保留；丢弃时留审计
    // 截断即摘要（设计 5.2）：超预算先对将丢弃的消息
    // 生成摘要；摘要不落库、不进 ChatMsg（只在本轮请求里并进末条消息）；
    // LLM 失败静默退回直接丢弃
    // 历史预算走配置解析（config historyBudgetChars，钳 20K..=500K；默认 100K）
    let cfg_for_budget = crate::bot::load_config(&app);
    let budget_used = crate::bot::params::resolve_history_budget_chars(&cfg_for_budget) as usize;
    // 截断滞回比例（config historyTruncateTargetPct，钳 50..=90；默认 70%）：
    // 超预算时一次裁到目标比例而不是贴着预算边裁——贴边裁每轮都可能只丢
    // 一两条，把缓存前缀断成每轮一次。留出余量能稳住好几轮的前缀。
    let truncate_ratio = crate::bot::params::resolve_history_truncate_target_ratio(&cfg_for_budget);
    let (messages, summary, dropped) = truncate_chat_history_with_summary(
        &app,
        session_id.as_deref(),
        messages,
        budget_used,
        truncate_ratio,
    )
    .await;
    // 记忆块（设计第 6 节）：检索查询 = 本轮用户消息原文 ≤200 字（汇报轮
    // 没有用户消息 → 空查询：记忆块照常注入，只是没有语义检索的抓手）；
    // DB 任何失败静默降级为无记忆块（build_memory_block 内部兜底）。
    let memory_query = user_input
        .as_ref()
        .map(|m| m.content.chars().take(200).collect::<String>())
        .unwrap_or_default();
    let memory_block = build_memory_block(&app, &memory_query).await;
    // 摘要 + 记忆块并进**末条**消息（末条 = 本轮用户发言）。
    //
    // 位置是刻意选的：这两块每轮都变（摘要按截断生成、记忆块按本轮用户消息
    // 做语义检索），原先它们卡在主 system 之后、全部历史之前——一变，后面
    // 整段历史的隐式前缀缓存当场作废，长会话每轮都在付全价 input。挪到末条
    // 消息里之后，稳定前缀 = 主 system + 全部历史，变的只有末条那一小段。
    //
    // 形态是「并进用户消息」而不是「另起 system 消息」：Anthropic 协议转换
    // 会把数组里所有 role=system 的消息提到顶层 system 字段，另起 system
    // 消息在 Anthropic 路径上等于没挪。
    let injection = build_injection(summary.as_deref(), memory_block.as_deref());
    if dropped > 0 {
        crate::audit_event!(&app, crate::audit::AuditLevel::Info, "chat.history_truncated",
            "dropped" => dropped, "budget" => budget_used);
    }
    // 最后 3 条消息内 user 消息的图片附件转多模态消息（更早的历史降级为路径文本）
    // 汇报轮不回看图片：历史里那几张图与「转述子任务结果」无关，每轮重读磁盘 +
    // 整图 base64 重发是白花的开销。
    let img_indices = match origin {
        TurnOrigin::User => image_attach_indices(&messages),
        TurnOrigin::Report => Vec::new(),
    };
    // 先把每条历史消息的最终 content 备齐（纯文本或多模态块数组），
    // 再由 build_history_msgs 统一组装消息数组并把注入块并进末条。
    let mut contents: Vec<serde_json::Value> = Vec::with_capacity(messages.len());
    for (i, m) in messages.iter().enumerate() {
        if img_indices.contains(&i) {
            // attach_images 全程同步 FS（canonicalize 白名单判定 + 整图读入转 base64，
            // 单图至多数 MB）——包 spawn_blocking，防多图/并发会话把 async runtime
            // 的 worker 钉住（同 create_exec_session 先例）；阻塞任务崩溃时降级为纯文本
            let app_img = app.clone();
            let content_img = m.content.clone();
            let img_content =
                tauri::async_runtime::spawn_blocking(move || attach_images(&app_img, &content_img))
                    .await
                    .unwrap_or_else(|_| serde_json::json!(m.content));
            contents.push(img_content);
        } else {
            contents.push(serde_json::json!(m.content));
        }
    }
    msgs.extend(build_history_msgs(
        &messages,
        &contents,
        injection.as_deref(),
    ));
    // L2 落点 · 子任务回报（设计 §3.7「下一轮 assemble 检查」）：父 agent 派出去的
    // 子任务，终态的在这里打包成一段注入，在途的只报计数。放最后是刻意的——
    // 模型对本轮末尾的内容权重最高，而回报正是「此刻新增」的信息。
    // 只注 result_json，子会话历史留在它自己的会话行里（§3.7 要点 3）。
    //
    // 汇报轮（TurnOrigin::Report）没带用户消息，模型面前只有系统提示词、历史和下面这段
    // 回报；不说清这一轮要它做什么，它会对着历史反问用户「你想让我做什么」。
    // 放在回报之前是为了不打断末尾那段的最高权重。
    if origin == TurnOrigin::Report {
        msgs.push(serde_json::json!({"role": "system", "content": REPORT_TURN_PROMPT}));
    }
    let digest = crate::bot_orchestrator::subagent_digest_block(&app, session_id.as_deref()).await;
    if let Some(d) = &digest {
        msgs.push(serde_json::json!({"role": "system", "content": d.text}));
    }
    // Failure 分支可达——模型循环 Err（LLM 5xx/流断/头超时等）也落 trace
    //（此前只在 Ok 收尾处 hook，失败轨迹全丢，trace 的 Failure 采样规则不可达）。
    // reason 用粗分类，不带原始错误消息（含路径/参数，不入 trace/audit 明细）
    // 收尾自动记忆抽取要用的句柄（run_model_loop 按值消费 app，提前克隆）
    let extract_app = app.clone();
    // 词元统计（P4+）：主聊天也落 exec_traces——此前只有任务执行链落 trace，
    // 设置页「词元统计」对纯聊天用户恒为空（卡片副标题与事实不符的根因）。
    // begin 降级 None 不阻断；end 无论成败成对调用（loop_result 先落捕获再分流）。
    // origin="chat" 与 manual/scheduled/batch 同列展示。
    let trace_app = app.clone();
    let trace_capture = std::sync::Arc::new(std::sync::Mutex::new(
        crate::trace_sink::TraceCapture::default(),
    ));
    // 失败路径的执行明细回传槽（run_model_loop 出错时写这里）
    let failed_trace = std::sync::Arc::new(std::sync::Mutex::new(None));
    // trace id 回传：开 trace 时就把 id 留在局部变量里，端到端带回结果。
    // 不能等收尾再从捕获槽取——end_trace 会把 trace_id take 走，那时拿到的
    // 永远是 None。None = 本回合没开 trace（无会话 id / 建行失败降级），不是错误。
    let mut chat_trace_id: Option<i64> = None;
    if let Some(sid) = session_id.as_ref() {
        let trace_id = crate::trace_sink::begin_trace(&trace_app, sid, None, "chat", None).await;
        chat_trace_id = trace_id;
        if let Ok(mut c) = trace_capture.lock() {
            c.trace_id = trace_id;
            c.session_id = Some(sid.clone());
        }
    }
    let loop_result = crate::bot_model_loop::run_model_loop(
        app,
        msgs,
        max_rounds,
        &stop,
        plan_state.as_mut(),
        reasoning_effort,
        // per-onion 温度覆盖：主聊天不挂（仅子 agent 用）
        None,
        // 失败轨迹也要落 trace：熔断/API 失败时把已跑出的轮数工具数交回，
        // 否则 trace 记 turns=0（跑满 50 轮撞熔断那次实测就是 0）
        Some(&failed_trace),
        // 主聊天不参与子 agent 墙钟治理
        None,
    )
    .await;
    // 「已吸收」的 id 交出去，不在这里记：谁送达、谁记。
    // 模型循环失败时这段回报压根没送进模型，提前记等于把「已经说过了」提前
    // 宣布——汇报轮会因此以为说过、从此不再重提，子任务结果就此永久丢失。
    let absorb: Vec<String> = digest
        .as_ref()
        .map(|d| d.absorb.clone())
        .unwrap_or_default();
    // LoopTrace 统计（tokens/轮数/模型名）塞进捕获，end_trace 汇总落库。
    // 失败时从 failed_trace 句柄取回——Err 路径没有返回值可带明细。
    {
        let captured = match &loop_result {
            Ok((_, _, lt)) => Some(lt.clone()),
            Err(_) => failed_trace.lock().ok().and_then(|g| g.clone()),
        };
        if let (Some(lt), Ok(mut c)) = (captured, trace_capture.lock()) {
            c.stats = Some(lt);
        }
    }
    crate::trace_sink::end_trace(
        &trace_app,
        &trace_capture,
        loop_result.is_ok(),
        // 用户停止时 run_model_loop 返回 Ok（「⏹ 已停止」），须显式传停止标志
        stop.stopped(),
        loop_result.as_ref().err().map(|e| e.to_string()),
    )
    .await;
    let (text, refs, _loop_trace) = match loop_result {
        Ok(v) => v,
        Err(e) => {
            return Err(e);
        }
    };
    //  自动记忆抽取（fire-and-forget）：交互式会话收尾触发，抽取失败静默
    // 降级不冒泡；非交互（任务执行/定时）、/stop 中止的会话、非 off 档的判定
    // 在入口内完成。session_id 此后不再使用，按值转移。
    if !stop.stopped() {
        crate::memory::extract::maybe_extract_from_session(
            extract_app,
            session_id,
            stop.is_interactive(),
        );
    }
    Ok(ChatOutcome {
        result: BotChatResult {
            text,
            task_refs: refs,
            busy: false,
            trace_id: chat_trace_id,
        },
        absorb,
    })
}
