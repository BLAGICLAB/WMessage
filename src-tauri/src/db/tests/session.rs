//! 会话删除原子性与在途子 agent 的连带取消。

use super::*;

/// bot_session_delete 原子性：消息与会话同一事务，任一失败不留下半删状态。
/// 直调生产 bot_session_delete_inner（内联复刻命令体裸 SQL 会让生产改动测试不红）。
#[test]
fn bot_session_delete_is_atomic() {
    let (dir, conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', 'user', 'm1', 1000)",
        [],
    )
    .unwrap();

    let mut conn = conn;
    bot_session_delete_inner(&mut conn, "s1").unwrap();

    let sess_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_sessions WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let msg_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sess_count, 0, "提交后会话应被删除");
    assert_eq!(msg_count, 0, "提交后消息应被删除");

    // 子 agent 行级联：终态随会话走，在途行留着（runner 还要靠它收尾）
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_done', 'succeeded', 'research', 's1', 'o1', 't1', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_live', 'running', 'research', 's1', 'o2', 't2', 1000)",
        [],
    )
    .unwrap();
    bot_session_delete_inner(&mut conn, "s1").unwrap();
    let done_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM subagents WHERE id = 'sa_done'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let live_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM subagents WHERE id = 'sa_live'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(done_left, 0, "会话删除后终态子 agent 行不应残留");
    assert_eq!(live_left, 1, "在途子 agent 行不能删——runner 还要靠它收尾");
    let live_status: String = conn
        .query_row(
            "SELECT status FROM subagents WHERE id = 'sa_live'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        live_status, "cancelled",
        "在途子 agent 必须随父会话一起取消——父会话没了，它的产物没人收、\
         它的行没人看得见，再跑下去纯烧预算"
    );

    // 原子性反向验证：第二条 DELETE（会话）注入失败 → 第一条（消息）也必须回滚
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s2', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s2', 'user', 'm2', 1000)",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER fail_sess_del BEFORE DELETE ON bot_sessions
         WHEN OLD.id = 's2' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
    )
    .unwrap();
    assert!(
        bot_session_delete_inner(&mut conn, "s2").is_err(),
        "trigger 注入失败必须返回 Err"
    );
    let sess_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_sessions WHERE id = 's2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let msg_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(sess_count, 1, "回滚后会话必须还在");
    assert_eq!(msg_count, 1, "回滚后消息必须还在（不留半删状态）");

    fs::remove_dir_all(&dir).ok();
}

/// 删会话时在途子 agent 一并取消：排队中与执行中各一条，外加一条以
/// `session_id` 挂在这个会话下的行——三个入口都得收干净。
///
/// 父会话没了 → 产物没人收、行没人看得见、还照烧完整份预算。
#[test]
fn deleting_a_session_cancels_its_live_subagents() {
    let (dir, conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_q', 'queued', 'research', 's1', 'o1', 't1', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, parent_session_id, objective, trace_id, created_at)
         VALUES ('sa_r', 'running', 'research', 's1', 'o2', 't2', 1000)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subagents (id, status, profile, session_id, objective, trace_id, created_at)
         VALUES ('sa_own', 'running', 'research', 's1', 'o3', 't3', 1000)",
        [],
    )
    .unwrap();

    let mut conn = conn;
    bot_session_delete_inner(&mut conn, "s1").unwrap();

    for id in ["sa_q", "sa_r", "sa_own"] {
        let status: String = conn
            .query_row(
                &format!("SELECT status FROM subagents WHERE id = '{id}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "cancelled", "{id} 应随父会话一起取消");
        let error: String = conn
            .query_row(
                &format!("SELECT error FROM subagents WHERE id = '{id}'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            error.contains("父会话已删除"),
            "{id} 应记明取消原因：{error}"
        );
    }

    fs::remove_dir_all(&dir).ok();
}
