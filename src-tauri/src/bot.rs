//! 内置机器人：大模型聊天 + WMessage 任务管理工具调用。
//!
//! F-6 step 5 拆分后（2026-08-18），本模块聚焦「工具调度核心 + 配置/审计底座」，
//! 编排/决策/工具循环拆到 4 个兄弟模块：
//! - `bot_chat`        — 入口编排（bot_chat / bot_compact / bot_execute_task）
//! - `bot_model_loop`  — 流式 SSE + 工具循环（run_model_loop / parse_sse_chunk / feed_think / TOOLS）
//! - `bot_scheduler`   — ⏰ 定时任务卡自动执行（start_scheduler / occurrence_after / sched_tests）
//! - `bot_slash`       — 旁路基础设施（bot_stop / 确认弹窗 / 机器人开关）
//!
//! 本模块保留：
//! - 工具分发核心：execute_tool + 20+ 个 tool_* 实现 + TaskRef 构造 helpers
//! - 配置底座：BotConfig / BotConfigView / keyring / migrate_legacy_key
//! - 审计底座：audit_log / audit_log_hook / escape_for_log / bot_log_read
//! - 参数上限常量 + check_len（bot_model_loop 拼装 tool_call 时也要用）
//!
//! 安全性（对齐《Harness 安全网关》需求）：
//! - 工具白名单：固定 TOOLS schema（在 bot_model_loop.rs）+ execute_tool match，模型编造的工具一律拒绝
//! - 调用熔断：单轮 Function 调用 ≤10 次 + 7 次软警告（收尾提醒）；聊天 8 轮/任务执行 10 轮工具循环；
//!   HTTP connect 15s / 总超时 300s
//! - 参数校验：标题/备注/关键词/子任务/截止时间长度上限、标签数量上限
//! - 审计日志：数据目录 bot.log 记录用户指令、工具名、参数、结果
//! - API Key 存系统凭据存储（keyring），文件不落明文

use crate::bot_skills::tool_use_skill;
use crate::error::{CommandError, CommandResult};

// 保留对外接口 re-export，避免拆分后 bot_skills / tests/llm_integration 等
// 已存在的调用方（`crate::bot::TaskRef` / `crate::bot::parse_sse_chunk` /
// `crate::bot::ToolCallDelta` / `crate::bot::BotChatResult`）中断。
// 新代码应优先直接引用 bot_chat / bot_model_loop 模块。
pub use crate::bot_chat::{BotChatResult, TaskRef};
pub use crate::bot_model_loop::{parse_sse_chunk, ToolCallDelta};

use serde::{Deserialize, Serialize};
use std::io::Write;
use tauri::{AppHandle, Emitter};

// ───────────────────────── API 配置 ─────────────────────────

/// 凭据存储条目：macOS 钥匙串 / Windows 凭据管理器
pub const KEYRING_SERVICE: &str = "wmessage-bot";
pub const KEYRING_USER: &str = "api-key";

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

pub fn config_path(app: &AppHandle) -> std::path::PathBuf {
    crate::db::data_dir(app).join("bot-config.json")
}

/// F-1 开关读取 helper：bot-config.json 缺字段 / 文件不存在 / 解析失败都默认 true（新行为）。
/// 比 bot_get_config 轻量：跳过 BotConfigView 构造 + key 校验，bot_chat 入口用。
pub fn read_bypass_llm_switch(app: &AppHandle) -> bool {
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

fn key_entry() -> CommandResult<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| CommandError::KeyringError(format!("系统凭据存储不可用：{e}")))
}

/// F1（Phase 6b）：get_password 结果分类——任何失败都映射为 KeyringError 结构化变体，
/// 不再走 String 逃生舱（同类故障产出两种 code，前端 hintForCode 失配）。
/// 抽成纯函数便于单测（keyring 真实存储在测试环境不可用）。
fn classify_get_password(r: Result<String, keyring::Error>) -> CommandResult<String> {
    r.map_err(|e| CommandError::KeyringError(format!("读取 API Key 失败：{e}")))
}

/// F1（Phase 6b）：「key 不存在」（NoEntry）→ Ok(false)；
/// keyring 真实故障（钥匙串锁定 / 权限拒绝）→ Err(KeyringError)，不静默吞成 false。
fn classify_has_key(r: Result<String, keyring::Error>) -> CommandResult<bool> {
    match r {
        Ok(_) => Ok(true),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(e) => Err(CommandError::KeyringError(format!("检查 API Key 失败：{e}"))),
    }
}

pub fn read_api_key() -> CommandResult<String> {
    classify_get_password(key_entry()?.get_password())
}

pub fn has_api_key() -> CommandResult<bool> {
    match key_entry() {
        Ok(e) => classify_has_key(e.get_password()),
        Err(e) => Err(e),
    }
}

fn write_api_key(key: &str) -> CommandResult<()> {
    key_entry()?
        .set_password(key)
        .map_err(|e| CommandError::KeyringError(format!("保存 API Key 失败：{e}")))
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
    // 凭据存储里没有 key 时才写入（避免旧明文覆盖用户新存的 key）；
    // keyring 故障（Err）按「写不入」同等处理：保留文件明文，下次再试
    if !has_api_key().unwrap_or(false) && write_api_key(&k).is_err() {
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

    // F1：keyring 真实故障（钥匙串锁定/权限拒绝）不再吞成「未配置」，
    // 结构化 KeyringError 透传给前端，设置页可提示用户检查 keychain
    let has_api_key = has_api_key()?;
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
pub fn audit_log_hook<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    audit_log(app, line);
}

pub fn audit_log<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    // 与 audit::write_event 共用同一把写锁，防并发 append 交错错行
    let _g = crate::audit::BOT_LOG_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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

/// 审计日志安全转义 + 截断（P2-11）：剥换行/管道符，防伪造「INFO |」前缀与多行撕裂。
/// 规则：`| ` → `|  `（双空格），剩余裸 `|` → `||`，`\n` → `\\n`，`\r` → `\\r`；
/// 转义后按字符数截到 max 加省略号。与 bot_py::escape_for_log 同一规则。
pub(crate) fn escape_for_log(s: &str, max: usize) -> String {
    let escaped = s
        .replace("| ", "|  ")
        .replace('|', "||")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let count = escaped.chars().count();
    if count <= max {
        escaped
    } else {
        let mut out: String = escaped.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// 向后兼容别名（P2-11）：bot_chat / bot_model_loop / bot_scheduler 仍用旧名，
/// 行为即 escape_for_log（同族日志伪造问题一并修复）；新代码请直接用 escape_for_log。
pub(crate) fn truncate_for_log(s: &str, max: usize) -> String {
    escape_for_log(s, max)
}

/// 读取机器人审计日志（倒序，最新在前；默认 200 行，上限 2000）
/// F3（Phase 6b）：读失败（权限/磁盘/损坏）返回 Err(IoError) + ERROR 审计，
/// 不再静默吞成「暂无日志」；仅「文件不存在」返回占位文案。
#[tauri::command]
pub fn bot_log_read(app: AppHandle, limit: Option<usize>) -> CommandResult<String> {
    let p = crate::db::data_dir(&app).join("bot.log");
    match read_log_tail(&p, limit) {
        Ok(s) => Ok(s),
        Err(e) => {
            let msg = e.message();
            crate::audit::write_error_audit(&app, "bot_log_read_fail", &[("err", msg.as_str())]);
            Err(e)
        }
    }
}

/// F3（Phase 6b）：纯路径参数版便于单测（tauri command 绑定 Wry AppHandle）。
/// NotFound → Ok("（暂无日志）")（日志确实没东西）；其他 IO 错误 → Err(IoError)。
fn read_log_tail(path: &std::path::Path, limit: Option<usize>) -> CommandResult<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("（暂无日志）".into()),
        Err(e) => return Err(CommandError::IoError(format!("读取日志失败：{e}"))),
    };
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    let mut lines: Vec<&str> = raw.lines().collect();
    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }
    lines.reverse();
    Ok(lines.join("\n"))
}

// ───────────────────────── 参数上限（防幻觉/防刷爆） ─────────────────────────

pub const MAX_TITLE: usize = 200;
pub const MAX_NOTE: usize = 5000;
pub const MAX_KEYWORD: usize = 100;
pub const MAX_SUBTASK_TEXT: usize = 200;
pub const MAX_DUE: usize = 30;
pub const MAX_TAGS: usize = 10;
pub const MAX_TAG_LEN: usize = 30;

/// 校验字符串长度上限，超限返回错误文案
pub fn check_len(value: &str, max: usize, what: &str) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{what}过长（上限 {max} 字）"));
    }
    Ok(())
}

// ───────────────────────── 工具调度核心（execute_tool dispatch） ─────────────────────────

/// NEW-D-1：早退路径（pre_execute 拦截 / skill_on_step 熔断）的审计事件序列。
/// `tool.call` 已在入口发出，这里按写入顺序补齐后续事件并以 `tool.return` 配平，
/// 否则统计面板出现「悬挂调用」（call > return）。
/// 抽成纯函数：事件名 + kv + 顺序可单测（execute_tool 是 Wry 签名，无法 mock runtime 直调，
/// 与 skill_e2e.rs 注释记录的「泛型化重构暂缓」一致）；调用点只负责逐条 emit。
/// - 拦截路径（err=None）：pre_execute.deny + tool.return(reason=denied)
/// - 熔断路径（err=Some）：skill_on_step_error（补 Warn 可见性）+ tool.return(reason=skill_step_failed)
fn early_return_events(
    name: &str,
    reason: &str,
    dur_ms: u64,
    err: Option<&str>,
) -> Vec<(
    crate::audit::AuditLevel,
    &'static str,
    Vec<(&'static str, String)>,
)> {
    let mut events = Vec::with_capacity(2);
    match err {
        Some(e) => events.push((
            crate::audit::AuditLevel::Warn,
            "skill_on_step_error",
            vec![("tool", name.to_string()), ("err", e.to_string())],
        )),
        None => events.push((
            crate::audit::AuditLevel::Warn,
            "pre_execute.deny",
            vec![("tool", name.to_string())],
        )),
    }
    events.push((
        crate::audit::AuditLevel::Warn,
        "tool.return",
        vec![
            ("tool", name.to_string()),
            ("reason", reason.to_string()),
            ("exit_code", "none".to_string()),
            ("duration_ms", dur_ms.to_string()),
        ],
    ));
    events
}

/// 进程内执行工具，返回 (给模型的文本结果, 涉及的任务引用)
/// `pub` 让 `bot_skills::run_skill_scheduler`（Phase 1 DSL 调度器）可调用，
/// 不暴露给前端 — 通过 `is_atomic_tool` 黑名单 + pre-execute 校验保护。
pub async fn execute_tool(app: &AppHandle, name: &str, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    execute_tool_with_stop(app, name, args, None).await
}

/// execute_tool 的可停止版本（NEW-C-4）：携带 /stop 守卫，run_python 等长耗时工具
/// 在执行中即可被中断；Skill 调度器等无守卫调用方走 execute_tool（stop=None）。
pub async fn execute_tool_with_stop(
    app: &AppHandle,
    name: &str,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let start = std::time::Instant::now();
    // 0. tool.call 结构化（F-3 第三步 2026-08-18）
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "tool.call",
        "tool" => name,
        "args_preview" => args.chars().take(80).collect::<String>(),
    );
    // 1. 后置拦截：原子黑名单（老板 2026-08-17 18:14 拍板）
    //    仅作为 Skill 内部子步骤、不允许裸调的底层原子 Function → 硬锁阻断
    //    只有 Skill 在 Running 状态时才放行；其他时候直接返回错误 + 提示走对应 Skill
    // F-2 抽象层：execute_tool 通过 middleware::run_pre_execute 调 pre-execute
    let active = crate::tool_guard::is_skill_active();
    if let Some(msg) = crate::middleware::run_pre_execute(app, name, active) {
        // NEW-D-1：tool.call 已发出，早退前必须配平 tool.return（reason=denied），
        // 否则统计面板出现「悬挂调用」（call > return）
        for (level, event, kv) in early_return_events(name, "denied", start.elapsed().as_millis() as u64, None) {
            crate::audit::write_event(app, level, event, &kv);
        }
        return (msg, Vec::new());
    }
    // 2. Skill 调度器步骤钩子：活动技能时计数/熔断/动作记录（use_skill 自身跳过）
    if name != "use_skill" {
        if let Err(e) = crate::bot_skills::skill_on_step(app, name, args) {
            // NEW-D-1：补 Warn 可见性（skill_on_step_error）+ tool.return 配平（reason=skill_step_failed）
            for (level, event, kv) in early_return_events(name, "skill_step_failed", start.elapsed().as_millis() as u64, Some(&e)) {
                crate::audit::write_event(app, level, event, &kv);
            }
            return (e, Vec::new());
        }
    }
    let (text, refs): (String, Vec<crate::bot_chat::TaskRef>) = match name {
        "list_tasks" => tool_list_tasks(app).await,
        "query_single_task" => tool_query_single_task(app, args).await,
        "create_task" => tool_create_task(app, args).await,
        "complete_task" => tool_complete_task(app, args).await,
        "delete_task" => tool_delete_task(app, args).await,
        "edit_task" => tool_edit_task(app, args).await,
        "add_subtask" => tool_add_subtask(app, args).await,
        "toggle_subtask" => tool_toggle_subtask(app, args).await,
        "bind_file" => tool_bind_file(app, args).await,
        "link_file_to_task" => tool_link_file_to_task(app, args).await,
        "search_tasks" => tool_search_tasks(app, args).await,
        "extract_document" => tool_extract_document(app, args).await,
        "create_word" => tool_create_word(app, args).await,
        "create_word_revisions" => tool_create_word_revisions(app, args).await,
        "create_excel" => tool_create_excel(app, args).await,
        "create_ppt" => tool_create_ppt(app, args).await,
        "create_pdf" => tool_create_pdf(app, args).await,
        "run_python" => tool_run_python(app, args, stop).await,
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
    crate::audit_event!(
        app,
        level,
        "tool.return",
        "tool" => name,
        "ms" => dur_ms,
        "refs" => refs.len(),
        "preview" => text.chars().take(80).collect::<String>(),
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
pub fn broadcast_after_mutation(app: &AppHandle, upserts: Vec<crate::db::Task>, deletes: Vec<String>) {
    if !upserts.is_empty() || !deletes.is_empty() {
        let _ = app.emit("tasks-changed", ());
        let _ = app.emit(
            "tasks-updated",
            serde_json::json!({ "source": "bot", "upserts": upserts, "deletes": deletes }),
        );
    }
}

async fn active_tasks(app: &AppHandle) -> Vec<crate::db::Task> {
    crate::db::db_load(app.clone())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.deleted_at.is_none() && t.archived != Some(true) && t.column != "done")
        .collect()
}

// ───────────────────────── 任务管理工具实现（20+ functions） ─────────────────────────

async fn tool_list_tasks(app: &AppHandle) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let tasks = active_tasks(app).await;
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
    let refs: Vec<crate::bot_chat::TaskRef> = tasks
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
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
async fn tool_query_single_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(id) = v["id"].as_str().map(|s| s.trim().to_string()) else {
        return ("query_single_task 缺少 id 参数".into(), Vec::new());
    };
    if id.is_empty() {
        return ("query_single_task id 不能为空".into(), Vec::new());
    }
    let Ok(tasks) = crate::db::db_load(app.clone()).await else {
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
        vec![crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        }],
    )
}

/// 搜索任务：搜所有任务卡（待办/进行中/已完成/已归档；不含回收站软删）。
/// 关键词匹配标题/备注/标签/子任务（大小写不敏感 contains）；结果带 id 供后续 taskId 操作
async fn tool_search_tasks(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(query) = v["query"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("search_tasks 缺少 query".into(), Vec::new());
    };
    if query.is_empty() {
        return ("搜索关键词不能为空".into(), Vec::new());
    };
    if let Err(e) = check_len(&query, MAX_KEYWORD, "搜索关键词") {
        return (e, Vec::new());
    }
    let tasks = crate::db::db_load(app.clone()).await.unwrap_or_default();
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
    let refs: Vec<crate::bot_chat::TaskRef> = hits
        .iter()
        .map(|t| crate::bot_chat::TaskRef {
            id: t.id.clone(),
            title: t.title.clone(),
        })
        .collect();
    (lines.join("\n"), refs)
}

async fn tool_create_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
    if let Ok(all) = crate::db::db_load(app.clone()).await {
        let min = all
            .iter()
            .filter_map(|t| t.order)
            .fold(f64::INFINITY, f64::min);
        task.order = Some(if min.is_finite() { min - 1.0 } else { 0.0 });
    }
    match crate::db::db_upsert(app.clone(), vec![task.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![task.clone()], vec![]);
            (
                format!("已新建任务「{}」", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("新建任务失败：{e}"), Vec::new()),
    }
}

async fn tool_complete_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(kw) = v["title"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("complete_task 缺少 title".into(), Vec::new());
    };
    let Some(task) = active_tasks(app).await
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
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已完成任务「{}」", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("完成任务失败：{e}"), Vec::new()),
    }
}

/// 删除任务到回收站：**弹窗确认后才执行**（危险操作护栏；60s 无响应默认拒绝）
async fn tool_delete_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let approved = crate::bot_slash::ask_user_confirm(app, "delete_task", &task.title).await;
    if !approved {
        return ("用户拒绝了删除，任务未删除".into(), Vec::new());
    }
    let mut next = task.clone();
    next.deleted_at = Some(chrono::Utc::now().timestamp_millis());
    next.updated_at = next.deleted_at;
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已删除任务「{}」（进回收站）", task.title),
                vec![crate::bot_chat::TaskRef {
                    id: task.id.clone(),
                    title: task.title.clone(),
                }],
            )
        }
        Err(e) => (format!("删除任务失败：{e}"), Vec::new()),
    }
}

/// 按标题关键词找第一条未完成任务（大小写不敏感）
async fn find_task_by_keyword(app: &AppHandle, kw: &str) -> Option<crate::db::Task> {
    active_tasks(app).await
        .into_iter()
        .find(|t| t.title.to_lowercase().contains(kw))
}

/// 定位任务：优先 taskId 精确匹配，其次标题关键词模糊匹配。
/// 返回 (task, 定位说明)；找不到返回错误文案。
async fn resolve_task(app: &AppHandle, v: &serde_json::Value) -> Result<crate::db::Task, String> {
    if let Some(id) = v["taskId"].as_str() {
        let id = id.trim();
        if !id.is_empty() {
            if let Some(t) = active_tasks(app).await.into_iter().find(|t| t.id == id) {
                return Ok(t);
            }
            return Err(format!("未找到 id={id} 的未完成任务（可能已完成或已删除）"));
        }
    }
    if let Some(kw) = v["title"].as_str() {
        let kw = kw.trim().to_lowercase();
        if !kw.is_empty() {
            if let Some(t) = find_task_by_keyword(app, &kw).await {
                return Ok(t);
            }
            return Err(format!(
                "未找到匹配「{}」的未完成任务",
                v["title"].as_str().unwrap_or("")
            ));
        }
    }
    // TODO(P0-6A): 无 1:1 CommandError 变体，暂走 Internal；待新增专用变体后迁移
    Err("缺少 taskId 或 title 参数".into())
}

async fn tool_edit_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let task = match resolve_task(app, &v).await {
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
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已更新任务「{}」（{}）", next.title, changed.join("、")),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("编辑任务失败：{e}"), Vec::new()),
    }
}

async fn tool_add_subtask(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
    let task = match resolve_task(app, &v).await {
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
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已给任务「{}」添加子任务「{}」", next.title, text),
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("添加子任务失败：{e}"), Vec::new()),
    }
}

async fn tool_toggle_subtask(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(skw) = v["text"].as_str().map(|s| s.trim().to_lowercase()) else {
        return ("toggle_subtask 缺少 text".into(), Vec::new());
    };
    if let Err(e) = check_len(&skw, MAX_SUBTASK_TEXT, "子任务关键词") {
        return (e, Vec::new());
    }
    let task = match resolve_task(app, &v).await {
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
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
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
                vec![crate::bot_chat::TaskRef {
                    id: next.id.clone(),
                    title: next.title.clone(),
                }],
            )
        }
        Err(e) => (format!("切换子任务状态失败：{e}"), Vec::new()),
    }
}

/// 绑定文件/文件夹：弹系统选择框由用户挑选，结果写回任务的 filePath/fileIsDir
async fn tool_bind_file(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let is_dir = v["isDir"].as_bool().unwrap_or(false);
    let task = match resolve_task(app, &v).await {
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
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!(
                    "已给任务「{}」绑定{}：{}",
                    next.title,
                    if is_dir { "文件夹" } else { "文件" },
                    path
                ),
                vec![crate::bot_chat::TaskRef {
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
async fn tool_link_file_to_task(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
    let task = match resolve_task(app, &v).await {
        Ok(t) => t,
        Err(e) => return (e, Vec::new()),
    };
    let mut next = task.clone();
    next.file_path = Some(path.clone());
    next.file_is_dir = Some(false);
    next.updated_at = Some(chrono::Utc::now().timestamp_millis());
    match crate::db::db_upsert(app.clone(), vec![next.clone()]).await {
        Ok(()) => {
            broadcast_after_mutation(app, vec![next.clone()], vec![]);
            (
                format!("已给任务「{}」绑定文件：{path}", next.title),
                vec![crate::bot_chat::TaskRef {
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
async fn extract_path_allowed(app: &AppHandle, path: &str) -> bool {
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
    if let Ok(tasks) = crate::db::db_load(app.clone()).await {
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

async fn tool_extract_document(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let path_opt = v["path"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // 模型直传 path 时白名单校验（无 path 走弹框，用户亲手选不受限）
    if let Some(p) = path_opt.as_deref() {
        if !extract_path_allowed(app, p).await {
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
async fn tool_create_word(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
async fn tool_create_word_revisions(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    // originalPath 由模型转述，同样过白名单（防回读任意文件）
    if let Some(op) = v["originalPath"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !extract_path_allowed(app, op).await {
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
async fn tool_create_excel(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
async fn tool_create_ppt(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
async fn tool_create_pdf(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
async fn tool_web_search(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
        &format!("web_search | query: {}", escape_for_log(&query, 100)),
    );
    match crate::bot_web::web_search(&query).await {
        Ok(results) => (results, Vec::new()),
        Err(e) => (format!("搜索失败：{e}"), Vec::new()),
    }
}

/// 抓取网页正文：http/https 公网地址，转纯文本回传（截 30000 字）
async fn tool_fetch_url(app: &AppHandle, args: &str) -> (String, Vec<crate::bot_chat::TaskRef>) {
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
        &format!("fetch_url | url: {}", escape_for_log(&u, 100)),
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
/// C4：py_exec_sync 是 sync 阻塞（最长 300s），必须经 async 包装挪到 blocking
/// 线程池，不得占住 async runtime worker（与 NEW-C-1 doc_* 同模式）。
/// NEW-C-4：透传 /stop 令牌，在途 Python 可被中断（StopGuard → owned StopToken）。
async fn tool_run_python(
    app: &AppHandle,
    args: &str,
    stop: Option<&crate::bot_slash::StopGuard>,
) -> (String, Vec<crate::bot_chat::TaskRef>) {
    let v = parse_args(args);
    let Some(code) = v["code"].as_str() else {
        return ("run_python 缺少 code".into(), Vec::new());
    };
    match crate::bot_py::py_exec_sync_async(
        app.clone(),
        code.to_string(),
        None,
        stop.map(|s| s.token()),
    )
    .await
    {
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

// ────────────────────────────────────────────────────────────────────
// 测试：BotConfig 序列化 + extract_document 输出格式化
// ────────────────────────────────────────────────────────────────────

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

/// F1（Phase 6b）单测：keyring 错误分类纯函数。
/// 真实 keychain 在测试环境不可用，用构造的 keyring::Error 注入故障。
#[cfg(test)]
mod f1_keyring_tests {
    use super::*;

    fn platform_failure() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "keychain locked",
        )))
    }

    #[test]
    fn read_api_key_keyring_failure_maps_to_keyring_error_variant() {
        // keyring 故障（钥匙串锁定/权限拒绝）→ KeyringError 结构化变体，不走 String 逃生舱
        let err = classify_get_password(Err(platform_failure())).unwrap_err();
        assert_eq!(err.code(), "KEYRING_ERROR");
        assert!(err.message().contains("读取 API Key 失败"));
        // error.rs 定义：KeyringError recoverable=false（用户需先解锁 keychain，重试无意义）
        assert!(!err.is_recoverable());
    }

    #[test]
    fn read_api_key_success_passes_value_through() {
        let key = classify_get_password(Ok("sk-test".into())).unwrap();
        assert_eq!(key, "sk-test");
    }

    #[test]
    fn has_api_key_no_entry_is_ok_false() {
        // 「key 不存在」不是故障：Ok(false)，前端显示「未配置 API Key」
        let r = classify_has_key(Err(keyring::Error::NoEntry)).unwrap();
        assert!(!r);
    }

    #[test]
    fn has_api_key_keyring_failure_not_swallowed_to_false() {
        // keyring 真实故障不得吞成 false（「反复填 key 仍失败无提示」假象）
        let err = classify_has_key(Err(platform_failure())).unwrap_err();
        assert_eq!(err.code(), "KEYRING_ERROR");
        assert!(err.message().contains("检查 API Key 失败"));
    }

    #[test]
    fn has_api_key_present_is_ok_true() {
        assert!(classify_has_key(Ok("sk-test".into())).unwrap());
    }
}

/// F3（Phase 6b）单测：bot_log_read 不再把读文件错吞成「暂无日志」。
#[cfg(test)]
mod f3_log_read_tests {
    use super::*;

    #[test]
    fn read_log_tail_missing_file_returns_placeholder() {
        // 文件不存在 = 日志确实没东西 → Ok 占位文案（前端按「暂无日志」显示）
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        assert_eq!(read_log_tail(&p, None).unwrap(), "（暂无日志）");
    }

    #[test]
    fn read_log_tail_io_error_not_swallowed() {
        // 路径是目录 → read_to_string 失败（非 NotFound）→ Err(IoError)，不静默吞
        let tmp = tempfile::tempdir().unwrap();
        let err = read_log_tail(tmp.path(), None).unwrap_err();
        assert_eq!(err.code(), "IO_ERROR");
        assert!(!err.is_recoverable());
    }

    #[test]
    fn read_log_tail_returns_reversed_tail_with_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bot.log");
        std::fs::write(&p, "l1\nl2\nl3\n").unwrap();
        assert_eq!(read_log_tail(&p, Some(2)).unwrap(), "l3\nl2");
        assert_eq!(read_log_tail(&p, None).unwrap(), "l3\nl2\nl1");
    }

    #[test]
    fn bot_log_read_fail_audit_line_written() {
        // ERROR 审计走 write_error_audit（泛型 Runtime，mock_app 跑同一条生产代码路径）。
        // generic_log_dir 探针命中测试二进制旁目录（target/debug/deps），读完即删。
        let app = tauri::test::mock_app();
        crate::audit::write_error_audit(app.handle(), "bot_log_read_fail", &[("err", "boom")]);
        let exe = std::env::current_exe().unwrap();
        let log = exe.parent().unwrap().join("bot.log");
        let content = std::fs::read_to_string(&log).unwrap();
        assert!(
            content.contains("bot_log_read_fail"),
            "ERROR 审计行应出现: {content}"
        );
        assert!(content.contains("boom"), "审计行应含 err 信息: {content}");
        let _ = std::fs::remove_file(&log);
    }
}
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
/// NEW-D-1 单测：早退路径审计事件序列（tool.call 配平 tool.return）。
/// execute_tool 是 Wry AppHandle 签名，无法 mock runtime 直调（见 tests/skill_e2e.rs 注释），
/// 故事件序列抽为纯函数 early_return_events，这里验证事件名/顺序/reason kv。
#[cfg(test)]
mod early_return_events_tests {
    use super::*;

    fn event_names(
        evs: &[(
            crate::audit::AuditLevel,
            &'static str,
            Vec<(&'static str, String)>,
        )],
    ) -> Vec<&'static str> {
        evs.iter().map(|(_, e, _)| *e).collect()
    }

    fn kv_get<'a>(kv: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
        kv.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn deny_path_emits_deny_then_balanced_tool_return() {
        // 拦截路径：pre_execute.deny → tool.return（顺序敏感），含 reason=denied / exit_code=none / duration_ms
        let evs = early_return_events("create_word_revisions", "denied", 3, None);
        assert_eq!(event_names(&evs), ["pre_execute.deny", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "tool"), Some("create_word_revisions"));
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "tool"), Some("create_word_revisions"));
        assert_eq!(kv_get(ret, "reason"), Some("denied"));
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert_eq!(kv_get(ret, "duration_ms"), Some("3"));
    }

    #[test]
    fn skill_step_error_path_warns_then_balanced_tool_return() {
        // 熔断路径：skill_on_step_error（补 Warn 可见性）→ tool.return(reason=skill_step_failed)
        let evs = early_return_events("run_python", "skill_step_failed", 5, Some("超过最大步数上限（8 步）"));
        assert_eq!(event_names(&evs), ["skill_on_step_error", "tool.return"]);
        assert!(evs
            .iter()
            .all(|(l, _, _)| *l == crate::audit::AuditLevel::Warn));
        assert_eq!(kv_get(&evs[0].2, "err"), Some("超过最大步数上限（8 步）"));
        let ret = &evs[1].2;
        assert_eq!(kv_get(ret, "reason"), Some("skill_step_failed"));
        assert_eq!(kv_get(ret, "exit_code"), Some("none"));
        assert!(kv_get(ret, "duration_ms").is_some());
    }
}
