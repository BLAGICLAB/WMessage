//! 工作项启动清扫：翻态与崩溃叙事合成同事务。

use super::*;
use crate::db::work_items::upsert_for_session;

/// 启动清扫把 working / awaiting_answer 收成 failed，且给每个受影响会话
/// 补一条 turn_status=failed 的合成 assistant 行——崩溃后聊天面不再永久
/// 悬空用户行（进程内失败路径有失败行，崩溃路径此前没有）。
#[test]
fn sweep_all_working_synthesizes_failure_rows() {
    let (dir, conn) = setup_bhs_db();
    let _g = crate::db::lock_db_write();
    let mut conn = conn;
    for sid in ["s1", "s2"] {
        conn.execute(
            "INSERT INTO bot_sessions VALUES (?1, 'T', 1000, 1000, 0)",
            [sid],
        )
        .unwrap();
        // 用户行先落库（真源化口径），终稿行未写 = 崩溃窗口
        conn.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES (?1, 'user', '帮我做 X', 1000)",
            [sid],
        )
        .unwrap();
        upsert_for_session(&conn, sid, "chat", Some("做 X"), None).unwrap();
    }
    // 与清扫无关的已完成会话：不应被触碰
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s_ok', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    upsert_for_session(&conn, "s_ok", "chat", Some("已收尾"), None).unwrap();
    crate::db::work_items::set_stage(&conn, "s_ok", "idle", None).unwrap();

    let swept = crate::db::work_items::sweep_all_working(&mut conn).unwrap();
    assert_eq!(
        swept, 2,
        "working + awaiting_answer 收 2 行（这里 2 个 working）"
    );

    for sid in ["s1", "s2"] {
        let stage: String = conn
            .query_row(
                "SELECT stage FROM work_items WHERE session_id = ?1",
                [sid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stage, "failed", "清扫后应 failed：{sid}");
        let (count, turn_status): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(turn_status, '') FROM bot_messages \
                 WHERE session_id = ?1 AND role = 'assistant'",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "合成失败行必须存在：{sid}");
        assert_eq!(turn_status, "failed", "合成行必须是失败轮形态：{sid}");
    }
    let ok_msgs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's_ok'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ok_msgs, 0, "与清扫无关的会话不得被合成行污染");
    let _ = dir;
}
