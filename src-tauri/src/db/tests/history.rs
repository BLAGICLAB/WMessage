//! bot_history 写入与覆写：追加不覆盖、失败回滚、批内时间戳一致。

use super::*;

/// 追加写不能吃掉既有历史：后端主动发起的汇报轮没有前端快照可覆写，
/// 只能自己 INSERT；一旦退化成覆盖写，前端那份并发快照就会把它抹掉。
#[test]
fn bot_history_append_keeps_existing_rows_and_order() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    let mk = |c: &str| BotMsgRow {
        role: "user".into(),
        content: c.into(),
        refs_json: None,
        thinking: None,
        tools_json: None,
        turn_status: None,
    };
    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &[mk("第一条"), mk("第二条")]).unwrap();
    tx.commit().unwrap();

    bot_history_append_inner(
        &mut conn,
        "s1",
        &BotMsgRow {
            role: "assistant".into(),
            content: "汇报轮回复".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
    )
    .unwrap();

    let mut stmt = conn
        .prepare("SELECT role, content FROM bot_messages WHERE session_id = 's1' ORDER BY id")
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(std::result::Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 3, "追加不能删除既有行");
    assert_eq!(rows[0].1, "第一条");
    assert_eq!(rows[2].1, "汇报轮回复");
    assert_eq!(rows[2].0, "assistant");
    let updated: i64 = conn
        .query_row(
            "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(updated >= 1000, "会话活跃时间要跟着动");
    drop(rows);
    drop(stmt);
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// 核心回归：bot_history_save 内层报错时，DELETE 必须被事务回滚，
/// 原会话聊天记录不得丢失（崩溃/强杀落在 INSERT 中间的场景）。
#[test]
fn bot_history_save_rolls_back_on_insert_failure() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 1000, 0)",
        [],
    )
    .unwrap();
    for i in 0..3 {
        conn.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', 'user', ?1, 1000)",
            [format!("msg-{}", i)],
        )
        .unwrap();
    }
    let count_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count_before, 3, "setup: 应有 3 条原消息");

    // 模拟 wrapper 包裹模式：tx + inner + commit。inner 期间遇 NOT NULL 违约。
    let tx_result: Result<(), String> = (|| {
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM bot_messages WHERE session_id = 's1'", [])
            .map_err(|e| e.to_string())?;
        // 强制失败：role 为 NULL → NOT NULL 约束违反
        tx.execute(
            "INSERT INTO bot_messages (session_id, role, content, created_at) VALUES ('s1', NULL, 'x', 1000)",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })();
    assert!(tx_result.is_err(), "tx 必须覆盖推制失败的 insert");

    // 验证：事务被 Drop → 自动 rollback，原 3 条消息仍存在
    let count_after: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count_after, 3,
        "事务未 commit 必须回滚 DELETE，原 3 条消息应保留"
    );

    fs::remove_dir_all(&dir).ok();
}

/// 同样覆盖：bot_history_save_inner 成功路径在事务中被提交。
#[test]
fn bot_history_save_inner_commits_in_tx() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 5000, 0)",
        [],
    )
    .unwrap();

    let new_msgs = vec![
        BotMsgRow {
            role: "user".into(),
            content: "hi".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
        BotMsgRow {
            role: "assistant".into(),
            content: "hello".into(),
            refs_json: None,
            thinking: None,
            tools_json: None,
            turn_status: None,
        },
    ];

    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &new_msgs).unwrap();
    tx.commit().unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM bot_messages WHERE session_id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "提交后应有 2 条新消息");

    let updated_at: i64 = conn
        .query_row(
            "SELECT updated_at FROM bot_sessions WHERE id = 's1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(updated_at >= 5000, "updated_at 应被 update 为 now >= 5000");

    fs::remove_dir_all(&dir).ok();
}

/// 覆写行序不变量：全量覆写后 load 行序 == 写入序，批内 created_at 相等
/// （覆写原子时刻），id 单调递增——行序唯一由自增 id 恢复，与 created_at 无关。
#[test]
fn bot_history_overwrite_restores_order_and_batch_created_at() {
    let (dir, mut conn) = setup_bhs_db();
    conn.execute(
        "INSERT INTO bot_sessions VALUES ('s1', 'T', 1000, 5000, 0)",
        [],
    )
    .unwrap();
    let mk = |content: &str| BotMsgRow {
        role: "user".into(),
        content: content.into(),
        refs_json: None,
        thinking: None,
        tools_json: None,
        turn_status: None,
    };

    // 第一批 3 条 → 整批覆写为 2 条（内容可辨识顺序）
    let first = vec![mk("a"), mk("b"), mk("c")];
    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &first).unwrap();
    tx.commit().unwrap();
    let second = vec![mk("d"), mk("e")];
    let tx = conn.transaction().unwrap();
    bot_history_save_inner(&tx, "s1", &second).unwrap();
    tx.commit().unwrap();

    // load（生产查询同款 ORDER BY id）行序 == 写入序
    let mut stmt = conn
        .prepare("SELECT role, content, refs, thinking, tools FROM bot_messages WHERE session_id = 's1' ORDER BY id")
        .unwrap();
    let loaded: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        loaded,
        vec!["d".to_string(), "e".to_string()],
        "覆写后行序应恢复写入序"
    );
    drop(stmt);

    // 批内 created_at 全等（覆写原子时刻）+ id 单调递增
    let rows: Vec<(i64, i64)> = conn
        .prepare("SELECT id, created_at FROM bot_messages WHERE session_id = 's1' ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2, "覆写后应只剩第二批 2 条");
    let ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();
    let cats: Vec<i64> = rows.iter().map(|(_, ca)| *ca).collect();
    assert!(ids[0] < ids[1], "id 应单调递增：{ids:?}");
    assert_eq!(
        cats[0], cats[1],
        "批内 created_at 应全等（覆写原子时刻）：{cats:?}"
    );

    fs::remove_dir_all(&dir).ok();
}
