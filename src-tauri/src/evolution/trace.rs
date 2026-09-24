//! 一次 bot 执行的最小可观测单元。
//!
//! Phase 1 只写 audit（`trace.completed` 事件），不落表。
//! `trace_id` 用确定性 hash 便于后续去重 / 关联。
//!
//! 隐私纪律：此处不存原始对话全文、工具参数原文、文件路径原文——
//! 只存结构化摘要（计数 + 分类标签），符合「不动隐私 + 控制体积」。

use serde::{Deserialize, Serialize};

use crate::mutation::MutationOrigin;

/// 一次 bot 执行的最小可观测单元。
///
/// 时间字段用 `i64` epoch ms 而非 `DateTime<Utc>`：
/// 严格遵循 spec 硬约束 #6（不改 Cargo.toml）——chrono 的 `serde` feature 未启用，
/// 直接 `DateTime<Utc>` 派生不了 `Deserialize`。改用 `i64` ms 与代码库其它
/// `_ms` 字段一致（memory::store、bot_scheduler 等），且无需 cargo 改配置。
/// HANDOFF.md 标注：序列化形态由 `DateTime<Utc>` (RFC3339) 退化为 epoch ms。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    /// 确定性 hash：`<session_id>|<started_at_ms>|<ended_at_ms>` 短 hash。
    /// 同一执行多次上报时 id 相同，audit 端可去重。
    pub trace_id: String,
    pub session_id: String,
    /// 复用现有枚举（`mutation.rs`）；serde 序列化与前端协议一致。
    pub origin: MutationOrigin,
    /// epoch 毫秒。spec 原写 `DateTime<Utc>`，因 chrono feature 不可改而收紧为 i64。
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    /// 本次执行的对话轮数（含工具调用轮）。
    pub turn_count: u32,
    /// 每次工具调用的 name + success + duration_ms。
    /// 故意只存 name（工具名），不存参数 / 返回值。
    pub tool_calls: Vec<ToolCallSummary>,
    /// 执行结局（Success / Failure / Aborted）。
    pub outcome: TraceOutcome,
    /// 命中的技能名（若有）。
    pub skill_used: Option<String>,
    /// 注入了多少条记忆（计数，不是内容）。
    pub memory_injected_count: u32,
    /// 涉及的任务卡 id（只存 id，不存标题等可识别内容）。
    pub task_refs: Vec<String>,
}

/// 单次工具调用的最小摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub name: String,
    pub success: bool,
    pub duration_ms: u64,
    /// 失败时的错误**类别**（如 `"timeout"` / `"permission_denied"` / `"user_rejected"`），
    /// 不存原始错误消息——原始消息含路径 / 参数，不应入 audit。
    pub error_kind: Option<String>,
}

/// 执行结局。`Failure` 携带分类后的原因（如 `"llm_5xx"` / `"tool_timeout"`），
/// 不存原始堆栈 / 异常消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceOutcome {
    Success,
    Failure {
        reason: String,
    },
    /// 用户主动 /stop。
    Aborted,
}

/// `trace_id` 短 hash（8 字节 hex，16 字符）。
/// 输入：`<session_id>|<started_at_ms>|<ended_at_ms>`，
/// 保证同一执行重放时 id 相同。
pub fn compute_trace_id(session_id: &str, started_at_ms: i64, ended_at_ms: i64) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    started_at_ms.hash(&mut h);
    ended_at_ms.hash(&mut h);
    let bytes = h.finish().to_be_bytes();
    // 取 8 字节 -> 16 hex 字符（短 hash，便于 audit grep）
    let mut out = String::with_capacity(16);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ───────────────────────── 采样 + 记录 ─────────────────────────

/// 采样阈值：单次执行时长 > 60s 才记（spec 1.5）。
pub const DURATION_THRESHOLD_MS: i64 = 60_000;

/// 采样阈值：单次执行 tool_calls > 10 才记（spec 1.5）。
pub const TOOL_CALLS_THRESHOLD: u32 = 10;

/// Trace 上报入参。builder 模式让 Phase 1 只填实测字段，
/// 未追踪字段（turn_count / skill_used / memory_injected_count）
/// 默认为 0/None 并在 HANDOFF.md 标注（Phase 2 再补）。
///
/// **aborted 唯一事实源 = `outcome` 枚举**：`TraceOutcome::Aborted`
/// 即用户 /stop，无独立 bool 旗标（双轨可矛盾，已坍塌）。
pub struct TraceContext<'a> {
    pub session_id: &'a str,
    pub origin: MutationOrigin,
    pub started_at_ms: i64,
    pub outcome: TraceOutcome,
    pub task_refs: Vec<String>,
    /// 工具调用明细（name + success + duration_ms + error_kind 分类）。
    /// Phase 1 生产 caller 尚无明细来源（run_model_loop 不返回），
    /// 管道先通；接线待 bot_model_loop 返回类型扩展（follow-up）。
    pub tool_calls: Vec<ToolCallSummary>,
    // Phase 1 占位字段：未追踪，统一 0/None
    // （采样用的工具调用计数由 tool_calls.len() 派生，无独立占位字段——
    //  两个相关字段无同步是双轨隐患）
    pub turn_count: u32,
    pub skill_used: Option<&'a str>,
    pub memory_injected_count: u32,
}

impl<'a> TraceContext<'a> {
    pub fn new(session_id: &'a str, origin: MutationOrigin, started_at_ms: i64) -> Self {
        Self {
            session_id,
            origin,
            started_at_ms,
            outcome: TraceOutcome::Success,
            task_refs: Vec::new(),
            tool_calls: Vec::new(),
            turn_count: 0,
            skill_used: None,
            memory_injected_count: 0,
        }
    }
    pub fn with_outcome(mut self, o: TraceOutcome) -> Self {
        self.outcome = o;
        self
    }
    pub fn with_task_refs(mut self, refs: Vec<String>) -> Self {
        self.task_refs = refs;
        self
    }
    pub fn with_tool_calls(mut self, calls: Vec<ToolCallSummary>) -> Self {
        self.tool_calls = calls;
        self
    }
}

/// 采样决策（纯函数，可单测）：
/// 只在以下条件之一成立时记录 trace：
/// 1. `outcome` 是 `Failure` 或 `Aborted`（用户 /stop 恒记录，不静默丢）
/// 2. 工具调用数 > 10（`tool_calls.len()`）
/// 3. `duration_ms > 60_000`（即 60s）
pub fn should_record_trace(
    outcome: &TraceOutcome,
    tool_calls_count: u32,
    duration_ms: i64,
) -> bool {
    if matches!(
        outcome,
        TraceOutcome::Failure { .. } | TraceOutcome::Aborted
    ) {
        return true;
    }
    if tool_calls_count > TOOL_CALLS_THRESHOLD {
        return true;
    }
    if duration_ms > DURATION_THRESHOLD_MS {
        return true;
    }
    false
}

/// 上报 trace（如采样命中）。同步，< 1ms（仅 KV 拼装 + 一次 audit_event!）。
/// 不记录对话原文 / 工具参数原文 / 文件路径原文——只结构化字段。
///
/// AppHandle 未注册：eprintln 后跳过 audit 写入（不 panic；trace 本身无 dedup，
/// 不造成状态污染）。
pub fn maybe_record_trace(ctx: TraceContext<'_>) {
    let ended_at_ms = chrono::Utc::now().timestamp_millis();
    let duration_ms = ended_at_ms - ctx.started_at_ms;
    if !should_record_trace(&ctx.outcome, ctx.tool_calls.len() as u32, duration_ms) {
        return;
    }
    let trace_id = compute_trace_id(ctx.session_id, ctx.started_at_ms, ended_at_ms);
    let trace = ExecutionTrace {
        trace_id,
        session_id: ctx.session_id.to_string(),
        origin: ctx.origin,
        started_at_ms: ctx.started_at_ms,
        ended_at_ms,
        turn_count: ctx.turn_count,
        tool_calls: ctx.tool_calls,
        outcome: ctx.outcome,
        skill_used: ctx.skill_used.map(String::from),
        memory_injected_count: ctx.memory_injected_count,
        task_refs: ctx.task_refs,
    };
    let Some(app) = super::emit::app_handle() else {
        eprintln!(
            "[evolution] trace AppHandle 未注册，跳过 trace.completed；trace_id={}",
            trace.trace_id
        );
        return;
    };
    crate::audit_event!(
        app,
        crate::audit::AuditLevel::Info,
        "trace.completed",
        "trace_id" => trace.trace_id.clone(),
        "session_id" => trace.session_id.clone(),
        "origin" => trace.origin.as_str(),
        // serde 规范形（snake_case tag）——与锁定测试契约一致，不用 Debug 文本
        "outcome" => serde_json::to_string(&trace.outcome)
            .unwrap_or_else(|_| String::from("unknown")),
        "duration_ms" => duration_ms,
        "turn_count" => trace.turn_count,
        "tool_calls" => trace.tool_calls.len() as u64,
        "skill_used" => trace.skill_used.clone().unwrap_or_default(),
        "memory_injected_count" => trace.memory_injected_count,
        "task_ref_count" => trace.task_refs.len() as u64,
        "aborted" => matches!(trace.outcome, TraceOutcome::Aborted),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_id_is_deterministic() {
        let a = compute_trace_id("sess-1", 1_000_000, 1_000_500);
        let b = compute_trace_id("sess-1", 1_000_000, 1_000_500);
        assert_eq!(a, b);
        assert_eq!(a.len(), 16, "trace_id 必须是 16 hex 字符");
    }

    #[test]
    fn trace_id_differs_across_sessions() {
        let a = compute_trace_id("sess-1", 1_000_000, 1_000_500);
        let b = compute_trace_id("sess-2", 1_000_000, 1_000_500);
        assert_ne!(a, b);
    }

    #[test]
    fn trace_id_differs_across_time_windows() {
        let a = compute_trace_id("sess-1", 1_000_000, 1_000_500);
        let b = compute_trace_id("sess-1", 1_000_000, 1_000_600);
        assert_ne!(a, b);
    }

    #[test]
    fn trace_outcome_serializes_with_snake_case_tag() {
        // 锁死 serde 形态：后续若改 tag 名 / rename_all 会破坏 audit 解析
        let ok = serde_json::to_value(&TraceOutcome::Success).unwrap();
        assert_eq!(ok, serde_json::json!({"kind": "success"}));

        let fail = serde_json::to_value(&TraceOutcome::Failure {
            reason: "llm_5xx".into(),
        })
        .unwrap();
        assert_eq!(
            fail,
            serde_json::json!({"kind": "failure", "reason": "llm_5xx"})
        );

        let abort = serde_json::to_value(&TraceOutcome::Aborted).unwrap();
        assert_eq!(abort, serde_json::json!({"kind": "aborted"}));
    }

    #[test]
    fn execution_trace_carries_minimal_fields() {
        // 锁死字段集——后续加字段会破坏 audit 解析，强制评审
        let t = ExecutionTrace {
            trace_id: "abc".into(),
            session_id: "s".into(),
            origin: MutationOrigin::Bot,
            started_at_ms: 1_700_000_000_000,
            ended_at_ms: 1_700_000_001_000,
            turn_count: 3,
            tool_calls: vec![ToolCallSummary {
                name: "list_tasks".into(),
                success: true,
                duration_ms: 12,
                error_kind: None,
            }],
            outcome: TraceOutcome::Success,
            skill_used: None,
            memory_injected_count: 5,
            task_refs: vec!["t1".into()],
        };
        let v = serde_json::to_value(&t).unwrap();
        // 字段名锁死（默认 snake 因为没加 rename_all）——后续若需 camelCase
        // 须显式指定且同步更新 audit 解析
        for key in [
            "trace_id",
            "session_id",
            "origin",
            "started_at_ms",
            "ended_at_ms",
            "turn_count",
            "tool_calls",
            "outcome",
            "skill_used",
            "memory_injected_count",
            "task_refs",
        ] {
            assert!(v.get(key).is_some(), "ExecutionTrace 缺字段 {key}");
        }
    }

    // ─── sampling 规则（spec 1.6 之 8）───

    #[test]
    fn sampling_rule_outcome_failure_fires() {
        assert!(
            should_record_trace(&TraceOutcome::Failure { reason: "x".into() }, 0, 100),
            "Failure outcome 无论 tool_calls / duration 都应记录"
        );
        // 即使 duration 短、tool_calls 少
        assert!(should_record_trace(
            &TraceOutcome::Failure { reason: "x".into() },
            0,
            0
        ));
        // 负例：Success 单独不触发
        assert!(!should_record_trace(&TraceOutcome::Success, 0, 100));
    }

    #[test]
    fn sampling_rule_tool_calls_over_ten_fires() {
        // 边界：> 10 才触发（11 起）
        assert!(should_record_trace(&TraceOutcome::Success, 11, 100));
        assert!(should_record_trace(&TraceOutcome::Success, 100, 100));
        // 边界以下：10 不触发
        assert!(!should_record_trace(&TraceOutcome::Success, 10, 100));
        assert!(!should_record_trace(&TraceOutcome::Success, 0, 100));
    }

    #[test]
    fn sampling_rule_duration_over_60s_fires() {
        // 边界：> 60_000ms 才触发（60_001 起）
        assert!(should_record_trace(&TraceOutcome::Success, 0, 60_001));
        assert!(should_record_trace(&TraceOutcome::Success, 0, 300_000));
        // 边界值：60_000 不触发
        assert!(!should_record_trace(&TraceOutcome::Success, 0, 60_000));
        assert!(!should_record_trace(&TraceOutcome::Success, 0, 100));
        // 零边界
        assert!(!should_record_trace(&TraceOutcome::Success, 0, 0));
    }

    #[test]
    fn sampling_rule_aborted_fires() {
        // 双轨坍塌：Aborted 由 outcome 枚举唯一承载，恒记录（用户 /stop
        // 不再因漏翻 bool 旗标而静默丢）
        assert!(should_record_trace(&TraceOutcome::Aborted, 0, 100));
        // 负例
        assert!(!should_record_trace(&TraceOutcome::Success, 0, 100));
    }

    #[test]
    fn sampling_rule_priority_is_or_not_priority() {
        // spec 未规定优先级；任一规则命中即记。
        // 验证：两条规则同时命中不会变成「记录两次」——只是 should_record 返回 true。
        let r = should_record_trace(&TraceOutcome::Failure { reason: "x".into() }, 100, 100_000);
        assert!(r);
    }

    // ─── maybe_record_trace 集成 ───

    #[test]
    fn maybe_record_drops_silent_when_no_rule_fires() {
        // Success outcome / 0 tool_calls / 100ms → 不采样（aborted 已并入 outcome）
        // AppHandle 未注册 → 不 panic
        let ctx = TraceContext::new(
            "silent",
            MutationOrigin::Main,
            chrono::Utc::now().timestamp_millis() - 100,
        );
        maybe_record_trace(ctx); // 不应 panic，不应崩
    }

    #[test]
    fn maybe_record_drops_silent_when_failure_but_no_app() {
        // 采样命中但 AppHandle 未注册 → eprintln 路径，不 panic
        let ctx = TraceContext::new(
            "silent2",
            MutationOrigin::Main,
            chrono::Utc::now().timestamp_millis(),
        )
        .with_outcome(TraceOutcome::Failure {
            reason: "test".into(),
        });
        maybe_record_trace(ctx); // 不应 panic
    }

    #[test]
    fn maybe_record_long_duration_triggers() {
        // duration > 60s 应被记录（即使 outcome Success）
        let started = chrono::Utc::now().timestamp_millis() - (DURATION_THRESHOLD_MS + 1);
        let ctx = TraceContext::new("long", MutationOrigin::Main, started);
        maybe_record_trace(ctx); // 不应 panic；AppHandle 未注册时仅 eprintln
    }

    #[test]
    fn outcome_serde_json_form_locked_for_emit() {
        // emit 的 outcome 键用 serde_json（非 Debug 文本）——锁 emit 输入的
        // snake_case tag 形态（无 AppHandle seam，端到端落盘不在这里验）
        let s = serde_json::to_string(&TraceOutcome::Failure {
            reason: "llm_5xx".into(),
        })
        .unwrap();
        assert_eq!(s, r#"{"kind":"failure","reason":"llm_5xx"}"#);
        assert_eq!(
            serde_json::to_string(&TraceOutcome::Aborted).unwrap(),
            r#"{"kind":"aborted"}"#
        );
    }

    #[test]
    fn trace_context_tool_calls_forwarded() {
        // tool_calls 管道：builder 接收 → ExecutionTrace 不再恒空
        let calls = vec![ToolCallSummary {
            name: "run_python".into(),
            success: true,
            duration_ms: 12,
            error_kind: None,
        }];
        let ctx = TraceContext::new("s", MutationOrigin::Main, 0).with_tool_calls(calls);
        assert_eq!(ctx.tool_calls.len(), 1);
        assert_eq!(ctx.tool_calls[0].name, "run_python");
    }
}
