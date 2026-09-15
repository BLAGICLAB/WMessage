//!
//! 背景：执行期注册表原先散落在 6 个模块，「有哪些全局状态」「谁负责清理」只能靠人记，
//! 测试隔离靠三把串行锁人工约定（`lib.rs:725-728` 的注释就是实锤：无差别全局广播会
//! 互相删对方的 run / 打断 StopReader 用例）。
//!
//! 本模块只做一件事：把**定义**集中到一处，各业务模块用 `pub(crate) use` / `use`
//! 再引入原名字 —— 常量与类型（如 `NEXT_STOP_ID`、`StopMap`）路径 1:1 不变；
//! 表访问器自 3.2 起按 `try_state` 注入，故签名上多了 `app`（调用点同步更新）。
//! **语义零改动**（RAII Drop 清理、fail-open/fail-closed 口径、
//! `/stop` 只停 interactive 实例等一律未动）。
//!
//! 全局状态总表（盘点时的真实位置，改动后会漂）：
//!
//! | 状态 | 类型 | 生命周期 | 清理责任人 | 测试隔离现状 |
//! |---|---|---|---|---|
//! | `SKILL_RUNS` → `AppState.skill_runs` | `Arc<Mutex<HashMap<String, SkillRun>>>` | 进程 | `skill_terminate_all` / `clear_terminal_skill_runs` / 状态机终态 | per-test `manage` 注入（串行锁已撤） |
//! | `STOP_REGISTRY` → `AppState.stop_registry` | `Arc<Mutex<HashMap<u64, (Arc<AtomicBool>, bool, Option<String>)>>>` | 单次执行 | `StopGuard::drop`（靠 acquire 时克隆的 Arc 注销） | per-test `manage` 注入（串行锁已撤） |
//! | `NEXT_STOP_ID`（**刻意留在进程级**） | `AtomicU64` | 进程 | —（单调递增，无清理责任） | 不需要 |
//! | `CONFIRMS` → `AppState.confirm_requests` | `Mutex<HashMap<String, (oneshot::Sender<ConfirmReply>, Option<String>)>>` | 单次弹窗 | 前端应答 / 60s 超时回收 | per-test `manage` 注入 |
//! | `CHAT_RUNNING` → `AppState.chat_running` | `Arc<Mutex<HashSet<String>>>` | 单次聊天 | `ChatGuard::drop`（靠 acquire 时克隆的 Arc 清理） | per-test `manage` 注入 |
//! | `EXEC_RUNNING` → `AppState.exec_running` | `Arc<Mutex<HashSet<String>>>` | 单次任务卡执行 | `ExecGuard::drop`（同上） | per-test `manage` 注入 |
//! | `SCHED_RUNNING` → `AppState.sched_running` | `Arc<Mutex<HashSet<String>>>` | 单次定时执行 | `SchedGuard::drop`（panic 展开也清；靠 acquire 时克隆的 Arc 清理） | 不需要 |
//! | `PENDING` → `AppState.pending` | `Mutex<HashMap<String, PendingExec>>` | 单次逐步执行 | 用户应答 / 超时 | 集成测试自带本地锁（`tests/skill_e2e.rs:258`） |
//! | `REGISTRY`（产物登记）→ `AppState.artifact_registry` | `tokio::sync::Mutex<HashMap<String, Vec<RegisteredArtifact>>>` | 单次任务卡执行流程 | 流程收尾 clear 系列 | per-test `manage` 注入（样板） |
//! | `SESSION_ORIGINS`（有意留档·情况 1，定义留 `tool_guard.rs:45`） | `Mutex<HashMap<String, TaskExecOrigin>>` | 单次执行 | `unregister_exec_session`（`bot_chat.rs:1415`） | 不需要（详情见下「不收口」条目） |
//!
//! 刻意**不收口**的四类（登记在册，避免下次重复盘查）：
//!
//! - `SESSION_ORIGINS`（`tool_guard.rs:45`，`HashMap<String, TaskExecOrigin>`）：
//!   情况 1「session 元数据」——有意留档，不进依赖容器。
//!   三条测量结论（`rg` 全仓核定）：
//!   1. **写一次**：生产仅 `bot_chat.rs:1373 register_exec_session` 一处写入、
//!      `bot_chat.rs:1415 unregister_exec_session` 一处删除，夹在同一函数
//!      `run_task_in_chat_with` 的一头一尾；
//!   2. **读一处**：生产仅 `bot/tools.rs:876 is_task_execution_flow(session_id)`
//!      （`tool_link_file_to_task` 内判定）——不在 tasks-updated / `bot_model_loop` /
//!      `dispatch` / `api_handlers` 里读；
//!   3. **随 session 消亡**：key = `session_id`，进出一对，与单次执行同生同死。
//!   两条附带事实（留给未来的 D 方案）：
//!   - **值从未被读取**：容器只有 `insert`/`remove`/`contains_key`（`tool_guard.rs:53/59/69`），
//!     无 `.get(sid)` 或遍历 → 真实语义等价 `HashSet<String>`，`TaskExecOrigin` 是冗余记录；
//!   - **与 `StopGuard.allow_atomic` 功能重叠**：`bot/dispatch.rs:115` 的
//!     `stop.is_some_and(|s| s.allow_atomic())` 表达的正是「任务卡执行流程内」，
//!     而它随执行对象传递、Drop 即失效——那才是「元数据随 session 走」的正确形态。
//!   → 未来若要删这张表（方案 D）：需把执行上下文透进工具层（`execute_tool` 签名链）
//!     并触碰停止守卫语义，属独立立项 + 需批准。
//!   （「搬进 `BotSession`」目前无落点：`db.rs:544 pub struct BotSession` 是磁盘行且
//!     `bot_sessions` schema 在禁止清单；内存中不存在会话对象。）
//! - `ROUTES`（`intent_router.rs:139`）：进程级派生表，随技能变更整体重建，无「清理责任人」问题
//!   （搬它要给私有的 `CompiledRule` 放宽可见性，不值当）；
//! - API 专属 4 项（`api_handlers.rs:59 API_RMW_LOCK` / `:177 RATE` / `:817 SSE_WRITERS` /
//!   `:820 API_HUB_KEY`）：随 API server 生命周期，与 bot 执行链无关；
//! - 不可变 / 幂等缓存：`memory/embed.rs:35`、`ocr.rs:187`、`bot_web.rs:22`、`paths.rs:129`、
//!   `bot_py.rs:154/200/562/770/779`、`profile.rs:20`、`bot/config.rs:471`、`db.rs:429`、
//!   `migration.rs:382`、`due_notify.rs:39`、`bot_skills/vars.rs` 的 4 个 Regex。
//!
//! 本阶段**不**引入新依赖（DashMap / parking_lot 属禁止清单），锁实现保持
//! `std::sync::Mutex` —— 这些表的临界区都是极短的（见 `tool_guard.rs:41` 的取舍说明），
//! 目前没有热点证据支持换锁。
//!
//! 测试期的跨进程共享（非全局状态，但同属「进程级假设」，留档观察）：
//! - 数据目录解析（`paths.rs::probe_log_dir`）在测试构建下 = `target/debug/deps/`，
//!   而 nextest 是每测试一进程 → `bot.log` / `wmessage.db` / `*.flag` / 降级 key 文件跨进程共享；
//! - `profile.json` 已按 pid 隔离（`profile.rs::test_isolated_dir`）—— 唯一被实证打中的共享文件
//!   （曾 6 次复现 profile 用例随机挂，修后 5×nextest 全绿）；
//! - 其余 5 次 nextest 全绿、暂无实证，按「等实锤再动」留档；下沉到 `probe_log_dir` 的
//!   三档方案（B1/B2/B3）与各自代价写在 `paths.rs::probe_log_dir` 的 doc 里。
//! - 追加：`cargo test --lib`（进程内并行）下 `exit_cleanup_tests` 的
//!   `api-enabled.flag` 断言失败过 1 次；pid 隔离覆盖不到进程内并行
//!   （同 pid 的测试共享目录，nextest 因每测试一进程才避开它）。实证细节与决策（C1 口径：
//!   等定位到具体调用点再动）见 `paths.rs::probe_log_dir` 的 doc「追加实证 / 决策」两段。
//!
//! 阶段 3.2 已开工：`AppState`（本文件尾部）+ `lib.rs` setup 里 `manage` 注入，
//! **产物登记表**是样板（见上表最后一行）；其余 7 张逐张迁移。每迁一张，对应测试改用
//! per-test `manage` 注入（先例：`middleware.rs:471`、`lib.rs:737`），届时上表里
//! 那两把串行锁可以撤掉。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::Manager;

// ───────────────────────── 执行期 · Skill 运行表 ─────────────────────────

// SKILL_RUNS 已迁入 `AppState.skill_runs`（访问器见文件尾）。

// ───────────────────────── 执行期 · /stop 停止注册表 ─────────────────────────

/// /stop 注册表形状：执行实例 id → (停止标志, 是否用户交互触发, 触发会话)
pub(crate) type StopMap =
    Mutex<HashMap<u64, (Arc<std::sync::atomic::AtomicBool>, bool, Option<String>)>>;

// STOP_REGISTRY 已迁入 `AppState.stop_registry`（访问器见文件尾）。
// NEXT_STOP_ID 留在这里：它是 id 发号器（不是表），无需 app 即可发号；
// 表取注入实例、发号器全局单调——与迁移前的组合行为一致。
/// /stop 实例 id 发号器（单调递增，进程内唯一）
pub(crate) static NEXT_STOP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ───────────────────────── 执行期 · 危险操作确认 ─────────────────────────

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

// ───────────────────────── 执行期 · 会话/任务防重入 ─────────────────────────

// CHAT_RUNNING / EXEC_RUNNING / SCHED_RUNNING 三张防重入表均已迁入 `AppState`
// （字段与访问器见文件尾；访问器一律返回 `Arc` 克隆，供 RAII 守卫在 Drop 里清理）。

// ───────────────────────── 执行期 · 逐步执行挂起表 ─────────────────────────

// PENDING 已迁入 `AppState.pending`（访问器见文件尾）。

// ────────────────── AppState（阶段 3.2 样板：产物登记表）──────────────────

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
///   `middleware.rs:187-190`「非原子工具 fail-open、安全闸门类才 fail-closed」同一口径。
/// - 要测试隔离（单元测试，同 crate）：`app.manage(AppState::default())` 注入自己的实例，
///   先例 `middleware.rs:471`、`lib.rs:737`。集成测试（`tests/*.rs`）暂时只能看见 `pub` 项，
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
}

/// 兜底实例：只在 `AppState` 未注入的路径上用（见上面的口径说明）
static FALLBACK: OnceLock<AppState> = OnceLock::new();

fn fallback_state() -> &'static AppState {
    FALLBACK.get_or_init(AppState::default)
}

/// 统一 `AppState` 拿取：注入实例优先，否则兜底实例。
///
/// Sprint B1：原 8 个 accessor（`skill_runs` / `artifact_registry` / `confirms` /
/// `sched_running` / `stop_registry` / `chat_running` / `exec_running` /
/// `pending_map`）各自重复 `try_state → map → unwrap_or(fallback)` 三段，
/// 抽出这一个 helper 收口，行为零变化。
///
/// 注意：不能用 `.map(|s| s.inner()).unwrap_or_else(fallback_state)` 形式——
/// 闭包内 `s.inner()` 的寿命绑在闭包局部变量上，Rust 没法把它桥接到 `app`；
/// `match` 让两条分支返回类型都受 `app` 寿命约束，`&'static` 走子类型
/// （covariant）自然满足。
pub(crate) fn ext<'a, R: tauri::Runtime>(app: &'a tauri::AppHandle<R>) -> &'a AppState {
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
pub(crate) fn artifact_registry<'a, R: tauri::Runtime>(
    app: &'a tauri::AppHandle<R>,
) -> &'a tokio::sync::Mutex<HashMap<String, Vec<crate::bot_artifacts::RegisteredArtifact>>> {
    &ext(app).artifact_registry
}

/// 待确认请求表访问器（`/stop` 收尾、`ask_confirm_inner` 登记/超时回收、`take_confirm` 取走）。
///
/// 无 RAII 守卫（锁只在 insert/remove 期间持有）→ 借用引用，同 `artifact_registry`。
pub(crate) fn confirms<'a, R: tauri::Runtime>(app: &'a tauri::AppHandle<R>) -> &'a ConfirmMap {
    &ext(app).confirm_requests
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
pub(crate) fn pending_map<'a, R: tauri::Runtime>(
    app: &'a tauri::AppHandle<R>,
) -> &'a Mutex<HashMap<String, crate::exec_steps::PendingExec>> {
    &ext(app).pending
}
