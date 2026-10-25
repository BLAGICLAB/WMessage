//! 上下文注入面：把每轮都变的注入块放到请求消息数组的**末尾**。
//!
//! 为什么要挪：隐式前缀缓存（OpenAI 兼容口径，MiniMax 官方文档口径）按
//! 「工具列表 → system → 消息」顺序做**最长前缀匹配**。摘要与记忆块每轮都变
//! （摘要按截断生成、记忆块按本轮用户消息做语义检索），它们原先卡在主
//! system 之后、全部历史之前——一变，后面整段历史的缓存前缀当场作废，
//! 长会话每轮都在付全价 input。
//!
//! 挪到末尾后，稳定前缀 = 主 system + 全部历史（末条用户消息除外），
//! 变的只有末条消息里那一小段注入文本。
//!
//! 形态：**并进本轮用户消息**，不另起 system 消息。两个原因：
//! - Anthropic 协议转换会把数组里**所有** role=system 的消息抽出来提到顶层
//!   system 字段（见 bot_anthropic::openai_msgs_to_anthropic）。另起 system
//!   消息在 Anthropic 路径上会被搬到最前面，等于没挪。
//! - 并进用户消息对任何厂商口径都安全：内容始终待在数组末尾，且用户原话
//!   仍在消息最后（recency 权重最高的那个位置）。
//!
//! 注入只发生在**请求装配时**：不进 ChatMsg、不落库、不污染历史。

use serde_json::Value;

use super::ChatMsg;

/// 注入块与本轮用户原话之间的分隔符
const INJECTION_SEP: &str = "\n\n";

/// 摘要与记忆块拼成一段注入文本（None = 没有可注入的内容）。
///
/// 两块都是「本轮视野的前置材料」：摘要服务「本轮对早前的记忆」，记忆块按
/// 本轮用户消息检索。顺序固定为摘要在前、记忆在后。
pub(super) fn build_injection(summary: Option<&str>, memory: Option<&str>) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for s in [summary, memory] {
        if let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(s);
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join(INJECTION_SEP))
}

/// 把注入块并进一条消息的 content（纯函数）。
///
/// - 纯文本消息：注入块拼在用户原话**前面**——用户原话留在消息最后，
///   recency 权重最高的那个位置留给真正的指令。
/// - 多模态消息（content 是块数组）：注入块作为独立 text 块插到数组最前，
///   图片块结构原样不动。
/// - 其它形状（理论上不会发生）：原样返回，不猜。
pub(super) fn merge_injection(content: &Value, injection: &str) -> Value {
    match content {
        Value::String(s) => Value::String(format!("{injection}{INJECTION_SEP}{s}")),
        Value::Array(blocks) => {
            let mut out: Vec<Value> = Vec::with_capacity(blocks.len() + 1);
            out.push(serde_json::json!({"type": "text", "text": injection}));
            out.extend(blocks.iter().cloned());
            Value::Array(out)
        }
        other => other.clone(),
    }
}

/// 历史消息 → 请求消息数组（纯函数）。
///
/// - `contents[i]`：第 i 条历史消息的最终 content（纯文本字符串或多模态块数组）
/// - `injection`：合并后的注入文本（None = 不注入）
///
/// 注入并进**最后一条**消息（末条 = 本轮用户发言）。末条之前的历史一个字
/// 都不动——那些是缓存前缀的主体。
pub(super) fn build_history_msgs(
    history: &[ChatMsg],
    contents: &[Value],
    injection: Option<&str>,
) -> Vec<Value> {
    let last = history.len().saturating_sub(1);
    history
        .iter()
        .enumerate()
        .map(|(i, m)| {
            // role 白名单：历史里的非法 role 一律按 user，
            // 防污染历史注入 system/tool 角色
            let role = if m.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            let content = match (i == last, injection) {
                (true, Some(inj)) => merge_injection(&contents[i], inj),
                _ => contents[i].clone(),
            };
            serde_json::json!({"role": role, "content": content})
        })
        .collect()
}
