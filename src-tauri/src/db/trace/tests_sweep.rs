use super::tests::{mk_trace, setup_conn};
use super::*;

/// 启动清扫：上一进程遗留的 running 行收尾成 stopped（带 finished_at 与中断
/// 说明），done/failed/stopped 不动；重复清扫幂等返回 0。
/// 复现的是「进程被杀后 stale running 永久残留，围观侧拿 running 判 inflight
/// 锁死会话」的实机发现（F-5）。
#[test]
fn sweep_stale_running_traces_marks_only_running_rows() {
    let conn = setup_conn();
    let stale = mk_trace(&conn, "sess-stale", None, "chat", 1_000);
    let done = mk_trace(&conn, "sess-done", None, "chat", 2_000);
    trace_finish(
        &conn,
        done,
        &TraceFinish {
            status: TRACE_STATUS_DONE,
            finished_at: 3_000,
            turn_count: 1,
            tool_calls: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            error: None,
            model: None,
        },
    )
    .unwrap();

    let n = sweep_stale_running_traces(&conn).unwrap();
    assert_eq!(n, 1, "只收 running 行");

    let (status, finished, err): (String, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT status, finished_at, error FROM exec_traces WHERE id = ?1",
            [stale],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, TRACE_STATUS_STOPPED);
    assert!(finished.is_some(), "收尾行必须带 finished_at");
    assert!(err.unwrap_or_default().contains("进程重启中断"));

    let done_status: String = conn
        .query_row(
            "SELECT status FROM exec_traces WHERE id = ?1",
            [done],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done_status, TRACE_STATUS_DONE, "终态行不被清扫波及");

    assert_eq!(
        sweep_stale_running_traces(&conn).unwrap(),
        0,
        "重复清扫幂等"
    );
}
