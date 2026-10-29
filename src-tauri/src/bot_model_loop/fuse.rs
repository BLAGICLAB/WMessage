//! 熔断层：单轮 Function 调用上限、收尾窗阶梯判定、轮数熔断与软警告注入。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::ModelLoopDeps;
use crate::bot::constants::{WRAP_UP_ROUNDS, WRAP_UP_TOOLS};
use crate::error::CommandError;

/// Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
// max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
//
// 上限是安全网常量（constants::MAX_FUNCTION_CALLS，主会话与子 agent 共用同一
// 个数）；fuse_cap_override 仅供 eval 用例与测试直驱注入。软警告阈值动态 =
// cap×90%（见 request.rs 的阶梯派生）。
pub(crate) const MAX_FUNCTION_CALLS_PER_REQUEST: usize =
    crate::bot::constants::MAX_FUNCTION_CALLS as usize;

/// 熔断消息标记（runner 端识别熔断结果用同一常量，勿散落字符串字面量——W5 r1）
pub(crate) const FUSE_MARKER: &str = "⏹ 已熔断";

/// 轮数熔断标记。与 `FUSE_MARKER` 分开是因为两类熔断的下游语义不同：
/// Function 熔断正文仍可被当普通回复展示，轮数熔断则要被子任务侧认出来
/// 还原 `budget_exceeded`（见 bot_orchestrator::classify_outcome）。
pub(crate) const ROUNDS_FUSE_MARKER: &str = "⏹ 已熔断（轮数上限）";

/// 熔断上界的 usize 投影：与子 agent 预算解析（params::MAX_FUNCTION_CALLS_CAP）
/// 是同一个数，这里只做类型转换——两处各写一份字面量就会在「预算按 500 记账、
/// 循环按 999 放行」这种偏差上分叉，且分叉到熔断才暴露。
pub(crate) fn fuse_cap_upper_bound() -> usize {
    crate::bot::params::MAX_FUNCTION_CALLS_CAP as usize
}

/// 熔断上限派生链的唯一入口：bot-config 覆盖 > 内置默认，末端钳到 [1, 上界]。
/// 主会话与子 agent 同一个数——子 agent 没有独立预算。
///
/// 收尾阶梯（软告警 / 收尾窗 / 收尾轮数用尽硬停）按这个上限的占比派生
/// （见 `gate_call`）——上限之上不再有任何裕量。
///
/// 上游（薄壳组装 deps）刻意不钳：`deps.fuse_cap_override` 也被单测直驱 core
/// 注入，绕过薄壳，钳在薄壳等于给「实际生效的那条边界」留了个不生效的副本。
pub(crate) fn fuse_cap_for(cfg_override: Option<usize>) -> usize {
    cfg_override
        .unwrap_or(MAX_FUNCTION_CALLS_PER_REQUEST)
        .clamp(1, fuse_cap_upper_bound())
}

/// 熔断判定：第 n 次（1-based 累计）Function 调用是否超上限。
///
/// 正常路径走不到这里——收尾窗阈值恒钳在 cap-1 之下，计数执行 +1 后仍低于
/// cap，真正的出口是收尾轮数用尽（`wrap_up_rounds_exhausted`）。保留它是
/// 派生链变化时的最后防线：wrap_up_at 的派生若被改动（如某轴不再设收尾窗），
/// 这里仍兜得住「无限执行」。业界同构：LangGraph 在 RemainingSteps 优雅
/// 降级层之下仍保留 GraphRecursionError 硬上限。cap 由会话派生（`fuse_cap_for`）。
pub(crate) fn should_fuse(calls_so_far: usize, cap: usize) -> bool {
    calls_so_far > cap
}

/// 单次工具调用在阶梯上的去向（见 `gate_call`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallGate {
    /// 预算内：计数并执行。
    Counted,
    /// 收尾窗内的写产物类调用：执行但不计数——窗口里的额度是给「把结果落盘」用的。
    WrapUpAllowed,
    /// 收尾窗内的探索类调用：拒绝且不计数——计数守的是「实际执行」的调用量。
    WrapUpDenied,
}

/// 阶梯判定：`counted` 是进入本次调用前已计数的执行次数，`rounds_in_wrap_up`
/// 是轮数轴是否已进收尾窗（round ≥ rounds_wrap_up_at，见 request.rs 派生）。
///
/// 只计实际执行：被拒的调用没有消耗任何东西，计进去会让一轮并行批把窗口
/// 吃光，模型永远轮不到写产物。写类放行也不计——否则窗口会被自己的写调用
/// 烧光；工具轴窗口的出口是轮数（`wrap_up_rounds_exhausted`），不是计数，
/// 轮数轴窗口的出口是 max_rounds 自然硬停。
pub(crate) fn gate_call(
    counted: usize,
    wrap_up_at: usize,
    rounds_in_wrap_up: bool,
    tool: &str,
) -> CallGate {
    if counted < wrap_up_at && !rounds_in_wrap_up {
        CallGate::Counted
    } else if WRAP_UP_TOOLS.contains(&tool) {
        CallGate::WrapUpAllowed
    } else {
        CallGate::WrapUpDenied
    }
}

/// 收尾窗轮数是否用尽：从进入那一轮起，之后最多再跑 `WRAP_UP_ROUNDS` 轮。
pub(crate) fn wrap_up_rounds_exhausted(entered_round: Option<usize>, round: usize) -> bool {
    entered_round.is_some_and(|r0| round > r0 + WRAP_UP_ROUNDS)
}

/// 收尾窗轮数用尽的硬停消息：与 `fuse_message` 同源（带上已产正文）。
pub(crate) fn wrap_up_exhausted_message(final_text: &str, hint: &str) -> String {
    format!(
        "{final_text}\n\n{FUSE_MARKER}：工具调用预算已进入收尾窗，收尾 {WRAP_UP_ROUNDS} 轮用尽（安全保护），已停止后续执行{hint}"
    )
}

/// 熔断返回消息（与主循环文案同源，单测直接断言）。
///
/// `pub(crate)`：子 agent 收尾判定要认这条熔断（工具预算触达 → budget_exceeded），
/// 测试也要用同一份文案——两处各拼一遍字符串会漂。
pub(crate) fn fuse_message(final_text: &str, hint: &str, cap: usize) -> String {
    format!(
        "{final_text}\n\n{FUSE_MARKER}：本轮 Function 调用超过 {cap} 次上限（安全保护），已停止后续执行{hint}"
    )
}

/// 轮数熔断返回消息：与 `fuse_message` 同源（都带上熔断前的正文），只是上限口径不同。
/// `partial` 传 `last_streamed`（上一轮 streamed 快照）——循环体退出时本轮的
/// final_text 尚未快照，能拿到的最新正文就是它。
pub(crate) fn rounds_fuse_message(partial: &str, hint: &str, max_rounds: usize) -> String {
    format!(
        "{partial}\n\n{ROUNDS_FUSE_MARKER}：对话轮数超过 {max_rounds} 轮上限（安全保护），已停止后续执行{hint}"
    )
}

/// 轮末软警告注入：轮数软警告。
///
/// 在「本轮 tool 响应全部回填之后」注入：此刻模型才看得见新消息，
/// 提前注入会被当成上一轮的工具结果丢掉。
pub(crate) fn inject_soft_warnings(
    deps: &ModelLoopDeps<'_>,
    msgs: &mut Vec<serde_json::Value>,
    rounds_warn_sent: &mut bool,
    max_rounds: usize,
    rounds_warn_at: usize,
    round: usize,
) {
    let audit = deps.audit;
    // 轮数软警告：同样推迟到本轮 tool 响应全部回填后再注入（上面的 400 教训）。
    // 剩最后一两轮时模型往往已经在连发工具调用——此刻不喊，它就只能撞熔断。
    let rounds_left = max_rounds.saturating_sub(round + 1);
    if !*rounds_warn_sent && rounds_left <= max_rounds.saturating_sub(rounds_warn_at) {
        *rounds_warn_sent = true;
        msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】对话已进行 {} / {max_rounds} 轮，最多还能跑 {rounds_left} 轮。请立刻停止展开新步骤：把已经拿到的结果整理出来收尾，收尾必须说明还剩什么没做、进行到哪一步，不得只交成品不交代缺口——子 agent 按任务包装末尾的 schema 输出 JSON，主对话直接给结论。"
                    , round + 1
                ),
            }));
        audit(
            crate::audit::AuditLevel::Info,
            "rounds_soft_warn",
            vec![
                ("round", (round + 1).to_string()),
                ("max_rounds", max_rounds.to_string()),
                ("left", rounds_left.to_string()),
            ],
        );
    }
}

/// 轮数熔断出口：落审计 + 带正文返回。
///
/// 出口与单轮 Function 熔断同源：返回 Ok 并带上熔断前的正文，不做裸 Err。
/// 早返回出口（已停止 / Skill 收尾 / 单轮熔断）都保正文，只有这里把正文连同
/// 已跑完的那十几轮一起丢掉——子 agent 收不到收尾 JSON 时 result_json 写 NULL，
/// 父 agent 回灌拿不到任何 summary，20 轮产出归零。熔断语义不靠返回类型表达
/// （主聊天与子 agent 的下游处理不同），靠正文里的 ROUNDS_FUSE_MARKER 认。
pub(crate) fn rounds_fuse_exit(
    deps: &ModelLoopDeps<'_>,
    last_streamed: &str,
    max_rounds: usize,
) -> String {
    let hint = (deps.skill_finish)(false, "对话轮数超限");
    let fuse = CommandError::Internal(format!("对话轮数超限{hint}"));
    let mut kv = crate::audit::error_kv(&fuse);
    kv.push(("max_rounds", max_rounds.to_string()));
    // partial_chars：熔断时带回了多少正文。恒为 0 说明模型全程只发工具调用、
    // 没说一个字——那是子任务派发卡本身写歪了（该写 objective 的没写），信号留痕备用。
    kv.push(("partial_chars", last_streamed.chars().count().to_string()));
    (deps.audit)(crate::audit::AuditLevel::Warn, "fuse_rounds", kv);
    rounds_fuse_message(last_streamed, &hint, max_rounds)
}
