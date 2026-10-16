//! L4 `pre_tool` 钩子扩展点：进程级注册表 + 只读 tool_call 视图 + 内置观察钩子。
//!
//! 三条纪律（见 docs/ONION-HOOKS-DESIGN-2026-10-16.md §四）：
//! 1. **层序不可配**：注册表只决定「同层内按注册序执行」，层序由 Rust 函数
//!    调用栈保证——本模块没有任何「层优先级 / 层顺序」字段或入参，
//!    结构上不存在「配置层序」这条路。`HookLayer` 只提供按层禁用的粒度。
//! 2. **L4 只读**：钩子入参是 `&ToolCallView<'_>`，字段全是共享借用，
//!    类型层面没有改写 `args` 的路径（改写属于 L2/L3）。
//! 3. **钩子不得中断主链路**：返 Err 或 panic 一律记审计并放行（fail-open），
//!    钩子实现里的 bug 不能变成用户主对话中断。
//!
//! trait 名字带层前缀（`PreToolHook`）而不是统一 `Hook`：本期只有一层，
//! 泛化入口还没有第二个读者。

use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::dispatch::ToolCallTrace;

/// 钩子层。层序**不由本枚举决定**（见模块头纪律 1）：这里既没有排序字段也没有
/// 比较实现，层序只由「谁在调用栈上先调用谁」决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookLayer {
    /// L4：工具执行前（三道闸入口）。只可拒绝或放行。
    PreTool,
    /// L5：工具执行后。可改工具结果（清洗、结构化、脱敏）。
    PostTool,
    /// L6：回合结束。可改最终回复（归档、记忆沉淀、指标）。
    PostTurn,
}

impl HookLayer {
    /// 已接线层的名（与配置层名一一对应）。
    pub fn name(&self) -> &'static str {
        match self {
            HookLayer::PreTool => "pre_tool",
            HookLayer::PostTool => "post_tool",
            HookLayer::PostTurn => "post_turn",
        }
    }

    /// 配置层名 → 已接线层。返回 None = 该层在设计里存在但**尚未接线**，
    /// 调用方必须把它当「没关成」记审计，不能静默当成功——一份配置写着
    /// `post_tool: ""` 却没关掉任何东西，正是「配置看着生效、实际落空」。
    pub fn from_name(name: &str) -> Option<HookLayer> {
        match name {
            "pre_tool" => Some(HookLayer::PreTool),
            "post_tool" => Some(HookLayer::PostTool),
            "post_turn" => Some(HookLayer::PostTurn),
            _ => None,
        }
    }

    /// 注册表已接线的层名全集。
    pub fn wired_names() -> &'static [&'static str] {
        const WIRED: &[&str] = &["pre_tool", "post_tool", "post_turn"];
        WIRED
    }
}

/// L4 钩子看到的只读工具调用视图。
///
/// 字段全是共享借用：`args` 是 `&str`、`trace` 是 `&ToolCallTrace`，
/// 钩子拿到的 `&ToolCallView<'_>` 因此没有任何可变路径——想在 L4 改参数
/// 在类型层面就做不到（不是靠约定，钩子实现里写不出那种代码）。
/// `trace` 整体借引用而不是拆成 `turn` / `tool_call_id` 两个影子字段：
/// 少一份会漂移的副本，`ToolCallTrace` 加字段时钩子侧自动跟上。
#[derive(Debug, Clone, Copy)]
pub struct ToolCallView<'a> {
    /// 工具名（与 `tool.call` 审计同名字段）
    pub tool: &'a str,
    /// 原始入参 JSON 文本（只读）
    pub args: &'a str,
    pub session_id: Option<&'a str>,
    /// 模型循环上下文（只读）
    pub trace: &'a ToolCallTrace,
}

/// 单个钩子对一次工具调用的判定：放行，或拒绝本次调用（带一条给模型看的理由）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreToolVerdict {
    /// 放行，继续走后续闸与工具本体。
    Continue,
    /// 拒绝执行本次工具调用；`reason` 进审计并回给模型。
    Interrupt { reason: String },
}

/// 钩子自报的错误 / panic。只用于审计留痕：运行时不据此中断（fail-open）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookFailureKind {
    /// 钩子返回了 Err。
    Returned(String),
    /// 钩子 panic 被 catch 住。
    Panic(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFailure {
    pub hook: &'static str,
    pub kind: HookFailureKind,
}

/// 跑完一层的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRun {
    /// 最终判定：首个 `Interrupt` 胜出（其后钩子不再跑），无人拒绝 = `Continue`。
    pub decision: PreToolVerdict,
    /// 本次跑出的失败（Err / panic），调用方逐条记审计后照常放行。
    pub failures: Vec<HookFailure>,
}

/// pre_tool 钩子。对象安全（无泛型方法、无 `Self` 返回、无关联类型）。
pub trait PreToolHook: Send + Sync {
    /// 钩子名，进审计 kv（`&'static str`：注册表按值持有钩子，不带生命周期）。
    fn name(&self) -> &'static str;
    /// 只读拦截：拿到的 `call` 不可改写（纪律 2），只能放行或拒绝。
    /// 返回 Err 不等于拒绝——按 fail-open 记审计后放行（纪律 3）。
    fn on_pre_tool(&self, call: &ToolCallView<'_>) -> Result<PreToolVerdict, String>;
}

/// L5 `post_tool` 钩子：工具已跑完，可改**工具结果**（清洗 / 结构化 / 脱敏）。
///
/// 与 L4 的关键差别：这里给 `&mut String`（可改写），且**不能拒绝**——
/// 工具已经执行完了，此时「拒绝」没有意义，只剩改写。返回 `Ok(())` 即放行，
/// 改写直接落在 `result` 上。
///
/// 改写约定（唯一的实现纪律）：**要整体赋值**（`*result = new_text`），
/// 不要原地 `clear()` 再 `push`。panic 时原地写会留下半成品，而 panic 的钩子
/// 是 fail-open 放行的——半成品会被当正常结果交给模型。
pub trait PostToolHook: Send + Sync {
    fn name(&self) -> &'static str;
    fn on_post_tool(&self, call: &ToolCallView<'_>, result: &mut String) -> Result<(), String>;
}

/// L6 `post_turn` 钩子：回合结束，可改**最终回复**（归档 / 记忆沉淀 / 指标）。
/// 与 L5 同理：给 `&mut String`、不能拒绝、panic fail-open、整体赋值。
pub trait PostTurnHook: Send + Sync {
    fn name(&self) -> &'static str;
    /// `round` 是本会话轮次序号（从 1 起），供「每 N 轮做一次」类钩子用。
    fn on_post_turn(&self, reply: &mut String, round: usize) -> Result<(), String>;
}

/// 跨层统一的可注册体。
///
/// 为什么是**每层一个 trait**而不是文档 §四 草稿里的「单 `Hook` trait +
/// `HookInput` 枚举」：三层签名本质不同（L4 只读且可拒绝，L5/L6 可改写且不可
/// 拒绝），合成一个 trait 只会让每个实现都要 match 自己永远收不到的变体，并把
/// 别层的 payload 形状暴露在每个签名里——那恰好废掉 §3.8 用类型层面换来的
/// 只读纪律。分层 trait 让「L4 拿不到 `&mut`」成为签名事实而非约定。
pub enum AnyHook {
    Pre(Box<dyn PreToolHook>),
    PostTool(Box<dyn PostToolHook>),
    PostTurn(Box<dyn PostTurnHook>),
}

struct HookEntry {
    layer: HookLayer,
    name: &'static str,
    hook: AnyHook,
}

/// 进程级钩子注册表：注册只在进程启动期（`with_builtins`）发生一次，
/// 之后只有「按会话禁用」是运行期可变。
pub struct HookRegistry {
    /// 按注册序排列；同层内的先后即向量下标先后（纪律 1）。
    hooks: Vec<HookEntry>,
    /// 层 → 已关掉该层的会话 id。表里没有的层 = 全开（默认全开）。
    disabled: Mutex<HashMap<HookLayer, HashSet<String>>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self {
            hooks: Vec::new(),
            disabled: Mutex::new(HashMap::new()),
        }
    }
}

impl HookRegistry {
    /// 注册 L4 钩子：同层内后注册的后跑。
    pub fn register(&mut self, hook: Box<dyn PreToolHook>) {
        let name = hook.name();
        self.hooks.push(HookEntry {
            layer: HookLayer::PreTool,
            name,
            hook: AnyHook::Pre(hook),
        });
    }

    /// 注册 L5 钩子。
    pub fn register_post_tool(&mut self, hook: Box<dyn PostToolHook>) {
        let name = hook.name();
        self.hooks.push(HookEntry {
            layer: HookLayer::PostTool,
            name,
            hook: AnyHook::PostTool(hook),
        });
    }

    /// 注册 L6 钩子。
    pub fn register_post_turn(&mut self, hook: Box<dyn PostTurnHook>) {
        let name = hook.name();
        self.hooks.push(HookEntry {
            layer: HookLayer::PostTurn,
            name,
            hook: AnyHook::PostTurn(hook),
        });
    }

    /// 进程级注册表的初始内容：装上内置钩子。
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        r.register(Box::new(BuiltinObserver));
        r.register_post_tool(Box::new(BuiltinPostToolObserver));
        r.register_post_turn(Box::new(BuiltinPostTurnObserver));
        r
    }

    /// 会话级开关：关掉某会话某一层的钩子。
    /// 无会话上下文的调用（`session_id = None` 的后台调度路径）不参与会话级
    /// 开关——没有会话可归属，就没有「这个会话关掉它」的落点。
    pub fn disable_layer_for_session(&self, layer: HookLayer, session_id: &str) {
        self.disabled_sessions()
            .entry(layer)
            .or_default()
            .insert(session_id.to_string());
    }

    fn disabled_sessions(&self) -> MutexGuard<'_, HashMap<HookLayer, HashSet<String>>> {
        // 锁中毒（别的线程 panic 时）不阻断工具调用：取回内层数据继续用。
        // 这张表在钩子调用之外才被取用（钩子 panic 不会发生在持锁期间），
        // 且每次写都是「插入一个会话 id」，不存在半写状态。
        self.disabled.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn layer_enabled(&self, layer: HookLayer, session_id: Option<&str>) -> bool {
        let Some(sid) = session_id else {
            return true;
        };
        let sessions = self.disabled_sessions();
        !sessions.get(&layer).is_some_and(|off| off.contains(sid))
    }

    /// 跑一层钩子（fail-open）。
    ///
    /// - 该层被本会话禁用 → 直接放行，一个钩子都不跑；
    /// - 首个 `Interrupt` 短路，后续钩子不再跑（同一层只该有一个拒绝理由）；
    /// - 返 Err / panic 的钩子进 `failures` 并继续跑后面的钩子，
    ///   只有 `Interrupt` 能左右最终判定。
    pub fn run_pre_tool(&self, call: &ToolCallView<'_>) -> HookRun {
        let mut failures = Vec::new();
        if !self.layer_enabled(HookLayer::PreTool, call.session_id) {
            return HookRun::continue_with(failures);
        }
        let mut decision = PreToolVerdict::Continue;
        for entry in self.hooks.iter().filter(|e| e.layer == HookLayer::PreTool) {
            if matches!(decision, PreToolVerdict::Interrupt { .. }) {
                break;
            }
            let AnyHook::Pre(h) = &entry.hook else {
                continue;
            };
            match std::panic::catch_unwind(AssertUnwindSafe(|| h.on_pre_tool(call))) {
                Ok(Ok(verdict)) => decision = verdict,
                Ok(Err(e)) => failures.push(failure(entry.name, HookFailureKind::Returned(e))),
                Err(p) => failures.push(failure(entry.name, HookFailureKind::Panic(panic_msg(p)))),
            }
        }
        HookRun { decision, failures }
    }

    /// L5：跑 post_tool 层（fail-open）。
    ///
    /// 与 L4 同样的三条容错（禁用即整层跳过 / Err / panic 记 failure 继续），
    /// 区别只有两点：不判 Interrupt，且 `result` 是可变引用——改写直接生效。
    pub fn run_post_tool(&self, call: &ToolCallView<'_>, result: &mut String) -> HookRun {
        let mut failures = Vec::new();
        if !self.layer_enabled(HookLayer::PostTool, call.session_id) {
            return HookRun::continue_with(failures);
        }
        for entry in self.hooks.iter().filter(|e| e.layer == HookLayer::PostTool) {
            let AnyHook::PostTool(h) = &entry.hook else {
                continue;
            };
            match std::panic::catch_unwind(AssertUnwindSafe(|| h.on_post_tool(call, result))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(failure(entry.name, HookFailureKind::Returned(e))),
                Err(p) => failures.push(failure(entry.name, HookFailureKind::Panic(panic_msg(p)))),
            }
        }
        HookRun::continue_with(failures)
    }

    /// L6：跑 post_turn 层（fail-open）。语义同 `run_post_tool`。
    pub fn run_post_turn(
        &self,
        session_id: Option<&str>,
        reply: &mut String,
        round: usize,
    ) -> HookRun {
        let mut failures = Vec::new();
        if !self.layer_enabled(HookLayer::PostTurn, session_id) {
            return HookRun::continue_with(failures);
        }
        for entry in self.hooks.iter().filter(|e| e.layer == HookLayer::PostTurn) {
            let AnyHook::PostTurn(h) = &entry.hook else {
                continue;
            };
            match std::panic::catch_unwind(AssertUnwindSafe(|| h.on_post_turn(reply, round))) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(failure(entry.name, HookFailureKind::Returned(e))),
                Err(p) => failures.push(failure(entry.name, HookFailureKind::Panic(panic_msg(p)))),
            }
        }
        HookRun::continue_with(failures)
    }
}

fn failure(hook: &'static str, kind: HookFailureKind) -> HookFailure {
    HookFailure { hook, kind }
}

fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    crate::audit::panic_payload_msg(payload.as_ref())
}

impl HookRun {
    fn continue_with(failures: Vec<HookFailure>) -> HookRun {
        HookRun {
            decision: PreToolVerdict::Continue,
            failures,
        }
    }
}

static REGISTRY: OnceLock<HookRegistry> = OnceLock::new();

/// 进程级注册表：首次取用时装配内置钩子。
pub fn registry() -> &'static HookRegistry {
    REGISTRY.get_or_init(HookRegistry::with_builtins)
}

/// 内置钩子的累计命中次数（进程内单调递增）。
pub fn builtin_calls() -> u64 {
    BUILTIN_CALLS.load(Ordering::Relaxed)
}

static BUILTIN_CALLS: AtomicU64 = AtomicU64::new(0);

/// 内置钩子：seam 观察者。计数 +1，并在 bot.log 留一行
/// （`log::info!` 经 AuditLogger 直落 bot.log，因此不需要 AppHandle，
/// 钩子 trait 才不必带 `&AppHandle` 参数、也才保持得住对象安全）。
///
/// 它存在的意义是让 seam 在生产里**可观察**：日志里出现 `bot::hooks` 行即
/// 「三道闸入口接线生效」。不记 args——`tool.call` 已经留痕 args_preview，
/// 同一份入参不重复落盘。
struct BuiltinObserver;

impl PreToolHook for BuiltinObserver {
    fn name(&self) -> &'static str {
        "builtin_observer"
    }

    fn on_pre_tool(&self, call: &ToolCallView<'_>) -> Result<PreToolVerdict, String> {
        BUILTIN_CALLS.fetch_add(1, Ordering::Relaxed);
        log::info!(target: "bot::hooks", "pre_tool hook hit: {}", call.tool);
        Ok(PreToolVerdict::Continue)
    }
}

/// L5 内置观察钩子：只计数 + 一行日志，**不改写**。
///
/// 它存在的意义与 L4 那个一样：让 seam 在生产日志里可观察（日志里出现
/// `post_tool hook hit` 即接线生效）。v1 不装任何清洗/脱敏逻辑——那属于
/// 用户配置的钩子（onion yaml），编造默认行为等于替用户做了没授权的决定。
struct BuiltinPostToolObserver;

static POST_TOOL_CALLS: AtomicU64 = AtomicU64::new(0);

impl PostToolHook for BuiltinPostToolObserver {
    fn name(&self) -> &'static str {
        "builtin_post_tool_observer"
    }

    fn on_post_tool(&self, call: &ToolCallView<'_>, _result: &mut String) -> Result<(), String> {
        POST_TOOL_CALLS.fetch_add(1, Ordering::Relaxed);
        log::info!(target: "bot::hooks", "post_tool hook hit: {}", call.tool);
        Ok(())
    }
}

/// L6 内置观察钩子：只计数 + 一行日志，**不改写**（同 L5 的理由）。
struct BuiltinPostTurnObserver;

static POST_TURN_CALLS: AtomicU64 = AtomicU64::new(0);

impl PostTurnHook for BuiltinPostTurnObserver {
    fn name(&self) -> &'static str {
        "builtin_post_turn_observer"
    }

    fn on_post_turn(&self, _reply: &mut String, round: usize) -> Result<(), String> {
        POST_TURN_CALLS.fetch_add(1, Ordering::Relaxed);
        log::info!(target: "bot::hooks", "post_turn hook hit: round={round}");
        Ok(())
    }
}

/// L5 内置钩子的累计命中次数（进程内单调递增）。
pub fn builtin_post_tool_calls() -> u64 {
    POST_TOOL_CALLS.load(Ordering::Relaxed)
}

/// L6 内置钩子的累计命中次数（进程内单调递增）。
pub fn builtin_post_turn_calls() -> u64 {
    POST_TURN_CALLS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 计数钩子：把「被调用」变成单测可断言的事实。
    struct CountingHook {
        name: &'static str,
        calls: Arc<AtomicUsize>,
        order: Option<Arc<Mutex<Vec<&'static str>>>>,
        verdict: PreToolVerdict,
        failure: Option<Result<(), String>>,
    }

    impl CountingHook {
        fn new(name: &'static str, calls: Arc<AtomicUsize>) -> Self {
            Self {
                name,
                calls,
                order: None,
                verdict: PreToolVerdict::Continue,
                failure: None,
            }
        }

        fn with_order(mut self, order: Arc<Mutex<Vec<&'static str>>>) -> Self {
            self.order = Some(order);
            self
        }

        fn interrupting(mut self, reason: &str) -> Self {
            self.verdict = PreToolVerdict::Interrupt {
                reason: reason.to_string(),
            };
            self
        }

        fn failing(mut self, err: &str) -> Self {
            self.failure = Some(Err(err.to_string()));
            self
        }
    }

    impl PreToolHook for CountingHook {
        fn name(&self) -> &'static str {
            self.name
        }

        fn on_pre_tool(&self, _call: &ToolCallView<'_>) -> Result<PreToolVerdict, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(order) = &self.order {
                order
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(self.name);
            }
            if let Some(Err(e)) = &self.failure {
                return Err(e.clone());
            }
            Ok(self.verdict.clone())
        }
    }

    /// panic 与返 Err 走的是 fail-open 的两条不同分支，分开计。
    struct PanicHook(Arc<AtomicUsize>);

    impl PreToolHook for PanicHook {
        fn name(&self) -> &'static str {
            "panicker"
        }

        fn on_pre_tool(&self, _call: &ToolCallView<'_>) -> Result<PreToolVerdict, String> {
            self.0.fetch_add(1, Ordering::Relaxed);
            panic!("hook 内 panic");
        }
    }

    fn view<'a>(
        args: &'a str,
        session_id: Option<&'a str>,
        trace: &'a ToolCallTrace,
    ) -> ToolCallView<'a> {
        ToolCallView {
            tool: "read_text_file",
            args,
            session_id,
            trace,
        }
    }

    fn trace() -> ToolCallTrace {
        ToolCallTrace {
            turn: Some(3),
            tool_call_id: Some("call_1".into()),
        }
    }

    fn continue_() -> PreToolVerdict {
        PreToolVerdict::Continue
    }

    #[test]
    fn registered_hook_runs_once_per_tool_call_in_registration_order() {
        let tr = trace();
        let v = view(r#"{"path":"a.txt"}"#, Some("s1"), &tr);
        let calls = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut reg = HookRegistry::default();
        reg.register(Box::new(
            CountingHook::new("first", Arc::clone(&calls)).with_order(Arc::clone(&order)),
        ));
        reg.register(Box::new(
            CountingHook::new("second", Arc::clone(&calls)).with_order(Arc::clone(&order)),
        ));

        let run = reg.run_pre_tool(&v);

        assert_eq!(run.decision, continue_());
        assert!(run.failures.is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 2, "两个钩子都必须被调用");
        let seen = order.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(seen, ["first", "second"], "同层内按注册序");
    }

    #[test]
    fn view_borrows_args_and_trace_instead_of_copying() {
        // 只读视图不得复制 args / trace：复制会与 dispatch 侧的真值漂移。
        let tr = trace();
        let v = view(r#"{"path":"a.txt"}"#, Some("s1"), &tr);
        assert_eq!(v.args, r#"{"path":"a.txt"}"#);
        assert_eq!(v.tool, "read_text_file");
        assert_eq!(v.session_id, Some("s1"));
        assert_eq!(v.trace.turn, tr.turn);
        assert_eq!(v.trace.tool_call_id, tr.tool_call_id);
    }

    #[test]
    fn hook_returning_err_fails_open_and_later_hooks_still_run() {
        let tr = trace();
        let v = view("{}", Some("s1"), &tr);
        let bad = Arc::new(AtomicUsize::new(0));
        let good = Arc::new(AtomicUsize::new(0));
        let mut reg = HookRegistry::default();
        reg.register(Box::new(
            CountingHook::new("bad", Arc::clone(&bad)).failing("hook 内部读配置失败"),
        ));
        reg.register(Box::new(CountingHook::new("good", Arc::clone(&good))));

        let run = reg.run_pre_tool(&v);

        assert_eq!(
            run.decision,
            continue_(),
            "钩子返 Err 不得改成拒绝（fail-open）"
        );
        assert_eq!(run.failures.len(), 1);
        assert_eq!(run.failures[0].hook, "bad");
        assert_eq!(
            run.failures[0].kind,
            HookFailureKind::Returned("hook 内部读配置失败".into())
        );
        assert_eq!(good.load(Ordering::Relaxed), 1, "失败钩子不得挡住后续钩子");
    }

    #[test]
    fn panicking_hook_is_caught_and_fails_open() {
        let tr = trace();
        let v = view("{}", Some("s1"), &tr);
        let calls = Arc::new(AtomicUsize::new(0));
        let after = Arc::new(AtomicUsize::new(0));
        let mut reg = HookRegistry::default();
        reg.register(Box::new(PanicHook(Arc::clone(&calls))));
        reg.register(Box::new(CountingHook::new("after", Arc::clone(&after))));

        let run = reg.run_pre_tool(&v);

        assert_eq!(run.decision, continue_(), "panic 不得中断主链路");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(after.load(Ordering::Relaxed), 1, "panic 后仍要跑后续钩子");
        assert_eq!(run.failures.len(), 1);
        assert_eq!(run.failures[0].hook, "panicker");
        match &run.failures[0].kind {
            HookFailureKind::Panic(msg) => assert_eq!(msg, "hook 内 panic"),
            other => panic!("期望 Panic 失败分支，实际 {other:?}"),
        }
    }

    #[test]
    fn interrupt_short_circuits_later_hooks_and_keeps_reason() {
        let tr = trace();
        let v = view("{}", Some("s1"), &tr);
        let later = Arc::new(AtomicUsize::new(0));
        let mut reg = HookRegistry::default();
        reg.register(Box::new(
            CountingHook::new("gate", Arc::new(AtomicUsize::new(0))).interrupting("本轮不允许写盘"),
        ));
        reg.register(Box::new(CountingHook::new("later", Arc::clone(&later))));

        let run = reg.run_pre_tool(&v);

        assert_eq!(
            run.decision,
            PreToolVerdict::Interrupt {
                reason: "本轮不允许写盘".into()
            }
        );
        assert_eq!(later.load(Ordering::Relaxed), 0, "拒绝后不再跑后续钩子");
    }

    #[test]
    fn disabled_layer_runs_no_hook_for_that_session_only() {
        let tr = trace();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = HookRegistry::default();
        reg.register(Box::new(CountingHook::new("counter", Arc::clone(&calls))));
        reg.disable_layer_for_session(HookLayer::PreTool, "s1");

        let off = reg.run_pre_tool(&view("{}", Some("s1"), &tr));
        assert_eq!(off.decision, continue_());
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "被禁用的层不得触发任何钩子"
        );

        let other = reg.run_pre_tool(&view("{}", Some("s2"), &tr));
        assert_eq!(other.decision, continue_());
        assert_eq!(calls.load(Ordering::Relaxed), 1, "开关只作用于本会话");

        let no_session = reg.run_pre_tool(&view("{}", None, &tr));
        assert_eq!(no_session.decision, continue_());
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "无会话上下文不参与会话级开关"
        );
    }

    #[test]
    fn builtin_hook_is_registered_and_counts_hits() {
        let tr = trace();
        let v = view("{}", Some("s1"), &tr);
        // 进程级注册表是全局单例：断言增量而不是绝对值（并行单测会各跑一次）。
        let before = builtin_calls();
        let run = registry().run_pre_tool(&v);
        assert_eq!(run.decision, continue_());
        assert!(run.failures.is_empty());
        assert!(
            builtin_calls() > before,
            "内置钩子必须真被跑到：{} -> {}",
            before,
            builtin_calls()
        );
        // 三层各有一个内置观察钩子（L4 期 1 起，L5/L6 期 2 起）
        let builtins = HookRegistry::with_builtins();
        assert_eq!(builtins.hooks.len(), 3, "每层一个内置观察钩子");
        for layer in [HookLayer::PreTool, HookLayer::PostTool, HookLayer::PostTurn] {
            assert_eq!(
                builtins.hooks.iter().filter(|e| e.layer == layer).count(),
                1,
                "{layer:?} 层应恰有一个内置钩子"
            );
        }
    }

    #[test]
    fn l4_hook_input_is_shared_borrow_at_type_level() {
        // 「改不了」本身无法用运行时断言证明（写不出那种代码才是证明），
        // 这里钉住类型形状：钩子入参是共享借用、视图字段全是共享借用，
        // 出现任何可变引用或 String/Value 副本即红。
        let src = include_str!("hooks.rs");
        assert!(
            src.contains("fn on_pre_tool(&self, call: &ToolCallView<'_>)"),
            "钩子入参必须是 &ToolCallView（只读），不得是 &mut：{src}"
        );
        let view_struct = src
            .split_once("pub struct ToolCallView<'a> {")
            .expect("找不到 ToolCallView 定义")
            .1
            .split("\n}\n")
            .next()
            .expect("ToolCallView 应以顶格 } 收尾");
        for line in view_struct.lines().filter(|l| l.contains("pub ")) {
            assert!(
                line.contains("&'a str") || line.contains("&'a ToolCallTrace"),
                "视图字段只允许共享借用，不得持有可变引用或值副本：{line}"
            );
        }
        // 视图字段「只读」的最终根据在 ToolCallTrace 自身：它是共享借用，
        // 但若哪天给它塞进 Cell/RefCell/Mutex 或加 DerefMut，「类型层面
        // 改不了」就是假的，而上面那条断言照样绿。只读必须钉在真正的可变源头上。
        let trace_src = include_str!("dispatch.rs");
        let trace_struct = trace_src
            .split_once("pub struct ToolCallTrace {")
            .expect("找不到 ToolCallTrace 定义")
            .1
            .split("\n}\n")
            .next()
            .expect("ToolCallTrace 应以顶格 } 收尾");
        for bad in [
            "Cell",
            "RefCell",
            "Mutex",
            "RwLock",
            "UnsafeCell",
            "DerefMut",
        ] {
            assert!(
                !trace_struct.contains(bad),
                "ToolCallTrace 出现 {bad}：钩子拿到的 trace 就不是真只读了：{trace_struct}"
            );
        }
        // 注意别把禁用词钉成「impl DerefMut for ToolCallTrace」这种带前缀的
        // 完整写法——变异实测写成 `impl std::ops::DerefMut for ToolCallTrace`
        // 就整条断言失效。只钉与限定路径无关的那一段。
        for bad in ["Deref for ToolCallTrace", "DerefMut for ToolCallTrace"] {
            assert!(
                !trace_src.contains(bad),
                "ToolCallTrace 实现了 {bad}：钩子拿到的 trace 就不是真只读了"
            );
        }
    }

    #[test]
    fn registry_has_no_way_to_configure_layer_order() {
        // 纪律 1 的结构锁：注册表不暴露任何「层顺序 / 层优先级」入口——
        // 层序只能来自调用栈。扫描范围排除测试段（禁用词本身是测试里的字面量）。
        // 说清边界：这是**禁用词扫描**不是结构锁（仓内没有 trybuild 负向编译
        // 测试，不为它新增依赖）。带 `sort_by*` / `impl Ord` / `rank` 字段的
        // 排序入口一并列入，正是因为它们都能绕过前面几个名字。
        let src = include_str!("hooks.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("hooks.rs 应有非测试段");
        for forbidden in [
            "layer_order",
            "layer_priority",
            "sort_by_layer",
            "LAYER_ORDER",
            "sort_by",
            "sort_by_key",
            "sort_unstable",
            "impl Ord",
            "impl PartialOrd",
            "rank",
            "priority",
        ] {
            assert!(
                !src.contains(forbidden),
                "注册表不得出现层序配置入口（{forbidden}）"
            );
        }
    }
    // ── 期 2：L5 / L6 两层 ──

    struct PostToolRewrite {
        calls: Arc<AtomicUsize>,
        failure: Option<Result<(), String>>,
    }

    impl PostToolHook for PostToolRewrite {
        fn name(&self) -> &'static str {
            "rewrite"
        }
        fn on_post_tool(
            &self,
            _call: &ToolCallView<'_>,
            result: &mut String,
        ) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(Err(ref e)) = self.failure {
                return Err(e.clone());
            }
            *result = format!("[脱敏]{result}");
            Ok(())
        }
    }

    struct PostTurnAppend {
        calls: Arc<AtomicUsize>,
    }

    impl PostTurnHook for PostTurnAppend {
        fn name(&self) -> &'static str {
            "append"
        }
        fn on_post_turn(&self, reply: &mut String, round: usize) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            *reply = format!("{reply}\n[第{round}轮]");
            Ok(())
        }
    }

    fn fresh_registry() -> HookRegistry {
        HookRegistry::default()
    }

    fn call_view<'a>(trace: &'a ToolCallTrace, session_id: Option<&'a str>) -> ToolCallView<'a> {
        ToolCallView {
            tool: "read_text_file",
            args: "{}",
            session_id,
            trace,
        }
    }

    #[test]
    fn post_tool_hook_can_rewrite_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = fresh_registry();
        reg.register_post_tool(Box::new(PostToolRewrite {
            calls: calls.clone(),
            failure: None,
        }));
        let trace = ToolCallTrace::default();
        let mut text = "手机号 13800000000".to_string();
        let run = reg.run_post_tool(&call_view(&trace, Some("s1")), &mut text);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(text, "[脱敏]手机号 13800000000");
        assert!(run.failures.is_empty(), "{:?}", run.failures);
        // L5 不判拒绝：decision 恒 Continue（工具已经跑完了）
        assert_eq!(run.decision, PreToolVerdict::Continue);
    }

    #[test]
    fn post_tool_hook_returning_err_fails_open_and_leaves_text_intact() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = fresh_registry();
        reg.register_post_tool(Box::new(PostToolRewrite {
            calls: calls.clone(),
            failure: Some(Err("脱敏规则编译失败".into())),
        }));
        let trace = ToolCallTrace::default();
        let mut text = "原文".to_string();
        let run = reg.run_post_tool(&call_view(&trace, Some("s1")), &mut text);
        assert_eq!(calls.load(Ordering::Relaxed), 1, "失败后仍算被调用过");
        assert_eq!(text, "原文", "失败的钩子不得留下半成品");
        assert_eq!(run.failures.len(), 1);
        assert!(matches!(run.failures[0].kind, HookFailureKind::Returned(_)));
    }

    #[test]
    fn post_turn_hook_can_rewrite_final_reply_and_receives_round() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = fresh_registry();
        reg.register_post_turn(Box::new(PostTurnAppend {
            calls: calls.clone(),
        }));
        let mut reply = "答复".to_string();
        let run = reg.run_post_turn(Some("s1"), &mut reply, 7);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(reply, "答复\n[第7轮]");
        assert!(run.failures.is_empty());
    }

    #[test]
    fn each_layer_only_runs_its_own_hooks() {
        // 层间不串：一个 pre_tool 钩子不该被 post_tool 的调用带起来，反之亦然。
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = fresh_registry();
        reg.register(Box::new(CountingHook {
            name: "pre",
            calls: calls.clone(),
            order: None,
            verdict: PreToolVerdict::Continue,
            failure: None,
        }));
        reg.register_post_tool(Box::new(PostToolRewrite {
            calls: calls.clone(),
            failure: None,
        }));
        let trace = ToolCallTrace::default();
        let mut text = String::new();
        reg.run_post_tool(&call_view(&trace, Some("s1")), &mut text);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "post_tool 只能碰到 post_tool 钩子"
        );
    }

    #[test]
    fn disabled_layer_is_skipped_for_post_layers_too() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut reg = fresh_registry();
        reg.register_post_tool(Box::new(PostToolRewrite {
            calls: calls.clone(),
            failure: None,
        }));
        reg.register_post_turn(Box::new(PostTurnAppend {
            calls: calls.clone(),
        }));
        reg.disable_layer_for_session(HookLayer::PostTool, "s-off");
        reg.disable_layer_for_session(HookLayer::PostTurn, "s-off");
        let trace = ToolCallTrace::default();

        let mut text = "原文".to_string();
        reg.run_post_tool(&call_view(&trace, Some("s-off")), &mut text);
        let mut reply = "答复".to_string();
        reg.run_post_turn(Some("s-off"), &mut reply, 1);
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "被禁用的层一个钩子都不该跑"
        );
        assert_eq!(text, "原文");
        assert_eq!(reply, "答复");

        // 别的会话不受影响
        let mut text2 = "原文".to_string();
        reg.run_post_tool(&call_view(&trace, Some("s-on")), &mut text2);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(text2, "[脱敏]原文");
    }

    /// 期 2 的核心不变式：**三层签名互不通用**，类型层面挡住跨层拿错 payload。
    ///
    /// 变异实测把 `on_post_tool` 的 `result: &mut String` 改成 `&str`（与文档
    /// §四「单 trait + HookInput 枚举」靠运行时 match 区分层相比，这里连改写的
    /// 能力都拿不到）时，下面的签名断言即红。L4 那一侧另有既有测试钉住
    /// 「入参是共享借用」。
    #[test]
    fn layer_traits_have_mutually_incompatible_signatures() {
        let src = include_str!("hooks.rs");
        let pre = src
            .split_once("pub trait PreToolHook")
            .expect("PreToolHook 定义")
            .1
            .split("\n}\n")
            .next()
            .expect("PreToolHook 应以顶格 } 收尾");
        let post_tool = src
            .split_once("pub trait PostToolHook")
            .expect("PostToolHook 定义")
            .1
            .split("\n}\n")
            .next()
            .expect("PostToolHook 应以顶格 } 收尾");
        let post_turn = src
            .split_once("pub trait PostTurnHook")
            .expect("PostTurnHook 定义")
            .1
            .split("\n}\n")
            .next()
            .expect("PostTurnHook 应以顶格 } 收尾");

        // L4：只读，且返回可拒绝的判定
        assert!(
            pre.contains("call: &ToolCallView<'_>") && !pre.contains("&mut"),
            "L4 入参必须全是共享借用：{pre}"
        );
        assert!(
            pre.contains("Result<PreToolVerdict, String>"),
            "L4 必须能返回 Interrupt 判定：{pre}"
        );
        // L5：可变结果，但不可拒绝
        assert!(
            post_tool.contains("result: &mut String"),
            "L5 必须能改工具结果：{post_tool}"
        );
        assert!(
            post_tool.contains("Result<(), String>"),
            "L5 不得能返回 Interrupt（工具已经跑完）：{post_tool}"
        );
        assert!(
            !post_tool.contains("&mut ToolCallView"),
            "L5 仍不得改写 tool_call 本身：{post_tool}"
        );
        // L6：可变回复，带轮次
        assert!(
            post_turn.contains("reply: &mut String"),
            "L6 必须能改最终回复：{post_turn}"
        );
        assert!(
            post_turn.contains("round: usize"),
            "L6 要拿到轮次（每 N 轮做一次类钩子需要）：{post_turn}"
        );
        assert!(
            post_turn.contains("Result<(), String>"),
            "L6 不得能返回 Interrupt：{post_turn}"
        );
    }

    /// `AnyHook` 变体必须与它声明的层一一对应：注册表按层过滤时，
    /// 变体错配会让钩子在该层被静默跳过（`let ... else { continue }`）。
    #[test]
    fn any_hook_variant_matches_its_layer() {
        let mut reg = fresh_registry();
        reg.register(Box::new(BuiltinObserver));
        reg.register_post_tool(Box::new(BuiltinPostToolObserver));
        reg.register_post_turn(Box::new(BuiltinPostTurnObserver));
        let by_layer = |l: HookLayer| -> Vec<(HookLayer, &'static str)> {
            reg.hooks
                .iter()
                .map(|e| {
                    (
                        e.layer,
                        match &e.hook {
                            AnyHook::Pre(_) => HookLayer::PreTool,
                            AnyHook::PostTool(_) => HookLayer::PostTool,
                            AnyHook::PostTurn(_) => HookLayer::PostTurn,
                        },
                    )
                })
                .filter(|(declared, actual)| *declared == *actual)
                .collect::<Vec<_>>()
                .into_iter()
                .filter(|(_, a)| *a == l)
                .map(|(d, _)| (d, a_layer_name(d)))
                .collect()
        };
        fn a_layer_name(l: HookLayer) -> &'static str {
            l.name()
        }
        assert_eq!(by_layer(HookLayer::PreTool).len(), 1);
        assert_eq!(by_layer(HookLayer::PostTool).len(), 1);
        assert_eq!(by_layer(HookLayer::PostTurn).len(), 1);
    }

    #[test]
    fn builtin_observers_are_reachable_in_production_registry() {
        // 内置三层观察钩子都在册（期 1 只有一个，这里补齐另外两层）
        let reg = registry();
        for (layer, count) in [
            (HookLayer::PreTool, builtin_calls()),
            (HookLayer::PostTool, builtin_post_tool_calls()),
            (HookLayer::PostTurn, builtin_post_turn_calls()),
        ] {
            assert!(
                reg.hooks.iter().any(|e| e.layer == layer),
                "{layer:?} 层应有内置观察钩子"
            );
            let _ = count;
        }
    }
}
