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
mod extract;
mod planner;
mod selfcheck;
mod subagent;
mod summary;
mod system;

pub(crate) use consolidate::CONSOLIDATE_PROMPT;
pub(crate) use execute::{EXECUTE_SYSTEM_PROMPT, STEPWISE_ADDENDUM};
pub(crate) use extract::{ADJUDICATE_PROMPT, EXTRACT_PROMPT};
pub(crate) use planner::{PLANNER_PROMPT, REPLANNER_PROMPT};
pub(crate) use selfcheck::{
    parse_selfcheck_reply_lenient, parse_selfcheck_response, FINAL_SELFCHECK_PROMPT,
    SELFCHECK_PROMPT,
};
pub(crate) use subagent::{
    MAIN_AGENT_ADDENDUM, PROFILE_CODER, PROFILE_EXECUTOR, PROFILE_GENERAL, PROFILE_RECON,
    PROFILE_RESEARCH, PROFILE_VERIFIER, RESULT_SCHEMA_HINT, SUBAGENT_BASE,
};
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
        ("SELFCHECK_PROMPT", SELFCHECK_PROMPT),
        ("EXECUTE_SYSTEM_PROMPT", EXECUTE_SYSTEM_PROMPT),
        ("STEPWISE_ADDENDUM", STEPWISE_ADDENDUM),
        ("PLANNER_PROMPT", PLANNER_PROMPT),
        ("REPLANNER_PROMPT", REPLANNER_PROMPT),
        ("CONSOLIDATE_PROMPT", CONSOLIDATE_PROMPT),
        ("EXTRACT_PROMPT", EXTRACT_PROMPT),
        ("ADJUDICATE_PROMPT", ADJUDICATE_PROMPT),
        ("MAIN_AGENT_ADDENDUM", MAIN_AGENT_ADDENDUM),
        ("SUBAGENT_BASE", SUBAGENT_BASE),
        ("PROFILE_RESEARCH", PROFILE_RESEARCH),
        ("PROFILE_CODER", PROFILE_CODER),
        ("PROFILE_GENERAL", PROFILE_GENERAL),
        ("PROFILE_RECON", PROFILE_RECON),
        ("PROFILE_EXECUTOR", PROFILE_EXECUTOR),
        ("PROFILE_VERIFIER", PROFILE_VERIFIER),
        ("RESULT_SCHEMA_HINT", RESULT_SCHEMA_HINT),
    ];

    #[test]
    fn all_prompts_are_registered_and_non_empty() {
        assert_eq!(ALL.len(), 20, "prompt 清单与实际常量数不一致（新增请登记）");
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
            "query_tasks",
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

    /// 子 agent 的预算纪律三条红线（整读 / 打转自检 / 产物早落盘）——实机踩过
    /// 「173 行文件碎读 91 次烧满预算零产出」的坑，删红线=复发，必须显式过 review。
    #[test]
    fn subagent_base_keeps_budget_redlines() {
        for anchor in ["≤500 行一次读完", "白烧预算", "原地打转", "先写骨架"] {
            assert!(
                SUBAGENT_BASE.contains(anchor),
                "SUBAGENT_BASE 预算红线缺锚点「{anchor}」"
            );
        }
    }

    /// camelCase → snake_case（只处理小写字母/数字接大写字母这一处断点）。
    fn to_snake(name: &str) -> String {
        let mut out = String::with_capacity(name.len() + 4);
        for (i, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        }
        out
    }

    /// 提示词里点名工具入参时，**必须用 schema 里的属性名**。
    ///
    /// 模型同时收到两样东西：tools 载荷里的 schema（属性名 camelCase）与本目录的
    /// 提示词正文。两边拼法不一致时模型可能任选一边，而入参解析是裸
    /// `v["<属性名>"]` 取值（无 snake→camel 归一）——照提示词写就静默丢参：
    /// `waitMs` 丢成不等待、`contextSummary` 丢成空背景，而
    /// `acceptanceCriteria` 丢成「必填参数缺失」直接 spawn 失败。
    ///
    /// 这条锁挡住的就是那三处：subagent 提示词曾教模型写 snake_case，而 schema 与
    /// 解析侧都是 camelCase，且同文件里 `read_own_card` 那段又写的是 camelCase——
    /// 门禁全绿、模型两头猜。
    #[test]
    fn prompts_never_spell_schema_params_as_snake_case() {
        let tools: serde_json::Value = serde_json::from_str(crate::bot::registry::tools_json())
            .expect("tools_json 必须是合法 JSON");
        let mut offenders = Vec::new();
        for t in tools.as_array().into_iter().flatten() {
            let fname = t["function"]["name"].as_str().unwrap_or("?");
            let Some(props) = t["function"]["parameters"]["properties"].as_object() else {
                continue;
            };
            for prop in props.keys() {
                // 单段全小写的属性名两种拼法同形，无需检查
                if !prop.chars().any(|c| c.is_ascii_uppercase()) {
                    continue;
                }
                let snake = to_snake(prop);
                for (name, prompt) in ALL {
                    // 整词匹配，且只判 ASCII：中文紧贴标识符是正常写法
                    //（「写 contextSummary 时…」），按 Unicode 的 alphanumeric 判会
                    // 把这类整段当成「不在词内」而漏报——锁必须比 bug 更严。
                    let hit = prompt.match_indices(&snake).any(|(i, _)| {
                        let before = prompt[..i].chars().next_back();
                        let after = prompt[i + snake.len()..].chars().next();
                        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    });
                    if hit {
                        offenders.push(format!(
                            "{name} 用了 {snake}（schema 属性名是 {prop}，工具 {fname}）"
                        ));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "提示词用 snake_case 拼 schema 入参，模型照抄即静默丢参：\n{}",
            offenders.join("\n")
        );
    }
}
