// 任务卡执行流程识别

use crate::bot_chat::TaskExecOrigin;
use std::collections::HashMap;
use std::sync::OnceLock;

/// sid → TaskExecOrigin 注册表。任务卡执行流程开跑时写、结束时删。
/// 用 std::sync::Mutex 同步锁（读写都极轻量，async context 里短临界区 OK）。
static SESSION_ORIGINS: OnceLock<std::sync::Mutex<HashMap<String, TaskExecOrigin>>> =
    OnceLock::new();

fn session_origins() -> &'static std::sync::Mutex<HashMap<String, TaskExecOrigin>> {
    SESSION_ORIGINS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 毒锁按全仓口径处理：into_inner 取回数据继续跑 + eprintln 留痕。
/// 静默吞掉会让 unregister 变 no-op，注册条目永久泄漏（is_task_execution_flow 常真）。
fn lock_session_origins() -> std::sync::MutexGuard<'static, HashMap<String, TaskExecOrigin>> {
    session_origins().lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] tool_guard::SESSION_ORIGINS: {e:?}");
        e.into_inner()
    })
}

pub fn register_exec_session(session_id: &str, origin: TaskExecOrigin) {
    lock_session_origins().insert(session_id.to_string(), origin);
}

pub fn unregister_exec_session(session_id: &str) {
    lock_session_origins().remove(session_id);
}

/// 当前 session 是否在任务卡执行流程内（🤖 按钮 / ⏰ 定时 / 📦 批量）
pub fn is_task_execution_flow(session_id: Option<&str>) -> bool {
    let Some(sid) = session_id else {
        return false;
    };
    lock_session_origins().contains_key(sid)
}

// 子 agent 会话注册表

use crate::db::{SubagentBudget, SubagentProfile};
use std::path::PathBuf;

/// 子 agent 执行会话的上下文（runner 开跑时写、收尾删）。
/// 白名单闸（dispatch）与 spawn 递归身份校验的数据源（设计 §5/§10 双保险②）。
#[derive(Debug, Clone)]
pub struct SubagentSessionCtx {
    pub subagent_id: String,
    pub task_id: String,
    pub profile: SubagentProfile,
    pub budget: SubagentBudget,
    /// 产物目录 gen_dir/subagents/{subagent_id}/（write_artifact_file 的唯一可写区）
    pub artifact_dir: PathBuf,
    /// 已执行工具调用累计（ 预算强制：dispatch 白名单闸后自增，
    /// 触顶拒绝新调用——max_tool_calls 的强制点）
    pub used_tool_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

static SUBAGENT_SESSIONS: OnceLock<std::sync::Mutex<HashMap<String, SubagentSessionCtx>>> =
    OnceLock::new();

fn subagent_sessions() -> &'static std::sync::Mutex<HashMap<String, SubagentSessionCtx>> {
    SUBAGENT_SESSIONS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 锁中毒按 into_inner 取数据（全仓 mutex 口径， r2/r3 high 采纳）：
/// 安全闸的锁不能 fail-open（中毒时把子会话当主会话 = 递归闸失守），
/// 也不能 fail-closed 到吞掉注册/清理（泄漏更难查）——取回数据继续跑 + eprintln 留痕。
fn lock_subagent_sessions() -> std::sync::MutexGuard<'static, HashMap<String, SubagentSessionCtx>> {
    subagent_sessions().lock().unwrap_or_else(|e| {
        eprintln!("[mutex_poisoned] tool_guard::SUBAGENT_SESSIONS: {e:?}");
        e.into_inner()
    })
}

/// 注册子 agent 会话上下文。同 id 已存在 = 上一 runner 泄漏或双跑 bug：
/// warn 留痕后覆盖（不静默）。
pub fn register_subagent_session(session_id: &str, ctx: SubagentSessionCtx) {
    let mut m = lock_subagent_sessions();
    if m.contains_key(session_id) {
        eprintln!(
            "[tool_guard] 子 agent 会话重复注册（旧条目被覆盖，疑似泄漏/双跑）：{session_id}"
        );
    }
    m.insert(session_id.to_string(), ctx);
}

pub fn unregister_subagent_session(session_id: &str) {
    lock_subagent_sessions().remove(session_id);
}

/// 会话是否为子 agent 执行会话（子 agent 调 spawn → 递归硬禁的身份依据）。
/// 锁中毒不改变判定语义（见 [lock_subagent_sessions]）。
pub fn is_subagent_session(session_id: Option<&str>) -> bool {
    let Some(sid) = session_id else {
        return false;
    };
    lock_subagent_sessions().contains_key(sid)
}

/// RAII 注册守卫：构造即注册、Drop 即反注册——runner 在 register 与 unregister
/// 之间 panic 也不会泄漏条目（ 采纳）。Drop 仅在条目仍属于本守卫的
/// subagent 时反注册——同会话被后来的注册覆盖时不误删他人条目（ 采纳）。
pub(crate) struct SubagentSessionGuard {
    session_id: String,
    subagent_id: String,
}

impl SubagentSessionGuard {
    pub(crate) fn new(session_id: &str, ctx: SubagentSessionCtx) -> Self {
        let subagent_id = ctx.subagent_id.clone();
        register_subagent_session(session_id, ctx);
        Self {
            session_id: session_id.to_string(),
            subagent_id,
        }
    }
}

impl Drop for SubagentSessionGuard {
    fn drop(&mut self) {
        // 归属判定与删除必须在同一临界区：分两次取锁的话，窗口内后来者用
        // 同一 session_id 重新注册的新条目会被本守卫误删
        let mut m = lock_subagent_sessions();
        if m.get(&self.session_id)
            .map(|c| c.subagent_id == self.subagent_id)
            .unwrap_or(false)
        {
            m.remove(&self.session_id);
        }
    }
}

pub fn subagent_ctx(session_id: Option<&str>) -> Option<SubagentSessionCtx> {
    let sid = session_id?;
    lock_subagent_sessions().get(sid).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 死命令 bind_file 已下线（前端用 bind_files 复数形）—— 锁防回退
    ///（匹配串用 concat! 拼接：本测试自身就在 tool_guard.rs 里，
    /// 裸写字面量会自匹配误判）
    #[test]
    fn dead_command_bind_file_not_in_dispatcher() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/bot.rs")).unwrap();
        assert!(
            !text.contains(concat!("\"bind", "_file\" => tool_bind_file")),
            "tool_bind_file 死命令不得再注册到 dispatcher（前端用 bind_files 复数形）"
        );
        let model_loop = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bot_model_loop.rs"
        ))
        .unwrap();
        assert!(
            !model_loop.contains(concat!("\"bind", "_file\"")),
            "bind_file 工具名不应出现在 bot_model_loop.rs（schema / tools list 都不应有）"
        );
    }

    // ── is_task_execution_flow 注册/反注册 ──

    #[test]
    fn session_origin_register_and_query() {
        let sid = "test_sid_register_query";
        register_exec_session(sid, TaskExecOrigin::Manual);
        assert!(is_task_execution_flow(Some(sid)));
        unregister_exec_session(sid);
        assert!(!is_task_execution_flow(Some(sid)));
    }

    // ── 子 agent 会话注册表（ sibling API 同等测试覆盖）──

    fn sample_ctx(subagent_id: &str) -> SubagentSessionCtx {
        SubagentSessionCtx {
            subagent_id: subagent_id.into(),
            task_id: format!("card-{subagent_id}"),
            profile: crate::db::SubagentProfile::Research,
            budget: crate::db::SubagentBudget::default(),
            artifact_dir: std::path::PathBuf::from(format!("/tmp/art/{subagent_id}")),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    #[test]
    fn subagent_session_register_query_unregister_roundtrip() {
        let sid = "test_subagent_session_roundtrip";
        assert!(!is_subagent_session(Some(sid)));
        register_subagent_session(sid, sample_ctx("sa_rt1"));
        assert!(is_subagent_session(Some(sid)));
        let ctx = subagent_ctx(Some(sid)).expect("注册后必能取到 ctx");
        assert_eq!(ctx.subagent_id, "sa_rt1");
        assert_eq!(ctx.task_id, "card-sa_rt1");
        assert_eq!(ctx.profile, crate::db::SubagentProfile::Research);
        assert_eq!(ctx.budget.max_turns, crate::db::DEFAULT_MAX_TURNS);
        unregister_subagent_session(sid);
        assert!(!is_subagent_session(Some(sid)));
        assert!(subagent_ctx(Some(sid)).is_none());
    }

    #[test]
    fn subagent_session_none_and_unknown_are_false() {
        assert!(!is_subagent_session(None));
        assert!(subagent_ctx(None).is_none());
        assert!(!is_subagent_session(Some("never_registered_subagent")));
    }

    /// 守卫 Drop 不得误删同 sid 后来者的注册：归属判定与删除必须同一临界区
    #[test]
    fn guard_drop_keeps_newer_registration_same_sid() {
        let sid = "test_guard_drop_overwrite";
        let g = SubagentSessionGuard::new(sid, sample_ctx("sa_old"));
        // 同 sid 覆盖注册（后来的 runner 接管）
        register_subagent_session(sid, sample_ctx("sa_new"));
        drop(g);
        assert!(
            is_subagent_session(Some(sid)),
            "旧守卫 Drop 不得删除后来者条目"
        );
        assert_eq!(
            subagent_ctx(Some(sid)).unwrap().subagent_id,
            "sa_new",
            "存留的必须是后来者的 ctx"
        );
        unregister_subagent_session(sid);
        assert!(!is_subagent_session(Some(sid)));
    }

    #[test]
    fn session_origin_none_returns_false() {
        assert!(!is_task_execution_flow(None));
        assert!(!is_task_execution_flow(Some("never_registered")));
    }

    #[test]
    fn session_origin_all_three_origins_qualify() {
        let s_manual = "s_manual";
        let s_sched = "s_sched";
        let s_batch = "s_batch";
        register_exec_session(s_manual, TaskExecOrigin::Manual);
        register_exec_session(s_sched, TaskExecOrigin::Scheduled);
        register_exec_session(s_batch, TaskExecOrigin::Batch);
        assert!(is_task_execution_flow(Some(s_manual)));
        assert!(is_task_execution_flow(Some(s_sched)));
        assert!(is_task_execution_flow(Some(s_batch)));
        unregister_exec_session(s_manual);
        unregister_exec_session(s_sched);
        unregister_exec_session(s_batch);
    }
}
