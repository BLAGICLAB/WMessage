//! Slash 命令与旁路基础设施：
//!
//! 主入口 `bot_chat`（在 bot_chat.rs）走 run_model_loop + 工具循环；
//! 这里集中「停止执行实例」「危险操作确认」「机器人总开关」三类旁路能力：
//! - `/stop` 快捷命令（bot_stop）：遍历 StopRegistry 把 interactive=true 的实例标志置位
//! - 弹窗确认（ConfirmMap）：挂件删除任务等危险操作走 ask_user_confirm（两按钮）；
//!   文件访问授权走 ask_path_confirm（三按钮：允许一次/始终允许该目录/拒绝）
//! - 机器人总开关（bot_get_enabled / bot_set_enabled）：flag 文件持久化
//!
//! 设计目标：bot_chat / bot_execute_task / bot_scheduler 共享 StopGuard，
//! 切到这里后 bot_model_loop.rs 只 import StopGuard，不再持有底层注册表。

use crate::error::CommandResult;
use tauri::{AppHandle, Emitter, Manager};

// ───────────────────────── /stop 停止标志（按执行实例隔离） ─────────────────────────

/// 活跃执行实例注册表：stop_id → (停止标志, 是否用户交互触发, 归属会话 id)
/// 注册表带会话：/stop 只停当前会话的实例，别的会话的 Skill/任务卡执行不受影响
type StopMap = std::sync::Mutex<
    std::collections::HashMap<u64, (std::sync::Arc<std::sync::atomic::AtomicBool>, bool, Option<String>)>,
>;
static STOP_REGISTRY: std::sync::OnceLock<StopMap> = std::sync::OnceLock::new();
static NEXT_STOP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn stop_registry() -> &'static StopMap {
    STOP_REGISTRY.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 执行实例的停止标志：run_model_loop 在流式/工具循环检查点检查；Drop 时注销
/// （interactive=true 表示由用户聊天/点 🤖 触发，/stop 只停这类实例，不动后台定时）
/// 携带 session_id（触发会话；后台定时任务为 None），
/// 供流式事件会话标记 / Skill 与确认按会话归属过滤；interactive 供流式广播开关
/// （后台任务不向挂件发流式增量，防串进用户当前对话气泡）。
pub struct StopGuard {
    pub(crate) id: u64,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    interactive: bool,
    session_id: Option<String>,
    /// 任务卡执行流程（run_task_in_chat / exec_steps）标记：
    /// 该流程是「内置编排流」，与 Skill 同级——EXECUTE_SYSTEM_PROMPT 要求调
    /// link_file_to_task 原子工具收尾，没有活动 SkillRun
    /// 开门会被 AtomicGuard 硬拦（prompt 要求的核心动作被自家网关否决）。
    allow_atomic: bool,
}

impl StopGuard {
    pub fn new(interactive: bool, session_id: Option<String>) -> Self {
        Self::with_atomic(interactive, session_id, false)
    }

    /// 任务卡执行流程专用：放行原子工具（视为内置编排流，等价于 Skill Running 上下文）
    pub fn new_task_exec(interactive: bool, session_id: Option<String>) -> Self {
        Self::with_atomic(interactive, session_id, true)
    }

    fn with_atomic(interactive: bool, session_id: Option<String>, allow_atomic: bool) -> Self {
        let id = NEXT_STOP_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        if let Ok(mut m) = stop_registry().lock() {
            m.insert(id, (flag.clone(), interactive, session_id.clone()));
        }
        Self { id, flag, interactive, session_id, allow_atomic }
    }

    /// 是否放行原子工具（仅任务卡执行流程为 true）
    pub(crate) fn allow_atomic(&self) -> bool {
        self.allow_atomic
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
        self.flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// StopGuard 的 'static 停止令牌：只读共享标志，供阻塞执行层轮询
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

/// 应用退出：置位**全部**执行实例的停止标志——
/// 与 /stop（只停本会话 interactive 实例）不同，进程都要退了，不存在会话误伤；
/// 后台定时任务平时没有任何停止入口，退出是唯一叫停机会。返回置位数量。
pub fn stop_all_executions() -> usize {
    let Ok(m) = stop_registry().lock() else {
        return 0;
    };
    let mut n = 0;
    for (_, (flag, _, _)) in m.iter() {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        n += 1;
    }
    n
}

/// 在途执行实例数（退出清理 drain 等待用；StopGuard Drop 时注销，归零 = 全部收尾完）
pub fn active_execution_count() -> usize {
    stop_registry().lock().map(|m| m.len()).unwrap_or(0)
}

/// STOP_REGISTRY 测试串行锁。
/// stop_all_executions() 是无差别全局广播（置位全进程所有已注册 StopGuard），
/// 与「持有 StopGuard 且断言停止前行为」的测试（如 bot_py 的 StopReader 用例）
/// 并行时互相干扰随机挂。这两类测试都必须全程持此锁。
#[cfg(test)]
pub(crate) static STOP_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// /stop 快捷命令：停止**当前会话**用户交互触发的执行（bot_chat / 🤖 任务卡执行），
/// 后台定时（interactive=false）与别的会话不受影响
/// （避免会话 B 的 /stop 误杀会话 A 的活动 Skill）
#[tauri::command]
pub fn bot_stop(app: AppHandle, session_id: Option<String>) -> Result<(), String> {
    // /stop 本身留痕——区分「用户停过」与「自己跑完」
    crate::bot::audit_log(
        &app,
        // session_id 前端传入，转义防日志伪造/多行撕裂
        &format!("bot_stop | session: {}", crate::bot::truncate_for_log(session_id.as_deref().unwrap_or("<none>"), 60)),
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
    let stopped = flag_session_stopped(session_id.as_deref());
    // 本会话在途确认弹窗立即按拒绝收尾——sender 随条目 drop，
    // 等待侧 rx 立即收到 Err 走超时拒绝分支；/stop 后迟到的确认点击不再放行危险动作
    {
        let mut map = confirms().lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<String> = map
            .iter()
            .filter(|(_, (_, sid))| sid.as_deref() == session_id.as_deref())
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            map.remove(&k);
        }
    }
    stopped.map(|_| ())
}

/// /stop 置位内核：本会话交互实例的停止标志置位，返回置位数量。
/// 锁中毒返回 Err，前端可感知失败。
/// 抽成纯函数便于单测（tauri command 绑定 Wry AppHandle，mock_app 无法直调——
/// 与 bot_py.rs 的 resolve_doc_path 同先例）。
fn flag_session_stopped(session_id: Option<&str>) -> Result<usize, String> {
    let m = stop_registry()
        .lock()
        .map_err(|_| "停止注册表锁中毒：停止标志未能置位".to_string())?;
    let mut n = 0;
    for (_, (flag, interactive, sid2)) in m.iter() {
        // 会话隔离：只停本会话的交互实例；session 不匹配的不动
        if *interactive && sid2.as_deref() == session_id {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            n += 1;
        }
    }
    Ok(n)
}

// ───────────────────────── 危险操作确认（删除任务弹窗） ─────────────────────────

/// 授权弹窗的用户选择：
/// 删任务等二选一场景只用 Once/Deny；file_access 场景多一个 Always（始终允许该目录）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmChoice {
    /// 允许本次
    Once,
    /// 允许且把该目录写进 allowedDirs（仅 file_access 弹窗有此按钮）
    Always,
    /// 拒绝 / 超时 / 挂件不可见 / 后台执行（安全兜底）
    Deny,
}

/// 前端回传：approved 放行与否 + always 是否「始终允许」（仅 file_access 弹窗会为 true）
#[derive(Debug, Clone, Copy)]
struct ConfirmReply {
    approved: bool,
    always: bool,
}

/// 待确认请求：id → (oneshot 通道, 归属会话 id)（挂件 bot_confirm_response 回填；
/// 会话 id 随 bot-confirm 事件下发，前端非当前会话不弹窗，
/// 后台任务（session=None）确认直接拒绝不弹窗）
type ConfirmMap = std::sync::Mutex<
    std::collections::HashMap<String, (tokio::sync::oneshot::Sender<ConfirmReply>, Option<String>)>,
>;
static CONFIRMS: std::sync::OnceLock<ConfirmMap> = std::sync::OnceLock::new();

fn confirms() -> &'static ConfirmMap {
    CONFIRMS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 弹窗确认公共内核：发 "bot-confirm" 事件（带 kind 供前端渲染两/三按钮、
/// 带 sessionId 供前端按会话过滤），60s 超时默认拒绝（安全兜底）；
/// 非交互执行（后台定时任务，interactive=false）直接拒绝不弹窗——
/// 无人在场时弹窗只会串进用户当前会话且必然超时。
/// 挂件不可见时同样直接拒绝，不白等 60s。
async fn ask_confirm_inner(
    app: &AppHandle,
    tool: &str,
    detail: &str,
    kind: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ConfirmReply {
    let deny = ConfirmReply { approved: false, always: false };
    if !interactive {
        crate::bot::audit_log(
            app,
            &format!("confirm_skipped | {tool} | {} | 后台执行不弹窗，默认拒绝", crate::bot::truncate_for_log(detail, 120)),
        );
        return deny;
    }
    // Skill 调度器联动：高危动作确认开始 → 本会话活动技能转入 Paused
    crate::bot_skills::skill_mark_paused(app, tool, session_id);
    let widget_visible = app
        .get_webview_window("widget")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !widget_visible {
        crate::bot::audit_log(
            app,
            &format!("confirm_skipped | {tool} | {} | 挂件不可见，默认拒绝", crate::bot::truncate_for_log(detail, 120)),
        );
        crate::bot_skills::skill_confirm_result(app, false, session_id);
        return deny;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let id = uuid::Uuid::new_v4().simple().to_string();
    confirms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.clone(), (tx, session_id.map(|s| s.to_string())));
    let _ = app.emit_to(
        "widget",
        "bot-confirm",
        serde_json::json!({
            "id": id,
            "tool": tool,
            "detail": detail,
            "kind": kind,
            "sessionId": session_id,
        }),
    );
    crate::bot::audit_log(
        app,
        &format!("confirm | id: {} | kind: {kind} | {tool} | {}", &id[..8], crate::bot::truncate_for_log(detail, 120)),
    );
    match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
        Ok(Ok(reply)) => reply,
        _ => {
            confirms()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            // 确认超时默认拒绝留痕
            crate::bot::audit_log(
                app,
                &format!("confirm_timeout | {tool} | {} | 60s 无响应，默认拒绝", crate::bot::truncate_for_log(detail, 120)),
            );
            crate::bot_skills::skill_confirm_result(app, false, session_id); // 超时默认拒绝
            deny
        }
    }
}

/// 请求用户在挂件确认危险操作（如删除任务）；60s 超时默认拒绝（安全兜底）
pub async fn ask_user_confirm(
    app: &AppHandle,
    tool: &str,
    detail: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> bool {
    ask_confirm_inner(app, tool, detail, "danger", interactive, session_id)
        .await
        .approved
}

/// 文件访问授权（Kimi CLI 风格）：白名单外路径弹三选一窗
/// （允许一次 / 始终允许该目录 / 拒绝）。挂件不可见/超时/后台执行 → Deny。
pub async fn ask_path_confirm(
    app: &AppHandle,
    tool: &str,
    detail: &str,
    interactive: bool,
    session_id: Option<&str>,
) -> ConfirmChoice {
    match ask_confirm_inner(app, tool, detail, "file_access", interactive, session_id).await {
        ConfirmReply { approved: true, always: true } => ConfirmChoice::Always,
        ConfirmReply { approved: true, always: false } => ConfirmChoice::Once,
        _ => ConfirmChoice::Deny,
    }
}

/// 挂件确认响应：允许/拒绝（前端点击后回传）；always 仅 file_access 弹窗的
/// 「始终允许该目录」按钮会为 true，老调用（删任务两按钮）不传 → None → false。
/// 会话归属从 ConfirmMap 条目取回：skill_confirm_result 按会话过滤。
#[tauri::command]
pub fn bot_confirm_response(app: AppHandle, request_id: String, approved: bool, always: Option<bool>) -> Result<(), String> {
    let (tx, session_id) = take_confirm(&request_id)?;
    // Skill 调度器联动：确认结果 → 本会话技能恢复 Running / 拒绝终止 / 暂停即终止
    crate::bot_skills::skill_confirm_result(&app, approved, session_id.as_deref());
    // 用户点「拒绝」留痕
    if !approved {
        crate::bot::audit_log(
            &app,
            &format!("confirm_denied | id: {} | 用户拒绝", &request_id[..8.min(request_id.len())]),
        );
    }
    deliver_confirm(tx, ConfirmReply { approved, always: always.unwrap_or(false) })
}

/// 取待确认条目（不存在/已超时 → Err）。
fn take_confirm(
    request_id: &str,
) -> Result<(tokio::sync::oneshot::Sender<ConfirmReply>, Option<String>), String> {
    confirms()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(request_id)
        .ok_or_else(|| {
            format!("确认请求不存在或已超时：{}", &request_id[..8.min(request_id.len())])
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

// ───────────────────────── 开关持久化 ─────────────────────────

fn bot_flag_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> std::path::PathBuf {
    crate::db::data_dir(app).join("bot-enabled.flag")
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
    let dir = crate::db::data_dir(&app);
    std::fs::create_dir_all(&dir)?;
    if enabled {
        std::fs::write(bot_flag_path(&app), b"1")?;
    } else {
        let _ = std::fs::remove_file(bot_flag_path(&app));
    }
    Ok(enabled)
}
#[cfg(test)]
mod stop_all_tests {
    /// 退出置位必须同时覆盖 interactive 与后台（interactive=false）
    /// 两类实例——/stop 只停交互实例，退出清理不能漏掉后台任务
    #[test]
    fn stop_all_covers_interactive_and_background() {
        // stop_all 是全局广播，与持 StopGuard 的并行测试互斥
        let _serial = super::STOP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let g1 = super::StopGuard::new(true, Some("batch5-s1".into()));
        let g2 = super::StopGuard::new(false, None);
        assert!(super::active_execution_count() >= 2);
        let n = super::stop_all_executions();
        assert!(n >= 2);
        assert!(g1.stopped() && g2.stopped(), "两类实例都必须被置位");
    }
}

#[cfg(test)]
mod command_result_tests {
    /// /stop 置位内核只停本会话交互实例并返回数量；
    /// 锁中毒路径经 map_err 返回 Err（命令绑定 Wry AppHandle 无法单测，测内核）。
    #[test]
    fn flag_session_stopped_only_hits_own_session_interactive() {
        // 与持 StopGuard 的并行测试互斥（同上面的 stop_all 用例）
        let _serial = super::STOP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let g1 = super::StopGuard::new(true, Some("t1-3-sess".into()));
        let g2 = super::StopGuard::new(true, Some("t1-3-other".into()));
        let g3 = super::StopGuard::new(false, Some("t1-3-sess".into()));
        let n = super::flag_session_stopped(Some("t1-3-sess")).expect("锁正常应 Ok");
        assert_eq!(n, 1, "只应置位本会话的交互实例");
        assert!(g1.stopped());
        assert!(!g2.stopped(), "别的会话不受影响");
        assert!(!g3.stopped(), "后台实例不受影响");
    }

    /// 不存在/已超时的确认请求 → Err
    #[test]
    fn take_confirm_unknown_id_errs() {
        let e = super::take_confirm("t1-3-no-such-id").unwrap_err();
        assert!(e.contains("不存在或已超时"), "Err 应说明原因：{e}");
    }

    /// 等待方已退出（rx dropped）时回填失败必须可见
    #[test]
    fn deliver_confirm_dropped_receiver_errs() {
        let (tx, rx) = tokio::sync::oneshot::channel();
        drop(rx);
        let r = super::deliver_confirm(tx, super::ConfirmReply { approved: true, always: false });
        assert!(r.is_err(), "送达失败应返回 Err");
        // 对照：rx 存活时正常送达
        let (tx, rx) = tokio::sync::oneshot::channel();
        super::deliver_confirm(tx, super::ConfirmReply { approved: false, always: true })
            .expect("rx 存活应送达");
        let reply = rx.blocking_recv().unwrap();
        assert!(!reply.approved && reply.always);
    }
}
