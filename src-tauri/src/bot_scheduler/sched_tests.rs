use super::*;

fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<chrono::Local> {
    chrono::NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .single()
        .unwrap()
}

#[test]
fn validate_schedule_accepts_all_four_formats() {
    assert!(validate_schedule("daily:09:30").is_ok());
    assert!(validate_schedule("weekly:3:08:00").is_ok());
    assert!(validate_schedule("monthly:1:10:00").is_ok());
    assert!(validate_schedule("at:2026-10-08T14:00").is_ok());
    // 过期的 at: 也算格式合法（过期消费由 at_expired 语义负责）
    assert!(validate_schedule("at:2000-01-01T00:00").is_ok());
}

#[test]
fn validate_schedule_rejects_malformed() {
    assert!(validate_schedule("weird").is_err());
    assert!(validate_schedule("daily:25:00").is_err());
    assert!(validate_schedule("daily:09").is_err());
    assert!(validate_schedule("weekly:8:08:00").is_err());
    assert!(validate_schedule("weekly:3:08").is_err());
    assert!(validate_schedule("monthly:32:10:00").is_err());
    assert!(validate_schedule("at:2026-10-08 14:00").is_err());
    assert!(validate_schedule("at:not-a-date").is_err());
    assert!(validate_schedule("").is_err());
}

#[test]
fn job_card_title_note_extracts_first_line_and_truncates() {
    // 单行短内容 → 标题即全文，无 note
    let (t, n) = job_card_title_note("整理周报");
    assert_eq!(t, "整理周报");
    assert_eq!(n, None);
    // 多行 → 首行做标题，全文进 note 不丢语义
    let (t, n) = job_card_title_note("每日站会准备\n检查昨天的遗留\n整理今日议程");
    assert_eq!(t, "每日站会准备");
    assert_eq!(n, Some("每日站会准备\n检查昨天的遗留\n整理今日议程".into()));
    // 超长单行 → 标题截断 80 字，note 存原文
    let long = "长".repeat(120);
    let (t, n) = job_card_title_note(&long);
    assert_eq!(t.chars().count(), JOB_TITLE_MAX);
    assert_eq!(n, Some(long));
    // 空白容忍：trim 后入卡
    let (t, _) = job_card_title_note("  前后空白  ");
    assert_eq!(t, "前后空白");
}

#[test]
fn daily_occurrence() {
    let after = dt(2026, 8, 16, 9, 0);
    assert_eq!(
        occurrence_after("daily:10:00", after),
        Some(dt(2026, 8, 16, 10, 0))
    );
    assert_eq!(
        occurrence_after("daily:09:00", after),
        Some(dt(2026, 8, 17, 9, 0))
    ); // 已过 → 明天
}

#[test]
fn weekly_occurrence() {
    // 2026-08-16 是周日（weekday=7）
    let after = dt(2026, 8, 16, 9, 0);
    assert_eq!(
        occurrence_after("weekly:1:09:00", after),
        Some(dt(2026, 8, 17, 9, 0))
    ); // 本周一已过 → 下周一
    assert_eq!(
        occurrence_after("weekly:7:10:00", after),
        Some(dt(2026, 8, 16, 10, 0))
    ); // 今天周日 10 点未到
    assert_eq!(
        occurrence_after("weekly:7:08:00", after),
        Some(dt(2026, 8, 23, 8, 0))
    ); // 已过 → 下周日
}

#[test]
fn at_occurrence() {
    let after = dt(2026, 8, 16, 9, 0);
    assert_eq!(
        occurrence_after("at:2026-08-16T10:00", after),
        Some(dt(2026, 8, 16, 10, 0))
    );
    assert_eq!(occurrence_after("at:2026-08-16T08:00", after), None); // 已过，一次性不再触发
}

#[test]
fn monthly_occurrence() {
    let after = dt(2026, 8, 16, 9, 0);
    // 本月 16 日 10:00 未到 → 今天
    assert_eq!(
        occurrence_after("monthly:16:10:00", after),
        Some(dt(2026, 8, 16, 10, 0))
    );
    // 本月 15 日已过 → 下月 15 日
    assert_eq!(
        occurrence_after("monthly:15:10:00", after),
        Some(dt(2026, 9, 15, 10, 0))
    );
    // 2 月无 31 日 → 顺延到 3 月 31 日（after=2026-01-20）
    let jan = dt(2026, 1, 20, 9, 0);
    assert_eq!(
        occurrence_after("monthly:31:08:00", jan),
        Some(dt(2026, 1, 31, 8, 0))
    );
    let feb = dt(2026, 2, 1, 9, 0);
    assert_eq!(
        occurrence_after("monthly:31:08:00", feb),
        Some(dt(2026, 3, 31, 8, 0))
    );
}

#[test]
fn at_expired_detection() {
    let now = dt(2026, 8, 16, 10, 0);
    // 从未执行且已过期 → 放弃
    assert!(at_expired("at:2026-08-16T09:00", None, now));
    // 未来 → 不放弃
    assert!(!at_expired("at:2026-08-16T11:00", None, now));
    // 执行过（sched_last 有值）→ 交给 occurrence_after 判断，不在此放弃
    assert!(!at_expired("at:2026-08-16T09:00", Some(1), now));
    // 非 at: 格式 → 不适用
    assert!(!at_expired("daily:09:00", None, now));
    // 坏数据 → 放弃
    assert!(at_expired("at:junk", None, now));
}

#[test]
fn resolve_local_equals_single_for_normal_times() {
    // 非 DST 切换日的普通时刻：resolve_local 与 single 等价（DST 行为依赖系统
    // 时区，单测环境（国内无 DST）只能锁正常路径不回归）
    let naive = chrono::NaiveDate::from_ymd_opt(2026, 8, 16)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap();
    assert_eq!(
        resolve_local(naive),
        naive.and_local_timezone(chrono::Local).single()
    );
}

#[test]
fn invalid_schedule() {
    let after = dt(2026, 8, 16, 9, 0);
    assert_eq!(occurrence_after("daily:25:00", after), None);
    assert_eq!(occurrence_after("weekly:8:09:00", after), None);
    assert_eq!(occurrence_after("", after), None);
    assert_eq!(occurrence_after("junk", after), None);
    assert_eq!(occurrence_after("at:not-a-time", after), None);
    assert_eq!(occurrence_after("monthly:0:09:00", after), None);
    assert_eq!(occurrence_after("monthly:32:09:00", after), None);
    assert_eq!(occurrence_after("monthly:5:25:00", after), None);
}

// ── recurring 补跑 2h 时效窗口 ──

fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    dt(y, mo, d, h, mi).timestamp_millis()
}

#[test]
fn classify_due_fresh_within_window_runs() {
    // daily 10:00，昨天 10:00 跑过，现在 10:30——窗口内，正常执行
    let now = dt(2026, 8, 16, 10, 30);
    assert_eq!(
        classify_due("daily:10:00", Some(ms(2026, 8, 15, 10, 0)), now),
        DueVerdict::Run
    );
}

#[test]
fn classify_due_stale_beyond_window_missed() {
    // daily 10:00，上次跑是 3 天前——下一 occurrence 在 3 天前，超窗 → 跳过
    let now = dt(2026, 8, 16, 12, 0);
    assert_eq!(
        classify_due("daily:10:00", Some(ms(2026, 8, 13, 10, 0)), now),
        DueVerdict::Missed
    );
    // weekly / monthly 同理
    assert_eq!(
        classify_due("weekly:1:09:00", Some(ms(2026, 8, 3, 9, 0)), now),
        DueVerdict::Missed
    );
    assert_eq!(
        classify_due("monthly:1:08:00", Some(ms(2026, 7, 1, 8, 0)), now),
        DueVerdict::Missed
    );
}

#[test]
fn classify_due_window_boundary() {
    // 到点恰好 2h 前 → 仍执行；2h1s 前 → 跳过
    let last = ms(2026, 8, 15, 10, 0);
    assert_eq!(
        classify_due("daily:10:00", Some(last), dt(2026, 8, 16, 12, 0)),
        DueVerdict::Run
    );
    assert_eq!(
        classify_due("daily:10:00", Some(last), dt(2026, 8, 16, 12, 1)),
        DueVerdict::Missed
    );
}

#[test]
fn classify_due_new_task_first_run_preserved() {
    // sched_last=None（新任务）保持「下一次触发立即生效」首跑语义，
    // 不因 occurrence 落在 1970 被误判 Missed
    let now = dt(2026, 8, 16, 12, 0);
    assert_eq!(classify_due("daily:10:00", None, now), DueVerdict::Run);
}

// ── 开关关闭时的本轮处置 ──

fn job(schedule: &str) -> crate::db::ScheduledJob {
    crate::db::ScheduledJob {
        id: "j1".into(),
        content: "做事".into(),
        schedule: schedule.into(),
        sched_last: None,
        enabled: Some(true),
        created_at: None,
        updated_at: None,
        last_status: None,
        last_error: None,
        pause_on_failure: None,
        retry_max: None,
        retry_at: None,
        retry_count: None,
        expected_updated_at: None,
    }
}

/// 机器人开关关闭时到点的作业不派发——派发了也会被 run_scheduled_job 拒掉，
/// 每 30s 一轮 spawn + 一条跳过审计就是一天几千条噪声。
#[test]
fn classify_job_due_held_when_bot_off() {
    let now = dt(2026, 8, 16, 10, 30);
    let j = job("daily:10:00");
    assert_eq!(classify_job(&j, now, true), JobAction::Dispatch);
    assert_eq!(classify_job(&j, now, false), JobAction::Held);
}

/// 开关关闭判 Held 时水位不推进——恢复后仍在 2h 窗口内补跑（语义不变）。
#[test]
fn classify_job_held_keeps_catchup_semantics() {
    // 昨天 10:00 跑过、今天 10:00 到点，现在 10:30（窗口内）
    let now = dt(2026, 8, 16, 10, 30);
    let mut j = job("daily:10:00");
    j.sched_last = Some(ms(2026, 8, 15, 10, 0));
    assert_eq!(classify_job(&j, now, false), JobAction::Held);
    // 恢复后（开关开启）判派发 → 窗口内补跑
    assert_eq!(classify_job(&j, now, true), JobAction::Dispatch);
}

#[test]
fn classify_job_not_due_and_missed() {
    let now = dt(2026, 8, 16, 10, 30);
    // 今天 12:00 未到（跑过一次的作业才有「未到点」这回事：
    // sched_last=None 的新作业按首跑语义立即生效）
    let mut j = job("daily:12:00");
    j.sched_last = Some(ms(2026, 8, 15, 12, 0));
    assert_eq!(classify_job(&j, now, true), JobAction::Idle);
    // 超补跑窗口 → 消费（与开关无关）
    let mut j = job("daily:10:00");
    j.sched_last = Some(ms(2026, 8, 15, 10, 0));
    assert_eq!(
        classify_job(&j, dt(2026, 8, 16, 13, 0), true),
        JobAction::Missed
    );
    assert_eq!(
        classify_job(&j, dt(2026, 8, 16, 13, 0), false),
        JobAction::Missed
    );
}

#[test]
fn classify_job_expired_at_is_deleted_regardless_of_switch() {
    let now = dt(2026, 8, 16, 12, 0);
    let j = job("at:2026-08-10T10:00");
    assert_eq!(classify_job(&j, now, true), JobAction::Delete);
    assert_eq!(classify_job(&j, now, false), JobAction::Delete);
}

/// 待触发的重试：到点且开关开启才派发；开关关闭同样不派发（不产生跳过噪声）。
#[test]
fn classify_job_retry_paths() {
    let now = dt(2026, 8, 16, 10, 30);
    let mut j = job("daily:10:00");
    j.retry_at = Some(now.timestamp_millis() - 1000);
    assert_eq!(classify_job(&j, now, true), JobAction::RetryDispatch);
    assert_eq!(classify_job(&j, now, false), JobAction::Held);
    // 未到点 → 本轮跳过
    j.retry_at = Some(now.timestamp_millis() + 60_000);
    assert_eq!(classify_job(&j, now, true), JobAction::RetryPending);
}

/// 重试保鲜窗口：拖过 CATCHUP_WINDOW 的过期重试不再恢复瞬间立刻跑，
/// 丢弃重试回到正常排期（暂停数日恢复是典型形态——重试本该 5 分钟内
/// 发生，拖几天后跑一次只会让用户莫名其妙）。
#[test]
fn classify_job_stale_retry_is_dropped_not_dispatched() {
    let now = dt(2026, 8, 16, 10, 30);
    let mut j = job("daily:10:00");
    // 恰好在窗口边界内（< 2h）→ 照常派发
    j.retry_at = Some(now.timestamp_millis() - CATCHUP_WINDOW.num_milliseconds() + 60_000);
    assert_eq!(classify_job(&j, now, true), JobAction::RetryDispatch);
    // 拖过窗口 → 丢弃重试
    j.retry_at = Some(now.timestamp_millis() - CATCHUP_WINDOW.num_milliseconds() - 1);
    assert_eq!(classify_job(&j, now, true), JobAction::RetryStale);
    assert_eq!(classify_job(&j, now, false), JobAction::RetryStale);
    // 暂停数日恢复的典型形态：retry_at 远在过去
    j.retry_at = Some(ms(2026, 8, 10, 10, 00));
    assert_eq!(classify_job(&j, now, true), JobAction::RetryStale);
}

#[test]
fn classify_due_at_never_missed() {
    // at: 一次性任务不走补跑窗口（由 at_expired 放弃逻辑处理）：
    // 过去的一次性任务保持既有 Run 判定（jobs_tick 的 stale 清理负责清场）
    let now = dt(2026, 8, 16, 12, 0);
    assert_eq!(
        classify_due("at:2026-08-10T10:00", None, now),
        DueVerdict::Run
    );
}

#[test]
fn classify_due_not_due() {
    let now = dt(2026, 8, 16, 9, 0);
    // 今天 10:00 未到
    assert_eq!(
        classify_due("daily:10:00", Some(ms(2026, 8, 15, 10, 0)), now),
        DueVerdict::NotDue
    );
    // 已跑过的一次性任务不再触发
    assert_eq!(
        classify_due(
            "at:2026-08-16T10:00",
            Some(ms(2026, 8, 16, 10, 0)),
            dt(2026, 8, 16, 11, 0)
        ),
        DueVerdict::NotDue
    );
    // 坏格式
    assert_eq!(classify_due("junk", None, now), DueVerdict::NotDue);
}
