//! 前置意图路由（路由表由已安装技能的 intents 声明驱动）
//!
//! ## 规则
//! - **未安装的技能不得有路由**：路由条目 = 已安装技能 SKILL.md frontmatter 的 `intents` 声明
//! - 安装技能（设置页导入等任何入口）→ `rebuild_intent_routes` 重建路由表，该技能路由即生效
//! - 卸载技能 → 重建路由表，相关条目随之移除
//! - 路由表重建时机：app 启动一次 + `skills_import` / `skills_delete` 成功后
//!
//! ## 运行语义
//! - **L1 正则硬锁**：用户首条消息命中某技能的 intent 模式 → 直接加载该 Skill，LLM **不参与选择 Skill**
//! - **选择任务卡批量执行**：`is_chat_execute_trigger`（「完成/执行」关键词 + [已选任务] 引用块）
//!   由 middleware 的 ChatExecuteMiddleware 包装为 `RouteAction::ExecuteTasks`，是 bot_chat 主流程
//!   pre-step 路由的一个分支
//! - 未命中 / 路由表为空（未初始化、无已安装技能声明 intents）→ 放行进 LLM（现有路径）
//! - 模式按正则匹配用户消息全文（含 [附件文件] 块内嵌的附件路径——附件上下文规则直接写进模式，
//!   如 `(?is)(润色|修订)[\s\S]*\.docx?`），大小写敏由模式内联 `(?i)` 控制

use regex::Regex;
use std::sync::LazyLock;
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteAction {
    /// 命中复合业务 → 加载指定 Skill 并直接进入 Skill 执行循环
    Skill(String),
    /// 选择任务卡模式：「完成/执行」类关键词 + [已选任务] 引用块 → 批量执行这些任务卡
    /// （chat_execute_tasks 复用 run_task_in_chat 整卡连续执行，路由终态由 bot_chat 主流程处理）
    ExecuteTasks(Vec<(String, String)>),
    /// 未命中 → 放行进 LLM
    PassThrough,
}

impl RouteAction {
    /// 审计 kv：`action` 是稳定短名（`grep 'action=skill'` 统计中间件命中率），
    /// `detail` 供人读（技能名 / 命中任务数）。
    /// PassThrough 也给值——它同样是一次「中间件命中」（IntentRouter 恒短路），
    /// 没有它就算不出真实命中率。
    pub fn audit_kv(&self) -> (&'static str, String) {
        match self {
            RouteAction::Skill(name) => ("skill", name.clone()),
            RouteAction::ExecuteTasks(tasks) => ("execute_tasks", format!("tasks={}", tasks.len())),
            RouteAction::PassThrough => ("pass_through", String::new()),
        }
    }
}

/// 解析 [已选任务] 引用块：`[已选任务]\n- id=xxx，标题=yyy` 列表。
/// 支持中英文逗号、`id=` / `title=`（英文），跳过格式破损行（id 缺失、空 id 等）。
/// 剥出便于单测：前端发送格式由 ChatPanel.tsx 拼装，破损行（手改、复制粘贴半截）不能污染解析。
pub fn parse_selected_tasks_block(content: &str) -> Vec<(String, String)> {
    let Some(idx) = content.find("[已选任务]") else {
        return Vec::new();
    };
    let after = &content[idx + "[已选任务]".len()..];
    let mut out = Vec::new();
    for line in after.lines() {
        let line = line.trim();
        if !line.starts_with("- ") {
            continue;
        }
        let body = line[2..].trim();
        // 优先按中文逗号切，否则英文逗号
        let (id_part, title_part) = if let Some(p) = body.split_once('，') {
            (p.0, p.1)
        } else if let Some(p) = body.split_once(',') {
            (p.0, p.1)
        } else {
            continue;
        };
        let id = match id_part.trim().strip_prefix("id=") {
            Some(s) => s.trim(),
            None => continue,
        };
        // 标题：中文「标题=」或英文「title=」都要识别
        let title = title_part
            .trim()
            .strip_prefix("标题=")
            .or_else(|| title_part.trim().strip_prefix("title="))
            .unwrap_or(title_part.trim())
            .trim();
        if id.is_empty() {
            continue;
        }
        out.push((id.to_string(), title.to_string()));
    }
    out
}

/// 聊天模式触发「批量执行」前置判定：用户最近消息是否有 [已选任务] 引用块 + 关键词。
/// 关键词集合：宽松（完成/执行/搞定/开干/做掉/go/run/do），口语化场景都覆盖。
/// 顺序敏感：必须先有引用块再识别关键词（避免「step 1: 用 [已选任务] 块修复 x」类教程消息误触发）。
pub fn is_chat_execute_trigger(content: &str) -> Option<Vec<(String, String)>> {
    let tasks = parse_selected_tasks_block(content);
    if tasks.is_empty() {
        return None;
    }
    // 截取 [已选任务] 之前的「用户指令」段（关键词识别只看这部分，避免教程片段误触发）
    let user_cmd = content
        .split("[已选任务]")
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    const KEYWORDS: &[&str] = &["完成", "执行", "搞定", "开干", "做掉", "go", "run", "do"];
    if !KEYWORDS.iter().any(|kw| user_cmd.contains(kw)) {
        return None;
    }
    Some(tasks)
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
            regexes: r
                .patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
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
static ROUTES: LazyLock<RwLock<Vec<CompiledRule>>> = LazyLock::new(|| RwLock::new(Vec::new()));

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

    // ── RouteAction::audit_kv：中间件命中率统计的稳定口径 ──

    #[test]
    fn audit_kv_names_are_stable_and_detail_is_human_readable() {
        assert_eq!(
            RouteAction::Skill("minimax-docx".into()).audit_kv(),
            ("skill", "minimax-docx".to_string())
        );
        assert_eq!(
            RouteAction::ExecuteTasks(vec![("a".into(), "标题A".into())]).audit_kv(),
            ("execute_tasks", "tasks=1".to_string())
        );
        // PassThrough 也必须给 action 值——它是 IntentRouter 恒短路的「命中」，
        // 缺了它命中率统计的分母就错了
        assert_eq!(
            RouteAction::PassThrough.audit_kv(),
            ("pass_through", String::new())
        );
    }

    fn rule(name: &str, patterns: &[&str]) -> IntentRule {
        IntentRule {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            skill_name: name.to_string(),
        }
    }

    /// minimax-docx 已安装 SKILL.md 的 intents 声明：与其 frontmatter 同源，
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
        for input in ["用修订模式润色这个 Word 文档", "润色一下 Word", "修订文档"]
        {
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
        // 核心断言：未安装 = 不在规则集 = 无路由。
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
        let rules = vec![rule("bad-skill", &[r"(?i)unclosed(", r"(?i)valid-pattern"])];
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

/// 聊天模式批量执行触发解析（pre-step 路由的 ExecuteTasks 分支）
#[cfg(test)]
mod chat_execute_parse_tests {
    use super::*;

    #[test]
    fn parse_block_extracts_id_and_title_chinese_comma() {
        let content =
            "完成这些\n\n[已选任务]\n- id=abc-123，标题=写 PPT\n- id=def-456，标题=分析销售数据";
        let parsed = parse_selected_tasks_block(content);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, "abc-123");
        assert_eq!(parsed[0].1, "写 PPT");
        assert_eq!(parsed[1].0, "def-456");
        assert_eq!(parsed[1].1, "分析销售数据");
    }

    #[test]
    fn parse_block_handles_english_comma_and_title() {
        let content = "[已选任务]\n- id=abc, title=Test";
        let parsed = parse_selected_tasks_block(content);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "abc");
        assert_eq!(parsed[0].1, "Test");
    }

    #[test]
    fn parse_block_skips_malformed_lines() {
        let content =
            "[已选任务]\n- id=abc，标题=Good\n- garbage line\n- id=, 标题=Empty\n- 标题=NoId";
        let parsed = parse_selected_tasks_block(content);
        assert_eq!(parsed.len(), 1, "只有格式完好的 1 条");
        assert_eq!(parsed[0].0, "abc");
        assert_eq!(parsed[0].1, "Good");
    }

    #[test]
    fn parse_block_returns_empty_when_no_marker() {
        let content = "普通消息，没有 [已选任务] 块";
        let parsed = parse_selected_tasks_block(content);
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_block_handles_block_at_start() {
        let content = "[已选任务]\n- id=x，标题=Y";
        let parsed = parse_selected_tasks_block(content);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "x");
        assert_eq!(parsed[0].1, "Y");
    }

    #[test]
    fn trigger_requires_keyword_and_block() {
        // 有块有关键词 → 命中
        let hit = is_chat_execute_trigger("完成\n\n[已选任务]\n- id=a，标题=b");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().len(), 1);

        // 有块无关键词 → 不命中
        assert!(is_chat_execute_trigger("看看这些\n\n[已选任务]\n- id=a，标题=b").is_none());

        // 无块有关键词 → 不命中
        assert!(is_chat_execute_trigger("完成任务").is_none());

        // 完全无关 → 不命中
        assert!(is_chat_execute_trigger("你好世界").is_none());
    }

    #[test]
    fn trigger_matches_all_keyword_variants() {
        for kw in &["完成", "执行", "搞定", "开干", "做掉", "go", "run", "do"] {
            let content = format!("{}一下\n\n[已选任务]\n- id=a，标题=b", kw);
            assert!(
                is_chat_execute_trigger(&content).is_some(),
                "关键词 {} 应命中",
                kw
            );
        }
    }

    #[test]
    fn trigger_keyword_only_looks_before_block() {
        // 关键词在 [已选任务] 之后（如教程片段）不触发
        let content = "[已选任务]\n- id=a，标题=完成后才执行";
        assert!(is_chat_execute_trigger(content).is_none());
    }

    #[test]
    fn trigger_keyword_case_insensitive() {
        assert!(is_chat_execute_trigger("GO\n\n[已选任务]\n- id=a，标题=b").is_some());
        assert!(is_chat_execute_trigger("Run\n\n[已选任务]\n- id=a，标题=b").is_some());
    }

    #[test]
    fn trigger_extracts_multiple_tasks() {
        let content = "执行\n\n[已选任务]\n- id=a，标题=A\n- id=b，标题=B\n- id=c，标题=C";
        let parsed = is_chat_execute_trigger(content).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].0, "a");
        assert_eq!(parsed[1].0, "b");
        assert_eq!(parsed[2].0, "c");
    }

    #[test]
    fn trigger_maps_to_execute_tasks_route_action() {
        // 收编主流程后的路由契约：触发命中 → RouteAction::ExecuteTasks（不再是 bot_chat 前置短路）
        let tasks = is_chat_execute_trigger("完成\n\n[已选任务]\n- id=a，标题=A").unwrap();
        let action = RouteAction::ExecuteTasks(tasks);
        match action {
            RouteAction::ExecuteTasks(ids) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0].0, "a");
            }
            other => panic!("应为 ExecuteTasks，得到 {other:?}"),
        }
    }
}
