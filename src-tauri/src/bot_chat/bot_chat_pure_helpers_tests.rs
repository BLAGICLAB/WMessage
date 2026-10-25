use super::*;

#[test]
fn format_recovery_hint_includes_reason_summary_and_yes_no() {
    let hint = format_recovery_hint("list_tasks 超时", "- Step 1 (list_tasks): 0 个任务\n", true);
    assert!(
        hint.contains("【Skill 失败可恢复上下文】"),
        "应有上下文标记：\n{hint}"
    );
    assert!(hint.contains("原因：list_tasks 超时"), "应含原因：\n{hint}");
    assert!(
        hint.contains("- Step 1 (list_tasks): 0 个任务"),
        "应含已完成产物：\n{hint}"
    );
    assert!(
        hint.contains("回滚已尝试：是"),
        "rollback_attempted=true 应输出 是：\n{hint}"
    );
    assert!(
        hint.contains("重试 / 调整 / 告知用户"),
        "应含 LLM 决策提示：\n{hint}"
    );

    let hint_no = format_recovery_hint("x", "y", false);
    assert!(
        hint_no.contains("回滚已尝试：否"),
        "rollback_attempted=false 应输出 否：\n{hint_no}"
    );
}

#[test]
fn format_recovery_hint_handles_empty_reason_and_summary() {
    let hint = format_recovery_hint("", "", false);
    assert!(hint.contains("原因："), "空 reason 也应含 key：\n{hint}");
    assert!(
        hint.contains("已完成产物："),
        "空 summary 也应含 key：\n{hint}"
    );
}

#[test]
fn merge_task_refs_dedup_empty_input_returns_empty() {
    let out = merge_task_refs_dedup(vec![]);
    assert!(out.is_empty());
}

#[test]
fn chat_result_carries_trace_id_as_camel_case() {
    let r = BotChatResult {
        text: "答复".into(),
        task_refs: vec![],
        busy: false,
        trace_id: Some(42),
    };
    let json = serde_json::to_value(&r).expect("序列化");
    assert_eq!(json["traceId"], serde_json::json!(42));
    // 没开 trace 的回合也要有这个键（null）：前端按缺省处理，不靠键在不在
    let r2 = BotChatResult {
        text: "答复".into(),
        task_refs: vec![],
        busy: false,
        trace_id: None,
    };
    let json2 = serde_json::to_value(&r2).expect("序列化");
    assert!(json2["traceId"].is_null());
}

/// 接线守卫：回传的 trace id 必须与开 trace 时拿到的**同一个值**。
///
/// 匹配串用 `concat!` 拆分防自匹配（本测试模块自身的源码不算命中）。
#[test]
fn returned_trace_id_comes_from_the_one_begin_trace_created() {
    let chat = include_str!("flow.rs");
    let opened = chat
        .find(concat!(
            "let trace_id = crate::trace_sink::begin_trace(",
            "&trace_app, sid, None, \"chat\", None).await;"
        ))
        .expect("开 trace 的调用点");
    let kept = chat
        .find("chat_trace_id = trace_id;")
        .expect("开 trace 时要把 id 留在局部变量里");
    let returned = chat
        .find("trace_id: chat_trace_id,")
        .expect("结果里必须带回这个 trace id");
    assert!(
        opened < kept && kept < returned,
        "trace id 的取值链必须是 开 trace → 存局部 → 进结果（现 opened={opened} \
         kept={kept} returned={returned}）"
    );
}

/// 接线守卫：汇报轮只在**落库成功**之后记「已吸收」。
///
/// 汇报轮没有前端快照，它的历史下一轮从库里读——回复没落库就等于没说出
/// 出去，用户那边看不到，模型下一轮也读不到这段汇报。落库失败还记着
/// 「已吸收」，重试时那段回报就不在 digest 里，模型无话可说，子任务结果
/// 在本次进程内永久消失。匹配串用 `concat!` 拆分防自匹配。
#[test]
fn report_turn_marks_absorbed_only_after_the_reply_is_persisted() {
    let chat = include_str!("report.rs");
    let start = chat
        .find("pub(crate) async fn report_subagent_result(")
        .expect("汇报轮入口");
    let body = &chat[start..];
    let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
    let fn_body = &body[..end];
    let persist = fn_body
        .find(concat!(
            "persist_report_reply(app, session_id, ",
            "&out.result)"
        ))
        .expect("汇报轮必须先落库");
    let mark = fn_body
        .find(concat!("mark_digested(Some(session_id), ", "id)"))
        .expect("落库成功后要记已吸收");
    assert!(
        persist < mark,
        "「已吸收」必须记在落库之后（现 persist={persist} mark={mark}）"
    );
    assert!(
        fn_body[persist..mark].contains("Ok(()) =>"),
        "标记必须落在落库成功的分支里——Err 分支是「没送达」，不能记"
    );
}

/// 接线守卫：任务执行入口读库失败必须上抛，不得吞成「任务卡不存在」。
///
/// 吞掉的后果是把 DB 故障说成一句根本不成立的结论（卡不存在 / 已在回收站），
/// 真正的故障原因被埋掉，排查只能靠猜。匹配串用 `concat!` 拆分防自匹配。
#[test]
fn task_execution_does_not_swallow_db_load_failures() {
    let src = format!(
        "{}{}",
        include_str!("../bot_chat.rs"),
        include_str!("exec.rs")
    );
    for anchor in [
        // 逐步执行分流那一处
        concat!("if let Some(task) = crate::db::db_load(app.", "clone())"),
        // 任务校验那一处
        concat!("let task = crate::db::db_load_for(", "app)"),
    ] {
        let at = src
            .find(anchor)
            .unwrap_or_else(|| panic!("找不到锚点，挪动代码后请同步这条守卫"));
        // 按字符切窗（按字节切会切在多字节汉字中间）
        let window: String = src[at..].chars().take(160).collect();
        assert!(
            !window.contains(concat!("unwrap_or_", "default")),
            "任务执行入口读库失败必须上抛，不得兜成空列表——\
             DB 故障会被伪装成「任务卡不存在或已在回收站」"
        );
    }
}

#[test]
fn busy_result_is_marked_busy_so_frontend_can_tell_it_from_a_model_reply() {
    // 拒答结果必须带 busy 标记：前端靠它认出「本轮压根没跑」，
    // 把拒答文案当本地提示而不是一条落库的 assistant 答复
    let r = busy_chat_result();
    assert!(r.busy, "会话锁被占的结果必须标 busy");
    assert_eq!(r.text, CHAT_BUSY_REPLY);
    assert!(r.task_refs.is_empty(), "拒答不挂任务引用");
    // 序列化口径：camelCase，busy 落在顶层
    let json = serde_json::to_value(&r).expect("序列化");
    assert_eq!(json["busy"], serde_json::json!(true));
    assert_eq!(json["taskRefs"], serde_json::json!([]));
}

#[test]
fn merge_task_refs_dedup_no_duplicates_returns_all_in_order() {
    let refs = vec![
        TaskRef {
            id: "a".into(),
            title: "标题 A".into(),
        },
        TaskRef {
            id: "b".into(),
            title: "标题 B".into(),
        },
        TaskRef {
            id: "c".into(),
            title: "标题 C".into(),
        },
    ];
    let out = merge_task_refs_dedup(refs);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].id, "a");
    assert_eq!(out[1].id, "b");
    assert_eq!(out[2].id, "c");
}

#[test]
fn merge_task_refs_dedup_duplicates_keeps_first_occurrence() {
    let refs = vec![
        TaskRef {
            id: "a".into(),
            title: "首次标题 A".into(),
        },
        TaskRef {
            id: "b".into(),
            title: "首次 B".into(),
        },
        TaskRef {
            id: "a".into(),
            title: "后续标题 A（应被丢弃）".into(),
        },
        TaskRef {
            id: "b".into(),
            title: "后续 B（应被丢弃）".into(),
        },
    ];
    let out = merge_task_refs_dedup(refs);
    assert_eq!(out.len(), 2, "去重后应剩 2 条");
    assert_eq!(
        out[0].title, "首次标题 A",
        "首次出现应保留原标题，不能用后续覆盖"
    );
    assert_eq!(out[1].title, "首次 B");
}

#[test]
fn merge_task_refs_dedup_consecutive_same_ids_collapses() {
    let refs = vec![
        TaskRef {
            id: "x".into(),
            title: "X1".into(),
        },
        TaskRef {
            id: "x".into(),
            title: "X2".into(),
        },
        TaskRef {
            id: "x".into(),
            title: "X3".into(),
        },
    ];
    let out = merge_task_refs_dedup(refs);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].title, "X1");
}

// ── L0 pre_ingest：输入规范化 ──

#[test]
fn ingest_strips_invisible_junk_but_keeps_text_untouched() {
    // 从 Word/网页复制的典型串：BOM + word-joiner + soft-hyphen 混在词中间，
    // 肉眼完全看不出，关键词匹配却整个失效
    let raw = "\u{FEFF}做\u{2060}一\u{00AD}份 PPT";
    let out = normalize_user_input_core(raw);
    assert_eq!(out.text, "做一份 PPT");
    assert!(out.changed);
    assert_eq!(out.stripped, 3, "三个不可见字符都要计数（审计靠它排障）");
}

#[test]
fn ingest_keeps_zwj_emoji_sequences_intact() {
    // 本条是「不可见字符当垃圾清」最容易踩的坑：U+200D 把三个 emoji
    // 拼成**一个**合成字形（家庭）。当噪音删掉，用户看到的家庭会变成
    // 三个单独的人——这是程序在改坏用户的话，不是清洗。
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    let out = normalize_user_input_core(&format!("我家是 {family}"));
    assert!(
        out.text.contains(family),
        "ZWJ 序列必须原样保留：{:?}",
        out.text
    );
    assert!(!out.changed, "含 ZWJ 的正常输入不应被判定为「已改写」");
    // ZWNJ 同理：波斯语/印地语里它表示字母不成形
    let persian = "\u{0641}\u{200C}\u{0645}";
    assert!(
        normalize_user_input_core(persian).text.contains('\u{200C}'),
        "ZWNJ 有语言语义，不得清"
    );
}

#[test]
fn ingest_maps_zero_width_space_to_real_space_not_away() {
    // 零宽空格在中文排版里就是个空格。删掉会把「做\u{200B}PPT」粘成
    // 「做PPT」；换回真空格才是「做 PPT」，匹配得上。
    let out = normalize_user_input_core("做\u{200B}PPT");
    assert_eq!(out.text, "做 PPT");
}

#[test]
fn ingest_unifies_crlf_without_counting_cr_as_stripped() {
    // 顺序反了（先逐字符过滤再统一行尾）会把 \r 当垃圾删掉：结果对，
    // 但审计会记「清掉 1 个不可见字符」，把排障引到错误方向。
    let out = normalize_user_input_core("第一行\r\n第二行\r\n");
    assert_eq!(out.text, "第一行\n第二行");
    assert_eq!(out.stripped, 0, "CRLF 的 \\r 不该计入不可见字符");
}

#[test]
fn ingest_does_not_touch_code_blocks_or_blank_line_runs() {
    // 刻意不做结构级改写：代码缩进与段落分隔都是 Markdown 语义。
    let src = "```rust\nfn main() {\n\n\n    println!();\n}\n```";
    let out = normalize_user_input_core(src);
    assert_eq!(out.text, src, "代码块内容不得被折叠或改写");
    assert!(!out.changed);
    assert!(
        normalize_user_input_core("A\n\n\n\nB")
            .text
            .contains("\n\n\n\n"),
        "连续空行不做折叠（排版意图与复制事故分不开，阈值判断留给别的轮次）"
    );
}

#[test]
fn ingest_leaves_clean_input_untouched() {
    let out = normalize_user_input_core("帮我做一份 PPT");
    assert_eq!(out.text, "帮我做一份 PPT");
    assert!(!out.changed, "干净输入不该被判改写（否则日志全是噪音）");
}

#[test]
fn ingest_rewrites_only_the_last_user_message() {
    let mut msgs = vec![
        msg("user", "旧问题"),
        msg("assistant", "旧回答"),
        msg("user", "\u{FEFF}新问题\u{200B}来了"),
    ];
    let n = normalize_user_input(&mut msgs).expect("有改动才返回");
    assert_eq!(n.text, "新问题 来了");
    assert_eq!(msgs[0].content, "旧问题", "历史不得被就地改写");
    assert_eq!(msgs[1].content, "旧回答");
    assert_eq!(msgs[2].content, "新问题 来了", "本轮必须改到位");
}

#[test]
fn ingest_skips_non_user_last_message_and_reports_none_when_unchanged() {
    // 只有 user 角色的最后一条是「本轮输入」；否则规范化它等于改写历史
    let mut assistant_last = vec![msg("assistant", "回答\u{FEFF}")];
    assert!(
        normalize_user_input(&mut assistant_last).is_none(),
        "最后一条不是用户消息时不得改写"
    );
    assert_eq!(assistant_last[0].content, "回答\u{FEFF}", "原样保留");

    let mut clean = vec![msg("user", "普通输入")];
    assert!(normalize_user_input(&mut clean).is_none());
}

/// 接线守卫：L0 必须在**所有**输入判定之前，且就地改 `messages`。
///
/// 挪到任何一处判定之后，就出现「有的扫描读清洗后、有的读原文」——
/// 规范化做了一半，症状是「清洗了但路由还是不匹配」。锚点用 concat! 拆分
/// 防自匹配。
#[test]
fn ingest_runs_before_every_input_decision_and_mutates_in_place() {
    // 限定在 bot_chat_inner 函数体内搜：本文件前部还有好几个同名符号的定义与
    // 文档提及，全文件搜第一个会命中它们，断言就变成在比谁先出现。
    let whole = include_str!("flow.rs");
    let start = whole
        .find("async fn bot_chat_inner(")
        .expect("bot_chat_inner 函数体起点");
    let body = &whole[start..];
    let l0 = body
        .find("normalize_user_input(&mut messages)")
        .expect("bot_chat 必须调 L0 规范化");
    // 五个输入侧判定各自扫 messages.last()，L0 必须排在它们之前
    for (label, needle) in [
        ("exec_steps 挂起恢复", "exec_steps::has_pending_for"),
        ("纠正信号检测", "corrections::detect_correction"),
        ("批量执行触发", "intent_router::is_chat_execute_trigger"),
        ("意图路由", "intent_router::route_user_input"),
        ("技能推荐", "build_skill_block_for"),
    ] {
        let at = body
            .find(needle)
            .unwrap_or_else(|| panic!("bot_chat 内找不到输入侧判定点：{label}"));
        assert!(
            l0 < at,
            "L0 规范化必须排在「{label}」之前（现 l0={l0} {label}={at}）"
        );
    }
    assert!(
        body[..l0].contains("let mut messages"),
        "入参必须先转成 mut 才能就地改写"
    );
}

/// 接线守卫：汇报轮的门控只有一个入口。
///
/// 汇报轮带着库里的历史走主链路，历史非空就意味着「messages.last() 就是本轮
/// 用户输入」这条老口径失效。任何在 `user_input` 之后新写的 `messages.last()`
/// 读法，都会让汇报轮把用户的旧消息当成本轮输入——挂起恢复、技能路由、计划生成
/// 全都会被误触发，而且这类误触发只在「子任务收尾后自动汇报」这条路上出现，
/// 手动聊天测不出来。故在这里钉死：门控点之后不允许再有裸的 messages.last()。
#[test]
fn report_turn_gate_is_the_only_input_read() {
    let whole = include_str!("flow.rs");
    let start = whole
        .find("async fn bot_chat_inner(")
        .expect("bot_chat_inner 函数体起点");
    let body = &whole[start..];
    let gate = body
        .find("let user_input: Option<ChatMsg>")
        .expect("必须有一个统一的用户输入门控点");
    // 从门控表达式之后开始扫：门控自己就要读 messages.last() 来区分两个分支。
    // match 的两个分支都不带花括号，紧随其后的 `};` 就是它的收尾。
    let after_gate = gate + body[gate..].find("};").expect("门控表达式收尾") + 2;
    let tail = &body[after_gate..];
    // 只看门控之后到函数收尾之间**真正的代码**：\n}\n 命中的是顶格右花括号，
    // 函数体内的嵌套块都是缩进的，所以这是「函数结束」的无歧义记号；
    // 注释行整行剔除，否则注释里提一嘴 messages.last() 就会误报。
    let code: String = tail[..tail.find("\n}\n").expect("bot_chat_inner 函数体收尾")]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !code.contains("messages.last()"),
        "user_input 之后又出现裸的 messages.last()：这条读法绕过汇报轮门控，\
         会把用户的旧消息当成本轮输入"
    );
    // 汇报轮的另一处显式门控：图片不回看，避免每轮把历史里的图重新 base64 发一遍
    assert!(
        code.contains("TurnOrigin::Report => Vec::new()"),
        "汇报轮不该回看历史图片"
    );
}

/// 汇报轮历史映射：库里有什么就带什么，一条不留。
///
/// 汇报轮带空历史的后果是 Anthropic 转换阶段硬失败（system 抽完就没有消息可发），
/// 这条用例把「库里几条就带几条」钉住，防止映射时多丢一条——丢了症状同样是转述
/// 内容对不上，但更难从日志看出来。
#[test]
fn report_turn_history_passes_rows_through_in_order() {
    let rows = vec![
        crate::db::BotMsgRow {
            role: "user".into(),
            content: "派子 agent 查一下".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
        crate::db::BotMsgRow {
            role: "assistant".into(),
            content: "已派发".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
    ];
    let out = report_turn_history_from_rows(rows);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].role, "user");
    assert_eq!(out[0].content, "派子 agent 查一下");
    assert_eq!(out[1].role, "assistant");
    assert_eq!(out[1].content, "已派发");
}

/// 历史为空时补的占位必须是 user 角色的一条真消息。
///
/// 角色写错（system / assistant）时 `openai_msgs_to_anthropic` 照样能把 system
/// 抽走，messages 依旧归零，失败现场和没补时一模一样——所以这条断言盯的是角色。
#[test]
fn report_turn_history_falls_back_to_a_user_turn_when_db_is_empty() {
    let out = report_turn_history_from_rows(vec![]);
    assert_eq!(
        out.len(),
        1,
        "空历史也必须有一条 user 消息，否则 Anthropic 转换直接失败"
    );
    assert_eq!(out[0].role, "user");
    assert_eq!(out[0].content, REPORT_TURN_STUB);
}

// ── 历史预算 / 图片窗口 / think 剥除 ──

fn msg(role: &str, content: &str) -> ChatMsg {
    ChatMsg {
        role: role.into(),
        content: content.into(),
    }
}

/// 不调摘要器的截断内核 = 纯截断语义（mock summarizer 返 Err）
async fn truncate_only(messages: Vec<ChatMsg>, budget: usize) -> (Vec<ChatMsg>, usize) {
    let (kept, _summary, dropped) = truncate_with_summary_core(messages, budget, |_m| async {
        Err(CommandError::DomainRule {
            domain: "test".to_string(),
            reason: "本测试不摘要".to_string(),
        })
    })
    .await;
    (kept, dropped)
}

#[test]
fn truncate_chat_history_within_budget_unchanged() {
    let msgs = vec![
        msg("user", "你好"),
        msg("assistant", "在的"),
        msg("user", "列任务"),
    ];
    let (kept, dropped) = futures::executor::block_on(truncate_only(msgs, 100));
    assert_eq!(dropped, 0);
    assert_eq!(kept.len(), 3);
}

#[test]
fn truncate_chat_history_drops_oldest_first() {
    let long = "x".repeat(60);
    let msgs = vec![
        msg("user", &long),
        msg("assistant", &long),
        msg("user", &long),
    ];
    let (kept, dropped) = futures::executor::block_on(truncate_only(msgs, 100));
    assert_eq!(dropped, 2, "超预算时最旧的先丢，dropped 数应正确");
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].role, "user");
}

#[test]
fn truncate_chat_history_keeps_last_even_over_budget() {
    let msgs = vec![msg("user", &"x".repeat(200))];
    let (kept, dropped) = futures::executor::block_on(truncate_only(msgs, 10));
    assert_eq!(dropped, 0, "最后一条（本轮用户消息）永远保留");
    assert_eq!(kept.len(), 1);
}

#[test]
fn image_attach_indices_hits_user_in_last_three() {
    let msgs = vec![
        msg("user", "a"),
        msg("assistant", "b"),
        msg("user", "c"),
        msg("user", "d"),
    ];
    assert_eq!(image_attach_indices(&msgs), vec![2, 3]);
}

#[test]
fn image_attach_indices_skips_older_user() {
    let msgs = vec![
        msg("user", "a"),
        msg("assistant", "b"),
        msg("assistant", "c"),
        msg("user", "d"),
    ];
    assert_eq!(image_attach_indices(&msgs), vec![3], "更早的 user 不命中");
}

#[test]
fn image_attach_indices_no_user_is_empty() {
    let msgs = vec![msg("assistant", "a"), msg("assistant", "b")];
    assert!(image_attach_indices(&msgs).is_empty());
}

#[test]
fn strip_think_blocks_complete_section_removed() {
    assert_eq!(strip_think_blocks("前<think>想很多</think>后"), "前后");
}

#[test]
fn strip_think_blocks_multiple_sections() {
    assert_eq!(
        strip_think_blocks("a<think>x</think>b<think>y</think>c"),
        "abc"
    );
}

#[test]
fn strip_think_blocks_unclosed_tail_dropped() {
    assert_eq!(strip_think_blocks("保留<think>没闭合的尾巴"), "保留");
}

#[test]
fn strip_think_blocks_no_tag_unchanged() {
    assert_eq!(strip_think_blocks("普通文本"), "普通文本");
    assert_eq!(strip_think_blocks(""), "");
}

// ── 截断即摘要（编排内核，摘要器注入） ──

#[tokio::test]
async fn truncate_with_summary_core_success_returns_summary() {
    // budget=63：本轮消息（4 字）+ 一条 60 字旧消息=64 超预算 → 最旧两条被丢
    let long = "x".repeat(60);
    let msgs = vec![
        msg("user", &long),
        msg("assistant", &long),
        msg("user", "本轮问题"),
    ];
    let (kept, summary, dropped) =
        truncate_with_summary_core(msgs, 63, |dropped_msgs| async move {
            assert_eq!(dropped_msgs.len(), 2, "摘要器应收到的恰是将被丢弃的消息");
            Ok("用户在做记忆模块".to_string())
        })
        .await;
    assert_eq!(dropped, 2);
    assert_eq!(summary.as_deref(), Some("用户在做记忆模块"));
    assert_eq!(kept.len(), 1, "最旧两条被丢，只留本轮消息");
    assert_eq!(kept[0].content, "本轮问题");
}

#[tokio::test]
async fn truncate_with_summary_core_llm_failure_falls_back_to_plain_drop() {
    let long = "x".repeat(60);
    let msgs = vec![
        msg("user", &long),
        msg("assistant", &long),
        msg("user", "本轮问题"),
    ];
    let (kept, summary, dropped) = truncate_with_summary_core(msgs, 63, |_| async {
        Err(CommandError::Internal("boom".into()))
    })
    .await;
    assert_eq!(dropped, 2, "失败也要按原行为丢弃最旧消息");
    assert_eq!(summary, None, "失败 → 无摘要（调用方静默退回）");
    assert_eq!(kept.len(), 1);
}

#[tokio::test]
async fn truncate_with_summary_core_empty_summary_treated_as_failure() {
    let long = "x".repeat(60);
    let msgs = vec![
        msg("user", &long),
        msg("assistant", &long),
        msg("user", "本轮问题"),
    ];
    let (_, summary, dropped) =
        truncate_with_summary_core(msgs, 63, |_| async { Ok("   ".to_string()) }).await;
    assert_eq!(summary, None, "空白摘要按失败处理");
    assert_eq!(dropped, 2);
}

#[tokio::test]
async fn truncate_with_summary_core_within_budget_never_calls_llm() {
    let msgs = vec![msg("user", "你好"), msg("assistant", "在的")];
    let (kept, summary, dropped) = truncate_with_summary_core(msgs, 100, |_| async {
        panic!("预算内不应触发摘要调用")
    })
    .await;
    assert_eq!((kept.len(), summary, dropped), (2, None, 0));
}
