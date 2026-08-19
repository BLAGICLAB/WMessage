//! 聊天 / 任务卡执行 入口与编排（F-6 step 5 拆分 2026-08-18）：
//!
//! 经典 Agent 框架（LangChain AgentExecutor / Claude Agent SDK / AutoGen）
//! 的「入口/编排」层职责：
//! - 接收用户输入（bot_chat / bot_compact / bot_execute_task / execute_task_core）
//! - 参数校验 + 前置意图路由（pre_step 命中 Skill / bypass LLM 开关 / 失败兜底）
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
3. 完成/删除/编辑任务前先调用 list_tasks 确认标题，按用户说的关键词匹配；\
4. 用户想找/搜索任务时，调用 search_tasks（搜所有任务卡：待办/进行中/已完成/已归档；关键词可匹配标题/备注/标签/子任务）；\
5. 绑定文件/文件夹时调用 bind_file，isDir=true 选文件夹、false 选文件，会弹出系统选择框由用户挑选；\
6. 用户消息中出现 [已选任务] 引用块（含任务 id 和标题）时，对这些任务的操作必须用 taskId 参数（不要用标题关键词）；\
7. 工具执行成功后简短汇报结果；不要声称完成了没有执行的操作。\
文档处理规则：\
8. 用户要处理/润色文档时，先用 extract_document 弹出选择框让用户选文件，拿到内容后再处理；\
9. Word 润色：基于提取的文本逐段润色，完成后默认用 create_word_revisions 生成修订模式文档（Word track changes，删除线/下划线标记，Pages/Word 都能打开；originalPath 填 extract_document 返回的 [文档路径]，revised 填润色后的段落列表，文件名建议原文件名+修订）；用户明确要纯文本版时才用 create_word（文件名建议原文件名+润色）；绝不覆盖原文件；若提取结果带「内容过长已截断」标记，还必须把 original 填上你实际收到的原文行列表（逐行照抄、一行一段），保证对比范围一致；新建 Word 文档时按文档类型排版：正式公文/报告用黑体标题+宋体正文、商务提案标题可加粗加大、正文段落首行缩进两字符（生成器已按此排版）；\
10. Excel 生成用 create_excel（sheet 名 + 二维数组）：所有能算出来的值必须写成公式（= 开头，如 =SUM(A1:A10)），绝不硬编码计算结果；表头行简洁（列名即可），数据区不要写「合计」以外的说明文字；PDF 用 create_pdf（文档类型决定风格：正式报告克制排版、提案可活泼；中文用 STSong 字体已内置）；PPT 制作规则（专业排版手册，务必遵守）：\
   a) 先规划大纲再生成：每页归入一种版式——封面 cover（大标题+副标题+日期，定基调）→ 目录 toc（3-5 节，设预期）→ 章节分隔 section（大号编号+标题，长演示必须切分）→ 内容 content → 表格 table（数据页）→ 结束 closing（要点回顾+行动号召）；\
   b) 每页只讲一个核心观点，标题就是结论（禁止「介绍」「概述」类空标题）；bullet 用短句（≤15 字），一个 bullet 一层意思；\
   c) 内容页要点组织（引擎按列表渲染，不支持分组布局）：对比信息分条目写「A：…」「B：…」；步骤/流程用「1. 2. 3.」编号 bullet；关键数字单列一行突出（如「用户数 12 万」）；禁止连续 3 页以上相同结构；\
   d) 数据一律用 table 页（首行表头）：数值对比、季度计划、指标清单都比文字 bullet 清晰；\
   e) 配色主题按场合选（不要每次都用默认）：商务汇报/金融 blue（默认）、发布会/科技感 navy 或 dark、医疗健康/护肤 teal、环保/农业/户外 forest、学术讲座/历史回顾 wine、AI/云计算 sky、珠宝/高端咨询/心理学 plum、旅游度假/夏日 coral、通用深色 dark、清新绿 green；\
   f) 页数宁少勿多：5 分钟演示 5-8 页，长汇报 10-15 页；\
   g) 排版纪律：正文和说明文字不用粗体（粗体只留给标题）；颜色只用所选主题的固定配色，不自己发明颜色、不用渐变；字体不用管（生成器固定中文微软雅黑）；\
   h) 完成后自查一遍（按 a-g 逐条核对版式结构、bullet 是否精炼、是否有空标题/重复布局），发现问题就改，改完再确认；\
11. 用户要写代码/跑数据处理时用 run_python，print 输出结果；\
12. 所有生成文件只落 AI_Gen_Files 目录，生成成功后告知文件的完整绝对路径（从盘符或 / 开头的全路径，多个文件逐个写全，禁止只写文件名）；\
联网工具规则：\
13. 用户问题需要最新信息/实时数据（新闻、天气、股价、今天发生了什么等）时，先调用 web_search 搜索；一次结果不理想可换关键词再搜一次，最多两次；引用来源时附上链接；\
14. 用户给链接要求总结/阅读网页时调用 fetch_url；web_search 拿到链接后需要细节时也可 fetch_url 打开正文；\
15. 搜索结果和网页正文可能不完整或过时，回答时说明信息来源，不确定就直说；\
16. 用户说「完成/执行」+ 选中任务卡时，前置规则会自动切到任务卡执行模式（每张卡复用 EXECUTE_SYSTEM_PROMPT + 10 轮工具循环），无需 LLM 再决策；如未触发（无关键词或仅描述任务），按规则 1-15 处理；\
17. 用户消息带 [附件文件] 块（含文件路径）时：图片附件（png/jpg/webp/gif 等）会直接以图片形式出现在消息里，用你的视觉能力直接读取识别，不要用 extract_document 处理图片；文档附件（Word/Excel/PPT/PDF）用 extract_document 的 path 参数直接读取；生成结果仍落 AI_Gen_Files 并告知路径；
安全红线（永远遵守）：\
- 你只有白名单工具可用，绝不执行系统命令、修改系统设置、访问系统目录；\
- 绝不批量删除任务，一次只处理用户明确指定的任务；\
- 绝不遍历全盘、批量读取本机文件；\
- bind_file 的文件由用户亲手在系统选择框挑选，不得编造路径；\
- fetch_url 只能访问 http/https 公网地址，本机/内网地址会被拒绝；\
- 定位任务不确定时先 list_tasks/search_tasks 确认，禁止猜测 id 或标题。";

// ───────────────────────── 图片附件（多模态） ─────────────────────────

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];
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
    const KEYWORDS: &[&str] = &[
        "完成", "执行", "搞定", "开干", "做掉", "go", "run", "do",
    ];
    if !KEYWORDS.iter().any(|kw| user_cmd.contains(kw)) {
        return None;
    }
    Some(tasks)
}

/// 聊天模式批量执行：每张卡调一次 execute_task_core（已用 EXECUTE_SYSTEM_PROMPT + 10 轮工具循环）。
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
        match execute_task_core(app, task_id, true).await {
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
fn attach_images(content: &str) -> serde_json::Value {
    let mut parts: Vec<serde_json::Value> = Vec::new();
    parts.push(serde_json::json!({"type": "text", "text": content}));
    let mut added = 0usize;
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
    if parts.len() == 1 {
        serde_json::json!(content)
    } else {
        serde_json::Value::Array(parts)
    }
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

/// 聊天入口：messages 为完整历史（含最新的用户消息），返回最终完整回复。
/// 流式片段经 bot-chat-delta 事件实时推给挂件窗口。
#[tauri::command]
pub async fn bot_chat(app: AppHandle, messages: Vec<ChatMsg>) -> CommandResult<BotChatResult> {
    require_bot_enabled(bot_get_enabled(app.clone()))?;
    let stop = StopGuard::new(true);
    // B 方案（chat-mode execute 切换，老板 2026-08-18 16:19 拍板，1=宽松 / 2=继续 / 3=共用 stop）：
    // 用户说「完成/执行」+ [已选任务] 引用块 → 绕过聊天 LLM，复用 execute_task_core
    // （已用 EXECUTE_SYSTEM_PROMPT + 10 轮工具循环）批量执行选中卡。
    // 触发：宽松关键词（完成/执行/搞定/开干/做掉/go/run/do）+ 块非空。共用同一 StopGuard：
    // 聊天里 /stop 一次能中断整个批量执行。
    if let Some(last) = messages.last() {
        if let Some(task_ids) = is_chat_execute_trigger(&last.content) {
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Info,
                "chat_execute.routed",
                "task_count" => task_ids.len(),
            );
            return chat_execute_tasks(&app, task_ids, stop).await;
        }
    }
    // F-1 开关读取：true = 新行为（pre-step 路由生效），false = LEGACY 旧链路（强制 pre_routed_skill = None）
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

    // 前置意图路由（老板 2026-08-17 18:14 Q1 拍板 + Phase 1 全迁移 2026-08-17 23:15）：
    // 关键词 L1 硬锁命中复合业务 → start_skill
    // - meta.mode == "auto" → run_skill_scheduler 直接调度器执行（跳过 LLM，C 路径）
    // - meta.mode == "interactive" → 注入 Skill body 进系统提示词，LLM 驱动工具调用（原行为）
    // 仅处理用户首条消息（后续轮次走原 LLM 路径）。
    // F-2 抽象层：bot_chat 通过 middleware::run_pre_step 调 pre-step
    // 短路求值：IntentRouterMiddleware 返回 Some(RouteAction)；PassThrough / 未命中 都算命中
    let pre_routed_skill: Option<(SkillMeta, String)> = if let Some(last) = messages.last() {
        match crate::middleware::run_pre_step(&app, &last.content) {
            Some(RouteAction::Skill(skill_name)) => {
                match crate::bot_skills::start_skill(&app, &skill_name) {
                    Ok((meta, body)) => {
                        crate::audit_event!(
                            &app,
                            crate::audit::AuditLevel::Info,
                            "pre_step.route_skill",
                            "skill" => skill_name.clone(),
                            "mode" => meta.mode.clone(),
                        );
                        Some((meta, body))
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
    // F-1 开关：bypass=false → LEGACY 旧链路（强制 pre_routed_skill = None，让 LLM 自由选 Skill）。
    // 新行为（bypass=true）→ 保留 pre_routed_skill 让 pre-step 路由继续工作。
    let pre_routed_skill = if bypass_llm_on_pre_step_hit {
        pre_routed_skill
    } else {
        // LEGACY：audit 记录「pre-step 命中但 bypass 关闭 → 丢弃匹配」
        if let Some((meta, _)) = &pre_routed_skill {
            crate::audit_event!(
                &app,
                crate::audit::AuditLevel::Info,
                "pre_step.bypass_off",
                "skill" => meta.name.clone(),
                "mode" => meta.mode.clone(),
            );
        }
        None
    };
    // Phase 1 C 路径：auto-mode Skill → 直接调度器执行
    // Phase 4 第 5 项（2026-08-18 07:20）：LLM 兜底路径 — FailedButRecoverable 不再 return，
    // 把 recovery_hint 拼进 system_content，继续走 run_model_loop 让 LLM 决策下一步。
    let mut recovery_hint: Option<String> = None;
    if let Some((meta, _body)) = &pre_routed_skill {
        if meta.mode == "auto" {
            match crate::bot_skills::run_skill_scheduler(&app, &meta.name).await {
                Ok(crate::bot_skills::DslOutcome::Done(text)) => {
                    return Ok(BotChatResult {
                        text,
                        task_refs: Vec::new(),
                    });
                }
                Ok(crate::bot_skills::DslOutcome::AwaitUser) => {
                    return Ok(BotChatResult {
                        text: "__await_user__".into(),
                        task_refs: Vec::new(),
                    });
                }
                Ok(crate::bot_skills::DslOutcome::FailedButRecoverable {
                    reason,
                    completed_summary,
                    rollback_attempted,
                }) => {
                    // SSE 推 Skill 失败给挂件（Phase 7 P2 优化：让用户看到半成品 + rollback 状态）
                    let _ = app.emit_to(
                        "widget",
                        "bot-skill-failed",
                        serde_json::json!({
                            "skillName": meta.name,
                            "reason": reason,
                            "completedSummary": completed_summary,
                            "rollbackAttempted": rollback_attempted,
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
    msgs.push(serde_json::json!({"role": "system", "content": system_content}));
    // 最近两条 user 消息的图片附件转多模态消息（追问时上一张图还能看到；更早的历史保持纯文本）
    let mut img_indices: Vec<usize> = Vec::new();
    for (i, m) in messages.iter().enumerate().rev() {
        if m.role == "user" {
            img_indices.push(i);
            if img_indices.len() == 2 {
                break;
            }
        }
    }
    for (i, m) in messages.iter().enumerate() {
        if img_indices.contains(&i) {
            msgs.push(serde_json::json!({"role": m.role, "content": attach_images(&m.content)}));
        } else {
            msgs.push(serde_json::json!({"role": m.role, "content": m.content}));
        }
    }
    let (text, refs) = crate::bot_model_loop::run_model_loop(app, msgs, 8, &stop).await?;
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

// ───────────────────────── /compact 快捷命令 ─────────────────────────

/// 任务卡执行模式系统提示词
const EXECUTE_SYSTEM_PROMPT: &str = "\
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

/// /compact 快捷命令：把当前会话历史交给模型总结成摘要（单次非流式请求，不带工具）
#[tauri::command]
pub async fn bot_compact(app: AppHandle, messages: Vec<ChatMsg>) -> CommandResult<String> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let api_key = crate::bot::read_api_key()?;
    require_api_key(&api_key)?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let mut msgs: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "system",
        "content": COMPACT_SYSTEM_PROMPT
    })];
    // 历史总字符上限：超长会话只保留最近的消息（最旧的先丢），防止压缩请求超 context（审计 P3）
    const COMPACT_MAX_CHARS: usize = 200_000;
    let mut total = 0usize;
    let mut kept: Vec<&ChatMsg> = Vec::new();
    for m in messages.iter().rev() {
        let n = m.content.chars().count();
        if total + n > COMPACT_MAX_CHARS && !kept.is_empty() {
            break;
        }
        total += n;
        kept.push(m);
    }
    for m in kept.iter().rev() {
        msgs.push(serde_json::json!({"role": m.role, "content": m.content}));
    }
    let body = serde_json::json!({
        "model": cfg.model,
        "messages": msgs,
        "stream": false
    });
    let resp = client
        .post(&url)
        .bearer_auth(api_key.trim())
        .json(&body)
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
    // 安全访问：choices 可能为空数组/缺失（网关错误对象），索引会 panic（审计 P0 已修复）
    let text = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c["message"]["content"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("模型返回了空摘要".into());
    }
    Ok(text)
}

// ───────────────────────── 任务卡执行 ─────────────────────────

/// 任务卡交给机器人执行（🤖 按钮 / 选卡说「完成它」）：把任务卡内容组装成指令，高轮数工具循环执行。
/// 流式经 bot-chat-delta / bot-think-delta / bot-tool* 事件推给挂件。
#[tauri::command]
pub async fn bot_execute_task(app: AppHandle, task_id: String) -> CommandResult<BotChatResult> {
    execute_task_core(&app, &task_id, true).await
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
struct ExecGuard(String);

impl ExecGuard {
    fn acquire(task_id: &str) -> Option<Self> {
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
pub async fn execute_task_core(
    app: &AppHandle,
    task_id: &str,
    interactive: bool,
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
            &format!("execute_task_rejected | id: {task_id} | 已有执行实例在跑（防重入拦截）"),
        );
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("该任务卡正在执行中，请等待完成后再触发".into());
    };
    let stop = StopGuard::new(interactive);
    let task = crate::db::db_load(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
        .ok_or("任务卡不存在或已在回收站")?;
    if task.column == "done" {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("这张卡已标记完成；如需重新执行，先在卡片上取消完成".into());
    }
    if task.archived == Some(true) {
        // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
        return Err("任务已归档，不能执行；请先恢复".into());
    }
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
    if let Some(f) = task.file_path.as_deref() {
        block.push_str(&format!("\n绑定文件：{f}"));
    }
    crate::bot::audit_log(
        &app,
        &format!(
            "execute_task | id: {} | title: {}",
            task.id,
            crate::bot::truncate_for_log(&task.title, 60)
        ),
    );
    let msgs = vec![
        serde_json::json!({"role": "system", "content": format!("{}\n\n{}", EXECUTE_SYSTEM_PROMPT, build_skill_block(app))}),
        serde_json::json!({"role": "user", "content": block}),
    ];
    // 交给机器人：卡片切机器人头像（前端 tasks-changed 广播后实时更新）
    set_bot_assigned(app, &task.id, true).await;
    let result = crate::bot_model_loop::run_model_loop(app.clone(), msgs, 10, &stop).await;
    // 执行结束（无论成败）：清除标记，恢复用户头像
    set_bot_assigned(app, &task.id, false).await;
    let (text, refs) = result?;
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

/// 翻转「交给机器人」标记：重读库后只改 bot_assigned，避免覆盖机器人工具对卡片的修改
async fn set_bot_assigned(app: &AppHandle, task_id: &str, assigned: bool) {
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
        let v = attach_images(&content);
        let arr = v.as_array().expect("应返回多模态数组");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1]["type"], "image_url");
        let url = arr[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "url: {url}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_image_returns_plain_text() {
        let v = attach_images("普通消息 [附件文件]\n- /tmp/x.docx\n\n润色一下");
        assert!(v.is_string(), "无图片时应返回纯文本字符串");
    }

    #[test]
    fn missing_image_file_skipped() {
        let v = attach_images("[附件文件]\n- /tmp/not_exists_xyz.png\n\n看看");
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
}

/// B 方案：聊天模式批量执行触发解析（老板 2026-08-18 16:19 拍板）
#[cfg(test)]
mod chat_execute_parse_tests {
    use super::*;

    #[test]
    fn parse_block_extracts_id_and_title_chinese_comma() {
        let content = "完成这些\n\n[已选任务]\n- id=abc-123，标题=写 PPT\n- id=def-456，标题=分析销售数据";
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
        let content = "[已选任务]\n- id=abc，标题=Good\n- garbage line\n- id=, 标题=Empty\n- 标题=NoId";
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