//! 熔断层：单轮 Function 调用上限、轮数/墙钟熔断与软警告注入。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

use super::ModelLoopDeps;
use crate::error::CommandError;

// Harness 第 5 层：单轮对话 Function 总调用上限（每轮可并行多个 tool_calls，
// max_rounds 管轮数管不住并行调用数，必须有独立计数熔断）
//
// 上限默认 100：工作流节点等长链任务
// 50 次不够用；更高上限的失控风险由幻觉守卫（claims_mutation）+ 软警告 +
// /stop 兜底。可用 bot-config.json `maxFunctionCalls` 覆盖（None = 100）；
// 子 agent 默认预算同源此值（resolve_subagent_budget），LLM 显式给的预算仍可覆盖。
// 软警告阈值动态 = cap*7/10（保持 ~30% buffer），钳 [1, cap-1]。
pub(crate) const MAX_FUNCTION_CALLS_PER_REQUEST: usize = 100;

/// 熔断消息标记（runner 端识别熔断结果用同一常量，勿散落字符串字面量——W5 r1）
pub(crate) const FUSE_MARKER: &str = "⏹ 已熔断";

/// 轮数熔断标记。与 `FUSE_MARKER` 分开是因为两类熔断的下游语义不同：
/// Function 熔断正文仍可被当普通回复展示，轮数熔断则要被子任务侧认出来
/// 还原 `budget_exceeded`（见 bot_orchestrator::classify_outcome）。
pub(crate) const ROUNDS_FUSE_MARKER: &str = "⏹ 已熔断（轮数上限）";

/// 墙钟熔断标记。子 agent 侧据此还原 `budget_exceeded(max_wall_seconds)`
/// 并**保留熔断前的正文**（与轮数熔断同款：同一个「预算触顶」，一个留产
/// 一个不留，没有理由）。
pub(crate) const WALL_FUSE_MARKER: &str = "⏹ 已熔断（墙钟上限）";

/// 熔断上界的 usize 投影：与子 agent 预算解析（params::MAX_FUNCTION_CALLS_CAP）
/// 是同一个数，这里只做类型转换——两处各写一份字面量就会在「预算按 500 记账、
/// 循环按 999 放行」这种偏差上分叉，且分叉到熔断才暴露。
pub(crate) fn fuse_cap_upper_bound() -> usize {
    crate::bot::params::MAX_FUNCTION_CALLS_CAP as usize
}

/// 子 agent 收尾裕量：工具预算触顶后，熔断上限在预算之上再放行的**实际执行**
/// 调用数（写产物 + 读契约）。dispatch 收尾窗拒掉的调用不计入熔断计数
/// （见循环体 Denied 回退），所以这份裕量不会被被拒的读类调用烧掉——
/// 否则一轮并行批就能把窗口吃光，模型永远轮不到写产物。
pub(crate) const SUBAGENT_WRAP_UP_FUSE_MARGIN: usize = 5;

/// 熔断上限派生链的唯一入口：子 agent 行上预算 > bot-config 覆盖 > 内置默认，
/// 末端钳到 [1, 上界]。
///
/// 子 agent 在预算之上加收尾裕量：dispatch 预算闸触顶后进入收尾窗（只拒读、
/// 放行写），这里的熔断退化为「收尾窗都没能让它停下」的绝对兜底。
/// 主会话无预算轴，不受裕量影响。
///
/// 上游（薄壳组装 deps）刻意不钳：`deps.fuse_cap_override` 也被单测直驱 core
/// 注入，绕过薄壳，钳在薄壳等于给「实际生效的那条边界」留了个不生效的副本。
pub(crate) fn fuse_cap_for(subagent_budget: Option<u32>, cfg_override: Option<usize>) -> usize {
    subagent_budget
        .map(|n| n as usize + SUBAGENT_WRAP_UP_FUSE_MARGIN)
        .or(cfg_override)
        .unwrap_or(MAX_FUNCTION_CALLS_PER_REQUEST)
        .clamp(1, fuse_cap_upper_bound())
}

/// 熔断判定：第 n 次（1-based 累计）Function 调用是否超上限。
/// cap 由会话派生：子 agent = 预算 max_tool_calls，其余 = 默认 100
/// bot-config `maxFunctionCalls` 可覆盖，钳 1..=500）。
pub(crate) fn should_fuse(calls_so_far: usize, cap: usize) -> bool {
    calls_so_far > cap
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

/// 墙钟熔断返回消息：与 `rounds_fuse_message` 同源（都带上熔断前的正文），
/// 只是口径不同——轮数熔断是「轮」，墙钟熔断是「秒」。
///
/// 带正文是关键：墙钟触达时子 agent 可能已经跑了几十分钟、产出齐了，
/// 只留一句「墙钟预算触达」等于把这段时间的产出全丢掉，父 agent 只能
/// 从零重烧。
pub(crate) fn wall_fuse_message(partial: &str, hint: &str, max_wall_seconds: u64) -> String {
    format!(
        "{partial}\n\n{WALL_FUSE_MARKER}：墙钟预算超过 {max_wall_seconds} 秒上限（安全保护），已停止后续执行{hint}"
    )
}

/// 墙钟熔断检查（轮边界）：期限已到就在这里退出，**带上上一轮的正文**。
///
/// 为什么不交给外层的 tokio::timeout：timeout 掐掉整条 future 时，
/// 正在跑的工具、已经说出口的正文、已落盘的产物全悬空——子 agent 跑
/// 了几十分钟的产出归零，父 agent 只能从零重烧。轮边界退出与轮数熔断
/// 同款：熔断是一种「设计好的落地」，不是失败。
///
/// 返回 Some(正文) = 墙钟熔断，调用方按熔断出口返回；None = 预算还在。
pub(crate) fn check_fuses(
    deps: &ModelLoopDeps<'_>,
    last_streamed: &str,
    round: usize,
) -> Option<String> {
    let wb = deps.wall_budget?;
    if !wb.expired() {
        return None;
    }
    let hint = (deps.skill_finish)(false, "墙钟预算超限");
    let kv: Vec<(&str, String)> = vec![
        ("max_wall_seconds", wb.max_wall_seconds.to_string()),
        ("round", round.to_string()),
        ("partial_chars", last_streamed.chars().count().to_string()),
    ];
    (deps.audit)(crate::audit::AuditLevel::Warn, "fuse_wall", kv);
    Some(wall_fuse_message(last_streamed, &hint, wb.max_wall_seconds))
}

/// 轮末软警告注入：轮数软警告 + 墙钟软警告。
///
/// 两条都在「本轮 tool 响应全部回填之后」注入：此刻模型才看得见新消息，
/// 提前注入会被当成上一轮的工具结果丢掉。
pub(crate) fn inject_soft_warnings(
    deps: &ModelLoopDeps<'_>,
    msgs: &mut Vec<serde_json::Value>,
    rounds_warn_sent: &mut bool,
    wall_warn_sent: &mut bool,
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
    // 软墙钟：剩余进入收尾预留窗口 → 提醒一次，让模型停止展开新步骤、
    // 把已经拿到的结果整理成收尾 JSON（子 agent 按任务包装末尾的 schema）。
    //
    // 与轮数软警告同一位置注入（tool 响应全部回填之后）：此刻模型才看得见
    // 新消息，提前注入会被当成上一轮的工具结果丢掉。
    //
    // 只提醒一次：反复刷同一条提醒只会挤占上下文，不会让模型更急着收尾。
    if let Some(wb) = deps.wall_budget {
        if !*wall_warn_sent && wb.in_reserve() {
            *wall_warn_sent = true;
            let remaining = wb.remaining().as_secs();
            msgs.push(serde_json::json!({
                "role": "user",
                "content": format!(
                    "【系统提示】时间预算即将耗尽（剩余约 {remaining} 秒，收尾预留窗口 {} 秒）：立刻停止展开新步骤——把已经拿到的结果整理出来收尾，收尾必须说明还剩什么没做、进行到哪一步，不得只交成品不交代缺口。子 agent 按任务包装末尾的 schema 输出 JSON，主对话直接给结论。",
                    wb.reserve.as_secs()
                ),
            }));
            audit(
                crate::audit::AuditLevel::Info,
                "wall_soft_warn",
                vec![
                    ("remaining_secs", remaining.to_string()),
                    ("reserve_secs", wb.reserve.as_secs().to_string()),
                ],
            );
        }
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
