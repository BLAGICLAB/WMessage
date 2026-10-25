//! 技能运行时的纯函数与集成测试。

use super::*;
use crate::bot_skills::test_run;

// ── skill_finish 零迁移审计门 ──

#[test]
fn skill_finish_no_transition_audits_only_when_reason_non_empty() {
    // 读写共享 bot.log，必须持测试串行锁（与 audit / bot::config 同名用例互斥）
    let _serial = crate::db::lock::lock_or_recover(
        &crate::audit::BOT_LOG_TEST_LOCK,
        "bot_skills::runtime::skill_runs",
    );
    let app = tauri::test::mock_app();
    let bot_log = crate::db::data_dir(app.handle()).join("bot.log");
    let read = || std::fs::read_to_string(&bot_log).unwrap_or_default();
    // 空注册表 + ok=true + reason 非空 → 记 no_transition
    let _ = std::fs::remove_file(&bot_log);
    skill_finish(app.handle(), true, "done", Some("sess-bt01c"));
    let c1 = read();
    assert!(
        c1.contains("skill_finish_no_transition"),
        "reason 非空零迁移应留痕: {c1}"
    );
    // 空 reason（清理性调用）→ 不记
    let _ = std::fs::remove_file(&bot_log);
    skill_finish(app.handle(), true, "", Some("sess-bt01c"));
    let c2 = read();
    assert!(
        !c2.contains("skill_finish_no_transition"),
        "空 reason 不应留痕: {c2}"
    );
    // ok=false + reason 非空 → 不记（合法 no-op 路径）
    let _ = std::fs::remove_file(&bot_log);
    skill_finish(app.handle(), false, "boom", Some("sess-bt01c"));
    let c3 = read();
    assert!(
        !c3.contains("skill_finish_no_transition"),
        "ok=false 不应留痕: {c3}"
    );
}

// ── 切技能语义 ──

/// 切技能：同会话的旧活动技能必须从表里摘掉——
/// 留在表里就是一条同会话的终态 run，活动快照可能取到它，
/// 主循环会把新技能当「已完成」直接收尾（技能一用就断）。
#[test]
fn take_over_switched_runs_drops_previous_skill_of_same_session() {
    let mut runs: std::collections::HashMap<String, SkillRun> = std::collections::HashMap::new();
    let mut old = test_run(8, 180);
    old.name = "old-skill".into();
    old.state = SkillState::Running;
    old.step = 4;
    old.session_id = Some("sess-a".into());
    runs.insert(old.name.clone(), old);

    let mut other = test_run(8, 180);
    other.name = "other-session-skill".into();
    other.state = SkillState::Running;
    other.session_id = Some("sess-b".into());
    runs.insert(other.name.clone(), other);

    let mut paused = test_run(8, 180);
    paused.name = "paused-skill".into();
    paused.state = SkillState::Paused;
    paused.session_id = Some("sess-a".into());
    runs.insert(paused.name.clone(), paused);

    let mut switched = take_over_switched_runs(&mut runs, "new-skill", Some("sess-a"));
    // 摘取顺序随 HashMap 迭代序走 → 比对前排序，别让断言依赖顺序
    switched.sort();

    assert_eq!(
        switched,
        vec![
            ("old-skill".to_string(), 4),
            ("paused-skill".to_string(), 0)
        ],
        "同会话的活动技能（Running/Paused）都要摘掉"
    );
    assert!(!runs.contains_key("old-skill"), "切走的技能必须出表");
    assert!(!runs.contains_key("paused-skill"), "切走的技能必须出表");
    assert!(
        runs.contains_key("other-session-skill"),
        "别的会话的技能不受影响"
    );
}

// ── 状态机推进 ──

#[test]
fn advance_loaded_returns_no_active() {
    let run = test_run(8, 180);
    // test_run 初始状态就是 Loaded
    assert_eq!(run.state, SkillState::Loaded);
    assert_eq!(advance_skill(&run, 1000), AdvanceAction::NoActive);
}

#[test]
fn advance_running_no_timeout_continues() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Running;
    // started_at_ms=1000, now=10s 后 11000, timeout=180s → 仍可继续
    assert_eq!(advance_skill(&run, 11000), AdvanceAction::Continue);
}

#[test]
fn advance_running_just_before_timeout_continues() {
    let mut run = test_run(8, 60);
    run.state = SkillState::Running;
    // 恰好 60s 未超时（入 strict > 才 Fail）
    assert_eq!(
        advance_skill(&run, 1000 + 60 * 1000),
        AdvanceAction::Continue
    );
}

#[test]
fn advance_running_timeout_returns_fail() {
    let mut run = test_run(8, 60);
    run.state = SkillState::Running;
    // 启动 61s 后超时 (60s 上限)
    let action = advance_skill(&run, 1000 + 61 * 1000);
    match action {
        AdvanceAction::Fail(reason) => {
            assert!(reason.contains("超时"), "reason 应含「超时」: {reason}");
            assert!(reason.contains("60"), "reason 应含阈值 60s: {reason}");
        }
        other => panic!("expected Fail, got {other:?}"),
    }
}

#[test]
fn advance_paused_returns_await_confirm() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Paused;
    assert_eq!(advance_skill(&run, 1000), AdvanceAction::AwaitConfirm);
}

#[test]
fn advance_completed_returns_finish() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Completed;
    assert_eq!(advance_skill(&run, 1000), AdvanceAction::Finish);
}

#[test]
fn advance_failed_returns_fail_with_end_reason() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Failed;
    run.end_reason = "超过最大步数上限（8 步）".into();
    assert_eq!(
        advance_skill(&run, 1000),
        AdvanceAction::Fail("超过最大步数上限（8 步）".into())
    );
}

#[test]
fn advance_terminated_returns_terminate_with_end_reason() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Terminated;
    run.end_reason = "用户拒绝确认".into();
    assert_eq!(
        advance_skill(&run, 1000),
        AdvanceAction::Terminate("用户拒绝确认".into())
    );
}

// ── DSL 调度器决策 ──

#[test]
fn advance_dsl_continue_returns_run() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Running;
    // Running 11s（未超 180s 上限）→ 正常执行
    assert!(matches!(advance_dsl(&run, 11000), DslAdvanceAction::Run));
}

#[test]
fn advance_dsl_no_active_loaded_returns_run() {
    // 防御性分支：生产上 Loaded 只是 SkillRun::new 的瞬时初态（start_skill 直接置
    // Running，见 runtime.rs start_skill 注释），advance_dsl 永远收不到 Loaded。
    // 本测试锁定兜底语义——万一收到 Loaded 也不阻断 step 执行（按 Run 处理）。
    let run = test_run(8, 180);
    assert_eq!(run.state, SkillState::Loaded);
    assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::Run));
}

#[test]
fn advance_dsl_paused_returns_await_user() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Paused;
    assert!(matches!(
        advance_dsl(&run, 1000),
        DslAdvanceAction::AwaitUser
    ));
}

#[test]
fn advance_dsl_completed_returns_finish() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Completed;
    assert!(matches!(advance_dsl(&run, 1000), DslAdvanceAction::Finish));
}

#[test]
fn advance_dsl_failed_returns_fail_with_rollback() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Failed;
    run.end_reason = "超过最大步数上限（8 步）".into();
    match advance_dsl(&run, 1000) {
        DslAdvanceAction::FailWithRollback(reason) => {
            assert_eq!(reason, "超过最大步数上限（8 步）");
        }
        other => panic!("expected FailWithRollback, got {other:?}"),
    }
}

#[test]
fn advance_dsl_terminated_returns_terminate_no_rollback() {
    let mut run = test_run(8, 180);
    run.state = SkillState::Terminated;
    run.end_reason = "用户 /stop".into();
    match advance_dsl(&run, 1000) {
        DslAdvanceAction::Terminate(reason) => assert_eq!(reason, "用户 /stop"),
        other => panic!("expected Terminate, got {other:?}"),
    }
}

#[test]
fn advance_dsl_timeout_returns_fail_with_rollback() {
    // 端到端：Running + 启动后 61s（60s 上限）→ advance_skill 内部超时检测 → Fail
    // → advance_dsl 映射成 FailWithRollback（含 reason）
    let mut run = test_run(8, 60);
    run.state = SkillState::Running;
    let action = advance_dsl(&run, 1000 + 61 * 1000);
    match action {
        DslAdvanceAction::FailWithRollback(reason) => {
            assert!(reason.contains("超时"), "reason 应含「超时」: {reason}");
            assert!(reason.contains("60"), "reason 应含阈值 60s: {reason}");
        }
        other => panic!("expected FailWithRollback, got {other:?}"),
    }
}

// ── 前置预审 ──

#[test]
fn preflight_rejects_disabled() {
    let m = SkillMeta {
        name: "x".into(),
        enabled: false,
        ..Default::default()
    };
    assert!(preflight(&m).is_err());
}

#[test]
fn preflight_rejects_blacklist_intent() {
    let mut m = SkillMeta {
        name: "x".into(),
        description: "批量删除所有任务".into(),
        ..Default::default()
    };
    assert!(preflight(&m).unwrap_err().contains("黑名单"));

    m.description = "全盘遍历文件".into();
    assert!(preflight(&m).is_err());
}

#[test]
fn preflight_passes_normal() {
    let m = SkillMeta {
        name: "ok".into(),
        description: "汇总今日任务并归档".into(),
        ..Default::default()
    };
    assert!(preflight(&m).is_ok());
}

// ── 步骤循环 / 熔断 ──

#[test]
fn step_check_counts_and_records_actions() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Running;
    // query_tasks 在 READONLY 豁免清单（ 后），只读不记入回滚清单
    assert!(step_check(&mut r, "query_tasks", "{}", 2000).is_ok());
    assert_eq!(r.step, 1);
    assert!(r.actions.is_empty()); // 只读不记
    assert!(step_check(&mut r, "create_task", "{\"title\":\"x\"}", 2000).is_ok());
    assert_eq!(r.actions.len(), 1); // 有副作用记入
    assert!(r.actions[0].starts_with("create_task"));
}

#[test]
fn step_check_fuses_on_max_steps() {
    let mut r = test_run(2, 180);
    r.state = SkillState::Running;
    assert!(step_check(&mut r, "create_task", "{}", 2000).is_ok());
    assert!(step_check(&mut r, "edit_task", "{}", 2000).is_ok());
    let e = step_check(&mut r, "complete_task", "{}", 2000).unwrap_err();
    assert!(e.to_string().contains("超过最大步数上限"));
    assert_eq!(r.state, SkillState::Failed);
}

#[test]
fn step_check_fuses_on_timeout() {
    let mut r = test_run(8, 60);
    r.state = SkillState::Running;
    // 已过 61 秒
    let e = step_check(&mut r, "list_tasks", "{}", 1000 + 61 * 1000).unwrap_err();
    assert!(e.to_string().contains("超时"));
    assert_eq!(r.state, SkillState::Failed);
}

#[test]
fn step_check_rejects_when_paused() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Paused;
    assert!(step_check(&mut r, "list_tasks", "{}", 2000).is_err());
    assert_eq!(r.step, 0); // 暂停态不计数
}

#[test]
fn pause_state_resumable_flows() {
    // resumable=true：暂停后可恢复
    let mut r = test_run(8, 180);
    r.state = SkillState::Running;
    r.resumable = true;
    pause_state(&mut r);
    assert_eq!(r.state, SkillState::Paused);
    assert!(!r.terminal_after_confirm);
    let now = r.started_at_ms;
    confirm_state(&mut r, true, now);
    assert_eq!(r.state, SkillState::Running); // 确认通过恢复
                                              // 再次暂停后拒绝
    pause_state(&mut r);
    let now = r.started_at_ms;
    confirm_state(&mut r, false, now);
    assert_eq!(r.state, SkillState::Terminated);
    assert_eq!(r.end_reason, "用户拒绝确认");
}

#[test]
fn pause_state_non_resumable_terminates_after_confirm() {
    // resumable=false：暂停即终止——确认通过后技能终止
    let mut r = test_run(8, 180);
    r.state = SkillState::Running;
    r.resumable = false;
    pause_state(&mut r);
    assert_eq!(r.state, SkillState::Paused);
    assert!(r.terminal_after_confirm);
    let now = r.started_at_ms;
    confirm_state(&mut r, true, now);
    assert_eq!(r.state, SkillState::Terminated);
    assert!(r.end_reason.contains("resumable=false"));
}

#[test]
fn rollback_section_extracted() {
    let body = "# 文档\n## 步骤\n1. 干活\n## 回滚\n1. 撤销 A\n2. 恢复 B\n## 其他\n尾巴";
    let rb = rollback_section(body);
    assert!(rb.contains("撤销 A"));
    assert!(rb.contains("恢复 B"));
    assert!(!rb.contains("尾巴")); // 到下一个 ## 为止

    assert_eq!(rollback_section("没有回滚章节"), "");
}

// ── 同名技能跨会话顶号拦截 ──

/// test_run 的起点是 1000ms；NOW 取起点后 10s（未超时口径内）。
const NOW: i64 = 11_000;

#[test]
fn skill_conflict_other_session_running_is_blocked() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Running;
    r.session_id = Some("s-a".into());
    assert!(skill_conflict_with_existing(Some(&r), Some("s-b"), NOW));
}

#[test]
fn skill_conflict_other_session_paused_is_blocked() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Paused;
    r.session_id = Some("s-a".into());
    assert!(skill_conflict_with_existing(Some(&r), Some("s-b"), NOW));
    // 一方无会话（后台任务）也算不同会话
    assert!(skill_conflict_with_existing(Some(&r), None, NOW));
}

#[test]
fn skill_conflict_same_session_restart_allowed() {
    // 同会话同名重启：保留原有覆盖语义，不拦截
    let mut r = test_run(8, 180);
    r.state = SkillState::Running;
    r.session_id = Some("s-a".into());
    assert!(!skill_conflict_with_existing(Some(&r), Some("s-a"), NOW));
}

#[test]
fn skill_conflict_terminal_state_allowed() {
    // 终态（Completed/Failed/Terminated）不拦截，可重启
    for state in [
        SkillState::Completed,
        SkillState::Failed,
        SkillState::Terminated,
    ] {
        let mut r = test_run(8, 180);
        r.state = state.clone();
        r.session_id = Some("s-a".into());
        assert!(
            !skill_conflict_with_existing(Some(&r), Some("s-b"), NOW),
            "终态 {state:?} 不应拦截"
        );
    }
}

#[test]
fn skill_conflict_no_existing_allowed() {
    assert!(!skill_conflict_with_existing(None, Some("s-b"), NOW));
    assert!(!skill_conflict_with_existing(None, None, NOW));
}

// ── 暂停（Paused）超时 ──

/// Paused 不是终态：不判超时它就一直占着同名技能，
/// 别的会话再也起不来（那个会话不回来就永久挡路）。
#[test]
fn skill_conflict_expired_pause_no_longer_blocks() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Paused;
    r.session_id = Some("s-a".into());
    // 超时后（180s 之后）不再算活动
    let expired_at = r.started_at_ms + 181 * 1000;
    assert!(!skill_conflict_with_existing(
        Some(&r),
        Some("s-b"),
        expired_at
    ));
    assert!(!skill_conflict_with_existing(Some(&r), None, expired_at));
}

/// 状态机：暂停等确认超过技能超时 → 判失败（不再无限等确认）。
#[test]
fn advance_skill_paused_fails_after_timeout() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Paused;
    let expired_at = r.started_at_ms + 181 * 1000;
    match advance_skill(&r, expired_at) {
        AdvanceAction::Fail(reason) => assert!(reason.contains("超时"), "{reason}"),
        other => panic!("超时的暂停应判失败，实得 {other:?}"),
    }
}

/// 未超时的暂停仍等确认（不得提前判失败）。
#[test]
fn advance_skill_paused_within_timeout_awaits_confirm() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Paused;
    assert_eq!(
        advance_skill(&r, r.started_at_ms + 179 * 1000),
        AdvanceAction::AwaitConfirm
    );
}

/// 过了超时的暂停不再恢复：确认通过也不能让过期计划继续动手。
#[test]
fn confirm_state_expired_pause_fails_instead_of_resuming() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Paused;
    r.resumable = true;
    let expired_at = r.started_at_ms + 181 * 1000;
    confirm_state(&mut r, true, expired_at);
    assert_eq!(r.state, SkillState::Failed);
    assert!(r.end_reason.contains("超时"), "{}", r.end_reason);
}

/// 清扫：过期的暂停被标成 Failed（终态），占位随之释放。
#[test]
fn sweep_expired_pauses_marks_failed_and_frees_slot() {
    let mut runs: std::collections::HashMap<String, SkillRun> = std::collections::HashMap::new();
    let mut paused = test_run(8, 180);
    paused.state = SkillState::Paused;
    paused.session_id = Some("s-a".into());
    let mut running = test_run(8, 180);
    running.state = SkillState::Running;
    running.session_id = Some("s-b".into());
    runs.insert("paused-skill".into(), paused);
    runs.insert("running-skill".into(), running);
    let expired_at = 1000 + 181 * 1000;
    let expired = sweep_expired_pauses(&mut runs, expired_at);
    // 两个 run 同一 started_at：Paused 与 Running（死会话残留）都过超时线
    assert_eq!(expired.len(), 2);
    assert!(expired.contains(&"paused-skill".to_string()));
    assert!(expired.contains(&"running-skill".to_string()));
    let swept = runs.get("paused-skill").unwrap();
    assert_eq!(swept.state, SkillState::Failed);
    assert!(swept.end_reason.contains("超时"), "{}", swept.end_reason);
    // 同一时刻的 Running（死会话残留）同样被扫——只扫 Paused 一半时，
    // 模型循环带 Err 退出的会话会永久占着同名技能
    let swept_running = runs.get("running-skill").unwrap();
    assert_eq!(swept_running.state, SkillState::Failed);
    assert!(
        swept_running.end_reason.contains("超时"),
        "{}",
        swept_running.end_reason
    );
    // 未超时的 Running 不受影响
    let mut fresh = test_run(8, 3600);
    fresh.state = SkillState::Running;
    runs.insert("fresh-skill".into(), fresh);
    let not_expired = sweep_expired_pauses(&mut runs, 1000 + 200 * 1000);
    assert!(not_expired.is_empty());
    assert_eq!(runs.get("fresh-skill").unwrap().state, SkillState::Running);
}

/// 冲突闸：超时的 Running（死会话残留）不再拦截同名技能启动
#[test]
fn conflict_gate_lets_timed_out_running_pass() {
    let mut r = test_run(8, 180);
    r.state = SkillState::Running;
    r.session_id = Some("s-dead".into());
    let expired_at = 1000 + 181 * 1000;
    assert!(!skill_conflict_with_existing(
        Some(&r),
        Some("s-new"),
        expired_at
    ));
    // 未超时的 Running 照旧拦截
    assert!(skill_conflict_with_existing(
        Some(&r),
        Some("s-new"),
        1000 + 10 * 1000
    ));
}

// ── 技能预算软警告 ──

/// 软警告：预算充裕时不提醒，临近硬闸时提醒一次。
#[test]
fn budget_warning_fires_only_when_budget_nears_exhaustion() {
    let mut r = test_run(50, 1800);
    r.state = SkillState::Running;
    // 刚起步：步数与墙钟都充裕
    assert!(skill_budget_warning(&r, 1000 + 10 * 1000).is_none());
    // 步数余量 ≤ 5 → 提醒
    r.step = 46;
    let w = skill_budget_warning(&r, 1000 + 10 * 1000).expect("步数快用完应提醒");
    assert!(w.contains("还剩 4 步"), "文案要带步数余量: {w}");
    assert!(w.contains("整理成最终回复"), "文案要指示模型收尾: {w}");
    // 墙钟余量 ≤ 60 秒 → 提醒（步数还很充裕）
    let mut r2 = test_run(50, 1800);
    r2.state = SkillState::Running;
    let w2 = skill_budget_warning(&r2, 1000 + 1741 * 1000).expect("墙钟快用完应提醒");
    assert!(w2.contains("59 秒"), "文案要带秒数余量: {w2}");
}

/// 软警告只对运行中的技能发：暂停/终态不在这里刷屏。
#[test]
fn budget_warning_skips_non_running_states() {
    let mut r = test_run(50, 1800);
    r.state = SkillState::Paused;
    r.step = 49;
    assert!(skill_budget_warning(&r, 1000 + 1790 * 1000).is_none());
    let mut done = test_run(50, 1800);
    done.state = SkillState::Completed;
    done.step = 50;
    assert!(skill_budget_warning(&done, 1000 + 1790 * 1000).is_none());
}

/// 小预算技能不该在第一轮就收到「快用完了」：阈值按预算规模取比例。
#[test]
fn budget_warning_stays_quiet_for_small_budget() {
    let mut r = test_run(6, 30);
    r.state = SkillState::Running;
    // 刚起步：6 步 / 30 秒都还充裕
    assert!(skill_budget_warning(&r, 1000 + 1000).is_none());
    // 走到只剩 1 步才提醒
    r.step = 5;
    assert!(skill_budget_warning(&r, 1000 + 1000).is_some());
}

// ── 技能出口兜底正文 ──

/// 模型说过话就用它，没说话才兜底。
#[test]
fn exit_body_keeps_streamed_text_when_present() {
    let out = skill_exit_body(
        "PPT 已生成",
        SkillExitKind::Completed,
        "ppt-generator",
        "",
        12,
        &[],
    );
    assert_eq!(out, "PPT 已生成");
}

/// 空回复兜底：说清楚发生了什么 + 已产出什么。
#[test]
fn exit_body_falls_back_when_model_spoke_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let ppt = dir.path().join("产品介绍.pptx");
    std::fs::write(&ppt, b"pptx").unwrap();
    let missing = dir.path().join("已删除.pptx");
    let out = skill_exit_body(
        "",
        SkillExitKind::Failed,
        "ppt-generator",
        "超时（超过 1800 秒）",
        21,
        &[
            ppt.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
        ],
    );
    assert!(out.contains("ppt-generator"), "要点名技能: {out}");
    assert!(
        out.contains("超时（超过 1800 秒）"),
        "要点明中断原因: {out}"
    );
    assert!(out.contains("产品介绍.pptx"), "要列出已产出文件: {out}");
    assert!(!out.contains("已删除.pptx"), "磁盘上没的文件不该列: {out}");
}

/// 技能正常完成但模型没说话：也要有一句可读正文。
#[test]
fn exit_body_fallback_for_completed_skill() {
    let out = skill_exit_body("", SkillExitKind::Completed, "ppt-generator", "", 30, &[]);
    assert!(out.contains("已完成"), "完成出口也要有正文: {out}");
    assert!(out.contains("30 步"), "要带步数: {out}");
}

/// 终止出口（用户叫停）与失败出口要分得开。
#[test]
fn exit_body_distinguishes_terminated_from_failed() {
    let failed = skill_exit_body("", SkillExitKind::Failed, "s", "超时", 3, &[]);
    let terminated = skill_exit_body("", SkillExitKind::Terminated, "s", "用户拒绝", 3, &[]);
    assert!(failed.contains("执行中断"), "失败出口文案: {failed}");
    assert!(terminated.contains("已终止"), "终止出口文案: {terminated}");
}
