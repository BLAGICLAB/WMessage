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
    /// L4：工具执行前（三道闸入口）。
    PreTool,
}

impl HookLayer {
    /// 已接线层的名（与配置层名一一对应）。
    pub fn name(&self) -> &'static str {
        match self {
            HookLayer::PreTool => "pre_tool",
        }
    }

    /// 配置层名 → 已接线层。返回 None = 该层在设计里存在但**尚未接线**，
    /// 调用方必须把它当「没关成」记审计，不能静默当成功——一份配置写着
    /// `post_tool: ""` 却没关掉任何东西，正是「配置看着生效、实际落空」。
    pub fn from_name(name: &str) -> Option<HookLayer> {
        match name {
            "pre_tool" => Some(HookLayer::PreTool),
            _ => None,
        }
    }

    /// 注册表已接线的层名全集。
    pub fn wired_names() -> &'static [&'static str] {
        const WIRED: &[&str] = &["pre_tool"];
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

struct HookEntry {
    layer: HookLayer,
    name: &'static str,
    hook: Box<dyn PreToolHook>,
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
    /// 注册一个钩子到指定层：同层内后注册的后跑。
    pub fn register(&mut self, layer: HookLayer, hook: Box<dyn PreToolHook>) {
        let name = hook.name();
        self.hooks.push(HookEntry { layer, name, hook });
    }

    /// 进程级注册表的初始内容：装上内置钩子。
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        r.register(HookLayer::PreTool, Box::new(BuiltinObserver));
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
    pub fn run_layer(&self, layer: HookLayer, call: &ToolCallView<'_>) -> HookRun {
        let mut failures = Vec::new();
        if !self.layer_enabled(layer, call.session_id) {
            return HookRun {
                decision: PreToolVerdict::Continue,
                failures,
            };
        }
        let mut decision = PreToolVerdict::Continue;
        for entry in self.hooks.iter().filter(|e| e.layer == layer) {
            if matches!(decision, PreToolVerdict::Interrupt { .. }) {
                break;
            }
            match std::panic::catch_unwind(AssertUnwindSafe(|| entry.hook.on_pre_tool(call))) {
                Ok(Ok(verdict)) => decision = verdict,
                Ok(Err(e)) => failures.push(HookFailure {
                    hook: entry.name,
                    kind: HookFailureKind::Returned(e),
                }),
                Err(payload) => failures.push(HookFailure {
                    hook: entry.name,
                    kind: HookFailureKind::Panic(crate::audit::panic_payload_msg(payload.as_ref())),
                }),
            }
        }
        HookRun { decision, failures }
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
        reg.register(
            HookLayer::PreTool,
            Box::new(CountingHook::new("first", Arc::clone(&calls)).with_order(Arc::clone(&order))),
        );
        reg.register(
            HookLayer::PreTool,
            Box::new(
                CountingHook::new("second", Arc::clone(&calls)).with_order(Arc::clone(&order)),
            ),
        );

        let run = reg.run_layer(HookLayer::PreTool, &v);

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
        reg.register(
            HookLayer::PreTool,
            Box::new(CountingHook::new("bad", Arc::clone(&bad)).failing("hook 内部读配置失败")),
        );
        reg.register(
            HookLayer::PreTool,
            Box::new(CountingHook::new("good", Arc::clone(&good))),
        );

        let run = reg.run_layer(HookLayer::PreTool, &v);

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
        reg.register(HookLayer::PreTool, Box::new(PanicHook(Arc::clone(&calls))));
        reg.register(
            HookLayer::PreTool,
            Box::new(CountingHook::new("after", Arc::clone(&after))),
        );

        let run = reg.run_layer(HookLayer::PreTool, &v);

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
        reg.register(
            HookLayer::PreTool,
            Box::new(
                CountingHook::new("gate", Arc::new(AtomicUsize::new(0)))
                    .interrupting("本轮不允许写盘"),
            ),
        );
        reg.register(
            HookLayer::PreTool,
            Box::new(CountingHook::new("later", Arc::clone(&later))),
        );

        let run = reg.run_layer(HookLayer::PreTool, &v);

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
        reg.register(
            HookLayer::PreTool,
            Box::new(CountingHook::new("counter", Arc::clone(&calls))),
        );
        reg.disable_layer_for_session(HookLayer::PreTool, "s1");

        let off = reg.run_layer(HookLayer::PreTool, &view("{}", Some("s1"), &tr));
        assert_eq!(off.decision, continue_());
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "被禁用的层不得触发任何钩子"
        );

        let other = reg.run_layer(HookLayer::PreTool, &view("{}", Some("s2"), &tr));
        assert_eq!(other.decision, continue_());
        assert_eq!(calls.load(Ordering::Relaxed), 1, "开关只作用于本会话");

        let no_session = reg.run_layer(HookLayer::PreTool, &view("{}", None, &tr));
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
        let run = registry().run_layer(HookLayer::PreTool, &v);
        assert_eq!(run.decision, continue_());
        assert!(run.failures.is_empty());
        assert!(
            builtin_calls() > before,
            "内置钩子必须真被跑到：{} -> {}",
            before,
            builtin_calls()
        );
        assert_eq!(
            HookRegistry::with_builtins().hooks.len(),
            1,
            "本期只有一个内置钩子"
        );
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
}
