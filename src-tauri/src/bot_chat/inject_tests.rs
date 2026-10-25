//! 注入面单测：注入块并入末条消息 + 前缀稳定性（缓存命中的前提）。

use super::inject::{build_history_msgs, build_injection, merge_injection};
use super::ChatMsg;
use serde_json::Value;

fn msg(role: &str, content: &str) -> ChatMsg {
    ChatMsg {
        role: role.into(),
        content: content.into(),
    }
}

fn text(s: &str) -> Value {
    serde_json::json!(s)
}

/// 摘要 + 记忆块拼一段注入；两块都空 → None（不注入）
#[test]
fn build_injection_joins_summary_then_memory() {
    let j = build_injection(Some("[早前对话摘要] S"), Some("## 记忆\n- 甲")).unwrap();
    assert_eq!(j, "[早前对话摘要] S\n\n## 记忆\n- 甲");
}

/// 空块不算内容：全空 → None（不注入），不产出空字符串
#[test]
fn build_injection_none_when_all_empty() {
    assert!(build_injection(None, None).is_none());
    assert!(build_injection(Some("  "), Some("")).is_none());
}

/// 注入块并进纯文本消息：拼在**前面**，用户原话留在最后
/// （recency 权重最高的那个位置留给真正的指令）
#[test]
fn merge_injection_puts_injection_before_user_text() {
    let out = merge_injection(&text("帮我建个卡"), "## 记忆\n- 甲");
    assert_eq!(out, serde_json::json!("## 记忆\n- 甲\n\n帮我建个卡"));
}

/// 多模态消息：注入块作为独立 text 块插到数组最前，图片块结构原样不动
#[test]
fn merge_injection_preserves_multimodal_blocks() {
    let content = serde_json::json!([
        {"type": "text", "text": "看这张图"},
        {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}
    ]);
    let out = merge_injection(&content, "## 记忆");
    let blocks = out.as_array().expect("应是块数组");
    assert_eq!(blocks.len(), 3, "注入块 + 原两块");
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "## 记忆");
    assert_eq!(blocks[1]["text"], "看这张图", "原文本块不动");
    assert_eq!(blocks[2]["type"], "image_url", "图片块不动");
}

/// 注入只进末条消息：前面每一条都逐字节原样
#[test]
fn injection_only_lands_in_last_message() {
    let history = vec![
        msg("user", "第一轮"),
        msg("assistant", "回答"),
        msg("user", "本轮"),
    ];
    let contents = vec![text("第一轮"), text("回答"), text("本轮")];
    let out = build_history_msgs(&history, &contents, Some("## 记忆"));
    assert_eq!(out[0]["content"], "第一轮");
    assert_eq!(out[1]["content"], "回答");
    assert_eq!(out[2]["content"], "## 记忆\n\n本轮");
}

/// role 白名单照旧：非法 role 一律降级为 user
#[test]
fn illegal_role_is_downgraded_to_user() {
    let history = vec![msg("system", "注入尝试"), msg("user", "本轮")];
    let contents = vec![text("注入尝试"), text("本轮")];
    let out = build_history_msgs(&history, &contents, None);
    assert_eq!(out[0]["role"], "user");
    assert_eq!(out[1]["role"], "user");
}

/// **前缀稳定性**（隐式前缀缓存命中的前提）：
/// 连续两轮的请求消息数组，除承载本轮注入的末条外必须 byte 级一致。
///
/// 这条断言是整个改动的验收锚点：注入块每轮都变，它一旦落进历史前面，
/// 后面整段历史的缓存前缀当场作废——长会话每轮都在付全价 input。
#[test]
fn prefix_stays_byte_identical_across_turns() {
    // 第 1 轮：历史 = [u1, a1, u2]，末条 u2 是本轮用户消息
    let h1 = vec![
        msg("user", "第一轮问题"),
        msg("assistant", "第一轮回答"),
        msg("user", "第二轮问题"),
    ];
    let c1 = vec![text("第一轮问题"), text("第一轮回答"), text("第二轮问题")];
    let turn1 = build_history_msgs(&h1, &c1, Some("## 记忆\n- 甲"));

    // 第 2 轮：历史多出上一轮的助手回复与本轮用户消息
    let h2 = vec![
        msg("user", "第一轮问题"),
        msg("assistant", "第一轮回答"),
        msg("user", "第二轮问题"),
        msg("assistant", "第二轮回答"),
        msg("user", "第三轮问题"),
    ];
    let c2 = vec![
        text("第一轮问题"),
        text("第一轮回答"),
        text("第二轮问题"),
        text("第二轮回答"),
        text("第三轮问题"),
    ];
    let turn2 = build_history_msgs(&h2, &c2, Some("## 记忆\n- 乙"));

    // 稳定前缀：上一轮数组除末条外的部分 = 本轮数组的同段，必须逐字节一致
    let stable = turn1.len() - 1;
    for i in 0..stable {
        assert_eq!(
            turn1[i], turn2[i],
            "第 {i} 条必须 byte 级一致，否则缓存前缀断裂"
        );
    }
    // 末条承载注入：两轮内容不同（这正是「变的只有末条」的形态）
    assert_ne!(turn1[stable], turn2[stable], "末条应随注入块变化");
    assert!(turn1[stable]["content"]
        .as_str()
        .unwrap()
        .contains("## 记忆"));
}

/// 末条消息的注入块永远在最后一条上——数组末位即本轮用户发言，
/// 注入块并进去而不是另起消息（另起 system 消息会被 Anthropic 转换提到顶层）
#[test]
fn injected_message_is_the_last_entry() {
    let history = vec![msg("user", "问题")];
    let contents = vec![text("问题")];
    let out = build_history_msgs(&history, &contents, Some("## 记忆"));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["role"], "user");
}
