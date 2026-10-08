//! 执行期全局状态单一入口：各表定义集中在此，业务模块 `pub(crate) use` 引入
//! 原名字；表访问器按 `try_state` 注入（测试用 per-test `manage` 隔离）。
//!
//! 全局状态总表（SKILL_RUNS / STOP_REGISTRY / CONFIRMS / CHAT_RUNNING /
//! EXEC_RUNNING / SCHED_RUNNING / PENDING / artifact_registry，及刻意留在
//! 进程级的 NEXT_STOP_ID 与 tool_guard::SESSION_ORIGINS）的类型、生命周期、
//! 清理责任人盘点：见 git log 本文件头的历史版本（2026-10-08 压缩——
//! 行号快照注定漂移，盘点型内容不驻留源码）。
//!
//! 刻意不收口的四类（SESSION_ORIGINS / ROUTES / API 专属 4 项 / 不可变缓存）
//! 与「锁保持 std::sync::Mutex、不引新依赖」的取舍理由同样留档 git log；
//! 新表入容器的样板：产物登记表。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::Manager;

// 执行期 · Skill 运行表

// SKILL_RUNS 已迁入 `AppState.skill_runs`（访问器见文件尾）。

// 执行期 · /stop 停止注册表

/// /stop 注册表形状：执行实例 id → (停止标志, 是否用户交互触发, 触发会话)
pub(crate) type StopMap =
    Mutex<HashMap<u64, (Arc<std::sync::atomic::AtomicBool>, bool, Option<String>)>>;

// STOP_REGISTRY 已迁入 `AppState.stop_registry`（访问器见文件尾）。
// NEXT_STOP_ID 留在这里：它是 id 发号器（不是表），无需 app 即可发号；
// 表取注入实例、发号器全局单调——与迁移前的组合行为一致。
/// /stop 实例 id 发号器（单调递增，进程内唯一）
pub(crate) static NEXT_STOP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// 执行期 · 危险操作确认

/// 待确认请求：id → (oneshot 通道, 归属会话 id)
type ConfirmMap = Mutex<
    HashMap<
        String,
        (
            tokio::sync::oneshot::Sender<crate::bot_slash::ConfirmReply>,
            Option<String>,
        ),
    >,
>;

// CONFIRMS 已迁入 `AppState.confirm_requests`（访问器 `confirms(app)` 见文件尾）。

/// 工作流提问等待表：questionId → oneshot 通道。
/// ask 端（runner）注册、应答端（`workflow_question_respond`）take 后发送。
/// 与 CONFIRMS 的区别：无 60s 超时默认拒——等待上限由 ask 端按提问超时配置兜底，
/// 通道关闭/超时一律回落到问题自带的假设值（红线：忽略问题工作流也能走）。
type QuestionWaiters = Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>;

// 执行期 · 会话/任务防重入

// CHAT_RUNNING / EXEC_RUNNING / SCHED_RUNNING 三张防重入表均已迁入 `AppState`
// （字段与访问器见文件尾；访问器一律返回 `Arc` 克隆，供 RAII 守卫在 Drop 里清理）。

// 执行期 · 逐步执行挂起表

// PENDING 已迁入 `AppState.pending`（访问器见文件尾）。

// AppState（阶段 3.2 样板：产物登记表）

/// 应用级状态容器：全局可变状态的注入式载体（`tauri::Manager::manage` 注入）。
///
/// 逐步收口；已迁入 **8 张**：`SKILL_RUNS`、`STOP_REGISTRY`、产物登记、
/// `SCHED_RUNNING`、`CONFIRMS`、`CHAT_RUNNING`、`EXEC_RUNNING`、`PENDING`
/// （见模块头总表里标「已迁入」的行）。`SESSION_ORIGINS` 按「情况 1：session 元数据」
/// **有意留档**，不进本容器（判据与三条测量结论见模块头总表的对应条目）。
///
/// **「state 缺失」口径（本次定稿）**：
/// - 生产：`lib.rs` setup 里 `app.manage(AppState::default())` 注入。`AppHandle` 上
///   就有 `try_state`，命令与内部调用链一律拿得到——**不需要**全局单例 + 线程局部。
/// - 缺失时（mock app / 未注入的单测 / 启动早期的旁路调用）：退回**进程级兜底实例**，
///   同型同语义——不降级功能、不改变行为。这是业务类状态的 **fail-open**，与
///   `middleware.rs` 的「非原子工具 fail-open、安全闸门类才 fail-closed」同一口径。
/// - 要测试隔离（单元测试，同 crate）：`app.manage(AppState::default())` 注入自己的实例，
///   先例见各测试中的 `manage(AppState::default())` 调用。集成测试（`tests/*.rs`）暂时只能看见 `pub` 项，
///   注不进 `pub(crate)` 的 `AppState`，故走兜底实例——要给它一个入口需另议（本阶段不扩公开面）。
#[derive(Default)]
pub(crate) struct AppState {
    /// 活动 Skill 运行表：name → SkillRun（同一技能同轮只允许一个实例）
    pub(crate) skill_runs: Arc<Mutex<HashMap<String, crate::bot_skills::SkillRun>>>,
    /// 产物登记表：task_id → 已登记产物（任务卡执行流程内收集，收尾按 origin 分流弹窗）
    pub(crate) artifact_registry:
        tokio::sync::Mutex<HashMap<String, Vec<crate::bot_artifacts::RegisteredArtifact>>>,
    /// 定时调度防重入（`SchedGuard` 持有）
    pub(crate) sched_running: Arc<Mutex<HashSet<String>>>,
    /// /stop 停止注册表（`StopGuard` 持有；`StopMap` 值 = 停止标志 + 是否交互触发 + 触发会话）
    pub(crate) stop_registry: Arc<StopMap>,
    /// 会话级聊天防重入（`ChatGuard` 持有；无会话 id 时不加锁）
    pub(crate) chat_running: Arc<Mutex<HashSet<String>>>,
    /// 任务卡执行防重入（`ExecGuard` 持有，同一 task_id 同时只允许一个执行实例）
    pub(crate) exec_running: Arc<Mutex<HashSet<String>>>,
    /// 逐步执行挂起表（`PendingExec` 按会话分槽；无守卫，锁只在 park/take 期间持有）
    pub(crate) pending: Mutex<HashMap<String, crate::exec_steps::PendingExec>>,
    /// 待确认请求（`ConfirmMap`：id → (oneshot 通道, 归属会话 id)）
    pub(crate) confirm_requests: ConfirmMap,
    /// 工作流提问等待表`QuestionWaiters`：questionId → oneshot 通道）
    pub(crate) question_waiters: QuestionWaiters,
    /// 工作流提问上下文表sessionId → AskRegistration；bot_chat 会话建立时
    /// 注册/收尾注销，ask_user 工具按 session_id 查表，预算在条目上）
    pub(crate) ask_contexts: Mutex<HashMap<String, crate::workflow_questions::AskRegistration>>,
    /// 子 agent 取消令牌表subagent_id → StopToken；
    /// cancel_subagent 持有的句柄，runner 注册 / 收尾删除）
    pub(crate) subagent_stops: Arc<Mutex<HashMap<String, crate::bot_slash::StopToken>>>,
    /// 执行痕迹注册表（Agent 透明化设计 §4.2）：session_id → 运行中 trace_id。
    /// 生命周期：run_task_in_chat_with 建 trace 时注册、收尾注销（trace_sink::begin/end_trace）。
    /// dispatch 据此判断「本会话在跑 trace」→ 决定 span/file_change 是否落库（无映射 = 零开销旁路）。
    pub(crate) trace_registry: Arc<Mutex<HashMap<String, i64>>>,
}

/// 兜底实例：只在 `AppState` 未注入的路径上用（见上面的口径说明）
static FALLBACK: OnceLock<AppState> = OnceLock::new();

fn fallback_state() -> &'static AppState {
    FALLBACK.get_or_init(AppState::default)
}

/// 统一 `AppState` 拿取：注入实例优先，否则兜底实例。
///
/// Sprint ：原 8 个 accessor（`skill_runs` / `artifact_registry` / `confirms` /
/// `sched_running` / `stop_registry` / `chat_running` / `exec_running` /
/// `pending_map`）各自重复 `try_state → map → unwrap_or(fallback)` 三段，
/// 抽出这一个 helper 收口，行为零变化。
///
/// 注意：不能用 `.map(|s| s.inner()).unwrap_or_else(fallback_state)` 形式——
/// 闭包内 `s.inner()` 的寿命绑在闭包局部变量上，Rust 没法把它桥接到 `app`；
/// `match` 让两条分支返回类型都受 `app` 寿命约束，`&'static` 走子类型
/// （covariant）自然满足。
pub(crate) fn ext<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> &AppState {
    match app.try_state::<AppState>() {
        Some(s) => s.inner(),
        None => fallback_state(),
    }
}

/// 活动 Skill 运行表访问器：返回 **Arc 克隆**。
///
/// 用 Arc 而非借用引用：这张表的访问点既有生产链（`app` 是参数），也有大量测试
/// （`tests/skill_e2e.rs` 15 处 + lib 单测 11 处）；测试里 handle 常是临时值，
/// 借用会踩 E0716（临时值先 drop）。
pub(crate) fn skill_runs<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashMap<String, crate::bot_skills::SkillRun>>> {
    ext(app).skill_runs.clone()
}

/// 产物登记表访问器：优先取注入的 `AppState` 实例，缺失则退回兜底实例。
/// 该表只在调用期间使用（无 Drop 清理诉求），故返回借用引用。
pub(crate) fn artifact_registry<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> &tokio::sync::Mutex<HashMap<String, Vec<crate::bot_artifacts::RegisteredArtifact>>> {
    &ext(app).artifact_registry
}

/// 待确认请求表访问器（`/stop` 收尾、`ask_confirm_inner` 登记/超时回收、`take_confirm` 取走）。
///
/// 无 RAII 守卫（锁只在 insert/remove 期间持有）→ 借用引用，同 `artifact_registry`。
pub(crate) fn confirms<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> &ConfirmMap {
    &ext(app).confirm_requests
}

/// 工作流提问等待表访问器ask 端注册 / 应答端 take 走；无 RAII 守卫，同 confirms 口径）。
pub(crate) fn question_waiters<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> &QuestionWaiters {
    &ext(app).question_waiters
}

/// 工作流提问上下文表访问器注册/注销/ask_user 查表共用一把锁）。
pub(crate) fn ask_contexts<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> &Mutex<HashMap<String, crate::workflow_questions::AskRegistration>> {
    &ext(app).ask_contexts
}

/// 定时调度防重入表访问器：返回 **Arc 克隆**。
///
/// 带 RAII 守卫的表一律用这种形态——`SchedGuard::drop` / `ChatGuard::drop` /
/// `ExecGuard::drop` 里拿不到 `app`，只能在 acquire 时把句柄克隆进守卫，
/// Drop 用手里这份清理（同一实例，是同一个 `Mutex`，语义与迁移前一致）。
pub(crate) fn sched_running<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashSet<String>>> {
    ext(app).sched_running.clone()
}

/// /stop 停止注册表访问器：Arc 克隆（`StopGuard::drop` 靠它注销自己的 id）。
///
/// 表进 `AppState` 后 `StopGuard::new` / `new_task_exec` 必须带 `app`
/// （公开构造函数签名变化，已获批准）——否则守卫会注册进兜底实例、而 `/stop`
/// 查注入实例，停止功能就真的坏了。id 发号仍是全局 `NEXT_STOP_ID`。
pub(crate) fn stop_registry<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Arc<StopMap> {
    ext(app).stop_registry.clone()
}

/// 会话级聊天防重入表访问器：Arc 克隆（`ChatGuard::drop` 靠它清本会话槽位）。
pub(crate) fn chat_running<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashSet<String>>> {
    ext(app).chat_running.clone()
}

/// 任务卡执行防重入表访问器：Arc 克隆（`ExecGuard::drop` 靠它清 task_id）。
pub(crate) fn exec_running<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashSet<String>>> {
    ext(app).exec_running.clone()
}

/// 逐步执行挂起表访问器（`has_pending_for` / `park` / `take_pending_for` 用）。
///
/// 无 RAII 守卫（锁只在 park/take 期间持有）→ 借用引用，同 `artifact_registry` / `confirms`。
pub(crate) fn pending_map<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> &Mutex<HashMap<String, crate::exec_steps::PendingExec>> {
    &ext(app).pending
}

/// 子 agent 取消令牌表访问器（返回 Arc 克隆——cancel 与 runner 分属不同任务，
/// 令牌生命周期需跨任务存活）。
pub(crate) fn subagent_stops<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashMap<String, crate::bot_slash::StopToken>>> {
    ext(app).subagent_stops.clone()
}

/// 执行痕迹注册表访问器：Arc 克隆（begin/end_trace 分属执行首尾两处，跨 await 存活）。
pub(crate) fn trace_registry<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Arc<Mutex<HashMap<String, i64>>> {
    ext(app).trace_registry.clone()
}

/// 查会话当前运行中的 trace_id（无会话 / 无运行中 trace → None；dispatch 据此跳过采集）。
pub(crate) fn trace_id_for_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    session_id: Option<&str>,
) -> Option<i64> {
    let sid = session_id?;
    // 锁毒化必须留痕：静默 .ok()? 会让后续 trace 查询全部落空且无线索可查
    match trace_registry(app).lock() {
        Ok(g) => g.get(sid).copied(),
        Err(e) => {
            eprintln!("[mutex_poisoned] app_state::trace_registry: {e:?}");
            e.into_inner().get(sid).copied()
        }
    }
}
