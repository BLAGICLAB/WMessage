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

/// 记忆块注入薄壳：语义记忆体（crate::memory：mem_items +
/// 语义嵌入混合检索）；任何失败一律 None 静默降级为「无记忆块」，绝不弄挂主对话
///（设计约束）；失败记 WARN 审计便于排查。
async fn build_memory_block(app: &AppHandle, query: &str) -> Option<String> {
    crate::memory::injection_block(app, query).await
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
    /// 本轮压根没跑（会话锁被占）：`text` 是拒答文案，不是模型答复。
    /// 前端据此把这条结果当拒答处理——不落库、不进上下文，只显示一条本地提示。
    /// 判据是这个标记而不是文案本身：文案可以被别的路径复述，标记不会。
    pub busy: bool,
    /// 本回合的 trace id（`exec_traces` 行号）：聊天回合也落 trace，前端据此
    /// 把这条答复关联到它的执行轨迹。None = 本回合没开 trace（无会话 id、
    /// 或 trace 建行失败降级）——不是错误。
    pub trace_id: Option<i64>,
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

/// `apply_skill_route` 的返回值。Done 是「路由终态」（chat 直接返回），
/// FallThrough 是「落入步骤 5」（继续走 run_model_loop）。
enum SkillRouteOutcome {
    /// 直接返回给前端（Done / AwaitUser 两种终态；Terminated 走 Err 不进 Outcome）
    Done(BotChatResult),
    /// 落入步骤 5，带可选的 recovery_hint（auto-mode FailedButRecoverable 才有内容）
    FallThrough { recovery_hint: Option<String> },
}

/// 会话已有执行实例时的统一拒答文案。
///
/// 「本轮根本没跑」的判据是 `BotChatResult::busy` 标记（见 `busy_chat_result`），
/// 不是这段文案本身——文案只是拒答时随结果带出去的说明。
pub(crate) const CHAT_BUSY_REPLY: &str = "这条会话正在处理上一条消息，请稍候再发。";

/// 聊天入口（真源化契约）：前端只发本轮意图（content + 预生成的 client_msg_id），
/// 不再传完整历史——上下文由后端从库里重建（真源唯一，前端投影）。
///
/// 前置顺序是 busy 零落行的根基：enabled 开关 → 会话防重入守卫 → **都过了才**
/// 落用户行进主链。busy 拒答发生在任何落库之前——旧链路「回合开始前端先存
/// 快照」会把被拒的用户消息孤立在库里（下轮上下文多一条没被理会的用户发言）。
#[tauri::command]
pub async fn bot_chat(
    app: AppHandle,
    session_id: Option<String>,
    content: String,
    client_msg_id: String,
    // 用户行随行的任务引用（前端选卡器的 JSON），原样落 refs 列供气泡读回
    refs_json: Option<String>,
    // 推理强度单次覆盖（RE-1）：聊天输入框按会话选的抽象档位（off/low/medium/high），
    // 由前端解析「会话覆盖 ?? 后台默认」后传入具体值；None = 按后台默认。
    // 只影响本次聊天调用，不回写 bot-config.json。
    reasoning_effort: Option<String>,
) -> CommandResult<BotChatResult> {
    bot_chat_entry(
        &app,
        session_id,
        content,
        client_msg_id,
        refs_json,
        reasoning_effort,
    )
    .await
}

/// bot_chat 命令的实体（命令薄壳直通，测试可达）。
async fn bot_chat_entry(
    app: &AppHandle,
    session_id: Option<String>,
    content: String,
    client_msg_id: String,
    refs_json: Option<String>,
    reasoning_effort: Option<String>,
) -> CommandResult<BotChatResult> {
    require_bot_enabled(bot_get_enabled(app.clone()))?;
    let _chat_guard = match ChatGuard::acquire(app, session_id.as_deref()) {
        Ok(g) => g,
        Err(()) => {
            crate::bot::audit_log(
                app,
                &format!(
                    "bot_chat_rejected | session: {} | 已有执行实例在跑（防重入拦截）",
                    crate::bot::truncate_for_log(session_id.as_deref().unwrap_or("<none>"), 60)
                ),
            );
            return Ok(busy_chat_result());
        }
    };
    // 用户行先落库再重建上下文（§5.2 ①→②）：末条 = 刚落的用户行
    let messages = user_turn_context(
        app,
        session_id.as_deref(),
        &content,
        &client_msg_id,
        refs_json.as_deref(),
    )
    .await?;
    let out = match bot_chat_guarded(
        app.clone(),
        messages,
        session_id.clone(),
        reasoning_effort,
        TurnOrigin::User,
    )
    .await
    {
        Ok(out) => out,
        // 失败轮也要落行（真源化 §5.2 ③ 的 Err 半边，批 4 收编）：已流出的
        // 正文只活在聚合器里（模型循环 Err 时拿不到 BotChatResult），不落行
        // = 重启后失败叙事凭空消失。失败行落库后前端 catch 只做本地视图兜底
        Err(e) => {
            if let Some(sid) = session_id.as_deref() {
                let record = crate::bot::turn_record::take(sid);
                let row = build_failed_row(record, &e);
                let sid2 = sid.to_string();
                let (msg_id, mut row) = crate::db::lock::with_write_conn(app, move |conn| {
                    let id =
                        crate::db::bot_history::bot_history_upsert_row_inner(conn, &sid2, &row)?;
                    Ok((id, row))
                })
                .await
                .map_err(CommandError::DbError)?;
                row.msg_id = Some(msg_id);
                emit_row_upserted(app, Some(sid), &row);
            }
            return Err(e);
        }
    };
    // 终稿落行（真源化 §5.2 ③）：聚合器收走本轮事实（thinking/tools/files/
    // 终态）→ upsert 终稿行 → bot-msg-upserted。恒在该回合 bot-turn-status
    // 终态事件之后——终态在循环唯一出口发出（core 的 final_status 收口），
    // 这里才返回，占位替换依赖此序
    if let Some(sid) = session_id.as_deref() {
        let record = crate::bot::turn_record::take(sid);
        let row = build_final_row(record, &out.result);
        let sid2 = sid.to_string();
        // 行内核就地补服务端身份并随返回值带回：事件载荷带同一 id（前端对账键）
        let (msg_id, mut row) = crate::db::lock::with_write_conn(app, move |conn| {
            let id = crate::db::bot_history::bot_history_upsert_row_inner(conn, &sid2, &row)?;
            Ok((id, row))
        })
        .await
        .map_err(CommandError::DbError)?;
        row.msg_id = Some(msg_id);
        emit_row_upserted(app, Some(sid), &row);
    }
    // 交互聊天：模型确实看到了那段回报 → 记「已吸收」（digest 不再重念它）。
    for id in &out.absorb {
        crate::bot_orchestrator::mark_digested(session_id.as_deref(), id);
    }
    Ok(out.result)
}

/// 失败行组装（纯函数，锚点可直测）：已流出正文 + ⚠️ 失败说明，格式与前端
/// 旧 catch 合成行一致（`{partial}\n\n{note}`）——重启后失败叙事从库还原。
/// 失败行无 refs / trace / 终态细节，turn_status = failed。
fn build_failed_row(
    record: crate::bot::turn_record::TurnRecord,
    e: &CommandError,
) -> crate::db::BotMsgRow {
    let partial = record.partial_text.unwrap_or_default();
    let note = format!("⚠️ {}", e.message());
    let content = if partial.is_empty() {
        note
    } else {
        format!("{partial}\n\n{note}")
    };
    crate::db::BotMsgRow {
        msg_id: None,
        role: "assistant".into(),
        content,
        refs_json: None,
        thinking: record.thinking,
        tools_json: record.tools_json,
        turn_status: Some("failed".into()),
        file_changes_json: record.file_changes_json,
        trace_id: None,
    }
}

/// 终稿行组装（纯函数，锚点可直测）：聚合事实 + 循环产出 → 一条 assistant 行。
/// 身份由行内核就地生成（前端经事件获得 id）。turn_status 兜底 completed——
/// 循环唯一出口必发终态，None 只在异常路径出现，与前端旧口径一致。
fn build_final_row(
    record: crate::bot::turn_record::TurnRecord,
    result: &BotChatResult,
) -> crate::db::BotMsgRow {
    crate::db::BotMsgRow {
        msg_id: None,
        role: "assistant".into(),
        content: result.text.clone(),
        refs_json: serde_json::to_string(&result.task_refs).ok(),
        thinking: record.thinking,
        tools_json: record.tools_json,
        turn_status: Some(
            record
                .turn_status
                .unwrap_or_else(|| "completed".to_string()),
        ),
        file_changes_json: record.file_changes_json,
        trace_id: result.trace_id,
    }
}

/// 用户轮的真源化落行 + 上下文重建（§5.2 ①）。
///
/// 用户行经行内核 upsert（client_msg_id 即行身份：retry 重放同 id 命中旧行，
/// 保位保首插时刻）→ 广播 bot-msg-upserted → 从库里全量行序重建本轮上下文。
/// 库里此刻就含刚落的用户行，汇报轮 append 过的行也天然在内——这正是把
/// 上下文来源从前端内存切到库的意义。
async fn user_turn_context<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Option<&str>,
    content: &str,
    client_msg_id: &str,
    refs_json: Option<&str>,
) -> CommandResult<Vec<ChatMsg>> {
    let Some(sid) = session_id else {
        // UI 恒带会话 id；None 属防御性保底（无真源可落，单消息上下文直通）
        return Ok(vec![ChatMsg {
            role: "user".into(),
            content: content.to_string(),
        }]);
    };
    let row = crate::db::BotMsgRow {
        msg_id: Some(client_msg_id.to_string()),
        role: "user".into(),
        content: content.to_string(),
        refs_json: refs_json.map(str::to_string),
        thinking: None,
        tools_json: None,
        turn_status: None,
        file_changes_json: None,
        trace_id: None,
    };
    let row_for_event = row.clone();
    let sid2 = sid.to_string();
    crate::db::lock::with_write_conn(app, move |conn| {
        crate::db::bot_history::bot_history_upsert_row_inner(conn, &sid2, &row)
    })
    .await
    .map_err(CommandError::DbError)?;
    emit_row_upserted(app, Some(sid), &row_for_event);
    let rows = crate::db::bot_history::bot_history_load(app.clone(), sid.to_string()).await?;
    Ok(rows
        .into_iter()
        .map(|r| ChatMsg {
            role: r.role,
            content: r.content,
        })
        .collect())
}

/// 行投影广播（事件契约 §七）：走 emit_recorded 统一出口（sessionId 信封 +
/// 回放缓冲）。载荷逐字段手拼而非 serde 整行——序列化是可失败路径，这里要
/// 不可失败的构造。
fn emit_row_upserted<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: Option<&str>,
    row: &crate::db::BotMsgRow,
) {
    let payload = serde_json::json!({
        "msgId": row.msg_id,
        "role": row.role,
        "content": row.content,
        "refsJson": row.refs_json,
        "thinking": row.thinking,
        "toolsJson": row.tools_json,
        "fileChangesJson": row.file_changes_json,
        "turnStatus": row.turn_status,
        "traceId": row.trace_id,
    });
    crate::bot::stream_log::emit_recorded(app, session_id, "bot-msg-upserted", payload);
}

/// 删单行（removeMessage 的真源侧操作）：按 msg_id 精确删。返回删除条数
/// （0 = 行不存在，前端提示即可）。bot-msg-dropped 指名广播。
#[tauri::command]
pub async fn bot_history_drop(
    app: AppHandle,
    session_id: String,
    msg_id: String,
) -> CommandResult<usize> {
    let n = {
        let sid = session_id.clone();
        let mid = msg_id.clone();
        crate::db::lock::with_write_conn(&app, move |conn| {
            crate::db::bot_history::bot_history_drop_inner(conn, &sid, &mid)
        })
        .await
        .map_err(CommandError::DbError)?
    };
    if n > 0 {
        crate::bot::stream_log::emit_recorded(
            &app,
            Some(&session_id),
            "bot-msg-dropped",
            serde_json::json!({ "msgIds": [msg_id] }),
        );
    }
    Ok(n)
}

/// 摘要行换血（compact 的真源侧操作）：msg_ids 集合行换成一条摘要行，
/// 摘要行身份/位置由内核定（占被删集合起点）。两个事件一起广播：
/// 被删行 bot-msg-dropped（指名）+ 摘要行 bot-msg-upserted（整行投影）。
#[tauri::command]
pub async fn bot_history_replace_summary(
    app: AppHandle,
    session_id: String,
    msg_ids: Vec<String>,
    content: String,
) -> CommandResult<crate::db::BotMsgRow> {
    let (dropped_ids, summary) = {
        let sid = session_id.clone();
        crate::db::lock::with_write_conn(&app, move |conn| {
            crate::db::bot_history::bot_history_replace_summary_inner(
                conn, &sid, &msg_ids, &content,
            )
        })
        .await
        .map_err(CommandError::DbError)?
    };
    if !dropped_ids.is_empty() {
        crate::bot::stream_log::emit_recorded(
            &app,
            Some(&session_id),
            "bot-msg-dropped",
            serde_json::json!({ "msgIds": dropped_ids }),
        );
        emit_row_upserted(&app, Some(&session_id), &summary);
    }
    Ok(summary)
}

/// retry 截尾（定向操作提前落地，原计划批 4；依赖见 §十二批 2）：删掉排在
/// msg_id 行之后的所有行，让后端重建的上下文不含上一轮的失败回复等残行。
/// 返回删除条数。锚行不存在 = 无事发生（0）。
#[tauri::command]
pub async fn bot_history_drop_after(
    app: AppHandle,
    session_id: String,
    msg_id: String,
) -> CommandResult<usize> {
    let (dropped_ids, n) = {
        let sid = session_id.clone();
        crate::db::lock::with_write_conn(&app, move |conn| {
            crate::db::bot_history::bot_history_drop_after_inner(conn, &sid, &msg_id)
        })
        .await
        .map_err(CommandError::DbError)?
    };
    if !dropped_ids.is_empty() {
        crate::bot::stream_log::emit_recorded(
            &app,
            Some(&session_id),
            "bot-msg-dropped",
            serde_json::json!({ "msgIds": dropped_ids }),
        );
    }
    Ok(n)
}

/// 汇报轮的轮次说明：这一轮没有用户输入，模型唯一的新信息是末尾那段子任务回报。
const REPORT_TURN_PROMPT: &str = "（本轮没有新的用户消息：这是子任务收尾后系统主动发起的汇报轮。）\
请只依据末尾的「子任务回报」向用户说明情况：做了什么、结果如何、有没有需要用户决定的下一步。\
不要反问用户要做什么，也不要重复用户已经知道的内容；\
不要为汇报再派发子任务；回报不足以判断下一步时直接说缺什么。";

/// 汇报轮占位 user 消息：库里一条历史都没有时补的那一条。
///
/// 不是装饰——Anthropic 协议不接受只含 system 的请求，`openai_msgs_to_anthropic`
/// 抽走 system 后 messages 为空就直接报错。文本要写成「等待转述」而不是伪造一条
/// 用户发言，免得模型据此以为用户问过什么。
const REPORT_TURN_STUB: &str = "（系统消息：请读取本轮末尾的「子任务回报」并转述给用户。）";

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
/// 防止压缩请求超 context。截断路径的待摘要消息已被 constants::HISTORY_BUDGET_CHARS
/// 限住，此上限对 /compact 的全量历史才实际生效。
/// 与 constants::CONTEXT_CHARS_HARD_CAP 同值不同域（摘要请求 vs 主请求），
/// 改其中一处前先看另一处。
const SUMMARIZE_MAX_CHARS: usize = 200_000;

/// /compact 快捷命令（真源化契约）：读库取全会话历史交给模型总结成摘要
/// （单次非流式请求，不带工具）。摘要行落库走 bot_history_replace_summary
/// （前端编排），本命令只产出摘要文本——幂等：库里的旧摘要行也参与总结。
#[tauri::command]
pub async fn bot_compact(app: AppHandle, session_id: String) -> CommandResult<String> {
    let rows = crate::db::bot_history::bot_history_load(app.clone(), session_id).await?;
    let messages: Vec<ChatMsg> = rows
        .into_iter()
        .map(|r| ChatMsg {
            role: r.role,
            content: r.content,
        })
        .collect();
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
        // 读库失败要上抛：吞成空列表等于把 DB 故障说成「这张卡没有未勾子任务」，
        // 用户看到的现象是「点了执行却直接走普通执行」，故障原因被埋掉
        if let Some(task) = crate::db::db_load(app.clone())
            .await?
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
    run_task_in_chat(&app, &task_id, TaskExecOrigin::Manual)
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
    t.updated_at = Some(crate::db::now_ms());
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

// 拆出后的子模块转出：保持 `crate::bot_chat::X` 旧路径与旧可见性不变
// （pub 的转成 pub、pub(crate) 的转成 pub(crate)、模块私有的仍是私有）
pub use exec::{
    chat_execute_tasks, run_task_in_chat, run_task_in_chat_with, BatchPolicy, TaskChatRun,
    TaskExecOrigin,
};
use exec::{create_exec_session, persist_exec_reply};
use flow::bot_chat_inner;
// 测试辅助，与 flow 侧定义的 cfg 门控必须一致：定义带 #[cfg(debug_assertions)]，
// 而这里的 re-export 若是无条件的，release 构建就编译不过（debug_assertions 关掉
// 后被转出的项不存在）。生产代码不调用它，release 下也不需要这条路径。
#[cfg(debug_assertions)]
pub use flow::chat_guard_is_held;
pub(crate) use flow::TurnOrigin;
// busy 零落行入口（bot_chat_entry）生产路径就要这三个：enabled 闸、busy 兜底
// 结果、自持守卫后的主链入口
use flow::{bot_chat_guarded, busy_chat_result, require_bot_enabled};
use inject::{build_history_msgs, build_injection};
#[cfg(test)]
use input::normalize_user_input_core;
pub use input::{format_recovery_hint, merge_task_refs_dedup};
pub(crate) use input::{normalize_user_input, strip_think_blocks};
#[cfg(test)]
use media::attach_images_in;
pub(crate) use media::image_part_from_file;
use media::{attach_images, image_attach_indices};
#[cfg(test)]
use report::report_turn_history_from_rows;
pub(crate) use report::{report_subagent_result, ReportOutcome};
pub use summary::{summarize_http, truncate_with_summary_core};
pub(crate) use summary::{summarize_messages, truncate_chat_history_with_summary};

#[cfg(test)]
mod bot_chat_pure_helpers_tests;
#[cfg(test)]
mod chat_exec_guard_tests;
#[cfg(test)]
mod command_error_mapping_tests;
mod exec;
mod flow;
#[cfg(test)]
mod image_attach_tests;
mod inject;
#[cfg(test)]
mod inject_tests;
mod input;
mod media;
mod report;
mod summary;
#[cfg(test)]
mod summary_reuse_tests;
#[cfg(test)]
mod user_turn_tests;
