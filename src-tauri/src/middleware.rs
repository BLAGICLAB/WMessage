//! F-2 Plugin/Extension 抽象层（2026-08-18 P2）
//!
//! ## 动机
//! 审计发现「业务 Skill 禁止注册底层中间件钩子」只是口头约束，没有编译期隔离。
//! 引入 `Middleware` trait + `MiddlewareRegistry` 让「中间件注册」成为受控行为：
//! - `lib.rs` setup 阶段注册 2 个内置中间件（意图路由 + 原子黑名单）
//! - 业务模块（bot_skills.rs）只能通过 helper 函数查询，不能 register
//!
//! ## 设计
//! - `Middleware` trait 提供 name / pre_step / pre_execute 三个方法
//! - `MiddlewareRegistry` 用 Vec<Box<dyn Middleware>> 存储，短路求值
//! - helper 函数 run_pre_step / run_pre_execute 通过 Tauri State 访问 registry
//! - 没有 state 时回退 None（legacy passthrough，不阻塞）

use crate::intent_router::{RouteAction, route_user_input};
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
    pub fn register_pre_step(&mut self, m: Box<dyn Middleware>) {
        self.pre_step.push(m);
    }
    /// 注册 pre-execute 中间件
    pub fn register_pre_execute(&mut self, m: Box<dyn Middleware>) {
        self.pre_execute.push(m);
    }
    /// pre-step 短路求值：任一中间件返回 Some(_)
    pub fn run_pre_step(&self, input: &str) -> Option<RouteAction> {
        for m in &self.pre_step {
            if let Some(action) = m.pre_step(input) {
                return Some(action);
            }
        }
        None
    }
    /// pre-execute 短路求值：任一中间件返回 Some(msg)
    pub fn run_pre_execute(&self, name: &str, active_skill: bool) -> Option<String> {
        for m in &self.pre_execute {
            if let Some(msg) = m.pre_execute(name, active_skill) {
                return Some(msg);
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
        self.pre_execute.iter().map(|m| m.name().to_string()).collect()
    }
}

/// 构建默认注册表：注册 2 个内置中间件（lib.rs setup 调用）
pub fn build_default_registry() -> MiddlewareRegistry {
    let mut r = MiddlewareRegistry::default();
    r.register_pre_step(Box::new(IntentRouterMiddleware));
    r.register_pre_execute(Box::new(AtomicGuardMiddleware));
    r
}

/// helper：通过 Tauri State 调 run_pre_step（state 未 manage 时回退 None = legacy passthrough）
pub fn run_pre_step(app: &tauri::AppHandle, input: &str) -> Option<RouteAction> {
    match app.try_state::<MiddlewareRegistry>() {
        Some(state) => state.run_pre_step(input),
        None => None,
    }
}

/// helper：通过 Tauri State 调 run_pre_execute
pub fn run_pre_execute(
    app: &tauri::AppHandle,
    name: &str,
    active_skill: bool,
) -> Option<String> {
    match app.try_state::<MiddlewareRegistry>() {
        Some(state) => state.run_pre_execute(name, active_skill),
        None => None,
    }
}

// ────────────────────────────────────────────────────────────────────
// 内置中间件：包装现有 intent_router / tool_guard
// ────────────────────────────────────────────────────────────────────

/// 内置：意图路由中间件（包装 intent_router::route_user_input）
pub struct IntentRouterMiddleware;
impl Middleware for IntentRouterMiddleware {
    fn name(&self) -> &str { "intent_router" }
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
    fn name(&self) -> &str { "atomic_guard" }
    fn pre_step(&self, _input: &str) -> Option<RouteAction> { None }
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
        let r = MiddlewareRegistry::default();
        assert_eq!(r.run_pre_step("hello"), None);
        assert_eq!(r.run_pre_execute("foo", false), None);
    }

    #[test]
    fn registry_pre_step_short_circuit_first_wins() {
        struct A;
        impl Middleware for A {
            fn name(&self) -> &str { "A" }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                Some(RouteAction::Skill("a_skill".into()))
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> { None }
        }
        struct B;
        impl Middleware for B {
            fn name(&self) -> &str { "B" }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> {
                Some(RouteAction::Skill("b_skill".into()))
            }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> { None }
        }
        let mut r = MiddlewareRegistry::default();
        r.register_pre_step(Box::new(A));
        r.register_pre_step(Box::new(B));
        // A 先注册，短路求值返回 A 的结果
        match r.run_pre_step("x") {
            Some(RouteAction::Skill(s)) => assert_eq!(s, "a_skill"),
            other => panic!("expected a_skill, got {other:?}"),
        }
    }

    #[test]
    fn registry_pre_execute_short_circuit() {
        struct Blocker;
        impl Middleware for Blocker {
            fn name(&self) -> &str { "Blocker" }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> { None }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> {
                Some("BLOCKED".into())
            }
        }
        struct PassMw;
        impl Middleware for PassMw {
            fn name(&self) -> &str { "Pass" }
            fn pre_step(&self, _input: &str) -> Option<RouteAction> { None }
            fn pre_execute(&self, _n: &str, _a: bool) -> Option<String> { None }
        }
        let mut r = MiddlewareRegistry::default();
        r.register_pre_execute(Box::new(PassMw));
        r.register_pre_execute(Box::new(Blocker));
        assert_eq!(r.run_pre_execute("any_tool", false), Some("BLOCKED".into()));
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
    fn intent_router_middleware_matches_keywords() {
        let m = IntentRouterMiddleware;
        match m.pre_step("帮我做 PPT") {
            Some(RouteAction::Skill(s)) => assert_eq!(s, "ppt-orchestra-skill"),
            other => panic!("expected ppt, got {other:?}"),
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
    fn build_default_registry_has_two_builtins() {
        let r = build_default_registry();
        assert_eq!(r.pre_step_list().len(), 1);
        assert_eq!(r.pre_execute_list().len(), 1);
    }
}