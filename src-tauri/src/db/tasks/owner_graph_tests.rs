use super::*;
use crate::db::tasks_transfer::normalize_owner;

fn setup_conn() -> rusqlite::Connection {
    super::super::test_conn(&[TASKS_DDL, super::super::people::PEOPLE_DDL])
}

const SELF: &str = "self-pid";

fn task_json(id: &str, owner: Option<&str>, updated: i64) -> String {
    let owner_part = match owner {
        Some(o) => format!(r#""ownerId":"{o}","#),
        None => String::new(),
    };
    format!(r#"{{"id":"{id}","title":"t-{id}","column":"todo",{owner_part}"updatedAt":{updated}}}"#)
}

/// 最小合法 Task 构造（全 None 缺省）
fn min_task(owner: Option<&str>) -> Task {
    Task {
        plan_start: None,
        plan_end: None,
        parent_id: None,
        id: "x".into(),
        title: "t".into(),
        due: None,
        note: None,
        tags: None,
        files: None,
        column: TaskStatus::Todo,
        subtasks: None,
        completed_at: None,
        archived: None,
        deleted_at: None,
        collapsed: None,
        order: None,
        updated_at: None,
        bot_assigned: None,
        assignee: None,
        budget: None,
        result: None,
        depends_on: None,
        model: None,
        owner_id: owner.map(str::to_string),
        created_at: None,
        expected_updated_at: None,
    }
}

#[test]
fn normalize_owner_rules() {
    // 显式外来 pid → Some
    assert_eq!(
        normalize_owner(&min_task(Some("p-a")), Some("env-pid"), SELF),
        Some("p-a".into())
    );
    // 显式本人 pid → None（库内 NULL = 本人）
    assert_eq!(
        normalize_owner(&min_task(Some(SELF)), Some("env"), SELF),
        None
    );
    // 空/空白 ownerId → 回退信封 pid
    assert_eq!(
        normalize_owner(&min_task(Some("  ")), Some("env"), SELF),
        Some("env".into())
    );
    // 信封也是本人 → None
    assert_eq!(normalize_owner(&min_task(Some("")), Some(SELF), SELF), None);
    // 无 ownerId → 信封 pid；信封缺失 → 本人 → None
    assert_eq!(
        normalize_owner(&min_task(None), Some("env"), SELF),
        Some("env".into())
    );
    assert_eq!(normalize_owner(&min_task(None), None, SELF), None);
}

#[test]
fn import_v1_bare_array_treated_as_self() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    let raw = format!(
        "[{},{}]",
        task_json("a", None, 100),
        task_json("b", Some(SELF), 100)
    );
    let merged = import_tasks_conn(&mut conn, &raw, SELF).unwrap();
    assert_eq!(merged, 2);
    for t in load_all(&conn).unwrap() {
        assert_eq!(t.owner_id, None, "v1 裸数组全部归属本人");
    }
}

#[test]
fn import_v2_stamps_foreign_owner_and_upserts_people() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    let raw = format!(
        r#"{{"version":2,"exportedAt":1,
            "profile":{{"personId":"zhang","name":"张三"}},
            "people":[{{"id":"li","name":"李四"}}],
            "tasks":[{},{},{}]}}"#,
        task_json("t1", None, 100),          // 张三的卡 → owner=zhang
        task_json("t2", Some("li"), 100),    // 张三库里李四的卡 → owner=li
        task_json("t3", Some("ghost"), 100), // 未注册归属 → 占位行
    );
    import_tasks_conn(&mut conn, &raw, SELF).unwrap();
    let by_id: std::collections::HashMap<String, Task> = load_all(&conn)
        .unwrap()
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    assert_eq!(by_id["t1"].owner_id.as_deref(), Some("zhang"));
    assert_eq!(by_id["t2"].owner_id.as_deref(), Some("li"));
    assert_eq!(by_id["t3"].owner_id.as_deref(), Some("ghost"));
    let names: std::collections::HashMap<String, String> = super::super::people::people_load(&conn)
        .unwrap()
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    assert_eq!(names["zhang"], "张三");
    assert_eq!(names["li"], "李四");
    assert_eq!(names["ghost"], "未知成员");
}

#[test]
fn import_v2_version_mismatch_and_missing_profile_rejected() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    let bad_ver = r#"{"version":3,"profile":{"personId":"p","name":"n"},"tasks":[]}"#;
    assert!(import_tasks_conn(&mut conn, bad_ver, SELF).is_err());
    let no_profile = r#"{"version":2,"tasks":[]}"#;
    assert!(import_tasks_conn(&mut conn, no_profile, SELF).is_err());
}

#[test]
fn import_garbage_rejected_with_readable_error() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    let err = import_tasks_conn(&mut conn, "这不是 JSON", SELF).unwrap_err();
    assert!(err.contains("不是有效的任务数据 JSON"), "{err}");
}

#[test]
fn export_stamps_self_owner_and_carries_people() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    // 本人的卡（NULL owner）+ 已知外来卡（people 表有注册）
    let raw = format!(
        r#"{{"version":2,"profile":{{"personId":"zhang","name":"张三"}},"tasks":[{}]}}"#,
        task_json("foreign", None, 100),
    );
    import_tasks_conn(&mut conn, &raw, SELF).unwrap();
    super::super::people::people_upsert_entry(&conn, "zhang", "张三", false, None).unwrap();
    let mut mine = min_task(None);
    mine.id = "mine".into();
    mine.title = "我的卡".into();
    mine.updated_at = Some(100);
    upsert_tasks(&conn, &[mine]).unwrap();
    let (json, count) = export_tasks_json(&conn, SELF, "我", None).unwrap();
    assert_eq!(count, 2);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["version"], 2);
    assert_eq!(v["profile"]["personId"], SELF);
    let tasks = v["tasks"].as_array().unwrap();
    let mine = tasks.iter().find(|t| t["id"] == "mine").unwrap();
    assert_eq!(mine["ownerId"], SELF, "NULL owner 导出时盖章为本人 pid");
    let foreign = tasks.iter().find(|t| t["id"] == "foreign").unwrap();
    assert_eq!(foreign["ownerId"], "zhang");
    let people = v["people"].as_array().unwrap();
    assert!(
        people
            .iter()
            .any(|p| p["id"] == "zhang" && p["name"] == "张三"),
        "people 应携带已知成员；实际 {people:?}"
    );
    assert!(
        !people.iter().any(|p| p["id"] == SELF),
        "people 不含本人（本人在 profile 里）"
    );
}

#[test]
fn import_then_export_roundtrip_preserves_attribution() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    let raw = format!(
        r#"{{"version":2,"profile":{{"personId":"zhang","name":"张三"}},"tasks":[{}]}}"#,
        task_json("t1", None, 100),
    );
    import_tasks_conn(&mut conn, &raw, SELF).unwrap();
    let (json, _) = export_tasks_json(&conn, SELF, "我", None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["tasks"][0]["ownerId"], "zhang", "往返后归属不串档");
}

#[test]
fn import_same_id_takes_later_updated_at_with_owner() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    // 先导旧版（本人、ua=100），再导新版同 id（张三、ua=200）→ 更新胜出且换归属
    let raw1 = format!("[{}]", task_json("dup", None, 100));
    import_tasks_conn(&mut conn, &raw1, SELF).unwrap();
    let raw2 = format!(
        r#"{{"version":2,"profile":{{"personId":"zhang","name":"张三"}},"tasks":[{}]}}"#,
        task_json("dup", None, 200),
    );
    import_tasks_conn(&mut conn, &raw2, SELF).unwrap();
    let t = &load_all(&conn).unwrap()[0];
    assert_eq!(t.owner_id.as_deref(), Some("zhang"));
    assert_eq!(t.updated_at, Some(200));
}

// ── 信封头像 ID 往返（profile.avatarId ↔ people.avatar） ──

fn people_avatar(conn: &rusqlite::Connection, pid: &str) -> Option<String> {
    conn.query_row("SELECT avatar FROM people WHERE id = ?1", [pid], |r| {
        r.get::<_, Option<String>>(0)
    })
    .unwrap()
}

/// 导出信封带头像 ID → 导入端按 ID 落 people.avatar（对端显示同一张内置图）
#[test]
fn envelope_avatar_id_roundtrips_into_people() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    let (json, _) = export_tasks_json(&conn, "zhang", "张三", Some("avatar-07")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["profile"]["avatarId"], "avatar-07");
    // 对端（SELF 视角）导入张三的信封
    import_tasks_conn(&mut conn, &json, SELF).unwrap();
    assert_eq!(
        people_avatar(&conn, "zhang"),
        Some("avatar-07".into()),
        "信封头像 ID 应落 people.avatar"
    );
}

#[test]
fn envelope_without_avatar_imports_as_null() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    // 旧版信封无 avatarId 字段 → serde default None → avatar 列 NULL，不报错
    let raw = r#"{"version":2,"profile":{"personId":"zhang","name":"张三"},"tasks":[]}"#;
    import_tasks_conn(&mut conn, raw, SELF).unwrap();
    assert_eq!(people_avatar(&conn, "zhang"), None);
}

#[test]
fn envelope_garbage_avatar_id_never_lands_in_people() {
    let _g = crate::db::lock_db_write();
    let mut conn = setup_conn();
    // 手改信封塞非法 ID（旧版拷贝文件名 / 穿越串）→ 白名单过滤，不落库
    let raw = r#"{"version":2,"profile":{"personId":"zhang","name":"张三","avatarId":"../../x"},"tasks":[]}"#;
    import_tasks_conn(&mut conn, raw, SELF).unwrap();
    assert_eq!(people_avatar(&conn, "zhang"), None, "非法 ID 不得落 people");
}

#[test]
fn export_filters_invalid_self_avatar_id() {
    let _g = crate::db::lock_db_write();
    let conn = setup_conn();
    // 防御：调用方传垃圾 ID（正常路径 profile 侧已过滤）→ 信封不携带
    let (json, _) = export_tasks_json(&conn, SELF, "我", Some("avatar-user.png")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        v["profile"]["avatarId"].is_null(),
        "非法 ID 不得进信封；实际 {v:?}"
    );
}

// ── 子任务卡（parent_id 出身列 + 子任务行 taskId 跳转标记） ──

/// 子任务卡出身列往返：upsert 写入 parent_id → load 读回一致；
/// 整行更新可清回 None；普通卡 None 不占位
#[test]
fn parent_id_roundtrips_through_upsert_and_load() {
    let _g = crate::db::lock_db_write(); // upsert_tasks 锁持有断言要求
    let conn = setup_conn();
    let mut child = min_task(None);
    child.id = "child".into();
    child.updated_at = Some(1000);
    child.parent_id = Some("mother".into());
    upsert_tasks(&conn, &[child]).unwrap();

    let loaded = load_all(&conn).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(
        loaded[0].parent_id.as_deref(),
        Some("mother"),
        "parent_id 应随 upsert 落库并读回"
    );

    // 整行更新（expected_updated_at 基线路径）清空 parent_id
    let mut cleared = load_task(&conn, "child").unwrap().unwrap();
    cleared.parent_id = None;
    cleared.expected_updated_at = cleared.updated_at;
    cleared.updated_at = Some(cleared.updated_at.unwrap() + 1);
    upsert_tasks(&conn, &[cleared]).unwrap();
    let reloaded = load_task(&conn, "child").unwrap().unwrap();
    assert_eq!(reloaded.parent_id, None, "整行更新应可清空 parent_id");
}

/// 子任务行 taskId（生成的任务卡指向）随 subtasks JSON 往返不丢；
/// 老格式（无 taskId 键）按 None 解析不报错
#[test]
fn subtask_task_id_survives_json_roundtrip() {
    let _g = crate::db::lock_db_write();
    let conn = setup_conn();
    let mut t = min_task(None);
    t.id = "mother".into();
    t.subtasks = Some(vec![
        Subtask {
            id: "s1".into(),
            text: "已生成卡".into(),
            done: false,
            task_id: Some("child-card".into()),
        },
        Subtask {
            id: "s2".into(),
            text: "未生成".into(),
            done: false,
            task_id: None,
        },
    ]);
    upsert_tasks(&conn, &[t]).unwrap();
    let loaded = load_task(&conn, "mother").unwrap().unwrap();
    let subs = loaded.subtasks.unwrap();
    assert_eq!(
        subs[0].task_id.as_deref(),
        Some("child-card"),
        "taskId 应随 subtasks JSON 往返保留"
    );
    assert_eq!(subs[1].task_id, None);

    // 老数据兼容：subtasks JSON 无 taskId 键 → 解析为 None（serde default）
    conn.execute(
        "UPDATE tasks SET subtasks = ?1 WHERE id = 'mother'",
        rusqlite::params![r#"[{"id":"s0","text":"老格式","done":true}]"#],
    )
    .unwrap();
    let legacy = load_task(&conn, "mother").unwrap().unwrap();
    let legacy_subs = legacy.subtasks.unwrap();
    assert_eq!(legacy_subs[0].task_id, None, "老格式应按 None 解析");
    assert_eq!(legacy_subs[0].text, "老格式");
}
