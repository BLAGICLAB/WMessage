//! 工具调用守卫（老板 2026-08-17 18:14 拍板）
//!
//! ## 后置拦截（原子黑名单）
//! - 凡是「仅作为某个 Skill 内部子步骤、不允许用户直接独立调用」的底层原子 Function
//!   → 命中即阻断、返回提示走对应 Skill，**不交给模型**（硬锁）
//! - 黑名单内 Function 仅在 Skill 运行时（state == Running）放行
//!
//! ## 待办：前置预路由（Q1 关键词硬锁）
//! - 第二步再实现：从用户输入预判复合业务 → 直接 `start_skill` + 状态机 + Harness，
//!   LLM 仅作可选 L2 兜底，不作主力

/// 禁止 LLM 裸调的原子 Function 黑名单
/// （老板 2026-08-17 18:14 拍板：仅作 Skill 内部子步骤、不能独立调用的底层原子）
pub const ATOMIC_TOOLS: &[&str] = &[
    // Word 修订模式（Word 修订 Skill 内专用；用户应直接说「润色 Word」让调度器加载 Skill）
    "create_word_revisions",
    // Skill 末尾绑产物专用（任务卡执行 Skill / 文档生成 Skill 完成后绑定产物到任务卡）
    "link_file_to_task",
];

/// 是否在原子黑名单
pub fn is_atomic_tool(name: &str) -> bool {
    ATOMIC_TOOLS.iter().any(|&t| t == name)
}

/// 阻断时的错误消息（明确告诉 LLM/用户走对应 Skill）
pub fn atomic_block_message(name: &str) -> String {
    match name {
        "create_word_revisions" => "⚠️ create_word_revisions 是 Word 修订 Skill 的内部原子，不允许裸调。\n请直接说「润色 Word」「修订这个 Word」等指令，我会自动加载 Word 修订 Skill 来执行。".to_string(),
        "link_file_to_task" => "⚠️ link_file_to_task 是 Skill 末尾绑产物的内部原子，不允许裸调。\n产物绑回任务卡请通过：①执行任务卡（🤖 按钮）；②加载对应生成 Skill 完成自动流程。".to_string(),
        _ => format!("⚠️ {} 是内部原子 Function，不允许裸调。请走对应 Skill。", name),
    }
}

/// 当前是否有 Skill 在 Running 状态（决定原子工具是否放行）
///
/// 委托 `bot_skills::is_skill_active` 实现（穿透 SkillRun 状态访问）
pub fn is_skill_active() -> bool {
    crate::bot_skills::is_skill_active()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_blacklist_recognizes_create_word_revisions() {
        assert!(is_atomic_tool("create_word_revisions"));
    }

    #[test]
    fn atomic_blacklist_recognizes_link_file_to_task() {
        assert!(is_atomic_tool("link_file_to_task"));
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
            "bind_file",
            "search_tasks",
            "extract_document",
            "create_word",
            "create_excel",
            "create_ppt",
            "create_pdf",
            "run_python",
            "web_search",
            "fetch_url",
            "use_skill",
        ] {
            assert!(
                !is_atomic_tool(name),
                "{name} 应该是单点（白名单），不应被判为原子"
            );
        }
    }

    #[test]
    fn atomic_block_message_mentions_skill() {
        for name in ATOMIC_TOOLS {
            let msg = atomic_block_message(name);
            assert!(
                msg.contains("Skill") || msg.contains("skill"),
                "{name} 的拦截消息应提及 Skill 引导用户走 Skill；实际：{msg}"
            );
        }
    }

    #[test]
    fn atomic_block_message_unknown_tool_fallback() {
        let msg = atomic_block_message("not_a_real_tool");
        assert!(msg.contains("not_a_real_tool"));
        assert!(msg.contains("Skill"));
    }

    #[test]
    fn is_skill_active_false_with_no_skills() {
        // SKILL_RUNS 是全局 OnceLock，初始为空 → 未运行任何 Skill
        assert!(!is_skill_active());
    }
}
