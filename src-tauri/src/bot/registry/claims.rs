//! 声明判据面：模型「声称做了」的提取与判定（变更声称 / 执行声称 / 数值 /
//! 文档路径 / 工具点名）。

use super::TOOLS_TABLE;

/// 合并 TOOLS_TABLE 中所有 mutating 工具的 claims_patterns，
/// 检查 text 是否含任一变更声称表述。
///
/// 原 bot_model_loop.rs::claims_mutation 的「14 个动词 + 4 个整段」硬编码
/// 改为从 ToolDef 表派生——加新 mutating 工具时只需填自己的 claims_patterns，
/// 这里的检测逻辑零修改。
///
/// 检测是 2 阶段：
/// 1. 子串包含（catches 「移至回收站」、「已完成任务」等整段）
/// 2. 「已」+ 8 字窗口包含任一 pattern（catches 「已...移除」类变体话术；
///    原实现同款，避免漏掉「已把附件全部移除」之类的中间插入副词话术）
///
/// 两阶段并集 OR，原 hallucination_guard_tests 6 个负面用例仍会正确返回 false。
///
/// 命中点带**领域豁免**：命中处前后 12 字内出现「剪贴板」时这次命中不算变更声称。
/// 「已复制到剪贴板」说的是系统剪贴板（clipboard_write / copy_file_to_clipboard
/// 都是非变更类工具），而 manage_file 的短动词「已复制」是它的子串——不豁免就会把
/// 「把这段文字复制到剪贴板」当成「文件复制」，凭空打回模型并给用户挂一条
/// 「这句没有对应凭证」的更正。剪贴板只改系统剪贴板，不改文件与任务卡，
/// 语义上确实不是变更。文件复制话术（「已复制：/a → /b」）不与「剪贴板」同现，
/// 照常命中。
pub fn claims_mutation(text: &str) -> bool {
    let patterns: Vec<&str> = TOOLS_TABLE
        .iter()
        .filter(|t| t.mutating)
        .flat_map(|t| t.claims_patterns.iter())
        .copied()
        .collect();
    let chars: Vec<char> = text.chars().collect();
    // 阶段 1：子串匹配
    for p in &patterns {
        for (s, e) in find_occurrences(&chars, p) {
            if !claim_exempt(&chars, s, e) {
                return true;
            }
        }
    }
    // 阶段 2：「已」后 8 字窗口包含任一 pattern（strip 前缀「已」后）
    // 原 bot_model_loop.rs VERBS 表不含「已」前缀（「删除」不是「已删除」），
    // 故 window 检查也需剥去 pattern 的前缀「已」才能覆盖「已...移除」类变体。
    for (i, &c) in chars.iter().enumerate() {
        if c != '已' {
            continue;
        }
        let w_end = (i + 1 + 8).min(chars.len());
        let window = &chars[i + 1..w_end];
        for p in &patterns {
            let stripped = p.strip_prefix('已').unwrap_or(p);
            for (ws, we) in find_occurrences(window, stripped) {
                // 窗口内位置换算回原文位置再判豁免
                let abs_s = i + 1 + ws;
                let abs_e = i + 1 + we;
                if !claim_exempt(&chars, abs_s, abs_e) {
                    return true;
                }
            }
        }
    }
    false
}

/// 命中点的领域豁免：前后 12 字窗口内出现「剪贴板」则这次命中不算变更声称。
///
/// 窗口取 12 字是够用的下限：剪贴板话术的常见形态（「已复制到剪贴板」、
/// 「文件已放入剪贴板」）里动词与「剪贴板」的距离都在 6 字内；12 字保证
/// 「已复制 /a 到 /b，并把说明复制到剪贴板」这种前后两处并存时，文件复制那处
/// 仍照常命中。
const CLAIM_EXEMPT_WINDOW: usize = 12;
/// 非变更语义领域标记：系统剪贴板（只改剪贴板，不改文件与任务卡）。
const CLAIM_EXEMPT_MARK: &str = "剪贴板";

fn claim_exempt(chars: &[char], start: usize, end: usize) -> bool {
    let lo = start.saturating_sub(CLAIM_EXEMPT_WINDOW);
    let hi = (end + CLAIM_EXEMPT_WINDOW).min(chars.len());
    chars[lo..hi]
        .iter()
        .collect::<String>()
        .contains(CLAIM_EXEMPT_MARK)
}

/// pattern 在 chars 里的全部出现位置（字符下标，半开区间，不重叠）。
fn find_occurrences(chars: &[char], pattern: &str) -> Vec<(usize, usize)> {
    let pat: Vec<char> = pattern.chars().collect();
    if pat.is_empty() || pat.len() > chars.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i + pat.len() <= chars.len() {
        if chars[i..i + pat.len()] == pat[..] {
            out.push((i, i + pat.len()));
            i += pat.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 执行类谎报的完成态措辞。
///
/// 与 `claims_mutation` 的词表分家而非合并：变更话术能逐工具派生（「删除」属
/// delete_task、「勾选」属 toggle_subtask），而「跑完了 / 工具返回」描述的是
/// 「调用工具」这个动作本身，与调的是哪个工具无关——挂到 ToolDef 上只能给同一
/// 组词抄 N 遍。故此处用共享表；加新工具同样不需要改检测逻辑。
///
/// 刻意不收「已完成 / 已完成搜索 / 分析已完成」这类中性收尾语——模型在一次
/// 工具都没调用的对话里也会自然说「分析已完成」，收了会把正常答复打回；
/// 「打印了」同理，用户粘一段输出问「这句什么意思」时模型复述「打印了 done」
/// 是诚实回答，不是谎报。
const EXECUTION_CLAIM_PATTERNS: &[&str] = &[
    // 跑了某件事的完成态
    "执行完毕",
    "运行完毕",
    "跑完了",
    // 「已跑完」与「跑完了」是同一个完成态的两种语序，漏了它整道闸对
    // 「已跑完，输出 5050」这种最常见的说法是瞎的
    "已跑完",
    "已执行",
    "已经执行",
    "已运行",
    // 工具给了结果（工具当主语——用户粘来的输出不会被叙述成「工具返回」）
    "工具返回",
    "返回结果是",
    "运行结果",
    "执行结果",
    "已打印",
    // 分步叙述「我按步骤跑完了」。实测编造原话（trace 51，零工具调用）：
    // 「完成计划2 步：第一步 X 触发 Y，第二步 Z 仍输出 W」。这几句对代码讲解
    // 不成立——讲解会说「可以分两步做」，不会说「完成计划 2 步」/「仍输出」。
    "完成计划",
    "已运行步骤",
    "仍输出",
    // 第一人称完成态。实测绕过的原话（trace 63，零凭证放行）：
    // 「我跑了一个 print 100 的小任务，返回了 100」——「我跑了」不匹配
    // 「跑完了」，整句没有第二个触发词点，于是不进判决。与「跑完了」的差别
    // 只在完成态后缀上，模型日常的说法反而是这一支。
    "我跑了",
    "我执行了",
    "我运行了",
    // 实测绕过的原话（trace 65，零凭证放行，且 spawn_subagent 也没调过）：
    // 「子 agent 跑 sleep(5) 实测约 5013 ms」——数字照抄上一轮真实回复的
    // 格式编的。刻意只收「约/为/结果」这几种结句，不收裸「实测」：模型引用
    // 用户粘来的计时输出时说的是「实测耗时: 2005.08 ms」（照抄原文形态），
    // 编造时说的是「实测约 X」（要显得可信又不绝对）。
    "实测约",
    "实测为",
    "实测结果",
    "跑出来",
    // 模型把工具调用结果列成清单的写法：零工具调用却报「✅ 调用成功」。
    // 实测原话形如「工具调用如下：- create_word：✅ 调用成功，返回路径 X」——
    // 上面那批词一个都不命中，整道执行类闸不开门（点名判定轮不到，词表是第一环）。
    // 只收完成态的四种：失败/被拒是相反形态（不含这些词），如实汇报失败不受影响。
    "调用成功",
    "执行成功",
    "调用结果",
    "调用完毕",
    "调用完成",
    // 刻意不收的一批（各自都有正当讲解句，扩进来即误伤，见同文件测试）：
    // 「执行完成 / 运行完成」——「任务执行完成后会通知你」
    // 「步骤执行完成」——「每步骤执行完成后记录日志」
    // 「运行抛出 / 抛出异常」——「运行抛出异常时该脚本会非零退出」
    // 「返回了 / 结果是」——「这个函数返回了 None」「1+1 的结果是 2」。
    //   这两个词抓得住 trace 63 的后半句，但那半句的抓法在上面「我跑了」已经
    //   覆盖，加进来等于拿代码讲解和算术题换一条冗余命中，不划算。
];

/// 中文/口语说法 → 工具名。
///
/// 模型写不出 snake_case 的工具名：它说「子 agent」而表里是 `spawn_subagent`，
/// 于是点名扫描（`mentioned_tool_names`）认不出这是个工具调用，判决退到
/// 「整条提问零凭证」的粗判。只靠粗判挡不住那句——模型照抄上一轮真实回复的
/// 格式报了个子 agent 的实测值（trace 65），而它压根没派过子 agent。
const TOOL_ALIASES: &[(&str, &str)] = &[
    ("子 agent", "spawn_subagent"),
    ("子代理", "spawn_subagent"),
    ("subagent", "spawn_subagent"),
];

/// 内置默认词表。既是设置页「恢复默认」的来源，也是没保存过自定义词表时的
/// 生效值。返回 owned：调用方要跨线程用（command 返回给前端）。
pub fn default_execution_claim_patterns() -> Vec<String> {
    EXECUTION_CLAIM_PATTERNS
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// 点名判据的成功形态标记（内置，不受设置页词表影响）。
///
/// 词表是用户可编辑的执行类触发器：删空就是「关掉这一档触发器」。点名判据要
/// 在词表之外还能立住，得有一个关不掉的形态标记——模型把工具结果列成清单时
/// 写的那种（`- create_word：✅ 调用成功`）。符号形态不参与措辞，模型换说法
/// 照样命中。
const BUILTIN_SUCCESS_MARKERS: &[&str] = &["✅", "✔"];

/// 终稿里「点名了某次执行」的工具。
///
/// 与 `mentioned_tool_names` 的差别有两条，都是为了不误伤：
///
/// · **执行形态词要落在工具名之后、且在同一句内**——「任务创建成功，你可以用
///   grep_files 搜」里的「成功」说的是任务，不是 grep_files；词落在名字前面
///   （同一句里前面那半截的执行叙述）不算。
/// · **只认词表里的完成态措辞**——失败形态（「调用失败」「调用被拒」）不在词表
///   里，如实汇报工具不可用是诚实行为，不该被判成编造。
pub fn claimed_executed_tools(text: &str, patterns: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for seg in split_claim_segments(text) {
        for (name, needle) in tool_needles() {
            if out.contains(&name) {
                continue;
            }
            let claimed = identifier_occurrences(seg, needle).any(|i| {
                let tail = &seg[i + needle.len()..];
                patterns
                    .iter()
                    .any(|p| !p.is_empty() && tail.contains(p.as_str()))
                    || BUILTIN_SUCCESS_MARKERS.iter().any(|m| tail.contains(m))
            });
            if claimed {
                out.push(name);
            }
        }
    }
    out
}

/// 工具名与它在正文里的写法：snake_case 名 + 中文别名。
///
/// 别名指向的名字必须是真存在的工具——别名表写错工具名时静默跳过（理由同
/// `mentioned_tool_names`）。
fn tool_needles() -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> =
        TOOLS_TABLE.iter().map(|t| (t.name, t.name)).collect();
    for (alias, target) in TOOL_ALIASES {
        if TOOLS_TABLE.iter().any(|t| t.name == *target) {
            out.push((target, alias));
        }
    }
    out
}

/// 切句：换行与句末标点断开。
///
/// 判定「执行形态词落在工具名之后」要在同一句内成立，否则一句里前半段的执行
/// 叙述会被算到后半段提到的工具头上。
fn split_claim_segments(text: &str) -> Vec<&str> {
    text.split(['\n', '。', '！', '？', '；', ';'])
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// 标识符的整词命中位置（左右不得续上标识符字符）。
///
/// 与 `mentions_identifier` 同一口径：工具名互为前缀时纯子串会互相污染
/// （`create_word` ⊂ `create_word_revisions`）。
fn identifier_occurrences<'a>(text: &'a str, ident: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(ident).filter_map(move |(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + ident.len()..].chars().next();
        if before.is_some_and(is_ident_char) || after.is_some_and(is_ident_char) {
            None
        } else {
            Some(i)
        }
    })
}

/// 按传入词表判定「终稿是否在叙述一次已完成的执行」。
///
/// 判决层唯一入口：词表来自 bot-config（设置页可编辑），没配过时由薄壳回落
/// 到内置表。**词表是全量替换而不是合并**——用户在设置页删掉某个词就是要它
/// 不再生效，合并会让内置那排负例测试形同虚设。
pub fn claims_execution_with(text: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| !p.is_empty() && text.contains(p))
}

/// 终稿是否在叙述一次「已完成的执行」（按内置默认词表）。
///
/// 单独判定、不带上下文：是否该拦由调用方结合凭证表一起看（见 bot_model_loop
/// 的幻觉守卫）。纯函数的职责只有「这句话像不像在转述执行结果」。词表改了
/// 这里跟着改——设置页改的是 bot-config，不改这段常量。
pub fn claims_execution(text: &str) -> bool {
    claims_execution_with(text, &default_execution_claim_patterns())
}

/// 结果措辞：把一个数说成「某次执行的产物」的引子。
///
/// 与 `EXECUTION_CLAIM_PATTERNS` 分家：那批词判「像不像在叙述一次执行」，
/// 这批词判「这个数是不是被当成那次执行的产出」。两者要同时命中才作数——
/// 少一个就是误伤的入口：
/// · 少了执行措辞 → 「1+1 的结果是 2」这类算术与讲解句会被当成谎报
///   （词表里刻意不收「结果是/返回了」，指的就是这个坑）。
/// · 少了结果措辞 → 「已跑完，共 3 步」里的 3 是计数不是结果。
const RESULT_CLAIM_MARKERS: &[&str] = &[
    "结果",
    "输出",
    "返回",
    "打印",
    "显示",
    "算出",
    "算出来",
    "得到",
    "统计",
    "实测",
    "答案",
    "读出",
    "产出",
];

/// 终稿是否如实说明了「那次执行没做成」。
///
/// 判据一（结果无出处）的让开条件：模型说了「跑不了 / 被拦了」，那它后面给
/// 的数是它自己的推算（哪怕没标注），不是把执行结果编出来。如实汇报失败是
/// 诚实行为，不该被判成编造——这条是执行类守卫一直守着的承诺。
const EXECUTION_NOT_DONE_MARKERS: &[&str] = &[
    "跑不了",
    "跑不成",
    "没法跑",
    "无法执行",
    "执行不了",
    "执行失败",
    "运行失败",
    "调用失败",
    "调用被拒",
    "被拒绝",
    "被拦截",
    "被挡",
    "未开启",
    "没开启",
    "没跑成",
    "没能跑",
    "跑不动",
    "不可用",
    "没有权限",
    "没权限",
    "无权限",
    "白名单",
    "失败",
    "公式",
    "推算",
    "估算",
    "估计",
    "凭印象",
];

/// 终稿是否如实说明了「那次执行没做成」（含「这是推算不是执行结果」）。
pub fn states_execution_not_done(text: &str) -> bool {
    EXECUTION_NOT_DONE_MARKERS
        .iter()
        .any(|m| !m.is_empty() && text.contains(m))
}

/// 存在性/核实类措辞：终稿在给出现场结论（文件在不在、查到没有）。
///
/// 只给回声那道判据用（见 `is_echo_call`）：那次调用没取回任何新信息，结论
/// 却靠它撑着。与 `RESULT_CLAIM_MARKERS` 分开——「没有」「确实」这类词太泛，
/// 放进结果值那道判据会把「已跑完，没有报错」当成在报结果。
const EXISTENCE_CLAIM_MARKERS: &[&str] = &[
    "存在",
    "不存在",
    "只有",
    "找到",
    "没找到",
    "确实",
    "已核实",
    "已确认",
    "已检查",
    "核对过",
    "已验证",
];

/// 终稿是否给出了「结果」——数值结果或现场结论。
///
/// 供回声那道判据用：调用没取回任何新信息时，终稿若还给不出结论，那次调用
/// 就是白调的，模型拿它当依据就是虚的。
pub fn states_a_result(text: &str) -> bool {
    RESULT_CLAIM_MARKERS.iter().any(|m| text.contains(m))
        || EXISTENCE_CLAIM_MARKERS.iter().any(|m| text.contains(m))
}

/// 文本里的数值（宽松扫描：只取数字，不排形态）。
///
/// 给「出处比对」用：工具给 1234.56、模型报 1235 时字面比不上，但四舍五入到
/// 1235 是等价的——那种是如实转述，不该判成编造。
pub fn numbers_in_text(text: &str) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() || (c == '.' && !cur.ends_with('.')) {
            cur.push(c);
        } else if !cur.is_empty() {
            if let Ok(v) = cur.parse::<f64>() {
                out.push(v);
            }
            cur.clear();
        }
    }
    if !cur.is_empty() {
        if let Ok(v) = cur.parse::<f64>() {
            out.push(v);
        }
    }
    out
}

/// 终稿里「被当成执行产出的结果」的具体数值。
///
/// 纯提取，不判真假：有没有出处要在运行期比（工具返回 + 对话上下文），那边
/// 依赖运行期状态，塞进纯函数就没法单测——与 `claimed_doc_paths` 同样的分工。
///
/// 触发要三样同时落在同一个句段里：执行完成态措辞（词表）、结果措辞、具体
/// 数值。句段按换行与句末标点切（`split_claim_segments`），跨行的那句
/// 「执行完毕：」与下一行的「输出 5050」不算同一段——宁可漏判不误伤。
pub fn claimed_result_numbers(text: &str, patterns: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in split_claim_segments(text) {
        if !patterns
            .iter()
            .any(|p| !p.is_empty() && seg.contains(p.as_str()))
        {
            continue;
        }
        if !RESULT_CLAIM_MARKERS.iter().any(|m| seg.contains(m)) {
            continue;
        }
        for n in result_numbers_in(seg) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out
}

/// 数值两侧的「贴边字符」：贴上就是别的 token 的一部分。
///
/// 路径（`/…/2026Q3复盘.docx`）、标识符（`2026Q3`）、日期（`2026-10-20`）
/// 里的数字都不是结果值。
fn is_value_edge_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/')
}

/// 数后面跟的中文量词：跟上了就是计数，不是结果值。
///
/// 「共 3 项」「3 步」「3 个文件」里的 3 是数出来的个数，模型没执行也数得出来，
/// 拿它当结果去查出处必然误伤。计时与体积单位（秒/ms/KB）不在此列——那是执行
/// 才产得出的量。
const COUNT_UNITS: &[char] = &[
    '个', '项', '条', '次', '份', '张', '页', '行', '轮', '步', '种', '类', '名', '位', '块', '道',
    '组', '批', '套', '只', '件', '本', '款', '台',
];

/// 数前面跟的估算词：跟上了就是估算，不是执行产出的确切值。
const ESTIMATE_WORDS: &[&str] = &[
    "约", "大概", "大致", "近", "超", "逾", "至少", "至多", "最多", "最少", "多达", "几百", "几千",
    "几", "余", "将", "接近", "超过", "多于", "少于", "不到",
];

/// 数前面跟的序数/条款标记：跟上了是编号，不是结果值。
const ORDINAL_WORDS: &[&str] = &["第", "规则", "版本", "条款", "条"];

/// 句段里的数值候选（排掉各自都有正当讲解句的形态）。
///
/// 排掉的形态：标识符/路径/日期的一部分、列表序号（`1.`）、计数（`3 项`）、
/// 估算（`约 5013`）、编号（`第 21 条`、`规则 21/19`）。
pub(super) fn result_numbers_in(seg: &str) -> Vec<String> {
    let chars: Vec<char> = seg.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        let mut end = i;
        // 小数：小数点后必须是数字，否则是列表序号或句末点号
        if i < chars.len()
            && chars[i] == '.'
            && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())
        {
            i += 1;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            end = i;
        }
        let num: String = chars[start..end].iter().collect();
        let prev = start.checked_sub(1).map(|k| chars[k]);
        let next = chars.get(end).copied();
        // 贴边字符：路径/标识符/日期的一部分
        if prev.is_some_and(is_value_edge_char) || next.is_some_and(is_value_edge_char) {
            i = end;
            continue;
        }
        // 列表序号（`1.` `2、`）
        if next == Some('、') {
            i = end;
            continue;
        }
        // 计数：数后面（跨过空格）跟中文量词
        if next_non_space(&chars, end).is_some_and(|c| COUNT_UNITS.contains(&c)) {
            i = end;
            continue;
        }
        // 估算：数前面（跨过空格）跟估算词
        if ends_with_any(&chars, start, ESTIMATE_WORDS) {
            i = end;
            continue;
        }
        // 编号：数前面跟序数/条款标记
        if ends_with_any(&chars, start, ORDINAL_WORDS) {
            i = end;
            continue;
        }
        if !out.contains(&num) {
            out.push(num);
        }
        i = end;
    }
    out
}

/// 从 `end` 起跳过空白后的第一个字符。
fn next_non_space(chars: &[char], end: usize) -> Option<char> {
    chars[end..].iter().find(|c| !c.is_whitespace()).copied()
}

/// `start` 之前的文字（跳过空白）是否以 `words` 里的某个词结尾。
fn ends_with_any(chars: &[char], start: usize, words: &[&str]) -> bool {
    let head: String = chars[..start].iter().rev().take(4).collect();
    let head: String = head.chars().rev().collect();
    let head = head.trim_end();
    words.iter().any(|w| !w.is_empty() && head.ends_with(w))
}

/// 一次工具调用是不是「回声」——返回内容没有取回任何新信息，全是自己写进参数的那句。
///
/// 判据：返回里的内容 token 全部能在入参里找到。模型把结论写进参数、再让工具
/// 原样吐回来，回声就是「这次调用替你核实了什么」的答案——没有。
///
/// 样板不算内容：外部 MCP 工具的返回常排成 `### Result` / `### Ran … code` /
/// `### Page` 几段，格式词（Result/Page/URL…）会让每个 MCP 调用都显得「有新
/// 信息」，所以只取第一段（结果段）与入参比对。
///
/// 返回得太少判不出「有没有新信息」——不猜，不算回声。
pub fn is_echo_call(args: &str, output: &str) -> bool {
    let tokens = content_tokens(result_section(output));
    if tokens.len() < 2 || tokens.iter().map(|t| t.chars().count()).sum::<usize>() < 12 {
        return false;
    }
    let arg_tokens = content_tokens(args);
    tokens
        .iter()
        .all(|t| arg_tokens.iter().any(|a| a == t) || args.contains(t.as_str()))
}

/// 工具返回里的结果段。
///
/// 外部 MCP 工具常把返回排成 `### Result` / `### Ran … code` / `### Page` 几段，
/// 样板段不是取回的内容。第一行不是 `### ` 格式头时整段都算内容。
fn result_section(output: &str) -> &str {
    let first_line = output.split_inclusive('\n').next().unwrap_or("");
    if !first_line.trim_start().starts_with("### ") {
        return output;
    }
    let rest = &output[first_line.len()..];
    match rest.find("\n### ") {
        Some(idx) => &rest[..idx],
        None => rest,
    }
}

/// 内容 token：字母数字（含中日韩）与 `_` `.` `-` `/` 连成的片段。
fn content_tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | '/') {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 终稿里点名的工具（按 TOOLS_TABLE 扫，snake_case 标识符不会与中文话术撞车）。
///
/// 供幻觉守卫把「模型说调了哪个工具」映射到凭证表——这是判决要查的键。
/// 返回全部命中而不是首个：模型一轮里常叙述多个工具，漏掉任何一个就等于
/// 给它留了一条白拿凭证的路。
///
/// 扫描按**标识符整词**匹配（`mentions_identifier`），不是纯子串：工具名互为
/// 前缀时纯子串会互相污染——`create_word` ⊂ `create_word_revisions`，模型如实
/// 汇报「create_word_revisions 执行完毕」时，纯子串扫描会多点出一个没调过的
/// `create_word`，凭证表查不到 → 判成编造 → 返工指令叫它去真调 `create_word`，
/// 产出一份非修订模式的 docx。为一次假指控付一次错误产物。
///
/// 再叠一层别名表（`TOOL_ALIASES`）：模型用中文称呼工具时写不出 snake_case 名，
/// 「子 agent」按原样扫不出来，会让点名判定退化成粗判。
pub fn mentioned_tool_names(text: &str) -> Vec<&'static str> {
    let mut hits: Vec<&'static str> = TOOLS_TABLE
        .iter()
        .map(|t| t.name)
        .filter(|name| mentions_identifier(text, name))
        .collect();
    for (alias, target) in TOOL_ALIASES {
        if !text.contains(alias) {
            continue;
        }
        // 别名指向的名字必须是真存在的工具——别名表写错工具名时静默跳过，
        // 拼错的名字查不到凭证，会把每次提到它的诚实汇报都判成伪造。
        if let Some(name) = TOOLS_TABLE
            .iter()
            .find(|t| t.name == *target)
            .map(|t| t.name)
        {
            if !hits.contains(&name) {
                hits.push(name);
            }
        }
    }
    hits
}

/// 标识符整词匹配：命中段的左右不得续上标识符字符（ASCII 字母数字或 `_`）。
///
/// 只判 ASCII——中文紧贴标识符是正常写法（「用 run_python 跑」），按 Unicode
/// 的 alphanumeric 判会把这类整段漏掉，等于把边界检查写成永假。
fn mentions_identifier(text: &str, ident: &str) -> bool {
    text.match_indices(ident).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + ident.len()..].chars().next();
        !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
    })
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 终稿里声称产出的文档路径（纯提取，不判真假）。
///
/// 出现「已生成/已保存/已写入/已导出」且给出文档绝对路径
/// （docx/xlsx/pptx/pdf）时逐个摘出候选。编造的路径经不起两道校验——
/// 磁盘存在性（路径压根不在）与落盘凭证（路径在、但从来不是工具写出来的）；
/// 实测「已生成XX规划 Word，文件路径：…」类话术带真实形态的假路径，
/// 绕过 claims_mutation 的窄词表，只能靠这两道拦。
///
/// **触发词与路径必须同句**：全文级提取会把「如实改口」读成声称——返工后模型
/// 照着改口提示复述那句「已生成」（还常带引号），而路径在后面另一段里，全文级
/// 提取照样把两者凑成一次声称，于是诚实更正被挂上「⚠️ 不是本次生成的」。真机
/// 原话：「之前那条「已生成」是我口胡，实际并没有落盘……你之前明确说过这份纪要
/// 要存到 `/path/x.docx`」——触发词在第一句、路径在最后一句，说的是两件事。
///
/// 真伪判定不在这里：两问都依赖运行期状态（磁盘、痕迹库），塞进纯函数就没法
/// 单测；提取与判定分开，判定在模型循环侧。
pub fn claimed_doc_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sent in split_claim_sentences(text) {
        if !["已生成", "已保存", "已写入", "已导出"]
            .iter()
            .any(|w| sent.contains(w))
        {
            continue;
        }
        for cand in doc_path_candidates(sent) {
            if !out.contains(&cand) {
                out.push(cand);
            }
        }
    }
    out
}

/// 切句：按句末标点断开，**换行不算**。
///
/// 换行不切是因为「已生成 Word 文档：\n\n`/path/x.docx`」是常见排版——路径
/// 换行到下一段仍是同一次声称。
fn split_claim_sentences(text: &str) -> Vec<&str> {
    text.split(['。', '！', '？', '；', ';'])
        .filter(|s| !s.trim().is_empty())
        .collect()
}

/// 一句里的文档路径候选（切片段 + 贪心合并）。
fn doc_path_candidates(text: &str) -> Vec<String> {
    // 候选路径切分：空白/反引号/引号/括号/书名号/中英文标点都是路径边界。
    // `\` 不入边界集（会拆碎 `C:\` 盘符路径）；UNC 路径（`\\server\share\…`）
    // 靠前缀白名单放行——split 已保证 token 起点必在行首/空白/边界符之后，
    // 故以 `\\` 起头的 token 就是完整的 UNC 前缀，原样进存在性校验
    let boundaries: &[char] = &[
        ' ', '\t', '\n', '\r', '`', '"', '\'', '「', '」', '（', '）', '(', ')', '，', '。', '；',
        '、', ',', ';', ':', '：', '　',
    ];
    // 片段先切出来再合并：含空格的路径（macOS 的 "Application Support"）会被
    // 空格切成两截，后半截不再以 '/' 开头 → 整条路径被丢掉、存在性与凭证两道
    // 闸对它失明。合并规则见 `join_path_fragment`。
    let frags: Vec<&str> = text
        .split(boundaries)
        .map(|tok| tok.trim_matches(|c: char| matches!(c, '*' | '_' | '-' | '>' | '#')))
        .filter(|tok| !tok.is_empty())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < frags.len() {
        let mut cand = frags[i].to_string();
        // 贪心吸收后续片段：把被空格切断的路径接回去
        while i + 1 < frags.len() && join_path_fragment(&cand, frags[i + 1]) {
            cand.push(' ');
            cand.push_str(frags[i + 1]);
            i += 1;
        }
        if claimed_doc_path_shape(&cand) && !out.contains(&cand) {
            out.push(cand);
        }
        i += 1;
    }
    out
}

/// 片段是否以已知文档后缀结尾（不看路径形态——合并时要看的是分段，不是整条）。
fn doc_ext_suffix(seg: &str) -> bool {
    const DOC_EXTS: [&str; 4] = [".docx", ".xlsx", ".pptx", ".pdf"];
    let low = seg.to_lowercase();
    DOC_EXTS.iter().any(|e| low.ends_with(e))
}

/// 绝对路径形态：POSIX 根、盘符、UNC 前缀。
fn abs_path_shape(tok: &str) -> bool {
    tok.starts_with('/') || (tok.len() > 2 && tok.as_bytes()[1] == b':') || tok.starts_with(r"\\")
}

/// 一条候选是否成立：文档后缀 + 绝对路径形态。
fn claimed_doc_path_shape(tok: &str) -> bool {
    doc_ext_suffix(tok) && abs_path_shape(tok)
}

/// `next` 能不能并进 `cand`，把被空格切断的路径接回去。
///
/// 四条都要满足：
/// 1. `cand` 已经是绝对路径的起点（否则合并只会造出更长的垃圾串）；
/// 2. `next` 含 '/'——它是目录片段而不是普通词（"详见" 这类词不能并进来）；
/// 3. `next` 不含句读标点（"…docx 详见。" 那种，标点一进来就不是路径了）；
/// 4. 合并后按空格分段，**至多一个**分段带文档后缀，且那个分段在最后。
///    这条把「一条含空格的路径」和「两个相邻的路径」分开：
///    `/a/b.docx` + `/c/d.docx` 合并后有两个带后缀的分段 → 不并；
///    `/…/Application` + `Support/…/x.docx` 只有一个带后缀的分段 → 并。
fn join_path_fragment(cand: &str, next: &str) -> bool {
    if !abs_path_shape(cand) {
        return false;
    }
    if !next.contains('/') {
        return false;
    }
    if next.chars().any(|c| "。，；：！？、—".contains(c)) {
        return false;
    }
    let mut segs: Vec<&str> = cand.split(' ').collect();
    segs.push(next);
    let mut hits = 0;
    for (idx, seg) in segs.iter().enumerate() {
        if doc_ext_suffix(seg) {
            hits += 1;
            if idx + 1 != segs.len() {
                return false;
            }
        }
    }
    hits <= 1
}

/// 路径在磁盘上不存在（存在性判据的纯函数形态，便于单测）。
///
/// 存在性是幻觉守卫对「声称产出」的第一道判据：编造的路径经不起这一问。
/// 单独成函数是为了让它能单测——挂在循环里就没法单独钉住。
pub fn path_missing_on_disk(path: &str) -> bool {
    !std::path::Path::new(path).exists()
}
