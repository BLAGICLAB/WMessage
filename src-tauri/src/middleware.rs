//! F-2 Plugin/Extension 抽象层（2026-08-18 P2）
//!
//! ## 动机
//! 审计发现「业务 Skill 禁止注册底层中间件钩子」只是口头约束，没有编译期隔离。
//! 引入 `Middleware` trait + `MiddlewareRegistry` 让「中间件注册」成为受控行为：
//! - `lib.rs` setup 阶段注册 3 个内置中间件（选择任务卡批量执行路由 + 意图路由 + 原子黑名单）
//! - 业务模块（bot_skills.rs）只能通过 helper 函数查询，不能 register
//!
//! ## 设计
//! - `Middleware` trait 提供 name / pre_step / pre_execute 三个方法
//! - `MiddlewareRegistry` 用 Vec<Box<dyn Middleware>> 存储，短路求值
//! - helper 函数 run_pre_step / run_pre_execute 通过 Tauri State 访问 registry
//!
//! ## fail 语义（D2，2026-08-19）
//! state 未 manage（测试、初始化竞态）时两类中间件区别对待：
//! - 安全闸门类（pre_execute / AtomicGuard）→ **fail-closed**：原子工具一律拒绝 + ERROR 审计，
//!   安全闸门缺席时绝不能静默放行原子工具
//! - 业务路由类（pre_step / IntentRouter）→ **fail-open**：回退 None（legacy passthrough），
//!   无锁语义不影响业务，上层照常走模型直接对话

use crate::intent_router::{route_user_input, RouteAction};
use crate::tool_guard::{atomic_block_message, is_atomic_tool};
use tauri::Manager; // F-6：泛型 Runtime 以适配 mock_runtime 集成测试

/// 中间件 trait（F-2 抽象层核心）
/// 任何「可插拔行为」都实现这个 trait，然后通过 `MiddlewareRegistry::register_*` 注册
pub trait Middleware: Send + Sync {
    /// 可读标识，用于 introspect / 日志 / 设置页 UI 展示
    fn name(&self) -> &str;
    /// 用户输入预处理：返回 Some(RouteAction) 触发短路求值，None 继续下一个中间件
    /// 返回 RouteAction::PassThrough 也算「命中」并触发短路
    fn pre_step(&self, input: &str) -> Option<RouteAction>;
    /// 工具调用前检查：返回 Some(阻断消息) 阻断，None 继续下一个中间件
    fn pre_execute(&self, name: &str, active_skill: bool) -> Option<String>;
}

/// 中间件注册表（F-2 P2）
/// 注册仅在 setup 阶段（lib.rs）发生；业务模块只能通过 helper 函数查询
pub struct MiddlewareRegistry {
    pre_step: Vec<Box<dyn Middleware>>,
    pre_execute: Vec<Box<dyn Middleware>>,
}

impl Default for MiddlewareRegistry {
    fn default() -> Self {
        Self {
            pre_step: Vec::new(),
            pre_execute: Vec::new(),
        }
    }
}

impl MiddlewareRegistry {
    /// 注册 pre-step 中间件（按注册顺序短路求值）
    /// D1：IntentRouterMiddleware 恒返回 Some 短路整条链，排它之后的中间件全是
    /// 死代码（拿不到调用）。这里把「运行时静默死亡」提前成「注册即 panic」。
    pub fn register_pre_step(&mut self, m: Box<dyn Middleware>) {
        assert!(
            self.pre_step.last().map(|last| last.name()) != Some(INTENT_ROUTER_NAME),
            "IntentRouterMiddleware 恒返回 Some 短路 pre_step 链，必须最后注册；禁止在其后追加 {}",
            m.name()
        );
        self.pre_step.push(m);
    }
    /// 注册 pre-execute 中间件
    pub fn register_pre_execute(&mut self, m: Box<dyn Middleware>) {
        self.pre_execute.push(m);
    }
    /// pre-step 短路求值：任一中间件返回 Some(_)
    /// P2-13：catch_unwind 兜底——中间件 panic 不再炸掉调用方所在的
    /// tauri::async_runtime worker 线程；记 ERROR 审计后按「未命中」处理，继续下一个
    pub fn run_pre_step<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        input: &str,
    ) -> Option<RouteAction> {
        for m in &self.pre_step {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.pre_step(input))) {
                Ok(Some(action)) => return Some(action),
                Ok(None) => {}
                Err(payload) => {
                    let msg = panic_message(payload);
                    crate::audit::write_error_audit(
                        app,
                        "middleware_panic",
                        &[("hook", "pre_step"), ("middleware", m.name()), ("panic", &msg)],
                    );
                }
            }
        }
        None
    }
    /// pre-execute 短路求值：任一中间件返回 Some(msg)
    /// P2-13：同 run_pre_step，panic 兜住记审计后按「不阻断」继续下一个
    /// P2-14：pre_step / pre_execute 双 Vec 分离，漏注册一边会静默半生效——
    /// 空注册表被调用时记 ERROR 审计（pre_execute_not_registered），不再无声放行
    pub fn run_pre_execute<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        name: &str,
        active_skill: bool,
    ) -> Option<String> {
        if self.pre_execute.is_empty() {
            crate::audit::write_error_audit(
                app,
                "pre_execute_not_registered",
                &[("tool", name)],
            );
            return None;
        }
        for m in &self.pre_execute {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                m.pre_execute(name, active_skill)
            })) {
                Ok(Some(msg)) => return Some(msg),
                Ok(None) => {}
                Err(payload) => {
                    let msg = panic_message(payload);
                    crate::audit::write_error_audit(
                        app,
                        "middleware_panic",
                        &[
                            ("hook", "pre_execute"),
                            ("middleware", m.name()),
                            ("panic", &msg),
                        ],
                    );
                }
            }
        }
        None
    }
    /// introspect：列出已注册 pre-step 中间件名（设置页 UI 用，F-2 后续接入）
    #[allow(dead_code)] // SettingsPage 扩展面板接入后用
    pub fn pre_step_list(&self) -> Vec<String> {
        self.pre_step.iter().map(|m| m.name().to_string()).collect()
    }
    /// introspect：列出已注册 pre-execute 中间件名（F-2 后续接入）
    #[allow(dead_code)] // SettingsPage 扩展面板接入后用
    pub fn pre_execute_list(&self) -> Vec<String> {
        self.pre_execute
            .iter()
            .map(|m| m.name().to_string())
            .collect()
    }
}

/// 构建默认注册表：注册 3 个内置中间件（lib.rs setup 调用）
pub fn build_default_registry() -> MiddlewareRegistry {
    let mut r = MiddlewareRegistry::default();
    // 选择任务卡批量执行路由（2026-08-20 收编主流程）：排在 IntentRouter 之前——
    // 「完成/执行」+ [已选任务] 引用块命中时直接短路为 ExecuteTasks，不再查技能路由表；
    // 未命中返回 None，链条继续走到 IntentRouter
    r.register_pre_step(Box::new(ChatExecuteMiddleware));
    r.register_pre_step(Box::new(IntentRouterMiddleware));
    r.register_pre_execute(Box::new(AtomicGuardMiddleware));
    // D1：显式断言 IntentRouterMiddleware 是 pre_step 链的最后一个——
    // 它恒返回 Some 短路全链，顺序错了后续中间件静默失效。
    // （register_pre_step 内部已拒绝「在其后追加」，这里再钉一次防未来重排顺序）
    assert!(
        r.pre_step.last().map(|m| m.name()) == Some(INTENT_ROUTER_NAME),
        "IntentRouterMiddleware 必须注册为 pre_step 链最后一个"
    );
    r
}

/// helper：通过 Tauri State 调 run_pre_step（state 未 manage 时回退 None = legacy passthrough）
/// NEW-D-6：泛型 Runtime，与文件头「适配 mock_runtime」注释一致，D2 fail-open 回退可用 mock 单测
/// D2：业务路由类 fail-open——无锁语义不影响业务，None 让上层走模型直接对话
pub fn run_pre_step<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    input: &str,
) -> Option<RouteAction> {
    match app.try_state::<MiddlewareRegistry>() {
        Some(state) => state.run_pre_step(app, input),
        None => None,
    }
}

/// helper：通过 Tauri State 调 run_pre_execute
/// D2：安全闸门类 **fail-closed**——state 未 manage 时原子工具一律拒绝并记 ERROR 审计，
/// 不能静默放行（registry 缺失 = AtomicGuard 缺席 = 原子工具失去唯一拦截点）。
/// 非原子工具没有闸门诉求，仍 fail-open 回退 None；Skill 活动态与 AtomicGuard 判定口径一致。
pub fn run_pre_execute<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    name: &str,
    active_skill: bool,
) -> Option<String> {
    match app.try_state::<MiddlewareRegistry>() {
        Some(state) => state.run_pre_execute(app, name, active_skill),
        None => {
            if is_atomic_tool(name) && !active_skill {
                crate::audit::write_error_audit(
                    app,
                    "middleware_registry_missing",
                    &[("tool", name), ("gate", "atomic_guard")],
                );
                Some(format!(
                    "⚠️ 安全闸门未初始化（MiddlewareRegistry 未注册），拒绝原子工具 {name} 的直接调用。请通过对应 Skill 执行。"
                ))
            } else {
                None
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// 内置中间件：包装现有 intent_router / tool_guard
// ────────────────────────────────────────────────────────────────────

/// P2-13：从 catch_unwind payload 提取 panic 信息（&str / String / 其他三种情况）
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// 内置：选择任务卡批量执行路由中间件（2026-08-20 收编主流程）
///
/// 原为 bot_chat 内部前置短路（在 bypass 开关读取与 pre-step 路由之前抢跑 return），
/// 现收编为 pre-step 链上的一个普通中间件：命中「完成/执行」关键词 + [已选任务] 引用块
/// 时返回 RouteAction::ExecuteTasks，由 bot_chat 主流程在路由步骤统一处理（含 bypass 开关语义）；
/// 未命中返回 None，链条继续走到 IntentRouterMiddleware。
pub struct ChatExecuteMiddleware;
impl Middleware for ChatExecuteMiddleware {
    fn name(&self) -> &str {
        "chat_execute"
    }
    fn pre_step(&self, input: &str) -> Option<RouteAction> {
        crate::intent_router::is_chat_execute_trigger(input).map(RouteAction::ExecuteTasks)
    }
    fn pre_execute(&self, _name: &str, _active_skill: bool) -> Option<String> {
        None
    }
}

/// 内置：意图路由中间件（包装 intent_router::route_user_input）
///
/// D1：pre_step 恒返回 Some（PassThrough 也算命中），会短路整条 pre_step 链——
/// 因此它必须是链上最后一个（register_pre_step 对「在其后追加」直接 panic）。
pub struct IntentRouterMiddleware;
/// D1：注册顺序断言按名字识别（与 introspect 列表同源），抽常量防拼写漂移
const INTENT_ROUTER_NAME: &str = "intent_router";
impl Middleware for IntentRouterMiddleware {
    fn name(&self) -> &str {
        INTENT_ROUTER_NAME
    }
    fn pre_step(&self, input: &str) -> Option<RouteAction> {
        // PassThrough 也算 Some，让 run_pre_step 短路
        Some(route_user_input(input))
    }
    fn pre_execute(&self, _name: &str, _active_skill: bool) -> Option<String> {
        None
    }
}

/// 内置：原子黑名单中间件（包装 tool_guard::is_atomic_tool + is_skill_active）
pub struct AtomicGuardMiddleware;
impl Middleware for AtomicGuardMiddleware {
    fn name(&self) -> &str {
        "atomic_guard"
    }
    fn pre_step(&self, _input: &str) -> Option<RouteAction> {
        None
    }
    fn pre_execute(&self, name: &str, active_skill: bool) -> Option<String> {
        if is_atomic_tool(name) && !active_skill {
            Some(atomic_block_message(name))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_empty_returns_none() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let r = MiddlewareRegistry::default();
        assert_eq!(r.run_pre_step(&handle, "hello"), None);
        assert_eq!(r.run_pre_execute(&handle, "foo", false), None);
    }

    #[test]
    fn registry_pre_step_short_circuit_first_wins() {
        struct A;
        impl Middleware for A {
            fn name(&self) -> &str {
                "A"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                Some(RouteAction::Skill("a_skill".into()))
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                None
            }
        }
        struct B;
        impl Middleware for B {
            fn name(&self) -> &str {
                "B"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                Some(RouteAction::Skill("b_skill".into()))
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                None
            }
        }
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(A));
        r.register_pre_step(Box::new(B));
        // A 先注册，短路求值返回 A 的结果
        let app = tauri::test::mock_app();
        match r.run_pre_step(app.handle(), "x") {
            Some(RouteAction::Skill(s)) => assert_eq!(s, "a_skill"),
            other => panic!("expected a_skill, got {other:?}"),
        }
    }

    #[test]
    fn registry_pre_execute_short_circuit() {
        struct Blocker;
        impl Middleware for Blocker {
            fn name(&self) -> &str {
                "Blocker"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                None
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                Some("BLOCKED".into())
            }
        }
        struct PassMw;
        impl Middleware for PassMw {
            fn name(&self) -> &str {
                "Pass"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                None
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                None
            }
        }
        let mut r = MiddlewareRegistry::default();
        r.register_pre_execute(Box::new(PassMw));
        r.register_pre_execute(Box::new(Blocker));
        let app = tauri::test::mock_app();
        assert_eq!(
            r.run_pre_execute(app.handle(), "any_tool", false),
            Some("BLOCKED".into())
        );
    }

    #[test]
    fn registry_introspect_lists_names() {
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(IntentRouterMiddleware));
        r.register_pre_execute(Box::new(AtomicGuardMiddleware));
        assert_eq!(r.pre_step_list(), vec!["intent_router"]);
        assert_eq!(r.pre_execute_list(), vec!["atomic_guard"]);
    }

    #[test]
    fn intent_router_middleware_empty_table_pass_through() {
        // 2026-08-19 动态路由：路由表 = 已安装技能的 intents 声明，测试进程未 rebuild → 空表。
        // 空表恒 PassThrough（未安装的技能不得有路由）；D1 契约不变：PassThrough 也返回 Some 短路 pre_step 链。
        let m = IntentRouterMiddleware;
        match m.pre_step("帮我做 PPT") {
            Some(RouteAction::PassThrough) => {}
            other => panic!("expected PassThrough, got {other:?}"),
        }
        match m.pre_step("hello world") {
            Some(RouteAction::PassThrough) => {}
            other => panic!("expected PassThrough, got {other:?}"),
        }
    }

    #[test]
    fn atomic_guard_blocks_atomic_tool_when_no_skill() {
        let m = AtomicGuardMiddleware;
        // 黑名单 + 非 Skill 状态 → 阻断
        assert!(m.pre_execute("create_word_revisions", false).is_some());
        assert!(m.pre_execute("link_file_to_task", false).is_some());
        // 黑名单 + Skill 状态 → 放行
        assert!(m.pre_execute("create_word_revisions", true).is_none());
        // 白名单 → 放行
        assert!(m.pre_execute("run_python", false).is_none());
        assert!(m.pre_execute("list_tasks", false).is_none());
    }

    #[test]
    fn build_default_registry_has_three_builtins() {
        let r = build_default_registry();
        // pre_step 链：chat_execute（选择任务卡批量执行，2026-08-20 收编）在前、intent_router 殿后
        assert_eq!(r.pre_step_list(), vec!["chat_execute", "intent_router"]);
        assert_eq!(r.pre_execute_list().len(), 1);
    }

    #[test]
    fn chat_execute_middleware_routes_selected_tasks() {
        // 「完成/执行」+ [已选任务] 引用块 → RouteAction::ExecuteTasks
        let m = ChatExecuteMiddleware;
        match m.pre_step("完成\n\n[已选任务]\n- id=a，标题=A\n- id=b，标题=B") {
            Some(RouteAction::ExecuteTasks(ids)) => {
                assert_eq!(ids.len(), 2);
                assert_eq!(ids[0].0, "a");
                assert_eq!(ids[1].0, "b");
            }
            other => panic!("应为 ExecuteTasks，得到 {other:?}"),
        }
        // 无关键词 / 无引用块 → None，链条继续走 intent_router
        assert!(m.pre_step("看看这些\n\n[已选任务]\n- id=a，标题=A").is_none());
        assert!(m.pre_step("帮我做 PPT").is_none());
    }

    // ── NEW-D-6：helper 泛型 Runtime 化后，D2 fail 语义可用 mock runtime 单测 ──

    #[test]
    fn helper_missing_state_pre_step_fail_open() {
        // D2：业务路由类 fail-open——state 未 manage → None（legacy passthrough，不阻塞）。
        // 该测试同时证明 helper 已泛型化：mock_app 的 AppHandle<MockRuntime> 能编译通过。
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        assert!(run_pre_step(&handle, "帮我做 PPT").is_none());
    }

    #[test]
    fn helper_missing_state_pre_execute_fail_closed() {
        // D2：安全闸门类 fail-closed——state 未 manage 时原子工具被拒绝（Some），
        // 不能静默放行；非原子工具无闸门诉求，仍放行（None）。
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let blocked = run_pre_execute(&handle, "create_word_revisions", false);
        let msg = blocked.expect("registry 缺失时原子工具必须被拒绝（fail-closed）");
        assert!(msg.contains("安全闸门未初始化"), "提示语应说明原因：{msg}");
        assert!(
            run_pre_execute(&handle, "list_tasks", false).is_none(),
            "非原子工具不受闸门影响，fail-open"
        );
        // Skill 活动态与 AtomicGuard 口径一致：活动 Skill 的原子调用不拦
        assert!(run_pre_execute(&handle, "create_word_revisions", true).is_none());
    }

    #[test]
    fn helper_with_managed_state_runs_registry() {
        // state 已 manage → helper 走 registry：atomic guard 阻断、intent router 放行
        let app = tauri::test::mock_app();
        app.manage(build_default_registry());
        let handle = app.handle().clone();
        assert!(run_pre_execute(&handle, "create_word_revisions", false).is_some());
        assert!(run_pre_execute(&handle, "list_tasks", false).is_none());
        // 2026-08-19 动态路由：测试进程路由表为空（未安装技能经 rebuild 注入）→ 恒 PassThrough
        match run_pre_step(&handle, "帮我做 PPT") {
            Some(RouteAction::PassThrough) => {}
            other => panic!("expected PassThrough（动态路由空表）, got {other:?}"),
        }
    }

    // ── D1：IntentRouterMiddleware 恒 Some 短路全链，必须最后注册 ──

    #[test]
    #[should_panic(expected = "必须最后注册")]
    fn register_pre_step_after_intent_router_panics() {
        // build_default_registry 已把 intent_router 放在 pre_step 链尾；
        // 任何后续 pre_step 注册都是死代码 → 注册时直接 panic，不容静默死亡
        let mut r = build_default_registry();
        r.register_pre_step(Box::new(AtomicGuardMiddleware));
    }

    #[test]
    fn register_pre_step_before_intent_router_is_allowed() {
        struct Custom;
        impl Middleware for Custom {
            fn name(&self) -> &str {
                "custom"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                None
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                None
            }
        }
        let mut r = MiddlewareRegistry::default();
        // 排在 intent_router 之前合法：custom 返回 None 时继续走到 intent_router
        r.register_pre_step(Box::new(Custom));
        r.register_pre_step(Box::new(IntentRouterMiddleware));
        assert_eq!(r.pre_step_list(), vec!["custom", "intent_router"]);
    }

    // ── P2-14：只注册 pre_step 的 registry，pre_execute 调用记审计不静默 ──

    #[test]
    fn pre_execute_empty_side_audits_not_registered() {
        struct OnlyStep;
        impl Middleware for OnlyStep {
            fn name(&self) -> &str {
                "only_step"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                None
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                unreachable!("未注册到 pre_execute 侧，不应被调用")
            }
        }
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(OnlyStep));
        // pre_execute 侧为空：返回 None（不阻断）+ 记 pre_execute_not_registered 审计
        assert_eq!(r.run_pre_execute(&handle, "list_tasks", false), None);
        let log =
            std::fs::read_to_string(crate::audit::probe_log_dir(&handle).join("bot.log"))
                .unwrap_or_default();
        assert!(
            log.contains("pre_execute_not_registered") && log.contains("tool=list_tasks"),
            "缺 pre_execute_not_registered 审计: {log}"
        );
    }

    // ── P2-13：中间件 panic 不得炸掉调用方线程（catch_unwind + ERROR 审计 + None）──

    #[test]
    fn middleware_panic_caught_audited_and_returns_none() {
        struct Bomber;
        impl Middleware for Bomber {
            fn name(&self) -> &str {
                "bomber"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                panic!("bomber pre_step boom")
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                panic!("bomber pre_execute bang")
            }
        }
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(Bomber));
        r.register_pre_execute(Box::new(Bomber));
        // panic 被 catch_unwind 兜住：返回 None（按未命中/不阻断处理），主流程不挂
        assert_eq!(r.run_pre_step(&handle, "x"), None);
        assert_eq!(r.run_pre_execute(&handle, "list_tasks", false), None);
        // ERROR 审计落盘：事件名 + 中间件名 + panic 信息
        let log = std::fs::read_to_string(
            crate::audit::probe_log_dir(&handle).join("bot.log"),
        )
        .unwrap_or_default();
        assert!(
            log.contains("middleware_panic") && log.contains("middleware=bomber"),
            "缺 middleware_panic 审计: {log}"
        );
        assert!(log.contains("hook=pre_step") && log.contains("hook=pre_execute"));
        assert!(log.contains("bomber pre_step boom"), "panic 信息应入审计: {log}");
    }
}
