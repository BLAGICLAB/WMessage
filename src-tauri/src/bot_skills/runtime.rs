use super::parse::SkillMeta;
use super::state::{load_skill_meta, now_ms, skill_runs, SkillRun, SkillState};
use crate::audit_event;
use crate::bot::registry::ToolResult;
use crate::error::CommandError;
use tauri::AppHandle;

/// 前置预审（Harness 意图预审层）：禁用/黑名单拒绝；超长/非法字段已在 parse_meta 兜底。
/// 返回 Ok 表示放行启动。
fn preflight(meta: &SkillMeta) -> Result<(), String> {
    if !meta.enabled {
        return Err(format!("技能「{}」已被禁用", meta.name));
    }
    // 黑名单意图关键词（与系统提示词安全红线一致）：命中即拒绝启动
    const BLACKLIST: [&str; 5] = [
        "全盘遍历",
        "批量删除",
        "无确认删除",
        "遍历文件系统",
        "清空所有",
    ];
    let hay = format!(
        "{} {}",
        meta.description.to_lowercase(),
        meta.intents.join(" ")
    );
    if let Some(hit) = BLACKLIST.iter().find(|b| hay.contains(&b.to_lowercase())) {
        return Err(format!(
            "技能「{}」意图命中安全黑名单（{hit}），已拒绝启动",
            meta.name
        ));
    }
    Ok(())
}

/// 同名技能冲突判定：SKILL_RUNS 以技能名为键，
/// 别的会话的 Running/Paused 同名 run 会被 insert 顶掉导致安全闸全失——启动前拒绝。
fn skill_conflict_with_existing(existing: Option<&SkillRun>, session_id: Option<&str>) -> bool {
    matches!(existing, Some(r) if (r.state == SkillState::Running || r.state == SkillState::Paused)
        && r.session_id.as_deref() != session_id)
}

/// 启动 Skill：use_skill 工具调用即启动生命周期（预审 → Running），返回文档 + 运行约束提示。
/// session_id：记录触发会话，活动判定/暂停/确认/推进按会话过滤（会话隔离）。
pub fn start_skill(
    app: &AppHandle,
    name: &str,
    session_id: Option<&str>,
) -> Result<(SkillMeta, String), String> {
    let (meta, body) = load_skill_meta(app, name)?;
    preflight(&meta)?;
    // 预审已通过 → 直接进入 Running：步骤钩子按 Running/Paused 查找，
    // 停在 Loaded 会让计数/熔断/动作记录全部静默失效
    let mut run = SkillRun::new(&meta);
    run.state = SkillState::Running;
    run.session_id = session_id.map(|s| s.to_string());
    let registry = skill_runs(app);
    let mut runs = registry.lock().unwrap_or_else(|e| e.into_inner());
    // 同名 run 属于别的会话且仍在活动 → 拒绝启动，
    // 否则 insert 会把对方的步数/超时熔断、动作记录、收尾整个顶掉。
    // 同会话同名重启（覆盖语义）与终态 run 不拦截。
    if skill_conflict_with_existing(runs.get(&meta.name), session_id) {
        return Err(format!(
            "技能「{}」正在另一个会话/任务中运行，请等它结束后再启动",
            meta.name
        ));
    }
    // 单活动技能策略：新技能启动时，把本会话其他 Running/Paused 技能标 Completed。
    // 步骤钩子/暂停/确认按「唯一活动技能」定位，多技能同时 Running 会导致
    // 步骤计数与暂停确认全部记到第一个技能、后续技能被静默忽略。
    // 会话隔离：只结束同会话的技能，别的会话的活动技能不受影响。
    let mut switched: Vec<String> = Vec::new();
    for (n, r) in runs.iter_mut() {
        if *n != meta.name
            && (r.state == SkillState::Running || r.state == SkillState::Paused)
            && r.session_id.as_deref() == session_id
        {
            r.state = SkillState::Completed;
            r.end_reason = "切换到其他技能".into();
            switched.push(n.clone());
        }
    }
    for n in &switched {
        crate::bot::audit_log_hook(
            app,
            &format!(
                "skill_completed | name: {n} | 切换技能结束 | steps: {}",
                runs.get(n).map(|r| r.step).unwrap_or(0)
            ),
        );
    }
    runs.insert(meta.name.clone(), run);
    // 审计：skill.start 结构化
    audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "skill.start",
        "name" => meta.name.clone(),
        "risk" => meta.risk_level.clone(),
        "mode" => meta.mode.clone(),
        "max_steps" => meta.max_steps,
        "timeout_secs" => meta.timeout_secs,
        "rollback" => meta.rollback.clone(),
    );
    Ok((meta, body))
}

/// 工具 use_skill：读取技能文档全文返回给模型。
/// session_id：透传给 start_skill 记录技能归属会话（会话隔离）。
pub fn tool_use_skill(app: &AppHandle, args: &str, session_id: Option<&str>) -> ToolResult {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
    let Some(name) = v["name"].as_str().map(|s| s.trim().to_string()) else {
        // 「use_skill 缺少 name」首字「u」非 error/warn 前缀 → ok
        return ToolResult::ok("use_skill 缺少 name".to_string(), Vec::new());
    };
    match start_skill(app, &name, session_id) {
        Ok((meta, body)) => {
            let hint = format!(
                "【技能文档：{}】\n风险等级 {} · 运行模式 {} · 最多 {} 步 · 超时 {} 秒 · 回滚 {}",
                meta.name,
                meta.risk_level,
                meta.mode,
                meta.max_steps,
                meta.timeout_secs,
                meta.rollback
            );
            let mut out = hint + "\n\n" + &body;
            if meta.mode == "interactive" {
                out.push_str(
                    "\n\n（运行约束：本技能为人机协同模式，中高危动作执行前会暂停等待用户确认）",
                );
            }
            // 技能文档全文，首字符任意 UTF-8 → ok
            ToolResult::ok(out, Vec::new())
        }
        // start_skill Err 返回 String（错误描述），首字可能是错误说明的「失」/「请」等
        // —— 首字符不定 → ok（除非 start_skill 内部显式标 ToolResult，否则这里一律 ok）
        Err(e) => ToolResult::ok(e, Vec::new()),
    }
}

/// 步骤钩子：每次非 use_skill 的工具调用前调用（Harness 七层之外、调度器层）。
/// 职责：步骤计数、步数/超时熔断、动作记录（回滚清单）。
/// 返回 Err 表示该 Skill 必须立即终止（模型收到错误后停止后续步骤）。
/// 步骤检查纯函数（单测入口）：计数、熔断、暂停拒绝、动作记录。不改日志。
fn step_check(run: &mut SkillRun, tool: &str, args: &str, now: i64) -> Result<(), CommandError> {
    if run.state == SkillState::Paused {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: "技能已暂停，等待用户确认中；确认通过后才能继续下一步".to_string(),
        });
    }
    run.step += 1;
    // 步数熔断（Skill 独立上限）
    if run.step > run.max_steps {
        run.state = SkillState::Failed;
        run.end_reason = format!("超过最大步数上限（{} 步）", run.max_steps);
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("技能「{}」{}，已强制终止", run.name, run.end_reason),
        });
    }
    // 超时熔断
    let elapsed = (now - run.started_at_ms) / 1000;
    if elapsed > run.timeout_secs as i64 {
        run.state = SkillState::Failed;
        run.end_reason = format!("超时（超过 {} 秒）", run.timeout_secs);
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("技能「{}」{}，已强制终止", run.name, run.end_reason),
        });
    }
    // 动作记录（回滚清单来源）：只记有副作用的工具，跳过只读查询
    const READONLY: [&str; 4] = ["list_tasks", "search_tasks", "use_skill", "web_search"];
    if !READONLY.contains(&tool) {
        let brief = truncate_skill_args(args, 120);
        run.actions.push(format!("{tool} | {brief}"));
    }
    Ok(())
}

pub fn skill_on_step(
    app: &AppHandle,
    tool: &str,
    args: &str,
    session_id: Option<&str>,
) -> Result<(), CommandError> {
    let registry = skill_runs(app);
    let mut runs = registry.lock().unwrap_or_else(|e| e.into_inner());
    // 会话隔离：只管归属当前会话的活动 Skill，别的会话的不计数不熔断
    let Some(run) = runs.values_mut().find(|r| {
        (r.state == SkillState::Running || r.state == SkillState::Paused)
            && r.session_id.as_deref() == session_id
    }) else {
        return Ok(()); // 无活动 Skill（模型自由调用工具），不干预
    };
    let res = step_check(run, tool, args, now_ms());
    if let Err(e) = &res {
        let reason = run.end_reason.clone();
        crate::bot::audit_log_hook(
            app,
            &format!("skill_failed | name: {} | {reason} | {e}", run.name),
        );
    }
    res
}

/// Post-execute Skill 步骤钩子（洋葱管线「出」钩子）
/// 与 `skill_on_step` 对称：执行工具后调用，记录步骤结果 + 检测失败。
/// 仅在 Skill 处于 Running 时干预；Paused/Completed 等状态不写动作记录。
///
/// 失败判定统一走 `audit::tool_call_failed`：「级别 ≥ Warn 且文本含失败关键词」
/// 这类双条件会漏掉「未知工具」（不含关键词）等形态。
/// 失败 → 把 Skill 标 Failed，写入 end_reason 让 `skill_finish` 知道不再继续后续步骤。
pub fn skill_on_step_post(
    app: &AppHandle,
    tool: &str,
    result: &str,
    dur_ms: u64,
    _level: crate::audit::AuditLevel,
    session_id: Option<&str>,
) {
    let registry = skill_runs(app);
    let mut runs = registry.lock().unwrap_or_else(|e| e.into_inner());
    // 会话隔离：只记录归属当前会话的 Running 技能
    let Some(run) = runs
        .values_mut()
        .find(|r| r.state == SkillState::Running && r.session_id.as_deref() == session_id)
    else {
        return;
    };
    let name = run.name.clone();
    let is_fail = crate::audit::tool_call_failed(tool, result);
    if is_fail {
        run.state = SkillState::Failed;
        run.end_reason = format!("工具 {tool} 执行失败");
        let preview: String = result.chars().take(120).collect();
        crate::bot::audit_log_hook(
            app,
            // preview 是工具结果原文，换行/管道符会撕裂日志行，必须转义
            &format!(
                "skill_step_fail | name: {name} | tool: {tool} | {}",
                crate::bot::truncate_for_log(&preview, 120)
            ),
        );
    } else {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_step_ok | name: {name} | tool: {tool} | {dur_ms}ms"),
        );
    }
}

/// 参数摘要（动作记录用）
fn truncate_skill_args(args: &str, max: usize) -> String {
    let mut s: String = args.chars().take(max).collect();
    if args.chars().count() > max {
        s.push('…');
    }
    s.replace('\n', " ")
}

/// 暂停语义纯函数（单测入口）：把 Running 技能转入暂停等待。
/// resumable=false → terminal_after_confirm=true（确认通过后技能终止，暂停即终止）。
fn pause_state(run: &mut SkillRun) {
    run.state = SkillState::Paused;
    run.terminal_after_confirm = !run.resumable;
}

/// 确认结果纯函数（单测入口）：通过 → 恢复 Running 或暂停即终止；拒绝 → Terminated。
fn confirm_state(run: &mut SkillRun, approved: bool) {
    if approved {
        if run.terminal_after_confirm {
            run.state = SkillState::Terminated;
            run.end_reason = "暂停即终止（技能不支持断点续跑，resumable=false）".into();
        } else {
            run.state = SkillState::Running;
        }
    } else {
        run.state = SkillState::Terminated;
        run.end_reason = "用户拒绝确认".into();
    }
}

/// 高危动作确认开始：本会话活动技能转入 Paused（ask_user_confirm 调用前触发）。
/// 会话隔离：只暂停归属当前会话的 Running 技能，不动别的会话。
pub fn skill_mark_paused(app: &AppHandle, tool: &str, session_id: Option<&str>) {
    let registry = skill_runs(app);
    let mut runs = registry.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = runs
        .values_mut()
        .find(|r| r.state == SkillState::Running && r.session_id.as_deref() == session_id)
    {
        pause_state(run);
        crate::bot::audit_log_hook(
            app,
            &format!(
                "skill_paused | name: {} | at: {tool} | resumable: {}",
                run.name, run.resumable
            ),
        );
    }
}

/// 确认结果到达：恢复 Running / 暂停即终止 / 拒绝终止（bot_confirm_response 调用）。
/// 会话隔离：只作用于归属该会话的 Paused 技能（session 从 ConfirmMap 条目取回）。
pub fn skill_confirm_result(app: &AppHandle, approved: bool, session_id: Option<&str>) {
    let registry = skill_runs(app);
    let mut runs = registry.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = runs
        .values_mut()
        .find(|r| r.state == SkillState::Paused && r.session_id.as_deref() == session_id)
    {
        confirm_state(run, approved);
        let name = run.name.clone();
        let state = format!("{:?}", run.state);
        let reason = run.end_reason.clone();
        crate::bot::audit_log_hook(
            app,
            &format!("skill_confirm | name: {name} | approved: {approved} | -> {state} {reason}"),
        );
    }
}

/// 提取 SKILL.md 正文里的回滚章节（无则空串）。
/// 与 parse_skill_steps 共用 `is_rollback_heading` 统一判定，中英文标题都认——
/// 判定不一致会让按中文标题写的技能回滚段被静默忽略。
fn rollback_section(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|l| super::parse::is_rollback_heading(l))
    else {
        return String::new();
    };
    let mut out = String::new();
    for l in &lines[start + 1..] {
        let t = l.trim();
        if t.starts_with("## ") {
            break; // 到下一个二级标题为止
        }
        out.push_str(l);
        out.push('\n');
    }
    out.trim().to_string()
}

/// 收尾钩子：模型循环结束时调用（成功/失败/用户停止）。
/// 返回回滚建议文本（失败且 rollback=auto 且有动作记录时非空），调用方拼进回复让模型执行逆操作。
/// 会话隔离：只收尾归属当前会话的 Running/Paused 技能，
/// 别的会话的技能不受本会话结束影响。
/// 泛型 Runtime：集成测试可用 MockRuntime 直调真收尾逻辑。
pub fn skill_finish<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    ok: bool,
    reason: &str,
    session_id: Option<&str>,
) -> String {
    let mut rollback_hint = String::new();
    let registry = skill_runs(app);
    let mut runs = registry.lock().unwrap_or_else(|e| e.into_inner());
    for (name, run) in runs.iter_mut() {
        if run.state != SkillState::Running && run.state != SkillState::Paused {
            continue;
        }
        if run.session_id.as_deref() != session_id {
            continue;
        }
        if ok {
            run.state = SkillState::Completed;
            crate::bot::audit_log_hook(
                app,
                &format!("skill_completed | name: {name} | steps: {}", run.step),
            );
        } else {
            run.state = SkillState::Failed;
            run.end_reason = reason.to_string();
            // reason 可含模型给的工具名/错误文本，转义后再落日志
            let mut log = format!(
                "skill_failed | name: {name} | {} | actions: {}",
                crate::bot::truncate_for_log(reason, 200),
                run.actions.len()
            );
            // 回滚建议（务实版）：失败 + 声明可回滚 + 有已执行动作 → 生成建议文本
            if run.rollback == "auto" && !run.actions.is_empty() {
                log.push_str(" | rollback_suggested");
                let actions_text = run
                    .actions
                    .iter()
                    .map(|a| format!("- {a}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let rb_section = rollback_section(
                    &load_skill_meta(app, name)
                        .map(|(_, b)| b)
                        .unwrap_or_default(),
                );
                rollback_hint = format!(
                    "\n\n【技能回滚建议】技能「{name}」执行中断（{reason}），已执行 {n} 个动作：\n{actions_text}\n{}",
                    if rb_section.is_empty() {
                        "该技能未提供回滚章节，请向用户说明已执行动作，由用户决定手动补救。".to_string()
                    } else {
                        format!("技能文档回滚章节：\n{rb_section}\n请先询问用户是否需要回滚；用户同意后，按回滚章节逐条执行逆操作（每一步同样经过安全校验）。")
                    },
                    n = run.actions.len()
                );
            }
            crate::bot::audit_log_hook(app, &log);
        }
    }
    rollback_hint
}

/// 状态机推进建议（主循环决策输入）
///
/// 由 `advance_skill()` 根据当前 SkillState 返回，告诉主循环下一步该做什么。
///
/// 设计动机：把散落在 step_check / skill_on_step_post / skill_finish 的
/// 状态推进决策集中到这一处，未来加横切关注点（审批/沙箱/上下文压缩）
/// 只动 advance_skill，主循环不重构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvanceAction {
    /// 无活动 Skill 或已终结（Loaded/Completed/Failed/Terminated）→ 主循环进入 LLM 自由调用
    NoActive,
    /// 等待用户确认（Paused）→ 主循环跳出当前迭代，等待 bot_confirm_response 唤起
    AwaitConfirm,
    /// 正常运行中 → 继续下一轮迭代
    Continue,
    /// Skill 成功完成 → 调 skill_finish 收尾
    Finish,
    /// Skill 失败 → 调 skill_finish 传错误信息（含 max_steps/超时/工具错误）
    Fail(String),
    /// Skill 被终止 → 调 skill_finish 传终止原因（含用户拒绝/强制停止）
    Terminate(String),
}

/// 状态机推进决策（主循环每轮迭代开头调用）
///
/// 纯函数：不写日志、不改全局状态、不改传入的 run，便于单测。
/// 实时超时检测：即使 step_check 没跑到（长工具执行期间），主循环也能感知超时。
///
/// 调用时机：
///   bot_chat 主循环每次 LLM 响应/工具执行后调一次，根据 AdvanceAction 决定：
///   - Continue → next iteration
///   - AwaitConfirm → break，等待 bot_confirm_response
///   - Finish / Fail / Terminate → 调 skill_finish 收尾，break
///   - NoActive → 继续 LLM 自由调用，下次迭代再问
pub fn advance_skill(run: &SkillRun, now_ms: i64) -> AdvanceAction {
    match run.state {
        SkillState::Loaded => AdvanceAction::NoActive,
        SkillState::Running => {
            // 实时超时检测（即使 step_check 没跑到，主循环也能感知）
            let elapsed = (now_ms - run.started_at_ms) / 1000;
            if elapsed > run.timeout_secs as i64 {
                AdvanceAction::Fail(format!("超时（超过 {} 秒）", run.timeout_secs))
            } else {
                AdvanceAction::Continue
            }
        }
        SkillState::Paused => AdvanceAction::AwaitConfirm,
        SkillState::Completed => AdvanceAction::Finish,
        SkillState::Failed => AdvanceAction::Fail(run.end_reason.clone()),
        SkillState::Terminated => AdvanceAction::Terminate(run.end_reason.clone()),
    }
}

/// DSL 调度器专用决策：把 `advance_skill` 的 6 分支映射到 DSL 调度器可执行的 5 种动作。
///
/// 动机：调度器不能一次性顺序跑所有 step 而不读 SkillRun.state，否则
/// step_check 步数熔断 / skill_on_step_post 工具失败 / skill_terminate_all 用户 /stop
/// 改 state 后，调度器仍会继续跑后续 step（漏停、超时后还跑、用户取消后还跑）。
/// 因此每 step 前查一次，按状态机决策：
/// - Run → 正常跑当前 step（Loaded → NoActive / Running 未超时 → Continue）
/// - Finish → 收尾，跳出循环（Completed）
/// - AwaitUser → 暂停中，返回 `__await_user__` 让主循环挂起（Paused，auto 模式基本不触发，留接口）
/// - FailWithRollback(reason) → 跑 rollback 段 + 报错（Failed / Running 超时）
/// - Terminate(reason) → 不跑 rollback 直接报错（Terminated / 用户主动取消 / 切技能）
///
/// 纯函数：不写日志、不改全局状态、不改 run，便于单测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DslAdvanceAction {
    /// 正常执行当前 step（Loaded / Running 未超时）
    Run,
    /// Skill 已成功完成（Completed）→ 跳出循环，跑汇总
    Finish,
    /// Skill 暂停中等用户确认（Paused）→ 返回 `__await_user__` 给主循环挂起
    AwaitUser,
    /// Skill 失败（Failed / 步数熔断 / 超时）→ 跑 rollback + 报错
    FailWithRollback(String),
    /// Skill 终止（Terminated / 用户 /stop / 切技能）→ 不跑 rollback + 报错
    Terminate(String),
}

pub fn advance_dsl(run: &SkillRun, now_ms: i64) -> DslAdvanceAction {
    match advance_skill(run, now_ms) {
        AdvanceAction::NoActive | AdvanceAction::Continue => DslAdvanceAction::Run,
        AdvanceAction::Finish => DslAdvanceAction::Finish,
        AdvanceAction::AwaitConfirm => DslAdvanceAction::AwaitUser,
        AdvanceAction::Fail(reason) => DslAdvanceAction::FailWithRollback(reason),
        AdvanceAction::Terminate(reason) => DslAdvanceAction::Terminate(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_skills::test_run;

    // ── 状态机推进 ──

    #[test]
    fn advance_loaded_returns_no_active() {
        let run = test_run(8, 180);
        // test_run 初始状态就是 Loaded
        assert_eq!(run.state, SkillState::Loaded);
        assert_eq!(advance_skill(&run, 1000), AdvanceAction::NoActive);
    }

    #[test]
    fn advance_running_no_timeout_continues() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Running;
        // started_at_ms=1000, now=10s 后 11000, timeout=180s → 仍可继续
        assert_eq!(advance_skill(&run, 11000), AdvanceAction::Continue);
    }

    #[test]
    fn advance_running_just_before_timeout_continues() {
        let mut run = test_run(8, 60);
        run.state = SkillState::Running;
        // 恰好 60s 未超时（入 strict > 才 Fail）
        assert_eq!(
            advance_skill(&run, 1000 + 60 * 1000),
            AdvanceAction::Continue
        );
    }

    #[test]
    fn advance_running_timeout_returns_fail() {
        let mut run = test_run(8, 60);
        run.state = SkillState::Running;
        // 启动 61s 后超时 (60s 上限)
        let action = advance_skill(&run, 1000 + 61 * 1000);
        match action {
            AdvanceAction::Fail(reason) => {
                assert!(reason.contains("超时"), "reason 应含「超时」: {reason}");
                assert!(reason.contains("60"), "reason 应含阈值 60s: {reason}");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn advance_paused_returns_await_confirm() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Paused;
        assert_eq!(advance_skill(&run, 1000), AdvanceAction::AwaitConfirm);
    }

    #[test]
    fn advance_completed_returns_finish() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Completed;
        assert_eq!(advance_skill(&run, 1000), AdvanceAction::Finish);
    }

    #[test]
    fn advance_failed_returns_fail_with_end_reason() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Failed;
        run.end_reason = "超过最大步数上限（8 步）".into();
        assert_eq!(
            advance_skill(&run, 1000),
            AdvanceAction::Fail("超过最大步数上限（8 步）".into())
        );
    }

    #[test]
    fn advance_terminated_returns_terminate_with_end_reason() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Terminated;
        run.end_reason = "用户拒绝确认".into();
        assert_eq!(
            advance_skill(&run, 1000),
            AdvanceAction::Terminate("用户拒绝确认".into())
        );
    }

    // ── DSL 调度器决策 ──

    #[test]
    fn advance_dsl_continue_returns_run() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Running;
        // Running 11s（未超 180s 上限）→ 正常执行
        assert!(matches!(advance_dsl(&run, 11000), DslAdvanceAction::Run));
    }

    #[test]
    fn advance_dsl_no_active_loaded_returns_run() {
        // 防御性分支：生产上 Loaded 只是 SkillRun::new 的瞬时初态（start_skill 直接置
        // Running，见 runtime.rs start_skill 注释），advance_dsl 永远收不到 Loaded。
        // 本测试锁定兜底语义——万一收到 Loaded 也不阻断 step 执行（按 Run 处理）。
        let run = test_run(8, 180);
        assert_eq!(run.state, SkillState::Loaded);
        assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::Run));
    }

    #[test]
    fn advance_dsl_paused_returns_await_user() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Paused;
        assert!(matches!(
            advance_dsl(&run, 1000),
            DslAdvanceAction::AwaitUser
        ));
    }

    #[test]
    fn advance_dsl_completed_returns_finish() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Completed;
        assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::Finish));
    }

    #[test]
    fn advance_dsl_failed_returns_fail_with_rollback() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Failed;
        run.end_reason = "超过最大步数上限（8 步）".into();
        match advance_dsl(&run, 1000) {
            DslAdvanceAction::FailWithRollback(reason) => {
                assert_eq!(reason, "超过最大步数上限（8 步）");
            }
            other => panic!("expected FailWithRollback, got {other:?}"),
        }
    }

    #[test]
    fn advance_dsl_terminated_returns_terminate_no_rollback() {
        let mut run = test_run(8, 180);
        run.state = SkillState::Terminated;
        run.end_reason = "用户 /stop".into();
        match advance_dsl(&run, 1000) {
            DslAdvanceAction::Terminate(reason) => assert_eq!(reason, "用户 /stop"),
            other => panic!("expected Terminate, got {other:?}"),
        }
    }

    #[test]
    fn advance_dsl_timeout_returns_fail_with_rollback() {
        // 端到端：Running + 启动后 61s（60s 上限）→ advance_skill 内部超时检测 → Fail
        // → advance_dsl 映射成 FailWithRollback（含 reason）
        let mut run = test_run(8, 60);
        run.state = SkillState::Running;
        let action = advance_dsl(&run, 1000 + 61 * 1000);
        match action {
            DslAdvanceAction::FailWithRollback(reason) => {
                assert!(reason.contains("超时"), "reason 应含「超时」: {reason}");
                assert!(reason.contains("60"), "reason 应含阈值 60s: {reason}");
            }
            other => panic!("expected FailWithRollback, got {other:?}"),
        }
    }

    // ── 前置预审 ──

    #[test]
    fn preflight_rejects_disabled() {
        let mut m = SkillMeta::default();
        m.name = "x".into();
        m.enabled = false;
        assert!(preflight(&m).is_err());
    }

    #[test]
    fn preflight_rejects_blacklist_intent() {
        let mut m = SkillMeta::default();
        m.name = "x".into();
        m.description = "批量删除所有任务".into();
        assert!(preflight(&m).unwrap_err().contains("黑名单"));

        m.description = "全盘遍历文件".into();
        assert!(preflight(&m).is_err());
    }

    #[test]
    fn preflight_passes_normal() {
        let mut m = SkillMeta::default();
        m.name = "ok".into();
        m.description = "汇总今日任务并归档".into();
        assert!(preflight(&m).is_ok());
    }

    // ── 步骤循环 / 熔断 ──

    #[test]
    fn step_check_counts_and_records_actions() {
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        assert!(step_check(&mut r, "list_tasks", "{}", 2000).is_ok());
        assert_eq!(r.step, 1);
        assert!(r.actions.is_empty()); // 只读不记
        assert!(step_check(&mut r, "create_task", "{\"title\":\"x\"}", 2000).is_ok());
        assert_eq!(r.actions.len(), 1); // 有副作用记入
        assert!(r.actions[0].starts_with("create_task"));
    }

    #[test]
    fn step_check_fuses_on_max_steps() {
        let mut r = test_run(2, 180);
        r.state = SkillState::Running;
        assert!(step_check(&mut r, "create_task", "{}", 2000).is_ok());
        assert!(step_check(&mut r, "edit_task", "{}", 2000).is_ok());
        let e = step_check(&mut r, "complete_task", "{}", 2000).unwrap_err();
        assert!(e.to_string().contains("超过最大步数上限"));
        assert_eq!(r.state, SkillState::Failed);
    }

    #[test]
    fn step_check_fuses_on_timeout() {
        let mut r = test_run(8, 60);
        r.state = SkillState::Running;
        // 已过 61 秒
        let e = step_check(&mut r, "list_tasks", "{}", 1000 + 61 * 1000).unwrap_err();
        assert!(e.to_string().contains("超时"));
        assert_eq!(r.state, SkillState::Failed);
    }

    #[test]
    fn step_check_rejects_when_paused() {
        let mut r = test_run(8, 180);
        r.state = SkillState::Paused;
        assert!(step_check(&mut r, "list_tasks", "{}", 2000).is_err());
        assert_eq!(r.step, 0); // 暂停态不计数
    }

    #[test]
    fn pause_state_resumable_flows() {
        // resumable=true：暂停后可恢复
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        r.resumable = true;
        pause_state(&mut r);
        assert_eq!(r.state, SkillState::Paused);
        assert!(!r.terminal_after_confirm);
        confirm_state(&mut r, true);
        assert_eq!(r.state, SkillState::Running); // 确认通过恢复
                                                  // 再次暂停后拒绝
        pause_state(&mut r);
        confirm_state(&mut r, false);
        assert_eq!(r.state, SkillState::Terminated);
        assert_eq!(r.end_reason, "用户拒绝确认");
    }

    #[test]
    fn pause_state_non_resumable_terminates_after_confirm() {
        // resumable=false：暂停即终止——确认通过后技能终止
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        r.resumable = false;
        pause_state(&mut r);
        assert_eq!(r.state, SkillState::Paused);
        assert!(r.terminal_after_confirm);
        confirm_state(&mut r, true);
        assert_eq!(r.state, SkillState::Terminated);
        assert!(r.end_reason.contains("resumable=false"));
    }

    #[test]
    fn rollback_section_extracted() {
        let body = "# 文档\n## 步骤\n1. 干活\n## 回滚\n1. 撤销 A\n2. 恢复 B\n## 其他\n尾巴";
        let rb = rollback_section(body);
        assert!(rb.contains("撤销 A"));
        assert!(rb.contains("恢复 B"));
        assert!(!rb.contains("尾巴")); // 到下一个 ## 为止

        assert_eq!(rollback_section("没有回滚章节"), "");
    }

    // ── 同名技能跨会话顶号拦截 ──

    #[test]
    fn skill_conflict_other_session_running_is_blocked() {
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        r.session_id = Some("s-a".into());
        assert!(skill_conflict_with_existing(Some(&r), Some("s-b")));
    }

    #[test]
    fn skill_conflict_other_session_paused_is_blocked() {
        let mut r = test_run(8, 180);
        r.state = SkillState::Paused;
        r.session_id = Some("s-a".into());
        assert!(skill_conflict_with_existing(Some(&r), Some("s-b")));
        // 一方无会话（后台任务）也算不同会话
        assert!(skill_conflict_with_existing(Some(&r), None));
    }

    #[test]
    fn skill_conflict_same_session_restart_allowed() {
        // 同会话同名重启：保留原有覆盖语义，不拦截
        let mut r = test_run(8, 180);
        r.state = SkillState::Running;
        r.session_id = Some("s-a".into());
        assert!(!skill_conflict_with_existing(Some(&r), Some("s-a")));
    }

    #[test]
    fn skill_conflict_terminal_state_allowed() {
        // 终态（Completed/Failed/Terminated）不拦截，可重启
        for state in [
            SkillState::Completed,
            SkillState::Failed,
            SkillState::Terminated,
        ] {
            let mut r = test_run(8, 180);
            r.state = state.clone();
            r.session_id = Some("s-a".into());
            assert!(
                !skill_conflict_with_existing(Some(&r), Some("s-b")),
                "终态 {state:?} 不应拦截"
            );
        }
    }

    #[test]
    fn skill_conflict_no_existing_allowed() {
        assert!(!skill_conflict_with_existing(None, Some("s-b")));
        assert!(!skill_conflict_with_existing(None, None));
    }
}
