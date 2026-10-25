//! 终稿自检 / 工具自检用例：自检判定、侧调、熔断续写。

use super::*;

// ────────────────────────────────────────────────────────────────────
// D-Y/N selfcheck 扩到 final_text 层（Round 7 final-selfcheck 节流闸）：
// 主出口前置触发 LLM 自检，Y 路径静默还原 final_text；N 路径追加 ⚠️ +
// emit + audit_log。节流：final_text 字数 ≥ 80 + function_calls ≥ 3 +
// 距上次 ≥ gap 轮。gap=0 = 关闭。Y/N 字样走 expecting_final_selfcheck
// 视觉过滤（与 SELFCHECK 同套 expecting_selfcheck 流式跳 emit 套路）。
// ────────────────────────────────────────────────────────────────────

/// 长文 + 3 function_calls + Y 响应：trigger 触发后还原 final_text（Y 路径），
/// 原文本保留，Y 字样因视觉过滤不展示给用户。
#[tokio::test]
async fn core_final_selfcheck_y_restores_final_text() {
    let server = MockLlmServer::start();
    // 3 个 tool_calls 让 function_calls_total 凑够 3
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 4 轮模型给长文 final_text（≥ 80 字触发 final 自检 trigger）
    server.push_behavior(MockBehavior::TextReply(
        "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。任务列表展示在文档中，每一条都核对过状态与期限，没有遗漏任何待办事项，可以放心进入下一阶段的安排，若有需要我可以继续细化每个条目。"
            .into(),
    ));
    // 第 5 轮 final 自检响应：模型判 Y
    server.push_behavior(MockBehavior::TextReply("Y".into()));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(1)), // gap=1：第 4 轮首次满足即触发
        exec,
        noop_replan,
    )
    .await
    .expect("Y 路径应 Ok");
    // Y 路径：final_text 还原到原文（Y 字样被丢弃，无 ⚠️ 追加）
    assert!(
        text.contains("已完成 3 项工作"),
        "Y 路径应保留原文：{text:?}"
    );
    assert!(
        !text.contains("⚠️ 自检未达预期"),
        "Y 路径不应追加警告：{text:?}"
    );
    // 5 次 LLM 请求（3 tool + 1 final text + 1 selfcheck response）
    assert_eq!(server.request_count(), 5);
}

/// 长文 + 3 function_calls + N 响应：trigger 触发后追加 ⚠️ + 写 audit_log。
#[tokio::test]
async fn core_final_selfcheck_n_appends_warning_and_audits() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    let long_text = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。所有条目都逐条核对过状态、期限与负责人，异常项已单独标注，完整清单以文档形式存档，随时可以复查或导出。";
    server.push_behavior(MockBehavior::TextReply(long_text.into()));
    // 第 5 轮 final 自检响应：模型判 N
    server.push_behavior(MockBehavior::TextReply("N".into()));

    let h = CoreHarness::new();
    // 收集结构化审计行（event + kv 拼一行；h.audit 默认只记事件名，自己包捕获版）
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 直接用 deps() 但覆盖 audit 闭包捕获 final_selfcheck.fail 事件
    let mut deps = h.deps_with_gap(Some(1));
    deps.audit = {
        let logs2 = logs.clone();
        Box::leak(Box::new(
            move |_lvl: AuditLevel, event: &'static str, kv: Vec<(&'static str, String)>| {
                let kvs = kv
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                logs2.lock().unwrap().push(format!("{event} {kvs}"));
            },
        ))
    };
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("N 路径应 Ok");
    // N 路径：原文保留 + ⚠️ 追加
    assert!(text.contains(long_text), "N 路径应保留原文：{text:?}");
    assert!(
        text.contains("⚠️ 自检未达预期"),
        "N 路径应追加警告：{text:?}"
    );
    // audit_log 应记 final_selfcheck.fail
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("final_selfcheck.fail")),
        "应记 final_selfcheck.fail 审计：{lines:?}"
    );
}

/// 自检响应附解释（"N，……"非纯 Y/N）：宽松前缀判定应识别出 N——原文 + ⚠️ 追加。
/// 修复前走严格整段匹配得到 None → 当通过处理，幻觉自白被静默丢弃。
#[tokio::test]
async fn core_final_selfcheck_explained_n_appends_warning() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    let long_text = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。所有条目都逐条核对过状态、期限与负责人，异常项已单独标注，完整清单以文档形式存档，随时可以复查或导出。";
    server.push_behavior(MockBehavior::TextReply(long_text.into()));
    // 第 5 轮：模型答「N + 解释」——幻觉自白不能因为不是纯 N 就被丢弃
    server.push_behavior(MockBehavior::TextReply(
        "N，报告中把完成日期写错了，实际是下周".into(),
    ));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(1)),
        exec,
        noop_replan,
    )
    .await
    .expect("附解释 N 路径应 Ok");
    assert!(text.contains(long_text), "原文应保留：{text:?}");
    assert!(
        text.contains("⚠️ 自检未达预期"),
        "附解释的 N 也应追加警告，实际：{text:?}"
    );
}

/// 终稿自检响应轮带工具调用（以行动代回答）：应释放快照与流抑制，
/// 最终返回模型新的正常回答——而不是把旧快照原样顶回去（修复前行为）。
#[tokio::test]
async fn core_final_selfcheck_tool_reply_releases_snapshot() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 4 轮长文触发 final 自检（gap 默认 8，首次满足即触发）
    server.push_behavior(MockBehavior::TextReply(
        "已完成全部工作：本轮列出并逐条核对了所有任务条目，统计了完成与遗留情况，生成了汇总报告并写入文档，任务列表与统计结果均已在会话中展示完毕，没有遗漏任何步骤与细节，可以放心查阅。"
            .into(),
    ));
    // 第 5 轮：模型不答 Y/N 而是继续调工具核验（以行动代回答）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 6 轮：核验后的新最终回答——不应被第 4 轮快照顶掉
    // （gap=8：第 6 轮距上次触发仅 2 轮，不会二次触发；字数 <80 双保险）
    server.push_behavior(MockBehavior::TextReply(
        "复核完成：所有条目与报告一致，任务全部就绪。".into(),
    ));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(None),
        exec,
        noop_replan,
    )
    .await
    .expect("以行动代回答路径应 Ok");
    assert!(
        text.contains("复核完成"),
        "应返回核验后的新回答，实际（旧快照被顶回）：{text:?}"
    );
    // 6 次 LLM 请求：3 tool + 长文 + 行动代答 + 新回答
    assert_eq!(server.request_count(), 6);
}

/// gap=0（关闭）：长文 + 3 function_calls 也不触发 final 自检，无额外 LLM 请求。
#[tokio::test]
async fn core_final_selfcheck_disabled_with_gap_zero() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    let long_text = "已完成 3 项工作：列出了所有任务、统计了完成情况、生成了汇总报告。";
    server.push_behavior(MockBehavior::TextReply(long_text.into()));
    // **不**推第 5 个行为：若 trigger 触发会因无响应挂死或 panic

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(0)), // 关闭
        exec,
        noop_replan,
    )
    .await
    .expect("关闭路径应 Ok");
    // 关闭：原文保留，无 ⚠️
    assert_eq!(text, long_text);
    // 4 次 LLM 请求（3 tool + 1 final text），无第 5 次自检
    assert_eq!(server.request_count(), 4);
}

/// final_text < 80 字 + 3 function_calls + gap=1：不触发（trigger 硬条件字数不过）。
#[tokio::test]
async fn core_final_selfcheck_skips_short_final_text() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 短 final_text（< 80 字，触发硬条件不过）
    server.push_behavior(MockBehavior::TextReply("完成了".into()));
    // **不**推第 5 个行为：若 trigger 触发会因无响应挂死或 panic

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_gap(Some(1)), // gap=1，但字数不过也不会触发
        exec,
        noop_replan,
    )
    .await
    .expect("短文应 Ok");
    assert_eq!(text, "完成了");
    assert_eq!(server.request_count(), 4);
}

/// create_task 成功触发工具自检，模型答 N：应发出 ⚠️ 视觉提示、注入换策略
/// 提示并续跑一轮拿到恢复汇报——而不是以 "N" 终结回合。
#[tokio::test]
async fn core_tool_selfcheck_n_switches_strategy_and_continues() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    // 第 2 轮：模型对自检答 N（纯 Y/N 不是给用户的答案）
    server.push_behavior(MockBehavior::TextReply("N".into()));
    // 第 3 轮：收到换策略提示后的恢复汇报
    server.push_behavior(MockBehavior::TextReply(
        "已换用正确参数重新创建任务成功".into(),
    ));

    let h = CoreHarness::new();
    let (deps, deltas) = deps_capturing_chat_deltas(&h);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("N 换策略路径应 Ok");
    assert_eq!(
        text.trim(),
        "已换用正确参数重新创建任务成功",
        "N 后应续跑并返回恢复汇报，实际：{text:?}"
    );
    // 3 次 LLM 请求：tool → 自检回复 N → 换策略后的恢复汇报
    assert_eq!(server.request_count(), 3, "N 应触发续跑而非终结回合");
    // 视觉：⚠️ 提示发给用户；N 字样本身不外露
    let joined = deltas.lock().unwrap().join("");
    assert!(
        joined.contains("⚠️ 自检未达预期"),
        "应发出 ⚠️ 视觉提示：{joined:?}"
    );
}

/// create_task 成功触发工具自检，模型答 Y：应续跑一轮拿到正常汇报，
/// 而不是以 "Y"（或孤零零的 ✅）终结回合把汇报顶掉。
#[tokio::test]
async fn core_tool_selfcheck_y_continues_to_report() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply("Y".into()));
    server.push_behavior(MockBehavior::TextReply(
        "任务已创建，可在任务页查看详情".into(),
    ));

    let h = CoreHarness::new();
    let (deps, deltas) = deps_capturing_chat_deltas(&h);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("Y 续跑路径应 Ok");
    assert_eq!(
        text.trim(),
        "任务已创建，可在任务页查看详情",
        "Y 后应续跑返回正常汇报，实际：{text:?}"
    );
    assert_eq!(server.request_count(), 3, "Y 应触发续跑而非终结回合");
    let joined = deltas.lock().unwrap().join("");
    assert!(
        joined.contains("✅ 自检通过"),
        "应发出 ✅ 视觉提示：{joined:?}"
    );
}

/// 自检回复附解释（非纯 Y/N）：当正常答案走主出口终结回合，不额外注入提示。
#[tokio::test]
async fn core_tool_selfcheck_explanation_treated_as_final_answer() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    let explained = "Y，文件已写入项目根目录，无需调整";
    server.push_behavior(MockBehavior::TextReply(explained.into()));
    // **不**推第 3 个行为：若续跑被误触发会因无响应挂死/panic

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("附解释路径应 Ok");
    assert_eq!(text, explained, "附解释应作为正常答案返回");
    assert_eq!(server.request_count(), 2, "附解释不续跑");
}

/// 迟到的自检判定不能当终稿。
///
/// 实景（真机 2026-10-23）：模型用行动代替了回答（自检态随「以行动代回答」
/// 释放），那句「请仅用一个字回答 Y/N」还留在上下文里，模型在后面某轮补了
/// 一个 Y——那个 Y 落到出口就成了终稿：用户只看到一个 Y，真正的报告压根没
/// 产出（模型正要写报告就被这一轮终结了）。
///
/// 出口闸：纯 Y/N 单字 + 本轮问过自检 → 续跑一轮，让模型说出真实结果。
#[tokio::test]
async fn core_late_selfcheck_verdict_is_not_returned_as_answer() {
    let server = MockLlmServer::start();
    // 第 1 轮：变更类工具成功 → 触发工具自检（那句 Y/N 问话进上下文）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    // 第 2 轮：模型用行动代替回答（继续调工具，不答 Y/N）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 3 轮：模型这才回答自检问话——一个裸 Y
    server.push_behavior(MockBehavior::TextReply("Y".into()));
    // 第 4 轮：被追问后说出真实结果
    server.push_behavior(MockBehavior::TextReply(
        "已创建任务并核验完成，可在任务页查看".into(),
    ));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("迟到自检判定路径应 Ok");
    assert!(
        text.contains("已创建任务并核验完成"),
        "裸 Y 不是答复，应被追问并拿到真实结果，实际：{text:?}"
    );
    assert!(
        text.trim() != "Y" && text.trim() != "y",
        "终稿不能是自检判定字样，实际：{text:?}"
    );
    // 4 次 LLM 请求：tool → 行动代答 → 迟到判定 → 追问后的真实结果
    assert_eq!(
        server.request_count(),
        4,
        "迟到判定应触发追问续跑，而不是拿 Y 终结回合"
    );
}

/// 出口闸的作用域：没问过自检的回合里，一个裸 Y 就是合法答复（用户明说
/// 「只回答 Y 或 N」）。闸不能把这种回答也劫走。
#[tokio::test]
async fn core_bare_yn_without_selfcheck_is_kept_as_answer() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply("Y".into()));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("无自检的裸 Y 路径应 Ok");
    assert_eq!(text.trim(), "Y", "没问过自检时，裸 Y 是合法答复");
    assert_eq!(server.request_count(), 1, "不该追问");
}

/// 模型只肯回判定字样时不能无限追着问：追问额度封顶后给一句可读的说明，
/// 而不是把裸 Y 交给用户（那等于没回答）。
#[tokio::test]
async fn core_late_selfcheck_verdict_retry_is_bounded() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_task".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 之后全是裸 Y：问够了也不给结果
    for _ in 0..3 {
        server.push_behavior(MockBehavior::TextReply("Y".into()));
    }

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("追问封顶路径应 Ok");
    assert!(
        text.contains("模型只回了自检判定"),
        "追问额度用尽后应给可读说明，实际：{text:?}"
    );
    // 5 次请求：tool → 行动代答 → 判定 → 追问 1 → 追问 2（封顶后收尾）
    assert_eq!(server.request_count(), 5, "追问次数有硬顶，不能无限追着问");
}

/// 终稿自检那条路上的迟到判定同样不能当终稿：模型用行动代替了回答（终稿
/// 自检态随「以行动代回答」释放），那句 Y/N 问话留在上下文里，模型在后面
/// 某轮补了个 Y——出口闸要把它认成判定并追问，而不是当成答复交出去。
#[tokio::test]
async fn core_late_final_selfcheck_verdict_is_not_returned_as_answer() {
    let server = MockLlmServer::start();
    // 前 3 轮工具调用：凑够终稿自检的 function_calls 下限
    for _ in 0..3 {
        server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    // 第 4 轮：长文触发终稿自检（gap 默认 8，首次满足即触发）
    server.push_behavior(MockBehavior::TextReply(
        "已完成全部工作：本轮列出并逐条核对了所有任务条目，统计了完成与遗留情况，生成了汇总报告并写入文档，任务列表与统计结果均已在会话中展示完毕，没有遗漏任何步骤与细节，可以放心查阅。"
            .into(),
    ));
    // 第 5 轮：模型不答 Y/N 而是继续调工具核验（以行动代回答）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 第 6 轮：模型这才回答那句问话——一个裸 Y
    server.push_behavior(MockBehavior::TextReply("Y".into()));
    // 第 7 轮：被追问后说出真实结果
    server.push_behavior(MockBehavior::TextReply(
        "复核完成：所有条目与报告一致，结果无误。".into(),
    ));

    let h = CoreHarness::new();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        12,
        &h.stop,
        None,
        &h.deps_with_gap(None),
        exec,
        noop_replan,
    )
    .await
    .expect("终稿自检迟到判定路径应 Ok");
    assert!(
        text.contains("复核完成"),
        "裸 Y 不是答复，应被追问并拿到真实结果，实际：{text:?}"
    );
    assert!(
        text.trim() != "Y" && text.trim() != "y",
        "终稿不能是自检判定字样，实际：{text:?}"
    );
    assert_eq!(
        server.request_count(),
        7,
        "迟到判定应触发追问续跑，而不是拿 Y 终结回合"
    );
}

/// 滚动降密度：连续 7 轮工具调用后，round-0 的工具结果轮龄 ≥5
/// → 触发 `loop.msgs.age_compacted` 软压审计事件；随后文本轮正常收尾。
#[tokio::test]
async fn core_age_based_rolling_compaction_fires() {
    let server = MockLlmServer::start();
    for _ in 0..7 {
        server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    server.push_behavior(MockBehavior::TextReply("全部查完了".into()));

    let h = CoreHarness::new();
    // 长工具结果：太短（≤300 字符）压缩器会跳过，滚动降密度就没有可压对象
    let long_result = "X".repeat(1500);
    let exec = |_n: String, _a: String, _t: ToolCallTrace| {
        let long_result = long_result.clone();
        async move { ToolResult::ok(long_result, Vec::new()) }
    };
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        12,
        &h.stop,
        None,
        &h.deps(),
        exec,
        noop_replan,
    )
    .await
    .expect("滚动降密度路径应 Ok");
    assert_eq!(text, "全部查完了");
    assert_eq!(server.request_count(), 8);
    let events = h.events();
    assert!(
        events.contains(&"loop.msgs.age_compacted"),
        "round-0 的工具结果轮龄应到 5 触发软压：{events:?}"
    );
}

/// 侧调 Y：原文还原、无 ⚠️；主服务器 4 次请求（3 tool + 终文），侧服务器 1 次。
#[tokio::test]
async fn core_final_selfcheck_sidecall_y_no_extra_main_round() {
    let main = MockLlmServer::start();
    let side = MockLlmServer::start();
    for _ in 0..3 {
        main.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    main.push_behavior(MockBehavior::TextReply(SIDE_LONG_TEXT.into()));
    side.push_behavior(MockBehavior::OpenAiJsonReply("Y".into()));

    let h = CoreHarness::new();
    let sc = selfcheck_http(&side);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&main),
        user_msgs(),
        10,
        &h.stop,
        None,
        &h.deps_with_selfcheck(&sc),
        exec,
        noop_replan,
    )
    .await
    .expect("侧调 Y 路径应 Ok");
    assert!(text.contains("已完成 3 项工作"), "Y 应还原原文：{text:?}");
    assert!(!text.contains("⚠️"), "Y 不应追加警告：{text:?}");
    assert_eq!(main.request_count(), 4, "侧调不占主循环轮次");
    assert_eq!(side.request_count(), 1, "自检请求应打到侧调服务器");
}

/// 侧调 N（附解释「N，……」）：宽松前缀判定命中 N → 原文 + ⚠️ + 审计。
#[tokio::test]
async fn core_final_selfcheck_sidecall_n_appends_warning() {
    let main = MockLlmServer::start();
    let side = MockLlmServer::start();
    for _ in 0..3 {
        main.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "list_tasks".into(),
            arguments: "{}".into(),
        }));
    }
    main.push_behavior(MockBehavior::TextReply(SIDE_LONG_TEXT.into()));
    // 非流式应答带解释——宽松前缀解析取首字符判 N
    side.push_behavior(MockBehavior::OpenAiJsonReply(
        "N，报告中把完成日期写错了".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sc = selfcheck_http(&side);
    let mut deps = h.deps_with_selfcheck(&sc);
    deps.audit = {
        let logs2 = logs.clone();
        Box::leak(Box::new(
            move |_lvl: AuditLevel, event: &'static str, kv: Vec<(&'static str, String)>| {
                let kvs = kv
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                logs2.lock().unwrap().push(format!("{event} {kvs}"));
            },
        ))
    };
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("done", Vec::new()) };
    let (text, _) = run_model_loop_core(
        &core_http(&main),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("侧调 N 路径应 Ok");
    assert!(text.contains("已完成 3 项工作"), "N 应保留原文：{text:?}");
    assert!(text.contains("⚠️ 自检未达预期"), "N 应追加警告：{text:?}");
    assert_eq!(main.request_count(), 4, "侧调不占主循环轮次");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("final_selfcheck.fail")),
        "应记 final_selfcheck.fail 审计：{lines:?}"
    );
}
