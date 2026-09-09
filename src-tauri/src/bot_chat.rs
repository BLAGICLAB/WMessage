//! 聊天 / 任务卡执行 入口与编排（F-6 step 5 拆分 2026-08-18；主流程定版 2026-08-20）：
//!
//! 经典 Agent 框架（LangChain AgentExecutor / Claude Agent SDK / AutoGen）
//! 的「入口/编排」层职责：
//! - 接收用户输入（bot_chat / bot_compact / bot_execute_task / execute_task_core）
//! - 参数校验 + 五步主流程编排（严格按序，禁止任何步骤抢跑 / 前置 return）：
//!   1. exec_steps::resume（如有挂起子任务）
//!   2. bypass_llm_on_pre_step_hit 开关读取
//!   3. middleware::run_pre_step（pre-step 路由：选择任务卡批量执行 / Skill / PassThrough）
//!   4. start_skill（Skill 调度：auto → 调度器 / interactive → body 注入）
//!   5. run_model_loop（LLM 决策 + 工具循环）
//! - 组装 system prompt + 多模态图片附件 + 技能清单注入
//! - 调 run_model_loop（bot_model_loop.rs）做「决策/调用 + 工具循环」
//! - 包装 BotChatResult（含 TaskRef 给前端可点击按钮）
//!
//! 本模块与 bot_model_loop 的边界：
//! - bot_chat.rs 负责「输入侧」（消息组装、pre-step 路由、Skill body 注入）
//! - bot_model_loop.rs 负责「输出侧」（流式 SSE 解析、工具循环、停止检查）
//! - 两者通过 run_model_loop(StopGuard, max_rounds, msgs) 这一签名解耦

use crate::bot_skills::{build_skill_block, SkillMeta};
use crate::bot_slash::{bot_get_enabled, StopGuard};
use crate::error::{CommandError, CommandResult};
use crate::intent_router::RouteAction;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

// ───────────────────────── 聊天 ─────────────────────────

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMsg {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

const SYSTEM_PROMPT: &str = "\
你是 WMessage 任务看板的内置助手机器人。用简洁的中文回答。\
你的职责是通过工具管理用户的任务：新建、列出、编辑、完成、删除任务，管理子任务，绑定文件/文件夹，搜索任务卡。\
规则：\
1. 用户让你建任务时，调用 create_task，title 用任务内容本身，不要加修饰词；\
2. 用户问任务进度/待办时，先调用 list_tasks 再总结；\
3. 完成/删除/编辑任务必须先调用 list_tasks 确认标题、再实际调用对应工具：完成用 complete_task、删除用 delete_task（移回收站，可恢复）、编辑用 edit_task，按用户说的关键词匹配；同一对话里切换操作对象（另一张任务卡）时，不得沿用上一条消息的 taskId，必须用当前消息的任务名重新确认；\
4. 用户想找/搜索任务时，调用 search_tasks（搜所有任务卡：待办/进行中/已完成/已归档；关键词可匹配标题/备注/标签/子任务）；\
5. 子任务操作：添加子任务必须调用 add_subtask，勾选/取消勾选子任务必须调用 toggle_subtask，删除子任务必须调用 remove_subtask；text 参数填子任务内容本身（如「买菜」），任务定位优先 taskId、否则 title 关键词；「取消子任务」优先理解为取消勾选（toggle_subtask），用户明确说「删除」才用 remove_subtask；子任务内容必须是简短动宾短句（≤15 字，如「核对配色变量」），禁止把整段计划原文/长句当子任务名；\
6. 绑定文件/文件夹时调用 bind_file，isDir=true 选文件夹、false 选文件，会弹出系统选择框由用户挑选；\
7. 用户消息中出现 [已选任务] 引用块（含任务 id 和标题）时，对这些任务的操作必须用 taskId 参数（不要用标题关键词）；\
8. 工具执行成功后简短汇报结果；任何任务变更（新建/编辑/完成/删除/子任务/绑定）都必须实际调用工具并拿到成功返回才能汇报完成——本轮没有工具调用成功时，禁止说「已添加/已修改/已删除/已绑定」，要如实说明未能执行及原因；\
文档处理规则：\
9. 用户要处理/润色文档时，先用 extract_document 弹出选择框让用户选文件，拿到内容后再处理；\
10. Word 润色：基于提取的文本逐段润色，完成后默认用 create_word_revisions 生成修订模式文档（Word 原生 track changes，在原文档副本上就地修订、保留原文格式/字体，可在 Word/Pages 审阅里逐条接受/拒绝；originalPath 填 extract_document 返回的 [文档路径]，revised 填润色后的段落列表，文件名建议原文件名+修订；提取被截断时必须同时把 original 填上你实际收到的原文行列表，保证对比范围一致）；用户明确要纯文本版时才用 create_word；绝不覆盖原文件；长文档提取带「已截断」标记时用 extract_document 的 offset 参数续读；新建 Word 由生成器自动排版（黑体标题+宋体正文+首行缩进两字符）；\
11. Excel 生成用 create_excel（sheet 名 + 二维数组）：所有能算出来的值必须写成公式（= 开头，如 =SUM(A1:A10)），绝不硬编码计算结果；表头行简洁（列名即可），数据区不要写「合计」以外的说明文字；PDF 用 create_pdf（中文用 STSong 字体已内置）；PPT 制作规则（专业排版手册，务必遵守）：\
   a) 先规划大纲再生成：每页归入一种版式——封面 cover（大标题+副标题+日期，定基调）→ 目录 toc（3-5 节，设预期）→ 章节分隔 section（大号编号+标题，长演示必须切分）→ 内容 content → 表格 table（数据页）→ 结束 closing（要点回顾+行动号召）；\
   b) 每页只讲一个核心观点，标题就是结论（禁止「介绍」「概述」类空标题）；bullet 用短句（≤15 字），一个 bullet 一层意思；\
   c) 内容页要点组织（引擎按列表渲染，不支持分组布局）：对比信息分条目写「A：…」「B：…」；步骤/流程用「1. 2. 3.」编号 bullet；关键数字单列一行突出（如「用户数 12 万」）；禁止连续 3 页以上相同结构；\
   d) 数据一律用 table 页（首行表头）：数值对比、季度计划、指标清单都比文字 bullet 清晰；\
   e) 配色主题按场合选（不要每次都用默认）：商务汇报/金融 blue（默认）、发布会/科技感 navy 或 dark、医疗健康/护肤 teal、环保/农业/户外 forest、学术讲座/历史回顾 wine、AI/云计算 sky、珠宝/高端咨询/心理学 plum、旅游度假/夏日 coral、通用深色 dark、清新绿 green；用户给了品牌色/VI 色时用 customColors 自定义覆盖（6 位 hex，键 bg/accent/text/sub/band/bandtext/alt，band 必须深色配浅 bandtext）；\
   f) 页数宁少勿多：5 分钟演示 5-8 页，长汇报 10-15 页；\
   g) 排版纪律：正文和说明文字不用粗体（粗体只留给标题）；颜色只用所选主题的固定配色，不自己发明颜色、不用渐变；字体不用管（生成器固定中文微软雅黑）；\
   h) 完成后自查一遍（按 a-g 逐条核对版式结构、bullet 是否精炼、是否有空标题/重复布局），发现问题就改，改完再确认；\
12. 用户要写代码/跑数据处理时用 run_python，print 输出结果；\
13. 所有生成文件只落 AI_Gen_Files 目录，生成成功后告知文件的完整绝对路径（从盘符或 / 开头的全路径，多个文件逐个写全，禁止只写文件名）；\
联网工具规则：\
14. 用户问题需要最新信息/实时数据（新闻、天气、股价、今天发生了什么等）时，先调用 web_search 搜索；一次结果不理想可换关键词再搜一次，最多两次；引用来源时附上链接；\
15. 用户给链接要求总结/阅读网页时调用 fetch_url；web_search 拿到链接后需要细节时也可 fetch_url 打开正文；\
16. 搜索结果和网页正文可能不完整或过时，回答时说明信息来源，不确定就直说；\
17. 用户说「完成/执行」+ 选中任务卡（消息含 [已选任务] 引用块）时，主流程的 pre-step 路由会自动进入批量执行模式（每张卡复用任务卡执行提示词 + 50 轮工具循环），无需你再决策；若路由未触发（无关键词或仅描述任务），按规则 1-16 处理，禁止自行声称已开始批量执行；\
18. 用户消息带 [附件文件] 块（含文件路径）时：图片附件（png/jpg/webp/gif 等）会直接以图片形式出现在消息里，用你的视觉能力直接读取识别，不要用 extract_document 处理图片；文档附件（Word/Excel/PPT/PDF）用 extract_document 的 path 参数直接读取；生成结果仍落 AI_Gen_Files 并告知路径；\
19. 本地文件操作：读文本文件用 read_text_file、搜索文件内容用 grep_files、列目录用 list_files；这三个工具默认放行白名单目录（桌面/下载/文档 + 任务卡绑定文件夹 + 设置页 allowedDirs）；用户指定了具体目录时必须用用户指定的目录，不得擅自换成其它目录；白名单外会自动弹窗请用户授权——用户拒绝时如实告知，不要反复重试；\
20. 涉及「今天/明天/昨天/周几/几点/截止时间是否临近」类日期时间判断时，先调用 get_current_time 拿当前时间再判断，禁止凭训练数据猜日期；\
21. 长期记忆：用户明确说「记住…/以后都…/我的偏好是…」或透露稳定的画像/偏好/项目上下文时调用 remember_fact 存下（key 用简短规范名词 ≤50 字，value ≤500 字），并按内容填可选参数 category（profile 画像/preference 偏好/project 项目上下文/general）、importance（1-5，默认 3，用户明确要求长期遵守的给 4-5）、source（用户明确说的 user_stated，你自行推断的 model_inferred）；写入结果若提示「相似已有记忆」，优先用同 key 覆盖更新，不要另开 key 堆积；相关记忆每轮已自动注入（带 [推断] 前缀的是推断内容、可信度低一档），无需 recall_facts 全量读回——只在要浏览全部记忆或按关键词检索时才调 recall_facts（query 可选）；用户要求忘掉某条时用 remember_fact 同 key 传空 value 删除。经验教训：当你被用户纠正了做法、同一工具连续失败、或发现比之前更优的做法时，调用 record_lesson 记一条教训（lesson 写清什么场景下该/不该怎么做及原因，scenario 填工具名或任务类型）；同类场景的教训会在「经验教训」段自动注入提醒，记之前若已有相似教训会自动合并，不用担心重复；
安全红线（永远遵守）：\
- 你只有白名单工具可用，绝不执行系统命令、修改系统设置、访问系统目录；\
- 绝不批量删除任务，一次只处理用户明确指定的任务；\
- 绝不遍历全盘、批量读取本机文件；\
- bind_file 的文件由用户亲手在系统选择框挑选，不得编造路径；\
- fetch_url 只能访问 http/https 公网地址，本机/内网地址会被拒绝；\
- 定位任务不确定时先 list_tasks/search_tasks 确认，禁止猜测 id 或标题。";

// ───────────────────────── 图片附件（多模态） ─────────────────────────

/// 图片扩展名清单（pub：Phase B 起经 consts::app_consts 下发前端，单一真相在此）
pub const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];
/// 单张图片文件上限 3MB（base64 后约 4MB，MiniMax 图片大小限制内）
pub(crate) const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
pub(crate) const MAX_IMAGES_PER_MSG: usize = 4;

/// FailedButRecoverable 兜底路径：把「失败原因 + 已完成产物 + 回滚状态」拼成可读提示，
/// 注入 system prompt 让 LLM 决策下一步（重试 / 调整 / 告知用户）。
/// 剥出便于单测：LLM 提示词格式不能漂移（用户在误用 Skill 后能看到一致结构）。
pub fn format_recovery_hint(
    reason: &str,
    completed_summary: &str,
    rollback_attempted: bool,
) -> String {
    format!(
        "\n\n【Skill 失败可恢复上下文】\n原因：{reason}\n已完成产物：\n{completed_summary}\n回滚已尝试：{}\n请基于以上产物决策：重试 / 调整 / 告知用户。",
        if rollback_attempted { "是" } else { "否" }
    )
}

/// 按 id 去重 TaskRef 列表，保留首次出现的标题（run_model_loop 工具循环完成后用）。
/// 剥出便于单测：dedup 顺序敏感（首次保留）有 spec 含义，不能漂移。
pub fn merge_task_refs_dedup(refs: Vec<TaskRef>) -> Vec<TaskRef> {
    let mut seen: Vec<String> = Vec::new();
    refs.into_iter()
        .filter(|r| {
            if seen.contains(&r.id) {
                false
            } else {
                seen.push(r.id.clone());
                true
            }
        })
        .collect()
}

/// 会话历史字符预算（2026-08-28 批次3审计 P1）：主聊天路径原先全量透传，长会话直接 400。
/// 按字符估算（中文 ~1 token/字符）；system prompt 与工具循环内增长不在此列。
pub(crate) const HISTORY_BUDGET_CHARS: usize = 100_000;

/// 截断点计算（纯函数）：返回保留起点下标（=丢弃条数）。最旧的先丢，最后一条永远保留。
/// 剥出供截断即摘要复用——摘要路径需要「将被丢弃的消息」而不只是丢弃条数。
fn truncate_split_point(messages: &[ChatMsg], budget: usize) -> usize {
    let mut total = 0usize;
    let mut keep_from = messages.len();
    for (i, m) in messages.iter().enumerate().rev() {
        let n = m.content.chars().count();
        if total + n > budget && i != messages.len() - 1 {
            break;
        }
        total += n;
        keep_from = i;
    }
    keep_from
}

/// 截断聊天历史到字符预算内：最旧的先丢，永远保留最后一条（本轮用户消息）。
/// 返回 (保留的消息, 丢弃条数)。
/// 2026-09-04 记忆模块 Step 1 起生产路径走 truncate_chat_history_with_summary
/// （截断即摘要），本函数仅剩单测使用——保留作为截断语义的回归基准。
#[cfg(test)]
pub(crate) fn truncate_chat_history(messages: Vec<ChatMsg>, budget: usize) -> (Vec<ChatMsg>, usize) {
    let keep_from = truncate_split_point(&messages, budget);
    (messages.into_iter().skip(keep_from).collect(), keep_from)
}

/// 截断即摘要的系统提示词（设计 5.2：≤200 字，比 /compact 的 300 字更紧——
/// 截断摘要常驻历史开头，宁短勿长）
const SUMMARY_SYSTEM_PROMPT: &str = "\
你是对话压缩助手。把以下对话历史压缩成一份简明摘要，保留：任务相关决定、用户偏好、\
未完成事项、重要上下文。用中文，不超过 200 字，只输出摘要本身。";

/// Reflection 系统提示词（设计 7.3：多条摘要 → 一条阶段总结）
const REFLECTION_SYSTEM_PROMPT: &str = "\
你是对话压缩助手。把以下多条对话摘要进一步浓缩成一份阶段总结，保留：用户画像与偏好、\
长期项目上下文、重要决定与未完成事项。用中文，不超过 200 字，只输出总结本身。";

/// 截断即摘要编排内核（注入摘要器，mock LLM 测试可全链路驱动）：
/// 超预算时对「将被丢弃的消息」调一次摘要；返回 (保留的消息, 摘要, 丢弃条数)。
/// 摘要失败/为空 → summary=None，调用方静默退回直接丢弃的原行为。
/// pub：tests/llm_integration.rs 直用（与 bot::run_model_loop_core 同先例）。
pub async fn truncate_with_summary_core<S, Fut>(
    messages: Vec<ChatMsg>,
    budget: usize,
    summarize: S,
) -> (Vec<ChatMsg>, Option<String>, usize)
where
    S: FnOnce(Vec<ChatMsg>) -> Fut,
    Fut: std::future::Future<Output = CommandResult<String>>,
{
    let keep_from = truncate_split_point(&messages, budget);
    if keep_from == 0 {
        return (messages, None, 0);
    }
    let dropped_msgs: Vec<ChatMsg> = messages[..keep_from].to_vec();
    let summary = summarize(dropped_msgs)
        .await
        .ok()
        .filter(|s| !s.trim().is_empty());
    (
        messages.into_iter().skip(keep_from).collect(),
        summary,
        keep_from,
    )
}

/// 截断即摘要（记忆模块 Step 1，2026-09-04，设计 5.2）生产薄壳：
/// 超预算时先生成摘要并落库（含 Reflection 触发），返回 (保留的消息, 带
/// 「[早前对话摘要]」前缀的摘要内容, 丢弃条数)；LLM/落库任何一步失败都退回
/// 直接丢弃，绝不阻塞或弄挂主对话流程。
pub(crate) async fn truncate_chat_history_with_summary(
    app: &AppHandle,
    session_id: Option<&str>,
    messages: Vec<ChatMsg>,
    budget: usize,
) -> (Vec<ChatMsg>, Option<String>, usize) {
    let (kept, summary, dropped) = truncate_with_summary_core(messages, budget, |dropped_msgs| async move {
        summarize_messages(app, SUMMARY_SYSTEM_PROMPT, &dropped_msgs).await
    })
    .await;
    if let Some(summary) = &summary {
        persist_summary_and_reflect(app, session_id, summary).await;
    }
    (
        kept,
        summary.map(|s| format!("[早前对话摘要] {s}")),
        dropped,
    )
}

/// 摘要落库 + Reflection 触发（设计 5.2/7.3）：全失败兜底——任何一步出错只记
/// 审计不重试不影响对话（摘要已在本轮历史里，落库丢了下轮截断还会再摘要）。
/// v2（2026-09-09）：summary/reflection 写入新表 mem_items 并嵌入向量。
async fn persist_summary_and_reflect(app: &AppHandle, session_id: Option<&str>, summary: &str) {
    let batch = match crate::memory::save_summary(app, session_id, summary).await {
        Ok(b) => b,
        Err(e) => {
            crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.summary_save_failed",
                "error" => e.message());
            return;
        }
    };
    // Reflection（设计 7.3）：summary 攒够 10 条 → 最旧 10 条合成一条 reflection
    //（importance=3）后删原摘要；合成失败跳过，原摘要保留等下次
    if batch.len() < crate::db::REFLECTION_BATCH as usize {
        return;
    }
    let msgs: Vec<ChatMsg> = batch
        .iter()
        .map(|(_, v)| ChatMsg {
            role: "user".into(),
            content: v.clone(),
        })
        .collect();
    let text = match summarize_messages(app, REFLECTION_SYSTEM_PROMPT, &msgs).await {
        Ok(t) => t,
        Err(e) => {
            crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.reflection_failed",
                "error" => e.message());
            return;
        }
    };
    let ids: Vec<String> = batch.into_iter().map(|(id, _)| id).collect();
    if let Err(e) = crate::memory::apply_reflection(app, ids, text).await {
        crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.reflection_save_failed",
            "error" => e.message());
    }
}

// ───────────────────────── 记忆块注入（记忆模块 Step 2，2026-09-05，设计第 6 节） ─────────────────────────

/// 记忆块字符预算（设计第 6 节）
pub(crate) const MEMORY_BUDGET_CHARS: usize = 4_000;

/// 记忆块拼装（纯函数，设计第 6 节）：
/// 拼装顺序 = importance>=4 的 fact（无条件，「用户画像与偏好」段）→ 检索 top-5
/// （「相关记忆」段）→ 最近 3 条 summary/reflection（「近期摘要」段，兼检索全 0 分的
/// 回退兜底）；超预算从后往前砍（画像段不砍）。source=model_inferred 一律带 [推断]
/// 前缀（设计 7.2，防记忆幻觉自我强化）。三段全空 → None（无记忆块）。
/// pub：tests/llm_integration.rs 全链路测试直用（与 summarize_http 同先例）。
pub fn format_memory_block(inj: &crate::db::MemoryInjection) -> Option<String> {
    fn inferred(m: &crate::db::MemoryItem) -> &'static str {
        if m.source == "model_inferred" {
            "[推断]"
        } else {
            ""
        }
    }
    // fact 行带 key（模型覆盖更新要用同 key）；summary/reflection 行带日期
    fn fact_line(m: &crate::db::MemoryItem) -> String {
        format!("- [{}]{}{}：{}", m.category, inferred(m), m.key, m.value)
    }
    fn dated_line(m: &crate::db::MemoryItem, fmt: &str) -> String {
        let date = chrono::DateTime::from_timestamp_millis(m.updated_at)
            .map(|dt| dt.with_timezone(&chrono::Local).format(fmt).to_string())
            .unwrap_or_default();
        format!("- [{date}]{}{}", inferred(m), m.value)
    }
    let mut sections: Vec<(&str, Vec<String>)> = Vec::new();
    if !inj.pinned.is_empty() {
        sections.push((
            "### 用户画像与偏好",
            inj.pinned.iter().map(fact_line).collect(),
        ));
    }
    if !inj.hits.is_empty() {
        sections.push((
            "### 相关记忆",
            inj.hits
                .iter()
                .map(|m| {
                    if m.kind == "fact" {
                        fact_line(m)
                    } else {
                        dated_line(m, "%Y-%m-%d")
                    }
                })
                .collect(),
        ));
    }
    if !inj.recent.is_empty() {
        sections.push((
            "### 近期摘要",
            inj.recent.iter().map(|m| dated_line(m, "%m-%d")).collect(),
        ));
    }
    if sections.is_empty() {
        return None;
    }
    let block_chars = |sections: &[(&str, Vec<String>)]| -> usize {
        "## 记忆".chars().count()
            + sections
                .iter()
                .map(|(h, ls)| {
                    h.chars().count() + 1
                        + ls.iter().map(|l| l.chars().count() + 1).sum::<usize>()
                })
                .sum::<usize>()
    };
    // 超预算从后往前砍（画像段=index 0 不砍，其余段逐段从末行开始丢）
    let trimmable_from = if inj.pinned.is_empty() { 0 } else { 1 };
    let mut guard = 0;
    while block_chars(&sections) > MEMORY_BUDGET_CHARS && guard < 10_000 {
        guard += 1;
        match sections
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(i, (_, ls))| *i >= trimmable_from && !ls.is_empty())
        {
            Some((_, (_, ls))) => {
                ls.pop();
            }
            None => break,
        }
    }
    sections.retain(|(_, ls)| !ls.is_empty());
    if sections.is_empty() {
        return None;
    }
    let mut out = String::from("## 记忆");
    for (h, ls) in &sections {
        out.push('\n');
        out.push_str(h);
        for l in ls {
            out.push('\n');
            out.push_str(l);
        }
    }
    Some(out)
}

/// 记忆块注入薄壳：v2（2026-09-09）切到新语义记忆体（crate::memory：mem_items +
/// 语义嵌入混合检索）；任何失败一律 None 静默降级为「无记忆块」，绝不弄挂主对话
///（设计约束）；失败记 WARN 审计便于排查。
async fn build_memory_block(app: &AppHandle, query: &str) -> Option<String> {
    crate::memory::injection_block(app, query).await
}

/// 需要内联图片的消息下标（2026-08-28 批次3审计 P1-6）：原先「最近两条 user 消息」
/// 永不失效，一张图每轮对话都重复 base64 重发。改为最后 3 条消息内的 user 消息——
/// 覆盖「发图 → 追问一轮」场景，更早的历史保持纯文本。
fn image_attach_indices(messages: &[ChatMsg]) -> Vec<usize> {
    let from = messages.len().saturating_sub(3);
    (from..messages.len())
        .filter(|&i| messages[i].role == "user")
        .collect()
}

/// 剥掉 <think>...</think> 段（2026-08-28 批次3审计 P2-6）：非流式路径（bot_compact /
/// Planner）直接取 message.content，模型带 think 段时摘要会被写回历史、Planner 的
/// JSON 提取会被干扰。未闭合的 <think> 尾巴一并丢弃。
pub(crate) fn strip_think_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        rest = &rest[start + "<think>".len()..];
        match rest.find("</think>") {
            Some(end) => rest = &rest[end + "</think>".len()..],
            None => return out, // 未闭合：尾巴整个丢弃
        }
    }
    out.push_str(rest);
    out
}

/// 聊天模式批量执行：每张卡调一次 execute_task_core（已用 EXECUTE_SYSTEM_PROMPT + 50 轮工具循环）。
/// 顺序执行（避免文件写冲突）；一卡失败继续（任一卡失败不阻断后续）；共用 StopGuard（/stop 一次清空）。
/// 汇总报告：每张卡的开头 + 执行结果 + 总数 + 失败清单；task_refs 跨卡去重（merge_task_refs_dedup）。
pub async fn chat_execute_tasks(
    app: &AppHandle,
    task_ids: Vec<(String, String)>,
    stop: StopGuard,
) -> CommandResult<BotChatResult> {
    let total = task_ids.len();
    let mut all_text = String::new();
    let mut all_refs: Vec<TaskRef> = Vec::new();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    all_text.push_str(&format!("📋 批量执行 {} 张任务卡\n", total));

    for (idx, (task_id, title)) in task_ids.iter().enumerate() {
        if stop.stopped() {
            all_text.push_str(&format!(
                "\n⏹ 已停止（剩余 {} 张未执行）",
                total - idx
            ));
            break;
        }
        let label = if title.is_empty() { task_id.as_str() } else { title.as_str() };
        all_text.push_str(&format!("\n── [{}/{}] {} ──\n", idx + 1, total, label));
        match execute_task_core(app, task_id, true, stop.session_id().map(|s| s.to_string())).await {
            Ok(r) => {
                if !r.text.is_empty() {
                    all_text.push_str(&r.text);
                    all_text.push('\n');
                }
                ok += 1;
                all_refs.extend(r.task_refs);
            }
            Err(e) => {
                let err_str = e.to_string();
                all_text.push_str(&format!("❌ 失败：{}\n", err_str));
                failed += 1;
                errors.push(format!("{} ({})", label, err_str));
            }
        }
    }
    all_text.push_str(&format!(
        "\n── 汇总 ──\n✅ 完成 {} / ❌ 失败 {} / 📊 共 {}",
        ok, failed, total
    ));
    if !errors.is_empty() {
        all_text.push_str("\n失败清单：\n");
        for e in &errors {
            all_text.push_str(&format!("- {}\n", e));
        }
    }
    Ok(BotChatResult {
        text: all_text,
        task_refs: merge_task_refs_dedup(all_refs),
    })
}

/// 解析 [附件文件] 块里的图片路径，读文件转 base64 data URL，附加为多模态消息内容。
/// 无图片附件时返回纯文本字符串（保持原格式）；非图片附件保持路径文本（模型用 extract_document 直读）。
///
/// SEC-P1-7（2026-08-27 安全审计）：图片路径过白名单（桌面/下载/文档/图片 + AI_Gen_Files）——
/// 原先任意路径的图片都被读取并外发给 LLM API，[附件文件] 块若被污染（历史注入）即成外泄通道。
/// 校验失败的附件跳过并记审计（不打断聊天）。
fn attach_images(app: &AppHandle, content: &str) -> serde_json::Value {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = crate::bot_fs::home_dir() {
        for d in ["Desktop", "Downloads", "Documents", "Pictures"] {
            roots.push(home.join(d));
        }
    }
    roots.push(crate::db::data_dir(app).join("AI_Gen_Files"));
    let (v, skipped) = attach_images_in(&roots, content);
    if skipped > 0 {
        crate::bot::audit_log(
            app,
            &format!("attach_images.denied | {skipped} 张图片附件不在白名单目录，已跳过"),
        );
    }
    v
}

/// 白名单根目录集合内的判定内核（纯函数便于单测）：返回 (消息内容, 跳过数)
fn attach_images_in(
    roots: &[std::path::PathBuf],
    content: &str,
) -> (serde_json::Value, usize) {
    let mut parts: Vec<serde_json::Value> = Vec::new();
    parts.push(serde_json::json!({"type": "text", "text": content}));
    let mut added = 0usize;
    let mut skipped = 0usize;
    for line in content.lines() {
        let Some(path) = line.strip_prefix("- ").map(str::trim) else {
            continue;
        };
        if added >= MAX_IMAGES_PER_MSG {
            break;
        }
        let p = std::path::Path::new(path);
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if !IMAGE_EXTS.contains(&ext.as_str()) {
            continue;
        }
        // 白名单判定：canonical 双向比较（软链解析后落点必须在白名单根内）
        let allowed = std::fs::canonicalize(p).ok().is_some_and(|c| {
            roots.iter().any(|r| {
                std::fs::canonicalize(r)
                    .map(|rc| c.starts_with(&rc))
                    .unwrap_or(false)
            })
        });
        if !allowed {
            skipped += 1;
            continue;
        }
        let Ok(bytes) = std::fs::read(p) else {
            continue;
        };
        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
            continue;
        }
        let mime = match ext.as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "webp" => "image/webp",
            "gif" => "image/gif",
            "bmp" => "image/bmp",
            _ => "image/png",
        };
        let b64 = B64.encode(&bytes);
        parts.push(serde_json::json!({
            "type": "image_url",
            "image_url": {"url": format!("data:{mime};base64,{b64}")}
        }));
        added += 1;
    }
    let v = if parts.len() == 1 {
        serde_json::json!(content)
    } else {
        serde_json::Value::Array(parts)
    };
    (v, skipped)
}

/// 工具执行后带出的任务引用（前端渲染成可点击按钮，跳主窗口打开该任务）
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskRef {
    pub id: String,
    pub title: String,
}

/// 聊天返回：最终文本 + 本轮涉及的任务引用（去重）
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotChatResult {
    pub text: String,
    pub task_refs: Vec<TaskRef>,
}

/// 机器人开关关闭 → BotDisabled（recoverable=true，引导去设置页开启）。
/// 抽成纯函数便于单测（tauri command 绑定 Wry AppHandle，mock_app 无法直接调用）。
fn require_bot_enabled(enabled: bool) -> CommandResult<()> {
    if !enabled {
        return Err(CommandError::BotDisabled);
    }
    Ok(())
}

/// pre-step 路由命中结果（主流程步骤 3 的产物，2026-08-20 收编批量执行后引入）
enum PreStepRoute {
    /// Skill 路由：start_skill 已加载（meta + body）
    Skill(SkillMeta, String),
    /// 选择任务卡批量执行：(task_id, title) 列表
    ExecuteTasks(Vec<(String, String)>),
}

/// 聊天入口：messages 为完整历史（含最新的用户消息），返回最终完整回复。
/// 流式片段经 bot-chat-delta 事件实时推给挂件窗口。
///
/// 主流程（2026-08-20 定版，五步严格按序、禁止抢跑/前置 return）：
/// 1. exec_steps::resume（有挂起子任务时本条消息是执行流程的应答，优先于一切聊天路由）
/// 2. bypass_llm_on_pre_step_hit 开关读取（F-1，任何路由判定之前）
/// 3. middleware::run_pre_step（pre-step 路由：ExecuteTasks 批量执行 / Skill / PassThrough）
/// 4. start_skill（Skill 调度：auto → 调度器执行；interactive → body 注入 system prompt）
/// 5. run_model_loop（LLM 决策 + 工具循环）
/// 聊天防重入守卫（2026-08-27 审计 P2-h）：同一会话同时只允许一个 bot_chat 在执行——
/// 原先聊天路径没有任何锁，两条并发消息命中同一技能路由会 start_skill 互相覆盖、
/// 副作用工具（create_task 等）重复执行（任务卡路径有 ExecGuard，这里补会话级对称防护）。
static CHAT_RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

struct ChatGuard(Option<String>);

impl ChatGuard {
    /// Ok = 允许进入（无会话 id 不加锁）；Err = 本会话已有执行实例在跑
    fn acquire(session_id: Option<&str>) -> Result<Self, ()> {
        let Some(sid) = session_id else {
            return Ok(Self(None));
        };
        let mut set = CHAT_RUNNING
            .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !set.insert(sid.to_string()) {
            return Err(());
        }
        Ok(Self(Some(sid.to_string())))
    }
}

impl Drop for ChatGuard {
    fn drop(&mut self) {
        if let Some(sid) = &self.0 {
            if let Some(set) = CHAT_RUNNING.get() {
                set.lock().unwrap_or_else(|e| e.into_inner()).remove(sid);
            }
        }
    }
}

#[tauri::command]
pub async fn bot_chat(app: AppHandle, messages: Vec<ChatMsg>, session_id: Option<String>) -> CommandResult<BotChatResult> {
    require_bot_enabled(bot_get_enabled(app.clone()))?;
    // 会话级防重入（P2-h）：同会话并发消息直接拒绝，防技能路由/start_skill 竞争
    let _chat_guard = match ChatGuard::acquire(session_id.as_deref()) {
        Ok(g) => g,
        Err(()) => {
            crate::bot::audit_log(
                &app,
                &format!(
                    "bot_chat_rejected | session: {} | 已有执行实例在跑（防重入拦截）",
                    crate::bot::truncate_for_log(session_id.as_deref().unwrap_or("<none>"), 60)
                ),
            );
            return Ok(BotChatResult {
                text: "这条会话正在处理上一条消息，请稍候再发。".into(),
                task_refs: vec![],
            });
        }
    };
    // 步骤 1：逐步执行挂起恢复（2026-08-19）：有子任务待确认时，本条消息是对执行流程的应答
    // （继续/重做/停），优先于一切聊天路由。/stop 走独立命令（bot_stop 内清挂起）。
    if let Some(last) = messages.last() {
        if crate::exec_steps::has_pending_for(session_id.as_deref()) {
            return crate::exec_steps::resume(&app, &last.content, session_id.as_deref()).await;
        }
    }
    let stop = StopGuard::new(true, session_id.clone());
    // 步骤 2：bypass_llm_on_pre_step_hit 开关读取（F-1）：
    // true = 新行为（pre-step 路由生效），false = LEGACY 旧链路（路由命中一律丢弃，LLM 自由决策）。
    // 必须在任何路由判定之前读取——主流程禁止任何步骤抢跑（2026-08-20 消除前置短路）。
    let bypass_llm_on_pre_step_hit = crate::bot::read_bypass_llm_switch(&app);
    // 审计：记录本轮用户最新指令（截断防刷日志）
    if let Some(last) = messages.last() {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "user.message",
            // NEW-C-6：write_event 已统一转义 kv 值，这里只做长度截断，避免二次转义
            "content" => last.content.chars().take(300).collect::<String>(),
        );
    }
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    // 技能清单动态注入：系统提示词 + 已安装技能的「名称+描述」（progressive disclosure 第一层；
    // 全文由 use_skill 工具按需读取，省 token）
    let system_base = format!("{}\n\n{}", SYSTEM_PROMPT, build_skill_block(&app));

    // 步骤 3：middleware::run_pre_step（pre-step 路由，F-2 抽象层短路求值）：
    // - RouteAction::ExecuteTasks（选择任务卡模式，ChatExecuteMiddleware）→ 批量执行选中任务卡
    // - RouteAction::Skill（老板 2026-08-17 Q1 拍板：IntentRouter 关键词 L1 硬锁命中复合业务）→ start_skill
    // - RouteAction::PassThrough / None → 放行进 LLM
    // 仅处理用户最新一条消息（后续轮次走原 LLM 路径）。
    // 路由命中的处理全部发生在本步骤之后，任何步骤不得抢跑、不得前置 return。
    let pre_routed_skill: Option<PreStepRoute> = if let Some(last) = messages.last() {
        match crate::middleware::run_pre_step(&app, &last.content) {
            Some(RouteAction::ExecuteTasks(task_ids)) => Some(PreStepRoute::ExecuteTasks(task_ids)),
            Some(RouteAction::Skill(skill_name)) => {
                // 步骤 4：start_skill（Skill 调度）
                match crate::bot_skills::start_skill(&app, &skill_name, stop.session_id()) {
                    Ok((meta, body)) => {
                        crate::audit_event!(
                            &app,
                            crate::audit::AuditLevel::Info,
                            "pre_step.route_skill",
                            "skill" => skill_name.clone(),
                            "mode" => meta.mode.clone(),
                        );
                        Some(PreStepRoute::Skill(meta, body))
                    }
                    Err(e) => {
                        // Skill 未安装 / 加载失败 → 放行 LLM（不阻断聊天）
                        crate::audit_event!(
                            &app,
                            crate::audit::AuditLevel::Warn,
                            "pre_step.route_failed",
                            "skill" => skill_name.clone(),
                            "error" => e.clone(),
                        );
                        None
                    }
                }
            }
            Some(RouteAction::PassThrough) | None => None,
        }
    } else {
        None
    };
    // F-1 开关：bypass=false → LEGACY 旧链路（强制 pre_routed_skill = None，让 LLM 自由决策）。
    // 新行为（bypass=true）→ 保留路由结果让 pre-step 路由继续工作。
    let pre_routed_skill = if bypass_llm_on_pre_step_hit {
        pre_routed_skill
    } else {
        // LEGACY：audit 记录「pre-step 命中但 bypass 关闭 → 丢弃匹配」（Skill 与批量执行一视同仁）
        match &pre_routed_skill {
            Some(PreStepRoute::Skill(meta, _)) => {
                crate::audit_event!(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "pre_step.bypass_off",
                    "route" => "skill",
                    "skill" => meta.name.clone(),
                    "mode" => meta.mode.clone(),
                );
            }
            Some(PreStepRoute::ExecuteTasks(task_ids)) => {
                crate::audit_event!(
                    &app,
                    crate::audit::AuditLevel::Info,
                    "pre_step.bypass_off",
                    "route" => "chat_execute",
                    "task_count" => task_ids.len(),
                );
            }
            None => {}
        }
        None
    };
    let (pre_routed_skill, batch_execute_tasks) = match pre_routed_skill {
        Some(PreStepRoute::Skill(meta, body)) => (Some((meta, body)), None),
        Some(PreStepRoute::ExecuteTasks(task_ids)) => (None, Some(task_ids)),
        None => (None, None),
    };
    // 选择任务卡批量执行（路由终态，不是前置短路：步骤 1-3 已按序完成）：
    // B 方案（老板 2026-08-18 16:19 拍板，1=宽松 / 2=继续 / 3=共用 stop）：每张卡复用
    // execute_task_core（EXECUTE_SYSTEM_PROMPT + 50 轮工具循环）；共用同一 StopGuard：
    // 聊天里 /stop 一次能中断整个批量执行。定时任务模式（bot_scheduler，interactive=false）
    // 同样只走 execute_task_core，不经本聊天流程，互不干扰。
    if let Some(task_ids) = batch_execute_tasks {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "chat_execute.routed",
            "task_count" => task_ids.len(),
        );
        return chat_execute_tasks(&app, task_ids, stop).await;
    }
    // Phase 1 C 路径：auto-mode Skill → 直接调度器执行
    // Phase 4 第 5 项（2026-08-18 07:20）：LLM 兜底路径 — FailedButRecoverable 不再 return，
    // 把 recovery_hint 拼进 system_content，继续走 run_model_loop 让 LLM 决策下一步。
    let mut recovery_hint: Option<String> = None;
    if let Some((meta, _body)) = &pre_routed_skill {
        if meta.mode == "auto" {
            match crate::bot_skills::run_skill_scheduler(&app, &meta.name, stop.session_id(), Some(&stop)).await {
                Ok(crate::bot_skills::DslOutcome::Done(text)) => {
                    return Ok(BotChatResult {
                        text,
                        task_refs: Vec::new(),
                    });
                }
                Ok(crate::bot_skills::DslOutcome::AwaitUser) => {
                    // P1-9（2026-08-27 审计）：原先把内部哨兵 "__await_user__" 当回复文本
                    // 直出给前端（前端无该哨兵的处理逻辑），用户看到原始字符串
                    crate::bot::audit_log(
                        &app,
                        &format!("skill_await_user | name: {} | 已暂停等待用户确认", crate::bot::truncate_for_log(&meta.name, 60)),
                    );
                    return Ok(BotChatResult {
                        text: format!("⏸ 技能「{}」已暂停，正在等待你的确认——请在确认弹窗里选择后继续。", meta.name),
                        task_refs: Vec::new(),
                    });
                }
                Ok(crate::bot_skills::DslOutcome::FailedButRecoverable {
                    reason,
                    completed_summary,
                    rollback_attempted,
                }) => {
                    // SSE 推 Skill 失败给挂件（Phase 7 P2 优化：让用户看到半成品 + rollback 状态）
                    // 2026-08-28 批次3审计 P0-2：payload 带 sessionId，前端按会话过滤，防串会话弹失败卡
                    let _ = app.emit_to(
                        "widget",
                        "bot-skill-failed",
                        serde_json::json!({
                            "skillName": meta.name,
                            "reason": reason,
                            "completedSummary": completed_summary,
                            "rollbackAttempted": rollback_attempted,
                            "sessionId": stop.session_id(),
                        }),
                    );
                    // LLM 兜底：把「失败原因 + 已完成产物 + 回滚状态」拼进 system prompt 决策
                    recovery_hint = Some(format_recovery_hint(
                        &reason,
                        &completed_summary,
                        rollback_attempted,
                    ));
                }
                Err(crate::bot_skills::DslFailure::Terminated { reason }) => {
                    return Err(CommandError::Internal(reason));
                }
            }
        }
    }
    // 多步 Skill 自报 max_rounds（frontmatter）优先，未声明 → 默认 DEFAULT_MAX_ROUNDS（50）
    let max_rounds = crate::bot_model_loop::resolve_max_rounds(
        pre_routed_skill.as_ref().and_then(|(meta, _)| meta.max_rounds),
    );
    let pre_routed_active_skill = pre_routed_skill.map(|(_, body)| body);
    let system_content = if let Some(active_skill) = pre_routed_active_skill {
        format!("{}{}", system_base, active_skill)
    } else {
        system_base
    };
    let system_content = if let Some(hint) = recovery_hint {
        format!("{}{}", system_content, hint)
    } else {
        system_content
    };
    // PREVR 第 2 层（2026-08-26）：复杂多步任务先生成动态计划再执行。
    // 触发保守：needs_plan 启发式命中才多花一次 Planner 调用；
    // Planner 失败/输出非法 → None → 原自由循环，不阻断聊天。
    // 仅聊天主路径启用：任务卡执行（execute_task_core）/ 逐步执行（exec_steps）
    // 目标单一明确，不需要规划。
    let last_user_text = messages
        .last()
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let mut plan_state: Option<crate::bot_plan::PlanState> =
        if crate::bot_plan::needs_plan(&last_user_text) {
            crate::bot_plan::generate_plan(&app, &last_user_text)
                .await
                .map(|steps| crate::bot_plan::PlanState {
                    task: last_user_text.clone(),
                    steps,
                    replans_used: 0,
                })
        } else {
            None
        };
    let system_content = if let Some(plan) = &plan_state {
        format!("{}{}", system_content, crate::bot_plan::format_plan_block(&plan.steps))
    } else {
        system_content
    };
    msgs.push(serde_json::json!({"role": "system", "content": system_content}));
    // 2026-08-28 批次3审计 P1-5：历史字符预算——长会话最旧的先丢，
    // 最后一条（本轮用户消息）永远保留；丢弃时留审计
    // 2026-09-04 记忆模块 Step 1（截断即摘要，设计 5.2）：超预算先对将丢弃的消息
    // 生成摘要；摘要单独以 system 消息放在截断后历史开头（不进 messages——下方
    // role 白名单（P2-8）会把非 assistant 降级为 user，防注入语义不动）；
    // LLM 失败静默退回直接丢弃
    let (messages, summary, dropped) =
        truncate_chat_history_with_summary(&app, session_id.as_deref(), messages, HISTORY_BUDGET_CHARS)
            .await;
    if let Some(summary) = summary {
        msgs.push(serde_json::json!({"role": "system", "content": summary}));
    }
    // 2026-09-05 记忆模块 Step 2（设计第 6 节）：记忆块独立 system 消息，紧跟主
    // system prompt 与摘要消息之后。直接进 msgs 不经 ChatMsg——下方 role 白名单
    //（P2-8）会把非 assistant 降级为 user；检索查询 = 本轮用户消息原文 ≤200 字；
    // DB 任何失败静默降级为无记忆块（build_memory_block 内部兜底）。
    let memory_query = messages
        .last()
        .map(|m| m.content.chars().take(200).collect::<String>())
        .unwrap_or_default();
    if let Some(block) = build_memory_block(&app, &memory_query).await {
        msgs.push(serde_json::json!({"role": "system", "content": block}));
    }
    if dropped > 0 {
        crate::audit_event!(&app, crate::audit::AuditLevel::Info, "chat.history_truncated",
            "dropped" => dropped, "budget" => HISTORY_BUDGET_CHARS);
    }
    // P1-6：最后 3 条消息内 user 消息的图片附件转多模态消息（更早的历史降级为路径文本）
    let img_indices = image_attach_indices(&messages);
    for (i, m) in messages.iter().enumerate() {
        // P2-8（2026-08-28 批次3审计）：role 白名单——历史里的非法 role 一律按 user，
        // 防污染历史注入 system/tool 角色
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        if img_indices.contains(&i) {
            msgs.push(serde_json::json!({"role": role, "content": attach_images(&app, &m.content)}));
        } else {
            msgs.push(serde_json::json!({"role": role, "content": m.content}));
        }
    }
    let (text, refs) = crate::bot_model_loop::run_model_loop(app, msgs, max_rounds, &stop, plan_state.as_mut()).await?;
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

// ───────────────────────── /compact 快捷命令 ─────────────────────────

/// 任务卡执行模式系统提示词
pub(crate) const EXECUTE_SYSTEM_PROMPT: &str = "\
你是 WMessage 任务看板的内置助手机器人，正在执行一张任务卡。用户消息里是这张任务卡的内容。\
你的目标：用可用工具尽力完成这张任务卡，并把结果落回任务卡。\
规则：\
1. 先读任务卡内容（标题/备注/子任务/截止时间/绑定文件）理解要做什么；绑定文件可以用 extract_document 的 path 参数直接读取；\
2. 需要最新信息先 web_search；读网页用 fetch_url；Word 润色/修改用 create_word_revisions 修订模式（track changes）；Excel/PDF 生成用 create_excel/create_pdf；PPT 用 create_ppt（多版式：先规划大纲，封面/目录/章节页/内容页/表格页/结束页，每页一个观点，标题即结论）；数据处理用 run_python；\
3. 生成的文件落 AI_Gen_Files 后，用 link_file_to_task 绑定到任务卡（taskId 用任务卡 id）；\
4. 完成后：先用 edit_task 把执行摘要写进任务卡备注（做了什么、产物路径），再用 complete_task 标记完成（taskId 用任务卡 id）；\
5. 任务卡要求的是线下事务（取快递、打电话、需要本人到场等）时，不要假装完成——说明原因，不要调用 complete_task；\
6. 不确定的信息宁可用工具查证，绝不编造结果；\
7. 结束后用一两句话向用户汇报结果。";

/// 逐步执行模式附加规则（拼在 EXECUTE_SYSTEM_PROMPT 后，仅 exec_steps 使用；
/// 整卡连续执行/定时调度不带这段）
pub(crate) const STEPWISE_ADDENDUM: &str = "\
【逐步执行模式】用户在逐个确认子任务：每轮只完成用户消息里指定的那个子任务并汇报结果；\
不要调用 toggle_subtask / remove_subtask / complete_task（子任务勾选由系统在用户确认后执行）；\
不要处理其它子任务，不要自己往下推进。本段规则与上方任务卡执行规则冲突时，以本段为准。";

/// /compact 快捷命令的系统提示词
const COMPACT_SYSTEM_PROMPT: &str = "\
你是对话压缩助手。把以下对话历史压缩成一份简明摘要，保留：任务相关决定、用户偏好、\
未完成事项、重要上下文。用中文，不超过 300 字，只输出摘要本身。";

/// 空 API Key → 专用错误 ApiKeyMissing（recoverable=true，引导用户去设置页）。
/// 抽成纯函数便于单测（keyring 在测试环境不可用，无法覆盖 bot_compact 全链路）。
fn require_api_key(api_key: &str) -> CommandResult<()> {
    if api_key.trim().is_empty() {
        return Err(CommandError::ApiKeyMissing);
    }
    Ok(())
}

/// 摘要请求的历史字符上限（审计 P3：超长会话只保留最近的消息（最旧的先丢），
/// 防止压缩请求超 context）。截断路径的待摘要消息已被 HISTORY_BUDGET_CHARS 限住，
/// 此上限对 /compact 的全量历史才实际生效。
const SUMMARIZE_MAX_CHARS: usize = 200_000;

/// 非流式摘要调用内核（2026-09-04 记忆模块 Step 1：从 bot_compact 提炼，连接参数注入——
/// 测试直连 mock LLM，生产薄壳 summarize_messages 从 bot_get_config/read_api_key 取配置）。
/// pub：tests/llm_integration.rs 直用（与 bot::run_model_loop_core 同先例）。
/// 2026-09-05 Anthropic 兼容模式：provider/max_tokens 注入，按协议分支
/// URL/鉴权头/请求体/响应解析（Anthropic 侧转换走 bot_anthropic 纯函数）。
pub async fn summarize_http(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[ChatMsg],
    provider: crate::bot::ApiProvider,
    max_tokens: u32,
) -> CommandResult<String> {
    let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "system",
        "content": system_prompt
    })];
    let mut total = 0usize;
    let mut kept: Vec<&ChatMsg> = Vec::new();
    for m in messages.iter().rev() {
        let n = m.content.chars().count();
        if total + n > SUMMARIZE_MAX_CHARS && !kept.is_empty() {
            break;
        }
        total += n;
        kept.push(m);
    }
    for m in kept.iter().rev() {
        msgs.push(serde_json::json!({"role": m.role, "content": m.content}));
    }
    let (url, body) = match provider {
        crate::bot::ApiProvider::Openai => (
            format!("{}/chat/completions", base_url.trim_end_matches('/')),
            serde_json::json!({
                "model": model,
                "messages": msgs,
                "stream": false
            }),
        ),
        crate::bot::ApiProvider::Anthropic => {
            let body = crate::bot_anthropic::build_anthropic_body(
                model,
                &msgs,
                &serde_json::json!([]),
                max_tokens,
                false,
            )
            .map_err(|e| format!("摘要消息转换失败：{e}"))?;
            (crate::bot_anthropic::anthropic_messages_url(base_url), body)
        }
    };
    let req = client.post(&url).json(&body);
    let req = match provider {
        crate::bot::ApiProvider::Openai => req.bearer_auth(api_key.trim()),
        crate::bot::ApiProvider::Anthropic => {
            crate::bot_anthropic::apply_anthropic_auth(req, api_key.trim())
        }
    };
    let resp = req
        .send()
        .await
        .map_err(|e| format!("请求大模型失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(CommandError::LlmApiError {
            status: status.as_u16(),
            body_preview: text.chars().take(300).collect::<String>(),
        });
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析响应失败：{e}"))?;
    let text = match provider {
        // 安全访问：choices 可能为空数组/缺失（网关错误对象），索引会 panic（审计 P0 已修复）
        crate::bot::ApiProvider::Openai => v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string(),
        crate::bot::ApiProvider::Anthropic => crate::bot_anthropic::parse_anthropic_response(&v),
    };
    // 2026-08-28 批次3审计 P2-6：模型带 <think> 段时先剥掉，防摘要带思考段写回历史
    let text = strip_think_blocks(&text).trim().to_string();
    if text.is_empty() {
        return Err(CommandError::DomainRule {
            domain: "llm".to_string(),
            reason: "模型返回了空摘要".to_string(),
        });
    }
    Ok(text)
}

/// 非流式摘要调用薄壳（截断即摘要 / Reflection / /compact 共用）：
/// 配置获取路径与 bot_compact 原路径一致（bot_get_config 拿 base_url/model，
/// read_api_key 拿 key，require_api_key 把空 key 映射为 ApiKeyMissing）。
pub(crate) async fn summarize_messages(
    app: &AppHandle,
    system_prompt: &str,
    messages: &[ChatMsg],
) -> CommandResult<String> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let api_key = crate::bot::read_api_key()?;
    require_api_key(&api_key)?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    summarize_http(
        &client,
        &cfg.base_url,
        &api_key,
        &cfg.model,
        system_prompt,
        messages,
        // 2026-09-05 Anthropic 兼容模式：协议与 max_tokens 从配置解析
        //（None/非法值 → Openai，老配置零影响）
        crate::bot::ApiProvider::from_cfg(cfg.api_provider.as_deref()),
        crate::bot::resolve_max_tokens(cfg.max_tokens),
    )
    .await
}

/// /compact 快捷命令：把当前会话历史交给模型总结成摘要（单次非流式请求，不带工具）
#[tauri::command]
pub async fn bot_compact(app: AppHandle, messages: Vec<ChatMsg>) -> CommandResult<String> {
    summarize_messages(&app, COMPACT_SYSTEM_PROMPT, &messages).await
}

// ───────────────────────── 任务卡执行 ─────────────────────────

/// 任务卡交给机器人执行（🤖 按钮 / 选卡说「完成它」）：把任务卡内容组装成指令，高轮数工具循环执行。
/// 流式经 bot-chat-delta / bot-think-delta / bot-tool* 事件推给挂件。
#[tauri::command]
pub async fn bot_execute_task(app: AppHandle, task_id: String, session_id: Option<String>) -> CommandResult<BotChatResult> {
    // 逐步执行模式（2026-08-19 老板拍板）：手动触发 + ≥2 个未勾子任务 → 一个一个做，
    // 每个子任务做完在聊天里等用户确认（继续=勾选+下一个 / 重做 / 停）；
    // 聊天批量执行与定时调度仍走整卡连续执行（多卡/无人在场不适合逐步确认）
    if bot_get_enabled(app.clone()) {
        if let Some(task) = crate::db::db_load(app.clone())
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|t| t.id == task_id && t.deleted_at.is_none())
        {
            let undone = task
                .subtasks
                .as_deref()
                .map(|s| s.iter().filter(|x| !x.done).count())
                .unwrap_or(0);
            if undone >= 2 {
                return crate::exec_steps::start(&app, &task, session_id.as_deref()).await;
            }
        }
    }
    execute_task_core(&app, &task_id, true, session_id).await
}

/// 任务卡执行防重入：同一 task_id 同时只允许一个执行实例。
/// 覆盖三条入口（🤖 连点 / chat 批量执行 / 定时调度），防同一卡并发跑多个 LLM 循环
/// （2026-08-18 事故：同一任务 id 被并发执行 ~10 次，日志交叠、结果互相覆盖）。
static EXEC_RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn exec_running() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    EXEC_RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 防重入 RAII 守卫：Drop（含 panic 展开）时自动释放，task_id 不残留
pub(crate) struct ExecGuard(String);

impl ExecGuard {
    pub(crate) fn acquire(task_id: &str) -> Option<Self> {
        let mut set = exec_running().lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(task_id) {
            return None;
        }
        set.insert(task_id.to_string());
        Some(Self(task_id.to_string()))
    }
}

impl Drop for ExecGuard {
    fn drop(&mut self) {
        exec_running()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// 任务卡执行核心（命令与定时调度共用）。interactive=true 表示用户直接触发（可被 /stop 停），
/// false 表示后台定时触发（/stop 不影响）
/// session_id（2026-08-26 会话隔离）：触发会话 id；后台定时任务为 None——
/// None 时 run_model_loop 不向挂件广播流式增量（防串进用户当前对话）。
pub async fn execute_task_core(
    app: &AppHandle,
    task_id: &str,
    interactive: bool,
    session_id: Option<String>,
) -> CommandResult<BotChatResult> {
    // 开关关闭时明确拒绝（二次审计 P2-3）
    if !bot_get_enabled(app.clone()) {
        return Err(CommandError::BotDisabled);
    }
    // 防重入：同一任务卡已有执行实例在跑 → 直接拒绝（RAII 守卫随函数返回/panic 自动释放）
    let Some(_exec_guard) = ExecGuard::acquire(task_id) else {
        // 拒绝也留痕：否则无法区分「用户在前次执行未结束时重复触发」与「守卫泄漏」
        crate::bot::audit_log(
            app,
            &format!("execute_task_rejected | id: {} | 已有执行实例在跑（防重入拦截）", crate::bot::truncate_for_log(task_id, 60)),
        );
        return Err(CommandError::TaskInvalidState {
            reason: "该任务卡正在执行中，请等待完成后再触发".into(),
        });
    };
    let stop = StopGuard::new_task_exec(interactive, session_id);
    let task = crate::db::db_load(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
        .ok_or("任务卡不存在或已在回收站")?;
    if task.column == "done" {
        return Err(CommandError::TaskInvalidState {
            reason: "这张卡已标记完成；如需重新执行，先在卡片上取消完成".into(),
        });
    }
    if task.archived == Some(true) {
        return Err(CommandError::TaskInvalidState {
            reason: "任务已归档，不能执行；请先恢复".into(),
        });
    }
    let block = build_task_block(&task);
    crate::bot::audit_log(
        &app,
        &format!(
            "execute_task | id: {} | title: {}",
            task.id,
            crate::bot::truncate_for_log(&task.title, 60)
        ),
    );
    let mut msgs = vec![
        serde_json::json!({"role": "system", "content": format!("{}\n\n{}", EXECUTE_SYSTEM_PROMPT, build_skill_block(app))}),
        serde_json::json!({"role": "user", "content": block}),
    ];
    // 记忆 v2（2026-09-09）：任务卡执行/定时调度也注入记忆块——助手执行任务时知道用户
    // 偏好；查询 = 任务标题+备注前 200 字；失败静默降级为无记忆块（injection_block 内部兜底）。
    let mem_query: String = format!(
        "{} {}",
        task.title,
        task.note.as_deref().unwrap_or("")
    )
    .chars()
    .take(200)
    .collect();
    if let Some(mem_block) = crate::memory::injection_block(app, &mem_query).await {
        msgs.insert(1, serde_json::json!({"role": "system", "content": mem_block}));
    }
    // 交给机器人：卡片切机器人头像（前端 tasks-changed 广播后实时更新）
    set_bot_assigned(app, &task.id, true).await;
    let result = crate::bot_model_loop::run_model_loop(
        app.clone(),
        msgs,
        crate::bot_model_loop::DEFAULT_MAX_ROUNDS,
        &stop,
        None,
    )
    .await;
    // 执行结束（无论成败）：清除标记，恢复用户头像
    set_bot_assigned(app, &task.id, false).await;
    let (text, refs) = match result {
        Ok(v) => v,
        Err(e) => {
            // 2026-09-09 lesson 特性：任务执行失败自动沉淀一条 lesson（source=system，
            // 语义去重合并同类失败）；写失败只记审计，不影响原错误返回
            crate::memory::auto_lesson_on_task_failure(app, &task.title, &e.message()).await;
            return Err(e);
        }
    };
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

/// 任务卡执行上下文块（[任务卡执行] + 标题/状态/备注/子任务/截止/绑定文件），
/// 整卡连续执行（execute_task_core）与逐步执行（exec_steps）共用
pub(crate) fn build_task_block(task: &crate::db::Task) -> String {
    let mut block = format!(
        "[任务卡执行]\nid={}\n标题：{}\n状态：{}",
        task.id,
        task.title,
        match task.column.as_str() {
            "doing" => "进行中",
            _ => "待办",
        }
    );
    if let Some(n) = task.note.as_deref().filter(|n| !n.trim().is_empty()) {
        block.push_str(&format!("\n备注：{n}"));
    }
    if let Some(subs) = task.subtasks.as_deref().filter(|s| !s.is_empty()) {
        block.push_str("\n子任务：");
        for s in subs {
            block.push_str(&format!(
                "\n- [{}] {}",
                if s.done { "x" } else { " " },
                s.text
            ));
        }
    }
    if let Some(d) = task.due.as_deref() {
        block.push_str(&format!("\n截止时间：{d}"));
    }
    let bound = task.effective_files();
    if !bound.is_empty() {
        block.push_str(&format!(
            "\n绑定文件：{}",
            bound
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    block
}

/// 翻转「交给机器人」标记：重读库后只改 bot_assigned，避免覆盖机器人工具对卡片的修改
pub(crate) async fn set_bot_assigned(app: &AppHandle, task_id: &str, assigned: bool) {
    let Ok(all) = crate::db::db_load(app.clone()).await else {
        return;
    };
    let Some(mut t) = all
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
    else {
        return;
    };
    if t.bot_assigned == Some(assigned) {
        return;
    }
    t.bot_assigned = Some(assigned);
    t.expected_updated_at = t.updated_at; // T1-1：RMW 基线 = 快照 updated_at
    t.updated_at = Some(chrono::Utc::now().timestamp_millis());
    if crate::db::db_upsert(app.clone(), vec![t.clone()]).await.is_ok() {
        crate::bot::broadcast_after_mutation(app, vec![t], vec![]);
    }
}

// ────────────────────────────────────────────────────────────────────
// 测试：图片附件纯函数 + 主编编排纯函数（Phase 7 Q3 2026-08-18 12:50）
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod image_attach_tests {
    use super::*;

    #[test]
    fn attach_image_from_block() {
        let dir = std::env::temp_dir().join(format!("wm_img_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]).unwrap(); // PNG 魔数
        let content = format!("[附件文件]\n- {}\n\n提取图片里的文字", png.display());
        // SEC-P1-7：白名单根目录传入（测试用临时目录充当白名单根）
        let (v, skipped) = attach_images_in(&[dir.clone()], &content);
        assert_eq!(skipped, 0);
        let arr = v.as_array().expect("应返回多模态数组");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["type"], "image_url");
        let url = arr[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "url: {url}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn attach_image_outside_whitelist_skipped() {
        // SEC-P1-7：白名单外的图片路径被跳过（防 [附件文件] 块污染外泄）
        let dir = std::env::temp_dir().join(format!("wm_img_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A]).unwrap();
        let content = format!("[附件文件]\n- {}\n\n看看", png.display());
        let other_root = std::env::temp_dir().join("wm_img_other_root");
        let (v, skipped) = attach_images_in(&[other_root], &content);
        assert_eq!(skipped, 1, "白名单外图片应被跳过");
        assert!(v.is_string(), "全部跳过时退回纯文本");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_image_returns_plain_text() {
        let (v, _) = attach_images_in(&[], "普通消息 [附件文件]\n- /tmp/x.docx\n\n润色一下");
        assert!(v.is_string(), "无图片时应返回纯文本字符串");
    }

    #[test]
    fn missing_image_file_skipped() {
        let (v, _) = attach_images_in(&[std::path::PathBuf::from("/tmp")], "[附件文件]\n- /tmp/not_exists_xyz.png\n\n看看");
        assert!(v.is_string(), "图片不存在时退回纯文本");
    }
}

#[cfg(test)]
mod bot_chat_pure_helpers_tests {
    use super::*;

    #[test]
    fn format_recovery_hint_includes_reason_summary_and_yes_no() {
        let hint =
            format_recovery_hint("list_tasks 超时", "- Step 1 (list_tasks): 0 个任务\n", true);
        assert!(
            hint.contains("【Skill 失败可恢复上下文】"),
            "应有上下文标记：\n{hint}"
        );
        assert!(hint.contains("原因：list_tasks 超时"), "应含原因：\n{hint}");
        assert!(
            hint.contains("- Step 1 (list_tasks): 0 个任务"),
            "应含已完成产物：\n{hint}"
        );
        assert!(
            hint.contains("回滚已尝试：是"),
            "rollback_attempted=true 应输出 是：\n{hint}"
        );
        assert!(
            hint.contains("重试 / 调整 / 告知用户"),
            "应含 LLM 决策提示：\n{hint}"
        );

        let hint_no = format_recovery_hint("x", "y", false);
        assert!(
            hint_no.contains("回滚已尝试：否"),
            "rollback_attempted=false 应输出 否：\n{hint_no}"
        );
    }

    #[test]
    fn format_recovery_hint_handles_empty_reason_and_summary() {
        let hint = format_recovery_hint("", "", false);
        assert!(hint.contains("原因："), "空 reason 也应含 key：\n{hint}");
        assert!(
            hint.contains("已完成产物："),
            "空 summary 也应含 key：\n{hint}"
        );
    }

    #[test]
    fn merge_task_refs_dedup_empty_input_returns_empty() {
        let out = merge_task_refs_dedup(vec![]);
        assert!(out.is_empty());
    }

    #[test]
    fn merge_task_refs_dedup_no_duplicates_returns_all_in_order() {
        let refs = vec![
            TaskRef {
                id: "a".into(),
                title: "标题 A".into(),
            },
            TaskRef {
                id: "b".into(),
                title: "标题 B".into(),
            },
            TaskRef {
                id: "c".into(),
                title: "标题 C".into(),
            },
        ];
        let out = merge_task_refs_dedup(refs);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id, "a");
        assert_eq!(out[1].id, "b");
        assert_eq!(out[2].id, "c");
    }

    #[test]
    fn merge_task_refs_dedup_duplicates_keeps_first_occurrence() {
        let refs = vec![
            TaskRef {
                id: "a".into(),
                title: "首次标题 A".into(),
            },
            TaskRef {
                id: "b".into(),
                title: "首次 B".into(),
            },
            TaskRef {
                id: "a".into(),
                title: "后续标题 A（应被丢弃）".into(),
            },
            TaskRef {
                id: "b".into(),
                title: "后续 B（应被丢弃）".into(),
            },
        ];
        let out = merge_task_refs_dedup(refs);
        assert_eq!(out.len(), 2, "去重后应剩 2 条");
        assert_eq!(
            out[0].title, "首次标题 A",
            "首次出现应保留原标题，不能用后续覆盖"
        );
        assert_eq!(out[1].title, "首次 B");
    }

    #[test]
    fn merge_task_refs_dedup_consecutive_same_ids_collapses() {
        let refs = vec![
            TaskRef {
                id: "x".into(),
                title: "X1".into(),
            },
            TaskRef {
                id: "x".into(),
                title: "X2".into(),
            },
            TaskRef {
                id: "x".into(),
                title: "X3".into(),
            },
        ];
        let out = merge_task_refs_dedup(refs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "X1");
    }

    // ── 2026-08-28 批次3审计：历史预算 / 图片窗口 / think 剥除 ──

    fn msg(role: &str, content: &str) -> ChatMsg {
        ChatMsg { role: role.into(), content: content.into() }
    }

    #[test]
    fn truncate_chat_history_within_budget_unchanged() {
        let msgs = vec![msg("user", "你好"), msg("assistant", "在的"), msg("user", "列任务")];
        let (kept, dropped) = truncate_chat_history(msgs, 100);
        assert_eq!(dropped, 0);
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn truncate_chat_history_drops_oldest_first() {
        let long = "x".repeat(60);
        let msgs = vec![msg("user", &long), msg("assistant", &long), msg("user", &long)];
        let (kept, dropped) = truncate_chat_history(msgs, 100);
        assert_eq!(dropped, 2, "超预算时最旧的先丢，dropped 数应正确");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].role, "user");
    }

    #[test]
    fn truncate_chat_history_keeps_last_even_over_budget() {
        let msgs = vec![msg("user", &"x".repeat(200))];
        let (kept, dropped) = truncate_chat_history(msgs, 10);
        assert_eq!(dropped, 0, "最后一条（本轮用户消息）永远保留");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn image_attach_indices_hits_user_in_last_three() {
        let msgs = vec![msg("user", "a"), msg("assistant", "b"), msg("user", "c"), msg("user", "d")];
        assert_eq!(image_attach_indices(&msgs), vec![2, 3]);
    }

    #[test]
    fn image_attach_indices_skips_older_user() {
        let msgs = vec![msg("user", "a"), msg("assistant", "b"), msg("assistant", "c"), msg("user", "d")];
        assert_eq!(image_attach_indices(&msgs), vec![3], "更早的 user 不命中");
    }

    #[test]
    fn image_attach_indices_no_user_is_empty() {
        let msgs = vec![msg("assistant", "a"), msg("assistant", "b")];
        assert!(image_attach_indices(&msgs).is_empty());
    }

    #[test]
    fn strip_think_blocks_complete_section_removed() {
        assert_eq!(strip_think_blocks("前<think>想很多</think>后"), "前后");
    }

    #[test]
    fn strip_think_blocks_multiple_sections() {
        assert_eq!(
            strip_think_blocks("a<think>x</think>b<think>y</think>c"),
            "abc"
        );
    }

    #[test]
    fn strip_think_blocks_unclosed_tail_dropped() {
        assert_eq!(strip_think_blocks("保留<think>没闭合的尾巴"), "保留");
    }

    #[test]
    fn strip_think_blocks_no_tag_unchanged() {
        assert_eq!(strip_think_blocks("普通文本"), "普通文本");
        assert_eq!(strip_think_blocks(""), "");
    }

    // ── 2026-09-04 记忆模块 Step 1：截断即摘要（编排内核，摘要器注入） ──

    #[tokio::test]
    async fn truncate_with_summary_core_success_returns_summary() {
        // budget=63：本轮消息（4 字）+ 一条 60 字旧消息=64 超预算 → 最旧两条被丢
        let long = "x".repeat(60);
        let msgs = vec![msg("user", &long), msg("assistant", &long), msg("user", "本轮问题")];
        let (kept, summary, dropped) = truncate_with_summary_core(msgs, 63, |dropped_msgs| async move {
            assert_eq!(dropped_msgs.len(), 2, "摘要器应收到的恰是将被丢弃的消息");
            Ok("用户在做记忆模块".to_string())
        })
        .await;
        assert_eq!(dropped, 2);
        assert_eq!(summary.as_deref(), Some("用户在做记忆模块"));
        assert_eq!(kept.len(), 1, "最旧两条被丢，只留本轮消息");
        assert_eq!(kept[0].content, "本轮问题");
    }

    #[tokio::test]
    async fn truncate_with_summary_core_llm_failure_falls_back_to_plain_drop() {
        let long = "x".repeat(60);
        let msgs = vec![msg("user", &long), msg("assistant", &long), msg("user", "本轮问题")];
        let (kept, summary, dropped) = truncate_with_summary_core(msgs, 63, |_| async {
            Err(CommandError::Internal("boom".into()))
        })
        .await;
        assert_eq!(dropped, 2, "失败也要按原行为丢弃最旧消息");
        assert_eq!(summary, None, "失败 → 无摘要（调用方静默退回）");
        assert_eq!(kept.len(), 1);
    }

    #[tokio::test]
    async fn truncate_with_summary_core_empty_summary_treated_as_failure() {
        let long = "x".repeat(60);
        let msgs = vec![msg("user", &long), msg("assistant", &long), msg("user", "本轮问题")];
        let (_, summary, dropped) = truncate_with_summary_core(msgs, 63, |_| async {
            Ok("   ".to_string())
        })
        .await;
        assert_eq!(summary, None, "空白摘要按失败处理");
        assert_eq!(dropped, 2);
    }

    #[tokio::test]
    async fn truncate_with_summary_core_within_budget_never_calls_llm() {
        let msgs = vec![msg("user", "你好"), msg("assistant", "在的")];
        let (kept, summary, dropped) = truncate_with_summary_core(msgs, 100, |_| async {
            panic!("预算内不应触发摘要调用")
        })
        .await;
        assert_eq!((kept.len(), summary, dropped), (2, None, 0));
    }

    // ── 2026-09-05 记忆模块 Step 2：记忆块拼装（format_memory_block 纯函数） ──

    fn mem_item(key: &str, value: &str, kind: &str, importance: i64, source: &str, updated_at: i64) -> crate::db::MemoryItem {
        crate::db::MemoryItem {
            key: key.into(),
            value: value.into(),
            kind: kind.into(),
            category: "preference".into(),
            importance,
            source: source.into(),
            access_count: 0,
            accessed_at: 0,
            updated_at,
        }
    }

    #[test]
    fn memory_block_sections_order_and_inferred_prefix() {
        let inj = crate::db::MemoryInjection {
            pinned: vec![mem_item("称呼", "老板", "fact", 5, "user_stated", 1_000)],
            hits: vec![
                mem_item("城市", "上海", "fact", 3, "user_stated", 1_000),
                // summary 命中走日期行格式
                mem_item("summary:s1:1", "讨论了记忆模块", "summary", 2, "model_inferred", 1_756_000_000_000),
            ],
            recent: vec![mem_item("summary:s1:2", "确定不用向量模型", "summary", 2, "model_inferred", 1_756_100_000_000)],
        };
        let block = format_memory_block(&inj).expect("有内容应有记忆块");
        assert!(block.starts_with("## 记忆"));
        let p_img = block.find("### 用户画像与偏好").unwrap();
        let p_rel = block.find("### 相关记忆").unwrap();
        let p_sum = block.find("### 近期摘要").unwrap();
        assert!(p_img < p_rel && p_rel < p_sum, "拼装顺序：画像 → 相关记忆 → 近期摘要");
        assert!(block.contains("- [preference]称呼：老板"), "fact 行带 category + key：{block}");
        // 设计 7.2：model_inferred 一律带 [推断] 前缀
        assert!(block.contains("[推断]讨论了记忆模块"), "推断记忆带前缀：{block}");
        assert!(block.contains("[推断]确定不用向量模型"), "近期摘要同样带前缀：{block}");
    }

    #[test]
    fn memory_block_empty_injection_is_none() {
        let inj = crate::db::MemoryInjection { pinned: vec![], hits: vec![], recent: vec![] };
        assert!(format_memory_block(&inj).is_none(), "三段全空 → 无记忆块");
    }

    #[test]
    fn memory_block_fallback_recent_summaries_when_no_hits() {
        // 回退兜底（设计第 4 节）：检索全 0 分（hits 空）时近期摘要段仍在
        let inj = crate::db::MemoryInjection {
            pinned: vec![mem_item("称呼", "老板", "fact", 5, "user_stated", 1_000)],
            hits: vec![],
            recent: vec![mem_item("summary:s1:9", "最近聊过发布计划", "summary", 2, "model_inferred", 1_756_100_000_000)],
        };
        let block = format_memory_block(&inj).unwrap();
        assert!(block.contains("### 用户画像与偏好"), "高重要度 fact 无条件在");
        assert!(!block.contains("### 相关记忆"), "无命中不出空段头");
        assert!(block.contains("### 近期摘要"), "回退兜底段在");
        assert!(block.contains("最近聊过发布计划"));
    }

    #[test]
    fn memory_block_budget_trims_from_back() {
        // 超预算从后往前砍：近期摘要先砍光，再砍相关记忆，画像段不砍
        let long = "长".repeat(2_000);
        let inj = crate::db::MemoryInjection {
            pinned: vec![mem_item("称呼", "老板", "fact", 5, "user_stated", 1_000)],
            hits: vec![
                mem_item("k1", &long, "fact", 3, "user_stated", 1_000),
                mem_item("k2", &long, "fact", 3, "user_stated", 1_000),
            ],
            recent: vec![
                mem_item("summary:s1:1", &long, "summary", 2, "model_inferred", 1_000),
                mem_item("summary:s1:2", &long, "summary", 2, "model_inferred", 2_000),
            ],
        };
        let block = format_memory_block(&inj).expect("画像段超预算也保留");
        assert!(block.chars().count() <= MEMORY_BUDGET_CHARS, "超预算应从后往前砍到预算内");
        assert!(block.contains("称呼：老板"), "画像段不砍");
        assert!(!block.contains("### 近期摘要"), "近期摘要段应先被砍光");
        assert!(!block.contains("k2"), "相关记忆从末行开始砍");
    }
}

// ────────────────────────────────────────────────────────────────────
// 测试：P0-6A — Err("...".into()) 逃生舱改走专用 CommandError 变体
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod command_error_mapping_tests {
    use super::*;

    /// bot_chat 的开关守卫：bot_get_enabled == false 时必须映射到 BotDisabled 专用变体
    /// （不是 From<&str> 兜底的 Internal）
    #[test]
    fn bot_disabled_maps_to_dedicated_variant() {
        let err = require_bot_enabled(false).expect_err("关闭时应返回 Err");
        assert_eq!(err, CommandError::BotDisabled, "应为 BotDisabled 专用变体");
        assert_eq!(err.code(), "BOT_DISABLED");
        assert!(err.is_recoverable(), "BotDisabled 应可恢复（引导去设置页开启）");
        assert!(err.message().contains("机器人聊天已关闭"));
        assert!(require_bot_enabled(true).is_ok(), "开启时应通过");
    }

    /// bot_compact 的 key 守卫：空 key → ApiKeyMissing 专用变体；非空 → Ok
    #[test]
    fn require_api_key_empty_returns_api_key_missing() {
        for empty in ["", "   ", "\n\t "] {
            let err = require_api_key(empty).expect_err("空 key 应返回 Err");
            assert_eq!(err, CommandError::ApiKeyMissing, "输入 {empty:?}");
            assert_eq!(err.code(), "API_KEY_MISSING");
            assert!(err.is_recoverable(), "ApiKeyMissing 应可恢复（去设置页填 key）");
        }
        assert!(require_api_key("sk-test-123").is_ok(), "非空 key 应通过");
    }
}