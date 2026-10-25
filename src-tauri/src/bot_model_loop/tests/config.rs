//! 模型循环单元测试（config 桶）。
//!
//! 原先内联在 bot_model_loop.rs 尾部，拆分时逐字搬移；
//! 只把源码守卫的扫描源换成 prod_src()。

use super::*;
use crate::bot_model_loop::request::build_request_body;
use crate::bot_model_loop::shell::resolve_loop_inference;

#[cfg(test)]
mod inference_param_tests {
    use super::*;

    #[test]
    fn injects_sampling_params_only_when_present() {
        // 两个都有 → 都写
        let mut body = serde_json::json!({"model": "m"});
        apply_inference_params(&mut body, Some(0.7), Some(0.9));
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["top_p"], 0.9);
        // 只有 temperature → top_p 字段不出现
        let mut body = serde_json::json!({});
        apply_inference_params(&mut body, Some(0.5), None);
        assert_eq!(body["temperature"], 0.5);
        assert!(body.get("top_p").is_none(), "无值不应写出 top_p 字段");
        // 只有 top_p → temperature 字段不出现
        let mut body = serde_json::json!({});
        apply_inference_params(&mut body, None, Some(0.8));
        assert!(
            body.get("temperature").is_none(),
            "无值不应写出 temperature 字段"
        );
        assert_eq!(body["top_p"], 0.8);
        // 都无 → body 原样（不得出现空字段污染请求体）
        let mut body = serde_json::json!({"model": "m"});
        apply_inference_params(&mut body, None, None);
        assert_eq!(body, serde_json::json!({"model": "m"}));
    }

    /// 配置档温度的优先级三态：配置档 > 模型条目 > 都不写。
    #[test]
    fn config_temperature_wins_over_the_model_entry() {
        // 配置档有值（temperature: 0 也算有值，不能被当成缺省）
        assert_eq!(resolve_temperature(Some(0.0), Some(0.9)), Some(0.0));
        // 配置档没写 → 用条目级
        assert_eq!(resolve_temperature(None, Some(0.9)), Some(0.9));
        // 两边都没有 → 仍无值，请求体不写 temperature（沿用「有值才写」）
        assert_eq!(resolve_temperature(None, None), None);
        let mut body = serde_json::json!({"model": "m"});
        apply_inference_params(&mut body, resolve_temperature(None, None), None);
        assert!(
            body.get("temperature").is_none(),
            "两侧都无值不得写出空 temperature 字段"
        );
    }

    /// 接线守卫：模型循环真的用了 `resolve_temperature` 取值。
    ///
    /// `concat!` 的**两段都非空**才算拆开：`concat!("...", "")` 等于没拆，
    /// 拼出来的串仍会以完整形态出现在测试自己的源码里，于是自匹配、假绿。
    /// 本条第一版正是这么写的（第二段是空串），改掉注入点照样绿。
    #[test]
    fn model_loop_applies_resolve_temperature() {
        assert!(
            prod_src().contains(concat!("temperature: resolve_", "temperature(")),
            "模型循环必须经 resolve_temperature 取温度（改反优先级会红）"
        );
    }

    #[test]
    fn anthropic_body_takes_sampling_after_build() {
        // Anthropic /v1/messages 的 temperature/top_p 是与 OpenAI 同名的顶层字段：
        // build_anthropic_body 之后注入即生效，max_tokens 等既有字段不受影响
        let msgs = vec![serde_json::json!({"role": "user", "content": "hi"})];
        let mut body = crate::bot_anthropic::build_anthropic_body(
            "claude-sonnet-4-5",
            &msgs,
            &serde_json::json!([]),
            8_192,
            false,
        )
        .expect("msgs 合法，转换必须成功");
        apply_inference_params(&mut body, Some(0.2), Some(1.0));
        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 1.0);
        assert_eq!(body["max_tokens"], 8_192);
    }

    /// active 条目 temperature=0.9 / 覆盖条目 temperature=0.2：override 命中时
    /// 主循环必须采用覆盖条目的推理参数（曾无条件走 active 条目解析，覆盖参数整组丢失）
    #[test]
    fn override_hit_takes_entry_inference_not_active() {
        use crate::bot::{ActiveModelId, ModelEntry, ModelsByProvider};

        fn entry(id: &str, temperature: f64, top_p: f64) -> ModelEntry {
            ModelEntry {
                id: id.into(),
                label: id.into(),
                base_url: "https://api.example.com/v1".into(),
                model: format!("{id}-model"),
                vendor: None,
                enabled: true,
                context_k: None,
                temperature: Some(temperature),
                top_p: Some(top_p),
                max_tokens: Some(1111),
                system_prompt: Some(format!("{id}-system")),
            }
        }
        let mbp = ModelsByProvider {
            openai: vec![entry("active-e", 0.9, 0.95)],
            anthropic: vec![],
        };
        let active_id = ActiveModelId {
            openai: Some("active-e".into()),
            anthropic: None,
        };
        let resolved = crate::bot::config::ResolvedModel {
            base_url: "https://override.example.com/v1".into(),
            model: "override-model".into(),
            api_provider: "openai".into(),
            api_key: "k".into(),
            vendor: None,
            inference: crate::bot::EffectiveInference {
                max_tokens: 4096,
                temperature: Some(0.2),
                top_p: Some(0.3),
                system_prompt: Some("override-system".into()),
            },
        };

        // 命中：temperature/top_p/max_tokens/system_prompt 全取覆盖条目，
        // 不得混入 active 条目的 0.9/0.95/1111
        let hit = resolve_loop_inference(
            Some(&resolved),
            Some("openai"),
            Some(8192),
            Some(&active_id),
            Some(&mbp),
        );
        assert_eq!(hit.temperature, Some(0.2));
        assert_eq!(hit.top_p, Some(0.3));
        assert_eq!(hit.max_tokens, 4096);
        assert_eq!(hit.system_prompt.as_deref(), Some("override-system"));

        // 未命中：回落 active 条目解析（原路径不得回归）
        let miss = resolve_loop_inference(
            None,
            Some("openai"),
            Some(8192),
            Some(&active_id),
            Some(&mbp),
        );
        assert_eq!(miss.temperature, Some(0.9));
        assert_eq!(miss.top_p, Some(0.95));
        assert_eq!(miss.max_tokens, 1111);
        assert_eq!(miss.system_prompt.as_deref(), Some("active-e-system"));
    }
}

#[cfg(test)]
mod anthropic_thinking_tests {
    use super::*;

    fn http_with(provider: crate::bot::ApiProvider) -> LlmHttp {
        LlmHttp {
            client: reqwest::Client::new(),
            base_url: "https://example.invalid".into(),
            api_key: "k".into(),
            model: "m".into(),
            provider,
            max_tokens: 1024,
            reasoning: crate::bot::reasoning::ReasoningWire::None,
            temperature: None,
            top_p: None,
            system_prompt: None,
        }
    }

    fn msgs_with_thinking() -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"role": "user", "content": "查一下"}),
            with_thinking(
                serde_json::json!({"role": "assistant", "content": "我查一下"}),
                &[serde_json::json!({
                    "type": "thinking", "thinking": "先想", "signature": "SIG"
                })],
            ),
        ]
    }

    /// OpenAI 请求体不得带 Anthropic 的 thinking 块载体（provider 专属键）。
    ///
    /// 部分 OpenAI 兼容网关对消息里的未知字段直接 400；且这个键对 OpenAI
    /// 毫无意义。
    #[test]
    fn openai_body_drops_thinking_key() {
        let http = http_with(crate::bot::ApiProvider::Openai);
        let body = build_request_body(&http, &msgs_with_thinking(), &serde_json::json!([]), true)
            .expect("请求体应能构造");
        let arr = body["messages"].as_array().expect("messages 应是数组");
        assert!(
            arr.iter()
                .all(|m| m.get(crate::bot_anthropic::THINKING_KEY).is_none()),
            "OpenAI 请求体不得带 thinking 块载体：{arr:?}"
        );
        // 正文不能被顺手剥掉
        assert_eq!(arr[1]["content"], "我查一下");
    }

    /// Anthropic 请求体要把 thinking 块带上（否则下一轮带 tool_result 的
    /// 请求被 400 "cannot be modified"）。
    #[test]
    fn anthropic_body_carries_thinking_block() {
        let http = http_with(crate::bot::ApiProvider::Anthropic);
        let body = build_request_body(&http, &msgs_with_thinking(), &serde_json::json!([]), true)
            .expect("请求体应能构造");
        let arr = body["messages"].as_array().expect("messages 应是数组");
        let asst = arr
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("应有 assistant 轮");
        let blocks = asst["content"].as_array().expect("content 应是块数组");
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["signature"], "SIG");
    }

    /// 剥键只动 provider 专属键，其余内容原样保留。
    #[test]
    fn strip_thinking_key_keeps_other_fields() {
        let msgs = msgs_with_thinking();
        let stripped = strip_thinking_key(&msgs);
        assert_eq!(stripped.len(), msgs.len());
        assert!(stripped[1]
            .get(crate::bot_anthropic::THINKING_KEY)
            .is_none());
        assert_eq!(stripped[1]["content"], "我查一下");
        // 没有这个键时原样返回（不重建数组）
        let plain = vec![serde_json::json!({"role": "user", "content": "x"})];
        let out = strip_thinking_key(&plain);
        assert_eq!(out, plain);
    }
}

/// 思考段回灌的盖章机制（DeepSeek 这类模型缺了 reasoning_content 直接 400）。
#[cfg(test)]
mod reasoning_echo_tests {
    use super::*;

    /// 盖章打在**最后一条** assistant 消息上，早先的那条不动。
    #[test]
    fn stamps_the_last_assistant_message_only() {
        let mut msgs = vec![
            serde_json::json!({"role": "user", "content": "查一下"}),
            serde_json::json!({"role": "assistant", "content": "早先那句"}),
            serde_json::json!({"role": "user", "content": "系统提示：返工"}),
            serde_json::json!({"role": "assistant", "content": "本轮答复"}),
            serde_json::json!({"role": "user", "content": "系统提示：续跑"}),
        ];
        stamp_last_assistant_reasoning(&mut msgs, "本轮思考");
        // 早先那条 assistant 没被动过（它是缓存前缀的一部分）
        assert!(msgs[1].get("reasoning_content").is_none());
        assert_eq!(msgs[3]["reasoning_content"], "本轮思考");
    }

    /// 已有 reasoning_content 的不覆盖（同一个 assistant 消息可能被盖两次）。
    #[test]
    fn does_not_overwrite_existing_reasoning() {
        let mut msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": "我查一下",
            "reasoning_content": "原来的",
        })];
        stamp_last_assistant_reasoning(&mut msgs, "新的");
        assert_eq!(msgs[0]["reasoning_content"], "原来的");
    }

    /// 空思考段不盖章：模型没思考就没有可回灌的东西。
    #[test]
    fn blank_reasoning_is_not_stamped() {
        let mut msgs = vec![serde_json::json!({"role": "assistant", "content": "答"})];
        stamp_last_assistant_reasoning(&mut msgs, "   ");
        assert!(msgs[0].get("reasoning_content").is_none());
    }

    /// 没有 assistant 消息时不报错（防御：空栈）。
    #[test]
    fn no_assistant_message_is_a_noop() {
        let mut msgs = vec![serde_json::json!({"role": "user", "content": "问"})];
        stamp_last_assistant_reasoning(&mut msgs, "思考");
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].get("reasoning_content").is_none());
    }
}

#[cfg(test)]
mod think_tests {
    use super::*;

    fn run(chunks: &[&str]) -> (String, String) {
        let mut mode = false;
        let mut buf = String::new();
        let mut normal = String::new();
        let mut think = String::new();
        for c in chunks {
            let (n, t) = feed_think(&mut mode, &mut buf, c);
            normal.push_str(&n);
            think.push_str(&t);
        }
        (normal, think)
    }

    #[test]
    fn think_split_across_chunks() {
        // 标签和内容都跨块
        let (n, t) = run(&["<thi", "nk>思考中…", "</th", "ink>答案是 42"]);
        assert_eq!(n, "答案是 42");
        assert_eq!(t, "思考中…");
    }

    #[test]
    fn think_whole_in_one_chunk() {
        let (n, t) = run(&["<think>先想一下</think>好的"]);
        assert_eq!(n, "好的");
        assert_eq!(t, "先想一下");
    }

    #[test]
    fn plain_text_no_tags() {
        let (n, t) = run(&["直接回答，没有思考"]);
        assert_eq!(n, "直接回答，没有思考");
        assert_eq!(t, "");
    }

    #[test]
    fn multiple_think_blocks() {
        let (n, t) = run(&["<think>A</think>正文1<think>B</think>正文2"]);
        assert_eq!(n, "正文1正文2");
        assert_eq!(t, "AB");
    }
}

#[cfg(test)]
mod context_overflow_tests {
    use super::*;

    #[test]
    fn overflow_detected_across_vendor_payload_shapes() {
        // OpenAI 形态
        assert!(is_context_overflow(
            400,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens","code":"context_length_exceeded"}}"#
        ));
        // Anthropic 形态
        assert!(is_context_overflow(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#
        ));
        // 国产网关中文形态 + 大小写不敏感
        assert!(is_context_overflow(400, "错误：上下文超长，请缩短对话"));
        assert!(is_context_overflow(
            413,
            "payload too large: context window"
        ));
        // 短 body 也认（关键词在）
        assert!(is_context_overflow(400, "too many tokens"));
    }

    #[test]
    fn other_400s_are_not_treated_as_overflow() {
        // 400 的其它形态不得触发降级：砍历史治不好鉴权/参数错，白砍一轮还多花一次请求
        assert!(!is_context_overflow(
            400,
            r#"{"error":{"message":"Invalid API key","code":"invalid_api_key"}}"#
        ));
        assert!(!is_context_overflow(
            400,
            r#"{"error":{"message":"tools[0].function.parameters: invalid schema"}}"#
        ));
        // 关键词出现在非超长语境也不误伤：只看 400/413 + 特征词，不做语义推理
        assert!(!is_context_overflow(401, "invalid api key"));
        assert!(!is_context_overflow(500, "internal error"));
        assert!(!is_context_overflow(200, r#"{"choices":[]}"#));
    }

    fn m(role: &str, n: usize) -> serde_json::Value {
        serde_json::json!({"role": role, "content": "x".repeat(n)})
    }

    fn assistant_with_tool_call(id: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "assistant",
            "content": serde_json::Value::Null,
            "tool_calls": [{"id": id, "type": "function",
                            "function": {"name": "noop", "arguments": "{}"}}]
        })
    }

    fn tool_msg(id: &str, n: usize) -> serde_json::Value {
        serde_json::json!({"role": "tool", "tool_call_id": id, "content": "y".repeat(n)})
    }

    /// 孤儿 tool：前面最近一条非 system 消息不是「带 tool_calls 的 assistant」。
    /// 这种结构发出去对端直接 400，降级等于白降。
    fn orphan_tool_positions(msgs: &[serde_json::Value]) -> Vec<usize> {
        msgs.iter()
            .enumerate()
            .filter(|(_, m)| m["role"] == "tool")
            .filter(|(i, _)| {
                msgs[..*i]
                    .iter()
                    .rev()
                    .find(|p| p["role"] != "system")
                    .is_none_or(|p| p["role"] != "assistant" || p.get("tool_calls").is_none())
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// 复现：降级砍历史会让「msgs 下标」记账的 tool 消息表整体失效。
    /// 记账值是入栈时的旧下标，砍完 msgs 变短却不修正——下一轮按它压缩就
    /// 越界 panic，或错指到别的消息。
    #[test]
    fn shed_keeps_recorded_tool_indices_addressable() {
        let mut msgs = vec![
            m("system", 10),
            m("user", 400),
            m("assistant", 400),
            m("user", 400),
            m("assistant", 400),
            assistant_with_tool_call("c1"),
            tool_msg("c1", 300),
            m("user", 10),
        ];
        // 与循环内一致：tool 消息入栈时记下 (轮次, msgs 下标)
        let mut recorded: Vec<(usize, usize)> = msgs
            .iter()
            .enumerate()
            .filter(|(_, x)| x["role"] == "tool")
            .map(|(i, _)| (0usize, i))
            .collect();
        assert_eq!(recorded.len(), 1, "本用例要有 tool 消息可记账");

        let (cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0, "超预算必须真砍：{dropped}");
        reindex_after_shed(&mut recorded, cut, dropped);

        for (_born, idx) in recorded {
            assert!(
                idx < msgs.len(),
                "降级后记账下标 {idx} 越界：msgs 只剩 {} 条（下标须随切口左移）",
                msgs.len()
            );
            assert_eq!(
                msgs[idx]["role"], "tool",
                "记账下标 {idx} 已错指到非 tool 消息：{:?}",
                msgs[idx]
            );
        }
    }

    /// 落在切口内的记账必须作废：那条 tool 消息已被砍掉，下标不再指向任何消息，
    /// 留着就是一颗定时炸弹（后续轮按它压缩必越界）。
    #[test]
    fn shed_drops_bookkeeping_for_removed_tool_messages() {
        let mut msgs = vec![
            m("system", 10),
            m("user", 400),
            assistant_with_tool_call("c1"),
            tool_msg("c1", 300),
            tool_msg("c2", 300),
            m("user", 10),
        ];
        let mut recorded: Vec<(usize, usize)> = msgs
            .iter()
            .enumerate()
            .filter(|(_, x)| x["role"] == "tool")
            .map(|(i, _)| (0usize, i))
            .collect();
        assert_eq!(recorded.len(), 2);

        let (cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0);
        reindex_after_shed(&mut recorded, cut, dropped);

        for (_born, idx) in &recorded {
            assert!(
                *idx < msgs.len(),
                "残留记账 {idx} 指向已被砍掉的位置（msgs 只剩 {} 条）：{recorded:?}",
                msgs.len()
            );
            assert_eq!(msgs[*idx]["role"], "tool");
        }
    }

    /// 切口不能停在 tool 分组中间：砍掉带 tool_calls 的 assistant 却留下它的
    /// tool 消息，对端 400（tool 必须紧跟带 tool_calls 的 assistant）。
    #[test]
    fn shed_never_leaves_orphan_tool_message() {
        let mut msgs = vec![
            m("system", 10),
            m("user", 400),
            assistant_with_tool_call("c1"),
            tool_msg("c1", 300),
            tool_msg("c2", 300),
            m("user", 10),
        ];
        let (_cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0, "超预算必须真砍：{dropped}");
        let orphans = orphan_tool_positions(&msgs);
        assert!(
            orphans.is_empty(),
            "降级后留下孤儿 tool 消息（下标 {orphans:?}），发出去必 400：{msgs:?}"
        );
    }

    #[test]
    fn shed_keeps_system_block_and_current_turn() {
        let mut msgs = vec![
            m("system", 10),     // 系统提示词本体：不可砍
            m("system", 5),      // 摘要/记忆块：不可砍
            m("user", 400),      // 旧历史：可砍
            m("assistant", 400), // 旧历史：可砍
            m("user", 10),       // 本轮输入：不可砍
        ];
        let (_cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert!(dropped > 0, "超预算必须真砍：{dropped}");
        let roles: Vec<&str> = msgs.iter().filter_map(|x| x["role"].as_str()).collect();
        assert_eq!(
            roles.iter().filter(|r| **r == "system").count(),
            3,
            "两个原 system + 一条裁剪说明都要在：{roles:?}"
        );
        assert_eq!(
            msgs.last().unwrap()["content"],
            serde_json::json!("x".repeat(10)),
            "最后一条（本轮用户输入）必须保留"
        );
        assert!(
            !msgs
                .iter()
                .any(|x| x["content"] == serde_json::json!("x".repeat(400))),
            "旧历史应被砍掉：{roles:?}"
        );
    }

    #[test]
    fn shed_is_noop_when_nothing_left_to_drop() {
        // 只有 system + 本轮输入：没得砍，调用方据此走报错出口而非空转重发
        let mut msgs = vec![m("system", 10), m("user", 10)];
        assert_eq!(shed_oldest_history(&mut msgs, 0.5).1, 0);
        assert_eq!(msgs.len(), 2, "空转不得插入裁剪说明");

        let mut empty: Vec<serde_json::Value> = Vec::new();
        assert_eq!(shed_oldest_history(&mut empty, 0.5).1, 0);
    }

    #[test]
    fn shed_never_touches_messages_after_first_non_system() {
        // 本轮 tool 消息（role=tool）必须原样保留：砍掉会破坏
        // tool_call/tool_result 配对，对端直接 400（那就白降级了）
        let mut msgs = vec![
            m("system", 10),
            m("user", 500),
            serde_json::json!({"role": "tool", "tool_call_id": "c1", "content": "y".repeat(300)}),
        ];
        let (_cut, dropped) = shed_oldest_history(&mut msgs, 0.5);
        assert_eq!(dropped, 1, "只砍 user 那条：{dropped}");
        assert!(
            msgs.iter().any(|x| x.get("tool_call_id").is_some()),
            "tool 消息不得被砍"
        );
    }
}

#[cfg(test)]
mod selfcheck_vendor_gate_tests {
    use super::*;
    use crate::bot::config::ResolvedModel;

    fn resolved(vendor: Option<&str>) -> Result<ResolvedModel, crate::error::CommandError> {
        Ok(ResolvedModel {
            base_url: "https://third-party.example/v1".into(),
            model: "m".into(),
            api_provider: "openai".into(),
            api_key: "sk-side".into(),
            vendor: vendor.map(|v| v.to_string()),
            inference: crate::bot::config::EffectiveInference {
                max_tokens: 32,
                temperature: None,
                top_p: None,
                system_prompt: None,
            },
        })
    }

    /// 侧调条目指向没过连接测试的厂商 → 侧调不发（回落主路径内的自检）
    ///
    /// 回归背景：主请求被 vendor 门拦下，侧调请求却照发——真实带 key 的请求
    /// 打往未验证的 base_url。
    #[test]
    fn selfcheck_must_not_use_a_vendor_that_failed_the_connection_test() {
        let d = decide_selfcheck(resolved(Some("未验证厂商")), &[]);
        assert!(
            matches!(d, SelfcheckDecision::VendorNotVerified { .. }),
            "未验证厂商的侧调条目必须被拦下（侧调请求是真实带 key 的请求）"
        );
        let d = decide_selfcheck(resolved(Some("未验证厂商")), &["别的厂商".into()]);
        assert!(
            matches!(d, SelfcheckDecision::VendorNotVerified { .. }),
            "已验证名单里没有它 → 仍须拦下"
        );
    }

    /// 已验证厂商 → 放行；无 vendor 的老条目 → 放行（与主路径同口径）
    #[test]
    fn selfcheck_allows_verified_vendors_and_legacy_entries() {
        assert!(
            matches!(
                decide_selfcheck(resolved(Some("已验证厂商")), &["已验证厂商".into()]),
                SelfcheckDecision::Use(_)
            ),
            "已验证厂商应当放行"
        );
        assert!(
            matches!(
                decide_selfcheck(resolved(None), &[]),
                SelfcheckDecision::Use(_)
            ),
            "无 vendor 的老条目没有验证概念，应当放行"
        );
    }

    /// 条目解析失败 → 回落（不发侧调），错误可行动
    #[test]
    fn selfcheck_falls_back_when_the_entry_cannot_be_resolved() {
        let d = decide_selfcheck(
            Err(crate::error::CommandError::ApiKeyMissing),
            &["已验证厂商".into()],
        );
        assert!(
            matches!(d, SelfcheckDecision::ResolveFailed(_)),
            "解析失败必须回落（不发侧调）"
        );
    }
}
