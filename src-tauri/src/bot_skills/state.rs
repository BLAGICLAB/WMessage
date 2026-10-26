use std::path::PathBuf;

use super::manage::skill_search_paths;
use super::parse::{parse_meta, SkillMeta, SKILL_NAME_CHARS_OK};
use crate::error::CommandError;
// SKILL_RUNS 已迁入 `AppState`；这里 `pub(crate) use` 让本模块与
// 兄弟模块（runtime.rs / scheduler.rs 的 `use super::state::{skill_runs, …}`）路径不变。
pub(crate) use crate::app_state::skill_runs;
use tauri::AppHandle;

pub(crate) const MAX_SKILL_BODY: usize = 50 * 1024;

// Skill 调度器（运行模型 v1.0：状态机 + 步骤循环 + 熔断 + 回滚记录）

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
    /// 触发本次暂停的确认窗 id：确认结果只作用于「由同一个确认窗暂停」的技能。
    ///
    /// 认错的暂停被误恢复或误终止，比不恢复更糟——没有 id 的暂停（手工置
    /// Paused 的路径）不被任何确认窗结果命中。
    pub pending_confirm_id: Option<String>,
    /// 归属会话：触发该技能的聊天会话 id；
    /// 后台定时任务为 None。active_skill_run / 暂停 / 确认按此过滤，
    /// 防会话 A 暂停中的 Skill 被会话 B 的主循环推进或确认。
    pub session_id: Option<String>,
    /// use_skill 调用传入的参数（对象）；${params.key} 替换源
    pub params: serde_json::Value,
    /// 技能目录（第三层渐进披露——references 清单 + 会话级读白名单）
    pub dir: Option<PathBuf>,
    /// 运行模式（auto | interactive，元数据镜像）。
    ///
    /// 失败语义按它分岔：**auto**（DSL 调度器）的步骤失败必须立刻终止该步并跑
    /// 回滚，调度器正是靠「run 变 Failed」感知失败的；**interactive**（LLM 驱动）
    /// 里模型自己会换策略，一次工具失败不该把整个技能判死。
    pub mode: String,
    /// 连续失败次数（成功一次即清零）。
    ///
    /// 只在 interactive 模式下参与判死：达到阈值仍不换策略，才认定这次技能
    /// 真的走不下去。
    pub consec_failures: usize,
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
            pending_confirm_id: None,
            session_id: None,
            params: serde_json::Value::Null,
            dir: None,
            mode: meta.mode.clone(),
            consec_failures: 0,
        }
    }
}

// SKILL_RUNS 已迁入 `AppState.skill_runs`（再导出见文件头 use）。

/// 回滚窗口临时重开：回滚段执行时 run 已是 Failed，
/// 而 AtomicGuard 只认 Running —— 回滚段里的原子工具（link_file_to_task 等）会被自家门禁拦截。
/// 把本会话该技能的 Failed run 临时置回 Running（回滚期间门禁放行），返回是否实际切换。
/// 调用方在回滚段结束后必须调 `restore_failed_run_after_rollback` 复原。
pub(crate) fn reopen_failed_run_for_rollback<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
    session_id: Option<&str>,
) -> bool {
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
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
pub(crate) fn restore_failed_run_after_rollback<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
    session_id: Option<&str>,
) {
    let registry = skill_runs(app);
    let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
    if let Some(run) = runs.get_mut(name) {
        if run.state == SkillState::Running && run.session_id.as_deref() == session_id {
            run.state = SkillState::Failed;
        }
    }
}

/// 活动优先级：Running > Paused > 终态。
///
/// 同会话可能同时存在「切走的旧技能（终态）」与「新启的技能（Running）」——
/// 取到终态那条，主循环会把新技能当「已完成」直接收尾（技能一用就断）。
fn run_liveness_rank(run: &SkillRun) -> u8 {
    match run.state {
        SkillState::Running => 0,
        SkillState::Paused => 1,
        _ => 2,
    }
}

/// 快照取哪一条（纯函数，单测入口）：活的优先 → 同层取最近启动的 → 名字兜底。
///
/// 输入顺序不参与决策：候选来自 HashMap 迭代，序是随机的，
/// 同一场景不能时好时坏（否则故障表现为「偶发」而不是「必现」）。
fn pick_active_run(cands: &[&SkillRun]) -> Option<SkillRun> {
    let mut ordered: Vec<&SkillRun> = cands.to_vec();
    ordered.sort_by(|a, b| {
        run_liveness_rank(a)
            .cmp(&run_liveness_rank(b))
            .then(b.started_at_ms.cmp(&a.started_at_ms))
            .then(a.name.cmp(&b.name))
    });
    ordered.first().map(|r| (*r).clone())
}

/// 主循环接入用：克隆当前会话活动 Skill 快照（任意非 Loaded 状态）。
/// 供 `bot_chat` 主循环与 DSL 调度器调，拿快照去 `advance_skill` / `advance_dsl` 决策，不再持锁。
/// 按 session_id 过滤——别的会话暂停/运行中的 Skill 不可见、不推进。
///
/// 状态过滤说明：Loaded 是 start_skill 中的过渡态——技能预审通过后立即转为 Running，
/// Loaded 仅在断言/异常路径短暂存在，advance_skill 返回 NoActive 与主循环预期一致。
///
/// 取哪一条：**活的优先**（Running > Paused > 终态），同层取最近启动的。
/// 同会话同时有旧技能（终态）与新技能（Running）时必须取新技能——
/// 取到终态那条，主循环会把新技能当「已完成」直接收尾（技能一用就断）。
///
/// ⚠️ 没有活着的 run 时才回退终态（Completed/Failed/Terminated）——这是 DSL 调度器
/// 与主循环感知「用户 /stop / 工具失败 / 已完成」所必需的；调用方若是新一轮执行的
/// 入口（run_model_loop / run_skill_scheduler），必须先 `clear_terminal_skill_runs()`
/// 清掉上轮遗留的僵尸终态，否则会在第 0 步被 advance 短路（agent 假死事故根因）。
pub fn active_skill_run_for<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Option<&str>,
) -> Option<SkillRun> {
    let registry = skill_runs(app);
    let guard = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
    let cands: Vec<&SkillRun> = guard
        .values()
        .filter(|r| !matches!(r.state, SkillState::Loaded) && r.session_id.as_deref() == session_id)
        .collect();
    pick_active_run(&cands)
}

/// 清理指定会话的终态 SkillRun（Completed/Failed/Terminated）——只清本会话，
/// 其他会话刚终态、正待各自调度器感知（/stop、工具失败）的 run 不受影响。
/// SKILL_RUNS 只进不出：上轮遗留的终态 run 会被 active_skill_run 当"活动"，
/// 在新一轮执行的第 0 步被 advance 短路——静默返回空文本、不发 LLM 请求（agent 假死）。
/// 在每轮执行入口（run_model_loop / run_skill_scheduler）以本会话 id 调用；
/// 本轮执行中新进入终态的 run 不受影响（清理发生在入口，轮内状态机照常可见）。
/// 增长注记：仅本会话入口触发清理，长期不再执行的会话会留存末条终态记录
/// （每会话至多一条，量级可控）。
pub fn clear_terminal_skill_runs<R: tauri::Runtime>(app: &AppHandle<R>, session_id: Option<&str>) {
    let registry = skill_runs(app);
    let mut guard = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
    guard.retain(|_, r| {
        r.session_id.as_deref() != session_id
            || !matches!(
                r.state,
                SkillState::Completed | SkillState::Failed | SkillState::Terminated
            )
    });
}

/// 读取技能正文 + 完整元数据 + 技能目录（多目录 fallback）
/// 遍历 `skill_search_paths(app)`：数据目录找不到 → dev 模式 fallback target/debug/skills。
/// 数据目录优先（用户已修改的 Skill 优先于 dev mock 版本）。
/// 返回目录供第三层渐进披露（references 清单）与会话级读白名单。
pub(crate) fn load_skill_meta<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    name: &str,
) -> Result<(SkillMeta, String, PathBuf), CommandError> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        return Err(CommandError::DomainRule {
            domain: "skill".to_string(),
            reason: "技能名无效".to_string(),
        });
    }
    for dir in &skill_search_paths(app) {
        let path = dir.join(name).join("SKILL.md");
        // is_file 预检：不存在/目录直接换下一个 fallback 目录
        if !path.is_file() {
            continue;
        }
        // 直接读并按 NotFound 继续 fallback——消除「预检通过后、读取前被删」
        // 间隙误报「读取失败」；其余 I/O 错误带原因报错（不再吞掉错误详情）
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(CommandError::DomainRule {
                    domain: "skill".to_string(),
                    reason: format!("技能「{name}」的 SKILL.md 读取失败：{e}"),
                });
            }
        };
        let meta = parse_meta(&text, name);
        let body: String = text.chars().take(MAX_SKILL_BODY).collect();
        return Ok((meta, body, dir.join(name)));
    }
    Err(CommandError::DomainRule {
        domain: "skill".to_string(),
        reason: format!("技能「{name}」不存在（检查设置页技能列表）"),
    })
}

// 集成测试钩子
// tests/ 集成测试是独立 crate，够不到下面 #[cfg(test)] 的 test_insert_skill_run；
// 调度器 e2e（tests/skill_e2e.rs）需要插入 Running/Paused/Failed 的 run 来驱动
// advance_dsl 分支与回滚窗口的真路径。仅测试使用，生产路径不调。

#[doc(hidden)]
#[cfg(debug_assertions)]
pub fn test_hook_insert_skill_run<R: tauri::Runtime>(app: &AppHandle<R>, run: SkillRun) {
    let registry = skill_runs(app);
    crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs")
        .insert(run.name.clone(), run);
}

#[doc(hidden)]
#[cfg(debug_assertions)]
pub fn test_hook_remove_skill_run<R: tauri::Runtime>(app: &AppHandle<R>, name: &str) {
    let registry = skill_runs(app);
    crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs").remove(name);
}

#[doc(hidden)]
#[cfg(debug_assertions)]
pub fn test_hook_skill_run_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
) -> Option<SkillState> {
    let registry = skill_runs(app);
    let guard = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
    guard.get(name).map(|r| r.state.clone())
}

/// 测试辅助：直接插入一个指定状态的 skill run（绕过磁盘 SKILL.md 加载）。
/// 与 tests::test_run 同一份字段构造，供跨模块测试（lib.rs 退出清理）使用。
#[cfg(test)]
pub(crate) fn test_insert_skill_run<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
    state: SkillState,
) {
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
        pending_confirm_id: None,
        session_id: None,
        params: serde_json::Value::Null,
        dir: None,
        mode: "interactive".into(),
        consec_failures: 0,
    };
    let registry = skill_runs(app);
    crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs")
        .insert(name.into(), run);
}

/// 测试辅助收尾：移除插入的 run，不给其他测试留状态
#[cfg(test)]
pub(crate) fn test_remove_skill_run<R: tauri::Runtime>(app: &AppHandle<R>, name: &str) {
    let registry = skill_runs(app);
    crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs").remove(name);
}

/// 测试辅助：读 run 当前状态（断言终止语义用）
#[cfg(test)]
pub(crate) fn test_skill_run_state<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
) -> Option<SkillState> {
    let registry = skill_runs(app);
    let guard = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
    guard.get(name).map(|r| r.state.clone())
}

// 原 `SKILL_RUNS_TEST_LOCK` 已删除。
// 理由：表随 `AppState` 注入，凡写 SKILL_RUNS 且对内容有断言的用例
// （state.rs 两个用例 + lib.rs 退出清理用例）都各自 `manage` 独立实例，
// 彼此不可见；而 runtime.rs / scheduler.rs 的测试是纯状态机（不碰表）。
// 换言之，「并行互删对方 run」这条路已从结构上消失，不再需要人肉串行。

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
        pending_confirm_id: None,
        session_id: None,
        params: serde_json::Value::Null,
        dir: None,
        mode: "interactive".into(),
        consec_failures: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入独立 `AppState` 的 mock handle（SKILL_RUNS 随 AppState 隔离）
    fn test_handle() -> AppHandle<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    }

    /// 僵尸终态清理：入口清理后本会话终态 run 不再被当"活动"，
    /// Running/Paused 不受影响（测试用 Paused：活动判定只认 Running，避免与并行测试竞争）；
    /// 跨会话隔离：其他会话的终态 run 不得被本会话入口清理误删
    #[test]
    fn clear_terminal_removes_only_terminal_states() {
        let app = test_handle();
        let zombie = "test-zombie-clear";
        // 本会话：一个 Failed 残留 + 一个 Paused 活跃；他会话：一个 Failed 终态
        let mut failed = test_run(8, 180);
        failed.name = zombie.into();
        failed.state = SkillState::Failed;
        failed.session_id = Some("sess-clear-a".into());
        let mut paused = test_run(8, 180);
        paused.name = "test-zombie-clear-live".into();
        paused.state = SkillState::Paused;
        paused.session_id = Some("sess-clear-a".into());
        let mut other = test_run(8, 180);
        other.name = "test-zombie-clear-other".into();
        other.state = SkillState::Failed;
        other.session_id = Some("sess-clear-b".into());
        {
            let registry = skill_runs(&app);
            let mut g =
                crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
            g.insert(zombie.into(), failed);
            g.insert("test-zombie-clear-live".into(), paused);
            g.insert("test-zombie-clear-other".into(), other);
        }
        clear_terminal_skill_runs(&app, Some("sess-clear-a"));
        {
            let registry = skill_runs(&app);
            let g = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
            assert!(g.get(zombie).is_none(), "本会话 Failed 残留应被清除");
            assert!(
                g.get("test-zombie-clear-live").is_some(),
                "本会话 Paused 不应被误清"
            );
            assert!(
                g.get("test-zombie-clear-other").is_some(),
                "其他会话终态 run 不应被本会话入口清理误删"
            );
        }
        // 收尾：不给其他测试留状态
        let registry = skill_runs(&app);
        let mut g = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
        g.remove("test-zombie-clear-live");
        g.remove("test-zombie-clear-other");
    }

    /// 同会话同时存在终态 run 与活着的 run → 快照必须取活着的：
    /// 取到终态那条，主循环会把新启的技能当「已完成」直接收尾（技能一用就断）。
    #[test]
    fn active_skill_run_prefers_live_run_over_terminal_one() {
        let app = test_handle();
        {
            let registry = skill_runs(&app);
            let mut g =
                crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
            let mut old = test_run(8, 180);
            old.name = "old-skill".into();
            old.state = SkillState::Completed;
            old.session_id = Some("sess-live".into());
            old.started_at_ms = 1000;
            let mut fresh = test_run(8, 180);
            fresh.name = "new-skill".into();
            fresh.state = SkillState::Running;
            fresh.session_id = Some("sess-live".into());
            fresh.started_at_ms = 2000;
            g.insert(old.name.clone(), old);
            g.insert(fresh.name.clone(), fresh);
        }
        let got = active_skill_run_for(&app, Some("sess-live")).expect("本会话应有活动 run");
        assert_eq!(got.name, "new-skill", "必须取活着的 run，不能取终态那条");
        assert_eq!(got.state, SkillState::Running);
    }

    /// 两条活着的 run → 取最近启动的那条：取哪一条不能取决于 HashMap 迭代序
    /// （同一会话多条 run 时结果必须稳定，否则同一场景时好时坏）。
    #[test]
    fn active_skill_run_picks_most_recently_started_live_run() {
        let app = test_handle();
        {
            let registry = skill_runs(&app);
            let mut g =
                crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
            let mut newer = test_run(8, 180);
            newer.name = "newer-skill".into();
            newer.state = SkillState::Running;
            newer.session_id = Some("sess-two".into());
            newer.started_at_ms = 5000;
            let mut older = test_run(8, 180);
            older.name = "older-skill".into();
            older.state = SkillState::Running;
            older.session_id = Some("sess-two".into());
            older.started_at_ms = 4000;
            // 先插新的再插旧的：结果不能随插入顺序变
            g.insert(newer.name.clone(), newer);
            g.insert(older.name.clone(), older);
        }
        let got = active_skill_run_for(&app, Some("sess-two")).expect("本会话应有活动 run");
        assert_eq!(got.name, "newer-skill", "同层多条取最近启动的");
    }

    /// 取数规则（纯函数）：活的必须压过终态，与输入顺序无关。
    /// 确定性锁——旧实现取「迭代序第一条」，候选顺序一变结果就变（表现为偶发）。
    #[test]
    fn pick_active_run_prefers_live_over_terminal_regardless_of_input_order() {
        let mut terminal = test_run(8, 180);
        terminal.name = "old-skill".into();
        terminal.state = SkillState::Completed;
        // 终态那条「启动更晚」——优先级必须压过时间序，否则取了它主循环就收尾了
        terminal.started_at_ms = 9000;
        let mut live = test_run(8, 180);
        live.name = "new-skill".into();
        live.state = SkillState::Running;
        live.started_at_ms = 1000;

        for cands in [vec![&terminal, &live], vec![&live, &terminal]] {
            let got = pick_active_run(&cands).expect("应有候选");
            assert_eq!(got.name, "new-skill", "活的必须压过终态");
        }
    }

    /// 取数规则（纯函数）：同层多条活着的取最近启动的，与输入顺序无关。
    #[test]
    fn pick_active_run_takes_most_recent_live_run_regardless_of_input_order() {
        let mut older = test_run(8, 180);
        older.name = "older-skill".into();
        older.state = SkillState::Running;
        older.started_at_ms = 1000;
        let mut newer = test_run(8, 180);
        newer.name = "newer-skill".into();
        newer.state = SkillState::Running;
        newer.started_at_ms = 2000;

        for cands in [vec![&older, &newer], vec![&newer, &older]] {
            let got = pick_active_run(&cands).expect("应有候选");
            assert_eq!(got.name, "newer-skill", "同层取最近启动的");
        }
    }

    /// 回滚窗口重开/复原——Failed 可重开为 Running（按会话匹配），
    /// 复原回 Failed；非 Failed 状态 / 别的会话的 run 不动
    #[test]
    fn reopen_failed_run_for_rollback_scoped_by_state_and_session() {
        let app = test_handle();
        let name = "test-rollback-reopen";
        let mut run = test_run(8, 180);
        run.name = name.into();
        run.state = SkillState::Failed;
        run.end_reason = "step 失败".into();
        run.session_id = Some("s-rb".into());
        {
            let registry = skill_runs(&app);
            crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs")
                .insert(name.into(), run);
        }

        // 会话不匹配 → 不重开
        assert!(!reopen_failed_run_for_rollback(&app, name, Some("other")));
        // 会话匹配 → 重开为 Running（原子工具门禁放行）
        assert!(reopen_failed_run_for_rollback(&app, name, Some("s-rb")));
        assert_eq!(test_skill_run_state(&app, name), Some(SkillState::Running));
        // 重复重开（已非 Failed）→ false
        assert!(!reopen_failed_run_for_rollback(&app, name, Some("s-rb")));
        // 复原回 Failed 终态
        restore_failed_run_after_rollback(&app, name, Some("s-rb"));
        assert_eq!(test_skill_run_state(&app, name), Some(SkillState::Failed));
        // 会话不匹配 → 不复原
        let registry = skill_runs(&app);
        let mut runs = crate::db::lock::lock_or_recover(&registry, "bot_skills::state::skill_runs");
        runs.get_mut(name).unwrap().state = SkillState::Running;
        drop(runs);
        restore_failed_run_after_rollback(&app, name, Some("other"));
        assert_eq!(test_skill_run_state(&app, name), Some(SkillState::Running));

        test_remove_skill_run(&app, name);
    }
}
