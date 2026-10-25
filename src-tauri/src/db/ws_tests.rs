use super::*;
use crate::error::CommandError;
use std::fs;

/// 折叠状态往返：upsert collapsed=true → load 读回一致
#[test]
fn workspace_collapsed_roundtrip() {
    let dir = std::env::temp_dir().join(format!("wm-ws-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
    )
    .unwrap();
    let item = WorkspaceItem {
        id: "w1".into(),
        title: "测试".into(),
        collapsed: Some(true),
        links: vec![WorkspaceLink {
            id: "l1".into(),
            display_name: "别名".into(),
            target_uri: "https://x.com".into(),
            kind: "url".into(),
        }],
        order: Some(1.0),
        updated_at: Some(123),
    };
    upsert_workspace(&mut conn, &[item]).unwrap();
    let got = load_workspace(&conn).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].collapsed, Some(true));
    assert_eq!(got[0].links[0].display_name, "别名");
    fs::remove_dir_all(&dir).ok();
}

/// workspace_* 命令的桥接层（spawn_blocking + join 错误映射）是无逻辑薄壳且依赖
/// AppHandle 无法单测；这里直调生产 helper（upsert/load/delete）覆盖命令体真正
/// 干活的部分（内联复刻桥接结构会让生产命令体改动测试不红，属弱断言）。
#[test]
fn workspace_commands_spawn_blocking_bridge() {
    let dir = std::env::temp_dir().join(format!("wm-ws-async-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
    )
    .unwrap();
    let item = WorkspaceItem {
        id: "w1".into(),
        title: "T".into(),
        collapsed: None,
        links: vec![],
        order: None,
        updated_at: Some(1),
    };

    // upsert → load 回读（生产 helper，同 workspace_upsert / workspace_load 命令体所调）
    upsert_workspace(&mut conn, &[item]).unwrap();
    let items = load_workspace(&conn).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "T");

    // delete（同 workspace_delete 命令体所调）
    delete_workspace(&mut conn, &["w1".to_string()]).unwrap();
    assert!(load_workspace(&conn).unwrap().is_empty());
    fs::remove_dir_all(&dir).ok();
}

///  #2 簇B：写入侧 fail-closed —— upsert 遇非法 kind 返回可辨识错误
/// InvalidWorkspaceLinkKind（带 kind / 来源 / 目标），不静默丢、不降级 url。
/// 白名单内的 kind（url/file/folder）正常落库。
#[test]
fn upsert_rejects_invalid_link_kind() {
    let dir = std::env::temp_dir().join(format!("wm-ws-badkind-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
    )
    .unwrap();
    let mk = |id: &str, kind: &str| WorkspaceItem {
        id: id.into(),
        title: "T".into(),
        collapsed: None,
        links: vec![WorkspaceLink {
            id: "l1".into(),
            display_name: "x".into(),
            target_uri: "/tmp/x.app".into(),
            kind: kind.into(),
        }],
        order: None,
        updated_at: Some(1),
    };

    // 危险 kind + 未来 kind 全部拒（fail-closed）
    for bad in ["app", "command", "future_foo"] {
        match upsert_workspace(&mut conn, &[mk(&format!("w-{bad}"), bad)]).unwrap_err() {
            crate::error::CommandError::InvalidWorkspaceLinkKind { kind, source, link } => {
                assert_eq!(kind, bad);
                assert_eq!(source, "workspace_upsert");
                assert_eq!(link, "/tmp/x.app");
            }
            other => panic!("应为 InvalidWorkspaceLinkKind，实得 {other:?}"),
        }
    }
    // 白名单内的 kind 正常落库
    for ok in ["url", "file", "folder"] {
        upsert_workspace(&mut conn, &[mk(&format!("ok-{ok}"), ok)]).unwrap();
    }
    assert_eq!(load_workspace(&conn).unwrap().len(), 3);

    // 混合批次（合法 + 非法同批）：整批拒绝且**零写入** —— 直接证明
    // validate_link_kinds 在事务之前跑（否则合法项会先落库）。
    let before = load_workspace(&conn).unwrap().len();
    let mixed = vec![mk("ok-mixed", "file"), mk("bad-mixed", "app")];
    assert!(matches!(
        upsert_workspace(&mut conn, &mixed),
        Err(crate::error::CommandError::InvalidWorkspaceLinkKind { .. })
    ));
    assert_eq!(
        load_workspace(&conn).unwrap().len(),
        before,
        "混合批次不得部分写入（fail-closed 必须在事务之前）"
    );
    fs::remove_dir_all(&dir).ok();
}

///  #2 簇B：import 路径同样 fail-closed（workspace_import_merge），
/// 且拒绝时不产生任何写入。
#[test]
fn import_merge_rejects_invalid_link_kind() {
    let dir = std::env::temp_dir().join(format!("wm-ws-impbad-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);",
    )
    .unwrap();
    let item = WorkspaceItem {
        id: "w1".into(),
        title: "T".into(),
        collapsed: None,
        links: vec![WorkspaceLink {
            id: "l1".into(),
            display_name: "x".into(),
            target_uri: "/tmp/evil.command".into(),
            kind: "command".into(),
        }],
        order: None,
        updated_at: Some(1),
    };
    match workspace_import_merge(&mut conn, &[item]).unwrap_err() {
        crate::error::CommandError::InvalidWorkspaceLinkKind { kind, source, link } => {
            assert_eq!(kind, "command");
            assert_eq!(source, "workspace_import_merge");
            assert_eq!(link, "/tmp/evil.command");
        }
        other => panic!("应为 InvalidWorkspaceLinkKind，实得 {other:?}"),
    }
    // fail-closed：无任何写入
    assert!(load_workspace(&conn).unwrap().is_empty());

    // 混合批次（合法 + 非法）针对**已有行**：整批拒绝且不污染/删除既有数据。
    // 先种一行合法数据，再交混合批次，验证拒绝是全局的（在事务之前）。
    let seed = WorkspaceItem {
        id: "seed".into(),
        title: "seed".into(),
        collapsed: None,
        links: vec![WorkspaceLink {
            id: "l0".into(),
            display_name: "s".into(),
            target_uri: "/tmp/ok.txt".into(),
            kind: "file".into(),
        }],
        order: None,
        updated_at: Some(1),
    };
    workspace_import_merge(&mut conn, &[seed]).unwrap();
    assert_eq!(load_workspace(&conn).unwrap().len(), 1);
    let ok_item = WorkspaceItem {
        id: "w-ok".into(),
        title: "ok".into(),
        collapsed: None,
        links: vec![WorkspaceLink {
            id: "l1".into(),
            display_name: "x".into(),
            target_uri: "/tmp/ok.txt".into(),
            kind: "file".into(),
        }],
        order: None,
        updated_at: Some(2),
    };
    let bad_item = WorkspaceItem {
        id: "w-bad".into(),
        title: "bad".into(),
        collapsed: None,
        links: vec![WorkspaceLink {
            id: "l2".into(),
            display_name: "b".into(),
            target_uri: "/tmp/evil.command".into(),
            kind: "command".into(),
        }],
        order: None,
        updated_at: Some(3),
    };
    assert!(matches!(
        workspace_import_merge(&mut conn, &[ok_item, bad_item]),
        Err(crate::error::CommandError::InvalidWorkspaceLinkKind { .. })
    ));
    let after = load_workspace(&conn).unwrap();
    assert_eq!(after.len(), 1, "混合批次不得合并任何行（fail-closed）");
    assert_eq!(after[0].id, "seed");
    fs::remove_dir_all(&dir).ok();
}

/// upsert_workspace 中途失败必须整体回滚——已写入的前序行不得落库。
/// 用 BEFORE INSERT trigger 在第 2 条注入失败（RAISE ABORT），模拟「中途 panic/磁盘错」。
#[test]
fn upsert_workspace_rolls_back_on_mid_loop_failure() {
    let dir = std::env::temp_dir().join(format!("wm-ws-tx-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);
         CREATE TRIGGER fail_boom BEFORE INSERT ON workspace_items
           WHEN NEW.id = 'boom' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
    )
    .unwrap();
    // 预置旧数据：失败回滚后必须保持原状
    conn.execute(
        "INSERT INTO workspace_items VALUES ('w1', 'old', NULL, '[]', NULL, 1)",
        [],
    )
    .unwrap();
    let mk = |id: &str, title: &str| WorkspaceItem {
        id: id.into(),
        title: title.into(),
        collapsed: None,
        links: vec![],
        order: None,
        updated_at: Some(2),
    };
    // w1 更新 + boom（触发失败）+ w2：无事务时 w1 会被半截更新
    let r = upsert_workspace(
        &mut conn,
        &[mk("w1", "new"), mk("boom", "x"), mk("w2", "y")],
    );
    assert!(r.is_err(), "trigger 注入失败必须返回 Err");
    let got = load_workspace(&conn).unwrap();
    assert_eq!(got.len(), 1, "半截写入必须被回滚（w2 不得落库）");
    assert_eq!(got[0].title, "old", "w1 必须保持旧值（事务回滚）");
    fs::remove_dir_all(&dir).ok();
}

/// delete_workspace 中途失败同样整体回滚——前序已删行恢复。
#[test]
fn delete_workspace_rolls_back_on_mid_loop_failure() {
    let dir = std::env::temp_dir().join(format!("wm-ws-tx-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);
         INSERT INTO workspace_items VALUES ('w1', 'a', NULL, '[]', NULL, 1);
         INSERT INTO workspace_items VALUES ('w2', 'b', NULL, '[]', NULL, 1);
         INSERT INTO workspace_items VALUES ('boom', 'c', NULL, '[]', NULL, 1);
         CREATE TRIGGER fail_del BEFORE DELETE ON workspace_items
           WHEN OLD.id = 'boom' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
    )
    .unwrap();
    // w1 删除成功 → boom 行存在，删它触发失败 → 无事务时 w1 已被半截删掉
    let r = delete_workspace(
        &mut conn,
        &["w1".to_string(), "boom".to_string(), "w2".to_string()],
    );
    assert!(r.is_err(), "trigger 注入失败必须返回 Err");
    let got = load_workspace(&conn).unwrap();
    assert_eq!(got.len(), 3, "回滚后 w1/boom/w2 都必须还在：{got:?}");
    fs::remove_dir_all(&dir).ok();
}

/// workspace_import 合并语义 + 跳空 id 脏数据（与 tasks_import 语义一致）：
/// 同 id 保留 updated_at 更晚者；新 id 直接写入；空 id 跳过。
/// 直调生产 workspace_import_merge（inline 复刻合并循环 SQL 会让生产改动测试不红）。
#[test]
fn workspace_import_merges_by_updated_at_and_skips_empty_id() {
    let dir = std::env::temp_dir().join(format!("wm-ws-imp-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);
         INSERT INTO workspace_items VALUES ('a','cur-50',NULL,'[]',NULL,50);
         INSERT INTO workspace_items VALUES ('b','cur-200',NULL,'[]',NULL,200);",
    )
    .unwrap();
    let mk = |id: &str, title: &str, ua: i64| WorkspaceItem {
        id: id.into(),
        title: title.into(),
        collapsed: None,
        links: vec![],
        order: None,
        updated_at: Some(ua),
    };
    let incoming = vec![
        mk("a", "in-100", 100), // 同 id · incoming > cur → UPDATE
        mk("b", "in-180", 180), // 同 id · incoming < cur → 跳过
        mk("c", "in-5", 5),     // 新 id → INSERT
        mk("", "blank", 999),   // 空 id → 跳过
    ];
    let merged = workspace_import_merge(&mut conn, &incoming).unwrap();

    assert_eq!(merged, 2, "应写 a（UPDATE）+ c（INSERT）；跳过 b + 空 id");
    let a: String = conn
        .query_row("SELECT title FROM workspace_items WHERE id='a'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(a, "in-100", "a 被新覆盖");
    let b: String = conn
        .query_row("SELECT title FROM workspace_items WHERE id='b'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(b, "cur-200", "b 未被覆盖（180<200）");
    let c: String = conn
        .query_row("SELECT title FROM workspace_items WHERE id='c'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(c, "in-5", "c 已写入");
    // 空 id 跳过：不应有 id='' 的行（setup 也没创建，double-check）
    let blank_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_items WHERE id=''",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(blank_count, 0, "空 id 应被跳过");
    fs::remove_dir_all(&dir).ok();
}

/// 目标行 updated_at 为 NULL → 整批 Err（fail-closed，tx 回滚）：
/// NULL 与导入行的新旧比较语义未定义，不得静默取 0。
#[test]
fn workspace_import_aborts_on_null_updated_at_target() {
    let dir = std::env::temp_dir().join(format!("wm-ws-null-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let mut conn = rusqlite::Connection::open(dir.join("t.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE workspace_items (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, collapsed INTEGER,
           links TEXT NOT NULL, ord REAL, updated_at INTEGER);
         INSERT INTO workspace_items VALUES ('null-ua','cur-null',NULL,'[]',NULL,NULL);
         INSERT INTO workspace_items VALUES ('ok','cur-50',NULL,'[]',NULL,50);",
    )
    .unwrap();
    let mk = |id: &str, ua: i64| WorkspaceItem {
        id: id.into(),
        title: format!("in-{ua}"),
        collapsed: None,
        links: vec![],
        order: None,
        updated_at: Some(ua),
    };

    // 同批两条：一条命中 NULL 目标行（Err），另一条本可正常合并——整批回滚
    let err = workspace_import_merge(&mut conn, &[mk("ok", 100), mk("null-ua", 1)])
        .expect_err("NULL updated_at 目标行必须中止导入");

    match err {
        CommandError::Internal(ref m) => {
            assert!(
                m.contains("updated_at 为空"),
                "错误消息应指明 NULL 语义未定义，got: {m}"
            );
            assert!(m.contains("null-ua"), "错误消息应指认目标行 id，got: {m}");
        }
        other => panic!("应返 Internal(String)，实际 {other:?}"),
    }

    // tx 回滚：本可合并的 ok 行未被写入，NULL 行未被覆盖
    let ok_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM workspace_items WHERE id='ok' AND title='in-100'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ok_count, 0, "整批回滚：本可合并的行也不得写入");
    let null_title: String = conn
        .query_row(
            "SELECT title FROM workspace_items WHERE id='null-ua'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(null_title, "cur-null", "NULL 目标行内容不得被覆盖");

    fs::remove_dir_all(&dir).ok();
}
