//! Prompt 常量集中地（提示词 = AI 应用的业务逻辑，与代码同等对待）。
//!
//! 为什么单独成目录：
//! - 提示词直接决定模型行为，之前散在 `bot_chat.rs` / `bot_plan.rs` / `memory/consolidate.rs`
//!   各处行号里，改一条规则在大文件 diff 里很容易漏 review；集中后「这轮改了哪些提示词」
//!   一眼看全，每个文件一个用途
//! - 与运行时副作用解耦：本目录只有 `&'static str` 常量，无 IO、无状态、无 AppHandle
//!
//! 为什么是 `&'static str` 而不是 `.md` + `include_str!`：
//! - 现有字面量用 `\` 续行拼接（**行与行之间没有换行符**，整段是一条长文本）；
//!   换成 `.md` 会引入真实换行，等于顺手改了提示词字节内容——提示词是模型输入，
//!   这种改动需要单独评估 + 人工验收，不该混进「纯提取」改动
//! - 保持「编译期内嵌常量」语义：不引入运行时路径解析，也就不必在
//!   `tauri.conf.json` 登记 resources
//! 将来若要把提示词当资源管（`.md` + `include_str!`），只需在本目录替换实现，
//! 调用方 `crate::prompts::NAME` 不变（编译期内嵌这一点必须保持）。
//!
//! 调用约定：统一从 `crate::prompts::NAME` 取，不直接引子模块。

mod consolidate;
mod execute;
mod planner;
mod reflection;
mod summary;
mod system;

pub(crate) use consolidate::CONSOLIDATE_PROMPT;
pub(crate) use execute::{EXECUTE_SYSTEM_PROMPT, STEPWISE_ADDENDUM};
pub(crate) use planner::{PLANNER_PROMPT, REPLANNER_PROMPT};
pub(crate) use reflection::REFLECTION_SYSTEM_PROMPT;
pub(crate) use summary::{COMPACT_SYSTEM_PROMPT, SUMMARY_SYSTEM_PROMPT};
pub(crate) use system::SYSTEM_PROMPT;

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部 prompt 的单一清单：新增常量必须登记（否则漏出下面的通用断言）
    const ALL: &[(&str, &str)] = &[
        ("SYSTEM_PROMPT", SYSTEM_PROMPT),
        ("SUMMARY_SYSTEM_PROMPT", SUMMARY_SYSTEM_PROMPT),
        ("COMPACT_SYSTEM_PROMPT", COMPACT_SYSTEM_PROMPT),
        ("REFLECTION_SYSTEM_PROMPT", REFLECTION_SYSTEM_PROMPT),
        ("EXECUTE_SYSTEM_PROMPT", EXECUTE_SYSTEM_PROMPT),
        ("STEPWISE_ADDENDUM", STEPWISE_ADDENDUM),
        ("PLANNER_PROMPT", PLANNER_PROMPT),
        ("REPLANNER_PROMPT", REPLANNER_PROMPT),
        ("CONSOLIDATE_PROMPT", CONSOLIDATE_PROMPT),
    ];

    #[test]
    fn all_prompts_are_registered_and_non_empty() {
        assert_eq!(ALL.len(), 9, "prompt 清单与实际常量数不一致（新增请登记）");
        for (name, p) in ALL {
            assert!(
                p.chars().count() >= 30,
                "{name} 过短（疑似抽取时被截断）: {p}"
            );
            assert!(!p.trim().is_empty(), "{name} 为空");
        }
    }

    /// 主聊天提示词的规则底座与安全红线不能丢（删规则=改产品行为，必须显式过 review）
    #[test]
    fn system_prompt_keeps_core_rules_and_safety_redlines() {
        assert!(SYSTEM_PROMPT.contains("规则："), "规则段缺失");
        for anchor in [
            "create_task",
            "list_tasks",
            "complete_task",
            "link_file_to_task",
            "web_search",
            "AI_Gen_Files",
        ] {
            assert!(
                SYSTEM_PROMPT.contains(anchor),
                "SYSTEM_PROMPT 缺锚点 {anchor}"
            );
        }
        assert!(SYSTEM_PROMPT.contains("绝不"), "安全红线段缺失");
    }

    /// 任务卡执行口径与 dispatch / bot_slash 的放行逻辑耦合：
    /// link_file_to_task 是收尾动作、complete_task 只在手动执行时调用
    #[test]
    fn execute_prompt_keeps_task_card_contract() {
        assert!(EXECUTE_SYSTEM_PROMPT.contains("link_file_to_task"));
        assert!(EXECUTE_SYSTEM_PROMPT.contains("complete_task"));
        assert!(
            STEPWISE_ADDENDUM.contains("以本段为准"),
            "逐步执行附加段必须写明冲突优先级（它拼在 EXECUTE 之后，语义最靠后）"
        );
    }

    /// 截断摘要比 /compact 更紧（它常驻历史开头）：字数上限是设计口径，别改混
    #[test]
    fn summary_and_compact_word_budgets_are_distinct() {
        assert!(
            SUMMARY_SYSTEM_PROMPT.contains("200 字"),
            "截断摘要应 ≤200 字"
        );
        assert!(
            COMPACT_SYSTEM_PROMPT.contains("300 字"),
            "/compact 摘要应 ≤300 字"
        );
    }

    /// Planner 两段都必须约束「只输出 JSON」——解析失败即降级，措辞是唯一防线
    #[test]
    fn planner_prompts_demand_json_only() {
        for (name, p) in [
            ("PLANNER_PROMPT", PLANNER_PROMPT),
            ("REPLANNER_PROMPT", REPLANNER_PROMPT),
        ] {
            assert!(p.contains("JSON"), "{name} 缺 JSON 约束");
            assert!(p.contains("只输出"), "{name} 缺「只输出」约束");
        }
    }
}
