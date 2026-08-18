//! 内置机器人：大模型聊天 + WMessage 任务管理工具调用。
//!
//! 安全性（对齐《Harness 安全网关》需求）：
//! - 工具白名单：固定 TOOLS schema + execute_tool match，模型编造的工具一律拒绝
//! - 调用熔断：单轮 Function 调用 ≤10 次 + 7 次软警告（收尾提醒）+ 聊天 8 轮/任务执行 10 轮工具循环；HTTP connect 15s / 总超时 300s
//! - Skill 调度器：use_skill 启动技能生命周期（预审/状态机/步数熔断/暂停确认/回滚建议），
//!   调度器仅编排与监督，Function 执行仍强制过七层 Harness（不可绕过）
//! - 参数校验：标题/备注/关键词/子任务/截止时间长度上限、标签数量上限
//! - 提示词黑名单：禁系统命令、禁全盘遍历、禁批量删除、禁编造路径
//! - 审计日志：数据目录 bot.log 记录用户指令、工具名、参数、结果
//! - API Key 存系统凭据存储（keyring），文件不落明文
//!
//! - 开关状态存 bot-enabled.flag（数据目录），与外部 API 的 api-enabled.flag 同一套路
//! - API 配置（base_url / model）存 bot-config.json；key 单独走凭据存储
//! - bot_chat：OpenAI 兼容协议，流式输出经 bot-chat-delta 事件推给挂件窗口；
//!   带 tools（任务管理 + 文档 + 联网工具），模型返回 tool_calls 时进程内执行并续聊；
//!   聊天最多 8 轮工具循环，任务卡执行（bot_execute_task）最多 10 轮
//! - 工具执行直接改 SQLite，改完广播 tasks-changed / tasks-updated(source:"bot")

use crate::audit_event;
use crate::bot_skills::{build_skill_block, tool_use_skill, SkillMeta};
use crate::error::{CommandError, CommandResult};
use crate::intent_router::RouteAction; // F-2：route_user_input 调用迁移到 middleware::run_pre_step
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use tauri::{AppHandle, Emitter, Manager}; // F-6：Runtime 给 audit_log 泛型化

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chrono::Datelike;

// ───────────────────────── /stop 停止标志（按执行实例隔离） ─────────────────────────

/// 活跃执行实例注册表：stop_id → (停止标志, 是否用户交互触发)
type StopMap = std::sync::Mutex<
    std::collections::HashMap<u64, (std::sync::Arc<std::sync::atomic::AtomicBool>, bool)>,
>;
static STOP_REGISTRY: std::sync::OnceLock<StopMap> = std::sync::OnceLock::new();
static NEXT_STOP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn stop_registry() -> &'static StopMap {
    STOP_REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 执行实例的停止标志：run_model_loop 在流式/工具循环检查点检查；Drop 时注销
/// （interactive=true 表示由用户聊天/点 🤖 触发，/stop 只停这类实例，不动后台定时）
struct StopGuard {
    id: u64,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl StopGuard {
    fn new(interactive: bool) -> Self {
        let id = NEXT_STOP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Ok(mut m) = stop_registry().lock() {
            m.insert(id, (flag.clone(), interactive));
        }
        Self { id, flag }
    }

    fn stopped(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = stop_registry().lock() {
            m.remove(&self.id);
        }
    }
}

/// /stop 快捷命令：停止所有用户交互触发的执行（bot_chat / 🤖 任务卡执行），后台定时不受影响
#[tauri::command]
pub fn bot_stop(app: AppHandle) {
    // Skill 调度器联动：强制终止所有活动技能
    crate::bot_skills::skill_terminate_all(&app, "用户停止");
    if let Ok(m) = stop_registry().lock() {
        for (_, (flag, interactive)) in m.iter() {
            if *interactive {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
}

// ───────────────────────── 危险操作确认（删除任务弹窗） ─────────────────────────

/// 待确认请求：id → oneshot 通道（挂件 bot_confirm_response 回填）
type ConfirmMap =
    std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>;
static CONFIRMS: std::sync::OnceLock<ConfirmMap> = std::sync::OnceLock::new();

fn confirms() -> &'static ConfirmMap {
    CONFIRMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 请求用户在挂件确认危险操作（如删除任务）；60s 超时默认拒绝（安全兜底）
async fn ask_user_confirm(app: &AppHandle, tool: &str, detail: &str) -> bool {
    // Skill 调度器联动：高危动作确认开始 → 活动技能转入 Paused
    crate::bot_skills::skill_mark_paused(app, tool);
    // 挂件窗口不存在/不可见时无人应答：直接拒绝，不白等 60s（审计 P2）
    let widget_visible = app
        .get_webview_window("widget")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !widget_visible {
        audit_log(
            app,
            &format!("confirm_skipped | {tool} | {detail} | 挂件不可见，默认拒绝"),
        );
        crate::bot_skills::skill_confirm_result(app, false);
        return false;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = uuid::Uuid::new_v4().simple().to_string();
    confirms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), tx);
    let _ = app.emit_to(
        "widget",
        "bot-confirm",
        serde_json::json!({ "id": id, "tool": tool, "detail": detail }),
    );
    audit_log(
        app,
        &format!("confirm | id: {} | {tool} | {detail}", &id[..8]),
    );
    match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
        Ok(Ok(approved)) => approved,
        _ => {
            confirms()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            crate::bot_skills::skill_confirm_result(app, false); // 超时默认拒绝
            false
        }
    }
}

/// 挂件确认响应：允许/拒绝（前端点击后回传）
#[tauri::command]
pub fn bot_confirm_response(app: AppHandle, request_id: String, approved: bool) {
    // Skill 调度器联动：确认结果 → 恢复 Running / 拒绝终止 / 暂停即终止
    crate::bot_skills::skill_confirm_result(&app, approved);
    if let Some(tx) = confirms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&request_id)
    {
        let _ = tx.send(approved);
    }
}

// ───────────────────────── 开关持久化 ─────────────────────────

fn bot_flag_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("bot-enabled.flag")
}

/// 机器人聊天当前是否开启
#[tauri::command]
pub fn bot_get_enabled(app: AppHandle) -> bool {
    bot_flag_path(&app).exists()
}

/// 设置机器人聊天开关（写/删 flag，返回生效后的状态）
#[tauri::command]
pub fn bot_set_enabled(app: AppHandle, enabled: bool) -> CommandResult<bool> {
    let dir = crate::db::data_dir(&app);
    std::fs::create_dir_all(&dir)?;
    if enabled {
        std::fs::write(bot_flag_path(&app), b"1")?;
    } else {
        let _ = std::fs::remove_file(bot_flag_path(&app));
    }
    Ok(enabled)
}

// ───────────────────────── API 配置 ─────────────────────────

/// 凭据存储条目：macOS 钥匙串 / Windows 凭据管理器
const KEYRING_SERVICE: &str = "wmessage-bot";
const KEYRING_USER: &str = "api-key";

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct BotConfig {
    /// OpenAI 兼容接口地址，如 https://api.deepseek.com/v1
    pub base_url: String,
    pub model: String,
    /// 仅用于旧版本迁移：老 bot-config.json 里的明文 key，读出迁入凭据存储后置 None 写回
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// F-1 [P0 release blocker] pre-step 命中 Skill 时是否跳过外层主 LLM。
    /// true = 新行为（auto 模式 bypass LLM，interactive 模式仍走 LLM 但 Skill body 注入 system prompt）；
    /// false = LEGACY 旧链路（强制 pre_routed_skill = None，让 LLM 自由选 Skill）。
    /// 默认 true，老 bot-config.json 自动兼容（struct 级 #[serde(default)] + Default::default()）。
    pub bypass_llm_on_pre_step_hit: bool,
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-chat".into(),
            api_key: None,
            bypass_llm_on_pre_step_hit: true, // 默认开启新行为
        }
    }
}

fn config_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("bot-config.json")
}

/// F-1 开关读取 helper：bot-config.json 缺字段 / 文件不存在 / 解析失败都默认 true（新行为）。
/// 比 bot_get_config 轻量：跳过 BotConfigView 构造 + key 校验，bot_chat 入口用。
fn read_bypass_llm_switch(app: &AppHandle) -> bool {
    let p = config_path(app);
    if !p.exists() {
        return true;
    }
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return true;
    };
    serde_json::from_str::<BotConfig>(&raw)
        .map(|c| c.bypass_llm_on_pre_step_hit)
        .unwrap_or(true)
}

fn key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("系统凭据存储不可用：{e}"))
}

fn read_api_key() -> Result<String, String> {
    key_entry()?
        .get_password()
        .map_err(|e| format!("读取 API Key 失败：{e}"))
}

fn has_api_key() -> bool {
    match key_entry() {
        Ok(e) => e.get_password().is_ok(),
        Err(_) => false,
    }
}

fn write_api_key(key: &str) -> Result<(), String> {
    key_entry()?
        .set_password(key)
        .map_err(|e| format!("保存 API Key 失败：{e}"))
}

/// 返回给前端的配置视图：不含 key 本体，只有 hasApiKey 标志
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotConfigView {
    pub base_url: String,
    pub model: String,
    pub has_api_key: bool,
    /// F-1 [P0 release blocker] pre-step 命中 Skill 时是否跳过外层主 LLM。
    /// 前端设置页 Toggle 直接透传到 bot-config.json。
    pub bypass_llm_on_pre_step_hit: bool,
}

/// 旧版本迁移：bot-config.json 里有明文 key → 迁入系统凭据存储并清掉文件里的明文。
/// App 启动时调用一次（设置页读配置时也会兜底触发）。
pub fn migrate_legacy_key(app: &AppHandle) -> Result<(), String> {
    let p = config_path(app);
    if !p.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut cfg: BotConfig = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let Some(k) = cfg.api_key.take() else {
        return Ok(());
    };
    let k = k.trim().to_string();
    if k.is_empty() {
        return Ok(());
    }
    // 凭据存储里没有 key 时才写入（避免旧明文覆盖用户新存的 key）
    if !has_api_key() && write_api_key(&k).is_err() {
        return Ok(()); // 写入失败：保留文件明文，下次再试
    }
    let dir = crate::db::data_dir(app);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let raw = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&p, raw).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn bot_get_config(app: AppHandle) -> CommandResult<BotConfigView> {
    let _ = migrate_legacy_key(&app); // 兜底：设置页读配置时也确保无明文残留
    let p = config_path(&app);
    let cfg: BotConfig = if p.exists() {
        let raw = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())?
    } else {
        BotConfig::default()
    };

    let has_api_key = has_api_key();
    Ok(BotConfigView {
        base_url: cfg.base_url,
        model: cfg.model,
        has_api_key,
        bypass_llm_on_pre_step_hit: cfg.bypass_llm_on_pre_step_hit,
    })
}

/// 保存配置。api_key：Some(非空) 写入凭据存储并覆盖；None/空串不动已存的 key。
#[tauri::command]
pub fn bot_set_config(
    app: AppHandle,
    config: BotConfig,
    api_key: Option<String>,
) -> CommandResult<()> {
    if let Some(k) = api_key {
        let k = k.trim();
        if !k.is_empty() {
            write_api_key(k)?;
        }
    }
    // 文件里只留非敏感配置（api_key 字段忽略）
    let mut cfg = config;
    cfg.api_key = None;
    let dir = crate::db::data_dir(&app);
    std::fs::create_dir_all(&dir).map_err(|e| CommandError::IoError(e.to_string()))?;
    let raw =
        serde_json::to_string_pretty(&cfg).map_err(|e| CommandError::IoError(e.to_string()))?;
    std::fs::write(config_path(&app), raw).map_err(|e| CommandError::IoError(e.to_string()))
}

/// 清除已保存的 API Key
#[tauri::command]
pub fn bot_clear_api_key() -> CommandResult<()> {
    key_entry()?
        .delete_credential()
        .map_err(|e| CommandError::KeyringError(format!("清除 API Key 失败：{e}")))
}

// ───────────────────────── 审计日志 ─────────────────────────

/// 追加机器人审计日志：用户指令、工具名、入参、结果全部留痕（数据目录 bot.log）
/// 审计日志外部钩子（bot_skills 调度器用；bot.rs 内部仍用 audit_log）
pub fn audit_log_hook(app: &AppHandle, line: &str) {
    audit_log(app, line);
}

fn audit_log(app: &AppHandle, line: &str) {
    crate::db::rotate_log_if_large(&crate::db::data_dir(app).join("bot.log"), 5 * 1024 * 1024);
    let p = crate::db::data_dir(app).join("bot.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {line}");
    }
}

/// 审计日志安全截断：超长文本截到 max 字符加省略号（按字符数）
fn truncate_for_log(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 读取机器人审计日志（倒序，最新在前；默认 200 行，上限 2000）
#[tauri::command]
pub fn bot_log_read(app: AppHandle, limit: Option<usize>) -> String {
    let p = crate::db::data_dir(&app).join("bot.log");
    let Ok(raw) = std::fs::read_to_string(p) else {
        return "（暂无日志）".into();
    };
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }
    lines.reverse();
    lines.join("\n")
}

// ───────────────────────── 参数上限（防幻觉/防刷爆） ─────────────────────────

const MAX_TITLE: usize = 200;
const MAX_NOTE: usize = 5000;
const MAX_KEYWORD: usize = 100;
const MAX_SUBTASK_TEXT: usize = 200;
const MAX_DUE: usize = 30;
const MAX_TAGS: usize = 10;
const MAX_TAG_LEN: usize = 30;

/// 校验字符串长度上限，超限返回错误文案
fn check_len(value: &str, max: usize, what: &str) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{what}过长（上限 {max} 字）"));
    }
    Ok(())
}

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

const TOOLS: &str = r#"[
  {"type":"function","function":{"name":"list_tasks","description":"列出未完成任务（含状态列）","parameters":{"type":"object","properties":{}}}},
  {"type":"function","function":{"name":"query_single_task","description":"按 id 查询单张任务卡完整详情（标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除状态指示；白名单单点，区别于 list_tasks 批量清单与 search_tasks 关键词检索）","parameters":{"type":"object","properties":{"id":{"type":"string","description":"任务卡 UUID"}},"required":["id"]}}},
  {"type":"function","function":{"name":"create_task","description":"新建任务","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"任务标题"},
    "note":{"type":"string","description":"备注，可选"},
    "due":{"type":"string","description":"截止时间，YYYY-MM-DD 或 YYYY-MM-DD HH:mm，可选"},
    "column":{"type":"string","enum":["todo","doing"],"description":"状态列，默认 todo"}
  },"required":["title"]}}},
  {"type":"function","function":{"name":"complete_task","description":"完成任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id（来自用户消息的 [已选任务] 引用块或 list_tasks 输出），可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}},
  {"type":"function","function":{"name":"delete_task","description":"删除任务到回收站（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"}
  },"required":[]}}},
  {"type":"function","function":{"name":"edit_task","description":"编辑任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配；改标题/备注/截止时间/标签/状态列，空串清字段）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时用于定位任务"},
    "newTitle":{"type":"string","description":"新标题，可选"},
    "note":{"type":"string","description":"新备注，可选；空串清除"},
    "due":{"type":"string","description":"新截止时间，可选；空串清除"},
    "column":{"type":"string","enum":["todo","doing","done"],"description":"新状态列，可选"},
    "tags":{"type":"array","items":{"type":"string"},"description":"新标签列表，可选；空数组清除"}
  },"required":[]}}},
  {"type":"function","function":{"name":"add_subtask","description":"给任务添加子任务（taskId 精确匹配优先；无 taskId 时按标题关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容"}
  },"required":["text"]}}},
  {"type":"function","function":{"name":"toggle_subtask","description":"勾选/取消勾选子任务（任务用 taskId 优先；子任务按内容关键词匹配）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "text":{"type":"string","description":"子任务内容关键词"}
  },"required":["text"]}}},
  {"type":"function","function":{"name":"bind_file","description":"给任务绑定文件或文件夹（弹系统选择框由用户挑选；任务用 taskId 优先定位）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "isDir":{"type":"boolean","description":"true=选文件夹，false=选文件"}
  },"required":[]}}},
  {"type":"function","function":{"name":"link_file_to_task","description":"把 AI_Gen_Files 目录内的生成文件绑定到任务卡（不弹选择框；只允许该目录内的文件，其他文件请在任务卡上手动绑定）","parameters":{"type":"object","properties":{
    "taskId":{"type":"string","description":"任务 id，可选，优先于 title"},
    "title":{"type":"string","description":"任务标题关键词，无 taskId 时使用"},
    "path":{"type":"string","description":"要绑定的文件绝对路径（必须位于 AI_Gen_Files 目录内）"}
  },"required":["path"]}}},
  {"type":"function","function":{"name":"search_tasks","description":"按关键词搜索所有任务卡（待办/进行中/已完成/已归档；匹配标题/备注/标签/子任务）","parameters":{"type":"object","properties":{
    "query":{"type":"string","description":"搜索关键词"}
  },"required":["query"]}}},
  {"type":"function","function":{"name":"extract_document","description":"提取文档内容（不传 path 时弹系统选择框由用户选 Word/Excel/PPT/PDF；传 path 时直接读取该文件，如任务卡的绑定文件）","parameters":{"type":"object","properties":{
    "path":{"type":"string","description":"文件绝对路径，可选"}
  }}}},
  {"type":"function","function":{"name":"create_word","description":"生成 Word 文档到 AI_Gen_Files（润色后的文本用这个落地；不覆盖任何已有文件）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表，每段一个字符串"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}},
  {"type":"function","function":{"name":"create_word_revisions","description":"生成带修订标记（修订模式）的 Word 到 AI_Gen_Files：自动对比原文与润色后的段落，删除内容标删除线、新增内容标红色下划线，可在 Word 审阅中逐条接受/拒绝","parameters":{"type":"object","properties":{
    "originalPath":{"type":"string","description":"原文 Word 路径（extract_document 返回的 [文档路径]）"},
    "original":{"type":"array","items":{"type":"string"},"description":"原文行列表（提取被截断时必须传，保证对比范围一致），可选"},
    "revised":{"type":"array","items":{"type":"string"},"description":"润色后的段落列表"},
    "title":{"type":"string","description":"文档标题，可选"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["revised"]}}},
  {"type":"function","function":{"name":"create_excel","description":"生成 Excel 到 AI_Gen_Files（单元格以 = 开头会写入原生公式如 =SUM(A1:A10)）","parameters":{"type":"object","properties":{
    "sheets":{"type":"array","items":{"type":"object","properties":{
      "name":{"type":"string"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}}}}},
    "description":"工作表列表：name 表名、rows 二维数组"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["sheets"]}}},
  {"type":"function","function":{"name":"create_ppt","description":"生成专业排版 PPT 到 AI_Gen_Files（多版式：封面/目录/章节页/内容页/表格页/结束页 + 三套配色主题）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"演示文稿主标题"},
    "theme":{"type":"string","enum":["blue","navy","teal","forest","wine","sky","plum","coral","dark","green"],"description":"配色主题（按场合选）：blue 商务与权威（默认，汇报/金融）/ navy 科技与夜景（深色发布会）/ teal 现代与健康（医疗/护肤）/ forest 自然与户外（环保/农业）/ wine 复古与学院（学术/历史）/ sky 纯净科技蓝（AI/云计算）/ plum 轻奢与神秘（珠宝/高端咨询）/ coral 海岸珊瑚（旅游/夏日）/ dark 深色通用 / green 清新绿"},
    "slides":{"type":"array","description":"幻灯片列表，按展示顺序；每页一个 type","items":{"type":"object","properties":{
      "type":{"type":"string","enum":["cover","toc","section","content","table","closing"],"description":"页面类型：cover 封面（title+subtitle）/ toc 目录（items 列表）/ section 章节分隔页 / content 内容要点页 / table 表格页（rows 二维数组首行表头）/ closing 结束页"},
      "title":{"type":"string","description":"页面标题"},
      "subtitle":{"type":"string","description":"副标题（cover/section/closing 用）"},
      "bullets":{"type":"array","items":{"type":"string"},"description":"要点列表（content 页；≤5 条大字号，6-8 条中号，8 条以上自动双栏）"},
      "items":{"type":"array","items":{"type":"string"},"description":"目录条目（toc 页）"},
      "rows":{"type":"array","items":{"type":"array","items":{"type":"string"}},"description":"表格数据（table 页；第一行是表头）"}
    },"required":["type","title"]}},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["slides"]}}},
  {"type":"function","function":{"name":"create_pdf","description":"生成 PDF 到 AI_Gen_Files（中文支持）","parameters":{"type":"object","properties":{
    "title":{"type":"string","description":"文档标题，可选"},
    "paragraphs":{"type":"array","items":{"type":"string"},"description":"正文段落列表"},
    "filename":{"type":"string","description":"文件名（不含扩展名），可选"}
  },"required":["paragraphs"]}}},
  {"type":"function","function":{"name":"run_python","description":"执行 Python 代码（本机沙箱：独立临时目录 + 超时 60s；需用户在设置页开启 Python 编程）","parameters":{"type":"object","properties":{
    "code":{"type":"string","description":"要执行的 Python 代码，print 输出返回给用户"}
  },"required":["code"]}}},
  {"type":"function","function":{"name":"web_search","description":"搜索互联网获取最新信息（Bing+百度双引擎，返回标题/链接/摘要）","parameters":{"type":"object","properties":{"query":{"type":"string","description":"搜索关键词"}},"required":["query"]}}},
  {"type":"function","function":{"name":"fetch_url","description":"抓取网页正文（仅 http/https 公网地址；返回纯文本，用于读链接/总结网页内容）","parameters":{"type":"object","properties":{
    "url":{"type":"string","description":"要抓取的网页地址"}
  },"required":["url"]}}},
  {"type":"function","function":{"name":"use_skill","description":"读取已安装技能（skill）的完整文档并按文档步骤执行。任务涉及的每个相关技能都要读（可多次调用）：例如做 PPT 时，若清单里同时有编排、生成、配色、风格类技能，应逐个读取、取长补短综合运用，不要只读一个","parameters":{"type":"object","properties":{
    "name":{"type":"string","description":"技能名（系统提示词「已安装技能」清单里的名称，一次一个，可多次调用）"}
  },"required":["name"]}}}
]"#;

/// <think> 标签拆分：喂入流式文本，返回 (正文, 思考)。标签跨流式块时用 think_buf 缓冲。
fn feed_think(in_think: &mut bool, buf: &mut String, text: &str) -> (String, String) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    buf.push_str(text);
    let mut normal = String::new();
    let mut think = String::new();
    loop {
        if !*in_think {
            if let Some(pos) = buf.find(OPEN) {
                normal.push_str(&buf[..pos]);
                buf.drain(..pos + OPEN.len());
                *in_think = true;
            } else {
                // 结尾可能是不完整的 <think> 前缀，留着等下一块
                let keep = tail_prefix_len(buf, OPEN);
                if keep > 0 {
                    let cut = buf.len() - keep; // keep 为 ASCII 前缀，字节数=字符数
                    normal.push_str(&buf[..cut]);
                    buf.drain(..cut);
                } else {
                    normal.push_str(buf);
                    buf.clear();
                }
                break;
            }
        } else if let Some(pos) = buf.find(CLOSE) {
            think.push_str(&buf[..pos]);
            buf.drain(..pos + CLOSE.len());
            *in_think = false;
        } else {
            let keep = tail_prefix_len(buf, CLOSE);
            if keep > 0 {
                let cut = buf.len() - keep;
                think.push_str(&buf[..cut]);
                buf.drain(..cut);
            } else {
                think.push_str(buf);
                buf.clear();
            }
            break;
        }
    }
    (normal, think)
}

/// s 结尾与 tag 开头重合的长度（如 s 尾是 "<thi"、tag "<think>" → 4）。匹配部分必是 ASCII，字节数=字符数。
fn tail_prefix_len(s: &str, tag: &str) -> usize {
    let mut k = tag.len().min(s.len());
    while k > 0 {
        if s.is_char_boundary(s.len() - k) && tag.starts_with(&s[s.len() - k..]) {
            break;
        }
        k -= 1;
    }
    k
}

// ───────────────────────── 图片附件（多模态） ─────────────────────────

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];
/// 单张图片文件上限 3MB（base64 后约 4MB，MiniMax 图片大小限制内）
const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;
const MAX_IMAGES_PER_MSG: usize = 4;

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

#[cfg(test)]
mod think_tests {
    use super::*;

    fn run(chunks: &[&str]) -> (String, String) {
        let mut mode = false;
        let mut buf = String::new();
        let mut normal = String::new();
        let mut think = String::new();
        for c in chunks {
            let (n, t) = feed_think(&mut mode, &mut buf, c);
            normal.push_str(&n);
            think.push_str(&t);
        }
        (normal, think)
    }

    #[test]
    fn think_split_across_chunks() {
        // 标签和内容都跨块
        let (n, t) = run(&["<thi", "nk>思考中…", "</th", "ink>答案是 42"]);
        assert_eq!(n, "答案是 42");
        assert_eq!(t, "思考中…");
    }

    #[test]
    fn think_whole_in_one_chunk() {
        let (n, t) = run(&["<think>先想一下</think>好的"]);
        assert_eq!(n, "好的");
        assert_eq!(t, "先想一下");
    }

    #[test]
    fn plain_text_no_tags() {
        let (n, t) = run(&["直接回答，没有思考"]);
        assert_eq!(n, "直接回答，没有思考");
        assert_eq!(t, "");
    }

    #[test]
    fn multiple_think_blocks() {
        let (n, t) = run(&["<think>A</think>正文1<think>B</think>正文2"]);
        assert_eq!(n, "正文1正文2");
        assert_eq!(t, "AB");
    }
}

#[cfg(test)]
mod tools_schema_tests {
    use super::*;

    /// TOOLS 是编译期字符串、运行期解析：语法坏会 panic 杀死聊天（历史 bug）。
    /// 此测试守住：加/改工具后必须合法且字段完整。
    #[test]
    fn tools_schema_parses() {
        let v: serde_json::Value = serde_json::from_str(TOOLS).expect("TOOLS 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        assert!(!arr.is_empty(), "TOOLS 不能为空");
        for t in arr {
            assert_eq!(
                t["type"].as_str(),
                Some("function"),
                "每项 type 必须是 function"
            );
            let name = t["function"]["name"]
                .as_str()
                .expect("每项必须有 function.name");
            assert!(!name.is_empty(), "工具名不能为空");
            assert!(
                t["function"]["description"].as_str().is_some(),
                "{name} 缺 description"
            );
        }
        // 关键工具必须存在（与 execute_tool match 对齐，改名会在此暴露）
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        for required in [
            "list_tasks",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "run_python",
            "web_search",
            "fetch_url",
        ] {
            assert!(names.contains(&required), "缺少工具 {required}");
        }
    }
}

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
mod sched_tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
    }

    #[test]
    fn daily_occurrence() {
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(
            occurrence_after("daily:10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        );
        assert_eq!(
            occurrence_after("daily:09:00", after),
            Some(dt(2026, 8, 17, 9, 0))
        ); // 已过 → 明天
    }

    #[test]
    fn weekly_occurrence() {
        // 2026-08-16 是周日（weekday=7）
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(
            occurrence_after("weekly:1:09:00", after),
            Some(dt(2026, 8, 17, 9, 0))
        ); // 本周一已过 → 下周一
        assert_eq!(
            occurrence_after("weekly:7:10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        ); // 今天周日 10 点未到
        assert_eq!(
            occurrence_after("weekly:7:08:00", after),
            Some(dt(2026, 8, 23, 8, 0))
        ); // 已过 → 下周日
    }

    #[test]
    fn at_occurrence() {
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(
            occurrence_after("at:2026-08-16T10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        );
        assert_eq!(occurrence_after("at:2026-08-16T08:00", after), None); // 已过，一次性不再触发
    }

    #[test]
    fn monthly_occurrence() {
        let after = dt(2026, 8, 16, 9, 0);
        // 本月 16 日 10:00 未到 → 今天
        assert_eq!(
            occurrence_after("monthly:16:10:00", after),
            Some(dt(2026, 8, 16, 10, 0))
        );
        // 本月 15 日已过 → 下月 15 日
        assert_eq!(
            occurrence_after("monthly:15:10:00", after),
            Some(dt(2026, 9, 15, 10, 0))
        );
        // 2 月无 31 日 → 顺延到 3 月 31 日（after=2026-01-20）
        let jan = dt(2026, 1, 20, 9, 0);
        assert_eq!(
            occurrence_after("monthly:31:08:00", jan),
            Some(dt(2026, 1, 31, 8, 0))
        );
        let feb = dt(2026, 2, 1, 9, 0);
        assert_eq!(
            occurrence_after("monthly:31:08:00", feb),
            Some(dt(2026, 3, 31, 8, 0))
        );
    }

    #[test]
    fn at_expired_detection() {
        let now = dt(2026, 8, 16, 10, 0);
        // 从未执行且已过期 → 放弃
        assert!(at_expired("at:2026-08-16T09:00", None, now));
        // 未来 → 不放弃
        assert!(!at_expired("at:2026-08-16T11:00", None, now));
        // 执行过（sched_last 有值）→ 交给 occurrence_after 判断，不在此放弃
        assert!(!at_expired("at:2026-08-16T09:00", Some(1), now));
        // 非 at: 格式 → 不适用
        assert!(!at_expired("daily:09:00", None, now));
        // 坏数据 → 放弃
        assert!(at_expired("at:junk", None, now));
    }

    #[test]
    fn invalid_schedule() {
        let after = dt(2026, 8, 16, 9, 0);
        assert_eq!(occurrence_after("daily:25:00", after), None);
        assert_eq!(occurrence_after("weekly:8:09:00", after), None);
        assert_eq!(occurrence_after("", after), None);
        assert_eq!(occurrence_after("junk", after), None);
        assert_eq!(occurrence_after("at:not-a-time", after), None);
        assert_eq!(occurrence_after("monthly:0:09:00", after), None);
        assert_eq!(occurrence_after("monthly:32:09:00", after), None);
        assert_eq!(occurrence_after("monthly:5:25:00", after), None);
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
    let bypass_llm_on_pre_step_hit = read_bypass_llm_switch(&app);
    // 审计：记录本轮用户最新指令（截断防刷日志）
    if let Some(last) = messages.last() {
        audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "user.message",
            "content" => truncate_for_log(&last.content, 300),
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
                        audit_event!(
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
                        audit_event!(
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
            audit_event!(
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
    // 最近两条 user 消息的图片附件转多模态消息（追问时上一张图还能看到；更早的历史保持纯文本省 token）
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
    let (text, refs) = run_model_loop(app, msgs, 8, &stop).await?;
    Ok(BotChatResult {
        text,
        task_refs: refs,
    })
}

// ────────────────────────────────────────────────────────────────────
// SSE chunk 解析（F-6 step 4 refactor 2026-08-18 10:31）
//
// 动机：消除 tests/llm_integration.rs 与 run_model_loop 的解析逻辑重复。
// 提取后：测试调 wmessage_lib::bot::parse_sse_chunk，生产代码同样调之，
//        OpenAI SSE 格式演化只改这一处。
// ────────────────────────────────────────────────────────────────────

/// 一次 SSE chunk 解析结果（content / tool_calls / finish_reason / [DONE]）
#[derive(Debug, Default, Clone)]
pub struct ParsedChunk {
    /// delta.content（仅在非空字符串时 Some，与原代码 `!t.is_empty()` 语义一致）
    pub content: Option<String>,
    /// delta.tool_calls 增量（多 chunk 拼成一个完整 tool_call）
    pub tool_calls: Vec<ToolCallDelta>,
    /// choices[0].finish_reason（最后一 chunk 通常为 "stop" / "tool_calls"）
    pub finish_reason: Option<String>,
    /// `data: [DONE]` 标记
    pub is_done: bool,
}

/// 单个 tool_call 增量字段
#[derive(Debug, Default, Clone)]
pub struct ToolCallDelta {
    /// tool_calls[*].index（默认 0）
    pub index: usize,
    /// tool_calls[*].id（Some = 原 JSON 含此字段，值可能为空串）
    pub id: Option<String>,
    /// tool_calls[*].function.name 追加块（多 chunk 拼接）
    pub name_chunk: Option<String>,
    /// tool_calls[*].function.arguments 追加块（多 chunk 拼接成完整 JSON）
    pub arguments_chunk: Option<String>,
}

/// 解析一行 OpenAI 兼容 SSE（`data: <json>` 或 `data: [DONE]`）。
/// 返回 None = 非 data 行 / JSON 解析失败 / choices 为空（与原代码 `else continue` 语义一致）。
///
/// 注意：纯函数，不产生任何 side effect（无 widget emit、无 think-block 处理、无 final_text push）。
/// 调用方（run_model_loop）负责 feed_think + emit + 累积。
pub fn parse_sse_chunk(line: &str) -> Option<ParsedChunk> {
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(ParsedChunk {
            is_done: true,
            ..Default::default()
        });
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let delta = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .map(|c| &c["delta"])?;

    let mut chunk = ParsedChunk::default();

    if let Some(t) = delta["content"].as_str() {
        if !t.is_empty() {
            chunk.content = Some(t.to_string());
        }
    }

    if let Some(tcs) = delta["tool_calls"].as_array() {
        for tc in tcs {
            let idx = tc["index"].as_u64().unwrap_or(0) as usize;
            let mut delta_tc = ToolCallDelta {
                index: idx,
                ..Default::default()
            };
            if let Some(id) = tc["id"].as_str() {
                delta_tc.id = Some(id.to_string());
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                delta_tc.name_chunk = Some(name.to_string());
            }
            if let Some(args) = tc["function"]["arguments"].as_str() {
                delta_tc.arguments_chunk = Some(args.to_string());
            }
            chunk.tool_calls.push(delta_tc);
        }
    }

    if let Some(reason) = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c["finish_reason"].as_str())
    {
        chunk.finish_reason = Some(reason.to_string());
    }

    Some(chunk)
}

#[cfg(test)]
mod parse_sse_chunk_tests {
    use super::*;

    #[test]
    fn parses_text_content_chunk() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(!parsed.is_done);
        assert_eq!(parsed.content.as_deref(), Some("hello"));
        assert!(parsed.tool_calls.is_empty());
    }

    #[test]
    fn parses_done_marker() {
        let parsed = parse_sse_chunk("data: [DONE]").unwrap();
        assert!(parsed.is_done);
        assert!(parsed.content.is_none());
        assert!(parsed.tool_calls.is_empty());
        assert!(parsed.finish_reason.is_none());
    }

    #[test]
    fn parses_tool_call_delta_with_index() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"list_tasks","arguments":"{}"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        let tc = &parsed.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        assert_eq!(tc.name_chunk.as_deref(), Some("list_tasks"));
        assert_eq!(tc.arguments_chunk.as_deref(), Some("{}"));
    }

    #[test]
    fn parses_multiple_tool_calls_with_distinct_indices() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"a"}},{"index":1,"function":{"name":"b"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].name_chunk.as_deref(), Some("a"));
        assert_eq!(parsed.tool_calls[1].name_chunk.as_deref(), Some("b"));
    }

    #[test]
    fn parses_finish_reason_stop() {
        let line = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parses_finish_reason_tool_calls() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"y","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("tool_calls"));
        assert!(!parsed.tool_calls.is_empty());
    }

    #[test]
    fn empty_content_string_treated_as_absent() {
        // SSE 中 content="" 时应等同 None（不触发内容推送）
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(
            parsed.content.is_none(),
            "空 content 应等同 None（不触发推送）"
        );
    }

    #[test]
    fn empty_name_treated_as_append_noop() {
        // name="" 时原代码 `!name.is_empty()` 跳过 push；这里 chunk 仍 Some("") 但 caller 决定是否 append
        let line =
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":""}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(
            parsed.tool_calls[0].name_chunk.as_deref(),
            Some(""),
            "name 字段存在但为空 — caller 决定是否跳过 append"
        );
    }

    #[test]
    fn returns_none_for_non_data_line() {
        assert!(parse_sse_chunk("event: ping").is_none());
        assert!(parse_sse_chunk("").is_none());
        assert!(parse_sse_chunk("data: not-json{").is_none());
        assert!(parse_sse_chunk("data:").is_none());
        assert!(parse_sse_chunk("data:    ").is_none());
    }

    #[test]
    fn returns_none_for_empty_choices() {
        // 网关异常：200 OK + choices:[] → 跳过（与原代码 else continue 语义一致）
        let line = r#"data: {"choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    #[test]
    fn returns_none_for_missing_choices() {
        let line = r#"data: {"id":"x","error":"auth_failed"}"#;
        assert!(
            parse_sse_chunk(line).is_none(),
            "异常响应（error 字段）应被忽略"
        );
    }

    #[test]
    fn parses_chinese_content_utf8() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好世界"}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.content.as_deref(), Some("你好世界"));
    }

    #[test]
    fn parses_arguments_split_across_chunks() {
        // 模拟 arguments 跨多个 SSE chunk（流式追加）
        let chunk1 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"ti"}}]}}]}"#;
        let chunk2 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"tle\":\"x\""}}]}}]}"#;
        let p1 = parse_sse_chunk(chunk1).unwrap();
        let p2 = parse_sse_chunk(chunk2).unwrap();
        assert_eq!(p1.tool_calls[0].arguments_chunk.as_deref(), Some(r#"{"ti"#));
        assert_eq!(
            p2.tool_calls[0].arguments_chunk.as_deref(),
            Some(r#"tle":"x""#)
        );
        // 生产代码会按顺序 push 拼成完整 JSON
    }

    #[test]
    fn done_marker_data_is_correctly_parsed() {
        // data:    [DONE]（中间多空格）也应正确识别
        let parsed = parse_sse_chunk("data:    [DONE]").unwrap();
        assert!(parsed.is_done);
    }

    #[test]
    fn defaults_index_to_zero_when_missing() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"a"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls[0].index, 0, "缺 index 时默认 0");
    }
}

/// 模型工具循环核心：配置/Key 检查、流式请求（思考拆分 + 工具折叠事件）、进程内执行工具。
/// msgs 需已含 system 消息；返回 (最终正文, 任务引用)。聊天 8 轮、任务执行 10 轮。
async fn run_model_loop(
    app: AppHandle,
    msgs: Vec<serde_json::Value>,
    max_rounds: usize,
    stop: &StopGuard,
) -> CommandResult<(String, Vec<TaskRef>)> {
    let cfg = bot_get_config(app.clone())?;
    let api_key = read_api_key()?;
    if api_key.trim().is_empty() {
        return Err("机器人 API 未配置：请到设置页「机器人设置」填写 API Key".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("初始化 HTTP 客户端失败：{e}"))?;
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    let tools: serde_json::Value = serde_json::from_str(TOOLS).unwrap();

    let mut msgs = msgs;
    // 最多 max_rounds 轮（工具循环），每轮流式输出；收到 tool_calls 则执行后把结果续进对话
    let mut collected_refs: Vec<TaskRef> = Vec::new();
    // Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
    // max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
    //
    // 阈值设定理由（2026-08-18 老板拍板从 5 提到 10）：
    // - 5 太激进：实际 Skill 复合流程（例：minimax-archive = list + query + edit + bind_file + verify）就要 5+，
    //   复杂 Skill（PPT 编排 + 配色 + 归档）需 8-10
    // - 10 中间偏严：覆盖 90% 真实复合任务，留 1.5x 余量给多技能联动
    // - 15+ 太宽：掩护 LLM 死循环 / 幻觉调工具
    // - 软警告（7）收尾提醒：避免刚警告完就熔断
    const MAX_FUNCTION_CALLS_PER_TURN: usize = 10;
    const SOFT_WARN_AT: usize = 7;
    let mut function_calls_total: usize = 0;
    let mut soft_warn_sent: bool = false;
    // 上轮 streamed 文本快照（Block 2 接入，2026-08-17 22:26）：
    // AwaitConfirm/Finish/Fail/Terminate 跳出主循环时，返回 user 已看到的文本
    let mut last_streamed = String::new();
    for _round in 0..max_rounds {
        if stop.stopped() {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止");
            return Ok((format!("⏹ 已停止{hint}"), collected_refs));
        }
        // 状态机推进决策（Block 2 2026-08-17 22:26）：集中 Skill 推进逻辑
        // 未来横切关注点（审批/沙箱/上下文压缩）只动 advance_skill，主循环不重构
        if let Some(run) = crate::bot_skills::active_skill_run() {
            use crate::bot_skills::{advance_skill, AdvanceAction};
            match advance_skill(&run, chrono::Utc::now().timestamp_millis()) {
                AdvanceAction::NoActive | AdvanceAction::Continue => {} // 继续本轮
                AdvanceAction::AwaitConfirm => {
                    // Skill 暂停等用户确认，跳出主循环等待 bot_confirm_response 唤起
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Finish => {
                    let _hint = crate::bot_skills::skill_finish(&app, true, "");
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Fail(reason) => {
                    let _hint = crate::bot_skills::skill_finish(&app, false, &reason);
                    return Ok((last_streamed.clone(), collected_refs));
                }
                AdvanceAction::Terminate(reason) => {
                    let _hint = crate::bot_skills::skill_finish(&app, false, &reason);
                    return Ok((last_streamed.clone(), collected_refs));
                }
            }
        }
        let body = serde_json::json!({
            "model": cfg.model,
            "messages": msgs,
            "tools": tools,
            "stream": true
        });

        // LLM 请求前记录（F-3 第四步 2026-08-18）
        audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "llm.request",
            "model" => cfg.model.clone(),
            "msgs_count" => msgs.len(),
        );

        let resp = client
            .post(&url)
            .bearer_auth(api_key.trim())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("请求大模型失败：{e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            audit_event!(
                &app,
                crate::audit::AuditLevel::Warn,
                "llm.response",
                "status" => status.as_u16(),
            );
            let hint = crate::bot_skills::skill_finish(&app, false, "大模型 API 错误");
            return Err(CommandError::LlmApiError {
                status: status.as_u16(),
                body_preview: format!("{}{hint}", text.chars().take(300).collect::<String>()),
            });
        }
        audit_event!(
            &app,
            crate::audit::AuditLevel::Info,
            "llm.response",
            "status" => status.as_u16(),
        );

        let mut stream = resp.bytes_stream();
        let mut line_buf = String::new();
        let mut final_text = String::new();
        let mut tool_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments)
                                                                        // <think> 思考块拆分：思考走 bot-think-delta，正文走 bot-chat-delta
        let mut think_mode = false;
        let mut think_buf = String::new();

        let mut stopped = false;
        while let Some(chunk) = stream.next().await {
            if stop.stopped() {
                stopped = true;
                break;
            }
            let chunk = chunk.map_err(|e| format!("流式读取失败：{e}"))?;
            line_buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = line_buf.find('\n') {
                let line: String = line_buf.drain(..=nl).collect();
                let line = line.trim();
                if let Some(parsed) = parse_sse_chunk(&line) {
                    if parsed.is_done {
                        continue;
                    }
                    if let Some(t) = parsed.content {
                        if !t.is_empty() {
                            let (normal, think) = feed_think(&mut think_mode, &mut think_buf, &t);
                            if !think.is_empty() {
                                let _ = app.emit_to(
                                    "widget",
                                    "bot-think-delta",
                                    serde_json::json!({ "text": think }),
                                );
                            }
                            if !normal.is_empty() {
                                final_text.push_str(&normal);
                                let _ = app.emit_to(
                                    "widget",
                                    "bot-chat-delta",
                                    serde_json::json!({ "text": normal }),
                                );
                            }
                        }
                    }
                    for tc_delta in parsed.tool_calls {
                        while tool_calls.len() <= tc_delta.index {
                            tool_calls.push((String::new(), String::new(), String::new()));
                        }
                        let t = &mut tool_calls[tc_delta.index];
                        if let Some(id) = tc_delta.id {
                            if t.0.is_empty() {
                                t.0 = id;
                                // 新工具调用开始：推折叠行给挂件
                                let _ = app.emit_to(
                                    "widget",
                                    "bot-tool",
                                    serde_json::json!({ "id": t.0, "name": t.1 }),
                                );
                            }
                        }
                        if let Some(name) = tc_delta.name_chunk {
                            if !name.is_empty() {
                                t.1.push_str(&name);
                                if !t.0.is_empty() {
                                    let _ = app.emit_to(
                                        "widget",
                                        "bot-tool-name",
                                        serde_json::json!({ "id": t.0, "name": t.1 }),
                                    );
                                }
                            }
                        }
                        if let Some(args) = tc_delta.arguments_chunk {
                            t.2.push_str(&args);
                        }
                    }
                }
            }
        }

        // 回合结束：冲刷思考缓冲（丢弃未闭合标签碎片）
        let tail = std::mem::take(&mut think_buf)
            .replace("<think>", "")
            .replace("</think>", "");
        if !tail.is_empty() {
            if think_mode {
                let _ = app.emit_to(
                    "widget",
                    "bot-think-delta",
                    serde_json::json!({ "text": tail }),
                );
            } else {
                final_text.push_str(&tail);
                let _ = app.emit_to(
                    "widget",
                    "bot-chat-delta",
                    serde_json::json!({ "text": tail }),
                );
            }
        }

        if stopped {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }

        if tool_calls.is_empty() {
            let _ = crate::bot_skills::skill_finish(&app, true, "");
            collected_refs = merge_task_refs_dedup(collected_refs);
            return Ok((final_text.clone(), collected_refs));
        }

        // 模型请求工具：进程内执行，结果回填后继续下一轮
        msgs.push(serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": tool_calls.iter().map(|(id, name, args)| serde_json::json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": args}
            })).collect::<Vec<_>>()
        }));
        if stop.stopped() {
            let hint = crate::bot_skills::skill_finish(&app, false, "用户停止");
            return Ok((format!("{final_text}\n\n⏹ 已停止{hint}"), collected_refs));
        }
        for (id, name, args) in &tool_calls {
            function_calls_total += 1;
            if function_calls_total > MAX_FUNCTION_CALLS_PER_TURN {
                let hint = crate::bot_skills::skill_finish(&app, false, "单轮 Function 调用超上限");
                audit_log(
                    &app,
                    &format!(
                        "fuse | 单轮 Function 调用超过 {} 次，已熔断",
                        MAX_FUNCTION_CALLS_PER_TURN
                    ),
                );
                return Ok((
                    format!(
                        "{final_text}\n\n⏹ 已熔断：本轮 Function 调用超过 {} 次上限（安全保护），已停止后续执行{hint}",
                        MAX_FUNCTION_CALLS_PER_TURN
                    ),
                    collected_refs,
                ));
            }
            // 软警告（SOFT_WARN_AT）：追加 user 消息提示 LLM 收尾，不中断流程
            if !soft_warn_sent && function_calls_total >= SOFT_WARN_AT {
                msgs.push(serde_json::json!({
                    "role": "user",
                    "content": format!(
                        "【系统提示】你已连续调用 {SOFT_WARN_AT} 个工具，最多还能调 {} 个。请尽快收尾：合并调用、必要时汇总报告给用户、避免在剩余额度内继续展开新步骤。",
                        MAX_FUNCTION_CALLS_PER_TURN - SOFT_WARN_AT
                    ),
                }));
                soft_warn_sent = true;
                audit_log(
                    &app,
                    &format!(
                        "soft_warn | Function 调用达 {} 次（上限 {}），追加收尾提醒",
                        SOFT_WARN_AT, MAX_FUNCTION_CALLS_PER_TURN
                    ),
                );
            }
            let (result, refs) = execute_tool(&app, name, args).await;
            let _ = app.emit_to(
                "widget",
                "bot-tool-done",
                serde_json::json!({ "id": id, "name": name, "args": args }),
            );
            audit_log(
                &app,
                &format!(
                    "tool: {name} | args: {} | result: {}",
                    truncate_for_log(args, 500),
                    truncate_for_log(&result, 300)
                ),
            );
            collected_refs.extend(refs);
            msgs.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": id,
                "content": result
            }));
        }
        // 快照上轮 streamed 文本（供 AwaitConfirm/Finish/Fail/Terminate 跳出时返回）
        last_streamed = final_text.clone();
    }
    let hint = crate::bot_skills::skill_finish(&app, false, "对话轮数超限");
    Err(CommandError::Internal(format!("对话轮数超限{hint}")))
}

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
    let cfg = bot_get_config(app.clone())?;
    let api_key = read_api_key()?;
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

/// 任务卡交给机器人执行（🤖 按钮 / 选卡说「完成它」）：把任务卡内容组装成指令，高轮数工具循环执行。
/// 流式经 bot-chat-delta / bot-think-delta / bot-tool* 事件推给挂件。
#[tauri::command]
pub async fn bot_execute_task(app: AppHandle, task_id: String) -> CommandResult<BotChatResult> {
    execute_task_core(&app, &task_id, true).await
}

/// 任务卡执行核心（命令与定时调度共用）。interactive=true 表示用户直接触发（可被 /stop 停），
/// false 表示后台定时触发（/stop 不影响）
async fn execute_task_core(
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
    audit_log(
        &app,
        &format!(
            "execute_task | id: {} | title: {}",
            task.id,
            truncate_for_log(&task.title, 60)
        ),
    );
    let msgs = vec![
        serde_json::json!({"role": "system", "content": format!("{}\n\n{}", EXECUTE_SYSTEM_PROMPT, build_skill_block(app))}),
        serde_json::json!({"role": "user", "content": block}),
    ];
    // 交给机器人：卡片切机器人头像（前端 tasks-changed 广播后实时更新）
    set_bot_assigned(app, &task.id, true);
    let result = run_model_loop(app.clone(), msgs, 10, &stop).await;
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
        broadcast_after_mutation(app, vec![t], vec![]);
    }
}

// ───────────────────────── 定时任务卡（阶段二：⏰ 到点自动执行） ─────────────────────────

/// 正在执行的定时任务 id（防同一任务并发重复跑）
static SCHED_RUNNING: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

fn sched_running() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    SCHED_RUNNING.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// 调度防重入 RAII 守卫：Drop（含 panic 展开）时自动清理，保证任务 id 不残留
/// （二次审计 P1：原先清理在 run_scheduled 末尾，panic 时该卡永久失效）
struct SchedGuard(String);

impl SchedGuard {
    fn acquire(task_id: &str) -> Option<Self> {
        let mut set = sched_running().lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(task_id) {
            return None;
        }
        set.insert(task_id.to_string());
        Some(Self(task_id.to_string()))
    }
}

impl Drop for SchedGuard {
    fn drop(&mut self) {
        sched_running()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// 解析 "HH:MM"
fn parse_hm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.split_once(':')?;
    let h = h.parse::<u32>().ok()?;
    let m = m.parse::<u32>().ok()?;
    if h < 24 && m < 60 {
        Some((h, m))
    } else {
        None
    }
}

/// 定时格式：
/// - "daily:HH:MM" 每天
/// - "weekly:D:HH:MM" 每周（D=1..7，周一起）
/// - "at:YYYY-MM-DDTHH:MM" 一次性
/// 返回在 after 之后最近的触发时间；一次性已过或格式无效返回 None。
fn occurrence_after(
    schedule: &str,
    after: chrono::DateTime<chrono::Local>,
) -> Option<chrono::DateTime<chrono::Local>> {
    if let Some(t) = schedule.strip_prefix("daily:") {
        let (h, m) = parse_hm(t)?;
        let mut occ = after
            .date_naive()
            .and_hms_opt(h, m, 0)?
            .and_local_timezone(chrono::Local)
            .single()?;
        if occ <= after {
            occ += chrono::Duration::days(1);
        }
        return Some(occ);
    }
    if let Some(t) = schedule.strip_prefix("weekly:") {
        let (d, rest) = t.split_once(':')?;
        let dow = d.parse::<u32>().ok()?;
        if !(1..=7).contains(&dow) {
            return None;
        }
        let (h, m) = parse_hm(rest)?;
        let weekday = after.weekday().num_days_from_monday() as u32 + 1; // 1=周一
        let diff = (dow + 7 - weekday) % 7;
        let mut occ = (after.date_naive() + chrono::Duration::days(diff as i64))
            .and_hms_opt(h, m, 0)?
            .and_local_timezone(chrono::Local)
            .single()?;
        if occ <= after {
            occ += chrono::Duration::days(7);
        }
        return Some(occ);
    }
    if let Some(t) = schedule.strip_prefix("monthly:") {
        // monthly:DD:HH:MM —— 每月 DD 日 HH:MM（该月无 DD 日如 2 月 31 日则顺延）
        let (dd, rest) = t.split_once(':')?;
        let day = dd.parse::<u32>().ok()?;
        if !(1..=31).contains(&day) {
            return None;
        }
        let (h, m) = parse_hm(rest)?;
        let mut y = after.year();
        let mut mo = after.month();
        for _ in 0..12 {
            if let Some(d) = chrono::NaiveDate::from_ymd_opt(y, mo, day) {
                if let Some(occ) = d
                    .and_hms_opt(h, m, 0)
                    .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
                {
                    if occ > after {
                        return Some(occ);
                    }
                }
            }
            mo += 1;
            if mo > 12 {
                mo = 1;
                y += 1;
            }
        }
        return None;
    }
    if let Some(t) = schedule.strip_prefix("at:") {
        let occ = chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M")
            .ok()?
            .and_local_timezone(chrono::Local)
            .single()?;
        return if occ > after { Some(occ) } else { None };
    }
    None
}

/// at: 一次性任务是否已错过且从未执行（应放弃补执行，防重启后补跑过期任务，审计 P1）
fn at_expired(sched: &str, sched_last: Option<i64>, now: chrono::DateTime<chrono::Local>) -> bool {
    let Some(at) = sched.strip_prefix("at:") else {
        return false;
    };
    if sched_last.is_some() {
        return false; // 执行过的不在此判断（由 occurrence_after 的 after 基准处理）
    }
    match chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M")
        .ok()
        .and_then(|dt| dt.and_local_timezone(chrono::Local).single())
    {
        Some(occ) => occ <= now,
        None => true, // 解析失败按过期处理（放弃）
    }
}

/// sched_last（epoch ms）→ DateTime；None 视为远古（下一次触发立即生效）
fn sched_last_dt(ms: Option<i64>) -> chrono::DateTime<chrono::Local> {
    match ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(u) => u.with_timezone(&chrono::Local),
        // epoch 0 必然可解析，但防御风格下不用裸 unwrap（审计 P3）
        None => chrono::DateTime::from_timestamp_millis(0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
            .with_timezone(&chrono::Local),
    }
}

/// 找出到点的定时任务（未删、未归档、未完成，且 sched_last < 触发点 ≤ now）。
/// 顺带清理「错过的一次性任务」：at: 从未执行且时间已过 → 放弃并清掉 schedule
///（审计 P1：否则重启后 30s 内会补执行过期任务）
fn find_due_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    let now = chrono::Local::now();
    let all = crate::db::db_load(app.clone()).unwrap_or_default();
    let stale_ids: Vec<String> = all
        .iter()
        .filter(|t| {
            t.deleted_at.is_none()
                && t.archived != Some(true)
                && t.column != "done"
                && t.schedule
                    .as_deref()
                    .map(str::trim)
                    .map(|s| at_expired(s, t.sched_last, now))
                    .unwrap_or(false)
        })
        .map(|t| t.id.clone())
        .collect();
    let due: Vec<crate::db::Task> = all
        .into_iter()
        .filter_map(|t| {
            if t.deleted_at.is_some() || t.archived == Some(true) || t.column == "done" {
                return None;
            }
            let Some(sched) = t
                .schedule
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return None;
            };
            match occurrence_after(sched, sched_last_dt(t.sched_last)) {
                Some(occ) if occ <= now => Some(t),
                _ => None,
            }
        })
        .collect();
    // 清理过期一次性任务的 schedule（基于最新数据合并，只清目标字段）。
    // 单次加载处理全部 stale id（审计 P3：原先每个 id 各开一次库，O(n) 次 db_load）
    if !stale_ids.is_empty() {
        if let Ok(cur) = crate::db::db_load(app.clone()) {
            let fresh: Vec<crate::db::Task> = cur
                .into_iter()
                .filter(|t| stale_ids.contains(&t.id))
                .map(|mut t| {
                    t.schedule = None;
                    t.updated_at = Some(now.timestamp_millis());
                    t
                })
                .collect();
            if !fresh.is_empty() {
                let _ = crate::db::db_upsert(app.clone(), fresh);
            }
        }
    }
    due
}

/// 执行一张到点的定时任务卡：先记 sched_last（防重复触发），跑执行循环，结果落备注标记
async fn run_scheduled(app: AppHandle, task: crate::db::Task) {
    // 防重入守卫：作用域结束（含 panic 展开）自动清理
    let Some(_sched_guard) = SchedGuard::acquire(&task.id) else {
        return;
    };
    // 机器人开关关闭时不执行定时任务（二次审计 P2-3：开关只管 UI 不管后端）
    if !bot_get_enabled(app.clone()) {
        audit_log(
            &app,
            &format!("sched_skip | id: {} | 机器人开关未开启", task.id),
        );
        return;
    }
    let now = chrono::Local::now();
    audit_log(
        &app,
        &format!(
            "sched_run | id: {} | title: {} | schedule: {}",
            task.id,
            truncate_for_log(&task.title, 60),
            task.schedule.as_deref().unwrap_or("")
        ),
    );

    // 先记 sched_last：30s 扫描周期内不会重复触发。
    // 基于库中最新数据合并（勿用扫描快照：会覆盖用户在扫描后的编辑）。
    // 记录失败则放弃本次执行（审计 P3：否则下个 tick 会因 sched_last 未更新而重复触发）
    let mut marked = false;
    if let Ok(cur) = crate::db::db_load(app.clone()) {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task.id) {
            fresh.sched_last = Some(now.timestamp_millis());
            fresh.updated_at = Some(now.timestamp_millis());
            marked = crate::db::db_upsert(app.clone(), vec![fresh]).is_ok();
        }
    }
    if !marked {
        audit_log(
            &app,
            &format!(
                "sched_skip | id: {} | 记录 sched_last 失败，放弃本次执行",
                task.id
            ),
        );
        return;
    }

    let result = execute_task_core(&app, &task.id, false).await;
    let time_str = now.format("%m-%d %H:%M").to_string();

    // 执行结果写备注（模型可能已写摘要，这里前置 ⏰ 标记兜底）。
    // ⚠️ 必须基于执行后的最新数据合并：旧快照会把机器人执行期间的修改
    // （完成状态/摘要/子任务）整体回滚（审计 P0 已修复）
    let summary = match result {
        Ok(r) => format!(
            "⏰ 自动执行 {time_str}：{}",
            r.text.chars().take(300).collect::<String>()
        ),
        Err(e) => format!("⏰ 自动执行 {time_str} 失败：{e}"),
    };
    if let Ok(cur) = crate::db::db_load(app.clone()) {
        if let Some(mut fresh) = cur.into_iter().find(|t| t.id == task.id) {
            let note = match fresh.note.as_deref().filter(|n| !n.trim().is_empty()) {
                Some(n) => format!("{summary}\n{n}"),
                None => summary,
            };
            fresh.note = Some(note);
            // 一次性定时执行完清掉 schedule（⏰ 徽标消失）。
            // 用执行后的 fresh.schedule 判断（审计 P2：执行期间用户改过定时，扫描快照会误清新设置）
            if fresh
                .schedule
                .as_deref()
                .is_some_and(|s| s.starts_with("at:"))
            {
                fresh.schedule = None;
            }
            fresh.updated_at = Some(chrono::Local::now().timestamp_millis());
            let _ = crate::db::db_upsert(app.clone(), vec![fresh]);
        }
    }
    audit_log(&app, &format!("sched_done | id: {}", task.id));
}

/// 启动定时调度器：每 30s 扫一次到点任务卡并顺序执行（App 启动时调用）
pub fn start_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
        ticker.tick().await; // 消耗首个立即触发的 tick
        loop {
            ticker.tick().await;
            let due = find_due_tasks(&app);
            for t in due {
                // 每张卡 spawn 到独立任务再 await：单张卡 panic 只废这一张，
                // 不会杀死调度器主循环（否则后续所有定时任务静默失效，审计 P0）
                let handle = tauri::async_runtime::spawn(run_scheduled(app.clone(), t));
                let _ = handle.await;
            }
        }
    });
}

/// 进程内执行工具，返回 (给模型的文本结果, 涉及的任务引用)
/// `pub` 让 `bot_skills::run_skill_scheduler`（Phase 1 DSL 调度器）可调用，
/// 不暴露给前端 — 通过 `is_atomic_tool` 黑名单 + pre-execute 校验保护。
pub async fn execute_tool(app: &AppHandle, name: &str, args: &str) -> (String, Vec<TaskRef>) {
    let start = std::time::Instant::now();
    // 0. tool.call 结构化（F-3 第三步 2026-08-18）
    audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "tool.call",
        "tool" => name,
        "args_preview" => truncate_for_log(args, 80),
    );
    // 1. 后置拦截：原子黑名单（老板 2026-08-17 18:14 拍板）
    //    仅作为 Skill 内部子步骤、不允许裸调的底层原子 Function → 硬锁阻断
    //    只有 Skill 在 Running 状态时才放行；其他时候直接返回错误 + 提示走对应 Skill
    // F-2 抽象层：execute_tool 通过 middleware::run_pre_execute 调 pre-execute
    let active = crate::tool_guard::is_skill_active();
    if let Some(msg) = crate::middleware::run_pre_execute(app, name, active) {
        audit_event!(
            app,
            crate::audit::AuditLevel::Warn,
            "pre_execute.deny",
            "tool" => name,
        );
        return (msg, Vec::new());
    }
    // 2. Skill 调度器步骤钩子：活动技能时计数/熔断/动作记录（use_skill 自身跳过）
    if name != "use_skill" {
        if let Err(e) = crate::bot_skills::skill_on_step(app, name, args) {
            return (e, Vec::new());
        }
    }
    let (text, refs): (String, Vec<TaskRef>) = match name {
        "list_tasks" => tool_list_tasks(app),
        "query_single_task" => tool_query_single_task(app, args),
        "create_task" => tool_create_task(app, args),
        "complete_task" => tool_complete_task(app, args),
        "delete_task" => tool_delete_task(app, args).await,
        "edit_task" => tool_edit_task(app, args),
        "add_subtask" => tool_add_subtask(app, args),
        "toggle_subtask" => tool_toggle_subtask(app, args),
        "bind_file" => tool_bind_file(app, args).await,
        "link_file_to_task" => tool_link_file_to_task(app, args).await,
        "search_tasks" => tool_search_tasks(app, args),
        "extract_document" => tool_extract_document(app, args).await,
        "create_word" => tool_create_word(app, args).await,
        "create_word_revisions" => tool_create_word_revisions(app, args).await,
        "create_excel" => tool_create_excel(app, args).await,
        "create_ppt" => tool_create_ppt(app, args).await,
        "create_pdf" => tool_create_pdf(app, args).await,
        "run_python" => tool_run_python(app, args),
        "web_search" => tool_web_search(app, args).await,
        "fetch_url" => tool_fetch_url(app, args).await,
        "use_skill" => tool_use_skill(app, args),
        other => (format!("未知工具：{other}"), Vec::new()),
    };

    // 3. post-execute 洋葱管线「出」钩子（2026-08-17 22:17 第一块落地）
    //    - 结构化审计事件（工具名/耗时/返回引用数/结果预览）写到 bot.log
    //    - 失败分类：未知工具→Error；含「失败/错误/error:」→Warn；其他→Info
    //    - 镜像调用 skill_on_step_post：技能步骤结果/失败检测
    let dur_ms = start.elapsed().as_millis() as u64;
    let level = crate::audit::classify_text(name, &text);
    audit_event!(
        app,
        level,
        "tool.return",
        "tool" => name,
        "ms" => dur_ms,
        "refs" => refs.len(),
        "preview" => truncate_for_log(&text, 80),
    );
    if name != "use_skill" {
        crate::bot_skills::skill_on_step_post(app, name, &text, dur_ms, level);
    }

    (text, refs)
}

fn parse_args(args: &str) -> serde_json::Value {
    serde_json::from_str(args).unwrap_or(serde_json::Value::Null)
}

/// 改库后广播：挂件重读（tasks-changed）+ 主窗口合并 UI 不回写（tasks-updated, source:"bot"）
fn broadcast_after_mutation(app: &AppHandle, upserts: Vec<crate::db::Task>, deletes: Vec<String>) {
    if !upserts.is_empty() || !deletes.is_empty() {
        let _ = app.emit("tasks-changed", ());
        let _ = app.emit(
            "tasks-updated",
            serde_json::json!({ "source": "bot", "upserts": upserts, "deletes": deletes }),
        );
    }
}

fn active_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    crate::db::db_load(app.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.deleted_at.is_none() && t.archived != Some(true) && t.column != "done")
        .collect()
}

fn tool_list_tasks(app: &AppHandle) -> (String, Vec<TaskRef>) {
    let tasks = active_tasks(app);
    if tasks.is_empty() {
        return ("当前没有未完成的任务".into(), Vec::new());
    }
    let mut lines: Vec<String> = Vec::new();
    for t in &tasks {
        let col = match t.column.as_str() {
            "doing" => "进行中",
            "done" => "已完成",
            _ => "待办",
        };
        let due = t
            .due
            .as_deref()
            .map(|d| format!("，截止 {d}"))
            .unwrap_or_default();
        lines.push(format!("- [{}] {}{}（id={}）", col, t.title, due, t.id));
    }
    let refs: Vec<TaskRef> = tasks
        .iter()
        .map(|t| TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

/// 单卡查询（白名单单点工具，2026-08-17 22:57 老板拍板补充）：
/// 按 id 取单张任务卡的完整详情（区别于 list_tasks 的批量清单 + search_tasks 的关键词检索）。
/// - 必填参数：id（任务卡 UUID）
/// - 输出：标题/列/截止/备注/子任务/标签/绑定文件 + 归档/删除/机器人执行状态指示
/// - 单点白名单工具（非原子黑名单），LLM 可裸调
/// - 返回的 TaskRef 供后续 taskId 操作（complete_task / edit_task / bind_file 等）跟随引用
fn tool_query_single_task(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(id) = v["id"].as_str().map(|s| s.trim().to_string()) else {
        return ("query_single_task 缺少 id 参数".into(), Vec::new());
    };
    if id.is_empty() {
        return ("query_single_task id 不能为空".into(), Vec::new());
    }
    let Ok(tasks) = crate::db::db_load(app.clone()) else {
        return ("查询失败：数据库读取错误".into(), Vec::new());
    };
    let Some(t) = tasks.into_iter().find(|t| t.id == id) else {
        return (format!("未找到 id={id} 的任务卡"), Vec::new());
    };
    let col = match t.column.as_str() {
        "doing" => "进行中",
        "done" => "已完成",
        _ => "待办",
    };
    let mut lines: Vec<String> = vec![format!("- [{}] {}（id={}）", col, t.title, t.id)];
    if let Some(note) = &t.note {
        if !note.is_empty() {
            lines.push(format!("  备注：{note}"));
        }
    }
    if let Some(due) = &t.due {
        if !due.is_empty() {
            lines.push(format!("  截止：{due}"));
        }
    }
    if let Some(subtasks) = &t.subtasks {
        if !subtasks.is_empty() {
            lines.push(format!("  子任务（{}）：", subtasks.len()));
            for st in subtasks {
                let mark = if st.done { "✓" } else { "·" };
                lines.push(format!("    [{mark}] {}（id={}）", st.text, st.id));
            }
        }
    }
    if let Some(tags) = &t.tags {
        if !tags.is_empty() {
            lines.push(format!("  标签：{}", tags.join(", ")));
        }
    }
    if let Some(fp) = &t.file_path {
        if !fp.is_empty() {
            let kind = if t.file_is_dir == Some(true) {
                "目录"
            } else {
                "文件"
            };
            lines.push(format!("  绑定{kind}：{fp}"));
        }
    }
    let mut status: Vec<&str> = Vec::new();
    if t.archived == Some(true) {
        status.push("已归档");
    }
    if t.deleted_at.is_some() {
        status.push("已删除（回收站）");
    }
    if t.bot_assigned == Some(true) {
        status.push("机器人执行中");
    }
    if !status.is_empty() {
        lines.push(format!("  状态：{}", status.join(" / ")));
    }
    (
        lines.join("\n"),
        vec![TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        }],
    )
}

/// 搜索任务：搜所有任务卡（待办/进行中/已完成/已归档；不含回收站软删）。
/// 关键词匹配标题/备注/标签/子任务（大小写不敏感 contains）；结果带 id 供后续 taskId 操作
fn tool_search_tasks(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("search_tasks 缺少 query".into(), Vec::new());
    };
    if query.is_empty() {
        return ("搜索关键词不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e, Vec::new());
    }
    let tasks = crate::db::db_load(app.clone()).unwrap_or_default();
    let mut hits: Vec<crate::db::Task> = tasks
        .into_iter()
        .filter(|t| {
            t.deleted_at.is_none() && {
                let title_hit = t.title.to_lowercase().contains(&query);
                let note_hit = t
                    .note
                    .as_deref()
                    .map(|n| n.to_lowercase().contains(&query))
                    .unwrap_or(false);
                let tag_hit = t
                    .tags
                    .as_deref()
                    .map(|tags| tags.iter().any(|tg| tg.to_lowercase().contains(&query)))
                    .unwrap_or(false);
                let sub_hit = t
                    .subtasks
                    .as_deref()
                    .map(|subs| subs.iter().any(|s| s.text.to_lowercase().contains(&query)))
                    .unwrap_or(false);
                title_hit || note_hit || tag_hit || sub_hit
            }
        })
        .collect();
    if hits.is_empty() {
        return (
            format!(
                "没有找到匹配「{}」的任务",
                v["query"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    }
    hits.sort_by(|a, b| {
        a.order
            .unwrap_or(f64::MAX)
            .partial_cmp(&b.order.unwrap_or(f64::MAX))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut lines: Vec<String> = Vec::new();
    for t in &hits {
        let col = match t.column.as_str() {
            "doing" => "进行中",
            "done" => "已完成",
            _ => "待办",
        };
        let arch = if t.archived == Some(true) {
            "（已归档）"
        } else {
            ""
        };
        let due = t
            .due
            .as_deref()
            .map(|d| format!("，截止 {d}"))
            .unwrap_or_default();
        let tags = t
            .tags
            .as_deref()
            .map(|ts| format!("，标签：{}", ts.join("/")))
            .unwrap_or_default();
        lines.push(format!(
            "- [{}] {}{}{}{}（id={}）",
            col, t.title, arch, due, tags, t.id
        ));
    }
    let refs: Vec<TaskRef> = hits
        .iter()
        .map(|t| TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

fn tool_create_task(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(title) = v["title"].as_str() else {
        return ("create_task 缺少 title".into(), Vec::new());
    };
    let title = title.trim();
    if title.is_empty() {
        return ("任务标题不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(title, MAX_TITLE, "任务标题") {
        return (e, Vec::new());
    }
    if let Some(n) = v["note"].as_str() {
        if let Err(e) = check_len(n, MAX_NOTE, "备注") {
            return (e, Vec::new());
        }
    }
    if let Some(d) = v["due"].as_str() {
        if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
            return (e, Vec::new());
        }
    }
    let mut task = crate::db::Task {
        id: uuid::Uuid::new_v4().simple().to_string(),
        title: title.to_string(),
        due: v["due"].as_str().map(|s| s.to_string()),
        note: v["note"].as_str().map(|s| s.to_string()),
        tags: None,
        file_path: None,
        file_is_dir: None,
        column: match v["column"].as_str() {
            Some("doing") => "doing".into(),
            _ => "todo".into(),
        },
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: Some(chrono::Utc::now().timestamp_millis()),
        schedule: None,
        sched_last: None,
        bot_assigned: None,
    };
    // 插到列表顶部：取当前最小 order 减 1
    if let Ok(all) = crate::db::db_load(app.clone()) {
        let min = all
            .iter()
            .filter_map(|t| t.order)
            .fold(f64::INFINITY, f64::min);
        task.order = Some(if min.is_finite() { min - 1.0 } else { 0.0 });
    }
    match crate::db::db_upsert(app.clone(), vec![task.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![task.clone()], vec![]);
            (
                format!("已新建任务「{}」", task.title),
                vec![TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("新建任务失败：{e}"), Vec::new()),
    }
}

fn tool_complete_task(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(kw) = v["title"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("complete_task 缺少 title".into(), Vec::new());
    };
    let Some(task) = active_tasks(app)
        .into_iter()
        .find(|t| t.title.to_lowercase().contains(&kw))
    else {
        return (
            format!(
                "未找到匹配「{}」的未完成任务",
                v["title"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    };
    let mut next = task.clone();
    next.column = "done".into();
    next.completed_at = Some(chrono::Utc::now().timestamp_millis());
    next.updated_at = next.completed_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已完成任务「{}」", task.title),
                vec![TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("完成任务失败：{e}"), Vec::new()),
    }
}

/// 删除任务到回收站：**弹窗确认后才执行**（危险操作护栏；60s 无响应默认拒绝）
async fn tool_delete_task(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v) {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let approved = ask_user_confirm(app, "delete_task", &task.title).await;
    if !approved {
        return ("用户拒绝了删除，任务未删除".into(), Vec::new());
    }
    let mut next = task.clone();
    next.deleted_at = Some(chrono::Utc::now().timestamp_millis());
    next.updated_at = next.deleted_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已删除任务「{}」（进回收站）", task.title),
                vec![TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("删除任务失败：{e}"), Vec::new()),
    }
}

/// 按标题关键词找第一条未完成任务（大小写不敏感）
fn find_task_by_keyword(app: &AppHandle, kw: &str) -> Option<crate::db::Task> {
    active_tasks(app)
        .into_iter()
        .find(|t| t.title.to_lowercase().contains(kw))
}

/// 定位任务：优先 taskId 精确匹配，其次标题关键词模糊匹配。
/// 返回 (task, 定位说明)；找不到返回错误文案。
fn resolve_task(app: &AppHandle, v: &serde_json::Value) -> Result<crate::db::Task, String> {
    if let Some(id) = v["taskId"].as_str() {
        let id = id.trim();
        if !id.is_empty() {
            if let Some(t) = active_tasks(app).into_iter().find(|t| t.id == id) {
                return Ok(t);
            }
            return Err(format!("未找到 id={id} 的未完成任务（可能已完成或已删除）"));
        }
    }
    if let Some(kw) = v["title"].as_str() {
        let kw = kw.trim().to_lowercase();
        if !kw.is_empty() {
            if let Some(t) = find_task_by_keyword(app, &kw) {
                return Ok(t);
            }
            return Err(format!(
                "未找到匹配「{}」的未完成任务",
                v["title"].as_str().unwrap_or("")
            ));
        }
    }
    Err("缺少 taskId 或 title 参数".into())
}

fn tool_edit_task(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v) {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let mut next = task.clone();
    let mut changed: Vec<&str> = Vec::new();
    if let Some(nt) = v["newTitle"].as_str() {
        let nt = nt.trim();
        if !nt.is_empty() {
            if let Err(e) = check_len(nt, MAX_TITLE, "新标题") {
                return (e, Vec::new());
            }
            next.title = nt.to_string();
            changed.push("标题");
        }
    }
    if let Some(n) = v["note"].as_str() {
        if !n.trim().is_empty() {
            if let Err(e) = check_len(n, MAX_NOTE, "备注") {
                return (e, Vec::new());
            }
        }
        next.note = if n.trim().is_empty() {
            None
        } else {
            Some(n.to_string())
        };
        changed.push("备注");
    }
    if let Some(d) = v["due"].as_str() {
        if !d.trim().is_empty() {
            if let Err(e) = check_len(d, MAX_DUE, "截止时间") {
                return (e, Vec::new());
            }
        }
        next.due = if d.trim().is_empty() {
            None
        } else {
            Some(d.to_string())
        };
        changed.push("截止时间");
    }
    if let Some(tags) = v["tags"].as_array() {
        if tags.len() > MAX_TAGS {
            return (format!("标签数量超上限（最多 {MAX_TAGS} 个）"), Vec::new());
        }
        for t in tags {
            if let Some(ts) = t.as_str() {
                if let Err(e) = check_len(ts, MAX_TAG_LEN, "标签") {
                    return (e, Vec::new());
                }
            }
        }
        let list: Vec<String> = tags
            .iter()
            .filter_map(|t| t.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        next.tags = if list.is_empty() { None } else { Some(list) };
        changed.push("标签");
    }
    if let Some(c) = v["column"].as_str() {
        let c = c.trim();
        if matches!(c, "todo" | "doing" | "done") && c != next.column {
            next.column = c.to_string();
            // 列变更补完成语义（与主窗口一致）
            if c == "done" {
                next.completed_at = Some(chrono::Utc::now().timestamp_millis());
                next.archived = Some(false);
            } else {
                next.completed_at = None;
                next.archived = None;
            }
            changed.push("状态列");
        }
    }
    if changed.is_empty() {
        return ("没有可修改的字段".into(), Vec::new());
    }
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已更新任务「{}」（{}）", next.title, changed.join("、")),
                vec![TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("编辑任务失败：{e}"), Vec::new()),
    }
}

fn tool_add_subtask(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(text) = v["text"].as_str().map(|s| s.trim()) else {
        return ("add_subtask 缺少 text".into(), Vec::new());
    };
    if text.is_empty() {
        return ("子任务内容不能为空".into(), Vec::new());
    }
    if let Err(e) = check_len(text, MAX_SUBTASK_TEXT, "子任务内容") {
        return (e, Vec::new());
    }
    let task = match resolve_task(app, &v) {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let mut next = task.clone();
    let mut subs = next.subtasks.unwrap_or_default();
    subs.push(crate::db::Subtask {
        id: uuid::Uuid::new_v4().simple().to_string(),
        text: text.to_string(),
        done: false,
    });
    next.subtasks = Some(subs);
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已给任务「{}」添加子任务「{}」", next.title, text),
                vec![TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("添加子任务失败：{e}"), Vec::new()),
    }
}

fn tool_toggle_subtask(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(skw) = v["text"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("toggle_subtask 缺少 text".into(), Vec::new());
    };
    if let Err(e) = check_len(&skw, MAX_SUBTASK_TEXT, "子任务关键词") {
        return (e, Vec::new());
    }
    let task = match resolve_task(app, &v) {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let subs = task.subtasks.clone().unwrap_or_default();
    let Some(idx) = subs
        .iter()
        .position(|s| s.text.to_lowercase().contains(&skw))
    else {
        return (
            format!(
                "任务「{}」没有匹配「{}」的子任务",
                task.title,
                v["text"].as_str().unwrap_or("")
            ),
            Vec::new(),
        );
    };
    let mut next = task.clone();
    let mut subs2 = subs;
    subs2[idx].done = !subs2[idx].done;
    next.subtasks = Some(subs2);
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            let st_text = next
                .subtasks
                .as_ref()
                .and_then(|s| s.get(idx))
                .map(|s| s.text.clone())
                .unwrap_or_default();
            let done_mark = next
                .subtasks
                .as_ref()
                .and_then(|s| s.get(idx))
                .map(|s| s.done)
                .unwrap_or(false);
            (
                format!(
                    "子任务「{st_text}」已{}",
                    if done_mark {
                        "勾选 ✓"
                    } else {
                        "取消勾选"
                    }
                ),
                vec![TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("切换子任务状态失败：{e}"), Vec::new()),
    }
}

/// 绑定文件/文件夹：弹系统选择框由用户挑选，结果写回任务的 filePath/fileIsDir
async fn tool_bind_file(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let is_dir = v["isDir"].as_bool().unwrap_or(false);
    let task = match resolve_task(app, &v) {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let handle = app.clone();
    // 弹框在后台线程阻塞执行，避免卡住异步运行时
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        let dlg = handle.dialog().file();
        if is_dir {
            dlg.blocking_pick_folder()
        } else {
            dlg.blocking_pick_file()
        }
    })
    .await
    .unwrap_or(None);

    let Some(path) = picked.and_then(file_path_to_string) else {
        return ("用户取消了选择，未绑定".into(), Vec::new());
    };
    let mut next = task;
    next.file_path = Some(path.clone());
    next.file_is_dir = Some(is_dir);
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!(
                    "已给任务「{}」绑定{}：{}",
                    next.title,
                    if is_dir { "文件夹" } else { "文件" },
                    path
                ),
                vec![TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("绑定失败：{e}"), Vec::new()),
    }
}

fn file_path_to_string(p: tauri_plugin_dialog::FilePath) -> Option<String> {
    match p {
        tauri_plugin_dialog::FilePath::Path(pb) => pb.to_str().map(|s| s.to_string()),
        tauri_plugin_dialog::FilePath::Url(u) => Some(u.to_string()),
    }
}

/// 把文件路径绑定到任务卡（不弹框；路径必须真实存在，防模型编造）
async fn tool_link_file_to_task(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(path) = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("link_file_to_task 缺少 path".into(), Vec::new());
    };
    if !std::path::Path::new(&path).exists() {
        return (format!("路径不存在，拒绝绑定：{path}"), Vec::new());
    }
    // 上线安全审计 P1：此前只校验"路径存在"，模型可绑定任意文件（如 ~/.ssh/id_rsa）到任务卡，
    // 再经 extract_document 的「任务卡绑定文件」白名单读走内容 —— 白名单被架空。
    // 收窄：只能绑定 AI_Gen_Files 目录内的文件（工具用途 = 把机器人产物绑回任务卡，产物必在此目录）。
    // 用户亲手绑定的其他文件走 bind_file 弹框，不在此限。
    {
        let canon =
            std::fs::canonicalize(&path).unwrap_or_else(|_| std::path::PathBuf::from(&path));
        let gen_dir = crate::db::data_dir(app).join("AI_Gen_Files");
        let in_gen = match std::fs::canonicalize(&gen_dir) {
            Ok(gen) => canon.starts_with(&gen),
            Err(_) => false,
        };
        if !in_gen {
            return (
                "已拒绝绑定该路径：link_file_to_task 只能绑定 AI_Gen_Files 目录内的文件；其他文件请在任务卡上手动「绑定文件」".into(),
                Vec::new(),
            );
        }
    }
    let task = match resolve_task(app, &v) {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let mut next = task.clone();
    next.file_path = Some(path.clone());
    next.file_is_dir = Some(false);
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]) {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已给任务「{}」绑定文件：{path}", next.title),
                vec![TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("绑定失败：{e}"), Vec::new()),
    }
}

// ───────────────────────── 文档 / Python 工具（bot_py 桥接） ─────────────────────────

/// 提取文档文本：path 给定则直读（任务卡绑定文件），否则弹框选文件；返回路径 + 文本供模型阅读/润色
/// extract_document path 白名单（二次审计 P1-2）：只允许任务卡绑定文件或 AI_Gen_Files 目录内文件。
/// 规范化路径比较，防 ../ 绕过。无 path 时走弹框（用户亲手选，不受此限）。
fn extract_path_allowed(app: &AppHandle, path: &str) -> bool {
    let Ok(canon) = std::fs::canonicalize(path) else {
        return false;
    };
    // 1) AI_Gen_Files 目录内
    let gen_dir = crate::db::data_dir(app).join("AI_Gen_Files");
    if let Ok(gen) = std::fs::canonicalize(&gen_dir) {
        if canon.starts_with(&gen) {
            return true;
        }
    }
    // 2) 任务卡绑定文件
    if let Ok(tasks) = crate::db::db_load(app.clone()) {
        for t in tasks {
            if let Some(fp) = t.file_path.as_deref() {
                if let Ok(fc) = std::fs::canonicalize(fp) {
                    if fc == canon {
                        return true;
                    }
                }
            }
        }
    }
    false
}

async fn tool_extract_document(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let path_opt = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 模型直传 path 时白名单校验（无 path 走弹框，用户亲手选不受限）
    if let Some(p) = path_opt.as_deref() {
        if !extract_path_allowed(app, p) {
            return (
                "已拒绝读取该路径：extract_document 的 path 只允许任务卡绑定文件或 AI_Gen_Files 目录内的文件；\
需要读取其他文件请先绑定到任务卡，或让用户通过弹框选择".into(),
                Vec::new(),
            );
        }
    }
    match crate::bot_py::doc_extract(app.clone(), path_opt).await {
        Ok(res) => (format_extract_output(&res.path, &res.text), Vec::new()),
        Err(e) => (format!("提取失败：{e}"), Vec::new()),
    }
}

/// extract_document 输出格式化（30000 字符截断 + 截断提示）。
/// 抽出来便于单测，避免每次都要 mock Tauri AppHandle。
fn format_extract_output(path: &str, text: &str) -> String {
    let limited: String = text.chars().take(30000).collect();
    let mut out = format!("[文档路径] {}\n[文档内容]\n{}", path, limited);
    if limited.chars().count() < text.chars().count() {
        out.push_str("\n\n（内容过长已截断，后面内容未提取；修订模式请把 original 参数填上你实际收到的原文行列表）");
    }
    out
}

/// 解析文档工具共用的 filename 参数
fn opt_filename(v: &serde_json::Value) -> Option<String> {
    v["filename"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 生成 Word：润色后的段落写新文档（只产出、不覆盖，落 AI_Gen_Files）
async fn tool_create_word(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ("create_word 缺少 paragraphs".into(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ("paragraphs 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_word(app.clone(), title, paragraphs, opt_filename(&v)).await {
        Ok(out) => (format!("已生成 Word 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 修订模式 Word：回读原文 + 修订段落 diff，产出带 track changes 标记的文档
async fn tool_create_word_revisions(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    // originalPath 由模型转述，同样过白名单（防回读任意文件）
    if let Some(op) = v["originalPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !extract_path_allowed(app, op) {
            return (
                "已拒绝读取原文路径：originalPath 只允许任务卡绑定文件或 AI_Gen_Files 目录内的文件"
                    .into(),
                Vec::new(),
            );
        }
    }
    let Some(arr) = v["revised"].as_array() else {
        return (
            "create_word_revisions 缺少 revised（润色后的段落列表）".into(),
            Vec::new(),
        );
    };
    let revised: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if revised.is_empty() {
        return ("revised 不能为空".into(), Vec::new());
    }
    let path = v["originalPath"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let original: Vec<String> = v["original"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if path.is_none() && original.is_empty() {
        return (
            "create_word_revisions 缺少原文：请传 originalPath（来自 extract_document 的 [文档路径]）或 original 行列表"
                .into(),
            Vec::new(),
        );
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_word_revisions(app.clone(), title, path, original, revised, opt_filename(&v)).await
    {
        Ok(out) => (
            format!("已生成修订版 Word（修订模式：删除线=删、红色下划线=增，可在 Word「审阅」里逐条接受/拒绝）：{out}"),
            Vec::new(),
        ),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}
async fn tool_create_excel(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(sheets) = v["sheets"].as_array() else {
        return ("create_excel 缺少 sheets".into(), Vec::new());
    };
    if sheets.is_empty() {
        return ("sheets 不能为空".into(), Vec::new());
    }
    match crate::bot_py::doc_make_excel(app.clone(), sheets.clone(), opt_filename(&v)).await {
        Ok(out) => (format!("已生成 Excel 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PPT：slides 结构 [{title, bullets: [..]}]
async fn tool_create_ppt(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(slides) = v["slides"].as_array() else {
        return ("create_ppt 缺少 slides".into(), Vec::new());
    };
    if slides.is_empty() {
        return ("slides 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    // theme：blue/navy/teal/forest/wine/sky/plum/coral/dark/green 十套；模型自选，非法回退 blue
    let theme = v["theme"].as_str().map(|s| s.to_string());
    match crate::bot_py::doc_make_ppt(app.clone(), title, slides.clone(), opt_filename(&v), theme)
        .await
    {
        Ok(out) => (format!("已生成 PPT 演示文稿：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 生成 PDF：title + 段落列表
async fn tool_create_pdf(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(arr) = v["paragraphs"].as_array() else {
        return ("create_pdf 缺少 paragraphs".into(), Vec::new());
    };
    let paragraphs: Vec<String> = arr
        .iter()
        .filter_map(|p| p.as_str().map(|s| s.to_string()))
        .collect();
    if paragraphs.is_empty() {
        return ("paragraphs 不能为空".into(), Vec::new());
    }
    let title = v["title"].as_str().unwrap_or("").to_string();
    match crate::bot_py::doc_make_pdf(app.clone(), title, paragraphs, opt_filename(&v)).await {
        Ok(out) => (format!("已生成 PDF 文档：{out}"), Vec::new()),
        Err(e) => (format!("生成失败：{e}"), Vec::new()),
    }
}

/// 联网搜索：本机执行 Bing 抓取，结果回传给模型（MiniMax web_search 由客户端执行）
async fn tool_web_search(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("web_search 缺少 query".into(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e, Vec::new());
    }
    audit_log(
        app,
        &format!("web_search | query: {}", truncate_for_log(&query, 100)),
    );
    match crate::bot_web::web_search(&query).await {
        Ok(results) => (results, Vec::new()),
        Err(e) => (format!("搜索失败：{e}"), Vec::new()),
    }
}

/// 抓取网页正文：http/https 公网地址，转纯文本回传（截 30000 字）
async fn tool_fetch_url(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(u) = v["url"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return ("fetch_url 缺少 url".into(), Vec::new());
    };
    if let Err(e) = check_len(&u, MAX_KEYWORD, "网址") {
        return (e, Vec::new());
    }
    audit_log(
        app,
        &format!("fetch_url | url: {}", truncate_for_log(&u, 100)),
    );
    match crate::bot_web::fetch_text(&u).await {
        Ok(text) => {
            let mut out: String = text.chars().take(30000).collect();
            if out.chars().count() >= 30000 {
                out.push_str("\n\n（内容过长已截断）");
            }
            (out, Vec::new())
        }
        Err(e) => (format!("抓取失败：{e}"), Vec::new()),
    }
}

/// 自由 Python 编程：开关开启才放行（超时 60s、独立临时目录、输出截断）
fn tool_run_python(app: &AppHandle, args: &str) -> (String, Vec<TaskRef>) {
    let v = parse_args(args);
    let Some(code) = v["code"].as_str() else {
        return ("run_python 缺少 code".into(), Vec::new());
    };
    // 工具链在 async 上下文：走同步核心（py_exec 命令是 async，这里不能 await）
    match crate::bot_py::py_exec_sync(app, code.to_string(), None) {
        Ok(r) => {
            let mut out = String::new();
            if !r.stdout.trim().is_empty() {
                out.push_str(&r.stdout);
            }
            if !r.stderr.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[stderr] {}", r.stderr.trim()));
            }
            if out.is_empty() {
                out = "执行完成（无输出）".into();
            }
            (out, Vec::new())
        }
        Err(e) => (format!("执行失败：{e}"), Vec::new()),
    }
}

/// F-1 BotConfig 序列化与默认值单测（2026-08-18 老板拍板 P0 release blocker）
#[cfg(test)]
mod bot_config_tests {
    use super::*;

    #[test]
    fn default_has_bypass_llm_on_pre_step_hit_true() {
        let cfg = BotConfig::default();
        assert!(
            cfg.bypass_llm_on_pre_step_hit,
            "Default impl 应默认开启新行为（bypass=true）"
        );
    }

    #[test]
    fn old_config_without_bypass_field_deserializes_to_true() {
        // 模拟老用户 bot-config.json 没有 bypass_llm_on_pre_step_hit 字段
        let raw = r#"{"baseUrl":"https://api.deepseek.com/v1","model":"deepseek-chat"}"#;
        let cfg: BotConfig =
            serde_json::from_str(raw).expect("老配置应通过 struct 级 #[serde(default)] 兼容");
        assert!(
            cfg.bypass_llm_on_pre_step_hit,
            "老配置缺字段应默认 true（开启新行为，保证 release 不破现有用户）"
        );
    }
}

/// Plan C 单函数测试（F-6 step 5）：tool_extract_document 输出格式化
/// 不依赖 Tauri AppHandle，验证 30000 字符截断 + 截断提示逻辑。
#[cfg(test)]
mod tool_extract_document_tests {
    use super::*;

    #[test]
    fn format_extract_output_short_text_returns_full_text_no_truncation_suffix() {
        let text = "短文本".repeat(100); // 100 个汉字 = 300 chars，远低于 30000
        let out = format_extract_output("/tmp/sample.md", &text);
        assert!(
            out.contains(&text),
            "短文本应原样保留：\n--out--\n{out}\n--text--\n{text}"
        );
        assert!(
            !out.contains("已截断"),
            "短文本不应出现截断提示，实际输出：\n{out}"
        );
        assert!(out.starts_with("[文档路径] /tmp/sample.md\n[文档内容]\n"));
    }

    #[test]
    fn format_extract_output_long_text_truncates_with_suffix() {
        // 35000 个 'A'，远超 30000 阈值
        let text = "A".repeat(35000);
        let out = format_extract_output("/tmp/big.md", &text);
        assert!(
            out.contains("已截断"),
            "长文本必须出现截断提示，实际输出末尾：\n{}",
            &out[out.len().saturating_sub(200)..]
        );
        // 输出含有的 'A' 数量应 == 30000（截断后）
        let a_count = out.matches('A').count();
        assert_eq!(
            a_count, 30000,
            "长文本截断后应剩 30000 个 'A'，实际 {a_count}"
        );
    }
}

/// Phase 7 Q3 主编编排单测补（2026-08-18 12:50）：
/// 盖 run_model_loop 调用的两个纯函数。format_recovery_hint 是 LLM 提示词、
/// merge_task_refs_dedup 是首次保留语义，都不能漂移。
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
