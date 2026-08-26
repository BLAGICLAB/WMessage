//! Slash 命令与旁路基础设施（F-6 step 5 拆分 2026-08-18）：
//!
//! 主入口 `bot_chat`（在 bot_chat.rs）走 run_model_loop + 工具循环；
//! 这里集中「停止执行实例」「危险操作确认」「机器人总开关」三类旁路能力：
//! - `/stop` 快捷命令（bot_stop）：遍历 StopRegistry 把 interactive=true 的实例标志置位
//! - 弹窗确认（ConfirmMap）：挂件删除任务等危险操作走 ask_user_confirm（两按钮）；
//!   文件访问授权走 ask_path_confirm（三按钮：允许一次/始终允许该目录/拒绝，2026-08-26）
//! - 机器人总开关（bot_get_enabled / bot_set_enabled）：flag 文件持久化
//!
//! 设计目标：bot_chat / bot_execute_task / bot_scheduler 共享 StopGuard，
//! 切到这里后 bot_model_loop.rs 只 import StopGuard，不再持有底层注册表。

use crate::error::CommandResult;
use tauri::{AppHandle, Emitter, Manager};

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
pub struct StopGuard {
    pub(crate) id: u64,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl StopGuard {
    pub fn new(interactive: bool) -> Self {
        let id = NEXT_STOP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Ok(mut m) = stop_registry().lock() {
            m.insert(id, (flag.clone(), interactive));
        }
        Self { id, flag }
    }

    pub fn stopped(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 派生停止令牌（NEW-C-4）：与 guard 共享同一标志，但 owned + 'static，
    /// 可跨 spawn_blocking 边界传给 run_python（&StopGuard 借用无法进 'static 闭包）
    pub fn token(&self) -> StopToken {
        StopToken(self.flag.clone())
    }

    /// 强制置位自身标志：单测模拟 /stop 用（不经注册表，只停本实例）
    pub fn force_stop(&self) {
        self.flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// StopGuard 的 'static 停止令牌（NEW-C-4）：只读共享标志，供阻塞执行层轮询
#[derive(Clone)]
pub struct StopToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl StopToken {
    pub fn stopped(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
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
    // 逐步执行联动：清掉挂起的子任务确认（2026-08-19 exec_steps）
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::exec_steps::clear(&app2, "/stop").await;
    });
    if let Ok(m) = stop_registry().lock() {
        for (_, (flag, interactive)) in m.iter() {
            if *interactive {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
}

// ───────────────────────── 危险操作确认（删除任务弹窗） ─────────────────────────

/// 授权弹窗的用户选择（2026-08-26 文件访问授权改造）：
/// 旧场景（删任务等二选一）只用 Once/Deny；file_access 场景多一个 Always（始终允许该目录）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmChoice {
    /// 允许本次
    Once,
    /// 允许且把该目录写进 allowedDirs（仅 file_access 弹窗有此按钮）
    Always,
    /// 拒绝 / 超时 / 挂件不可见（安全兜底）
    Deny,
}

/// 前端回传：approved 放行与否 + always 是否「始终允许」（仅 file_access 弹窗会为 true）
#[derive(Debug, Clone, Copy)]
struct ConfirmReply {
    approved: bool,
    always: bool,
}

/// 待确认请求：id → oneshot 通道（挂件 bot_confirm_response 回填）
type ConfirmMap =
    std::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<ConfirmReply>>>;
static CONFIRMS: std::sync::OnceLock<ConfirmMap> = std::sync::OnceLock::new();

fn confirms() -> &'static ConfirmMap {
    CONFIRMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 弹窗确认公共内核：发 "bot-confirm" 事件（带 kind 供前端渲染两/三按钮），
/// 60s 超时默认拒绝（安全兜底）；挂件不可见时直接拒绝，不白等 60s（审计 P2）。
async fn ask_confirm_inner(app: &AppHandle, tool: &str, detail: &str, kind: &str) -> ConfirmReply {
    // Skill 调度器联动：高危动作确认开始 → 活动技能转入 Paused
    crate::bot_skills::skill_mark_paused(app, tool);
    let widget_visible = app
        .get_webview_window("widget")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    let deny = ConfirmReply { approved: false, always: false };
    if !widget_visible {
        crate::bot::audit_log(
            app,
            &format!("confirm_skipped | {tool} | {detail} | 挂件不可见，默认拒绝"),
        );
        crate::bot_skills::skill_confirm_result(app, false);
        return deny;
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
        serde_json::json!({ "id": id, "tool": tool, "detail": detail, "kind": kind }),
    );
    crate::bot::audit_log(
        app,
        &format!("confirm | id: {} | kind: {kind} | {tool} | {detail}", &id[..8]),
    );
    match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
        Ok(Ok(reply)) => reply,
        _ => {
            confirms()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            crate::bot_skills::skill_confirm_result(app, false); // 超时默认拒绝
            deny
        }
    }
}

/// 请求用户在挂件确认危险操作（如删除任务）；60s 超时默认拒绝（安全兜底）
pub async fn ask_user_confirm(app: &AppHandle, tool: &str, detail: &str) -> bool {
    ask_confirm_inner(app, tool, detail, "danger").await.approved
}

/// 文件访问授权（2026-08-26，Kimi CLI 风格）：白名单外路径弹三选一窗
/// （允许一次 / 始终允许该目录 / 拒绝）。挂件不可见/超时 → Deny。
pub async fn ask_path_confirm(app: &AppHandle, tool: &str, detail: &str) -> ConfirmChoice {
    match ask_confirm_inner(app, tool, detail, "file_access").await {
        ConfirmReply { approved: true, always: true } => ConfirmChoice::Always,
        ConfirmReply { approved: true, always: false } => ConfirmChoice::Once,
        _ => ConfirmChoice::Deny,
    }
}

/// 挂件确认响应：允许/拒绝（前端点击后回传）；always 仅 file_access 弹窗的
/// 「始终允许该目录」按钮会为 true，老调用（删任务两按钮）不传 → None → false。
#[tauri::command]
pub fn bot_confirm_response(app: AppHandle, request_id: String, approved: bool, always: Option<bool>) {
    // Skill 调度器联动：确认结果 → 恢复 Running / 拒绝终止 / 暂停即终止
    crate::bot_skills::skill_confirm_result(&app, approved);
    if let Some(tx) = confirms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&request_id)
    {
        let _ = tx.send(ConfirmReply { approved, always: always.unwrap_or(false) });
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