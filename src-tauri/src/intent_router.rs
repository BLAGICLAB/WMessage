//! 前置意图路由（老板 2026-08-17 18:14 Q1 拍板）
//!
//! - **L1 关键词/正则硬锁**：用户首条消息触发复合业务 → 直接加载对应 Skill，LLM **不参与选择 Skill**
//! - 未命中 → 放行进 LLM（现有路径）
//! - LLM 语义 L2 留作可选兑底（本版暂不上，先用关键词跑稳）
//!
//! ## 复合业务 → Skill 映射（仅多步骤、带状态/确认/审计的流程才进 Skill）
//! | 用户输入关键字（正则） | 加载 Skill |
//! |---|---|
//! | 做/生成/搞 + ppt/幻灯片/演示(文稿) | `ppt-orchestra-skill` |
//! | 润色/修订 + word/文档，用修订模式 | `minimax-docx` |
//! | 做/生成 + excel/xlsx/表格 | `minimax-xlsx` |
//! | 做/生成 + pdf | `minimax-pdf` |
//! | 搜/搜索/查 + 联网 | `minimax-web-search` |
//! | 任务/事项 + 汇总/总结，汇总/总结 + 任务/事项 | `minimax-task-summary` |
//! | 归档/迁移/清理 + 文件/桌面 | `minimax-archive` |
//!
//! 简单查询（list_tasks / 跑一个脚本算 1+1 等）不进 Skill，由 LLM 单点白名单直接处理。

use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAction {
    /// 命中复合业务 → 加载指定 Skill 并直接进入 Skill 执行循环
    Skill(String),
    /// 未命中 → 放行进 LLM
    PassThrough,
}

pub struct IntentRule {
    /// 正则模式列表（每条 case-insensitive）；任一匹配即视为命中该 Skill
    pub patterns: &'static [&'static str],
    pub skill_name: &'static str,
}

pub const INTENT_RULES: &[IntentRule] = &[
    // PPT 生成（多步骤：大纲 → 版式 → 配色 → 文件输出）
    IntentRule {
        patterns: &[
            r"(?i)(做|生成|搞|需要|想).{0,25}(ppt|幻灯片|演示(文稿)?)",
            r"(?i)ppt模板",
            r"(?i)做个.{0,15}ppt",
        ],
        skill_name: "ppt-orchestra-skill",
    },
    // Word 修订（多步骤：EXTRACT → 润色 → create_word_revisions）
    IntentRule {
        patterns: &[
            r"(?i)(润色|修订).{0,15}(word|文档)",
            r"(?i)用修订模式",
            r"(?i)word.{0,10}(润色|修订)",
        ],
        skill_name: "minimax-docx",
    },
    // Excel 生成（多步骤：数据 → 公式 → 样式）
    IntentRule {
        patterns: &[r"(?i)(做|生成).{0,15}(excel|xlsx|表格)", r"(?i)做个表格"],
        skill_name: "minimax-xlsx",
    },
    // PDF 生成
    IntentRule {
        patterns: &[r"(?i)(做|生成).{0,15}pdf", r"(?i)做个pdf"],
        skill_name: "minimax-pdf",
    },
    // 联网搜索（多步骤：搜 → 抓 → 总结）
    IntentRule {
        patterns: &[r"(?i)(搜|搜索|查).{0,5}(一下|看|找|找)", r"(?i)联网搜索"],
        skill_name: "minimax-web-search",
    },
    // 任务汇总（多步骤：list → 分类 → 摘要输出）
    IntentRule {
        patterns: &[
            r"(?i)(任务|事项).{0,5}(汇总|总结)",
            r"(?i)(汇总|总结)(任务|事项)",
        ],
        skill_name: "minimax-task-summary",
    },
    // 归档迁移（多步骤：list → 过滤 → move → log）
    IntentRule {
        patterns: &[r"(?i)(归档|迁移|清理).{0,15}(文件|桌面)", r"(?i)归档迁移"],
        skill_name: "minimax-archive",
    },
];

struct CompiledRule {
    regexes: Vec<Regex>,
    skill_name: &'static str,
}

/// 进程启动时一次性编译所有规则的正则
static COMPILED_RULES: Lazy<Vec<CompiledRule>> = Lazy::new(|| {
    INTENT_RULES
        .iter()
        .map(|r| CompiledRule {
            regexes: r
                .patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            skill_name: r.skill_name,
        })
        .collect()
});

/// L1 正则前置路由：返回命中的 Skill 名，或 PassThrough
pub fn route_user_input(text: &str) -> RouteAction {
    for rule in COMPILED_RULES.iter() {
        for re in &rule.regexes {
            if re.is_match(text) {
                return RouteAction::Skill(rule.skill_name.to_string());
            }
        }
    }
    RouteAction::PassThrough
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_ppt_intent() {
        assert_eq!(
            route_user_input("帮我做一份 XX 主题的 PPT"),
            RouteAction::Skill("ppt-orchestra-skill".to_string())
        );
        assert_eq!(
            route_user_input("做个 PPT 模板"),
            RouteAction::Skill("ppt-orchestra-skill".to_string())
        );
        assert_eq!(
            route_user_input("做一份幻灯片，关于产品介绍"),
            RouteAction::Skill("ppt-orchestra-skill".to_string())
        );
        assert_eq!(
            route_user_input("做一个 PPT"),
            RouteAction::Skill("ppt-orchestra-skill".to_string())
        );
    }

    #[test]
    fn routes_word_revisions_intent() {
        assert_eq!(
            route_user_input("用修订模式润色这个 Word 文档"),
            RouteAction::Skill("minimax-docx".to_string())
        );
        assert_eq!(
            route_user_input("润色一下 Word"),
            RouteAction::Skill("minimax-docx".to_string())
        );
        assert_eq!(
            route_user_input("修订文档"),
            RouteAction::Skill("minimax-docx".to_string())
        );
    }

    #[test]
    fn routes_excel_intent() {
        assert_eq!(
            route_user_input("帮我生成 Excel"),
            RouteAction::Skill("minimax-xlsx".to_string())
        );
        assert_eq!(
            route_user_input("做个表格，关于 Q3 数据"),
            RouteAction::Skill("minimax-xlsx".to_string())
        );
    }

    #[test]
    fn routes_pdf_intent() {
        assert_eq!(
            route_user_input("做一份 PDF 报告"),
            RouteAction::Skill("minimax-pdf".to_string())
        );
    }

    #[test]
    fn routes_web_search_intent() {
        assert_eq!(
            route_user_input("搜一下今天天气"),
            RouteAction::Skill("minimax-web-search".to_string())
        );
        assert_eq!(
            route_user_input("查一下 XX 公司的最新财报"),
            RouteAction::Skill("minimax-web-search".to_string())
        );
    }

    #[test]
    fn routes_task_summary_intent() {
        assert_eq!(
            route_user_input("任务汇总"),
            RouteAction::Skill("minimax-task-summary".to_string())
        );
        assert_eq!(
            route_user_input("汇总任务"),
            RouteAction::Skill("minimax-task-summary".to_string())
        );
    }

    #[test]
    fn routes_archive_intent() {
        assert_eq!(
            route_user_input("归档迁移"),
            RouteAction::Skill("minimax-archive".to_string())
        );
        assert_eq!(
            route_user_input("清理桌面"),
            RouteAction::Skill("minimax-archive".to_string())
        );
    }

    #[test]
    fn passthrough_for_simple_queries() {
        assert_eq!(
            route_user_input("新建任务：买牛奶"),
            RouteAction::PassThrough
        );
        assert_eq!(
            route_user_input("和机器人说点什么"),
            RouteAction::PassThrough
        );
        assert_eq!(route_user_input("列出我的任务"), RouteAction::PassThrough);
        assert_eq!(
            route_user_input("用 python 算 1+1"),
            RouteAction::PassThrough
        );
    }

    #[test]
    fn passthrough_for_empty_string() {
        assert_eq!(route_user_input(""), RouteAction::PassThrough);
    }

    #[test]
    fn case_insensitive_matching() {
        // regex (?i) flag 处理大小写不敏感：英文输入如「PDF」→「pdf」能命中中文混合关键字
        assert_eq!(
            route_user_input("做一份 PDF 报告"),
            RouteAction::Skill("minimax-pdf".to_string())
        );
        assert_eq!(
            route_user_input("做一份 pdf 报告"),
            RouteAction::Skill("minimax-pdf".to_string())
        );
        // 不含 pdf 关键字的报告 → 不命中
        assert_eq!(
            route_user_input("做一份 txt 报告"),
            RouteAction::PassThrough
        );
    }
}
