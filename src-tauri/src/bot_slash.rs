//! Slash 命令与旁路基础设施：
//!
//! 主入口 `bot_chat`（在 bot_chat.rs）走 run_model_loop + 工具循环；
//! 这里集中「停止执行实例」「危险操作确认」「机器人总开关」三类旁路能力：
//! - `/stop` 快捷命令（bot_stop）：遍历 StopRegistry 把 interactive=true 的实例标志置位
//! - 弹窗确认（ConfirmMap）：删除任务等危险操作走 ask_user_confirm（两按钮）；
//!   文件访问授权走 ask_path_confirm（三按钮：允许一次/始终允许该目录/拒绝）
//! - 机器人总开关（bot_get_enabled / bot_set_enabled）：flag 文件持久化
//!
//! 设计目标：bot_chat / bot_execute_task / bot_scheduler 共享 StopGuard，
//! 切到这里后 bot_model_loop.rs 只 import StopGuard，不再持有底层注册表。

use crate::error::CommandResult;
use tauri::{AppHandle, Manager};

// /stop 停止标志（按执行实例隔离）

/// 活跃执行实例注册表：stop_id → (停止标志, 是否用户交互触发, 归属会话 id)
/// 注册表带会话：/stop 只停当前会话的实例，别的会话的 Skill/任务卡执行不受影响
// 停止注册表已进 `AppState`（`NEXT_STOP_ID` 留在 app_state 作全局发号器）；
// StopGuard 本体、/stop 的 interactive 会话筛选口径、Drop 注销语义全部未动。
use crate::app_state::{stop_registry, NEXT_STOP_ID};

/// 执行实例的停止标志：run_model_loop 在流式/工具循环检查点检查；Drop 时注销
/// （interactive=true 表示由用户聊天/点 🤖 触发，/stop 只停这类实例，不动后台定时）
/// 携带 session_id（触发会话；后台定时任务为 None），
/// 供流式事件会话标记 / Skill 与确认按会话归属过滤；interactive 供流式广播开关
/// （后台任务不发流式增量，防串进用户当前对话气泡）。
pub struct StopGuard {
    pub(crate) id: u64,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    interactive: bool,
    session_id: Option<String>,
    /// 注册表句柄（阶段 3.3）：与构造时取的注入实例是同一个 `Mutex`，
    /// Drop 里拿不到 `app`，靠这份 Arc 注销
    registry: std::sync::Arc<crate::app_state::StopMap>,
}

impl StopGuard {
    pub fn new<R: tauri::Runtime>(
        app: &tauri::AppHandle<R>,
        interactive: bool,
        session_id: Option<String>,
    ) -> Self {
        let id = NEXT_STOP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let registry = stop_registry(app);
        if let Ok(mut m) = registry.lock() {
            m.insert(id, (flag.clone(), interactive, session_id.clone()));
        }
        Self {
            id,
            flag,
            interactive,
            session_id,
            registry,
        }
    }

    pub fn stopped(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 是否用户交互触发（聊天 / 🤖 / 逐步执行）；后台定时任务为 false
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    /// 触发会话 id（后台定时任务为 None）：Skill/确认/逐步执行按此归属过滤
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// 派生停止令牌：与 guard 共享同一标志，但 owned + 'static，
    /// 可跨 spawn_blocking 边界传给 run_python（&StopGuard 借用无法进 'static 闭包）
    pub fn token(&self) -> StopToken {
        StopToken(self.flag.clone())
    }

    /// 强制置位自身标志：单测模拟 /stop 用（不经注册表，只停本实例）
    pub fn force_stop(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// StopGuard 的 'static 停止令牌：只读共享标志，供阻塞执行层轮询
#[derive(Clone)]
pub struct StopToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl StopToken {
    pub fn stopped(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 远程置位停止标志cancel_subagent 持令牌停对应 runner）
    pub fn stop(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 单测造一个可置位的停止令牌：唯一的生产构造路径是 `StopGuard::token()`，
    /// 而它要 AppHandle——编排层的等待单测没有 handle（只测等待循环本身）。
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )))
    }
}

impl Drop for StopGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.registry.lock() {
            m.remove(&self.id);
        }
    }
}

/// 应用退出：置位**全部**执行实例的停止标志——
/// 与 /stop（只停本会话 interactive 实例）不同，进程都要退了，不存在会话误伤；
/// 后台定时任务平时没有任何停止入口，退出是唯一叫停机会。返回置位数量。
pub fn stop_all_executions<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> usize {
    let registry = stop_registry(app);
    let Ok(m) = registry.lock() else {
        return 0;
    };
    let mut n = 0;
    for (flag, _, _) in m.values() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        n += 1;
    }
    n
}

/// 在途执行实例数（退出清理 drain 等待用；StopGuard Drop 时注销，归零 = 全部收尾完）
pub fn active_execution_count<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> usize {
    stop_registry(app).lock().map(|m| m.len()).unwrap_or(0)
}

// 原 `STOP_TEST_LOCK` 已删除。
// 理由：表随 `AppState` 注入，`stop_all_executions` / `active_execution_count`
// 都按传入的 app 取实例；相关用例（bot_slash 两个 + bot_py 一个 + lib.rs 退出清理）
// 各自注入独立实例，且**没有任何调用方再对兜底实例做全局广播**
// → 「并行置位别人 StopGuard」这条路已从结构上消失。

/// /stop 快捷命令：停止**当前会话**用户交互触发的执行（bot_chat / 🤖 任务卡执行），
/// 后台定时（interactive=false）与别的会话不受影响
/// （避免会话 B 的 /stop 误杀会话 A 的活动 Skill）
///
/// 返回置位的执行实例数：前端据此知道「停了几条」。原先 `map(|_| ())` 把计数
/// 丢了，前端只能靠「有没有在途」猜，停没停成功都拿不到。
#[tauri::command]
pub fn bot_stop(app: AppHandle, session_id: Option<String>) -> Result<usize, String> {
    // /stop 本身留痕——区分「用户停过」与「自己跑完」
    crate::bot::audit_log(
        &app,
        // session_id 前端传入，转义防日志伪造/多行撕裂
        &format!(
            "bot_stop | session: {}",
            crate::bot::truncate_for_log(session_id.as_deref().unwrap_or("<none>"), 60)
        ),
    );
    // Skill 调度器联动：强制终止本会话的活动技能（None = 全部会话）
    crate::bot_skills::skill_terminate_all(&app, "用户停止", session_id.as_deref());
    // 逐步执行联动：清掉本会话挂起的子任务确认
    let app2 = app.clone();
    let sid = session_id.clone();
    tauri::async_runtime::spawn(async move {
        crate::exec_steps::clear_for(&app2, sid.as_deref(), "/stop").await;
    });
    // 锁中毒返回 Err 给前端，不静默吞；
    // 确认弹窗收尾与注册表无关，锁失败也要照常执行
    let stopped = flag_session_stopped(&app, session_id.as_deref());
    // 本会话在途确认弹窗立即按拒绝收尾——sender 随条目 drop，
    // 等待侧 rx 立即收到 Err 走超时拒绝分支；/stop 后迟到的确认点击不再放行危险动作
    {
        let mut map = crate::db::lock::lock_or_recover(confirms(&app), "app_state::confirms");
        let keys: Vec<String> = map
            .iter()
            .filter(|(_, r)| r.session_id.as_deref() == session_id.as_deref())
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            map.remove(&k);
        }
    }
    // 置位数量原样带回前端（0 = 本会话没有可停的交互实例）
    stopped
}

/// /stop 置位内核：本会话交互实例的停止标志置位，返回置位数量。
/// 锁中毒返回 Err，前端可感知失败。
/// 抽成纯函数便于单测（tauri command 绑定 Wry AppHandle，mock_app 无法直调——
/// 与 bot_py.rs 的 resolve_doc_path 同先例）。
fn flag_session_stopped<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Option<&str>,
) -> Result<usize, String> {
    let registry = stop_registry(app);
    let m = registry
        .lock()
        .map_err(|_| "停止注册表锁中毒：停止标志未能置位".to_string())?;
    let mut n = 0;
    for (flag, interactive, sid2) in m.values() {
        // 会话隔离：只停本会话的交互实例；session 不匹配的不动
        if *interactive && sid2.as_deref() == session_id {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            n += 1;
        }
    }
    Ok(n)
}

// 危险操作确认（删除任务弹窗）

/// 授权弹窗的用户选择：
/// 删任务等二选一场景只用 Once/Deny；file_access 场景多一个 Always（始终允许该目录）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmChoice {
    /// 允许本次
    Once,
    /// 允许且把该目录写进 allowedDirs（仅 file_access 弹窗有此按钮）
    Always,
    /// 拒绝 / 超时 / 主窗口不可见 / 后台执行（安全兜底）
    Deny,
}

/// 前端回传：approved 放行与否 + always 是否「始终允许」（仅 file_access 弹窗会为 true）
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConfirmReply {
    approved: bool,
    always: bool,
}

// 阶段 3.1：待确认请求表（ConfirmMap / CONFIRMS / confirms）的定义已集中到
// `crate::app_state`。ConfirmReply 只是被 map 值类型引用到，定义留在这里，
// 可见性放宽到 pub(crate)（非公开 API），字段仍私有。
use crate::app_state::confirms;

/// 确认窗种类：危险动作确认（删除、装技能、覆盖文件、危险 UI 点击…）。
///
/// 这类确认会让本会话的活动技能转入暂停等确认——确认通过后按技能自身的
/// resumable 语义恢复或终止。
pub const CONFIRM_KIND_DANGER: &str = "danger";

/// 确认窗种类：文件访问授权（白名单外路径的例行权限询问）。
///
/// 这是**环境权限**，不是技能的风险节点：弹窗期间工具调用本身阻塞，技能不会
/// 并发推进，不需要靠暂停冻结。故它不触发技能暂停。
pub const CONFIRM_KIND_FILE_ACCESS: &str = "file_access";

/// 弹窗确认公共内核：发 "bot-confirm" 事件（带 kind 供前端渲染两/三按钮、
/// 带 sessionId 供前端按会话过滤），60s 超时默认拒绝（安全兜底）；
/// 非交互执行（后台定时任务，interactive=false）直接拒绝不弹窗——
/// 无人在场时弹窗只会串进用户当前会话且必然超时。
/// 主窗口不可见时同样直接拒绝，不白等 60s。
async fn ask_confirm_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool: &str,
    detail: &str,
    kind: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ConfirmReply {
    let deny = ConfirmReply {
        approved: false,
        always: false,
    };
    if !interactive {
        crate::bot::audit_log(
            app,
            &format!(
                "confirm_skipped | {tool} | {} | 后台执行不弹窗，默认拒绝",
                crate::bot::truncate_for_log(detail, 120)
            ),
        );
        return deny;
    }
    // 确认窗 id 先造出来：技能暂停要记它，确认结果只认这个 id
    // （认错暂停被误恢复/误终止，比不恢复更糟）。
    let id = uuid::Uuid::new_v4().simple().to_string();
    // Skill 调度器联动：**只有危险动作确认**才把本会话活动技能转入 Paused。
    //
    // 文件访问授权是环境的例行权限询问，不是技能的风险节点——两类确认接到
    // 同一个钩子上，等于让「读一个白名单外路径」拥有杀技能的权力：技能不支持
    // 断点续跑时（resumable=false）确认通过后即终止，用户点一次「允许」就把
    // 整段流程连同已做的工作一起作废。
    if kind == CONFIRM_KIND_DANGER {
        crate::bot_skills::skill_mark_paused(app, tool, session_id, &id);
    }
    // 确认弹窗只由主窗口的 ConfirmMap 呈现（bot-confirm 全窗广播 + ConfirmMap 挂主窗口）。
    // 主窗口不可见时没人能点确认，等满超时只会白等一轮——直接按默认拒绝走。
    let main_visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !main_visible {
        crate::bot::audit_log(
            app,
            &format!(
                "confirm_skipped | {tool} | {} | 主窗口不可见，默认拒绝",
                crate::bot::truncate_for_log(detail, 120)
            ),
        );
        crate::bot_skills::skill_confirm_result(app, false, session_id, &id);
        return deny;
    }
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    crate::db::lock::lock_or_recover(confirms(app), "bot_slash confirms").insert(
        id.clone(),
        crate::app_state::PendingConfirmRequest {
            tx,
            session_id: session_id.map(|s| s.to_string()),
            tool: tool.to_string(),
            detail: detail.to_string(),
            kind: kind.to_string(),
        },
    );
    // 走统一出口盖章 + 进回放缓冲：webview 重载后挂载的 ConfirmMap 按
    // bot_pending_confirms 重查这张表还原弹窗，事件丢了也恢复得回来
    crate::bot::stream_log::emit_recorded(
        app,
        session_id,
        "bot-confirm",
        serde_json::json!({
            "id": id,
            "tool": tool,
            "detail": detail,
            "kind": kind,
        }),
    );
    crate::bot::audit_log(
        app,
        &format!(
            "confirm | id: {} | kind: {kind} | {tool} | {}",
            id.chars().take(8).collect::<String>(),
            crate::bot::truncate_for_log(detail, 120)
        ),
    );
    // 500ms 粒度同时盯应答与 /stop：用户按停止后这一步不该再挂满 60s
    //（engine_ask_user 同口径）。先收应答后判停止/超时，同时就绪时答案优先。
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut stopped = false;
    let reply = loop {
        // oneshot 关闭（/stop 已把条目摘走）与超时同走拒绝分支
        if let Ok(Ok(reply)) =
            tokio::time::timeout(std::time::Duration::from_millis(500), &mut rx).await
        {
            break Some(reply);
        }
        let hit_stop = session_id
            .map(|sid| is_session_stopped(app, sid))
            .unwrap_or(false);
        if hit_stop {
            stopped = true;
            break None;
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
    };
    match reply {
        Some(reply) => reply,
        None => {
            crate::db::lock::lock_or_recover(confirms(app), "bot_slash confirms").remove(&id);
            let reason = if stopped { "/stop" } else { "60s 无响应" };
            // 确认超时/停止默认拒绝留痕
            crate::bot::audit_log(
                app,
                &format!(
                    "confirm_timeout | {tool} | {} | {reason}，默认拒绝",
                    crate::bot::truncate_for_log(detail, 120)
                ),
            );
            crate::bot_skills::skill_confirm_result(app, false, session_id, &id); // 超时默认拒绝
            deny
        }
    }
}

/// 在途确认列表（前端 ConfirmMap 挂载时重查还原弹窗用）。
#[tauri::command]
pub fn bot_pending_confirms<R: tauri::Runtime>(app: AppHandle<R>) -> Vec<PendingConfirmView> {
    let map = crate::db::lock::lock_or_recover(confirms(&app), "bot_slash confirms");
    map.iter()
        .map(|(id, r)| PendingConfirmView {
            id: id.clone(),
            tool: r.tool.clone(),
            detail: r.detail.clone(),
            kind: r.kind.clone(),
            session_id: r.session_id.clone(),
        })
        .collect()
}

/// `bot_pending_confirms` 的行载荷（serde camelCase 与前端对齐）。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingConfirmView {
    pub id: String,
    pub tool: String,
    pub detail: String,
    pub kind: String,
    pub session_id: Option<String>,
}

/// 某会话是否有执行实例已被置位停止：/stop 后的等待类操作（确认弹窗等）
/// 不持有 StopGuard，只能按会话号反查注册表。
fn is_session_stopped<R: tauri::Runtime>(app: &tauri::AppHandle<R>, session_id: &str) -> bool {
    stop_registry(app)
        .lock()
        .map(|m| {
            m.values().any(|(flag, _, sid)| {
                flag.load(std::sync::atomic::Ordering::SeqCst) && sid.as_deref() == Some(session_id)
            })
        })
        .unwrap_or(false)
}

/// 请求用户确认危险操作（如删除任务）；60s 超时默认拒绝（安全兜底）
pub async fn ask_user_confirm<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool: &str,
    detail: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> bool {
    ask_confirm_inner(
        app,
        tool,
        detail,
        CONFIRM_KIND_DANGER,
        interactive,
        session_id,
    )
    .await
    .approved
}

/// 文件访问授权（Kimi CLI 风格）：白名单外路径弹三选一窗
/// （允许一次 / 始终允许该目录 / 拒绝）。主窗口不可见/超时/后台执行 → Deny。
pub async fn ask_path_confirm<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    tool: &str,
    detail: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ConfirmChoice {
    match ask_confirm_inner(
        app,
        tool,
        detail,
        CONFIRM_KIND_FILE_ACCESS,
        interactive,
        session_id,
    )
    .await
    {
        ConfirmReply {
            approved: true,
            always: true,
        } => ConfirmChoice::Always,
        ConfirmReply {
            approved: true,
            always: false,
        } => ConfirmChoice::Once,
        _ => ConfirmChoice::Deny,
    }
}

/// 确认响应：允许/拒绝（前端点击后回传）；always 仅 file_access 弹窗的
/// 「始终允许该目录」按钮会为 true，老调用（删任务两按钮）不传 → None → false。
/// 会话归属从 ConfirmMap 条目取回：skill_confirm_result 按会话过滤。
#[tauri::command]
pub fn bot_confirm_response(
    app: AppHandle,
    request_id: String,
    approved: bool,
    always: Option<bool>,
) -> Result<(), String> {
    let entry = take_confirm(&app, &request_id)?;
    let (tx, session_id) = (entry.tx, entry.session_id);
    // Skill 调度器联动：确认结果 → 本会话技能恢复 Running / 拒绝终止 / 暂停即终止
    // 确认窗绑定：request_id 就是产生这次暂停的确认窗 id——别的确认窗的答案
    // 碰巧落到这里时不会误伤这条暂停。
    crate::bot_skills::skill_confirm_result(&app, approved, session_id.as_deref(), &request_id);
    // 用户点「拒绝」留痕
    if !approved {
        crate::bot::audit_log(
            &app,
            &format!(
                "confirm_denied | id: {} | 用户拒绝",
                request_id.chars().take(8).collect::<String>()
            ),
        );
    }
    deliver_confirm(
        tx,
        ConfirmReply {
            approved,
            always: always.unwrap_or(false),
        },
    )
}

/// 取待确认条目（不存在/已超时 → Err）。
fn take_confirm<R: tauri::Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
) -> Result<crate::app_state::PendingConfirmRequest, String> {
    crate::db::lock::lock_or_recover(confirms(app), "bot_slash confirms")
        .remove(request_id)
        .ok_or_else(|| {
            format!(
                "确认请求不存在或已超时：{}",
                request_id.chars().take(8).collect::<String>()
            )
        })
}

/// 回填确认结果（等待方已退出时返回 Err）。
fn deliver_confirm(
    tx: tokio::sync::oneshot::Sender<ConfirmReply>,
    reply: ConfirmReply,
) -> Result<(), String> {
    tx.send(reply)
        .map_err(|_| "确认结果送达失败：等待方已退出（可能已超时）".to_string())
}

// 开关持久化

fn bot_flag_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    crate::paths::flags_dir(app).join("bot-enabled.flag")
}

/// 机器人聊天开关读取（泛型 Runtime，mock runtime 可调）
pub(crate) fn bot_enabled<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    bot_flag_path(app).exists()
}

/// 机器人聊天当前是否开启
#[tauri::command]
pub fn bot_get_enabled(app: AppHandle) -> bool {
    bot_enabled(&app)
}

/// 设置机器人聊天开关（写/删 flag，返回生效后的状态）
#[tauri::command]
pub fn bot_set_enabled(app: AppHandle, enabled: bool) -> CommandResult<bool> {
    let dir = crate::paths::flags_dir(&app);
    std::fs::create_dir_all(&dir)?;
    if enabled {
        std::fs::write(bot_flag_path(&app), b"1")?;
    } else {
        // 承认语义：flag 文件本就不存在 = 禁用目标已达成，非吞错
        // （与 keyring.rs PlaintextFile 分支 Err(NotFound) => Ok(()) 同款）
        match std::fs::remove_file(bot_flag_path(&app)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(enabled)
}
/// 测试用 mock handle：注入独立 `AppState`（停止注册表/确认表随 AppState 隔离）
#[cfg(test)]
fn test_handle() -> tauri::AppHandle<tauri::test::MockRuntime> {
    let app = tauri::test::mock_app();
    app.manage(crate::app_state::AppState::default());
    app.handle().clone()
}

#[cfg(test)]
mod stop_all_tests {
    /// 退出置位必须同时覆盖 interactive 与后台（interactive=false）
    /// 两类实例——/stop 只停交互实例，退出清理不能漏掉后台任务
    #[test]
    fn stop_all_covers_interactive_and_background() {
        // 表随 AppState，本用例用独立实例 → 不共享全局态，无需串行锁
        let app = super::test_handle();
        let g1 = super::StopGuard::new(&app, true, Some("batch5-s1".into()));
        let g2 = super::StopGuard::new(&app, false, None);
        // 精确计数：注入实例只装本用例两个守卫——若实例被共享，别的并行用例
        // 注册的 StopGuard 会让这里 > 2（这就是「隔离」的可观测证据）
        assert_eq!(super::active_execution_count(&app), 2);
        let n = super::stop_all_executions(&app);
        assert_eq!(n, 2, "只应置位本实例的两个守卫");
        assert!(g1.stopped() && g2.stopped(), "两类实例都必须被置位");
    }
}

#[cfg(test)]
mod command_result_tests {
    // 注入式 mock handle 统一在文件作用域定义
    use super::test_handle as mock_handle;

    /// 接线守卫：bot_stop 必须把置位数量返回给前端。
    ///
    /// 原先把计数 `map(|_| ())` 丢了：前端只能靠「有没有在途」猜，停没停
    /// 成功、停了几条都拿不到。命令绑定 Wry AppHandle 没法直接单测，故钉源码。
    /// 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）。
    #[test]
    fn bot_stop_returns_the_stopped_count() {
        let src = include_str!("bot_slash.rs");
        assert!(
            !src.contains(concat!("stopped.map(|_| (", "))")),
            "bot_stop 不得丢弃置位数量"
        );
        assert!(
            src.contains(concat!(
                "pub fn bot_stop(app: AppHandle, session_id: Option<String>) -> Result<usize, ",
                "String>"
            )),
            "bot_stop 的返回类型必须是置位数量"
        );
    }

    /// /stop 置位内核只停本会话交互实例并返回数量；
    /// 锁中毒路径经 map_err 返回 Err（命令绑定 Wry AppHandle 无法单测，测内核）。
    #[test]
    fn flag_session_stopped_only_hits_own_session_interactive() {
        // 注入独立实例，不再与别的持 StopGuard 用例互斥
        let app = mock_handle();
        let g1 = super::StopGuard::new(&app, true, Some("t1-3-sess".into()));
        let g2 = super::StopGuard::new(&app, true, Some("t1-3-other".into()));
        let g3 = super::StopGuard::new(&app, false, Some("t1-3-sess".into()));
        let n = super::flag_session_stopped(&app, Some("t1-3-sess")).expect("锁正常应 Ok");
        assert_eq!(n, 1, "只应置位本会话的交互实例");
        assert!(g1.stopped());
        assert!(!g2.stopped(), "别的会话不受影响");
        assert!(!g3.stopped(), "后台实例不受影响");
    }

    /// 不存在/已超时的确认请求 → Err
    #[test]
    fn take_confirm_unknown_id_errs() {
        let e = super::take_confirm(&mock_handle(), "t1-3-no-such-id").unwrap_err();
        assert!(e.contains("不存在或已超时"), "Err 应说明原因：{e}");
    }

    /// 待确认表迁入 `AppState` 后，注入实例之间必须隔离
    /// （迁移前是进程级全局表，`cargo test --lib` 同进程并行用例会互相看见对方条目）。
    #[test]
    fn confirm_requests_isolated_between_injected_instances() {
        let a = mock_handle();
        let b = mock_handle();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        crate::db::lock::lock_or_recover(super::confirms(&a), "bot_slash confirms").insert(
            "req-iso".into(),
            crate::app_state::PendingConfirmRequest {
                tx,
                session_id: Some("sess-1".into()),
                tool: "test_tool".into(),
                detail: "d".into(),
                kind: super::CONFIRM_KIND_DANGER.into(),
            },
        );
        assert_eq!(
            crate::db::lock::lock_or_recover(super::confirms(&a), "bot_slash confirms").len(),
            1
        );
        assert_eq!(
            crate::db::lock::lock_or_recover(super::confirms(&b), "bot_slash confirms").len(),
            0,
            "另一个注入实例不得看到该条目"
        );
        // take_confirm 走注入实例：取走后本实例为空
        let taken = super::take_confirm(&a, "req-iso");
        assert!(taken.is_ok(), "注入实例内应能取到");
        assert_eq!(
            crate::db::lock::lock_or_recover(super::confirms(&a), "bot_slash confirms").len(),
            0
        );
    }

    /// 等待方已退出（rx dropped）时回填失败必须可见
    #[test]
    fn deliver_confirm_dropped_receiver_errs() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        drop(rx);
        let r = super::deliver_confirm(
            tx,
            super::ConfirmReply {
                approved: true,
                always: false,
            },
        );
        assert!(r.is_err(), "送达失败应返回 Err");
        // 对照：rx 存活时正常送达
        let (tx, rx) = tokio::sync::oneshot::channel();
        super::deliver_confirm(
            tx,
            super::ConfirmReply {
                approved: false,
                always: true,
            },
        )
        .expect("rx 存活应送达");
        let reply = rx.blocking_recv().unwrap();
        assert!(!reply.approved && reply.always);
    }
}

#[cfg(test)]
mod confirm_skill_pause_tests {
    use super::test_handle as mock_handle;

    /// 文件访问授权（白名单外路径的例行权限询问）不得把活动技能转入暂停。
    ///
    /// 回归背景：文件访问授权与危险动作确认复用了同一个确认内核，内核无条件
    /// 触发 skill_mark_paused —— 不支持断点续跑的技能（resumable=false）在用户
    /// 点「允许」之后直接终止，整段流程连同已做的工作一起作废。
    ///
    /// 用 mock handle 走「主窗口不可见 → 直接拒绝」分支：确认被拒也不该
    /// 暂停/终止技能（拒绝路径同样经过 skill_confirm_result）。
    #[tokio::test]
    async fn file_access_confirm_leaves_skill_running() {
        let app = mock_handle();
        let mut run = crate::bot_skills::test_run(8);
        run.name = "confirm-pause-file-access".into();
        run.state = crate::bot_skills::SkillState::Running;
        run.resumable = false; // 事故机型：暂停即终止
        run.session_id = Some("sess-file-access".into());
        let runs = crate::bot_skills::skill_runs(&app);
        crate::db::lock::lock_or_recover(&runs, "bot_slash confirm_skill_pause_tests")
            .insert(run.name.clone(), run);

        let choice = super::ask_path_confirm(
            &app,
            "list_files",
            "访问白名单外路径：C:\\Users\\x\\AppData\\Local\\Temp",
            true,
            Some("sess-file-access"),
        )
        .await;
        assert!(
            matches!(choice, super::ConfirmChoice::Deny),
            "mock handle 没有 main 窗口，应按默认拒绝收场"
        );

        let runs = crate::bot_skills::skill_runs(&app);
        let state = crate::db::lock::lock_or_recover(&runs, "bot_slash confirm_skill_pause_tests")
            .get("confirm-pause-file-access")
            .map(|r| r.state.clone());
        assert_eq!(
            state,
            Some(crate::bot_skills::SkillState::Running),
            "文件访问授权不得暂停（更不得终止）活动技能"
        );
    }

    /// 危险动作确认仍要暂停技能——文件访问授权分流不能把这条也放跑。
    #[tokio::test]
    async fn danger_confirm_pauses_skill() {
        let app = mock_handle();
        let mut run = crate::bot_skills::test_run(8);
        run.name = "confirm-pause-danger".into();
        run.state = crate::bot_skills::SkillState::Running;
        run.session_id = Some("sess-danger".into());
        // 确认超时的裁决看 started_at_ms：test_run 的初值是假 epoch，
        // 按真实 now 起算才会走「未超时」分支
        run.started_at_ms = crate::db::now_ms();
        let runs = crate::bot_skills::skill_runs(&app);
        crate::db::lock::lock_or_recover(&runs, "bot_slash confirm_skill_pause_tests")
            .insert(run.name.clone(), run);

        // 危险确认被拒 → 技能终止（回滚语义），说明确认确实作用到了技能
        let approved = super::ask_user_confirm(
            &app,
            "delete_task",
            "删除任务：演示",
            true,
            Some("sess-danger"),
        )
        .await;
        assert!(!approved, "mock handle 无 main 窗口，危险确认应被拒");

        let runs = crate::bot_skills::skill_runs(&app);
        let state = crate::db::lock::lock_or_recover(&runs, "bot_slash confirm_skill_pause_tests")
            .get("confirm-pause-danger")
            .map(|r| r.state.clone());
        assert_eq!(
            state,
            Some(crate::bot_skills::SkillState::Terminated),
            "危险动作确认被拒 → 技能终止（回滚语义）"
        );
    }
}
