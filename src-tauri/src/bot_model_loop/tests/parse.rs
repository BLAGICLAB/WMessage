//! 模型循环单元测试（parse 桶）。
//!
//! 原先内联在 bot_model_loop.rs 尾部，拆分时逐字搬移；
//! 只把源码守卫的扫描源换成 prod_src()。

use super::*;

#[cfg(test)]
mod parse_sse_chunk_tests {
    use super::*;

    #[test]
    fn parses_text_content_chunk() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(!parsed.is_done);
        assert_eq!(parsed.content.as_deref(), Some("hello"));
        assert!(parsed.tool_calls.is_empty());
    }

    #[test]
    fn parses_done_marker() {
        let parsed = parse_sse_chunk("data: [DONE]").unwrap();
        assert!(parsed.is_done);
        assert!(parsed.content.is_none());
        assert!(parsed.tool_calls.is_empty());
        assert!(parsed.finish_reason.is_none());
    }

    #[test]
    fn parses_tool_call_delta_with_index() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"list_tasks","arguments":"{}"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        let tc = &parsed.tool_calls[0];
        assert_eq!(tc.index, 0);
        assert_eq!(tc.id.as_deref(), Some("call_1"));
        assert_eq!(tc.name_chunk.as_deref(), Some("list_tasks"));
        assert_eq!(tc.arguments_chunk.as_deref(), Some("{}"));
    }

    #[test]
    fn parses_multiple_tool_calls_with_distinct_indices() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"a"}},{"index":1,"function":{"name":"b"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].name_chunk.as_deref(), Some("a"));
        assert_eq!(parsed.tool_calls[1].name_chunk.as_deref(), Some("b"));
    }

    #[test]
    fn parses_finish_reason_stop() {
        let line = r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn parses_finish_reason_tool_calls() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"x","function":{"name":"y","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.finish_reason.as_deref(), Some("tool_calls"));
        assert!(!parsed.tool_calls.is_empty());
    }

    #[test]
    fn empty_content_string_treated_as_absent() {
        // SSE 中 content="" 时应等同 None（不触发内容推送）
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert!(
            parsed.content.is_none(),
            "空 content 应等同 None（不触发推送）"
        );
    }

    #[test]
    fn empty_name_treated_as_append_noop() {
        // name="" 时 chunk 仍为 Some("")；是否跳过 append 由 caller 决定（生产代码只追加非空 name）
        let line =
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":""}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(
            parsed.tool_calls[0].name_chunk.as_deref(),
            Some(""),
            "name 字段存在但为空 — caller 决定是否跳过 append"
        );
    }

    #[test]
    fn returns_none_for_non_data_line() {
        assert!(parse_sse_chunk("event: ping").is_none());
        assert!(parse_sse_chunk("").is_none());
        assert!(parse_sse_chunk("data: not-json{").is_none());
        assert!(parse_sse_chunk("data:").is_none());
        assert!(parse_sse_chunk("data:    ").is_none());
    }

    #[test]
    fn returns_none_for_empty_choices() {
        // 网关异常：200 OK + choices:[] 且无 usage → 跳过
        let line = r#"data: {"choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    /// OpenAI 流式 usage：末块 `{"choices":[],"usage":{...}}` 必须被取到
    /// （choices 为空数组，早于本改动会在空判定处直接 return 丢掉）。
    #[test]
    fn openai_usage_chunk_with_empty_choices_is_captured() {
        let line = r#"data: {"id":"x","choices":[],"usage":{"prompt_tokens":5000,"completion_tokens":120,"total_tokens":5120,"prompt_tokens_details":{"cached_tokens":4000}}}"#;
        let c = parse_sse_chunk(line).expect("usage 末块不得被丢弃");
        assert!(c.content.is_none() && c.tool_calls.is_empty());
        let u = c.usage.expect("usage 末块必须带出用量");
        // 关键口径：prompt_tokens 含 cached，落库的普通 input 要减掉
        assert_eq!(u.input_tokens, 1000);
        assert_eq!(u.cache_read_tokens, 4000);
        assert_eq!(u.output_tokens, 120);
        assert_eq!(u.cache_creation_tokens, 0, "OpenAI 无写入口");
    }

    /// 无 cached 细节（网关不上报或未命中）时 input 原样透传
    #[test]
    fn openai_usage_without_cache_details_passes_input_through() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":800,"completion_tokens":30}}"#;
        let u = parse_sse_chunk(line).unwrap().usage.unwrap();
        assert_eq!((u.input_tokens, u.cache_read_tokens), (800, 0));
        assert_eq!(u.output_tokens, 30);
    }

    /// 脏网关给出 cached > prompt（口径反了/重复计入）时不得下溢
    #[test]
    fn openai_usage_saturates_when_cached_exceeds_prompt() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":9999}}}"#;
        let u = parse_sse_chunk(line).unwrap().usage.unwrap();
        assert_eq!(u.input_tokens, 0, "减法必须 saturating，不能绕成天文数字");
        assert_eq!(u.cache_read_tokens, 9999);
    }

    /// 普通内容块不带 usage → None（调用方据此只认真正带用量的块）
    #[test]
    fn openai_content_chunk_has_no_usage() {
        let line = r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#;
        assert!(parse_sse_chunk(line).unwrap().usage.is_none());
    }

    /// OpenAI 网关**每块都带 usage** 时，token 数不得按块数翻倍。
    ///
    /// usage 是本请求的**总量**（`stream_options.include_usage` 约定），逐块
    /// 累加会把同一份总量重复计入——落库 token、缓存命中率、前端水位条全部
    /// 虚高。合并口径：按字段取最后一次非零值。
    #[test]
    fn stream_usage_is_not_multiplied_when_every_chunk_carries_it() {
        let chunk =
            r#"data: {"choices":[],"usage":{"prompt_tokens":5000,"completion_tokens":120}}"#;
        let mut acc = TokenUsage::default();
        for _ in 0..5 {
            let u = parse_sse_chunk(chunk).unwrap().usage.unwrap();
            merge_stream_usage(&mut acc, u);
        }
        assert_eq!(acc.input_tokens, 5000, "不得累加成 25000");
        assert_eq!(acc.output_tokens, 120, "不得累加成 600");
    }

    /// Anthropic 的字段分散在两个事件里（input 在 message_start、output 在
    /// message_delta），按字段取最后一次非零值才两样都在——整块替换会把
    /// Anthropic 的 input 抹成 0。
    #[test]
    fn anthropic_usage_keeps_input_from_start_and_output_from_delta() {
        let start = crate::bot_anthropic::parse_anthropic_event(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":50,"cache_read_input_tokens":4000,"cache_creation_input_tokens":1500}}}"#,
        )
        .unwrap()
        .usage
        .unwrap();
        let delta = crate::bot_anthropic::parse_anthropic_event(
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#,
        )
        .unwrap()
        .usage
        .unwrap();
        let mut acc = TokenUsage::default();
        merge_stream_usage(&mut acc, start);
        merge_stream_usage(&mut acc, delta);
        assert_eq!(
            acc.input_tokens, 50,
            "message_start 的 input 不能被后到的 delta 抹掉"
        );
        assert_eq!(acc.output_tokens, 7);
        assert_eq!(acc.cache_read_tokens, 4000);
        assert_eq!(acc.cache_creation_tokens, 1500);
    }

    /// Anthropic 的 message_delta.output_tokens 是**累计**输出量：再来一个更大的
    /// delta 时应覆盖，不是相加（相加会把 5+7 算成 12）。
    #[test]
    fn anthropic_cumulative_output_replaces_rather_than_adds() {
        let mut acc = TokenUsage::default();
        for o in [5u64, 7] {
            let u = crate::bot_anthropic::parse_anthropic_event(&format!(
                r#"data: {{"type":"message_delta","usage":{{"output_tokens":{o}}}}}"#
            ))
            .unwrap()
            .usage
            .unwrap();
            merge_stream_usage(&mut acc, u);
        }
        assert_eq!(acc.output_tokens, 7, "累计值应覆盖，不得相加");
    }

    /// 非对象 usage（脏网关）不得当成有效用量
    #[test]
    fn openai_non_object_usage_is_ignored() {
        let line = r#"data: {"choices":[],"usage":"n/a"}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    /// 流内错误信封的优先级高于 usage（错误必须冒泡，不能被当末块静默吞掉）
    #[test]
    fn openai_error_envelope_beats_usage_chunk() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":5},"error":{"message":"rate limited"}}"#;
        let c = parse_sse_chunk(line).expect("错误信封不该被当成纯 usage 末块");
        assert_eq!(
            c.error.as_ref().map(|e| e.message.as_str()),
            Some("rate limited")
        );
    }

    /// OpenAI 形状的 usage 末块不得被算成「协议正常成分之外」的坏块
    #[test]
    fn openai_usage_chunk_is_not_counted_as_bad_sse() {
        use super::unparsable_sse_payload;
        use crate::bot::ApiProvider::Openai;
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":5}}"#;
        assert!(unparsable_sse_payload(line, Openai).is_none());
    }

    #[test]
    fn unparsable_payload_flags_only_bad_data_lines() {
        use super::unparsable_sse_payload;
        use crate::bot::ApiProvider::{Anthropic, Openai};
        // 坏块：data 行 + 非空 + 解析不出来 → 计入降级（两协议同一判）
        for p in [Openai, Anthropic] {
            assert_eq!(
                unparsable_sse_payload("data: not-json{", p),
                Some("not-json{")
            );
            assert_eq!(unparsable_sse_payload("data: {半截", p), Some("{半截"));
        }
        // 协议正常成分：一律不算坏块（否则正常流每次都记降级）
        for p in [Openai, Anthropic] {
            assert_eq!(unparsable_sse_payload("event: ping", p), None);
            assert_eq!(unparsable_sse_payload("", p), None);
            assert_eq!(unparsable_sse_payload(": keep-alive", p), None);
            assert_eq!(unparsable_sse_payload("data:", p), None);
            assert_eq!(unparsable_sse_payload("data:    ", p), None);
            assert_eq!(unparsable_sse_payload("data: [DONE]", p), None);
            assert_eq!(unparsable_sse_payload("data:    [DONE]", p), None);
            // 标准形状（含空心跳）
            assert_eq!(unparsable_sse_payload(r#"data: {"choices":[]}"#, p), None);
            assert_eq!(
                unparsable_sse_payload(r#"data: {"choices":[{"delta":{}}]}"#, p),
                None
            );
        }
        // 合法 Anthropic 事件没有 choices 键，不得被 OpenAI 那条形状臂误伤
        assert_eq!(
            unparsable_sse_payload(r#"data: {"type":"message_start","message":{}}"#, Anthropic),
            None
        );
        // OpenAI 形状缺失：错误信封 / 外来载荷照样丢内容，必须计入
        assert_eq!(
            unparsable_sse_payload(r#"data: {"code":401,"msg":"invalid api key"}"#, Openai),
            Some(r#"{"code":401,"msg":"invalid api key"}"#)
        );
        assert_eq!(
            unparsable_sse_payload(r#"data: {"id":"x","object":"unknown"}"#, Openai),
            Some(r#"{"id":"x","object":"unknown"}"#)
        );
    }

    #[test]
    fn stream_error_payload_surfaced() {
        // 200 流内错误载荷不得静默忽略（否则用户拿到空白回复）；
        // 必须显式冒出（主循环据此报错 + 审计）
        let line = r#"data: {"id":"x","error":{"message":"auth_failed","type":"auth_error"}}"#;
        let parsed = parse_sse_chunk(line).expect("error 载荷必须冒出，不得静默丢弃");
        let e = parsed.error.expect("error 载荷必须带出");
        assert_eq!(e.message, "auth_failed");
        // 上游标的 type 一并带出——可重试性由它决定，丢了就只剩一句文案
        assert_eq!(e.kind.as_deref(), Some("auth_error"));

        // error 为裸字符串的兼容形态
        let line2 = r#"data: {"error":"rate limited"}"#;
        let parsed2 = parse_sse_chunk(line2).unwrap();
        assert_eq!(
            parsed2.error.as_ref().map(|e| e.message.as_str()),
            Some("rate limited")
        );
    }

    #[test]
    fn returns_none_for_empty_choices_without_error() {
        // 空 choices 且无 error 字段 → 仍静默跳过（与原语义一致）
        let line = r#"data: {"id":"x","choices":[]}"#;
        assert!(parse_sse_chunk(line).is_none());
    }

    #[test]
    fn parses_reasoning_content() {
        // DeepSeek-reasoner 类模型的独立推理字段
        let line =
            r#"data: {"choices":[{"delta":{"reasoning_content":"先想一下","content":null}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.reasoning.as_deref(), Some("先想一下"));
        assert!(parsed.content.is_none());
    }

    #[test]
    fn parses_chinese_content_utf8() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好世界"}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.content.as_deref(), Some("你好世界"));
    }

    #[test]
    fn parses_arguments_split_across_chunks() {
        // 模拟 arguments 跨多个 SSE chunk（流式追加）
        let chunk1 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"ti"}}]}}]}"#;
        let chunk2 = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"tle\":\"x\""}}]}}]}"#;
        let p1 = parse_sse_chunk(chunk1).unwrap();
        let p2 = parse_sse_chunk(chunk2).unwrap();
        assert_eq!(p1.tool_calls[0].arguments_chunk.as_deref(), Some(r#"{"ti"#));
        assert_eq!(
            p2.tool_calls[0].arguments_chunk.as_deref(),
            Some(r#"tle":"x""#)
        );
        // 生产代码会按顺序 push 拼成完整 JSON
    }

    #[test]
    fn done_marker_data_is_correctly_parsed() {
        // data:    [DONE]（中间多空格）也应正确识别
        let parsed = parse_sse_chunk("data:    [DONE]").unwrap();
        assert!(parsed.is_done);
    }

    #[test]
    fn defaults_index_to_zero_when_missing() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[{"function":{"name":"a"}}]}}]}"#;
        let parsed = parse_sse_chunk(line).unwrap();
        assert_eq!(parsed.tool_calls[0].index, 0, "缺 index 时默认 0");
    }
}

#[cfg(test)]
mod stream_accumulate_tests {
    use super::*;

    /// 多字节 UTF-8 字符跨 chunk 切断时不得产生 U+FFFD 替换字符
    #[test]
    fn drain_sse_lines_no_replacement_char_across_chunks() {
        let text = "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}\n";
        let bytes = text.as_bytes();
        // 「你」是 3 字节字符，切在它的第 2 个字节后
        let pos = bytes
            .windows(3)
            .position(|w| w == "你".as_bytes())
            .expect("应含「你」");
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&bytes[..pos + 1]);
        assert!(
            drain_sse_lines(&mut buf).is_empty(),
            "行未完成时不应产出任何行"
        );
        buf.extend_from_slice(&bytes[pos + 1..]);
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines.len(), 1);
        assert!(
            !lines[0].contains('\u{FFFD}'),
            "多字节字符跨 chunk 不得产生替换字符：{:?}",
            lines[0]
        );
        assert!(lines[0].contains("你好"));
        assert!(buf.is_empty());
    }

    #[test]
    fn drain_sse_lines_keeps_incomplete_tail() {
        let mut buf = b"data: a\ndata: partial".to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines, vec!["data: a\n".to_string()]);
        assert_eq!(buf, b"data: partial".to_vec(), "残余不完整行留在 buf");
    }

    #[test]
    fn drain_sse_lines_empty_buf() {
        // 空 buf → 空 vec，buf 仍空
        let mut buf: Vec<u8> = Vec::new();
        let lines = drain_sse_lines(&mut buf);
        assert!(lines.is_empty(), "空 buf 应产出空 vec");
        assert!(buf.is_empty(), "buf 应保持空");
    }

    #[test]
    fn drain_sse_lines_no_newline() {
        // 无 \n → 空 vec，buf 不变（不完整行不走 drain）
        let mut buf = b"data: incomplete".to_vec();
        let original = buf.clone();
        let lines = drain_sse_lines(&mut buf);
        assert!(lines.is_empty(), "无 \\n 不应产出行");
        assert_eq!(buf, original, "buf 应保持原样");
    }

    #[test]
    fn drain_sse_lines_single_line() {
        // b"a\n" → ["a\n"]，buf 清空
        let mut buf = b"a\n".to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines, vec!["a\n".to_string()]);
        assert!(buf.is_empty(), "完整行后 buf 应清空");
    }

    #[test]
    fn drain_sse_lines_multiple_lines() {
        // b"a\nb\nc\n" → ["a\n", "b\n", "c\n"]，buf 清空
        let mut buf = b"a\nb\nc\n".to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(
            lines,
            vec!["a\n".to_string(), "b\n".to_string(), "c\n".to_string()]
        );
        assert!(buf.is_empty(), "多行完整后 buf 应清空");
    }

    #[test]
    fn drain_sse_lines_multibyte_utf8() {
        // 完整多字节 UTF-8 字符串中含 \n → 正确切分且无 U+FFFD
        let text = "你好\n世界\n";
        let mut buf = text.as_bytes().to_vec();
        let lines = drain_sse_lines(&mut buf);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("你好") && lines[0].ends_with('\n'));
        assert!(lines[1].contains("世界") && lines[1].ends_with('\n'));
        assert!(!lines.iter().any(|l| l.contains('\u{FFFD}')));
        assert!(buf.is_empty());
    }

    #[test]
    fn accumulate_merges_shards_by_index() {
        let mut calls: Vec<(String, String, String)> = Vec::new();
        // 两个并行 tool_call 交错到达；id 只在首帧；arguments 分片
        let deltas = [
            ToolCallDelta {
                index: 0,
                id: Some("call_a".into()),
                name_chunk: Some("list".into()),
                arguments_chunk: Some("{\"ti".into()),
            },
            ToolCallDelta {
                index: 1,
                id: Some("call_b".into()),
                name_chunk: Some("web".into()),
                arguments_chunk: Some("{\"q".into()),
            },
            ToolCallDelta {
                index: 0,
                id: None,
                name_chunk: Some("_tasks".into()),
                arguments_chunk: Some("tle\":\"x\"}".into()),
            },
            ToolCallDelta {
                index: 1,
                id: None,
                name_chunk: Some("_search".into()),
                arguments_chunk: Some("\":\"y\"}".into()),
            },
        ];
        for d in &deltas {
            assert!(accumulate_tool_call_delta(&mut calls, d));
        }
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            (
                "call_a".into(),
                "list_tasks".into(),
                "{\"title\":\"x\"}".into()
            )
        );
        assert_eq!(
            calls[1],
            ("call_b".into(), "web_search".into(), "{\"q\":\"y\"}".into())
        );
    }

    #[test]
    fn accumulate_late_id_fills_empty_slot() {
        let mut calls: Vec<(String, String, String)> = Vec::new();
        let d1 = ToolCallDelta {
            index: 0,
            id: None,
            name_chunk: Some("list_tasks".into()),
            arguments_chunk: None,
        };
        assert!(accumulate_tool_call_delta(&mut calls, &d1));
        let d2 = ToolCallDelta {
            index: 0,
            id: Some("call_late".into()),
            name_chunk: None,
            arguments_chunk: None,
        };
        assert!(accumulate_tool_call_delta(&mut calls, &d2));
        assert_eq!(calls[0].0, "call_late", "迟到的 id 应补上");
    }

    /// 畸形/恶意 index 不得撑爆内存
    #[test]
    fn accumulate_rejects_insane_index() {
        let mut calls: Vec<(String, String, String)> = Vec::new();
        let d = ToolCallDelta {
            index: MAX_TOOL_CALL_INDEX + 1,
            id: Some("evil".into()),
            name_chunk: Some("x".into()),
            arguments_chunk: None,
        };
        assert!(!accumulate_tool_call_delta(&mut calls, &d));
        assert!(calls.is_empty(), "超限 index 不得伸长列表");
        // 边界：恰好上限放行
        let ok = ToolCallDelta {
            index: MAX_TOOL_CALL_INDEX,
            id: None,
            name_chunk: Some("x".into()),
            arguments_chunk: None,
        };
        assert!(accumulate_tool_call_delta(&mut calls, &ok));
        assert_eq!(calls.len(), MAX_TOOL_CALL_INDEX + 1);
    }

    /// 重试状态白名单
    #[test]
    fn retryable_status_429_and_5xx_only() {
        assert!(is_retryable_llm_status(429));
        assert!(is_retryable_llm_status(500));
        assert!(is_retryable_llm_status(503));
        assert!(!is_retryable_llm_status(400));
        assert!(!is_retryable_llm_status(401));
        assert!(!is_retryable_llm_status(404));
        assert!(!is_retryable_llm_status(200));
    }
}

#[cfg(test)]
mod tools_schema_tests {
    use crate::bot::registry::tools_json as TOOLS;

    /// TOOLS 是编译期字符串、运行期解析：语法坏会 panic 杀死聊天（历史 bug）。
    /// 此测试守住：加/改工具后必须合法且字段完整。
    #[test]
    fn tools_schema_parses() {
        let v: serde_json::Value = serde_json::from_str(TOOLS()).expect("TOOLS 必须是合法 JSON");
        let arr = v.as_array().expect("TOOLS 顶层必须是数组");
        assert!(!arr.is_empty(), "TOOLS 不能为空");
        for t in arr {
            assert_eq!(
                t["type"].as_str(),
                Some("function"),
                "每项 type 必须是 function"
            );
            let name = t["function"]["name"]
                .as_str()
                .expect("每项必须有 function.name");
            assert!(!name.is_empty(), "工具名不能为空");
            assert!(
                t["function"]["description"].as_str().is_some(),
                "{name} 缺 description"
            );
        }
        // 关键工具必须存在（与 execute_tool match 对齐，改名会在此暴露）
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        for required in [
            "query_tasks",
            "create_task",
            "complete_task",
            "delete_task",
            "edit_task",
            "run_python",
            "web_search",
            "fetch_url",
        ] {
            assert!(names.contains(&required), "缺少工具 {required}");
        }
    }
}
