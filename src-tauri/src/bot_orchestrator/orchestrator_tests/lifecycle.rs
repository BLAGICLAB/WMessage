use super::super::*;
use super::*;
use crate::bot::subagent_digest::build_subagent_digest;
use crate::bot::subagent_digest::parse_result_json;
use crate::bot::subagent_digest::DIGEST_MAX_ITEMS;
use crate::bot::subagent_digest::DIGEST_MAX_SUMMARY_CHARS;
use crate::bot::subagent_runner::panic_to_command_error;
use crate::bot::subagent_spawn::gate;
use crate::bot::subagent_spawn::GateTicket;

/// runner panic 必须归一成普通失败，收尾路径照常走。
///
/// panic 会跳过整条收尾（取消令牌反注册、状态落库、subagent-finished
/// 事件）——前端那一行永远转圈，行状态停在 running。
#[test]
fn runner_panic_becomes_ordinary_failure() {
    let e = panic_to_command_error("index out of bounds");
    let msg = e.message();
    assert!(
        msg.contains("panic") && msg.contains("index out of bounds"),
        "panic 信息要如实带进失败原因：{msg}"
    );
    // 超长信息截断（审计日志与失败原因都不该被一段长 payload 撑爆）
    let long = "x".repeat(5000);
    let e2 = panic_to_command_error(&long);
    assert!(
        e2.message().chars().count() < 400,
        "panic 信息应截断到 200 量级：{}",
        e2.message().chars().count()
    );
}
/// 接线守卫：runner future 必须包 catch_unwind，且取消令牌登记要有
/// RAII 反注册——两者都是「panic 路径不漏」的前提。
#[test]
fn runner_is_panic_isolated_and_stop_token_has_raii_cleanup() {
    // runner 已搬进子模块：守卫跟着扫新文件
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/bot/subagent_runner.rs"
    ))
    .expect("应能读到自己");
    let start = src
        .find("async fn run_subagent(")
        .expect("应能找到 run_subagent");
    let rest = &src[start..];
    // 函数体到顶格 "}" 为止（嵌套块的 "}" 都带缩进）
    let end = rest[1..].find("\n}\n").map(|i| i + 1).unwrap_or(rest.len());
    let body = &rest[..end];
    assert!(
        body.contains("catch_unwind"),
        "runner future 必须包 catch_unwind（panic 要归一成普通失败走收尾）：{body}"
    );
    assert!(
        body.contains("StopTokenGuard"),
        "取消令牌登记必须配 RAII 反注册（panic 路径不能把条目留在 subagent_stops）：{body}"
    );
}
#[test]
fn poll_step_ladder_is_monotonic_and_capped() {
    // 起手就稀疏（终态走事件唤醒，阶梯只兜漏通知）：每次读都要开一次库
    // 跑全量 DDL，越密只是越频繁地开库
    let steps: Vec<u64> = (0..12).map(next_poll_step).collect();
    assert_eq!(steps[0], 1_500, "首档 1500ms");
    assert_eq!(steps[9], 600_000, "阶梯末档 10 分钟");
    assert_eq!(steps[11], 600_000, "出阶梯后封顶不增长");
    for w in steps.windows(2) {
        assert!(w[0] <= w[1], "步长不递减：{steps:?}");
    }
    // 上限与末档同档：预算给满时兜底阶梯不会越过预算本身
    assert_eq!(
        next_poll_step(CHECK_POLL_LADDER_MS.len() - 1),
        CHECK_WAIT_MS_CAP
    );
}
#[tokio::test]
async fn wait_returns_at_once_when_already_terminal() {
    let (state, exit) = wait_until_terminal(
        || async { Ok::<_, CommandError>(fake_state(SubagentStatus::Succeeded)) },
        CHECK_WAIT_MS_CAP,
        None,
    )
    .await
    .unwrap();
    assert_eq!(exit, WaitExit::Terminal);
    assert_eq!(state.status, SubagentStatus::Succeeded);
}
#[tokio::test]
async fn wait_wakes_on_finish_event_instead_of_burning_budget() {
    // 终态事件必须立刻唤醒等待方：预算给满 30s，若退化成纯兜底轮询，
    // 这个用例会白等一个封顶步长（3s）才返回。
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        done2.store(true, std::sync::atomic::Ordering::SeqCst);
        finish_notify().notify_waiters();
    });
    let started = std::time::Instant::now();
    let (state, exit) = wait_until_terminal(
        || async {
            let terminal = done.load(std::sync::atomic::Ordering::SeqCst);
            Ok::<_, CommandError>(fake_state(if terminal {
                SubagentStatus::Succeeded
            } else {
                SubagentStatus::Running
            }))
        },
        CHECK_WAIT_MS_CAP,
        None,
    )
    .await
    .unwrap();
    assert_eq!(exit, WaitExit::Terminal);
    assert_eq!(state.status, SubagentStatus::Succeeded);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "没被终态事件唤醒，等了 {:?}",
        started.elapsed()
    );
}
#[tokio::test]
async fn wait_returns_early_when_user_stops() {
    // 预算上限已到 30s，而 /stop 只在轮次边界检查——等待循环不自己查令牌，
    // 停止键就会锁死整个等待时长。
    let stop = crate::bot_slash::StopToken::new_for_test();
    stop.stop();
    let started = std::time::Instant::now();
    let (_, exit) = wait_until_terminal(
        || async { Ok::<_, CommandError>(fake_state(SubagentStatus::Running)) },
        CHECK_WAIT_MS_CAP,
        Some(&stop),
    )
    .await
    .unwrap();
    assert_eq!(exit, WaitExit::Stopped);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "/stop 没能打断等待，等了 {:?}",
        started.elapsed()
    );
}
#[tokio::test]
async fn long_budget_does_not_reopen_db_on_every_step() {
    // 阶梯的意义就在这：每次读都要开一次库跑全量 DDL。旧的固定 150ms
    // 步长下 1.5s 预算要读 11 次，递进后只读 2 次（起手就 1500ms）。
    //
    // 预算取 1500ms（正好一个首档）而不是更大：阶梯末档已是分钟级，
    // 预算再大这个用例就得跑好几分钟，而 2 次 vs 11 次已足够判别。
    // 合并了原先「超时也带回当前状态」那条：断的仍是同一行预算判定。
    let reads = std::cell::Cell::new(0usize);
    let started = std::time::Instant::now();
    let (state, exit) = wait_until_terminal(
        || {
            reads.set(reads.get() + 1);
            async { Ok::<_, CommandError>(fake_state(SubagentStatus::Running)) }
        },
        1_500,
        None,
    )
    .await
    .unwrap();
    assert_eq!(exit, WaitExit::Timeout);
    assert_eq!(
        state.status,
        SubagentStatus::Running,
        "超时也要带回当前状态"
    );
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(1_500),
        "预算没走满就返回了：{:?}",
        started.elapsed()
    );
    // 上界留 3 而不是 2：第二次读若因计时抖动落在 deadline 前几百毫秒，会
    // 多走一档 3000ms——那是无害的，判别力仍远强于固定步长的 11 次。
    // finish_notify 是进程级信号，别的子 agent 收尾也会唤醒本等待方。
    assert!(
        reads.get() <= 3,
        "1.5s 预算读了 {} 次，阶梯没生效",
        reads.get()
    );
}
/// 并发闸状态机（合并为单测：static GATE 进程级共享，cargo 并行跑两个测试
/// 会互相踩计数；手动构造票据，不调 wait_slot——它会与其他并行测试竞争
/// 全局槽导致排队挂死）。
#[test]
fn gate_state_machine_acquire_release_and_queue_position() {
    let before_running = gate().state.lock().unwrap().global_running;
    // ① 拿槽 → 计数 +1、per_session 登记、未排队
    {
        let t = Some(GateTicket {
            parent_session: Some("ps_drop".into()),
        });
        // 手动占槽（wait_slot 锁内路径的等价操作）
        {
            let mut st = gate().state.lock().unwrap();
            st.global_running += 1;
            *st.per_session.entry("ps_drop".into()).or_insert(0) += 1;
        }
        {
            let st = gate().state.lock().unwrap();
            assert_eq!(st.global_running, before_running + 1, "拿槽 +1");
            assert_eq!(st.per_session.get("ps_drop"), Some(&1));
        }
        // queue_position 也拿 gate 锁（非递归 mutex），必须在持锁块外调用，
        // 否则同线程二次加锁自死锁
        assert_eq!(SubagentGate::queue_position(Some("ps_drop")), None);
        // ② Drop → 释放：计数回基线、归零清键
        drop(t);
    }
    {
        let st = gate().state.lock().unwrap();
        assert_eq!(st.global_running, before_running, "Drop 必须释放槽位");
        assert!(!st.per_session.contains_key("ps_drop"), "计数归零须清键");
    }
    // ③ 多持多放对称：两张票据只放一张 → 残 1；全放 → 清键
    {
        let t1 = Some(GateTicket {
            parent_session: Some("ps_rel".into()),
        });
        let t2 = GateTicket {
            parent_session: Some("ps_rel".into()),
        };
        {
            let mut st = gate().state.lock().unwrap();
            st.global_running += 2;
            *st.per_session.entry("ps_rel".into()).or_insert(0) += 2;
        }
        drop(t2);
        assert_eq!(
            gate().state.lock().unwrap().per_session.get("ps_rel"),
            Some(&1),
            "放一张残 1"
        );
        drop(t1);
    }
    {
        let st = gate().state.lock().unwrap();
        assert_eq!(st.global_running, before_running, "全放回基线");
        assert!(!st.per_session.contains_key("ps_rel"), "归零清键");
    }
    // ④ queue_position：同会话等待计数语义——前面还有 waiting-1 个
    {
        let mut st = gate().state.lock().unwrap();
        *st.waiting.entry("ps_q1".into()).or_insert(0) += 2;
        *st.waiting.entry("ps_q2".into()).or_insert(0) += 1;
    }
    assert_eq!(SubagentGate::queue_position(Some("ps_q1")), Some(1));
    assert_eq!(SubagentGate::queue_position(Some("ps_q2")), Some(0));
    assert_eq!(SubagentGate::queue_position(Some("ps_not_waiting")), None);
    {
        let mut st = gate().state.lock().unwrap();
        st.waiting.clear();
    }
    assert_eq!(SubagentGate::queue_position(Some("ps_q1")), None, "清场");
}
/// U20D 批 5：子 agent 会话的身份由 bot_sessions.is_subagent 结构化字段
/// 承载，路由契约从「标题 🧩 前缀」升级为该字段。此处锁定：
/// ① runner setup 必须经 subagent 变体建会话（漂移即测试红）；
/// ② 标题已去 emoji 前缀（路由不再依赖标题）。
///
/// 这一列用于「会话列表不列子 agent 会话」与库内判定；活会话的判定走
/// tool_guard 注册表，前端拿到的 Session.isSubagent 恒为 false。
#[test]
fn subagent_sessions_flagged_structurally_not_by_title() {
    // concat! 拼接防本测试自匹配（dead_command_tests 同款手法）
    let wiring = concat!(
        "bot_session_create_",
        "subagent_inner(&tx, Some(title.clone()))"
    );
    assert!(
        orch_prod_src().contains(wiring),
        "runner 建会话必须走 subagent 变体（is_subagent=1 结构化标记）"
    );
    assert!(
        !orch_prod_src().contains(concat!("🧩 子", "任务：")),
        "标题已去 emoji 前缀（路由不依赖标题；子 agent 身份由 is_subagent 字段承载）"
    );
}
#[test]
fn digest_reports_terminal_rows_and_only_absorbs_those() {
    let rows = vec![
        digest_row("sa_ok", SubagentStatus::Succeeded, Some(OK_RESULT)),
        digest_row("sa_running", SubagentStatus::Running, None),
        digest_row("sa_fail", SubagentStatus::Failed, None),
    ];
    let d = build_subagent_digest(&rows, &std::collections::HashSet::new())
        .expect("有未汇报的子任务就该出段");
    assert!(d.text.contains("覆盖 5 个竞品"), "摘要要进段：{}", d.text);
    assert!(
        d.text.contains("report.md（竞品对比）"),
        "产物路径与说明都要进段：{}",
        d.text
    );
    assert!(
        d.text.contains("→ 失败：LLM 5xx"),
        "失败必须回报（否则父 agent 会一直等）：{}",
        d.text
    );
    assert!(
        d.text.contains("1 个子任务仍在排队或执行中"),
        "在途要报计数：{}",
        d.text
    );
    assert!(
        !d.text.contains("目标-sa_running"),
        "在途不逐条铺开：{}",
        d.text
    );
    // 只有终态被吸收：在途行下轮还要看它完没完
    assert_eq!(d.absorb, vec!["sa_ok".to_string(), "sa_fail".to_string()]);
}
#[test]
fn digest_is_idempotent_across_turns() {
    let rows = vec![digest_row(
        "sa_ok",
        SubagentStatus::Succeeded,
        Some(OK_RESULT),
    )];
    let first = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
    let mut set = std::collections::HashSet::new();
    for id in &first.absorb {
        set.insert(id.clone());
    }
    assert!(
        build_subagent_digest(&rows, &set).is_none(),
        "同一份结果第二轮不得再念一遍"
    );
    assert!(
        build_subagent_digest(&[], &set).is_none(),
        "没有子任务时不该出段"
    );
}
#[test]
fn digest_never_injects_child_conversation_history() {
    // §3.7 要点 3：父 payload 只注 result_json，不注子会话历史。
    // 段里出现「system」「assistant」这类角色标记或子会话 id 即为破线。
    let rows = vec![digest_row(
        "sa_ok",
        SubagentStatus::Succeeded,
        Some(OK_RESULT),
    )];
    let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
    assert!(
        !d.text.contains("sub-sa_ok"),
        "子会话 id 不该出现：{}",
        d.text
    );
    assert!(
        !d.text.contains("\"role\""),
        "不得把子会话消息行当 payload 注入：{}",
        d.text
    );
}
#[test]
fn digest_caps_items_and_summary_length() {
    // 一次注入不能把本轮对话挤掉：条数与摘要长度都要封顶。
    let long = "长".repeat(DIGEST_MAX_SUMMARY_CHARS * 3);
    let result = format!(
        r#"{{"summary":"{long}","artifacts":[{{"path":"a.md"}},{{"path":"b.md"}},{{"path":"c.md"}},{{"path":"d.md"}},{{"path":"e.md"}},{{"path":"f.md"}}]}}"#
    );
    let rows: Vec<SubagentRow> = (0..DIGEST_MAX_ITEMS + 5)
        .map(|i| {
            digest_row(
                &format!("sa_{i}"),
                SubagentStatus::Succeeded,
                Some(result.as_str()),
            )
        })
        .collect();
    let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
    assert_eq!(
        d.absorb.len(),
        DIGEST_MAX_ITEMS,
        "超出上限的行不注入（也不吸收，否则它永远进不了下一轮）"
    );
    let summary_line = d
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("摘要："))
        .expect("有摘要行");
    assert!(
        summary_line.chars().count() <= DIGEST_MAX_SUMMARY_CHARS + 8,
        "摘要必须被截断：{} 字",
        summary_line.chars().count()
    );
    let artifacts_line = d
        .text
        .lines()
        .find(|l| l.trim_start().starts_with("产物："))
        .expect("有产物行");
    assert!(
        !artifacts_line.contains("f.md"),
        "产物条数封顶：{artifacts_line}"
    );
}
/// 失败/超预算行必须摆出**残留产出**。
///
/// 父 agent 此前只看到「失败」二字，判断不了「该不该带残留重试」——
/// 于是从零重烧一遍。跑了几十分钟查到的东西就在 result_json.summary 里，
/// 不摆出来等于没有。
#[test]
fn digest_shows_residual_output_for_failed_rows() {
    let rows = vec![digest_row(
        "sa_failed",
        SubagentStatus::BudgetExceeded,
        Some(r#"{"summary":"已核到 3 家供应商报价单，还差比价","note":"墙钟熔断"}"#),
    )];
    let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
    assert!(
        d.text.contains("残留产出："),
        "失败行必须带残留产出：{}",
        d.text
    );
    assert!(
        d.text.contains("3 家供应商"),
        "残留产出的内容要能看到：{}",
        d.text
    );
}
/// 成功行的**机检未过项**必须摆出来。
///
/// 「成功」与「机检没过」两件事同时成立时，父 agent 只看到「成功」就会把
/// 没过的项一起收下。只认服务端核验过的（machine=true 且 met=false）——
/// 模型自报的 met:false 是自我评价，混进来会让父 agent 分不清。
#[test]
fn digest_shows_machine_unmet_criteria() {
    let result = r#"{"status":"succeeded","acceptance_check":[
            {"criterion":"file_exists:调研.md","met":true,"machine":true},
            {"criterion":"file_nonempty:数据.xlsx","met":false,"machine":true},
            {"criterion":"内容符合用户口径","met":false}
        ]}"#;
    let rows = vec![digest_row(
        "sa_machine",
        SubagentStatus::Succeeded,
        Some(result),
    )];
    let d = build_subagent_digest(&rows, &std::collections::HashSet::new()).unwrap();
    assert!(
        d.text.contains("机检未过"),
        "成功行必须带机检未过项：{}",
        d.text
    );
    assert!(
        d.text.contains("file_nonempty:数据.xlsx"),
        "未过项要指名道姓：{}",
        d.text
    );
    assert!(
        !d.text.contains("内容符合用户口径"),
        "模型自报的未过项不摆（服务端没核验过）：{}",
        d.text
    );
}
#[test]
fn digest_survives_malformed_result_json() {
    // 收尾 JSON 解析失败当「没有结果」：宁可少报一条，也不把半截 JSON
    // 念进提示词。行仍要被吸收——否则它每轮都重来一次。
    let rows = vec![digest_row(
        "sa_bad",
        SubagentStatus::Succeeded,
        Some("{半截"),
    )];
    let d =
        build_subagent_digest(&rows, &std::collections::HashSet::new()).expect("行本身仍要出段");
    assert!(!d.text.contains("半截"), "坏 JSON 不得进段：{}", d.text);
    assert!(d.text.contains("→ 成功"));
    assert_eq!(d.absorb, vec!["sa_bad".to_string()]);
}
#[test]
fn malformed_result_json_is_not_wrapped_and_carried_on() {
    // 直接钉函数契约：解析失败返回 None。
    // 只在渲染输出上断言「文本里没有坏 JSON」是不够的——把失败包成
    // `{"raw": ...}` 继续往下走的写法同样能让输出里搜不到坏串，
    // 那样原始文本就还在数据流里（且总有一天会换个字段被渲染出来）。
    assert!(
        parse_result_json("{半截").is_none(),
        "解析失败必须返回 None，不得包装后继续"
    );
    assert!(
        parse_result_json(OK_RESULT).is_some(),
        "合法收尾 JSON 要能解析出来"
    );
    assert!(
        parse_result_json("[]").is_some(),
        "合法但结构不是对象也算解析成功（取字段时自然落空，不该当语法错）"
    );
}
/// 接线守卫：回报段的注入点必须在消息组装的**末尾**（历史之后）。
///
/// 放在摘要/记忆块那段之前，模型看到的是「先回报后用户问题」，权重结构
/// 反了；放在更早还会被后续逻辑改写。匹配串 `concat!` 拆分防自匹配。
#[test]
fn digest_is_injected_at_the_end_of_message_assembly() {
    let chat = include_str!("../../bot_chat.rs");
    let at = chat
        .find(concat!(
            "crate::bot_orchestrator::",
            "subagent_digest_block(&app, session_id.as_deref())"
        ))
        .expect("组装末尾必须调 subagent_digest_block");
    let history_end = chat
        .find("msgs.push(serde_json::json!({\"role\": role, \"content\": m.content}))")
        .expect("历史消息循环的收尾 push");
    // 模型循环锚点必须在注入点**之后**找：bot_chat.rs 前面还有若干处提到
    // run_model_loop 的注释/别名，全文件搜第一个会命中它们。
    let runs_loop = chat[at..]
        .find("run_model_loop(")
        .map(|i| i + at)
        .expect("注入点之后应有模型循环调用");
    assert!(
        history_end < at && at < runs_loop,
        "回报段必须夹在历史消息与模型循环之间（现 history_end={history_end} \
             digest={at} loop={runs_loop}）"
    );
    assert!(
        chat[at..].contains("\"role\": \"system\""),
        "回报段作为 system 消息追加"
    );
}
