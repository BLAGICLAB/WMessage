use super::super::*;
use super::*;
use crate::bot::subagent_digest::merge_machine_verdicts;
use crate::bot::subagent_digest::parse_machine_criterion;
use crate::bot::subagent_digest::should_report_to_parent;
use crate::bot::subagent_digest::MachineCheck;
use crate::bot::subagent_digest::MachineVerdict;
use crate::bot::subagent_runner::effective_status;
use crate::bot::subagent_spawn::apply_onion_turn_cap;
use crate::bot::subagent_spawn::spawn_subagent_locked;
use crate::bot::subagent_tools::spawn_receipt_text;

#[test]
fn spawn_creates_row_only_and_no_subcard() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let parent = parent_card(&conn);
    let ack = spawn_subagent_locked(&mut conn, &req(Some(parent)), 5_000).unwrap();
    assert!(ack.subagent_id.starts_with("sa_"));
    assert_eq!(ack.status, SubagentStatus::Queued);
    // 不再建卡：行上 task_id 为 NULL，tasks 表除 parent 外无新增
    assert_eq!(ack.task_id, None);
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.task_id, None);
    assert_eq!(row.parent_session_id.as_deref(), Some("main-sess"));
    let cards = crate::db::load_all(&conn).unwrap();
    assert_eq!(cards.len(), 1, "只应有 parent 卡，子 agent 不建卡");
    assert_eq!(cards[0].id, "parent-1");
    // 验收标准与上下文摘要落在行上（机器侧，不再双写 note）
    let criteria: Vec<String> =
        serde_json::from_str(row.acceptance_json.as_deref().unwrap()).unwrap();
    assert_eq!(criteria.len(), 2);
    assert!(row.context_summary.is_none());
    // 预算上表可见（默认值）
    let budget: SubagentBudget = serde_json::from_str(row.budget_json.as_deref().unwrap()).unwrap();
    assert_eq!(budget.max_turns, DEFAULT_MAX_TURNS);
    assert_eq!(budget.max_tool_calls, DEFAULT_MAX_TOOL_CALLS);
    assert_eq!(budget.max_wall_seconds, DEFAULT_MAX_WALL_SECONDS);
}
/// 空白上下文摘要不落库（否则包装会渲染一个空标题段）
#[test]
fn spawn_trims_blank_context_summary_to_none() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let mut r = req(None);
    r.context_summary = Some("   \n  ".into());
    let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.context_summary, None);
}
#[test]
fn spawn_without_parent_card_is_allowed() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    assert!(ack.task_id.is_none());
    assert!(!ack.trace_id.is_empty());
}
/// 重复派发同一份请求不建第二条活行（幂等键：父会话 + 档位 + 目标 +
/// 验收标准 + 指定模型）。
///
/// 模型重复调用 spawn_subagent（同一轮发两次、或隔几轮忘了自己派过）原先
/// 会建两条活行，各拿一份完整预算——同一份活干两遍。
#[test]
fn repeated_spawn_of_the_same_request_reuses_the_live_row() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    assert!(!first.deduped, "首次派发必须是新建");
    let second = spawn_subagent_locked(&mut conn, &req(None), 6_000).unwrap();
    assert!(second.deduped, "重复派发必须命中幂等键");
    assert_eq!(
        second.subagent_id, first.subagent_id,
        "重复派发必须返回同一个子 agent id"
    );
    let rows = crate::db::load_subagents_by_parent(&conn, "main-sess", 100).unwrap();
    assert_eq!(rows.len(), 1, "重复派发不得再建一条活行");
}
/// 已终态的不参与去重：「再跑一次」是重试语义，不能被幂等键吞掉。
#[test]
fn spawn_after_the_previous_one_finished_creates_a_new_row() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    crate::db::update_subagent_status(
        &conn,
        &first.subagent_id,
        SubagentStatus::Cancelled,
        6_000,
        None,
    )
    .unwrap();
    let second = spawn_subagent_locked(&mut conn, &req(None), 7_000).unwrap();
    assert!(!second.deduped, "已终态的旧行不该再命中幂等键");
    assert_ne!(second.subagent_id, first.subagent_id);
    let rows = crate::db::load_subagents_by_parent(&conn, "main-sess", 100).unwrap();
    assert_eq!(rows.len(), 2, "重试应另起一行");
}
/// 验收标准不同 = 不同的活，不去重（模型改需求后重派是正当的）。
#[test]
fn spawn_with_different_acceptance_criteria_is_not_deduped() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let mut other = req(None);
    other.acceptance_criteria = vec!["覆盖至少 8 个产品".into()];
    let second = spawn_subagent_locked(&mut conn, &other, 6_000).unwrap();
    assert!(!second.deduped, "验收标准不同不该命中幂等键");
    assert_ne!(second.subagent_id, first.subagent_id);
}
/// 不同父会话的同目标派发是不同意图，不互相吞掉。
#[test]
fn spawn_from_a_different_parent_session_is_not_deduped() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let first = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let mut other = req(None);
    other.parent_session_id = Some("other-sess".into());
    let second = spawn_subagent_locked(&mut conn, &other, 6_000).unwrap();
    assert!(!second.deduped, "跨会话不命中幂等键");
    assert_ne!(second.subagent_id, first.subagent_id);
}
/// 没有父会话（拿不到会话 id 的调用路径）时不去重：宁可多建一行，
/// 也不要把别人的活吞掉。
#[test]
fn spawn_without_parent_session_is_not_deduped() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let mut r = req(None);
    r.parent_session_id = None;
    let first = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
    let second = spawn_subagent_locked(&mut conn, &r, 6_000).unwrap();
    assert!(!first.deduped && !second.deduped);
    assert_ne!(first.subagent_id, second.subagent_id);
}
/// 幂等键挡住的是「建第二条活行」，挡不住「再起一个 runner」——后者照样
/// 把同一份活跑两遍、各烧一份预算。deduped 的早退必须排在 runner 派发前。
#[test]
fn deduped_spawn_returns_before_the_runner_is_dispatched() {
    let src = orch_prod_src();
    let start = src
        .find("pub async fn spawn_subagent(")
        .expect("bot_orchestrator.rs 应有 spawn_subagent");
    let dedup = start
        + src[start..]
            .find("ack.deduped")
            .expect("spawn_subagent 应有 deduped 早退分支");
    let runner = start
        + src[start..]
            .find("run_subagent(")
            .expect("spawn_subagent 应派发 runner");
    assert!(
        dedup < runner,
        "命中幂等键必须早于 runner 派发返回（否则同一份活跑两遍）"
    );
    assert!(
        src[dedup..runner].contains("return Ok(ack);"),
        "deduped 分支必须直接返回，不得落到 runner 派发"
    );
}
/// 回执文案两种形态：新建说「已派发」，命中幂等键要说清「没新建、
/// 还是那个」——模型照抄「已派发」会把一个子 agent 说成两个。
#[test]
fn spawn_receipt_text_distinguishes_new_from_deduped() {
    let ack = SpawnAck {
        subagent_id: "sa_x".into(),
        task_id: None,
        trace_id: "trc_x".into(),
        status: SubagentStatus::Queued,
        deduped: false,
        budget: SubagentBudget::default(),
    };
    let fresh = spawn_receipt_text(&ack);
    assert!(fresh.contains("已派发子 agent"), "{fresh}");
    assert!(fresh.contains("sa_x"), "{fresh}");
    // 回执必须带生效预算——模型不知道墙钟被钳过，就会按自己给的数安排动作
    assert!(fresh.contains("生效预算"), "{fresh}");

    let deduped = SpawnAck {
        deduped: true,
        ..ack
    };
    let dup = spawn_receipt_text(&deduped);
    assert!(dup.contains("未重复派发"), "{dup}");
    assert!(dup.contains("sa_x"), "{dup}");
    assert!(
        !dup.contains("已派发子 agent"),
        "命中幂等键的回执不得说成新建：{dup}"
    );
}
#[test]
fn spawn_with_missing_parent_card_is_task_not_found() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    // Result 的 OK 侧 SpawnAck 无 Debug——用 match 拆错误，不 unwrap_err
    let err = match spawn_subagent_locked(&mut conn, &req(Some("ghost-card".into())), 5_000) {
        Err(e) => e,
        Ok(_) => panic!("缺卡 spawn 必须失败"),
    };
    match err {
        CommandError::TaskNotFound(id) => assert_eq!(id, "ghost-card"),
        other => panic!("应 TaskNotFound，实际 {other:?}"),
    }
}
/// 机器可判定条目带杂质要在**派发期**就拒。
///
/// 模型写 `file_exists:调研.md 已完成`，服务端把路径当成「调研.md 已完成」，
/// 文件当然不存在——假阴性。此前要等子 agent 跑完十几分钟才暴露，白烧一整轮。
#[test]
fn spawn_rejects_machine_criterion_with_trailing_prose() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let mut r = req(None);
    r.acceptance_criteria = vec!["file_exists:调研.md 已完成".into()];
    let err = match spawn_subagent_locked(&mut conn, &r, 5_000) {
        Ok(_) => panic!("带杂质的机器可判定条目必须当场拒"),
        Err(e) => e,
    };
    let msg = err.message();
    assert!(
        msg.contains("file_exists:report.md"),
        "报错要带正例（模型不知道对的长什么样）：{msg}"
    );
    // 干净的写法照常放行
    let mut ok = req(None);
    ok.acceptance_criteria = vec![
        "file_exists:调研.md".into(),
        "file_nonempty:output/数据.xlsx".into(),
        "内容符合用户口径".into(),
    ];
    assert!(
        spawn_subagent_locked(&mut conn, &ok, 5_000).is_ok(),
        "干净的机器可判定条目不得被拒"
    );
}
#[test]
fn spawn_rejects_blank_objective_and_empty_acceptance() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let mut r = req(None);
    r.objective = "  ".into();
    assert!(spawn_subagent_locked(&mut conn, &r, 5_000).is_err());
    let mut r2 = req(None);
    r2.acceptance_criteria = Vec::new();
    assert!(spawn_subagent_locked(&mut conn, &r2, 5_000).is_err());
    let mut r3 = req(None);
    r3.acceptance_criteria = vec!["  ".into()];
    assert!(spawn_subagent_locked(&mut conn, &r3, 5_000).is_err());
}
#[test]
fn budget_clamps_turns_to_hard_cap() {
    let b = clamp_budget(Some(SubagentBudget {
        max_turns: 9_999,
        max_tool_calls: 0,
        max_wall_seconds: 0,
    }));
    assert_eq!(b.max_turns, MAX_TURNS_HARD_CAP);
    assert_eq!(b.max_tool_calls, 1);
    // 墙钟下界与配置键同源（30s）——「1 秒墙钟」等于白派一个子 agent
    assert_eq!(b.max_wall_seconds, 30);
    // 默认预算 = 均衡档全局池的 20% 份额（引用常量，不写字面量）
    assert_eq!(
        clamp_budget(None),
        SubagentBudget {
            max_turns: DEFAULT_MAX_TURNS,
            max_tool_calls: DEFAULT_MAX_TOOL_CALLS,
            max_wall_seconds: DEFAULT_MAX_WALL_SECONDS
        }
    );
}
/// 墙钟的硬顶必须与配置键走同一组常量——两条路两个闸时，LLM 显式给的
/// `maxWallSeconds=999999` 会原样生效，墙钟实际由调用方说了算。
#[test]
fn budget_wall_clock_is_capped_at_the_same_bound_as_config() {
    let too_big = clamp_budget(Some(SubagentBudget {
        max_turns: 30,
        max_tool_calls: 100,
        max_wall_seconds: 999_999,
    }));
    assert_eq!(
        too_big.max_wall_seconds,
        crate::bot::params::SUBAGENT_MAX_WALL_SECS_MAX
    );
    // 配置键路径给同一个数，钳完必须一模一样
    let via_config = crate::bot::params::clamp_subagent_max_wall_secs(999_999);
    assert_eq!(too_big.max_wall_seconds, via_config);
}
#[test]
fn cancel_sets_cancelled_and_terminal_noop() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let c = cancel_subagent_locked(&conn, &ack.subagent_id, "用户取消", 6_000).unwrap();
    assert!(c.ok);
    assert!(!c.already_terminal);
    assert_eq!(c.status, SubagentStatus::Cancelled);
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, SubagentStatus::Cancelled);
    assert_eq!(row.error.as_deref(), Some("用户取消"));
    assert_eq!(row.finished_at, Some(6_000));
    // 取消不动 tasks 表（无卡可退）
    assert!(crate::db::load_all(&conn).unwrap().is_empty());
    // 终态再取消 = no-op 幂等
    let again = cancel_subagent_locked(&conn, &ack.subagent_id, "再取消", 7_000).unwrap();
    assert!(again.already_terminal);
    assert_eq!(again.status, SubagentStatus::Cancelled);
    assert_eq!(
        crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .unwrap()
            .finished_at,
        Some(6_000),
        "幂等重入不得改写首收尾时刻"
    );
}
/// 有卡的历史行仍能按 task_id 解析（子会话键之外的第二把键，降级路径不炸）
#[test]
fn cancel_resolves_by_task_id_on_legacy_row() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    // 模拟老库遗留：给行补一个 task_id
    conn.execute(
        "UPDATE subagents SET task_id = 'legacy-card' WHERE id = ?1",
        [&ack.subagent_id],
    )
    .unwrap();
    let c = cancel_subagent_locked(&conn, "legacy-card", "卡片停止按钮", 6_000).unwrap();
    assert_eq!(c.subagent_id, ack.subagent_id);
    assert_eq!(c.status, SubagentStatus::Cancelled);
}
/// 新行 task_id 为 NULL，按卡键解析落到会话键探测后仍报「不存在」而非 panic
#[test]
fn cancel_by_task_key_on_cardless_row_is_invalid_argument() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let err = match cancel_subagent_locked(&conn, "no-such-card", "x", 6_000) {
        Err(e) => e,
        Ok(_) => panic!("无卡行的假卡键不得解析成功"),
    };
    assert!(matches!(err, CommandError::InvalidArgument { .. }));
    assert_eq!(ack.task_id, None);
}
#[test]
fn cancel_unknown_id_is_invalid_argument() {
    let conn = test_conn();
    // OK 侧 CancelAck 无 Debug——match 拆错误
    let err = match cancel_subagent_locked(&conn, "sa_ghost", "x", 1) {
        Err(e) => e,
        Ok(_) => panic!("未知 id 必须失败"),
    };
    assert!(matches!(err, CommandError::InvalidArgument { .. }));
}
#[test]
fn cancel_beats_classified_failure_when_reporting_terminal_state() {
    // 用户取消 → StopToken 打断 → classify 只会给 Failed；上报必须用行上的
    // cancelled，否则每次取消都记一条 subagent_failed + 空原因教训
    assert_eq!(
        effective_status(Some(SubagentStatus::Cancelled), SubagentStatus::Failed),
        SubagentStatus::Cancelled
    );
    // 行没拿到（收尾闭包整体失败）才回落到观感值
    assert_eq!(
        effective_status(None, SubagentStatus::Failed),
        SubagentStatus::Failed
    );
}
/// 用户取消的子 agent 不给父会话起汇报轮：用户刚点过停止，
/// 不需要模型再解释一遍。
#[test]
fn cancelled_subagent_does_not_report_to_parent() {
    assert!(!should_report_to_parent(SubagentStatus::Cancelled));
    for st in [
        SubagentStatus::Succeeded,
        SubagentStatus::Failed,
        SubagentStatus::BudgetExceeded,
        SubagentStatus::Running,
        SubagentStatus::Queued,
    ] {
        assert!(should_report_to_parent(st), "{st:?} 应汇报");
    }
}
/// 接线守卫：汇报轮的门控必须用收尾时的真实终态，不能用行快照。
///
/// 行是派发时建的，收尾前 status 恒为 Queued——门控拿 `row.status` 判取消
/// 永远判不成立，用户取消后照样起汇报轮（违反设计「用户取消不汇报」）。
/// 单测验不到调用点传的是哪个值，故这里扫源码钉住接线。
#[test]
fn parent_report_gate_is_wired_to_the_effective_status() {
    let src = orch_prod_src();
    let production = src
        .split_once("#[cfg(test)]")
        .expect("bot_orchestrator.rs 应有测试模块")
        .0;
    assert!(
        production.contains(concat!(
            "spawn_parent_report(&app, &row, effective_",
            "status)"
        )),
        "汇报轮必须传收尾时的真实终态（effective_status）"
    );
    // 只扫汇报轮函数体：别处读行快照是合法的（如 read_own_card 读的是
    // 库里当前行，不是收尾前的快照）
    let digest = include_str!("../../bot/subagent_digest.rs");
    let start = digest
        .find("fn spawn_parent_report")
        .expect("应能找到 spawn_parent_report");
    let rest = &digest[start..];
    let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
    let func = &rest[..end];
    assert!(
        func.contains("should_report_to_parent("),
        "汇报轮门控必须走 should_report_to_parent"
    );
    assert!(
        !func.contains("row.status"),
        "汇报轮门控不得读行快照判取消（快照恒为 Queued）"
    );
}
/// 在跑的子 agent 必须报得出「已跑 N / 预算 M 轮」。
///
/// 回归背景：轮数此前完全不落任何地方，主 agent 靠 `check_subagent` 轮询
/// 只看得到验收进度——一个跑满预算、在原地打转的子任务和一个刚启动的子任务
/// 在 UI 上长得一模一样，只能干等 budget_exceeded 才知道它在烧钱。
#[test]
fn check_reports_rounds_while_subagent_runs() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let sid = format!("sa-test-rounds-{}", ack.subagent_id);
    // 注册子会话（行上 session_id 与之对应），模拟在跑的状态
    crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

    // 守卫注销时（终态）读不到计数器 → 不报轮数，字段整体不出现
    let queued = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
    assert!(
        queued.progress.get("roundsUsed").is_none(),
        "无在跑计数器时不得报轮数（凭空造数比不报更坏）"
    );

    let _g2 = crate::tool_guard::SubagentSessionGuard::new(
        &sid,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: ack.subagent_id.clone(),
            task_id: None,
            profile: SubagentProfile::Research,
            tools: std::sync::Arc::new(vec![]),
            budget: SubagentBudget::default(),
            artifact_dir: std::path::PathBuf::from("/tmp/art_rounds"),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(14)),
            wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
        },
    );
    let running = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
    assert_eq!(running.progress["roundsUsed"], 14);
    assert_eq!(
        running.progress["roundsBudget"],
        SubagentBudget::default().max_turns,
        "预算必须与轮数同报：10/20 与 10/200 紧迫度完全不同"
    );
}
/// list_subagents 的行视图与 check_subagent 同口径：在途报轮数与预算，
/// 读不到仪表（终态/未开跑）就整段留空——两条链路口径必须一致，否则
/// 「侧栏说跑了 14 轮、check 说没有」会让人怀疑数据。
#[test]
fn list_view_attaches_rounds_while_subagent_runs() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let sid = format!("sa-test-view-{}", ack.subagent_id);
    crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .expect("spawn 后应有行");
    let queued = attach_rounds(row);
    assert_eq!(
        (queued.rounds_used, queued.rounds_budget),
        (None, None),
        "无在跑仪表时不得报轮数（凭空造数比不报更坏）"
    );

    let _g2 = crate::tool_guard::SubagentSessionGuard::new(
        &sid,
        crate::tool_guard::SubagentSessionCtx {
            subagent_id: ack.subagent_id.clone(),
            task_id: None,
            profile: SubagentProfile::Research,
            tools: std::sync::Arc::new(vec![]),
            budget: SubagentBudget::default(),
            artifact_dir: std::path::PathBuf::from("/tmp/art_view"),
            used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(14)),
            wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
        },
    );
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .expect("行仍在");
    let running = attach_rounds(row);
    assert_eq!(running.rounds_used, Some(14));
    assert_eq!(
        running.rounds_budget,
        Some(SubagentBudget::default().max_turns),
        "预算必须与轮数同报：10/20 与 10/200 紧迫度完全不同"
    );
}
/// 守卫注销（子 agent 收尾）后轮数归空：列表要显示的是「这轮跑了多久」，
/// 不是死后残留的最后一帧计数。终态行拿 started/finished 算时长即可。
#[test]
fn list_view_drops_rounds_after_guard_dropped() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    let sid = format!("sa-test-drop-{}", ack.subagent_id);
    crate::db::set_subagent_session(&conn, &ack.subagent_id, &sid).unwrap();

    {
        let _guard = crate::tool_guard::SubagentSessionGuard::new(
            &sid,
            crate::tool_guard::SubagentSessionCtx {
                subagent_id: ack.subagent_id.clone(),
                task_id: None,
                profile: SubagentProfile::Research,
                tools: std::sync::Arc::new(vec![]),
                budget: SubagentBudget::default(),
                artifact_dir: std::path::PathBuf::from("/tmp/art_drop"),
                used_tool_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                used_rounds: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(7)),
                wrap_up: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                wall_deadline: std::time::Instant::now() + std::time::Duration::from_secs(600),
            },
        );
        let row = crate::db::load_subagent(&conn, &ack.subagent_id)
            .unwrap()
            .expect("行仍在");
        assert_eq!(attach_rounds(row).rounds_used, Some(7));
    }

    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .expect("行仍在");
    let after = attach_rounds(row);
    assert_eq!(
        (after.rounds_used, after.rounds_budget),
        (None, None),
        "守卫注销后不得继续报轮数"
    );
}
/// 子 agent 预算默认值单源：db 侧常量必须引用 bot::params，改一处不漏两处。
#[test]
fn subagent_budget_defaults_have_single_source() {
    assert_eq!(
        crate::db::subagents::DEFAULT_MAX_TURNS,
        crate::bot::params::DEFAULT_SUBAGENT_MAX_TURNS
    );
    assert_eq!(
        crate::db::subagents::DEFAULT_MAX_TOOL_CALLS,
        crate::bot::params::DEFAULT_SUBAGENT_MAX_TOOL_CALLS
    );
    assert_eq!(
        crate::db::subagents::DEFAULT_MAX_WALL_SECONDS,
        crate::bot::params::DEFAULT_SUBAGENT_MAX_WALL_SECS
    );
}
#[test]
fn check_counts_progress_from_acceptance_and_result() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    // 未收尾：total = 验收标准条数，done = 0
    let by_id = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
    assert_eq!(by_id.subagent_id, ack.subagent_id);
    assert_eq!(by_id.status, SubagentStatus::Queued);
    assert_eq!(by_id.progress["subtasksDone"], 0);
    assert_eq!(by_id.progress["subtasksTotal"], 2);
    assert!(by_id.result.is_none());
    assert_eq!(by_id.task_id, None);
    // 收尾后：done = result_json.acceptance_check 里 met=true 的条数
    crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"acceptance_check":[{"criterion":"覆盖至少 5 个产品","met":true},{"criterion":"每条含官网 URL","met":false}]}"#,
        )
        .unwrap();
    let done = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
    assert_eq!(done.progress["subtasksDone"], 1);
    assert_eq!(done.progress["subtasksTotal"], 2);
    // 幂等：两次 check 结果一致
    assert_eq!(
        check_subagent_locked(&conn, &ack.subagent_id)
            .unwrap()
            .progress,
        done.progress
    );
}
/// 机器可判定谓词：只有带前缀的条目进机器核验，自由文本原样留给 verifier
#[test]
fn machine_criterion_parsed_only_for_known_prefixes() {
    assert_eq!(
        parse_machine_criterion("file_exists:report.md"),
        Some(MachineCheck::FileExists("report.md".into()))
    );
    assert_eq!(
        parse_machine_criterion("  file_nonempty: out/数据.xlsx "),
        Some(MachineCheck::FileNonEmpty("out/数据.xlsx".into()))
    );
    assert_eq!(parse_machine_criterion("覆盖至少 5 个产品"), None);
    assert_eq!(parse_machine_criterion("每条含官网 URL"), None);
    assert_eq!(parse_machine_criterion(""), None);
}
/// 路径逃逸一律不解析：判定路径能跑出产物目录，验收就成了任意文件探测器
#[test]
fn machine_criterion_rejects_paths_escaping_artifact_dir() {
    for bad in [
        "file_exists:../../etc/passwd",
        "file_exists:/etc/passwd",
        "file_exists:C:\\Windows\\win.ini",
        "file_exists:./report.md",
        "file_exists:",
        "file_nonempty:a//b.md",
    ] {
        assert_eq!(
            parse_machine_criterion(bad),
            None,
            "逃逸路径必须不解析：{bad}"
        );
    }
    assert_eq!(safe_rel_path("a/b/c.md"), Some("a/b/c.md"));
}
/// 机器判定真去 FS 看：存在/不存在/空文件三种形态
#[test]
fn eval_machine_criteria_reads_real_files() {
    let dir = std::env::temp_dir().join(format!("wm-mc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("report.md"), "# 报告").unwrap();
    std::fs::write(dir.join("empty.md"), "").unwrap();

    let out = eval_machine_criteria(
        &[
            "file_exists:report.md".into(),
            "file_nonempty:report.md".into(),
            "file_nonempty:empty.md".into(),
            "file_exists:missing.md".into(),
            "自由文本标准".into(),
        ],
        &dir,
    );
    assert_eq!(out.len(), 4, "自由文本条目不进机器判定");
    assert!(out[0].met && out[1].met);
    assert!(!out[2].met, "空文件不满足 nonempty");
    assert!(!out[3].met, "缺失文件不通过");
    assert_eq!(out[3].criterion, "file_exists:missing.md");
    let _ = std::fs::remove_dir_all(&dir);
}
/// 机器结论覆盖模型自评：模型说 met:true 而文件不存在 → 以服务端为准
#[test]
fn merge_machine_verdicts_overrides_self_report_and_fills_missing() {
    let mut result = serde_json::json!({
        "acceptance_check": [
            {"criterion": "file_exists:report.md", "met": true, "evidence": "我写好了"},
            {"criterion": "内容覆盖 5 个产品", "met": false}
        ]
    });
    merge_machine_verdicts(
        &mut result,
        &[MachineVerdict {
            criterion: "file_exists:report.md".into(),
            met: false,
            reason: "产物不存在：NotFound".into(),
        }],
    );
    let arr = result["acceptance_check"].as_array().unwrap();
    assert_eq!(arr[0]["met"], false, "自评必须被服务端结论覆盖");
    assert_eq!(arr[0]["machine"], true);
    assert!(arr[0]["machine_reason"]
        .as_str()
        .unwrap()
        .contains("产物不存在"));
    assert_eq!(arr[1]["met"], false, "非机器条目不受影响");

    // 模型压根没报这条 → 服务端补一条（不依赖模型主动上报）
    let mut only_free = serde_json::json!({"acceptance_check": []});
    merge_machine_verdicts(
        &mut only_free,
        &[MachineVerdict {
            criterion: "file_nonempty:out.xlsx".into(),
            met: true,
            reason: "服务端核验：out.xlsx 存在（12 字节）".into(),
        }],
    );
    let arr = only_free["acceptance_check"].as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["met"], true);

    // verifier 行用 checks 键，同样处理
    let mut verifier = serde_json::json!({"checks": []});
    merge_machine_verdicts(
        &mut verifier,
        &[MachineVerdict {
            criterion: "file_exists:v.md".into(),
            met: true,
            reason: "ok".into(),
        }],
    );
    assert_eq!(verifier["checks"].as_array().unwrap().len(), 1);
}
/// 机器判定落到用户可见的正文：未通过项必须点名，不能只躺在 result_json
#[test]
fn machine_failures_are_named_in_final_text() {
    let verdict = |c: &str, met: bool| MachineVerdict {
        criterion: c.into(),
        met,
        reason: "产物不存在：NotFound".into(),
    };
    // 全通过 → 正文不动
    let (r, text) = apply_machine_verdicts(
        Some(serde_json::json!({"acceptance_check": []})),
        "干完了".into(),
        &[verdict("file_exists:a.md", true)],
    );
    assert_eq!(text, "干完了");
    assert_eq!(r.unwrap()["acceptance_check"].as_array().unwrap().len(), 1);

    // 有未通过 → 正文点名 + 原因
    let (r, text) = apply_machine_verdicts(
        Some(serde_json::json!({"acceptance_check": []})),
        "干完了".into(),
        &[
            verdict("file_exists:a.md", true),
            verdict("file_exists:b.md", false),
        ],
    );
    assert!(text.contains("机器验收未通过 1 项"), "{text}");
    assert!(text.contains("file_exists:b.md"), "{text}");
    assert!(text.contains("产物不存在"), "{text}");
    assert!(text.starts_with("干完了"), "原正文必须保留：{text}");
    assert_eq!(r.unwrap()["acceptance_check"].as_array().unwrap().len(), 2);

    // 无机器条目 → 原样返回（不改 JSON、不动正文）
    let (r, text) = apply_machine_verdicts(
        Some(serde_json::json!({"acceptance_check": [{"criterion":"x","met":true}]})),
        "干完了".into(),
        &[],
    );
    assert_eq!(text, "干完了");
    assert_eq!(r.unwrap()["acceptance_check"][0]["met"], true);
    // 异常退出无收尾 JSON：不得凭空造一个
    let (r, _) = apply_machine_verdicts(
        None,
        "执行失败".into(),
        &[verdict("file_exists:a.md", false)],
    );
    assert!(r.is_none(), "没有收尾 JSON 时不应伪造");
}
/// 交叉核对：没派发过的标准、重复上报，都不得灌满进度
#[test]
fn progress_ignores_undeclared_and_duplicate_criteria() {
    let declared = vec![
        "覆盖至少 5 个产品".to_string(),
        "每条含官网 URL".to_string(),
    ];
    // 虚报一条从未派发的标准 + 重复报同一条 → 只应计 1
    let reported = vec![
        serde_json::json!({"criterion": "覆盖至少 5 个产品", "met": true}),
        serde_json::json!({"criterion": "覆盖至少 5 个产品", "met": true}),
        serde_json::json!({"criterion": "我其实做了 10 个产品", "met": true}),
    ];
    assert_eq!(count_met_against_declared(&declared, &reported), 1);
    // met 非 bool / 缺 criterion 的条目不计
    assert_eq!(
        count_met_against_declared(
            &declared,
            &[serde_json::json!({"criterion": "每条含官网 URL", "met": "yes"})]
        ),
        0
    );
    assert_eq!(
        count_met_against_declared(&declared, &[serde_json::json!({"met": true})]),
        0
    );
    // 白空格差异不算漏报（模型可能顺手加了空格）
    assert_eq!(
        count_met_against_declared(
            &declared,
            &[serde_json::json!({"criterion": " 每条含官网 URL ", "met": true})]
        ),
        1
    );
}
#[test]
fn verifier_row_progress_counts_checks_array_not_zero() {
    // verifier 行的收尾 JSON 是 checks 数组（verifier 独有 schema）：
    // done 若只认 acceptance_check，验收全过的 verifier 行也显示 0/N，
    // 主 agent 会误判「没做成」而重派
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    crate::db::set_subagent_result(
            &conn,
            &ack.subagent_id,
            r#"{"checks":[{"criterion":"覆盖至少 5 个产品","met":true},{"criterion":"每条含官网 URL","met":true}]}"#,
        )
        .unwrap();
    let state = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
    assert_eq!(state.progress["subtasksDone"], 2);
    assert_eq!(state.progress["subtasksTotal"], 2);
}
#[test]
fn check_unknown_key_is_invalid_argument() {
    let conn = test_conn();
    let err = check_subagent_locked(&conn, "sa_ghost").unwrap_err();
    assert!(matches!(err, CommandError::InvalidArgument { .. }));
}
#[test]
fn check_returns_parsed_result_json() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let ack = spawn_subagent_locked(&mut conn, &req(None), 5_000).unwrap();
    crate::db::set_subagent_result(
        &conn,
        &ack.subagent_id,
        r#"{"summary":"结论","confidence":0.9}"#,
    )
    .unwrap();
    crate::db::update_subagent_status(
        &conn,
        &ack.subagent_id,
        SubagentStatus::Running,
        5_100,
        None,
    )
    .unwrap();
    crate::db::update_subagent_status(
        &conn,
        &ack.subagent_id,
        SubagentStatus::Succeeded,
        5_200,
        None,
    )
    .unwrap();
    let state = check_subagent_locked(&conn, &ack.subagent_id).unwrap();
    assert_eq!(state.status, SubagentStatus::Succeeded);
    assert_eq!(state.result.unwrap()["summary"], "结论");
}
#[test]
fn subagent_stage_roundtrips_string() {
    for (s, expected) in [
        (SubagentStage::Recon, "recon"),
        (SubagentStage::Executor, "executor"),
        (SubagentStage::Verifier, "verifier"),
    ] {
        assert_eq!(s.as_str(), expected);
        assert_eq!(SubagentStage::from_str(expected), Some(s));
    }
    assert_eq!(SubagentStage::from_str("unknown"), None);
}
#[test]
fn stage_default_profile_is_research_or_coder() {
    // recon/verifier 只读足够 → research
    assert_eq!(
        SubagentStage::Recon.default_profile(),
        SubagentProfile::Research
    );
    assert_eq!(
        SubagentStage::Verifier.default_profile(),
        SubagentProfile::Research
    );
    // executor 写类必需 → coder
    assert_eq!(
        SubagentStage::Executor.default_profile(),
        SubagentProfile::Coder
    );
}
#[test]
fn stage_is_not_persisted_and_profile_field_unaffected() {
    let mut conn = test_conn();
    let _g = crate::db::lock_db_write();
    let mut r = req(None);
    r.profile = SubagentProfile::Coder;
    r.stage = Some(SubagentStage::Executor);
    let ack = spawn_subagent_locked(&mut conn, &r, 5_000).unwrap();
    let row = crate::db::load_subagent(&conn, &ack.subagent_id)
        .unwrap()
        .unwrap();
    // 行 profile 是显式给的 Coder（stage 在内存里走闭包，不落表）
    assert_eq!(row.profile, SubagentProfile::Coder);
}
/// profile 解析优先级：显式 > 配置档 > stage 派生，都没给则 None。
///
/// 「配置档 > stage」这一档是配置档 profile 唯一的生效路径——它排在
/// 显式之后，才不会让 yaml 覆盖运行时档位（那等于放宽）。
#[test]
fn spawn_profile_priority_is_explicit_then_config_then_stage() {
    let e = Some(SubagentProfile::General);
    let c = Some(SubagentProfile::Coder);
    // ① 显式压过配置档与 stage（防「配置覆盖运行时 profile = 放宽」）
    assert_eq!(
        resolve_spawn_profile(e, c, Some(SubagentStage::Executor)),
        Some(SubagentProfile::General)
    );
    // ② 没给显式 → 配置档兜底，压过 stage
    assert_eq!(
        resolve_spawn_profile(None, c, Some(SubagentStage::Executor)),
        Some(SubagentProfile::Coder)
    );
    // ③ 只有 stage → 按 stage 派生
    assert_eq!(
        resolve_spawn_profile(None, None, Some(SubagentStage::Verifier)),
        Some(SubagentProfile::Research)
    );
    // ④ 三者皆无 → None，调用方据此报错（不许猜一个默认档）
    assert_eq!(resolve_spawn_profile(None, None, None), None);
}
#[test]
fn onion_turn_cap_only_tightens() {
    let mut b = SubagentBudget {
        max_turns: 30,
        max_tool_calls: 100,
        max_wall_seconds: 600,
    };
    // 配置档更紧 → 压到配置档的值
    apply_onion_turn_cap(&mut b, Some(4));
    assert_eq!(b.max_turns, 4);
    // 配置档更松 → 不许被它顶开（配置档是上限，不是覆盖）
    apply_onion_turn_cap(&mut b, Some(50));
    assert_eq!(b.max_turns, 4, "配置档比派发值宽时不应放宽轮数");
    // 没给上限 → 不动
    apply_onion_turn_cap(&mut b, None);
    assert_eq!(b.max_turns, 4);
}
/// 接线守卫：配置档从派发到执行的四段链路，一段都不能靠单测推断。
///
/// 四段分别是「spawn 解析 → 内存闭包进 runner → 建会话写两列 →
/// bind 出的交集进守卫白名单」。缺任一段的症状都是同一个：模型以为
/// 派了个受限子 agent，实际跑的是全档位——纯函数单测全绿也照样漏。
///
/// **所有匹配串都用 `concat!` 拆开**：本测试就住在被扫描的同一个文件里，
/// 裸写字面量会被测试自己的源码匹配上，删掉真接线照样绿（本条守卫第一版
/// 就是这么假绿的）。与 `tool_guard::dead_command_bind_file_not_in_dispatcher` 同款。
#[test]
fn onion_config_is_wired_from_spawn_to_guard_whitelist() {
    let body = orch_prod_src();

    // ① 派发层解析出配置档并装进 SpawnRequest
    let parse_arm = body
        .split_once(concat!("crate::bot_onions::", "load(app, id)"))
        .expect("派发层必须先加载配置档再建行")
        .1
        .split("parent_session_id: session_id,")
        .next()
        .expect("SpawnRequest 构造应在 session_id 之前");
    assert!(
        parse_arm.contains(concat!("onion: ", "onion_cfg")),
        "SpawnRequest 必须带上已解析的配置档：{parse_arm}"
    );

    // ② 配置档随内存闭包进 runner（不落 subagents 行）
    assert!(
        body.contains(concat!("let runner_onion = ", "req.onion.clone();")),
        "配置档必须从 req 摘出来交给 runner，否则 runner 拿不到配置名"
    );
    assert!(
        body.contains(concat!(
            "run_subagent(runner_app, runner_sid, ",
            "runner_stage, runner_onion)"
        )),
        "run_subagent 必须收到配置档参数"
    );

    // ③ 建会话时把配置写进 bot_sessions 两列——不写则 bind_session 无从读起
    let setup = body
        .split_once(concat!("bot_session_", "set_onion("))
        .expect("子会话建会话时必须写配置档两列")
        .1;
    assert!(
        setup.contains(concat!("Some(&", "o.config)")),
        "要写配置档名（否则 bind 读到空指针，配置静默失效）"
    );

    // ④ 守卫白名单取交集，且 bind 发生在守卫注册之前
    let bind_at = body
        .find(concat!(
            "crate::bot_onions::",
            "bind_session(&app, &session_id)"
        ))
        .expect("runner 必须绑定配置");
    let guard_at = body
        .find("SubagentSessionGuard::new(")
        .expect("runner 必须注册守卫");
    assert!(
        bind_at < guard_at,
        "bind_session 必须在守卫注册之前：守卫的白名单取自绑定后的交集，\
             顺序反了等于拿 profile 白名单当交集"
    );
    let guard_arm = body.split_at(guard_at).1;
    let guard_arm = &guard_arm[..guard_arm.find(");").unwrap_or(guard_arm.len())];
    assert!(
        guard_arm.contains(concat!("tools: std::sync::Arc::", "new(subagent_tools)")),
        "守卫必须带交集后的工具集，闸才是按收窄判的：{guard_arm}"
    );
}
/// 接线守卫：配置档的角色与实际可用工具要进子 agent 的 system 提示词。
///
/// 少了这段，配置档对模型就是「工具莫名少了一半」——它会反复调被收掉的
/// 工具、每次吃一次白名单拒绝，白烧轮数。匹配串同样 `concat!` 拆开。
#[test]
fn onion_prompt_note_is_injected_into_subagent_system() {
    let body = orch_prod_src();
    assert!(
        body.contains(concat!(
            "crate::bot_onions::",
            "render_prompt_note(cfg, row.profile)"
        )),
        "system 拼装必须注入配置档提示词段"
    );
    let system_at = body
        .find("let system = format!(")
        .expect("子 agent system 拼装点");
    let arm = &body[system_at..];
    let arm = &arm[..arm.find("let msgs = vec![").unwrap_or(arm.len())];
    assert!(
        arm.contains("onion_note"),
        "system 格式串必须带上配置档段，无配置档时为空串占位：{arm}"
    );
}
