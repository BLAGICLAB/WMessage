//! 工具调用守卫
//!
//! ## 后置拦截（原子黑名单）
//! - 凡是「仅作为某个 Skill 内部子步骤、不允许用户直接独立调用」的底层原子 Function
//!   → 命中即阻断、返回提示走对应 Skill，**不交给模型**（硬锁）
//! - 黑名单内 Function 仅在 Skill 运行时（state == Running）放行
//!
//! ## 前置预路由
//! 已实现于 intent_router.rs（L1 正则硬锁：命中技能 intents 直接加载，LLM 不参与选择 Skill）

/// 已废弃：原子黑名单 D4d 后清空。
///
/// 保留空数组 + 函数签名仅为兼容 lib.rs dead_command_tests 与既有调用方；
/// `is_atomic_tool` 现在永远返回 false，新工具不再走黑名单拦截，改由
/// `is_task_execution_flow` 在工具内部按 session 上下文判定合法性。
pub const ATOMIC_TOOLS: &[&str] = &[];

/// 是否在原子黑名单（永远 false，保留为死函数以防编译错误）
pub fn is_atomic_tool(name: &str) -> bool {
    ATOMIC_TOOLS.iter().any(|&t| t == name)
}

/// 阻断时的错误消息（仅占位，调用方不应再走到这里）
pub fn atomic_block_message(name: &str) -> String {
    format!("⚠️ {name} 不允许裸调（黑名单已废弃，但保留该消息以兼容调用方）")
}

/// 当前会话是否有 Skill 在 Running 状态
///（保留供 bot_skills 内部使用；D4d 后 link_file_to_task 等不再依赖此判定）
pub fn is_skill_active(session_id: Option<&str>) -> bool {
    crate::bot_skills::is_skill_active_for(session_id)
}

// ───────────────────────── 任务卡执行流程识别（D4d） ─────────────────────────

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

pub fn register_exec_session(session_id: &str, origin: TaskExecOrigin) {
    if let Ok(mut m) = session_origins().lock() {
        m.insert(session_id.to_string(), origin);
    }
}

pub fn unregister_exec_session(session_id: &str) {
    if let Ok(mut m) = session_origins().lock() {
        m.remove(session_id);
    }
}

/// 当前 session 是否在任务卡执行流程内（🤖 按钮 / ⏰ 定时 / 📦 批量）
pub fn is_task_execution_flow(session_id: Option<&str>) -> bool {
    let Some(sid) = session_id else {
        return false;
    };
    session_origins()
        .lock()
        .ok()
        .map(|m| m.contains_key(sid))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归锁：create_word_revisions 不在黑名单（聊天直调）
    #[test]
    fn create_word_revisions_is_not_atomic_anymore() {
        assert!(!is_atomic_tool("create_word_revisions"));
    }

    /// D4d 黑名单清空后：所有工具都不再判为原子
    #[test]
    fn atomic_blacklist_is_empty() {
        assert!(ATOMIC_TOOLS.is_empty());
        assert!(!is_atomic_tool("link_file_to_task"));
    }

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

    #[test]
    fn single_point_tools_are_not_atomic() {
        for name in [
            "list_tasks",
            "query_single_task",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "add_subtask",
            "toggle_subtask",
            "remove_subtask",
            "read_text_file",
            "ocr_image",
            "grep_files",
            "list_files",
            "search_tasks",
            "extract_document",
            "create_word",
            "create_word_revisions",
            "create_excel",
            "create_ppt",
            "create_pdf",
            "run_python",
            "web_search",
            "fetch_url",
            "get_current_time",
            "remember_fact",
            "recall_facts",
            "record_lesson",
            "use_skill",
            "link_file_to_task",
        ] {
            assert!(
                !is_atomic_tool(name),
                "{name} 应该是单点（白名单），不应被判为原子"
            );
        }
    }

    /// 死函数 atomic_block_message 仍兼容未知工具名 fallback
    #[test]
    fn atomic_block_message_unknown_tool_fallback() {
        let msg = atomic_block_message("not_a_real_tool");
        assert!(msg.contains("not_a_real_tool"));
    }

    #[test]
    fn is_skill_active_false_with_no_skills() {
        // SKILL_RUNS 是全局 OnceLock，初始为空 → 未运行任何 Skill
        assert!(!is_skill_active(None));
        assert!(!is_skill_active(Some("s1")));
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
