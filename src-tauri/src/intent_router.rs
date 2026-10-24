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
//!   由 bot_chat 主流程 pre-step 路由的第一步消费（`RouteAction::ExecuteTasks`），是 bot_chat 主流程
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

/// [已选任务] 引用块标记（与前端 ChatPanel.tsx 拼装格式逐字对齐）
const TASK_BLOCK_MARKER: &str = "[已选任务]";
/// [附件文件] 块标记（引用块可以跟在它后面）
const ATTACH_BLOCK_MARKER: &str = "[附件文件]";

/// 拆消息头的引用块：返回（块里的任务、块之后的用户正文）。
///
/// 消息头的结构由前端拼装，顺序固定：`[附件文件]`? → `[已选任务]`? → 用户正文。
/// 引用块本体是连续的 `- ` 行，**到第一行非 `- ` 为止**——正文里形似
/// `- id=…，标题=…` 的行（粘贴的日志、教程片段、手写的说明）不是引用块。
///
/// **只认消息头**：标记出现在正文中间一律不认。引用块命中意味着「把选中的
/// 任务卡全部自主执行一遍」，认错一次就是替用户执行了他没打算执行的卡。
fn split_task_block(content: &str) -> (Vec<(String, String)>, &str) {
    let rest = strip_attachment_block(content.trim_start());
    let Some(body) = after_marker_line(rest, TASK_BLOCK_MARKER) else {
        return (Vec::new(), "");
    };
    let mut tasks = Vec::new();
    // 块体（含块尾空行）已消费到的位置：用户正文从它后面开始
    let mut end = 0usize;
    for line in body.split_inclusive('\n') {
        let t = line.trim();
        if t.is_empty() {
            // 块尾空行是分隔符
            end += line.len();
            break;
        }
        if !t.starts_with("- ") {
            break;
        }
        end += line.len();
        if let Some(row) = parse_task_row(t) {
            tasks.push(row);
        }
    }
    (tasks, body[end..].trim())
}

/// 去掉标记所在行的行尾：标记后要么换行、要么直接跟内容。
/// 标记不在开头（内容前面还有别的字）时返回 None。
fn after_marker_line<'a>(content: &'a str, marker: &str) -> Option<&'a str> {
    let rest = content.strip_prefix(marker)?;
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    Some(rest.strip_prefix('\n').unwrap_or(rest))
}

/// 剥掉消息头的 [附件文件] 块（若有）。
///
/// 块 = 标记 + 连续的 `- ` 行（路径行）+ 其后的空行。形态不符就原样返回——
/// 格式破损不该把整条消息的解析一起带坏。
fn strip_attachment_block(content: &str) -> &str {
    let Some(rest) = after_marker_line(content, ATTACH_BLOCK_MARKER) else {
        return content;
    };
    let mut end = 0usize;
    for line in rest.split_inclusive('\n') {
        let t = line.trim();
        if t.is_empty() {
            return &rest[end + line.len()..];
        }
        if !t.starts_with("- ") {
            return content;
        }
        end += line.len();
    }
    &rest[end..]
}

/// 解析引用块的一行：`- id=xxx，标题=yyy`。格式破损返回 None——
/// 跳过该行但不终止块（块是连续的 `- ` 行，坏行不等于块结束）。
fn parse_task_row(line: &str) -> Option<(String, String)> {
    let body = line.trim().strip_prefix("- ")?.trim();
    // 优先按中文逗号切，否则英文逗号
    let (id_part, title_part) = body.split_once('，').or_else(|| body.split_once(','))?;
    let id = id_part.trim().strip_prefix("id=")?.trim();
    if id.is_empty() {
        return None;
    }
    // 标题：中文「标题=」或英文「title=」都要识别
    let title = title_part
        .trim()
        .strip_prefix("标题=")
        .or_else(|| title_part.trim().strip_prefix("title="))
        .unwrap_or(title_part.trim())
        .trim();
    Some((id.to_string(), title.to_string()))
}

/// 解析 [已选任务] 引用块：`[已选任务]\n- id=xxx，标题=yyy` 列表。
/// 支持中英文逗号、`id=` / `title=`（英文），跳过格式破损行（id 缺失、空 id 等）。
/// 剥出便于单测：前端发送格式由 ChatPanel.tsx 拼装，破损行（手改、复制粘贴半截）不能污染解析。
pub fn parse_selected_tasks_block(content: &str) -> Vec<(String, String)> {
    split_task_block(content).0
}

/// 聊天模式触发「批量执行」前置判定：用户最近消息是否有 [已选任务] 引用块 + 关键词。
/// 关键词集合：宽松（完成/执行/搞定/开干/做掉/go/run/do），口语化场景都覆盖。
/// 顺序敏感：引用块必须来自消息头（应用自己拼的），触发词来自块之后的用户正文——
/// 前端拼装顺序是「块在前、正文在后」，块之前只有附件块（里面是路径，不是命令）。
pub fn is_chat_execute_trigger(content: &str) -> Option<Vec<(String, String)>> {
    let (tasks, user_text) = split_task_block(content);
    if tasks.is_empty() {
        return None;
    }
    let user_cmd = user_text.to_lowercase();
    const KEYWORDS: &[&str] = &["完成", "执行", "搞定", "开干", "做掉", "go", "run", "do"];
    if !KEYWORDS.iter().any(|kw| keyword_hit(&user_cmd, kw)) {
        return None;
    }
    Some(tasks)
}

/// 触发词命中（纯函数，便于单测）：
///
/// - CJK 词按子串命中——中文没有词边界，「这些都完成」就该触发。
/// - ASCII 词必须是命令的**第一个词**且右边界干净：go/run/do 单独成词太容易
///   撞进英文句子（download、google 含 go；cargo run 的 run 不是首词），
///   而命中意味着把选中卡**全部按自主执行跑一遍**——误判代价是用户只想聊天
///   却开跑了。宁可漏触发：没触发时主流程按规则 1-16 正常处理，
///   且系统提示词明令模型不得自行声称已开始批量执行。
fn keyword_hit(user_cmd: &str, kw: &str) -> bool {
    if !kw.is_ascii() {
        return user_cmd.contains(kw);
    }
    let Some(rest) = user_cmd.strip_prefix(kw) else {
        return false;
    };
    match rest.as_bytes().first() {
        None => true,
        // 多字节 UTF-8 首字节 ≥ 0x80，不落 ASCII 分支——「go一下」照样算边界
        Some(&b) => !(b.is_ascii_alphanumeric() || b == b'_'),
    }
}

/// 一条路由规则：技能名 + 该技能 frontmatter `intents` 声明的正则模式列表。
/// 任一模式匹配即路由到该技能。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRule {
    pub patterns: Vec<String>,
    pub skill_name: String,
    /// 会话档：Some(session_id) = 仅该会话内可见（模型 create_skill 建的技能——
    /// 本会话立即生效、跨会话不劫持，随进程消亡不落任何持久层）；
    /// None = 全局档（用户 install / import / 设置页导入的技能，有人工确认背书）。
    pub session: Option<String>,
}

/// 技能名的「点名」路由正则：正文里出现技能名本身就是「用户点名这个技能」。
///
/// intent 正则管的是「说这件事该用谁」，点名管的是「我要用谁」——两者都要能路由，
/// 且点名优先（否则「用 X 技能做个 PPT」会被另一个技能的 "PPT" 正则截胡，
/// 只能靠模型中途 use_skill 切换——切换路径出过「刚切就断」的事故）。
///
/// 边界要求：名字前后不能是字母数字/连字符/下划线，否则 "my-ppt-craft-master"
/// 会命中 "ppt-craft-master"。只对带 '-' / '_' 的名字生效——单词名（docx、note）
/// 撞普通词的概率高，命中了就是路由劫持。
///
/// 大小写不敏感：正文里写 "用 PPT-craft-master 技能做" 与技能名 "ppt-craft-master"
/// 大小写常常不一致，严格匹配等于没匹配上。
pub fn explicit_name_pattern(name: &str) -> Option<String> {
    if !name.contains('-') && !name.contains('_') {
        return None;
    }
    Some(format!(
        r"(?i)(?:^|[^A-Za-z0-9_.\-]){}(?:$|[^A-Za-z0-9_.\-])",
        regex::escape(name)
    ))
}

struct CompiledRule {
    regexes: Vec<Regex>,
    skill_name: String,
    session: Option<String>,
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
            session: r.session.clone(),
        })
        .filter(|r| !r.regexes.is_empty())
        .collect()
}

// ── intent 模式准入（装技能时拦住「匹配一切」的模式）────────────────────

/// 单条 intent 模式的扫描结果。
struct PatternScan {
    /// 连续字面量段（被通配 / 分支 / 量词切断）
    runs: Vec<String>,
    /// 模式涉及的全部字面量字符（字符类里的字符也算——探针串要避开它们）
    literal_chars: std::collections::HashSet<char>,
}

/// 扫一遍正则模式，抽出「模式确定要匹配的固定文本」。
///
/// 只认字面量：通配 `.`、分支 `|`、量词、字符类、`\d` 一类字符类转义都不算——
/// 它们决定「匹配哪个字符」，不决定「匹配哪段文本」。分组括号不切断字面量段：
/// `(ab)c` 匹配的仍是连续文本 "abc"。
fn scan_pattern(pattern: &str) -> PatternScan {
    let mut runs: Vec<String> = Vec::new();
    let mut literal_chars: std::collections::HashSet<char> = std::collections::HashSet::new();
    let mut cur = String::new();
    let mut it = pattern.chars().peekable();

    while let Some(c) = it.next() {
        match c {
            // 分组：`(?flags)` 整组跳过（标志不是文本）；`(?:` / `(?P<name>` /
            // `(?flags:` 只跳前缀，组内文本照常计入
            '(' => {
                if it.peek() == Some(&'?') {
                    it.next();
                    match it.peek().copied() {
                        Some(':') => {
                            it.next();
                        }
                        Some('P') => {
                            it.next();
                            if it.peek() == Some(&'<') {
                                it.next();
                                for n in it.by_ref() {
                                    if n == '>' {
                                        break;
                                    }
                                }
                            }
                        }
                        Some('<') => {
                            it.next();
                            for n in it.by_ref() {
                                if n == '>' {
                                    break;
                                }
                            }
                        }
                        _ => {
                            // `(?flags)` 或 `(?flags:`：标志字符集合固定
                            while let Some(f) = it.peek().copied() {
                                match f {
                                    'i' | 'm' | 's' | 'U' | 'x' | '-' => {
                                        it.next();
                                    }
                                    ':' => {
                                        it.next();
                                        break;
                                    }
                                    ')' => {
                                        it.next();
                                        break;
                                    }
                                    _ => break,
                                }
                            }
                        }
                    }
                }
            }
            // 分支：两侧是互斥候选，不是连续文本
            '|' | '.' | '^' | '$' | '*' | '+' | '?' => {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
            }
            // 重复量词 `{m,n}`：里面的数字不是文本
            '{' => {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
                for n in it.by_ref() {
                    if n == '}' {
                        break;
                    }
                }
            }
            // 字符类 `[...]`：是「字符集合」不是「文本」，整段跳过
            '[' => {
                if !cur.is_empty() {
                    runs.push(std::mem::take(&mut cur));
                }
                let mut first = true;
                while let Some(n) = it.next() {
                    if n == ']' && !first {
                        break;
                    }
                    first = false;
                    if n == '\\' {
                        if let Some(e) = it.next() {
                            literal_chars.insert(e);
                        }
                        continue;
                    }
                    literal_chars.insert(n);
                }
            }
            // 转义：`\d` 一类是字符类（不是文本），其余转义的是字面字符
            '\\' => match it.next() {
                Some(
                    e @ ('d' | 'D' | 'w' | 'W' | 's' | 'S' | 'b' | 'B' | 'A' | 'z' | 'Z' | 'G'
                    | 'p' | 'P'),
                ) => {
                    literal_chars.insert(e);
                    if !cur.is_empty() {
                        runs.push(std::mem::take(&mut cur));
                    }
                }
                Some(e) => {
                    cur.push(e);
                    literal_chars.insert(e);
                }
                None => {}
            },
            // 其余都是字面字符
            c => {
                cur.push(c);
                literal_chars.insert(c);
            }
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    PatternScan {
        runs,
        literal_chars,
    }
}

/// 探针串：一段不含模式任何字面量字符的重复文本。
/// 模式若能匹配它，说明匹配靠的是通配而非意图词 → 过宽。
fn content_free_probe(literal_chars: &std::collections::HashSet<char>) -> Option<String> {
    const FILLERS: [char; 12] = ['x', 'y', 'z', '0', '9', '-', '_', '~', '@', '#', '中', '日'];
    FILLERS
        .iter()
        .find(|f| !literal_chars.contains(f))
        .map(|f| f.to_string().repeat(8))
}

/// intent 模式准入检查（纯函数）：装技能时拦住「能匹配几乎任意输入」的模式。
///
/// 装上的技能立刻进**进程级**路由表，命中的用户消息会被直接送进该技能
/// （正文进系统提示；mode=auto 时整段 DSL 无 LLM 参与地执行）。一个 `.*`
/// 就能把此后每条用户消息都劫持进去——这是提示注入的落点，不是「宽一点」。
///
/// 三道闸：
/// 1. **正则必须可编译**——编译不了的模式在 `compile_rules` 里被 `.ok()` 静默
///    丢弃，装上却永不触发（哑弹技能），不如当场拒掉。
/// 2. **必须含 ≥2 字符的连续字面量段**——`.*` / `(?s).*` / `^.*$` / `.{0,}` /
///    `[\s\S]*` 这类模式没有字面量，能匹配任意文本（含空串）。
/// 3. **不得能匹配「不含其字面量的探针串」**——`(?s)(润色|修订)?.*` 这类
///    「关键词可选」的写法含字面量却仍能匹配任意文本，同样过宽。
pub fn check_intent_pattern(pattern: &str) -> Result<(), String> {
    let re =
        Regex::new(pattern).map_err(|e| format!("正则语法错误（该模式永远不会被触发）：{e}"))?;
    let scan = scan_pattern(pattern);
    if !scan.runs.iter().any(|r| r.chars().count() >= 2) {
        return Err("模式过宽：必须包含至少 2 个连续的字面量字符（如「日报」或「(?i)(润色|修订).{0,15}(word|文档)」），不能是 .* 一类通配".into());
    }
    if let Some(probe) = content_free_probe(&scan.literal_chars) {
        if re.is_match(&probe) {
            return Err("模式过宽：能匹配与本模式意图词无关的任意文本（如 .* 、(?s).* ），会把此后每条用户消息都路由进本技能".into());
        }
    }
    Ok(())
}

fn match_compiled_refs(text: &str, rules: &[&CompiledRule]) -> RouteAction {
    for rule in rules {
        for re in &rule.regexes {
            if re.is_match(text) {
                return RouteAction::Skill(rule.skill_name.clone());
            }
        }
    }
    RouteAction::PassThrough
}

/// 进程级路由表：启动时为空（空表 = 全量 PassThrough，fail-open 与原中间件业务路由语义一致），
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

/// 追加一条会话档路由（模型 create_skill 建技能后本会话即时生效用）。
/// 不去重不重建：create_skill 落盘有排他（同名目录已存在会失败），不会产生
/// 同名双规则；会话档随进程消亡，不落任何持久层——重启后该技能不再自动触发，
/// 直到用户显式改 frontmatter 的 scope 提升为全局。
pub fn add_session_route(rule: IntentRule) {
    let compiled = compile_rules(std::slice::from_ref(&rule));
    match ROUTES.write() {
        Ok(mut guard) => guard.extend(compiled),
        Err(e) => e.into_inner().extend(compiled),
    }
}

/// 会话可见性过滤：全局档恒可见；会话档只对本会话可见（无会话上下文时不可见）。
fn visible_under_session<'a>(
    rules: &'a [CompiledRule],
    session: Option<&str>,
) -> Vec<&'a CompiledRule> {
    rules
        .iter()
        .filter(|r| match r.session.as_deref() {
            None => true,
            Some(s) => session == Some(s),
        })
        .collect()
}

/// L1 前置路由（生产路径）：读全局路由表，会话档只对本会话可见
pub fn route_user_input_for(text: &str, session: Option<&str>) -> RouteAction {
    let guard = match ROUTES.read() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    let visible = visible_under_session(&guard, session);
    match_compiled_refs(text, &visible)
}

/// 兼容入口：无会话上下文的路由（只看得见全局档）
pub fn route_user_input(text: &str) -> RouteAction {
    route_user_input_for(text, None)
}

/// 纯函数核：规则注入式匹配。测试主入口（不碰全局表，无并行污染）；
/// 也可用于「这份规则集会产生什么路由」的离线核验。
pub fn route_with_rules(text: &str, rules: &[IntentRule]) -> RouteAction {
    route_with_rules_for(text, rules, None)
}

/// 纯函数核的会话版：离线核验「这份规则集在某个会话视角下会产生什么路由」。
pub fn route_with_rules_for(
    text: &str,
    rules: &[IntentRule],
    session: Option<&str>,
) -> RouteAction {
    let compiled = compile_rules(rules);
    let visible = visible_under_session(&compiled, session);
    match_compiled_refs(text, &visible)
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
        rule_scoped(name, patterns, None)
    }

    fn rule_scoped(name: &str, patterns: &[&str], session: Option<&str>) -> IntentRule {
        IntentRule {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            skill_name: name.to_string(),
            session: session.map(str::to_string),
        }
    }

    // ── 点名路由：正文里出现技能名 ──

    #[test]
    fn explicit_name_pattern_matches_standalone_name_only() {
        let pattern =
            explicit_name_pattern("ppt-craft-master").expect("带连字符的名字应有点名规则");
        let re = Regex::new(&pattern).expect("点名正则应可编译");
        assert!(re.is_match("用ppt-craft-master技能做PPT"));
        assert!(re.is_match("跑一下 ppt-craft-master"));
        assert!(re.is_match("ppt-craft-master"));
        assert!(
            re.is_match("用PPT-craft-master技能做"),
            "正文大小写与技能名不一致也算点名"
        );
        assert!(
            !re.is_match("my-ppt-craft-master-副本"),
            "前后是名字字符时不算点名"
        );
        // 单词名不产生点名规则——撞普通词的概率高
        assert!(explicit_name_pattern("docx").is_none());
    }

    #[test]
    fn explicit_name_rule_wins_over_intent_rule() {
        // 点名规则在前、intent 规则在后 → 两条都命中时走点名的那个
        let pattern = explicit_name_pattern("ppt-craft-master").expect("点名规则");
        let rules = vec![
            rule("ppt-craft-master", &[pattern.as_str()]),
            rule("pptx-design", &["PPT"]),
        ];
        assert_eq!(
            route_with_rules("用ppt-craft-master技能做个PPT", &rules),
            RouteAction::Skill("ppt-craft-master".to_string()),
            "点名优先于 intent 正则"
        );
        // 没点名时仍按 intent 正则走
        assert_eq!(
            route_with_rules("做个PPT", &rules),
            RouteAction::Skill("pptx-design".to_string())
        );
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

    /// **每条 pattern 一正一负**：intent 规则越写越松（把 `.{0,15}` 放宽、
    /// 把裸「润色」单独成条）会让无关输入误进 Skill 执行链路——多花一次 LLM 调用，
    /// 甚至触发不必要的文件处理。逐条钉住：正例命中该条模式，负例既不该命中该条、
    /// 也不该被同技能其它模式捞走（负例断言的是整条路由 PassThrough）。
    ///
    /// 新增/修改 frontmatter intent 时，本表必须同步（`patterns.len()` 断言会拦下漏改）。
    #[test]
    fn every_docx_pattern_has_positive_and_negative_case() {
        let rules = docx_rules();
        let patterns = &rules[0].patterns;
        assert_eq!(
            patterns.len(),
            5,
            "docx fixture 的 pattern 数变了——同步下面的 cases 表（每条一正一负）"
        );

        // (pattern 下标, 正例, 负例)
        let cases: &[(usize, &str, &str)] = &[
            (0, "帮我润色一下这个 Word 文档", "帮我润色一下这段话"),
            (1, "用修订模式改一下", "用别的模式改一下"),
            (2, "Word 文档润色", "Excel 表格润色"),
            (3, "润色\n附件：/tmp/汇报.docx", "润色\n附件：/tmp/图.png"),
            (4, "/tmp/合同.doc 帮我修订一下", "/tmp/图.png 帮我修订一下"),
        ];

        for (idx, positive, negative) in cases {
            let raw = &patterns[*idx];
            let re = regex::Regex::new(raw).expect("fixture 模式应为合法正则");
            assert!(
                re.is_match(positive),
                "pattern[{idx}] {raw} 应命中正例「{positive}」"
            );
            assert!(
                !re.is_match(negative),
                "pattern[{idx}] {raw} 不该命中负例「{negative}」"
            );
            // 端到端：正例路由到该技能；负例必须是 PassThrough（不被其它模式捞走）
            assert_eq!(
                route_with_rules(positive, &rules),
                RouteAction::Skill("minimax-docx".to_string()),
                "正例「{positive}」应路由到 minimax-docx"
            );
            assert_eq!(
                route_with_rules(negative, &rules),
                RouteAction::PassThrough,
                "负例「{negative}」不该被任何模式命中"
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

    /// 会话档规则只对本会话可见：别的会话、无会话上下文都看不见——
    /// create_skill 建的技能不得跨会话劫持。
    #[test]
    fn session_scoped_rules_only_match_their_own_session() {
        let rules = vec![
            rule_scoped("zz-session-skill", &["zzSessionProbe"], Some("s1")),
            rule("zz-global-skill", &["zzGlobalProbe"]),
        ];
        assert_eq!(
            route_with_rules_for("zzSessionProbe", &rules, Some("s1")),
            RouteAction::Skill("zz-session-skill".to_string()),
            "本会话必须看得见自己的会话档"
        );
        assert_eq!(
            route_with_rules_for("zzSessionProbe", &rules, Some("s2")),
            RouteAction::PassThrough,
            "别的会话不得看见 s1 的会话档"
        );
        assert_eq!(
            route_with_rules_for("zzSessionProbe", &rules, None),
            RouteAction::PassThrough,
            "无会话上下文（旧入口）不得看见会话档"
        );
        for s in [Some("s1"), Some("s2"), None] {
            assert_eq!(
                route_with_rules_for("zzGlobalProbe", &rules, s),
                RouteAction::Skill("zz-global-skill".to_string()),
                "全局档不受会话过滤影响（session={s:?}）"
            );
        }
    }

    /// 会话档注册进全局表后的生产路径：add_session_route 追加的规则
    /// 只在对应会话的路由视角里命中（探针模式独一无二，不与并行测试交叉）。
    #[test]
    fn add_session_route_registers_visible_only_to_its_session() {
        rebuild_routes(vec![]);
        add_session_route(rule_scoped(
            "zz-session-probe-skill",
            &["zzSessionRouteProbe"],
            Some("zz-sid"),
        ));
        assert_eq!(
            route_user_input_for("zzSessionRouteProbe", Some("zz-sid")),
            RouteAction::Skill("zz-session-probe-skill".to_string())
        );
        assert_eq!(
            route_user_input_for("zzSessionRouteProbe", Some("other-sid")),
            RouteAction::PassThrough
        );
        assert_eq!(
            route_user_input("zzSessionRouteProbe"),
            RouteAction::PassThrough,
            "旧入口（无会话）不得看见会话档"
        );
        // 还原空表，不污染其他测试
        rebuild_routes(vec![]);
    }

    // ── intent 模式准入闸 ──

    /// 通配型模式必须被拒：它们能匹配任意输入（含空串），装上即把此后每条
    /// 用户消息都路由进该技能。
    #[test]
    fn wildcard_only_patterns_are_rejected() {
        for p in [
            ".*",
            "(?s).*",
            "^.*$",
            ".{0,}",
            "[\\s\\S]*",
            "(?is)[\\s\\S]*",
            "^$",
            ".+",
        ] {
            assert!(
                check_intent_pattern(p).is_err(),
                "过宽模式「{p}」必须被准入闸拒掉"
            );
        }
    }

    /// 含字面量但仍能匹配任意文本的模式同样要拒：关键词是**可选**时，
    /// `(?s)(润色|修订)?.*` 看着像「带关键词」，实际匹配一切。
    #[test]
    fn pattern_matching_arbitrary_text_around_literal_is_rejected() {
        for p in [
            "(?s)(润色|修订)?.*",
            "(?i)(abc)?.*",
            "(?i)(abc)*",
            "(?i)(a|bc)?.*",
        ] {
            assert!(
                check_intent_pattern(p).is_err(),
                "模式「{p}」能匹配与意图无关的任意文本，必须被拒"
            );
        }
    }

    /// 合法意图模式必须放行：复合模式（量词 / 字符类 / 分支 / 转义）不能被误杀。
    #[test]
    fn legitimate_intent_patterns_are_accepted() {
        for p in [
            "日报",
            "PPT",
            "用修订模式",
            r"(?i)(润色|修订).{0,15}(word|文档)",
            r"(?i)word.{0,10}(润色|修订)",
            r"(?is)(润色|修订)[\s\S]*\.docx?",
            r"(?is)\.docx?[\s\S]*(润色|修订)",
            r"(?i)create\s+(ppt|deck)",
        ] {
            assert!(
                check_intent_pattern(p).is_ok(),
                "合法模式「{p}」不该被准入闸拒掉：{:?}",
                check_intent_pattern(p)
            );
        }
    }

    /// 装不上的技能等于哑弹：语法非法的模式在路由表里被静默丢弃，
    /// 准入闸要在装的时候就说出来。
    #[test]
    fn invalid_regex_is_rejected_with_reason() {
        let err = check_intent_pattern(r"(?i)unclosed(").unwrap_err();
        assert!(err.contains("正则语法错误"), "{err}");
    }

    /// 已装技能的声明必须过得了闸——闸不能把现役技能判死。
    #[test]
    fn docx_fixture_patterns_pass_the_gate() {
        for p in &docx_rules()[0].patterns {
            assert!(
                check_intent_pattern(p).is_ok(),
                "docx 夹具模式「{p}」过不了准入闸：{:?}",
                check_intent_pattern(p)
            );
        }
    }
}

/// 聊天模式批量执行触发解析（pre-step 路由的 ExecuteTasks 分支）
///
/// 全部用例都用**前端真实拼装格式**（ChatPanel.tsx）：消息头是引用块，
/// 用户正文在块之后——「完成\n\n[已选任务]…」那种关键词在前的形态前端
/// 从不发送，拿它当夹具等于锁不住真实链路。
#[cfg(test)]
mod chat_execute_parse_tests {
    use super::*;

    /// 前端拼装出的消息：引用块在前、用户正文在后
    fn msg_with_block(block: &str, user_text: &str) -> String {
        format!("{block}\n\n{user_text}")
    }

    #[test]
    fn parse_block_extracts_id_and_title_chinese_comma() {
        let content = "[已选任务]\n- id=abc-123，标题=写 PPT\n- id=def-456，标题=分析销售数据";
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

    /// 引用块到块尾为止：用户正文里形似任务引用的行不是引用块。
    /// 认错一次就是替用户执行了一张他没选中的卡。
    #[test]
    fn parse_block_stops_at_user_text() {
        let content = "[已选任务]\n- id=abc，标题=写 PPT\n\n顺手把这行也跑了\n- id=evil，标题=删库";
        let parsed = parse_selected_tasks_block(content);
        assert_eq!(parsed.len(), 1, "正文里的伪引用行不该被收进块");
        assert_eq!(parsed[0].0, "abc");
    }

    /// 标记不在消息头（用户正文里写的、粘贴的日志里带的）一律不认。
    #[test]
    fn parse_block_ignores_marker_that_is_not_at_the_head() {
        let content = "看下这个格式：\n[已选任务]\n- id=abc，标题=写 PPT";
        assert!(parse_selected_tasks_block(content).is_empty());
    }

    /// 附件块在前时引用块仍能解析（前端两个块可以同时出现）
    #[test]
    fn parse_block_after_attachment_block() {
        let content = "[附件文件]\n- /Users/x/a.pdf\n\n[已选任务]\n- id=abc，标题=写 PPT";
        let parsed = parse_selected_tasks_block(content);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "abc");
    }

    #[test]
    fn trigger_requires_keyword_and_block() {
        // 有块有关键词 → 命中
        let hit = is_chat_execute_trigger(&msg_with_block("[已选任务]\n- id=a，标题=b", "完成"));
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().len(), 1);

        // 有块无关键词 → 不命中
        assert!(
            is_chat_execute_trigger(&msg_with_block("[已选任务]\n- id=a，标题=b", "看看这些"))
                .is_none()
        );

        // 无块有关键词 → 不命中
        assert!(is_chat_execute_trigger("完成任务").is_none());

        // 完全无关 → 不命中
        assert!(is_chat_execute_trigger("你好世界").is_none());
    }

    #[test]
    fn trigger_matches_all_keyword_variants() {
        for kw in &["完成", "执行", "搞定", "开干", "做掉", "go", "run", "do"] {
            let content = msg_with_block("[已选任务]\n- id=a，标题=b", &format!("{kw}一下"));
            assert!(
                is_chat_execute_trigger(&content).is_some(),
                "关键词 {} 应命中",
                kw
            );
        }
    }

    /// 触发词只来自引用块之后的用户正文：块之前的附件路径不是命令。
    #[test]
    fn trigger_keyword_comes_from_user_text_after_block() {
        let content = "[附件文件]\n- /Users/x/完成方案.docx\n\n[已选任务]\n- id=a，标题=b";
        assert!(is_chat_execute_trigger(content).is_none());
    }

    #[test]
    fn trigger_keyword_case_insensitive() {
        assert!(
            is_chat_execute_trigger(&msg_with_block("[已选任务]\n- id=a，标题=b", "GO")).is_some()
        );
        assert!(
            is_chat_execute_trigger(&msg_with_block("[已选任务]\n- id=a，标题=b", "Run")).is_some()
        );
    }

    /// 命中 = 把选中卡全部自主执行一遍，所以英文句子里**含** go/run/do 不足以触发：
    /// download / google 含 go，cargo run 的 run 不是首词——这些都只是普通聊天。
    #[test]
    fn trigger_rejects_ascii_keywords_embedded_in_english() {
        for cmd in [
            "download the report",
            "google 一下",
            "cargo run",
            "running tests",
            "domain model",
            "please run the build",
        ] {
            let content = msg_with_block("[已选任务]\n- id=a，标题=b", cmd);
            assert!(
                is_chat_execute_trigger(&content).is_none(),
                "「{cmd}」不该触发批量执行"
            );
        }
    }

    /// 反过来：中英混说里 ASCII 词确实在首位时仍要触发（口语「go一下」）。
    #[test]
    fn trigger_accepts_leading_ascii_keyword_with_cjk_suffix() {
        for cmd in ["go一下", "run 一下", "do it", "go", "run"] {
            let content = msg_with_block("[已选任务]\n- id=a，标题=b", cmd);
            assert!(
                is_chat_execute_trigger(&content).is_some(),
                "「{cmd}」应触发批量执行"
            );
        }
    }

    /// 前端真实发送格式（ChatPanel.tsx 拼装的 [已选任务] 块在前、用户正文在后）
    /// 必须能触发：标记位置或字段名对不上，批量执行路由就静默不触发，
    /// 模型只能按标题模糊匹配找卡。
    #[test]
    fn trigger_parses_the_block_frontend_actually_sends() {
        let content = "[已选任务]\n- id=t1，标题=改文档\n- id=t2，标题=写周报\n\n完成";
        let parsed = is_chat_execute_trigger(content).expect("前端格式必须命中");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ("t1".to_string(), "改文档".to_string()));
        assert_eq!(parsed[1], ("t2".to_string(), "写周报".to_string()));
    }

    /// 附件块 + 引用块同时出现时也照常触发
    #[test]
    fn trigger_with_attachment_and_task_block() {
        let content = "[附件文件]\n- /Users/x/a.pdf\n\n[已选任务]\n- id=t1，标题=改文档\n\n完成";
        let parsed = is_chat_execute_trigger(content).expect("带附件也应命中");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "t1");
    }

    /// 正文里的伪引用行不得跟着开跑
    #[test]
    fn trigger_ignores_task_like_lines_in_user_text() {
        let content =
            "[已选任务]\n- id=t1，标题=改文档\n\n完成这批，顺手把这行也跑了\n- id=evil，标题=删库";
        let parsed = is_chat_execute_trigger(content).expect("应命中");
        assert_eq!(parsed.len(), 1, "只有块里的那一张");
        assert_eq!(parsed[0].0, "t1");
    }

    #[test]
    fn trigger_extracts_multiple_tasks() {
        let content = msg_with_block(
            "[已选任务]\n- id=a，标题=A\n- id=b，标题=B\n- id=c，标题=C",
            "执行",
        );
        let parsed = is_chat_execute_trigger(&content).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].0, "a");
        assert_eq!(parsed[1].0, "b");
        assert_eq!(parsed[2].0, "c");
    }

    #[test]
    fn trigger_maps_to_execute_tasks_route_action() {
        // 收编主流程后的路由契约：触发命中 → RouteAction::ExecuteTasks（不再是 bot_chat 前置短路）
        let tasks =
            is_chat_execute_trigger(&msg_with_block("[已选任务]\n- id=a，标题=A", "完成")).unwrap();
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
