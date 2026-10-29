use super::parse::SkillMeta;
use super::state::{active_skill_run_for, load_skill_meta, skill_runs, SkillRun, SkillState};
use crate::audit_event;
use crate::bot::registry::ToolResult;
use crate::db::now_ms;
use crate::error::CommandError;
use tauri::AppHandle;

/// interactive 模式技能的连续失败上限：累计到这次仍不换策略，才判技能失败。
///
/// 取 3 而不是 1：一次工具失败往往是模型的一次参数用错（目录当文件传、模块
/// 路径没找对），模型换个工具就能继续——判死等于把整个回合一起带走，用户只
/// 看到「技能中断」。也不取 2：同一轮里模型可能并行发几个调用，一次性把计数
/// 打满，留不出「失败 → 换策略」的那一轮。
///
/// auto 模式（DSL 调度器）不适用：它的步骤失败必须立刻终止该步并跑回滚。
pub(crate) const SKILL_INTERACTIVE_FAIL_LIMIT: usize = 3;

/// 意图/描述危险关键词黑名单（与系统提示词安全红线一致）：命中即拒绝启动；
/// create_skill 创建时也做同款预检（创建早于启动，把拒绝提前到落盘前）。
pub(crate) const INTENT_BLACKLIST: [&str; 5] = [
    "全盘遍历",
    "批量删除",
    "无确认删除",
    "遍历文件系统",
    "清空所有",
];

/// 前置预审（Harness 意图预审层）：禁用/黑名单拒绝；超长/非法字段已在 parse_meta 兜底。
/// 返回 Ok 表示放行启动。
fn preflight(meta: &SkillMeta) -> Result<(), String> {
    if !meta.enabled {
        return Err(format!("技能「{}」已被禁用", meta.name));
    }
    let hay = format!(
        "{} {}",
        meta.description.to_lowercase(),
        meta.intents.join(" ")
    );
    if let Some(hit) = INTENT_BLACKLIST
        .iter()
        .find(|b| hay.contains(&b.to_lowercase()))
    {
        return Err(format!(
            "技能「{}」意图命中安全黑名单（{hit}），已拒绝启动",
            meta.name
        ));
    }
    Ok(())
}

/// 技能寿命判定（单一口径）：已跑秒数超过绝对寿命上限。步骤熔断、状态机
/// 实时检测、Paused 占位清扫都走这里——口径分叉会让「同一次运行在不同入口
/// 过期时刻不同」。这不是性能预算，是 Paused 槽位的泄漏保护。
fn skill_timed_out(run: &SkillRun, now_ms: i64) -> bool {
    (now_ms - run.started_at_ms) / 1000 > crate::bot::constants::SKILL_MAX_LIFETIME_SECS as i64
}

/// 技能步数软警告的阈值口径：按总步数取比例，再钳到上下界。
///
/// 固定阈值对大预算太晚（步数多的时候剩几步已经来不及收尾），对小预算又太早
/// （预算只有 3 步的技能会在第一轮就收到「快用完了」，纯属噪音）。
/// 比例口径两头都对：默认 100 步落在钳制区间的 10 步。
fn skill_budget_warn_limits(max_steps: usize) -> usize {
    (max_steps / crate::bot::constants::SKILL_BUDGET_WARN_STEPS_DIVISOR).clamp(
        crate::bot::constants::SKILL_BUDGET_WARN_STEPS_CLAMP.0,
        crate::bot::constants::SKILL_BUDGET_WARN_STEPS_CLAMP.1,
    )
}

/// 技能预算软警告文案：预算快用完时提醒模型收尾（一个运行只提醒一次）。
///
/// 返回 None = 预算还充裕。
///
/// 为什么要有软警告：步数是硬闸（仅 auto 模式），被它砍掉的那一轮模型往往刚做完关键动作
/// （产物已落盘、正准备总结）——硬杀留给用户一个空气泡，软警告让模型有机会
/// 先把结论和产物路径说出来。
pub(crate) fn skill_budget_warning(run: &SkillRun, _now_ms: i64) -> Option<String> {
    if run.state != SkillState::Running {
        return None;
    }
    let steps_limit = skill_budget_warn_limits(run.max_steps);
    let steps_left = run.max_steps.saturating_sub(run.step);
    if steps_left > steps_limit {
        return None;
    }
    Some(format!(
        "【系统提示】技能「{name}」步数快用完（还剩 {steps_left} 步）：立刻停止展开新步骤——把已经做出来的东西整理成最终回复（产物文件的完整路径 + 结论），不要再调工具。",
        name = run.name,
        steps_left = steps_left,
    ))
}

/// 技能出口类型（主循环的三个收尾出口）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillExitKind {
    /// 技能跑完（Completed）
    Completed,
    /// 技能被闸门砍掉：步数（仅 auto）/工具失败（Failed）
    Failed,
    /// 技能被叫停：用户拒绝/强制停止（Terminated）
    Terminated,
}

/// 技能出口正文：模型说了就用它，一个字没说就给一句可读兜底。
///
/// 兜底场景：模型一路只发工具调用、一个字没说，技能却结束了。这种情况下正文
/// 是空串，用户看到的是空气泡——「什么也没干」和「干完了但没说」从界面上分不
/// 出来。兜底把「发生了什么 + 已经产出什么」讲清楚。
///
/// produced：本提问里工具写过的路径。只列磁盘上还存在的——已被清理掉的临时
/// 文件列出来反而误导用户去找一个不存在的文件。
pub(crate) fn skill_exit_body(
    streamed: &str,
    kind: SkillExitKind,
    name: &str,
    reason: &str,
    step: usize,
    produced: &[String],
) -> String {
    if !streamed.trim().is_empty() {
        return streamed.to_string();
    }
    let head = match kind {
        SkillExitKind::Completed => format!("✅ 技能「{name}」已完成（共 {step} 步）。"),
        SkillExitKind::Failed => format!("⏹ 技能「{name}」执行中断：{reason}（共 {step} 步）。"),
        SkillExitKind::Terminated => format!("⏹ 技能「{name}」已终止：{reason}（共 {step} 步）。"),
    };
    let files: Vec<&str> = produced
        .iter()
        .map(|p| p.as_str())
        .filter(|p| std::path::Path::new(p).exists())
        .collect();
    if files.is_empty() {
        return head;
    }
    let list = files
        .iter()
        .map(|p| format!("- {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{head}\n已产出（文件仍在原处）：\n{list}")
}

/// 超时清扫：把等到超时的非终态 run（Paused 等确认 / Running 跑在已死会话里）
/// 标成 Failed（终态），返回被扫掉的技能名。非终态不扫的话它一直占着同名
/// 技能——Paused 那个会话再也不回来确认、Running 那个会话模型循环带 Err
/// 退出（advance_skill 只由本会话循环调），都会永久挡路。
fn sweep_expired_pauses(
    runs: &mut std::collections::HashMap<String, SkillRun>,
    now_ms: i64,
) -> Vec<String> {
    let mut expired = Vec::new();
    for (name, run) in runs.iter_mut() {
        if run.state == SkillState::Paused && skill_timed_out(run, now_ms) {
            run.state = SkillState::Failed;
            run.end_reason = "等待确认超时，已过期".into();
            expired.push(name.clone());
        }
        if run.state == SkillState::Running && skill_timed_out(run, now_ms) {
            run.state = SkillState::Failed;
            run.end_reason = "运行超时（会话未收尾），已过期".into();
            expired.push(name.clone());
        }
    }
    expired
}

/// 同名技能冲突判定：SKILL_RUNS 以技能名为键，
/// 别的会话的 Running/Paused 同名 run 会被 insert 顶掉导致安全闸全失——启动前拒绝。
/// 过了超时的 Paused 不算活动：没人会再来确认它，占着就是永久挡路，启动方有权接管。
fn skill_conflict_with_existing(
    existing: Option<&SkillRun>,
    session_id: Option<&str>,
    now_ms: i64,
) -> bool {
    let Some(r) = existing else {
        return false;
    };
    if r.session_id.as_deref() == session_id {
        return false; // 同会话同名重启：覆盖语义
    }
    match r.state {
        // Running 超时同样不算活动：死会话（模型循环带 Err 退出后无人推进）
        // 留下的行不扫就是永久拒绝，直到重启进程
        SkillState::Running => !skill_timed_out(r, now_ms),
        SkillState::Paused => !skill_timed_out(r, now_ms),
        _ => false,
    }
}

/// 切技能语义纯函数（单测入口）：把同会话里其他活动技能从表里摘掉，
/// 返回被摘技能的审计载荷（技能名 + 已走步数）。
///
/// 摘掉而不是「标成终态留在表里」：留在表里就是一条同会话的终态 run，
/// 活动快照可能取到它 → 主循环把新技能当「已完成」直接收尾（技能一用就断）。
fn take_over_switched_runs(
    runs: &mut std::collections::HashMap<String, SkillRun>,
    new_name: &str,
    session_id: Option<&str>,
) -> Vec<(String, usize)> {
    let mut switched: Vec<(String, usize)> = Vec::new();
    let mut drop_names: Vec<String> = Vec::new();
    for (n, r) in runs.iter_mut() {
        if *n != new_name
            && (r.state == SkillState::Running || r.state == SkillState::Paused)
            && r.session_id.as_deref() == session_id
        {
            switched.push((n.clone(), r.step));
            drop_names.push(n.clone());
        }
    }
    for n in &drop_names {
        runs.remove(n);
    }
    switched
}

/// 启动 Skill：use_skill 工具调用即启动生命周期（预审 → 参数校验 → Running），
/// 返回文档 + 运行约束提示。
/// session_id：记录触发会话，活动判定/暂停/确认/推进按会话过滤（会话隔离）。
/// params：use_skill 传入的参数对象（缺失必填参数拒绝启动并教学化提示）。
pub fn start_skill(
    app: &AppHandle,
    name: &str,
    params: serde_json::Value,
    session_id: Option<&str>,
) -> Result<(SkillMeta, String), String> {
    let (meta, body, dir) = load_skill_meta(app, name)?;
    // 会话档技能只归创建它的会话用：清单层已把它挡在全局广告面外，
    // 启动层还得拦点名调用——否则别的会话直接 use_skill 照样能跑。
    // 归属缺席（后台链路创建）的会话档技能仅在无会话上下文的调用可用。
    if meta.session_scoped && meta.session_owner.as_deref() != session_id {
        return Err(format!(
            "技能「{name}」是会话档技能（scope: session），仅创建它的会话可用；如需在当前会话使用，请让用户在设置页把它的 scope 改为全局"
        ));
    }
    // `rollback: auto` 但正文没有 `## Rollback` 段 → 降为 `none`。
    //
    // SKILL-DSL 承诺过这个降级：声明了 auto 却没有回滚段可跑 = 没有回滚。
    // 不降级的话，「回滚 auto」会被读成「会回滚」，而实际上一段都没有。
    let meta = crate::bot_skills::parse::normalize_rollback_meta(meta, &body);
    preflight(&meta)?;
    // 参数契约校验——必填缺失拒绝启动（教模型带参重调）；非必填缺省回填 default
    let mut params = params;
    if !params.is_object() {
        params = serde_json::Value::Null;
    }
    if let Some(obj) = params.as_object() {
        // 空串参数视为未提供（模型常传空占位）——先收集再删，避免可变/不可变借用冲突
        let empty_keys: Vec<String> = obj
            .iter()
            .filter(|(_, v)| v.as_str() == Some(""))
            .map(|(k, _)| k.clone())
            .collect();
        for k in empty_keys {
            params.as_object_mut().unwrap().remove(&k);
        }
    }
    let mut missing: Vec<String> = Vec::new();
    for p in &meta.params {
        let provided = params
            .get(&p.key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(v) = provided {
            params[&p.key] = serde_json::Value::String(v);
        } else if let Some(d) = &p.default {
            params[&p.key] = serde_json::Value::String(d.clone());
        } else if p.required {
            missing.push(format!("{}：{}", p.key, p.desc));
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "技能「{}」缺少必填参数：{}。请带 params 重新调用 use_skill（params 为对象，键为参数名）",
            meta.name,
            missing.join("；")
        ));
    }
    // 预审已通过 → 直接进入 Running：步骤钩子按 Running/Paused 查找，
    // 停在 Loaded 会让计数/熔断/动作记录全部静默失效
    let mut run = SkillRun::new(&meta);
    run.state = SkillState::Running;
    run.session_id = session_id.map(|s| s.to_string());
    run.params = params;
    run.dir = Some(dir);
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::runtime::skill_runs");
    let now = now_ms();
    // 超时清扫：等确认等到超时的 Paused run 标成 Failed（终态）——
    // 它占着同名技能，不清就一直挡着别的会话。
    let expired = sweep_expired_pauses(&mut runs, now);
    // 同名 run 属于别的会话且仍在活动 → 拒绝启动，
    // 否则 insert 会把对方的步数/超时熔断、动作记录、收尾整个顶掉。
    // 同会话同名重启（覆盖语义）与终态 run 不拦截。
    if skill_conflict_with_existing(runs.get(&meta.name), session_id, now) {
        return Err(format!(
            "技能「{}」正在另一个会话/任务中运行，请等它结束后再启动",
            meta.name
        ));
    }
    // 单活动技能策略：新技能启动时，把本会话其他 Running/Paused 技能摘掉。
    // 步骤钩子/暂停/确认按「唯一活动技能」定位，多技能同时 Running 会导致
    // 步骤计数与暂停确认全部记到第一个技能、后续技能被静默忽略。
    // 会话隔离：只摘同会话的技能，别的会话的活动技能不受影响。
    // 锁内只做状态迁移 + 收集审计载荷；audit_log_hook（写盘 IO）移到锁外发，
    // 持锁跨外部调用 = 慢 IO 串行化全部写者 + 钩子重入 registry 即死锁。
    let switched = take_over_switched_runs(&mut runs, &meta.name, session_id);
    runs.insert(meta.name.clone(), run);
    drop(runs);
    for name in &expired {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_expired | name: {name} | 等待确认超时，已过期并释放占位"),
        );
    }
    for (n, step) in &switched {
        crate::bot::audit_log_hook(
            app,
            &format!("skill_completed | name: {n} | 切换技能结束 | steps: {step}"),
        );
    }
    // 审计：skill.start 结构化
    audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "skill.start",
        "name" => meta.name.clone(),
        "risk" => meta.risk_level.clone(),
        "mode" => meta.mode.clone(),
        "max_steps" => meta.max_steps,
        "rollback" => meta.rollback.clone(),
    );
    Ok((meta, body))
}

/// 工具 use_skill：读取技能文档全文返回给模型。
/// session_id：透传给 start_skill 记录技能归属会话（会话隔离）。
/// 新增 params 对象透传（参数契约）；返回头部兼容警告（未知工具）+
/// 尾部第三层资料清单（references/*.md）。
pub fn tool_use_skill(app: &AppHandle, args: &str, session_id: Option<&str>) -> ToolResult {
    let v: serde_json::Value = serde_json::from_str(args).unwrap_or(serde_json::Value::Null);
    let Some(name) = v["name"].as_str().map(|s| s.trim().to_string()) else {
        return ToolResult::denied("use_skill 缺少 name");
    };
    let params = v.get("params").cloned().unwrap_or(serde_json::Value::Null);
    match start_skill(app, &name, params, session_id) {
        Ok((meta, body)) => {
            let hint = format!(
                "【技能文档：{}】{}\n风险等级 {} · 运行模式 {} · 最多 {} 步 · 回滚 {}",
                meta.name,
                meta.version
                    .as_deref()
                    .map(|v| format!("v{v} "))
                    .unwrap_or_default(),
                meta.risk_level,
                meta.mode,
                meta.max_steps,
                meta.rollback
            );
            let mut out = hint + "\n\n" + &body;
            // 兼容审计——DSL 步骤引用了未内置的工具 → 头部警告（MCP 工具可忽略）
            if meta.mode == "auto" {
                if let Ok((steps, rollback)) = crate::bot_skills::parse_skill_steps(&body) {
                    let known: std::collections::HashSet<String> =
                        crate::bot::registry::tools_index()
                            .keys()
                            .map(|s| s.to_string())
                            .collect();
                    let unknown =
                        crate::bot_skills::parse::unknown_tool_names(&steps, &rollback, &known);
                    if !unknown.is_empty() {
                        out = format!(
                            "⚠️ 兼容提醒：本技能引用了未内置的工具（{}）——若为 MCP 工具可正常调用，否则对应步骤会失败。\n\n{}",
                            unknown.join("、"),
                            out
                        );
                        crate::bot::audit_log_hook(
                            app,
                            &format!(
                                "skill.compat_warn | name: {} | unknown: {}",
                                meta.name,
                                unknown.join(",")
                            ),
                        );
                    }
                }
            }
            if meta.mode == "interactive" {
                out.push_str(
                    "\n\n（运行约束：本技能为人机协同模式，中高危动作执行前会暂停等待用户确认）",
                );
            }
            // 第三层渐进披露——技能目录附带资料按需读（会话级读白名单已放行）
            if let Some(run) = active_skill_run_for(app, session_id) {
                if run.name == meta.name {
                    if let Some(dir) = &run.dir {
                        let refs_dir = dir.join("references");
                        if let Ok(rd) = std::fs::read_dir(&refs_dir) {
                            let mut files: Vec<String> = rd
                                .flatten()
                                .filter(|e| {
                                    e.path()
                                        .extension()
                                        .and_then(|x| x.to_str())
                                        .map(|x| x == "md")
                                        .unwrap_or(false)
                                })
                                .map(|e| e.path().display().to_string())
                                .collect();
                            if !files.is_empty() {
                                files.sort();
                                out.push_str(
                                    "\n\n（本技能附带资料，需要时用 read_text_file 读取：\n- ",
                                );
                                out.push_str(&files.join("\n- "));
                                out.push('）');
                            }
                        }
                    }
                }
            }
            ToolResult::ok(out, Vec::new())
        }
        Err(e) => ToolResult::denied(e),
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
    // 寿命熔断（Paused 占位保护；步骤/连续失败两道闸才是主闸）
    if skill_timed_out(run, now) {
        run.state = SkillState::Failed;
        run.end_reason = format!(
            "超过寿命上限（{} 秒）",
            crate::bot::constants::SKILL_MAX_LIFETIME_SECS
        );
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: format!("技能「{}」{}，已强制终止", run.name, run.end_reason),
        });
    }
    // 动作记录（回滚清单来源）：只记有副作用的工具，跳过只读查询
    // （：list_tasks/search_tasks 合并为 query_tasks）
    const READONLY: [&str; 3] = ["query_tasks", "use_skill", "web_search"];
    if !READONLY.contains(&tool) {
        let brief = crate::bot::log_sanitize::truncate_chars(args, 120, true).replace('\n', " ");
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
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::runtime::skill_runs");
    // 会话隔离：只管归属当前会话的活动 Skill，别的会话的不计数不熔断
    let Some(run) = runs.values_mut().find(|r| {
        (r.state == SkillState::Running || r.state == SkillState::Paused)
            && r.session_id.as_deref() == session_id
    }) else {
        return Ok(()); // 无活动 Skill（模型自由调用工具），不干预
    };
    let res = step_check(run, tool, args, now_ms());
    // 锁内只做状态迁移 + 拼审计载荷；audit_log_hook（写盘 IO）移到锁外发，
    // 持锁跨外部调用 = 慢 IO 串行化全部写者 + 钩子重入 registry 即死锁
    let fail_line = res.as_ref().err().map(|e| {
        format!(
            "skill_failed | name: {} | {} | {e}",
            run.name, run.end_reason
        )
    });
    drop(runs);
    if let Some(line) = fail_line {
        crate::bot::audit_log_hook(app, &line);
    }
    res
}

/// Post-execute Skill 步骤钩子（洋葱管线「出」钩子）
/// 与 `skill_on_step` 对称：执行工具后调用，记录步骤结果 + 检测失败。
/// 仅在 Skill 处于 Running 时干预；Paused/Completed 等状态不写动作记录。
///
/// 失败判定走 `ToolStatus::is_success()`（status 是成败唯一真相源）。
/// 此前走 `audit::tool_call_failed(result)` 文本匹配，于是规则 deny / 用户拒绝 /
/// 预算触顶 / 白名单外这 4 类「工具根本没执行」的文案被判成**成功**——SkillRun
/// 停在 Running，技能对外报「✅ 完成」，而审计那边按 status 记的是 WARN。
/// `result` 现在只用于失败时的日志预览与模型文案，不参与成败判定。
/// 失败 → 把 Skill 标 Failed，写入 end_reason 让 `skill_finish` 知道不再继续后续步骤。
pub fn skill_on_step_post<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool: &str,
    result: &str,
    dur_ms: u64,
    status: crate::bot::registry::ToolStatus,
    session_id: Option<&str>,
) {
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::runtime::skill_runs");
    // 会话隔离：只记录归属当前会话的 Running 技能
    let Some(run) = runs
        .values_mut()
        .find(|r| r.state == SkillState::Running && r.session_id.as_deref() == session_id)
    else {
        return;
    };
    let name = run.name.clone();
    let is_fail = !status.is_success();
    // 锁内只做状态迁移 + 拼审计载荷；audit_log_hook（写盘 IO）移到锁外发，
    // 理由同 start_skill：持锁跨外部调用会串行化全部写者且钩子重入即死锁
    let log_line = if is_fail {
        let preview: String = result.chars().take(120).collect();
        // preview 是工具结果原文，换行/管道符会撕裂日志行，必须转义
        let preview = crate::bot::truncate_for_log(&preview, 120);
        // 失败语义按运行模式分岔（见 SkillRun.mode 的说明）：
        // - auto（DSL 调度器）：步骤失败必须立刻终止该步并跑回滚——调度器正是
        //   靠「run 变 Failed」感知失败的（advance_dsl → advance_skill），这里改
        //   判死时机就等于让回滚静默失效。
        // - interactive（LLM 驱动）：模型自己会换策略，一次工具失败不该把技能
        //   和整个回合一起带走。连续失败累计到阈值才判死——给模型留出换策略的
        //   机会（实测：模型一次参数用错就把技能判死，用户只看到「中断」）。
        if run.mode == "auto" {
            run.state = SkillState::Failed;
            run.end_reason = format!("工具 {tool} 执行失败");
            format!("skill_step_fail | name: {name} | tool: {tool} | {preview}")
        } else {
            run.consec_failures += 1;
            if should_fail_skill_on_step_error(&run.mode, run.consec_failures) {
                run.state = SkillState::Failed;
                run.end_reason = format!("工具 {tool} 连续失败 {} 次", run.consec_failures);
                format!(
                    "skill_step_fail | name: {name} | tool: {tool} | 连续失败 {} 次，判死 | {preview}",
                    run.consec_failures
                )
            } else {
                format!(
                    "skill_step_fail | name: {name} | tool: {tool} | 第 {} 次失败，暂不判死 | {preview}",
                    run.consec_failures
                )
            }
        }
    } else {
        // 成功一次即清零：模型换了策略就重新开始算
        run.consec_failures = 0;
        format!("skill_step_ok | name: {name} | tool: {tool} | {dur_ms}ms")
    };
    drop(runs);
    crate::bot::audit_log_hook(app, &log_line);
}

/// 失败判死决策（纯函数，单测入口）。
///
/// auto（DSL 调度器）：一步失败即判死——调度器靠「run 变 Failed」感知步骤失败
/// 并触发回滚，这里放宽就等于回滚静默失效。
/// interactive（LLM 驱动）：连续失败累计到阈值才判死，给模型换策略的机会。
fn should_fail_skill_on_step_error(mode: &str, consec_failures: usize) -> bool {
    mode == "auto" || consec_failures >= SKILL_INTERACTIVE_FAIL_LIMIT
}

/// 暂停语义纯函数（单测入口）：把 Running 技能转入暂停等待。
/// resumable=false → terminal_after_confirm=true（确认通过后技能终止，暂停即终止）。
fn pause_state(run: &mut SkillRun) {
    run.state = SkillState::Paused;
    run.terminal_after_confirm = !run.resumable;
}

/// 确认结果纯函数（单测入口）：通过 → 恢复 Running 或暂停即终止；拒绝 → Terminated。
/// 过了超时的暂停不再恢复：技能已过期（没人会再来确认它），
/// 恢复等于让一个过期计划继续动手——标成 Failed（终态）并释放占位。
fn confirm_state(run: &mut SkillRun, approved: bool, now_ms: i64) {
    if skill_timed_out(run, now_ms) {
        run.state = SkillState::Failed;
        run.end_reason = "等待确认超过寿命上限，已过期".into();
        return;
    }
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
/// confirm_id 记进 run：确认结果只作用于由这同一个确认窗暂停的技能。
pub fn skill_mark_paused<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool: &str,
    session_id: Option<&str>,
    confirm_id: &str,
) {
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::runtime::skill_runs");
    let mut log_line: Option<String> = None;
    if let Some(run) = runs
        .values_mut()
        .find(|r| r.state == SkillState::Running && r.session_id.as_deref() == session_id)
    {
        pause_state(run);
        run.pending_confirm_id = Some(confirm_id.to_string());
        log_line = Some(format!(
            "skill_paused | name: {} | at: {tool} | resumable: {}",
            run.name, run.resumable
        ));
    }
    // audit_log_hook（写盘 IO）移到锁外发：持锁跨外部调用会串行化全部写者
    // 且钩子重入 registry 即死锁，理由同 start_skill
    drop(runs);
    if let Some(line) = log_line {
        crate::bot::audit_log_hook(app, &line);
    }
}

/// 确认结果到达：恢复 Running / 暂停即终止 / 拒绝终止（bot_confirm_response 调用）。
/// 会话隔离：只作用于归属该会话的 Paused 技能（session 从 ConfirmMap 条目取回）。
/// 确认窗绑定：只作用于 pending_confirm_id == 本确认窗 id 的那条暂停——别的确认窗
/// （哪怕同一会话）不能顺手恢复或终止这条暂停。
pub fn skill_confirm_result<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    approved: bool,
    session_id: Option<&str>,
    confirm_id: &str,
) {
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::runtime::skill_runs");
    let mut log_line: Option<String> = None;
    if let Some(run) = runs.values_mut().find(|r| {
        r.state == SkillState::Paused
            && r.session_id.as_deref() == session_id
            && r.pending_confirm_id.as_deref() == Some(confirm_id)
    }) {
        confirm_state(run, approved, now_ms());
        run.pending_confirm_id = None;
        let name = run.name.clone();
        let state = format!("{:?}", run.state);
        let reason = run.end_reason.clone();
        log_line = Some(format!(
            "skill_confirm | name: {name} | approved: {approved} | -> {state} {reason}"
        ));
    }
    // audit_log_hook（写盘 IO）移到锁外发：持锁跨外部调用会串行化全部写者
    // 且钩子重入 registry 即死锁，理由同 start_skill
    drop(runs);
    if let Some(line) = log_line {
        crate::bot::audit_log_hook(app, &line);
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
/// 技能失败教训直沉淀（无 LLM）。仿 auto_lesson_on_task_failure：
/// record_lesson_core（kind=lesson、source=system、scenario=skill:{name}）——
/// lesson 自动进下轮 consolidate 候选池与聊天 lesson 槽。
/// DB 打开失败静默（教训丢失可接受，不打断收尾）。pub(crate)：调度器失败分支共用。
pub(crate) fn sink_skill_failure_lesson<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    name: &str,
    reason: &str,
    actions: &[String],
) {
    let Ok(conn) = crate::db::open_db(app) else {
        return;
    };
    let lesson = format!(
        "技能「{name}」执行失败：{}。已执行动作：{}。建议：检查失败步骤的工具参数与依赖，必要时修正技能文档或改由人工处理。",
        crate::bot::truncate_for_log(reason, 160),
        if actions.is_empty() {
            "无".to_string()
        } else {
            actions.join(" → ")
        }
    );
    let msg = match crate::memory::record_lesson_core(
        &conn,
        &lesson,
        &format!("skill:{name}"),
        "system",
        None,
        now_ms(),
        &crate::memory::store::StoreParams::of(&crate::bot::read_memory_tuning(app)),
    ) {
        Ok(m) => m,
        Err(e) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "skill.lesson_sink_failed | name: {name} | {}",
                    crate::bot::truncate_for_log(&e, 120)
                ),
            );
            return;
        }
    };
    crate::bot::audit_log_hook(
        app,
        &format!(
            "skill.lesson_sunk | name: {name} | {}",
            crate::bot::truncate_for_log(&msg, 120)
        ),
    );
}

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
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::runtime::skill_runs");
    // 锁内只做状态迁移 + 收集审计/回滚载荷；load_skill_meta（逐技能读 SKILL.md
    // 磁盘 IO）与 audit_log_hook（写盘）移到锁外——持锁跨 IO 会把全部并发
    // start/step/confirm 串行到磁盘速度，钩子重入 registry 即死锁。
    enum FinishPayload {
        Completed(String, usize),
        Failed(String, Vec<String>, String),
    }
    let mut transitioned = false;
    let mut payloads: Vec<FinishPayload> = Vec::new();
    for (name, run) in runs.iter_mut() {
        if run.state != SkillState::Running && run.state != SkillState::Paused {
            continue;
        }
        if run.session_id.as_deref() != session_id {
            continue;
        }
        transitioned = true;
        if ok {
            run.state = SkillState::Completed;
            payloads.push(FinishPayload::Completed(name.clone(), run.step));
        } else {
            run.state = SkillState::Failed;
            run.end_reason = reason.to_string();
            payloads.push(FinishPayload::Failed(
                name.clone(),
                run.actions.clone(),
                run.rollback.clone(),
            ));
        }
    }
    drop(runs);
    for p in payloads {
        match p {
            FinishPayload::Completed(name, step) => {
                crate::bot::audit_log_hook(
                    app,
                    &format!("skill_completed | name: {name} | steps: {step}"),
                );
            }
            FinishPayload::Failed(name, actions, rollback) => {
                // reason 可含模型给的工具名/错误文本，转义后再落日志
                let mut log = format!(
                    "skill_failed | name: {name} | {} | actions: {}",
                    crate::bot::truncate_for_log(reason, 200),
                    actions.len()
                );
                // 回滚建议（务实版）：失败 + 声明可回滚 + 有已执行动作 → 生成建议文本
                if rollback == "auto" && !actions.is_empty() {
                    log.push_str(" | rollback_suggested");
                    let actions_text = actions
                        .iter()
                        .map(|a| format!("- {a}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let rb_section = rollback_section(
                        &load_skill_meta(app, &name)
                            .map(|(_, b, _)| b)
                            .unwrap_or_default(),
                    );
                    rollback_hint = format!(
                        "\n\n【技能回滚建议】技能「{name}」执行中断（{reason}），已执行 {n} 个动作：\n{actions_text}\n{}",
                        if rb_section.is_empty() {
                            "该技能未提供回滚章节，请向用户说明已执行动作，由用户决定手动补救。".to_string()
                        } else {
                            format!("技能文档回滚章节：\n{rb_section}\n请先询问用户是否需要回滚；用户同意后，按回滚章节逐条执行逆操作（每一步同样经过安全校验）。")
                        },
                        n = actions.len()
                    );
                }
                crate::bot::audit_log_hook(app, &log);
                // 失败教训直沉淀（无 LLM，仿 auto_lesson_on_task_failure）——
                // lesson 进 mem_items 后自动进下轮 consolidate 候选池与聊天 lesson 槽。
                // 有实际动作/步数才沉淀（防空转噪音）。
                if !actions.is_empty() {
                    sink_skill_failure_lesson(app, &name, reason, &actions);
                }
            }
        }
    }
    // 零迁移留痕的闸门 = reason 非空:传 reason 的调用方(scheduler "done" 收尾)
    // 期待状态机真的迁移,零迁移 = 僵尸 Running 不可见,记审计;空 reason 是清理性
    // 调用(bot_model_loop :1006 每轮聊天收尾 / :594 Finish 后兜底),零迁移是常态不记。
    // false 路径不记:无活动技能时的错误收尾是合法 no-op,记了会刷正常路径日志。
    if ok && !transitioned && !reason.is_empty() {
        crate::bot::audit_log_hook(
            app,
            &format!(
                "skill_finish_no_transition | session: {} | reason: {}",
                session_id.unwrap_or("-"),
                crate::bot::truncate_for_log(reason, 120)
            ),
        );
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
            // 实时寿命检测（即使 step_check 没跑到，主循环也能感知）
            if skill_timed_out(run, now_ms) {
                AdvanceAction::Fail(format!(
                    "超过寿命上限（{} 秒）",
                    crate::bot::constants::SKILL_MAX_LIFETIME_SECS
                ))
            } else {
                AdvanceAction::Continue
            }
        }
        SkillState::Paused => {
            // 暂停等确认同样有寿命上限：Paused 不是终态，不判过期它就一直占着
            // 同名技能的槽，那个会话不再回来就永久挡着别的会话。
            if skill_timed_out(run, now_ms) {
                AdvanceAction::Fail(format!(
                    "超过寿命上限（{} 秒）",
                    crate::bot::constants::SKILL_MAX_LIFETIME_SECS
                ))
            } else {
                AdvanceAction::AwaitConfirm
            }
        }
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
    // 暂停态：DSL 层只表达「等用户」——暂停超时的裁决在状态机层
    // （advance_skill 的主循环判定）与 start_skill 的清扫，两处口径一致。
    // 这里不重复判超时：同一份判定出现两个答案，DSL 与主循环就会打架。
    if run.state == SkillState::Paused {
        return DslAdvanceAction::AwaitUser;
    }
    match advance_skill(run, now_ms) {
        AdvanceAction::NoActive | AdvanceAction::Continue => DslAdvanceAction::Run,
        AdvanceAction::Finish => DslAdvanceAction::Finish,
        AdvanceAction::AwaitConfirm => DslAdvanceAction::AwaitUser,
        AdvanceAction::Fail(reason) => DslAdvanceAction::FailWithRollback(reason),
        AdvanceAction::Terminate(reason) => DslAdvanceAction::Terminate(reason),
    }
}

#[cfg(test)]
mod tests;
