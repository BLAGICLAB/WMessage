//! 侧调自检的单元测试（路由阈值两态 + 意图抽取）。
//!
//! 侧调这条路要成立，前提是它拿到的信息不比 in-loop 少太多：拿不到用户意图
//! 它只会答 Y（那道闸等于白设），上下文不够长则省下的那一轮不值当。两条
//! 前提各有边界用例钉住。

use crate::bot_model_loop::context::user_intent_of;
use crate::bot_model_loop::guard::{side_selfcheck_routes_to_sidecall, SIDE_SELFCHECK_MIN_CHARS};

/// 路由阈值两态：够长才走侧调，不够长留在主循环。
#[test]
fn sidecall_routes_only_when_history_is_long_enough() {
    // 没配侧调模型 → 哪都不去
    assert!(!side_selfcheck_routes_to_sidecall(
        false,
        true,
        SIDE_SELFCHECK_MIN_CHARS * 10
    ));
    // 拿不到意图 → 留在主循环（侧调没意图只会答 Y）
    assert!(!side_selfcheck_routes_to_sidecall(
        true,
        false,
        SIDE_SELFCHECK_MIN_CHARS * 10
    ));
    // 上下文不够长 → 留在主循环
    assert!(!side_selfcheck_routes_to_sidecall(
        true,
        true,
        SIDE_SELFCHECK_MIN_CHARS - 1
    ));
    // 够长 + 有意图 → 走侧调
    assert!(side_selfcheck_routes_to_sidecall(
        true,
        true,
        SIDE_SELFCHECK_MIN_CHARS
    ));
}

/// 阈值本身：消息预算的 1/5（40K 字符）。
///
/// 钉死这个数是为了防「有人调小阈值 → 侧调在短会话里乱接管 → 判定力下降」
/// 这条退化路径。改动它必须同步设计文档的偏离登记。
#[test]
fn sidecall_threshold_is_one_fifth_of_msgs_budget() {
    assert_eq!(SIDE_SELFCHECK_MIN_CHARS, 40_000);
}

/// 任务执行链路：只有一条用户消息（任务卡块），意图就是它。
#[test]
fn intent_of_single_user_message_is_that_message() {
    let msgs = vec![
        serde_json::json!({"role": "system", "content": "系统提示"}),
        serde_json::json!({"role": "user", "content": "[任务卡执行]\n标题：写季度报告"}),
    ];
    let intent = user_intent_of(&msgs);
    assert!(intent.contains("写季度报告"), "意图丢了：{intent}");
    assert!(
        !intent.contains("系统提示"),
        "system 消息不是意图：{intent}"
    );
}

/// 聊天链路：首条是最初的诉求、末条是本轮诉求，两头都要。
#[test]
fn intent_of_chat_history_covers_first_and_last() {
    let msgs = vec![
        serde_json::json!({"role": "system", "content": "系统提示"}),
        serde_json::json!({"role": "user", "content": "帮我把待办按项目分组"}),
        serde_json::json!({"role": "assistant", "content": "好的"}),
        serde_json::json!({"role": "user", "content": "[早前对话摘要]\n……\n\n继续处理第二组"}),
    ];
    let intent = user_intent_of(&msgs);
    assert!(
        intent.contains("帮我把待办按项目分组"),
        "最早的诉求丢了：{intent}"
    );
    assert!(intent.contains("继续处理第二组"), "本轮诉求丢了：{intent}");
}

/// 注入块拼在用户原话前面（见 bot_chat::inject），取尾才拿得到原话。
#[test]
fn intent_takes_tail_of_last_message_to_skip_injected_block() {
    let msgs = vec![
        serde_json::json!({"role": "user", "content": "最早的需求"}),
        serde_json::json!({"role": "user", "content": format!("{}{}", "摘要".repeat(400), "本轮原话")}),
    ];
    let intent = user_intent_of(&msgs);
    assert!(intent.contains("本轮原话"), "本轮原话丢了：{intent}");
    assert!(
        !intent.contains(&"摘要".repeat(400)),
        "注入块不该整段进侧调请求：{intent}"
    );
}

/// 没有真人用户消息 → 意图为空（调用方据此不路由侧调）。
#[test]
fn intent_is_empty_without_user_messages() {
    let msgs = vec![
        serde_json::json!({"role": "system", "content": "系统提示"}),
        serde_json::json!({"role": "assistant", "content": "模型自己说的话"}),
    ];
    assert!(user_intent_of(&msgs).is_empty());
}

/// 多模态用户消息（content 是块数组）不参与抽取——抽不出纯文本就当没有。
#[test]
fn intent_skips_non_string_content() {
    let msgs = vec![serde_json::json!({
        "role": "user",
        "content": [{"type": "text", "text": "带图的消息"}]
    })];
    assert!(user_intent_of(&msgs).is_empty());
}
