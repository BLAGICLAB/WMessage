use super::manage::skill_search_paths;
use super::parse::{parse_meta, SkillMeta, SKILL_NAME_CHARS_OK};
use crate::error::CommandError;

const MAX_SKILL_BODY: usize = 50 * 1024;

// ───────────────────────── Skill 调度器（运行模型 v1.0：状态机 + 步骤循环 + 熔断 + 回滚记录） ─────────────────────────

/// Skill 运行状态（生命周期状态机，与 docs/SKILL-RUNTIME.md 对齐）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillState {
    /// 已加载：SKILL.md 解析成功，尚未预审
    Loaded,
    /// 前置预审通过，进入执行
    Running,
    /// 暂停等待人工确认（模式 B 中高危节点）
    Paused,
    /// 成功完成
    Completed,
    /// 失败异常（步骤失败/超时/熔断）
    Failed,
    /// 已终止（预审拦截/用户取消/强制停止）
    Terminated,
}

/// Skill 运行实例（执行实例级，内存态，不持久化）
#[derive(Clone)]
pub struct SkillRun {
    pub name: String,
    pub state: SkillState,
    /// 已执行步骤数（每次非 use_skill 的工具调用计一步）
    pub step: usize,
    pub max_steps: usize,
    pub started_at_ms: i64,
    pub timeout_secs: u64,
    pub rollback: String,
    /// 已执行动作记录（工具名+参数摘要；回滚清单来源）
    pub actions: Vec<String>,
    /// 终止原因（Terminated/Failed 时填写）
    pub end_reason: String,
    /// 是否支持暂停后续跑（元数据 resumable 镜像）
    pub resumable: bool,
    /// resumable=false 时的暂停即终止：确认通过后执行当前动作，随后技能终止
    pub terminal_after_confirm: bool,
    /// 归属会话：触发该技能的聊天会话 id；
    /// 后台定时任务为 None。active_skill_run / 暂停 / 确认按此过滤，
    /// 防会话 A 暂停中的 Skill 被会话 B 的主循环推进或确认。
    pub session_id: Option<String>,
}

impl SkillRun {
    pub(crate) fn new(meta: &SkillMeta) -> Self {
        Self {
            name: meta.name.clone(),
            state: SkillState::Loaded,
            step: 0,
            max_steps: meta.max_steps,
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            timeout_secs: meta.timeout_secs,
            rollback: meta.rollback.clone(),
            actions: Vec::new(),
            end_reason: String::new(),
            resumable: meta.resumable,
            terminal_after_confirm: false,
            session_id: None,
        }
    }
}

/// 活动 Skill 运行表：name → SkillRun（同一技能同轮只允许一个实例）
static SKILL_RUNS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, SkillRun>>,
> = std::sync::OnceLock::new();

pub(crate) fn skill_runs() -> &'static std::sync::Mutex<std::collections::HashMap<String, SkillRun>>
{
    SKILL_RUNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 当前会话是否有 Skill 处于 Running 状态（按会话过滤，别的会话的 Skill 不算本会话活动）
///
/// - Running：LLM 正在 Skill 内部循环中，原子工具是 Skill 的子步骤 → 放行
/// - Loaded / Finished / Terminated / Failed / Paused：原子工具应被阻断
///
/// 供 `tool_guard::is_skill_active` 调用（穿透 private Mutex 访问）
pub fn is_skill_active_for(session_id: Option<&str>) -> bool {
    let Ok(guard) = skill_runs().lock() else {
        return false;
    };
    guard
        .values()
        .any(|r| matches!(r.state, SkillState::Running) && r.session_id.as_deref() == session_id)
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 回滚窗口临时重开：回滚段执行时 run 已是 Failed，
/// 而 AtomicGuard 只认 Running —— 回滚段里的原子工具（link_file_to_task 等）会被自家门禁拦截。
/// 把本会话该技能的 Failed run 临时置回 Running（回滚期间门禁放行），返回是否实际切换。
/// 调用方在回滚段结束后必须调 `restore_failed_run_after_rollback` 复原。
pub(crate) fn reopen_failed_run_for_rollback(name: &str, session_id: Option<&str>) -> bool {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    let Some(run) = runs.get_mut(name) else {
        return false;
    };
    if run.state == SkillState::Failed && run.session_id.as_deref() == session_id {
        run.state = SkillState::Running;
        return true;
    }
    false
}

/// 回滚窗口复原：回滚段结束后把临时重开的 run 置回 Failed（终态）。
/// run 已被其它路径改动（如回滚步骤再失败被标 Failed）时是安全 no-op。
pub(crate) fn restore_failed_run_after_rollback(name: &str, session_id: Option<&str>) {
    let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = runs.get_mut(name) {
        if run.state == SkillState::Running && run.session_id.as_deref() == session_id {
            run.state = SkillState::Failed;
        }
    }
}

/// 主循环接入用：克隆当前会话活动 Skill 快照（任意非 Loaded 状态）。
/// 供 `bot_chat` 主循环与 DSL 调度器调，拿快照去 `advance_skill` / `advance_dsl` 决策，不再持锁。
/// 按 session_id 过滤——别的会话暂停/运行中的 Skill 不可见、不推进。
///
/// 状态过滤说明：Loaded 是 start_skill 中的过渡态——技能预审通过后立即转为 Running，
/// Loaded 仅在断言/异常路径短暂存在，advance_skill 返回 NoActive 与主循环预期一致。
///
/// ⚠️ 终态（Completed/Failed/Terminated）也会被返回——这是 DSL 调度器感知
/// 「用户 /stop / 工具失败」所必需的；调用方若是新一轮执行的入口（run_model_loop /
/// run_skill_scheduler），必须先 `clear_terminal_skill_runs()` 清掉上轮遗留的僵尸终态，
/// 否则会在第 0 步被 advance 短路（agent 假死事故根因）。
pub fn active_skill_run_for(session_id: Option<&str>) -> Option<SkillRun> {
    let guard = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .values()
        .find(|r| !matches!(r.state, SkillState::Loaded) && r.session_id.as_deref() == session_id)
        .cloned()
}

/// 清理终态 SkillRun（Completed/Failed/Terminated）。
/// SKILL_RUNS 只进不出：上轮遗留的终态 run 会被 active_skill_run 当"活动"，
/// 在新一轮执行的第 0 步被 advance 短路——静默返回空文本、不发 LLM 请求（agent 假死）。
/// 在每轮执行入口（run_model_loop / run_skill_scheduler）调用；
/// 本轮执行中新进入终态的 run 不受影响（清理发生在入口，轮内状态机照常可见）。
pub fn clear_terminal_skill_runs() {
    let mut guard = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
    guard.retain(|_, r| {
        !matches!(
            r.state,
            SkillState::Completed | SkillState::Failed | SkillState::Terminated
        )
    });
}

/// 读取技能正文 + 完整元数据（多目录 fallback）
/// 遍历 `skill_search_paths(app)`：数据目录找不到 → dev 模式 fallback target/debug/skills。
/// 数据目录优先（用户已修改的 Skill 优先于 dev mock 版本）。
pub(crate) fn load_skill_meta<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    name: &str,
) -> Result<(SkillMeta, String), CommandError> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: "技能名无效".to_string(),
        });
    }
    for dir in &skill_search_paths(app) {
        let path = dir.join(name).join("SKILL.md");
        if !path.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|_| format!("技能「{name}」存在但 SKILL.md 读取失败"))?;
        let meta = parse_meta(&text, name);
        let body: String = text.chars().take(MAX_SKILL_BODY).collect();
        return Ok((meta, body));
    }
    Err(CommandError::DomainRule {
        domain: "skill".to_string(),
        reason: format!("技能「{name}」不存在（检查设置页技能列表）"),
    })
}

// ───────────────────────── 集成测试钩子 ─────────────────────────
// tests/ 集成测试是独立 crate，够不到下面 #[cfg(test)] 的 test_insert_skill_run；
// 调度器 e2e（tests/skill_e2e.rs）需要插入 Running/Paused/Failed 的 run 来驱动
// advance_dsl 分支与回滚窗口的真路径。仅测试使用，生产路径不调。

#[doc(hidden)]
pub fn test_hook_insert_skill_run(run: SkillRun) {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(run.name.clone(), run);
}

#[doc(hidden)]
pub fn test_hook_remove_skill_run(name: &str) {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(name);
}

#[doc(hidden)]
pub fn test_hook_skill_run_state(name: &str) -> Option<SkillState> {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .map(|r| r.state.clone())
}

/// 测试辅助：直接插入一个指定状态的 skill run（绕过磁盘 SKILL.md 加载）。
/// 与 tests::test_run 同一份字段构造，供跨模块测试（lib.rs 退出清理）使用。
#[cfg(test)]
pub(crate) fn test_insert_skill_run(name: &str, state: SkillState) {
    let run = SkillRun {
        name: name.into(),
        state,
        step: 0,
        max_steps: 8,
        started_at_ms: 0,
        timeout_secs: 180,
        rollback: "auto".into(),
        actions: Vec::new(),
        end_reason: String::new(),
        resumable: true,
        terminal_after_confirm: false,
        session_id: None,
    };
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(name.into(), run);
}

/// 测试辅助收尾：移除插入的 run，不给其他测试留状态
#[cfg(test)]
pub(crate) fn test_remove_skill_run(name: &str) {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(name);
}

/// 测试辅助：读 run 当前状态（断言终止语义用）
#[cfg(test)]
pub(crate) fn test_skill_run_state(name: &str) -> Option<SkillState> {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .map(|r| r.state.clone())
}

/// SKILL_RUNS 测试串行锁。
/// clear_terminal_skill_runs / skill_terminate_all(None) 是无差别全局操作，
/// 并行测试互相删/改对方的 run 会随机挂（state 清理测试 × lib.rs 退出清理测试
/// 实锤交错路径）。凡写全局 SKILL_RUNS 且对内容有断言的测试必须全程持此锁。
#[cfg(test)]
pub(crate) static SKILL_RUNS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 测试辅助：构造一个指定 max_steps / timeout 的 SkillRun（跨子模块测试共用）
#[cfg(test)]
pub(crate) fn test_run(max_steps: usize, timeout_secs: u64) -> SkillRun {
    SkillRun {
        name: "test-skill".into(),
        state: SkillState::Loaded,
        step: 0,
        max_steps,
        started_at_ms: 1000,
        timeout_secs,
        rollback: "auto".into(),
        actions: Vec::new(),
        end_reason: String::new(),
        resumable: true,
        terminal_after_confirm: false,
        session_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 僵尸终态清理：入口清理后终态 run 不再被当"活动"，
    /// Running/Paused 不受影响（测试用 Paused：is_skill_active 只认 Running，避免与并行测试竞争）
    #[test]
    fn clear_terminal_removes_only_terminal_states() {
        let _serial = SKILL_RUNS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let zombie = "test-zombie-clear";
        // 用一个 Failed 残留 + 一个 Paused 活跃
        let mut failed = test_run(8, 180);
        failed.name = zombie.into();
        failed.state = SkillState::Failed;
        let mut paused = test_run(8, 180);
        paused.name = "test-zombie-clear-live".into();
        paused.state = SkillState::Paused;
        {
            let mut g = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
            g.insert(zombie.into(), failed);
            g.insert("test-zombie-clear-live".into(), paused);
        }
        clear_terminal_skill_runs();
        {
            let g = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
            assert!(g.get(zombie).is_none(), "Failed 残留应被清除");
            assert!(
                g.get("test-zombie-clear-live").is_some(),
                "Paused 不应被误清"
            );
        }
        // 收尾：不给其他测试留状态
        let mut g = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
        g.remove("test-zombie-clear-live");
    }

    /// 回滚窗口重开/复原——Failed 可重开为 Running（按会话匹配），
    /// 复原回 Failed；非 Failed 状态 / 别的会话的 run 不动
    #[test]
    fn reopen_failed_run_for_rollback_scoped_by_state_and_session() {
        let _serial = SKILL_RUNS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let name = "test-rollback-reopen";
        let mut run = test_run(8, 180);
        run.name = name.into();
        run.state = SkillState::Failed;
        run.end_reason = "step 失败".into();
        run.session_id = Some("s-rb".into());
        skill_runs()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.into(), run);

        // 会话不匹配 → 不重开
        assert!(!reopen_failed_run_for_rollback(name, Some("other")));
        // 会话匹配 → 重开为 Running（原子工具门禁放行）
        assert!(reopen_failed_run_for_rollback(name, Some("s-rb")));
        assert_eq!(test_skill_run_state(name), Some(SkillState::Running));
        // 重复重开（已非 Failed）→ false
        assert!(!reopen_failed_run_for_rollback(name, Some("s-rb")));
        // 复原回 Failed 终态
        restore_failed_run_after_rollback(name, Some("s-rb"));
        assert_eq!(test_skill_run_state(name), Some(SkillState::Failed));
        // 会话不匹配 → 不复原
        let mut runs = skill_runs().lock().unwrap_or_else(|e| e.into_inner());
        runs.get_mut(name).unwrap().state = SkillState::Running;
        drop(runs);
        restore_failed_run_after_rollback(name, Some("other"));
        assert_eq!(test_skill_run_state(name), Some(SkillState::Running));

        test_remove_skill_run(name);
    }
}
