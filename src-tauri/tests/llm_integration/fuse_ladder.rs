//! 工具调用阶梯端到端：被拒不计数 / 收尾窗读拒写放 / 收尾轮数出口 / 轮数轴收尾窗。
//!
//! 与 core.rs 分开放：阶梯是独立的一层语义（fuse::gate_call），用例也比
//! 主循环其他用例长——合在一处会把 core.rs 推过行数上限。

use super::*;

/// 被闸拒（Denied）的调用不得计入熔断计数：熔断上限 2，模型连发
/// 9 轮被拒的读类调用（exec 模拟 dispatch 白名单 / 规则闸的拒绝回包）：若被拒
/// 的也计数，第 3 次就熔断；只有「实际执行的才算数」才能让模型继续试探并交卷。
#[tokio::test]
async fn core_denied_tool_calls_do_not_count_toward_fuse() {
    use std::sync::atomic::AtomicUsize;

    let sid = "sess-fuse-denied-e2e";
    let used = Arc::new(AtomicUsize::new(0));
    wmessage_lib::tool_guard::register_subagent_session(
        sid,
        wmessage_lib::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_fuse_denied".into(),
            task_id: None,
            profile: wmessage_lib::db::SubagentProfile::Research,
            tools: Arc::new(vec!["read_text_file", "write_artifact_file"]),
            artifact_dir: std::env::temp_dir().join("wmsg-fuse-denied-e2e"),
            used_tool_calls: used.clone(),
            used_rounds: Arc::new(AtomicUsize::new(0)),
            last_progress_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        },
    );

    let server = MockLlmServer::start();
    for _ in 0..9 {
        server.push_behavior(MockBehavior::ToolCall(ToolCallResponse {
            name: "read_text_file".into(),
            arguments: r#"{"path":"/x.txt"}"#.into(),
        }));
    }
    server.push_behavior(MockBehavior::TextReply(
        r#"{"status":"succeeded","summary":"交卷"}"#.into(),
    ));

    let h = CoreHarness::with_session(Some(sid));
    let exec = |name: String, _args: String, _trace: ToolCallTrace| async move {
        assert_eq!(name, "read_text_file");
        ToolResult::denied("⚠️ 子 agent 白名单外工具已拒绝。")
    };
    // 熔断上限与主会话同源（配置覆盖位）：这里直接注入 2
    let mut deps = h.deps();
    deps.fuse_cap_override = Some(2);

    let (text, _refs) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        20,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("被拒不计数时不应熔断，应跑到交卷轮");

    wmessage_lib::tool_guard::unregister_subagent_session(sid);

    assert!(
        text.contains("succeeded") && !text.contains("⏹ 已熔断"),
        "9 次被拒调用不得烧掉 2 的熔断上限：{text}"
    );
}

/// 收尾窗端到端：熔断上限 4 → 收尾窗阈值 3。前 3 轮读计满进窗；窗内读被核心循环
/// 拒绝（根本不会到 exec）、写放行（到 exec）且都不计数；进入后第 4 轮硬停，
/// 走熔断标记而不是计数熔断。
///
/// 行为按轮：0/1/2 读（执行，计 1/2/3，第 2 轮末进窗）→ 3 读（拒）→ 4 写（放行）
/// → 5 读（拒）→ 6 = 进入后第 4 轮：硬停。exec 应恰好被调 4 次（3 读 + 1 写）。
#[tokio::test]
async fn core_wrap_up_window_denies_reads_allows_writes_then_stops_by_rounds() {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    let sid = "sess-wrap-up-e2e";
    wmessage_lib::tool_guard::register_subagent_session(
        sid,
        wmessage_lib::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_wrap_up".into(),
            task_id: None,
            profile: wmessage_lib::db::SubagentProfile::Research,
            tools: Arc::new(vec!["read_text_file", "write_artifact_file"]),
            artifact_dir: std::env::temp_dir().join("wmsg-wrap-up-e2e"),
            used_tool_calls: Arc::new(AtomicUsize::new(0)),
            used_rounds: Arc::new(AtomicUsize::new(0)),
            last_progress_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        },
    );

    let server = MockLlmServer::start();
    let read = || {
        MockBehavior::ToolCall(ToolCallResponse {
            name: "read_text_file".into(),
            arguments: r#"{"path":"/x.txt"}"#.into(),
        })
    };
    let write = || {
        MockBehavior::ToolCall(ToolCallResponse {
            name: "write_artifact_file".into(),
            arguments: r#"{"path":"out.md","content":"x"}"#.into(),
        })
    };
    for behavior in [
        read(),
        read(),
        read(),
        read(),
        write(),
        read(),
        read(),
        // 硬停发生在第 6 轮进入 dispatch 之前；多备几轮确保不是「跑完了」
        read(),
        read(),
    ] {
        server.push_behavior(behavior);
    }

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let executed_for_exec = executed.clone();
    let h = CoreHarness::with_session(Some(sid));
    let exec = move |name: String, _args: String, _trace: ToolCallTrace| {
        let executed = executed_for_exec.clone();
        async move {
            executed.lock().expect("executed 锁").push(name);
            ToolResult::ok("ok", Vec::new())
        }
    };

    // 熔断上限与主会话同源（配置覆盖位）：注入 4 → 收尾窗阈值 3
    let mut deps = h.deps();
    deps.fuse_cap_override = Some(4);

    let (text, _refs) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        20,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("收尾轮数用尽是带正文的硬停，不是 Err");

    wmessage_lib::tool_guard::unregister_subagent_session(sid);

    let executed = executed.lock().expect("executed 锁").clone();
    assert_eq!(
        executed,
        vec![
            "read_text_file",
            "read_text_file",
            "read_text_file",
            "write_artifact_file"
        ],
        "窗内的读不得到达 exec，窗内的写必须到达 exec：{executed:?}"
    );
    assert!(
        text.contains("⏹ 已熔断") && text.contains("收尾 3 轮用尽"),
        "进入收尾窗后第 4 轮必须按轮数硬停：{text}"
    );
    assert!(
        !text.contains("次上限"),
        "硬停必须是轮数出口，不是计数熔断（窗内写不计数，计数永远到不了上限）：{text}"
    );
}

/// 轮数轴收尾窗端到端：max_rounds 8 → 剩最后 3 轮（第 6 轮起）只放写产物类，
/// 工具计数远未触轴（cap 100 / 阈值 95）也照拦——两轴各自独立生效。
///
/// 行为按轮：0..=4 读（执行，计 1..=5）→ 5 读（轮数轴进窗，拒）→ 6 写（放行）
/// → 7 读（拒）→ 循环落出：轮数上限硬停、带正文。exec 应恰好 6 次（5 读 + 1 写）。
#[tokio::test]
async fn core_rounds_wrap_up_window_denies_reads_before_tool_budget() {
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    let sid = "sess-rounds-wrap-up-e2e";
    wmessage_lib::tool_guard::register_subagent_session(
        sid,
        wmessage_lib::tool_guard::SubagentSessionCtx {
            subagent_id: "sa_rounds_wrap_up".into(),
            task_id: None,
            profile: wmessage_lib::db::SubagentProfile::Research,
            tools: Arc::new(vec!["read_text_file", "write_artifact_file"]),
            artifact_dir: std::env::temp_dir().join("wmsg-rounds-wrap-up-e2e"),
            used_tool_calls: Arc::new(AtomicUsize::new(0)),
            used_rounds: Arc::new(AtomicUsize::new(0)),
            last_progress_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        },
    );

    let server = MockLlmServer::start();
    let read = || {
        MockBehavior::ToolCall(ToolCallResponse {
            name: "read_text_file".into(),
            arguments: r#"{"path":"/x.txt"}"#.into(),
        })
    };
    let write = || {
        MockBehavior::ToolCall(ToolCallResponse {
            name: "write_artifact_file".into(),
            arguments: r#"{"path":"out.md","content":"x"}"#.into(),
        })
    };
    for behavior in [
        read(),
        read(),
        read(),
        read(),
        read(),
        read(),
        write(),
        read(),
        // 硬停发生在循环自然落出；多备几轮确保不是「mock 用尽提前停」
        read(),
        read(),
    ] {
        server.push_behavior(behavior);
    }

    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let executed_for_exec = executed.clone();
    let h = CoreHarness::with_session(Some(sid));
    let exec = move |name: String, _args: String, _trace: ToolCallTrace| {
        let executed = executed_for_exec.clone();
        async move {
            executed.lock().expect("executed 锁").push(name);
            ToolResult::ok("ok", Vec::new())
        }
    };

    // 工具轴天花板拉到 100（阈值 95）：整场只执行 6 次，工具轴绝无可能触发——
    // 进窗的只能是轮数轴（max_rounds 8 − WRAP_UP_ROUNDS 3 = 第 6 轮起）。
    let mut deps = h.deps();
    deps.fuse_cap_override = Some(100);

    let (text, _refs) = run_model_loop_core(
        &core_http(&server),
        user_msgs(),
        8,
        &h.stop,
        None,
        &deps,
        exec,
        noop_replan,
    )
    .await
    .expect("轮数上限硬停是带正文的返回，不是 Err");

    wmessage_lib::tool_guard::unregister_subagent_session(sid);

    let executed = executed.lock().expect("executed 锁").clone();
    assert_eq!(
        executed,
        vec![
            "read_text_file",
            "read_text_file",
            "read_text_file",
            "read_text_file",
            "read_text_file",
            "write_artifact_file"
        ],
        "轮数进窗后读不得到达 exec、写必须到达 exec（工具计数远未触轴）：{executed:?}"
    );
    assert!(
        text.contains("⏹ 已熔断（轮数上限）") && text.contains("8 轮上限"),
        "轮数轴窗口的出口是 max_rounds 自然硬停：{text}"
    );
    assert!(
        !text.contains("Function 调用超过"),
        "工具计数熔断不得出现（整场只执行 6 次，cap 100）：{text}"
    );
}
