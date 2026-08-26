use super::manage::skill_search_paths;
use super::parse::{parse_meta, SkillMeta, SKILL_NAME_CHARS_OK};
use tauri::AppHandle;

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
    /// 运行模式（auto/interactive）：v1.0 暂停语义由底层 ask_user_confirm 承接，
    /// 字段为审计与后续暂停态切换保留
    #[allow(dead_code)]
    pub mode: String,
    #[allow(dead_code)]
    pub risk_level: String,
    pub rollback: String,
    /// 已执行动作记录（工具名+参数摘要；回滚清单来源）
    pub actions: Vec<String>,
    /// 终止原因（Terminated/Failed 时填写）
    pub end_reason: String,
    /// 是否支持暂停后续跑（元数据 resumable 镜像）
    pub resumable: bool,
    /// resumable=false 时的暂停即终止：确认通过后执行当前动作，随后技能终止
    pub terminal_after_confirm: bool,
    /// 归属会话（2026-08-26 会话隔离）：触发该技能的聊天会话 id；
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
            mode: meta.mode.clone(),
            risk_level: meta.risk_level.clone(),
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

pub(crate) fn skill_runs() -> &'static std::sync::Mutex<std::collections::HashMap<String, SkillRun>> {
    SKILL_RUNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 当前会话是否有 Skill 处于 Running 状态（老板 2026-08-17 18:14 后置拦截拍板；
/// 2026-08-26 会话隔离：加会话过滤，别的会话的 Skill 不算本会话活动）
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

/// 主循环接入用（Block 2 2026-08-17 22:26）：克隆当前会话活动 Skill 快照（任意非 Loaded 状态）。
/// 供 `bot_chat` 主循环与 DSL 调度器调，拿快照去 `advance_skill` / `advance_dsl` 决策，不再持锁。
/// 2026-08-26 会话隔离：按 session_id 过滤——别的会话暂停/运行中的 Skill 不可见、不推进。
///
/// 状态过滤说明：Loaded 是 start_skill 中的过渡态——技能预审通过后立即转为 Running，
/// Loaded 仅在断言/异常路径短暂存在，advance_skill 返回 NoActive 与主循环预期一致。
///
/// ⚠️ 终态（Completed/Failed/Terminated）也会被返回——这是 DSL 调度器感知
/// 「用户 /stop / 工具失败」所必需的；调用方若是新一轮执行的入口（run_model_loop /
/// run_skill_scheduler），必须先 `clear_terminal_skill_runs()` 清掉上轮遗留的僵尸终态，
/// 否则会在第 0 步被 advance 短路（2026-08-18 agent 假死事故根因）。
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

/// 读取技能正文 + 完整元数据（Phase 4 第 4 项 2026-08-18 07:09：多目录 fallback）
/// 遍历 `skill_search_paths(app)`：数据目录找不到 → dev 模式 fallback target/debug/skills。
/// 数据目录优先（用户已修改的 Skill 优先于 dev mock 版本）。
pub(crate) fn load_skill_meta(app: &AppHandle, name: &str) -> Result<(SkillMeta, String), String> {
    if name.is_empty() || !name.chars().all(SKILL_NAME_CHARS_OK) {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("技能名无效".into());
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
    Err(format!("技能「{name}」不存在（检查设置页技能列表）"))
}

/// P2-24 测试辅助：直接插入一个指定状态的 skill run（绕过磁盘 SKILL.md 加载）。
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
        mode: "interactive".into(),
        risk_level: "medium".into(),
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

/// P2-24 测试辅助收尾：移除插入的 run，不给其他测试留状态
#[cfg(test)]
pub(crate) fn test_remove_skill_run(name: &str) {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(name);
}

/// P2-24 测试辅助：读 run 当前状态（断言终止语义用）
#[cfg(test)]
pub(crate) fn test_skill_run_state(name: &str) -> Option<SkillState> {
    skill_runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(name)
        .map(|r| r.state.clone())
}


/// 测试辅助：构造一个指定 max_steps / timeout 的 SkillRun（原 tests::test_run，跨子模块测试共用）
#[cfg(test)]
pub(crate) fn test_run(max_steps: usize, timeout_secs: u64) -> SkillRun {
        SkillRun {
            name: "test-skill".into(),
            state: SkillState::Loaded,
            step: 0,
            max_steps,
            started_at_ms: 1000,
            timeout_secs,
            mode: "interactive".into(),
            risk_level: "medium".into(),
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

    /// 僵尸终态清理（2026-08-18 agent 假死根因）：入口清理后终态 run 不再被当"活动"，
    /// Running/Paused 不受影响（测试用 Paused：is_skill_active 只认 Running，避免与并行测试竞争）
    #[test]
    fn clear_terminal_removes_only_terminal_states() {
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

}
