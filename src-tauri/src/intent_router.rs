//! 前置意图路由（2026-08-19 重构：静态表 → 安装声明驱动）
//!
//! ## 规则（老板拍板 2026-08-19）
//! - **未安装的技能不得有路由**：路由条目 = 已安装技能 SKILL.md frontmatter 的 `intents` 声明
//! - 安装技能（设置页导入等任何入口）→ `rebuild_intent_routes` 重建路由表，该技能路由即生效
//! - 卸载技能 → 重建路由表，相关条目随之移除
//! - 路由表重建时机：app 启动一次 + `skills_import` / `skills_delete` 成功后
//!
//! ## 运行语义（沿用 2026-08-17 Q1 拍板）
//! - **L1 正则硬锁**：用户首条消息命中某技能的 intent 模式 → 直接加载该 Skill，LLM **不参与选择 Skill**
//! - 未命中 / 路由表为空（未初始化、无已安装技能声明 intents）→ 放行进 LLM（现有路径）
//! - 模式按正则匹配用户消息全文（含 [附件文件] 块内嵌的附件路径——附件上下文规则直接写进模式，
//!   如 `(?is)(润色|修订)[\s\S]*\.docx?`），大小写敏由模式内联 `(?i)` 控制

use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAction {
    /// 命中复合业务 → 加载指定 Skill 并直接进入 Skill 执行循环
    Skill(String),
    /// 未命中 → 放行进 LLM
    PassThrough,
}

/// 一条路由规则：技能名 + 该技能 frontmatter `intents` 声明的正则模式列表。
/// 任一模式匹配即路由到该技能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRule {
    pub patterns: Vec<String>,
    pub skill_name: String,
}

struct CompiledRule {
    regexes: Vec<Regex>,
    skill_name: String,
}

fn compile_rules(rules: &[IntentRule]) -> Vec<CompiledRule> {
    rules
        .iter()
        .map(|r| CompiledRule {
            // 非法正则跳过（技能作者写错不拖垮整张表）
            regexes: r.patterns.iter().filter_map(|p| Regex::new(p).ok()).collect(),
            skill_name: r.skill_name.clone(),
        })
        .filter(|r| !r.regexes.is_empty())
        .collect()
}

fn match_compiled(text: &str, rules: &[CompiledRule]) -> RouteAction {
    for rule in rules {
        for re in &rule.regexes {
            if re.is_match(text) {
                return RouteAction::Skill(rule.skill_name.clone());
            }
        }
    }
    RouteAction::PassThrough
}

/// 进程级路由表：启动时为空（空表 = 全量 PassThrough，fail-open 与 middleware 业务路由语义一致），
/// 由 `bot_skills::rebuild_intent_routes` 在启动 / 导入 / 删除技能后重建。
static ROUTES: Lazy<RwLock<Vec<CompiledRule>>> = Lazy::new(|| RwLock::new(Vec::new()));

/// 重建全局路由表（入参来自已安装技能扫描；RwLock 写中毒兜底取 inner，不让路由整体崩掉）
pub fn rebuild_routes(rules: Vec<IntentRule>) {
    let compiled = compile_rules(&rules);
    match ROUTES.write() {
        Ok(mut guard) => *guard = compiled,
        Err(e) => *e.into_inner() = compiled,
    }
}

/// L1 前置路由（生产路径）：读全局路由表
pub fn route_user_input(text: &str) -> RouteAction {
    let guard = match ROUTES.read() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match_compiled(text, &guard)
}

/// 纯函数核：规则注入式匹配。测试主入口（不碰全局表，无并行污染）；
/// 也可用于「这份规则集会产生什么路由」的离线核验。
pub fn route_with_rules(text: &str, rules: &[IntentRule]) -> RouteAction {
    match_compiled(text, &compile_rules(rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, patterns: &[&str]) -> IntentRule {
        IntentRule {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            skill_name: name.to_string(),
        }
    }

    /// minimax-docx 已安装 SKILL.md 的 intents 声明（2026-08-19）：与其 frontmatter 同源，
    /// 改一边必须同步另一边（frontmatter 才是真相，这里只是测试副本）
    fn docx_rules() -> Vec<IntentRule> {
        vec![rule(
            "minimax-docx",
            &[
                r"(?i)(润色|修订).{0,15}(word|文档)",
                r"(?i)用修订模式",
                r"(?i)word.{0,10}(润色|修订)",
                r"(?is)(润色|修订)[\s\S]*\.docx?",
                r"(?is)\.docx?[\s\S]*(润色|修订)",
            ],
        )]
    }

    #[test]
    fn routes_word_revisions_intent() {
        let rules = docx_rules();
        for input in [
            "用修订模式润色这个 Word 文档",
            "润色一下 Word",
            "修订文档",
        ] {
            assert_eq!(
                route_with_rules(input, &rules),
                RouteAction::Skill("minimax-docx".to_string()),
                "输入「{input}」应命中 minimax-docx"
            );
        }
    }

    #[test]
    fn routes_word_revisions_by_attachment_context() {
        // 附件以 [附件文件] 路径块嵌在消息文本里（ChatPanel 拼装），
        // 单说「润色/修订」+ 带 .doc/.docx 附件 → minimax-docx
        let rules = docx_rules();
        for input in [
            "[附件文件]\n- /tmp/汇报稿.docx\n\n润色一下",
            "[附件文件]\n- /tmp/a.DOCX\n- /tmp/b.png\n\n帮我修订",
            "[附件文件]\n- /Users/x/old.doc\n\n润色",
        ] {
            assert_eq!(
                route_with_rules(input, &rules),
                RouteAction::Skill("minimax-docx".to_string()),
                "输入「{input}」应命中 minimax-docx"
            );
        }
        // 裸关键词无附件 → 不命中（无法判断目标是 Word 还是普通文字）
        assert_eq!(
            route_with_rules("润色一下这段话", &rules),
            RouteAction::PassThrough
        );
        // 附件类型不符 → 不命中
        assert_eq!(
            route_with_rules("[附件文件]\n- /tmp/截图.png\n\n润色一下", &rules),
            RouteAction::PassThrough
        );
    }

    #[test]
    fn uninstalled_skill_has_no_route() {
        // 新规则核心断言（2026-08-19 老板拍板）：未安装 = 不在规则集 = 无路由。
        // PPT/Excel/PDF/联网搜索/任务汇总/归档 均未安装 → 全部 PassThrough 进 LLM 单点路径
        let rules = docx_rules(); // 只装了 docx
        for input in [
            "帮我做一个 PPT",
            "生成 Excel 表格",
            "做一份 PDF 报告",
            "搜一下今天天气",
            "任务汇总",
            "归档迁移",
        ] {
            assert_eq!(
                route_with_rules(input, &rules),
                RouteAction::PassThrough,
                "未安装技能的输入「{input}」不应有路由"
            );
        }
        // 空表（什么都没装/未初始化）→ 恒 PassThrough
        assert_eq!(
            route_with_rules("用修订模式润色 Word", &[]),
            RouteAction::PassThrough
        );
    }

    #[test]
    fn invalid_regex_pattern_is_skipped() {
        let rules = vec![rule(
            "bad-skill",
            &[r"(?i)unclosed(", r"(?i)valid-pattern"],
        )];
        // 非法模式被跳过，合法模式仍生效
        assert_eq!(
            route_with_rules("hit valid-pattern here", &rules),
            RouteAction::Skill("bad-skill".to_string())
        );
    }

    #[test]
    fn first_matching_rule_wins() {
        let rules = vec![
            rule("skill-a", &[r"(?i)foo"]),
            rule("skill-b", &[r"(?i)foo"]),
        ];
        assert_eq!(
            route_with_rules("foo", &rules),
            RouteAction::Skill("skill-a".to_string())
        );
    }

    #[test]
    fn global_table_rebuild_and_route() {
        // 全局表读写路径冒烟：用独一无二的探针模式，不与任何并行测试的输入交叉
        rebuild_routes(vec![rule("zz-router-probe", &[r"zzRouterProbePattern"])]);
        assert_eq!(
            route_user_input("zzRouterProbePattern"),
            RouteAction::Skill("zz-router-probe".to_string())
        );
        assert_eq!(route_user_input("无关输入"), RouteAction::PassThrough);
        // 还原空表，不污染其他测试
        rebuild_routes(vec![]);
        assert_eq!(
            route_user_input("zzRouterProbePattern"),
            RouteAction::PassThrough
        );
    }
}
