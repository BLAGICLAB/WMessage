//! 聊天 / 任务卡执行 入口与编排：
//!
//! 经典 Agent 框架（LangChain AgentExecutor / Claude Agent SDK / AutoGen）
//! 的「入口/编排」层职责：
//! - 接收用户输入（bot_chat / bot_compact / bot_execute_task / run_task_in_chat）
//! - 参数校验 + 五步主流程编排（严格按序，禁止任何步骤抢跑 / 前置 return）：
//!   1. exec_steps::resume（如有挂起子任务）
//!   2. bypass_llm_on_pre_step_hit 开关读取
//!   3. pre-step 路由（intent_router：选择任务卡批量执行 / Skill / PassThrough）
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

use crate::bot_skills::{build_skill_block_for, SkillMeta};
use crate::bot_slash::{bot_get_enabled, StopGuard};
use crate::error::{CommandError, CommandResult};
use crate::intent_router::RouteAction;
use crate::prompt_builder::{PromptSlot, SystemPromptBuilder};
use crate::prompts::{
    COMPACT_SYSTEM_PROMPT, EXECUTE_SYSTEM_PROMPT, SUMMARY_SYSTEM_PROMPT, SYSTEM_PROMPT,
};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

// 聊天

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatMsg {
    pub role: String, // "user" | "assistant"
    pub content: String,
}

// 图片附件（多模态）

/// 产物落盘规则（动态注入 AI_Gen_Files / 系统 temp 的绝对路径）。
/// WM_GEN_DIR / WM_TMP_DIR 由 run_python 注入子进程环境（bot_py::run_python_at），
/// 这里把同一约束写进系统提示词：产物必进 AI_Gen_Files，临时文件必进系统 temp。
pub(crate) fn gen_dir_rule<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let gen = crate::db::gen_dir(app)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "WM_GEN_DIR 指向的目录".to_string());
    let tmp = std::env::temp_dir().to_string_lossy().to_string();
    format!(
        "产物落盘规则（硬性约束）：所有要交付给用户的文件（文档/表格/图片/代码产物等）必须保存到 {gen}（即 AI_Gen_Files 目录；run_python 子进程内等同 WM_GEN_DIR 环境变量指向的目录）；临时中间文件必须放系统临时目录 {tmp}（子进程内等同 WM_TMP_DIR）；禁止写到桌面/下载/当前目录等其他任何位置。交付给用户的文件名用与内容相符的可读中文名（如「销售数据分析.docx」），不用 report_v2、output、拼音或无意义代号。"
    )
}

/// 图片扩展名清单（pub：前端镜像硬编码在 src/components/ChatPanel/UserBubbleContent.tsx IMAGE_EXTS，
/// 改动需两侧同步）
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
    // HashSet 判重（O(1) 查找）：批量大卡累积 refs 时 Vec::contains 是 O(n²)；
    // insert 返回「是否新插入」，首次保留语义不变
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(refs.len());
    refs.into_iter()
        .filter(|r| seen.insert(r.id.clone()))
        .collect()
}

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

/// L0 `pre_ingest` 的规范化字符表（设计 §二：L0 是唯一的输入侧改写点）。
///
/// **只含「不可见且无语义」的字符**：每加一个字符都要能回答「删掉它会改变
/// 任何语义吗」——回答不了就不进这张表。
///
/// **刻意不含 ZWJ(U+200D) 与 ZWNJ(U+200C)**：它们看着是零宽，实际有语义——
/// ZWJ 拼合 emoji 序列（👨‍👩‍👧 是**一个**字符家族的合成图，删掉就拆成三个人），
/// ZWNJ 在波斯语/印地语里表示字母不成形。把它们当噪音清掉是在制造新 bug。
const INVISIBLE_JUNK: &[char] = &['\u{FEFF}', '\u{2060}', '\u{00AD}'];

/// L0 输入规范化：把用户这一轮的原文归一成干净文本。
///
/// 只做四件无争议的事（返回清洗后的串）：
/// 1. **删不可见垃圾字符**（BOM / word-joiner / soft-hyphen）：它们从网页、
///    Word、PDF 复制时会混进来，肉眼完全看不见，但会让关键词匹配整个失效。
/// 2. **零宽空格换成普通空格**（U+200B → U+0020）：它在中文排版里就是个空格，
///    删掉会把「做\u{200B}PPT」粘成「做PPT」，反而更匹配不上。
/// 3. **CRLF → LF**：Windows 来源的粘贴带 `\r`，会污染行尾比对。
/// 4. **去首尾空白**：粘贴尾巴上的换行很常见。
///
/// **刻意不做**：空行折叠、代码块内容改写、大小写归一、繁简转换。前两者会
/// 改掉用户的排版与代码缩进，后两者会改掉原话——收益抵不过「用户看到自己
/// 说的话被程序动了」的代价。要做折叠，阈值得高到能区分「排版意图」与
/// 「复制事故」，那已经是另一个判断题（见设计文档 §十）。
///
/// 返回值里带 `changed` 与统计，调用方据此决定要不要记审计——没变化就不该
/// 在日志里多一条噪音。
pub(crate) struct NormalizedInput {
    pub text: String,
    /// 原文是否被改过（false = 原样透传，零分配走 `Cow`）
    pub changed: bool,
    /// 被清掉的不可见字符数（审计用）
    pub stripped: usize,
}

/// 纯函数核（不碰 DB / app），便于把每条规则分别钉住。
fn normalize_user_input_core(raw: &str) -> NormalizedInput {
    let mut text = String::with_capacity(raw.len());
    let mut stripped = 0usize;
    let mut changed = false;

    // 先统一行尾，再逐字符过滤：顺序反了会把 `\r\n` 里的 `\r` 当垃圾删掉，
    // 结果对、但审计里会记成「清掉了 1 个不可见字符」，误导排障。
    let unified: std::borrow::Cow<'_, str> = if raw.contains('\r') {
        changed = true;
        std::borrow::Cow::Owned(raw.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        // 不含 \r 时借用原串，避免为绝大多数输入做一次全量分配
        std::borrow::Cow::Borrowed(raw)
    };

    for c in unified.chars() {
        if INVISIBLE_JUNK.contains(&c) {
            stripped += 1;
            changed = true;
            continue;
        }
        if c == '\u{200B}' {
            // 零宽空格是「有语义的空间」→ 归成真空格，而不是当垃圾删掉
            text.push(' ');
            changed = true;
            continue;
        }
        text.push(c);
    }

    let trimmed = text.trim();
    if trimmed.len() != text.len() {
        changed = true;
    }
    NormalizedInput {
        text: trimmed.to_string(),
        changed,
        stripped,
    }
}

/// L0 入口：规范化本轮用户输入并就地改写（只动最后一条）。
///
/// 就地改 `messages` 而不是局部变量：这一轮之后，历史落库、记忆检索查询、
/// 技能推荐全都读同一个 `messages`——只清洗局部变量会让「本轮看到的」和
/// 「下一轮历史里的」不一致，于是下一轮又按原文重新判一遍，规范化等于没做。
pub(crate) fn normalize_user_input(messages: &mut [ChatMsg]) -> Option<NormalizedInput> {
    let last = messages.last_mut()?;
    if last.role != "user" {
        return None;
    }
    let out = normalize_user_input_core(&last.content);
    if out.changed {
        last.content = out.text.clone();
    }
    if out.changed {
        Some(out)
    } else {
        None
    }
}

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

/// 截断即摘要（设计 5.2）生产薄壳：
/// 超预算时先生成摘要并落库，返回 (保留的消息, 带
/// 「[早前对话摘要]」前缀的摘要内容, 丢弃条数)；LLM/落库任何一步失败都退回
/// 直接丢弃，绝不阻塞或弄挂主对话流程。
pub(crate) async fn truncate_chat_history_with_summary(
    app: &AppHandle,
    session_id: Option<&str>,
    messages: Vec<ChatMsg>,
    budget: usize,
) -> (Vec<ChatMsg>, Option<String>, usize) {
    let (kept, summary, dropped) =
        truncate_with_summary_core(messages, budget, |dropped_msgs| async move {
            summarize_messages(app, SUMMARY_SYSTEM_PROMPT, &dropped_msgs).await
        })
        .await;
    if let Some(summary) = &summary {
        persist_summary(app, session_id, summary).await;
    }
    (
        kept,
        summary.map(|s| format!("[早前对话摘要] {s}")),
        dropped,
    )
}

/// 摘要落库（设计 5.2）：全失败兜底——出错只记审计不重试不影响对话
/// （摘要已在本轮历史里，落库丢了下轮截断还会再摘要）。
async fn persist_summary(app: &AppHandle, session_id: Option<&str>, summary: &str) {
    if let Err(e) = crate::memory::save_summary(app, session_id, summary).await {
        crate::audit_event!(app, crate::audit::AuditLevel::Warn, "memory.summary_save_failed",
            "error" => e.message());
    }
}

/// 记忆块注入薄壳：语义记忆体（crate::memory：mem_items +
/// 语义嵌入混合检索）；任何失败一律 None 静默降级为「无记忆块」，绝不弄挂主对话
///（设计约束）；失败记 WARN 审计便于排查。
async fn build_memory_block(app: &AppHandle, query: &str) -> Option<String> {
    crate::memory::injection_block(app, query).await
}

/// 需要内联图片的消息下标：原先「最近两条 user 消息」
/// 永不失效，一张图每轮对话都重复 base64 重发。改为最后 3 条消息内的 user 消息——
/// 覆盖「发图 → 追问一轮」场景，更早的历史保持纯文本。
fn image_attach_indices(messages: &[ChatMsg]) -> Vec<usize> {
    let from = messages.len().saturating_sub(3);
    (from..messages.len())
        .filter(|&i| messages[i].role == "user")
        .collect()
}

/// 剥掉 <think>...</think> 段：非流式路径（bot_compact /
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

/// 批量执行的失败策略。显式化一卡失败后是「继续下一张」还是「中止」。
///
/// 默认 `ContinueOnError`（现状不变）；`StopOnFirstError` 可调用方选启用。
#[derive(Default)]
pub enum BatchPolicy {
    /// 一卡失败继续下一张（现有默认行为）。汇总报告里仍会列失败清单。
    #[default]
    ContinueOnError,
    /// 一卡失败即中止后续，报告「已执行 i 张，剩余 N-i-1 张未执行」。
    StopOnFirstError,
}

/// 聊天模式批量执行：每张卡调一次 run_task_in_chat（每卡独立新会话，
/// EXECUTE_SYSTEM_PROMPT + 50 轮工具循环），单卡失败不污染其他卡的执行记录。
/// 顺序执行（避免文件写冲突）；一卡失败继续（任一卡失败不阻断后续）。
/// /stop 语义：外层 stop 只在**卡片之间**判定——每卡内部用自己新建的
/// StopGuard（新会话 id，/stop 按会话匹配不到它），故 /stop 只阻止下一张卡
/// 启动，不中断正在执行的卡。
/// 汇总报告：每张卡的开头 + 执行结果 + 总数 + 失败清单；task_refs 跨卡去重（merge_task_refs_dedup）。
///
/// 接受 `policy` 参数控制失败是否继续。默认 `BatchPolicy::default()`（ContinueOnError）。
pub async fn chat_execute_tasks(
    app: &AppHandle,
    task_ids: Vec<(String, String)>,
    stop: StopGuard,
    policy: BatchPolicy,
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
            all_text.push_str(&format!("\n⏹ 已停止（剩余 {} 张未执行）", total - idx));
            break;
        }
        let label = if title.is_empty() {
            task_id.as_str()
        } else {
            title.as_str()
        };
        all_text.push_str(&format!("\n── [{}/{}] {} ──\n", idx + 1, total, label));
        match run_task_in_chat(app, task_id, TaskExecOrigin::Batch, None).await {
            Ok(r) => {
                if !r.result.text.is_empty() {
                    all_text.push_str(&r.result.text);
                    all_text.push('\n');
                }
                ok += 1;
                all_refs.extend(r.result.task_refs);
            }
            Err(e) => {
                let err_str = e.to_string();
                all_text.push_str(&format!("❌ 失败：{}\n", err_str));
                failed += 1;
                errors.push(format!("{} ({})", label, err_str));
                // StopOnFirstError —— 第一卡失败即中止后续，附“已完成 i 张”报告
                if matches!(policy, BatchPolicy::StopOnFirstError) {
                    let done = idx + 1; // 当前卡已计入 idx（0-based），+1 = 已处理数
                    let remaining = total.saturating_sub(done);
                    all_text.push_str(&format!(
                        "\n⛔ 中止：第 {} 张卡失败，后续 {} 张未执行。",
                        done, remaining
                    ));
                    break;
                }
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
/// 图片路径过白名单（桌面/下载/文档/图片 + AI_Gen_Files）——
/// 原先任意路径的图片都被读取并外发给 LLM API，[附件文件] 块若被污染（历史注入）即成外泄通道。
/// 校验失败的附件跳过并记审计（不打断聊天）。
fn attach_images(app: &AppHandle, content: &str) -> serde_json::Value {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = crate::bot_fs::home_dir() {
        for d in ["Desktop", "Downloads", "Documents", "Pictures"] {
            roots.push(home.join(d));
        }
    }
    if let Ok(gen) = crate::db::gen_dir(app) {
        roots.push(gen);
    }
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
fn attach_images_in(roots: &[std::path::PathBuf], content: &str) -> (serde_json::Value, usize) {
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
        // 白名单判定：canonical 双向比较（软链解析后落点必须在白名单根内）。
        // 校验与读取必须同一对象——读 canonical 落点而非原路径 p，
        // 消 check-then-read 之间 symlink 掉包窗（掉包则任意文件进 LLM prompt）
        let canon = std::fs::canonicalize(p).ok().filter(|c| {
            roots.iter().any(|r| {
                std::fs::canonicalize(r)
                    .map(|rc| c.starts_with(&rc))
                    .unwrap_or(false)
            })
        });
        let Some(canon) = canon else {
            skipped += 1;
            continue;
        };
        let Ok(bytes) = std::fs::read(&canon) else {
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

/// 单个图片文件 → OpenAI image_url content part（data URL）。
/// 工具图片回传（screenshot）与附件链路共用同上限/同 mime 表；
/// 不做白名单校验（调用方是工具自身产物，非用户不可信输入）。
/// 文件缺失/超限/非图片扩展名返回 None（调用方跳过，不阻断）。
pub(crate) fn image_part_from_file(p: &std::path::Path) -> Option<serde_json::Value> {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return None;
    }
    let bytes = std::fs::read(p).ok()?;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    };
    let b64 = B64.encode(&bytes);
    Some(serde_json::json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{mime};base64,{b64}")}
    }))
}

/// 工具执行后带出的任务引用（前端渲染成可点击按钮，跳主窗口打开该任务）
#[derive(Serialize, Clone, Debug)]
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

/// pre-step 路由命中结果（主流程步骤 3 的产物）
// Skill 变体整体携带 SkillMeta+正文，装箱会牵连构造点，无栈压力场景下保留 #[allow]
#[allow(clippy::large_enum_variant)]
enum PreStepRoute {
    /// Skill 路由：start_skill 已加载（meta + body）
    Skill(SkillMeta, String),
    /// 选择任务卡批量执行：(task_id, title) 列表
    ExecuteTasks(Vec<(String, String)>),
}

/// 聊天入口：messages 为完整历史（含最新的用户消息），返回最终完整回复。
/// 流式片段经 bot-chat-delta 事件实时推给前端聊天区。
///
/// 主流程（五步严格按序、禁止抢跑/前置 return）：
/// 1. exec_steps::resume（有挂起子任务时本条消息是执行流程的应答，优先于一切聊天路由）
/// 2. bypass_llm_on_pre_step_hit 开关读取
/// 3. pre-step 路由（intent_router：ExecuteTasks 批量执行 / Skill / PassThrough）
/// 4. start_skill（Skill 调度：auto → 调度器执行；interactive → body 注入 system prompt）
/// 5. run_model_loop（LLM 决策 + 工具循环）
/// 聊天防重入守卫：同一会话同时只允许一个 bot_chat 在执行——
/// 原先聊天路径没有任何锁，两条并发消息命中同一技能路由会 start_skill 互相覆盖、
/// 副作用工具（create_task 等）重复执行（任务卡路径有 ExecGuard，这里补会话级对称防护）。
// CHAT_RUNNING 已迁入 `AppState`，访问器返回 Arc 克隆——
// `ChatGuard::drop` 里拿不到 `app`，靠 acquire 时克隆的这份句柄清理。
use crate::app_state::chat_running;

/// 会话级防重入守卫：Drop（含 panic 展开）时释放本会话槽位。
pub(crate) struct ChatGuard {
    /// None = 调用没带会话 id（不加锁）；清理只在 Some 时发生
    session_id: Option<String>,
    /// 表句柄：与 acquire 时取的注入实例是同一个 `Mutex`
    running: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl ChatGuard {
    /// Ok = 允许进入（无会话 id 不加锁）；Err = 本会话已有执行实例在跑
    pub(crate) fn acquire<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        session_id: Option<&str>,
    ) -> Result<Self, ()> {
        let running = chat_running(app);
        let Some(sid) = session_id else {
            return Ok(Self {
                session_id: None,
                running,
            });
        };
        {
            let mut set = crate::db::lock::lock_or_recover(&running, "app_state::chat_running");
            if !set.insert(sid.to_string()) {
                return Err(());
            }
        }
        Ok(Self {
            session_id: Some(sid.to_string()),
            running,
        })
    }
}

impl Drop for ChatGuard {
    fn drop(&mut self) {
        if let Some(sid) = &self.session_id {
            crate::db::lock::lock_or_recover(&self.running, "bot_chat::ChatGuard::running")
                .remove(sid);
        }
    }
}

/// 会话锁是否被持有：tests/task_chat_exec.rs 断言
/// run_task_in_chat 执行期持有 ChatGuard——bot_execute_task 纳入会话锁的回归证据。
/// 生产代码不调用。表随 `AppState` 走，故取注入实例（缺失时兜底实例，
/// 与生产路径用的是同一个——集成测试的 mock handle 没注入时两边都落到兜底）。
#[cfg(debug_assertions)]
pub fn chat_guard_is_held<R: tauri::Runtime>(app: &AppHandle<R>, session_id: &str) -> bool {
    crate::db::lock::lock_or_recover(&chat_running(app), "bot_chat::chat_running")
        .contains(session_id)
}

///  辅助：start_skill + 审计。成功 log `pre_step.route_skill`；失败 log `pre_step.route_failed`。
/// 返回 `Option<(SkillMeta, String)>`：None 表示路由失败 → 放行 LLM（不阻断聊天）。
fn start_skill_with_audit(
    app: &tauri::AppHandle,
    skill_name: String,
    session_id: Option<&str>,
) -> Option<(SkillMeta, String)> {
    match crate::bot_skills::start_skill(app, &skill_name, serde_json::Value::Null, session_id) {
        Ok((meta, body)) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Info,
                "pre_step.route_skill",
                "skill" => skill_name,
                "mode" => meta.mode.clone(),
            );
            Some((meta, body))
        }
        Err(e) => {
            crate::audit_event!(
                app,
                crate::audit::AuditLevel::Warn,
                "pre_step.route_failed",
                "skill" => skill_name,
                "error" => e,
            );
            None
        }
    }
}

///  抽出的「步骤 4」函数：处理 pre_step 命中 Skill 路由后的全部副作用。
///
/// 返回值契约：
/// - `Done(BotChatResult)`：auto-mode 调度器返回终态（Done / AwaitUser），调用方应直接 return 该结果
/// - `FallThrough { recovery_hint }`：需要落入「步骤 5」继续走 run_model_loop：
///   - interactive 模式 → recovery_hint 为 None（body 由 caller 拼入 system prompt）
///   - auto-mode 调度器返回 FailedButRecoverable → recovery_hint 为 Some(...)
/// - 调度器返回 Terminated → 走 Err(CommandError::Internal)，不进 Outcome
///
/// 调用方负责：
/// - 在调用本函数前完成 pre_step 路由 + bypass switch（pre_routed_skill 已通过 bypass）
/// - 在收到 FallThrough 时把 (meta, body) 装回 pre_routed_skill 并把 recovery_hint 写到主流程变量
async fn apply_skill_route(
    app: &tauri::AppHandle,
    meta: SkillMeta,
    _body: String,
    stop: &StopGuard,
    _session_id: Option<&str>,
) -> CommandResult<SkillRouteOutcome> {
    if meta.mode != "auto" {
        // interactive（以及未来其他非 auto 模式）：body 由 caller 拼入 system prompt
        return Ok(SkillRouteOutcome::FallThrough {
            recovery_hint: None,
        });
    }
    // auto-mode：调用 Skill 调度器，四种 DslOutcome 映射
    match crate::bot_skills::run_skill_scheduler(app, &meta.name, stop.session_id(), Some(stop))
        .await
    {
        Ok(crate::bot_skills::DslOutcome::Done(text)) => {
            Ok(SkillRouteOutcome::Done(BotChatResult {
                text,
                task_refs: Vec::new(),
            }))
        }
        Ok(crate::bot_skills::DslOutcome::AwaitUser) => {
            // 不把内部哨兵 "__await_user__" 当回复文本直出给前端（前端无该哨兵的
            // 处理逻辑，用户会看到原始字符串），改出可读提示
            crate::bot::audit_log(
                app,
                &format!(
                    "skill_await_user | name: {} | 已暂停等待用户确认",
                    crate::bot::truncate_for_log(&meta.name, 60)
                ),
            );
            Ok(SkillRouteOutcome::Done(BotChatResult {
                text: format!(
                    "⏸ 技能「{}」已暂停，正在等待你的确认——请在确认弹窗里选择后继续。",
                    meta.name
                ),
                task_refs: Vec::new(),
            }))
        }
        Ok(crate::bot_skills::DslOutcome::FailedButRecoverable {
            reason,
            completed_summary,
            rollback_attempted,
        }) => {
            // SSE 推 Skill 失败给前端（让用户看到半成品 + rollback 状态）
            // payload 带 sessionId，前端按会话过滤，防串会话弹失败卡
            let _ = app.emit_to(
                "main",
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
            Ok(SkillRouteOutcome::FallThrough {
                recovery_hint: Some(format_recovery_hint(
                    &reason,
                    &completed_summary,
                    rollback_attempted,
                )),
            })
        }
        Err(crate::bot_skills::DslFailure::Terminated { reason }) => {
            Err(CommandError::Internal(reason))
        }
    }
}

/// `apply_skill_route` 的返回值。Done 是「路由终态」（chat 直接返回），
/// FallThrough 是「落入步骤 5」（继续走 run_model_loop）。
enum SkillRouteOutcome {
    /// 直接返回给前端（Done / AwaitUser 两种终态；Terminated 走 Err 不进 Outcome）
    Done(BotChatResult),
    /// 落入步骤 5，带可选的 recovery_hint（auto-mode FailedButRecoverable 才有内容）
    FallThrough { recovery_hint: Option<String> },
}

#[tauri::command]
#[allow(clippy::too_many_lines)]
pub async fn bot_chat(
    app: AppHandle,
    messages: Vec<ChatMsg>,
    session_id: Option<String>,
    // 推理强度单次覆盖（RE-1）：聊天输入框按会话选的抽象档位（off/low/medium/high），
    // 由前端解析「会话覆盖 ?? 后台默认」后传入具体值；None = 按后台默认。
    // 只影响本次聊天调用，不回写 bot-config.json。
    reasoning_effort: Option<String>,
) -> CommandResult<BotChatResult> {
    require_bot_enabled(bot_get_enabled(app.clone()))?;
    // 会话级防重入：同会话并发消息直接拒绝，防技能路由/start_skill 竞争
    let _chat_guard = match ChatGuard::acquire(&app, session_id.as_deref()) {
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
    // L0 pre_ingest（设计 §二）：输入侧唯一改写点，在**所有**输入判定之前。
    // 位置是刻意的——挂起恢复、技能路由、纠正检测、批量执行触发四处都扫
    // 同一条原文，只要有一处读到未清洗的原文，规范化就只做了一半
    // （「做\u{FEFF}PPT」照旧匹配不上技能）。
    let mut messages = messages;
    if let Some(n) = normalize_user_input(&mut messages) {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "user.input_normalized",
            "stripped" => n.stripped.to_string(),
        );
    }
    // 步骤 1：逐步执行挂起恢复：有子任务待确认时，本条消息是对执行流程的应答
    // （继续/重做/停），优先于一切聊天路由。/stop 走独立命令（bot_stop 内清挂起）。
    if let Some(last) = messages.last() {
        if crate::exec_steps::has_pending_for(&app, session_id.as_deref()) {
            return crate::exec_steps::resume(&app, &last.content, session_id.as_deref()).await;
        }
    }
    let stop = StopGuard::new(&app, true, session_id.clone());
    // 步骤 2：bypass_llm_on_pre_step_hit 开关读取：
    // true = 新行为（pre-step 路由生效），false = LEGACY 旧链路（路由命中一律丢弃，LLM 自由决策）。
    // 必须在任何路由判定之前读取——主流程禁止任何步骤抢跑。
    let bypass_llm_on_pre_step_hit = crate::bot::read_bypass_llm_switch(&app);
    // 审计：记录本轮用户最新指令（截断防刷日志）
    if let Some(last) = messages.last() {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "user.message",
            // write_event 已统一转义 kv 值，这里只做长度截断，避免二次转义
            "content" => last.content.chars().take(300).collect::<String>(),
        );
        // 用户纠正信号检测（Round 4 学习闭环）：push-back 模式命中 →
        // (a) 写 user.correction 事件 + 持久化为记忆 item（fire-and-forget）；
        // (b) 即时触发一次聚类判定：簇 ≥3 → record_lesson（同样 fire-and-forget）。
        //     二者均不 await，主流程零阻塞。
        if let Some(pat) = crate::bot::corrections::detect_correction(&last.content) {
            crate::bot::corrections::record_correction(
                &app,
                session_id.as_deref(),
                &last.content,
                pat,
            );
            let app3 = app.clone();
            tauri::async_runtime::spawn(async move {
                // 即时路径：刚写一条 correction，立刻尝试聚类不能漏历史同源 → since=0 走全表
                crate::bot::corrections::try_cluster_correction_lessons(&app3, 0).await;
            });
        }
    }
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    // 技能清单动态注入：系统提示词 + 已安装技能的「名称+描述」（progressive disclosure 第一层；
    // 全文由 use_skill 工具按需读取，省 token）
    //  改造：用 SystemPromptBuilder 按 PromptSlot 声明顺序排序拼接，替代脆弱的
    // 顺序 format!("{}{}", a, b) 链。Base/GenDir/SkillCatalog 是 system_base 的 3 段，
    // 段间用 \n\n 分隔；后续 SkillBody/Recovery/Plan 拼在尾部，无分隔符。
    let mut prompt = SystemPromptBuilder::new();
    prompt.push(PromptSlot::Base, SYSTEM_PROMPT);
    // 主 agent 派发子 agent 的职责段——同 Base 槽位按 push
    // 顺序追加在 SYSTEM_PROMPT 之后（同槽多次 push 按顺序拼接，无新 slot）。
    prompt.push(
        PromptSlot::Base,
        format!("\n\n{}", crate::prompts::MAIN_AGENT_ADDENDUM.trim_start()),
    );
    // 动态设置项放末尾的 VerifierRetryTail 槽位：保持 Base/GenDir/SkillCatalog
    // 这些「静态前缀」字字节节稳定，让 Anthropic prompt caching 断点 1（system
    // 末尾）能跨会话命中——若把 verifier_retry_max 写进 Base 段，每会话变动
    // 会让 GenDir/SkillCatalog/工具 schema 缓存全部作废（miss → 1.25× 写价）。
    let verifier_retry_max_cfg = crate::bot::bot_get_config(app.clone())
        .map(|c| c.verifier_retry_max.unwrap_or(1).clamp(0, 3))
        .unwrap_or(1);
    prompt.push(
        PromptSlot::VerifierRetryTail,
        format!(
            "\n\n## 当前设置\nverifier 拒绝重试预算上限 = {verifier_retry_max_cfg}\
             （0 = 不重试；循环同一任务 verifier → executor 修复 → 再 verifier 最多此数）"
        ),
    );
    prompt.push(PromptSlot::GenDir, format!("\n\n{}", gen_dir_rule(&app)));
    prompt.push(
        PromptSlot::SkillCatalog,
        format!(
            "\n\n{}",
            // 按用户末条消息做语义化技能推荐排序（embed 不可用降级全清单）
            crate::bot_skills::build_skill_block_for(
                &app,
                messages.last().map(|m| m.content.as_str()),
            )
            .await
        ),
    );

    // 步骤 3：pre-step 路由（固定两步直调：先选任务卡批量执行，再意图路由）：
    // - is_chat_execute_trigger 命中（「完成/执行」+ [已选任务] 引用块）→ 批量执行选中任务卡
    // - route_user_input 关键词 L1 硬锁命中复合业务 → start_skill
    // - PassThrough → 放行进 LLM
    // 仅处理用户最新一条消息（后续轮次走原 LLM 路径）。
    // 路由命中的处理全部发生在本步骤之后，任何步骤不得抢跑、不得前置 return。
    let pre_routed_skill: Option<PreStepRoute> = if let Some(last) = messages.last() {
        if let Some(task_ids) = crate::intent_router::is_chat_execute_trigger(&last.content) {
            Some(PreStepRoute::ExecuteTasks(task_ids))
        } else {
            match crate::intent_router::route_user_input(&last.content) {
                RouteAction::Skill(skill_name) => {
                    start_skill_with_audit(&app, skill_name, stop.session_id())
                        .map(|(meta, body)| PreStepRoute::Skill(meta, body))
                }
                RouteAction::ExecuteTasks(_) | RouteAction::PassThrough => None,
            }
        }
    } else {
        None
    };
    //  开关：bypass=false → LEGACY 旧链路（强制 pre_routed_skill = None，让 LLM 自由决策）。
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
    // B 方案（1=宽松 / 2=继续 / 3=共用 stop）：每张卡复用
    // run_task_in_chat（EXECUTE_SYSTEM_PROMPT + 50 轮工具循环）；外层 stop 在
    // 卡片之间判定：聊天里 /stop 一次能阻止后续所有卡启动（正在执行的卡有
    // 自己新会话的 StopGuard，不受影响，跑到本卡结束）。定时任务模式
    // （bot_scheduler，interactive=false）同样只走 run_task_in_chat，不经本聊天流程，互不干扰。
    if let Some(task_ids) = batch_execute_tasks {
        crate::audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "chat_execute.routed",
            "task_count" => task_ids.len(),
        );
        return chat_execute_tasks(&app, task_ids, stop, BatchPolicy::default()).await;
    }
    // auto-mode Skill → 调用 apply_skill_route（步骤 4 抽出）；interactive 模式走 FallThrough。
    // LLM 兜底路径：FailedButRecoverable 不再 return，把 recovery_hint 拼进 system_content，
    // 继续走 run_model_loop 让 LLM 决策下一步。
    let mut recovery_hint: Option<String> = None;
    if let Some((meta, body)) = pre_routed_skill.as_ref() {
        match apply_skill_route(
            &app,
            meta.clone(),
            body.clone(),
            &stop,
            session_id.as_deref(),
        )
        .await?
        {
            SkillRouteOutcome::Done(result) => return Ok(result),
            SkillRouteOutcome::FallThrough {
                recovery_hint: hint,
            } => {
                recovery_hint = hint;
            }
        }
    }
    // 轮数三层优先级：Skill frontmatter 自报 > 配置 maxRounds > 默认 50
    let cfg_for_rounds = crate::bot::load_config(&app);
    let max_rounds = crate::bot_model_loop::resolve_max_rounds(
        pre_routed_skill
            .as_ref()
            .and_then(|(meta, _)| meta.max_rounds),
        crate::bot::params::resolve_max_rounds_cfg(&cfg_for_rounds) as usize,
    );
    let pre_routed_active_skill = pre_routed_skill.map(|(_, body)| body);
    // interactive 模式的技能正文拼入 system prompt（无分隔符接 SkillCatalog）
    if let Some(active_skill) = pre_routed_active_skill {
        prompt.push(PromptSlot::SkillBody, active_skill);
    }
    // auto-mode 失败兜底：recovery_hint 拼入让 LLM 决策下一步
    if let Some(hint) = recovery_hint {
        prompt.push(PromptSlot::Recovery, hint);
    }
    // PREVR 第 2 层：复杂多步任务先生成动态计划再执行。
    // 触发保守：needs_plan 启发式命中才多花一次 Planner 调用；
    // Planner 失败/输出非法 → None → 原自由循环，不阻断聊天。
    // 仅聊天主路径启用：任务卡执行（run_task_in_chat）/ 逐步执行（exec_steps）
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
    if let Some(plan) = &plan_state {
        prompt.push(
            PromptSlot::Plan,
            crate::bot_plan::format_plan_block(&plan.steps),
        );
    }
    msgs.push(serde_json::json!({"role": "system", "content": prompt.build()}));
    // 历史字符预算——长会话最旧的先丢，
    // 最后一条（本轮用户消息）永远保留；丢弃时留审计
    // 截断即摘要（设计 5.2）：超预算先对将丢弃的消息
    // 生成摘要；摘要单独以 system 消息放在截断后历史开头（不进 messages——下方
    // role 白名单会把非 assistant 降级为 user，防注入语义不动）；
    // LLM 失败静默退回直接丢弃
    // 历史预算走配置解析（config historyBudgetChars，钳 20K..=500K；默认 100K）
    let budget_used =
        crate::bot::params::resolve_history_budget_chars(&crate::bot::load_config(&app)) as usize;
    let (messages, summary, dropped) =
        truncate_chat_history_with_summary(&app, session_id.as_deref(), messages, budget_used)
            .await;
    if let Some(summary) = summary {
        msgs.push(serde_json::json!({"role": "system", "content": summary}));
    }
    // 记忆块独立 system 消息（设计第 6 节），紧跟主
    // system prompt 与摘要消息之后。直接进 msgs 不经 ChatMsg——下方 role 白名单
    // 会把非 assistant 降级为 user；检索查询 = 本轮用户消息原文 ≤200 字；
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
            "dropped" => dropped, "budget" => budget_used);
    }
    // 最后 3 条消息内 user 消息的图片附件转多模态消息（更早的历史降级为路径文本）
    let img_indices = image_attach_indices(&messages);
    for (i, m) in messages.iter().enumerate() {
        // role 白名单：历史里的非法 role 一律按 user，
        // 防污染历史注入 system/tool 角色
        let role = if m.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        if img_indices.contains(&i) {
            // attach_images 全程同步 FS（canonicalize 白名单判定 + 整图读入转 base64，
            // 单图至多数 MB）——包 spawn_blocking，防多图/并发会话把 async runtime
            // 的 worker 钉住（同 create_exec_session 先例）；阻塞任务崩溃时降级为纯文本
            let app_img = app.clone();
            let content_img = m.content.clone();
            let img_content =
                tauri::async_runtime::spawn_blocking(move || attach_images(&app_img, &content_img))
                    .await
                    .unwrap_or_else(|_| serde_json::json!(m.content));
            msgs.push(serde_json::json!({"role": role, "content": img_content}));
        } else {
            msgs.push(serde_json::json!({"role": role, "content": m.content}));
        }
    }
    // L2 落点 · 子任务回报（设计 §3.7「下一轮 assemble 检查」）：父 agent 派出去的
    // 子任务，终态的在这里打包成一段注入，在途的只报计数。放最后是刻意的——
    // 模型对本轮末尾的内容权重最高，而回报正是「此刻新增」的信息。
    // 只注 result_json，子会话历史留在它自己的会话行里（§3.7 要点 3）。
    if let Some(digest) =
        crate::bot_orchestrator::subagent_digest_block(&app, session_id.as_deref()).await
    {
        msgs.push(serde_json::json!({"role": "system", "content": digest}));
    }
    // Failure 分支可达——模型循环 Err（LLM 5xx/流断/头超时等）也落 trace
    //（此前只在 Ok 收尾处 hook，失败轨迹全丢，trace 的 Failure 采样规则不可达）。
    // reason 用粗分类，不带原始错误消息（含路径/参数，不入 trace/audit 明细）
    // 收尾自动记忆抽取要用的句柄（run_model_loop 按值消费 app，提前克隆）
    let extract_app = app.clone();
    // 词元统计（P4+）：主聊天也落 exec_traces——此前只有任务执行链落 trace，
    // 设置页「词元统计」对纯聊天用户恒为空（卡片副标题与事实不符的根因）。
    // begin 降级 None 不阻断；end 无论成败成对调用（loop_result 先落捕获再分流）。
    // origin="chat" 与 manual/scheduled/batch 同列展示。
    let trace_app = app.clone();
    let trace_capture = std::sync::Arc::new(std::sync::Mutex::new(
        crate::trace_sink::TraceCapture::default(),
    ));
    if let Some(sid) = session_id.as_ref() {
        let trace_id = crate::trace_sink::begin_trace(&trace_app, sid, None, "chat", None).await;
        if let Ok(mut c) = trace_capture.lock() {
            c.trace_id = trace_id;
            c.session_id = Some(sid.clone());
        }
    }
    let loop_result = crate::bot_model_loop::run_model_loop(
        app,
        msgs,
        max_rounds,
        &stop,
        plan_state.as_mut(),
        reasoning_effort,
        // 主聊天不挂每卡模型覆盖（ 仅工作流节点使用）
        None,
        // per-onion 温度覆盖只走子 agent（主会话尚未绑定配置档，见设计文档 §十）
        None,
    )
    .await;
    // LoopTrace 统计（tokens/轮数/模型名）塞进捕获，end_trace 汇总落库
    if let Ok((_, _, lt)) = &loop_result {
        if let Ok(mut c) = trace_capture.lock() {
            c.stats = Some(lt.clone());
        }
    }
    crate::trace_sink::end_trace(
        &trace_app,
        &trace_capture,
        loop_result.is_ok(),
        // 用户停止时 run_model_loop 返回 Ok（「⏹ 已停止」），须显式传停止标志
        stop.stopped(),
        loop_result.as_ref().err().map(|e| e.to_string()),
    )
    .await;
    let (text, refs, _loop_trace) = match loop_result {
        Ok(v) => v,
        Err(e) => {
            return Err(e);
        }
    };
    //  自动记忆抽取（fire-and-forget）：交互式会话收尾触发，抽取失败静默
    // 降级不冒泡；非交互（任务执行/定时）、/stop 中止的会话、非 off 档的判定
    // 在入口内完成。session_id 此后不再使用，按值转移。
    if !stop.stopped() {
        crate::memory::extract::maybe_extract_from_session(
            extract_app,
            session_id,
            stop.is_interactive(),
        );
    }
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

// /compact 快捷命令

/// 空 API Key → 专用错误 ApiKeyMissing（recoverable=true，引导用户去设置页）。
/// 抽成纯函数便于单测（keyring 在测试环境不可用，无法覆盖 bot_compact 全链路）。
fn require_api_key(api_key: &str) -> CommandResult<()> {
    if api_key.trim().is_empty() {
        return Err(CommandError::ApiKeyMissing);
    }
    Ok(())
}

/// 摘要请求的历史字符上限：超长会话只保留最近的消息（最旧的先丢），
/// 防止压缩请求超 context。截断路径的待摘要消息已被 DEFAULT_HISTORY_BUDGET_CHARS 限住，
/// 此上限对 /compact 的全量历史才实际生效。
const SUMMARIZE_MAX_CHARS: usize = 200_000;

/// 非流式摘要调用内核（从 bot_compact 提炼，连接参数注入——
/// 测试直连 mock LLM，生产薄壳 summarize_messages 从 bot_get_config/read_api_key 取配置）。
/// pub：tests/llm_integration.rs 直用（与 bot::run_model_loop_core 同先例）。
/// Anthropic 兼容模式：provider/max_tokens 注入，按协议分支
/// URL/鉴权头/请求体/响应解析（Anthropic 侧转换走 bot_anthropic 纯函数）。
/// temperature/top_p（ 条目级覆盖）有值才写请求体，两协议顶层字段同名。
pub async fn summarize_http(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system_prompt: &str,
    messages: &[ChatMsg],
    provider: crate::bot::ApiProvider,
    max_tokens: u32,
    temperature: Option<f64>,
    top_p: Option<f64>,
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
    let (url, mut body) = match provider {
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
    // 条目级采样参数注入：temperature/top_p 有值才写（两协议顶层字段同名）
    crate::bot_model_loop::apply_inference_params(&mut body, temperature, top_p);
    let req = client
        .post(&url)
        // per-request 总超时（共享客户端无默认超时）：摘要单次请求预算不变
        .timeout(std::time::Duration::from_secs(60))
        .json(&body);
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
        // 安全访问：choices 可能为空数组/缺失（网关错误对象），索引会 panic
        crate::bot::ApiProvider::Openai => v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c["message"]["content"].as_str())
            .unwrap_or("")
            .to_string(),
        crate::bot::ApiProvider::Anthropic => crate::bot_anthropic::parse_anthropic_response(&v),
    };
    // 模型带 <think> 段时先剥掉，防摘要带思考段写回历史
    let text = strip_think_blocks(&text).trim().to_string();
    if text.is_empty() {
        return Err(CommandError::DomainRule {
            domain: "llm".to_string(),
            reason: "模型返回了空摘要".to_string(),
        });
    }
    Ok(text)
}

/// 非流式摘要调用（截断即摘要 / /compact / 记忆整理 / 会话抽取共用）：
/// 配置获取路径与 bot_compact 原路径一致（bot_get_config 拿 base_url/model，
/// read_llm_key 拿 key，require_api_key 把空 key 映射为 ApiKeyMissing）。
/// 摘要：跟随全局当前模型。
///
/// 不接受「用另一个模型跑这次摘要」的参数——没有调用方需要。摘要场景全走
/// 当前模型：截断摘要（SUMMARY）、/compact（COMPACT）、记忆整理
/// （consolidate）、会话抽取（extract）、标签归并（task_autotag）。
/// 真要让某个任务走轻量模型，正确位置是模型条目本身（设置页配一个便宜条目
/// 切过去），不是给每次调用加一个没人传的开关。
pub(crate) async fn summarize_messages(
    app: &AppHandle,
    system_prompt: &str,
    messages: &[ChatMsg],
) -> CommandResult<String> {
    let cfg = crate::bot::bot_get_config(app.clone())?;
    let provider = crate::bot::ApiProvider::from_cfg(cfg.api_provider.as_deref());
    let active_id = cfg.active_model_id.clone();
    let base_url = cfg.base_url.clone();
    let model = cfg.model.clone();
    let provider_str = provider.as_str();
    let api_key = crate::bot::read_llm_key(
        Some(provider_str),
        active_id.as_ref(),
        cfg.models_by_provider.as_ref(),
    )?;
    require_api_key(&api_key)?;
    // 共享客户端（连接池复用）；原 60s 总超时改为 per-request 保留
    let client = crate::bot_model_loop::shared_llm_client().clone();
    // 条目级推理参数：max_tokens 条目值覆盖全局（再钳制）；temperature/top_p
    // 条目有值才发。摘要不追加条目 system_prompt（这里的 system_prompt 是固定任务提示词）
    let inference = crate::bot::effective_inference(
        Some(provider_str),
        cfg.max_tokens,
        active_id.as_ref(),
        cfg.models_by_provider.as_ref(),
    );
    summarize_http(
        &client,
        &base_url,
        &api_key,
        &model,
        system_prompt,
        messages,
        provider,
        inference.max_tokens,
        inference.temperature,
        inference.top_p,
    )
    .await
}

/// /compact 快捷命令：把当前会话历史交给模型总结成摘要（单次非流式请求，不带工具）
#[tauri::command]
pub async fn bot_compact(app: AppHandle, messages: Vec<ChatMsg>) -> CommandResult<String> {
    summarize_messages(&app, COMPACT_SYSTEM_PROMPT, &messages).await
}

// 任务卡执行

/// 任务卡交给机器人执行（🤖 按钮 / 选卡说「完成它」）。
/// 任务执行聊天化：执行永远在**新会话**里（run_task_in_chat 统一入口），
/// 前端经 chat-open-session 事件切过去围观；session_id 参数废弃（旧前端兼容保留，
/// 不再使用）。会话级 ChatGuard 由 run_task_in_chat 对新会话持有（补上原先后端无锁的漏洞）。
/// 流式经 bot-chat-delta / bot-think-delta / bot-tool* 事件（带新会话 sessionId）推给前端。
#[tauri::command]
pub async fn bot_execute_task(
    app: AppHandle,
    task_id: String,
    session_id: Option<String>,
) -> CommandResult<BotChatResult> {
    let _ = session_id; // 废弃：执行会话由后端新建
                        // 逐步执行模式：手动触发 + ≥2 个未勾子任务 → 一个一个做，
                        // 每个子任务做完在聊天里等用户确认（继续=勾选+下一个 / 重做 / 停）；
                        // 聊天批量执行与定时调度仍走整卡连续执行（多卡/无人在场不适合逐步确认）。
                        // 逐步执行也在新会话内（exec_steps 挂起态按新会话 id 停放）。
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
                let sid = create_exec_session(&app, &task, TaskExecOrigin::Manual).await?;
                let r = crate::exec_steps::start(&app, &task, Some(&sid)).await;
                persist_exec_reply(&app, &task, &sid, &r).await;
                return r;
            }
        }
    }
    run_task_in_chat(&app, &task_id, TaskExecOrigin::Manual, None)
        .await
        .map(|r| r.result)
}

/// 任务卡执行防重入：同一 task_id 同时只允许一个执行实例。
/// 覆盖三条入口（🤖 连点 / chat 批量执行 / 定时调度），防同一卡并发跑多个 LLM 循环
/// （并发执行会日志交叠、结果互相覆盖）。
// EXEC_RUNNING 已迁入 `AppState`，访问器返回 Arc 克隆——
// `ExecGuard::drop` 里拿不到 `app`，靠 acquire 时克隆的这份句柄清理。
use crate::app_state::exec_running;

/// 防重入 RAII 守卫：Drop（含 panic 展开）时自动释放，task_id 不残留
pub(crate) struct ExecGuard {
    task_id: String,
    /// 表句柄：与 acquire 时取的注入实例是同一个 `Mutex`
    running: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl ExecGuard {
    pub(crate) fn acquire<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        task_id: &str,
    ) -> Option<Self> {
        let running = exec_running(app);
        {
            let mut set = crate::db::lock::lock_or_recover(&running, "app_state::exec_running");
            if set.contains(task_id) {
                return None;
            }
            set.insert(task_id.to_string());
        }
        Some(Self {
            task_id: task_id.to_string(),
            running,
        })
    }
}

impl Drop for ExecGuard {
    fn drop(&mut self) {
        crate::db::lock::lock_or_recover(&self.running, "bot_chat::DB_WRITE_LOCK")
            .remove(&self.task_id);
    }
}

/// 任务执行聊天化（一次性执行 = 一个新会话，不再 headless 黑箱）：
///
/// 评审已确认的决策：
/// 1. 一次执行 = 一个新会话（执行质量优先，不带杂历史）
/// 2. busy 时执行不排队（新会话立即开跑），只有"自动跳转查看"排队
/// 3. 会话膨胀靠手动删除（不做自动清理；删除不影响已沉淀记忆）
/// 4. 不做"查看执行对话"入口和 last_session_id 关联——直接在聊天窗口按前缀找
/// 5. 批量执行：每张卡一个独立新会话（单卡失败不污染）
///
/// 统一入口 run_task_in_chat 供 🤖 按钮 / ⏰ 定时 / 📦 批量三路径
/// 共用（取代原 run_task_in_chat 的 headless 模式——定时任务不再是黑箱，全程流式可见、
/// 可按会话 /stop、永久落库可回看）。
///
/// 执行来源（会话标题前缀 + chat-open-session 事件 origin 字段）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskExecOrigin {
    /// 🤖 按钮 / 选卡说「完成它」
    Manual,
    /// ⏰ 定时调度触发
    Scheduled,
    /// 📦 聊天批量执行（每卡一个独立会话）
    Batch,
}

impl TaskExecOrigin {
    fn title_prefix(self) -> &'static str {
        match self {
            TaskExecOrigin::Manual => "📋 任务：",
            TaskExecOrigin::Scheduled => "⏰ 定时：",
            TaskExecOrigin::Batch => "📦 批量：",
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            TaskExecOrigin::Manual => "manual",
            TaskExecOrigin::Scheduled => "scheduled",
            TaskExecOrigin::Batch => "batch",
        }
    }
}

/// run_task_in_chat 的返回：新会话 id + 执行结果（前端靠 chat-open-session 事件，
/// 不依赖返回值；调度器用 result.text 写 ⏰ 兜底摘要）
pub struct TaskChatRun {
    pub session_id: String,
    pub result: BotChatResult,
}

/// 创建执行会话（标题 = 来源前缀 + 任务标题）并把任务块作为 user 消息落库，
/// 广播 chat-open-session 给前端（busy 时排队提示，见设计第 5 节）。
async fn create_exec_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    task: &crate::db::Task,
    origin: TaskExecOrigin,
) -> CommandResult<String> {
    let title = format!(
        "{}{}",
        origin.title_prefix(),
        crate::bot::truncate_for_log(task.title.trim(), 30)
    );
    let block = build_task_block(task);
    let session = crate::db::lock::with_write_conn(app, move |conn| {
        let s = crate::db::bot_session_create_inner(conn, Some(title))?;
        crate::db::bot_history_save_inner(
            conn,
            &s.id,
            &[crate::db::BotMsgRow {
                role: "user".into(),
                content: block,
                refs_json: None,
                thinking: None,
                tools_json: None,
            }],
        )?;
        Ok(s)
    })
    .await
    .map_err(CommandError::DbError)?;
    // per-onion 配置绑定（两层配置的 DB 层）：见 bot_onions 模块头
    crate::bot_onions::bind_session(app, &session.id);
    let _ = app.emit_to(
        "main",
        "chat-open-session",
        serde_json::json!({
            "sessionId": session.id,
            "taskId": task.id,
            "title": session.title,
            "origin": origin.as_str(),
        }),
    );
    Ok(session.id)
}

/// 执行会话的 assistant 回复落库（user 任务块 + 回复整体覆盖写，bot_history_save_inner
/// 语义即全量覆盖）。持久化失败只记审计——执行结果已经产生，记录缺失不阻断返回。
async fn persist_exec_reply<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    task: &crate::db::Task,
    sid: &str,
    reply: &CommandResult<BotChatResult>,
) {
    let (content, refs) = match reply {
        Ok(r) => (r.text.clone(), serde_json::to_string(&r.task_refs).ok()),
        Err(e) => (format!("⚠️ 执行失败：{}", e.message()), None),
    };
    let block = build_task_block(task);
    let sid = sid.to_string();
    let r = crate::db::lock::with_write_conn(app, move |conn| {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        crate::db::bot_history_save_inner(
            &tx,
            &sid,
            &[
                crate::db::BotMsgRow {
                    role: "user".into(),
                    content: block,
                    refs_json: None,
                    thinking: None,
                    tools_json: None,
                },
                crate::db::BotMsgRow {
                    role: "assistant".into(),
                    content,
                    refs_json: refs,
                    thinking: None,
                    tools_json: None,
                },
            ],
        )?;
        tx.commit().map_err(|e| e.to_string())
    })
    .await;
    match r {
        Ok(()) => {}
        Err(e) => {
            crate::audit_event!(app, crate::audit::AuditLevel::Warn, "task_exec.persist_failed",
                "error" => e);
        }
    }
}

/// 任务执行统一入口（生产薄壳：run_model_loop 真实循环；Wry 命令/调度路径用。
/// 测试请用 run_task_in_chat_with（泛型 Runtime + 注入模型循环））
pub async fn run_task_in_chat(
    app: &AppHandle,
    task_id: &str,
    origin: TaskExecOrigin,
    // 每卡模型覆盖：模型库条目 id；None = 跟随全局 active
    model: Option<String>,
) -> CommandResult<TaskChatRun> {
    run_task_in_chat_impl(app, task_id, origin, model).await
}

async fn run_task_in_chat_impl(
    app: &AppHandle,
    task_id: &str,
    origin: TaskExecOrigin,
    // 每卡模型覆盖：模型库条目 id；None = 跟随全局 active
    model: Option<String>,
) -> CommandResult<TaskChatRun> {
    // 执行 trace 挂钩——壳造槽，闭包填 LoopTrace 统计，
    // 内核（run_task_in_chat_with）填归属并收尾。
    // 轮数走配置解析（config maxRounds，钳 5..=200；默认 50）
    let cfg_for_rounds = crate::bot::load_config(app);
    let hook: std::sync::Arc<std::sync::Mutex<crate::trace_sink::TraceCapture>> =
        Default::default();
    let hook_for_run = hook.clone();
    run_task_in_chat_with(
        app,
        task_id,
        origin,
        Some(hook),
        crate::bot::params::resolve_ask_budget_cfg(&cfg_for_rounds),
        |app2, msgs, stop| async move {
            let outcome = crate::bot_model_loop::run_model_loop(
                app2,
                msgs,
                crate::bot::params::resolve_max_rounds_cfg(&cfg_for_rounds) as usize,
                &stop,
                None,
                // 任务执行链路不挂单次覆盖，按 bot-config.json 全局默认（RE-1）
                None,
                model,
                None, // per-onion 温度覆盖：任务执行链路不挂
            )
            .await;
            // LoopTrace 统计进 trace 汇总槽（轮数/工具数/tokens → exec_traces 行）
            if let Ok((_, _, trace)) = &outcome {
                if let Ok(mut c) = hook_for_run.lock() {
                    c.stats = Some(trace.clone());
                }
            }
            outcome.map(|(text, refs, _)| (text, refs))
        },
    )
    .await
}

/// run_task_in_chat 内核（模型循环注入——测试接 mock LLM server 全链路驱动，
/// 与 run_model_loop_core / truncate_with_summary_core 同先例）。
/// 流程：开关/防重入/任务校验 → 建新会话 + 任务块落库 + chat-open-session
/// → ChatGuard（执行期同会话 bot_chat 插话被拒）→ 记忆注入 → 模型循环
/// → assistant 回复落库（失败也落 ⚠️ 行）→ 失败沉淀 lesson。
pub async fn run_task_in_chat_with<R: tauri::Runtime, Run, Fut>(
    app: &tauri::AppHandle<R>,
    task_id: &str,
    origin: TaskExecOrigin,
    // 执行 trace 挂钩（None = 不采集，测试与遗留路径用）。
    // 内核负责建 trace（begin）与收尾（end）——成对执行，中途无早退分支。
    trace_hook: Option<std::sync::Arc<std::sync::Mutex<crate::trace_sink::TraceCapture>>>,
    // ask_user 提问预算（Wry 壳从 bot 参数解析后传入；内核是泛型 Runtime，
    // 读不了 Wry 绑定的 bot-config。测试传任意值即可）
    ask_budget: u8,
    run: Run,
) -> CommandResult<TaskChatRun>
where
    Run: FnOnce(tauri::AppHandle<R>, Vec<serde_json::Value>, StopGuard) -> Fut,
    Fut: std::future::Future<Output = CommandResult<(String, Vec<TaskRef>)>>,
{
    // 开关关闭时明确拒绝
    if !crate::bot_slash::bot_enabled(app) {
        return Err(CommandError::BotDisabled);
    }
    // 防重入：同一任务卡已有执行实例在跑 → 直接拒绝（RAII 守卫随函数返回/panic 自动释放）
    let Some(_exec_guard) = ExecGuard::acquire(app, task_id) else {
        // 拒绝也留痕：否则无法区分「用户在前次执行未结束时重复触发」与「守卫泄漏」
        crate::bot::audit_log(
            app,
            &format!(
                "execute_task_rejected | id: {} | 已有执行实例在跑（防重入拦截）",
                crate::bot::truncate_for_log(task_id, 60)
            ),
        );
        return Err(CommandError::TaskInvalidState {
            reason: "该任务卡正在执行中，请等待完成后再触发".into(),
        });
    };
    let task = crate::db::db_load_for(app)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.id == task_id && t.deleted_at.is_none())
        .ok_or("任务卡不存在或已在回收站")?;
    if task.column == crate::db::TaskStatus::Done {
        return Err(CommandError::TaskInvalidState {
            reason: "这张卡已标记完成；如需重新执行，先在卡片上取消完成".into(),
        });
    }
    if task.archived == Some(true) {
        return Err(CommandError::TaskInvalidState {
            reason: "任务已归档，不能执行；请先恢复".into(),
        });
    }
    crate::bot::audit_log(
        app,
        &format!(
            "execute_task | id: {} | title: {} | origin: {}",
            task.id,
            crate::bot::truncate_for_log(&task.title, 60),
            origin.as_str()
        ),
    );
    // 1. 新会话 + 任务块 user 消息落库 + chat-open-session 广播
    let sid = create_exec_session(app, &task, origin).await?;
    // 2. ChatGuard（设计 3.4：bot_execute_task 纳入会话锁——执行期间同会话的
    // bot_chat 插话会被拒「稍候再发」，防流式/历史交错）。新会话正常不会冲突，
    // 冲突说明守卫串号，按内部错误处理。
    let _chat_guard = match ChatGuard::acquire(app, Some(&sid)) {
        Ok(g) => g,
        Err(()) => {
            return Err(CommandError::Internal(format!(
                "执行会话 {sid} 已被占用（会话锁串号）"
            )));
        }
    };
    // 登记 session_id → TaskExecOrigin 映射，tool_link_file_to_task 内部查询。
    // 末尾无论成败都要 unregister_exec_session 清理。守卫成功后再登记——
    // acquire 失败的早退分支不应泄漏登记（无人 unregister）。
    crate::tool_guard::register_exec_session(&sid, origin);
    // 执行提问授权（ask_user 按 session_id 查表）：
    // - 手动执行任务卡：默认开启（预算 + 假设兜底 + 可停止的等待）——
    //   交互式技能的确认点（题材/书名等）在手动执行里也要有通道；
    // - 预算 = bot 参数 tools.ask_budget（Wry 壳解析后传入）；
    //   定时/批量保持无人值守自治（不注册，调用即回落假设）
    let ask_registered = if origin == TaskExecOrigin::Manual {
        crate::ask_engine::register_ask_context(app, &sid, &task.id, &task.title, ask_budget);
        true
    } else {
        false
    };
    // 建执行 trace + 注册 session→trace 映射（span/file_change 采集开关）。
    // 位置保证：此后到函数尾无早退分支，end_trace 必然成对执行。观测面失败降级 None。
    if let Some(hook) = &trace_hook {
        let trace_id = crate::trace_sink::begin_trace(
            app,
            &sid,
            Some(task.id.clone()),
            origin.as_str(),
            Some(task.title.clone()),
        )
        .await;
        if let Ok(mut c) = hook.lock() {
            c.trace_id = trace_id;
            c.session_id = Some(sid.clone());
        }
    }
    let stop = StopGuard::new(app, true, Some(sid.clone()));
    // 收尾判定停止用：stop 本体 move 进 run，先留只读令牌
    let stop_token = stop.token();
    let block = build_task_block(&task);
    let mut msgs = vec![
        serde_json::json!({"role": "system", "content": format!("{}\n\n{}\n\n{}", EXECUTE_SYSTEM_PROMPT, gen_dir_rule(app), build_skill_block_for(app, Some(&task.title)).await)}),
        serde_json::json!({"role": "user", "content": block}),
    ];
    // 任务卡执行/定时调度也注入记忆块——助手执行任务时知道用户
    // 偏好；查询 = 任务标题+备注前 200 字；失败静默降级为无记忆块（injection_block 内部兜底）。
    let mem_query: String = format!("{} {}", task.title, task.note.as_deref().unwrap_or(""))
        .chars()
        .take(200)
        .collect();
    if let Some(mem_block) = crate::memory::injection_block(app, &mem_query).await {
        msgs.insert(
            1,
            serde_json::json!({"role": "system", "content": mem_block}),
        );
    }
    // 交给机器人：卡片切机器人头像（前端 tasks-changed 广播后实时更新）
    set_bot_assigned(app, &task.id, true).await;
    let outcome = run(app.clone(), msgs, stop).await;
    // 执行结束（无论成败）：清除标记，恢复用户头像
    set_bot_assigned(app, &task.id, false).await;
    let outcome = outcome.map(|(text, refs)| BotChatResult {
        text,
        task_refs: refs,
    });
    // trace 收尾（无论成败；与 begin 成对）+ 注销 session→trace 映射
    if let Some(hook) = &trace_hook {
        crate::trace_sink::end_trace(
            app,
            hook,
            outcome.is_ok(),
            // 用户停止也返回 Ok（「⏹ 已停止」），按令牌判定收尾为 stopped
            stop_token.stopped(),
            outcome.as_ref().err().map(|e| e.to_string()),
        )
        .await;
    }
    // assistant 回复落库（失败也落 ⚠️ 行——会话即执行记录，留证可回看）
    persist_exec_reply(app, &task, &sid, &outcome).await;
    //  收尾：解除 session 注册（无论成败），按 TaskExecOrigin 分流落绑定通知。
    crate::tool_guard::unregister_exec_session(&sid);
    // 提问上下文成对注销（同执行会话生命周期）
    if ask_registered {
        crate::ask_engine::unregister_ask_context(app, &sid);
    }
    let task_column = crate::db::db_load_for(app).await.ok().and_then(|tasks| {
        tasks
            .into_iter()
            .find(|t| t.id == task_id)
            .map(|t| t.column)
    });
    if let Some(artifacts) =
        crate::bot_artifacts::should_emit(app, task_id, origin, task_column.map(|s| s.as_str()))
            .await
    {
        // 通知中心落一条持久化消息（原  挂件 artifact-batch-ready 弹窗已下线：
        // 消息互不覆盖、重启不丢）。失败不阻断执行收尾，stderr 留痕。
        let paths: Vec<String> = artifacts.iter().map(|a| a.path.clone()).collect();
        let origin_label = match origin {
            TaskExecOrigin::Manual => "手动执行",
            TaskExecOrigin::Scheduled => "定时执行",
            TaskExecOrigin::Batch => "批量执行",
        };
        let payload = serde_json::json!({
            "taskId": task_id,
            "taskTitle": task.title,
            "origin": origin_label,
            "paths": paths,
        });
        let title = format!(
            "任务「{}」完成，{} 个产物待绑定",
            task.title,
            payload["paths"].as_array().map(|a| a.len()).unwrap_or(0)
        );
        let body = format!("{origin_label} · 勾选要绑定到任务卡的文件");
        let insert = crate::db::open_db(app).and_then(|conn| {
            crate::notifications::notif_insert(
                &conn,
                &format!("artifact:{task_id}:{sid}"),
                crate::notifications::KIND_ARTIFACT,
                &title,
                &body,
                &payload,
            )
        });
        match insert {
            Ok(true) => crate::notifications::emit_changed(app),
            Ok(false) => {}
            Err(e) => eprintln!("[notifications] 产物绑定通知落库失败：{e}"),
        }
    }
    match outcome {
        Ok(result) => Ok(TaskChatRun {
            session_id: sid,
            result,
        }),
        Err(e) => {
            // 任务执行失败自动沉淀一条 lesson（source=system，
            // 语义去重合并同类失败）；写失败只记审计，不影响原错误返回
            crate::memory::auto_lesson_on_task_failure(app, &task.title, &e.message()).await;
            Err(e)
        }
    }
}

/// 任务卡执行上下文块（[任务卡执行] + 标题/状态/备注/验收标准/子任务/截止/绑定文件），
/// 整卡连续执行（run_task_in_chat）与逐步执行（exec_steps）共用
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
/// （泛型 Runtime：mock runtime 测试可直调）
pub(crate) async fn set_bot_assigned<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    task_id: &str,
    assigned: bool,
) {
    let Ok(all) = crate::db::db_load_for(app).await else {
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
    t.expected_updated_at = t.updated_at; // RMW 基线 = 快照 updated_at
    t.updated_at = Some(chrono::Utc::now().timestamp_millis());
    if let Err(e) = crate::db::db_upsert_for(app, vec![t.clone()]).await {
        // 写失败不能只当没发生过：头像与库里的归属会一直不一致，且无迹可查
        crate::bot::audit_log(
            app,
            &format!(
                "bot_assigned_commit_failed | task_id: {} | err: {}",
                task_id,
                e.message()
            ),
        );
        return;
    }
    crate::bot::broadcast_after_mutation(app, vec![t], vec![]);
}

// 测试：图片附件纯函数 + 主编编排纯函数

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
        // 白名单根目录传入（测试用临时目录充当白名单根）
        let (v, skipped) = attach_images_in(std::slice::from_ref(&dir), &content);
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
        // 白名单外的图片路径被跳过（防 [附件文件] 块污染外泄）
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
        let (v, _) = attach_images_in(
            &[std::path::PathBuf::from("/tmp")],
            "[附件文件]\n- /tmp/not_exists_xyz.png\n\n看看",
        );
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

    // ── L0 pre_ingest：输入规范化 ──

    #[test]
    fn ingest_strips_invisible_junk_but_keeps_text_untouched() {
        // 从 Word/网页复制的典型串：BOM + word-joiner + soft-hyphen 混在词中间，
        // 肉眼完全看不出，关键词匹配却整个失效
        let raw = "\u{FEFF}做\u{2060}一\u{00AD}份 PPT";
        let out = normalize_user_input_core(raw);
        assert_eq!(out.text, "做一份 PPT");
        assert!(out.changed);
        assert_eq!(out.stripped, 3, "三个不可见字符都要计数（审计靠它排障）");
    }

    #[test]
    fn ingest_keeps_zwj_emoji_sequences_intact() {
        // 本条是「不可见字符当垃圾清」最容易踩的坑：U+200D 把三个 emoji
        // 拼成**一个**合成字形（家庭）。当噪音删掉，用户看到的家庭会变成
        // 三个单独的人——这是程序在改坏用户的话，不是清洗。
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        let out = normalize_user_input_core(&format!("我家是 {family}"));
        assert!(
            out.text.contains(family),
            "ZWJ 序列必须原样保留：{:?}",
            out.text
        );
        assert!(!out.changed, "含 ZWJ 的正常输入不应被判定为「已改写」");
        // ZWNJ 同理：波斯语/印地语里它表示字母不成形
        let persian = "\u{0641}\u{200C}\u{0645}";
        assert!(
            normalize_user_input_core(persian).text.contains('\u{200C}'),
            "ZWNJ 有语言语义，不得清"
        );
    }

    #[test]
    fn ingest_maps_zero_width_space_to_real_space_not_away() {
        // 零宽空格在中文排版里就是个空格。删掉会把「做\u{200B}PPT」粘成
        // 「做PPT」；换回真空格才是「做 PPT」，匹配得上。
        let out = normalize_user_input_core("做\u{200B}PPT");
        assert_eq!(out.text, "做 PPT");
    }

    #[test]
    fn ingest_unifies_crlf_without_counting_cr_as_stripped() {
        // 顺序反了（先逐字符过滤再统一行尾）会把 \r 当垃圾删掉：结果对，
        // 但审计会记「清掉 1 个不可见字符」，把排障引到错误方向。
        let out = normalize_user_input_core("第一行\r\n第二行\r\n");
        assert_eq!(out.text, "第一行\n第二行");
        assert_eq!(out.stripped, 0, "CRLF 的 \\r 不该计入不可见字符");
    }

    #[test]
    fn ingest_does_not_touch_code_blocks_or_blank_line_runs() {
        // 刻意不做结构级改写：代码缩进与段落分隔都是 Markdown 语义。
        let src = "```rust\nfn main() {\n\n\n    println!();\n}\n```";
        let out = normalize_user_input_core(src);
        assert_eq!(out.text, src, "代码块内容不得被折叠或改写");
        assert!(!out.changed);
        assert!(
            normalize_user_input_core("A\n\n\n\nB")
                .text
                .contains("\n\n\n\n"),
            "连续空行不做折叠（排版意图与复制事故分不开，阈值判断留给别的轮次）"
        );
    }

    #[test]
    fn ingest_leaves_clean_input_untouched() {
        let out = normalize_user_input_core("帮我做一份 PPT");
        assert_eq!(out.text, "帮我做一份 PPT");
        assert!(!out.changed, "干净输入不该被判改写（否则日志全是噪音）");
    }

    #[test]
    fn ingest_rewrites_only_the_last_user_message() {
        let mut msgs = vec![
            msg("user", "旧问题"),
            msg("assistant", "旧回答"),
            msg("user", "\u{FEFF}新问题\u{200B}来了"),
        ];
        let n = normalize_user_input(&mut msgs).expect("有改动才返回");
        assert_eq!(n.text, "新问题 来了");
        assert_eq!(msgs[0].content, "旧问题", "历史不得被就地改写");
        assert_eq!(msgs[1].content, "旧回答");
        assert_eq!(msgs[2].content, "新问题 来了", "本轮必须改到位");
    }

    #[test]
    fn ingest_skips_non_user_last_message_and_reports_none_when_unchanged() {
        // 只有 user 角色的最后一条是「本轮输入」；否则规范化它等于改写历史
        let mut assistant_last = vec![msg("assistant", "回答\u{FEFF}")];
        assert!(
            normalize_user_input(&mut assistant_last).is_none(),
            "最后一条不是用户消息时不得改写"
        );
        assert_eq!(assistant_last[0].content, "回答\u{FEFF}", "原样保留");

        let mut clean = vec![msg("user", "普通输入")];
        assert!(normalize_user_input(&mut clean).is_none());
    }

    /// 接线守卫：L0 必须在**所有**输入判定之前，且就地改 `messages`。
    ///
    /// 挪到任何一处判定之后，就出现「有的扫描读清洗后、有的读原文」——
    /// 规范化做了一半，症状是「清洗了但路由还是不匹配」。锚点用 concat! 拆分
    /// 防自匹配。
    #[test]
    fn ingest_runs_before_every_input_decision_and_mutates_in_place() {
        // 限定在 bot_chat 函数体内搜：本文件前部还有好几个同名符号的定义与
        // 文档提及，全文件搜第一个会命中它们，断言就变成在比谁先出现。
        let whole = include_str!("bot_chat.rs");
        let start = whole
            .find("pub async fn bot_chat(")
            .expect("bot_chat 函数体起点");
        let body = &whole[start..];
        let l0 = body
            .find("normalize_user_input(&mut messages)")
            .expect("bot_chat 必须调 L0 规范化");
        // 五个输入侧判定各自扫 messages.last()，L0 必须排在它们之前
        for (label, needle) in [
            ("exec_steps 挂起恢复", "exec_steps::has_pending_for"),
            ("纠正信号检测", "corrections::detect_correction"),
            ("批量执行触发", "intent_router::is_chat_execute_trigger"),
            ("意图路由", "intent_router::route_user_input"),
            ("技能推荐", "build_skill_block_for"),
        ] {
            let at = body
                .find(needle)
                .unwrap_or_else(|| panic!("bot_chat 内找不到输入侧判定点：{label}"));
            assert!(
                l0 < at,
                "L0 规范化必须排在「{label}」之前（现 l0={l0} {label}={at}）"
            );
        }
        assert!(
            body[..l0].contains("let mut messages"),
            "入参必须先转成 mut 才能就地改写"
        );
    }

    // ── 历史预算 / 图片窗口 / think 剥除 ──

    fn msg(role: &str, content: &str) -> ChatMsg {
        ChatMsg {
            role: role.into(),
            content: content.into(),
        }
    }

    /// 不调摘要器的截断内核 = 纯截断语义（mock summarizer 返 Err）
    async fn truncate_only(messages: Vec<ChatMsg>, budget: usize) -> (Vec<ChatMsg>, usize) {
        let (kept, _summary, dropped) = truncate_with_summary_core(messages, budget, |_m| async {
            Err(CommandError::DomainRule {
                domain: "test".to_string(),
                reason: "本测试不摘要".to_string(),
            })
        })
        .await;
        (kept, dropped)
    }

    #[test]
    fn truncate_chat_history_within_budget_unchanged() {
        let msgs = vec![
            msg("user", "你好"),
            msg("assistant", "在的"),
            msg("user", "列任务"),
        ];
        let (kept, dropped) = futures::executor::block_on(truncate_only(msgs, 100));
        assert_eq!(dropped, 0);
        assert_eq!(kept.len(), 3);
    }

    #[test]
    fn truncate_chat_history_drops_oldest_first() {
        let long = "x".repeat(60);
        let msgs = vec![
            msg("user", &long),
            msg("assistant", &long),
            msg("user", &long),
        ];
        let (kept, dropped) = futures::executor::block_on(truncate_only(msgs, 100));
        assert_eq!(dropped, 2, "超预算时最旧的先丢，dropped 数应正确");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].role, "user");
    }

    #[test]
    fn truncate_chat_history_keeps_last_even_over_budget() {
        let msgs = vec![msg("user", &"x".repeat(200))];
        let (kept, dropped) = futures::executor::block_on(truncate_only(msgs, 10));
        assert_eq!(dropped, 0, "最后一条（本轮用户消息）永远保留");
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn image_attach_indices_hits_user_in_last_three() {
        let msgs = vec![
            msg("user", "a"),
            msg("assistant", "b"),
            msg("user", "c"),
            msg("user", "d"),
        ];
        assert_eq!(image_attach_indices(&msgs), vec![2, 3]);
    }

    #[test]
    fn image_attach_indices_skips_older_user() {
        let msgs = vec![
            msg("user", "a"),
            msg("assistant", "b"),
            msg("assistant", "c"),
            msg("user", "d"),
        ];
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

    // ── 截断即摘要（编排内核，摘要器注入） ──

    #[tokio::test]
    async fn truncate_with_summary_core_success_returns_summary() {
        // budget=63：本轮消息（4 字）+ 一条 60 字旧消息=64 超预算 → 最旧两条被丢
        let long = "x".repeat(60);
        let msgs = vec![
            msg("user", &long),
            msg("assistant", &long),
            msg("user", "本轮问题"),
        ];
        let (kept, summary, dropped) =
            truncate_with_summary_core(msgs, 63, |dropped_msgs| async move {
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
        let msgs = vec![
            msg("user", &long),
            msg("assistant", &long),
            msg("user", "本轮问题"),
        ];
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
        let msgs = vec![
            msg("user", &long),
            msg("assistant", &long),
            msg("user", "本轮问题"),
        ];
        let (_, summary, dropped) =
            truncate_with_summary_core(msgs, 63, |_| async { Ok("   ".to_string()) }).await;
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
}

// 测试：Err("...".into()) 逃生舱改走专用 CommandError 变体

#[cfg(test)]
mod command_error_mapping_tests {
    use super::*;

    /// bot_chat 的开关守卫：bot_get_enabled == false 时必须映射到 BotDisabled 专用变体
    /// （不是 From<&str> 兜底的 Internal）
    #[test]
    fn bot_disabled_maps_to_dedicated_variant() {
        let err = require_bot_enabled(false).expect_err("关闭时应返回 Err");
        assert_eq!(err, CommandError::BotDisabled, "应为 BotDisabled 专用变体");
        assert_eq!(err.code(), crate::error::CommandErrorCode::BotDisabled);
        assert!(
            err.is_recoverable(),
            "BotDisabled 应可恢复（引导去设置页开启）"
        );
        assert!(err.message().contains("机器人聊天已关闭"));
        assert!(require_bot_enabled(true).is_ok(), "开启时应通过");
    }

    /// bot_compact 的 key 守卫：空 key → ApiKeyMissing 专用变体；非空 → Ok
    #[test]
    fn require_api_key_empty_returns_api_key_missing() {
        for empty in ["", "   ", "\n\t "] {
            let err = require_api_key(empty).expect_err("空 key 应返回 Err");
            assert_eq!(err, CommandError::ApiKeyMissing, "输入 {empty:?}");
            assert_eq!(err.code(), crate::error::CommandErrorCode::ApiKeyMissing);
            assert!(
                err.is_recoverable(),
                "ApiKeyMissing 应可恢复（去设置页填 key）"
            );
        }
        assert!(require_api_key("sk-test-123").is_ok(), "非空 key 应通过");
    }
}

/// `ChatGuard` / `ExecGuard` 迁入 `AppState` 后的守卫语义与实例隔离。
/// 这两个守卫此前没有单测（只由 `tests/task_chat_exec.rs` 间接覆盖），而它们正是
/// 「同会话/同卡并发重复执行」的唯一闸门；Drop 清错实例 = 闸门泄漏。
#[cfg(test)]
mod chat_exec_guard_tests {
    use super::*;

    fn test_handle() -> AppHandle<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        tauri::Manager::manage(&app, crate::app_state::AppState::default());
        app.handle().clone()
    }

    #[test]
    fn chat_guard_blocks_same_session_and_releases_on_drop() {
        let app = test_handle();
        let guard = ChatGuard::acquire(&app, Some("sess-guard-1")).expect("首次应可进入");
        assert!(
            ChatGuard::acquire(&app, Some("sess-guard-1")).is_err(),
            "同会话并发应被拒"
        );
        assert!(
            ChatGuard::acquire(&app, Some("sess-other")).is_ok(),
            "别的会话不受影响"
        );
        assert!(chat_guard_is_held(&app, "sess-guard-1"));
        drop(guard);
        assert!(
            !chat_guard_is_held(&app, "sess-guard-1"),
            "Drop 后应释放（清错实例这里就会是 true）"
        );
        // 无会话 id 不加锁：连续两次都应成功
        assert!(ChatGuard::acquire(&app, None).is_ok());
        assert!(ChatGuard::acquire(&app, None).is_ok());
    }

    #[test]
    fn guards_isolated_between_injected_instances() {
        let a = test_handle();
        let b = test_handle();
        let _ga = ChatGuard::acquire(&a, Some("same-sess")).expect("a 首次");
        assert!(
            ChatGuard::acquire(&b, Some("same-sess")).is_ok(),
            "两个注入实例之间不得互相干扰（ChatGuard）"
        );
        let _ea = ExecGuard::acquire(&a, "same-task").expect("a 首次");
        assert!(
            ExecGuard::acquire(&b, "same-task").is_some(),
            "两个注入实例之间不得互相干扰（ExecGuard）"
        );
        // 反向：注入实例与兜底实例也必须互不可见
        let bare = tauri::test::mock_app().handle().clone();
        assert!(
            ChatGuard::acquire(&bare, Some("same-sess")).is_ok(),
            "未注入的 handle 走兜底实例，不得看到注入实例里的会话"
        );
    }
}
