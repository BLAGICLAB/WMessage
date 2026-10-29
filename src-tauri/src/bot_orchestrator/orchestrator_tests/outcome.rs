use super::super::*;
use super::*;
use crate::bot::subagent_digest::bot_session_exists_locked;
use crate::bot::subagent_digest::build_subagent_digest;
use crate::bot::subagent_digest::is_digested;
use crate::bot::subagent_digest::MachineVerdict;
use crate::bot::subagent_repair::json_object_spans;
use crate::bot::subagent_runner::render_task_wrapper;
use crate::bot::subagent_spawn::spawn_subagent_locked;
use crate::bot::subagent_tools::own_card_payload;

/// `temperature` 是 yaml 里长期没有消费者的字段（已解析、已落盘，就是没人用）。
/// 这条把它钉成「真的进了模型循环」：优先级是配置档 > 模型条目。
#[test]
fn onion_temperature_reaches_the_model_loop() {
    let body = orch_prod_src();
    let runner = body
        .split_once("let first = crate::bot_model_loop::run_model_loop(")
        .expect("子 agent 调模型循环")
        .1
        .split(";")
        .next()
        .expect("调用应在此处收尾");
    // 两个 token 分开断言而不是拼成 `bound_onion.as_ref()`：rustfmt 会按行宽
    // 把链式调用拆行，拼出来的串匹配不到——守卫被格式器牵着走。
    assert!(
        runner.contains("bound_onion"),
        "子 agent 必须把配置档的温度传进模型循环：{runner}"
    );
    assert!(
        runner.contains("c.temperature"),
        "传的是配置档的 temperature 字段：{runner}"
    );
}
/// tool_calls 计数器（ctx 侧）——dispatch 累计语义的单测锚点（纯仪表，强制在模型循环层）
#[test]
fn tool_call_counter_accumulates() {
    let ctx = crate::tool_guard::SubagentSessionCtx {
        subagent_id: "sa_c1".into(),
        task_id: None,
        profile: SubagentProfile::Coder,
        tools: std::sync::Arc::new(
            crate::bot::registry::profile_whitelist(SubagentProfile::Coder).to_vec(),
        ),
        artifact_dir: std::path::PathBuf::from("/tmp/art_c1"),
        used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        last_progress_ms: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };
    assert_eq!(
        ctx.used_tool_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        ctx.used_tool_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        1
    );
}
#[test]
fn extract_last_json_takes_the_last_object() {
    // 纯 JSON
    assert_eq!(
        json_object_spans(r#"{"a":1}"#),
        vec![r#"{"a":1}"#.to_string()],
        "纯 JSON 应切出一个顶层对象"
    );
    // 散文夹 JSON + 多对象 → 取最后一个
    let t = r#"结论如下 {"status":"succeeded","summary":"第一步"} 中间 {"status":"succeeded","summary":"最终"}"#;
    let got = parse_final_json(t).expect("多对象时取最后一个");
    assert_eq!(got["summary"], "最终");
    // 字符串内花括号不破坏扫描
    let t2 = r#"{"summary":"带 } 花括号与 \" 引号"}"#;
    assert!(parse_final_json(t2).is_some());
    // 无 JSON
    assert!(parse_final_json("没有任何大括号的普通回复").is_none());
}
#[test]
fn classify_outcome_maps_all_paths() {
    use SubagentStatus::*;
    // 轮数熔断 → budget_exceeded（标记在正文里，出口见 bot_model_loop 熔断）
    let fused = crate::bot_model_loop::rounds_fuse_message("上轮正文", "", 20);
    let (st, err, _) = classify_outcome(None, Some(&fused));
    assert_eq!(st, BudgetExceeded);
    assert_eq!(err.as_deref(), Some("max_turns"));
    // 其他错误 → failed
    let (st, _, _) = classify_outcome(Some("HTTP 500"), None);
    assert_eq!(st, Failed);
    // 解析成功 + 模型自报 succeeded
    let (st, err, res) =
        classify_outcome(None, Some(r#"结论 {"status":"succeeded","summary":"ok"}"#));
    assert_eq!(st, Succeeded);
    assert_eq!(err, None);
    assert_eq!(res.unwrap()["summary"], "ok");
    // 模型自报 failed → failed + 说明
    let (st, err, _) = classify_outcome(None, Some(r#"{"status":"failed","summary":"x"}"#));
    assert_eq!(st, Failed);
    assert_eq!(err.as_deref(), Some("model_reported_failure"));
    // 解析失败 → failed + result_json_unparseable + 原文存 summary（部分结果不丢）
    let (st, err, res) = classify_outcome(None, Some("纯文本收尾，没有 JSON"));
    assert_eq!(st, Failed);
    assert_eq!(err.as_deref(), Some("result_json_unparseable"));
    let res = res.unwrap();
    assert!(res["summary"].as_str().unwrap().contains("纯文本收尾"));
}
/// read_own_card 的返回体：budgetUsage 一块含全部实时余量（上限取共享安全网），
/// 静态 budget 键不再单独出现（两份会让模型不知道看哪个）。
#[test]
fn own_card_payload_reports_live_budget_usage() {
    let row = crate::db::subagents::SubagentRow {
        id: "sa_x".into(),
        status: SubagentStatus::Running,
        profile: SubagentProfile::Research,
        model: None,
        parent_session_id: None,
        session_id: None,
        task_id: None,
        objective: "调研 X".into(),
        trace_id: "trc_x".into(),
        acceptance_json: Some(r#"["覆盖 X"]"#.into()),
        context_summary: None,
        result_json: None,
        error: None,
        created_at: 0,
        started_at: None,
        finished_at: None,
        prompt_tokens: None,
        completion_tokens: None,
        cache_read_tokens: None,
    };
    let v = own_card_payload(&row, vec!["覆盖 X".into()], 47, 100, 12, 50);
    assert_eq!(v["budgetUsage"]["toolCallsUsed"], 47);
    assert_eq!(v["budgetUsage"]["toolCallsMax"], 100);
    assert_eq!(v["budgetUsage"]["roundsUsed"], 12);
    assert_eq!(v["budgetUsage"]["roundsMax"], 50);
    assert!(
        v.get("budget").is_none(),
        "静态 budget 键已并入 budgetUsage，不得再单独出现"
    );
}
/// 轮数熔断必须保住已跑完的那十几轮产出。
///
/// 回归背景：recon 子任务 20 轮用尽、28 次工具调用查完后熔断，终态
/// budget_exceeded/max_turns 而 result_json 是 NULL——产出整段蒸发，父 agent
/// 回灌拿不到 summary，重试只能从零再来（设计 §6「部分结果不丢弃」）。
#[test]
fn classify_outcome_keeps_partial_output_on_rounds_fuse() {
    use SubagentStatus::*;
    let partial = "查到 pp-ocr-v6/ 下有 det.onnx / cls.onnx / rec.onnx + keys.txt";
    let fused = crate::bot_model_loop::rounds_fuse_message(partial, "", 20);
    let (st, err, res) = classify_outcome(None, Some(&fused));
    assert_eq!(st, BudgetExceeded);
    assert_eq!(err.as_deref(), Some("max_turns"));
    let res = res.expect("轮数熔断不得写 NULL result_json");
    let summary = res["summary"].as_str().expect("熔断行必须带 summary");
    assert!(
        summary.contains("det.onnx"),
        "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
    );
}
/// 工具熔断必须落 budget_exceeded(max_tool_calls)。
///
/// 此前 classify_outcome 只认轮数熔断标记，工具熔断（另一个常量）落不进
/// 预算分支：模型照办输出 {"status":"succeeded"} 就报成功，父 agent 因此
/// 不重试——预算烧满、产出残缺，却记成一次干净的成功。
#[test]
fn classify_outcome_treats_tool_fuse_as_budget_exceeded() {
    use SubagentStatus::*;
    let partial = "查到 3 个候选包，还差最后一步没跑";
    let fused = crate::bot_model_loop::fuse_message(partial, "", 100);
    let (st, err, res) = classify_outcome(None, Some(&fused));
    assert_eq!(st, BudgetExceeded, "工具熔断必须落预算触达");
    assert_eq!(err.as_deref(), Some("max_tool_calls"));
    let res = res.expect("工具熔断不得写 NULL result_json");
    let summary = res["summary"].as_str().expect("熔断行必须带 summary");
    assert!(
        summary.contains("3 个候选包"),
        "熔断前的正文应存进 summary 供父 agent 判断是否重试：{summary}"
    );

    // 轮数熔断仍归 max_turns（轮数标记是工具标记的前缀，判据不能反）
    let rounds = crate::bot_model_loop::rounds_fuse_message(partial, "", 20);
    let (st, err, _) = classify_outcome(None, Some(&rounds));
    assert_eq!(st, BudgetExceeded);
    assert_eq!(err.as_deref(), Some("max_turns"));
}
/// 收尾 JSON 的本地修复（结构化输出三段修复的第 2 段）：零 token、零延迟，
/// 必须排在补答之前——补答要再走一次 LLM 轮次。
#[test]
fn parse_final_json_repairs_trailing_comma_and_truncation() {
    // 尾逗号
    let v = parse_final_json(r#"{"status":"succeeded","summary":"好了",}"#)
        .expect("尾逗号应被本地修复救回");
    assert_eq!(v["status"], "succeeded");
    // 被截断（写到一半没了）
    let v = parse_final_json(r#"{"status":"succeeded","summary":"调研完成，报告"#)
        .expect("截断的 JSON 应被补闭合救回");
    assert_eq!(v["status"], "succeeded");
    // markdown 围栏（既有能力，顺带锁住）
    let v = parse_final_json("```json\n{\"status\":\"failed\"}\n```").expect("围栏应能取到");
    assert_eq!(v["status"], "failed");
}
/// 本地修复不得改坏字符串内容：模型写的正文里出现逗号/花括号是合法的。
#[test]
fn parse_final_json_does_not_corrupt_string_content() {
    let raw = r#"{"status":"succeeded","summary":"结论：a, b, c"}"#;
    let v = parse_final_json(raw).expect("正常 JSON 必须能解析");
    assert_eq!(v["summary"], "结论：a, b, c");
    // 字符串里的尾逗号形态不能被当成尾逗号删掉
    let raw = r#"{"status":"succeeded","summary":"结尾带逗号,"}"#;
    let v = parse_final_json(raw).expect("字符串里的逗号不得被改");
    assert_eq!(v["summary"], "结尾带逗号,");
}
/// 散文收尾仍然解析不出来——修复不是万能的，解析失败要如实落失败。
#[test]
fn parse_final_json_leaves_prose_unparseable() {
    assert!(parse_final_json("纯文本收尾，没有 JSON").is_none());
    assert!(parse_final_json("").is_none());
}
/// 截断的 JSON 修出来若**认不出收尾语义**（没有 status / checks），
/// 不得认成收尾结果——判定里「没有 status」默认按 succeeded 算，
/// 把「没说完」说成「说完了」。
#[test]
fn parse_final_json_rejects_truncated_fragment_without_status() {
    assert!(
        parse_final_json(r#"{"summary":"写到一半断了"#).is_none(),
        "半截 JSON 没有 status，不得被认成收尾结果"
    );
    // 有 status 的截断件认得出来（模型已经表明了结论）
    assert!(
        parse_final_json(r#"{"status":"succeeded","summary":"写到一半断了"#).is_some(),
        "已有 status 的截断件应被救回"
    );
}
#[test]
fn machine_pass_overrides_unparseable_finish() {
    let verdicts = vec![
        MachineVerdict {
            criterion: "file_exists:调研.md".into(),
            met: true,
            reason: "服务端核验：存在（13780 字节）".into(),
        },
        MachineVerdict {
            criterion: "file_nonempty:调研.md".into(),
            met: true,
            reason: "服务端核验：存在（13780 字节）".into(),
        },
    ];
    let (st, err, res) = classify_outcome(None, Some("散文收尾，没写 JSON"));
    assert_eq!(st, SubagentStatus::Failed, "起点是解析失败");
    assert!(res
        .expect("原文必须留在 result_json")
        .get("summary")
        .and_then(|v| v.as_str())
        .unwrap()
        .contains("散文收尾"));

    let (st2, err2, recovered) = reconcile_machine_override(st, err, &verdicts);
    assert_eq!(st2, SubagentStatus::Succeeded);
    assert!(recovered);
    // 推翻这件事本身要留痕：error 上保留解析失败标记并补出理由
    let e = err2.expect("推翻后仍须留标记");
    assert!(e.starts_with(UNPARSEABLE_MARKER), "{e}");
    assert!(e.contains("2 项"), "{e}");
}
/// 边界：机器全过只推翻「解析失败」这一种。模型自报失败说的可能是服务端
/// 验不了的自由文本条目；一条机器结论都没有时服务端零证据；验了没过更是
/// 实打实的不合格——这三种都必须维持失败。
#[test]
fn machine_pass_overrides_nothing_else() {
    let all_met = vec![MachineVerdict {
        criterion: "file_exists:a.md".into(),
        met: true,
        reason: "存在".into(),
    }];
    for (st, err) in [
        (
            SubagentStatus::Failed,
            Some("model_reported_failure".to_string()),
        ),
        (SubagentStatus::Failed, Some("HTTP 500".to_string())),
        (
            SubagentStatus::BudgetExceeded,
            Some("max_turns".to_string()),
        ),
    ] {
        let (s2, e2, recovered) = reconcile_machine_override(st, err.clone(), &all_met);
        assert_eq!(s2, st);
        assert_eq!(e2, err);
        assert!(!recovered);
    }
    // 零证据（一条机器标准都没声明）→ 维持失败
    let (s2, _, r2) =
        reconcile_machine_override(SubagentStatus::Failed, Some(UNPARSEABLE_MARKER.into()), &[]);
    assert_eq!(s2, SubagentStatus::Failed);
    assert!(!r2);
    // 验了没过 → 维持失败
    let unmet = vec![MachineVerdict {
        criterion: "file_exists:a.md".into(),
        met: false,
        reason: "产物不存在".into(),
    }];
    let (s3, _, r3) = reconcile_machine_override(
        SubagentStatus::Failed,
        Some(UNPARSEABLE_MARKER.into()),
        &unmet,
    );
    assert_eq!(s3, SubagentStatus::Failed);
    assert!(!r3);
}
/// 接线守卫：机器验收的修正必须落在终态快照之前。判定顺序反了的话，
/// 「机器验收通过」与「行状态 failed」就会永远同时成立——那正是本次修的 bug。
#[test]
fn machine_override_runs_before_status_snapshot() {
    let src = orch_prod_src();
    let fix = src
        .find("reconcile_machine_override(status, error,")
        .expect("finalize 必须调用 reconcile_machine_override");
    let snapshot = src.find("let status_for_db = status;").expect("终态快照点");
    assert!(fix < snapshot, "修正必须早于落库快照");
}
/// 「问过即已吸收」：`check_subagent` 拿到终态后结果已在父上下文里，
/// digest 与自动汇报轮都不该再讲一遍。去重表按会话隔离，别把 A 会话的
/// 结果算到 B 会话头上。
#[test]
fn digest_mark_is_per_session() {
    let sid = format!("dm-{}", uuid::Uuid::new_v4());
    assert!(!is_digested(&sid, "sa_a"));
    mark_digested(Some(&sid), "sa_a");
    assert!(is_digested(&sid, "sa_a"));
    assert!(
        !is_digested(&format!("{sid}-other"), "sa_a"),
        "去重表按会话隔离"
    );
    mark_digested(None, "sa_b");
    assert!(!is_digested(&sid, "sa_b"), "无会话 id 不得污染任何会话");

    // 回报段的吸收是两步：先构造（不碰已吸收集合），模型循环成功后才提交。
    // 构造即提交是「digest 失败即丢」的根因——循环失败时这段回报压根没送进
    // 模型，却已被记成已吸收，汇报轮从此不再重提这条子任务结果。
    let sid2 = format!("dmc-{}", uuid::Uuid::new_v4());
    let rows = vec![digest_row(
        "sa_x",
        SubagentStatus::Succeeded,
        Some(OK_RESULT),
    )];
    let built = build_subagent_digest(&rows, &std::collections::HashSet::new())
        .expect("有未汇报的终态子任务就该出段");
    assert_eq!(built.absorb, vec!["sa_x".to_string()]);
    assert!(
        !is_digested(&sid2, "sa_x"),
        "构造回报段不构成吸收——模型循环还没成功"
    );
    for id in &built.absorb {
        mark_digested(Some(&sid2), id);
    }
    assert!(is_digested(&sid2, "sa_x"), "提交后才算已吸收");
}
/// 接线守卫：汇报轮失败不能直接放弃——那等于这条子任务结果永远没人汇报。
///
/// 失败与 Busy 同款处理：deadline 内 sleep 后再试（digest 此时也没记成已
/// 吸收，下一轮还有机会把结果说出去）。
#[test]
fn report_failure_is_retried_instead_of_given_up() {
    let src = orch_prod_src();
    // 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）
    assert!(
        !src.contains(concat!(
            "ReportOutcome::Ran | crate::bot_chat::",
            "ReportOutcome::Failed"
        )),
        "汇报失败不能和成功同款直接 return"
    );
    assert!(
        src.contains(concat!(
            "ReportOutcome::Failed | crate::bot_chat::",
            "ReportOutcome::Busy"
        )),
        "汇报失败要与 Busy 同款：deadline 内 sleep 后重试"
    );
}
/// 接线守卫：兜底写终态失败必须留痕。
///
/// 兜底写的是「行停在 queued/running」的唯一补救；它自己失败时若不记，
/// 那行就永远停在那里，面板上一直转圈，事后也查不到是哪一步失败的。
/// 匹配串用 `concat!` 拆分防自匹配（本测试自身的源码不算命中）。
#[test]
fn terminal_state_fallback_write_leaves_a_trace_on_failure() {
    let src = orch_prod_src();
    assert!(
        !src.contains(concat!("let _ = db_", "locked(")),
        "兜底写终态不得吞掉失败——行会永远停在 queued/running"
    );
    let write_needle = concat!("let write = db_", "locked(");
    let trace_needle = concat!("subagent_terminal_state_write_", "failed");
    // 按「兜底写」切段：每段到下一个兜底写为止，每段都要有自己的失败留痕
    let segments: Vec<&str> = src.split(write_needle).skip(1).collect();
    assert!(
        segments.len() >= 4,
        "兜底写终态有四处（加载失败 / 产物目录不可用 / 会话建立失败 / 收尾失败），\
             现 {len} 处",
        len = segments.len()
    );
    for seg in &segments {
        assert!(
            seg.contains(trace_needle),
            "兜底写终态失败必须留痕——行永远停在 queued/running 时只有这条线索"
        );
    }
}
/// 接线守卫：`subagent_digest_block` 只造段，不得顺手记「已吸收」。
///
/// 它被模型循环**之前**调用——在这里记，等于把「这段回报已经说过了」提前
/// 宣布；循环失败时它压根没送进模型，汇报轮也会因此以为说过、从此不再
/// 重提，子任务结果在本次进程内永久消失。
#[test]
fn digest_block_builds_without_absorbing() {
    let src = orch_prod_src();
    let start = src
        .find("pub(crate) async fn subagent_digest_block(")
        .expect("要有 subagent_digest_block");
    let rest = &src[start..];
    let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
    let body = &rest[..end];
    assert!(
        !body.contains("set.insert"),
        "回报段的构造函数里不得记已吸收——记动作在模型循环成功之后"
    );
    assert!(
        body.contains("build_subagent_digest("),
        "构造函数仍要调 build_subagent_digest 出段"
    );
}
/// 接线守卫：「已吸收」的标记不在 `bot_chat_inner` 里做。
///
/// 这一层既不知道送达结果、也不该替调用方决定：交互聊天（前端驱动）在
/// 命令返回前记（模型确实看到了那段回报），汇报轮（后端驱动）要等落库
/// 成功才记（它的历史下一轮从库里读，没落库的回复等于没说出去）。在这里
/// 无条件记，等于把「已经说过了」提前宣布——落库失败时那段回报从此不再
/// 进 digest，子任务结果在本次进程内永久消失。
#[test]
fn digest_absorb_mark_is_left_to_the_caller() {
    let chat = include_str!("../../bot_chat/flow.rs");
    let start = chat
        .find("async fn bot_chat_inner(")
        .expect("bot_chat_inner 函数体起点");
    let body = &chat[start..];
    let end = body.find("\n}\n").map(|i| i + 3).unwrap_or(body.len());
    let inner = &body[..end];
    assert!(
        !inner.contains("mark_digested("),
        "bot_chat_inner 里不得记「已吸收」——标记由知道送达结果的调用方做"
    );
}
/// 派发方会话被删后不该再汇报：bot_messages 没有外键约束，硬跑一轮只会给
/// 一个不存在的会话写一条永远读不到的孤儿消息。
#[test]
fn missing_parent_session_blocks_the_report() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE bot_sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                is_subagent INTEGER NOT NULL DEFAULT 0
            );",
    )
    .unwrap();
    assert!(
        !bot_session_exists_locked(&conn, "s-gone"),
        "不存在的会话判为不在"
    );
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    assert!(bot_session_exists_locked(&conn, "s1"));
}
/// 接线守卫：汇报轮的轮次说明要排在 digest 注入之前。digest 放末尾是
/// 刻意的（模型对末尾权重最高），说明插在它前面才不抢这份权重。
#[test]
fn report_turn_prompt_precedes_digest_injection() {
    let src = include_str!("../../bot_chat/flow.rs");
    let prompt = src
        .find("REPORT_TURN_PROMPT}))")
        .expect("汇报轮必须注入轮次说明");
    let digest = src
        .find("subagent_digest_block(&app, session_id.as_deref())")
        .expect("digest 注入点");
    assert!(prompt < digest, "说明应在 digest 之前，末尾留给回报本身");
}
/// verifier 输出 verified.json schema 的判定分支覆盖。
/// checks 数组结构而非 status 字段是 verifier 路径的真正信号源。
#[test]
fn classify_outcome_verifier_paths() {
    use SubagentStatus::*;
    // 全部 met=true → Succeeded
    let (st, err, _) = classify_outcome(
        None,
        Some(r#"{"checks":[{"criterion":"c1","met":true,"evidence":"x"}],"summary":"all ok"}"#),
    );
    assert_eq!(st, Succeeded);
    assert_eq!(err, None);
    // 全部 met=false → Failed + reason 列前 3 项 criterion
    let (st, err, res) = classify_outcome(
        None,
        Some(
            r#"{"checks":[{"criterion":"导出 PDF","met":false,"evidence":"缺依赖"},
                              {"criterion":"样式对齐","met":false,"evidence":"字体不一致"}],
                   "summary":"全部失败"}"#,
        ),
    );
    assert_eq!(st, Failed);
    let err_str = err.unwrap();
    assert!(
        err_str.starts_with("verifier_all_checks_failed:"),
        "actual: {err_str}"
    );
    assert!(err_str.contains("导出 PDF=false"), "actual: {err_str}");
    assert!(err_str.contains("样式对齐=false"), "actual: {err_str}");
    assert_eq!(res.unwrap()["summary"], "全部失败");
    // 部分 met=true → Succeeded（主 agent 看 checks 自己决策）
    let (st, err, _) = classify_outcome(
        None,
        Some(
            r#"{"checks":[{"criterion":"c1","met":true,"evidence":"x"},
                              {"criterion":"c2","met":false,"evidence":"y"}],
                   "summary":"部分通过"}"#,
        ),
    );
    assert_eq!(st, Succeeded);
    assert_eq!(err, None);
    // checks 空 + summary 含「未通过」 → Failed
    let (st, err, _) = classify_outcome(
        None,
        Some(r#"{"checks":[],"summary":"未通过：缺关键证据"}"#),
    );
    assert_eq!(st, Failed);
    let err_str = err.unwrap();
    assert!(
        err_str.starts_with("verifier_summary_rejected:"),
        "actual: {err_str}"
    );
    assert!(err_str.contains("缺关键证据"), "actual: {err_str}");
    // checks 空 + summary 不含否决词 → Succeeded（fail-safe 放行）
    let (st, err, _) = classify_outcome(
        None,
        Some(r#"{"checks":[],"summary":"没有可检查项，跳过验证"}"#),
    );
    assert_eq!(st, Succeeded);
    assert_eq!(err, None);
    // status=succeeded 的普通收尾无 checks → 走现有路径不受影响
    let (st, err, _) = classify_outcome(None, Some(r#"{"status":"succeeded","summary":"ok"}"#));
    assert_eq!(st, Succeeded);
    assert_eq!(err, None);
}
#[test]
fn task_wrapper_contains_all_sections() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let mut r = req(None);
    r.context_summary = Some("上游已确认五个竞品清单".into());
    let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .unwrap();
    let w = render_task_wrapper(&row, std::path::Path::new("/tmp/art"));
    assert!(w.contains("[子任务派发]"));
    assert!(w.contains(&format!("subagent_id: {}", row.id)));
    assert!(w.contains("objective: 调研五个竞品并输出报告"));
    assert!(w.contains("- 覆盖至少 5 个产品"));
    // 上下文摘要取自行上 context_summary（不再从子卡 note 文本解析）
    assert!(w.contains("上下文摘要:\n上游已确认五个竞品清单"));
    // 子 agent 没有独立预算，包装里不再有预算段
    assert!(!w.contains("预算:"));
    assert!(w.contains("产物目录:"));
    assert!(w.contains("/tmp/art"));
    assert!(w.contains("完成后必须输出以下 JSON"));
    assert!(w.contains("\"succeeded|failed|cancelled|budget_exceeded\""));
}
/// 无上下文摘要时包装不带该段（不留空标题噪声）
#[test]
fn task_wrapper_omits_absent_context_summary() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .unwrap();
    let w = render_task_wrapper(&row, std::path::Path::new("/a"));
    assert!(!w.contains("上下文摘要"));
}
