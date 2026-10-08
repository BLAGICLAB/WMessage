//! F-2 Plugin/Extension 抽象层
//!
//! ## 动机
//! 「业务 Skill 禁止注册底层中间件钩子」不能只是口头约束，要有编译期隔离。
//! 引入 `Middleware` trait + `MiddlewareRegistry` 让「中间件注册」成为受控行为：
//! - `lib.rs` setup 阶段注册 2 个内置中间件（选择任务卡批量执行路由 + 意图路由）
//! - 业务模块（bot_skills.rs）只能通过 helper 函数查询，不能 register
//!
//! ## 设计
//! - `Middleware` trait 提供 name / pre_step / pre_execute 三个方法
//! - `MiddlewareRegistry` 用 Vec<Box<dyn Middleware>> 存储，短路求值
//! - helper 函数 run_pre_step / run_pre_execute 通过 Tauri State 访问 registry
//!
//! ## fail 语义（D2）
//! state 未 manage（测试、初始化竞态）时两类中间件区别对待：
//! - pre_execute 闸门（run_pre_execute helper / 空链防线）→ 原子名单命中时 **fail-closed**：
//!   拒绝 + ERROR 审计（当前黑名单已空、防线随名单回填生效）
//! - 业务路由类（pre_step / IntentRouter）→ **fail-open**：回退 None（legacy passthrough），
//!   无锁语义不影响业务，上层照常走模型直接对话

use crate::intent_router::{route_user_input, RouteAction};
use crate::tool_guard::is_atomic_tool;
use tauri::Manager; // F-6：泛型 Runtime 以适配 mock_runtime 集成测试

/// pre_execute 的判定结果（自解释语义；替代旧 Option<String>）
/// - Allow：放行，继续走下一个中间件 / 调用栈
/// - Deny：阻断，`reason` 即给用户/审计的可读原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionDecision {
    Allow,
    Deny { reason: String },
}

/// 中间件 trait（F-2 抽象层核心）
/// 任何「可插拔行为」都实现这个 trait，然后通过 `MiddlewareRegistry::register_*` 注册
pub trait Middleware: Send + Sync {
    /// 可读标识，用于 introspect / 日志 / 设置页 UI 展示
    fn name(&self) -> &str;
    /// 用户输入预处理：返回 Some(RouteAction) 触发短路求值，None 继续下一个中间件
    /// 返回 RouteAction::PassThrough 也算「命中」并触发短路
    fn pre_step(&self, input: &str) -> Option<RouteAction>;
    /// 工具调用前检查：返回 ExecutionDecision::Allow 放行，Deny 阻断并附 reason
    fn pre_execute(&self, name: &str, active_skill: bool) -> ExecutionDecision;
}

/// 中间件注册表（F-2 P2）
/// 注册仅在 setup 阶段（lib.rs）发生；业务模块只能通过 helper 函数查询
#[derive(Default)]
pub struct MiddlewareRegistry {
    pre_step: Vec<Box<dyn Middleware>>,
    pre_execute: Vec<Box<dyn Middleware>>,
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
    /// catch_unwind 兜底——中间件 panic 不得炸掉调用方所在的
    /// tauri::async_runtime worker 线程；记 ERROR 审计后按「未命中」处理，继续下一个
    pub fn run_pre_step<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        input: &str,
    ) -> Option<RouteAction> {
        for (pos, m) in self.pre_step.iter().enumerate() {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| m.pre_step(input))) {
                Ok(Some(action)) => {
                    // 路由命中审计：中间件名 + 短路位置 + action 种类/细节。
                    // 用途：统计各中间件命中率，并看出到底在哪一环短路
                    // （pos 越大说明前面几个中间件都返回了 None）。
                    let (kind, detail) = action.audit_kv();
                    crate::audit::write_event(
                        app,
                        crate::audit::AuditLevel::Info,
                        "middleware.route",
                        &[
                            ("hook", "pre_step".to_string()),
                            ("middleware", m.name().to_string()),
                            ("chain_pos", pos.to_string()),
                            ("chain_len", self.pre_step.len().to_string()),
                            ("action", kind.to_string()),
                            ("detail", detail),
                        ],
                    );
                    return Some(action);
                }
                Ok(None) => {}
                Err(payload) => {
                    let msg = crate::audit::panic_message(payload);
                    crate::audit::write_error_audit(
                        app,
                        "middleware_panic",
                        &[
                            ("hook", "pre_step"),
                            ("middleware", m.name()),
                            ("panic", &msg),
                        ],
                    );
                }
            }
        }
        None
    }
    /// pre-execute 短路求值：任一中间件返回 Deny 即短路放回 Deny；Allow 继续下一个
    /// 同 run_pre_step，panic 兜住记审计后按「不阻断」继续下一个
    /// pre_step / pre_execute 双 Vec 分离，漏注册一边会静默半生效——
    /// 空注册表被调用时记 ERROR 审计（pre_execute_not_registered），不无声放行
    pub fn run_pre_execute<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        name: &str,
        active_skill: bool,
    ) -> ExecutionDecision {
        if self.pre_execute.is_empty() {
            // pre_execute 链可为空（D4d 后黑名单拦截职责移到各工具内部）：
            // 空链即全放行，是合法态而非漏注册，不记 ERROR 审计。
            // 唯一防线：原子名单回填时对裸调 fail-closed（is_atomic_tool 现恒
            // false、本分支不可达），与 run_pre_execute helper 的缺失口径一致
            if is_atomic_tool(name) && !active_skill {
                return ExecutionDecision::Deny {
                    reason: format!(
                        "⚠️ 安全闸门未注册（pre_execute 为空），拒绝原子工具 {name} 的直接调用。请通过对应 Skill 执行。"
                    ),
                };
            }
            return ExecutionDecision::Allow;
        }
        for m in &self.pre_execute {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                m.pre_execute(name, active_skill)
            })) {
                Ok(decision) => {
                    if let ExecutionDecision::Deny { .. } = decision {
                        return decision;
                    }
                }
                Err(payload) => {
                    let msg = crate::audit::panic_message(payload);
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
        ExecutionDecision::Allow
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

/// 构建默认注册表：注册 2 个内置中间件（lib.rs setup 调用）
pub fn build_default_registry() -> MiddlewareRegistry {
    let mut r = MiddlewareRegistry::default();
    // 选择任务卡批量执行路由：排在 IntentRouter 之前——
    // 「完成/执行」+ [已选任务] 引用块命中时直接短路为 ExecuteTasks，不再查技能路由表；
    // 未命中返回 None，链条继续走到 IntentRouter
    r.register_pre_step(Box::new(ChatExecuteMiddleware));
    r.register_pre_step(Box::new(IntentRouterMiddleware));
    // D4d 后 pre_execute 链为空（原子黑名单中间件已删，拦截职责在工具内部；
    // 空链即放行，原子名单回填时由空链防线 fail-closed）
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
/// D2：闸门口径 **fail-closed**——state 未 manage 时原子名单命中的工具拒绝并记
/// ERROR 审计（当前黑名单已空，防线随名单回填生效），非原子工具仍 fail-open 回退 Allow。
pub fn run_pre_execute<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    name: &str,
    active_skill: bool,
) -> ExecutionDecision {
    match app.try_state::<MiddlewareRegistry>() {
        Some(state) => state.run_pre_execute(app, name, active_skill),
        None => {
            if is_atomic_tool(name) && !active_skill {
                crate::audit::write_error_audit(
                    app,
                    "middleware_registry_missing",
                    &[("tool", name), ("gate", "atomic_guard")],
                );
                ExecutionDecision::Deny {
                    reason: format!(
                        "⚠️ 安全闸门未初始化（MiddlewareRegistry 未注册），拒绝原子工具 {name} 的直接调用。请通过对应 Skill 执行。"
                    ),
                }
            } else {
                ExecutionDecision::Allow
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// 内置中间件：包装现有 intent_router / tool_guard
// ────────────────────────────────────────────────────────────────────
// （原子黑名单中间件已于 2026-10-08 移除：黑名单 D4d 清空后恒 Allow 的空骨架，
// 拦截职责由各工具内部 is_task_execution_flow 承担；原子名单回填时走空链防线）

/// 内置：选择任务卡批量执行路由中间件
///
/// pre-step 链上的一个普通中间件：命中「完成/执行」关键词 + [已选任务] 引用块
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
    fn pre_execute(&self, _name: &str, _active_skill: bool) -> ExecutionDecision {
        ExecutionDecision::Allow
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
    fn pre_execute(&self, _name: &str, _active_skill: bool) -> ExecutionDecision {
        ExecutionDecision::Allow
    }
}

// 原子黑名单中间件（已移除）：原包装 tool_guard::is_atomic_tool 的集中拦截
// 骨架——黑名单 D4d 清空后恒 Allow，属无行为死代码，2026-10-08 整洁度批删除。
// 拦截职责由各工具内部的 is_task_execution_flow 按会话上下文自行承担；后续若
// 新增需要裸调拦截的工具，把工具名加回 ATOMIC_TOOLS 后由 run_pre_execute
// 空链防线 fail-closed。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_empty_returns_none() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let r = MiddlewareRegistry::default();
        assert_eq!(r.run_pre_step(&handle, "hello"), None);
        assert_eq!(
            r.run_pre_execute(&handle, "foo", false),
            ExecutionDecision::Allow
        );
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
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Allow
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
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Allow
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
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Deny {
                    reason: "BLOCKED".to_string(),
                }
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
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Allow
            }
        }
        let mut r = MiddlewareRegistry::default();
        r.register_pre_execute(Box::new(PassMw));
        r.register_pre_execute(Box::new(Blocker));
        let app = tauri::test::mock_app();
        assert_eq!(
            r.run_pre_execute(app.handle(), "any_tool", false),
            ExecutionDecision::Deny {
                reason: "BLOCKED".to_string(),
            }
        );
    }

    #[test]
    fn registry_introspect_lists_names() {
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(IntentRouterMiddleware));
        r.register_pre_execute(Box::new(ChatExecuteMiddleware));
        assert_eq!(r.pre_step_list(), vec!["intent_router"]);
        assert_eq!(r.pre_execute_list(), vec!["chat_execute"]);
    }

    #[test]
    fn intent_router_middleware_empty_table_pass_through() {
        // 动态路由：路由表 = 已安装技能的 intents 声明，测试进程未 rebuild → 空表。
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
    fn build_default_registry_has_two_builtins() {
        let r = build_default_registry();
        // pre_step 链：chat_execute（选择任务卡批量执行）在前、intent_router 殿后；
        // pre_execute 链为空（D4d 后拦截职责在工具内部，空链即放行）
        assert_eq!(r.pre_step_list(), vec!["chat_execute", "intent_router"]);
        assert!(r.pre_execute_list().is_empty());
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
        assert!(m
            .pre_step("看看这些\n\n[已选任务]\n- id=a，标题=A")
            .is_none());
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
    fn helper_missing_state_pre_execute_fail_open() {
        // D4d 后 ATOMIC_TOOLS 已清空 → registry 缺失时所有工具 fail-open 放行
        //（原子工具黑名单拦截职责已转移到 tool_link_file_to_task 内部的
        // is_task_execution_flow 判定，中间件层不再承担）。
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        assert!(matches!(
            run_pre_execute(&handle, "link_file_to_task", false),
            ExecutionDecision::Allow
        ));
        assert!(matches!(
            run_pre_execute(&handle, "list_tasks", false),
            ExecutionDecision::Allow
        ));
        assert!(matches!(
            run_pre_execute(&handle, "run_python", false),
            ExecutionDecision::Allow
        ));
    }

    #[test]
    fn helper_with_managed_state_runs_registry() {
        // state 已 manage → helper 走 registry：pre_execute 链为空（拦截职责在
        // 工具内部）恒 Allow，intent_router 路由表为空 → 恒 PassThrough
        let app = tauri::test::mock_app();
        app.manage(build_default_registry());
        let handle = app.handle().clone();
        assert!(matches!(
            run_pre_execute(&handle, "link_file_to_task", false),
            ExecutionDecision::Allow
        ));
        assert!(matches!(
            run_pre_execute(&handle, "list_tasks", false),
            ExecutionDecision::Allow
        ));
        // 动态路由：测试进程路由表为空（未安装技能经 rebuild 注入）→ 恒 PassThrough
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
        r.register_pre_step(Box::new(IntentRouterMiddleware));
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
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Allow
            }
        }
        let mut r = MiddlewareRegistry::default();
        // 排在 intent_router 之前合法：custom 返回 None 时继续走到 intent_router
        r.register_pre_step(Box::new(Custom));
        r.register_pre_step(Box::new(IntentRouterMiddleware));
        assert_eq!(r.pre_step_list(), vec!["custom", "intent_router"]);
    }

    // ── 中间件 panic 不得炸掉调用方线程（catch_unwind + ERROR 审计 + None）──

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
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
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
        assert_eq!(
            r.run_pre_execute(&handle, "list_tasks", false),
            ExecutionDecision::Allow
        );
        // ERROR 审计落盘：事件名 + 中间件名 + panic 信息
        let log = std::fs::read_to_string(crate::paths::probe_log_dir(&handle).join("bot.log"))
            .unwrap_or_default();
        assert!(
            log.contains("middleware_panic") && log.contains("middleware=bomber"),
            "缺 middleware_panic 审计: {log}"
        );
        assert!(log.contains("hook=pre_step") && log.contains("hook=pre_execute"));
        assert!(
            log.contains("bomber pre_step boom"),
            "panic 信息应入审计: {log}"
        );
    }

    // ── 路由命中审计：中间件名 + 短路位置 + action（命中率/短路位置可统计）──

    #[test]
    fn route_hit_is_audited_with_chain_position_and_action() {
        struct Miss;
        impl Middleware for Miss {
            fn name(&self) -> &str {
                "miss_first"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                None
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Allow
            }
        }
        let case_tag = uuid::Uuid::new_v4().simple().to_string();
        struct Hit(String);
        impl Middleware for Hit {
            fn name(&self) -> &str {
                "hit_second"
            }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                Some(RouteAction::Skill(format!("probe_skill_{}", self.0)))
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> ExecutionDecision {
                ExecutionDecision::Allow
            }
        }
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(Miss));
        r.register_pre_step(Box::new(Hit(case_tag.clone())));
        assert!(matches!(
            r.run_pre_step(&handle, "x"),
            Some(RouteAction::Skill(_))
        ));
        let log = std::fs::read_to_string(crate::paths::probe_log_dir(&handle).join("bot.log"))
            .unwrap_or_default();
        // 只对本用例生成的唯一技能名断言，避免与并行用例的 bot.log 写入互相干扰
        let line = log
            .lines()
            .find(|l| {
                l.contains("middleware.route") && l.contains(&format!("probe_skill_{case_tag}"))
            })
            .unwrap_or_else(|| panic!("缺 middleware.route 审计行: {log}"));
        assert!(line.contains("middleware=hit_second"), "{line}");
        assert!(
            line.contains("chain_pos=1"),
            "短路位置应为 1（前面 miss_first 放过）: {line}"
        );
        assert!(line.contains("chain_len=2"), "{line}");
        assert!(line.contains("action=skill"), "{line}");
    }
}
