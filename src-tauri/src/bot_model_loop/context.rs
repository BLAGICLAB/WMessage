//! 上下文瘦身层：超限判定、历史裁剪、轮内工具结果滑动窗与重排索引修正。
//!
//! 本文件由 bot_model_loop.rs 拆分而来（纯搬移，行为零变化）。

// 轮内工具结果滑动窗（Anthropic context editing 对标）
//
// 触发阈值按**字符**而不是估算 token：真实痕迹库（124 trace / 861 span）里
// 单次提问的工具结果总量 p50=1071 / p75=9704 / p90=22637 / p95=32764 /
// max=359810 字符。取 24_000 落在 p90 与 p95 之间，约 9% 的提问会触发；
// 换算成 token（24K token ≈ 72K 字符）只命中 0.8%——等于不做。
//
// 为什么不引 tokenizer：要多一个依赖 + 一份词表，换来的是把上面那个字符口径
// 换成一个同样粗的估算（中文 1 字 ≈ 1 token，ASCII 1 字符 ≈ 0.3 token，两边
// 都差着倍数），实测阈值本来也只要求「落在 p90 附近」这个量级。

/// 轮内工具结果滑动窗的触发阈值（字符）：轮内累计工具结果超过它就触发裁剪。
pub(crate) const TOOL_WINDOW_TRIGGER_CHARS: usize = 24_000;

/// 滑动窗保留的最近工具结果条数：这几条永不裁。
///
/// 6 条的来由：SWE-agent 只留最近 5 条观察比全历史高 3 分；这里留 6 条，
/// 比那条口径多一条余量——多一条的代价是几千字符，少一条的代价是模型忘了
/// 上一轮刚做过什么（重跑工具 = 重跑副作用）。
pub(crate) const TOOL_WINDOW_KEEP_RECENT: usize = 6;

/// 桩子前缀：模型一眼认出「这条被裁了、原文还在、能调回来」。
const TOOL_WINDOW_STUB_MARK: &str = "⚠️ 已省略以控制上下文";

/// 轮内工具结果累计字符数（只算 role=tool 且 content 是字符串的消息）。
///
/// 只数工具结果：user/assistant 文本不参与这个口径（它们既不裁也不该触发裁剪）。
pub(crate) fn tool_result_chars(msgs: &[serde_json::Value]) -> usize {
    msgs.iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
        .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
        .map(|s| s.chars().count())
        .sum()
}

/// 轮内工具结果滑动窗：把「最近 keep_recent 条之外」的工具结果消息替换成桩子。
///
/// 护栏（逐条对着设计文档的口径）：
/// - **只裁 role=tool 的消息**——user/assistant 文本一律不碰；
/// - **最近 keep_recent 条永不裁**；
/// - **带图片的消息整体跳过**——图片挂在 role=user 的多模态消息上，本来就不在
///   可裁集合里（本函数只改 role=tool 消息的 content，多模态数组不碰）；
/// - **只改 prompt，DB 零改动**——执行痕迹表与溢出文件原样保留，桩子给的是
///   「调回原文」的入口，不是「原文没了」。
///
/// 返回改写条数（0 = 没裁）。
pub(crate) fn apply_tool_window(msgs: &mut [serde_json::Value], keep_recent: usize) -> usize {
    // 可裁集合：role=tool 且 content 是字符串的消息（content 是数组的多模态
    // 消息不动——改它会毁掉图片块的结构）。
    let candidates: Vec<usize> = msgs
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            m.get("role").and_then(|r| r.as_str()) == Some("tool")
                && m.get("content").and_then(|c| c.as_str()).is_some()
        })
        .map(|(i, _)| i)
        .collect();
    if candidates.len() <= keep_recent {
        return 0;
    }
    // 最近 keep_recent 条留在窗口内
    let cut_end = candidates.len() - keep_recent;
    let mut trimmed = 0usize;
    for &idx in candidates.iter().take(cut_end) {
        let Some(call_id) = msgs[idx]
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        // 名字从上面那条 assistant(tool_calls) 消息里找——它不裁（护栏②），
        // 桩子带上名字模型才知道自己省掉的是什么。
        let name = tool_name_of(msgs, idx, &call_id).unwrap_or_else(|| "未知工具".to_string());
        let stub = format!(
            "{TOOL_WINDOW_STUB_MARK}。工具 {name} 的完整结果已存档（调用 id={call_id}），\
             需要时调用 lookup_receipt(tool_call_id=\"{call_id}\") 调取原文。"
        );
        if let Some(obj) = msgs[idx].as_object_mut() {
            obj.insert("content".to_string(), serde_json::Value::String(stub));
            trimmed += 1;
        }
    }
    trimmed
}

/// 某条 tool 结果对应的工具名：往上找最近那条带 tool_calls 的 assistant 消息，
/// 按 tool_call_id 对上号。找不到返回 None（桩子里就不写名字，不编）。
fn tool_name_of(msgs: &[serde_json::Value], idx: usize, tool_call_id: &str) -> Option<String> {
    msgs[..idx].iter().rev().find_map(|m| {
        if m.get("role").and_then(|r| r.as_str()) != Some("assistant") {
            return None;
        }
        m.get("tool_calls")?.as_array()?.iter().find_map(|tc| {
            if tc.get("id").and_then(|v| v.as_str()) != Some(tool_call_id) {
                return None;
            }
            tc.get("function")?
                .get("name")?
                .as_str()
                .map(str::to_string)
        })
    })
}

/// 意图片段的字符上限：每段最多留这么多字进侧调请求。
const INTENT_PART_CHARS: usize = 200;

/// 用户意图摘要：侧调自检要用的「用户要什么」（纯函数）。
///
/// 侧调模型看不到主对话历史——不带意图，它判「是否达意」就退化成「工具有没有
/// 报错」，那道闸等于白设（永远答 Y）。带上了才与 in-loop 判定站在同一条信息线上。
///
/// 取**最早**与**最近**两条真人用户消息：聊天链路的消息栈是整段会话历史
/// （首条是最初的诉求、末条是本轮诉求），任务执行链路只有一条（任务卡块），
/// 两种链路都落在这条规则上。首条取头、末条取尾——注入块（摘要/记忆）拼在
/// 用户原话**前面**（见 bot_chat::inject），取尾才拿得到原话。
pub(crate) fn user_intent_of(msgs: &[serde_json::Value]) -> String {
    let users: Vec<&str> = msgs
        .iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
        .collect();
    let first = users.first().copied().unwrap_or_default();
    let last = users.last().copied().unwrap_or_default();
    if first.is_empty() {
        return String::new();
    }
    let head: String = first.chars().take(INTENT_PART_CHARS).collect();
    let tail: String = {
        let n = last.chars().count();
        if n <= INTENT_PART_CHARS {
            last.to_string()
        } else {
            last.chars().skip(n - INTENT_PART_CHARS).collect()
        }
    };
    if head == tail {
        return head;
    }
    format!("最早：{head}；最近：{tail}")
}

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
