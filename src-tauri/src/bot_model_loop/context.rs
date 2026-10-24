//! 上下文瘦身层：超限判定、历史裁剪与重排索引修正。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

/// 可重试的 HTTP 状态：429 限流 + 5xx 服务端瞬时错误；401/400 等重试无意义
pub(crate) fn is_retryable_llm_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// 上下文超长判定：400/413 + 载荷特征。各家文案不一（OpenAI
/// `context_length_exceeded`、Anthropic `prompt is too long`、中转网关
/// `maximum context length`、国产网关中文「上下文超长」），故 code 与关键词
/// 两类形态都认；只看状态码会把 400 的其它形态（鉴权、schema 错）误伤。
pub(crate) fn is_context_overflow(status: u16, body: &str) -> bool {
    if status != 400 && status != 413 {
        return false;
    }
    let b = body.to_lowercase();
    const MARKS: &[&str] = &[
        "context_length_exceeded",
        "maximum context length",
        "context window",
        "prompt is too long",
        "input is too long",
        "too many tokens",
        "reduce the length",
        "上下文超长",
        "超出上下文",
        "上下文长度",
    ];
    MARKS.iter().any(|m| b.contains(m))
}

/// 单条消息的字符数（序列化后计——比对的是请求体实际大小）
pub(crate) fn msg_chars(m: &serde_json::Value) -> usize {
    serde_json::to_string(m)
        .map(|s| s.chars().count())
        .unwrap_or(0)
}

/// 降级动作：砍掉最旧的会话消息，直到总量降到目标比例（纯函数，单测锚点）。
///
/// 只动 system 段之后、最后一条之前的消息：system 段是提示词/摘要/记忆块本体，
/// 最后一条是本轮用户输入；本轮已执行的 tool 消息追加在本轮请求构造之后，
/// 不落在可砍区间——砍它会破坏 tool_call/tool_result 配对，对端直接 400。
///
/// 返回 `(切口下标, 砍掉条数)`：调用方必须拿它修正按 msgs 下标记账的旁路表
/// （`tool_msg_ages`），否则砍完 msgs 变短、旧下标越界。0 条 = 没得砍（调用方走报错出口）。
pub(crate) fn shed_oldest_history(
    msgs: &mut Vec<serde_json::Value>,
    keep_ratio: f64,
) -> (usize, usize) {
    fn role_of(m: &serde_json::Value) -> &str {
        m.get("role").and_then(|r| r.as_str()).unwrap_or_default()
    }
    let first = msgs
        .iter()
        .position(|m| role_of(m) != "system")
        .unwrap_or(msgs.len());
    let mut used: usize = msgs.iter().map(msg_chars).sum();
    let target = (used as f64 * keep_ratio) as usize;
    let mut removable = msgs.len().saturating_sub(first).saturating_sub(1); // 最后一条本轮输入不可砍
    let mut removed = 0usize;
    while used > target && removable > 0 {
        used = used.saturating_sub(msg_chars(&msgs[first + removed]));
        removed += 1;
        removable -= 1;
    }
    if removed == 0 {
        return (first, 0);
    }
    // 切口不能停在 tool 分组中间：被砍走的 assistant(tool_calls) 会让紧邻的
    // tool 消息变成孤儿（对端 400：tool 必须紧跟带 tool_calls 的 assistant）。
    // 边界消息是 tool 就连带砍掉——它的父 assistant 已落在可砍区间里。
    // 最后一条是本轮输入不动，故上界停在 len-1（此时其父 assistant 仍被保留）。
    while first + removed < msgs.len().saturating_sub(1)
        && role_of(&msgs[first + removed]) == "tool"
    {
        removed += 1;
    }
    msgs.drain(first..first + removed);
    msgs.insert(
        first,
        serde_json::json!({
            "role": "system",
            "content": "[上下文超限] 早前对话已因超出模型上下文窗口被裁剪，上文从本条之后开始。"
        }),
    );
    (first, removed)
}

/// 降级砍历史后修正按 msgs 下标记账的旁路表：落在切口内的条目作废（消息已不存在），
/// 其后条目左移——净变化是 `-(removed-1)`（砍掉 removed 条、补回 1 条裁剪占位）。
pub(crate) fn reindex_after_shed(ages: &mut Vec<(usize, usize)>, first: usize, removed: usize) {
    if removed == 0 {
        return;
    }
    let end = first + removed;
    let shift = removed - 1;
    ages.retain_mut(|(_, idx)| {
        if *idx < first {
            true
        } else if *idx < end {
            false
        } else {
            *idx -= shift;
            true
        }
    });
}
