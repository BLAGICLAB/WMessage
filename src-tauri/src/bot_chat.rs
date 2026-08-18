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
use tauri::AppHandle;

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
16. 用户说「完成/执行」且消息带 [已选任务] 引用块时，进入执行模式：用工具尽力完成引用块里的任务卡；生成的文件用 link_file_to_task 绑回对应任务卡；完成后 edit_task 写执行摘要 + complete_task（taskId 用引用块里的 id）；任务卡是线下事务时说明原因、不要标完成；\
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

/// 聊天入口：messages 为完整历史（含最新的用户消息），返回最终完整回复。
/// 流式片段经 bot-chat-delta 事件实时推给挂件窗口。
#[tauri::command]
pub async fn bot_chat(app: AppHandle, messages: Vec<ChatMsg>) -> CommandResult<BotChatResult> {
    if !bot_get_enabled(app.clone()) {
        return Err("机器人聊天已关闭：请到设置页「机器人设置」开启".into());
    }
    let stop = StopGuard::new(true);
    // F-1 开关读取：true = 新行为（pre-step 路由生效），false = LEGACY 旧链路（强制 pre_routed_skill = None）
    let bypass_llm_on_pre_step_hit = crate::bot::read_bypass_llm_switch(&app);
    // 审计：记录本轮用户最新指令（截断防刷日志）
    if let Some(last) = messages.last() {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "user.message",
            "content" => crate::bot::truncate_for_log(&last.content, 300),
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

/// /compact 快捷命令：把当前会话历史交给模型总结成摘要（单次非流式请求，不带工具）
#[tauri::command]
pub async fn bot_compact(app: AppHandle, messages: Vec<ChatMsg>) -> CommandResult<String> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let api_key = crate::bot::read_api_key()?;
    if api_key.trim().is_empty() {
        return Err("机器人 API 未配置：请到设置页「机器人设置」填写 API Key".into());
    }
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
    let stop = StopGuard::new(interactive);
    let task = crate::db::db_load(app.clone())
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
        .ok_or("任务卡不存在或已在回收站")?;
    if task.column == "done" {
        return Err("这张卡已标记完成；如需重新执行，先在卡片上取消完成".into());
    }
    if task.archived == Some(true) {
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
    set_bot_assigned(app, &task.id, true);
    let result = crate::bot_model_loop::run_model_loop(app.clone(), msgs, 10, &stop).await;
    // 执行结束（无论成败）：清除标记，恢复用户头像
    set_bot_assigned(app, &task.id, false);
    let (text, refs) = result?;
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

/// 翻转「交给机器人」标记：重读库后只改 bot_assigned，避免覆盖机器人工具对卡片的修改
fn set_bot_assigned(app: &AppHandle, task_id: &str, assigned: bool) {
    let Ok(all) = crate::db::db_load(app.clone()) else {
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
    if crate::db::db_upsert(app.clone(), vec![t.clone()]).is_ok() {
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