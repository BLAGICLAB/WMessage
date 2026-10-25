//! 幻觉守卫用例：执行声称 / 落盘凭证 / 点名判据的对账。

use super::*;

/// 零工具调用 + 执行完成态终稿：应打回一轮，模型改口如实说明后才终结回合。
#[tokio::test]
async fn core_execution_claim_without_any_tool_call_is_pushed_back() {
    let server = MockLlmServer::start();
    // 第 1 轮：事故原话——零工具调用却报执行完成
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，sleep 120 秒后工具真实返回，打印了 `done`。".into(),
    ));
    // 第 2 轮：收到系统提示后改口，如实说明做不到（同会话 684/686 的正确行为）
    server.push_behavior(MockBehavior::TextReply(
        "受白名单约束，run_python 无法执行，不能编造结果。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    // exec_never：整场不该有任何工具被执行；真被调会 panic
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("打回后应 Ok");
    assert_eq!(
        text.trim(),
        "受白名单约束，run_python 无法执行，不能编造结果。",
        "应续跑并以模型的如实说明终结，实际：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "守卫应打回一轮而不是直接终结");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但本次提问零凭证")),
        "应记执行类谎报审计：{lines:?}"
    );
}

/// 真的调过工具之后：同样的完成态话术不得被拦（跨轮转述工具结果是常态）。
#[tokio::test]
async fn core_execution_claim_passes_after_a_tool_actually_ran() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 工具跑过之后的收尾话术——带执行完成态，但确实动过手
    server.push_behavior(MockBehavior::TextReply("已执行完毕，任务清单如上。".into()));

    let h = CoreHarness::new();
    let deps = h.deps();
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
    .expect("工具跑过后应直接 Ok");
    assert_eq!(
        text.trim(),
        "已执行完毕，任务清单如上。",
        "动过手就不该拦：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "不应被守卫多打一轮");
}

/// 零工具调用但如实拒绝：一次请求就该终结。守卫若误拦，等于惩罚诚实——
/// 同会话 message 684/686/688 正是这个形态。
#[tokio::test]
async fn core_honest_refusal_without_tool_call_ends_immediately() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "受白名单约束，`node_modules` 路径不在白名单，工具拒绝真去扫。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("如实拒绝应直接 Ok");
    assert!(text.contains("不在白名单"), "应原样返回模型的拒绝说明");
    assert_eq!(server.request_count(), 1, "如实拒绝不该被守卫多打一轮");
}

/// 实机抓到的第二起漏网：模型零工具调用，却用「分步叙述」报出完整的两步执行、
/// 异常原文和 stdout。窄词表（「已执行」家族）全不命中，扩表后必须打回。
#[tokio::test]
async fn core_step_narrated_execution_fabrication_is_pushed_back() {
    let server = MockLlmServer::start();
    // 第 1 轮：trace 51 的原话
    server.push_behavior(MockBehavior::TextReply(
        "完成计划2 步：第一步 `import nosuchmodule_zzz` 触发 `ModuleNotFoundError: No module named 'nosuchmodule_zzz'`，\
         第二步 `print(\"done\")` 仍输出 `done`（异常已捕获，未中断程序）。"
            .into(),
    ));
    // 第 2 轮：收到「必须发起 tool_call」的返工指令后，这次真去调
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"import nosuchmodule_zzz"}"#.into(),
    }));
    // 第 3 轮：拿到真实失败返回后才汇报
    server.push_behavior(MockBehavior::TextReply(
        "已执行完毕，确实失败了：ModuleNotFoundError: No module named 'nosuchmodule_zzz'。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec = |_n: String, _a: String, _t: ToolCallTrace| async {
        ToolResult::ok("ModuleNotFoundError", Vec::new())
    };
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
    .expect("打回后应 Ok");
    assert!(
        text.contains("ModuleNotFoundError"),
        "应续跑到真实执行后的汇报，实际：{text:?}"
    );
    assert_eq!(
        server.request_count(),
        3,
        "应「编造→打回→真调工具→据实汇报」三段"
    );
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但本次提问零凭证")),
        "应记执行类谎报审计：{lines:?}"
    );
}

/// 工具发起过但全部失败：返工指令必须点明「调过了但没成功」，而不是笼统说
/// 「你从未调用工具」——说错会让模型以为自己漏调了一次、转而去重复同一调用。
#[tokio::test]
async fn core_all_failed_tools_report_distinct_remedy() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"1/0"}"#.into(),
    }));
    // 工具失败后模型仍报成功（终稿含执行完成态）
    server.push_behavior(MockBehavior::TextReply("已执行完毕，一切正常。".into()));
    // 第 3 轮改口
    server.push_behavior(MockBehavior::TextReply(
        "执行失败了：ZeroDivisionError。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let exec = |_n: String, _a: String, _t: ToolCallTrace| async {
        ToolResult::error("ZeroDivisionError: division by zero", Vec::new())
    };
    let (_text, _) = run_model_loop_core(
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
    .expect("应 Ok");
    assert_eq!(
        server.request_count(),
        3,
        "工具失败后的成功汇报同样应打回一轮"
    );
}

/// 零工具调用的代码讲解轮：不得被扩表后的守卫误伤（讲「会抛出什么」是正当回答）。
#[tokio::test]
async fn core_code_explanation_without_tool_call_is_not_pushed_back() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "这段代码在 `import nosuchmodule_zzz` 处会抛出 ModuleNotFoundError，因为环境里没装这个模块。"
            .into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("讲解轮应直接 Ok");
    assert!(text.contains("ModuleNotFoundError"), "应原样返回讲解");
    assert_eq!(server.request_count(), 1, "代码讲解不该被守卫多打一轮");
}

// ────────────────────────────────────────────────────────────────────────────
// 凭证表判决：点名的工具逐个查本次提问的凭证池。
//
// 旧写法是一个布尔「本回合有没有任何工具成功」，同一次提问里不同工具的凭证能
// 互相担保——本轮成功调了 A，编造 B 的执行结果照样放行。下面两条把两侧都钉死。
// ────────────────────────────────────────────────────────────────────────────

/// 本轮成功调了 list_tasks，终稿却声称调了 run_python —— 必须拦。
/// 这是旧布尔写法漏掉的那一类：any_tool_succeeded 为真 → claim_execution 直接 false。
#[tokio::test]
async fn core_receipt_table_blocks_claiming_an_unrun_tool() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "list_tasks".into(),
        arguments: "{}".into(),
    }));
    // 编造：点名 run_python，但本次提问里从未调过它
    server.push_behavior(MockBehavior::TextReply(
        "已执行完毕，run_python 跑出来是 42。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply(
        "抱歉，我并没有调用过 run_python——本次只列了任务清单。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("打回后应 Ok");
    assert!(
        text.contains("并没有调用过 run_python"),
        "应改口承认没调过：{text:?}"
    );
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但无对应凭证: run_python")),
        "审计应点名缺失的工具：{lines:?}"
    );
}

/// 跨轮据实汇报：凭证在上一轮、汇报在下一轮，必须放行且不多问一轮。
///
/// 真实事故：trace 53 的最终汇报「执行完毕：模块不存在…最终输出 done」是真话，
/// 凭证落在上一轮。按轮清空凭证池会把它判成幻觉，再让模型重跑一个已完成的
/// 动作——对 delete_task / link_file_to_task 这类就是重复的破坏性操作。
#[tokio::test]
async fn core_receipt_table_survives_cross_round_truthful_report() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print('done')"}"#.into(),
    }));
    // 下一轮据实转述上一轮的凭证：点名 run_python，且凭证表里确有它
    server.push_behavior(MockBehavior::TextReply(
        "已执行完毕，run_python 输出 `done`。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
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
    .expect("据实汇报应直接 Ok");
    assert_eq!(
        text.trim(),
        "已执行完毕，run_python 输出 `done`。",
        "跨轮据实转述不该被拦：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "凭证表跨轮累积，不得多打一轮");
}

/// 建议语气里提到工具名（零工具调用）不该被当成「声称调过它」。
#[tokio::test]
async fn core_advice_mentioning_tool_name_is_not_a_claim() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "这个问题你可以用 grep_files 搜，但我这边没有文件访问权限，搜不了。".into(),
    ));

    let h = CoreHarness::new();
    let deps = h.deps();
    let (text, _) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        10,
        &h.stop,
        None,
        &deps,
        exec_never,
        noop_replan,
    )
    .await
    .expect("如实说明应直接 Ok");
    assert!(text.contains("没有文件访问权限"), "应原样返回：{text:?}");
    assert_eq!(server.request_count(), 1, "建议语气不得被拦");
}

/// 实机漏网一（trace 63，tools=0）：整句没有完成态词点，闸门压根没进判决。
///
/// 模型零工具调用却报出一次完整执行，而且连历史都是编的——那个会话只跑过
/// time.sleep(2)，从来没有 print 100。
#[tokio::test]
async fn core_first_person_execution_narration_without_receipt_is_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "我跑了一个 print 100 的小任务，返回了 100。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply(
        "我没有执行过任何脚本——刚才那句是错的。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("打回后应 Ok");
    assert!(text.contains("没有执行过任何脚本"), "应改口：{text:?}");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但本次提问零凭证")),
        "应记零凭证：{lines:?}"
    );
}

/// 实机漏网二（trace 65，tools=0）：声称派了子 agent 并报了个实测值。
///
/// 双层都得在位才拦得住——「实测约」把话术引进判决，别名表把「子 agent」
/// 映射成 spawn_subagent 让点名查询有键可查。缺任一层都是静默放行。
#[tokio::test]
async fn core_subagent_fabrication_is_blocked_via_alias() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "子 agent 跑 sleep(5) 实测约 5013 ms。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply(
        "我没有派过子 agent，那是编的。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("打回后应 Ok");
    assert!(text.contains("没有派过子 agent"), "应改口：{text:?}");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但无对应凭证: spawn_subagent")),
        "审计应点名 spawn_subagent（别名映射的证据）：{lines:?}"
    );
}

/// 子 agent 凭证归并的正路：主 agent 派过子 agent 并如实汇报它跑的活，不该被拦。
///
/// 子 agent 的 span 在自己的 trace 下，主池里没有 run_python——不做归并的话
/// 这条正路每次汇报都会被当成凭空伪造。
#[tokio::test]
async fn core_subagent_receipts_credit_lets_truthful_report_through() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "spawn_subagent".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply(
        "子 agent 已执行完毕，run_python 实测约 5013 ms。".into(),
    ));

    let h = CoreHarness::new();
    // 子 agent 真的用 run_python 跑出了那个数（本会话调过这两个工具）
    h.set_session_tools(&["run_python", "spawn_subagent"]);
    let deps = h.deps();
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("据实汇报应直接 Ok");
    assert!(
        text.contains("run_python 实测约 5013 ms"),
        "应原样返回：{text:?}"
    );
    assert_eq!(server.request_count(), 2, "子 agent 归并后不得返工");
}

/// 返工之后仍编造：不再返工，终稿末尾挂一条如实更正。
///
/// 守的是「谎话不能单独到达用户」。此前第二次谎报整块守卫直接跳过（返工机会
/// 一次用完就作废），原文原样出到用户眼前——用户看到的是一句纯假的执行汇报。
/// 现在第二次改成不返工、只标注：再返工会让模型重跑已完成的动作（delete_task
/// 那类是重复的破坏性操作），而谎报已经拦过一次，再拦一次收益不抵风险。
#[tokio::test]
async fn core_second_fabrication_is_marked_not_silently_passed() {
    let server = MockLlmServer::start();
    // 第一轮：零工具调用却叙述执行结果 → 守卫返工
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，工具真实返回，打印了 done。".into(),
    ));
    // 第二轮：仍然编造 → 不再返工，改为在终稿标注
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，工具真实返回，打印了 done。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("第二次编造应 Ok");
    // 谎话本身仍在（不删原文——删掉只会让用户更信那句假的）
    assert!(text.contains("执行完毕"), "应保留原文：{text:?}");
    // 但必须带一条如实更正，让用户知道这句没有凭证
    assert!(
        text.contains("⚠️") && text.contains("没有实际发生"),
        "第二次编造应在终稿标注：{text:?}"
    );
    // 不再返工：只有两次请求（第二次是模型的作答，不是第三次返工）
    assert_eq!(server.request_count(), 2, "第二次编造不得再返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("hallucination_guard.repeat")),
        "应记 repeat 审计：{lines:?}"
    );
}

/// 正路不回归：第一次编造被返工、模型改口后终稿干净，不该挂更正标注。
#[tokio::test]
async fn core_truthful_rewrite_after_rework_carries_no_notice() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "执行完毕，工具真实返回，打印了 done。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply("我没有执行过任何脚本。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("改口后应 Ok");
    assert!(text.contains("没有执行过任何脚本"), "应改口：{text:?}");
    assert!(
        !text.contains("⚠️"),
        "改口后的干净终稿不该挂更正标注：{text:?}"
    );
}

/// 返工后的如实改口不得再被当成第二次谎报挂标注。
///
/// 真机 trace 110：第一遍谎报「文件已生成在 X」被拦下，模型第二遍
/// 照着改口提示写如实更正——复述了那句「已生成」，路径出现在后面另一段。全文级
/// 的路径提取把两者凑成一次声称，于是诚实更正被挂上「⚠️ 不是本次生成的」。
///
/// 触发词与路径必须同句：改口那句话里「已生成」在第一句、路径在最后一句。
#[tokio::test]
async fn core_truthful_retraction_of_file_claim_carries_no_notice() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("2026Q3复盘.docx");
    std::fs::write(&doc, b"docx").unwrap();
    let path = doc.to_string_lossy().into_owned();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(format!(
        "好的，已整理，文件已生成在 `{}`。",
        path
    )));
    server.push_behavior(MockBehavior::TextReply(format!(
        "你说得对，之前那条「已生成」是我口胡，实际并没有落盘。\n\n你之前明确说过这份纪要要存到 `{}`，但这一条我会卡死。",
        path
    )));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 这个路径查不到落盘凭证：它不是任何工具写出来的
    h.file_receipts_missing.lock().unwrap().push(path.clone());
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("改口后应 Ok");
    assert_eq!(server.request_count(), 2, "第一遍谎报应被打回一轮");
    assert!(text.contains("是我口胡"), "应如实改口：{text:?}");
    assert!(!text.contains("⚠️"), "如实改口不该被挂上更正标注：{text:?}");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines
            .iter()
            .any(|l| l.contains("hallucination_guard.repeat")),
        "如实改口不该记第二次谎报：{lines:?}"
    );
}

/// 落盘凭证正路：模型真调了文档生成工具（带回凭证），终稿报同一个路径——
/// 存在性与凭证两道都站得住，不该被拦。
///
/// 这条是正路回归锁：凭证那道判据一旦接线错（凭证没进池），每一次正常的
/// 文档生成都会被当成凭空声称 → 守卫让模型重做 → 白跑一遍生成。
#[tokio::test]
async fn core_generated_doc_with_receipt_is_not_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("周报.pptx");
    std::fs::write(&doc, b"pptx").unwrap();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "create_ppt".into(),
        arguments: "{}".into(),
    }));
    server.push_behavior(MockBehavior::TextReply(format!(
        "已生成 PPT 演示文稿：{}",
        doc.display()
    )));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let receipt = wmessage_lib::bot::FileChangeReceipt {
        path: doc.to_string_lossy().into_owned(),
        kind: "create",
        added: 0,
        deleted: 0,
        diff: None,
        truncated: false,
        before_ref: None,
        before_sha: None,
        after_sha: Some("0f0f".into()),
        replacements: 0,
    };
    let exec = move |_n: String, _a: String, _t: ToolCallTrace| {
        let receipt = receipt.clone();
        async move { ToolResult::ok("已生成 PPT 演示文稿", Vec::new()).with_file_change(receipt) }
    };
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
    .expect("有凭证的正路应 Ok");
    assert!(text.contains("已生成 PPT 演示文稿"), "应原样返回：{text:?}");
    assert!(!text.contains("⚠️"), "有凭证的正路不该被标注：{text:?}");
    assert_eq!(server.request_count(), 2, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "不该记守卫审计：{lines:?}"
    );
}

/// 落盘凭证反路：路径在磁盘上确实存在，但从来不是任何工具写出来的——
/// 模型在凭空声称「已导出」这个文件。
///
/// 用「已导出」而不是「已生成」是有意的：「已导出」不在变更类词表里
/// （claims_mutation 抓不到），所以这一条只有落盘凭证那道判据能拦——测的就是它。
#[tokio::test]
async fn core_claimed_doc_without_receipt_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("不是我生成的.pdf");
    std::fs::write(&doc, b"pdf").unwrap();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(format!(
        "已导出 PDF 文档：{}",
        doc.display()
    )));
    server.push_behavior(MockBehavior::TextReply("这个文件不是我导出的。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 这个路径查不到落盘凭证：它不是任何工具写出来的
    h.file_receipts_missing
        .lock()
        .unwrap()
        .push(doc.to_string_lossy().into_owned());
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("无凭证的声称应被打回后 Ok");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("声称已生成但无落盘凭证")),
        "应记无落盘凭证：{lines:?}"
    );
    assert_eq!(server.request_count(), 2, "应返工一轮");
    assert!(text.contains("不是我导出的"), "模型应被要求改口：{text:?}");
}

// ────────────────────────────────────────────────────────────────────────────
// 点名判据（独立判据）：终稿点名某工具跑过，而那个工具从没成功返回过。
//
// 真机事故（2026-10-20 trace 101）：零工具调用，终稿却把工具调用结果列成清单
// 「- create_word：✅ 调用成功，返回路径 …」，声称的产物根本不存在。守卫生效与否
// 取决于两件事：触发层认不认这种清单写法，判决层查不查得到这个工具的凭证。
// ────────────────────────────────────────────────────────────────────────────

/// 清单写法报「✅ 调用成功」，而 create_word 从没被调用过 —— 必须拦。
///
/// 这条就是真机 trace 101 的原话形态。清单里的成功标记（✅）与工具名落在同一
/// 行，触发层认得；判决层查不到 create_word 的成功记录，判成编造。
#[tokio::test]
async fn core_named_tool_claimed_success_without_receipt_is_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "工具调用如下：\n- create_word：✅ 调用成功，返回路径 `/tmp/AI_Gen_Files/2026Q3复盘.docx`"
            .into(),
    ));
    server.push_behavior(MockBehavior::TextReply("我没有调用过 create_word。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("打回后应 Ok");
    assert_eq!(server.request_count(), 2, "应返工一轮");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但无对应凭证: create_word")),
        "应点名缺凭证的工具：{lines:?}"
    );
    assert!(
        text.contains("没有调用过 create_word"),
        "模型应被要求改口：{text:?}"
    );
}

/// 如实汇报失败不受影响：词表收的是完成态措辞，「调用失败」不在其中。
///
/// 真机 trace 103 的原话形态：模型如实报「run_python：调用失败」「complete_task：
/// ❌ 任务不存在，调用被拒」——工具确实没跑成（Python 开关关着、任务不存在），
/// 它是在如实交代，不是编造。
///
/// 标题那行「工具调用结果」带执行措辞，但工具名在下一行——同名不同句，不能
/// 把标题的措辞算到工具名头上。这条钉的就是这个分句边界。
#[tokio::test]
async fn core_honest_failure_report_is_not_treated_as_fabrication() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "工具调用结果：\n- run_python：调用失败，返回错误 `[ErrNo -7] Function execution failed.`\n- complete_task：❌ 任务不存在，调用被拒".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("如实汇报失败应直接 Ok");
    assert_eq!(server.request_count(), 1, "如实汇报失败不该被拦");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "不该记守卫审计：{lines:?}"
    );
    assert!(text.contains("调用失败"), "应原样返回：{text:?}");
}

/// 建议语气提到工具名不算声称（点名判据的触发层按句判，不是全局判一次）。
///
/// 词表命中与否在这句里是「没有」——句中没有执行措辞。
#[tokio::test]
async fn core_advice_mentioning_tool_name_with_claim_word_elsewhere_is_not_a_claim() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "任务我已经执行完了，但这一步你可以用 grep_files 搜，我没有文件访问权限。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("建议语气应直接 Ok");
    assert_eq!(server.request_count(), 1, "建议语气不该被拦");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "不该记守卫审计：{lines:?}"
    );
    assert!(text.contains("grep_files"), "应原样返回：{text:?}");
}

/// 跨提问的重述是真话：本会话里 create_word 成功过，这次提到它不算编造。
#[tokio::test]
async fn core_cross_question_restatement_of_succeeded_tool_passes() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(
        "刚才 create_word 调用成功了，文件在 `/tmp/AI_Gen_Files/周报.docx`。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 本会话上一条提问里 create_word 成功过
    h.set_session_tools(&["create_word"]);
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("据实重述应直接 Ok");
    assert_eq!(server.request_count(), 1, "跨提问的重述不该被拦");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "不该记守卫审计：{lines:?}"
    );
    assert!(text.contains("create_word"), "应原样返回：{text:?}");
}

/// 调用过但没成功，却报成功 —— 判成编造（判据问的是「成功过没有」）。
#[tokio::test]
async fn core_named_tool_claimed_success_though_call_failed_is_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print('done')"}"#.into(),
    }));
    server.push_behavior(MockBehavior::TextReply(
        "- run_python：✅ 调用成功，输出 done".into(),
    ));
    server.push_behavior(MockBehavior::TextReply("run_python 其实失败了。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    // 调用返回失败：只有「调用过」，没有「成功过」
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::error("boom", Vec::new()) };
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
    .expect("打回后应 Ok");
    assert_eq!(server.request_count(), 3, "应返工一轮");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("声称已执行但无对应凭证: run_python")),
        "应点名缺成功记录的工具：{lines:?}"
    );
    assert!(text.contains("失败了"), "模型应被要求改口：{text:?}");
}

/// 不可证伪的写入通道按**本次提问**让开，不是整条会话。
///
/// 本次提问跑过 run_python（它能写盘、但走的是我们没有凭证的通道）→ 落盘凭证
/// 那道判据让开。早先按会话判：一次 run_python 会把这条会话后续所有提问的凭证
/// 闸全打死——那不是「无从确证」，那是把闸门焊死。
#[tokio::test]
async fn core_unverifiable_writer_exemption_is_per_question() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("不是我生成的.pdf");
    std::fs::write(&doc, b"pdf").unwrap();

    let server = MockLlmServer::start();
    // 本次提问先跑一次 run_python（自己写盘的通道，我们拿不到它的落盘凭证）
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print('done')"}"#.into(),
    }));
    server.push_behavior(MockBehavior::TextReply(format!(
        "已导出 PDF 文档：{}",
        doc.display()
    )));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 这个路径查不到落盘凭证：run_python 写的东西走的是我们没有凭证的通道
    h.file_receipts_missing
        .lock()
        .unwrap()
        .push(doc.to_string_lossy().into_owned());
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("让开时应直接 Ok");
    assert_eq!(server.request_count(), 2, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "本次提问跑过不可证伪的写入通道，凭证闸应让开：{lines:?}"
    );
    assert!(text.contains("已导出 PDF 文档"), "应原样返回：{text:?}");
}

/// 让开是**本次提问**的事：上一条提问跑过 run_python，不能把这次的凭证闸一起关掉。
///
/// 早先按会话判：一次 run_python 会让这条会话后续所有提问的落盘凭证闸全打死——
/// 那不是「无从确证」，是把闸门焊死。这条钉的就是这个口径。
#[tokio::test]
async fn core_unverifiable_writer_exemption_does_not_leak_into_next_question() {
    let dir = tempfile::tempdir().unwrap();
    let doc = dir.path().join("不是我生成的.pdf");
    std::fs::write(&doc, b"pdf").unwrap();

    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::TextReply(format!(
        "已导出 PDF 文档：{}",
        doc.display()
    )));
    server.push_behavior(MockBehavior::TextReply("这个文件不是我导出的。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    // 上一条提问跑过 run_python（自己写盘的通道）
    h.set_session_tools(&["run_python"]);
    // 这个路径查不到落盘凭证
    h.file_receipts_missing
        .lock()
        .unwrap()
        .push(doc.to_string_lossy().into_owned());
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("ok", Vec::new()) };
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
    .expect("无凭证的声称应被打回后 Ok");
    assert_eq!(server.request_count(), 2, "应返工一轮");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("声称已生成但无落盘凭证")),
        "上一条提问的 run_python 不该让开这一条：{lines:?}"
    );
    assert!(text.contains("不是我导出的"), "模型应被要求改口：{text:?}");
}

/// 判据一（结果无出处）：工具跑过，但报出来的数不是它给的。
///
/// 真机那类形态：模型调了工具（凭证表里有「调过」），终稿却报了一个工具返回
/// 里根本没有的数。凭证表答不了这一问——它记的是「哪个工具成功过」，不是
/// 「这个数是哪来的」。
#[tokio::test]
async fn core_result_value_without_source_is_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print('done')"}"#.into(),
    }));
    // 工具真的返回了，但返回里没有 5050
    server.push_behavior(MockBehavior::TextReply("已跑完，输出 5050".into()));
    server.push_behavior(MockBehavior::TextReply("刚才那个数是编的。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
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
    .expect("打回后应 Ok");
    assert_eq!(server.request_count(), 3, "应返工一轮");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("声称的结果无出处: 5050")),
        "报出来的数没有出处，应判成编造：{lines:?}"
    );
    assert!(text.contains("编的"), "模型应被要求改口：{text:?}");
}

/// 判据一的对家：有出处的数不判。工具给的是 5050，模型报 5050 就是如实转述。
#[tokio::test]
async fn core_result_value_with_source_is_not_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print(sum(range(1,101)))"}"#.into(),
    }));
    server.push_behavior(MockBehavior::TextReply("已跑完，输出 5050".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec =
        |_n: String, _a: String, _t: ToolCallTrace| async { ToolResult::ok("5050", Vec::new()) };
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
    .expect("有出处应直接 Ok");
    assert_eq!(server.request_count(), 2, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "数有出处，不该判：{lines:?}"
    );
    assert!(text.contains("5050"), "应原样返回：{text:?}");
}

/// 判据二（回声调用）：模型把结论写进参数，让工具原样吐回来，再拿它当依据。
///
/// 真机 trace 106 的形态：browser_evaluate 的函数体是一句硬编码字符串，工具
/// 「成功」返回的正是模型自己写的那句。这次调用没替模型核实任何东西。
#[tokio::test]
async fn core_echo_call_cannot_back_a_conclusion() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "mcp_playwright_browser_evaluate".into(),
        arguments: r#"{"function":"async () => { return { desktopDocx: 'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found' }; }"}"#.into(),
    }));
    server.push_behavior(MockBehavior::TextReply(
        "已核实：桌面上没有 /Users/renshi/Desktop/季度复盘.docx，只有 2026Q3复盘.docx。".into(),
    ));
    server.push_behavior(MockBehavior::TextReply("刚才没真的核实过。".into()));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    // 工具把参数里那句话原样吐回来——回声
    let exec = |_n: String, _a: String, _t: ToolCallTrace| async {
        ToolResult::ok(
            "### Result\n{\n  \"desktopDocx\": \"only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\"\n}\n### Ran Playwright code\n```js\nawait page.evaluate('async () => { return { desktopDocx: \\'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\\' }; }');\n```\n### Page\n- Page URL: about:blank",
            Vec::new(),
        )
    };
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
    .expect("打回后应 Ok");
    assert_eq!(server.request_count(), 3, "应返工一轮");
    let lines = logs.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("结论无工具支撑")),
        "回声调用不能给结论背书：{lines:?}"
    );
    assert!(text.contains("没真的核实过"), "模型应被要求改口：{text:?}");
}

/// 判据二的对家：同一提问里还有一次真读到现场的调用，结论就有依据，不判。
///
/// 真机 trace 106 就是这个形态——browser_evaluate 那次回声，但 run_python 真的
/// 读了桌面，模型报的结论站得住。
#[tokio::test]
async fn core_echo_call_alongside_real_lookup_is_not_blocked() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "mcp_playwright_browser_evaluate".into(),
        arguments: r#"{"function":"async () => { return { desktopDocx: 'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found' }; }"}"#.into(),
    }));
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"import glob; print(glob.glob('/Users/renshi/Desktop/*.docx'))"}"#
            .into(),
    }));
    server.push_behavior(MockBehavior::TextReply(
        "已核实：桌面上没有 /Users/renshi/Desktop/季度复盘.docx，只有 2026Q3复盘.docx。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
    let exec = |name: String, _a: String, _t: ToolCallTrace| async move {
        if name == "run_python" {
            ToolResult::ok(
                "docx on desktop: ['/Users/renshi/Desktop/2026Q3复盘.docx']",
                Vec::new(),
            )
        } else {
            ToolResult::ok(
                "### Result\n{\n  \"desktopDocx\": \"only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\"\n}\n### Ran Playwright code\n```js\nawait page.evaluate('async () => { return { desktopDocx: \\'only 2026Q3复盘.docx exists, 季度复盘.docx NOT found\\' }; }');\n```\n### Page\n- Page URL: about:blank",
                Vec::new(),
            )
        }
    };
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
    .expect("有真读到现场的调用应直接 Ok");
    assert_eq!(server.request_count(), 3, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "还有一次读到现场的调用，结论有依据：{lines:?}"
    );
    assert!(text.contains("已核实"), "应原样返回：{text:?}");
}

/// 如实说明「这个数是我的推算、不是执行产物」不算谎报执行结果。
///
/// 工具确实调过（凭证表里有），但模型报的那个数不是工具给的——它自己标了是
/// 推算。这类不该判：判了就是把「如实标注」也当成编造。
#[tokio::test]
async fn core_honest_failure_report_is_not_blocked_by_value_criterion() {
    let server = MockLlmServer::start();
    server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
        name: "run_python".into(),
        arguments: r#"{"code":"print(sum(range(1,101)))"}"#.into(),
    }));
    server.push_behavior(MockBehavior::TextReply(
        "我跑了脚本，答案是 5050（按公式推算的，不是这次跑出来的）。".into(),
    ));

    let h = CoreHarness::new();
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let deps = deps_capturing_logs(&h, &logs);
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
    .expect("如实标注推算应直接 Ok");
    assert_eq!(server.request_count(), 2, "不该返工");
    let lines = logs.lock().unwrap().clone();
    assert!(
        !lines.iter().any(|l| l.contains("hallucination_guard")),
        "如实标注推算不受结果值判据影响：{lines:?}"
    );
    assert!(text.contains("5050"), "应原样返回：{text:?}");
}
