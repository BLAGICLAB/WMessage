use super::*;

fn setup_conn() -> rusqlite::Connection {
    super::super::test_conn(&[TASKS_DDL])
}

fn run_patch(
    conn: &mut rusqlite::Connection,
    id: &str,
    patch: serde_json::Value,
) -> CommandResult<super::Task> {
    let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求
    task_patch_locked(conn, id, &patch, 9000)
}

#[test]
fn task_patch_applies_fields_and_null_clears() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, completed_at, archived, updated_at)
         VALUES ('a', '任务A', 'done', 111, 1, 1000)",
        [],
    )
    .unwrap();
    // 归档恢复语义：archived=false + completedAt 重置为 now（服务端打戳）
    let row = run_patch(&mut conn, "a", serde_json::json!({"archived": false})).unwrap();
    assert_eq!(row.archived, Some(false));
    assert_eq!(row.updated_at, Some(9000));
    // null = 清空：deletedAt（软删恢复语义）
    let row = run_patch(
        &mut conn,
        "a",
        serde_json::json!({"deletedAt": 123, "column": "todo"}),
    )
    .unwrap();
    assert_eq!(row.deleted_at, Some(123));
    assert_eq!(row.column, TaskStatus::Todo);
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "a")
        .unwrap();
    assert_eq!(reloaded.deleted_at, Some(123));
}

#[test]
fn task_patch_rejects_unknown_and_protected_keys() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('b', '任务B', 'todo', 1000)",
        [],
    )
    .unwrap();
    for bad in ["foo", "id", "updatedAt", "expectedUpdatedAt"] {
        let err = match run_patch(&mut conn, "b", serde_json::json!({ bad: 1 })) {
            Err(e) => e,
            Ok(_) => panic!("{bad} 应被拒绝"),
        };
        assert!(
            matches!(err, CommandError::InvalidArgument { .. }),
            "{bad} 应 InvalidArgument，got {err:?}"
        );
    }
}

#[test]
fn task_patch_rejects_self_dependency() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('a', '任务A', 'todo', 1000)",
        [],
    )
    .unwrap();
    // 自己依赖自己：写进去的卡在图谱里连出一条自环边，度数虚高
    let err = match run_patch(&mut conn, "a", serde_json::json!({"dependsOn": ["a"]})) {
        Err(e) => e,
        Ok(_) => panic!("自环依赖应被拒绝"),
    };
    assert!(
        matches!(err, CommandError::InvalidArgument { .. }),
        "自环依赖应 InvalidArgument，got {err:?}"
    );
    // 正常依赖不受影响
    let row = run_patch(&mut conn, "a", serde_json::json!({"dependsOn": ["b"]})).unwrap();
    assert_eq!(row.depends_on.as_deref(), Some(&["b".to_string()][..]));
}

#[test]
fn task_patch_rejects_empty_title_and_missing_id() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('c', '任务C', 'todo', 1000)",
        [],
    )
    .unwrap();
    assert!(matches!(
        run_patch(&mut conn, "c", serde_json::json!({"title": "  "})),
        Err(CommandError::InvalidArgument { .. })
    ));
    assert!(matches!(
        run_patch(&mut conn, "ghost", serde_json::json!({"title": "x"})),
        Err(CommandError::TaskNotFound(_))
    ));
}

/// 子 agent 编排三字段（assignee/budget/result）——设值、null 清空、
/// 往返落库一致（JSON TEXT 序列化）。
#[test]
fn task_patch_orchestration_fields_set_and_null_clears() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('suba', '子任务卡', 'doing', 1000)",
        [],
    )
    .unwrap();
    let row = run_patch(
        &mut conn,
        "suba",
        serde_json::json!({
            "assignee": "sess-child-1",
            "budget": {"maxTurns": 30, "maxToolCalls": 100, "maxWallSeconds": 600},
            "result": {"status": "succeeded", "summary": "结论"}
        }),
    )
    .unwrap();
    assert_eq!(row.assignee.as_deref(), Some("sess-child-1"));
    assert_eq!(row.budget.as_ref().unwrap().max_turns, 30);
    assert_eq!(row.result.as_ref().unwrap()["status"], "succeeded");
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "suba")
        .unwrap();
    assert_eq!(reloaded.assignee.as_deref(), Some("sess-child-1"));
    // 往返后按值复核（不只 is_some）：JSON TEXT 序列化无损
    let rb = reloaded.budget.as_ref().unwrap();
    assert_eq!(rb.max_turns, 30);
    assert_eq!(rb.max_tool_calls, 100);
    assert_eq!(rb.max_wall_seconds, 600);
    assert_eq!(reloaded.result.as_ref().unwrap()["summary"], "结论");
    // null = 清空三字段
    let cleared = run_patch(
        &mut conn,
        "suba",
        serde_json::json!({"assignee": null, "budget": null, "result": null}),
    )
    .unwrap();
    assert_eq!(cleared.assignee, None);
    assert_eq!(cleared.budget, None);
    assert_eq!(cleared.result, None);
}

/// budget 走 task_patch 也必须被硬顶钳制（写口强制，防旁路）
#[test]
fn task_patch_budget_is_clamped_to_hard_cap() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('clamp', '预算钳制', 'doing', 1000)",
        [],
    )
    .unwrap();
    let row = run_patch(
        &mut conn,
        "clamp",
        serde_json::json!({"budget": {"maxTurns": 9999, "maxToolCalls": 0, "maxWallSeconds": 0}}),
    )
    .unwrap();
    let b = row.budget.as_ref().unwrap();
    assert_eq!(b.max_turns, 50, "上超钳到硬顶");
    assert_eq!(b.max_tool_calls, 1, "下超钳到下限");
    // 墙钟下界与配置键同源（30s）——「1 秒墙钟」等于白派一个子 agent
    assert_eq!(
        b.max_wall_seconds, 30,
        "下超钳到墙钟下限（与 subagent.max_wall_secs 同源）"
    );
}

/// budget/result 列损坏 → 兜底 None + 行仍可读（同 subtasks/files 契约）
#[test]
fn load_all_tolerates_corrupted_budget_result_json() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('bad', '损坏行', 'doing', 1000)",
        [],
    )
    .unwrap();
    run_patch(
        &mut conn,
        "bad",
        serde_json::json!({"assignee": "keep-me", "budget": {"maxTurns": 10, "maxToolCalls": 10, "maxWallSeconds": 60}}),
    )
    .unwrap();
    conn.execute("UPDATE tasks SET result = '{not-json' WHERE id = 'bad'", [])
        .unwrap();
    let row = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "bad")
        .unwrap();
    assert_eq!(row.result, None, "损坏 result 必须兜底 None");
    assert_eq!(
        row.assignee.as_deref(),
        Some("keep-me"),
        "同行其他字段不受影响"
    );
    assert!(
        row.budget.is_some(),
        "完好的 budget 不受同行的损坏 result 影响"
    );
}

/// 计划起止（周时间网格）：设值成对生效、null 清除、往返落库一致
#[test]
fn task_patch_plan_fields_set_null_clears_and_roundtrips() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('plan', '排期卡', 'todo', 1000)",
        [],
    )
    .unwrap();
    let row = run_patch(
        &mut conn,
        "plan",
        serde_json::json!({
            "planStart": "2026-10-07T09:30",
            "planEnd": "2026-10-07T10:30"
        }),
    )
    .unwrap();
    assert_eq!(row.plan_start.as_deref(), Some("2026-10-07T09:30"));
    assert_eq!(row.plan_end.as_deref(), Some("2026-10-07T10:30"));
    // 往返：落库后重读一致（DB 列 TEXT 直存）
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "plan")
        .unwrap();
    assert_eq!(reloaded.plan_start.as_deref(), Some("2026-10-07T09:30"));
    assert_eq!(reloaded.plan_end.as_deref(), Some("2026-10-07T10:30"));
    // null = 清除排期（拖回任务池语义）
    let row = run_patch(
        &mut conn,
        "plan",
        serde_json::json!({"planStart": null, "planEnd": null}),
    )
    .unwrap();
    assert_eq!(row.plan_start, None);
    assert_eq!(row.plan_end, None);
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "plan")
        .unwrap();
    assert_eq!(reloaded.plan_start, None);
    assert_eq!(reloaded.plan_end, None);
}

/// 计划起止：非法格式（错格式/越界月份）拒写且不落库
#[test]
fn task_patch_plan_fields_reject_invalid_format() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('badp', '排期卡', 'todo', 1000)",
        [],
    )
    .unwrap();
    for bad in [
        "明天下午",
        "2026-10-07",
        "2026-13-01T10:00",
        "2026-10-07T09:70",
    ] {
        let err = match run_patch(
            &mut conn,
            "badp",
            serde_json::json!({ "planStart": bad, "planEnd": "2026-10-07T10:00" }),
        ) {
            Err(e) => e,
            Ok(_) => panic!("{bad} 应被拒绝"),
        };
        assert!(
            matches!(err, CommandError::InvalidArgument { .. }),
            "{bad} 应 InvalidArgument，got {err:?}"
        );
    }
    // 拒写后不产生半写状态：两字段都保持 NULL
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "badp")
        .unwrap();
    assert_eq!(reloaded.plan_start, None);
    assert_eq!(reloaded.plan_end, None);
}

/// 截止时间格式：两种合法形态放行，其余一律拒。
///
/// 半截值（"2026-10"）与越界值（2026-02-31 / 25:00）落进库里的话，
/// due_notify 解析失败是「视为无截止」——任务就这么无声地过期了。
#[test]
fn task_patch_due_rejects_invalid_format_and_accepts_both_valid_shapes() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('duet', '截止卡', 'todo', 1000)",
        [],
    )
    .unwrap();
    for bad in [
        "2026-10",
        "明天下午",
        "2026-02-31",
        "2026-10-07T25:00",
        "2026-10-07T09:70",
        "2026-10-07 09:00",
    ] {
        let err = match run_patch(&mut conn, "duet", serde_json::json!({ "due": bad })) {
            Err(e) => e,
            Ok(_) => panic!("{bad} 应被拒绝"),
        };
        assert!(
            matches!(err, CommandError::InvalidArgument { .. }),
            "{bad} 应 InvalidArgument，got {err:?}"
        );
    }
    // 两种合法形态都放行
    for good in ["2026-10-07", "2026-10-07T09:00"] {
        run_patch(&mut conn, "duet", serde_json::json!({ "due": good }))
            .unwrap_or_else(|e| panic!("{good} 应放行，got {e:?}"));
        let reloaded = load_all(&conn)
            .unwrap()
            .into_iter()
            .find(|t| t.id == "duet")
            .unwrap();
        assert_eq!(reloaded.due.as_deref(), Some(good));
    }
    // null 清空
    run_patch(&mut conn, "duet", serde_json::json!({ "due": null })).unwrap();
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "duet")
        .unwrap();
    assert_eq!(reloaded.due, None);
}

/// UI 主写口的 tags 双上限：与 bot 工具 / HTTP 同一常量（MAX_TAGS=10 /
/// MAX_TAG_LEN=30）。这条写口此前零校验——UI 写入的超长标签，后续经
/// bot edit_task 提交或选为整理主标签时会被同一常量拒绝，两写口打架。
#[test]
fn task_patch_tags_enforces_shared_limits() {
    let mut conn = setup_conn();
    conn.execute(
        "INSERT INTO tasks (id, title, col, updated_at) VALUES ('tagt', '标签卡', 'todo', 1000)",
        [],
    )
    .unwrap();
    // 超长标签
    let err = match run_patch(
        &mut conn,
        "tagt",
        serde_json::json!({ "tags": ["x".repeat(crate::bot::MAX_TAG_LEN + 1)] }),
    ) {
        Err(e) => e,
        Ok(_) => panic!("超长标签应被拒绝"),
    };
    assert!(
        format!("{err:?}").contains("标签"),
        "超长标签应被拒：{err:?}"
    );
    // 超数量
    let many: Vec<String> = (0..=crate::bot::MAX_TAGS)
        .map(|i| format!("t{i}"))
        .collect();
    let err = match run_patch(&mut conn, "tagt", serde_json::json!({ "tags": many })) {
        Err(e) => e,
        Ok(_) => panic!("超数量标签应被拒绝"),
    };
    assert!(
        format!("{err:?}").contains("标签"),
        "超数量标签应被拒：{err:?}"
    );
    // 边界内放行 + 空数组清除
    run_patch(&mut conn, "tagt", serde_json::json!({ "tags": ["a", "b"] })).unwrap();
    run_patch(&mut conn, "tagt", serde_json::json!({ "tags": [] })).unwrap();
    let reloaded = load_all(&conn)
        .unwrap()
        .into_iter()
        .find(|t| t.id == "tagt")
        .unwrap();
    assert_eq!(reloaded.tags, None);
}
